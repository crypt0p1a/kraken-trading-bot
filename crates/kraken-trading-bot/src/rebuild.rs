//! The virtual managers found again in the orders of Kraken, when the database doesn't know them
//! (lost, or new). Their id tells which manager of the YAML file they copied; the prices of their
//! orders check its parameters:
//!
//! - the budget: the first buy is placed with all of it, fees included
//! - `sell_above`: the last sell brings back what the position cost + `sell_above`%, fees deducted
//! - `buy_below`: the last buy is at -`buy_below`% of the price of the market when it was placed
//!
//! A parameter the orders contradict is taken from the orders. A virtual manager whose copied
//! manager is gone is rebuilt closing, from its orders alone: without any sell, it sells for
//! what its buy cost + `FALLBACK_SELL_ABOVE`%. One which holds nothing is rebuilt
//! deleted: its id stays taken.

use crate::config::{
    BotConfig, FALLBACK_SELL_ABOVE, ManagerConfig, ManagerMode, VirtualId, is_virtual,
};
use crate::decimal::percent;
use crate::exchange::Exchange;
use crate::ledger;
use crate::order::BotOrder;
use crate::pair::PairRules;
use crate::storage::{ActionRecord, Storage, VirtualManager};
use crate::sync;
use crate::{BotError, BotResult};
use kraken_api::trading::{ClosedOrdersRequest, OrderSide};
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashSet};

/// In the sync state once the orders of Kraken have been searched for virtual managers
pub const REBUILT: &str = "virtual_managers_rebuilt";
/// In the sync state after a failed search: when to try again
pub const RETRY_AT: &str = "virtual_managers_rebuild_retry_at";
/// A failed search is tried again an hour later
pub const RETRY_SECONDS: f64 = 3600.0;

/// A budget the first buy differs from by more than this ratio is the one of the orders
const BUDGET_TOLERANCE: Decimal = Decimal::from_parts(2, 0, 0, false, 2);
/// A percentage the orders differ from by more than these points is the one of the orders
const PERCENT_TOLERANCE: Decimal = Decimal::from_parts(1, 0, 0, false, 1);
/// The decimals of a percentage found in the orders
const PERCENT_DECIMALS: u32 = 2;
/// The decimals of a budget found in the orders: an amount of Kraken, whatever the quote (a budget
/// in XBT has more than 2)
const BUDGET_DECIMALS: u32 = 8;
/// The trades read to find the price of the market when a buy was placed, before it
const PRICE_WINDOW: f64 = 60.0;

/// The budget the first buy was placed with: everything, fees included
pub fn implied_budget(first_buy: &BotOrder, fee_rate: Decimal) -> Option<Decimal> {
    let price = first_buy.limit_price?;
    Some(first_buy.volume * price * (Decimal::ONE + fee_rate))
}

/// The `sell_above` of a sell, from what the position it sells cost
pub fn implied_sell_above(
    sell: &BotOrder,
    position_cost: Decimal,
    fee_rate: Decimal,
) -> Option<Decimal> {
    let price = sell.limit_price?;
    if position_cost <= Decimal::ZERO {
        return None;
    }
    let brought = price * sell.volume * (Decimal::ONE - fee_rate);
    Some((brought / position_cost - Decimal::ONE) * Decimal::ONE_HUNDRED)
}

/// The `buy_below` of a buy, from the price of the market when it was placed
pub fn implied_buy_below(buy: &BotOrder, reference: Decimal) -> Option<Decimal> {
    let price = buy.limit_price?;
    if reference <= Decimal::ZERO {
        return None;
    }
    Some((Decimal::ONE - price / reference) * Decimal::ONE_HUNDRED)
}

/// The value of the copied manager when the orders agree with it, else the one of the orders,
/// with the value of the orders rounded to `decimals`
fn choose(
    template: Option<Decimal>,
    implied: Option<Decimal>,
    agrees: impl Fn(Decimal, Decimal) -> bool,
    decimals: u32,
) -> Option<Decimal> {
    match (template, implied) {
        (Some(template), Some(implied)) if agrees(template, implied) => Some(template),
        (_, Some(implied)) => Some(implied.round_dp(decimals).normalize()),
        (template, None) => template,
    }
}

