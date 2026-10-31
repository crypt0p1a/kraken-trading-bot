//! A tick of the bot: synchronizes the orders, then each manager decides and acts. Port of the
//! ManagersPool of cex-trading-bot

use crate::clock::now_millis;
use crate::config::{BotConfig, ManagerConfig, ManagerMode};
use crate::decimal::{dec, percent};
use crate::exchange::Exchange;
use crate::ledger::{self, Balances};
use crate::order::BotOrder;
use crate::pair::PairRules;
use crate::storage::{ActionRecord, ManagerState, Storage, TickerRecord};
use crate::strategy::{self, Action, Decision};
use crate::sync::{self, SyncReport};
use crate::{BotError, BotResult};
use kraken_api::trading::{NewOrder, OrderFlag, OrderSide, OrderStatus, OrderType, TimeInForce};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// What a manager did during a tick
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Waited,
    Idle(String),
    /// An order was placed (its txid), or only validated (dry run, no txid)
    Placed {
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        txid: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TickReport {
    pub sync: SyncReport,
    /// Per manager id
    pub outcomes: HashMap<i32, Outcome>,
}

/// The market of a pair at a tick
struct Market {
    rules: PairRules,
    reference: Decimal,
    fee_rate: Decimal,
}

pub struct Bot<E: Exchange> {
    exchange: E,
    storage: Storage,
    config: BotConfig,
    /// Pair of the configuration -> Kraken name
    pair_keys: HashMap<String, String>,
}

impl<E: Exchange> Bot<E> {
    pub fn new(exchange: E, storage: Storage, config: BotConfig) -> Self {
        Self {
            exchange,
            storage,
            config,
            pair_keys: HashMap::new(),
        }
    }

    pub fn exchange(&self) -> &E {
        &self.exchange
    }

    pub fn into_exchange(self) -> E {
        self.exchange
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn config(&self) -> &BotConfig {
        &self.config
    }

    /// Finds the pairs of the managers on Kraken: fails on a pair unknown or not tradable
    pub async fn prepare(&mut self) -> BotResult<()> {
        let all = self.exchange.asset_pairs(&[]).await?;
        for manager in &self.config.managers {
            let (key, pair) = all
                .iter()
                .find(|(key, pair)| PairRules::matches(&manager.pair, key, pair))
                .ok_or_else(|| {
                    BotError::Config(format!(
                        "manager {}: unknown pair {}",
                        manager.id, manager.pair
                    ))
                })?;
            if pair
                .status
                .is_some_and(|it| it != kraken_api::market::AssetPairStatus::Online)
            {
                tracing::warn!(manager = manager.id, pair = %pair.altname, status = ?pair.status, "the pair isn't online");
            }
            self.pair_keys.insert(manager.pair.clone(), key.clone());
        }
        Ok(())
    }

    async fn markets(&self) -> BotResult<HashMap<String, Market>> {
        let mut keys: Vec<String> = self.pair_keys.values().cloned().collect();
        keys.sort();
        keys.dedup();

        let pairs = self.exchange.asset_pairs(&keys).await?;
        let tickers = self.exchange.tickers(&keys).await?;
        let fees = self.exchange.trade_volume(&keys).await?;
        let at = now_millis();

        let mut markets = HashMap::new();
        for key in keys {
            let pair = pairs
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no information for {key}")))?;
            let ticker = tickers
                .get(&key)
                .ok_or_else(|| BotError::Config(format!("no ticker for {key}")))?;
            // the taker fee: the highest one, whatever the order becomes
            let fee = fees
                .fees
                .get(&key)
                .map(|it| dec(it.fee))
                .ok_or_else(|| BotError::Config(format!("no fee for {key}")))?;

            self.storage.record_ticker(&TickerRecord {
                pair: key.clone(),
                at,
                last: dec(ticker.last_trade.price),
                bid: dec(ticker.bid.price),
                ask: dec(ticker.ask.price),
                volume_24h: dec(ticker.volume.last_24h),
                vwap_24h: dec(ticker.vwap.last_24h),
            })?;

            markets.insert(
                key.clone(),
                Market {
                    rules: PairRules::from_asset_pair(&key, pair),
                    reference: dec(ticker.last_trade.price),
                    fee_rate: percent(fee),
                },
            );
        }
        Ok(markets)
    }

    pub async fn tick(&mut self) -> BotResult<TickReport> {
        if self.pair_keys.len() < self.config.managers.len() {
            self.prepare().await?;
        }

        let markets = self.markets().await?;
        let sync = sync::synchronize(
            &self.exchange,
            &self.storage,
            &self.config.ids(),
            self.exchange.now(),
        )
        .await?;

        // what the account has, decreased locally when orders are placed
        let mut available: HashMap<String, Decimal> = self
            .exchange
            .balances()
            .await?
            .into_iter()
            .map(|(asset, balance)| (asset, dec(balance.available())))
            .collect();

        let mut outcomes = HashMap::new();
        for manager in self.config.managers.clone() {
            let market = &markets[&self.pair_keys[&manager.pair]];
            let outcome = match self.run_manager(&manager, market, &mut available).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::error!(manager = manager.id, %error, "the manager failed");
                    Outcome::Failed(error.to_string())
                }
            };
            outcomes.insert(manager.id, outcome);
        }

        Ok(TickReport { sync, outcomes })
    }

    async fn run_manager(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        available: &mut HashMap<String, Decimal>,
    ) -> BotResult<Outcome> {
        let orders = self.storage.orders_of(manager.id)?;
        let balances = ledger::compute(manager.budget, &orders);
        let state = ManagerState {
            config: manager.clone(),
            balances: balances.clone(),
            last_price: market.reference,
            at: now_millis(),
        };
        self.storage.save_manager(&state)?;
        log_balances(manager, &state);

        let open: Vec<&BotOrder> = orders.iter().filter(|it| !it.is_final()).collect();
        for order in &open {
            tracing::info!(manager = manager.id, txid = %order.txid, side = ?order.side, "waiting for its open order");
        }

        let action = strategy::decide(&Decision {
            balances: &balances,
            has_open_order: !open.is_empty(),
            active: manager.mode == ManagerMode::Active,
            buy_below: manager.buy_below,
            sell_above: manager.sell_above,
            reference: market.reference,
            fee_rate: market.fee_rate,
            rules: &market.rules,
        });

        match action {
            Action::Wait => Ok(Outcome::Waited),
            Action::Idle(reason) => self.idle(manager, reason),
            Action::Sell { amount, price } => {
                self.place(manager, market, OrderSide::Sell, amount, price, &balances)
                    .await
            }
            Action::Buy { amount, price } => {
                let cost = amount * price * (Decimal::ONE + market.fee_rate);
                let in_account = available
                    .get(&market.rules.quote_asset)
                    .copied()
                    .unwrap_or_default();
                if cost > in_account {
                    return self.idle(
                        manager,
                        format!(
                            "the account has {} {} available, {} required",
                            in_account.normalize(),
                            market.rules.quote_asset,
                            cost.round_dp(8).normalize()
                        ),
                    );
                }
                let outcome = self
                    .place(manager, market, OrderSide::Buy, amount, price, &balances)
                    .await?;
                if matches!(outcome, Outcome::Placed { .. }) {
                    available.insert(market.rules.quote_asset.clone(), in_account - cost);
                }
                Ok(outcome)
            }
        }
    }

    /// Records the reason, unless it's the same as the last action of the manager
    fn idle(&self, manager: &ManagerConfig, reason: String) -> BotResult<Outcome> {
        tracing::info!(manager = manager.id, %reason, "idle");
        let last = self.storage.actions_of(manager.id, 1)?;
        let repeated = last
            .first()
            .is_some_and(|it| it.kind == "idle" && it.reason.as_deref() == Some(&reason));
        if !repeated {
            self.storage.record_action(&ActionRecord {
                manager_id: manager.id,
                at: now_millis(),
                kind: "idle".into(),
                amount: None,
                price: None,
                reason: Some(reason.clone()),
                dry_run: self.config.dry_run,
                txid: None,
                error: None,
            })?;
        }
        Ok(Outcome::Idle(reason))
    }

    async fn place(
        &self,
        manager: &ManagerConfig,
        market: &Market,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        balances: &Balances,
    ) -> BotResult<Outcome> {
        let dry_run = self.config.dry_run;
        let (time_in_force, expire_time, reason) = match side {
            OrderSide::Buy => (
                TimeInForce::Gtd,
                Some(format!("+{}", self.config.buy_order_timeout.as_secs())),
                format!("reference {} -{}%", market.reference, manager.buy_below),
            ),
            OrderSide::Sell => (
                TimeInForce::Gtc,
                None,
                format!(
                    "cost {} +{}%, fee {}",
                    balances.position_cost.round_dp(8).normalize(),
                    manager.sell_above,
                    market.fee_rate.normalize()
                ),
            ),
        };

        let request = NewOrder {
            pair: market.rules.altname.clone(),
            side,
            order_type: OrderType::Limit,
            volume: amount.normalize().to_string(),
            price: Some(price.normalize().to_string()),
            price2: None,
            // the fees in the quote currency: what the ledger expects
            oflags: Some(vec![OrderFlag::Fciq]),
            time_in_force: Some(time_in_force),
            start_time: None,
            expire_time,
            cl_ord_id: None,
            userref: Some(manager.id),
            validate: dry_run.then_some(true),
        };
        tracing::info!(manager = manager.id, ?side, %amount, %price, dry_run, "placing an order");

        let kind = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let mut record = ActionRecord {
            manager_id: manager.id,
            at: now_millis(),
            kind: kind.into(),
            amount: Some(amount),
            price: Some(price),
            reason: Some(reason),
            dry_run,
            txid: None,
            error: None,
        };

        if let Err(error) = ensure_limit(&request) {
            record.error = Some(error.clone());
            self.storage.record_action(&record)?;
            return Err(BotError::Config(error));
        }

        match self.exchange.new_order(&request).await {
            Ok(answer) => {
                let txid = answer.txids.first().cloned();
                if let Some(txid) = &txid {
                    // saved right away: the next tick doesn't place another order
                    self.storage.upsert_orders(&[BotOrder {
                        txid: txid.clone(),
                        manager_id: manager.id,
                        pair: market.rules.altname.clone(),
                        side,
                        status: OrderStatus::Open,
                        volume: amount,
                        volume_executed: Decimal::ZERO,
                        cost: Decimal::ZERO,
                        fee: Decimal::ZERO,
                        limit_price: Some(price),
                        average_price: None,
                        open_time: self.exchange.now(),
                        close_time: None,
                        expire_time: None,
                    }])?;
                }
                tracing::info!(manager = manager.id, order = %answer.description.order, ?txid, dry_run, "order accepted");
                record.txid = txid.clone();
                self.storage.record_action(&record)?;
                Ok(Outcome::Placed {
                    side,
                    amount,
                    price,
                    txid,
                })
            }
            Err(error) => {
                tracing::error!(manager = manager.id, %error, "order refused");
                record.error = Some(error.to_string());
                self.storage.record_action(&record)?;
                Ok(Outcome::Failed(error.to_string()))
            }
        }
    }
}

