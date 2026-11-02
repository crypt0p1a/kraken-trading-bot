//! A tick of the bot: synchronizes the orders, then each manager decides and acts. Port of the
//! ManagersPool of cex-trading-bot

use crate::clock::now_millis;
use crate::config::{BotConfig, EventKind, ManagerConfig, ManagerMode};
use crate::decimal::percent;
use crate::exchange::Exchange;
use crate::ledger::{self, Balances};
use crate::notify::{self, OrderChange, OrderEvent};
use crate::order::BotOrder;
use crate::pair::PairRules;
use crate::storage::{ActionRecord, ManagerState, Storage, TickerRecord};
use crate::strategy::{self, Action, Decision};
use crate::sync::{self, SyncReport};
use crate::{BotError, BotResult};
use kraken_api::trading::{NewOrder, OrderFlag, OrderSide, OrderStatus, OrderType, TimeInForce};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Mutex;
use trading_notify::messages::{self, Label, ManagerSummary};
use trading_notify::{Catalog, Notification, Notifier};

/// The day of the last daily summary, in the sync state
const DAILY_SUMMARY: &str = "daily_summary_day";

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
    /// The failure last notified, per source: a failure repeated at each tick is notified once
    failures: Mutex<HashMap<String, String>>,
}

impl<E: Exchange> Bot<E> {
    pub fn new(exchange: E, storage: Storage, config: BotConfig) -> Self {
        Self {
            exchange,
            storage,
            config,
            pair_keys: HashMap::new(),
            notifier: Notifier::Silent,
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Where the notifications of the configuration go
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = notifier;
        self
    }

    fn texts(&self) -> &'static Catalog {
        self.config
            .notifications
            .as_ref()
            .map(|it| it.language)
            .unwrap_or_default()
            .catalog()
    }

    fn wants(&self, event: EventKind) -> bool {
        self.config
            .notifications
            .as_ref()
            .is_some_and(|it| it.wants(event))
    }

    async fn notify(&self, notification: Notification) {
        if self.wants(notification.event) {
            self.notifier.send(notification).await;
        }
    }

    pub async fn notify_started(&self) {
        self.notify(messages::started(
            self.texts(),
            self.config.managers.len(),
            self.config.dry_run,
        ))
        .await;
    }

    /// Notifies a failure of `source`, unless it's the one already notified
    pub async fn notify_failure(&self, source: &str, label: Option<&Label>, error: &str) {
        let new = {
            let mut failures = self.failures.lock().unwrap();
            let known = failures.get(source).is_some_and(|it| it == error);
            failures.insert(source.to_string(), error.to_string());
            !known
        };
        if new {
            self.notify(messages::failed(self.texts(), label, error))
                .await;
        }
    }

    /// `source` works again: its next failure will be notified
    pub fn clear_failure(&self, source: &str) {
        self.failures.lock().unwrap().remove(source);
    }

    async fn notify_order_events(
        &self,
        events: &[OrderEvent],
        markets: &HashMap<String, Market>,
    ) -> BotResult<()> {
        for event in events {
            let order = &event.order;
            let Some(manager) = self
                .config
                .managers
                .iter()
                .find(|it| it.id == order.manager_id)
            else {
                continue;
            };
            let market = &markets[&self.pair_keys[&manager.pair]];
            let label = notify::label(manager.id, &market.rules);
            let notification = match event.change {
                OrderChange::Filled | OrderChange::PartiallyFilled => {
                    // what the base sold had cost: the position before the sale
                    let before: Vec<BotOrder> = self
                        .storage
                        .orders_of(manager.id)?
                        .into_iter()
                        .filter(|it| it.open_time <= order.open_time && it.txid != order.txid)
                        .collect();
                    let entry = ledger::compute(manager.budget, &before).entry_price;
                    messages::filled(self.texts(), &label, &notify::view(order), entry)
                }
                OrderChange::Ended => messages::ended(self.texts(), &label, &notify::view(order)),
            };
            self.notify(notification).await;
        }
        Ok(())
    }

