//! The chart, one of four: the P&L over time, the portfolio, the price with the levels of the
//! orders, the P&L of each sell

use super::{panel, title};
use crate::app::{App, Chart, now_millis};
use crate::data::{Side, to_f64};
use crate::format;
use crate::stats::canonical;
use crate::theme::{self, AMBER, CYAN, DOWN, FAINT, GOLD, UP};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Dataset, GraphType, LegendPosition, Paragraph};

fn tabs(app: &App) -> Vec<Span<'static>> {
    [
        (Chart::Pnl, "P&L"),
        (Chart::Portfolio, "VALUE"),
        (Chart::Price, "PRICE"),
        (Chart::Trades, "SELLS"),
    ]
    .into_iter()
    .map(|(chart, name)| {
        if chart == app.chart {
            Span::styled(format!(" {name} "), Style::new().fg(Color::Black).bg(AMBER))
        } else {
            Span::styled(format!(" {name} "), theme::dim())
        }
    })
    .collect()
}

fn block(app: &App, name: String) -> Block<'static> {
    block_with(app, name, Vec::new())
}

/// The chart, its range, what it sums up, then the tabs of the charts
fn block_with(app: &App, name: String, summary: Vec<Span<'static>>) -> Block<'static> {
    let mut details = vec![Span::styled(
        format!("· {} ", app.range.label()),
        theme::dim(),
    )];
    details.extend(summary);
    details.push(Span::raw(" "));
    details.extend(tabs(app));
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
    let short = now - since <= 24 * 3_600_000;
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

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    match app.chart {
        Chart::Pnl => render_pnl(frame, app, area),
        Chart::Portfolio => render_portfolio(frame, app, area),
        Chart::Price => render_price(frame, app, area),
        Chart::Trades => render_trades(frame, app, area),
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

    let now = now_millis();
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

    let now = now_millis();
    let since = app.model.data.since.min(series[0].0 as i64);
    let (current, target) = totals.current_and_target(account);
    let (current, target) = (to_f64(current), to_f64(target));
    let moved = current - series[0].1;

    // the orders filled in the range, on the curve
    let mut buys = Vec::new();
    let mut sells = Vec::new();
    for order in
        app.model.data.orders.iter().filter(|it| {
            it.is_final && !it.volume_executed.is_zero() && ids.contains(&it.manager_id)
        })
    {
        let at = order.close_time.unwrap_or(order.open_time) * 1000.0;
        if at >= since as f64 {
            let point = (at, value_at(&series, at));
            match order.side {
                Side::Buy => buys.push(point),
                Side::Sell => sells.push(point),
            }
        }
    }

    let y = bounds(series.iter().map(|it| it.1).chain([target]));
    let target_line = level(since, now, target);
    let color = theme::trend(moved);
    let shade = match color {
        UP => theme::UP_SHADE,
        DOWN => theme::DOWN_SHADE,
        _ => FAINT,
    };
    let datasets = vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Area)
            .fill_to_y(y[0])
            .style(Style::new().fg(shade))
            .data(&series),
        Dataset::default()
            .name(Line::from(Span::styled(
                format!("TARGET {}", format::money(target)),
                Style::new().fg(GOLD),
            )))
            .marker(Marker::Dot)
            .graph_type(GraphType::Scatter)
            .style(Style::new().fg(GOLD))
            .data(&target_line),
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(color))
            .data(&series),
        Dataset::default()
            .name(Line::from(Span::styled(
                format!("BUY {}", buys.len()),
                Style::new().fg(UP),
            )))
            .marker(Marker::Block)
            .graph_type(GraphType::Scatter)
            .style(Style::new().fg(UP))
            .data(&buys),
        Dataset::default()
            .name(Line::from(Span::styled(
                format!("SELL {}", sells.len()),
                Style::new().fg(DOWN),
            )))
            .marker(Marker::Block)
            .graph_type(GraphType::Scatter)
            .style(Style::new().fg(DOWN))
            .data(&sells),
    ];
    let block = block_with(
        app,
        name,
        vec![
            Span::styled(format!("{} ", format::money(current)), theme::bold(color)),
            Span::styled(
                format!("{} in range", format::signed(moved)),
                Style::new().fg(color),
            ),
        ],
    );
    let chart = ratatui::widgets::Chart::new(datasets)
        .block(block)
        .x_axis(time_axis(since, now))
        .y_axis(value_axis(y, format::money))
        .legend_position(Some(LegendPosition::TopLeft))
        .hidden_legend_constraints((
            ratatui::layout::Constraint::Percentage(90),
            ratatui::layout::Constraint::Percentage(90),
        ));
    frame.render_widget(chart, area);
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

    let now = now_millis();
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
