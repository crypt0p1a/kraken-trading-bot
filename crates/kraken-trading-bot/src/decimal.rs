//! Exact decimals: the amounts of the bot are computed with [`Decimal`], never with floats. The
//! models of `kraken-spot-api` already hold exact decimals, parsed from the strings of Kraken

use rust_decimal::Decimal;

/// `value`% as a ratio: 1.5 -> 0.015
pub fn percent(value: Decimal) -> Decimal {
    value / Decimal::ONE_HUNDRED
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn computes_the_percents() {
        assert_eq!(
            percent(Decimal::from_str("1.5").unwrap()),
            Decimal::from_str("0.015").unwrap()
        );
    }
}
