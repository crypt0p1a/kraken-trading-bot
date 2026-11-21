//! A tick of the bot: synchronizes the orders, then each manager decides and acts. Port of the
//! ManagersPool of cex-trading-bot

mod actions;
mod manager;
mod markets;
mod notifications;
mod orders;
mod trailing;
mod virtuals;

pub use orders::ensure_limit;
pub use virtuals::STARVED_TICKS;

use crate::config::{BotConfig, ManagerConfig, ManagerMode};
use crate::exchange::Exchange;
use crate::notify;
use crate::pair::PairRules;
use crate::storage::Storage;
use crate::sync::{self, SyncReport};
use crate::{BotError, BotResult};
use kraken_spot_api::trading::{ClosedOrdersRequest, OrderSide};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Mutex;
use trading_notify::Notifier;

/// What a manager did during a tick
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Waited,
    Idle(String),
    /// An order was placed (its txid), or only validated (dry run, no txid)
    Placed {
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        txid: Option<String>,
    },
    Failed(String),
    /// A virtual manager deleted: it held nothing, and the account couldn't pay for its buy
    Deleted(String),
    /// Its buy cancelled, the price having risen too far above it: placed again at the next tick
    Cancelled(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TickReport {
    pub sync: SyncReport,
    /// Per manager id
    pub outcomes: HashMap<i32, Outcome>,
}

/// The market of a pair at a tick
struct Market {
    rules: PairRules,
    reference: Decimal,
    fee_rate: Decimal,
}

pub struct Bot<E: Exchange> {
    exchange: E,
    storage: Storage,
    config: BotConfig,
    /// Pair of the configuration -> Kraken name
    pair_keys: HashMap<String, String>,
    notifier: Notifier,
    /// Put before the title of the notifications, when there are several accounts
    account_label: Option<String>,
    /// The failure last notified, per source: a failure repeated at each tick is notified once
    failures: Mutex<HashMap<String, String>>,
    /// Picks the manager a virtual manager copies
    random: fastrand::Rng,
    /// Ticks done, to tell the ticks in a row
    ticks: u64,
    /// Per virtual manager: the last tick it couldn't buy for lack of money, and how many in a row
    starved: Mutex<HashMap<i32, (u64, u32)>>,
}

impl<E: Exchange> Bot<E> {
    pub fn new(exchange: E, storage: Storage, config: BotConfig) -> Self {
        Self {
            exchange,
            storage,
            config,
            pair_keys: HashMap::new(),
            notifier: Notifier::Silent,
            account_label: None,
            failures: Mutex::new(HashMap::new()),
            random: fastrand::Rng::new(),
            ticks: 0,
            starved: Mutex::new(HashMap::new()),
        }
    }

    /// The same managers copied by the virtual managers at each run: for the tests
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.random = fastrand::Rng::with_seed(seed);
        self
    }

    /// The managers of the YAML file, then the virtual managers. A virtual manager closes with the
    /// manager it copies, closing or gone from the file: it sells what it holds, then is deleted
    pub fn managers(&self) -> BotResult<Vec<ManagerConfig>> {
        let mut managers = self.config.managers.clone();
        if self.config.virtual_managers {
            for virtual_manager in self.storage.virtual_managers()? {
                let mut config = virtual_manager.config;
                let template = self
                    .config
                    .managers
                    .iter()
                    .find(|it| it.id == virtual_manager.template_id);
                if template.is_none_or(|it| it.mode == ManagerMode::Closing) {
                    config.mode = ManagerMode::Closing;
                }
                managers.push(config);
            }
        }
        Ok(managers)
    }

    /// Where the notifications of the configuration go
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = notifier;
        self
    }

    /// Tells in its notifications which account the bot trades on
    pub fn with_account_label(mut self, label: &str) -> Self {
        self.account_label = Some(label.to_string());
        self
    }

    pub fn exchange(&self) -> &E {
        &self.exchange
    }

    pub fn into_exchange(self) -> E {
        self.exchange
    }

    /// The exchange and the database: to start another bot on them
    pub fn into_parts(self) -> (E, Storage) {
        (self.exchange, self.storage)
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn config(&self) -> &BotConfig {
        &self.config
    }

    /// Finds the pairs of the managers on Kraken: fails on a pair unknown or not tradable
    pub async fn prepare(&mut self) -> BotResult<()> {
        let all = self.exchange.asset_pairs(&[]).await?;
        for manager in &self.managers()? {
            let (key, pair) = all
                .iter()
                .find(|(key, pair)| PairRules::matches(&manager.pair, key, pair))
                .ok_or_else(|| {
                    BotError::Config(format!(
                        "manager {}: unknown pair {}",
                        manager.id, manager.pair
                    ))
                })?;
            if pair
                .status
                .is_some_and(|it| it != kraken_spot_api::market::AssetPairStatus::Online)
            {
                tracing::warn!(manager = manager.id, pair = %pair.altname, status = ?pair.status, "the pair isn't online");
            }
            self.pair_keys.insert(manager.pair.clone(), key.clone());
        }
        Ok(())
    }

    /// What's wrong with the API key, nothing when the bot can trade with it and it can't withdraw.
    /// Kraken refusing it at all is an error
    pub async fn key_problems(&self) -> BotResult<Vec<String>> {
        let permissions = self.exchange.key_permissions().await?;
        let has = |name: &str| permissions.iter().any(|it| it == name);
        let mut problems: Vec<String> = [
            ("query-funds", "Query Funds"),
            ("query-open-trades", "Query Open Orders & Trades"),
            ("modify-trades", "Create & Modify Orders"),
        ]
        .into_iter()
        .filter(|(name, _)| !has(name))
        .map(|(_, label)| format!("the key is missing the permission {label}"))
        .collect();
        // a read rather than the name of the permission
        if let Err(error) = self
            .exchange
            .closed_orders(&ClosedOrdersRequest::default())
            .await
        {
            problems.push(format!(
                "the key can't read the closed orders (Query Closed Orders & Trades): {error}"
            ));
        }
        if permissions.iter().any(|it| it.contains("withdraw")) {
            problems.push("the key can withdraw funds: it must never have this permission".into());
        }
        Ok(problems)
    }

    pub async fn tick(&mut self) -> BotResult<TickReport> {
        self.ticks += 1;
        self.rebuild_virtual_managers().await?;
        let mut managers = self.managers()?;
        // the pairs of the managers sharing one are already known
        if managers
            .iter()
            .any(|it| !self.pair_keys.contains_key(&it.pair))
        {
            self.prepare().await?;
        }

        let markets = self.markets().await?;
        // only for the dashboard, each manager saves itself below: this never stops the trading
        if let Err(error) = self.save_new_managers(&managers, &markets) {
            tracing::warn!(%error, "the new managers couldn't be saved before the synchronization");
        }
        let ids = managers.iter().map(|it| it.id).collect();
        let sync =
            sync::synchronize(&self.exchange, &self.storage, &ids, self.exchange.now()).await?;
        // the orders are synchronized already: a notification failing doesn't stop the managers
        if let Err(error) = self
            .notify_order_events(&sync.events, &markets, &managers)
            .await
        {
            tracing::error!(%error, "the changes of the orders couldn't be notified");
        }

        let balances = self.exchange.balances().await?;
        self.record_account(&balances, &markets);
        // what the account has, decreased locally when orders are placed
        let mut available: HashMap<String, Decimal> = balances
            .into_iter()
            .map(|(asset, balance)| (asset, balance.available()))
            .collect();

        if let Some(created) = self.add_virtual_manager(&managers, &markets, &available)? {
            managers.push(created);
        }
        if let Some(created) = self.add_free_base_manager(&managers, &markets, &available)? {
            managers.push(created);
        }

        let mut outcomes = HashMap::new();
        for manager in managers.clone() {
            let market = &markets[&self.pair_keys[&manager.pair]];
            let outcome = match self.run_manager(&manager, market, &mut available).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::error!(manager = manager.id, %error, "the manager failed");
                    Outcome::Failed(error.to_string())
                }
            };
            let source = format!("manager {}", manager.id);
            if let Outcome::Failed(error) = &outcome {
                let label = notify::label(manager.id, &market.rules);
                self.notify_failure(&source, Some(&label), error).await;
            } else {
                self.clear_failure(&source);
            }
            outcomes.insert(manager.id, outcome);
        }

        managers.retain(|it| !matches!(outcomes.get(&it.id), Some(Outcome::Deleted(_))));
        // the orders are placed already: the tick is done whatever the summary
        if let Err(error) = self.notify_daily_summary(&markets, &managers).await {
            tracing::error!(%error, "the daily summary couldn't be sent");
        }
        Ok(TickReport { sync, outcomes })
    }
}
