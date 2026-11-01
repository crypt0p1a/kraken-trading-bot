use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
pub use trading_notify::{EventKind, Language};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("can't read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("invalid configuration: {0}")]
    Parse(String),
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// What a manager does
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagerMode {
    /// Buys, then sells, in a loop
    #[default]
    Active,
    /// Sells what it holds, doesn't buy anymore: the way to stop a manager
    Closing,
}

/// A manager: trades `budget` of the quote currency of `pair`
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerConfig {
    /// Kraken `userref` of its orders: unique, from 1 to 2147483647, never reused
    pub id: i32,
    /// Kraken pair, e.g. "XBTUSD", "XBT/USD" or "XXBTZUSD"
    pub pair: String,
    /// Amount of the quote currency (USD for XBTUSD) the manager starts with
    pub budget: Decimal,
    /// Buys at -buy_below% of the last price, in [0, 100[
    pub buy_below: Decimal,
    /// Sells for what the position cost + sell_above%, fees included, > 0
    pub sell_above: Decimal,
    #[serde(default)]
    pub mode: ManagerMode,
}

/// The YAML configuration of the bot
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    /// Orders only validated by Kraken, never placed. True unless set to false
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// Delay between two ticks, e.g. "5m"
    #[serde(default = "default_tick_interval", with = "humantime_serde")]
    pub tick_interval: Duration,
    /// A buy order not filled after this delay expires, and is placed again from the new price
    #[serde(default = "default_buy_order_timeout", with = "humantime_serde")]
    pub buy_order_timeout: Duration,
    /// Push notifications, none when missing
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,
    pub managers: Vec<ManagerConfig>,
}

/// ntfy server: the topic, and its access token if any, are in the environment
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyConfig {
    /// https://ntfy.sh by default, or a self-hosted server
    #[serde(default = "default_ntfy_server")]
    pub server: String,
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".into()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsConfig {
    pub ntfy: NtfyConfig,
    /// Every event when missing
    #[serde(default = "default_events")]
    pub events: Vec<EventKind>,
    /// Notifies the orders of a dry run too: false by default, a dry run validating an order at
    /// every tick
    #[serde(default)]
    pub dry_run: bool,
    /// Hour of the daily summary, UTC
    #[serde(default = "default_daily_summary_hour")]
    pub daily_summary_hour: u32,
    /// Language of the notifications: en (default) or fr
    #[serde(default)]
    pub language: Language,
}

fn default_events() -> Vec<EventKind> {
    EventKind::ALL.to_vec()
}

fn default_daily_summary_hour() -> u32 {
    20
}

impl NotificationsConfig {
    pub fn wants(&self, event: EventKind) -> bool {
        self.events.contains(&event)
    }
}

fn default_dry_run() -> bool {
    true
}

fn default_tick_interval() -> Duration {
    Duration::from_secs(5 * 60)
}

fn default_buy_order_timeout() -> Duration {
    Duration::from_secs(12 * 3600)
}

