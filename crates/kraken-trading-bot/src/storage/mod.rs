//! The SQLite database of the bot, to work with what it did:
//!
//! | Table | Content |
//! |-------|---------|
//! | `orders` | the orders of the managers, synchronized from Kraken |
//! | `managers` | the current state of each manager: configuration, balances, value |
//! | `manager_snapshots` | the state of each manager at each tick |
//! | `actions` | the decisions of the managers: orders placed or validated (dry run), idle reasons, errors |
//! | `tickers` | the prices of the pairs of the managers, at each tick |
//! | `sync_state` | where the synchronization of the orders is |
//!
//! The amounts are stored as text, to stay exact. The timestamps are UTC milliseconds, except the
//! times of the orders which are the seconds of Kraken.

mod journal;
mod managers;
mod orders;

pub use journal::{ActionRecord, TickerRecord};
pub use managers::ManagerState;

use rusqlite::{Connection, OptionalExtension, params};
use rust_decimal::Decimal;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid value in the database: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Migrations of the schema, applied in order: the version of the database is their count
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE orders (
    txid            TEXT PRIMARY KEY,
    manager_id      INTEGER NOT NULL,
    pair            TEXT NOT NULL,
    side            TEXT NOT NULL,
    status          TEXT NOT NULL,
    is_final        INTEGER NOT NULL,
    volume          TEXT NOT NULL,
    volume_executed TEXT NOT NULL,
    cost            TEXT NOT NULL,
    fee             TEXT NOT NULL,
    limit_price     TEXT,
    average_price   TEXT,
    open_time       REAL NOT NULL,
    close_time      REAL,
    expire_time     REAL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX orders_by_manager ON orders (manager_id, open_time);
CREATE INDEX orders_not_final ON orders (is_final) WHERE is_final = 0;

CREATE TABLE managers (
    id                 INTEGER PRIMARY KEY,
    pair               TEXT NOT NULL,
    budget             TEXT NOT NULL,
    buy_below          TEXT NOT NULL,
    sell_above         TEXT NOT NULL,
    mode               TEXT NOT NULL,
    quote_available    TEXT NOT NULL,
    quote_in_buy_order TEXT NOT NULL,
    base_available     TEXT NOT NULL,
    base_in_sell_order TEXT NOT NULL,
    position_cost      TEXT NOT NULL,
    entry_price        TEXT,
    last_price         TEXT NOT NULL,
    value              TEXT NOT NULL,
    first_seen         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE manager_snapshots (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    manager_id         INTEGER NOT NULL,
    at                 INTEGER NOT NULL,
    quote_available    TEXT NOT NULL,
    quote_in_buy_order TEXT NOT NULL,
    base_available     TEXT NOT NULL,
    base_in_sell_order TEXT NOT NULL,
    position_cost      TEXT NOT NULL,
    entry_price        TEXT,
    last_price         TEXT NOT NULL,
    value              TEXT NOT NULL
);
CREATE INDEX manager_snapshots_by_manager ON manager_snapshots (manager_id, at);

CREATE TABLE actions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    manager_id INTEGER NOT NULL,
    at         INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    amount     TEXT,
    price      TEXT,
    reason     TEXT,
    dry_run    INTEGER NOT NULL,
    txid       TEXT,
    error      TEXT
);
CREATE INDEX actions_by_manager ON actions (manager_id, at);

CREATE TABLE tickers (
    pair       TEXT NOT NULL,
    at         INTEGER NOT NULL,
    last       TEXT NOT NULL,
    bid        TEXT NOT NULL,
    ask        TEXT NOT NULL,
    volume_24h TEXT NOT NULL,
    vwap_24h   TEXT NOT NULL,
    PRIMARY KEY (pair, at)
);

CREATE TABLE sync_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#];

pub struct Storage {
    connection: Connection,
}

impl Storage {
    /// Opens the database, creating it and migrating its schema when needed
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|it| !it.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|error| StorageError::Invalid(format!("{}: {error}", parent.display())))?;
        }
        Self::with(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::with(Connection::open_in_memory()?)
    }

    fn with(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let storage = Self { connection };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        let version = self.version()?;
        if version > MIGRATIONS.len() {
            return Err(StorageError::Invalid(format!(
                "the database is at version {version}, newer than this bot ({})",
                MIGRATIONS.len()
            )));
        }
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute_batch(migration)?;
            transaction.pragma_update(None, "user_version", (index + 1) as i64)?;
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn version(&self) -> Result<usize> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(version as usize)
    }

    pub fn state(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

pub(crate) fn decimal(text: String) -> Result<Decimal> {
    Decimal::from_str(&text).map_err(|error| StorageError::Invalid(format!("{text}: {error}")))
}

pub(crate) fn optional_decimal(text: Option<String>) -> Result<Option<Decimal>> {
    text.map(decimal).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_schema_once() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.version().unwrap(), MIGRATIONS.len());
        // migrating again changes nothing
        storage.migrate().unwrap();
        assert_eq!(storage.version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn keeps_the_data_in_the_file() {
        let directory =
            std::env::temp_dir().join(format!("kraken-bot-test-{}", std::process::id()));
        let path = directory.join("nested/bot.sqlite");
        {
            let storage = Storage::open(&path).unwrap();
            storage.set_state("watermark", "42").unwrap();
        }
        let storage = Storage::open(&path).unwrap();
        assert_eq!(storage.state("watermark").unwrap().as_deref(), Some("42"));
        // closed before its files are removed: Windows refuses to remove an open file
        drop(storage);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refuses_a_database_of_a_newer_bot() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .connection
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(storage.migrate(), Err(StorageError::Invalid(_))));
    }

    #[test]
    fn updates_the_sync_state() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.state("key").unwrap(), None);
        storage.set_state("key", "1").unwrap();
        storage.set_state("key", "2").unwrap();
        assert_eq!(storage.state("key").unwrap().as_deref(), Some("2"));
    }
}
