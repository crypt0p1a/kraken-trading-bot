//! What the bot needs from the exchange: [`KrakenExchange`] calls Kraken, [`SimulatedExchange`]
//! simulates it for the tests and the rehearsals

mod kraken;
mod simulated;

pub use kraken::KrakenExchange;
pub use simulated::SimulatedExchange;

use kraken_api::KrakenError;
use kraken_api::account::{ExtendedBalance, TradeVolume};
use kraken_api::market::{AssetPair, PublicTrade, Ticker};
use kraken_api::trading::{ClosedOrders, ClosedOrdersRequest, NewOrder, NewOrderAnswer, Order};
use std::collections::HashMap;

pub type ExchangeResult<T> = Result<T, KrakenError>;

#[allow(async_fn_in_trait)]
pub trait Exchange {
    /// UTC seconds: the clock of the exchange
    fn now(&self) -> f64 {
        crate::clock::now_seconds()
    }

    /// The pairs, keyed by Kraken name; all of them when `pairs` is empty
    async fn asset_pairs(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, AssetPair>>;

    /// The tickers, keyed by Kraken name
    async fn tickers(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, Ticker>>;

    /// Trades of the pair from `since` (UTC seconds), oldest first: a page of them, the first ones
    async fn trades(&self, pair: &str, since: f64) -> ExchangeResult<Vec<PublicTrade>>;

    /// The fees of the pairs, keyed by Kraken name
    async fn trade_volume(&self, pairs: &[String]) -> ExchangeResult<TradeVolume>;

    /// The balances of the account, keyed by asset
    async fn balances(&self) -> ExchangeResult<HashMap<String, ExtendedBalance>>;

    /// Every open order of the account
    async fn open_orders(&self) -> ExchangeResult<Vec<Order>>;

    /// A page of closed orders
    async fn closed_orders(&self, request: &ClosedOrdersRequest) -> ExchangeResult<ClosedOrders>;

    /// The orders of these txids
    async fn query_orders(&self, txids: &[String]) -> ExchangeResult<Vec<Order>>;

    async fn new_order(&self, order: &NewOrder) -> ExchangeResult<NewOrderAnswer>;

    /// Cancels the open order of this txid
    async fn cancel_order(&self, txid: &str) -> ExchangeResult<()>;
}
