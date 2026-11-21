//! What the dashboard reads from the database of the bot, opened read only: the bot keeps
//! writing it while the dashboard is open.
//!
//! The series of the charts (the values of the managers, the prices) are bucketed by SQLite, so a
//! range of months costs as much as an hour.
//!
//! | Module | What's read |
//! |--------|-------------|
//! | [`database`] | the opening, the schema, what's read at once, the tick asked for |
//! | `accounts` | what the accounts are worth, alone and added up |
//! | `managers` | the managers and their history |
//! | `orders` | the orders and the actions |
//! | `markets` | the tickers, their candles and their indicators |

mod accounts;
pub mod database;
mod managers;
mod markets;
mod orders;

pub use database::Database;

use rusqlite::Row;
use rust_decimal::Decimal;
use std::str::FromStr;

/// Points of a series in a chart, at most
pub const POINTS: i64 = 480;

/// A manager among those of every account: the same id runs on each of them. The key of the first
/// account is the id itself
pub type ManagerKey = i64;

/// The id of the manager in its account, as the YAML file and Kraken name it
pub fn manager_number(key: ManagerKey) -> i32 {
    key as i32
}

/// The id of the account of the manager
pub fn account_of(key: ManagerKey) -> i64 {
    (key >> 32) + 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub txid: String,
    pub manager_id: ManagerKey,
    /// Alternate name of the pair, e.g. "XBTUSD"
    pub pair: String,
    pub side: Side,
    /// pending, open, closed, canceled, expired
    pub status: String,
    pub is_final: bool,
    pub volume: Decimal,
    pub volume_executed: Decimal,
    /// Quote paid or received, fees excluded
    pub cost: Decimal,
    /// In the quote
    pub fee: Decimal,
    pub limit_price: Option<Decimal>,
    pub average_price: Option<Decimal>,
    /// UTC seconds, as Kraken gives them
    pub open_time: f64,
    pub close_time: Option<f64>,
}

impl Order {
    /// What the table shows: the average price once something is executed, the limit before
    pub fn shown_price(&self) -> f64 {
        self.average_price
            .filter(|_| !self.volume_executed.is_zero())
            .or(self.limit_price)
            .map(to_f64)
            .unwrap_or_default()
    }

    /// Share of the volume executed, from 0 to 1
    pub fn filled(&self) -> f64 {
        if self.volume.is_zero() {
            return 0.0;
        }
        to_f64(self.volume_executed / self.volume).clamp(0.0, 1.0)
    }
}

/// The last state of a manager, saved by the bot at each tick
#[derive(Debug, Clone, PartialEq)]
pub struct Manager {
    pub id: ManagerKey,
    /// The pair as configured, e.g. "XBTUSD", "XBT/USD" or "XXBTZUSD"
    pub pair: String,
    pub budget: Decimal,
    pub buy_below: Decimal,
    pub sell_above: Decimal,
    /// active or closing
    pub mode: String,
    pub quote_available: Decimal,
    pub quote_in_buy_order: Decimal,
    pub base_available: Decimal,
    pub base_in_sell_order: Decimal,
    pub position_cost: Decimal,
    pub entry_price: Option<Decimal>,
    pub last_price: Decimal,
    pub value: Decimal,
    /// UTC milliseconds
    pub first_seen: i64,
    pub updated_at: i64,
    /// The manager of the YAML file a virtual manager copies
    pub template_id: Option<i32>,
    /// When the bot deleted the virtual manager
    pub deleted_at: Option<i64>,
    /// The crypto a virtual manager started with instead of its budget, as bought with it
    pub holding: Option<Decimal>,
}

impl Manager {
    /// The id of the manager in its account
    pub fn number(&self) -> i32 {
        manager_number(self.id)
    }

    pub fn base(&self) -> Decimal {
        self.base_available + self.base_in_sell_order
    }

    pub fn pnl(&self) -> Decimal {
        self.value - self.budget
    }

    /// What the manager would be worth once its open buy filled and all it holds sold, as the
    /// strategy sells: for what the position cost + `sell_above`%, fees included
    pub fn target(&self) -> Decimal {
        let cost = self.position_cost + self.quote_in_buy_order;
        if cost.is_zero() {
            return self.value;
        }
        self.quote_available + cost * (Decimal::ONE + self.sell_above / Decimal::ONE_HUNDRED)
    }

    pub fn pnl_percent(&self) -> f64 {
        percent(self.pnl(), self.budget)
    }
}

