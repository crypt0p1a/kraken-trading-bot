//! What the dashboard reads from the database of the bot, opened read only: the bot keeps
//! writing it while the dashboard is open.
//!
//! The series of the charts (the values of the managers, the prices) are bucketed by SQLite, so a
//! range of months costs as much as an hour.

use rusqlite::{Connection, OpenFlags, Row, params};
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

/// The tables without which there's nothing to show
const REQUIRED: [&str; 5] = [
    "orders",
    "managers",
    "manager_snapshots",
    "actions",
    "tickers",
];
/// Points of a series in a chart, at most
pub const POINTS: i64 = 480;
/// The last actions read, for the tape: the idle ones apart, the bot writes one per manager and tick
const ACTIONS: usize = 300;
const IDLE_ACTIONS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub txid: String,
    pub manager_id: i32,
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
    pub id: i32,
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
}

impl Manager {
    pub fn base(&self) -> Decimal {
        self.base_available + self.base_in_sell_order
    }

    pub fn pnl(&self) -> Decimal {
        self.value - self.budget
    }

    pub fn pnl_percent(&self) -> f64 {
        percent(self.pnl(), self.budget)
    }
}

/// A point of the history of a manager
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    pub manager_id: i32,
    /// UTC milliseconds
    pub at: i64,
    pub value: f64,
}

/// A decision of a manager, and what came of it
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub manager_id: i32,
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
    /// Whether the last order was only validated by Kraken (dry run), None before any order
    pub dry_run: Option<bool>,
    /// Version of the schema of the bot
    pub version: usize,
    /// Start of the range, UTC milliseconds
    pub since: i64,
}

pub struct Database {
    connection: Connection,
    tables: HashSet<String>,
    version: usize,
}

type Result<T> = std::result::Result<T, String>;

fn sql(error: rusqlite::Error) -> String {
    format!("database: {error}")
}

