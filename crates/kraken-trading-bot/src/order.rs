//! The orders of the managers, as the bot uses them

use kraken_api::trading::{Order, OrderSide, OrderStatus};
use rust_decimal::Decimal;

/// An order of a manager. With the `fciq` flag, the fees are in the quote currency
#[derive(Debug, Clone, PartialEq)]
pub struct BotOrder {
    pub txid: String,
    /// The manager: Kraken userref of the order
    pub manager_id: i32,
    /// Alternate name of the pair, e.g. "XBTUSD"
    pub pair: String,
    pub side: OrderSide,
    pub status: OrderStatus,
    /// Volume ordered, in the base
    pub volume: Decimal,
    /// Volume executed, in the base
    pub volume_executed: Decimal,
    /// Quote paid (buy) or received (sell), fees excluded
    pub cost: Decimal,
    /// Fees, in the quote
    pub fee: Decimal,
    /// Price of the limit order
    pub limit_price: Option<Decimal>,
    /// Average price of the executions
    pub average_price: Option<Decimal>,
    /// UTC timestamps, in seconds
    pub open_time: f64,
    pub close_time: Option<f64>,
    pub expire_time: Option<f64>,
}

impl BotOrder {
    /// The order won't change anymore
    pub fn is_final(&self) -> bool {
        self.status.is_final()
    }

    /// The order of Kraken, None when it doesn't belong to a manager
    pub fn from_kraken(order: &Order) -> Option<Self> {
        let manager_id = i32::try_from(order.userref?).ok().filter(|it| *it > 0)?;
        let positive = |value: Option<Decimal>| value.filter(|it| *it > Decimal::ZERO);

        Some(Self {
            txid: order.txid.clone(),
            manager_id,
            pair: order.description.pair.clone(),
            side: order.description.side,
            status: order.status,
            volume: order.volume,
            volume_executed: order.volume_executed,
            cost: order.cost,
            fee: order.fee,
            limit_price: positive(order.description.price),
            average_price: positive(Some(order.average_price)),
            open_time: order.open_time,
            close_time: order.close_time.filter(|it| *it > 0.0),
            expire_time: order.expire_time.filter(|it| *it > 0.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn kraken(userref: serde_json::Value) -> Order {
        let mut order: Order = serde_json::from_value(serde_json::json!({
            "userref": userref, "status": "closed", "opentm": 1761900000.25, "closetm": 1761900060.5,
            "starttm": 0, "expiretm": 0,
            "descr": {"pair": "XBTUSD", "type": "buy", "ordertype": "limit", "price": "50000.0",
                "price2": "0", "order": "buy 0.00100000 XBTUSD @ limit 50000.0"},
            "vol": "0.00100000", "vol_exec": "0.00058823", "cost": "29.41150", "fee": "0.07647",
            "price": "50000.0", "misc": "", "oflags": "fciq"
        }))
        .unwrap();
        order.txid = "OAAAAA-BBBBB-CCCCCC".into();
        order
    }

    #[test]
    fn reads_the_orders_of_the_managers() {
        let order = BotOrder::from_kraken(&kraken(serde_json::json!(7))).unwrap();
        let d = |value: &str| Decimal::from_str(value).unwrap();

        assert_eq!(order.manager_id, 7);
        assert_eq!(order.txid, "OAAAAA-BBBBB-CCCCCC");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.volume, d("0.001"));
        assert_eq!(order.volume_executed, d("0.00058823"));
        assert_eq!(order.cost, d("29.4115"));
        assert_eq!(order.fee, d("0.07647"));
        assert_eq!(order.limit_price, Some(d("50000")));
        assert_eq!(order.close_time, Some(1761900060.5));
        assert_eq!(order.expire_time, None);
        assert!(order.is_final());
    }

    #[test]
    fn ignores_the_orders_of_no_manager() {
        assert_eq!(
            BotOrder::from_kraken(&kraken(serde_json::Value::Null)),
            None
        );
        assert_eq!(BotOrder::from_kraken(&kraken(serde_json::json!(0))), None);
        assert_eq!(BotOrder::from_kraken(&kraken(serde_json::json!(-4))), None);
        assert_eq!(
            BotOrder::from_kraken(&kraken(serde_json::json!(4_000_000_000_i64))),
            None
        );
    }
}
