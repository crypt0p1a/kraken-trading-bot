//! What the bot pushes to its web backend, KRAKEN_BOT_PUSH_URL
//! ([kraken-dashboard-web](https://github.com/crypt0p1a/kraken-dashboard-web)): every screen of
//! the dashboard read from its database (the [`wire`](kraken_dashboard_data::wire) of
//! kraken-dashboard-data), as soon as the bot starts, at the end of each tick, and when a forced
//! tick starts. Read only: nothing comes back from the backend.
//!
//! Each push is signed with KRAKEN_BOT_PUSH_KEY, which the backend knows too, never sent: the
//! backend takes the pushes of the bot only ([`sign`]).
//!
//! On a thread of its own, with a connection of its own: the backend away or slow, the ticks go on,
//! never waiting for it. A push which failed is tried again, the last state only.

use crate::clock::now_millis;
use kraken_dashboard_data::Database;
use kraken_dashboard_data::wire::{self, Push};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// When the bot signed a push, UTC milliseconds
pub const SIGNED_AT: &str = "x-kraken-push-at";
/// The signature of a push: `sha256=` and the HMAC-SHA256 in hex
pub const SIGNATURE: &str = "x-kraken-push-signature";

/// The signature of a push signed at `at`: HMAC-SHA256 of the key over `"<at>\n"` and the body as
/// sent, compressed. kraken-dashboard-web checks it the same way
pub fn sign(key: &[u8], at: i64, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    // HMAC takes a key of any length
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("a key of any length");
    mac.update(format!("{at}\n").as_bytes());
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A push which failed is tried again this often, until it works
pub const RETRY: Duration = Duration::from_secs(30);

/// A push, at most: the last one, when the bot stops, fits in what it waits for
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Tells the pusher a tick ended, or a forced tick started: pushed again
#[derive(Clone)]
pub struct Ticked(mpsc::Sender<()>);

impl Ticked {
    /// Never waits; nothing when the pusher stopped
    pub fn notify(&self) {
        let _ = self.0.send(());
    }
}

/// The thread of the pusher: waited for when the bot stops, for the push of its last tick
pub struct Running(std::thread::JoinHandle<()>);

impl Running {
    /// Once every [`Ticked`] is dropped: the last push done, `limit` at most
    pub fn wait(self, limit: Duration) {
        let start = std::time::Instant::now();
        while !self.0.is_finished() {
            if start.elapsed() >= limit {
                tracing::warn!("the last push to the web backend didn't end in time");
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.join();
    }
}

/// Pushes to the backend at `url` from the database at `path`, on a thread of its own, at once and
/// then at each [`Ticked`], until every one is dropped: then what's left to push is pushed, and the
/// thread ends
pub fn spawn(url: &str, key: &str, path: &Path) -> Result<(Ticked, Running), String> {
    check(url)?;
    let (url, key, path) = (url.to_string(), key.to_string(), path.to_path_buf());
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("pusher".into())
        .spawn(move || {
            // its client blocks: made out of the runtime of the ticks, which it would stop
            let mut pusher = match Pusher::new(&url, &key, &path) {
                Ok(pusher) => pusher,
                Err(error) => {
                    tracing::error!(%error, "nothing is pushed to the web backend");
                    return;
                }
            };
            // the last state of the database, at once: the backend may hold what a previous run
            // of the bot pushed
            pusher.step();
            loop {
                // waiting for a tick; tried again meanwhile when the last push failed
                let next = if pusher.wanted {
                    receiver.recv_timeout(RETRY)
                } else {
                    receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
                };
                match next {
                    Ok(()) => {
                        // the ticks of several accounts ended meanwhile: a single push
                        while receiver.try_recv().is_ok() {}
                        pusher.wanted = true;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    // the bot stops: the push of its last tick, if it's still to do
                    Err(RecvTimeoutError::Disconnected) => {
                        if pusher.wanted {
                            pusher.step();
                        }
                        return;
                    }
                }
                pusher.step();
            }
        })
        .map_err(|error| format!("the pusher couldn't start: {error}"))?;
    Ok((Ticked(sender), Running(thread)))
}

/// An http or https URL without a query nor a fragment, `/state` going after its path: refused at
/// the start rather than at each push
fn check(url: &str) -> Result<(), String> {
    match reqwest::Url::parse(url) {
        Ok(parsed)
            if matches!(parsed.scheme(), "http" | "https")
                && parsed.has_host()
                && parsed.query().is_none()
                && parsed.fragment().is_none() =>
        {
            Ok(())
        }
        _ => Err(format!(
            "KRAKEN_BOT_PUSH_URL: \"{}\" isn't an http(s) URL without a query, e.g. http://dashboard-web:8080",
            redacted(url)
        )),
    }
}

/// The URL as it can be logged: without the user and the password it may hold, nor its query
pub fn redacted(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => "(not a URL)".to_string(),
    }
}

pub struct Pusher {
    /// Where the pushes go: the URL of the backend and its route
    state: String,
    /// As it's logged
    shown: String,
    /// What the pushes are signed with
    key: Vec<u8>,
    client: reqwest::blocking::Client,
    path: PathBuf,
    /// Opened at the first push: the bot creates the database before
    database: Option<Database>,
    /// A push to do: the bot started, a tick ended, the last push failed
    pub wanted: bool,
    /// What failed last, told once until it works again
    failing: Option<String>,
}

impl Pusher {
    /// Not inside a tokio runtime: its client blocks. A push wanted at once
    pub fn new(url: &str, key: &str, path: &Path) -> Result<Self, String> {
        check(url)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| format!("KRAKEN_BOT_PUSH_URL: {error}"))?;
        let state = format!("{}{}", url.trim_end_matches('/'), wire::STATE);
        Ok(Self {
            shown: redacted(&state),
            state,
            key: key.as_bytes().to_vec(),
            client,
            path: path.to_path_buf(),
            database: None,
            wanted: true,
            failing: None,
        })
    }

    /// What failed last, as it's logged: None once it works again
    pub fn failing(&self) -> Option<&str> {
        self.failing.as_deref()
    }

    /// Pushes when there's something new to push, told once when it fails
    pub fn step(&mut self) {
        if !self.wanted {
            return;
        }
        let result = self.push();
        match (&result, &self.failing) {
            (Ok(()), Some(_)) => {
                tracing::info!(url = %self.shown, "the web backend is reached again");
                self.failing = None;
            }
            (Err(error), failing) if failing.as_ref() != Some(error) => {
                tracing::warn!(url = %self.shown, %error, "nothing pushed to the web backend, the ticks go on");
                self.failing = Some(error.clone());
            }
            _ => {}
        }
    }

    /// Every screen, as the database is now
    fn push(&mut self) -> Result<(), String> {
        if self.database.is_none() {
            self.database = Some(Database::open(&self.path)?);
        }
        let database = self.database.as_ref().expect("opened above");
        let at = now_millis();
        let push = Push {
            at,
            views: wire::views(database, at)?,
        };
        let json = serde_json::to_vec(&push).map_err(|error| error.to_string())?;
        let body = wire::gzip(&json);
        // signed as it's sent, compressed
        let signed_at = now_millis();
        let response = self
            .client
            .put(&self.state)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::CONTENT_ENCODING, "gzip")
            .header(SIGNED_AT, signed_at.to_string())
            .header(SIGNATURE, sign(&self.key, signed_at, &body))
            .body(body)
            .send()
            // its text holds the URL: without it
            .map_err(|error| format!("push: {}", error.without_url()))?;
        let status = response.status();
        if !status.is_success() {
            // the backend tells why: not its key, a clock too far from its own, a push of before
            let why = response.text().unwrap_or_default();
            let why: String = why.trim().chars().take(200).collect();
            return Err(format!("push refused, {status}: {why}"));
        }
        tracing::debug!(
            views = push.views.len(),
            bytes = json.len(),
            "pushed to the web backend"
        );
        self.wanted = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_isnt_an_http_url() {
        assert!(check("http://dashboard-web:8080").is_ok());
        assert!(check("https://dashboard.example/").is_ok());
        for url in [
            "dashboard-web:8080",
            "ftp://host",
            "http://",
            "nothing",
            // /state would go after the query
            "http://dashboard-web:8080/?token=abc",
            "http://dashboard-web:8080/#here",
        ] {
            let error = check(url).unwrap_err();
            assert!(error.contains("KRAKEN_BOT_PUSH_URL"), "{error}");
            assert!(!error.contains("abc"), "{error}");
        }
    }

    /// The very value kraken-dashboard-web checks: the same vector is in its tests, computed apart
    /// (Python's hmac). A change of one side only fails here or there
    #[test]
    fn signs_as_the_backend_checks() {
        assert_eq!(
            sign(
                b"kraken-dashboard-web",
                1_795_316_768_688,
                b"{\"at\":1795316768688,\"views\":[]}"
            ),
            "sha256=a7c3f506b4154c876d825fe791df3f461d098fc9527c99930e86473c935c6f34"
        );
    }

    /// HMAC-SHA256 itself, RFC 4231, test case 2: the key "Jefe", over "what do ya want for
    /// nothing?"
    #[test]
    fn computes_hmac_sha256() {
        let body = b"what do ya want for nothing?";
        // the time first: a body without it, signed at 0, is another message
        assert_ne!(sign(b"Jefe", 0, body), sign(b"Jefe", 1, body));
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"Jefe").unwrap();
        mac.update(body);
        assert_eq!(
            hex::encode(mac.finalize().into_bytes()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert!(sign(b"Jefe", 1, body).starts_with("sha256="));
        assert_eq!(sign(b"Jefe", 1, body).len(), "sha256=".len() + 64);
    }

    #[test]
    fn never_logs_what_the_url_holds_secret() {
        // put together: never written whole in the sources, which the scanners of secrets read
        let secret = ["https://bot", ":s3cret", "@dash.example/base?token=abc#x"].concat();
        assert_eq!(redacted(&secret), "https://dash.example/base");
        assert_eq!(
            redacted("http://dashboard-web:8080"),
            "http://dashboard-web:8080/"
        );
        assert_eq!(redacted("nothing"), "(not a URL)");
    }
}
