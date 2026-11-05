use super::{Result, Storage};
use crate::market::Indicators;
use rusqlite::{OptionalExtension, params};
use rust_decimal::Decimal;

/// A decision of a manager, and what came of it
#[derive(Debug, Clone, PartialEq)]
pub struct ActionRecord {
    pub manager_id: i32,
    /// UTC milliseconds
    pub at: i64,
    /// buy, sell or idle
    pub kind: String,
    pub amount: Option<Decimal>,
    pub price: Option<Decimal>,
    pub reason: Option<String>,
    /// The order was only validated by Kraken
    pub dry_run: bool,
    /// The order placed
    pub txid: Option<String>,
    /// Why the order couldn't be placed
    pub error: Option<String>,
}

/// The price of a pair at a tick
#[derive(Debug, Clone, PartialEq)]
pub struct TickerRecord {
    /// Kraken name of the pair
    pub pair: String,
    /// UTC milliseconds
    pub at: i64,
    pub last: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    pub volume_24h: Decimal,
    pub vwap_24h: Decimal,
}

/// What the whole account is worth in a quote currency at a tick, managers or not
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecord {
    /// UTC milliseconds
    pub at: i64,
    /// Kraken name of the asset, e.g. "ZEUR"
    pub quote: String,
    /// The quote in the account, held by orders or not
    pub cash: Decimal,
    /// The bases of the pairs of this quote in the account, at their last price
    pub holdings: Decimal,
}

