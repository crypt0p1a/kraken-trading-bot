//! A demo database for the dashboard: 7 days of a simulated market, traded by the strategy of the
//! bot, written by the storage of the bot itself
//!
//! ```text
//! cargo run -p kraken-dashboard --example demo -- demo.sqlite
//! cargo run -p kraken-dashboard -- demo.sqlite
//! ```

use kraken_spot_api::trading::{OrderSide, OrderStatus};
use kraken_trading_bot::config::{ManagerConfig, ManagerMode};
use kraken_trading_bot::ledger;
use kraken_trading_bot::market::Indicators;
use kraken_trading_bot::order::BotOrder;
use kraken_trading_bot::storage::{
    AccountRecord, ActionRecord, ManagerState, Storage, TickerRecord, VirtualManager,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use std::collections::BTreeMap;
use std::path::PathBuf;

const TICK: i64 = 5 * 60_000;
const TICKS: i64 = 7 * 24 * 12;
const FEE: f64 = 0.004;
/// A buy not filled after 12 hours expires
const BUY_TIMEOUT: i64 = 12 * 3_600_000;
/// A buy not filled is placed again once the price rose 1.5% above the one it was placed from
const TRAILING: f64 = 1.5;
/// The virtual manager copying #2, added after a day
const VIRTUAL: i32 = 1_001_002;

struct Pair {
    key: &'static str,
    altname: &'static str,
    /// The price the market comes back to
    base: f64,
    /// Distance to the base, in log
    drift: f64,
    price: f64,
    /// Volatility of a tick, in %
    volatility: f64,
    decimals: u32,
    history: Vec<f64>,
}

struct Manager {
    config: ManagerConfig,
    orders: Vec<BotOrder>,
    /// The price each open buy was placed from
    reference: f64,
}

fn decimal(value: f64, decimals: u32) -> Decimal {
    Decimal::from_f64(value)
        .unwrap_or_default()
        .round_dp(decimals)
}

fn f(value: Decimal) -> f64 {
    value.to_f64().unwrap_or_default()
}

fn manager(id: i32, pair: &str, budget: &str, buy_below: &str, sell_above: &str) -> Manager {
    Manager {
        config: ManagerConfig {
            id,
            pair: pair.into(),
            budget: budget.parse().unwrap(),
            buy_below: buy_below.parse().unwrap(),
            sell_above: sell_above.parse().unwrap(),
            mode: ManagerMode::Active,
        },
        orders: Vec::new(),
        reference: 0.0,
    }
}

fn ema(values: &[f64], period: usize) -> f64 {
    let k = 2.0 / (period as f64 + 1.0);
    values
        .iter()
        .skip(1)
        .fold(values[0], |ema, it| it * k + ema * (1.0 - k))
}

fn indicators(history: &[f64]) -> Indicators {
    // 24 hours of ticks of 5 minutes
    let day = &history[history.len().saturating_sub(288)..];
    let close = *day.last().unwrap();
    let (low, high) = day.iter().fold((f64::MAX, f64::MIN), |(low, high), it| {
        (low.min(*it), high.max(*it))
    });
    let moves: f64 =
        day.windows(2).map(|it| (it[1] - it[0]).abs()).sum::<f64>() / day.len().max(2) as f64;
    Indicators {
        close,
        atr_percent: moves * 3.0 / close * 100.0,
        ema_fast: ema(&day[day.len().saturating_sub(36)..], 12),
        ema_slow: ema(&day[day.len().saturating_sub(144)..], 48),
        slope_24h_percent: (close - day[0]) / day[0] * 100.0 * 0.8,
        range_24h_percent: (high - low) / close * 100.0,
        change_24h_percent: (close - day[0]) / day[0] * 100.0,
        candles: (day.len() / 3).min(96),
    }
}

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "demo.sqlite".into()),
    );
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let storage = Storage::open(&path).expect("the demo database");
    fastrand::seed(7);

    let mut pairs = [
        Pair {
            key: "XXBTZUSD",
            base: 74_800.0,
            drift: 0.0,
            altname: "XBTUSD",
            price: 74_800.0,
            volatility: 0.22,
            decimals: 1,
            history: Vec::new(),
        },
        Pair {
            key: "XETHZUSD",
            base: 2_190.0,
            drift: 0.0,
            altname: "ETHUSD",
            price: 2_190.0,
            volatility: 0.30,
            decimals: 2,
            history: Vec::new(),
        },
        Pair {
            key: "SOLUSD",
            base: 176.0,
            drift: 0.0,
            altname: "SOLUSD",
            price: 176.0,
            volatility: 0.42,
            decimals: 2,
            history: Vec::new(),
        },
    ];
    let mut managers = vec![
        manager(1, "XBTUSD", "30", "0.5", "0.8"),
        manager(2, "XBTUSD", "30", "1.0", "1.5"),
        manager(3, "XBT/USD", "30", "2.0", "3.0"),
        manager(4, "ETHUSD", "20", "0.8", "1.2"),
        manager(5, "ETHUSD", "20", "1.5", "2.5"),
        manager(6, "SOLUSD", "25", "1.0", "2.0"),
    ];

    let now = kraken_trading_bot::clock::now_millis();
    let start = now - TICKS * TICK;
    let mut counter = 0;

    for tick in 0..TICKS {
        let at = start + tick * TICK + fastrand::i64(0..900);
        let seconds = at as f64 / 1000.0;

        // the market: noise around waves of a few hours, swinging ±3% around its base
        for (index, pair) in pairs.iter_mut().enumerate() {
            let target = ((tick as f64) / (9.0 + index as f64 * 2.5)).sin() * 0.03
                + ((tick as f64) / 190.0).sin() * 0.02;
            let noise = (fastrand::f64() - 0.5) * 2.0 * pair.volatility / 100.0;
            pair.drift += noise + (target - pair.drift) * 0.06;
            pair.price = pair.base * pair.drift.exp();
            pair.history.push(pair.price);
            let price = decimal(pair.price, pair.decimals);
            let spread = pair.price * 0.00008;
            storage
                .record_ticker(&TickerRecord {
                    pair: pair.key.into(),
                    at,
                    last: price,
                    bid: decimal(pair.price - spread, pair.decimals),
                    ask: decimal(pair.price + spread, pair.decimals),
                    volume_24h: decimal(1_000.0 + fastrand::f64() * 500.0, 4),
                    vwap_24h: decimal(pair.price * 0.998, pair.decimals),
                })
                .unwrap();
            if pair.history.len() >= 48 {
                storage
                    .record_indicators(pair.key, at, &indicators(&pair.history))
                    .unwrap();
            }
        }

        if tick == TICKS / 2 {
            let mut copy = manager(VIRTUAL, "XBTUSD", "30", "1.0", "1.5");
            copy.config.id = VIRTUAL;
            storage
                .add_virtual_manager(&VirtualManager {
                    config: copy.config.clone(),
                    template_id: 2,
                    created_at: at,
                    reason: "30 USD unused in the account".into(),
                })
                .unwrap();
            storage
                .record_action(&ActionRecord {
                    manager_id: VIRTUAL,
                    at,
                    kind: "virtual_created".into(),
                    amount: Some(copy.config.budget),
                    price: None,
                    reason: Some("copy of #2: 30 USD unused in the account".into()),
                    dry_run: false,
                    txid: None,
                    error: None,
                })
                .unwrap();
            managers.push(copy);
        }

        for manager in &mut managers {
            let pair = pairs
                .iter()
                .find(|it| it.altname == manager.config.pair.replace('/', ""))
                .unwrap();
            let price = pair.price;
            let record = |kind: &str,
                          amount: Option<Decimal>,
                          price: Option<Decimal>,
                          reason: String,
                          txid: Option<String>,
                          error: Option<String>| {
                storage
                    .record_action(&ActionRecord {
                        manager_id: manager.config.id,
                        at,
                        kind: kind.into(),
                        amount,
                        price,
                        reason: Some(reason),
                        dry_run: false,
                        txid,
                        error,
                    })
                    .unwrap();
            };

            // the open order: filled when the price crosses it, in parts sometimes
            let mut changed = Vec::new();
            if let Some(order) = manager.orders.iter_mut().find(|it| !it.is_final()) {
                let limit = f(order.limit_price.unwrap());
                let crossed = match order.side {
                    OrderSide::Buy => price <= limit,
                    OrderSide::Sell => price >= limit,
                };
                let opened = (order.open_time * 1000.0) as i64;
                if crossed {
                    let share = if order.volume_executed.is_zero() && fastrand::f64() < 0.3 {
                        0.35 + fastrand::f64() * 0.35
                    } else {
                        1.0
                    };
                    let executed = if share >= 1.0 {
                        order.volume
                    } else {
                        (order.volume * decimal(share, 4)).round_dp(8)
                    };
                    order.volume_executed = executed;
                    order.cost = (executed * order.limit_price.unwrap()).round_dp(5);
                    order.fee = (order.cost * decimal(FEE, 4)).round_dp(5);
                    order.average_price = order.limit_price;
                    if executed == order.volume {
                        order.status = OrderStatus::Closed;
                        order.close_time = Some(seconds);
                    }
                    changed.push(order.clone());
                } else if order.side == OrderSide::Buy && order.volume_executed.is_zero() {
                    if price > manager.reference * (1.0 + TRAILING / 100.0) {
                        order.status = OrderStatus::Canceled;
                        order.close_time = Some(seconds);
                        changed.push(order.clone());
                        record(
                            "buy_cancelled",
                            None,
                            None,
                            format!(
                                "the price rose {TRAILING}% above the one the buy was placed from"
                            ),
                            Some(order.txid.clone()),
                            None,
                        );
                    } else if at - opened > BUY_TIMEOUT {
                        order.status = OrderStatus::Expired;
                        order.close_time = Some(seconds);
                        changed.push(order.clone());
                    }
                }
            }
            if !changed.is_empty() {
                storage.upsert_orders(&changed).unwrap();
            }

            let balances = ledger::compute(manager.config.budget, &manager.orders);
            storage
                .save_manager(&ManagerState {
                    config: manager.config.clone(),
                    balances: balances.clone(),
                    last_price: decimal(price, pair.decimals),
                    at,
                })
                .unwrap();

            if manager.orders.iter().any(|it| !it.is_final()) {
                if fastrand::f64() < 0.15 {
                    record("idle", None, None, "an order is open".into(), None, None);
                }
                continue;
            }

            // no open order: sell what's held for its cost + sell_above, else buy below the market
            let (side, volume, limit, reason) = if balances.base_available > Decimal::ZERO {
                let entry = f(balances.entry_price.unwrap());
                let limit = entry * (1.0 + f(manager.config.sell_above) / 100.0);
                (
                    OrderSide::Sell,
                    balances.base_available,
                    limit,
                    format!("sells at cost +{}%", manager.config.sell_above),
                )
            } else {
                let limit = price * (1.0 - f(manager.config.buy_below) / 100.0);
                let volume = decimal(f(balances.quote_available) / (limit * (1.0 + FEE)), 8);
                manager.reference = price;
                (
                    OrderSide::Buy,
                    volume,
                    limit,
                    format!(
                        "buys at -{}% of {}",
                        manager.config.buy_below,
                        decimal(price, pair.decimals)
                    ),
                )
            };
            let limit = decimal(limit, pair.decimals);
            let kind = if side == OrderSide::Buy {
                "buy"
            } else {
                "sell"
            };

            // now and then, Kraken refuses an order
            if fastrand::f64() < 0.01 {
                record(
                    kind,
                    Some(volume),
                    Some(limit),
                    reason,
                    None,
                    Some("EGeneral:Temporary lockout".into()),
                );
                continue;
            }

            counter += 1;
            let txid = format!(
                "O{:05}-{:05}-DEMO{:02}",
                counter,
                manager.config.id % 100_000,
                tick % 100
            );
            let order = BotOrder {
                txid: txid.clone(),
                manager_id: manager.config.id,
                pair: pair.altname.into(),
                side,
                status: OrderStatus::Open,
                volume,
                volume_executed: Decimal::ZERO,
                cost: Decimal::ZERO,
                fee: Decimal::ZERO,
                limit_price: Some(limit),
                average_price: None,
                open_time: seconds,
                close_time: None,
                expire_time: None,
            };
            storage.upsert_orders(std::slice::from_ref(&order)).unwrap();
            if side == OrderSide::Buy {
                storage
                    .set_order_reference(&txid, decimal(price, pair.decimals))
                    .unwrap();
            }
            manager.orders.push(order);
            record(kind, Some(volume), Some(limit), reason, Some(txid), None);
        }

        // the whole account: the managers, and 20 of each quote no manager uses
        let mut accounts: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
        for manager in &managers {
            let pair = pairs
                .iter()
                .find(|it| it.altname == manager.config.pair.replace('/', ""))
                .unwrap();
            let balances = ledger::compute(manager.config.budget, &manager.orders);
            let account = accounts
                .entry(kraken_dashboard::stats::split(pair.altname).1)
                .or_insert((Decimal::from(20), Decimal::ZERO));
            account.0 += balances.quote_available + balances.quote_in_buy_order;
            account.1 += (balances.base_available + balances.base_in_sell_order)
                * decimal(pair.price, pair.decimals);
        }
        for (quote, (cash, holdings)) in accounts {
            storage
                .record_account(&AccountRecord {
                    at,
                    quote,
                    cash,
                    holdings,
                })
                .unwrap();
        }
    }

    let orders: usize = managers.iter().map(|it| it.orders.len()).sum();
    println!(
        "{}: {} managers, {orders} orders, {TICKS} ticks of 5 minutes",
        path.display(),
        managers.len()
    );
}
