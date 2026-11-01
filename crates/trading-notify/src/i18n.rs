//! The languages of the notifications. A language is a [`Catalog`] of templates whose
//! `{placeholders}` are filled by [`render`]; every catalog has the placeholders of [`ENGLISH`].
//! Adding a language: a new catalog and a new [`Language`].

use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Fr,
}

impl Language {
    pub fn catalog(self) -> &'static Catalog {
        match self {
            Language::En => &ENGLISH,
            Language::Fr => &FRENCH,
        }
    }
}

/// The texts of a language
#[derive(Debug, PartialEq)]
pub struct Catalog {
    pub decimal_separator: char,

    pub started_title: &'static str,
    /// {managers} {mode}
    pub started_message: &'static str,
    pub mode_dry_run: &'static str,
    pub mode_live: &'static str,

    pub dry_run_prefix: &'static str,
    pub buy_placed: &'static str,
    pub sell_placed: &'static str,
    /// {amount} {base} {price} {quote} {total}
    pub placed_message: &'static str,
    /// {hours}
    pub expires_in: &'static str,

    pub buy_filled: &'static str,
    pub buy_partially_filled: &'static str,
    pub sell_filled: &'static str,
    pub sell_partially_filled: &'static str,
    /// {executed} {volume} {base}
    pub partial_volume: &'static str,
    /// {volume} {price} {quote} {cost}
    pub buy_filled_message: &'static str,
    /// {volume} {price} {quote} {received}
    pub sell_filled_message: &'static str,
    /// {gain} {quote} {percent}
    pub gain: &'static str,

    pub buy_expired: &'static str,
    pub buy_canceled: &'static str,
    pub sell_expired: &'static str,
    pub sell_canceled: &'static str,
    pub buy_not_executed: &'static str,
    pub sell_not_executed: &'static str,
    /// {executed} {volume} {base}
    pub partially_executed: &'static str,

    pub manager_error: &'static str,
    pub bot_error: &'static str,

    pub daily_title: &'static str,
    /// {id} {pair} {value} {quote} {gain} {percent} {sales}
    pub daily_line: &'static str,
    /// {quote} {value} {gain}
    pub daily_total: &'static str,
}

pub const ENGLISH: Catalog = Catalog {
    decimal_separator: '.',

    started_title: "Bot started",
    started_message: "{managers} manager(s) · {mode}",
    mode_dry_run: "dry run: the orders are only validated by Kraken, never placed",
    mode_live: "live: the orders are placed on Kraken",

    dry_run_prefix: "[dry run] ",
    buy_placed: "Buy placed",
    sell_placed: "Sell placed",
    placed_message: "{amount} {base} at {price} {quote} (≈ {total} {quote})",
    expires_in: "expires in {hours} h if not filled",

    buy_filled: "Buy filled",
    buy_partially_filled: "Buy partially filled",
    sell_filled: "Sell filled",
    sell_partially_filled: "Sell partially filled",
    partial_volume: "{executed} / {volume} {base}",
    buy_filled_message: "{volume} at {price} {quote} · cost {cost} {quote} fees included",
    sell_filled_message: "{volume} at {price} {quote} · received {received} {quote} after fees",
    gain: "gain {gain} {quote} ({percent} %)",

    buy_expired: "Buy expired",
    buy_canceled: "Buy canceled",
    sell_expired: "Sell expired",
    sell_canceled: "Sell canceled",
    buy_not_executed: "not filled: a new buy will be placed from the market price",
    sell_not_executed: "not filled: a new sell will be placed",
    partially_executed: "partially filled: {executed} / {volume} {base}, the rest is handled at the next tick",

    manager_error: "Error",
    bot_error: "Bot error",

    daily_title: "Daily summary",
    daily_line: "#{id} {pair}: {value} {quote} ({gain} · {percent} %) · {sales} sale(s) in 24 h",
    daily_total: "total {quote}: {value} ({gain})",
};

pub const FRENCH: Catalog = Catalog {
    decimal_separator: ',',

    started_title: "Bot démarré",
    started_message: "{managers} manager(s) · {mode}",
    mode_dry_run: "simulation : les ordres sont seulement validés par Kraken, jamais placés",
    mode_live: "réel : les ordres sont placés sur Kraken",

    dry_run_prefix: "[simulation] ",
    buy_placed: "Achat placé",
    sell_placed: "Vente placée",
    placed_message: "{amount} {base} à {price} {quote} (≈ {total} {quote})",
    expires_in: "expire dans {hours} h s'il n'est pas exécuté",

    buy_filled: "Achat exécuté",
    buy_partially_filled: "Achat partiellement exécuté",
    sell_filled: "Vente exécutée",
    sell_partially_filled: "Vente partiellement exécutée",
    partial_volume: "{executed} / {volume} {base}",
    buy_filled_message: "{volume} à {price} {quote} · coût {cost} {quote} frais inclus",
    sell_filled_message: "{volume} à {price} {quote} · reçu {received} {quote} frais déduits",
    gain: "gain {gain} {quote} ({percent} %)",

    buy_expired: "Achat expiré",
    buy_canceled: "Achat annulé",
    sell_expired: "Vente expirée",
    sell_canceled: "Vente annulée",
    buy_not_executed: "non exécuté : un nouvel achat sera placé depuis le prix du marché",
    sell_not_executed: "non exécutée : une nouvelle vente sera placée",
    partially_executed: "exécuté en partie : {executed} / {volume} {base}, la suite est gérée au prochain tick",

    manager_error: "Erreur",
    bot_error: "Erreur du bot",

    daily_title: "Résumé du jour",
    daily_line: "#{id} {pair} : {value} {quote} ({gain} · {percent} %) · {sales} vente(s) en 24 h",
    daily_total: "total {quote} : {value} ({gain})",
};

