//! The bot tick after tick against a simulated Kraken: synchronization, database, ledger and
//! strategy together, while the market moves

use kraken_spot_api::trading::{OrderSide, OrderStatus};
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

fn exchange(usd: &str) -> SimulatedExchange {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.add_pair("XETHZUSD", "ETHUSD", "XETH", "ZUSD", d("2500"));
    exchange.set_fee_percent(d("0.25"));
    exchange.deposit("ZUSD", d(usd));
    exchange
}

fn config(dry_run: bool, managers: &str) -> BotConfig {
    BotConfig::parse(&format!(
        "dry_run: {dry_run}\nbuy_order_timeout: 12h\nmanagers:\n{managers}"
    ))
    .unwrap()
}

const ONE: &str = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}";

async fn bot(exchange: SimulatedExchange, managers: &str) -> Bot<SimulatedExchange> {
    let mut bot = Bot::new(
        exchange,
        Storage::open_in_memory().unwrap(),
        config(false, managers),
    );
    bot.prepare().await.unwrap();
    bot
}

async fn tick(bot: &mut Bot<SimulatedExchange>, id: i32) -> Outcome {
    bot.tick().await.unwrap().outcomes.remove(&id).unwrap()
}

fn open_orders(bot: &Bot<SimulatedExchange>, userref: i64) -> Vec<kraken_spot_api::trading::Order> {
    bot.exchange()
        .orders()
        .into_iter()
        .filter(|it| it.userref == Some(userref) && it.status == OrderStatus::Open)
        .collect()
}

fn placed(outcome: &Outcome) -> (OrderSide, Decimal, Decimal, String) {
    match outcome {
        Outcome::Placed {
            side,
            amount,
            price,
            txid: Some(txid),
        } => (*side, *amount, *price, txid.clone()),
        other => panic!("an order expected, got {other:?}"),
    }
}

#[tokio::test]
async fn a_dry_run_never_places_an_order() {
    let mut bot = Bot::new(
        exchange("1000"),
        Storage::open_in_memory().unwrap(),
        config(true, ONE),
    );

    for _ in 0..3 {
        let outcome = tick(&mut bot, 1).await;
        assert!(
            matches!(
                outcome,
                Outcome::Placed {
                    side: OrderSide::Buy,
                    txid: None,
                    ..
                }
            ),
            "{outcome:?}"
        );
    }

    // validated by the exchange each time, never placed
    assert_eq!(bot.exchange().add_order_calls(), 3);
    assert!(bot.exchange().orders().is_empty());
    let actions = bot.storage().actions_of(1, 10).unwrap();
    assert_eq!(actions.len(), 3);
    assert!(
        actions
            .iter()
            .all(|it| it.dry_run && it.txid.is_none() && it.kind == "buy")
    );
}

#[tokio::test]
async fn a_full_cycle_ends_with_more_than_the_budget() {
    let mut bot = bot(exchange("1000"), ONE).await;

    // buys at -1%, with everything it has, fees included
    let (side, amount, price, txid) = placed(&tick(&mut bot, 1).await);
    assert_eq!((side, price), (OrderSide::Buy, d("49500")));
    // 100 / (49500 * 1.0025)
    assert_eq!(amount, d("0.00201516"));
    let order = bot
        .exchange()
        .orders()
        .into_iter()
        .find(|it| it.txid == txid)
        .unwrap();
    assert_eq!(order.userref, Some(1));
    assert_eq!(order.oflags, "fciq");
    assert_eq!(
        order.expire_time,
        Some(bot.exchange().now() + 12.0 * 3600.0)
    );

    // waits while its order is open
    assert_eq!(tick(&mut bot, 1).await, Outcome::Waited);

    // the market goes down: bought
    bot.exchange().set_price(BTC, d("49400"));
    let (side, amount, sell_price, _) = placed(&tick(&mut bot, 1).await);
    assert_eq!((side, amount), (OrderSide::Sell, d("0.00201516")));
    // the cost + 1.5%, fees of the sale included
    assert!(sell_price > d("49500") * d("1.015"), "{sell_price}");
    let (balances, _, _) = bot.storage().manager_balances(1).unwrap().unwrap();
    assert_eq!(balances.base_available, d("0.00201516"));
    let cost = balances.position_cost;

    // the market goes up: sold
    bot.exchange().set_price(BTC, sell_price);
    let (side, amount, _, _) = placed(&tick(&mut bot, 1).await);
    assert_eq!(side, OrderSide::Buy);
    let (balances, _, _) = bot.storage().manager_balances(1).unwrap().unwrap();
    assert!(balances.base_available.is_zero());
    // at least the budget + 1.5% of the cost, a few hundred-millionths of rounding aside
    let after = balances.quote_available + balances.quote_in_buy_order;
    assert!(
        after >= d("100") + cost * d("0.015") - d("0.00000002"),
        "{after}"
    );
    // and it buys again with all of it
    assert!(amount > d("0.00201516"), "{amount}");
}

