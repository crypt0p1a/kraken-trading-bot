//! The tables: the managers, the order blotter, the markets, the tape of the decisions

use super::{panel, title};
use crate::app::{App, Panel};
use crate::data::{Action, Order, Side, to_f64};
use crate::format;
use crate::stats::canonical;
use crate::theme::{self, AMBER, CYAN, DIM, DOWN, FAINT, GOLD, TEXT, UP, VIOLET};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

fn right<'a>(text: impl Into<String>, style: Style) -> Cell<'a> {
    Cell::from(Line::from(Span::styled(text.into(), style)).right_aligned())
}

fn left<'a>(text: impl Into<String>, style: Style) -> Cell<'a> {
    Cell::from(Span::styled(text.into(), style))
}

fn header<'a>(names: &[(&'a str, bool)]) -> Row<'a> {
    Row::new(names.iter().map(|(name, at_right)| {
        let line = Line::from(Span::styled(*name, theme::label()));
        Cell::from(if *at_right {
            line.right_aligned()
        } else {
            line
        })
    }))
    .style(Style::new().bg(theme::SELECTED))
}

fn table<'a>(
    rows: Vec<Row<'a>>,
    widths: &[Constraint],
    header: Row<'a>,
    focused: bool,
) -> Table<'a> {
    Table::new(rows, widths.to_vec())
        .header(header)
        .column_spacing(1)
        .row_highlight_style(if focused {
            Style::new()
                .bg(theme::SELECTED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(theme::SELECTED)
        })
        .highlight_symbol(Span::styled(
            if focused { "▶" } else { " " },
            theme::bold(AMBER),
        ))
}

fn empty(frame: &mut Frame, area: Rect, block: ratatui::widgets::Block, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), theme::faint())))
            .centered()
            .block(block),
        area,
    );
}

