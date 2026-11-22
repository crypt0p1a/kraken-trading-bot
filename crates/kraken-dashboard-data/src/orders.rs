//! The orders of the managers, and their actions: the tape

use super::{Action, Database, Order, Side, decimal, optional_decimal};
use rusqlite::OptionalExtension;

/// The last actions read, for the tape: the idle ones apart, the bot writes one per manager and tick
const ACTIONS: usize = 300;
const IDLE_ACTIONS: usize = 100;

impl Database {
    pub(super) fn orders(&self, account: Option<i64>) -> rusqlite::Result<Vec<Order>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT txid, {}, pair, side, status, is_final, volume, volume_executed, cost,
                fee, limit_price, average_price, open_time, close_time
             FROM orders WHERE {} ORDER BY open_time, txid",
            self.key(),
            self.only(account)
        ))?;
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

    pub(super) fn actions(&self, account: Option<i64>) -> rusqlite::Result<Vec<Action>> {
        let only = self.only(account);
        let mut actions = self.actions_where(&format!("{only} AND kind != 'idle'"), ACTIONS)?;
        actions.extend(self.actions_where(&format!("{only} AND kind = 'idle'"), IDLE_ACTIONS)?);
        actions.sort_by_key(|it| std::cmp::Reverse(it.at));
        Ok(actions)
    }

    fn actions_where(&self, condition: &str, limit: usize) -> rusqlite::Result<Vec<Action>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {}, at, kind, amount, price, reason, dry_run, txid, error FROM actions
             WHERE {condition} ORDER BY at DESC, id DESC LIMIT ?1",
            self.key(),
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

    pub(super) fn dry_run(&self, account: Option<i64>) -> rusqlite::Result<Option<bool>> {
        self.connection
            .query_row(
                &format!(
                    "SELECT dry_run FROM actions WHERE {} AND kind IN ('buy', 'sell')
                     ORDER BY at DESC, id DESC LIMIT 1",
                    self.only(account)
                ),
                [],
                |row| row.get(0),
            )
            .optional()
    }
}
