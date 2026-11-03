//! The virtual managers against a simulated Kraken: copies of the managers of the YAML file, added
//! by the bot when the account has money no manager uses

use kraken_api::trading::{OrderSide, OrderStatus};
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::{BotConfig, ManagerConfig, VirtualId};
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;
use std::str::FromStr;

const BTC: &str = "XXBTZUSD";
const ETH: &str = "XETHZUSD";

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
    VirtualId::decode(id).unwrap()
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
    let mut bot = bot(exchange("100"), "virtual_managers: true", MANAGERS).await;

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
        "virtual_managers: true",
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
async fn copies_only_the_active_managers_of_the_quote_in_the_account() {
    let mut bot = bot(
        exchange("10000"),
        "virtual_managers: true",
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
        "virtual_managers: true",
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

const ONE: &str = "\n  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}";
const VIRTUAL: i32 = 1_000_001;

/// Manager 1 and a copy of it, VIRTUAL, both waiting for their buy order
async fn with_a_virtual_manager() -> Bot<SimulatedExchange> {
    let mut bot = bot(exchange("80"), "virtual_managers: true", ONE).await;
    bot.tick().await.unwrap();
    assert_eq!(
        virtuals(&bot).iter().map(|it| it.id).collect::<Vec<_>>(),
        vec![VIRTUAL]
    );
    bot
}

/// Both buy orders expire unfilled, then `usd` leaves the account
fn expire_and_withdraw(bot: &Bot<SimulatedExchange>, usd: &str) {
    bot.exchange().advance(13.0 * 3600.0);
    bot.exchange().deposit("ZUSD", -d(usd));
}

async fn outcome(bot: &mut Bot<SimulatedExchange>, id: i32) -> Outcome {
    bot.tick().await.unwrap().outcomes.remove(&id).unwrap()
}

#[tokio::test]
async fn deletes_a_virtual_manager_the_account_cannot_pay_for_anymore() {
    let mut bot = with_a_virtual_manager().await;
    // 30 USD left: manager 1 buys with them, its copy has nothing to buy with
    expire_and_withdraw(&bot, "50");

    for round in 1..3 {
        let outcome = outcome(&mut bot, VIRTUAL).await;
        assert!(
            matches!(&outcome, Outcome::Idle(reason) if reason.ends_with(&format!("({round}/3)"))),
            "{outcome:?}"
        );
    }
    let outcome = outcome(&mut bot, VIRTUAL).await;
    assert!(matches!(outcome, Outcome::Deleted(_)), "{outcome:?}");

    assert!(virtuals(&bot).is_empty());
    assert_eq!(bot.storage().last_virtual_id().unwrap(), Some(VIRTUAL));
    assert_eq!(bot.storage().manager_balances(VIRTUAL).unwrap(), None);
    let actions = bot.storage().actions_of(VIRTUAL, 1).unwrap();
    assert_eq!(actions[0].kind, "virtual_deleted");

    // gone, and not added again: no money is left for anybody
    for _ in 0..3 {
        let report = bot.tick().await.unwrap();
        assert!(!report.outcomes.contains_key(&VIRTUAL));
    }
    assert!(virtuals(&bot).is_empty());
}

#[tokio::test]
async fn the_ticks_without_money_must_be_in_a_row() {
    let mut bot = with_a_virtual_manager().await;
    expire_and_withdraw(&bot, "50");
    outcome(&mut bot, VIRTUAL).await;
    outcome(&mut bot, VIRTUAL).await;

    // the money is back: it buys, and starts counting again
    bot.exchange().deposit("ZUSD", d("50"));
    let bought = outcome(&mut bot, VIRTUAL).await;
    assert!(matches!(bought, Outcome::Placed { .. }), "{bought:?}");
    expire_and_withdraw(&bot, "50");

    for _ in 0..2 {
        let outcome = outcome(&mut bot, VIRTUAL).await;
        assert!(matches!(outcome, Outcome::Idle(_)), "{outcome:?}");
    }
    let outcome = outcome(&mut bot, VIRTUAL).await;
    assert!(matches!(outcome, Outcome::Deleted(_)), "{outcome:?}");
}

#[tokio::test]
async fn keeps_a_virtual_manager_which_holds_a_position() {
    let mut bot = with_a_virtual_manager().await;
    // both bought, both selling: then the rest of the account leaves
    bot.exchange().set_price(BTC, d("49000"));
    bot.tick().await.unwrap();
    let rest = bot.exchange().balance("ZUSD");
    bot.exchange().deposit("ZUSD", -rest);

    for _ in 0..5 {
        let outcome = outcome(&mut bot, VIRTUAL).await;
        assert_eq!(outcome, Outcome::Waited);
    }
    assert_eq!(virtuals(&bot).len(), 1);
}

#[tokio::test]
async fn never_deletes_a_manager_of_the_file() {
    let mut bot = bot(exchange("10"), "virtual_managers: true", ONE).await;
    for _ in 0..5 {
        let outcome = outcome(&mut bot, 1).await;
        assert!(
            matches!(&outcome, Outcome::Idle(reason) if !reason.contains("/3")),
            "{outcome:?}"
        );
    }
}

/// The same Kraken, a new database
async fn restart(bot: Bot<SimulatedExchange>, managers: &str) -> Bot<SimulatedExchange> {
    let mut bot = Bot::new(
        bot.into_exchange(),
        Storage::open_in_memory().unwrap(),
        config("virtual_managers: true", managers),
    )
    .with_seed(7);
    bot.prepare().await.unwrap();
    bot
}

/// VIRTUAL bought at -1% of 50000, and is selling at +1.5%
async fn selling_virtual_manager() -> Bot<SimulatedExchange> {
    let mut bot = with_a_virtual_manager().await;
    bot.exchange().set_price(BTC, d("49000"));
    let report = bot.tick().await.unwrap();
    assert!(
        matches!(
            report.outcomes[&VIRTUAL],
            Outcome::Placed {
                side: OrderSide::Sell,
                ..
            }
        ),
        "{report:?}"
    );
    bot
}

fn rebuilt(bot: &Bot<SimulatedExchange>) -> ManagerConfig {
    let found = virtuals(bot);
    assert_eq!(found.len(), 1, "{found:?}");
    found[0].clone()
}

#[tokio::test]
async fn rebuilds_a_virtual_manager_lost_with_the_database() {
    let bot = selling_virtual_manager().await;
    let before = rebuilt(&bot);

    let mut bot = restart(bot, ONE).await;
    let report = bot.tick().await.unwrap();

    assert_eq!(rebuilt(&bot), before);
    // its sell found again, waited for
    assert_eq!(report.outcomes[&VIRTUAL], Outcome::Waited);
    assert_eq!(bot.storage().orders_of(VIRTUAL).unwrap().len(), 2);
    let actions = bot.storage().actions_of(VIRTUAL, 1).unwrap();
    assert_eq!(actions[0].kind, "virtual_rebuilt");
}

#[tokio::test]
async fn the_orders_win_over_a_copied_manager_which_changed() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(
        bot,
        "\n  - {id: 1, pair: XBTUSD, budget: 50, buy_below: 2, sell_above: 3}",
    )
    .await;
    bot.tick().await.unwrap();

    let manager = rebuilt(&bot);
    assert_eq!(
        // what the first buy spent: its volume was rounded down
        (
            manager.budget.round_dp(2),
            manager.buy_below,
            manager.sell_above
        ),
        (d("30"), d("1"), d("1.5"))
    );
    assert_eq!(
        manager.mode,
        kraken_trading_bot::config::ManagerMode::Active
    );
}

#[tokio::test]
async fn a_virtual_manager_whose_copied_manager_is_gone_closes() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(
        bot,
        "\n  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}",
    )
    .await;
    bot.tick().await.unwrap();

    let manager = bot
        .storage()
        .virtual_managers()
        .unwrap()
        .into_iter()
        .find(|it| it.config.id == VIRTUAL)
        .unwrap()
        .config;
    assert_eq!(
        manager.mode,
        kraken_trading_bot::config::ManagerMode::Closing
    );
    assert_eq!(
        (
            manager.pair.as_str(),
            manager.budget.round_dp(2),
            manager.sell_above
        ),
        ("XBTUSD", d("30"), d("1.5"))
    );
}

