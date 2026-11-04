//! The chart, one of five: the P&L over time, the portfolio, the price with the levels of the
//! orders, the P&L of each sell, the portfolio if the market moves

use super::{panel, title};
use crate::app::{App, Chart};
use crate::data::{Side, percent, to_f64};
use crate::format;
use crate::stats::canonical;
use crate::theme::{self, AMBER, CYAN, DOWN, FAINT, GOLD, UP};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as Segment, Points};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Dataset, GraphType, LegendPosition, Paragraph, Row, Table,
};

fn block(app: &App, name: String) -> Block<'static> {
    block_with(app, name, Vec::new())
}

/// The chart, its range, what it sums up
fn block_with(app: &App, name: String, summary: Vec<Span<'static>>) -> Block<'static> {
    let mut details = vec![Span::styled(
        format!("· {} ", app.range.label()),
        theme::dim(),
    )];
    details.extend(summary);
    panel(title(&name, details), false)
}

fn empty(frame: &mut Frame, area: Rect, block: Block, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), theme::faint())))
            .centered()
            .block(block),
        area,
    );
}

/// The bounds of the values, a little wider than them
fn bounds(values: impl Iterator<Item = f64>) -> [f64; 2] {
    let (low, high) = values.fold((f64::MAX, f64::MIN), |(low, high), it| {
        (low.min(it), high.max(it))
    });
    if low > high {
        return [0.0, 1.0];
    }
    let margin = ((high - low) * 0.08).max(high.abs() * 0.0005).max(1e-9);
    [low - margin, high + margin]
}

fn time_axis(since: i64, now: i64) -> Axis<'static> {
    let short = now - since < 24 * 3_600_000;
    let label = |at: i64| {
        Span::styled(
            if short {
                format::clock(at)[..5].to_string()
            } else {
                format::stamp(at)
            },
            theme::dim(),
        )
    };
    Axis::default()
        .style(Style::new().fg(FAINT))
        .bounds([since as f64, now as f64])
        .labels(vec![label(since), label((since + now) / 2), label(now)])
}

fn value_axis(bounds: [f64; 2], show: fn(f64) -> String) -> Axis<'static> {
    let middle = (bounds[0] + bounds[1]) / 2.0;
    Axis::default()
        .style(Style::new().fg(FAINT))
        .bounds(bounds)
        .labels(
            [bounds[0], middle, bounds[1]]
                .map(|it| Span::styled(show(it), theme::dim()))
                .to_vec(),
        )
}

/// A price drawn across the chart: its name, its color, its points
type Level = (String, Color, Vec<(f64, f64)>);

/// A level, as a dotted line across the chart
fn level(since: i64, now: i64, value: f64) -> Vec<(f64, f64)> {
    let step = ((now - since) / 60).max(1);
    (since..=now)
        .step_by(step as usize)
        .map(|at| (at as f64, value))
        .collect()
}

/// The height each chart needs to be readable: fewer charts in a low terminal
const CHART_HEIGHT: u16 = 12;

/// The charts of the column, one above the other
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let count = (area.height / CHART_HEIGHT).clamp(1, app.charts.len() as u16) as usize;
    let places = Layout::vertical(vec![Constraint::Fill(1); count]).split(area);
    for (place, chart) in places.iter().zip(app.charts) {
        render_chart(frame, app, *place, chart);
    }
}

fn render_chart(frame: &mut Frame, app: &App, area: Rect, chart: Chart) {
    match chart {
        Chart::Pnl => render_pnl(frame, app, area),
        Chart::Portfolio => render_portfolio(frame, app, area),
        Chart::Price => render_price(frame, app, area),
        Chart::Trades => render_trades(frame, app, area),
        Chart::Scenarios => render_scenarios(frame, app, area),
    }
}