fn close_percent(template: Decimal, implied: Decimal) -> bool {
    (template - implied).abs() <= PERCENT_TOLERANCE
}

fn close_budget(template: Decimal, implied: Decimal) -> bool {
    (template - implied).abs() <= implied * BUDGET_TOLERANCE
}

fn by_time(orders: &[BotOrder], side: OrderSide) -> Vec<&BotOrder> {
    let mut found: Vec<&BotOrder> = orders.iter().filter(|it| it.side == side).collect();
    found.sort_by(|a, b| a.open_time.total_cmp(&b.open_time));
    found
}

/// The price of the market when an order was placed at `time`: the last trade before it (the
/// price the bot saw), else the first one after
async fn price_at<E: Exchange>(exchange: &E, pair: &str, time: f64) -> BotResult<Option<Decimal>> {
    let trades = exchange.trades(pair, time - PRICE_WINDOW).await?;
    Ok(trades
        .iter()
        .rev()
        .find(|it| it.time < time)
        .or(trades.first())
        .map(|it| it.price))
}

/// What a search of the orders of Kraken rebuilt
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RebuildReport {
    pub rebuilt: Vec<Rebuilt>,
    /// Userrefs which couldn't be rebuilt now (a call to Kraken failing): tried again later
    pub failed: usize,
}

/// What the rebuild found for a virtual manager
#[derive(Debug, Clone, PartialEq)]
pub struct Rebuilt {
    pub config: ManagerConfig,
    pub template_id: i32,
    /// It holds something: it's a manager again. Otherwise it's rebuilt deleted
    pub holds: bool,
    pub reason: String,
}

/// The parameters of a virtual manager, from its orders and the manager its id says it copied.
/// None when its orders aren't enough to sell what it holds
pub async fn rebuild_one<E: Exchange>(
    exchange: &E,
    id: i32,
    template: Option<&ManagerConfig>,
    fallback_sell_above: Decimal,
    orders: &[BotOrder],
    rules: &PairRules,
    fee_rate: Decimal,
) -> BotResult<Option<ManagerConfig>> {
    let buys = by_time(orders, OrderSide::Buy);
    let sells = by_time(orders, OrderSide::Sell);

    let budget = choose(
        template.map(|it| it.budget),
        buys.first().and_then(|it| implied_budget(it, fee_rate)),
        close_budget,
        BUDGET_DECIMALS,
    );
    let Some(budget) = budget else {
        return Ok(None);
    };

    let sell_above = match sells.last() {
        Some(sell) => {
            // placed in the same second as the sell, the buy is before it
            let before: Vec<BotOrder> = orders
                .iter()
                .filter(|it| it.open_time <= sell.open_time && it.txid != sell.txid)
                .cloned()
                .collect();
            let cost = ledger::compute(budget, &before).position_cost;
            choose(
                template.map(|it| it.sell_above),
                implied_sell_above(sell, cost, fee_rate),
                close_percent,
                PERCENT_DECIMALS,
            )
        }
        // never sold: the copied manager, else the fallback over what the buy cost
        None => Some(template.map_or(fallback_sell_above, |it| it.sell_above)),
    };
    let Some(sell_above) = sell_above.filter(|it| *it > Decimal::ZERO) else {
        return Ok(None);
    };

    let implied_buy_below = match buys.last() {
        Some(buy) => price_at(exchange, &rules.key, buy.open_time)
            .await?
            .and_then(|reference| implied_buy_below(buy, reference)),
        None => None,
    };
    let buy_below = choose(
        template.map(|it| it.buy_below),
        implied_buy_below,
        close_percent,
        PERCENT_DECIMALS,
    )
    .unwrap_or_default()
    .clamp(Decimal::ZERO, Decimal::new(99, 0));

    Ok(Some(ManagerConfig {
        id,
        pair: rules.altname.clone(),
        budget,
        buy_below,
        sell_above,
        // closing with the manager it copies, or without it
        mode: match template {
            Some(template) if template.mode == ManagerMode::Active => ManagerMode::Active,
            _ => ManagerMode::Closing,
        },
    }))
}

