//! kraken-trading-bot: runs the managers of the YAML configuration on Kraken
//!
//! ```text
//! kraken-trading-bot           ticks until SIGTERM / Ctrl-C
//! kraken-trading-bot --once    a single tick
//! kraken-trading-bot --check   checks the configuration and the pairs on Kraken, then stops
//! ```
//!
//! Environment (or `.env`): KRAKEN_API_KEY, KRAKEN_API_SECRET, KRAKEN_BOT_CONFIG,
//! KRAKEN_BOT_DATABASE, RUST_LOG

use kraken_api::RestOptions;
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::{BotConfig, Environment};
use kraken_trading_bot::exchange::KrakenExchange;
use kraken_trading_bot::storage::Storage;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Loop,
    Once,
    Check,
}

fn mode(arguments: &[String]) -> Result<Mode, String> {
    match arguments {
        [] => Ok(Mode::Loop),
        [flag] if flag == "--once" => Ok(Mode::Once),
        [flag] if flag == "--check" => Ok(Mode::Check),
        other => Err(format!(
            "unknown arguments {other:?}, expected nothing, --once or --check"
        )),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match mode(&arguments) {
        Ok(mode) => run(mode).await,
        Err(error) => Err(error),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run(mode: Mode) -> Result<(), String> {
    let environment = Environment::load()?;
    let config = BotConfig::load(&environment.config_path).map_err(|error| error.to_string())?;

    if config.dry_run {
        tracing::warn!(
            "DRY RUN: the orders are only validated by Kraken, never placed (dry_run: false to trade)"
        );
    } else {
        tracing::warn!("LIVE: the orders are placed on Kraken");
    }
    tracing::info!(
        managers = config.managers.len(),
        tick_interval = ?config.tick_interval,
        config = %environment.config_path.display(),
        database = %environment.database_path.display(),
        "starting"
    );

    let storage = Storage::open(&environment.database_path).map_err(|error| error.to_string())?;
    let exchange = KrakenExchange::new(
        environment.api_key,
        environment.api_secret,
        RestOptions::default(),
    )
    .map_err(|error| error.to_string())?;
    let mut bot = Bot::new(exchange, storage, config);
    bot.prepare().await.map_err(|error| error.to_string())?;

    if mode == Mode::Check {
        tracing::info!("the configuration and the pairs are valid");
        return Ok(());
    }

    let interval = bot.config().tick_interval;
    let mut shutdown = std::pin::pin!(shutdown());
    loop {
        match bot.tick().await {
            Ok(report) => {
                let failed = report
                    .outcomes
                    .values()
                    .filter(|it| matches!(it, Outcome::Failed(_)))
                    .count();
                tracing::info!(
                    managers = report.outcomes.len(),
                    failed,
                    open_orders = report.sync.open,
                    "tick done"
                );
            }
            Err(error) => tracing::error!(%error, "tick failed, retried at the next one"),
        }

        if mode == Mode::Once {
            return Ok(());
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = &mut shutdown => {
                tracing::info!("stopping");
                return Ok(());
            }
        }
    }
}

/// SIGTERM (docker stop) or Ctrl-C
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_arguments() {
        let args = |values: &[&str]| values.iter().map(|it| it.to_string()).collect::<Vec<_>>();
        assert_eq!(mode(&args(&[])), Ok(Mode::Loop));
        assert_eq!(mode(&args(&["--once"])), Ok(Mode::Once));
        assert_eq!(mode(&args(&["--check"])), Ok(Mode::Check));
        assert!(mode(&args(&["--loop"])).is_err());
        assert!(mode(&args(&["--once", "--check"])).is_err());
    }
}
