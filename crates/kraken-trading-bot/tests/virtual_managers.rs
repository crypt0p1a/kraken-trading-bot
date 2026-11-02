//! The virtual managers against a simulated Kraken: copies of the managers of the YAML file, added
//! by the bot when the account has money no manager uses

use kraken_api::trading::{OrderSide, OrderStatus};
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::{BotConfig, ManagerConfig, VirtualId, VirtualManagersConfig};
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;
use std::str::FromStr;

const BTC: &str = "XXBTZUSD";
const ETH: &str = "XETHZUSD";
const MIN: i32 = 1_000_000;

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn exchange(usd: &str) -> SimulatedExchange {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.add_pair(ETH, "ETHUSD", "XETH", "ZUSD", d("2500"));
    exchange.add_pair("XXBTZEUR", "XBTEUR", "XXBT", "ZEUR", d("45000"));
    exchange.set_fee_percent(d("0.25"));
    exchange.deposit("ZUSD", d(usd));
    exchange
}

const MANAGERS: &str = "
  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}
  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}";

fn config(virtuals: &str, managers: &str) -> BotConfig {
    BotConfig::parse(&format!("dry_run: false\n{virtuals}\nmanagers:{managers}")).unwrap()
}

async fn bot(
    exchange: SimulatedExchange,
    virtuals: &str,
    managers: &str,
) -> Bot<SimulatedExchange> {
    let mut bot = Bot::new(
        exchange,
        Storage::open_in_memory().unwrap(),
        config(virtuals, managers),
    )
    .with_seed(7);
    bot.prepare().await.unwrap();
    bot
}

fn virtuals(bot: &Bot<SimulatedExchange>) -> Vec<ManagerConfig> {
    bot.storage()
        .virtual_managers()
        .unwrap()
        .into_iter()
        .map(|it| it.config)
        .collect()
}

fn decode(id: i32) -> VirtualId {
    VirtualManagersConfig {
        min_virtual_id: MIN,
        max: 5,
    }
    .decode(id)
    .unwrap()
}

#[tokio::test]
async fn adds_no_manager_without_the_configuration() {
    let mut bot = bot(exchange("1000"), "", MANAGERS).await;
    for _ in 0..3 {
        bot.tick().await.unwrap();
    }
    assert!(bot.storage().virtual_managers().unwrap().is_empty());
}

#[tokio::test]
async fn copies_the_managers_while_the_margin_pays_for_them() {
    // 50 USD for the managers of the file, 50 for nobody
    let mut bot = bot(
        exchange("100"),
        "virtual_managers: {min_virtual_id: 1000000, max: 10}",
        MANAGERS,
    )
    .await;

    for _ in 0..6 {
        bot.tick().await.unwrap();
    }

    let added = virtuals(&bot);
    assert!(!added.is_empty());
    let mut budgets = d("50");
    for (index, manager) in added.iter().enumerate() {
        // the sequence in the order of creation, the manager it copies in the id
        let id = decode(manager.id);
        assert_eq!(id.sequence, index as i32);
        let template = bot
            .config()
            .managers
            .iter()
            .find(|it| it.id == id.template)
            .unwrap();
        assert_eq!(
            (
                &manager.pair,
                manager.budget,
                manager.buy_below,
                manager.sell_above
            ),
            (
                &template.pair,
                template.budget,
                template.buy_below,
                template.sell_above
            )
        );
        budgets += manager.budget;
    }
    // never more than the account has, and no budget left unused
    assert!(budgets <= d("100"), "{budgets}");
    assert!(d("100") - budgets < d("20"), "{budgets}");
}

#[tokio::test]
async fn a_virtual_manager_trades_from_the_tick_it_is_added() {
    let mut bot = bot(
        exchange("80"),
        "virtual_managers: {min_virtual_id: 1000000}",
        "\n  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
    )
    .await;

    let report = bot.tick().await.unwrap();
    let added = virtuals(&bot);
    assert_eq!(added.len(), 1);
    let id = added[0].id;
    assert_eq!(id, 1_000_001);
    assert!(
        matches!(
            report.outcomes[&id],
            Outcome::Placed {
                side: OrderSide::Buy,
                ..
            }
        ),
        "{:?}",
        report.outcomes
    );
    let orders: Vec<_> = bot
        .exchange()
        .orders()
        .into_iter()
        .filter(|it| it.userref == Some(id as i64) && it.status == OrderStatus::Open)
        .collect();
    assert_eq!(orders.len(), 1);

    // 80 - 30 - 30: no room for another one
    bot.tick().await.unwrap();
    assert_eq!(virtuals(&bot).len(), 1);
}

#[tokio::test]
async fn keeps_to_the_most_virtual_managers() {
    let mut bot = bot(
        exchange("10000"),
        "virtual_managers: {min_virtual_id: 1000000, max: 2}",
        MANAGERS,
    )
    .await;
    for _ in 0..5 {
        bot.tick().await.unwrap();
    }
    assert_eq!(virtuals(&bot).len(), 2);
}

#[tokio::test]
async fn copies_only_the_active_managers_of_the_quote_in_the_account() {
    let mut bot = bot(
        exchange("10000"),
        "virtual_managers: {min_virtual_id: 1000000, max: 5}",
        "
  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5, mode: closing}
  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}
  - {id: 3, pair: XBTEUR, budget: 20, buy_below: 2, sell_above: 3}",
    )
    .await;
    for _ in 0..5 {
        bot.tick().await.unwrap();
    }
    let added = virtuals(&bot);
    assert_eq!(added.len(), 5);
    // 1 is closing, the account has no EUR
    assert!(
        added.iter().all(|it| decode(it.id).template == 2),
        "{added:?}"
    );
}

#[tokio::test]
async fn the_gains_of_a_manager_are_not_a_margin() {
    let exchange = exchange("30");
    let mut bot = bot(
        exchange,
        "virtual_managers: {min_virtual_id: 1000000}",
        "\n  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
    )
    .await;

    // a whole cycle: bought at -1%, sold at +1.5%
    bot.tick().await.unwrap();
    bot.exchange().set_price(BTC, d("49000"));
    bot.tick().await.unwrap();
    bot.exchange().set_price(BTC, d("60000"));
    bot.tick().await.unwrap();
    bot.tick().await.unwrap();

    assert!(bot.exchange().balance("ZUSD") > d("30"));
    assert!(virtuals(&bot).is_empty());
}
