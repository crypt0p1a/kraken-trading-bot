//! The indicators of the market against a simulated Kraken: recorded at each tick once 12 hours of
//! candles exist, and nothing decided on them

use kraken_trading_bot::bot::Bot;
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::storage::Storage;
use rust_decimal::Decimal;

const BTC: &str = "XXBTZUSD";

async fn bot() -> Bot<SimulatedExchange> {
    let exchange = SimulatedExchange::new(1_800_000_000.0);
    exchange.add_pair(BTC, "XBTUSD", "XXBT", "ZUSD", Decimal::new(50_000, 0));
    exchange.deposit("ZUSD", Decimal::new(1000, 0));
    let config = BotConfig::parse(
        "dry_run: false
managers:
  - {id: 1, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}",
    )
    .unwrap();
    let mut bot = Bot::new(exchange, Storage::open_in_memory().unwrap(), config);
    bot.prepare().await.unwrap();
    bot
}

#[tokio::test]
async fn records_the_trend_once_twelve_hours_of_candles_exist() {
    let mut bot = bot().await;

    // +10 USD every 15 minutes
    for step in 0..60 {
        bot.exchange()
            .set_price(BTC, Decimal::new(50_000 + 10 * step, 0));
        bot.tick().await.unwrap();
        if step < 47 {
            assert_eq!(bot.storage().last_indicators(BTC).unwrap(), None, "{step}");
        }
        bot.exchange().advance(900.0);
    }

    let (_, indicators) = bot.storage().last_indicators(BTC).unwrap().unwrap();
    assert!(indicators.candles >= 48, "{indicators:?}");
    assert_eq!(indicators.trend(), "rising");
    assert!(indicators.slope_24h_percent > 0.0);
    assert!(indicators.change_24h_percent > 0.0);
}
