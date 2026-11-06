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
    // a tick before, then 60 of budget, 20 in the account no manager uses
    storage
        .record_account(&AccountRecord {
            at: now - 600_000,
            quote: "ZUSD".into(),
            cash: d("57.23"),
            holdings: d("22.00"),
        })
        .unwrap();
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

    // the whole account over the range: its name as the pairs say it, the oldest point first
    let series = model.account_series("USD");
    assert_eq!(series.len(), 2);
    assert!((series[0].1 - 79.23).abs() < 1e-9, "{series:?}");
    assert!((series[1].1 - 80.06).abs() < 1e-9, "{series:?}");
    assert_eq!(model.manager_series(2).len(), 2);

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
        "1 MONTH",
        "3 MONTHS",
        "1 YEAR",
        // 0.1584 earned in the first day: carried on for 30, 90 and 365 days
        "+4.75",
        "+14.26",
        "+57.82",
        "PORTFOLIO",
        "80.06 USD",
        "TARGET",
        "TO FILL",
        "▼0.79%",
        "RELOAD",
    ] {
        assert!(text.contains(expected), "no {expected} in\n{text}");
    }

    // the price of an open order lined up, whole, in the managers
    let wide = screen(&app, 200, 52);
    assert!(wide.contains("BUY  75,500.0"), "{wide}");
    // the reason of a refused order, whole, before its amount
    assert!(wide.contains("✗ EOrder:Insufficient funds"), "{wide}");
    assert!(text.contains("✗ EOrder:Insufficient"), "{text}");

    // every chart, the focus on a manager, the help: drawn without a panic
    for chart in [Chart::Price, Chart::Trades, Chart::Pnl] {
        app.charts[0] = chart;
        screen(&app, 180, 48);
    }
    app.charts[0] = Chart::Scenarios;
    let scenarios = screen(&app, 180, 48);
    assert!(scenarios.contains("IF THE MARKET MOVES"), "{scenarios}");
    assert!(scenarios.contains("NOW"), "{scenarios}");
    assert!(scenarios.contains("-10%"), "{scenarios}");
    app.charts[0] = Chart::Portfolio;
    let portfolio = screen(&app, 180, 48);
    assert!(portfolio.contains("PORTFOLIO · USD"), "{portfolio}");
    assert!(portfolio.contains("▲ 2 BUYS"), "{portfolio}");
    assert!(portfolio.contains("▼ 1 SELLS +0.16"), "{portfolio}");
    assert!(portfolio.contains("TARGET"), "{portfolio}");
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
fn plays_the_moves_of_the_market_on_the_portfolio() {
    let fixture = path("scenarios");
    let _storage = write(&fixture.path);
    let (app, _database) = app(&fixture.path);
    let model = &app.model;
    let account = model.account("USD");
    let scenarios = model.scenarios(&[1, 2], account);
    let at = |moved: i32| scenarios.iter().find(|it| it.moved == moved).unwrap();

    // no move: the portfolio as it is, nothing filled
    assert_eq!(at(0).value, d("80.06"));
    assert_eq!((at(0).buys, at(0).sells), (0, 0));

    // -2%: 74578 reaches the buy of manager 1 at 75500, its 0.0003 left bought
    let (first, second) = (model.manager(1).unwrap(), model.manager(2).unwrap());
    let price = d("76100") * d("0.98");
    let rest = d("80.06") - first.value - second.value;
    assert_eq!(
        at(-2).value,
        rest + first.quote_available
            + (first.base() + d("0.0003")) * price
            + second.quote_available
            + second.base() * price
    );
    assert_eq!((at(-2).buys, at(-2).sells), (1, 0));

    // +2%: the buy stays open, its quote in the portfolio
    assert_eq!(at(2).buys, 0);
    assert_eq!(
        at(2).value,
        rest + first.quote_available
            + first.quote_in_buy_order
            + first.base() * d("76100") * d("1.02")
            + second.quote_available
            + second.base() * d("76100") * d("1.02")
    );
}

