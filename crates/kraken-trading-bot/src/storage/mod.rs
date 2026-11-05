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
//! | `virtual_managers` | the managers added by the bot, deleted ones included |
//! | `order_references` | the price of the market each buy was placed from |
//! | `indicators` | the trend and the volatility of the pairs over 24 hours, at each tick |
//! | `account_snapshots` | what the whole account is worth at each tick, per quote currency |
//! | `commands` | what is asked of the bot from outside: `forced_tick`, a tick now |
//! | `accounts` | the Kraken accounts of the bot, by name |
//!
//! A `Storage` works for one account: every table but `tickers` and `indicators`, the same market
//! for all of them, has an `account` column.
//!
//! The amounts are stored as text, to stay exact. The timestamps are UTC milliseconds, except the
//! times of the orders which are the seconds of Kraken.

mod commands;
mod journal;
mod managers;
mod orders;
mod virtuals;

pub use commands::{CommandState, FORCED_TICK};
pub use journal::{AccountRecord, ActionRecord, TickerRecord};
pub use managers::ManagerState;
pub use virtuals::VirtualManager;

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

/// The account of a bot with a single key: what the database held before there were several
pub const MAIN_ACCOUNT: &str = "main";

/// Migrations of the schema, applied in order: the version of the database is their count
const MIGRATIONS: &[&str] = &[
    r#"
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
"#,
    r#"
CREATE TABLE virtual_managers (
    id          INTEGER PRIMARY KEY,
    template_id INTEGER NOT NULL,
    pair        TEXT NOT NULL,
    budget      TEXT NOT NULL,
    buy_below   TEXT NOT NULL,
    sell_above  TEXT NOT NULL,
    mode        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    reason      TEXT NOT NULL,
    deleted_at  INTEGER,
    deleted_why TEXT
);
"#,
    // the indicators describe the market, they're not amounts: in REAL, as computed
    r#"
CREATE TABLE indicators (
    pair               TEXT NOT NULL,
    at                 INTEGER NOT NULL,
    close              REAL NOT NULL,
    atr_percent        REAL NOT NULL,
    ema_fast           REAL NOT NULL,
    ema_slow           REAL NOT NULL,
    slope_24h_percent  REAL NOT NULL,
    range_24h_percent  REAL NOT NULL,
    change_24h_percent REAL NOT NULL,
    candles            INTEGER NOT NULL,
    PRIMARY KEY (pair, at)
);
"#,
    r#"
CREATE TABLE order_references (
    txid      TEXT PRIMARY KEY,
    reference TEXT NOT NULL
);
"#,
    // the whole account, managers or not, per quote currency of the managers
    r#"
CREATE TABLE account_snapshots (
    at       INTEGER NOT NULL,
    quote    TEXT NOT NULL,
    cash     TEXT NOT NULL,
    holdings TEXT NOT NULL,
    PRIMARY KEY (at, quote)
);
"#,
    // what is asked of the bot from outside, the dashboard: one row per command
    r#"
CREATE TABLE commands (
    key        TEXT PRIMARY KEY,
    state      TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
INSERT INTO commands (key, state, updated_at) VALUES ('forced_tick', 'idle', 0);
"#,
    // several Kraken accounts in one database. What was there belongs to the first one, `main`.
    // The tickers and the indicators aren't tied to an account: the market is the same for all
    r#"
CREATE TABLE accounts (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT INTO accounts (id, name) VALUES (1, 'main');

CREATE TABLE orders_of_accounts (
    account         INTEGER NOT NULL,
    txid            TEXT NOT NULL,
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
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (account, txid)
);
INSERT INTO orders_of_accounts (account, txid, manager_id, pair, side, status, is_final, volume,
    volume_executed, cost, fee, limit_price, average_price, open_time, close_time, expire_time,
    updated_at)
SELECT 1, txid, manager_id, pair, side, status, is_final, volume, volume_executed, cost, fee,
    limit_price, average_price, open_time, close_time, expire_time, updated_at FROM orders;
DROP TABLE orders;
ALTER TABLE orders_of_accounts RENAME TO orders;
CREATE INDEX orders_by_manager ON orders (account, manager_id, open_time);
CREATE INDEX orders_not_final ON orders (account, is_final) WHERE is_final = 0;

CREATE TABLE order_references_of_accounts (
    account   INTEGER NOT NULL,
    txid      TEXT NOT NULL,
    reference TEXT NOT NULL,
    PRIMARY KEY (account, txid)
);
INSERT INTO order_references_of_accounts (account, txid, reference)
SELECT 1, txid, reference FROM order_references;
DROP TABLE order_references;
ALTER TABLE order_references_of_accounts RENAME TO order_references;

ALTER TABLE manager_snapshots ADD COLUMN account INTEGER NOT NULL DEFAULT 1;
DROP INDEX manager_snapshots_by_manager;
CREATE INDEX manager_snapshots_by_manager ON manager_snapshots (account, manager_id, at);

ALTER TABLE actions ADD COLUMN account INTEGER NOT NULL DEFAULT 1;
DROP INDEX actions_by_manager;
CREATE INDEX actions_by_manager ON actions (account, manager_id, at);

CREATE TABLE managers_of_accounts (
    account            INTEGER NOT NULL,
    id                 INTEGER NOT NULL,
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
    updated_at         INTEGER NOT NULL,
    PRIMARY KEY (account, id)
);
INSERT INTO managers_of_accounts (account, id, pair, budget, buy_below, sell_above, mode,
    quote_available, quote_in_buy_order, base_available, base_in_sell_order, position_cost,
    entry_price, last_price, value, first_seen, updated_at)
SELECT 1, id, pair, budget, buy_below, sell_above, mode, quote_available, quote_in_buy_order,
    base_available, base_in_sell_order, position_cost, entry_price, last_price, value, first_seen,
    updated_at FROM managers;
DROP TABLE managers;
ALTER TABLE managers_of_accounts RENAME TO managers;

CREATE TABLE sync_state_of_accounts (
    account INTEGER NOT NULL,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL,
    PRIMARY KEY (account, key)
);
INSERT INTO sync_state_of_accounts (account, key, value) SELECT 1, key, value FROM sync_state;
DROP TABLE sync_state;
ALTER TABLE sync_state_of_accounts RENAME TO sync_state;

CREATE TABLE virtual_managers_of_accounts (
    account     INTEGER NOT NULL,
    id          INTEGER NOT NULL,
    template_id INTEGER NOT NULL,
    pair        TEXT NOT NULL,
    budget      TEXT NOT NULL,
    buy_below   TEXT NOT NULL,
    sell_above  TEXT NOT NULL,
    mode        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    reason      TEXT NOT NULL,
    deleted_at  INTEGER,
    deleted_why TEXT,
    PRIMARY KEY (account, id)
);
INSERT INTO virtual_managers_of_accounts (account, id, template_id, pair, budget, buy_below,
    sell_above, mode, created_at, reason, deleted_at, deleted_why)
SELECT 1, id, template_id, pair, budget, buy_below, sell_above, mode, created_at, reason,
    deleted_at, deleted_why FROM virtual_managers;
DROP TABLE virtual_managers;
ALTER TABLE virtual_managers_of_accounts RENAME TO virtual_managers;

CREATE TABLE account_snapshots_of_accounts (
    account  INTEGER NOT NULL,
    at       INTEGER NOT NULL,
    quote    TEXT NOT NULL,
    cash     TEXT NOT NULL,
    holdings TEXT NOT NULL,
    PRIMARY KEY (account, at, quote)
);
INSERT INTO account_snapshots_of_accounts (account, at, quote, cash, holdings)
SELECT 1, at, quote, cash, holdings FROM account_snapshots;
DROP TABLE account_snapshots;
ALTER TABLE account_snapshots_of_accounts RENAME TO account_snapshots;

CREATE TABLE commands_of_accounts (
    account    INTEGER NOT NULL,
    key        TEXT NOT NULL,
    state      TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account, key)
);
INSERT INTO commands_of_accounts (account, key, state, updated_at)
SELECT 1, key, state, updated_at FROM commands;
DROP TABLE commands;
ALTER TABLE commands_of_accounts RENAME TO commands;
"#,
];

pub struct Storage {
    connection: Connection,
    /// The account everything read and written here belongs to
    account: i64,
}

impl Storage {
    /// Opens the database for the main account, creating it and migrating its schema when needed
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_account(path, MAIN_ACCOUNT)
    }

    /// Opens the database for this account, adding it when it's new
    pub fn open_account(path: &Path, account: &str) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|it| !it.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|error| StorageError::Invalid(format!("{}: {error}", parent.display())))?;
        }
        Self::with(Connection::open(path)?, account)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::with(Connection::open_in_memory()?, MAIN_ACCOUNT)
    }

    fn with(connection: Connection, account: &str) -> Result<Self> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let mut storage = Self {
            connection,
            account: 0,
        };
        storage.migrate()?;
        storage.account = storage.add_account(account)?;
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

    /// The id of the account, added with its commands when it's new
    fn add_account(&self, name: &str) -> Result<i64> {
        self.connection
            .execute("INSERT OR IGNORE INTO accounts (name) VALUES (?1)", [name])?;
        let id: i64 = self.connection.query_row(
            "SELECT id FROM accounts WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        self.connection.execute(
            "INSERT OR IGNORE INTO commands (account, key, state, updated_at)
             VALUES (?1, ?2, 'idle', 0)",
            params![id, FORCED_TICK],
        )?;
        Ok(id)
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
                "SELECT value FROM sync_state WHERE account = ?1 AND key = ?2",
                params![self.account, key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO sync_state (account, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (account, key) DO UPDATE SET value = excluded.value",
            params![self.account, key, value],
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
    fn gives_what_the_database_held_to_the_main_account() {
        let connection = Connection::open_in_memory().unwrap();
        // the last version with a single account, in use
        for migration in &MIGRATIONS[..6] {
            connection.execute_batch(migration).unwrap();
        }
        connection.pragma_update(None, "user_version", 6).unwrap();
        connection
            .execute_batch(
                "INSERT INTO managers (id, pair, budget, buy_below, sell_above, mode,
                    quote_available, quote_in_buy_order, base_available, base_in_sell_order,
                    position_cost, entry_price, last_price, value, first_seen, updated_at)
                 VALUES (7, 'XBTEUR', '30', '0.5', '2', 'active', '0.1', '0', '0', '0.0004', '29.9',
                    '71658.4', '71414.1', '29.9', 1, 2);
                 INSERT INTO manager_snapshots (manager_id, at, quote_available, quote_in_buy_order,
                    base_available, base_in_sell_order, position_cost, last_price, value)
                 VALUES (7, 2, '0.1', '0', '0', '0.0004', '29.9', '71414.1', '29.9');
                 INSERT INTO orders (txid, manager_id, pair, side, status, is_final, volume,
                    volume_executed, cost, fee, open_time, updated_at)
                 VALUES ('T', 7, 'XBTEUR', 'sell', 'open', 0, '0.0004', '0', '0', '0', 1.0, 2);
                 INSERT INTO actions (manager_id, at, kind, dry_run) VALUES (7, 2, 'sell', 0);
                 INSERT INTO sync_state (key, value) VALUES ('watermark', '42');
                 INSERT INTO virtual_managers (id, template_id, pair, budget, buy_below, sell_above,
                    mode, created_at, reason)
                 VALUES (1001001, 1, 'XBTEUR', '30', '1', '2', 'active', 1, 'why');
                 INSERT INTO account_snapshots (at, quote, cash, holdings)
                 VALUES (2, 'ZEUR', '11', '30');
                 UPDATE commands SET state = 'refresh';",
            )
            .unwrap();

        let storage = Storage::with(connection, MAIN_ACCOUNT).unwrap();
        assert_eq!(storage.version().unwrap(), MIGRATIONS.len());
        assert!(storage.manager_balances(7).unwrap().is_some());
        assert_eq!(storage.snapshot_count(7).unwrap(), 1);
        assert_eq!(storage.open_orders().unwrap().len(), 1);
        assert_eq!(storage.actions_of(7, 10).unwrap().len(), 1);
        assert_eq!(storage.state("watermark").unwrap().as_deref(), Some("42"));
        assert_eq!(storage.virtual_ids().unwrap(), [1001001]);
        assert_eq!(storage.last_accounts().unwrap().len(), 1);
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Refresh)
        );
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
