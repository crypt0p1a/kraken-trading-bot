//! The last state of the managers, and their history

use super::{Database, Manager, Snapshot, decimal, optional_decimal};
use rusqlite::params;

impl Database {
    pub(super) fn managers(&self, account: Option<i64>) -> rusqlite::Result<Vec<Manager>> {
        let virtuals = self.tables.contains("virtual_managers");
        let query = format!(
            "SELECT {}, m.pair, m.budget, m.buy_below, m.sell_above, m.mode, m.quote_available,
                m.quote_in_buy_order, m.base_available, m.base_in_sell_order, m.position_cost,
                m.entry_price, m.last_price, m.value, m.first_seen, m.updated_at, {}
             FROM managers m {} WHERE {} ORDER BY 1",
            self.key_of("m.account", "m.id"),
            match (virtuals, self.holdings) {
                (true, true) => "v.template_id, v.deleted_at, v.holding",
                (true, false) => "v.template_id, v.deleted_at, NULL",
                _ => "NULL, NULL, NULL",
            },
            if virtuals {
                if self.has_accounts() {
                    "LEFT JOIN virtual_managers v ON v.id = m.id AND v.account = m.account"
                } else {
                    "LEFT JOIN virtual_managers v ON v.id = m.id"
                }
            } else {
                ""
            },
            self.only_in("m.account", account),
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
                holding: optional_decimal(row, 18)?,
            })
        })?;
        rows.collect()
    }

    pub(super) fn snapshots(
        &self,
        account: Option<i64>,
        since: i64,
        bucket: i64,
    ) -> rusqlite::Result<Vec<Snapshot>> {
        // the bare column of SQLite: `value` comes from the row of MAX(at)
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {}, MAX(at), CAST(value AS REAL),
                (CAST(base_available AS REAL) + CAST(base_in_sell_order AS REAL))
                    * CAST(last_price AS REAL)
             FROM manager_snapshots
             WHERE {} AND at >= ?1 GROUP BY 1, at / ?2 ORDER BY 2",
            self.key(),
            self.only(account)
        ))?;
        let rows = statement.query_map(params![since, bucket], |row| {
            Ok(Snapshot {
                manager_id: row.get(0)?,
                at: row.get(1)?,
                value: row.get(2)?,
                held: row.get(3)?,
            })
        })?;
        rows.collect()
    }
}
