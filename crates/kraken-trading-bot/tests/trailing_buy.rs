//! The trailing of the buys against a simulated Kraken: a buy the price left behind is cancelled,
//! then placed again from the new price

use kraken_api::trading::{OrderSide, OrderStatus};
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;
use std::str::FromStr;

const BTC: &str = "XXBTZUSD";

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn config(dry_run: bool, settings: &str) -> BotConfig {
    BotConfig::parse(&format!(
        "dry_run: {dry_run}
{settings}
managers:
  - {{id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}}"
    ))
    .unwrap()
}

/// Manager 1 buys at -1% of 50000: 49500
async fn bot(settings: &str) -> Bot<SimulatedExchange> {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.set_fee_percent(d("0.25"));
    exchange.deposit("ZUSD", d("1000"));
    let mut bot = Bot::new(
        exchange,
        Storage::open_in_memory().unwrap(),
        config(false, settings),
    );
    bot.prepare().await.unwrap();
    bot
}

async fn tick(bot: &mut Bot<SimulatedExchange>) -> Outcome {
    bot.tick().await.unwrap().outcomes.remove(&1).unwrap()
}

fn buy_price(outcome: &Outcome) -> Decimal {
    match outcome {
        Outcome::Placed {
            side: OrderSide::Buy,
            price,
            ..
        } => *price,
        other => panic!("a buy expected, got {other:?}"),
    }
}

fn open_orders(bot: &Bot<SimulatedExchange>) -> usize {
    bot.exchange()
        .orders()
        .into_iter()
        .filter(|it| it.status == OrderStatus::Open)
        .count()
}

#[tokio::test]
async fn places_the_buy_again_once_the_price_left_it_behind() {
    let mut bot = bot("buy_trailing: 0.5").await;
    assert_eq!(buy_price(&tick(&mut bot).await), d("49500"));

    // +0.4% of 50000: it waits
    bot.exchange().set_price(BTC, d("50200"));
    assert_eq!(tick(&mut bot).await, Outcome::Waited);

    // +0.6%: cancelled
    bot.exchange().set_price(BTC, d("50300"));
    let outcome = tick(&mut bot).await;
    assert!(matches!(outcome, Outcome::Cancelled(_)), "{outcome:?}");
    assert_eq!(open_orders(&bot), 0);
    let action = &bot.storage().actions_of(1, 1).unwrap()[0];
    assert_eq!(action.kind, "buy_cancelled");

    // placed again at -1% of the new price
    assert_eq!(buy_price(&tick(&mut bot).await), d("49797"));
    assert_eq!(open_orders(&bot), 1);
    // the cancelled buy didn't change what the manager has
    let (balances, _, _) = bot.storage().manager_balances(1).unwrap().unwrap();
    assert_eq!(
        balances.quote_available + balances.quote_in_buy_order,
        d("100")
    );
}

#[tokio::test]
async fn waits_for_the_timeout_without_trailing() {
    let mut bot = bot("").await;
    tick(&mut bot).await;
    bot.exchange().set_price(BTC, d("60000"));
    for _ in 0..3 {
        assert_eq!(tick(&mut bot).await, Outcome::Waited);
    }
    assert_eq!(open_orders(&bot), 1);
}

#[tokio::test]
async fn keeps_a_buy_partially_filled() {
    let mut bot = bot("buy_trailing: 0.5").await;
    tick(&mut bot).await;
    let txid = bot.exchange().orders()[0].txid.clone();
    bot.exchange().fill(&txid, Some(d("0.001")));
    bot.tick().await.unwrap();

    bot.exchange().set_price(BTC, d("60000"));
    assert_eq!(tick(&mut bot).await, Outcome::Waited);
    assert_eq!(open_orders(&bot), 1);
}

#[tokio::test]
async fn never_touches_a_sell() {
    let mut bot = bot("buy_trailing: 0.5").await;
    tick(&mut bot).await;
    // bought, then selling
    bot.exchange().set_price(BTC, d("49000"));
    tick(&mut bot).await;

    bot.exchange().set_price(BTC, d("50200"));
    assert_eq!(tick(&mut bot).await, Outcome::Waited);
    assert_eq!(open_orders(&bot), 1);
}

#[tokio::test]
async fn a_dry_run_never_cancels_a_buy() {
    // a real buy placed by a live run, then the same database in a dry run
    let mut live = bot("buy_trailing: 0.5").await;
    tick(&mut live).await;
    let storage = Storage::open_in_memory().unwrap();
    storage
        .upsert_orders(&live.storage().orders_of(1).unwrap())
        .unwrap();
    let mut dry = Bot::new(
        live.into_exchange(),
        storage,
        config(true, "buy_trailing: 0.5"),
    );
    dry.prepare().await.unwrap();

    dry.exchange().set_price(BTC, d("60000"));
    for _ in 0..3 {
        assert_eq!(tick(&mut dry).await, Outcome::Waited);
    }
    assert_eq!(open_orders(&dry), 1);
}

#[tokio::test]
async fn a_cancellation_refused_is_a_failure_recorded_once() {
    let mut bot = bot("buy_trailing: 0.5").await;
    tick(&mut bot).await;
    bot.exchange()
        .fail_cancels(Some("EGeneral:Permission denied"));
    bot.exchange().set_price(BTC, d("60000"));

    for _ in 0..3 {
        let outcome = tick(&mut bot).await;
        assert!(
            matches!(&outcome, Outcome::Failed(error) if error.contains("Permission denied")),
            "{outcome:?}"
        );
    }
    let cancellations: Vec<_> = bot
        .storage()
        .actions_of(1, 10)
        .unwrap()
        .into_iter()
        .filter(|it| it.kind == "buy_cancelled")
        .collect();
    assert_eq!(cancellations.len(), 1);
    assert!(
        cancellations[0]
            .error
            .as_deref()
            .unwrap()
            .contains("Permission denied")
    );
    assert_eq!(open_orders(&bot), 1);

    // allowed again: cancelled, then placed again
    bot.exchange().fail_cancels(None);
    assert!(matches!(tick(&mut bot).await, Outcome::Cancelled(_)));
    assert_eq!(buy_price(&tick(&mut bot).await), d("59400"));
}

#[tokio::test]
async fn a_buy_gone_meanwhile_is_waited_for() {
    let mut bot = bot("buy_trailing: 0.5").await;
    tick(&mut bot).await;
    bot.exchange().fail_cancels(Some("EOrder:Unknown order"));
    bot.exchange().set_price(BTC, d("60000"));
    assert_eq!(tick(&mut bot).await, Outcome::Waited);
    assert!(
        bot.storage()
            .actions_of(1, 10)
            .unwrap()
            .iter()
            .all(|it| it.kind != "buy_cancelled")
    );
}
