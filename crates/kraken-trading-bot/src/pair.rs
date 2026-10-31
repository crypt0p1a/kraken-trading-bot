//! The constraints of a Kraken pair on the orders

use crate::decimal::dec;
use kraken_api::market::AssetPair;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq)]
pub struct PairRules {
    /// Kraken name, key of the tickers and of the fees, e.g. "XXBTZUSD"
    pub key: String,
    /// Alternate name, given to AddOrder and written in the orders, e.g. "XBTUSD"
    pub altname: String,
    /// Asset of the quote, key of the balances, e.g. "ZUSD"
    pub quote_asset: String,
    /// Decimals of the price
    pub price_decimals: u32,
    /// Increment of the price
    pub tick_size: Decimal,
    /// Decimals of the volume
    pub lot_decimals: u32,
    /// Minimum volume of an order, in the base
    pub ordermin: Decimal,
    /// Minimum cost of an order, in the quote
    pub costmin: Decimal,
}

impl PairRules {
    pub fn from_asset_pair(key: &str, pair: &AssetPair) -> Self {
        let price_decimals = pair.pair_decimals.max(0) as u32;
        Self {
            key: key.to_string(),
            altname: pair.altname.clone(),
            quote_asset: pair.quote.clone(),
            price_decimals,
            tick_size: pair
                .tick_size
                .map(dec)
                .filter(|it| *it > Decimal::ZERO)
                .unwrap_or_else(|| Decimal::new(1, price_decimals)),
            lot_decimals: pair.lot_decimals.max(0) as u32,
            ordermin: pair.ordermin.map(dec).unwrap_or_default(),
            costmin: pair.costmin.map(dec).unwrap_or_default(),
        }
    }

    /// Whether `name` designates this pair: Kraken name, alternate name or WebSocket name
    pub fn matches(name: &str, key: &str, pair: &AssetPair) -> bool {
        let name = name.trim();
        name.eq_ignore_ascii_case(key)
            || name.eq_ignore_ascii_case(&pair.altname)
            || pair
                .wsname
                .as_deref()
                .is_some_and(|it| name.eq_ignore_ascii_case(it))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn pair(tick_size: Option<f64>) -> AssetPair {
        serde_json::from_value(serde_json::json!({
            "altname": "XBTUSD", "wsname": "XBT/USD", "base": "XXBT", "quote": "ZUSD",
            "pair_decimals": 1, "lot_decimals": 8, "ordermin": "0.00005", "costmin": "0.5",
            "tick_size": tick_size.map(|it| it.to_string()),
        }))
        .unwrap()
    }

    #[test]
    fn reads_the_constraints_of_the_pair() {
        let rules = PairRules::from_asset_pair("XXBTZUSD", &pair(Some(0.1)));
        assert_eq!(rules.altname, "XBTUSD");
        assert_eq!(rules.quote_asset, "ZUSD");
        assert_eq!(rules.price_decimals, 1);
        assert_eq!(rules.tick_size, Decimal::from_str("0.1").unwrap());
        assert_eq!(rules.lot_decimals, 8);
        assert_eq!(rules.ordermin, Decimal::from_str("0.00005").unwrap());
        assert_eq!(rules.costmin, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn the_tick_defaults_to_the_decimals_of_the_price() {
        let rules = PairRules::from_asset_pair("XXBTZUSD", &pair(None));
        assert_eq!(rules.tick_size, Decimal::from_str("0.1").unwrap());
    }

    #[test]
    fn matches_every_name_of_the_pair() {
        let pair = pair(Some(0.1));
        for name in ["XXBTZUSD", "XBTUSD", "xbtusd", "XBT/USD", " XBTUSD "] {
            assert!(PairRules::matches(name, "XXBTZUSD", &pair), "{name}");
        }
        assert!(!PairRules::matches("ETHUSD", "XXBTZUSD", &pair));
    }
}