#[tokio::test]
async fn a_lost_virtual_manager_which_held_nothing_keeps_its_id() {
    let bot = with_a_virtual_manager().await;
    // its buy expires unfilled: it held nothing
    bot.exchange().advance(13.0 * 3600.0);

    let mut bot = restart(bot, ONE).await;
    bot.tick().await.unwrap();

    let ids = bot.storage().virtual_ids().unwrap();
    // rebuilt deleted, then a new one added after it
    assert_eq!(ids, vec![VIRTUAL, 1_001_001]);
    assert_eq!(
        virtuals(&bot).iter().map(|it| it.id).collect::<Vec<_>>(),
        vec![1_001_001]
    );
}

#[tokio::test]
async fn searches_the_orders_of_kraken_once_per_database() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(bot, ONE).await;
    bot.tick().await.unwrap();
    bot.tick().await.unwrap();
    assert_eq!(bot.storage().virtual_ids().unwrap(), vec![VIRTUAL]);
}

#[tokio::test]
async fn a_closing_virtual_manager_is_deleted_once_it_sold_everything() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(
        bot,
        "\n  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}",
    )
    .await;
    // closing: it waits for its sell
    assert_eq!(outcome(&mut bot, VIRTUAL).await, Outcome::Waited);

    // sold: it doesn't buy again, and is gone
    let sell_price = bot
        .storage()
        .orders_of(VIRTUAL)
        .unwrap()
        .into_iter()
        .find(|it| it.side == OrderSide::Sell)
        .unwrap()
        .limit_price
        .unwrap();
    bot.exchange().set_price(BTC, sell_price);
    let outcome = outcome(&mut bot, VIRTUAL).await;
    assert!(matches!(outcome, Outcome::Deleted(_)), "{outcome:?}");
    assert!(
        bot.storage()
            .virtual_managers()
            .unwrap()
            .iter()
            .all(|it| it.config.id != VIRTUAL)
    );
}