/// A point of the history of a manager
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    pub manager_id: ManagerKey,
    /// UTC milliseconds
    pub at: i64,
    pub value: f64,
    /// What its base is worth, at the last price
    pub held: f64,
}

/// A decision of a manager, and what came of it
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub manager_id: ManagerKey,
    /// UTC milliseconds
    pub at: i64,
    /// buy, sell, idle, buy_cancelled, virtual_created, virtual_deleted…
    pub kind: String,
    pub amount: Option<Decimal>,
    pub price: Option<Decimal>,
    pub reason: Option<String>,
    pub dry_run: bool,
    pub txid: Option<String>,
    pub error: Option<String>,
}

/// The trend and the volatility of a pair over 24 hours
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Indicators {
    pub atr_percent: f64,
    pub ema_fast: f64,
    pub ema_slow: f64,
    pub slope_24h_percent: f64,
    pub range_24h_percent: f64,
    pub change_24h_percent: f64,
}

impl Indicators {
    /// 1 rising, -1 falling, 0 mixed: the words of the bot
    pub fn trend(&self) -> i8 {
        match (self.ema_fast > self.ema_slow, self.slope_24h_percent) {
            (true, slope) if slope > 0.0 => 1,
            (false, slope) if slope < 0.0 => -1,
            _ => 0,
        }
    }
}

/// The last ticker of a pair
#[derive(Debug, Clone, PartialEq)]
pub struct Market {
    /// Kraken name of the pair, e.g. "XXBTZUSD"
    pub key: String,
    /// UTC milliseconds
    pub at: i64,
    pub last: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    pub volume_24h: Decimal,
    pub vwap_24h: Decimal,
    /// None for the databases older than the indicators
    pub indicators: Option<Indicators>,
    /// The prices of the range, oldest first: (UTC milliseconds, last)
    pub prices: Vec<(i64, f64)>,
    /// The ticks of the range bucketed, oldest first: the closes are the prices
    pub candles: Vec<Candle>,
    /// The indicators of the range, oldest first: empty for the databases older than them
    pub trend: Vec<TrendPoint>,
}

/// The ticks of a bucket of the range: the first price, the extremes, the last one, with the book
/// and the volume of the last tick
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candle {
    /// UTC milliseconds of the last tick
    pub at: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub bid: f64,
    pub ask: f64,
    /// What was traded over the 24 hours before, in the base
    pub volume_24h: f64,
    pub vwap_24h: f64,
}

impl Candle {
    /// The gap between the ask and the bid, in basis points of the middle
    pub fn spread_bps(&self) -> f64 {
        let middle = (self.bid + self.ask) / 2.0;
        if middle > 0.0 {
            (self.ask - self.bid) / middle * 10_000.0
        } else {
            0.0
        }
    }
}

/// The indicators of a pair at a tick
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrendPoint {
    /// UTC milliseconds
    pub at: i64,
    pub ema_fast: f64,
    pub ema_slow: f64,
    pub atr_percent: f64,
}

/// What the whole account is worth in a quote currency, managers or not
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Kraken name of the asset, e.g. "ZEUR"
    pub quote: String,
    /// UTC milliseconds
    pub at: i64,
    /// The quote in the account, held by orders or not
    pub cash: Decimal,
    /// The bases of the pairs of the managers in this quote, at their last price
    pub holdings: Decimal,
}

impl Account {
    pub fn value(&self) -> Decimal {
        self.cash + self.holdings
    }
}

/// What the whole account was worth in a quote currency at a tick
#[derive(Debug, Clone, PartialEq)]
pub struct AccountPoint {
    /// Kraken name of the asset, e.g. "ZEUR"
    pub quote: String,
    /// UTC milliseconds
    pub at: i64,
    pub value: f64,
    /// The quote in the account, held by orders or not
    pub cash: f64,
}

/// A Kraken account on its own, whatever is shown, to compare it with the others
#[derive(Debug, Clone, PartialEq)]
pub struct AccountLine {
    pub name: String,
    /// Its managers: (key, pair, budget)
    pub managers: Vec<(ManagerKey, String, f64)>,
    /// The range only, one point per bucket and manager, oldest first
    pub snapshots: Vec<Snapshot>,
    /// What it was worth over the range, per quote
    pub series: Vec<AccountPoint>,
}