#[test]
fn a_move_up_sells_at_the_target_and_moves_what_no_manager_owns() {
    let fixture = path("scenarios-sell");
    let storage = Storage::open(&fixture.path).unwrap();
    let now = now_millis();
    let seconds = now as f64 / 1000.0 - 3_600.0;
    let orders = [
        order(
            "B1",
            1,
            OrderSide::Buy,
            OrderStatus::Closed,
            "0.0004",
            "0.0004",
            "70000",
            "28",
            "0.112",
            seconds,
        ),
        order(
            "S1",
            1,
            OrderSide::Sell,
            OrderStatus::Open,
            "0.0004",
            "0",
            "71000",
            "0",
            "0",
            seconds + 60.0,
        ),
    ];
    storage.upsert_orders(&orders).unwrap();
    let config = config(1, "XBTUSD");
    storage
        .save_manager(&ManagerState {
            balances: ledger::compute(config.budget, &orders),
            config,
            last_price: d("70000"),
            at: now,
        })
        .unwrap();
    // 0.001 XBT and 50 USD no manager owns, beside the 0.0004 XBT and 1.888 USD of manager 1
    storage
        .record_account(&AccountRecord {
            at: now,
            quote: "ZUSD".into(),
            cash: d("51.888"),
            holdings: d("98"),
        })
        .unwrap();
    let (app, _database) = app(&fixture.path);
    let model = &app.model;
    let manager = model.manager(1).unwrap();
    assert_eq!(manager.position_cost, d("28.112"));
    let scenarios = model.scenarios(&[1], model.account("USD"));
    let at = |moved: i32| scenarios.iter().find(|it| it.moved == moved).unwrap();

    assert_eq!(at(0).value, d("149.888"));

    // +2%: 71400 reaches the sell: its cost + 1.5%, and the bitcoins of the account at 71400
    assert_eq!((at(2).buys, at(2).sells), (0, 1));
    assert_eq!(
        at(2).value,
        d("50") + d("70") * d("1.02") + d("1.888") + d("28.112") * d("1.015")
    );

    // -2%: the sell stays open, everything at 68600
    assert_eq!((at(-2).buys, at(-2).sells), (0, 0));
    assert_eq!(at(-2).value, d("51.888") + d("98") * d("0.98"));
}

#[test]
fn shows_a_column_of_different_charts() {
    let fixture = path("column");
    let _storage = write(&fixture.path);
    let (mut app, _database) = app(&fixture.path);

    let text = screen(&app, 180, 48);
    assert!(text.contains("PORTFOLIO · USD"), "{text}");
    assert!(text.contains("IF THE MARKET MOVES · USD"), "{text}");
    assert!(text.contains("XBTUSD · #1"), "{text}");

    // each key its own place, never a chart of another: back at the start in 5 presses, the 7
    // charts without the 2 of the other places
    let charts = app.charts;
    for key in ['c', 'v', 'b'] {
        for _ in 0..5 {
            app.on_key(KeyEvent::from(KeyCode::Char(key)));
            let [first, second, third] = app.charts;
            assert!(first != second && first != third && second != third);
            screen(&app, 180, 48);
        }
        assert_eq!(app.charts, charts);
    }

    // lower: fewer charts, the first ones
    let low = screen(&app, 180, 36);
    assert!(low.contains("PORTFOLIO · USD"), "{low}");
    assert!(low.contains("IF THE MARKET MOVES · USD"), "{low}");
    assert!(!low.contains("XBTUSD · #1"), "{low}");
    let lower = screen(&app, 140, 28);
    assert!(lower.contains("PORTFOLIO · USD"), "{lower}");
    assert!(!lower.contains("IF THE MARKET MOVES · USD"), "{lower}");
}