#[tokio::test]
async fn without_a_sell_nor_its_copied_manager_it_sells_at_the_fallback() {
    // its buy open, its copied manager gone: nothing but the buy says what it was about
    let bot = with_a_virtual_manager().await;
    let mut bot = Bot::new(
        bot.into_exchange(),
        Storage::open_in_memory().unwrap(),
        config(
            "virtual_managers: true",
            "\n  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}",
        ),
    )
    .with_seed(7);
    bot.prepare().await.unwrap();

    // closing, the budget of its buy: it lets the buy end
    assert_eq!(outcome(&mut bot, VIRTUAL).await, Outcome::Waited);
    let manager = bot
        .storage()
        .virtual_managers()
        .unwrap()
        .into_iter()
        .find(|it| it.config.id == VIRTUAL)
        .unwrap()
        .config;
    assert_eq!(
        manager.mode,
        kraken_trading_bot::config::ManagerMode::Closing
    );
    assert_eq!(manager.sell_above, d("1.5"));
    assert_eq!(manager.budget.round_dp(1), d("30"));

    // filled: sold for what it cost + 2%, fees deducted
    bot.exchange().set_price(BTC, d("49000"));
    let (amount, price) = match outcome(&mut bot, VIRTUAL).await {
        Outcome::Placed {
            side: OrderSide::Sell,
            amount,
            price,
            ..
        } => (amount, price),
        other => panic!("a sell expected, got {other:?}"),
    };
    let cost = bot
        .storage()
        .orders_of(VIRTUAL)
        .unwrap()
        .iter()
        .filter(|it| it.side == OrderSide::Buy)
        .map(|it| it.cost + it.fee)
        .sum::<Decimal>();
    let brought = amount * price * (Decimal::ONE - d("0.0025"));
    assert!(brought >= cost * d("1.015"), "{brought} for {cost}");
    assert!(brought < cost * d("1.016"), "{brought} for {cost}");
}

/// The same Kraken and database, another configuration
async fn reconfigure(bot: Bot<SimulatedExchange>, managers: &str) -> Bot<SimulatedExchange> {
    let (exchange, storage) = bot.into_parts();
    let mut bot = Bot::new(
        exchange,
        storage,
        config("virtual_managers: true", managers),
    )
    .with_seed(7);
    bot.prepare().await.unwrap();
    bot
}

