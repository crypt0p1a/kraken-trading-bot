//! The P&L over time, and the portfolio if the market moves

use super::{block, block_with, bounds, empty, time_axis, value_axis};
use crate::app::App;
use crate::data::{percent, to_f64};
use crate::format;
use crate::stats::moves;
use crate::theme::{self, AMBER, DOWN, FAINT, UP};
use crate::ui::{panel, title};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Dataset, GraphType, Paragraph, Row, Table};

pub(super) fn render_pnl(frame: &mut Frame, app: &App, area: Rect) {
    let ids = app.charted();
    let quote = ids
        .first()
        .and_then(|id| app.model.quotes.get(id))
        .cloned()
        .unwrap_or_default();
    let series = app.model.pnl_series(&ids);
    if series.len() < 2 {
        let block = block(app, format!("P&L · {quote}"));
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }

    let now = app.loaded_at;
    let since = app.model.data.since.min(series[0].0 as i64);
    let last = series.last().map_or(0.0, |it| it.1);
    let moved = last - series[0].1;
    let color = theme::trend(last);
    let shade = match color {
        UP => theme::UP_SHADE,
        DOWN => theme::DOWN_SHADE,
        _ => FAINT,
    };
    let block = block_with(
        app,
        format!("P&L · {quote}"),
        vec![
            Span::styled(
                format!("{} {} ", theme::arrow(last), format::signed(last)),
                theme::bold(color),
            ),
            Span::styled(
                format!("{} in range", format::signed(moved)),
                Style::new().fg(theme::trend(moved)),
            ),
        ],
    );
    let zero = [(since as f64, 0.0), (now as f64, 0.0)];
    let y = bounds(series.iter().map(|it| it.1).chain([0.0]));

    let datasets = vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Area)
            .fill_to_y(0.0)
            .style(Style::new().fg(shade))
            .data(&series),
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(FAINT))
            .data(&zero),
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(color))
            .data(&series),
    ];
    let chart = ratatui::widgets::Chart::new(datasets)
        .block(block)
        .x_axis(time_axis(since, now))
        .y_axis(value_axis(y, format::signed));
    frame.render_widget(chart, area);
}

pub(super) fn render_scenarios(frame: &mut Frame, app: &App, area: Rect) {
    let ids = app.charted();
    let totals = app.model.totals_of(&ids);
    let block = panel(
        title(&format!("IF THE MARKET MOVES · {}", totals.quote), []),
        false,
    );
    if ids.is_empty() {
        empty(frame, area, block, "NO MANAGER");
        return;
    }
    let account = app
        .focus()
        .is_none()
        .then(|| app.model.account(&totals.quote))
        .flatten();
    // as many moves as the height holds: the header, a blank line and the 3 lines of the note aside
    const NOTE: u16 = 3;
    let rows = block.inner(area).height.saturating_sub(1 + 1 + NOTE);
    let scenarios = app.model.scenarios(&ids, account, &moves(rows as usize));
    let now = scenarios
        .iter()
        .find(|it| it.moved == 0)
        .map_or(totals.value, |it| it.value);
    let largest = scenarios
        .iter()
        .map(|it| to_f64(it.value - now).abs())
        .fold(0.0, f64::max);

    const BAR: usize = 18;
    let at_right = |text: String, style: Style| {
        Cell::from(Line::from(Span::styled(text, style)).right_aligned())
    };
    let rows: Vec<Row> = scenarios
        .iter()
        .map(|scenario| {
            let change = to_f64(scenario.value - now);
            let color = theme::trend(change);
            let length = if largest > 0.0 {
                (change.abs() / largest * BAR as f64).round() as usize
            } else {
                0
            };
            let count = |count: usize| {
                if count == 0 {
                    ("—".to_string(), theme::faint())
                } else {
                    (count.to_string(), theme::text())
                }
            };
            let (buys, buys_style) = count(scenario.buys);
            let (sells, sells_style) = count(scenario.sells);
            let moved = if scenario.moved == 0 {
                "NOW".to_string()
            } else {
                format!("{:+}%", scenario.moved)
            };
            let row = Row::new(vec![
                at_right(moved, theme::bold(AMBER)),
                at_right(
                    format::money(to_f64(scenario.value)),
                    theme::bold(theme::TEXT),
                ),
                at_right(format::signed(change), Style::new().fg(color)),
                at_right(
                    format::percent(percent(scenario.value - now, now)),
                    Style::new().fg(color),
                ),
                at_right(buys, buys_style),
                at_right(sells, sells_style),
                Cell::from(Span::styled("█".repeat(length), Style::new().fg(color))),
            ]);
            if scenario.moved == 0 {
                row.style(Style::new().bg(theme::SELECTED))
            } else {
                row
            }
        })
        .collect();

    let name = |name| Line::from(Span::styled(name, theme::label())).right_aligned();
    let header = Row::new(vec![
        Cell::from(name("MOVE")),
        Cell::from(name("VALUE")),
        Cell::from(name("VS NOW")),
        Cell::from(name("%")),
        Cell::from(name("BUYS")),
        Cell::from(name("SELLS")),
        Cell::from(Line::from(Span::styled("", theme::label()))),
    ]);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [table, note] = Layout::vertical([
        Constraint::Length(scenarios.len() as u16 + 2),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(5),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(8),
                Constraint::Length(5),
                Constraint::Length(5),
                Constraint::Fill(1),
            ],
        )
        .header(header)
        .column_spacing(1),
        table,
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                " Every price moves at once, from the last ones. Not a forecast.",
                theme::dim(),
            )),
            Line::from(Span::styled(
                " Filled: the buys the move reaches, the sells at their target, fees included.",
                theme::dim(),
            )),
            Line::from(Span::styled(
                " The orders the bot would place next aren't played.",
                theme::dim(),
            )),
        ]),
        note,
    );
}
