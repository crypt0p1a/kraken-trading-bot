//! Exact decimals: the amounts of the bot are computed with [`Decimal`], never with floats

use rust_decimal::Decimal;
use std::str::FromStr;

/// The decimal written by Kraken: its models hold f64 parsed from strings like "0.00058823",
/// whose shortest representation is that string
pub fn dec(value: f64) -> Decimal {
    if !value.is_finite() {
        return Decimal::ZERO;
    }
    Decimal::from_str(&value.to_string())
        .or_else(|_| Decimal::from_scientific(&format!("{value:e}")))
        .unwrap_or(Decimal::ZERO)
}

/// `value`% as a ratio: 1.5 -> 0.015
pub fn percent(value: Decimal) -> Decimal {
    value / Decimal::ONE_HUNDRED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_decimals_written_by_kraken() {
        for text in [
            "0.00058823",
            "149.9056",
            "77379",
            "0.1",
            "1234567.12345678",
            "0.00000001",
        ] {
            let value: f64 = text.parse().unwrap();
            assert_eq!(dec(value), Decimal::from_str(text).unwrap(), "{text}");
        }
        assert_eq!(dec(f64::NAN), Decimal::ZERO);
        assert_eq!(dec(f64::INFINITY), Decimal::ZERO);
    }

    #[test]
    fn computes_the_percents() {
        assert_eq!(
            percent(Decimal::from_str("1.5").unwrap()),
            Decimal::from_str("0.015").unwrap()
        );
    }
}
