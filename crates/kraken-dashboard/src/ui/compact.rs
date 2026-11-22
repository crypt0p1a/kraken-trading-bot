//! The dashboard on a screen too small for the desk (100 by 28 cells): a phone, a terminal too
//! small. What the portfolio is worth and how it moves, nothing to move through
//!
//! ```text
//! ◆ KRAKEN ● LIVE              TICK 2m AGO
//! ┌ACCOUNT───────────┐┌RANGE─────────────┐   tapped: the next account, the next range
//! │▸ ALL ACCOUNTS · 2││▸ 24H             │
//! ┌PORTFOLIO─────────┐┌P&L───────────────┐
//! │595.83 EUR        ││-141.79           │
//! ┌REALIZED──────────┐┌TARGET────────────┐
//! ┌PORTFOLIO · EUR · 24H─────────────────┐
//! ┌P&L · EUR · 24H───────────────────────┐
//! ┌BY CRYPTO · 24H───────────────────────┐
//! ```
//!
//! On a wide and low screen, a phone held sideways, the tiles on a row and the charts side by side.

use super::charts::render_chart;
use super::tile;
use crate::app::{App, Chart, now_millis};
use crate::data::{percent, to_f64};
use crate::format;
use crate::keys::KeyCode;
use crate::theme::{self, AMBER, DOWN, GOLD, TEXT, UP};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Smaller still, there's nothing to read
pub const MIN_WIDTH: u16 = 30;
pub const MIN_HEIGHT: u16 = 12;

/// The tiles on a row from this width, in two rows of two below
const TILES_ON_A_ROW: u16 = 80;
/// The height of a row of tiles: a border, a value, a detail, a border
const TILE_HEIGHT: u16 = 4;
/// The height of the buttons: high enough to be tapped
const BUTTON_HEIGHT: u16 = 3;
/// What a chart wants at least, in rows
const CHART_HEIGHT: u16 = 8;
/// A chart is at least this wide, the charts side by side
const CHART_WIDTH: u16 = 40;

/// The charts, the most telling first: as many as the screen holds
const CHARTS: [Chart; 3] = [Chart::Portfolio, Chart::Pnl, Chart::Allocation];

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let message = Paragraph::new(vec![
            Line::from("◆ KRAKEN").style(theme::label()),
            Line::from(format!("{}×{}", area.width, area.height)).style(theme::dim()),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(message, area);
        return;
    }
    let [header, buttons, tiles, charts] = layout(area);
    render_header(frame, app, header);
    for (place, button) in buttons_of(app, buttons) {
        super::tile(
            frame,
            place,
            button.title,
            Span::styled(format!("▸ {}", button.label), theme::bold(AMBER)),
            Line::from(""),
        );
    }
    render_tiles(frame, app, tiles, area.width >= TILES_ON_A_ROW);
    render_charts(frame, app, charts);
}

/// The header, the buttons, the tiles, the charts
fn layout(area: Rect) -> [Rect; 4] {
    let tiles_height = if area.width >= TILES_ON_A_ROW {
        TILE_HEIGHT
    } else {
        2 * TILE_HEIGHT
    };
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(BUTTON_HEIGHT),
        Constraint::Length(tiles_height),
        Constraint::Fill(1),
    ])
    .areas(area)
}

/// What a button does when tapped, as the key would
struct Button {
    title: &'static str,
    label: String,
    key: KeyCode,
}

/// The buttons and their places: the next account when there are several, the next range
fn buttons_of(app: &App, area: Rect) -> Vec<(Rect, Button)> {
    let mut buttons = Vec::new();
    if let Some(name) = app.view_name() {
        buttons.push(Button {
            title: "ACCOUNT",
            label: name,
            key: KeyCode::Char('a'),
        });
    }
    let index = crate::app::Range::ALL
        .iter()
        .position(|it| *it == app.range)
        .unwrap_or(0);
    let next = (index + 1) % crate::app::Range::ALL.len();
    buttons.push(Button {
        title: "RANGE",
        label: app.range.label().to_string(),
        key: KeyCode::Char(char::from_digit(next as u32 + 1, 10).unwrap_or('1')),
    });
    let places = Layout::horizontal(vec![Constraint::Fill(1); buttons.len()]).split(area);
    places.iter().copied().zip(buttons).collect()
}

