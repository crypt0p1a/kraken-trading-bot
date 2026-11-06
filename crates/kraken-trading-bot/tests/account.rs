//! The value of the whole account at each tick, managers or not, against a simulated Kraken

use kraken_trading_bot::bot::Bot;
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;
use std::str::FromStr;

const BTC: &str = "XXBTZUSD";

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

#[tokio::test]
async fn records_what_the_whole_account_is_worth() {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.set_fee_percent(d("0.25"));
    // 100 USD for a manager of 30, and bitcoins no manager bought
    exchange.deposit("ZUSD", d("100"));
    exchange.deposit("XXBT", d("0.001"));
    let config = BotConfig::parse(
        "dry_run: false
managers:
  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
    )
    .unwrap();
    let mut bot = Bot::new(exchange, Storage::open_in_memory().unwrap(), config).with_seed(7);
    bot.prepare().await.unwrap();

    // the buy placed: its quote is held, still in the account
    bot.tick().await.unwrap();
    let accounts = bot.storage().last_accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].quote, "ZUSD");
    assert_eq!(accounts[0].cash, d("100"));
    assert_eq!(accounts[0].holdings, d("50"));

    // the buy filled: the bitcoins at the last price
    bot.exchange().set_price(BTC, d("49000"));
    bot.tick().await.unwrap();
    let account = &bot.storage().last_accounts().unwrap()[0];
    let bitcoins = bot.exchange().balance("XXBT");
    assert!(bitcoins > d("0.001"), "{bitcoins}");
    assert_eq!(account.holdings, bitcoins * d("49000"));
    assert!(account.cash < d("71"), "{}", account.cash);
}

fn exchange_and_config() -> (SimulatedExchange, BotConfig) {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", d("50000"));
    exchange.deposit("ZUSD", d("100"));
    let config = BotConfig::parse(
        "dry_run: false
managers:
  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
    )
    .unwrap();
    (exchange, config)
}

/// The synchronization of the orders can take minutes: the dashboard shows the managers before it
#[tokio::test]
async fn saves_a_new_manager_before_synchronizing_the_orders() {
    let (exchange, config) = exchange_and_config();
    exchange.fail_open_orders(Some("EService:Unavailable"));
    let mut bot = Bot::new(exchange, Storage::open_in_memory().unwrap(), config).with_seed(7);
    bot.prepare().await.unwrap();

    assert!(bot.tick().await.is_err());
    let (balances, price, _) = bot.storage().manager_balances(1).unwrap().unwrap();
    assert_eq!(balances.quote_available, d("30"));
    assert_eq!(price, d("50000"));
    assert_eq!(bot.exchange().add_order_calls(), 0);
}
