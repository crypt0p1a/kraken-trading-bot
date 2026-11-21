//! The market of each pair at a tick: its rules, its price and its fees, the indicators of the
//! market and what the account is worth

use super::{Bot, Market};
use crate::clock::now_millis;
use crate::decimal::percent;
use crate::exchange::Exchange;
use crate::market;
use crate::pair::PairRules;
use crate::storage::{AccountRecord, TickerRecord};
use crate::{BotError, BotResult};
use kraken_spot_api::account::ExtendedBalance;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashMap, HashSet};

impl<E: Exchange> Bot<E> {
    pub(super) async fn markets(&self) -> BotResult<HashMap<String, Market>> {
        let mut keys: Vec<String> = self.pair_keys.values().cloned().collect();
        keys.sort();
        keys.dedup();

        let pairs = self.exchange.asset_pairs(&keys).await?;
        let tickers = self.exchange.tickers(&keys).await?;
        let fees = self.exchange.trade_volume(&keys).await?;
        let at = now_millis();

        let mut markets = HashMap::new();
        for key in keys {
            let pair = pairs
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no information for {key}")))?;
            let ticker = tickers
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no ticker for {key}")))?;
            // the taker fee: the highest one, whatever the order becomes
            let fee = fees
                .fees
                .get(&key)
                .map(|it| it.fee)
                .ok_or_else(|| BotError::Config(format!("no fee for {key}")))?;

            self.storage.record_ticker(&TickerRecord {
                pair: key.clone(),
                at,
                last: ticker.last_trade.price,
                bid: ticker.bid.price,
                ask: ticker.ask.price,
                volume_24h: ticker.volume.last_24h,
                vwap_24h: ticker.vwap.last_24h,
            })?;
            self.record_indicators(&key, at).await;

            markets.insert(
                key.clone(),
                Market {
                    rules: PairRules::from_asset_pair(&key, pair),
                    reference: ticker.last_trade.price,
                    fee_rate: percent(fee),
                },
            );
        }
        Ok(markets)
    }

    /// The trend and the volatility of the pair over 24 hours, logged and recorded: nothing decides
    /// on them yet. A failure is only logged, the tick goes on
    async fn record_indicators(&self, key: &str, at: i64) {
        let since = self.exchange.now() as i64 - market::WINDOW_SECONDS;
        let candles = match self.exchange.candles(key, market::INTERVAL, since).await {
            Ok(candles) => candles,
            Err(error) => {
                tracing::warn!(pair = key, %error, "no candles: no indicators");
                return;
            }
        };
        let Some(indicators) = market::compute(&candles) else {
            tracing::info!(
                pair = key,
                candles = candles.len(),
                "not enough candles for the indicators"
            );
            return;
        };
        tracing::info!(
            pair = key,
            trend = indicators.trend(),
            atr = format!("{:.3}%", indicators.atr_percent),
            slope_24h = format!("{:+.2}%", indicators.slope_24h_percent),
            change_24h = format!("{:+.2}%", indicators.change_24h_percent),
            range_24h = format!("{:.2}%", indicators.range_24h_percent),
            "indicators"
        );
        if let Err(error) = self.storage.record_indicators(key, at, &indicators) {
            tracing::warn!(pair = key, %error, "the indicators couldn't be recorded");
        }
    }

    /// What the whole account is worth in each quote currency of the managers: the quote, plus the
    /// bases of its pairs at their last price. An asset no pair of the managers trades isn't counted
    pub(super) fn record_account(
        &self,
        balances: &HashMap<String, ExtendedBalance>,
        markets: &HashMap<String, Market>,
    ) {
        let at = now_millis();
        let total = |asset: &str| balances.get(asset).map_or(Decimal::ZERO, |it| it.balance);
        let mut keys: Vec<&String> = markets.keys().collect();
        keys.sort();
        let mut accounts: BTreeMap<&str, AccountRecord> = BTreeMap::new();
        let mut valued = HashSet::new();
        for key in keys {
            let rules = &markets[key].rules;
            let account = accounts
                .entry(&rules.quote_asset)
                .or_insert_with(|| AccountRecord {
                    at,
                    quote: rules.quote_asset.clone(),
                    cash: total(&rules.quote_asset),
                    holdings: Decimal::ZERO,
                });
            // a base traded in two quotes is counted once
            if valued.insert(&rules.base_asset) {
                account.holdings += total(&rules.base_asset) * markets[key].reference;
            }
        }
        for account in accounts.values() {
            if let Err(error) = self.storage.record_account(account) {
                tracing::warn!(quote = %account.quote, %error, "the account couldn't be recorded");
            }
        }
    }
}
