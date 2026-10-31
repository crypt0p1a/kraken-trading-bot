//! One tick of the bot on the real Kraken, as a dry run: nothing is placed. Credentials from the
//! environment or the `.env` file at the root of the repository:
//! `cargo test -p kraken-trading-bot --test live_dry_run -- --ignored --nocapture`

use kraken_api::RestOptions;
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::{Exchange, KrakenExchange};
use kraken_trading_bot::storage::Storage;

fn credential(name: &str) -> String {
    std::env::var(name)
        .ok()
        .or_else(|| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
            let content = std::fs::read_to_string(path).ok()?;
            content
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(key, _)| key.trim() == name)
                .map(|(_, value)| {
                    value
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                })
        })
        .filter(|it| !it.is_empty())
        .unwrap_or_else(|| panic!("{name} is required, see .env.example"))
}

#[tokio::test]
#[ignore = "calls the real Kraken API with credentials (dry run, nothing is placed)"]
async fn a_dry_run_tick_on_the_real_kraken() {
    let exchange = KrakenExchange::new(
        credential("KRAKEN_API_KEY"),
        credential("KRAKEN_API_SECRET"),
        RestOptions::default(),
    )
    .unwrap();
    // ids no real manager uses
    let config = BotConfig::parse(
        "dry_run: true
managers:
  - {id: 2000000001, pair: XBTUSD, budget: 10, buy_below: 1, sell_above: 1.5}
  - {id: 2000000002, pair: ETH/EUR, budget: 10, buy_below: 2, sell_above: 3}",
    )
    .unwrap();
    let mut bot = Bot::new(exchange, Storage::open_in_memory().unwrap(), config);
    bot.prepare().await.unwrap();

    let report = bot.tick().await.unwrap();
    println!("{report:#?}");

    for (id, outcome) in &report.outcomes {
        match outcome {
            // validated by Kraken, not placed
            Outcome::Placed { txid, .. } => assert!(txid.is_none(), "{id}: an order was placed"),
            // the account may not have the money
            Outcome::Idle(_) => {}
            other => panic!("{id}: {other:?}"),
        }
    }
    let open = bot.exchange().open_orders().await.unwrap();
    assert!(
        open.iter()
            .all(|it| it.userref.unwrap_or_default() < 2_000_000_000)
    );
}
