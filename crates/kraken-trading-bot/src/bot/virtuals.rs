//! The virtual managers: copies of the managers of the YAML file, added when the account has
//! money or crypto no manager uses, rebuilt from the orders of Kraken, deleted once they hold
//! nothing

use super::{Bot, Market, Outcome};
use crate::BotResult;
use crate::clock::now_millis;
use crate::config::{ManagerConfig, ManagerMode, VirtualId, is_virtual};
use crate::exchange::Exchange;
use crate::ledger;
use crate::planner;
use crate::rebuild;
use crate::storage::{ActionKind, ActionRecord, VirtualManager};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Ticks in a row a virtual manager can't buy for lack of money before it's deleted: a balance
/// seen at a bad time doesn't delete it
pub const STARVED_TICKS: u32 = 3;

impl<E: Exchange> Bot<E> {
    /// Once per database: the virtual managers found in the orders of Kraken it doesn't know,
    /// lost with a previous database. Before any virtual manager is added, not to reuse their ids
    pub(super) async fn rebuild_virtual_managers(&self) -> BotResult<()> {
        if !self.config.virtual_managers || self.storage.state(rebuild::REBUILT)?.is_some() {
            return Ok(());
        }
        let now = self.exchange.now();
        let retry_at = self
            .storage
            .state(rebuild::RETRY_AT)?
            .and_then(|it| it.parse::<f64>().ok());
        if retry_at.is_some_and(|it| now < it) {
            return Ok(());
        }

        // a failure doesn't stop the managers: the search is tried again an hour later
        let failure =
            match rebuild::rebuild(&self.exchange, &self.storage, &self.config, now_millis()).await
            {
                Ok(report) => {
                    if !report.rebuilt.is_empty() {
                        tracing::warn!(
                            rebuilt = report.rebuilt.len(),
                            holding = report.rebuilt.iter().filter(|it| it.holds).count(),
                            "virtual managers rebuilt from the orders of Kraken"
                        );
                    }
                    (report.failed > 0).then(|| {
                        format!(
                            "{} virtual manager(s) couldn't be rebuilt from the orders of Kraken",
                            report.failed
                        )
                    })
                }
                Err(error) => Some(format!(
                    "the orders of Kraken couldn't be searched: {error}"
                )),
            };
        match failure {
            None => {
                self.storage.set_state(rebuild::REBUILT, &now.to_string())?;
                self.clear_failure("rebuild");
            }
            Some(error) => {
                tracing::warn!(%error, "the rebuild of the virtual managers is tried again in an hour");
                self.storage.set_state(
                    rebuild::RETRY_AT,
                    &(now + rebuild::RETRY_SECONDS).to_string(),
                )?;
                self.notify_failure("rebuild", None, &error).await;
            }
        }
        Ok(())
    }

    /// Per quote asset: what the account has available that no manager owns. The quote of the
    /// open buy orders isn't available, the quote a manager has to buy with is its own
    fn margins(
        &self,
        managers: &[ManagerConfig],
        markets: &HashMap<String, Market>,
        available: &HashMap<String, Decimal>,
    ) -> BotResult<HashMap<String, Decimal>> {
        let mut margins = HashMap::new();
        for manager in managers {
            let quote = &markets[&self.pair_keys[&manager.pair]].rules.quote_asset;
            let owned = ledger::of(manager, &self.storage.orders_of(manager.id)?)
                .quote_available
                .max(Decimal::ZERO);
            *margins
                .entry(quote.clone())
                .or_insert_with(|| available.get(quote).copied().unwrap_or_default()) -= owned;
        }
        Ok(margins)
    }

    /// Per base asset: what the account has available that no manager holds. The base of the open
    /// sell orders isn't available, the base a manager hasn't put on sale yet is its own
    fn base_margins(
        &self,
        managers: &[ManagerConfig],
        markets: &HashMap<String, Market>,
        available: &HashMap<String, Decimal>,
    ) -> BotResult<HashMap<String, Decimal>> {
        let mut margins = HashMap::new();
        for manager in managers {
            let base = &markets[&self.pair_keys[&manager.pair]].rules.base_asset;
            let owned = ledger::of(manager, &self.storage.orders_of(manager.id)?)
                .base_available
                .max(Decimal::ZERO);
            *margins
                .entry(base.clone())
                .or_insert_with(|| available.get(base).copied().unwrap_or_default()) -= owned;
        }
        Ok(margins)
    }

