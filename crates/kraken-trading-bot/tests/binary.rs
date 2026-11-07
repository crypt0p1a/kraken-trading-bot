//! The executable itself, as Docker runs it, against a mocked Kraken (KRAKEN_BOT_API_HOST): its
//! arguments, its environment, --check, --once, and the loop until it's asked to stop

mod common;

use common::{PERMISSIONS, kraken, kraken_with};
use kraken_trading_bot::storage::{FORCED_TICK, Storage};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// A folder of its own for each test, removed at the end
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "kraken-bot-binary-{name}-{}-{}",
            std::process::id(),
            kraken_trading_bot::clock::now_millis()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("managers.yaml"),
            "dry_run: true
tick_interval: 1h
managers:
  - {id: 42, pair: XBTUSD, budget: 100, buy_below: 1, sell_above: 1.5}
",
        )
        .unwrap();
        Self(path)
    }

    fn database(&self) -> PathBuf {
        self.0.join("bot.sqlite")
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The bot in the folder, the environment of the machine left out: no .env read, no real key
fn bot(folder: &Folder, host: Option<&str>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kraken-trading-bot"));
    command.current_dir(&folder.0).env("NO_COLOR", "1");
    for (name, _) in std::env::vars() {
        if name.starts_with("KRAKEN_") {
            command.env_remove(name);
        }
    }
    if let Some(host) = host {
        command
            .env("KRAKEN_API_KEY", "key")
            .env("KRAKEN_API_SECRET", "c2VjcmV0")
            .env("KRAKEN_BOT_API_HOST", host)
            .env("KRAKEN_BOT_CONFIG", folder.0.join("managers.yaml"))
            .env("KRAKEN_BOT_DATABASE", folder.database());
    }
    command
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The command run aside, the mocked Kraken answering meanwhile
async fn run(mut command: Command) -> Output {
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

#[test]
fn refuses_unknown_arguments() {
    let folder = Folder::new("arguments");
    let output = bot(&folder, None).arg("--loop").output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("unknown arguments"),
        "{}",
        text(&output)
    );
}

#[test]
fn refuses_to_start_without_credentials() {
    let folder = Folder::new("credentials");
    let output = bot(&folder, None).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("KRAKEN_API_KEY is required"),
        "{}",
        text(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_the_configuration_the_pairs_and_the_key() {
    let folder = Folder::new("check");
    let server = kraken().await;
    let output = run({
        let mut command = bot(&folder, Some(&server.uri()));
        command.arg("--check");
        command
    })
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("the configuration, the pairs and the API keys are valid"),
        "{}",
        text(&output)
    );
    // nothing placed, nothing written but the account
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|it| it.url.path() == "/0/private/AddOrder")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_key_that_can_withdraw() {
    let folder = Folder::new("withdraw");
    let mut permissions = PERMISSIONS.to_vec();
    permissions.push("withdraw-funds");
    let server = kraken_with(&permissions).await;
    let output = run({
        let mut command = bot(&folder, Some(&server.uri()));
        command.arg("--check");
        command
    })
    .await;
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("withdraw"), "{}", text(&output));
}

#[tokio::test(flavor = "multi_thread")]
async fn ticks_once() {
    let folder = Folder::new("once");
    let server = kraken().await;
    let output = run({
        let mut command = bot(&folder, Some(&server.uri()));
        command.arg("--once");
        command
    })
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(text(&output).contains("tick done"), "{}", text(&output));

    // the manager saved, its buy only validated by Kraken: a dry run
    let storage = Storage::open(&folder.database()).unwrap();
    assert!(storage.manager_balances(42).unwrap().is_some());
    let requests = server.received_requests().await.unwrap();
    let add_order = requests
        .iter()
        .find(|it| it.url.path() == "/0/private/AddOrder")
        .expect("AddOrder");
    assert!(common::form(add_order).contains(&("validate".into(), "true".into())));
}

#[cfg(unix)]
type Lines = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

#[cfg(unix)]
fn count(lines: &Lines, text: &str) -> usize {
    lines
        .lock()
        .unwrap()
        .iter()
        .filter(|it| it.contains(text))
        .count()
}

/// Until the output says it `times` times, a minute at most
#[cfg(unix)]
async fn wait_for(lines: &Lines, text: &str, times: usize) {
    let start = std::time::Instant::now();
    while count(lines, text) < times {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "no {text}: {:?}",
            lines.lock().unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The loop: a tick, a tick asked for by the dashboard, then SIGTERM as `docker stop` sends it
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ticks_until_it_is_asked_to_stop() {
    use std::io::{BufRead, BufReader};
    use std::sync::{Arc, Mutex};

    let folder = Folder::new("loop");
    let server = kraken().await;
    let mut child = bot(&folder, Some(&server.uri()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines: Lines = Arc::new(Mutex::new(Vec::new()));
    let readers: Vec<_> = [
        Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ]
    .into_iter()
    .map(|stream| {
        let lines = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(line);
            }
        })
    })
    .collect();
    wait_for(&lines, "tick done", 1).await;
    // the dashboard asks for a tick: done without waiting for the hour of the interval
    Storage::open(&folder.database())
        .unwrap()
        .request_command(FORCED_TICK, kraken_trading_bot::clock::now_millis())
        .unwrap();
    wait_for(&lines, "tick forced", 1).await;
    wait_for(&lines, "tick done", 2).await;

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(status.code(), Some(0), "{:?}", lines.lock().unwrap());
    assert_eq!(count(&lines, "stopping"), 1);
}
