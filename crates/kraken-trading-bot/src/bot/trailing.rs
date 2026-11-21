//! The trailing of the buys: an open buy cancelled once the price has risen too far above the
//! price it was placed from, to be placed again from the new price

use super::{Bot, Market, Outcome};
use crate::BotResult;
use crate::clock::now_millis;
use crate::config::ManagerConfig;
use crate::decimal::percent;
use crate::exchange::Exchange;
use crate::order::BotOrder;
use crate::storage::ActionRecord;
use kraken_spot_api::trading::OrderSide;
use rust_decimal::Decimal;

impl<E: Exchange> Bot<E> {
    /// Cancels the open buy, not filled at all, once the price is `buy_trailing`% above the price
    /// it was placed from. The next tick sees it cancelled, and places the buy again from the new
    /// price: nothing is bought twice, even if the buy was filled meanwhile
    pub(super) async fn trail_buy(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        open: &[&BotOrder],
    ) -> BotResult<Option<Outcome>> {
        let Some(trailing) = self.config.buy_trailing else {
            return Ok(None);
        };
        // a dry run never changes the orders on Kraken: an open buy left by a live run stays
        if self.config.dry_run {
            return Ok(None);
        }
        let Some((order, limit)) = open
            .iter()
            .filter(|it| it.side == OrderSide::Buy && it.volume_executed.is_zero())
            .find_map(|it| it.limit_price.map(|limit| (it, limit)))
        else {
            return Ok(None);
        };
        let placed_from = match self.storage.order_reference(&order.txid)? {
            Some(reference) => reference,
            // placed before the references were recorded: the highest price it can have been
            // placed from, its limit having been rounded down to the tick of the pair
            None => {
                let below = Decimal::ONE - percent(manager.buy_below);
                if below <= Decimal::ZERO {
                    return Ok(None);
                }
                (limit + market.rules.tick_size) / below
            }
        };
        if market.reference <= placed_from * (Decimal::ONE + percent(trailing)) {
            return Ok(None);
        }

        let reason = format!(
            "price {} more than {trailing}% above {}, the price the buy was placed from",
            market.reference.normalize(),
            placed_from.round_dp(8).normalize()
        );
        let mut record = ActionRecord {
            manager_id: manager.id,
            at: now_millis(),
            kind: "buy_cancelled".into(),
            amount: Some(order.volume),
            price: Some(limit),
            reason: Some(reason.clone()),
            dry_run: self.config.dry_run,
            txid: Some(order.txid.clone()),
            error: None,
        };
        match self.exchange.cancel_order(&order.txid).await {
            Ok(true) => {
                tracing::info!(manager = manager.id, txid = %order.txid, %reason, "buy cancelled");
                self.storage.record_action(&record)?;
                Ok(Some(Outcome::Cancelled(reason)))
            }
            // closed meanwhile: the next synchronization tells
            Ok(false) => Ok(None),
            // gone from the open orders meanwhile: the next synchronization tells
            Err(error) if is_unknown_order(&error) => Ok(None),
            // a permission, a rate limit, the network: a failure of the manager, notified once,
            // recorded when it changes
            Err(error) => {
                tracing::warn!(manager = manager.id, txid = %order.txid, %error, "the buy couldn't be cancelled");
                record.error = Some(error.to_string());
                let last = self.storage.actions_of(manager.id, 1)?;
                let repeated = last
                    .first()
                    .is_some_and(|it| it.kind == record.kind && it.error == record.error);
                if !repeated {
                    self.storage.record_action(&record)?;
                }
                Ok(Some(Outcome::Failed(format!(
                    "the buy couldn't be cancelled: {error}"
                ))))
            }
        }
    }
}

/// Kraken doesn't know the order as an open one
fn is_unknown_order(error: &kraken_spot_api::KrakenError) -> bool {
    matches!(error, kraken_spot_api::KrakenError::Api { body, .. } if body.contains("EOrder:Unknown order"))
}