impl Storage {
    pub fn record_action(&self, action: &ActionRecord) -> Result<()> {
        self.connection.execute(
            "INSERT INTO actions (manager_id, at, kind, amount, price, reason, dry_run, txid, error,
                account)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                action.manager_id,
                action.at,
                action.kind,
                action.amount.map(|it| it.to_string()),
                action.price.map(|it| it.to_string()),
                action.reason,
                action.dry_run,
                action.txid,
                action.error,
                self.account,
            ],
        )?;
        Ok(())
    }

    /// The last actions of a manager, most recent first
    /// The bot cancelled the order itself: a buy the price left behind
    pub fn cancelled_by_the_bot(&self, txid: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM actions
                WHERE account = ?1 AND txid = ?2 AND kind = 'buy_cancelled' AND error IS NULL)",
            params![self.account, txid],
            |row| row.get(0),
        )?)
    }

    pub fn actions_of(&self, manager_id: i32, limit: usize) -> Result<Vec<ActionRecord>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT manager_id, at, kind, amount, price, reason, dry_run, txid, error FROM actions
             WHERE account = ?3 AND manager_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![manager_id, limit as i64, self.account], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;

        rows.map(|row| {
            let (manager_id, at, kind, amount, price, reason, dry_run, txid, error) = row?;
            Ok(ActionRecord {
                manager_id,
                at,
                kind,
                amount: super::optional_decimal(amount)?,
                price: super::optional_decimal(price)?,
                reason,
                dry_run,
                txid,
                error,
            })
        })
        .collect()
    }

    /// Records the price of a pair; a second record at the same time replaces the first one
    pub fn record_ticker(&self, ticker: &TickerRecord) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO tickers (pair, at, last, bid, ask, volume_24h, vwap_24h)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ticker.pair,
                ticker.at,
                ticker.last.to_string(),
                ticker.bid.to_string(),
                ticker.ask.to_string(),
                ticker.volume_24h.to_string(),
                ticker.vwap_24h.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn record_account(&self, account: &AccountRecord) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO account_snapshots (account, at, quote, cash, holdings)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.account,
                account.at,
                account.quote,
                account.cash.to_string(),
                account.holdings.to_string(),
            ],
        )?;
        Ok(())
    }

    /// The last snapshot of the account in each quote currency
    pub fn last_accounts(&self) -> Result<Vec<AccountRecord>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT at, quote, cash, holdings FROM account_snapshots
             WHERE account = ?1
                AND at = (SELECT MAX(at) FROM account_snapshots WHERE account = ?1)
             ORDER BY quote",
        )?;
        let rows = statement.query_map([self.account], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut accounts = Vec::new();
        for row in rows {
            let (at, quote, cash, holdings) = row?;
            accounts.push(AccountRecord {
                at,
                quote,
                cash: super::decimal(cash)?,
                holdings: super::decimal(holdings)?,
            });
        }
        Ok(accounts)
    }

    /// The indicators of the pair at a tick (UTC milliseconds)
    pub fn record_indicators(&self, pair: &str, at: i64, indicators: &Indicators) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO indicators (pair, at, close, atr_percent, ema_fast, ema_slow,
                slope_24h_percent, range_24h_percent, change_24h_percent, candles)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                pair,
                at,
                indicators.close,
                indicators.atr_percent,
                indicators.ema_fast,
                indicators.ema_slow,
                indicators.slope_24h_percent,
                indicators.range_24h_percent,
                indicators.change_24h_percent,
                indicators.candles as i64,
            ],
        )?;
        Ok(())
    }

    /// The last indicators of the pair, with their time
    pub fn last_indicators(&self, pair: &str) -> Result<Option<(i64, Indicators)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT at, close, atr_percent, ema_fast, ema_slow, slope_24h_percent,
                    range_24h_percent, change_24h_percent, candles
                 FROM indicators WHERE pair = ?1 ORDER BY at DESC LIMIT 1",
                [pair],
                |row| {
                    Ok((
                        row.get(0)?,
                        Indicators {
                            close: row.get(1)?,
                            atr_percent: row.get(2)?,
                            ema_fast: row.get(3)?,
                            ema_slow: row.get(4)?,
                            slope_24h_percent: row.get(5)?,
                            range_24h_percent: row.get(6)?,
                            change_24h_percent: row.get(7)?,
                            candles: row.get::<_, i64>(8)? as usize,
                        },
                    ))
                },
            )
            .optional()?)
    }

    pub fn ticker_count(&self, pair: &str) -> Result<usize> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM tickers WHERE pair = ?1",
            [pair],
            |row| row.get::<_, i64>(0).map(|it| it as usize),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::tests::d;

    fn action(at: i64, kind: &str) -> ActionRecord {
        ActionRecord {
            manager_id: 1,
            at,
            kind: kind.into(),
            amount: Some(d("0.0006")),
            price: Some(d("49500")),
            reason: None,
            dry_run: true,
            txid: None,
            error: None,
        }
    }

    #[test]
    fn records_the_actions_most_recent_first() {
        let storage = Storage::open_in_memory().unwrap();
        storage.record_action(&action(1, "buy")).unwrap();
        storage
            .record_action(&ActionRecord {
                error: Some("EOrder:Insufficient funds".into()),
                ..action(2, "sell")
            })
            .unwrap();
        storage
            .record_action(&ActionRecord {
                manager_id: 2,
                ..action(3, "buy")
            })
            .unwrap();

        let actions = storage.actions_of(1, 10).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].kind, "sell");
        assert_eq!(
            actions[0].error.as_deref(),
            Some("EOrder:Insufficient funds")
        );
        assert_eq!(actions[1], action(1, "buy"));
        assert_eq!(storage.actions_of(1, 1).unwrap().len(), 1);
    }

    #[test]
    fn records_the_tickers() {
        let storage = Storage::open_in_memory().unwrap();
        let ticker = TickerRecord {
            pair: "XXBTZUSD".into(),
            at: 1,
            last: d("77382.3"),
            bid: d("77378.9"),
            ask: d("77379"),
            volume_24h: d("2223.30614445"),
            vwap_24h: d("76937.95745"),
        };
        storage.record_ticker(&ticker).unwrap();
        storage.record_ticker(&ticker).unwrap();
        storage
            .record_ticker(&TickerRecord { at: 2, ..ticker })
            .unwrap();
        assert_eq!(storage.ticker_count("XXBTZUSD").unwrap(), 2);
    }

    #[test]
    fn records_the_indicators() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.last_indicators("XXBTZEUR").unwrap(), None);
        let indicators = |close: f64| Indicators {
            close,
            atr_percent: 0.4,
            ema_fast: 101.0,
            ema_slow: 100.0,
            slope_24h_percent: 1.5,
            range_24h_percent: 3.2,
            change_24h_percent: 1.1,
            candles: 96,
        };
        storage
            .record_indicators("XXBTZEUR", 1, &indicators(100.0))
            .unwrap();
        storage
            .record_indicators("XXBTZEUR", 2, &indicators(102.5))
            .unwrap();
        assert_eq!(
            storage.last_indicators("XXBTZEUR").unwrap(),
            Some((2, indicators(102.5)))
        );
        assert_eq!(storage.last_indicators("XETHZEUR").unwrap(), None);
    }
}