impl Database {
    /// Opens the database of the bot, read only
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(format!(
                "no database at {} (the path of KRAKEN_BOT_DATABASE)",
                path.display()
            ));
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        // the bot may be writing: wait for it rather than failing
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql)?;
        Self::with(connection)
    }

    fn with(connection: Connection) -> Result<Self> {
        let tables = {
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .map_err(sql)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sql)?
                .collect::<rusqlite::Result<HashSet<_>>>()
                .map_err(sql)?
        };
        if let Some(missing) = REQUIRED.iter().find(|it| !tables.contains(**it)) {
            return Err(format!(
                "not a database of kraken-trading-bot: no table {missing}"
            ));
        }
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sql)?;
        Ok(Self {
            connection,
            tables,
            version: version as usize,
        })
    }

    /// Changes each time another connection, the bot, commits
    pub fn data_version(&self) -> Result<i64> {
        self.connection
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(sql)
    }

    /// Reads everything, the series from `since` (UTC milliseconds, 0 for all) to `now`
    pub fn load(&self, since: i64, now: i64) -> Result<Data> {
        // a single read transaction: a tick of the bot is seen whole, or not at all
        let transaction = self.connection.unchecked_transaction().map_err(sql)?;

        let start = if since > 0 {
            since
        } else {
            transaction
                .query_row("SELECT MIN(at) FROM manager_snapshots", [], |row| {
                    row.get::<_, Option<i64>>(0)
                })
                .map_err(sql)?
                .unwrap_or(now)
        };
        let bucket = ((now - start) / POINTS).max(1);

        let data = Data {
            managers: self.managers().map_err(sql)?,
            orders: self.orders().map_err(sql)?,
            snapshots: self.snapshots(start, bucket).map_err(sql)?,
            actions: self.actions().map_err(sql)?,
            dry_run: self.dry_run().map_err(sql)?,
            markets: self.markets(start, bucket).map_err(sql)?,
            accounts: self.accounts().map_err(sql)?,
            version: self.version,
            since: start,
        };
        transaction.finish().map_err(sql)?;
        Ok(data)
    }

    fn accounts(&self) -> rusqlite::Result<Vec<Account>> {
        if !self.tables.contains("account_snapshots") {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare_cached(
            "SELECT quote, at, cash, holdings FROM account_snapshots
             WHERE at = (SELECT MAX(at) FROM account_snapshots) ORDER BY quote",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Account {
                quote: row.get(0)?,
                at: row.get(1)?,
                cash: decimal(row, 2)?,
                holdings: decimal(row, 3)?,
            })
        })?;
        rows.collect()
    }

    fn managers(&self) -> rusqlite::Result<Vec<Manager>> {
        let virtuals = self.tables.contains("virtual_managers");
        let query = format!(
            "SELECT m.id, m.pair, m.budget, m.buy_below, m.sell_above, m.mode, m.quote_available,
                m.quote_in_buy_order, m.base_available, m.base_in_sell_order, m.position_cost,
                m.entry_price, m.last_price, m.value, m.first_seen, m.updated_at, {}
             FROM managers m {} ORDER BY m.id",
            if virtuals {
                "v.template_id, v.deleted_at"
            } else {
                "NULL, NULL"
            },
            if virtuals {
                "LEFT JOIN virtual_managers v ON v.id = m.id"
            } else {
                ""
            },
        );
        let mut statement = self.connection.prepare_cached(&query)?;
        let rows = statement.query_map([], |row| {
            Ok(Manager {
                id: row.get(0)?,
                pair: row.get(1)?,
                budget: decimal(row, 2)?,
                buy_below: decimal(row, 3)?,
                sell_above: decimal(row, 4)?,
                mode: row.get(5)?,
                quote_available: decimal(row, 6)?,
                quote_in_buy_order: decimal(row, 7)?,
                base_available: decimal(row, 8)?,
                base_in_sell_order: decimal(row, 9)?,
                position_cost: decimal(row, 10)?,
                entry_price: optional_decimal(row, 11)?,
                last_price: decimal(row, 12)?,
                value: decimal(row, 13)?,
                first_seen: row.get(14)?,
                updated_at: row.get(15)?,
                template_id: row.get(16)?,
                deleted_at: row.get(17)?,
            })
        })?;
        rows.collect()
    }

    fn orders(&self) -> rusqlite::Result<Vec<Order>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT txid, manager_id, pair, side, status, is_final, volume, volume_executed, cost,
                fee, limit_price, average_price, open_time, close_time
             FROM orders ORDER BY open_time, txid",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Order {
                txid: row.get(0)?,
                manager_id: row.get(1)?,
                pair: row.get(2)?,
                side: if row.get::<_, String>(3)? == "sell" {
                    Side::Sell
                } else {
                    Side::Buy
                },
                status: row.get(4)?,
                is_final: row.get(5)?,
                volume: decimal(row, 6)?,
                volume_executed: decimal(row, 7)?,
                cost: decimal(row, 8)?,
                fee: decimal(row, 9)?,
                limit_price: optional_decimal(row, 10)?,
                average_price: optional_decimal(row, 11)?,
                open_time: row.get(12)?,
                close_time: row.get(13)?,
            })
        })?;
        rows.collect()
    }

    fn snapshots(&self, since: i64, bucket: i64) -> rusqlite::Result<Vec<Snapshot>> {
        // the bare column of SQLite: `value` comes from the row of MAX(at)
        let mut statement = self.connection.prepare_cached(
            "SELECT manager_id, MAX(at), CAST(value AS REAL) FROM manager_snapshots
             WHERE at >= ?1 GROUP BY manager_id, at / ?2 ORDER BY 2",
        )?;
        let rows = statement.query_map(params![since, bucket], |row| {
            Ok(Snapshot {
                manager_id: row.get(0)?,
                at: row.get(1)?,
                value: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    fn actions(&self) -> rusqlite::Result<Vec<Action>> {
        let mut actions = self.actions_where("kind != 'idle'", ACTIONS)?;
        actions.extend(self.actions_where("kind = 'idle'", IDLE_ACTIONS)?);
        actions.sort_by_key(|it| std::cmp::Reverse(it.at));
        Ok(actions)
    }

    fn actions_where(&self, condition: &str, limit: usize) -> rusqlite::Result<Vec<Action>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT manager_id, at, kind, amount, price, reason, dry_run, txid, error FROM actions
             WHERE {condition} ORDER BY at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(Action {
                manager_id: row.get(0)?,
                at: row.get(1)?,
                kind: row.get(2)?,
                amount: optional_decimal(row, 3)?,
                price: optional_decimal(row, 4)?,
                reason: row.get(5)?,
                dry_run: row.get(6)?,
                txid: row.get(7)?,
                error: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    fn dry_run(&self) -> rusqlite::Result<Option<bool>> {
        use rusqlite::OptionalExtension;
        self.connection
            .query_row(
                "SELECT dry_run FROM actions WHERE kind IN ('buy', 'sell')
                 ORDER BY at DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
    }

    fn markets(&self, since: i64, bucket: i64) -> rusqlite::Result<Vec<Market>> {
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
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut prices = self.connection.prepare_cached(
            "SELECT MAX(at), CAST(last AS REAL) FROM tickers WHERE pair = ?1 AND at >= ?2
             GROUP BY at / ?3 ORDER BY 1",
        )?;
        for market in &mut markets {
            market.prices = prices
                .query_map(params![market.key, since, bucket], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
        }

        if self.tables.contains("indicators") {
            let mut statement = self.connection.prepare_cached(
                "SELECT atr_percent, ema_fast, ema_slow, slope_24h_percent, range_24h_percent,
                    change_24h_percent FROM indicators WHERE pair = ?1 ORDER BY at DESC LIMIT 1",
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
            }
        }
        Ok(markets)
    }
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
