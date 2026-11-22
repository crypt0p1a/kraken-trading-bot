//! The screen: a header with the ticker tape, the key figures, the managers and a chart, the
//! order blotter, the markets and the tape of the decisions
//!
//! ```text
//! ◆ KRAKEN  TRADING DESK  ● LIVE                      TICK 42s AGO │ UTC 2026-11-03 14:32:07
//!  XBTUSD 76,567.1 ▲ +1.23%  ◆  ETHUSD 2,233.67 ▼ -0.45%  ◆  …
//! ┌PORTF.──┐┌TARGET┐┌P&L───┐┌REALIZ┐┌UNREAL┐┌1 MON.┐┌3 MON.┐┌1 YEAR┐┌FEES──┐┌ORDERS┐
//! ┌MANAGERS───────────────────────────────────┐┌PORTFOLIO · USD · 24H───┐
//! │                                           ││                        │
//! ┌ORDERS──────────────────┐┌MARKETS───────────┐┌IF THE MARKET MOVES─────┐
//! │                        │┌TAPE──────────────┐┌XBTUSD · #1─────────────┐
//!  q QUIT  tab PANEL  ↑↓ MOVE  ⏎ FOCUS  c CHART  1-5 RANGE …
//! ```
//!
//! `d` replaces the key figures and the tables with every chart, then with the page of the markets,
//! one pair at a time, then brings the desk back.
//!
//! A screen smaller than 100 by 28 cells, a phone or a terminal too small, gets the [`compact`]
//! view: what the portfolio is worth and how it moves, nothing to move through

mod charts;
pub mod compact;
mod markets;
mod panels;

use crate::app::{App, Range, now_millis};
use crate::data::{percent, to_f64};
use crate::format;
use crate::keys::KeyCode;
use crate::stats::Totals;
use crate::theme::{self, AMBER, BACKGROUND, DIM, DOWN, GOLD, TEXT, UP, VIOLET};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

/// The smallest screen the desk is drawn on: the compact view below
const MIN_WIDTH: u16 = 100;
const MIN_HEIGHT: u16 = 28;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(BACKGROUND)), area);

    // too small for the desk: the portfolio and its charts only
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        compact::render(frame, app, area);
        return;
    }

    let [header, tape, tiles, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, app, header);
    render_ticker_tape(frame, app, tape);

    if app.all_charts || app.markets {
        if app.markets {
            markets::render(frame, app, tiles.union(body));
        } else {
            charts::render_all(frame, app, tiles.union(body));
        }
        render_footer(frame, app, footer);
        if app.help {
            render_help(frame, area);
        }
        return;
    }

    render_tiles(frame, app, tiles);

    // the tables on two thirds, the charts in a column on the last one
    let [tables, chart_column] =
        Layout::horizontal([Constraint::Percentage(67), Constraint::Percentage(33)]).areas(body);
    charts::render(frame, app, chart_column);

    let [managers, bottom] =
        Layout::vertical([Constraint::Fill(5), Constraint::Fill(4)]).areas(tables);
    panels::render_managers(frame, app, managers);

    let markets = app.model.data.markets.len().max(1) as u16 + 3;
    let [orders, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(bottom);
    let [market, actions] =
        Layout::vertical([Constraint::Length(markets), Constraint::Fill(1)]).areas(right);
    panels::render_orders(frame, app, orders);
    panels::render_markets(frame, app, market);
    panels::render_tape(frame, app, actions);

    render_footer(frame, app, footer);

    if app.help {
        render_help(frame, area);
    }
}

/// The key a tap at (`column`, `row`) of a screen of `area` stands for, where nothing else than a
/// screen can be typed on: the badge of the account and the footer of the desk, the buttons of the
/// compact view
pub fn tap(app: &App, area: Rect, column: u16, row: u16) -> Option<KeyCode> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return compact::tap(app, area, column, row);
    }
    if row == area.y {
        under(&header_left(app), area.x, column)
    } else if row + 1 == area.y + area.height {
        // the status on the right hides the keys under it: none of them
        let status = footer_status(app).width() as u16;
        if column + status >= area.x + area.width {
            return None;
        }
        under(&footer_items(app), area.x, column)
    } else {
        None
    }
}

/// A panel: rounded, amber when it has the keys
pub(crate) fn panel<'a>(title: Line<'a>, focused: bool) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::new().fg(if focused {
            theme::BORDER_FOCUS
        } else {
            theme::BORDER
        }))
        .title(title)
        .style(Style::new().bg(theme::PANEL))
}

