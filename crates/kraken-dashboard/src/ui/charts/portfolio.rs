//! The portfolio over time, its target, and the P&L of the sells above the curve

use super::flags::{flags_line, sell_flags};
use super::{block, block_with, bounds, empty, level};
use crate::app::App;
use crate::data::{Side, to_f64};
use crate::format;
use crate::theme::{self, DOWN, FAINT, GOLD, UP};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as Segment, Points};
use ratatui::widgets::{Block, Borders, Paragraph};

/// The value at a time: the last point before it, the first one before them all
fn value_at(series: &[(f64, f64)], at: f64) -> f64 {
    series
        .iter()
        .take_while(|it| it.0 <= at)
        .last()
        .or(series.first())
        .map_or(0.0, |it| it.1)
}

pub(super) fn render_portfolio(frame: &mut Frame, app: &App, area: Rect) {
    let ids = app.charted();
    let totals = app.model.totals_of(&ids);
    let quote = totals.quote.clone();
    // the whole account, or the manager focused on
    let account = app
        .focus()
        .is_none()
        .then(|| app.model.account(&quote))
        .flatten();
    let (name, series) = match app.focus() {
        Some(manager) => (
            format!("MANAGER {} · {quote}", app.manager_name(manager.id)),
            app.model.manager_series(manager.id),
        ),
        None => (
            format!("PORTFOLIO · {quote}"),
            app.model.account_series(&quote),
        ),
    };
    if series.len() < 2 {
        let block = block(app, name);
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }

    let now = app.loaded_at;
    let since = app.model.data.since.min(series[0].0 as i64);
    let (current, target) = totals.current_and_target(account);
    let (current, target) = (to_f64(current), to_f64(target));
    let moved = current - series[0].1;

    // what happened in the range: the buys filled, the sells and what they earned
    let buys: Vec<(f64, f64)> = app
        .model
        .data
        .orders
        .iter()
        .filter(|it| {
            it.side == Side::Buy
                && it.is_final
                && !it.volume_executed.is_zero()
                && ids.contains(&it.manager_id)
        })
        .map(|it| it.close_time.unwrap_or(it.open_time) * 1000.0)
        .filter(|at| *at >= since as f64)
        .map(|at| (at, value_at(&series, at)))
        .collect();
    let sells: Vec<(f64, f64)> = app
        .model
        .trades
        .iter()
        .filter(|it| ids.contains(&it.manager_id) && it.at * 1000.0 >= since as f64)
        .map(|it| (it.at * 1000.0, to_f64(it.pnl)))
        .collect();
    let earned: f64 = sells.iter().map(|it| it.1).sum();

    let y = bounds(series.iter().map(|it| it.1).chain([target]));
    let color = theme::trend(moved);
    let shade = match color {
        UP => theme::UP_SHADE,
        DOWN => theme::DOWN_SHADE,
        _ => FAINT,
    };
    let block = block_with(
        app,
        name,
        vec![
            Span::styled(format!("{} ", format::money(current)), theme::bold(color)),
            Span::styled(format::signed(moved), Style::new().fg(color)),
        ],
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [flags_row, middle, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [labels, plot] =
        Layout::horizontal([Constraint::Length(LABELS), Constraint::Fill(1)]).areas(middle);

    // the values at the left, the times and the counts below
    let height = labels.height as usize;
    let mut values = vec![Line::from(""); height];
    for (row, value) in [
        (0, y[1]),
        (height / 2, (y[0] + y[1]) / 2.0),
        (height.saturating_sub(1), y[0]),
    ] {
        if let Some(line) = values.get_mut(row) {
            *line = Line::from(Span::styled(format::money(value), theme::dim()));
        }
    }
    frame.render_widget(Paragraph::new(values), labels);

    let moment = |at: i64| {
        if now - since < 24 * 3_600_000 {
            format::clock(at)[..5].to_string()
        } else {
            format::stamp(at)
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(moment(since), theme::dim())),
        footer,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(moment(now), theme::dim())).alignment(Alignment::Right),
        footer,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("▲ {} BUYS  ", buys.len()), Style::new().fg(UP)),
            Span::styled(format!("▼ {} SELLS ", sells.len()), Style::new().fg(DOWN)),
            Span::styled(format::signed(earned), theme::bold(theme::trend(earned))),
        ]))
        .alignment(Alignment::Center),
        footer,
    );

    let columns = plot.width.saturating_sub(1) as usize;
    // on a row of their own, at whole columns: never redrawn elsewhere, nor over the curve
    let flags = sell_flags(&sells, since as f64, now as f64, columns);
    let [_, flags_area] =
        Layout::horizontal([Constraint::Length(LABELS + 1), Constraint::Fill(1)]).areas(flags_row);
    frame.render_widget(Paragraph::new(flags_line(&flags)), flags_area);
    let span = (now - since).max(1) as f64;
    let (start, stop) = (since as f64, now as f64);
    let target_line = level(since, now, target);
    let dots = columns * 2;
    let canvas = Canvas::default()
        .block(
            Block::new()
                .borders(Borders::LEFT)
                .border_style(Style::new().fg(FAINT)),
        )
        .background_color(theme::PANEL)
        .marker(Marker::Braille)
        .x_bounds([start, stop])
        .y_bounds(y)
        .paint(|context| {
            // the area below the curve, a column of dots at a time
            for dot in 0..dots {
                let at = start + span * dot as f64 / dots.saturating_sub(1).max(1) as f64;
                if let Some(value) = interpolated(&series, at) {
                    context.draw(&Segment {
                        x1: at,
                        y1: y[0],
                        x2: at,
                        y2: value,
                        color: shade,
                    });
                }
            }
            context.layer();
            context.draw(&Points {
                coords: &target_line,
                color: GOLD,
            });
            for (at, _) in &sells {
                let coords: Vec<(f64, f64)> = (0..=24)
                    .map(|step| (*at, y[0] + (y[1] - y[0]) * step as f64 / 24.0))
                    .collect();
                context.draw(&Points {
                    coords: &coords,
                    color: DOWN,
                });
            }
            for pair in series.windows(2) {
                context.draw(&Segment {
                    x1: pair[0].0,
                    y1: pair[0].1,
                    x2: pair[1].0,
                    y2: pair[1].1,
                    color,
                });
            }
            context.layer();
            context.print(
                start,
                target,
                Span::styled(
                    format!("TARGET {}", format::money(target)),
                    Style::new().fg(GOLD),
                ),
            );
            for (at, value) in &buys {
                context.print(*at, *value, Span::styled("▲", Style::new().fg(UP)));
            }
            for (at, _) in &sells {
                context.print(
                    *at,
                    value_at(&series, *at),
                    Span::styled("▼", Style::new().fg(DOWN)),
                );
            }
        });
    frame.render_widget(canvas, plot);
}

/// The value at a time, between the points around it. None outside of the series
fn interpolated(series: &[(f64, f64)], at: f64) -> Option<f64> {
    let after = series.iter().position(|it| it.0 >= at)?;
    if after == 0 {
        return (series[0].0 == at).then_some(series[0].1);
    }
    let (before, next) = (series[after - 1], series[after]);
    let part = (at - before.0) / (next.0 - before.0).max(1.0);
    Some(before.1 + (next.1 - before.1) * part)
}

/// The width of the values at the left of the curve
pub(super) const LABELS: u16 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_between_the_points_only() {
        let series = [(10.0, 100.0), (20.0, 200.0)];
        assert_eq!(interpolated(&series, 15.0), Some(150.0));
        assert_eq!(interpolated(&series, 10.0), Some(100.0));
        assert_eq!(interpolated(&series, 9.0), None);
        assert_eq!(interpolated(&series, 21.0), None);
    }
}