#[tokio::test]
async fn a_buy_order_not_filled_expires_and_is_placed_again_from_the_new_price() {
    let mut bot = bot(exchange("1000"), ONE).await;
    let (_, _, first_price, first) = placed(&tick(&mut bot, 1).await);
    assert_eq!(first_price, d("49500"));

    bot.exchange().set_price(BTC, d("60000"));
    assert_eq!(tick(&mut bot, 1).await, Outcome::Waited);

    bot.exchange().advance(12.0 * 3600.0 + 1.0);
    let (side, _, price, second) = placed(&tick(&mut bot, 1).await);
    assert_eq!((side, price), (OrderSide::Buy, d("59400")));
    assert_ne!(first, second);

    let expired = bot
        .exchange()
        .orders()
        .into_iter()
        .find(|it| it.txid == first)
        .unwrap();
    assert_eq!(expired.status, OrderStatus::Expired);
    let stored = bot.storage().orders_of(1).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].status, OrderStatus::Expired);
}

#[tokio::test]
async fn what_a_buy_partially_filled_bought_is_sold() {
    let mut bot = bot(exchange("1000"), ONE).await;
    let (_, _, _, txid) = placed(&tick(&mut bot, 1).await);

    bot.exchange().fill(&txid, Some(d("0.001")));
    assert_eq!(tick(&mut bot, 1).await, Outcome::Waited);

    bot.exchange().advance(12.0 * 3600.0 + 1.0);
    let (side, amount, _, _) = placed(&tick(&mut bot, 1).await);
    assert_eq!((side, amount), (OrderSide::Sell, d("0.001")));
}

#[tokio::test]
async fn a_closing_manager_sells_what_it_holds_then_stops() {
    let exchange = exchange("1000");
    let mut buying = bot(exchange, ONE).await;
    let (_, _, _, _) = placed(&tick(&mut buying, 1).await);
    buying.exchange().set_price(BTC, d("49400"));

    // the manager is now closing: same id, so same orders
    let closing = ONE.replace('}', ", mode: closing}");
    let exchange = SimulatedExchangeHandle::take(buying);
    let mut bot = Bot::new(
        exchange,
        Storage::open_in_memory().unwrap(),
        config(false, &closing),
    );
    bot.prepare().await.unwrap();

    let (side, _, price, _) = placed(&tick(&mut bot, 1).await);
    assert_eq!(side, OrderSide::Sell);

    bot.exchange().set_price(BTC, price);
    let outcome = tick(&mut bot, 1).await;
    assert!(
        matches!(&outcome, Outcome::Idle(reason) if reason.contains("closing")),
        "{outcome:?}"
    );
    assert!(open_orders(&bot, 1).is_empty());
    // the idle reason is recorded once, not at every tick
    tick(&mut bot, 1).await;
    let idle = bot
        .storage()
        .actions_of(1, 10)
        .unwrap()
        .into_iter()
        .filter(|it| it.kind == "idle")
        .count();
    assert_eq!(idle, 1);
}

/// Moves the exchange out of a bot, to give it to another one
struct SimulatedExchangeHandle;

impl SimulatedExchangeHandle {
    fn take(bot: Bot<SimulatedExchange>) -> SimulatedExchange {
        bot.into_exchange()
    }
}

