use super::{Exchange, ExchangeResult};
use kraken_api::account::{ExtendedBalance, TradeVolume};
use kraken_api::market::{
    AssetPair, Candle, CandleInterval, CandlesRequest, PublicTrade, RecentTradesRequest, Ticker,
};
use kraken_api::trading::{
    ClosedOrders, ClosedOrdersRequest, NewOrder, NewOrderAnswer, OpenOrdersRequest, Order,
    OrderReference, QueryOrdersRequest,
};
use kraken_api::{PrivateApi, PublicApi, RateLimiter, RestOptions};
use std::collections::HashMap;
use std::sync::Arc;

/// QueryOrders accepts at most 50 txids
const QUERY_BATCH: usize = 50;

/// The real Kraken
pub struct KrakenExchange {
    public: PublicApi,
    private: PrivateApi,
}

impl KrakenExchange {
    pub fn new(api_key: String, api_secret: String, options: RestOptions) -> ExchangeResult<Self> {
        // shared: Kraken limits the calls per IP and per key
        let limiter = Arc::new(RateLimiter::default());
        Ok(Self {
            public: PublicApi::new(options.clone(), limiter.clone())?,
            private: PrivateApi::new(api_key, api_secret, options, limiter)?,
        })
    }
}

impl Exchange for KrakenExchange {
    async fn asset_pairs(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, AssetPair>> {
        self.public.asset_pairs(pairs).await
    }

    async fn tickers(&self, pairs: &[String]) -> ExchangeResult<HashMap<String, Ticker>> {
        self.public.tickers(pairs).await
    }

    async fn trades(&self, pair: &str, since: f64) -> ExchangeResult<Vec<PublicTrade>> {
        let request = RecentTradesRequest {
            pair: pair.to_string(),
            since: Some((since.floor() as i64).to_string()),
            count: None,
            asset_class: None,
        };
        Ok(self.public.recent_trades(&request).await?.trades)
    }

    async fn candles(
        &self,
        pair: &str,
        interval: CandleInterval,
        since: i64,
    ) -> ExchangeResult<Vec<Candle>> {
        let request = CandlesRequest {
            pair: pair.to_string(),
            interval,
            since: Some(since),
        };
        Ok(self.public.candles(&request).await?.candles)
    }

    async fn trade_volume(&self, pairs: &[String]) -> ExchangeResult<TradeVolume> {
        self.private.trade_volume(pairs).await
    }

    async fn balances(&self) -> ExchangeResult<HashMap<String, ExtendedBalance>> {
        self.private.extended_balance().await
    }

    async fn open_orders(&self) -> ExchangeResult<Vec<Order>> {
        self.private
            .open_orders(&OpenOrdersRequest::default())
            .await
    }

    async fn closed_orders(&self, request: &ClosedOrdersRequest) -> ExchangeResult<ClosedOrders> {
        self.private.closed_orders(request).await
    }

    async fn query_orders(&self, txids: &[String]) -> ExchangeResult<Vec<Order>> {
        let mut orders = Vec::new();
        for batch in txids.chunks(QUERY_BATCH) {
            let request = QueryOrdersRequest {
                txids: batch.to_vec(),
                trades: None,
                userref: None,
            };
            orders.extend(self.private.query_orders(&request).await?);
        }
        Ok(orders)
    }

    async fn new_order(&self, order: &NewOrder) -> ExchangeResult<NewOrderAnswer> {
        self.private.new_order(order).await
    }

    async fn cancel_order(&self, txid: &str) -> ExchangeResult<()> {
        let order = OrderReference::Txid {
            txid: txid.to_string(),
        };
        self.private.cancel_order(&order).await.map(|_| ())
    }
}
