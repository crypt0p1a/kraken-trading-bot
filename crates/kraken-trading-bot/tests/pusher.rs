//! The pusher against a web backend of the tests, on a port of its own: what it pushes, when, the
//! backend away

use kraken_dashboard_data::Range;
use kraken_dashboard_data::wire::{Push, gunzip};
use kraken_trading_bot::pusher::Pusher;
use kraken_trading_bot::storage::{FORCED_TICK, Storage};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The key the bot and the backend of the tests share: never written whole, which the scanners of
/// secrets would take for one
fn key() -> String {
    "0123456789abcdef".repeat(2)
}

/// The signature of a push, as kraken-dashboard-web checks it: HMAC-SHA256 of the key over the time
/// it was signed and the body as sent
fn signed_by_the_bot(headers: &axum::http::HeaderMap, body: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    let (Some(at), Some(signature)) = (
        headers
            .get("x-kraken-push-at")
            .and_then(|it| it.to_str().ok()),
        headers
            .get("x-kraken-push-signature")
            .and_then(|it| it.to_str().ok())
            .and_then(|it| it.strip_prefix("sha256="))
            .and_then(|it| hex::decode(it).ok()),
    ) else {
        return false;
    };
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key().as_bytes()).unwrap();
    mac.update(format!("{at}\n").as_bytes());
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

/// What the backend of the tests received, in order
#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<Push>>>);

impl Received {
    fn pushes(&self) -> Vec<Push> {
        self.0.lock().unwrap().clone()
    }
}

/// A backend taking `PUT /state` as kraken-dashboard-web does, in a runtime of its own: the pusher
/// blocks, it's never in a runtime
fn backend() -> (String, Received) {
    use axum::body::Bytes;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::put;

    let received = Received::default();
    let stored = received.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let router = axum::Router::new().route(
                    "/state",
                    put(move |headers: HeaderMap, body: Bytes| {
                        let stored = stored.clone();
                        async move {
                            // compressed and signed, always
                            assert_eq!(headers["content-encoding"], "gzip");
                            if !signed_by_the_bot(&headers, &body) {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    "a wrong signature: not the key of the backend",
                                )
                                    .into_response();
                            }
                            let json = gunzip(&body, 1 << 30).unwrap();
                            stored
                                .0
                                .lock()
                                .unwrap()
                                .push(serde_json::from_slice(&json).unwrap());
                            StatusCode::NO_CONTENT.into_response()
                        }
                    }),
                );
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                sender.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, router).await.unwrap();
            });
    });
    (format!("http://{}", receiver.recv().unwrap()), received)
}

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "kraken-bot-pusher-{name}-{}-{}.sqlite",
            std::process::id(),
            kraken_trading_bot::clock::now_millis()
        )))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
    }
}

#[test]
fn pushes_every_screen_of_the_database() {
    let (url, received) = backend();
    let fixture = Fixture::new("screens");
    let storage = Storage::open(&fixture.0).unwrap();
    // a trailing / of the URL is the same backend
    let mut pusher = Pusher::new(&format!("{url}/"), &key(), &fixture.0).unwrap();

    // wanted at once: the bot started
    assert!(pusher.wanted);
    pusher.step();
    assert!(!pusher.wanted);
    let pushes = received.pushes();
    assert_eq!(pushes.len(), 1);
    // a single account: all of them added up, every range
    let screens: Vec<_> = pushes[0]
        .views
        .iter()
        .map(|it| (it.account, it.range))
        .collect();
    assert_eq!(screens, Range::ALL.map(|it| (None, it)));
    assert_eq!(pushes[0].views[0].data.forced_tick.as_deref(), Some("idle"));
    assert!(pushes[0].at > 0);

    // nothing new: nothing pushed
    pusher.step();
    assert_eq!(received.pushes().len(), 1);

    // a tick: pushed as the database is now
    storage
        .request_command(FORCED_TICK, kraken_trading_bot::clock::now_millis())
        .unwrap();
    pusher.wanted = true;
    pusher.step();
    let pushes = received.pushes();
    assert_eq!(pushes.len(), 2);
    assert_eq!(
        pushes[1].views[0].data.forced_tick.as_deref(),
        Some("refresh")
    );
}

#[test]
fn pushes_again_once_the_backend_is_back() {
    let fixture = Fixture::new("away");
    let _storage = Storage::open(&fixture.0).unwrap();
    // nobody listens there
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut pusher = Pusher::new(&format!("http://127.0.0.1:{port}"), &key(), &fixture.0).unwrap();
    pusher.step();
    // the last state is pushed at the next try
    assert!(pusher.wanted);
}

#[test]
fn waits_for_the_database_of_the_bot() {
    let (url, received) = backend();
    let fixture = Fixture::new("missing");
    let mut pusher = Pusher::new(&url, &key(), &fixture.0).unwrap();
    pusher.step();
    assert!(pusher.wanted);
    assert!(received.pushes().is_empty());
    // the bot created it meanwhile
    let _storage = Storage::open(&fixture.0).unwrap();
    pusher.step();
    assert_eq!(received.pushes().len(), 1);
}

#[test]
fn refuses_what_isnt_an_http_url() {
    let fixture = Fixture::new("url");
    let error = Pusher::new("dashboard-web:8080", &key(), &fixture.0)
        .err()
        .unwrap();
    assert!(error.contains("KRAKEN_BOT_PUSH_URL"), "{error}");
}

/// A push not signed with the key of the backend is refused: told, and pushed again
#[test]
fn signs_its_pushes_with_its_key() {
    let (url, received) = backend();
    let fixture = Fixture::new("key");
    let _storage = Storage::open(&fixture.0).unwrap();
    let mut pusher =
        Pusher::new(&url, "another key, which isn't the backend's", &fixture.0).unwrap();
    pusher.step();
    assert!(pusher.wanted);
    assert!(received.pushes().is_empty());
    // told why, as the backend tells it
    let failing = pusher.failing().unwrap();
    assert!(
        failing.contains("401") && failing.contains("not the key of the backend"),
        "{failing}"
    );
}
