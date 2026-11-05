use super::virtuals::mode_name;
use super::{Result, Storage, decimal, optional_decimal};
use crate::config::ManagerConfig;
use crate::ledger::Balances;
use rusqlite::params;
use rust_decimal::Decimal;

/// What a manager has at a time, with the price it was valued at
#[derive(Debug, Clone, PartialEq)]
pub struct ManagerState {
    pub config: ManagerConfig,
    pub balances: Balances,
    pub last_price: Decimal,
    /// UTC milliseconds
    pub at: i64,
}

impl ManagerState {
    pub fn value(&self) -> Decimal {
        self.balances.value(self.last_price)
    }
}

impl Storage {
    /// Saves the current state of the manager, and adds it to its history
    pub fn save_manager(&self, state: &ManagerState) -> Result<()> {
        let ManagerState {
            config,
            balances,
            last_price,
            at,
        } = state;
        let value = state.value();
        let transaction = self.connection.unchecked_transaction()?;

        transaction.execute(
            "INSERT INTO managers (account, id, pair, budget, buy_below, sell_above, mode, quote_available,
                quote_in_buy_order, base_available, base_in_sell_order, position_cost, entry_price,
                last_price, value, first_seen, updated_at)
             VALUES (?16, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)
             ON CONFLICT (account, id) DO UPDATE SET
                pair = excluded.pair, budget = excluded.budget, buy_below = excluded.buy_below,
                sell_above = excluded.sell_above, mode = excluded.mode,
                quote_available = excluded.quote_available,
                quote_in_buy_order = excluded.quote_in_buy_order,
                base_available = excluded.base_available,
                base_in_sell_order = excluded.base_in_sell_order,
                position_cost = excluded.position_cost, entry_price = excluded.entry_price,
                last_price = excluded.last_price, value = excluded.value,
                updated_at = excluded.updated_at",
            params![
                config.id,
                config.pair,
                config.budget.to_string(),
                config.buy_below.to_string(),
                config.sell_above.to_string(),
                mode_name(config.mode),
                balances.quote_available.to_string(),
                balances.quote_in_buy_order.to_string(),
                balances.base_available.to_string(),
                balances.base_in_sell_order.to_string(),
                balances.position_cost.to_string(),
                balances.entry_price.map(|it| it.to_string()),
                last_price.to_string(),
                value.to_string(),
                at,
                self.account,
            ],
        )?;

        transaction.execute(
            "INSERT INTO manager_snapshots (account, manager_id, at, quote_available, quote_in_buy_order,
                base_available, base_in_sell_order, position_cost, entry_price, last_price, value)
             VALUES (?11, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                config.id,
                at,
                balances.quote_available.to_string(),
                balances.quote_in_buy_order.to_string(),
                balances.base_available.to_string(),
                balances.base_in_sell_order.to_string(),
                balances.position_cost.to_string(),
                balances.entry_price.map(|it| it.to_string()),
                last_price.to_string(),
                value.to_string(),
                self.account,
            ],
        )?;

        transaction.commit()?;
        Ok(())
    }

    /// The balances of the manager when it was last saved, with the price and the time
    pub fn manager_balances(&self, id: i32) -> Result<Option<(Balances, Decimal, i64)>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT quote_available, quote_in_buy_order, base_available, base_in_sell_order,
                position_cost, entry_price, last_price, updated_at FROM managers
             WHERE account = ?1 AND id = ?2",
        )?;
        let mut rows = statement.query(params![self.account, id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let balances = Balances {
            quote_available: decimal(row.get(0)?)?,
            quote_in_buy_order: decimal(row.get(1)?)?,
            base_available: decimal(row.get(2)?)?,
            base_in_sell_order: decimal(row.get(3)?)?,
            position_cost: decimal(row.get(4)?)?,
            entry_price: optional_decimal(row.get(5)?)?,
        };
        Ok(Some((balances, decimal(row.get(6)?)?, row.get(7)?)))
    }

    /// How many states of the manager are in its history
    pub fn snapshot_count(&self, id: i32) -> Result<usize> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM manager_snapshots WHERE account = ?1 AND manager_id = ?2",
            params![self.account, id],
            |row| row.get::<_, i64>(0).map(|it| it as usize),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ManagerMode;
    use crate::planner::tests::d;

    fn state(at: i64, quote: &str) -> ManagerState {
        ManagerState {
            config: ManagerConfig {
                id: 4,
                pair: "XBTUSD".into(),
                budget: d("30"),
                buy_below: d("1"),
                sell_above: d("1.5"),
                mode: ManagerMode::Active,
            },
            balances: Balances {
                quote_available: d(quote),
                quote_in_buy_order: d("0"),
                base_available: d("0.0002"),
                base_in_sell_order: d("0"),
                position_cost: d("10.02"),
                entry_price: Some(d("50100")),
            },
            last_price: d("51000"),
            at,
        }
    }

    #[test]
    fn saves_the_state_and_its_history() {
        let storage = Storage::open_in_memory().unwrap();
        storage.save_manager(&state(1, "20")).unwrap();
        storage.save_manager(&state(2, "19.5")).unwrap();

        let (balances, price, at) = storage.manager_balances(4).unwrap().unwrap();
        assert_eq!(balances.quote_available, d("19.5"));
        assert_eq!(balances.entry_price, Some(d("50100")));
        assert_eq!(price, d("51000"));
        assert_eq!(at, 2);
        assert_eq!(storage.snapshot_count(4).unwrap(), 2);
        assert_eq!(storage.manager_balances(5).unwrap(), None);
    }

    #[test]
    fn values_the_manager_at_the_last_price() {
        // 20 USD + 0.0002 BTC at 51000
        assert_eq!(state(1, "20").value(), d("30.2"));
    }
}
