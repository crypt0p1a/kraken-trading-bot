use super::{Exchange, ExchangeResult};
use kraken_api::KrakenError;
use kraken_api::account::{ExtendedBalance, FeeInfo, TradeVolume};
use kraken_api::market::{
    AssetPair, PriceLevel, PublicTrade, Ticker, TradeLevel, Window, WindowCount,
};
use kraken_api::trading::{
    CloseTime, ClosedOrders, ClosedOrdersRequest, NewOrder, NewOrderAnswer, NewOrderDescription,
    Order, OrderDescription, OrderSide, OrderStatus, OrderType, TimeInForce,
};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

/// An in-memory Kraken: the limit orders are filled when the price reaches them, the GTD orders
/// expire, the funds are held by the open orders. The fees are in the quote currency.
pub struct SimulatedExchange {
    state: Mutex<State>,
}

struct State {
    /// Kraken name -> pair
    pairs: HashMap<String, AssetPair>,
    prices: HashMap<String, Decimal>,
    /// Kraken name -> every price it had, with its time, oldest first
    history: HashMap<String, Vec<(f64, Decimal)>>,
    balances: HashMap<String, Decimal>,
    /// Percent, e.g. 0.25
    fee_percent: Decimal,
    orders: Vec<Order>,
    /// UTC seconds
    now: f64,
    next_txid: u64,
    /// Calls of AddOrder, validated ones included
    add_order_calls: usize,
}

fn error(body: &str) -> KrakenError {
    KrakenError::Api {
        status: 200,
        body: body.into(),
    }
}

