//! The dashboard on a database written by the storage of the bot itself: a change of its schema
//! breaks these tests, not the screen

use kraken_api::trading::{OrderSide, OrderStatus};
use kraken_dashboard::app::{App, Chart, now_millis};
use kraken_dashboard::data::Database;
use kraken_trading_bot::config::{ManagerConfig, ManagerMode};
use kraken_trading_bot::ledger;
use kraken_trading_bot::order::BotOrder;
use kraken_trading_bot::storage::{
    AccountRecord, ActionRecord, CommandState, FORCED_TICK, ManagerState, Storage, TickerRecord,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use rust_decimal::Decimal;
use std::path::{Path, PathBuf};
use std::str::FromStr;

fn d(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

/// A database of its own for each test, removed at the end
struct Fixture {
    path: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn path(name: &str) -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "kraken-dashboard-{name}-{}.sqlite",
        std::process::id()
    ));
    Fixture { path }
}

fn config(id: i32, pair: &str) -> ManagerConfig {
    ManagerConfig {
        id,
        pair: pair.into(),
        budget: d("30"),
        buy_below: d("1"),
        sell_above: d("1.5"),
        mode: ManagerMode::Active,
    }
}

#[allow(clippy::too_many_arguments)]
fn order(
    txid: &str,
    manager_id: i32,
    side: OrderSide,
    status: OrderStatus,
    volume: &str,
    executed: &str,
    price: &str,
    cost: &str,
    fee: &str,
    time: f64,
) -> BotOrder {
    BotOrder {
        txid: txid.into(),
        manager_id,
        pair: "XBTUSD".into(),
        side,
        status,
        volume: d(volume),
        volume_executed: d(executed),
        cost: d(cost),
        fee: d(fee),
        limit_price: Some(d(price)),
        average_price: Some(d(price)),
        open_time: time,
        close_time: status.is_final().then_some(time + 60.0),
        expire_time: None,
    }
}

/// Manager 1 bought and sold once, then placed a buy; manager 2 ("XBT/USD") holds what it bought
fn write(path: &Path) -> Storage {
    let storage = Storage::open(path).unwrap();
    let now = now_millis();
    let seconds = now as f64 / 1000.0 - 3_600.0;

    let first = [
        order(
            "O1",
            1,
            OrderSide::Buy,
            OrderStatus::Closed,
            "0.0004",
            "0.0004",
            "75000",
            "30",
            "0.12",
            seconds,
        ),
        order(
            "O2",
            1,
            OrderSide::Sell,
            OrderStatus::Closed,
            "0.0004",
            "0.0004",
            "76000",
            "30.4",
            "0.1216",
            seconds + 600.0,
        ),
        order(
            "O3",
            1,
            OrderSide::Buy,
            OrderStatus::Open,
            "0.0004",
            "0.0001",
            "75500",
            "7.55",
            "0.0302",
            seconds + 1200.0,
        ),
    ];
    let second = [order(
        "O4",
        2,
        OrderSide::Buy,
        OrderStatus::Closed,
        "0.0003",
        "0.0003",
        "74000",
        "22.2",
        "0.0888",
        seconds,
    )];
    storage.upsert_orders(&first).unwrap();
    storage.upsert_orders(&second).unwrap();

    for (index, price) in ["75800", "76100"].into_iter().enumerate() {
        let at = now - 600_000 + index as i64 * 300_000;
        storage
            .record_ticker(&TickerRecord {
                pair: "XXBTZUSD".into(),
                at,
                last: d(price),
                bid: d(price) - d("0.1"),
                ask: d(price) + d("0.1"),
                volume_24h: d("1200"),
                vwap_24h: d("75900"),
            })
            .unwrap();
        for (config, orders) in [
            (config(1, "XBTUSD"), &first[..]),
            (config(2, "XBT/USD"), &second[..]),
        ] {
            storage
                .save_manager(&ManagerState {
                    balances: ledger::compute(config.budget, orders),
                    config,
                    last_price: d(price),
                    at: at + 2,
                })
                .unwrap();
        }
    }

    let action = |kind: &str, txid: Option<&str>, error: Option<&str>| ActionRecord {
        manager_id: 1,
        at: now - 60_000,
        kind: kind.into(),
        amount: Some(d("0.0004")),
        price: Some(d("75500")),
        reason: Some("buys at -1%".into()),
        dry_run: false,
        txid: txid.map(Into::into),
        error: error.map(Into::into),
    };
    // 60 of budget, 20 in the account no manager uses
    storage
        .record_account(&AccountRecord {
            at: now - 300_000,
            quote: "ZUSD".into(),
            cash: d("57.23"),
            holdings: d("22.83"),
        })
        .unwrap();
    storage.record_action(&action("idle", None, None)).unwrap();
    storage
        .record_action(&action("sell", None, Some("EOrder:Insufficient funds")))
        .unwrap();
    storage
        .record_action(&action("buy", Some("O3"), None))
        .unwrap();
    storage
}

