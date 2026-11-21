//! What the page of the markets computes from the ticks of a pair: the pairs in the order of the
//! YAML file, the candles at the width of the screen, the returns and their distribution, the
//! drawdown, the fills and the levels of the managers

use crate::data::{Candle, ManagerKey, Market, Side, manager_number, to_f64};
use crate::stats::{Model, canonical};

/// A price of the managers on the pair: an open order, or what a position cost
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub price: f64,
    /// None for an entry price
    pub side: Option<Side>,
    /// In the quote: what the order buys or sells, what the position cost
    pub amount: f64,
    pub manager_id: ManagerKey,
}

/// A buy or a sell of the pair, at its time
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    /// UTC milliseconds
    pub at: i64,
    pub price: f64,
    pub side: Side,
}

/// The values in bins of the same width, from the lowest to the highest
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub low: f64,
    pub width: f64,
    pub counts: Vec<usize>,
}

impl Histogram {
    /// The middle of a bin
    pub fn middle(&self, bin: usize) -> f64 {
        self.low + self.width * (bin as f64 + 0.5)
    }
}

impl Model {
    /// The markets in the order of the YAML file: a pair comes where its first manager is, the
    /// smallest id of the managers of the file. The virtual managers count only for a pair no
    /// manager of the file trades. A pair without a manager, ticks left from an older file, is
    /// left out, unless no pair has a manager: then every pair, by its name
    pub fn pairs(&self) -> Vec<usize> {
        let first = |index: usize, virtuals: bool| {
            self.data
                .managers
                .iter()
                .filter(|it| it.template_id.is_some() == virtuals)
                .filter(|it| self.markets.get(&it.id) == Some(&index))
                .map(|it| manager_number(it.id))
                .min()
        };
        let mut order: Vec<usize> = (0..self.data.markets.len()).collect();
        order.sort_by_key(|index| {
            let rank = match (first(*index, false), first(*index, true)) {
                (Some(id), _) => (0, id),
                (None, Some(id)) => (1, id),
                (None, None) => (2, 0),
            };
            (rank, canonical(&self.data.markets[*index].key))
        });
        let traded: Vec<usize> = order
            .iter()
            .copied()
            .filter(|index| !self.managers_of_market(*index).is_empty())
            .collect();
        if traded.is_empty() { order } else { traded }
    }

    /// The managers trading the market of this index
    pub fn managers_of_market(&self, index: usize) -> Vec<ManagerKey> {
        self.data
            .managers
            .iter()
            .filter(|it| self.markets.get(&it.id) == Some(&index))
            .map(|it| it.id)
            .collect()
    }

    /// The buys and the sells of the pair executed since `since`, oldest first
    pub fn fills(&self, market: &Market, since: i64) -> Vec<Fill> {
        let pair = canonical(&market.key);
        let mut fills: Vec<Fill> = self
            .data
            .orders
            .iter()
            .filter(|it| canonical(&it.pair) == pair && !it.volume_executed.is_zero())
            .filter_map(|it| {
                let at = (it.close_time.unwrap_or(it.open_time) * 1000.0) as i64;
                let price = to_f64(it.average_price.or(it.limit_price)?);
                (at >= since).then_some(Fill {
                    at,
                    price,
                    side: it.side,
                })
            })
            .collect();
        fills.sort_by_key(|it| it.at);
        fills
    }

    /// The open orders of the managers of the market, and what their positions cost, the highest
    /// price first
    pub fn levels(&self, index: usize) -> Vec<Level> {
        let ids = self.managers_of_market(index);
        let mut levels: Vec<Level> = Vec::new();
        for manager in self.data.managers.iter().filter(|it| ids.contains(&it.id)) {
            if let Some(entry) = manager.entry_price.filter(|_| !manager.base().is_zero()) {
                levels.push(Level {
                    price: to_f64(entry),
                    side: None,
                    amount: to_f64(manager.position_cost),
                    manager_id: manager.id,
                });
            }
            for order in self.open_orders_of(manager.id) {
                let Some(price) = order.limit_price.map(to_f64) else {
                    continue;
                };
                levels.push(Level {
                    price,
                    side: Some(order.side),
                    amount: to_f64(order.volume - order.volume_executed) * price,
                    manager_id: manager.id,
                });
            }
        }
        levels.sort_by(|a, b| b.price.total_cmp(&a.price));
        levels
    }
}

/// The candles merged to `count` at most, each of the same number of the ones read: as many as the
/// chart has columns
pub fn merged(candles: &[Candle], count: usize) -> Vec<Candle> {
    if candles.len() <= count || count == 0 {
        return candles.to_vec();
    }
    let size = candles.len().div_ceil(count);
    candles
        .chunks(size)
        .map(|chunk| {
            let last = chunk[chunk.len() - 1];
            Candle {
                open: chunk[0].open,
                high: chunk.iter().map(|it| it.high).fold(f64::MIN, f64::max),
                low: chunk.iter().map(|it| it.low).fold(f64::MAX, f64::min),
                ..last
            }
        })
        .collect()
}

