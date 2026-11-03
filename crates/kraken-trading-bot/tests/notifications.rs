//! The notifications of the bot, tick after tick on a simulated Kraken

use kraken_api::trading::OrderSide;
use kraken_trading_bot::bot::Bot;
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;
use std::str::FromStr;
use trading_notify::{Notifier, Recorded};

const BTC: &str = "XXBTZUSD";
/// 2027-01-15 08:00 UTC
const START: f64 = 1_800_000_000.0;

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn exchange(usd: &str) -> SimulatedExchange {
    let exchange = SimulatedExchange::new(START);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.set_fee_percent(d("0.25"));
    exchange.deposit("ZUSD", d(usd));
    exchange
}

const ONE: &str = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}";

async fn bot(
    exchange: SimulatedExchange,
    dry_run: bool,
    notifications: &str,
    managers: &str,
) -> (Bot<SimulatedExchange>, Recorded) {
    let config = BotConfig::parse(&format!(
        "dry_run: {dry_run}\nnotifications:\n  ntfy: {{}}\n{notifications}\nmanagers:\n{managers}"
    ))
    .unwrap();
    let (notifier, recorded) = Notifier::recording();
    let mut bot =
        Bot::new(exchange, Storage::open_in_memory().unwrap(), config).with_notifier(notifier);
    bot.prepare().await.unwrap();
    (bot, recorded)
}

/// The titles notified since the last call
fn titles(recorded: &Recorded) -> Vec<String> {
    recorded
        .lock()
        .unwrap()
        .drain(..)
        .map(|it| it.title)
        .collect()
}

fn messages(recorded: &Recorded) -> Vec<(String, String)> {
    recorded
        .lock()
        .unwrap()
        .drain(..)
        .map(|it| (it.title, it.message))
        .collect()
}

#[tokio::test]
async fn a_full_cycle_is_notified() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "", ONE).await;

    bot.notify_started().await;
    assert_eq!(titles(&recorded), ["Bot started"]);

    bot.tick().await.unwrap();
    assert_eq!(titles(&recorded), ["Buy placed · #1 XBTUSD"]);

    // nothing new: nothing notified
    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());

    bot.exchange().set_price(BTC, d("49400"));
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified[0].0, "Buy filled · #1 XBTUSD");
    assert_eq!(
        notified[0].1,
        "0.00201516 XBT at 49500 USD · cost 100 USD fees included"
    );
    assert_eq!(notified[1].0, "Sell placed · #1 XBTUSD");
    assert_eq!(notified.len(), 2);

    let sell_price = bot.storage().actions_of(1, 1).unwrap()[0].price.unwrap();
    bot.exchange().set_price(BTC, sell_price);
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified[0].0, "Sell filled · #1 XBTUSD");
    // the gain of the cycle, after every fee: +1.5% of the cost
    assert!(
        notified[0].1.contains("\ngain +1.5 USD (+1.5"),
        "{}",
        notified[0].1
    );
    assert_eq!(notified[1].0, "Buy placed · #1 XBTUSD");
}

#[tokio::test]
async fn a_dry_run_notifies_its_orders_only_when_asked() {
    let (mut quiet, recorded) = bot(exchange("1000"), true, "", ONE).await;
    quiet.tick().await.unwrap();
    quiet.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());

    let (mut verbose, recorded) = bot(exchange("1000"), true, "  dry_run: true", ONE).await;
    verbose.tick().await.unwrap();
    assert_eq!(titles(&recorded), ["[dry run] Buy placed · #1 XBTUSD"]);
}

#[tokio::test]
async fn an_expired_buy_is_notified() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "", ONE).await;
    bot.tick().await.unwrap();
    titles(&recorded);

    bot.exchange().set_price(BTC, d("60000"));
    bot.exchange().advance(12.0 * 3600.0 + 1.0);
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified[0].0, "Buy expired · #1 XBTUSD");
    assert_eq!(
        notified[0].1,
        "not filled: a new buy will be placed from the market price"
    );
    // and the new buy, from the new price
    assert_eq!(notified[1].0, "Buy placed · #1 XBTUSD");
    assert!(notified[1].1.contains("at 59400 USD"), "{}", notified[1].1);
}

#[tokio::test]
async fn a_partial_fill_is_notified_once() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "", ONE).await;
    bot.tick().await.unwrap();
    titles(&recorded);
    let txid = bot.exchange().orders()[0].txid.clone();

    bot.exchange().fill(&txid, Some(d("0.001")));
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0].0, "Buy partially filled · #1 XBTUSD");
    assert!(
        notified[0].1.starts_with("0.001 / 0.00201516 XBT"),
        "{}",
        notified[0].1
    );

    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());
}

#[tokio::test]
async fn a_failure_is_notified_once_until_it_recovers() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}";
    let (mut bot, recorded) = bot(exchange("1000"), false, "", managers).await;
    bot.tick().await.unwrap();
    bot.exchange().set_price(BTC, d("49400"));
    // the bitcoins bought vanish from the account: the sale is refused at every tick
    bot.exchange().deposit("XXBT", d("-0.00201516"));
    titles(&recorded);

    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    let errors: Vec<_> = notified
        .iter()
        .filter(|(title, _)| title.starts_with("Error"))
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, "Error · #1 XBTUSD");
    assert!(
        errors[0].1.contains("Insufficient funds"),
        "{}",
        errors[0].1
    );

    bot.tick().await.unwrap();
    bot.tick().await.unwrap();
    assert!(titles(&recorded).iter().all(|it| !it.starts_with("Error")));

    // they come back: the sale is placed, and the next failure will be notified
    bot.exchange().deposit("XXBT", d("0.00201516"));
    bot.tick().await.unwrap();
    assert!(titles(&recorded).contains(&"Sell placed · #1 XBTUSD".to_string()));

    bot.notify_failure("tick", None, "Kraken: Transport error: timeout")
        .await;
    bot.notify_failure("tick", None, "Kraken: Transport error: timeout")
        .await;
    assert_eq!(titles(&recorded), ["Bot error"]);
    bot.clear_failure("tick");
    bot.notify_failure("tick", None, "Kraken: Transport error: timeout")
        .await;
    assert_eq!(titles(&recorded), ["Bot error"]);
}

