//! The notifications of the bot through ntfy: its start, its failures, the changes of the orders
//! and the daily summary

use super::{Bot, Market};
use crate::BotResult;
use crate::config::{EventKind, ManagerConfig};
use crate::exchange::Exchange;
use crate::ledger;
use crate::notify::{self, OrderChange, OrderEvent};
use crate::order::BotOrder;
use kraken_spot_api::trading::OrderSide;
use std::collections::HashMap;
use trading_notify::messages::{self, Label, ManagerSummary};
use trading_notify::{Catalog, Notification};

/// The day of the last daily summary, in the sync state
const DAILY_SUMMARY: &str = "daily_summary_day";

impl<E: Exchange> Bot<E> {
    pub(super) fn texts(&self) -> &'static Catalog {
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

    pub(super) async fn notify(&self, mut notification: Notification) {
        if self.wants(notification.event) {
            if let Some(label) = &self.account_label {
                notification.title = format!("[{label}] {}", notification.title);
            }
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

    pub(super) async fn notify_order_events(
        &self,
        events: &[OrderEvent],
        markets: &HashMap<String, Market>,
        managers: &[ManagerConfig],
    ) -> BotResult<()> {
        for event in events {
            let order = &event.order;
            // the bot placed it again from the new price: the new buy is notified
            if event.change == OrderChange::Ended
                && self.storage.cancelled_by_the_bot(&order.txid)?
            {
                continue;
            }
            let Some(manager) = managers.iter().find(|it| it.id == order.manager_id) else {
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
                    let entry = ledger::of(manager, &before).entry_price;
                    messages::filled(self.texts(), &label, &notify::view(order), entry)
                }
                OrderChange::Ended => messages::ended(self.texts(), &label, &notify::view(order)),
            };
            self.notify(notification).await;
        }
        Ok(())
    }

    /// Once a day, after the hour of the configuration
    pub(super) async fn notify_daily_summary(
        &self,
        markets: &HashMap<String, Market>,
        managers: &[ManagerConfig],
    ) -> BotResult<()> {
        let Some(notifications) = &self.config.notifications else {
            return Ok(());
        };
        if !notifications.wants(EventKind::DailySummary) || managers.is_empty() {
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
        for manager in managers {
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
}
