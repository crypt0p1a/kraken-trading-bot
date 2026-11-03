//! The bot on the real Kraken client, against a mocked Kraken: the requests it sends

use kraken_api::RestOptions;
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::KrakenExchange;
use kraken_trading_bot::storage::Storage;
use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ok(result: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "error": [], "result": result }))
}

fn form(request: &Request) -> Vec<(String, String)> {
    let body = std::str::from_utf8(&request.body).unwrap();
    body.split('&')
        .filter_map(|it| it.split_once('='))
        .map(|(key, value)| {
            (
                key.to_string(),
                value.replace("%2C", ",").replace("%2B", "+"),
            )
        })
        .collect()
}

async fn kraken() -> MockServer {
    let server = MockServer::start().await;
    let routes = [
        (
            "/0/public/AssetPairs",
            json!({"XXBTZUSD": {"altname": "XBTUSD", "wsname": "XBT/USD", "base": "XXBT", "quote": "ZUSD",
                "pair_decimals": 1, "lot_decimals": 8, "ordermin": "0.00006", "costmin": "0.5",
                "tick_size": "0.1", "status": "online"}}),
        ),
        (
            "/0/public/Ticker",
            json!({"XXBTZUSD": {"a": ["50010.0", "1", "1.000"], "b": ["49990.0", "1", "1.000"],
                "c": ["50000.0", "0.001"], "v": ["10", "20"], "p": ["50000", "50000"], "t": [10, 20],
                "l": ["49000", "49000"], "h": ["51000", "51000"], "o": "50000"}}),
        ),
        (
            "/0/private/TradeVolume",
            json!({"currency": "ZUSD", "volume": "0", "fees": {"XXBTZUSD": {"fee": "0.4000"}},
                "fees_maker": {"XXBTZUSD": {"fee": "0.2500"}}}),
        ),
        (
            "/0/private/BalanceEx",
            json!({"ZUSD": {"balance": "500", "hold_trade": "0"}}),
        ),
        (
            "/0/public/Trades",
            json!({"XXBTZUSD": [["50000.0", "0.1", 1800000001.5, "b", "l", "", 7]],
                "last": "1800000001500000000"}),
        ),
        (
            "/0/public/OHLC",
            json!({"XXBTZUSD": [[1800000000, "50000.0", "50100.0", "49900.0", "50050.0", "50010.0", "1.5", 12]],
                "last": 1800000000}),
        ),
        ("/0/private/OpenOrders", json!({"open": {}})),
        ("/0/private/CancelOrder", json!({"count": 1})),
        ("/0/private/QueryOrders", json!({})),
        ("/0/private/ClosedOrders", json!({"closed": {}, "count": 0})),
        (
            "/0/private/AddOrder",
            json!({"descr": {"order": "buy 0.00198 XBTUSD @ limit 49500.0"}}),
        ),
    ];
    for (route, result) in routes {
        Mock::given(path(route))
            .respond_with(ok(result))
            .mount(&server)
            .await;
    }
    server
}

#[tokio::test]
async fn a_tick_sends_the_expected_requests_to_kraken() {
    let server = kraken().await;
    let exchange = KrakenExchange::new(
        "key".into(),
        "c2VjcmV0".into(),
        RestOptions {
            host: server.uri(),
            ..RestOptions::default()
        },
    )
    .unwrap();
    let config = BotConfig::parse(
        "dry_run: true
buy_order_timeout: 12h
managers:
  - {id: 42, pair: XBT/USD, budget: 100, buy_below: 1, sell_above: 1.5}
  - {id: 43, pair: XBTUSD, budget: 100, buy_below: 2, sell_above: 3}",
    )
    .unwrap();
    let mut bot = Bot::new(exchange, Storage::open_in_memory().unwrap(), config);
    bot.prepare().await.unwrap();

    let report = bot.tick().await.unwrap();
    assert!(
        matches!(report.outcomes[&42], Outcome::Placed { txid: None, .. }),
        "{report:?}"
    );

    let requests = server.received_requests().await.unwrap();
    let add_order = requests
        .iter()
        .find(|it| it.url.path() == "/0/private/AddOrder")
        .expect("AddOrder");
    assert_eq!(add_order.method.as_str(), "POST");
    let mut params: Vec<(String, String)> = form(add_order)
        .into_iter()
        .filter(|(key, _)| key != "nonce")
        .collect();
    params.sort();
    // 100 / (49500 * 1.004) = 0.00201215..., the taker fee of 0.4%
    assert_eq!(
        params,
        [
            ("expiretm", "+43200"),
            ("oflags", "fciq"),
            ("ordertype", "limit"),
            ("pair", "XBTUSD"),
            ("price", "49500"),
            ("timeinforce", "GTD"),
            ("type", "buy"),
            ("userref", "42"),
            ("validate", "true"),
            ("volume", "0.00201215"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()))
    );

    // every order sent is a limit order at a price, never a market order
    for request in requests
        .iter()
        .filter(|it| it.url.path() == "/0/private/AddOrder")
    {
        let params = form(request);
        assert!(
            params.contains(&("ordertype".into(), "limit".into())),
            "{params:?}"
        );
        assert!(params.iter().any(|(key, value)| key == "price" && value.parse::<f64>().is_ok_and(|it| it > 0.0)));
    }

    // the history of the manager was asked once, by its userref
    let history: Vec<_> = requests
        .iter()
        .filter(|it| it.url.path() == "/0/private/ClosedOrders")
        .map(form)
        .collect();
    assert!(
        history
            .iter()
            .any(|params| params.contains(&("userref".into(), "42".into())))
    );

    // a second tick doesn't ask the history again
    bot.tick().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let with_userref = requests
        .iter()
        .filter(|it| it.url.path() == "/0/private/ClosedOrders")
        .filter(|it| form(it).contains(&("userref".into(), "42".into())))
        .count();
    assert_eq!(with_userref, 1);

    // every pair is asked once, to find the ones of the managers: then only theirs, even when
    // managers share a pair
    let all_pairs = requests
        .iter()
        .filter(|it| it.url.path() == "/0/public/AssetPairs")
        .filter(|it| !it.url.query().unwrap_or_default().contains("pair="))
        .count();
    assert_eq!(all_pairs, 1);

    // an order the database believes open, found nowhere else: asked to Kraken by its txid
    bot.storage()
        .upsert_orders(&[kraken_trading_bot::order::BotOrder {
            txid: "OAAAAA-BBBBB-CCCCCC".into(),
            manager_id: 42,
            pair: "XBTUSD".into(),
            side: kraken_api::trading::OrderSide::Buy,
            status: kraken_api::trading::OrderStatus::Open,
            volume: "0.001".parse().unwrap(),
            volume_executed: Default::default(),
            cost: Default::default(),
            fee: Default::default(),
            limit_price: Some("49500".parse().unwrap()),
            average_price: None,
            open_time: 1.0,
            close_time: None,
            expire_time: None,
        }])
        .unwrap();
    bot.tick().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let query = requests
        .iter()
        .find(|it| it.url.path() == "/0/private/QueryOrders")
        .expect("QueryOrders");
    assert!(form(query).contains(&("txid".into(), "OAAAAA-BBBBB-CCCCCC".into())));
}

