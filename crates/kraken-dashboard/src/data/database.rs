//! The database of the bot opened read only, and the only write of the dashboard: a tick asked for

use super::{Data, POINTS};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::HashSet;
use std::path::Path;

/// The tables without which there's nothing to show
const REQUIRED: [&str; 5] = [
    "orders",
    "managers",
    "manager_snapshots",
    "actions",
    "tickers",
];

pub struct Database {
    pub(super) connection: Connection,
    path: std::path::PathBuf,
    pub(super) tables: HashSet<String>,
    /// The virtual managers have the crypto they started with: the databases of the bots since
    /// sell_free_base
    pub(super) holdings: bool,
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
        Self::with(connection, path)
    }

    pub(super) fn with(connection: Connection, path: &Path) -> Result<Self> {
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
        let holdings = tables.contains("virtual_managers")
            && connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM pragma_table_info('virtual_managers')
                        WHERE name = 'holding')",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            tables,
            holdings,
            version: version as usize,
        })
    }

    /// Changes each time another connection, the bot, commits
    pub fn data_version(&self) -> Result<i64> {
        self.connection
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(sql)
    }

    /// The accounts are in the database, the older ones have a single, implicit one
    pub(super) fn has_accounts(&self) -> bool {
        self.tables.contains("accounts")
    }

    /// The condition of the rows of the account, every row without one
    pub(super) fn only(&self, account: Option<i64>) -> String {
        self.only_in("account", account)
    }

    pub(super) fn only_in(&self, column: &str, account: Option<i64>) -> String {
        match account {
            Some(id) if self.has_accounts() => format!("{column} = {id}"),
            _ => "1 = 1".into(),
        }
    }

    /// The key of a manager from the column of its account and of its id: see [`ManagerKey`]
    pub(super) fn key_of(&self, account: &str, id: &str) -> String {
        if self.has_accounts() {
            format!("(({account} - 1) << 32) | {id}")
        } else {
            id.into()
        }
    }

    pub(super) fn key(&self) -> String {
        self.key_of("account", "manager_id")
    }

    /// Reads everything of the account, or of all of them added up when None, the series from
    /// `since` (UTC milliseconds, 0 for all) to `now`
    pub fn load(&self, since: i64, now: i64, account: Option<i64>) -> Result<Data> {
        // a single read transaction: a tick of the bot is seen whole, or not at all
        let transaction = self.connection.unchecked_transaction().map_err(sql)?;

        let start = if since > 0 {
            since
        } else {
            transaction
                .query_row(
                    &format!(
                        "SELECT MIN(at) FROM manager_snapshots WHERE {}",
                        self.only(account)
                    ),
                    [],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(sql)?
                .unwrap_or(now)
        };
        let bucket = ((now - start) / POINTS).max(1);

        let data = Data {
            managers: self.managers(account).map_err(sql)?,
            orders: self.orders(account).map_err(sql)?,
            snapshots: self.snapshots(account, start, bucket).map_err(sql)?,
            actions: self.actions(account).map_err(sql)?,
            dry_run: self.dry_run(account).map_err(sql)?,
            forced_tick: self.forced_tick(account).map_err(sql)?,
            markets: self.markets(start, bucket).map_err(sql)?,
            accounts: self.accounts(account).map_err(sql)?,
            account_series: self.account_series(account, start, bucket).map_err(sql)?,
            account_lines: self.account_lines(start, bucket).map_err(sql)?,
            names: self.names().map_err(sql)?,
            version: self.version,
            since: start,
        };
        transaction.finish().map_err(sql)?;
        Ok(data)
    }

    /// The state of the tick: of several accounts, the one furthest along
    fn forced_tick(&self, account: Option<i64>) -> rusqlite::Result<Option<String>> {
        if !self.tables.contains("commands") {
            return Ok(None);
        }
        self.connection
            .query_row(
                &format!(
                    "SELECT state FROM commands WHERE key = 'forced_tick' AND {}
                     ORDER BY CASE state WHEN 'pending' THEN 0 WHEN 'refresh' THEN 1 ELSE 2 END
                     LIMIT 1",
                    self.only(account)
                ),
                [],
                |row| row.get(0),
            )
            .optional()
    }

    /// Asks the bot for a tick of the account now, of all of them when None, rather than at the end
    /// of its interval: the only write of the dashboard, through a connection of its own. Nothing
    /// changes while a tick is under way
    pub fn request_tick(&self, at: i64, account: Option<i64>) -> Result<()> {
        if !self.tables.contains("commands") {
            return Err("this bot can't be asked for a tick: update it".into());
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql)?;
        connection
            .execute(
                &format!(
                    "UPDATE commands SET state = 'refresh', updated_at = ?1
                     WHERE key = 'forced_tick' AND state <> 'pending' AND {}",
                    self.only(account)
                ),
                [at],
            )
            .map_err(sql)?;
        Ok(())
    }
}