/// The key a tap at (`column`, `row`) stands for: a button
pub fn tap(app: &App, area: Rect, column: u16, row: u16) -> Option<KeyCode> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let [_, buttons, _, _] = layout(area);
    let at = ratatui::layout::Position::new(column, row);
    buttons_of(app, buttons)
        .into_iter()
        .find(|(place, _)| place.contains(at))
        .map(|(_, button)| button.key)
}

/// LIVE or DRY RUN, the account shown, how long since the bot ticked
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![
        Span::styled(" ◆ KRAKEN ", Style::new().fg(Color::Black).bg(AMBER).bold()),
        Span::raw(" "),
    ];
    left.push(match app.model.data.dry_run.map(|it| !it) {
        Some(true) => Span::styled(" ● LIVE ", Style::new().fg(Color::Black).bg(UP).bold()),
        Some(false) => Span::styled(" ◌ DRY RUN ", Style::new().fg(Color::Black).bg(GOLD).bold()),
        None => Span::styled(" NO ORDER YET ", theme::dim()),
    });
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    let last_tick = app.model.data.managers.iter().map(|it| it.updated_at).max();
    let right = match last_tick {
        Some(at) => {
            let late = now_millis() - at;
            let color = match late {
                it if it < 10 * 60_000 => TEXT,
                it if it < 30 * 60_000 => GOLD,
                _ => DOWN,
            };
            Line::from(vec![
                Span::styled("TICK ", theme::dim()),
                Span::styled(format!("{} AGO ", format::ago(late)), theme::bold(color)),
            ])
        }
        None => Line::from(Span::styled("NO TICK YET ", theme::dim())),
    };
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
}

/// What the portfolio is worth, its P&L, what the sells made, and what it would be worth at its
/// targets: as the tiles of the desk say it
fn render_tiles(frame: &mut Frame, app: &App, area: Rect, wide: bool) {
    let totals = app.model.totals.first().cloned().unwrap_or_default();
    let quote = totals.quote.as_str();
    let account = app.model.account(quote);
    let value = account.map_or(totals.value, |it| it.value());
    let pnl = to_f64(totals.pnl());
    let realized = to_f64(totals.realized);
    let (current, target) = totals.current_and_target(account);
    let gain = to_f64(target - current);

    let places: Vec<Rect> = if wide {
        Layout::horizontal([Constraint::Fill(1); 4])
            .split(area)
            .to_vec()
    } else {
        let [top, bottom] = Layout::vertical([Constraint::Length(TILE_HEIGHT); 2]).areas(area);
        [top, bottom]
            .into_iter()
            .flat_map(|row| {
                Layout::horizontal([Constraint::Fill(1); 2])
                    .split(row)
                    .to_vec()
            })
            .collect()
    };
    tile(
        frame,
        places[0],
        if account.is_some() {
            "PORTFOLIO"
        } else {
            "EQUITY"
        },
        Span::styled(
            format!("{} {quote}", format::money(to_f64(value))),
            theme::bold(TEXT),
        ),
        Line::from(Span::styled(
            format!("budget {}", format::money(to_f64(totals.budget))),
            theme::dim(),
        )),
    );
    tile(
        frame,
        places[1],
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
        places[2],
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
        places[3],
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
}

/// As many charts as the screen holds, one above the other, or side by side on a wide and low one
fn render_charts(frame: &mut Frame, app: &App, area: Rect) {
    let stacked = (area.height / CHART_HEIGHT).clamp(1, CHARTS.len() as u16);
    let beside = (area.width / CHART_WIDTH).clamp(1, CHARTS.len() as u16);
    // side by side when they'd be flat one above the other: a cell is twice as high as it's wide
    let flat = area.width >= 3 * area.height;
    let (count, places) = if beside > stacked || (beside == stacked && flat) {
        let count = beside as usize;
        (
            count,
            Layout::horizontal(vec![Constraint::Fill(1); count]).split(area),
        )
    } else {
        let count = stacked as usize;
        (
            count,
            Layout::vertical(vec![Constraint::Fill(1); count]).split(area),
        )
    };
    for (place, chart) in places.iter().zip(CHARTS).take(count) {
        render_chart(frame, app, *place, chart);
    }
}
