//! What a manager has, computed from its budget and the executions of its orders. Port of the
//! ManagerLedger of cex-trading-bot, the fees being in the quote (`fciq`)

use crate::order::BotOrder;
use kraken_spot_api::trading::OrderSide;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq)]
pub struct Balances {
    /// Quote available to buy, not including what an open buy order holds
    pub quote_available: Decimal,
    /// Quote held by the open buy order
    pub quote_in_buy_order: Decimal,
    /// Base available to sell, not including what an open sell order holds
    pub base_available: Decimal,
    /// Base held by the open sell order
    pub base_in_sell_order: Decimal,
    /// What the base currently held cost, fees included
    pub position_cost: Decimal,
    /// Average price, fees included, of the base currently held. None when nothing is held
    pub entry_price: Option<Decimal>,
}

impl Balances {
    /// Everything the manager has, valued at `price`
    pub fn value(&self, price: Decimal) -> Decimal {
        self.quote_available
            + self.quote_in_buy_order
            + (self.base_available + self.base_in_sell_order) * price
    }
}

pub fn compute(budget: Decimal, orders: &[BotOrder]) -> Balances {
    let mut quote = budget;
    let mut base = Decimal::ZERO;
    let mut quote_in_buy_order = Decimal::ZERO;
    let mut base_in_sell_order = Decimal::ZERO;

    // the position is what has been bought and not sold yet, with what it cost
    let mut position_base = Decimal::ZERO;
    let mut position_cost = Decimal::ZERO;

    let mut sorted: Vec<&BotOrder> = orders.iter().collect();
    sorted.sort_by(|a, b| {
        a.open_time
            .total_cmp(&b.open_time)
            .then_with(|| a.txid.cmp(&b.txid))
    });

    for order in sorted {
        let executed = order.volume_executed;

        match order.side {
            OrderSide::Buy => {
                let spent = order.cost + order.fee;
                quote -= spent;
                base += executed;
                position_base += executed;
                position_cost += spent;
            }
            OrderSide::Sell => {
                quote += order.cost - order.fee;
                base -= executed;

                if executed >= position_base {
                    position_base = Decimal::ZERO;
                    position_cost = Decimal::ZERO;
                } else if executed > Decimal::ZERO {
                    position_cost *= (position_base - executed) / position_base;
                    position_base -= executed;
                }
            }
        }

        if !order.is_final() {
            let remaining = order.volume - executed;
            if remaining > Decimal::ZERO {
                match order.side {
                    OrderSide::Buy => {
                        let held = remaining * order.limit_price.unwrap_or_default();
                        quote -= held;
                        quote_in_buy_order += held;
                    }
                    OrderSide::Sell => {
                        base -= remaining;
                        base_in_sell_order += remaining;
                    }
                }
            }
        }
    }

    let entry_price = (position_base > Decimal::ZERO).then(|| position_cost / position_base);

    Balances {
        quote_available: quote,
        quote_in_buy_order,
        base_available: base,
        base_in_sell_order,
        position_cost,
        entry_price,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::planner::tests::d;
    use kraken_spot_api::trading::OrderStatus;

    pub fn order(
        time: f64,
        side: OrderSide,
        status: OrderStatus,
        volume: &str,
        price: &str,
    ) -> BotOrder {
        BotOrder {
            txid: format!("O{time}"),
            manager_id: 1,
            pair: "XBTUSD".into(),
            side,
            status,
            volume: d(volume),
            volume_executed: Decimal::ZERO,
            cost: Decimal::ZERO,
            fee: Decimal::ZERO,
            limit_price: Some(d(price)),
            average_price: None,
            open_time: time,
            close_time: None,
            expire_time: None,
        }
    }

    pub fn executed(order: BotOrder, volume: &str, cost: &str, fee: &str) -> BotOrder {
        BotOrder {
            volume_executed: d(volume),
            cost: d(cost),
            fee: d(fee),
            ..order
        }
    }

    fn compute100(orders: &[BotOrder]) -> Balances {
        compute(d("100"), orders)
    }

    #[test]
    fn nothing_done_keeps_the_initial_amount() {
        let balances = compute100(&[]);
        assert_eq!(balances.quote_available, d("100"));
        assert_eq!(balances.base_available, d("0"));
        assert_eq!(balances.entry_price, None);
    }

    #[test]
    fn an_open_buy_order_holds_the_quote() {
        let balances = compute100(&[order(
            1.0,
            OrderSide::Buy,
            OrderStatus::Open,
            "0.001",
            "50000",
        )]);
        assert_eq!(balances.quote_available, d("50"));
        assert_eq!(balances.quote_in_buy_order, d("50"));
    }

    #[test]
    fn a_filled_buy_then_a_filled_sell() {
        let buy = executed(
            order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.001", "50000"),
            "0.001",
            "50",
            "0.1",
        );

        let after_buy = compute100(std::slice::from_ref(&buy));
        assert_eq!(after_buy.quote_available, d("49.9"));
        assert_eq!(after_buy.base_available, d("0.001"));
        // the fees are part of the entry price
        assert_eq!(after_buy.entry_price, Some(d("50100")));

        let open_sell = order(2.0, OrderSide::Sell, OrderStatus::Open, "0.001", "51000");
        let while_selling = compute100(&[buy.clone(), open_sell.clone()]);
        assert_eq!(while_selling.base_available, d("0"));
        assert_eq!(while_selling.base_in_sell_order, d("0.001"));

        let sell = executed(
            BotOrder {
                status: OrderStatus::Closed,
                ..open_sell
            },
            "0.001",
            "51",
            "0.1",
        );
        let after_sell = compute100(&[buy, sell]);
        assert_eq!(after_sell.quote_available, d("100.8"));
        assert_eq!(after_sell.base_available, d("0"));
        assert_eq!(after_sell.entry_price, None);
    }

    #[test]
    fn an_expired_buy_partially_filled_is_held() {
        let balances = compute100(&[
            executed(
                order(1.0, OrderSide::Buy, OrderStatus::Expired, "0.002", "40000"),
                "0.0005",
                "20",
                "0",
            ),
            // canceled before any execution
            order(2.0, OrderSide::Buy, OrderStatus::Canceled, "1", "1"),
        ]);

        assert_eq!(balances.quote_available, d("80"));
        assert_eq!(balances.quote_in_buy_order, d("0"));
        assert_eq!(balances.base_available, d("0.0005"));
        assert_eq!(balances.entry_price, Some(d("40000")));
    }

    #[test]
    fn a_partial_sell_keeps_the_cost_of_the_rest() {
        let buy = executed(
            order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.002", "50000"),
            "0.002",
            "100",
            "0",
        );
        let sell = executed(
            order(
                2.0,
                OrderSide::Sell,
                OrderStatus::Canceled,
                "0.002",
                "55000",
            ),
            "0.0005",
            "27.5",
            "0",
        );

        let balances = compute100(&[sell, buy]);
        assert_eq!(balances.base_available, d("0.0015"));
        assert_eq!(balances.position_cost, d("75"));
        assert_eq!(balances.entry_price, Some(d("50000")));
        assert_eq!(balances.quote_available, d("27.5"));
    }

    #[test]
    fn values_everything_held() {
        let balances = compute100(&[
            executed(
                order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.001", "50000"),
                "0.001",
                "50",
                "0",
            ),
            order(2.0, OrderSide::Sell, OrderStatus::Open, "0.0004", "60000"),
        ]);
        // 50 USD + 0.001 BTC at 60000
        assert_eq!(balances.value(d("60000")), d("110"));
    }
}