#[test]
fn the_charts_dont_move_between_two_frames() {
    let fixture = path("still");
    let _storage = write(&fixture.path);
    let (mut app, _database) = app(&fixture.path);
    app.charts = [Chart::Portfolio, Chart::Price, Chart::Trades];
    // without the header and the prices scrolling
    let charts = |app: &App| {
        screen(app, 180, 48)
            .lines()
            .skip(2)
            .map(String::from)
            .collect::<Vec<_>>()
    };
    let first = charts(&app);
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(charts(&app), first);
    }
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
    for chart in [
        Chart::Pnl,
        Chart::Portfolio,
        Chart::Price,
        Chart::Trades,
        Chart::Scenarios,
        Chart::Growth,
    ] {
        app.charts[0] = chart;
        println!("{}", screen(&app, 200, 52));
    }
}

/// A second account next to the one of `write`, with the same manager ids and its own money
fn second_account(path: &Path) -> Storage {
    let storage = Storage::open_account(path, "alice").unwrap();
    let now = now_millis();
    let balances = ledger::compute(d("500"), &[]);
    storage
        .save_manager(&ManagerState {
            config: ManagerConfig {
                budget: d("500"),
                ..config(1, "XBTUSD")
            },
            balances,
            last_price: d("70000"),
            at: now - 1_000,
        })
        .unwrap();
    storage
        .record_account(&AccountRecord {
            at: now - 1_000,
            quote: "ZUSD".into(),
            cash: d("500"),
            holdings: d("0"),
        })
        .unwrap();
    storage
}

fn press(app: &mut App, database: &Database, key: char) {
    if app.on_key(KeyEvent::from(KeyCode::Char(key))) {
        app.refresh(database, true);
    }
}

#[test]
fn adds_up_the_accounts_and_goes_through_them_in_a_circle() {
    let fixture = path("accounts");
    let main = write(&fixture.path);
    let _alice = second_account(&fixture.path);
    main.record_account(&AccountRecord {
        at: now_millis(),
        quote: "ZUSD".into(),
        cash: d("100"),
        holdings: d("20"),
    })
    .unwrap();
    let (mut app, database) = app(&fixture.path);

    // every account together: both managers #1, told apart, and the money of both
    let main_managers = app
        .model
        .data
        .managers
        .iter()
        .filter(|it| kraken_dashboard::data::account_of(it.id) == 1)
        .count();
    assert_eq!(app.view, None);
    assert_eq!(app.model.data.managers.len(), main_managers + 1);
    assert_eq!(app.model.account("USD").unwrap().value(), d("620"));
    let text = screen(&app, 200, 50);
    assert!(text.contains("ALL ACCOUNTS · 2"), "{text}");
    assert!(text.contains("main·#1"), "{text}");
    assert!(text.contains("alice·#1"), "{text}");

    // the first account alone
    press(&mut app, &database, 'a');
    assert_eq!(app.view, Some(1));
    assert_eq!(app.model.data.managers.len(), main_managers);
    assert_eq!(app.model.account("USD").unwrap().value(), d("120"));
    let text = screen(&app, 200, 50);
    assert!(text.contains("MAIN · 1/2"), "{text}");
    assert!(!text.contains("alice·"), "{text}");

    // the second one, its manager with the id it has in the YAML file
    press(&mut app, &database, 'a');
    assert_eq!(app.view, Some(2));
    assert_eq!(app.model.data.managers.len(), 1);
    assert_eq!(app.model.data.managers[0].number(), 1);
    assert_eq!(app.model.account("USD").unwrap().value(), d("500"));
    assert!(app.model.data.orders.is_empty());
    assert!(screen(&app, 200, 50).contains("ALICE · 2/2"));

    // and back to all of them
    press(&mut app, &database, 'a');
    assert_eq!(app.view, None);
    assert_eq!(app.model.data.managers.len(), main_managers + 1);
}

