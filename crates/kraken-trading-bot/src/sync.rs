//! Synchronizes the orders of the managers, from Kraken to the database. The orders are found by
//! their userref, which is the id of their manager:
//!
//! 1. the open orders of the account
//! 2. the first time a manager is seen, every closed order of its userref
//! 3. the orders closed since the previous synchronization
//! 4. the orders still open in the database but in none of the above, queried one by one

use crate::exchange::Exchange;
use crate::notify::{OrderEvent, change};
use crate::order::BotOrder;
use crate::storage::Storage;
use crate::{BotError, BotResult};
use kraken_api::trading::{CloseTime, ClosedOrdersRequest, Order};
use std::collections::{BTreeSet, HashSet};

/// Margin of the orders closed since the previous synchronization, in seconds
const OVERLAP: f64 = 3600.0;
const WATERMARK: &str = "closed_orders_watermark";
const PAGE: i32 = 50;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    /// Orders of the managers updated in the database
    pub updated: usize,
    /// Open orders of the managers
    pub open: usize,
    /// Userrefs of open orders which belong to no configured manager
    pub orphans: BTreeSet<i64>,
    /// What changed in the orders known before: fills, expiries, cancellations
    pub events: Vec<OrderEvent>,
}

/// Saves the orders, noting what changed in the ones known before
fn track(storage: &Storage, orders: &[BotOrder], events: &mut Vec<OrderEvent>) -> BotResult<()> {
    for order in orders {
        let previous = storage.order(&order.txid)?;
        if let Some(change) = change(previous.as_ref(), order) {
            events.push(OrderEvent {
                change,
                order: order.clone(),
            });
        }
    }
    storage.upsert_orders(orders)?;
    Ok(())
}

fn backfilled(id: i32) -> String {
    format!("backfilled:{id}")
}

fn of_managers(orders: &[Order], ids: &HashSet<i32>) -> Vec<BotOrder> {
    orders
        .iter()
        .filter_map(BotOrder::from_kraken)
        .filter(|it| ids.contains(&it.manager_id))
        .collect()
}

/// Every closed order of the request, page after page
pub(crate) async fn all_closed<E: Exchange>(
    exchange: &E,
    request: ClosedOrdersRequest,
) -> BotResult<Vec<Order>> {
    let mut orders = Vec::new();
    loop {
        let page = exchange
            .closed_orders(&ClosedOrdersRequest {
                offset: Some(orders.len() as i32),
                ..request.clone()
            })
            .await?;
        let received = page.orders.len();
        orders.extend(page.orders);
        if received < PAGE as usize || orders.len() as i64 >= page.count {
            return Ok(orders);
        }
    }
}

pub async fn synchronize<E: Exchange>(
    exchange: &E,
    storage: &Storage,
    ids: &HashSet<i32>,
    now: f64,
) -> BotResult<SyncReport> {
    let mut report = SyncReport::default();

    // 1. open orders
    let open = exchange.open_orders().await?;
    report.orphans = open
        .iter()
        .filter_map(|it| it.userref)
        .filter(|it| *it != 0 && !i32::try_from(*it).is_ok_and(|id| ids.contains(&id)))
        .collect();
    let open = of_managers(&open, ids);
    let open_txids: HashSet<String> = open.iter().map(|it| it.txid.clone()).collect();
    report.open = open.len();
    track(storage, &open, &mut report.events)?;
    report.updated += open.len();

    // 2. the history of the managers seen for the first time
    let mut seen: HashSet<String> = open_txids.clone();
    let mut sorted_ids: Vec<_> = ids.iter().copied().collect();
    sorted_ids.sort();
    for id in sorted_ids {
        if storage.state(&backfilled(id))?.is_some() {
            continue;
        }
        let history = all_closed(
            exchange,
            ClosedOrdersRequest {
                userref: Some(id),
                ..Default::default()
            },
        )
        .await?;
        let orders = of_managers(&history, ids);
        track(storage, &orders, &mut report.events)?;
        report.updated += orders.len();
        seen.extend(orders.into_iter().map(|it| it.txid));
        storage.set_state(&backfilled(id), &now.to_string())?;
    }

    // 3. the orders closed since the previous synchronization
    if let Some(watermark) = storage
        .state(WATERMARK)?
        .and_then(|it| it.parse::<f64>().ok())
    {
        let recent = all_closed(
            exchange,
            ClosedOrdersRequest {
                start: Some((watermark - OVERLAP).floor() as i64),
                close_time: Some(CloseTime::Close),
                ..Default::default()
            },
        )
        .await?;
        let orders = of_managers(&recent, ids);
        track(storage, &orders, &mut report.events)?;
        report.updated += orders.len();
        seen.extend(orders.into_iter().map(|it| it.txid));
    }

    // 4. the orders the database believes open, found nowhere above
    let dangling: Vec<String> = storage
        .open_orders()?
        .into_iter()
        .filter(|it| ids.contains(&it.manager_id) && !seen.contains(&it.txid))
        .map(|it| it.txid)
        .collect();
    if !dangling.is_empty() {
        let queried = of_managers(&exchange.query_orders(&dangling).await?, ids);
        track(storage, &queried, &mut report.events)?;
        report.updated += queried.len();
        let found: HashSet<_> = queried.iter().map(|it| it.txid.as_str()).collect();
        for txid in dangling.iter().filter(|it| !found.contains(it.as_str())) {
            tracing::warn!(txid, "an order of the database is unknown to Kraken");
        }
    }

    storage.set_state(WATERMARK, &now.to_string())?;
    if !report.orphans.is_empty() {
        tracing::warn!(orphans = ?report.orphans, "open orders whose userref is no configured manager: left untouched");
    }
    Ok(report)
}