/// Rebuilds the virtual manager of this userref from its orders. None when it's left untouched
#[allow(clippy::too_many_arguments)]
async fn rebuild_id<E: Exchange>(
    exchange: &E,
    storage: &Storage,
    config: &BotConfig,
    pairs: &std::collections::HashMap<String, kraken_api::market::AssetPair>,
    id: i32,
    orders: &[BotOrder],
    now_ms: i64,
) -> BotResult<Option<Rebuilt>> {
    let Some(decoded) = VirtualId::decode(id) else {
        tracing::warn!(
            manager = id,
            "a userref of virtual manager which copies no manager: left untouched"
        );
        return Ok(None);
    };
    let Some((key, pair)) = pairs
        .iter()
        .find(|(key, pair)| PairRules::matches(&orders[0].pair, key, pair))
    else {
        tracing::warn!(manager = id, pair = %orders[0].pair, "the pair of a virtual manager is unknown: left untouched");
        return Ok(None);
    };
    let rules = PairRules::from_asset_pair(key, pair);
    let fee = exchange
        .trade_volume(std::slice::from_ref(key))
        .await?
        .fees
        .get(key)
        .map(|it| it.fee)
        .ok_or_else(|| BotError::Config(format!("no fee for {key}")))?;
    let fee_rate = percent(fee);

    // the copied manager, if it still trades the same pair
    let template = config
        .managers
        .iter()
        .find(|it| it.id == decoded.template && PairRules::matches(&it.pair, key, pair));

    let found = rebuild_one(
        exchange,
        id,
        template,
        FALLBACK_SELL_ABOVE,
        orders,
        &rules,
        fee_rate,
    )
    .await?;
    let holds_orders = orders.iter().any(|it| !it.is_final());
    let (manager, holds, reason) = match found {
        Some(config) => {
            let balances = ledger::compute(config.budget, orders);
            let base = balances.base_available + balances.base_in_sell_order;
            let holds = holds_orders || base >= rules.ordermin;
            let source = match template {
                Some(_) => format!("copy of manager {}", decoded.template),
                None => format!("manager {} is gone: closing", decoded.template),
            };
            let reason = format!("rebuilt from {} orders of Kraken, {source}", orders.len());
            (config, holds, reason)
        }
        None => {
            tracing::warn!(
                manager = id,
                "the orders of a virtual manager don't say how to sell what it holds: left untouched, its id stays taken"
            );
            let manager = ManagerConfig {
                id,
                pair: rules.altname.clone(),
                budget: Decimal::ZERO,
                buy_below: Decimal::ZERO,
                sell_above: Decimal::ZERO,
                mode: ManagerMode::Closing,
            };
            (
                manager,
                false,
                "can't be rebuilt from its orders".to_string(),
            )
        }
    };

    storage.add_virtual_manager(&VirtualManager {
        config: manager.clone(),
        template_id: decoded.template,
        created_at: now_ms,
        reason: reason.clone(),
    })?;
    if !holds {
        storage.delete_virtual_manager(id, now_ms, &format!("{reason}: holds nothing"))?;
    }
    storage.record_action(&ActionRecord {
        manager_id: id,
        at: now_ms,
        kind: "virtual_rebuilt".into(),
        amount: Some(manager.budget),
        price: None,
        reason: Some(reason.clone()),
        dry_run: config.dry_run,
        txid: None,
        error: None,
    })?;
    tracing::info!(manager = id, holds, pair = %manager.pair, budget = %manager.budget, buy_below = %manager.buy_below, sell_above = %manager.sell_above, %reason, "virtual manager rebuilt");
    Ok(Some(Rebuilt {
        config: manager,
        template_id: decoded.template,
        holds,
        reason,
    }))
}