#[tokio::test]
async fn a_virtual_manager_closes_with_the_manager_it_copies() {
    let bot = with_a_virtual_manager().await;
    let mut bot = reconfigure(
        bot,
        "\n  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5, mode: closing}",
    )
    .await;
    // its buy expires: closing, it doesn't buy again and is deleted
    bot.exchange().advance(13.0 * 3600.0);
    let outcome = outcome(&mut bot, VIRTUAL).await;
    assert!(matches!(outcome, Outcome::Deleted(_)), "{outcome:?}");
    // the manager of the file stays, closing
    assert!(matches!(outcome_of_one(&mut bot).await, Outcome::Idle(_)));
    assert!(virtuals(&bot).is_empty());
}

async fn outcome_of_one(bot: &mut Bot<SimulatedExchange>) -> Outcome {
    bot.tick().await.unwrap().outcomes.remove(&1).unwrap()
}

#[tokio::test]
async fn a_virtual_manager_closes_without_the_manager_it_copies() {
    let bot = selling_virtual_manager().await;
    let mut bot = reconfigure(
        bot,
        "\n  - {id: 2, pair: ETHUSD, budget: 20, buy_below: 2, sell_above: 3}",
    )
    .await;
    let manager = bot
        .managers()
        .unwrap()
        .into_iter()
        .find(|it| it.id == VIRTUAL)
        .unwrap();
    assert_eq!(
        manager.mode,
        kraken_trading_bot::config::ManagerMode::Closing
    );
    // it still sells what it holds
    assert_eq!(outcome(&mut bot, VIRTUAL).await, Outcome::Waited);
}

#[tokio::test]
async fn a_copy_of_a_closing_manager_is_rebuilt_closing() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(
        bot,
        "\n  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5, mode: closing}",
    )
    .await;
    bot.tick().await.unwrap();
    assert_eq!(
        rebuilt(&bot).mode,
        kraken_trading_bot::config::ManagerMode::Closing
    );
}

#[tokio::test]
async fn a_rebuild_failing_doesnt_stop_the_managers_and_is_tried_again() {
    let bot = selling_virtual_manager().await;
    let mut bot = restart(bot, ONE).await;
    bot.exchange().fail_trades(Some("EService:Unavailable"));

    // the managers of the file trade, the virtual one waits for its rebuild
    let report = bot.tick().await.unwrap();
    assert!(report.outcomes.contains_key(&1));
    assert!(!report.outcomes.contains_key(&VIRTUAL));
    assert!(bot.storage().virtual_ids().unwrap().is_empty());

    // not tried again within the hour, even once Kraken answers
    bot.exchange().fail_trades(None);
    bot.tick().await.unwrap();
    assert!(bot.storage().virtual_ids().unwrap().is_empty());

    // an hour later: rebuilt
    bot.exchange().advance(3601.0);
    bot.tick().await.unwrap();
    assert_eq!(bot.storage().virtual_ids().unwrap(), vec![VIRTUAL]);
}

#[tokio::test]
async fn the_ids_used_up_stop_only_the_virtual_managers() {
    // the last id taken: the next sequence doesn't fit in a userref
    let sequence = (i32::MAX - kraken_trading_bot::config::MIN_VIRTUAL_ID) / 1000;
    let last = VirtualId {
        sequence,
        template: 1,
    }
    .encode()
    .unwrap();
    assert_eq!(
        VirtualId {
            sequence: sequence + 1,
            template: 1
        }
        .encode(),
        None
    );
    let mut bot = bot(exchange("1000"), "virtual_managers: true", ONE).await;
    bot.storage()
        .add_virtual_manager(&kraken_trading_bot::storage::VirtualManager {
            config: ManagerConfig {
                id: last,
                pair: "XBTUSD".into(),
                budget: d("30"),
                buy_below: d("1"),
                sell_above: d("1.5"),
                mode: kraken_trading_bot::config::ManagerMode::Active,
            },
            template_id: 1,
            created_at: 0,
            reason: "the last one".into(),
        })
        .unwrap();
    bot.storage()
        .delete_virtual_manager(last, 0, "held nothing")
        .unwrap();

    // money for many: none added, manager 1 trades
    let report = bot.tick().await.unwrap();
    assert!(
        matches!(report.outcomes[&1], Outcome::Placed { .. }),
        "{report:?}"
    );
    assert!(virtuals(&bot).is_empty());
}