pub fn render_managers(frame: &mut Frame, app: &App, area: Rect) {
    let model = &app.model;
    let focused = app.panel == Panel::Managers;
    let active = model
        .data
        .managers
        .iter()
        .filter(|it| it.deleted_at.is_none())
        .count();
    let block = panel(
        title(
            "MANAGERS",
            [Span::styled(
                format!("· {active} active · {} total", model.data.managers.len()),
                theme::dim(),
            )],
        ),
        focused,
    );
    if model.data.managers.is_empty() {
        empty(frame, area, block, "NO MANAGER YET: THE BOT HASN'T TICKED");
        return;
    }

    let spark = 10;
    let rows: Vec<Row> = model
        .data
        .managers
        .iter()
        .map(|manager| {
            let deleted = manager.deleted_at.is_some();
            let pnl = to_f64(manager.pnl());
            let last = to_f64(manager.last_price);
            let (base, _) = crate::stats::split(&manager.pair);

            let id = match (manager.template_id, deleted) {
                (Some(_), true) => Span::styled(format!("✕{}", manager.id), theme::faint()),
                (Some(_), false) => {
                    Span::styled(format!("◇{}", manager.id), Style::new().fg(VIOLET))
                }
                (None, _) => Span::styled(format!("#{}", manager.id), theme::bold(TEXT)),
            };
            let mode = if manager.mode == "closing" {
                Span::styled("◐", Style::new().fg(GOLD))
            } else if deleted {
                Span::styled("○", theme::faint())
            } else {
                Span::styled("●", Style::new().fg(UP))
            };

            let position = manager.base();
            let (entry, versus) = match manager.entry_price {
                Some(entry) if !position.is_zero() => {
                    let entry = to_f64(entry);
                    let versus = (last - entry) / entry * 100.0;
                    (
                        right(format::price(entry), Style::new().fg(GOLD)),
                        right(
                            format::percent(versus),
                            Style::new().fg(theme::trend(versus)),
                        ),
                    )
                }
                _ => (right("—", theme::faint()), right("—", theme::faint())),
            };

            let order = model.open_orders_of(manager.id).last();
            let gap = match order.and_then(|it| Some((model.to_fill(it)?, it.side))) {
                Some((percent, side)) => {
                    Cell::from(Line::from(to_fill(percent, side)).right_aligned())
                }
                None => right("—", theme::faint()),
            };
            let order = match order {
                Some(order) => {
                    let (name, color) = side(order.side);
                    let price = order.limit_price.map(to_f64).unwrap_or_default();
                    let mut spans = vec![
                        Span::styled(format!("{name} "), theme::bold(color)),
                        Span::styled(format::price(price), theme::text()),
                    ];
                    if order.filled() > 0.0 {
                        spans.push(Span::styled(
                            format!(" {:.0}%", order.filled() * 100.0),
                            Style::new().fg(CYAN),
                        ));
                    }
                    Cell::from(Line::from(spans))
                }
                None => left("—", theme::faint()),
            };

            let values = model.values_of(manager.id);
            let change = match (values.first(), values.last()) {
                (Some(first), Some(last)) => last - first,
                _ => 0.0,
            };
            let row = Row::new(vec![
                Cell::from(Line::from(vec![mode, Span::raw(" "), id])),
                left(canonical(&manager.pair), theme::bold(AMBER)),
                right(format::money(to_f64(manager.value)), theme::bold(TEXT)),
                right(format::signed(pnl), theme::bold(theme::trend(pnl))),
                right(
                    format::percent(manager.pnl_percent()),
                    Style::new().fg(theme::trend(pnl)),
                ),
                right(
                    if position.is_zero() {
                        "—".to_string()
                    } else {
                        format!("{} {base}", format::volume(to_f64(position)))
                    },
                    if position.is_zero() {
                        theme::faint()
                    } else {
                        theme::text()
                    },
                ),
                entry,
                versus,
                order,
                gap,
                left(
                    format::sparkline(&values, spark),
                    Style::new().fg(if change == 0.0 {
                        DIM
                    } else {
                        theme::trend(change)
                    }),
                ),
            ]);
            if deleted {
                row.style(theme::faint())
            } else {
                row
            }
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(15),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Length(7),
        Constraint::Length(spark as u16),
    ];
    let header = header(&[
        ("  MANAGER", false),
        ("PAIR", false),
        ("VALUE", true),
        ("P&L", true),
        ("P&L %", true),
        ("POSITION", true),
        ("ENTRY", true),
        ("VS ENTRY", true),
        ("OPEN ORDER", false),
        ("TO FILL", true),
        ("TREND", false),
    ]);
    let mut state = TableState::new().with_selected(Some(app.cursor));
    frame.render_stateful_widget(
        table(rows, &widths, header, focused).block(block),
        area,
        &mut state,
    );
}

fn side(side: Side) -> (&'static str, ratatui::style::Color) {
    match side {
        Side::Buy => ("BUY", UP),
        Side::Sell => ("SELL", DOWN),
    }
}

/// How far the market is from an open order: ▼1.41% for a buy the price has to drop to, in gold
/// once it's close, in cyan when the market is already through it
fn to_fill(percent: f64, side: Side) -> Span<'static> {
    let arrow = if percent < 0.0 { "▼" } else { "▲" };
    let text = format!("{arrow}{:.2}%", percent.abs());
    let through = match side {
        Side::Buy => percent >= 0.0,
        Side::Sell => percent <= 0.0,
    };
    let style = if through {
        theme::bold(CYAN)
    } else if percent.abs() < 0.25 {
        theme::bold(GOLD)
    } else {
        theme::dim()
    };
    Span::styled(text, style)
}

/// ██████░░░░ in the color of the side
fn fill_bar(order: &Order, width: usize) -> Line<'static> {
    let filled = order.filled();
    let full = (filled * width as f64).round() as usize;
    let (_, color) = side(order.side);
    Line::from(vec![
        Span::styled("█".repeat(full), Style::new().fg(color)),
        Span::styled("░".repeat(width - full), theme::faint()),
        Span::styled(format!(" {:>3.0}%", filled * 100.0), theme::dim()),
    ])
}