    /// Once a day, after the hour of the configuration
    async fn notify_daily_summary(&self, markets: &HashMap<String, Market>) -> BotResult<()> {
        let Some(notifications) = &self.config.notifications else {
            return Ok(());
        };
        if !notifications.wants(EventKind::DailySummary) || self.config.managers.is_empty() {
            return Ok(());
        }
        let now = self.exchange.now();
        let day = (now / 86_400.0).floor() as i64;
        let hour = ((now.rem_euclid(86_400.0)) / 3600.0).floor() as u32;
        if hour < notifications.daily_summary_hour
            || self.storage.state(DAILY_SUMMARY)? == Some(day.to_string())
        {
            return Ok(());
        }

        let mut summaries = Vec::new();
        for manager in &self.config.managers {
            let market = &markets[&self.pair_keys[&manager.pair]];
            let value = match self.storage.manager_balances(manager.id)? {
                Some((balances, price, _)) => balances.value(price),
                None => manager.budget,
            };
            let sales = self
                .storage
                .orders_of(manager.id)?
                .iter()
                .filter(|it| it.side == OrderSide::Sell && !it.volume_executed.is_zero())
                .filter(|it| it.close_time.is_some_and(|time| time > now - 86_400.0))
                .count();
            summaries.push(ManagerSummary {
                label: notify::label(manager.id, &market.rules),
                budget: manager.budget,
                value,
                sales,
            });
        }

        self.notify(messages::daily_summary(self.texts(), &summaries))
            .await;
        self.storage.set_state(DAILY_SUMMARY, &day.to_string())?;
        Ok(())
    }

    pub fn exchange(&self) -> &E {
        &self.exchange
    }

    pub fn into_exchange(self) -> E {
        self.exchange
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
        for manager in &self.config.managers {
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
                .is_some_and(|it| it != kraken_api::market::AssetPairStatus::Online)
            {
                tracing::warn!(manager = manager.id, pair = %pair.altname, status = ?pair.status, "the pair isn't online");
            }
            self.pair_keys.insert(manager.pair.clone(), key.clone());
        }
        Ok(())
    }

    async fn markets(&self) -> BotResult<HashMap<String, Market>> {
        let mut keys: Vec<String> = self.pair_keys.values().cloned().collect();
        keys.sort();
        keys.dedup();

        let pairs = self.exchange.asset_pairs(&keys).await?;
        let tickers = self.exchange.tickers(&keys).await?;
        let fees = self.exchange.trade_volume(&keys).await?;
        let at = now_millis();

        let mut markets = HashMap::new();
        for key in keys {
            let pair = pairs
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no information for {key}")))?;
            let ticker = tickers
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no ticker for {key}")))?;
            // the taker fee: the highest one, whatever the order becomes
            let fee = fees
                .fees
                .get(&key)
                .map(|it| it.fee)
                .ok_or_else(|| BotError::Config(format!("no fee for {key}")))?;

            self.storage.record_ticker(&TickerRecord {
                pair: key.clone(),
                at,
                last: ticker.last_trade.price,
                bid: ticker.bid.price,
                ask: ticker.ask.price,
                volume_24h: ticker.volume.last_24h,
                vwap_24h: ticker.vwap.last_24h,
            })?;

            markets.insert(
                key.clone(),
                Market {
                    rules: PairRules::from_asset_pair(&key, pair),
                    reference: ticker.last_trade.price,
                    fee_rate: percent(fee),
                },
            );
        }
        Ok(markets)
    }

