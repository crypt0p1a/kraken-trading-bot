//! What a manager does at a tick: its state saved, then its open order waited for, or the action
//! of the strategy taken

use super::{Bot, Market, Outcome};
use crate::BotResult;
use crate::clock::now_millis;
use crate::config::{ManagerConfig, ManagerMode};
use crate::exchange::Exchange;
use crate::ledger;
use crate::order::BotOrder;
use crate::storage::{ActionKind, ActionRecord, ManagerState};
use crate::strategy::{self, Action, Decision};
use kraken_spot_api::trading::OrderSide;
use rust_decimal::Decimal;
use std::collections::HashMap;

impl<E: Exchange> Bot<E> {
    /// The managers the database doesn't have yet, before the synchronization of the orders: with
    /// several accounts sharing the limit of the calls to Kraken, it takes minutes, and the
    /// dashboard would show nothing of a new account until then
    pub(super) fn save_new_managers(
        &self,
        managers: &[ManagerConfig],
        markets: &HashMap<String, Market>,
    ) -> BotResult<()> {
        for manager in managers {
            if self.storage.manager_balances(manager.id)?.is_none() {
                let market = &markets[&self.pair_keys[&manager.pair]];
                let orders = self.storage.orders_of(manager.id)?;
                self.storage
                    .save_new_manager(&manager_state(manager, market, &orders))?;
            }
        }
        Ok(())
    }

    pub(super) async fn run_manager(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        available: &mut HashMap<String, Decimal>,
    ) -> BotResult<Outcome> {
        let orders = self.storage.orders_of(manager.id)?;
        let state = manager_state(manager, market, &orders);
        let balances = state.balances.clone();
        self.storage.save_manager(&state)?;
        log_balances(manager, &state);

        let open: Vec<&BotOrder> = orders.iter().filter(|it| !it.is_final()).collect();
        if let Some(outcome) = self.trail_buy(manager, market, &open).await? {
            return Ok(outcome);
        }
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

        // too small to be sold, a virtual manager isn't deleted with it: it's still its own
        let base = balances.base_available + balances.base_in_sell_order;
        match action {
            Action::Wait => Ok(Outcome::Waited),
            // closing, with nothing left: a virtual manager is done
            Action::Idle(reason)
                if self.is_virtual(manager)
                    && manager.mode == ManagerMode::Closing
                    && base.is_zero() =>
            {
                self.delete_virtual(manager, format!("{reason}, nothing left"))
            }
            Action::Idle(reason) => self.idle(manager, reason),
            Action::Sell { amount, price } => {
                let base_asset = &market.rules.base_asset;
                let in_account = available.get(base_asset).copied().unwrap_or_default();
                // the crypto it started with, gone from the account (withdrawn): a copy which
                // never traded holds nothing of its own, it's deleted like one which can't buy
                if manager.holding.is_some()
                    && amount > in_account
                    && orders.iter().all(|it| it.volume_executed.is_zero())
                {
                    let reason = format!(
                        "the account has {} {base_asset} available, {} to sell",
                        in_account.normalize(),
                        amount.normalize()
                    );
                    return self.starve(manager, reason, Decimal::ZERO);
                }
                let outcome = self
                    .place(manager, market, OrderSide::Sell, amount, price, &balances)
                    .await?;
                if matches!(outcome, Outcome::Placed { .. }) {
                    available.insert(base_asset.clone(), in_account - amount);
                }
                Ok(outcome)
            }
            Action::Buy { amount, price } => {
                let cost = amount * price * (Decimal::ONE + market.fee_rate);
                let in_account = available
                    .get(&market.rules.quote_asset)
                    .copied()
                    .unwrap_or_default();
                if cost > in_account {
                    let reason = format!(
                        "the account has {} {} available, {} required",
                        in_account.normalize(),
                        market.rules.quote_asset,
                        cost.round_dp(8).normalize()
                    );
                    return self.starve(manager, reason, base);
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
    pub(super) fn idle(&self, manager: &ManagerConfig, reason: String) -> BotResult<Outcome> {
        tracing::info!(manager = manager.id, %reason, "idle");
        let last = self.storage.actions_of(manager.id, 1)?;
        let repeated = last
            .first()
            .is_some_and(|it| it.kind == ActionKind::Idle && it.reason.as_deref() == Some(&reason));
        if !repeated {
            self.storage.record_action(&ActionRecord {
                manager_id: manager.id,
                at: now_millis(),
                kind: ActionKind::Idle,
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
}

/// What the manager has now, from its orders
fn manager_state(manager: &ManagerConfig, market: &Market, orders: &[BotOrder]) -> ManagerState {
    ManagerState {
        config: manager.clone(),
        balances: ledger::of(manager, orders),
        last_price: market.reference,
        at: now_millis(),
    }
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
