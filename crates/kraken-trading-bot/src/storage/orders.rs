use super::{Result, Storage, StorageError, decimal, optional_decimal};
use crate::clock::now_millis;
use crate::order::BotOrder;
use kraken_spot_api::trading::{OrderSide, OrderStatus};
use rusqlite::{Row, params};

fn side_name(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

fn status_name(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::Pending => "pending",
        OrderStatus::Open => "open",
        OrderStatus::Closed => "closed",
        OrderStatus::Canceled => "canceled",
        OrderStatus::Expired => "expired",
        OrderStatus::Unknown => "unknown",
    }
}

fn parse_side(text: &str) -> Result<OrderSide> {
    match text {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        other => Err(StorageError::Invalid(format!("side {other}"))),
    }
}

fn parse_status(text: &str) -> OrderStatus {
    match text {
        "pending" => OrderStatus::Pending,
        "open" => OrderStatus::Open,
        "closed" => OrderStatus::Closed,
        "canceled" => OrderStatus::Canceled,
        "expired" => OrderStatus::Expired,
        _ => OrderStatus::Unknown,
    }
}

const COLUMNS: &str = "txid, manager_id, pair, side, status, volume, volume_executed, cost, fee, \
                       limit_price, average_price, open_time, close_time, expire_time";

fn read(row: &Row) -> rusqlite::Result<[Option<String>; 14]> {
    Ok([
        row.get(0)?,
        row.get::<_, i64>(1).map(|it| Some(it.to_string()))?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get::<_, f64>(11).map(|it| Some(it.to_string()))?,
        row.get::<_, Option<f64>>(12)?.map(|it| it.to_string()),
        row.get::<_, Option<f64>>(13)?.map(|it| it.to_string()),
    ])
}

fn to_order(values: [Option<String>; 14]) -> Result<BotOrder> {
    let [
        txid,
        manager,
        pair,
        side,
        status,
        volume,
        executed,
        cost,
        fee,
        limit,
        average,
        open,
        close,
        expire,
    ] = values;
    let required =
        |value: Option<String>| value.ok_or_else(|| StorageError::Invalid("missing value".into()));
    let float = |value: Option<String>| value.and_then(|it| it.parse::<f64>().ok());

    Ok(BotOrder {
        txid: required(txid)?,
        manager_id: required(manager)?
            .parse()
            .map_err(|_| StorageError::Invalid("manager".into()))?,
        pair: required(pair)?,
        side: parse_side(&required(side)?)?,
        status: parse_status(&required(status)?),
        volume: decimal(required(volume)?)?,
        volume_executed: decimal(required(executed)?)?,
        cost: decimal(required(cost)?)?,
        fee: decimal(required(fee)?)?,
        limit_price: optional_decimal(limit)?,
        average_price: optional_decimal(average)?,
        open_time: float(open).unwrap_or_default(),
        close_time: float(close),
        expire_time: float(expire),
    })
}

impl Storage {
    /// Inserts the orders, or updates them: the last state known wins
    pub fn upsert_orders(&self, orders: &[BotOrder]) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut statement = transaction.prepare_cached(&format!(
                "INSERT INTO orders ({COLUMNS}, is_final, updated_at, account)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT (account, txid) DO UPDATE SET
                    status = excluded.status, is_final = excluded.is_final,
                    volume = excluded.volume, volume_executed = excluded.volume_executed,
                    cost = excluded.cost, fee = excluded.fee, limit_price = excluded.limit_price,
                    average_price = excluded.average_price, close_time = excluded.close_time,
                    open_time = excluded.open_time, expire_time = excluded.expire_time,
                    updated_at = excluded.updated_at"
            ))?;
            let now = now_millis();
            for order in orders {
                statement.execute(params![
                    order.txid,
                    order.manager_id,
                    order.pair,
                    side_name(order.side),
                    status_name(order.status),
                    order.volume.to_string(),
                    order.volume_executed.to_string(),
                    order.cost.to_string(),
                    order.fee.to_string(),
                    order.limit_price.map(|it| it.to_string()),
                    order.average_price.map(|it| it.to_string()),
                    order.open_time,
                    order.close_time,
                    order.expire_time,
                    order.is_final(),
                    now,
                    self.account,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn orders_where(
        &self,
        condition: &str,
        parameters: impl rusqlite::Params,
    ) -> Result<Vec<BotOrder>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM orders WHERE account = {} AND {condition}
             ORDER BY open_time, txid",
            self.account
        ))?;
        let rows = statement.query_map(parameters, read)?;
        rows.map(|row| to_order(row?)).collect()
    }