pub fn render_orders(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.panel == Panel::Orders;
    let orders = app.orders();
    let open = orders.iter().filter(|it| !it.is_final).count();
    let mut details = vec![Span::styled(
        format!("· {} · {open} open", orders.len()),
        theme::dim(),
    )];
    if app.open_only {
        details.push(Span::styled(" · OPEN ONLY", theme::bold(CYAN)));
    }
    let block = panel(title("ORDERS", details), focused);
    if orders.is_empty() {
        empty(frame, area, block, "NO ORDER");
        return;
    }

    let rows: Vec<Row> = orders
        .iter()
        .map(|order| {
            let (name, color) = side(order.side);
            let price = order
                .average_price
                .filter(|_| !order.volume_executed.is_zero())
                .or(order.limit_price)
                .map(to_f64)
                .unwrap_or_default();
            let status = match order.status.as_str() {
                "open" => Span::styled("OPEN", theme::bold(CYAN)),
                "pending" => Span::styled("PENDING", theme::bold(GOLD)),
                "closed" => Span::styled("FILLED", Style::new().fg(color)),
                "canceled" => Span::styled("CANCELED", theme::dim()),
                "expired" => Span::styled("EXPIRED", theme::dim()),
                other => Span::styled(other.to_uppercase(), theme::dim()),
            };
            let dead = order.is_final && order.volume_executed.is_zero();
            let row = Row::new(vec![
                left(
                    format::stamp((order.open_time * 1000.0) as i64),
                    theme::dim(),
                ),
                left(format!("#{}", order.manager_id), theme::text()),
                left(name, theme::bold(color)),
                left(canonical(&order.pair), Style::new().fg(AMBER)),
                right(format::price(price), theme::bold(TEXT)),
                match app.model.to_fill(order) {
                    Some(percent) => {
                        Cell::from(Line::from(to_fill(percent, order.side)).right_aligned())
                    }
                    None => right("—", theme::faint()),
                },
                right(format::volume(to_f64(order.volume)), theme::text()),
                Cell::from(fill_bar(order, 6)),
                if order.volume_executed.is_zero() {
                    right("—", theme::faint())
                } else {
                    right(format::money(to_f64(order.cost)), theme::text())
                },
                if order.volume_executed.is_zero() {
                    right("—", theme::faint())
                } else {
                    right(format::money(to_f64(order.fee)), Style::new().fg(GOLD))
                },
                Cell::from(status),
            ]);
            if dead {
                row.style(Style::new().fg(FAINT))
            } else {
                row
            }
        })
        .collect();

    let widths = [
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(4),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Fill(1),
    ];
    let header = header(&[
        ("  OPENED", false),
        ("MGR", false),
        ("SIDE", false),
        ("PAIR", false),
        ("PRICE", true),
        ("TO FILL", true),
        ("VOLUME", true),
        ("FILLED", false),
        ("COST", true),
        ("FEE", true),
        ("STATUS", false),
    ]);
    let mut state = TableState::new().with_selected(Some(app.orders_row.min(orders.len() - 1)));
    frame.render_stateful_widget(
        table(rows, &widths, header, focused).block(block),
        area,
        &mut state,
    );
}

pub fn render_markets(frame: &mut Frame, app: &App, area: Rect) {
    let markets = &app.model.data.markets;
    let block = panel(
        title("MARKETS", [Span::styled("· 24H", theme::dim())]),
        false,
    );
    if markets.is_empty() {
        empty(frame, area, block, "NO TICKER YET");
        return;
    }
    let selected = app
        .selected()
        .and_then(|it| app.model.markets.get(&it.id))
        .copied();

    let rows: Vec<Row> = markets
        .iter()
        .enumerate()
        .map(|(index, market)| {
            let last = to_f64(market.last);
            let mid = (to_f64(market.bid) + to_f64(market.ask)) / 2.0;
            let spread = if mid > 0.0 {
                (to_f64(market.ask) - to_f64(market.bid)) / mid * 10_000.0
            } else {
                0.0
            };
            let (change, atr, range, trend) = match market.indicators {
                Some(it) => (
                    right(
                        format!(
                            "{} {}",
                            theme::arrow(it.change_24h_percent),
                            format::percent(it.change_24h_percent)
                        ),
                        theme::bold(theme::trend(it.change_24h_percent)),
                    ),
                    right(format!("{:.2}%", it.atr_percent), theme::text()),
                    right(format!("{:.2}%", it.range_24h_percent), theme::text()),
                    match it.trend() {
                        1 => left("▲ RISING", theme::bold(UP)),
                        -1 => left("▼ FALLING", theme::bold(DOWN)),
                        _ => left("◆ MIXED", theme::bold(GOLD)),
                    },
                ),
                None => (
                    right("—", theme::faint()),
                    right("—", theme::faint()),
                    right("—", theme::faint()),
                    left("—", theme::faint()),
                ),
            };
            let pair = canonical(&market.key);
            let row = Row::new(vec![
                left(pair, theme::bold(AMBER)),
                right(format::price(last), theme::bold(TEXT)),
                change,
                atr,
                range,
                right(format!("{spread:.1}"), theme::dim()),
                trend,
            ]);
            if Some(index) == selected {
                row.style(Style::new().bg(theme::SELECTED))
            } else {
                row
            }
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Fill(1),
    ];
    let header = header(&[
        ("PAIR", false),
        ("LAST", true),
        ("24H", true),
        ("ATR", true),
        ("RANGE", true),
        ("BPS", true),
        ("TREND", false),
    ]);
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .block(block),
        area,
    );
}

