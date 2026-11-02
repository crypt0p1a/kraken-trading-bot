use super::{Result, Storage, StorageError, decimal};
use crate::config::{ManagerConfig, ManagerMode};
use rusqlite::{OptionalExtension, params};

/// A manager added by the bot, copy of a manager of the YAML file
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualManager {
    pub config: ManagerConfig,
    /// Id of the manager of the YAML file it copies
    pub template_id: i32,
    /// UTC milliseconds
    pub created_at: i64,
    /// Why it was added
    pub reason: String,
}

pub(crate) fn mode_name(mode: ManagerMode) -> &'static str {
    match mode {
        ManagerMode::Active => "active",
        ManagerMode::Closing => "closing",
    }
}

fn mode(name: &str) -> Result<ManagerMode> {
    match name {
        "active" => Ok(ManagerMode::Active),
        "closing" => Ok(ManagerMode::Closing),
        other => Err(StorageError::Invalid(format!("unknown mode {other}"))),
    }
}

impl Storage {
    pub fn add_virtual_manager(&self, manager: &VirtualManager) -> Result<()> {
        let config = &manager.config;
        self.connection.execute(
            "INSERT INTO virtual_managers (id, template_id, pair, budget, buy_below, sell_above,
                mode, created_at, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                config.id,
                manager.template_id,
                config.pair,
                config.budget.to_string(),
                config.buy_below.to_string(),
                config.sell_above.to_string(),
                mode_name(config.mode),
                manager.created_at,
                manager.reason,
            ],
        )?;
        Ok(())
    }

    /// The virtual managers not deleted, by id
    pub fn virtual_managers(&self) -> Result<Vec<VirtualManager>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, template_id, pair, budget, buy_below, sell_above, mode, created_at, reason
             FROM virtual_managers WHERE deleted_at IS NULL ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        let mut managers = Vec::new();
        for row in rows {
            let (
                id,
                template_id,
                pair,
                budget,
                buy_below,
                sell_above,
                mode_text,
                created_at,
                reason,
            ) = row?;
            managers.push(VirtualManager {
                config: ManagerConfig {
                    id,
                    pair,
                    budget: decimal(budget)?,
                    buy_below: decimal(buy_below)?,
                    sell_above: decimal(sell_above)?,
                    mode: mode(&mode_text)?,
                },
                template_id,
                created_at,
                reason,
            });
        }
        Ok(managers)
    }

    /// Deletes the virtual manager: it's kept in the table, its id is never reused, and its
    /// current state is removed from `managers` (its history stays)
    pub fn delete_virtual_manager(&self, id: i32, at: i64, why: &str) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE virtual_managers SET deleted_at = ?2, deleted_why = ?3
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, at, why],
        )?;
        transaction.execute("DELETE FROM managers WHERE id = ?1", [id])?;
        transaction.commit()?;
        Ok(())
    }

    /// The ids of every virtual manager, deleted ones included
    pub fn virtual_ids(&self) -> Result<Vec<i32>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT id FROM virtual_managers ORDER BY id")?;
        let ids = statement.query_map([], |row| row.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// The highest id of the virtual managers, deleted ones included: the ids are never reused
    pub fn last_virtual_id(&self) -> Result<Option<i32>> {
        Ok(self
            .connection
            .query_row("SELECT MAX(id) FROM virtual_managers", [], |row| row.get(0))
            .optional()?
            .flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::tests::d;

    fn manager(id: i32, mode: ManagerMode) -> VirtualManager {
        VirtualManager {
            config: ManagerConfig {
                id,
                pair: "XBTEUR".into(),
                budget: d("20.5"),
                buy_below: d("0.5"),
                sell_above: d("0.8"),
                mode,
            },
            template_id: 2,
            created_at: 42,
            reason: "margin 22 ZEUR".into(),
        }
    }

    #[test]
    fn saves_the_virtual_managers() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.last_virtual_id().unwrap(), None);
        assert!(storage.virtual_managers().unwrap().is_empty());

        storage
            .add_virtual_manager(&manager(1_001_002, ManagerMode::Closing))
            .unwrap();
        storage
            .add_virtual_manager(&manager(1_000_002, ManagerMode::Active))
            .unwrap();

        assert_eq!(
            storage.virtual_managers().unwrap(),
            vec![
                manager(1_000_002, ManagerMode::Active),
                manager(1_001_002, ManagerMode::Closing)
            ]
        );
        assert_eq!(storage.last_virtual_id().unwrap(), Some(1_001_002));

        storage
            .delete_virtual_manager(1_001_002, 50, "no money")
            .unwrap();
        assert_eq!(
            storage.virtual_managers().unwrap(),
            vec![manager(1_000_002, ManagerMode::Active)]
        );
        // deleted, the id stays taken
        assert_eq!(storage.last_virtual_id().unwrap(), Some(1_001_002));
        assert_eq!(storage.virtual_ids().unwrap(), vec![1_000_002, 1_001_002]);
        // an id is added once
        assert!(
            storage
                .add_virtual_manager(&manager(1_000_002, ManagerMode::Active))
                .is_err()
        );
    }
}