    /// Every order of a manager, oldest first
    pub fn orders_of(&self, manager_id: i32) -> Result<Vec<BotOrder>> {
        self.orders_where("manager_id = ?1", [manager_id])
    }

    /// The orders which may still change
    pub fn open_orders(&self) -> Result<Vec<BotOrder>> {
        self.orders_where("is_final = 0", [])
    }

    pub fn order(&self, txid: &str) -> Result<Option<BotOrder>> {
        Ok(self.orders_where("txid = ?1", [txid])?.into_iter().next())
    }

    /// The price of the market the order was placed from: Kraken doesn't keep it
    pub fn set_order_reference(&self, txid: &str, reference: rust_decimal::Decimal) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO order_references (account, txid, reference)
             VALUES (?1, ?2, ?3)",
            params![self.account, txid, reference.to_string()],
        )?;
        Ok(())
    }

    /// The price of the market the order was placed from, None for an order placed before it was
    /// recorded, or by another database
    pub fn order_reference(&self, txid: &str) -> Result<Option<rust_decimal::Decimal>> {
        use rusqlite::OptionalExtension;
        self.connection
            .query_row(
                "SELECT reference FROM order_references WHERE account = ?2 AND txid = ?1",
                params![txid, self.account],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(decimal)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::tests::{executed, order};
    use crate::planner::tests::d;

    #[test]
    fn keeps_every_field_of_the_orders() {
        let storage = Storage::open_in_memory().unwrap();
        let original = BotOrder {
            close_time: Some(1761900060.5),
            expire_time: Some(1761943200.0),
            average_price: Some(d("49999.95")),
            ..executed(
                order(
                    1761900000.25,
                    OrderSide::Buy,
                    OrderStatus::Closed,
                    "0.00100000",
                    "50000.0",
                ),
                "0.00058823",
                "29.41150",
                "0.07647",
            )
        };
        storage
            .upsert_orders(std::slice::from_ref(&original))
            .unwrap();

        let read = storage.order(&original.txid).unwrap().unwrap();
        assert_eq!(read, original);
        // the decimals are kept exactly, trailing zeros included
        assert_eq!(read.cost.to_string(), "29.41150");
    }

    #[test]
    fn updates_the_orders_and_lists_the_open_ones() {
        let storage = Storage::open_in_memory().unwrap();
        let open = order(1.0, OrderSide::Buy, OrderStatus::Open, "0.001", "50000");
        let other_manager = BotOrder {
            txid: "O9".into(),
            manager_id: 2,
            ..order(2.0, OrderSide::Sell, OrderStatus::Open, "1", "1")
        };
        storage
            .upsert_orders(&[open.clone(), other_manager])
            .unwrap();
        assert_eq!(storage.open_orders().unwrap().len(), 2);

        let filled = executed(
            BotOrder {
                status: OrderStatus::Closed,
                ..open.clone()
            },
            "0.001",
            "50",
            "0.1",
        );
        storage
            .upsert_orders(std::slice::from_ref(&filled))
            .unwrap();

        assert_eq!(storage.order(&open.txid).unwrap(), Some(filled));
        assert_eq!(storage.open_orders().unwrap().len(), 1);
        assert_eq!(storage.orders_of(1).unwrap().len(), 1);
        assert_eq!(storage.orders_of(2).unwrap().len(), 1);
        assert!(storage.orders_of(3).unwrap().is_empty());
    }

    #[test]
    fn lists_the_orders_of_a_manager_oldest_first() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .upsert_orders(&[
                order(3.0, OrderSide::Sell, OrderStatus::Open, "1", "1"),
                order(1.0, OrderSide::Buy, OrderStatus::Closed, "1", "1"),
                order(2.0, OrderSide::Buy, OrderStatus::Expired, "1", "1"),
            ])
            .unwrap();
        let times: Vec<f64> = storage
            .orders_of(1)
            .unwrap()
            .iter()
            .map(|it| it.open_time)
            .collect();
        assert_eq!(times, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn keeps_the_reference_of_an_order() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(storage.order_reference("OAAAAA").unwrap(), None);
        storage
            .set_order_reference("OAAAAA", crate::planner::tests::d("50000.1"))
            .unwrap();
        assert_eq!(
            storage.order_reference("OAAAAA").unwrap(),
            Some(crate::planner::tests::d("50000.1"))
        );
    }
}