fn app(path: &Path) -> (App, Database) {
    let database = Database::open(path).unwrap();
    let mut app = App::new(path.display().to_string());
    app.refresh(&database, true);
    assert_eq!(app.error, None);
    (app, database)
}

fn screen(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| kraken_dashboard::ui::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

#[test]
fn reads_what_the_bot_wrote() {
    let fixture = path("reads");
    let _storage = write(&fixture.path);
    let (app, _database) = app(&fixture.path);
    let model = &app.model;

    assert_eq!(model.data.managers.len(), 2);
    assert_eq!(model.data.orders.len(), 4);
    assert_eq!(model.data.actions.len(), 3);
    assert_eq!(model.data.markets.len(), 1);
    assert_eq!(model.data.markets[0].prices.len(), 2);

    // 30.4 - 0.1216 received for what cost 30.12
    assert_eq!(model.performances[&1].realized, d("0.1584"));
    assert_eq!(model.performances[&1].trades.len(), 1);
    assert_eq!(model.performances[&2].realized, Decimal::ZERO);

    // "XBTUSD" and "XBT/USD" are the pair of the ticker "XXBTZUSD"
    assert_eq!(model.market_of(1).unwrap().key, "XXBTZUSD");
    assert_eq!(model.market_of(2).unwrap().key, "XXBTZUSD");

    let totals = &model.totals[0];
    assert_eq!(totals.quote, "USD");
    assert_eq!(totals.managers, 2);
    assert_eq!(totals.budget, d("60"));
    assert_eq!(totals.open_orders, 1);
    assert_eq!(totals.trades, 1);
    assert_eq!(totals.wins, 1);
    assert_eq!(totals.pnl(), totals.realized + totals.unrealized());

    // the whole account, and how far the open buy is from the market: 75500 for 76100
    assert_eq!(model.account("USD").unwrap().value(), d("80.06"));
    let open = model.data.orders.iter().find(|it| it.txid == "O3").unwrap();
    let gap = model.to_fill(open).unwrap();
    assert!(
        (gap - (75500.0 - 76100.0) / 76100.0 * 100.0).abs() < 1e-9,
        "{gap}"
    );
    let filled = model.data.orders.iter().find(|it| it.txid == "O1").unwrap();
    assert_eq!(model.to_fill(filled), None);

    // sold at its target: the position of manager 2 + 1.5%, fees included
    let second = model.manager(2).unwrap();
    assert_eq!(second.position_cost, d("22.2888"));
    assert_eq!(
        second.target(),
        second.quote_available + d("22.2888") * d("1.015")
    );
    // manager 1: its open buy filled and sold, what it holds sold
    let first = model.manager(1).unwrap();
    assert_eq!(
        first.target(),
        first.quote_available + (first.position_cost + first.quote_in_buy_order) * d("1.015")
    );
    assert_eq!(totals.target, first.target() + second.target());

    // the idle decisions are hidden by default
    assert_eq!(app.actions().len(), 2);
    assert_eq!(app.model.pnl_series(&[1, 2]).len(), 2);
}

#[test]
fn draws_the_desk() {
    let fixture = path("draws");
    let _storage = write(&fixture.path);
    let (mut app, _database) = app(&fixture.path);

    let text = screen(&app, 180, 48);
    for expected in [
        "KRAKEN",
        "LIVE",
        "MANAGERS",
        "ORDERS",
        "MARKETS",
        "TAPE",
        "XBTUSD",
        "#1",
        "FILLED",
        "OPEN",
        "EOrder:Insufficient funds",
        "WIN RATE",
        "100.0%",
        "PORTFOLIO",
        "80.06 USD",
        "TARGET",
        "TO FILL",
        "▼0.79%",
        "RELOAD",
    ] {
        assert!(text.contains(expected), "no {expected} in\n{text}");
    }

    // every chart, the focus on a manager, the help: drawn without a panic
    for chart in [Chart::Price, Chart::Trades, Chart::Pnl] {
        app.chart = chart;
        screen(&app, 180, 48);
    }
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.focus().map(|it| it.id), Some(1));
    // a manager alone: what it's worth, not the account
    let focused = screen(&app, 180, 48);
    assert!(focused.contains("MANAGER #1"), "{focused}");
    assert!(focused.contains("EQUITY"), "{focused}");
    app.on_key(KeyEvent::from(KeyCode::Char('?')));
    assert!(screen(&app, 120, 32).contains("HELP"));
    assert!(screen(&app, 60, 20).contains("enlarge the terminal"));
}