/// " TITLE " in amber, then the details in grey
pub(crate) fn title<'a>(name: &str, details: impl IntoIterator<Item = Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(format!(" {name} "), theme::label())];
    spans.extend(details);
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// The left of the header, each span with the key it stands for when tapped: the account shown
fn header_left(app: &App) -> Vec<(Span<'static>, Option<KeyCode>)> {
    let blink = (app.elapsed() / 600).is_multiple_of(2);
    let mut left: Vec<(Span<'static>, Option<KeyCode>)> = vec![
        (
            Span::styled(" ◆ KRAKEN ", Style::new().fg(Color::Black).bg(AMBER).bold()),
            None,
        ),
        (
            Span::styled(
                " TRADING DESK ",
                Style::new().fg(AMBER).bg(theme::SELECTED).bold(),
            ),
            None,
        ),
        (Span::raw(" "), None),
    ];
    left.push((
        match app.model.data.dry_run.map(|it| !it) {
            Some(true) => Span::styled(
                if blink { " ● LIVE " } else { " ○ LIVE " },
                Style::new().fg(Color::Black).bg(UP).bold(),
            ),
            Some(false) => {
                Span::styled(" ◌ DRY RUN ", Style::new().fg(Color::Black).bg(GOLD).bold())
            }
            None => Span::styled(" NO ORDER YET ", theme::dim()),
        },
        None,
    ));
    if let Some(name) = app.view_name() {
        left.push((Span::raw(" "), None));
        left.push((
            Span::styled(
                format!(" ▣ {name} "),
                Style::new().fg(Color::Black).bg(VIOLET).bold(),
            ),
            Some(KeyCode::Char('a')),
        ));
    }
    if let Some(manager) = app.focus() {
        left.push((Span::raw("  "), None));
        left.push((
            Span::styled(
                format!(
                    " ▸ MANAGER {} · {} ",
                    app.manager_name(manager.id),
                    manager.pair
                ),
                Style::new().fg(Color::Black).bg(theme::CYAN).bold(),
            ),
            None,
        ));
    }
    left
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let now = now_millis();
    let blink = (app.elapsed() / 600).is_multiple_of(2);
    let left: Vec<Span> = header_left(app).into_iter().map(|(span, _)| span).collect();
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    let last_tick = app.model.data.managers.iter().map(|it| it.updated_at).max();
    let mut right = Vec::new();
    match app.model.data.forced_tick.as_deref() {
        Some("refresh") => {
            right.push(Span::styled(
                " ⟳ TICK ASKED ",
                Style::new().fg(Color::Black).bg(GOLD).bold(),
            ));
            right.push(Span::raw(" "));
        }
        Some("pending") => {
            right.push(Span::styled(
                if blink {
                    " ⟳ TICKING "
                } else {
                    " ⟲ TICKING "
                },
                Style::new().fg(Color::Black).bg(theme::CYAN).bold(),
            ));
            right.push(Span::raw(" "));
        }
        _ => {}
    }
    if let Some(at) = last_tick {
        let late = now - at;
        let color = match late {
            it if it < 10 * 60_000 => TEXT,
            it if it < 30 * 60_000 => GOLD,
            _ => DOWN,
        };
        right.push(Span::styled("TICK ", theme::dim()));
        right.push(Span::styled(
            format!("{} AGO", format::ago(late)),
            theme::bold(color),
        ));
        if late >= 30 * 60_000 {
            right.push(Span::styled(" STALE", theme::bold(DOWN)));
        }
        right.push(Span::styled(" │ ", theme::faint()));
    }
    right.push(Span::styled("UTC ", theme::dim()));
    right.push(Span::styled(
        format!("{} ", format::date(now)),
        theme::text(),
    ));
    right.push(Span::styled(format::clock(now), theme::bold(AMBER)));
    right.push(Span::raw(" "));
    frame.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        area,
    );
}

