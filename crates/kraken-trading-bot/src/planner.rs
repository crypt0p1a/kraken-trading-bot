//! Prices and amounts of the limit orders, following the constraints of the pair. Port of the
//! OrderPlanner of cex-trading-bot. The fee rates are ratios: 0.0025 for 0.25%

use crate::decimal::percent;
use crate::pair::PairRules;
use rust_decimal::{Decimal, RoundingStrategy};

fn round_price(price: Decimal, rules: &PairRules, strategy: RoundingStrategy) -> Decimal {
    let rounded = price.round_dp_with_strategy(rules.price_decimals, strategy);
    let ticks = (rounded / rules.tick_size).round_dp_with_strategy(0, strategy);
    (ticks * rules.tick_size).normalize()
}

/// The reference price minus `buy_below`%, rounded down
pub fn buy_price(reference: Decimal, buy_below: Decimal, rules: &PairRules) -> Decimal {
    round_price(
        reference * (Decimal::ONE - percent(buy_below)),
        rules,
        RoundingStrategy::ToNegativeInfinity,
    )
}

/// Price at which selling `amount` brings back, fees deducted, at least `position_cost` plus
/// `sell_above`%: what couldn't be sold because of the rounding of the pair is paid as well.
/// Rounded up, to never get less
pub fn sell_price(
    position_cost: Decimal,
    amount: Decimal,
    sell_above: Decimal,
    fee_rate: Decimal,
    rules: &PairRules,
) -> Decimal {
    let wanted = position_cost * (Decimal::ONE + percent(sell_above));
    round_price(
        wanted / (amount * (Decimal::ONE - fee_rate)),
        rules,
        RoundingStrategy::ToPositiveInfinity,
    )
}

/// Amount of base to buy at `price` with `quote`, fees included, None when the pair doesn't
/// allow it
pub fn buy_amount(
    quote: Decimal,
    price: Decimal,
    fee_rate: Decimal,
    rules: &PairRules,
) -> Option<Decimal> {
    if price <= Decimal::ZERO || quote <= Decimal::ZERO {
        return None;
    }
    let amount = round_amount(quote / (price * (Decimal::ONE + fee_rate)), rules);
    Some(amount).filter(|it| is_valid(*it, price, rules))
}

/// Rounds down `amount` to the decimals of the volume of the pair
pub fn round_amount(amount: Decimal, rules: &PairRules) -> Decimal {
    if amount <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    amount
        .round_dp_with_strategy(rules.lot_decimals, RoundingStrategy::ToNegativeInfinity)
        .normalize()
}

/// Whether an order of `amount` at `price` respects the minimums of the pair
pub fn is_valid(amount: Decimal, price: Decimal, rules: &PairRules) -> bool {
    amount > Decimal::ZERO
        && price > Decimal::ZERO
        && amount >= rules.ordermin
        && amount * price >= rules.costmin
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::str::FromStr;

    pub fn d(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    /// The pair of the tests of cex-trading-bot
    pub fn rules() -> PairRules {
        PairRules {
            key: "XXBTZUSD".into(),
            altname: "XBTUSD".into(),
            base_asset: "XXBT".into(),
            quote_asset: "ZUSD".into(),
            price_decimals: 1,
            tick_size: d("0.1"),
            lot_decimals: 8,
            ordermin: d("0.0001"),
            costmin: d("5"),
        }
    }

    #[test]
    fn prices_follow_the_percentages_and_the_precision() {
        assert_eq!(buy_price(d("50000"), d("1.0"), &rules()), d("49500"));
        assert_eq!(buy_price(d("50000.33"), d("1.234"), &rules()), d("49383.3"));
        // 100 * 1.015 / (0.002 * 0.99) = 51262.62..., rounded up to never get less
        assert_eq!(
            sell_price(d("100"), d("0.002"), d("1.5"), d("0.01"), &rules()),
            d("51262.7")
        );
    }

    #[test]
    fn prices_follow_the_tick_size() {
        let rules = PairRules {
            price_decimals: 2,
            tick_size: d("0.05"),
            ..rules()
        };
        assert_eq!(buy_price(d("100"), d("0.13"), &rules), d("99.85"));
        assert_eq!(
            sell_price(d("100"), d("1"), d("0.13"), d("0"), &rules),
            d("100.15")
        );
    }

    #[test]
    fn amounts_follow_the_pair_constraints() {
        // 100 / (49500 * 1.0025) = 0.0020151671..., rounded down
        assert_eq!(
            buy_amount(d("100"), d("49500"), d("0.0025"), &rules()),
            Some(d("0.00201516"))
        );
        // under the minimum cost
        assert_eq!(buy_amount(d("4"), d("49500"), d("0.0025"), &rules()), None);
        assert_eq!(round_amount(d("0.001234567"), &rules()), d("0.00123456"));
        assert_eq!(round_amount(d("-1"), &rules()), d("0"));
        // under the minimum volume
        assert!(!is_valid(d("0.00005"), d("200000"), &rules()));
        assert!(is_valid(d("0.0001"), d("50000"), &rules()));
    }

    #[test]
    fn nothing_is_bought_without_money_or_price() {
        assert_eq!(buy_amount(d("0"), d("49500"), d("0.0025"), &rules()), None);
        assert_eq!(buy_amount(d("100"), d("0"), d("0.0025"), &rules()), None);
    }
}