/// The bot only trades with limit orders, at the price it computed, with the fees in the quote:
/// any other order is refused before reaching Kraken
pub fn ensure_limit(order: &NewOrder) -> Result<(), String> {
    if order.order_type != OrderType::Limit {
        return Err(format!(
            "refused: a {:?} order, the bot only places limit orders",
            order.order_type
        ));
    }
    let price = order
        .price
        .as_deref()
        .and_then(|it| it.parse::<Decimal>().ok());
    if price.is_none_or(|it| it <= Decimal::ZERO) {
        return Err(format!(
            "refused: a limit order without a valid price ({:?})",
            order.price
        ));
    }
    if order.price2.is_some() {
        return Err("refused: a second price, the bot only places plain limit orders".into());
    }
    if !order
        .oflags
        .as_ref()
        .is_some_and(|it| it.contains(&OrderFlag::Fciq))
    {
        return Err(
            "refused: an order without fciq, the ledger expects the fees in the quote".into(),
        );
    }
    if order
        .oflags
        .as_ref()
        .is_some_and(|it| it.contains(&OrderFlag::Viqc))
    {
        return Err("refused: a volume in the quote (viqc), only for market orders".into());
    }
    Ok(())
}

fn log_balances(manager: &ManagerConfig, state: &ManagerState) {
    let balances = &state.balances;
    tracing::info!(
        manager = manager.id,
        pair = %manager.pair,
        quote = %balances.quote_available.round_dp(8).normalize(),
        quote_in_buy = %balances.quote_in_buy_order.round_dp(8).normalize(),
        base = %balances.base_available.normalize(),
        base_in_sell = %balances.base_in_sell_order.normalize(),
        entry = ?balances.entry_price.map(|it| it.round_dp(8).normalize()),
        value = %state.value().round_dp(8).normalize(),
        budget = %manager.budget,
        "balances"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit() -> NewOrder {
        NewOrder {
            pair: "XBTUSD".into(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            volume: "0.001".into(),
            price: Some("49500".into()),
            price2: None,
            oflags: Some(vec![OrderFlag::Fciq]),
            time_in_force: Some(TimeInForce::Gtd),
            start_time: None,
            expire_time: Some("+43200".into()),
            cl_ord_id: None,
            userref: Some(1),
            validate: None,
        }
    }

    #[test]
    fn accepts_the_limit_orders_of_the_bot() {
        assert_eq!(ensure_limit(&limit()), Ok(()));
        let sell = NewOrder {
            side: OrderSide::Sell,
            time_in_force: Some(TimeInForce::Gtc),
            expire_time: None,
            ..limit()
        };
        assert_eq!(ensure_limit(&sell), Ok(()));
    }

    #[test]
    fn refuses_every_order_which_is_not_a_plain_limit_order() {
        let refused = [
            NewOrder {
                order_type: OrderType::Market,
                price: None,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Market,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::StopLoss,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TakeProfit,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::StopLossLimit,
                price2: Some("49000".into()),
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TakeProfitLimit,
                price2: Some("51000".into()),
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TrailingStop,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::TrailingStopLimit,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Iceberg,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::SettlePosition,
                ..limit()
            },
            NewOrder {
                order_type: OrderType::Unknown,
                ..limit()
            },
            NewOrder {
                price: None,
                ..limit()
            },
            NewOrder {
                price: Some("0".into()),
                ..limit()
            },
            NewOrder {
                price: Some("-1".into()),
                ..limit()
            },
            NewOrder {
                price: Some("market".into()),
                ..limit()
            },
            NewOrder {
                price2: Some("1".into()),
                ..limit()
            },
            NewOrder {
                oflags: None,
                ..limit()
            },
            NewOrder {
                oflags: Some(vec![OrderFlag::Fcib]),
                ..limit()
            },
            NewOrder {
                oflags: Some(vec![OrderFlag::Fciq, OrderFlag::Viqc]),
                ..limit()
            },
        ];
        for order in refused {
            assert!(ensure_limit(&order).is_err(), "{order:?}");
        }
    }
}
