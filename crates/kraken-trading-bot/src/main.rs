//! kraken-trading-bot: runs the managers of the YAML configuration on Kraken
//!
//! ```text
//! kraken-trading-bot           ticks until SIGTERM / Ctrl-C
//! kraken-trading-bot --once    a single tick
//! kraken-trading-bot --check   checks the configuration and the pairs on Kraken, then stops
//! ```
//!
//! Environment (or `.env`): KRAKEN_API_KEY, KRAKEN_API_SECRET, KRAKEN_BOT_CONFIG,
//! KRAKEN_BOT_DATABASE, RUST_LOG. For several Kraken accounts, trading the same managers:
//! KRAKEN_BOT_ACCOUNTS=main,alice and KRAKEN_API_KEY_ALICE, KRAKEN_API_SECRET_ALICE

use kraken_api::{RateLimiter, RestOptions};
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::clock::now_millis;
use kraken_trading_bot::config::{AccountKey, BotConfig, Environment};
use kraken_trading_bot::exchange::KrakenExchange;
use kraken_trading_bot::storage::{FORCED_TICK, Storage};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::{JoinSet, LocalSet};
use tracing::Instrument;
use tracing_subscriber::EnvFilter;
use trading_notify::Notifier;

/// How often a tick asked for from outside (the dashboard) is looked for, between two ticks
const COMMAND_POLL: Duration = Duration::from_secs(10);

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
        accounts = environment.accounts.len(),
        managers = config.managers.len(),
        tick_interval = ?config.tick_interval,
        config = %environment.config_path.display(),
        database = %environment.database_path.display(),
        "starting"
    );

    // the pairs of every account are checked before any of them trades. Not its key, which only
    // private calls use: a refused key fails the ticks of its account, the others go on
    let limiter = Arc::new(RateLimiter::default());
    let several = environment.accounts.len() > 1;
    let mut bots = Vec::new();
    for account in &environment.accounts {
        let span = tracing::info_span!("account", name = %account.name);
        let bot = prepared(account, &config, &environment, limiter.clone(), several)
            .instrument(span.clone())
            .await
            .map_err(|error| format!("account {}: {error}", account.name))?;
        bots.push((bot, span));
    }

    if mode == Mode::Check {
        tracing::info!("the configuration and the pairs are valid");
        return Ok(());
    }

    let (stop, stopped) = watch::channel(false);
    // on one thread: a connection to SQLite can't be shared between threads, and a tick waits for
    // Kraken most of the time
    LocalSet::new()
        .run_until(async move {
            let mut accounts = JoinSet::new();
            for (bot, span) in bots {
                accounts.spawn_local(trade(bot, mode, stopped.clone()).instrument(span));
            }
            let mut shutdown = std::pin::pin!(shutdown());
            let mut asked = false;
            loop {
                tokio::select! {
                    finished = accounts.join_next() => match finished {
                        None => return Ok(()),
                        Some(Ok(())) if asked || mode == Mode::Once => {}
                        // the loop of an account only ends when asked to: a panic. The others stop
                        // too, and the bot is started again by whatever watches it
                        Some(outcome) => {
                            let _ = stop.send(true);
                            while accounts.join_next().await.is_some() {}
                            return Err(format!("an account stopped: {outcome:?}"));
                        }
                    },
                    _ = &mut shutdown, if !asked => {
                        tracing::info!("stopping");
                        asked = true;
                        let _ = stop.send(true);
                    }
                }
            }
        })
        .await
}

/// The bot of this account, its pairs checked on Kraken
async fn prepared(
    account: &AccountKey,
    config: &BotConfig,
    environment: &Environment,
    limiter: Arc<RateLimiter>,
    several: bool,
) -> Result<Bot<KrakenExchange>, String> {
    let storage = Storage::open_account(&environment.database_path, &account.name)
        .map_err(|error| error.to_string())?;
    let exchange = KrakenExchange::with_limiter(
        account.api_key.clone(),
        account.api_secret.clone(),
        RestOptions::default(),
        limiter,
    )
    .map_err(|error| error.to_string())?;
    let mut bot =
        Bot::new(exchange, storage, config.clone()).with_notifier(notifier(config, environment)?);
    if several {
        bot = bot.with_account_label(&account.name);
    }
    bot.prepare().await.map_err(|error| error.to_string())?;
    Ok(bot)
}

/// The ticks of an account, until it's asked to stop: the tick in progress ends first
async fn trade(mut bot: Bot<KrakenExchange>, mode: Mode, mut stopped: watch::Receiver<bool>) {
    bot.notify_started().await;

    let interval = bot.config().tick_interval;
    loop {
        match bot.tick().await {
            Ok(report) => {
                bot.clear_failure("tick");
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
            Err(error) => {
                tracing::error!(%error, "tick failed, retried at the next one");
                bot.notify_failure("tick", None, &error.to_string()).await;
            }
        }

        // a forced tick is done with this one, whatever its outcome
        if let Err(error) = bot.storage().end_command(FORCED_TICK, now_millis()) {
            tracing::warn!(%error, "the forced tick couldn't be ended");
        }

        if mode == Mode::Once || *stopped.borrow() {
            return;
        }

        // until the next tick, or a tick asked for meanwhile (the dashboard)
        let next = tokio::time::Instant::now() + interval;
        loop {
            let wait = next
                .saturating_duration_since(tokio::time::Instant::now())
                .min(COMMAND_POLL);
            if wait.is_zero() {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                // asked to stop, or nobody left to ask
                _ = stopped.changed() => return,
            }
            match bot.storage().start_command(FORCED_TICK, now_millis()) {
                Ok(true) => {
                    tracing::info!("tick forced");
                    break;
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "the forced tick couldn't be read"),
            }
        }
    }
}

/// The ntfy topic of the configuration: required when notifications are configured
fn notifier(config: &BotConfig, environment: &Environment) -> Result<Notifier, String> {
    let Some(notifications) = &config.notifications else {
        return Ok(Notifier::Silent);
    };
    let topic = environment
        .ntfy_topic
        .as_deref()
        .ok_or("notifications are configured: KRAKEN_BOT_NTFY_TOPIC is required")?;
    tracing::info!(server = %notifications.ntfy.server, language = ?notifications.language, "notifications through ntfy");
    Notifier::ntfy(
        &notifications.ntfy.server,
        topic,
        environment.ntfy_token.as_deref(),
    )
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