#[test]
fn asks_for_the_tick_of_the_account_shown() {
    let fixture = path("account-tick");
    let main = write(&fixture.path);
    let alice = second_account(&fixture.path);
    let (mut app, database) = app(&fixture.path);

    press(&mut app, &database, 'a');
    press(&mut app, &database, 'a');
    assert_eq!(app.view, Some(2));
    press(&mut app, &database, 't');
    app.ask_tick(&database);
    assert_eq!(
        alice.command(FORCED_TICK).unwrap(),
        Some(CommandState::Refresh)
    );
    assert_eq!(main.command(FORCED_TICK).unwrap(), Some(CommandState::Idle));
    alice.start_command(FORCED_TICK, now_millis()).unwrap();
    alice.end_command(FORCED_TICK, now_millis()).unwrap();

    // all of them together: a tick for each
    press(&mut app, &database, 'a');
    assert_eq!(app.view, None);
    press(&mut app, &database, 't');
    app.ask_tick(&database);
    assert_eq!(
        alice.command(FORCED_TICK).unwrap(),
        Some(CommandState::Refresh)
    );
    assert_eq!(
        main.command(FORCED_TICK).unwrap(),
        Some(CommandState::Refresh)
    );
}

#[test]
fn says_nothing_of_the_accounts_with_a_single_one() {
    let fixture = path("single-account");
    let _main = write(&fixture.path);
    let (mut app, database) = app(&fixture.path);
    let text = screen(&app, 200, 50);
    assert!(!text.contains("ALL ACCOUNTS"), "{text}");
    assert!(!text.contains("ACCOUNT "), "{text}");
    assert!(!text.contains("main·"), "{text}");
    press(&mut app, &database, 'a');
    assert_eq!(app.view, None);
}

#[test]
fn shows_the_growth_of_each_period() {
    let fixture = path("growth");
    let storage = Storage::open(&fixture.path).unwrap();
    let now = now_millis();
    const HOUR: i64 = 3_600_000;
    // +1 then -0.5 on 100 in the account, a deposit of 100 in the meantime
    for (hours, value, account) in [(3, "30", "100"), (2, "31", "100"), (1, "30.5", "200")] {
        let at = now - hours * HOUR;
        storage
            .save_manager(&ManagerState {
                config: config(1, "XBTUSD"),
                balances: ledger::compute(d(value), &[]),
                last_price: d("70000"),
                at,
            })
            .unwrap();
        storage
            .record_account(&AccountRecord {
                at,
                quote: "ZUSD".into(),
                cash: d(account),
                holdings: d("0"),
            })
            .unwrap();
    }
    let (mut app, _database) = app(&fixture.path);
    app.charts[0] = Chart::Growth;
    let text = screen(&app, 200, 50);
    assert!(text.contains("GROWTH PER HOUR"), "{text}");
    assert!(text.contains("-0.50% prev +1.00% avg +0.25%"), "{text}");
}

fn arrow(app: &mut App, code: KeyCode, times: usize) {
    for _ in 0..times {
        app.on_key(KeyEvent::from(code));
    }
}

#[test]
fn sorts_the_managers_with_the_cursor_on_the_same_one() {
    let fixture = path("sort-managers");
    let _storage = write(&fixture.path);
    let (mut app, database) = app(&fixture.path);
    let numbers = |app: &App| -> Vec<i32> {
        app.model
            .data
            .managers
            .iter()
            .map(|it| it.number())
            .collect()
    };
    assert_eq!(numbers(&app), [1, 2]);
    assert!(screen(&app, 200, 50).contains("MANAGER▲"));

    // the P&L, the largest first: three columns on, the rows in its order
    arrow(&mut app, KeyCode::Right, 3);
    let pnl: Vec<_> = app.model.data.managers.iter().map(|it| it.pnl()).collect();
    assert!(pnl[0] > pnl[1], "{pnl:?}");
    let first = numbers(&app);
    assert_eq!(app.selected().unwrap().number(), 1);
    assert!(screen(&app, 200, 50).contains("P&L▼"));

    // the other way, the cursor still on its manager
    let selected = app.selected().unwrap().id;
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    assert_eq!(numbers(&app), [first[1], first[0]]);
    assert_eq!(app.selected().unwrap().id, selected);
    assert_eq!(app.selected().unwrap().number(), 1);
    assert!(screen(&app, 200, 50).contains("P&L▲"));

    // reading the database again keeps both
    app.refresh(&database, true);
    assert_eq!(numbers(&app), [first[1], first[0]]);
    assert_eq!(app.selected().unwrap().id, selected);

    // back to where it started, then around the circle
    arrow(&mut app, KeyCode::Left, 3);
    assert_eq!(numbers(&app), [1, 2]);
    arrow(&mut app, KeyCode::Left, 1);
    assert!(screen(&app, 200, 50).contains("TREND▼"));
}