#[tokio::test]
async fn two_managers_of_the_same_pair_spend_their_own_budget() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}
  - {id: 2, pair: XBT/USD, budget: 50, buy_below: 2, sell_above: 3}";
    let mut bot = bot(exchange("1000"), managers).await;
    let report = bot.tick().await.unwrap();

    let (_, amount1, price1, _) = placed(&report.outcomes[&1]);
    let (_, amount2, price2, _) = placed(&report.outcomes[&2]);
    assert_eq!((price1, price2), (d("49500"), d("49000")));
    assert!(amount1 * price1 * d("1.0025") <= d("100"));
    assert!(amount2 * price2 * d("1.0025") <= d("50"));
    assert_eq!(open_orders(&bot, 1).len(), 1);
    assert_eq!(open_orders(&bot, 2).len(), 1);

    // the fill of one doesn't change the other
    bot.exchange().set_price(BTC, d("49400"));
    let report = bot.tick().await.unwrap();
    assert!(matches!(
        report.outcomes[&1],
        Outcome::Placed {
            side: OrderSide::Sell,
            ..
        }
    ));
    assert_eq!(report.outcomes[&2], Outcome::Waited);
}

#[tokio::test]
async fn a_manager_waits_for_the_account_to_have_its_money() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 50, buy_below: 1, sell_above: 1.5}
  - {id: 2, pair: XBTUSD, budget: 50, buy_below: 1, sell_above: 1.5}";
    let mut bot = bot(exchange("60"), managers).await;
    let report = bot.tick().await.unwrap();

    assert!(matches!(report.outcomes[&1], Outcome::Placed { .. }));
    assert!(
        matches!(&report.outcomes[&2], Outcome::Idle(reason) if reason.contains("the account has"))
    );
    assert!(open_orders(&bot, 2).is_empty());
}

#[tokio::test]
async fn the_orders_of_no_manager_are_reported_and_left_untouched() {
    let exchange = exchange("1000");
    let orphan =
        exchange.add_foreign_order(Some(99), "XBTUSD", OrderSide::Buy, d("0.001"), d("40000"));
    exchange.add_foreign_order(None, "XBTUSD", OrderSide::Buy, d("0.001"), d("40000"));
    exchange.add_foreign_order(Some(0), "XBTUSD", OrderSide::Buy, d("0.001"), d("40000"));
    let mut bot = bot(exchange, ONE).await;

    let report = bot.tick().await.unwrap();
    assert_eq!(report.sync.orphans.into_iter().collect::<Vec<_>>(), [99]);
    let still = bot
        .exchange()
        .orders()
        .into_iter()
        .find(|it| it.txid == orphan)
        .unwrap();
    assert_eq!(still.status, OrderStatus::Open);
    // not counted in the money of the manager
    assert!(matches!(
        report.outcomes[&1],
        Outcome::Placed {
            side: OrderSide::Buy,
            ..
        }
    ));
}

#[tokio::test]
async fn the_history_of_a_manager_is_found_at_the_first_start() {
    // the manager traded before the database existed
    let exchange = exchange("1000");
    let txid =
        exchange.add_foreign_order(Some(1), "XBTUSD", OrderSide::Buy, d("0.002"), d("49500"));
    exchange.deposit("ZUSD", d("-99.24750000"));
    exchange.fill(&txid, None);
    exchange.advance(3600.0);

    let mut bot = bot(exchange, ONE).await;
    let (side, amount, _, _) = placed(&tick(&mut bot, 1).await);
    assert_eq!((side, amount), (OrderSide::Sell, d("0.002")));
    assert_eq!(bot.storage().orders_of(1).unwrap().len(), 2);
}

#[tokio::test]
async fn a_bot_restarted_without_its_database_doesnt_place_twice() {
    let mut first = bot(exchange("1000"), ONE).await;
    placed(&tick(&mut first, 1).await);

    // Kraken knows the open order: the new bot waits for it
    let mut second = Bot::new(
        first.into_exchange(),
        Storage::open_in_memory().unwrap(),
        config(false, ONE),
    );
    second.prepare().await.unwrap();
    assert_eq!(tick(&mut second, 1).await, Outcome::Waited);
    assert_eq!(open_orders(&second, 1).len(), 1);
}

#[tokio::test]
async fn an_order_refused_by_kraken_is_recorded_and_the_others_go_on() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}
  - {id: 2, pair: ETHUSD, budget: 100, buy_below: 1, sell_above: 1.5}";
    let mut bot = bot(exchange("1000"), managers).await;
    bot.tick().await.unwrap();
    bot.exchange().set_price(BTC, d("49400"));
    // the bitcoins bought vanish from the account
    bot.exchange().deposit("XXBT", d("-0.00201516"));

    let report = bot.tick().await.unwrap();
    assert!(
        matches!(&report.outcomes[&1], Outcome::Failed(error) if error.contains("Insufficient funds"))
    );
    assert_eq!(report.outcomes[&2], Outcome::Waited);
    let last = &bot.storage().actions_of(1, 1).unwrap()[0];
    assert_eq!(last.kind, "sell");
    assert!(
        last.error
            .as_deref()
            .is_some_and(|it| it.contains("Insufficient funds"))
    );
}