/// Fills the `{placeholders}` of `template`
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (key, value) in values {
        text = text.replace(&format!("{{{key}}}"), value);
    }
    text
}

impl Catalog {
    /// A decimal, with the separator of the language
    pub fn number(&self, value: Decimal) -> String {
        value
            .normalize()
            .to_string()
            .replace('.', &self.decimal_separator.to_string())
    }

    /// An amount of money: 2 decimals
    pub fn money(&self, value: Decimal) -> String {
        self.number(value.round_dp(2))
    }

    /// A signed amount of money: "+1.52" or "-0.40"
    pub fn signed(&self, value: Decimal) -> String {
        let text = self.money(value);
        if value >= Decimal::ZERO {
            format!("+{text}")
        } else {
            text
        }
    }

    /// Every template, with its name: to check the catalogs
    pub fn templates(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("started_title", self.started_title),
            ("started_message", self.started_message),
            ("mode_dry_run", self.mode_dry_run),
            ("mode_live", self.mode_live),
            ("dry_run_prefix", self.dry_run_prefix),
            ("buy_placed", self.buy_placed),
            ("sell_placed", self.sell_placed),
            ("placed_message", self.placed_message),
            ("expires_in", self.expires_in),
            ("buy_filled", self.buy_filled),
            ("buy_partially_filled", self.buy_partially_filled),
            ("sell_filled", self.sell_filled),
            ("sell_partially_filled", self.sell_partially_filled),
            ("partial_volume", self.partial_volume),
            ("buy_filled_message", self.buy_filled_message),
            ("sell_filled_message", self.sell_filled_message),
            ("gain", self.gain),
            ("buy_expired", self.buy_expired),
            ("buy_canceled", self.buy_canceled),
            ("sell_expired", self.sell_expired),
            ("sell_canceled", self.sell_canceled),
            ("buy_not_executed", self.buy_not_executed),
            ("sell_not_executed", self.sell_not_executed),
            ("partially_executed", self.partially_executed),
            ("manager_error", self.manager_error),
            ("bot_error", self.bot_error),
            ("daily_title", self.daily_title),
            ("daily_line", self.daily_line),
            ("daily_total", self.daily_total),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::str::FromStr;

    fn placeholders(template: &str) -> BTreeSet<String> {
        template
            .split('{')
            .skip(1)
            .filter_map(|it| it.split_once('}'))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    #[test]
    fn every_language_has_the_placeholders_of_english() {
        for language in [Language::En, Language::Fr] {
            let catalog = language.catalog();
            for ((name, english), (_, translated)) in
                ENGLISH.templates().into_iter().zip(catalog.templates())
            {
                assert!(!translated.trim().is_empty(), "{language:?} {name}");
                assert_eq!(
                    placeholders(english),
                    placeholders(translated),
                    "{language:?} {name}"
                );
            }
        }
    }

    #[test]
    fn fills_the_placeholders() {
        assert_eq!(
            render("{a} and {b}, {a}", &[("a", "1"), ("b", "2")]),
            "1 and 2, 1"
        );
        assert_eq!(render("no placeholder", &[("a", "1")]), "no placeholder");
    }

    #[test]
    fn writes_the_numbers_of_each_language() {
        let d = |value: &str| Decimal::from_str(value).unwrap();
        assert_eq!(ENGLISH.number(d("1234.50")), "1234.5");
        assert_eq!(FRENCH.number(d("1234.50")), "1234,5");
        assert_eq!(ENGLISH.money(d("102.516")), "102.52");
        assert_eq!(ENGLISH.signed(d("1.5")), "+1.5");
        assert_eq!(FRENCH.signed(d("-0.404")), "-0,4");
        assert_eq!(ENGLISH.signed(d("0")), "+0");
    }

    #[test]
    fn english_is_the_default() {
        assert_eq!(Language::default(), Language::En);
        assert_eq!(
            serde_norway::from_str::<Language>("fr").unwrap(),
            Language::Fr
        );
        assert!(serde_norway::from_str::<Language>("de").is_err());
    }
}
