//! The bot on the real Kraken client, against a mocked Kraken: the requests it sends

use kraken_spot_api::RestOptions;
use kraken_trading_bot::bot::{Bot, Outcome};
use kraken_trading_bot::config::BotConfig;
use kraken_trading_bot::exchange::KrakenExchange;
use kraken_trading_bot::storage::Storage;
use wiremock::MockServer;

mod common;

use common::{form, kraken};

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
            side: kraken_spot_api::trading::OrderSide::Buy,
            status: kraken_spot_api::trading::OrderStatus::Open,
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
virtual_managers: true
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
    use kraken_spot_api::market::CandleInterval;
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

#[tokio::test]
async fn reads_the_permissions_of_the_key() {
    use kraken_trading_bot::exchange::Exchange;
    let server = kraken().await;
    let permissions = exchange(&server).key_permissions().await.unwrap();
    assert_eq!(permissions, common::PERMISSIONS);
    let requests = server.received_requests().await.unwrap();
    let info = requests
        .iter()
        .find(|it| it.url.path() == "/0/private/GetApiKeyInfo")
        .expect("GetApiKeyInfo");
    // a private call: signed, with its nonce
    assert_eq!(info.method.as_str(), "POST");
    assert!(info.headers.contains_key("API-Sign"));
}
