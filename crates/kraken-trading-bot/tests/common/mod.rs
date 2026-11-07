//! A mocked Kraken answering every call of a tick, for the tests of the client and of the binary
#![allow(dead_code)]

use serde_json::{Value, json};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// What the bot needs of a key, the permissions GetApiKeyInfo answers by default
pub const PERMISSIONS: [&str; 4] = [
    "query-funds",
    "query-open-trades",
    "query-closed-trades",
    "modify-trades",
];

pub fn ok(result: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "error": [], "result": result }))
}

pub fn form(request: &Request) -> Vec<(String, String)> {
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

pub async fn kraken() -> MockServer {
    kraken_with(&PERMISSIONS).await
}

/// A mocked Kraken whose key has these permissions
pub async fn kraken_with(permissions: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path("/0/private/GetApiKeyInfo"))
        .respond_with(ok(
            json!({"api_key_name": "bot", "api_key": "key", "nonce": "1",
            "permissions": permissions}),
        ))
        .mount(&server)
        .await;
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