impl BotConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Read {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: BotConfig =
            serde_norway::from_str(text).map_err(|error| ConfigError::Parse(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |reason: String| Err(ConfigError::Invalid(reason));

        if self.tick_interval < Duration::from_secs(30) {
            return invalid(format!(
                "tick_interval must be at least 30s, got {:?}",
                self.tick_interval
            ));
        }
        if self.buy_order_timeout < Duration::from_secs(60) {
            return invalid(format!(
                "buy_order_timeout must be at least 1m, got {:?}",
                self.buy_order_timeout
            ));
        }

        if let Some(notifications) = &self.notifications {
            if notifications.daily_summary_hour > 23 {
                return invalid(format!(
                    "notifications: daily_summary_hour must be from 0 to 23, got {}",
                    notifications.daily_summary_hour
                ));
            }
            let server = notifications.ntfy.server.trim();
            if !(server.starts_with("https://") || server.starts_with("http://")) {
                return invalid(format!(
                    "notifications: the ntfy server must be an http(s) URL, got {server}"
                ));
            }
        }

        let mut ids = HashSet::new();
        for manager in &self.managers {
            let id = manager.id;
            if id < 1 {
                return invalid(format!("manager {id}: the id must be from 1 to 2147483647"));
            }
            if !ids.insert(id) {
                return invalid(format!("manager {id}: the id is used twice"));
            }
            if manager.pair.trim().is_empty() {
                return invalid(format!("manager {id}: the pair is empty"));
            }
            if manager.budget <= Decimal::ZERO {
                return invalid(format!("manager {id}: the budget must be > 0"));
            }
            if manager.buy_below < Decimal::ZERO || manager.buy_below >= Decimal::ONE_HUNDRED {
                return invalid(format!("manager {id}: buy_below must be in [0, 100["));
            }
            if manager.sell_above <= Decimal::ZERO {
                return invalid(format!("manager {id}: sell_above must be > 0"));
            }
        }

        Ok(())
    }

    /// The ids of the managers: the `userref` of their orders
    pub fn ids(&self) -> HashSet<i32> {
        self.managers.iter().map(|it| it.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const EXAMPLE: &str = r#"
dry_run: false
tick_interval: 2m
buy_order_timeout: 6h
managers:
  - id: 1
    pair: XBTUSD
    budget: 30
    buy_below: 1.0
    sell_above: 1.5
  - id: 2
    pair: ETH/EUR
    budget: "50.25"
    buy_below: 0.8
    sell_above: 1.2
    mode: closing
"#;

    fn d(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    #[test]
    fn reads_the_managers() {
        let config = BotConfig::parse(EXAMPLE).unwrap();

        assert!(!config.dry_run);
        assert_eq!(config.tick_interval, Duration::from_secs(120));
        assert_eq!(config.buy_order_timeout, Duration::from_secs(6 * 3600));
        assert_eq!(config.managers.len(), 2);

        let first = &config.managers[0];
        assert_eq!(first.id, 1);
        assert_eq!(first.pair, "XBTUSD");
        assert_eq!(first.budget, d("30"));
        assert_eq!(first.buy_below, d("1.0"));
        assert_eq!(first.mode, ManagerMode::Active);

        let second = &config.managers[1];
        assert_eq!(second.budget, d("50.25"));
        assert_eq!(second.buy_below, d("0.8"));
        assert_eq!(second.mode, ManagerMode::Closing);
        assert_eq!(config.ids(), HashSet::from([1, 2]));
    }

    #[test]
    fn is_a_dry_run_unless_said_otherwise() {
        let config = BotConfig::parse("managers: []").unwrap();
        assert!(config.dry_run);
        assert_eq!(config.tick_interval, Duration::from_secs(300));
        assert_eq!(config.buy_order_timeout, Duration::from_secs(12 * 3600));
    }

    #[test]
    fn reads_the_decimals_exactly() {
        let config = BotConfig::parse(
            "managers: [{id: 1, pair: XBTUSD, budget: 0.1, buy_below: 0.3, sell_above: 2.675}]",
        )
        .unwrap();
        assert_eq!(config.managers[0].budget, d("0.1"));
        assert_eq!(config.managers[0].buy_below, d("0.3"));
        assert_eq!(config.managers[0].sell_above, d("2.675"));
    }

    #[test]
    fn rejects_the_invalid_managers() {
        let manager = |fields: &str| format!("managers: [{{{fields}}}]");
        let valid = "id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5";
        assert!(BotConfig::parse(&manager(valid)).is_ok());

        for (fields, expected) in [
            (
                "id: 0, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5",
                "id",
            ),
            (
                "id: -3, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5",
                "id",
            ),
            (
                "id: 2147483648, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5",
                "",
            ),
            (
                "id: 1, pair: ' ', budget: 30, buy_below: 1, sell_above: 1.5",
                "pair",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 0, buy_below: 1, sell_above: 1.5",
                "budget",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: 100, sell_above: 1.5",
                "buy_below",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: -1, sell_above: 1.5",
                "buy_below",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 0",
                "sell_above",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: 1",
                "sell_above",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1, mode: paused",
                "mode",
            ),
            (
                "id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1, bugdet: 3",
                "bugdet",
            ),
        ] {
            let error = BotConfig::parse(&manager(fields)).unwrap_err().to_string();
            assert!(error.contains(expected), "{fields}: {error}");
        }
    }

    #[test]
    fn rejects_the_ids_used_twice() {
        let error = BotConfig::parse(
            "managers:
  - {id: 7, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}
  - {id: 7, pair: ETHUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
        )
        .unwrap_err();
        assert!(error.to_string().contains("used twice"), "{error}");
    }

    #[test]
    fn reads_the_notifications() {
        let config = BotConfig::parse(
            "notifications:
  ntfy: {server: https://ntfy.example.com}
  events: [placed, filled]
  dry_run: true
  daily_summary_hour: 7
  language: fr
managers: []",
        )
        .unwrap();
        let notifications = config.notifications.unwrap();
        assert_eq!(notifications.ntfy.server, "https://ntfy.example.com");
        assert!(notifications.wants(EventKind::Filled));
        assert!(!notifications.wants(EventKind::Failed));
        assert!(notifications.dry_run);
        assert_eq!(notifications.daily_summary_hour, 7);
        assert_eq!(notifications.language, Language::Fr);
    }

    #[test]
    fn notifies_everything_on_ntfy_sh_by_default() {
        let config = BotConfig::parse("notifications: {ntfy: {}}\nmanagers: []").unwrap();
        let notifications = config.notifications.unwrap();
        assert_eq!(notifications.ntfy.server, "https://ntfy.sh");
        assert!(EventKind::ALL.iter().all(|it| notifications.wants(*it)));
        assert!(!notifications.dry_run);
        assert_eq!(notifications.daily_summary_hour, 20);
        assert_eq!(notifications.language, Language::En);

        assert_eq!(
            BotConfig::parse("managers: []").unwrap().notifications,
            None
        );
    }

    #[test]
    fn rejects_the_invalid_notifications() {
        for yaml in [
            "notifications: {ntfy: {}, daily_summary_hour: 24}",
            "notifications: {ntfy: {server: ntfy.sh}}",
            "notifications: {ntfy: {}, events: [everything]}",
            "notifications: {ntfy: {topic: secret}}",
            "notifications: {ntfy: {}, language: de}",
            "notifications: {events: [placed]}",
        ] {
            assert!(
                BotConfig::parse(&format!("{yaml}\nmanagers: []")).is_err(),
                "{yaml}"
            );
        }
    }

    #[test]
    fn rejects_the_too_short_delays() {
        assert!(BotConfig::parse("tick_interval: 10s\nmanagers: []").is_err());
        assert!(BotConfig::parse("buy_order_timeout: 30s\nmanagers: []").is_err());
        assert!(BotConfig::parse("tick_interval: often\nmanagers: []").is_err());
    }

    #[test]
    fn reads_the_example_of_the_repository() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../managers.example.yaml");
        let config = BotConfig::load(&path).unwrap();
        assert!(config.dry_run, "the example must stay a dry run");
        assert!(!config.managers.is_empty());
    }
}
