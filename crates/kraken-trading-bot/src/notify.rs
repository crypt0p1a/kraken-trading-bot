//! The notifications of the bot: what changed in its orders, described for `trading-notify`

use crate::order::BotOrder;
use crate::pair::{PairRules, display_asset};
use kraken_api::trading::{OrderSide, OrderStatus};
use trading_notify::messages::{Label, OrderState, OrderView, Side};

/// How an order changed between two synchronizations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderChange {
    /// Entirely filled
    Filled,
    /// More of it was filled, it's still open
    PartiallyFilled,
    /// Expired or canceled, possibly partially filled
    Ended,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderEvent {
    pub change: OrderChange,
    pub order: BotOrder,
}

/// What changed from `previous`, the order known before, to `current`. Nothing for an order not
/// known before: its past (the history of a manager, an order placed by a lost database) isn't news
pub fn change(previous: Option<&BotOrder>, current: &BotOrder) -> Option<OrderChange> {
    let previous = previous?;
    if previous.is_final() {
        return None;
    }
    match current.status {
        OrderStatus::Closed => Some(OrderChange::Filled),
        OrderStatus::Expired | OrderStatus::Canceled => Some(OrderChange::Ended),
        _ if current.volume_executed > previous.volume_executed => {
            Some(OrderChange::PartiallyFilled)
        }
        _ => None,
    }
}

pub fn side(side: OrderSide) -> Side {
    match side {
        OrderSide::Buy => Side::Buy,
        OrderSide::Sell => Side::Sell,
    }
}

pub fn view(order: &BotOrder) -> OrderView {
    OrderView {
        side: side(order.side),
        state: match order.status {
            OrderStatus::Closed => OrderState::Filled,
            OrderStatus::Expired => OrderState::Expired,
            OrderStatus::Canceled => OrderState::Canceled,
            _ => OrderState::Open,
        },
        volume: order.volume,
        executed: order.volume_executed,
        cost: order.cost,
        fee: order.fee,
        average_price: order.average_price,
    }
}

pub fn label(manager_id: i32, rules: &PairRules) -> Label {
    Label {
        manager_id,
        pair: rules.altname.clone(),
        base: display_asset(&rules.base_asset).to_string(),
        quote: display_asset(&rules.quote_asset).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::tests::{executed, order};
    use crate::planner::tests::{d, rules};

    #[test]
    fn tells_what_changed() {
        let open = order(1.0, OrderSide::Buy, OrderStatus::Open, "0.002", "49500");
        let partially = executed(open.clone(), "0.0005", "24.75", "0.06");
        let filled = BotOrder {
            status: OrderStatus::Closed,
            ..executed(open.clone(), "0.002", "99", "0.25")
        };
        let expired = BotOrder {
            status: OrderStatus::Expired,
            ..open.clone()
        };
        let canceled = BotOrder {
            status: OrderStatus::Canceled,
            ..partially.clone()
        };

        assert_eq!(change(Some(&open), &open), None);
        assert_eq!(
            change(Some(&open), &partially),
            Some(OrderChange::PartiallyFilled)
        );
        assert_eq!(change(Some(&partially), &partially), None);
        assert_eq!(change(Some(&partially), &filled), Some(OrderChange::Filled));
        assert_eq!(change(Some(&open), &filled), Some(OrderChange::Filled));
        assert_eq!(change(Some(&open), &expired), Some(OrderChange::Ended));
        assert_eq!(
            change(Some(&partially), &canceled),
            Some(OrderChange::Ended)
        );
        // already final, or unknown before: no news
        assert_eq!(change(Some(&filled), &filled), None);
        assert_eq!(change(None, &filled), None);
    }

    #[test]
    fn describes_the_orders_and_the_pairs() {
        let filled = BotOrder {
            status: OrderStatus::Closed,
            average_price: Some(d("49500")),
            ..executed(
                order(1.0, OrderSide::Sell, OrderStatus::Open, "0.002", "49500"),
                "0.002",
                "99",
                "0.25",
            )
        };
        assert_eq!(
            view(&filled),
            OrderView {
                side: Side::Sell,
                state: OrderState::Filled,
                volume: d("0.002"),
                executed: d("0.002"),
                cost: d("99"),
                fee: d("0.25"),
                average_price: Some(d("49500")),
            }
        );
        assert_eq!(
            label(4, &rules()),
            Label {
                manager_id: 4,
                pair: "XBTUSD".into(),
                base: "XBT".into(),
                quote: "USD".into()
            }
        );
    }
}