impl From<crate::storage::StorageError> for BotError {
    fn from(error: crate::storage::StorageError) -> Self {
        BotError::Storage(error.to_string())
    }
}

impl From<kraken_api::KrakenError> for BotError {
    fn from(error: kraken_api::KrakenError) -> Self {
        BotError::Exchange(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::SimulatedExchange;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    fn exchange() -> SimulatedExchange {
        let exchange = SimulatedExchange::new(1_800_000_000.0);
        exchange.add_pair("XXBTZUSD", "XBTUSD", "XXBT", "ZUSD", d("50000"));
        exchange.deposit("ZUSD", d("1000000"));
        exchange
    }

    fn closed_order(exchange: &SimulatedExchange, userref: i64) -> String {
        let txid = exchange.add_foreign_order(
            Some(userref),
            "XBTUSD",
            kraken_api::trading::OrderSide::Buy,
            d("0.001"),
            d("49000"),
        );
        exchange.fill(&txid, None);
        exchange.advance(1.0);
        txid
    }

    #[tokio::test]
    async fn reads_every_page_of_the_history() {
        let exchange = exchange();
        for _ in 0..120 {
            closed_order(&exchange, 1);
        }
        closed_order(&exchange, 2);
        let storage = Storage::open_in_memory().unwrap();

        let report = synchronize(&exchange, &storage, &HashSet::from([1]), exchange.now())
            .await
            .unwrap();
        assert_eq!(report.updated, 120);
        assert_eq!(storage.orders_of(1).unwrap().len(), 120);
        assert!(storage.orders_of(2).unwrap().is_empty());

        // the history is read once; the orders closed within the hour before a synchronization
        // are read again at the next one (the overlap), then no more
        exchange.advance(2.0 * OVERLAP);
        let again = synchronize(&exchange, &storage, &HashSet::from([1]), exchange.now())
            .await
            .unwrap();
        assert_eq!(again.updated, 120, "{again:?}");
        let then = synchronize(&exchange, &storage, &HashSet::from([1]), exchange.now())
            .await
            .unwrap();
        assert_eq!(then.updated, 0, "{then:?}");
    }

    #[tokio::test]
    async fn finds_the_orders_closed_since_the_previous_synchronization() {
        let exchange = exchange();
        let storage = Storage::open_in_memory().unwrap();
        let ids = HashSet::from([1]);
        synchronize(&exchange, &storage, &ids, exchange.now())
            .await
            .unwrap();

        let txid = closed_order(&exchange, 1);
        synchronize(&exchange, &storage, &ids, exchange.now())
            .await
            .unwrap();
        assert!(
            storage
                .order(&txid)
                .unwrap()
                .is_some_and(|it| it.is_final())
        );
    }

    #[tokio::test]
    async fn asks_kraken_for_the_orders_believed_open() {
        let exchange = exchange();
        let storage = Storage::open_in_memory().unwrap();
        let ids = HashSet::from([1]);
        synchronize(&exchange, &storage, &ids, exchange.now())
            .await
            .unwrap();

        // closed long before the watermark: only QueryOrders finds it
        let txid = exchange.add_foreign_order(
            Some(1),
            "XBTUSD",
            kraken_api::trading::OrderSide::Buy,
            d("0.001"),
            d("49000"),
        );
        let open = BotOrder::from_kraken(
            &exchange
                .orders()
                .into_iter()
                .find(|it| it.txid == txid)
                .unwrap(),
        )
        .unwrap();
        storage.upsert_orders(&[open]).unwrap();
        exchange.cancel(&txid);
        storage
            .set_state(WATERMARK, &(exchange.now() + 100_000.0).to_string())
            .unwrap();

        synchronize(&exchange, &storage, &ids, exchange.now())
            .await
            .unwrap();
        assert_eq!(
            storage.order(&txid).unwrap().unwrap().status,
            kraken_api::trading::OrderStatus::Canceled
        );
    }

    #[tokio::test]
    async fn keeps_an_order_unknown_to_kraken() {
        let exchange = exchange();
        let storage = Storage::open_in_memory().unwrap();
        let ids = HashSet::from([1]);
        let ghost = BotOrder {
            txid: "OGHOST-AAAAA-BBBBBB".into(),
            manager_id: 1,
            pair: "XBTUSD".into(),
            side: kraken_api::trading::OrderSide::Buy,
            status: kraken_api::trading::OrderStatus::Open,
            volume: d("0.001"),
            volume_executed: Decimal::ZERO,
            cost: Decimal::ZERO,
            fee: Decimal::ZERO,
            limit_price: Some(d("49000")),
            average_price: None,
            open_time: 1.0,
            close_time: None,
            expire_time: None,
        };
        storage.upsert_orders(std::slice::from_ref(&ghost)).unwrap();

        synchronize(&exchange, &storage, &ids, exchange.now())
            .await
            .unwrap();
        // left as it is: the manager waits for it, which is the safe side
        assert_eq!(storage.order(&ghost.txid).unwrap(), Some(ghost));
    }
}
