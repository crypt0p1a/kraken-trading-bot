//! Synchronizes the orders of the managers, from Kraken to the database. The orders are found by
//! their userref, which is the id of their manager:
//!
//! 1. the open orders of the account
//! 2. the first time a manager is seen, every closed order of its userref
//! 3. the orders closed since the previous synchronization
//! 4. the orders still open in the database but in none of the above, queried one by one

use crate::exchange::Exchange;
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

async fn all_closed<E: Exchange>(
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
    storage.upsert_orders(&open)?;
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
        storage.upsert_orders(&orders)?;
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
        storage.upsert_orders(&orders)?;
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
        storage.upsert_orders(&queried)?;
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