#[test]
fn sorts_the_orders_and_the_tape_on_their_own() {
    let fixture = path("sort-orders");
    let _storage = write(&fixture.path);
    let (mut app, _database) = app(&fixture.path);
    let txids =
        |app: &App| -> Vec<String> { app.orders().iter().map(|it| it.txid.clone()).collect() };
    arrow(&mut app, KeyCode::Tab, 1);
    assert_eq!(txids(&app)[0], "O3");
    assert!(screen(&app, 200, 50).contains("OPENED▼"));

    // the highest price first, and the row back at the top
    arrow(&mut app, KeyCode::Down, 2);
    arrow(&mut app, KeyCode::Right, 4);
    assert_eq!(app.orders_row, 0);
    assert_eq!(txids(&app), ["O2", "O3", "O1", "O4"]);
    assert!(screen(&app, 200, 50).contains("PRICE▼"));

    // only the open order is to fill: it stays first, the "—" after it, both ways
    arrow(&mut app, KeyCode::Right, 1);
    assert_eq!(txids(&app)[0], "O3");
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    assert_eq!(txids(&app)[0], "O3");

    // a column this width leaves out: the title of the panel tells it
    arrow(&mut app, KeyCode::Right, 3);
    assert!(screen(&app, 200, 50).contains("BY COST ▼"));

    // the tape has its own order, the one of the managers didn't move
    arrow(&mut app, KeyCode::Tab, 1);
    assert!(screen(&app, 200, 50).contains("AT▼"));
    arrow(&mut app, KeyCode::Right, 2);
    let kinds: Vec<&str> = app.actions().iter().map(|it| it.kind.as_str()).collect();
    assert_eq!(kinds, ["buy", "sell"]);
    assert!(screen(&app, 200, 50).contains("WHAT▲"));
    assert!(screen(&app, 200, 50).contains("MANAGER▲"));
}

#[test]
fn tells_what_is_free_and_what_is_missing_in_the_account() {
    let fixture = path("missing");
    let storage = write(&fixture.path);
    let (mut app, database) = app(&fixture.path);

    // enough for the managers: what's free is what no open order holds
    let account = app.model.account("USD").unwrap().clone();
    let free = app.model.free_cash(&account);
    let in_buy_orders: Decimal = app
        .model
        .data
        .managers
        .iter()
        .map(|it| it.quote_in_buy_order)
        .sum();
    assert!(!in_buy_orders.is_zero());
    assert_eq!(free, account.cash - in_buy_orders);
    let text = screen(&app, 200, 50);
    assert!(text.contains("mgrs "), "{text}");
    assert!(text.contains(" · free "), "{text}");
    assert!(!text.contains("missing"), "{text}");

    // less than the managers count on, nothing free
    storage
        .record_account(&AccountRecord {
            at: now_millis(),
            quote: "ZUSD".into(),
            cash: d("10"),
            holdings: d("0"),
        })
        .unwrap();
    app.refresh(&database, true);
    let account = app.model.account("USD").unwrap().clone();
    assert_eq!(app.model.free_cash(&account), d("0"));
    let text = screen(&app, 200, 50);
    assert!(text.contains("missing "), "{text}");
    assert!(text.contains(" · free 0.00"), "{text}");
    assert!(!text.contains("mgrs "), "{text}");
}

