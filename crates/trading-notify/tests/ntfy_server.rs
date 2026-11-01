//! Sending to a mocked ntfy server, with the `ntfy` crate

use ntfy::Priority;
use trading_notify::{EventKind, Notification, Notifier};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn notification() -> Notification {
    Notification {
        event: EventKind::Filled,
        title: "Sell filled · #1 XBTUSD".into(),
        message: "received 100.75 USD after fees\ngain +1.5 USD (+1.51 %)".into(),
        tags: vec!["moneybag"],
        priority: Priority::High,
    }
}

/// What ntfy answers to a publication
fn published() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "x1", "event": "message"}))
}

#[tokio::test]
async fn publishes_on_the_topic_with_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer the-token"))
        .respond_with(published())
        .expect(1)
        .mount(&server)
        .await;

    let notifier = Notifier::ntfy(&server.uri(), "kraken-bot", Some("the-token")).unwrap();
    notifier.send(notification()).await;

    let body: serde_json::Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["topic"], "kraken-bot");
    assert_eq!(body["title"], "Sell filled · #1 XBTUSD");
    assert_eq!(
        body["message"],
        "received 100.75 USD after fees\ngain +1.5 USD (+1.51 %)"
    );
    assert_eq!(body["tags"], serde_json::json!(["moneybag"]));
    assert_eq!(body["priority"], 4);
}

#[tokio::test]
async fn sends_no_token_when_there_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(published())
        .mount(&server)
        .await;

    Notifier::ntfy(&server.uri(), "topic", None)
        .unwrap()
        .send(notification())
        .await;
    assert!(
        !server.received_requests().await.unwrap()[0]
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn a_failure_of_ntfy_doesnt_stop_the_caller() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("daily quota reached"))
        .expect(1)
        .mount(&server)
        .await;

    // logged, not returned, not panicking
    Notifier::ntfy(&server.uri(), "topic", None)
        .unwrap()
        .send(notification())
        .await;
    Notifier::ntfy("http://127.0.0.1:1", "topic", None)
        .unwrap()
        .send(notification())
        .await;
}