    /// With `sell_free_base`, adds a copy of a manager of the YAML file which starts with the
    /// crypto of the account no manager holds, as if its buy had just been filled: what its
    /// budget buys, fees included, at -`buy_below`% of the price of the market. It sells it for
    /// its budget + `sell_above`%, then trades like any manager. The first active manager of the
    /// file the free crypto pays for, among the ones with the fewest copies holding crypto, so
    /// every one gets some: one at most per tick
    pub(super) fn add_free_base_manager(
        &mut self,
        managers: &[ManagerConfig],
        markets: &HashMap<String, Market>,
        available: &HashMap<String, Decimal>,
    ) -> BotResult<Option<ManagerConfig>> {
        if !self.config.virtual_managers || !self.config.sell_free_base {
            return Ok(None);
        }

        let margins = self.base_margins(managers, markets, available)?;
        // what the buy of a manager of the file would have bought, if the free crypto pays for it
        let holding_of = |manager: &ManagerConfig| {
            let market = &markets[&self.pair_keys[&manager.pair]];
            let price = planner::buy_price(market.reference, manager.buy_below, &market.rules);
            let amount =
                planner::buy_amount(manager.budget, price, market.fee_rate, &market.rules)?;
            let free = margins
                .get(&market.rules.base_asset)
                .copied()
                .unwrap_or_default();
            (amount <= free).then_some((amount, price))
        };
        let copies: Vec<i32> = self
            .storage
            .virtual_managers()?
            .into_iter()
            .filter(|it| it.config.holding.is_some())
            .map(|it| it.template_id)
            .collect();
        let candidates: Vec<(&ManagerConfig, usize)> = self
            .config
            .managers
            .iter()
            .filter(|it| it.mode == ManagerMode::Active && holding_of(it).is_some())
            .map(|it| (it, copies.iter().filter(|id| **id == it.id).count()))
            .collect();
        let Some(fewest) = candidates.iter().map(|it| it.1).min() else {
            return Ok(None);
        };
        let Some(template) = candidates
            .into_iter()
            .find(|it| it.1 == fewest)
            .map(|it| it.0.clone())
        else {
            return Ok(None);
        };
        let Some((holding, price)) = holding_of(&template) else {
            return Ok(None);
        };
        let Some(id) = self.next_virtual_id(template.id)? else {
            return Ok(None);
        };

        let market = &markets[&self.pair_keys[&template.pair]];
        let base = &market.rules.base_asset;
        let reason = format!(
            "copy of manager {}, sells {} of the {} {base} held by no manager, as bought at {} (-{}% of {})",
            template.id,
            holding.normalize(),
            margins[base].round_dp(8).normalize(),
            price.normalize(),
            template.buy_below.normalize(),
            market.reference.normalize()
        );
        let config = ManagerConfig {
            id,
            mode: ManagerMode::Active,
            holding: Some(holding),
            ..template.clone()
        };
        self.created(&config, template.id, reason)?;
        Ok(Some(config))
    }

    /// The id of the next virtual manager, a copy of `template`. None when they are used up: no
    /// more virtual managers, the others go on
    fn next_virtual_id(&self, template: i32) -> BotResult<Option<i32>> {
        let sequence = self
            .storage
            .last_virtual_id()?
            .and_then(VirtualId::decode)
            .map_or(0, |it| it.sequence + 1);
        let id = VirtualId { sequence, template }.encode();
        if id.is_none() {
            tracing::warn!(
                sequence,
                "no id left for a virtual manager: none is added anymore"
            );
        }
        Ok(id)
    }