/// Everything shown, read at once
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Data {
    pub managers: Vec<Manager>,
    /// Oldest first
    pub orders: Vec<Order>,
    /// The range only, one point per bucket and manager, oldest first
    pub snapshots: Vec<Snapshot>,
    /// Most recent first
    pub actions: Vec<Action>,
    pub markets: Vec<Market>,
    /// The last value of the account per quote; empty for the databases older than it
    pub accounts: Vec<Account>,
    /// The range only, one point per bucket and quote, oldest first
    pub account_series: Vec<AccountPoint>,
    /// Each account on its own, in the order of their ids
    pub account_lines: Vec<AccountLine>,
    /// Whether the last order was only validated by Kraken (dry run), None before any order
    pub dry_run: Option<bool>,
    /// The state of the tick asked for from here: "idle", "refresh" (asked for), "pending" (under
    /// way). None for the databases older than it
    pub forced_tick: Option<String>,
    /// The Kraken accounts of the bot, whatever is shown: (id, name). Empty for the databases older
    /// than them
    pub names: Vec<(i64, String)>,
    /// Version of the schema of the bot
    pub version: usize,
    /// Start of the range, UTC milliseconds
    pub since: i64,
}

fn invalid(index: usize, text: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("not a decimal: {text}").into(),
    )
}

fn decimal(row: &Row, index: usize) -> rusqlite::Result<Decimal> {
    let text: String = row.get(index)?;
    Decimal::from_str(&text).map_err(|_| invalid(index, &text))
}

fn optional_decimal(row: &Row, index: usize) -> rusqlite::Result<Option<Decimal>> {
    let text: Option<String> = row.get(index)?;
    text.map(|it| Decimal::from_str(&it).map_err(|_| invalid(index, &it)))
        .transpose()
}

pub fn to_f64(value: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    value.to_f64().unwrap_or_default()
}