#[tokio::test]
async fn an_unknown_pair_is_refused_before_trading() {
    let mut bot = Bot::new(
        exchange("1000"),
        Storage::open_in_memory().unwrap(),
        config(
            false,
            "  - {id: 1, pair: NOPEUSD, budget: 100, buy_below: 1, sell_above: 1.5}",
        ),
    );
    let error = bot.prepare().await.unwrap_err();
    assert!(
        error.to_string().contains("unknown pair NOPEUSD"),
        "{error}"
    );
}

#[tokio::test]
async fn every_tick_records_the_prices_and_the_state_of_the_managers() {
    let mut bot = bot(exchange("1000"), ONE).await;
    for _ in 0..4 {
        bot.tick().await.unwrap();
    }
    assert_eq!(bot.storage().snapshot_count(1).unwrap(), 4);
    assert!(bot.storage().ticker_count(BTC).unwrap() >= 1);
}

/// Deterministic random numbers (xorshift64*)
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// A factor in [1 - spread, 1 + spread[
    fn factor(&mut self, spread: f64) -> Decimal {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        Decimal::from_f64_retain(1.0 - spread + unit * 2.0 * spread)
            .unwrap()
            .round_dp(4)
    }
}

#[tokio::test]
async fn trading_for_days_on_a_moving_market_only_makes_money() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 0.5, sell_above: 0.8}
  - {id: 2, pair: XBTUSD, budget: 60, buy_below: 1.5, sell_above: 2}";
    let mut bot = bot(exchange("1000"), managers).await;
    let mut random = Random(7);
    let mut price = d("50000");
    let mut sales = 0;

    for _ in 0..600 {
        let report = bot.tick().await.unwrap();
        for outcome in report.outcomes.values() {
            assert!(!matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
            if matches!(
                outcome,
                Outcome::Placed {
                    side: OrderSide::Sell,
                    ..
                }
            ) {
                sales += 1;
            }
        }
        price = (price * random.factor(0.012)).round_dp(1);
        bot.exchange().set_price(BTC, price);
        bot.exchange().advance(300.0);
    }

    assert!(sales >= 5, "only {sales} sales");

    for (id, budget) in [(1, d("100")), (2, d("60"))] {
        // each sale brought back more than what was bought since the previous one
        let orders = bot.storage().orders_of(id).unwrap();
        let mut spent = Decimal::ZERO;
        let mut cycles = 0;
        for order in orders
            .iter()
            .filter(|it| it.volume_executed > Decimal::ZERO)
        {
            match order.side {
                OrderSide::Buy => spent += order.cost + order.fee,
                OrderSide::Sell if order.volume_executed == order.volume => {
                    let received = order.cost - order.fee;
                    assert!(received > spent, "{id}: sold {received} for {spent}");
                    spent = Decimal::ZERO;
                    cycles += 1;
                }
                OrderSide::Sell => {}
            }
        }
        assert!(cycles >= 2, "{id}: {cycles} cycles");

        // and what is not in a position is more than the budget
        let (balances, _, _) = bot.storage().manager_balances(id).unwrap().unwrap();
        if balances.base_available.is_zero() && balances.base_in_sell_order.is_zero() {
            assert!(
                balances.quote_available + balances.quote_in_buy_order > budget,
                "{id}: {balances:?}"
            );
        }
    }
    // only limit orders, at a price
    for order in bot.exchange().orders() {
        assert_eq!(
            order.description.order_type,
            kraken_spot_api::trading::OrderType::Limit,
            "{order:?}"
        );
        assert!(
            order.description.price.is_some_and(|it| it > Decimal::ZERO),
            "{order:?}"
        );
        assert_eq!(order.oflags, "fciq");
    }

    // the account never went below zero
    assert!(bot.exchange().balance("ZUSD") >= Decimal::ZERO);
    assert!(bot.exchange().balance("XXBT") >= Decimal::ZERO);
}