/// Searches every order of the account for the virtual managers the database doesn't know, and
/// saves them
pub async fn rebuild<E: Exchange>(
    exchange: &E,
    storage: &Storage,
    config: &BotConfig,
    now_ms: i64,
) -> BotResult<RebuildReport> {
    let known: HashSet<i32> = storage.virtual_ids()?.into_iter().collect();
    let open = exchange.open_orders().await?;
    let closed = sync::all_closed(exchange, ClosedOrdersRequest::default()).await?;

    let mut by_id: BTreeMap<i32, Vec<BotOrder>> = BTreeMap::new();
    let mut txids = HashSet::new();
    for order in open.iter().chain(&closed).filter_map(BotOrder::from_kraken) {
        if is_virtual(order.manager_id)
            && !known.contains(&order.manager_id)
            && txids.insert(order.txid.clone())
        {
            by_id.entry(order.manager_id).or_default().push(order);
        }
    }
    if by_id.is_empty() {
        return Ok(RebuildReport::default());
    }

    let pairs = exchange.asset_pairs(&[]).await?;
    let mut report = RebuildReport::default();
    for (id, orders) in by_id {
        // one which can't be rebuilt now doesn't stop the others: it's tried again later
        match rebuild_id(exchange, storage, config, &pairs, id, &orders, now_ms).await {
            Ok(Some(rebuilt)) => report.rebuilt.push(rebuilt),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(manager = id, %error, "a virtual manager couldn't be rebuilt: tried again later");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::tests::d;
    use kraken_api::trading::OrderStatus;

    fn order(side: OrderSide, volume: &str, price: &str) -> BotOrder {
        BotOrder {
            txid: "OAAAAA-BBBBB-CCCCCC".into(),
            manager_id: 1_000_001,
            pair: "XBTEUR".into(),
            side,
            status: OrderStatus::Open,
            volume: d(volume),
            volume_executed: Decimal::ZERO,
            cost: Decimal::ZERO,
            fee: Decimal::ZERO,
            limit_price: Some(d(price)),
            average_price: None,
            open_time: 1.0,
            close_time: None,
            expire_time: None,
        }
    }

    #[test]
    fn finds_the_parameters_in_the_prices_of_the_orders() {
        let fee = d("0.0025");
        // 30 / (49500 * 1.0025), rounded down to 8 decimals
        let buy = order(OrderSide::Buy, "0.00060453", "49500");
        let budget = implied_budget(&buy, fee).unwrap();
        assert!((budget - d("30")).abs() < d("0.001"), "{budget}");
        assert_eq!(implied_buy_below(&buy, d("50000")), Some(d("1")));

        // sells for 29.99... + 1.5%, fees deducted
        let cost = d("29.9999");
        let sell = order(OrderSide::Sell, "0.00060453", "50496.4");
        let sell_above = implied_sell_above(&sell, cost, fee).unwrap();
        assert!((sell_above - d("1.5")).abs() < d("0.01"), "{sell_above}");

        assert_eq!(implied_sell_above(&sell, Decimal::ZERO, fee), None);
        assert_eq!(implied_buy_below(&buy, Decimal::ZERO), None);
    }

    #[test]
    fn keeps_the_decimals_of_a_budget_in_a_crypto_quote() {
        // 0.004 XBT spent by the first buy, its copied manager gone
        assert_eq!(
            choose(
                None,
                Some(d("0.00400451234")),
                close_budget,
                BUDGET_DECIMALS
            ),
            Some(d("0.00400451"))
        );
        assert_eq!(
            choose(None, Some(d("1.23456")), close_percent, PERCENT_DECIMALS),
            Some(d("1.23"))
        );
    }

    #[test]
    fn keeps_the_copied_value_the_orders_agree_with() {
        assert_eq!(
            choose(Some(d("1.5")), Some(d("1.53")), close_percent, 2),
            Some(d("1.5"))
        );
        assert_eq!(
            choose(Some(d("3")), Some(d("1.5012")), close_percent, 2),
            Some(d("1.5"))
        );
        assert_eq!(choose(Some(d("3")), None, close_percent, 2), Some(d("3")));
        assert_eq!(choose(None, None, close_percent, 2), None);
        assert_eq!(
            choose(Some(d("30")), Some(d("30.4")), close_budget, 8),
            Some(d("30"))
        );
        assert_eq!(
            choose(Some(d("50")), Some(d("29.999")), close_budget, 8),
            Some(d("29.999"))
        );
    }
}