/// The change of each candle from the close of the one before, in %
pub fn returns(candles: &[Candle]) -> Vec<f64> {
    candles
        .windows(2)
        .filter(|it| it[0].close > 0.0)
        .map(|it| (it[1].close - it[0].close) / it[0].close * 100.0)
        .collect()
}

/// The values in `bins` bins, from the lowest to the highest; None without a value
pub fn histogram(values: &[f64], bins: usize) -> Option<Histogram> {
    if values.is_empty() || bins == 0 {
        return None;
    }
    let low = values.iter().copied().fold(f64::MAX, f64::min);
    let high = values.iter().copied().fold(f64::MIN, f64::max);
    let width = ((high - low) / bins as f64).max(1e-9);
    let mut counts = vec![0; bins];
    for value in values {
        let bin = (((value - low) / width) as usize).min(bins - 1);
        counts[bin] += 1;
    }
    Some(Histogram { low, width, counts })
}

/// How far each close is below the highest price before it, in % (0 or less)
pub fn drawdown(candles: &[Candle]) -> Vec<(i64, f64)> {
    let mut peak = f64::MIN;
    candles
        .iter()
        .map(|it| {
            peak = peak.max(it.high);
            let below = if peak > 0.0 {
                (it.close - peak) / peak * 100.0
            } else {
                0.0
            };
            (it.at, below.min(0.0))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(at: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            at,
            open,
            high,
            low,
            close,
            bid: close - 1.0,
            ask: close + 1.0,
            volume_24h: 10.0,
            vwap_24h: close,
        }
    }

    #[test]
    fn merges_the_candles_to_the_width_of_the_chart() {
        let candles = [
            candle(1, 10.0, 12.0, 9.0, 11.0),
            candle(2, 11.0, 15.0, 10.0, 14.0),
            candle(3, 14.0, 14.0, 7.0, 8.0),
            candle(4, 8.0, 9.0, 8.0, 9.0),
            candle(5, 9.0, 10.0, 9.0, 10.0),
        ];
        let merged = merged(&candles, 2);
        assert_eq!(merged.len(), 2);
        // the first open, the extremes, the last close and the time of the last
        assert_eq!(merged[0], candle(3, 10.0, 15.0, 7.0, 8.0));
        assert_eq!(merged[1], candle(5, 8.0, 10.0, 8.0, 10.0));
        // room enough: as read
        assert_eq!(super::merged(&candles, 9), candles);
        assert_eq!(super::merged(&candles, 0), candles);
    }

    #[test]
    fn counts_the_returns_in_bins() {
        let candles = [
            candle(1, 100.0, 100.0, 100.0, 100.0),
            candle(2, 100.0, 110.0, 100.0, 110.0),
            candle(3, 110.0, 110.0, 99.0, 99.0),
            candle(4, 99.0, 99.0, 99.0, 99.0),
        ];
        let returns = returns(&candles);
        assert_eq!(returns.len(), 3);
        assert!((returns[0] - 10.0).abs() < 1e-9);
        assert!((returns[1] + 10.0).abs() < 1e-9);
        assert_eq!(returns[2], 0.0);

        let histogram = histogram(&returns, 4).unwrap();
        assert_eq!(histogram.counts, [1, 0, 1, 1]);
        assert!((histogram.low + 10.0).abs() < 1e-9);
        assert!((histogram.middle(0) + 7.5).abs() < 1e-9);
        // the same value everywhere: one bin, never a division by zero
        assert_eq!(super::histogram(&[1.0, 1.0], 3).unwrap().counts, [2, 0, 0]);
        assert_eq!(super::histogram(&[], 3), None);
    }

    #[test]
    fn measures_the_drawdown_from_the_highest_before() {
        let candles = [
            candle(1, 100.0, 100.0, 100.0, 100.0),
            candle(2, 100.0, 120.0, 100.0, 110.0),
            candle(3, 110.0, 110.0, 90.0, 90.0),
            candle(4, 90.0, 125.0, 90.0, 125.0),
        ];
        let drawdown = drawdown(&candles);
        assert_eq!(drawdown[0], (1, 0.0));
        assert!((drawdown[1].1 - (110.0 - 120.0) / 120.0 * 100.0).abs() < 1e-9);
        assert!((drawdown[2].1 + 25.0).abs() < 1e-9);
        assert_eq!(drawdown[3], (4, 0.0));
    }

    #[test]
    fn spreads_in_basis_points() {
        let candle = candle(1, 100.0, 100.0, 100.0, 100.0);
        // 99 to 101: 2 of a middle of 100
        assert!((candle.spread_bps() - 200.0).abs() < 1e-9);
    }
}
