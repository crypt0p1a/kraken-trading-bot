//! The trend and the volatility of a pair over the last 24 hours, from its candles of 15 minutes,
//! with the methods of `yata`. Only logged and recorded for now: no decision is taken on them.
//!
//! In f64, as `yata` computes: they describe the market, the prices of the orders stay decimals.

use kraken_api::market::{Candle, CandleInterval};
use rust_decimal::prelude::ToPrimitive;
use yata::core::Method;
use yata::methods::{EMA, LinReg, RMA};

/// Candles of 15 minutes: 96 for 24 hours
pub const INTERVAL: CandleInterval = CandleInterval::Res15m;
const DAY: usize = 96;
/// The candles asked: 25 hours, the last one being still built
pub const WINDOW_SECONDS: i64 = 25 * 3600;
/// The fewest candles to compute the indicators from: 12 hours
const FEWEST: usize = 48;
/// The true ranges averaged by the ATR (Wilder)
const ATR_PERIOD: u8 = 14;
/// The EMAs of the closes: 3 and 12 hours
const EMA_FAST: u8 = 12;
const EMA_SLOW: u8 = 48;

/// The market of a pair at a tick
#[derive(Debug, Clone, PartialEq)]
pub struct Indicators {
    /// The last close
    pub close: f64,
    /// Average true range of 14 candles, in % of the close: how much a candle moves
    pub atr_percent: f64,
    /// EMA of the closes, 3 hours
    pub ema_fast: f64,
    /// EMA of the closes, 12 hours: under the fast one when the price rises
    pub ema_slow: f64,
    /// Slope of the linear regression of the closes over 24 hours, in % of the close per 24 hours:
    /// the trend, positive when the price rises
    pub slope_24h_percent: f64,
    /// Highest high minus lowest low over 24 hours, in % of the close
    pub range_24h_percent: f64,
    /// The close against the open of 24 hours before, in %
    pub change_24h_percent: f64,
    /// Candles they're computed from, at most 96
    pub candles: usize,
}

impl Indicators {
    /// The trend in words, for the logs
    pub fn trend(&self) -> &'static str {
        match (self.ema_fast > self.ema_slow, self.slope_24h_percent) {
            (true, slope) if slope > 0.0 => "rising",
            (false, slope) if slope < 0.0 => "falling",
            _ => "mixed",
        }
    }
}

fn f(value: rust_decimal::Decimal) -> f64 {
    value.to_f64().unwrap_or_default()
}