#[test]
fn draws_an_empty_database() {
    let fixture = path("empty");
    let _storage = Storage::open(&fixture.path).unwrap();
    let (app, _database) = app(&fixture.path);
    let text = screen(&app, 140, 40);
    assert!(text.contains("NO MANAGER YET"), "{text}");
    assert!(text.contains("NO ORDER YET"), "{text}");
}

#[test]
fn sees_when_the_bot_writes() {
    let fixture = path("version");
    let storage = write(&fixture.path);
    let database = Database::open(&fixture.path).unwrap();
    let before = database.data_version().unwrap();
    assert_eq!(database.data_version().unwrap(), before);

    storage
        .record_action(&ActionRecord {
            manager_id: 1,
            at: now_millis(),
            kind: "idle".into(),
            amount: None,
            price: None,
            reason: None,
            dry_run: true,
            txid: None,
            error: None,
        })
        .unwrap();
    assert_ne!(database.data_version().unwrap(), before);
}

#[test]
fn asks_the_bot_for_a_tick() {
    let fixture = path("tick");
    let storage = write(&fixture.path);
    let (mut app, database) = app(&fixture.path);
    assert_eq!(app.model.data.forced_tick.as_deref(), Some("idle"));

    app.on_key(KeyEvent::from(KeyCode::Char('t')));
    app.ask_tick(&database);
    assert_eq!(app.error, None);
    assert_eq!(app.model.data.forced_tick.as_deref(), Some("refresh"));
    assert!(screen(&app, 180, 48).contains("TICK ASKED"));

    // the bot starts it, then ends it
    assert!(storage.start_command(FORCED_TICK, now_millis()).unwrap());
    app.refresh(&database, false);
    assert!(screen(&app, 180, 48).contains("TICKING"));
    // asked again while under way: the same tick
    app.on_key(KeyEvent::from(KeyCode::Char('t')));
    app.ask_tick(&database);
    assert_eq!(
        storage.command(FORCED_TICK).unwrap(),
        Some(CommandState::Pending)
    );
    storage.end_command(FORCED_TICK, now_millis()).unwrap();
    app.refresh(&database, false);
    assert_eq!(app.model.data.forced_tick.as_deref(), Some("idle"));
    let text = screen(&app, 180, 48);
    assert!(
        !text.contains("TICK ASKED") && !text.contains("TICKING"),
        "{text}"
    );
}

#[test]
fn refuses_what_isnt_a_database_of_the_bot() {
    let fixture = path("other");
    rusqlite::Connection::open(&fixture.path)
        .unwrap()
        .execute_batch("CREATE TABLE things (id INTEGER)")
        .unwrap();
    let error = Database::open(&fixture.path).err().unwrap();
    assert!(error.contains("no table orders"), "{error}");

    let error = Database::open(&fixture.path.with_extension("missing"))
        .err()
        .unwrap();
    assert!(error.contains("no database"), "{error}");
}

/// `DASHBOARD_DEMO=demo.sqlite cargo test -p kraken-dashboard demo -- --ignored --nocapture`
/// prints the screen of a database, e.g. the one of `--example demo`
#[test]
#[ignore]
fn demo() {
    let path = PathBuf::from(std::env::var("DASHBOARD_DEMO").unwrap());
    let (mut app, _database) = app(&path);
    for chart in [Chart::Pnl, Chart::Price, Chart::Trades] {
        app.chart = chart;
        println!("{}", screen(&app, 200, 52));
    }
}