impl SimulatedExchange {
    pub fn new(now: f64) -> Self {
        Self {
            state: Mutex::new(State {
                pairs: HashMap::new(),
                prices: HashMap::new(),
                history: HashMap::new(),
                balances: HashMap::new(),
                fee_percent: Decimal::new(25, 2),
                orders: Vec::new(),
                now,
                next_txid: 1,
                add_order_calls: 0,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Adds a pair: price with `price_decimals` decimals, volume with 8, minimums of Kraken
    pub fn add_pair(&self, key: &str, altname: &str, base: &str, quote: &str, price: Decimal) {
        let pair: AssetPair = serde_json::from_value(serde_json::json!({
            "altname": altname, "wsname": format!("{}/{}", &altname[..3], &altname[3..]),
            "base": base, "quote": quote, "pair_decimals": 1, "lot_decimals": 8,
            "ordermin": "0.0001", "costmin": "5", "tick_size": "0.1", "status": "online",
        }))
        .unwrap();
        let mut state = self.state();
        state.pairs.insert(key.into(), pair);
        state.prices.insert(key.into(), price);
        let now = state.now;
        state.history.insert(key.into(), vec![(now, price)]);
    }

    pub fn set_fee_percent(&self, fee: Decimal) {
        self.state().fee_percent = fee;
    }

    pub fn deposit(&self, asset: &str, amount: Decimal) {
        *self.state().balances.entry(asset.into()).or_default() += amount;
    }

    pub fn balance(&self, asset: &str) -> Decimal {
        self.state()
            .balances
            .get(asset)
            .copied()
            .unwrap_or_default()
    }

    pub fn now(&self) -> f64 {
        self.state().now
    }

    /// Moves the clock: the GTD orders past their expiry expire
    pub fn advance(&self, seconds: f64) {
        let mut state = self.state();
        state.now += seconds;
        let now = state.now;
        for order in state.orders.iter_mut() {
            let expired = order.expire_time.is_some_and(|it| it > 0.0 && it <= now);
            if order.status == OrderStatus::Open && expired {
                order.status = OrderStatus::Expired;
                order.close_time = Some(now);
            }
        }
    }

    /// Sets the price of a pair: the open limit orders it reaches are filled at their price
    pub fn set_price(&self, key: &str, price: Decimal) {
        let mut state = self.state();
        state.prices.insert(key.into(), price);
        let now = state.now;
        state
            .history
            .entry(key.into())
            .or_default()
            .push((now, price));
        let altname = state.pairs[key].altname.clone();
        let txids: Vec<String> = state
            .orders
            .iter()
            .filter(|it| it.status == OrderStatus::Open && it.description.pair == altname)
            .filter(|it| {
                let limit = it.description.price.unwrap_or_default();
                match it.description.side {
                    OrderSide::Buy => price <= limit,
                    OrderSide::Sell => price >= limit,
                }
            })
            .map(|it| it.txid.clone())
            .collect();
        drop(state);

        for txid in txids {
            self.fill(&txid, None);
        }
    }

    /// Executes `volume` of the order (all that remains when None) at its limit price
    pub fn fill(&self, txid: &str, volume: Option<Decimal>) {
        let mut state = self.state();
        let fee_rate = state.fee_percent / Decimal::ONE_HUNDRED;
        let now = state.now;
        let index = state
            .orders
            .iter()
            .position(|it| it.txid == txid)
            .expect("the order");
        let (pair, side, limit, remaining) = {
            let order = &state.orders[index];
            (
                order.description.pair.clone(),
                order.description.side,
                order.description.price.unwrap_or_default(),
                order.volume - order.volume_executed,
            )
        };
        let executed = volume.unwrap_or(remaining).min(remaining);
        let cost = executed * limit;
        let fee = (cost * fee_rate)
            .round_dp_with_strategy(8, rust_decimal::RoundingStrategy::ToPositiveInfinity);
        let pair_info = state
            .pairs
            .values()
            .find(|it| it.altname == pair)
            .cloned()
            .expect("the pair");

        match side {
            OrderSide::Buy => {
                *state.balances.entry(pair_info.quote.clone()).or_default() -= cost + fee;
                *state.balances.entry(pair_info.base.clone()).or_default() += executed;
            }
            OrderSide::Sell => {
                *state.balances.entry(pair_info.quote.clone()).or_default() += cost - fee;
                *state.balances.entry(pair_info.base.clone()).or_default() -= executed;
            }
        }

        let order = &mut state.orders[index];
        let total_executed = order.volume_executed + executed;
        let total_cost = order.cost + cost;
        order.volume_executed = total_executed;
        order.cost = total_cost;
        order.fee += fee;
        order.average_price = total_cost / total_executed;
        if total_executed >= order.volume {
            order.status = OrderStatus::Closed;
            order.close_time = Some(now);
        }
    }

    /// Cancels an open order, what was executed stays
    pub fn cancel(&self, txid: &str) {
        let mut state = self.state();
        let now = state.now;
        if let Some(order) = state.orders.iter_mut().find(|it| it.txid == txid) {
            order.status = OrderStatus::Canceled;
            order.close_time = Some(now);
        }
    }

    /// Adds an order placed by someone else (another tool, a manager no longer configured)
    pub fn add_foreign_order(
        &self,
        userref: Option<i64>,
        pair: &str,
        side: OrderSide,
        volume: Decimal,
        price: Decimal,
    ) -> String {
        let mut state = self.state();
        let txid = state.txid();
        let now = state.now;
        state.orders.push(order(
            &txid, userref, pair, side, volume, price, now, None, None,
        ));
        txid
    }

    pub fn orders(&self) -> Vec<Order> {
        self.state().orders.clone()
    }

    pub fn add_order_calls(&self) -> usize {
        self.state().add_order_calls
    }

    fn held(state: &State, asset: &str) -> Decimal {
        let fee_rate = state.fee_percent / Decimal::ONE_HUNDRED;
        state
            .orders
            .iter()
            .filter(|it| it.status == OrderStatus::Open)
            .filter_map(|it| {
                let pair = state
                    .pairs
                    .values()
                    .find(|pair| pair.altname == it.description.pair)?;
                let remaining = it.volume - it.volume_executed;
                let limit = it.description.price.unwrap_or_default();
                match it.description.side {
                    OrderSide::Buy if pair.quote == asset => {
                        Some(remaining * limit * (Decimal::ONE + fee_rate))
                    }
                    OrderSide::Sell if pair.base == asset => Some(remaining),
                    _ => None,
                }
            })
            .sum()
    }
}

impl State {
    fn txid(&mut self) -> String {
        let txid = format!(
            "OSIM{:02}-{:05}-{:06}",
            self.next_txid % 100,
            self.next_txid,
            self.next_txid
        );
        self.next_txid += 1;
        txid
    }
}

#[allow(clippy::too_many_arguments)]
fn order(
    txid: &str,
    userref: Option<i64>,
    pair: &str,
    side: OrderSide,
    volume: Decimal,
    price: Decimal,
    now: f64,
    expire_time: Option<f64>,
    time_in_force: Option<TimeInForce>,
) -> Order {
    let side_name = match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    };
    Order {
        txid: txid.into(),
        refid: None,
        userref,
        cl_ord_id: None,
        status: OrderStatus::Open,
        open_time: now,
        start_time: Some(0.0),
        expire_time: expire_time.or(Some(0.0)),
        close_time: None,
        description: OrderDescription {
            pair: pair.into(),
            aclass: Some("forex".into()),
            side,
            order_type: OrderType::Limit,
            price: Some(price),
            price2: Some(Decimal::ZERO),
            leverage: Some("none".into()),
            order: format!("{side_name} {volume} {pair} @ limit {price}"),
            close: None,
        },
        volume,
        volume_executed: Decimal::ZERO,
        cost: Decimal::ZERO,
        fee: Decimal::ZERO,
        average_price: Decimal::ZERO,
        stop_price: None,
        limit_price: None,
        trigger: None,
        margin: None,
        amended: None,
        sender_sub_id: None,
        time_in_force,
        misc: String::new(),
        oflags: "fciq".into(),
        trades: None,
        reason: None,
    }
}

impl Exchange for SimulatedExchange {
    fn now(&self) -> f64 {
        self.state().now
    }

    async fn asset_pairs(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, AssetPair>> {
        let state = self.state();
        if pairs.is_empty() {
            return Ok(state.pairs.clone());
        }
        let mut found = HashMap::new();
        for name in pairs {
            let (key, pair) = state
                .pairs
                .iter()
                .find(|(key, pair)| *key == name || pair.altname == *name)
                .ok_or_else(|| error("EQuery:Unknown asset pair"))?;
            found.insert(key.clone(), pair.clone());
        }
        Ok(found)
    }

    async fn tickers(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, Ticker>> {
        let keys = self.asset_pairs(pairs).await?;
        let state = self.state();
        Ok(keys
            .into_keys()
            .map(|key| {
                let price = state.prices[&key];
                let window = Window {
                    today: price,
                    last_24h: price,
                };
                let ticker = Ticker {
                    ask: PriceLevel {
                        price,
                        whole_lot_volume: Decimal::ONE,
                        lot_volume: Decimal::ONE,
                    },
                    bid: PriceLevel {
                        price,
                        whole_lot_volume: Decimal::ONE,
                        lot_volume: Decimal::ONE,
                    },
                    last_trade: TradeLevel {
                        price,
                        lot_volume: Decimal::new(1, 1),
                    },
                    volume: Window {
                        today: Decimal::TEN,
                        last_24h: Decimal::from(20),
                    },
                    vwap: window,
                    trades: WindowCount {
                        today: 10,
                        last_24h: 20,
                    },
                    low: window,
                    high: window,
                    open: price,
                };
                (key, ticker)
            })
            .collect())
    }

    /// A trade at `since` at the price of then, then one at each change of the price
    async fn trades(&self, pair: &str, since: f64) -> ExchangeResult<Vec<PublicTrade>> {
        let state = self.state();
        let history = state.history.get(pair).cloned().unwrap_or_default();
        let then = history
            .iter()
            .rev()
            .find(|(time, _)| *time <= since)
            .or(history.first())
            .map(|(_, price)| (since, *price));
        let trade = |(index, (time, price)): (usize, (f64, Decimal))| PublicTrade {
            price,
            volume: Decimal::ONE,
            time,
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            miscellaneous: String::new(),
            trade_id: index as i64,
        };
        Ok(then
            .into_iter()
            .chain(history.into_iter().filter(|(time, _)| *time > since))
            .enumerate()
            .map(trade)
            .collect())
    }

    async fn trade_volume(&self, pairs: &[String]) -> ExchangeResult<TradeVolume> {
        let keys = self.asset_pairs(pairs).await?;
        let fee = self.state().fee_percent;
        let info = FeeInfo {
            fee,
            minfee: Some(Decimal::ZERO),
            maxfee: Some(fee),
            nextfee: None,
            nextvolume: None,
            tiervolume: Some(Decimal::ZERO),
            nextfuturesvolume: None,
        };
        let fees: HashMap<String, FeeInfo> =
            keys.into_keys().map(|key| (key, info.clone())).collect();
        Ok(TradeVolume {
            currency: "ZUSD".into(),
            asset_class: Some("currency".into()),
            volume: Decimal::ZERO,
            inputs: None,
            fees_maker: fees.clone(),
            fees,
        })
    }

    async fn balances(&self) -> ExchangeResult<HashMap<String, ExtendedBalance>> {
        let state = self.state();
        Ok(state
            .balances
            .iter()
            .map(|(asset, balance)| {
                let held = Self::held(&state, asset);
                let extended = ExtendedBalance {
                    balance: *balance,
                    hold_trade: Some(held),
                    credit: None,
                    credit_used: None,
                };
                (asset.clone(), extended)
            })
            .collect())
    }

    async fn open_orders(&self) -> ExchangeResult<Vec<Order>> {
        let state = self.state();
        Ok(state
            .orders
            .iter()
            .filter(|it| !it.status.is_final())
            .cloned()
            .collect())
    }

    async fn closed_orders(&self, request: &ClosedOrdersRequest) -> ExchangeResult<ClosedOrders> {
        let state = self.state();
        let mut matching: Vec<Order> = state
            .orders
            .iter()
            .filter(|it| it.status.is_final())
            .filter(|it| {
                request
                    .userref
                    .is_none_or(|userref| it.userref == Some(userref as i64))
            })
            .filter(|it| {
                let time = match request.close_time {
                    Some(CloseTime::Open) => it.open_time,
                    _ => it.close_time.unwrap_or(it.open_time),
                };
                request.start.is_none_or(|start| time > start as f64)
                    && request.end.is_none_or(|end| time <= end as f64)
            })
            .cloned()
            .collect();
        matching.sort_by(|a, b| b.open_time.total_cmp(&a.open_time));
        let count = matching.len() as i64;
        let offset = request.offset.unwrap_or_default().max(0) as usize;
        Ok(ClosedOrders {
            orders: matching.into_iter().skip(offset).take(50).collect(),
            count,
        })
    }

    async fn query_orders(&self, txids: &[String]) -> ExchangeResult<Vec<Order>> {
        let state = self.state();
        Ok(state
            .orders
            .iter()
            .filter(|it| txids.contains(&it.txid))
            .cloned()
            .collect())
    }

    async fn cancel_order(&self, txid: &str) -> ExchangeResult<()> {
        let open = self
            .state()
            .orders
            .iter()
            .any(|it| it.txid == txid && it.status == OrderStatus::Open);
        if !open {
            return Err(error("EOrder:Unknown order"));
        }
        self.cancel(txid);
        Ok(())
    }

    async fn new_order(&self, request: &NewOrder) -> ExchangeResult<NewOrderAnswer> {
        let mut state = self.state();
        state.add_order_calls += 1;

        let pair = state
            .pairs
            .values()
            .find(|it| it.altname == request.pair)
            .cloned()
            .ok_or_else(|| error("EQuery:Unknown asset pair"))?;
        if request.order_type != OrderType::Limit {
            return Err(error(
                "EGeneral:Invalid arguments:the simulation only knows limit orders",
            ));
        }
        let volume = Decimal::from_str(&request.volume)
            .map_err(|_| error("EGeneral:Invalid arguments:volume"))?;
        let price = request
            .price
            .as_deref()
            .and_then(|it| Decimal::from_str(it).ok())
            .ok_or_else(|| error("EGeneral:Invalid arguments:price"))?;
        if volume < pair.ordermin.unwrap_or_default() {
            return Err(error("EGeneral:Invalid arguments:volume minimum not met"));
        }
        if volume * price < pair.costmin.unwrap_or_default() {
            return Err(error("EOrder:Cost minimum not met"));
        }

        let fee_rate = state.fee_percent / Decimal::ONE_HUNDRED;
        let (asset, needed) = match request.side {
            OrderSide::Buy => (
                pair.quote.clone(),
                volume * price * (Decimal::ONE + fee_rate),
            ),
            OrderSide::Sell => (pair.base.clone(), volume),
        };
        let available =
            state.balances.get(&asset).copied().unwrap_or_default() - Self::held(&state, &asset);
        if needed > available {
            return Err(error("EOrder:Insufficient funds"));
        }

        let side_name = match request.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let description = NewOrderDescription {
            order: format!("{side_name} {volume} {} @ limit {price}", request.pair),
            close: None,
        };
        if request.validate == Some(true) {
            return Ok(NewOrderAnswer {
                description,
                txids: vec![],
            });
        }

        let now = state.now;
        let expire_time = match (request.time_in_force, request.expire_time.as_deref()) {
            (Some(TimeInForce::Gtd), Some(relative)) if relative.starts_with('+') => relative[1..]
                .parse::<f64>()
                .ok()
                .map(|seconds| now + seconds),
            (Some(TimeInForce::Gtd), Some(absolute)) => absolute.parse::<f64>().ok(),
            _ => None,
        };
        let txid = state.txid();
        state.orders.push(order(
            &txid,
            request.userref.map(i64::from),
            &request.pair,
            request.side,
            volume,
            price,
            now,
            expire_time,
            request.time_in_force,
        ));
        Ok(NewOrderAnswer {
            description,
            txids: vec![txid],
        })
    }
}
