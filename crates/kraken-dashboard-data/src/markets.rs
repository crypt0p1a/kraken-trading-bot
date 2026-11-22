//! The last ticker of each pair, its candles over the range and its indicators

use super::{Candle, Database, Indicators, Market, TrendPoint, decimal};
use rusqlite::params;

impl Database {
    pub(super) fn markets(&self, since: i64, bucket: i64) -> rusqlite::Result<Vec<Market>> {
        let mut markets = {
            let mut statement = self.connection.prepare_cached(
                "SELECT pair, MAX(at), last, bid, ask, volume_24h, vwap_24h FROM tickers
                 GROUP BY pair ORDER BY pair",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(Market {
                    key: row.get(0)?,
                    at: row.get(1)?,
                    last: decimal(row, 2)?,
                    bid: decimal(row, 3)?,
                    ask: decimal(row, 4)?,
                    volume_24h: decimal(row, 5)?,
                    vwap_24h: decimal(row, 6)?,
                    indicators: None,
                    prices: Vec::new(),
                    candles: Vec::new(),
                    trend: Vec::new(),
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // the first and the last tick of each bucket found again by their key, (pair, at)
        let mut candles = self.connection.prepare_cached(
            "WITH buckets AS (
                SELECT MIN(at) AS first, MAX(at) AS last, MIN(CAST(last AS REAL)) AS low,
                    MAX(CAST(last AS REAL)) AS high
                FROM tickers WHERE pair = ?1 AND at >= ?2 GROUP BY at / ?3
             )
             SELECT b.last, CAST(o.last AS REAL), b.high, b.low, CAST(c.last AS REAL),
                CAST(c.bid AS REAL), CAST(c.ask AS REAL), CAST(c.volume_24h AS REAL),
                CAST(c.vwap_24h AS REAL)
             FROM buckets b
             JOIN tickers o ON o.pair = ?1 AND o.at = b.first
             JOIN tickers c ON c.pair = ?1 AND c.at = b.last
             ORDER BY 1",
        )?;
        for market in &mut markets {
            market.candles = candles
                .query_map(params![market.key, since, bucket], |row| {
                    Ok(Candle {
                        at: row.get(0)?,
                        open: row.get(1)?,
                        high: row.get(2)?,
                        low: row.get(3)?,
                        close: row.get(4)?,
                        bid: row.get(5)?,
                        ask: row.get(6)?,
                        volume_24h: row.get(7)?,
                        vwap_24h: row.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            market.prices = market.candles.iter().map(|it| (it.at, it.close)).collect();
        }

        if self.tables.contains("indicators") {
            let mut statement = self.connection.prepare_cached(
                "SELECT atr_percent, ema_fast, ema_slow, slope_24h_percent, range_24h_percent,
                    change_24h_percent FROM indicators WHERE pair = ?1 ORDER BY at DESC LIMIT 1",
            )?;
            let mut trend = self.connection.prepare_cached(
                "SELECT MAX(at), ema_fast, ema_slow, atr_percent FROM indicators
                 WHERE pair = ?1 AND at >= ?2 GROUP BY at / ?3 ORDER BY 1",
            )?;
            for market in &mut markets {
                let mut rows = statement.query([&market.key])?;
                if let Some(row) = rows.next()? {
                    market.indicators = Some(Indicators {
                        atr_percent: row.get(0)?,
                        ema_fast: row.get(1)?,
                        ema_slow: row.get(2)?,
                        slope_24h_percent: row.get(3)?,
                        range_24h_percent: row.get(4)?,
                        change_24h_percent: row.get(5)?,
                    });
                }
                market.trend = trend
                    .query_map(params![market.key, since, bucket], |row| {
                        Ok(TrendPoint {
                            at: row.get(0)?,
                            ema_fast: row.get(1)?,
                            ema_slow: row.get(2)?,
                            atr_percent: row.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
            }
        }
        Ok(markets)
    }
}
