//! What the accounts are worth: each one alone, and added up per quote

use super::{Account, AccountLine, AccountPoint, Database, account_of, decimal, to_f64};
use rusqlite::params;
use std::collections::HashMap;

impl Database {
    pub(super) fn account_lines(
        &self,
        since: i64,
        bucket: i64,
    ) -> rusqlite::Result<Vec<AccountLine>> {
        let mut names = self.names()?;
        // the databases older than the accounts have a single, implicit one
        if names.is_empty() {
            names.push((1, "main".to_string()));
        }
        let managers = self.managers(None)?;
        let snapshots = self.snapshots(None, since, bucket)?;
        names
            .into_iter()
            .map(|(id, name)| {
                Ok(AccountLine {
                    name,
                    managers: managers
                        .iter()
                        .filter(|it| account_of(it.id) == id)
                        .map(|it| (it.id, it.pair.clone(), to_f64(it.budget)))
                        .collect(),
                    snapshots: snapshots
                        .iter()
                        .filter(|it| account_of(it.manager_id) == id)
                        .copied()
                        .collect(),
                    series: self.account_series(Some(id), since, bucket)?,
                })
            })
            .collect()
    }

    pub(super) fn names(&self) -> rusqlite::Result<Vec<(i64, String)>> {
        if !self.has_accounts() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached("SELECT id, name FROM accounts ORDER BY id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// The last value of each account, added up per quote
    pub(super) fn accounts(&self, account: Option<i64>) -> rusqlite::Result<Vec<Account>> {
        if !self.tables.contains("account_snapshots") {
            return Ok(Vec::new());
        }
        let same_account = if self.has_accounts() {
            "WHERE other.account = it.account"
        } else {
            ""
        };
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT quote, at, cash, holdings FROM account_snapshots it
             WHERE {} AND at = (SELECT MAX(at) FROM account_snapshots other {same_account})
             ORDER BY quote",
            self.only(account)
        ))?;
        let rows = statement.query_map([], |row| {
            Ok(Account {
                quote: row.get(0)?,
                at: row.get(1)?,
                cash: decimal(row, 2)?,
                holdings: decimal(row, 3)?,
            })
        })?;
        let mut total: Vec<Account> = Vec::new();
        for row in rows {
            let row = row?;
            match total.last_mut().filter(|it| it.quote == row.quote) {
                Some(it) => {
                    it.at = it.at.max(row.at);
                    it.cash += row.cash;
                    it.holdings += row.holdings;
                }
                None => total.push(row),
            }
        }
        Ok(total)
    }

    pub(super) fn account_series(
        &self,
        account: Option<i64>,
        since: i64,
        bucket: i64,
    ) -> rusqlite::Result<Vec<AccountPoint>> {
        if !self.tables.contains("account_snapshots") {
            return Ok(Vec::new());
        }
        // the bare columns of SQLite: the amounts come from the row of MAX(at)
        let id = if self.has_accounts() { "account" } else { "1" };
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {id}, quote, MAX(at), CAST(cash AS REAL) + CAST(holdings AS REAL),
                CAST(cash AS REAL)
             FROM account_snapshots WHERE {} AND at >= ?1 GROUP BY {id}, quote, at / ?2 ORDER BY 3",
            self.only(account)
        ))?;
        let rows = statement.query_map(params![since, bucket], |row| {
            Ok(AccountRow {
                account: row.get(0)?,
                quote: row.get(1)?,
                at: row.get(2)?,
                value: row.get(3)?,
                cash: row.get(4)?,
            })
        })?;
        Ok(added_up(
            &rows.collect::<rusqlite::Result<Vec<_>>>()?,
            bucket,
        ))
    }
}

/// A point of the value of an account, before it's added to the others
#[derive(Debug, Clone, PartialEq)]
struct AccountRow {
    account: i64,
    quote: String,
    at: i64,
    value: f64,
    cash: f64,
}

/// What the accounts are worth together per bucket and quote, oldest first: the accounts don't tick
/// at the same millisecond, so each counts for its last value known. The rows are sorted by time
fn added_up(rows: &[AccountRow], bucket: i64) -> Vec<AccountPoint> {
    let mut last: HashMap<(i64, &str), (f64, f64)> = HashMap::new();
    let mut points: Vec<AccountPoint> = Vec::new();
    let mut indexes: HashMap<(&str, i64), usize> = HashMap::new();
    for row in rows {
        last.insert((row.account, &row.quote), (row.value, row.cash));
        let (value, cash) = last
            .iter()
            .filter(|((_, quote), _)| *quote == row.quote)
            .fold((0.0, 0.0), |(value, cash), (_, it)| {
                (value + it.0, cash + it.1)
            });
        let index = *indexes
            .entry((&row.quote, row.at / bucket))
            .or_insert_with(|| {
                points.push(AccountPoint {
                    quote: row.quote.clone(),
                    at: row.at,
                    value,
                    cash,
                });
                points.len() - 1
            });
        points[index].at = row.at;
        points[index].value = value;
        points[index].cash = cash;
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(account: i64, at: i64, value: f64) -> AccountRow {
        AccountRow {
            account,
            quote: "ZEUR".into(),
            at,
            value,
            cash: 0.0,
        }
    }

    #[test]
    fn adds_up_the_accounts_though_they_tick_apart() {
        let points = added_up(
            &[
                row(1, 1_001, 100.0),
                row(2, 1_002, 50.0),
                row(1, 2_001, 110.0),
                row(2, 3_002, 40.0),
            ],
            1_000,
        );
        let values: Vec<(i64, f64)> = points.iter().map(|it| (it.at, it.value)).collect();
        // the first bucket with both, the second with the last value of the second account
        assert_eq!(values, [(1_002, 150.0), (2_001, 160.0), (3_002, 150.0)]);
    }
}