fn exchange(server: &MockServer) -> KrakenExchange {
    KrakenExchange::new(
        "key".into(),
        "c2VjcmV0".into(),
        RestOptions {
            host: server.uri(),
            ..RestOptions::default()
        },
    )
    .unwrap()
}

#[tokio::test]
async fn reads_the_trades_from_a_time() {
    use kraken_trading_bot::exchange::Exchange;
    let server = kraken().await;
    let trades = exchange(&server)
        .trades("XXBTZUSD", 1_800_000_000.7)
        .await
        .unwrap();
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, "50000.0".parse().unwrap());

    let requests = server.received_requests().await.unwrap();
    let query = requests[0].url.query().unwrap_or_default().to_string();
    assert_eq!(requests[0].url.path(), "/0/public/Trades");
    assert!(query.contains("pair=XXBTZUSD"), "{query}");
    assert!(query.contains("since=1800000000"), "{query}");
}

#[tokio::test]
async fn searches_the_whole_history_for_virtual_managers_once() {
    let server = kraken().await;
    let config = BotConfig::parse(
        "dry_run: true
virtual_managers: {min_virtual_id: 1000000}
managers:
  - {id: 42, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}",
    )
    .unwrap();
    let mut bot = Bot::new(
        exchange(&server),
        Storage::open_in_memory().unwrap(),
        config,
    );
    bot.prepare().await.unwrap();
    bot.tick().await.unwrap();
    bot.tick().await.unwrap();

    // neither a userref nor a start: every closed order of the account
    let requests = server.received_requests().await.unwrap();
    let whole = requests
        .iter()
        .filter(|it| it.url.path() == "/0/private/ClosedOrders")
        .map(form)
        .filter(|params| {
            !params
                .iter()
                .any(|(key, _)| key == "userref" || key == "start")
        })
        .count();
    assert_eq!(whole, 1);
}

#[tokio::test]
async fn cancels_an_order_by_its_txid() {
    use kraken_trading_bot::exchange::Exchange;
    let server = kraken().await;
    assert!(
        exchange(&server)
            .cancel_order("OAAAAA-BBBBB-CCCCCC")
            .await
            .unwrap()
    );

    let requests = server.received_requests().await.unwrap();
    let cancel = requests
        .iter()
        .find(|it| it.url.path() == "/0/private/CancelOrder")
        .expect("CancelOrder");
    assert_eq!(cancel.method.as_str(), "POST");
    assert!(form(cancel).contains(&("txid".into(), "OAAAAA-BBBBB-CCCCCC".into())));
}

#[tokio::test]
async fn reads_the_candles_of_fifteen_minutes() {
    use kraken_api::market::CandleInterval;
    use kraken_trading_bot::exchange::Exchange;
    let server = kraken().await;
    let candles = exchange(&server)
        .candles("XXBTZUSD", CandleInterval::Res15m, 1_799_910_000)
        .await
        .unwrap();
    assert_eq!(candles.len(), 1);
    assert_eq!(candles[0].close, "50050.0".parse().unwrap());

    let requests = server.received_requests().await.unwrap();
    let query = requests[0].url.query().unwrap_or_default().to_string();
    assert_eq!(requests[0].url.path(), "/0/public/OHLC");
    for expected in ["pair=XXBTZUSD", "interval=15", "since=1799910000"] {
        assert!(query.contains(expected), "{query}");
    }
}