    pub async fn tick(&mut self) -> BotResult<TickReport> {
        if self.pair_keys.len() < self.config.managers.len() {
            self.prepare().await?;
        }

        let markets = self.markets().await?;
        let sync = sync::synchronize(
            &self.exchange,
            &self.storage,
            &self.config.ids(),
            self.exchange.now(),
        )
        .await?;
        self.notify_order_events(&sync.events, &markets).await?;

        // what the account has, decreased locally when orders are placed
        let mut available: HashMap<String, Decimal> = self
            .exchange
            .balances()
            .await?
            .into_iter()
            .map(|(asset, balance)| (asset, balance.available()))
            .collect();

        let mut outcomes = HashMap::new();
        for manager in self.config.managers.clone() {
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

        self.notify_daily_summary(&markets).await?;
        Ok(TickReport { sync, outcomes })
    }

    async fn run_manager(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        available: &mut HashMap<String, Decimal>,
    ) -> BotResult<Outcome> {
        let orders = self.storage.orders_of(manager.id)?;
        let balances = ledger::compute(manager.budget, &orders);
        let state = ManagerState {
            config: manager.clone(),
            balances: balances.clone(),
            last_price: market.reference,
            at: now_millis(),
        };
        self.storage.save_manager(&state)?;
        log_balances(manager, &state);

        let open: Vec<&BotOrder> = orders.iter().filter(|it| !it.is_final()).collect();
        for order in &open {
            tracing::info!(manager = manager.id, txid = %order.txid, side = ?order.side, "waiting for its open order");
        }

        let action = strategy::decide(&Decision {
            balances: &balances,
            has_open_order: !open.is_empty(),
            active: manager.mode == ManagerMode::Active,
            buy_below: manager.buy_below,
            sell_above: manager.sell_above,
            reference: market.reference,
            fee_rate: market.fee_rate,
            rules: &market.rules,
        });

        match action {
            Action::Wait => Ok(Outcome::Waited),
            Action::Idle(reason) => self.idle(manager, reason),
            Action::Sell { amount, price } => {
                self.place(manager, market, OrderSide::Sell, amount, price, &balances)
                    .await
            }
            Action::Buy { amount, price } => {
                let cost = amount * price * (Decimal::ONE + market.fee_rate);
                let in_account = available
                    .get(&market.rules.quote_asset)
                    .copied()
                    .unwrap_or_default();
                if cost > in_account {
                    return self.idle(
                        manager,
                        format!(
                            "the account has {} {} available, {} required",
                            in_account.normalize(),
                            market.rules.quote_asset,
                            cost.round_dp(8).normalize()
                        ),
                    );
                }
                let outcome = self
                    .place(manager, market, OrderSide::Buy, amount, price, &balances)
                    .await?;
                if matches!(outcome, Outcome::Placed { .. }) {
                    available.insert(market.rules.quote_asset.clone(), in_account - cost);
                }
                Ok(outcome)
            }
        }
    }

    /// Records the reason, unless it's the same as the last action of the manager
    fn idle(&self, manager: &ManagerConfig, reason: String) -> BotResult<Outcome> {
        tracing::info!(manager = manager.id, %reason, "idle");
        let last = self.storage.actions_of(manager.id, 1)?;
        let repeated = last
            .first()
            .is_some_and(|it| it.kind == "idle" && it.reason.as_deref() == Some(&reason));
        if !repeated {
            self.storage.record_action(&ActionRecord {
                manager_id: manager.id,
                at: now_millis(),
                kind: "idle".into(),
                amount: None,
                price: None,
                reason: Some(reason.clone()),
                dry_run: self.config.dry_run,
                txid: None,
                error: None,
            })?;
        }
        Ok(Outcome::Idle(reason))
    }

    async fn place(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        balances: &Balances,
    ) -> BotResult<Outcome> {
        let dry_run = self.config.dry_run;
        let (time_in_force, expire_time, reason) = match side {
            OrderSide::Buy => (
                TimeInForce::Gtd,
                Some(format!("+{}", self.config.buy_order_timeout.as_secs())),
                format!("reference {} -{}%", market.reference, manager.buy_below),
            ),
            OrderSide::Sell => (
                TimeInForce::Gtc,
                None,
                format!(
                    "cost {} +{}%, fee {}",
                    balances.position_cost.round_dp(8).normalize(),
                    manager.sell_above,
                    market.fee_rate.normalize()
                ),
            ),
        };

        let request = NewOrder {
            pair: market.rules.altname.clone(),
            side,
            order_type: OrderType::Limit,
            volume: amount.normalize().to_string(),
            price: Some(price.normalize().to_string()),
            price2: None,
            // the fees in the quote currency: what the ledger expects
            oflags: Some(vec![OrderFlag::Fciq]),
            time_in_force: Some(time_in_force),
            start_time: None,
            expire_time,
            cl_ord_id: None,
            userref: Some(manager.id),
            validate: dry_run.then_some(true),
            display_volume: None,
            asset_class: None,
            trigger: None,
            leverage: None,
            reduce_only: None,
            self_trade_prevention: None,
            deadline: None,
            broker: None,
        };
        tracing::info!(manager = manager.id, ?side, %amount, %price, dry_run, "placing an order");

        let kind = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let mut record = ActionRecord {
            manager_id: manager.id,
            at: now_millis(),
            kind: kind.into(),
            amount: Some(amount),
            price: Some(price),
            reason: Some(reason),
            dry_run,
            txid: None,
            error: None,
        };

        if let Err(error) = ensure_limit(&request) {
            record.error = Some(error.clone());
            self.storage.record_action(&record)?;
            return Err(BotError::Config(error));
        }

        match self.exchange.new_order(&request).await {
            Ok(answer) => {
                let txid = answer.txids.first().cloned();
                if let Some(txid) = &txid {
                    // saved right away: the next tick doesn't place another order
                    self.storage.upsert_orders(&[BotOrder {
                        txid: txid.clone(),
                        manager_id: manager.id,
                        pair: market.rules.altname.clone(),
                        side,
                        status: OrderStatus::Open,
                        volume: amount,
                        volume_executed: Decimal::ZERO,
                        cost: Decimal::ZERO,
                        fee: Decimal::ZERO,
                        limit_price: Some(price),
                        average_price: None,
                        open_time: self.exchange.now(),
                        close_time: None,
                        expire_time: None,
                    }])?;
                }
                tracing::info!(manager = manager.id, order = %answer.description.order, ?txid, dry_run, "order accepted");
                let notify_dry_run = self
                    .config
                    .notifications
                    .as_ref()
                    .is_some_and(|it| it.dry_run);
                if !dry_run || notify_dry_run {
                    let expires = (side == OrderSide::Buy).then_some(self.config.buy_order_timeout);
                    let label = notify::label(manager.id, &market.rules);
                    self.notify(messages::placed(
                        self.texts(),
                        &label,
                        notify::side(side),
                        amount,
                        price,
                        dry_run,
                        expires,
                    ))
                    .await;
                }
                record.txid = txid.clone();
                self.storage.record_action(&record)?;
                Ok(Outcome::Placed {
                    side,
                    amount,
                    price,
                    txid,
                })
            }
            Err(error) => {
                tracing::error!(manager = manager.id, %error, "order refused");
                record.error = Some(error.to_string());
                self.storage.record_action(&record)?;
                Ok(Outcome::Failed(error.to_string()))
            }
        }
    }
}

/// The bot only trades with limit orders, at the price it computed, with the fees in the quote:
/// any other order is refused before reaching Kraken
pub fn ensure_limit(order: &NewOrder) -> Result<(), String> {
    if order.order_type != OrderType::Limit {
        return Err(format!(
            "refused: a {:?} order, the bot only places limit orders",
            order.order_type
        ));
    }
    let price = order
        .price
        .as_deref()
        .and_then(|it| it.parse::<Decimal>().ok());
    if price.is_none_or(|it| it <= Decimal::ZERO) {
        return Err(format!(
            "refused: a limit order without a valid price ({:?})",
            order.price
        ));
    }
    if order.price2.is_some() {
        return Err("refused: a second price, the bot only places plain limit orders".into());
    }
    if !order
        .oflags
        .as_ref()
        .is_some_and(|it| it.contains(&OrderFlag::Fciq))
    {
        return Err(
            "refused: an order without fciq, the ledger expects the fees in the quote".into(),
        );
    }
    if order
        .oflags
        .as_ref()
        .is_some_and(|it| it.contains(&OrderFlag::Viqc))
    {
        return Err("refused: a volume in the quote (viqc), only for market orders".into());
    }
    // the parameters of the margin, iceberg and triggered orders
    if order.leverage.is_some() || order.reduce_only.is_some() {
        return Err("refused: a margin order, the bot only trades its own funds".into());
    }
    if order.display_volume.is_some() {
        return Err("refused: a displayed volume, only for iceberg orders".into());
    }
    if order.trigger.is_some() {
        return Err("refused: a trigger, only for stop-loss and take-profit orders".into());
    }
    Ok(())
}

fn log_balances(manager: &ManagerConfig, state: &ManagerState) {
    let balances = &state.balances;
    tracing::info!(
        manager = manager.id,
        pair = %manager.pair,
        quote = %balances.quote_available.round_dp(8).normalize(),
        quote_in_buy = %balances.quote_in_buy_order.round_dp(8).normalize(),
        base = %balances.base_available.normalize(),
        base_in_sell = %balances.base_in_sell_order.normalize(),
        entry = ?balances.entry_price.map(|it| it.round_dp(8).normalize()),
        value = %state.value().round_dp(8).normalize(),
        budget = %manager.budget,
        "balances"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_api::trading::Trigger;

    fn limit() -> NewOrder {
        NewOrder {
            pair: "XBTUSD".into(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            volume: "0.001".into(),
            price: Some("49500".into()),
            price2: None,
            oflags: Some(vec![OrderFlag::Fciq]),
            time_in_force: Some(TimeInForce::Gtd),
            start_time: None,
            expire_time: Some("+43200".into()),
            cl_ord_id: None,
            userref: Some(1),
            validate: None,
            display_volume: None,
            asset_class: None,
            trigger: None,
            leverage: None,
            reduce_only: None,
            self_trade_prevention: None,
            deadline: None,
            broker: None,
        }
    }

    #[test]
    fn accepts_the_limit_orders_of_the_bot() {
        assert_eq!(ensure_limit(&limit()), Ok(()));
        let sell = NewOrder {
            side: OrderSide::Sell,
            time_in_force: Some(TimeInForce::Gtc),
            expire_time: None,
            ..limit()
        };
        assert_eq!(ensure_limit(&sell), Ok(()));
    }

    #[test]
    fn refuses_every_order_which_is_not_a_plain_limit_order() {
        let refused = [
            NewOrder {
                order_type: OrderType::Market,
                price: None,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Market,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::StopLoss,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TakeProfit,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::StopLossLimit,
                price2: Some("49000".into()),
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TakeProfitLimit,
                price2: Some("51000".into()),
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TrailingStop,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TrailingStopLimit,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Iceberg,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::SettlePosition,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Unknown,
                ..limit()
            },
            NewOrder {
                price: None,
                ..limit()
            },
            NewOrder {
                price: Some("0".into()),
                ..limit()
            },
            NewOrder {
                price: Some("-1".into()),
                ..limit()
            },
            NewOrder {
                price: Some("market".into()),
                ..limit()
            },
            NewOrder {
                price2: Some("1".into()),
                ..limit()
            },
            NewOrder {
                oflags: None,
                ..limit()
            },
            NewOrder {
                oflags: Some(vec![OrderFlag::Fcib]),
                ..limit()
            },
            NewOrder {
                oflags: Some(vec![OrderFlag::Fciq, OrderFlag::Viqc]),
                ..limit()
            },
            NewOrder {
                leverage: Some("2:1".into()),
                ..limit()
            },
            NewOrder {
                reduce_only: Some(true),
                ..limit()
            },
            NewOrder {
                display_volume: Some("0.0001".into()),
                ..limit()
            },
            NewOrder {
                trigger: Some(Trigger::Last),
                ..limit()
            },
        ];
        for order in refused {
            assert!(ensure_limit(&order).is_err(), "{order:?}");
        }
    }
}
