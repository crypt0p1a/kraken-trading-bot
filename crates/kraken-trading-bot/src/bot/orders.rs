//! The orders the bot places: plain limit orders only, checked before they reach Kraken, saved
//! and notified once accepted

use super::{Bot, Market, Outcome};
use crate::clock::now_millis;
use crate::config::ManagerConfig;
use crate::exchange::Exchange;
use crate::ledger::Balances;
use crate::notify;
use crate::order::BotOrder;
use crate::storage::ActionRecord;
use crate::{BotError, BotResult};
use kraken_spot_api::trading::{
    NewOrder, OrderFlag, OrderSide, OrderStatus, OrderType, TimeInForce,
};
use rust_decimal::Decimal;
use trading_notify::messages;

impl<E: Exchange> Bot<E> {
    pub(super) async fn place(
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
                    if side == OrderSide::Buy {
                        self.storage.set_order_reference(txid, market.reference)?;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_spot_api::trading::Trigger;

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