#[tokio::test]
async fn only_the_events_of_the_configuration_are_notified() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "  events: [filled]", ONE).await;
    bot.notify_started().await;
    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());

    bot.exchange().set_price(BTC, d("49400"));
    bot.tick().await.unwrap();
    assert_eq!(titles(&recorded), ["Buy filled · #1 XBTUSD"]);
}

#[tokio::test]
async fn the_daily_summary_is_sent_once_a_day_after_its_hour() {
    let managers = "  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}
  - {id: 2, pair: XBTUSD, budget: 50, buy_below: 2, sell_above: 3, mode: closing}";
    let (mut bot, recorded) = bot(
        exchange("1000"),
        false,
        "  events: [daily_summary]\n  daily_summary_hour: 20",
        managers,
    )
    .await;

    // 08:00: not yet
    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());

    // 20:00
    bot.exchange().advance(12.0 * 3600.0);
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0].0, "Daily summary");
    assert_eq!(
        notified[0].1,
        "#1 XBTUSD: 100 USD (+0 · +0 %) · 0 sale(s) in 24 h
#2 XBTUSD: 50 USD (+0 · +0 %) · 0 sale(s) in 24 h
total USD: 150 (+0)"
    );

    // the same day: no more
    bot.exchange().advance(3600.0);
    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());

    // the next day, after 20:00
    bot.exchange().advance(23.0 * 3600.0);
    bot.tick().await.unwrap();
    assert_eq!(titles(&recorded), ["Daily summary"]);
}

#[tokio::test]
async fn the_history_found_at_the_first_start_isnt_notified() {
    let exchange = exchange("1000");
    let txid =
        exchange.add_foreign_order(Some(1), "XBTUSD", OrderSide::Buy, d("0.002"), d("49500"));
    exchange.fill(&txid, None);
    exchange.advance(3600.0);

    let (mut bot, recorded) = bot(exchange, false, "", ONE).await;
    bot.tick().await.unwrap();
    // only the sale it places now
    assert_eq!(titles(&recorded), ["Sell placed · #1 XBTUSD"]);
}

#[tokio::test]
async fn the_notifications_speak_the_language_of_the_configuration() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "  language: fr", ONE).await;
    bot.notify_started().await;
    bot.tick().await.unwrap();
    let notified = messages(&recorded);
    assert_eq!(notified[0].0, "Bot démarré");
    assert_eq!(notified[1].0, "Achat placé · #1 XBTUSD");
    assert!(
        notified[1].1.starts_with("0,00201516 XBT à 49500 USD"),
        "{}",
        notified[1].1
    );
}

#[tokio::test]
async fn the_orders_of_a_virtual_manager_are_notified_and_summed_up() {
    // 100 USD for manager 1, 100 for a copy of it
    let (mut bot, recorded) = bot(
        exchange("200"),
        false,
        "  events: [placed, filled, daily_summary]\n  daily_summary_hour: 20
virtual_managers: true",
        ONE,
    )
    .await;

    bot.tick().await.unwrap();
    assert_eq!(
        titles(&recorded),
        ["Buy placed · #1 XBTUSD", "Buy placed · #1000001 XBTUSD"]
    );

    bot.exchange().set_price(BTC, d("49400"));
    bot.tick().await.unwrap();
    let notified = titles(&recorded);
    assert!(
        notified.contains(&"Buy filled · #1000001 XBTUSD".to_string()),
        "{notified:?}"
    );

    // 20:00
    bot.exchange().advance(12.0 * 3600.0);
    bot.tick().await.unwrap();
    let summary = messages(&recorded)
        .into_iter()
        .find(|(title, _)| title == "Daily summary")
        .unwrap()
        .1;
    assert!(summary.contains("#1 XBTUSD"), "{summary}");
    assert!(summary.contains("#1000001 XBTUSD"), "{summary}");
}

#[tokio::test]
async fn a_buy_placed_again_by_the_trailing_isnt_notified_as_ended() {
    let (mut bot, recorded) = bot(exchange("1000"), false, "buy_trailing: 0.5", ONE).await;
    bot.tick().await.unwrap();
    titles(&recorded);

    // cancelled by the bot, then placed again: only the new buy is notified
    bot.exchange().set_price(BTC, d("60000"));
    bot.tick().await.unwrap();
    assert!(titles(&recorded).is_empty());
    bot.tick().await.unwrap();
    assert_eq!(titles(&recorded), ["Buy placed · #1 XBTUSD"]);

    // an expiry is still notified
    bot.exchange().advance(12.0 * 3600.0 + 1.0);
    bot.tick().await.unwrap();
    assert_eq!(titles(&recorded)[0], "Buy expired · #1 XBTUSD");
}