/// The indicators of the last 24 hours of closed candles, oldest first. None with less than 12
/// hours of them
pub fn compute(candles: &[Candle]) -> Option<Indicators> {
    let closed: Vec<&Candle> = candles.iter().filter(|it| it.is_closed).collect();
    let day = &closed[closed.len().saturating_sub(DAY)..];
    if day.len() < FEWEST {
        return None;
    }
    let first = day[0];
    let last = day[day.len() - 1];
    let close = f(last.close);
    if close <= 0.0 {
        return None;
    }

    let mut ema_fast = EMA::new(EMA_FAST, &f(first.close)).ok()?;
    let mut ema_slow = EMA::new(EMA_SLOW, &f(first.close)).ok()?;
    let mut regression = LinReg::new(day.len() as u8, &f(first.close)).ok()?;
    let mut atr = RMA::new(ATR_PERIOD, &f(first.high - first.low)).ok()?;
    let (mut fast, mut slow, mut range) = (0.0, 0.0, 0.0);
    let (mut high, mut low) = (f64::MIN, f64::MAX);

    let mut previous_close = f(first.close);
    for candle in day {
        let (candle_high, candle_low, candle_close) =
            (f(candle.high), f(candle.low), f(candle.close));
        fast = ema_fast.next(&candle_close);
        slow = ema_slow.next(&candle_close);
        regression.next(&candle_close);
        // the true range: the gap from the previous close counts
        let true_range = (candle_high - candle_low)
            .max((candle_high - previous_close).abs())
            .max((candle_low - previous_close).abs());
        range = atr.next(&true_range);
        high = high.max(candle_high);
        low = low.min(candle_low);
        previous_close = candle_close;
    }

    Some(Indicators {
        close,
        atr_percent: range / close * 100.0,
        ema_fast: fast,
        ema_slow: slow,
        slope_24h_percent: regression.tan() * day.len() as f64 / close * 100.0,
        range_24h_percent: (high - low) / close * 100.0,
        change_24h_percent: (close / f(first.open) - 1.0) * 100.0,
        candles: day.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn candle(index: usize, open: f64, high: f64, low: f64, close: f64) -> Candle {
        let d = |value: f64| Decimal::from_f64_retain(value).unwrap().round_dp(4);
        Candle {
            time: 1_800_000_000 + index as i64 * 900,
            open: d(open),
            high: d(high),
            low: d(low),
            close: d(close),
            vwap: d(close),
            volume: Decimal::ONE,
            count: 1,
            is_closed: true,
        }
    }

    /// 96 candles from 100, each `step` above the previous one, 1 high and low around it
    fn line(step: f64) -> Vec<Candle> {
        (0..96)
            .map(|index| {
                let close = 100.0 + step * index as f64;
                candle(index, close - step, close + 1.0, close - 1.0, close)
            })
            .collect()
    }

    #[test]
    fn a_rising_market() {
        let indicators = compute(&line(0.1)).unwrap();
        assert_eq!(indicators.candles, 96);
        assert!((indicators.close - 109.5).abs() < 1e-9);
        // +9.5 over 95 candles: the slope over 96 of them, in % of the close
        let expected = 0.1 * 96.0 / 109.5 * 100.0;
        assert!(
            (indicators.slope_24h_percent - expected).abs() < 1e-6,
            "{indicators:?}"
        );
        assert!(indicators.ema_fast > indicators.ema_slow);
        assert_eq!(indicators.trend(), "rising");
        // a range of 2 for each candle
        assert!((indicators.atr_percent - 2.0 / 109.5 * 100.0).abs() < 0.05);
        assert!((indicators.change_24h_percent - (109.5 / 99.9 - 1.0) * 100.0).abs() < 1e-6);
    }

    #[test]
    fn a_falling_market() {
        let indicators = compute(&line(-0.1)).unwrap();
        assert!(indicators.slope_24h_percent < 0.0);
        assert!(indicators.ema_fast < indicators.ema_slow);
        assert_eq!(indicators.trend(), "falling");
        assert!(indicators.change_24h_percent < 0.0);
    }

    #[test]
    fn the_gaps_count_in_the_true_range() {
        // flat closes, then a candle opening 10 above the previous close
        let mut candles: Vec<Candle> = (0..95)
            .map(|index| candle(index, 100.0, 100.5, 99.5, 100.0))
            .collect();
        candles.push(candle(95, 110.0, 110.5, 109.5, 110.0));
        let indicators = compute(&candles).unwrap();
        // the last true range is 10.5, not 1: it weighs 1/14 in the average
        let expected = (13.0 * 1.0 + 10.5) / 14.0;
        assert!(indicators.atr_percent > 1.0 / 110.0 * 100.0);
        assert!(indicators.atr_percent < expected / 110.0 * 100.0 * 1.01);
        assert!((indicators.range_24h_percent - 11.0 / 110.0 * 100.0).abs() < 1e-6);
    }

    #[test]
    fn needs_twelve_hours_of_closed_candles() {
        let mut candles = line(0.1);
        assert!(compute(&candles[..47]).is_none());
        assert!(compute(&candles[..48]).is_some());

        // the candle still being built is left out
        candles.push(candle(96, 200.0, 300.0, 100.0, 200.0));
        candles[96].is_closed = false;
        let indicators = compute(&candles).unwrap();
        assert!((indicators.close - 109.5).abs() < 1e-9);

        // only the last 24 hours
        let mut longer = line(0.1);
        longer.splice(0..0, (0..50).map(|index| candle(index, 1.0, 1.0, 1.0, 1.0)));
        assert_eq!(compute(&longer).unwrap().candles, 96);
    }
}