#[test]
fn shows_what_the_portfolio_is_made_of() {
    let fixture = path("allocation");
    let storage = write(&fixture.path);
    let (mut app, database) = app(&fixture.path);
    let (now, since) = (app.loaded_at, app.model.data.since);

    // the 0.0004 XBT of the managers at 76100, the cash of the account
    let allocation = app.model.allocation("USD", since, now, 20);
    assert_eq!(allocation.names, ["XBT", "USD"]);
    let (at, last) = allocation.points.last().unwrap();
    assert_eq!(*at, now as f64);
    assert!((last[0] - 0.0004 * 76_100.0).abs() < 1e-6, "{last:?}");
    assert!((last[1] - 57.23).abs() < 1e-6, "{last:?}");
    // from the first tick of the account on: the range is longer than the history
    assert!(allocation.points.len() < 20);

    // bitcoins no manager bought
    storage
        .record_account(&AccountRecord {
            at: now_millis(),
            quote: "ZUSD".into(),
            cash: d("10"),
            holdings: d("100"),
        })
        .unwrap();
    app.refresh(&database, true);
    let allocation = app.model.allocation("USD", since, app.loaded_at, 20);
    assert_eq!(allocation.names, ["XBT", "USD", "OTHER"]);
    let last = &allocation.points.last().unwrap().1;
    assert!(
        (last[2] - (100.0 - 0.0004 * 76_100.0)).abs() < 1e-6,
        "{last:?}"
    );

    // the place of the chart in the cycle, with what each is worth now
    app.charts[0] = Chart::Growth;
    app.on_key(KeyEvent::from(KeyCode::Char('c')));
    assert_eq!(app.charts[0], Chart::Allocation);
    let text = screen(&app, 200, 50);
    assert!(text.contains("BY CRYPTO · USD"), "{text}");
    assert!(text.contains("■XBT 30 "), "{text}");
    assert!(text.contains("■OTHER 70 "), "{text}");
}

#[test]
fn goes_from_the_desk_to_every_chart_and_back() {
    let fixture = path("all-charts");
    let _storage = write(&fixture.path);
    let (mut app, _database) = app(&fixture.path);
    let charts = app.charts;
    assert!(screen(&app, 200, 50).contains("MANAGERS"));

    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(app.all_charts);
    let text = screen(&app, 200, 50);
    for title in [
        "PORTFOLIO · USD",
        "BY CRYPTO · USD",
        "GROWTH",
        "P&L",
        "SELLS",
        "IF THE MARKET MOVES · USD",
        "XBTUSD · #1",
    ] {
        assert!(text.contains(title), "{title}: {text}");
    }
    assert!(!text.contains("MANAGERS"), "{text}");
    assert!(!text.contains("TAPE"), "{text}");
    assert!(text.contains("DESK"), "{text}");

    // the arrows choose the manager of the price, whatever the panel; the keys of the desk do nothing
    app.on_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(app.cursor, 1);
    assert!(screen(&app, 200, 50).contains(" · #2"));
    for key in ['c', 'v', 'b', 'o', 'i', 's'] {
        app.on_key(KeyEvent::from(KeyCode::Char(key)));
    }
    app.on_key(KeyEvent::from(KeyCode::Tab));
    assert_eq!(app.charts, charts);
    assert!(!app.open_only && !app.show_idle);

    // every size it's drawn on, without panicking
    for (width, height) in [(100, 28), (120, 30), (160, 40), (300, 80)] {
        assert!(screen(&app, width, height).contains("PORTFOLIO"));
    }

    // back with esc, or the same key; ctrl-c always quits
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.all_charts);
    assert!(screen(&app, 200, 50).contains("MANAGERS"));
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(!app.all_charts);
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    app.on_key(KeyEvent::new(
        KeyCode::Char('c'),
        ratatui::crossterm::event::KeyModifiers::CONTROL,
    ));
    assert!(app.quit);
}