/// What an action says, in a few words
fn describe(action: &Action) -> (Span<'static>, Line<'static>) {
    let amount_at_price = || {
        let amount = action.amount.map(to_f64).unwrap_or_default();
        let price = action.price.map(to_f64).unwrap_or_default();
        format!("{} @ {}", format::volume(amount), format::price(price))
    };
    let reason = || action.reason.clone().unwrap_or_default();

    match action.kind.as_str() {
        kind @ ("buy" | "sell") => {
            let color = if kind == "buy" { UP } else { DOWN };
            let mut spans = vec![Span::styled(amount_at_price(), theme::text())];
            if let Some(error) = &action.error {
                spans.push(Span::styled(format!("  ✗ {error}"), theme::bold(DOWN)));
            } else if action.dry_run {
                spans.push(Span::styled("  DRY", theme::bold(GOLD)));
            } else if let Some(txid) = &action.txid {
                spans.push(Span::styled(format!("  ✓ {txid}"), Style::new().fg(UP)));
            }
            (
                Span::styled(kind.to_uppercase(), theme::bold(color)),
                Line::from(spans),
            )
        }
        "buy_cancelled" => (
            Span::styled("TRAIL", theme::bold(VIOLET)),
            Line::from(vec![
                Span::styled("buy cancelled, placed again higher ", theme::text()),
                match &action.error {
                    Some(error) => Span::styled(format!("✗ {error}"), theme::bold(DOWN)),
                    None => Span::styled(reason(), theme::dim()),
                },
            ]),
        ),
        "virtual_created" => (
            Span::styled("+VIRT", theme::bold(VIOLET)),
            Line::from(Span::styled(reason(), theme::text())),
        ),
        "virtual_deleted" => (
            Span::styled("-VIRT", theme::bold(VIOLET)),
            Line::from(Span::styled(reason(), theme::dim())),
        ),
        "idle" => (
            Span::styled("IDLE", theme::faint()),
            Line::from(Span::styled(reason(), theme::faint())),
        ),
        other => (
            Span::styled(other.to_uppercase(), theme::dim()),
            Line::from(Span::styled(
                action.error.clone().unwrap_or_else(reason),
                if action.error.is_some() {
                    theme::bold(DOWN)
                } else {
                    theme::dim()
                },
            )),
        ),
    }
}

pub fn render_tape(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.panel == Panel::Tape;
    let actions = app.actions();
    let errors = actions.iter().filter(|it| it.error.is_some()).count();
    let mut details = vec![Span::styled(format!("· {}", actions.len()), theme::dim())];
    if errors > 0 {
        details.push(Span::styled(format!(" · {errors} ✗"), theme::bold(DOWN)));
    }
    if !app.show_idle {
        details.push(Span::styled(" · idle hidden", theme::faint()));
    }
    let block = panel(title("TAPE", details), focused);
    if actions.is_empty() {
        empty(frame, area, block, "NO DECISION YET");
        return;
    }

    let rows: Vec<Row> = actions
        .iter()
        .map(|action| {
            let (kind, detail) = describe(action);
            Row::new(vec![
                left(format::stamp(action.at), theme::dim()),
                left(format!("#{}", action.manager_id), theme::text()),
                Cell::from(kind),
                Cell::from(detail),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(5),
        Constraint::Fill(1),
    ];
    let header = header(&[
        ("  AT", false),
        ("MGR", false),
        ("WHAT", false),
        ("", false),
    ]);
    let mut state = TableState::new().with_selected(Some(app.tape_row.min(actions.len() - 1)));
    frame.render_stateful_widget(
        table(rows, &widths, header, focused).block(block),
        area,
        &mut state,
    );
}