/// The prices of the pairs, scrolling from right to left
fn render_ticker_tape(frame: &mut Frame, app: &App, area: Rect) {
    let markets = &app.model.data.markets;
    if markets.is_empty() {
        frame.render_widget(
            Paragraph::new(" NO TICKER YET: THE BOT HASN'T TICKED").style(theme::faint()),
            area,
        );
        return;
    }

    let mut cells: Vec<(char, Style)> = Vec::new();
    let mut push = |text: &str, style: Style| cells.extend(text.chars().map(|it| (it, style)));
    for market in markets {
        let last = to_f64(market.last);
        let change = market
            .indicators
            .map(|it| it.change_24h_percent)
            .or_else(|| {
                let first = market.prices.first()?.1;
                (first > 0.0).then(|| (last - first) / first * 100.0)
            })
            .unwrap_or_default();
        let color = theme::trend(change);
        push(&crate::stats::canonical(&market.key), theme::bold(AMBER));
        push(" ", theme::text());
        push(&format::price(last), theme::bold(TEXT));
        push(" ", theme::text());
        push(
            &format!("{} {}", theme::arrow(change), format::percent(change)),
            theme::bold(color),
        );
        push("     ◆     ", theme::faint());
    }

    // the tape is repeated to fill the line, and moves by one character every 150 ms
    let width = area.width as usize;
    let length = cells.len();
    let offset = (app.elapsed() / 150) as usize % length;
    let line: Vec<Span> = (0..width)
        .map(|index| cells[(offset + index) % length])
        .fold(Vec::<(String, Style)>::new(), |mut spans, (char, style)| {
            match spans.last_mut() {
                Some((text, last)) if *last == style => text.push(char),
                _ => spans.push((char.to_string(), style)),
            }
            spans
        })
        .into_iter()
        .map(|(text, style)| Span::styled(text, style))
        .collect();
    frame.render_widget(
        Paragraph::new(Line::from(line)).style(Style::new().bg(theme::PANEL)),
        area,
    );
}

fn tile(frame: &mut Frame, area: Rect, name: &str, value: Span, detail: Line) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled(format!(" {name} "), theme::label()))
        .style(Style::new().bg(theme::PANEL));
    frame.render_widget(
        Paragraph::new(vec![Line::from(value), detail]).block(block),
        area,
    );
}