fn render_pnl(frame: &mut Frame, app: &App, area: Rect) {
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

fn render_scenarios(frame: &mut Frame, app: &App, area: Rect) {
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
    let scenarios = app.model.scenarios(&ids, account);
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
    let [table, note] = Layout::vertical([Constraint::Length(9), Constraint::Fill(1)]).areas(inner);
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

/// The value at a time: the last point before it, the first one before them all
fn value_at(series: &[(f64, f64)], at: f64) -> f64 {
    series
        .iter()
        .take_while(|it| it.0 <= at)
        .last()
        .or(series.first())
        .map_or(0.0, |it| it.1)
}

fn render_portfolio(frame: &mut Frame, app: &App, area: Rect) {
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
            format!("MANAGER #{} · {quote}", manager.id),
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
const LABELS: u16 = 10;

/// What a sell says above the curve
#[derive(Debug, PartialEq)]
struct Flag {
    column: usize,
    text: String,
    pnl: f64,
}

/// The flags at their columns
fn flags_line(flags: &[Flag]) -> Line<'static> {
    let mut spans = Vec::new();
    let mut end = 0;
    for flag in flags {
        spans.push(Span::raw(" ".repeat(flag.column.saturating_sub(end))));
        spans.push(Span::styled(
            flag.text.clone(),
            theme::bold(theme::trend(flag.pnl)),
        ));
        end = flag.column + flag.text.chars().count();
    }
    Line::from(spans)
}

/// A flag per sell (time, P&L), the oldest first, at its column in the width of the chart. Sells
/// too close for their texts to fit share a flag, "3× +1.02", which never covers the one before
fn sell_flags(sells: &[(f64, f64)], since: f64, now: f64, columns: usize) -> Vec<Flag> {
    /// Sells sharing a flag, and the column of the first one
    struct Group {
        wanted: usize,
        count: usize,
        pnl: f64,
    }
    impl Group {
        fn text(&self) -> String {
            if self.count > 1 {
                format!("{}× {}", self.count, format::signed(self.pnl))
            } else {
                format::signed(self.pnl)
            }
        }
        fn column(&self, columns: usize) -> usize {
            self.wanted
                .min(columns.saturating_sub(self.text().chars().count()))
        }
    }

    let span = (now - since).max(1.0);
    let mut groups: Vec<Group> = Vec::new();
    for (at, pnl) in sells {
        groups.push(Group {
            wanted: ((at - since) / span * columns.saturating_sub(1) as f64) as usize,
            count: 1,
            pnl: *pnl,
        });
        // a flag which doesn't fit after the one before joins it, which may then not fit either
        while let [.., before, last] = groups.as_slice() {
            if last.column(columns) > before.column(columns) + before.text().chars().count() {
                break;
            }
            let last = groups.pop().unwrap();
            let before = groups.last_mut().unwrap();
            before.count += last.count;
            before.pnl += last.pnl;
        }
    }
    groups
        .iter()
        .map(|it| Flag {
            column: it.column(columns),
            text: it.text(),
            pnl: it.pnl,
        })
        .collect()
}

fn render_price(frame: &mut Frame, app: &App, area: Rect) {
    let Some(manager) = app.selected() else {
        empty(frame, area, block(app, "PRICE".into()), "NO MANAGER");
        return;
    };
    let pair = canonical(&manager.pair);
    let block = block(app, format!("{pair} · #{}", manager.id));
    let Some(market) = app.model.market_of(manager.id) else {
        empty(frame, area, block, "NO TICKER FOR THIS PAIR");
        return;
    };
    let prices: Vec<(f64, f64)> = market
        .prices
        .iter()
        .map(|(at, price)| (*at as f64, *price))
        .collect();
    if prices.len() < 2 {
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }

    let now = app.loaded_at;
    let since = app.model.data.since.min(prices[0].0 as i64);

    // the levels of the manager: what its base cost, its open orders
    let mut levels: Vec<Level> = Vec::new();
    if let Some(entry) = manager.entry_price.filter(|_| !manager.base().is_zero()) {
        let entry = to_f64(entry);
        levels.push((
            format!("ENTRY {}", format::price(entry)),
            GOLD,
            level(since, now, entry),
        ));
    }
    for order in app.model.open_orders_of(manager.id) {
        if let Some(limit) = order.limit_price.map(to_f64) {
            let (name, color) = match order.side {
                Side::Buy => ("BUY", UP),
                Side::Sell => ("SELL", DOWN),
            };
            levels.push((
                format!("{name} {}", format::price(limit)),
                color,
                level(since, now, limit),
            ));
        }
    }

    let y = bounds(
        prices.iter().map(|it| it.1).chain(
            levels
                .iter()
                .filter_map(|it| it.2.first().map(|point| point.1)),
        ),
    );
    let last = prices.last().map_or(0.0, |it| it.1);
    let change = (last - prices[0].1) / prices[0].1 * 100.0;

    let mut datasets = vec![
        Dataset::default()
            .name(Line::from(vec![
                Span::styled(
                    format!("{pair} {} ", format::price(last)),
                    theme::bold(CYAN),
                ),
                Span::styled(
                    format!("{} {}", theme::arrow(change), format::percent(change)),
                    theme::bold(theme::trend(change)),
                ),
            ]))
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(CYAN))
            .data(&prices),
    ];
    for (name, color, points) in &levels {
        datasets.push(
            Dataset::default()
                .name(Line::from(Span::styled(
                    name.clone(),
                    Style::new().fg(*color),
                )))
                .marker(Marker::Dot)
                .graph_type(GraphType::Scatter)
                .style(Style::new().fg(*color))
                .data(points),
        );
    }

    let chart = ratatui::widgets::Chart::new(datasets)
        .block(block)
        .x_axis(time_axis(since, now))
        .y_axis(value_axis(y, format::price))
        .legend_position(Some(LegendPosition::TopLeft))
        .hidden_legend_constraints((
            ratatui::layout::Constraint::Percentage(90),
            ratatui::layout::Constraint::Percentage(90),
        ));
    frame.render_widget(chart, area);
}

fn render_trades(frame: &mut Frame, app: &App, area: Rect) {
    let ids = app.charted();
    let since = app.model.data.since as f64 / 1000.0;
    let trades: Vec<_> = app
        .model
        .trades
        .iter()
        .filter(|it| ids.contains(&it.manager_id) && it.at >= since)
        .collect();
    // the columns of the plot, without the borders and the labels of the values
    let columns = area.width.saturating_sub(12).max(1) as usize;
    // as many bars as the width holds, the most recent, 2 columns each at least
    let trades = &trades[trades.len().saturating_sub(columns / 2)..];
    if trades.is_empty() {
        empty(
            frame,
            area,
            block(app, "SELLS".into()),
            "NO SELL IN THE RANGE",
        );
        return;
    }

    let total: f64 = trades.iter().map(|it| to_f64(it.pnl)).sum();
    let average = total / trades.len() as f64;
    let wins = trades
        .iter()
        .filter(|it| it.pnl.is_sign_positive() && !it.pnl.is_zero())
        .count();
    let block = block_with(
        app,
        "SELLS".into(),
        vec![
            Span::styled(format!("{} ", trades.len()), theme::text()),
            Span::styled(format!("{wins}W "), Style::new().fg(UP)),
            Span::styled(format!("{}L ", trades.len() - wins), Style::new().fg(DOWN)),
            Span::styled(
                format!("Σ {} ", format::signed(total)),
                theme::bold(theme::trend(total)),
            ),
            Span::styled(format!("avg {}", format::signed(average)), theme::dim()),
        ],
    );

    // a bar is a vertical line per column it covers: 60% of the room of a sell
    let per_trade = columns as f64 / trades.len() as f64;
    let width = (per_trade * 0.6).round().max(1.0) as usize;
    let bars = |positive: bool| -> Vec<(f64, f64)> {
        trades
            .iter()
            .enumerate()
            .filter(|(_, it)| (to_f64(it.pnl) > 0.0) == positive)
            .flat_map(|(index, it)| {
                let pnl = to_f64(it.pnl);
                (0..width).map(move |column| (index as f64 + column as f64 / per_trade, pnl))
            })
            .collect()
    };
    let gains = bars(true);
    let losses = bars(false);
    let y = bounds(trades.iter().map(|it| to_f64(it.pnl)).chain([0.0]));

    let datasets = vec![
        Dataset::default()
            .marker(Marker::Block)
            .graph_type(GraphType::Bar)
            .style(Style::new().fg(UP))
            .data(&gains),
        Dataset::default()
            .marker(Marker::Block)
            .graph_type(GraphType::Bar)
            .style(Style::new().fg(DOWN))
            .data(&losses),
    ];
    let chart = ratatui::widgets::Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .style(Style::new().fg(FAINT))
                .bounds([-0.2, trades.len() as f64])
                .labels(vec![
                    Span::styled("older", theme::dim()),
                    Span::styled("latest", theme::dim()),
                ]),
        )
        .y_axis(value_axis(y, format::signed));
    frame.render_widget(chart, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puts_each_sell_at_its_time_and_merges_the_close_ones() {
        let flags = sell_flags(&[(0.0, 0.36), (50.0, 0.5), (51.0, 0.25)], 0.0, 100.0, 100);
        assert_eq!(
            flags,
            vec![
                Flag {
                    column: 0,
                    text: "+0.36".into(),
                    pnl: 0.36
                },
                Flag {
                    column: 49,
                    text: "2× +0.75".into(),
                    pnl: 0.75
                },
            ]
        );
    }

    #[test]
    fn a_flag_growing_at_the_edge_never_covers_the_one_before() {
        // 3 sells at the very end of 30 columns: their flag moves left, onto the one before
        let flags = sell_flags(
            &[(70.0, 0.38), (98.0, 1.0), (99.0, 1.0), (100.0, 2.4)],
            0.0,
            100.0,
            30,
        );
        for pair in flags.windows(2) {
            assert!(
                pair[0].column + pair[0].text.chars().count() < pair[1].column,
                "{flags:?}"
            );
        }
        let last = flags.last().unwrap();
        assert!(last.column + last.text.chars().count() <= 30, "{flags:?}");
        let total: f64 = flags.iter().map(|it| it.pnl).sum();
        assert!((total - 4.78).abs() < 1e-9, "{flags:?}");
    }

    #[test]
    fn writes_the_flags_at_their_columns() {
        let line = flags_line(&[
            Flag {
                column: 2,
                text: "+0.36".into(),
                pnl: 0.36,
            },
            Flag {
                column: 10,
                text: "-1.00".into(),
                pnl: -1.0,
            },
        ]);
        assert_eq!(line.to_string(), "  +0.36   -1.00");
    }

    #[test]
    fn keeps_a_flag_inside_the_chart() {
        let flags = sell_flags(&[(100.0, 0.36)], 0.0, 100.0, 40);
        assert_eq!(flags[0].column, 35);
    }

    #[test]
    fn interpolates_between_the_points_only() {
        let series = [(10.0, 100.0), (20.0, 200.0)];
        assert_eq!(interpolated(&series, 15.0), Some(150.0));
        assert_eq!(interpolated(&series, 10.0), Some(100.0));
        assert_eq!(interpolated(&series, 9.0), None);
        assert_eq!(interpolated(&series, 21.0), None);
    }
}