/// `part` in % of `whole`, 0 when `whole` is 0
pub fn percent(part: Decimal, whole: Decimal) -> f64 {
    if whole.is_zero() {
        0.0
    } else {
        to_f64(part / whole * Decimal::ONE_HUNDRED)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;

    #[test]
    fn splits_the_manager_keys() {
        assert_eq!(manager_number(7), 7);
        assert_eq!(account_of(7), 1);
        let key = (2_i64 << 32) | 1_001_007;
        assert_eq!(manager_number(key), 1_001_007);
        assert_eq!(account_of(key), 3);
        assert_eq!(manager_number((1_i64 << 32) | 2_147_483_647), 2_147_483_647);
    }

    /// The database of a bot from before the accounts, the virtual managers, the indicators, the
    /// account snapshots and the commands: its tables as they were
    const OLDEST: &str = "
        CREATE TABLE orders (txid TEXT, manager_id INTEGER, pair TEXT, side TEXT, status TEXT,
            is_final INTEGER, volume TEXT, volume_executed TEXT, cost TEXT, fee TEXT,
            limit_price TEXT, average_price TEXT, open_time REAL, close_time REAL);
        CREATE TABLE managers (id INTEGER, pair TEXT, budget TEXT, buy_below TEXT,
            sell_above TEXT, mode TEXT, quote_available TEXT, quote_in_buy_order TEXT,
            base_available TEXT, base_in_sell_order TEXT, position_cost TEXT, entry_price TEXT,
            last_price TEXT, value TEXT, first_seen INTEGER, updated_at INTEGER);
        CREATE TABLE manager_snapshots (manager_id INTEGER, at INTEGER, value TEXT,
            base_available TEXT, base_in_sell_order TEXT, last_price TEXT);
        CREATE TABLE actions (id INTEGER PRIMARY KEY, manager_id INTEGER, at INTEGER, kind TEXT,
            amount TEXT, price TEXT, reason TEXT, dry_run INTEGER, txid TEXT, error TEXT);
        CREATE TABLE tickers (pair TEXT, at INTEGER, last TEXT, bid TEXT, ask TEXT,
            volume_24h TEXT, vwap_24h TEXT);
        INSERT INTO managers VALUES (3, 'XBTEUR', '20', '0.5', '0.8', 'active', '10', '0', '0.0002',
            '0', '10', '50000', '51000', '20.2', 1000, 2000);
        INSERT INTO manager_snapshots VALUES (3, 1000, '20', '0', '0', '50000');
        INSERT INTO manager_snapshots VALUES (3, 2000, '20.2', '0.0002', '0', '51000');
        INSERT INTO orders VALUES ('O-1', 3, 'XBTEUR', 'buy', 'closed', 1, '0.0002', '0.0002',
            '10', '0.04', '50000', '50000', 1.5, 1.8);
        INSERT INTO actions VALUES (1, 3, 1500, 'buy', '0.0002', '50000', 'below', 1, NULL, NULL);
        INSERT INTO tickers VALUES ('XXBTZEUR', 2000, '51000', '50990', '51010', '12', '50500');
    ";

    fn oldest(more: &str) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(OLDEST).unwrap();
        connection.execute_batch(more).unwrap();
        Database::with(connection, Path::new(":memory:")).unwrap()
    }

    #[test]
    fn reads_a_database_from_before_the_accounts() {
        let database = oldest("");
        let data = database.load(0, 3000, Some(1)).unwrap();

        // the key of the only account is the id itself
        assert_eq!(data.managers.len(), 1);
        assert_eq!(data.managers[0].id, 3);
        assert_eq!(data.managers[0].template_id, None);
        assert_eq!(data.managers[0].holding, None);
        assert_eq!(data.orders[0].manager_id, 3);
        assert_eq!(data.actions[0].manager_id, 3);
        assert_eq!(data.dry_run, Some(true));
        // the range starts at the first snapshot
        assert_eq!(data.since, 1000);
        assert_eq!(data.snapshots.len(), 2);
        // a single, implicit account, which nothing else of it was recorded for
        assert!(data.names.is_empty());
        assert_eq!(data.account_lines.len(), 1);
        assert_eq!(data.account_lines[0].name, "main");
        assert_eq!(data.account_lines[0].managers.len(), 1);
        assert!(data.accounts.is_empty());
        assert!(data.account_series.is_empty());
        assert_eq!(data.forced_tick, None);
        // the prices, without the indicators
        assert_eq!(data.markets.len(), 1);
        assert_eq!(data.markets[0].indicators, None);
        assert!(data.markets[0].trend.is_empty());
        assert_eq!(data.markets[0].prices, [(2000, 51000.0)]);
        // nothing to ask a tick of
        assert!(database.request_tick(3000, None).is_err());
    }

    #[test]
    fn reads_the_virtual_managers_from_before_their_holdings() {
        let database = oldest(
            "CREATE TABLE virtual_managers (id INTEGER, template_id INTEGER, deleted_at INTEGER);
             INSERT INTO managers SELECT 1000003, pair, budget, buy_below, sell_above, mode,
                quote_available, quote_in_buy_order, base_available, base_in_sell_order,
                position_cost, entry_price, last_price, value, first_seen, updated_at
             FROM managers;
             INSERT INTO virtual_managers VALUES (1000003, 3, NULL);",
        );
        let managers = database.load(0, 3000, None).unwrap().managers;
        let copies: Vec<_> = managers.iter().map(|it| (it.id, it.template_id)).collect();
        assert_eq!(copies, [(3, None), (1_000_003, Some(3))]);
        assert_eq!(managers[1].holding, None);
    }

    #[test]
    fn refuses_an_amount_which_isnt_a_decimal() {
        let database = oldest("UPDATE managers SET budget = 'twenty'");
        let error = database.load(0, 3000, None).unwrap_err();
        assert!(error.starts_with("database: "), "{error}");
        assert!(error.contains("not a decimal: twenty"), "{error}");

        let database = oldest("UPDATE orders SET limit_price = '50 000'");
        let error = database.load(0, 3000, None).unwrap_err();
        assert!(error.contains("not a decimal: 50 000"), "{error}");
    }

    #[test]
    fn an_order_without_volume_has_nothing_filled() {
        let database = oldest("UPDATE orders SET volume = '0', volume_executed = '0'");
        let order = &database.load(0, 3000, None).unwrap().orders[0];
        assert_eq!(order.filled(), 0.0);
        // nothing executed: the limit
        assert_eq!(order.shown_price(), 50000.0);
    }

    #[test]
    fn a_book_without_prices_has_no_spread() {
        let candle = Candle {
            at: 0,
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            bid: 0.0,
            ask: 0.0,
            volume_24h: 0.0,
            vwap_24h: 0.0,
        };
        assert_eq!(candle.spread_bps(), 0.0);
        let candle = Candle {
            bid: 99.0,
            ask: 101.0,
            ..candle
        };
        assert_eq!(candle.spread_bps(), 200.0);
    }

    #[test]
    fn values_a_part_of_nothing_at_zero() {
        assert_eq!(percent(Decimal::ONE, Decimal::ZERO), 0.0);
        assert_eq!(percent(Decimal::ONE, Decimal::TWO), 50.0);
    }
}