fn render_tiles(frame: &mut Frame, app: &App, area: Rect) {
    let totals: Totals = match app.focus() {
        Some(manager) => app.model.totals_of(&[manager.id]),
        None => app.model.totals.first().cloned().unwrap_or_default(),
    };
    let quote = totals.quote.as_str();
    let pnl = to_f64(totals.pnl());
    let realized = to_f64(totals.realized);
    let unrealized = to_f64(totals.unrealized());
    // the first tile says the most
    let mut widths = [Constraint::Fill(2); 10];
    widths[0] = Constraint::Fill(3);
    let areas: [Rect; 10] = Layout::horizontal(widths).areas(area);

    // the whole account, managers or not, when the bot records it
    let account = app
        .focus()
        .is_none()
        .then(|| app.model.account(quote))
        .flatten();
    let (name, value, mut detail) = match account {
        Some(account) => {
            let free = to_f64(app.model.free_cash(account));
            // what the managers count on and the account doesn't have, first: a narrow tile cuts
            // the end
            let missing = to_f64(totals.value - account.value());
            let first = if missing > 0.0 {
                Span::styled(
                    format!("missing {}", format::money(missing)),
                    theme::bold(DOWN),
                )
            } else {
                Span::styled(
                    format!("mgrs {}", format::money(to_f64(totals.value))),
                    theme::dim(),
                )
            };
            (
                "PORTFOLIO",
                account.value(),
                vec![
                    first,
                    Span::styled(
                        format!(" · free {}", format::money(free)),
                        Style::new().fg(if free >= 1.0 { theme::GOLD } else { theme::DIM }),
                    ),
                ],
            )
        }
        None => (
            "EQUITY",
            totals.value,
            vec![Span::styled(
                format!("budget {}", format::money(to_f64(totals.budget))),
                theme::dim(),
            )],
        ),
    };
    // the other quote currencies don't add up with the main one
    if app.focus().is_none() {
        for other in app.model.totals.iter().skip(1) {
            let value = app
                .model
                .account(&other.quote)
                .map_or(other.value, |it| it.value());
            detail.push(Span::styled(
                format!(" + {} {}", format::money(to_f64(value)), other.quote),
                Style::new().fg(theme::VIOLET),
            ));
        }
    }
    tile(
        frame,
        areas[0],
        name,
        Span::styled(
            format!("{} {quote}", format::money(to_f64(value))),
            theme::bold(TEXT),
        ),
        Line::from(detail),
    );
    // every open buy filled, every position sold at its target: what the account would be worth
    let (current, target) = totals.current_and_target(account);
    let gain = to_f64(target - current);
    tile(
        frame,
        areas[1],
        "TARGET",
        Span::styled(
            format!("{} {quote}", format::money(to_f64(target))),
            theme::bold(theme::trend(gain)),
        ),
        Line::from(Span::styled(
            format!(
                "{} · {}",
                format::signed(gain),
                format::percent(percent(target - current, current))
            ),
            Style::new().fg(theme::trend(gain)),
        )),
    );
    tile(
        frame,
        areas[2],
        "P&L",
        Span::styled(format::signed(pnl), theme::bold(theme::trend(pnl))),
        Line::from(Span::styled(
            format!(
                "{} {}",
                theme::arrow(pnl),
                format::percent(totals.pnl_percent())
            ),
            Style::new().fg(theme::trend(pnl)),
        )),
    );
    tile(
        frame,
        areas[3],
        "REALIZED",
        Span::styled(
            format::signed(realized),
            theme::bold(theme::trend(realized)),
        ),
        Line::from(Span::styled(
            format!("{} sells", totals.trades),
            theme::dim(),
        )),
    );
    tile(
        frame,
        areas[4],
        "UNREALIZED",
        Span::styled(
            format::signed(unrealized),
            theme::bold(theme::trend(unrealized)),
        ),
        Line::from(Span::styled(
            format!("exposure {:.0}%", totals.exposure_percent()),
            theme::dim(),
        )),
    );
    // what the sells earned per day since the first order, carried on as it is
    let ids = app.charted();
    let per_day = app.model.realized_per_day(&ids, app.loaded_at);
    for (place, name, days) in [
        (5, "1 MONTH", 30.0),
        (6, "3 MONTHS", 90.0),
        (7, "1 YEAR", 365.0),
    ] {
        let projected = per_day * days;
        tile(
            frame,
            areas[place],
            name,
            Span::styled(
                format::signed(projected),
                theme::bold(theme::trend(projected)),
            ),
            Line::from(Span::styled(
                format!(
                    "{} {}",
                    theme::arrow(projected),
                    format::percent(projected / to_f64(totals.budget).max(1.0) * 100.0)
                ),
                Style::new().fg(theme::trend(projected)),
            )),
        );
    }
    tile(
        frame,
        areas[8],
        "FEES",
        Span::styled(format::money(to_f64(totals.fees)), theme::bold(GOLD)),
        Line::from(Span::styled(
            format!("vol {}", format::money(to_f64(totals.volume))),
            theme::dim(),
        )),
    );
    tile(
        frame,
        areas[9],
        "OPEN ORDERS",
        Span::styled(totals.open_orders.to_string(), theme::bold(theme::CYAN)),
        Line::from(Span::styled(
            format!("{} managers", totals.managers),
            theme::dim(),
        )),
    );
}

fn key<'a>(key: &'a str, name: &'a str) -> [Span<'a>; 2] {
    [
        Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(DIM).bold(),
        ),
        Span::styled(format!(" {name}  "), theme::dim()),
    ]
}

