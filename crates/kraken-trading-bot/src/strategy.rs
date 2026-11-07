//! What a manager does next. Port of the ManagerStrategy of cex-trading-bot

use crate::ledger::Balances;
use crate::pair::PairRules;
use crate::planner;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// An order of the manager is open: nothing to do until it closes
    Wait,
    /// Nothing can be done, for this reason
    Idle(String),
    Buy {
        amount: Decimal,
        price: Decimal,
    },
    Sell {
        amount: Decimal,
        price: Decimal,
    },
}

pub struct Decision<'a> {
    pub balances: &'a Balances,
    pub has_open_order: bool,
    /// False for a closing manager: it sells what it holds but doesn't buy
    pub active: bool,
    pub buy_below: Decimal,
    pub sell_above: Decimal,
    /// Last price of the pair
    pub reference: Decimal,
    pub fee_rate: Decimal,
    pub rules: &'a PairRules,
}

/// Waits for its open order, sells what it holds or buys with everything it has
pub fn decide(decision: &Decision) -> Action {
    let Decision {
        balances,
        rules,
        fee_rate,
        ..
    } = decision;

    if decision.has_open_order {
        return Action::Wait;
    }

    if balances.base_available > Decimal::ZERO && balances.position_cost > Decimal::ZERO {
        let amount = planner::round_amount(balances.base_available, rules);
        if amount > Decimal::ZERO {
            let price = planner::sell_price(
                balances.position_cost,
                amount,
                decision.sell_above,
                *fee_rate,
                rules,
            );
            if planner::is_valid(amount, price, rules) {
                return Action::Sell { amount, price };
            }
        }
        // otherwise, too small to be sold: buying more
    }

    if !decision.active {
        return Action::Idle("closing: doesn't buy anymore".into());
    }

    let price = planner::buy_price(decision.reference, decision.buy_below, rules);
    match planner::buy_amount(balances.quote_available, price, *fee_rate, rules) {
        Some(amount) => Action::Buy { amount, price },
        None => Action::Idle(format!(
            "can't buy with {} at {} (minimums of the pair)",
            balances.quote_available.normalize(),
            price
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::compute;
    use crate::ledger::tests::{executed, order};
    use crate::order::BotOrder;
    use crate::planner::tests::{d, rules};
    use kraken_spot_api::trading::{OrderSide, OrderStatus};

    fn decide_with(
        balances: &Balances,
        reference: Decimal,
        fee_rate: Decimal,
        buy_below: Decimal,
        sell_above: Decimal,
    ) -> Action {
        decide(&Decision {
            balances,
            has_open_order: false,
            active: true,
            buy_below,
            sell_above,
            reference,
            fee_rate,
            rules: &rules(),
        })
    }

    fn simple(balances: &Balances, reference: &str, fee_rate: &str) -> Action {
        decide_with(balances, d(reference), d(fee_rate), d("1.0"), d("1.5"))
    }

    #[test]
    fn waits_for_its_open_order() {
        let balances = compute(d("50"), &[]);
        let action = decide(&Decision {
            balances: &balances,
            has_open_order: true,
            active: true,
            buy_below: d("1"),
            sell_above: d("1.5"),
            reference: d("50000"),
            fee_rate: d("0.0025"),
            rules: &rules(),
        });
        assert_eq!(action, Action::Wait);
    }

    #[test]
    fn a_manager_only_spends_its_own_money() {
        let balances = compute(d("50"), &[]);
        let Action::Buy { amount, price } = simple(&balances, "50000", "0.0025") else {
            panic!("a buy expected");
        };
        assert_eq!(price, d("49500"));
        assert!(amount * price * d("1.0025") <= d("50"));
    }

    #[test]
    fn a_manager_buys_again_with_everything_its_sale_brought() {
        // bought 0.001 BTC for 50 USD, sold it for 55 USD
        let orders = [
            executed(
                order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.001", "50000"),
                "0.001",
                "50",
                "0",
            ),
            executed(
                order(2.0, OrderSide::Sell, OrderStatus::Closed, "0.001", "55000"),
                "0.001",
                "55",
                "0",
            ),
        ];
        let balances = compute(d("50"), &orders);
        assert_eq!(balances.quote_available, d("55"));

        let fee = d("0.0025");
        let Action::Buy { amount, price } = simple(&balances, "55000", "0.0025") else {
            panic!("a buy expected");
        };
        // 55 USD minus at most one lot of the pair, fees included
        let spent = amount * price * (Decimal::ONE + fee);
        let one_lot = d("0.00000001") * price * (Decimal::ONE + fee);
        assert!(spent <= d("55"), "spent {spent}");
        assert!(spent > d("55") - one_lot, "spent {spent}");
    }

    #[test]
    fn sells_everything_it_holds_above_its_cost() {
        let orders = [executed(
            order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.002", "50000"),
            "0.002",
            "100",
            "0",
        )];
        let balances = compute(d("100"), &orders);

        let Action::Sell { amount, price } = simple(&balances, "40000", "0.01") else {
            panic!("a sell expected");
        };
        assert_eq!(amount, d("0.002"));
        // 100 * 1.015 / (0.002 * 0.99), whatever the market
        assert_eq!(price, d("51262.7"));
    }

    #[test]
    fn a_closing_manager_sells_but_never_buys() {
        let decision = |balances: &Balances| {
            decide(&Decision {
                balances,
                has_open_order: false,
                active: false,
                buy_below: d("1"),
                sell_above: d("1.5"),
                reference: d("50000"),
                fee_rate: d("0.0025"),
                rules: &rules(),
            })
        };

        assert!(matches!(decision(&compute(d("100"), &[])), Action::Idle(_)));

        let holding = compute(
            d("100"),
            &[executed(
                order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.001", "50000"),
                "0.001",
                "50",
                "0",
            )],
        );
        assert!(matches!(decision(&holding), Action::Sell { .. }));
    }

    #[test]
    fn idles_when_the_budget_is_under_the_minimums() {
        let balances = compute(d("4"), &[]);
        let Action::Idle(reason) = simple(&balances, "50000", "0.0025") else {
            panic!("idle expected");
        };
        assert!(reason.contains("minimums"), "{reason}");
    }

    #[test]
    fn buys_more_when_what_it_holds_is_too_small_to_be_sold() {
        // 0.00005 BTC, under the minimum volume of the pair
        let orders = [executed(
            order(1.0, OrderSide::Buy, OrderStatus::Closed, "0.00005", "50000"),
            "0.00005",
            "2.5",
            "0",
        )];
        let balances = compute(d("100"), &orders);
        assert!(matches!(
            simple(&balances, "50000", "0.0025"),
            Action::Buy { .. }
        ));
    }

    /// Deterministic random numbers for the simulation (xorshift64*)
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }

        /// In [from, until[, with 6 decimals
        fn decimal(&mut self, from: f64, until: f64) -> Decimal {
            let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
            Decimal::from_f64_retain(from + unit * (until - from))
                .unwrap()
                .round_dp(6)
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    fn filled(
        time: f64,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        fee_rate: Decimal,
    ) -> BotOrder {
        let cost = amount * price;
        // the fees rounded up, at the expense of the manager
        let fee = (cost * fee_rate)
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToPositiveInfinity);
        BotOrder {
            volume: amount,
            volume_executed: amount,
            cost,
            fee,
            average_price: Some(price),
            ..order(time, side, OrderStatus::Closed, "0", "0")
        }
    }

    /// Runs cycles at random prices, fees and percentages, including a sell percentage lower than
    /// the fees and buy orders expiring partially filled: every cycle ends with more than it
    /// started with
    #[test]
    fn every_cycle_ends_with_more_than_it_started_with() {
        let mut random = Random(42);
        let mut orders: Vec<BotOrder> = Vec::new();
        let mut time = 0.0;
        let mut reference = d("50000");
        let ledger = |orders: &[BotOrder]| compute(d("50"), orders);

        for cycle in 0..500 {
            let fee_rate = random
                .decimal(0.0, 0.006)
                .round_dp_with_strategy(5, rust_decimal::RoundingStrategy::ToNegativeInfinity);
            let buy_below = random.decimal(0.0, 5.0);
            let sell_above = random.decimal(0.05, 3.0);

            let before = ledger(&orders);

            // buys until there is something to sell, sometimes partially
            let mut action = decide_with(&before, reference, fee_rate, buy_below, sell_above);
            let mut bought = 0;
            while let Action::Buy { amount, price } = action {
                let partial = random.below(4) == 0 && bought < 3;
                let executed = if partial {
                    (amount / Decimal::from(random.below(198) + 2)).round_dp_with_strategy(
                        8,
                        rust_decimal::RoundingStrategy::ToNegativeInfinity,
                    )
                } else {
                    amount
                };
                time += 1.0;
                orders.push(filled(time, OrderSide::Buy, executed, price, fee_rate));
                bought += 1;
                action = decide_with(&ledger(&orders), reference, fee_rate, buy_below, sell_above);
            }

            let Action::Sell { amount, price } = action else {
                panic!("cycle {cycle}: a sell expected, got {action:?}");
            };
            let cost = ledger(&orders).position_cost;
            time += 1.0;
            orders.push(filled(time, OrderSide::Sell, amount, price, fee_rate));

            let after = ledger(&orders);
            let expected = before.quote_available + cost * sell_above / Decimal::ONE_HUNDRED;

            assert!(
                after.quote_available > before.quote_available,
                "cycle {cycle}: {} -> {}",
                before.quote_available,
                after.quote_available
            );
            // the fees rounded up cost at most 2 hundred-millionths
            assert!(
                after.quote_available >= expected - d("0.00000002"),
                "cycle {cycle}: {} < {expected}",
                after.quote_available
            );

            reference = (reference * random.decimal(0.9, 1.1)).round_dp(2);
        }

        assert!(ledger(&orders).quote_available > d("50"));
    }
}