    /// Saves the virtual manager added, and tells it
    fn created(&self, config: &ManagerConfig, template: i32, reason: String) -> BotResult<()> {
        let at = now_millis();
        self.storage.add_virtual_manager(&VirtualManager {
            config: config.clone(),
            template_id: template,
            created_at: at,
            reason: reason.clone(),
        })?;
        self.storage.record_action(&ActionRecord {
            manager_id: config.id,
            at,
            kind: ActionKind::VirtualCreated,
            amount: Some(config.budget),
            price: None,
            reason: Some(reason.clone()),
            dry_run: self.config.dry_run,
            txid: None,
            error: None,
        })?;
        tracing::info!(manager = config.id, template, pair = %config.pair, budget = %config.budget, holding = ?config.holding, %reason, "virtual manager added");
        Ok(())
    }

    /// Adds a copy of a manager of the YAML file, picked at random among the active ones whose
    /// budget the margin of their quote pays for: one at most per tick
    pub(super) fn add_virtual_manager(
        &mut self,
        managers: &[ManagerConfig],
        markets: &HashMap<String, Market>,
        available: &HashMap<String, Decimal>,
    ) -> BotResult<Option<ManagerConfig>> {
        if !self.config.virtual_managers {
            return Ok(None);
        }

        let margins = self.margins(managers, markets, available)?;
        let margin_of = |manager: &ManagerConfig| {
            let quote = &markets[&self.pair_keys[&manager.pair]].rules.quote_asset;
            (
                quote.clone(),
                margins.get(quote).copied().unwrap_or_default(),
            )
        };
        let candidates: Vec<&ManagerConfig> = self
            .config
            .managers
            .iter()
            .filter(|it| it.mode == ManagerMode::Active && margin_of(it).1 >= it.budget)
            .collect();
        if candidates.is_empty() {
            return Ok(None);
        }
        let template = candidates[self.random.usize(..candidates.len())].clone();
        let Some(id) = self.next_virtual_id(template.id)? else {
            return Ok(None);
        };

        let (quote, margin) = margin_of(&template);
        let reason = format!(
            "copy of manager {}, {} {quote} used by no manager",
            template.id,
            margin.round_dp(8).normalize()
        );
        let config = ManagerConfig {
            id,
            mode: ManagerMode::Active,
            ..template.clone()
        };
        self.created(&config, template.id, reason)?;
        Ok(Some(config))
    }

    /// The account can't pay for the buy of the manager, which holds nothing: a virtual manager is
    /// deleted after `STARVED_TICKS` ticks in a row, its money going back to the margin
    pub(super) fn starve(
        &self,
        manager: &ManagerConfig,
        reason: String,
        base: Decimal,
    ) -> BotResult<Outcome> {
        if !self.is_virtual(manager) {
            return self.idle(manager, reason);
        }
        if !base.is_zero() {
            return self.idle(
                manager,
                format!(
                    "{reason}, keeps {} too small to be sold: not deleted",
                    base.normalize()
                ),
            );
        }

        let count = {
            let mut starved = self.starved.lock().unwrap();
            let entry = starved.entry(manager.id).or_insert((0, 0));
            *entry = if entry.0 + 1 == self.ticks {
                (self.ticks, entry.1 + 1)
            } else {
                (self.ticks, 1)
            };
            entry.1
        };
        if count < STARVED_TICKS {
            return self.idle(manager, format!("{reason} ({count}/{STARVED_TICKS})"));
        }

        self.delete_virtual(manager, format!("{reason}, {count} ticks in a row"))
    }

    pub(super) fn is_virtual(&self, manager: &ManagerConfig) -> bool {
        self.config.virtual_managers && is_virtual(manager.id)
    }

    /// Deletes a virtual manager which holds nothing: its id is never reused
    pub(super) fn delete_virtual(
        &self,
        manager: &ManagerConfig,
        reason: String,
    ) -> BotResult<Outcome> {
        let at = now_millis();
        self.storage
            .delete_virtual_manager(manager.id, at, &reason)?;
        self.storage.record_action(&ActionRecord {
            manager_id: manager.id,
            at,
            kind: ActionKind::VirtualDeleted,
            amount: None,
            price: None,
            reason: Some(reason.clone()),
            dry_run: self.config.dry_run,
            txid: None,
            error: None,
        })?;
        self.starved.lock().unwrap().remove(&manager.id);
        tracing::info!(manager = manager.id, %reason, "virtual manager deleted");
        Ok(Outcome::Deleted(reason))
    }
}