/// The key a label of the footer stands for, tapped: none for those which want a direction
fn key_of(label: &str) -> Option<KeyCode> {
    match label {
        "tab" => Some(KeyCode::Tab),
        "⏎" => Some(KeyCode::Enter),
        single if single.chars().count() == 1 => single.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

/// The keys of the footer and the ranges, each span with the key it stands for when tapped
fn footer_items(app: &App) -> Vec<(Span<'static>, Option<KeyCode>)> {
    let keys: Vec<(&str, &str)> = if app.markets {
        vec![
            ("q", "QUIT"),
            ("d", "DESK"),
            ("←→", "PAIR"),
            ("r", "RELOAD"),
            ("t", "TICK"),
            ("?", "HELP"),
            ("1-5", "RANGE"),
        ]
    } else if app.all_charts {
        vec![
            ("q", "QUIT"),
            ("d", "MARKETS"),
            ("↑↓", "MANAGER"),
            ("⏎", if app.drilled { "ALL" } else { "FOCUS" }),
            ("r", "RELOAD"),
            ("t", "TICK"),
            ("?", "HELP"),
            ("1-5", "RANGE"),
        ]
    } else {
        vec![
            ("q", "QUIT"),
            ("d", "ALL CHARTS"),
            ("tab", "PANEL"),
            ("↑↓", "MOVE"),
            ("←→ s", "SORT"),
            ("⏎", if app.drilled { "ALL" } else { "FOCUS" }),
            ("c v b", "CHARTS"),
            ("o", if app.open_only { "ALL ORDERS" } else { "OPEN" }),
            ("i", if app.show_idle { "HIDE IDLE" } else { "IDLE" }),
            ("r", "RELOAD"),
            ("t", "TICK"),
            ("?", "HELP"),
            ("1-5", "RANGE"),
        ]
    };
    let mut items: Vec<(Span<'static>, Option<KeyCode>)> = Vec::new();
    let push_key = |items: &mut Vec<_>, label: &str, name: &str| {
        let code = key_of(label);
        let [key, name] = key(label, name);
        items.push((Span::styled(key.content.into_owned(), key.style), code));
        items.push((Span::styled(name.content.into_owned(), name.style), code));
    };
    if app.several_accounts() {
        push_key(&mut items, "a", "ACCOUNT");
    }
    for (label, name) in keys {
        push_key(&mut items, label, name);
    }
    for (index, range) in Range::ALL.iter().enumerate() {
        items.push((
            Span::styled(
                format!(" {} ", range.label()),
                if *range == app.range {
                    Style::new().fg(Color::Black).bg(AMBER).bold()
                } else {
                    theme::dim()
                },
            ),
            char::from_digit(index as u32 + 1, 10).map(KeyCode::Char),
        ));
    }
    items
}

/// What's under `column` of a line of spans, each with what it stands for
fn under<T: Copy>(items: &[(Span, Option<T>)], start: u16, column: u16) -> Option<T> {
    let mut left = start;
    for (span, what) in items {
        let width = span.width() as u16;
        if column >= left && column < left + width {
            return *what;
        }
        left += width;
    }
    None
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let spans: Vec<Span> = footer_items(app)
        .into_iter()
        .map(|(span, _)| span)
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    frame.render_widget(
        Paragraph::new(Line::from(footer_status(app))).alignment(Alignment::Right),
        area,
    );
}

/// The right of the footer, over the keys when the screen is narrow: an error, or how long since
/// it was read
fn footer_status(app: &App) -> Span<'static> {
    match &app.error {
        Some(error) => Span::styled(format!("✗ {error} "), theme::bold(DOWN)),
        None => Span::styled(
            format!(
                "● SYNCED {} AGO · v{} ",
                format::ago(now_millis() - app.loaded_at),
                app.model.data.version
            ),
            Style::new().fg(UP).add_modifier(Modifier::DIM),
        ),
    }
}

fn render_help(frame: &mut Frame, area: Rect) {
    let lines = [
        ("tab / ⇧tab", "the panel the arrows move in"),
        ("↑ ↓ j k", "move, PgUp PgDn by 10, g G to the ends"),
        (
            "← → s",
            "the column the table of the panel is sorted on, s the other way",
        ),
        (
            "⏎",
            "focus on the manager of the cursor: tiles, chart, orders, tape",
        ),
        ("esc", "back to all the managers"),
        (
            "c v b",
            "the chart at the top, in the middle, at the bottom: P&L, portfolio, price, sells, if the market moves, growth, by crypto, heatmaps of the pairs and of the accounts",
        ),
        (
            "d",
            "the next screen: every chart, then the markets a pair at a time in the order of the YAML file (← → the pair), then the desk; esc back to the desk",
        ),
        ("1 … 5", "range of the charts: 1H 24H 7D 30D ALL"),
        (
            "a",
            "next account, in a circle: all of them added up, then each one",
        ),
        ("o", "only the open orders"),
        ("i", "the idle decisions in the tape"),
        ("r", "read the database again"),
        (
            "t",
            "a tick of the bot now, without waiting for its interval",
        ),
        ("q", "quit"),
    ];
    let mut text: Vec<Line> = vec![Line::from("")];
    text.extend(lines.iter().map(|(keys, what)| {
        Line::from(vec![
            Span::styled(format!("  {keys:<12}"), theme::bold(AMBER)),
            Span::styled(*what, theme::text()),
        ])
    }));
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        "  Read only: the bot keeps trading. Any key closes this.",
        theme::dim(),
    )));

    let width = 78.min(area.width);
    let height = (text.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text).block(panel(title("HELP", []), true)),
        popup,
    );
}
