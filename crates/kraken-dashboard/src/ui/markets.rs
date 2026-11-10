//! The page of the markets, after every chart with `d`: one pair at a time, in the order of the YAML file, ← → to the
//! next one
//!
//! ```text
//! ┌1 XBTUSD──────┐┌2 ETHUSD──────┐┌3 SOLUSD──────┐   the pairs: price, change, sparkline
//! ┌XBTUSD · PRICE · 24H──────────────────────┐┌QUOTE────────┐
//! │ price, EMA fast and slow, VWAP, fills    ││ last bid ask│
//! │                                          │┌POSITION─────┐  in the range, exposure
//! ┌CANDLES───────────────┐┌RETURNS────┐┌LEVELS────┐           OHLC, distribution, the orders
//! ┌VOLUME 24H┐┌SPREAD────┐┌VOLATILITY┐┌DRAWDOWN──┐             sparkline, line, areas
//! ```

use super::charts::{block_with, bounds, empty, time_axis, value_axis};
use super::{panel, title};
use crate::app::App;
use crate::data::{Candle, Market, Side, to_f64};
use crate::format;
use crate::pairs;
use crate::stats::{canonical, split};
use crate::theme::{self, AMBER, CYAN, DIM, DOWN, FAINT, GOLD, TEXT, UP, VIOLET};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Bar, BarChart, BarGroup, Block, BorderType, Borders, Cell, Dataset, Gauge, GraphType,
    LegendPosition, LineGauge, Paragraph, Row, Sparkline, Table,
};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some((place, index)) = app.shown_pair() else {
        let block = panel(title("MARKETS", []), false);
        empty(frame, area, block, "NO TICKER YET: THE BOT HASN'T TICKED");
        return;
    };
    let market = &app.model.data.markets[index];
    let [strip, top, middle, bottom] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Fill(5),
        Constraint::Fill(4),
        Constraint::Fill(3),
    ])
    .areas(area);
    render_strip(frame, app, strip, place);

    let [price, side] =
        Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(top);
    render_price(frame, app, price, market);
    let [quote, position] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(6)]).areas(side);
    render_quote(frame, app, quote, index, market);
    render_position(frame, app, position, index, market);

    let [candles, returns, levels] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
    ])
    .areas(middle);
    render_candles(frame, app, candles, market);
    render_returns(frame, app, returns, market);
    render_levels(frame, app, levels, index, market);

    let [volume, spread, volatility, drawdown] =
        Layout::horizontal([Constraint::Fill(1); 4]).areas(bottom);
    render_volume(frame, app, volume, market);
    render_spread(frame, app, spread, market);
    render_volatility(frame, app, volatility, market);
    render_drawdown(frame, app, drawdown, market);
}

/// The change over the range, in %
fn change(market: &Market) -> f64 {
    match (market.prices.first(), market.prices.last()) {
        (Some(first), Some(last)) if first.1 > 0.0 => (last.1 - first.1) / first.1 * 100.0,
        _ => 0.0,
    }
}

/// Every pair, in the order of the YAML file: the one shown framed in amber
fn render_strip(frame: &mut Frame, app: &App, area: Rect, shown: usize) {
    let pairs = app.model.pairs();
    let places = Layout::horizontal(vec![Constraint::Fill(1); pairs.len()]).split(area);
    for (place, (rank, index)) in places.iter().zip(pairs.iter().enumerate()) {
        let market = &app.model.data.markets[*index];
        let focused = rank == shown;
        let change = change(market);
        let name = format!(" {} {} ", rank + 1, canonical(&market.key));
        let block = Block::new()
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
            .title(Span::styled(
                name,
                if focused {
                    Style::new().fg(Color::Black).bg(AMBER).bold()
                } else {
                    theme::label()
                },
            ))
            .style(Style::new().bg(if focused {
                theme::SELECTED
            } else {
                theme::PANEL
            }));
        let width = place.width.saturating_sub(2) as usize;
        let prices: Vec<f64> = market.prices.iter().map(|it| it.1).collect();
        let lines = vec![
            Line::from(vec![
                Span::styled(format::price(to_f64(market.last)), theme::bold(TEXT)),
                Span::raw(" "),
                Span::styled(
                    format!("{} {}", theme::arrow(change), format::percent(change)),
                    theme::bold(theme::trend(change)),
                ),
            ]),
            Line::from(Span::styled(
                format::sparkline(&prices, width),
                Style::new().fg(theme::trend(change)),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).block(block), *place);
    }
}

/// The price, its averages and the fills of the managers
fn render_price(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let pair = canonical(&market.key);
    let name = format!("{pair} · PRICE");
    let prices: Vec<(f64, f64)> = market
        .prices
        .iter()
        .map(|(at, price)| (*at as f64, *price))
        .collect();
    if prices.len() < 2 {
        let block = block_with(app, name, Vec::new());
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }
    let now = app.loaded_at;
    let since = app.model.data.since.min(prices[0].0 as i64);
    let last = prices[prices.len() - 1].1;
    let change = change(market);

    let fast: Vec<(f64, f64)> = market
        .trend
        .iter()
        .map(|it| (it.at as f64, it.ema_fast))
        .collect();
    let slow: Vec<(f64, f64)> = market
        .trend
        .iter()
        .map(|it| (it.at as f64, it.ema_slow))
        .collect();
    let vwap: Vec<(f64, f64)> = market
        .candles
        .iter()
        .map(|it| (it.at as f64, it.vwap_24h))
        .collect();
    let fills = app.model.fills(market, since);
    let points = |side: Side| -> Vec<(f64, f64)> {
        fills
            .iter()
            .filter(|it| it.side == side)
            .map(|it| (it.at as f64, it.price))
            .collect()
    };
    let buys = points(Side::Buy);
    let sells = points(Side::Sell);

    let y = bounds(
        prices
            .iter()
            .chain(&fast)
            .chain(&slow)
            .chain(&vwap)
            .chain(&buys)
            .chain(&sells)
            .map(|it| it.1),
    );
    // the averages under the price, the fills above it
    let mut datasets = Vec::new();
    for (name, color, data) in [
        ("VWAP 24H", DIM, &vwap),
        ("EMA SLOW", VIOLET, &slow),
        ("EMA FAST", GOLD, &fast),
    ] {
        if data.len() > 1 {
            datasets.push(
                Dataset::default()
                    .name(Line::from(Span::styled(name, Style::new().fg(color))))
                    .marker(Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::new().fg(color))
                    .data(data),
            );
        }
    }
    datasets.push(
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
    );
    for (name, color, data) in [("▲ BUYS", UP, &buys), ("▼ SELLS", DOWN, &sells)] {
        if !data.is_empty() {
            datasets.push(
                Dataset::default()
                    .name(Line::from(Span::styled(
                        format!("{name} {}", data.len()),
                        Style::new().fg(color),
                    )))
                    .marker(Marker::Block)
                    .graph_type(GraphType::Scatter)
                    .style(Style::new().fg(color))
                    .data(data),
            );
        }
    }

    let summary = vec![Span::styled(
        format!("{} fills ", fills.len()),
        theme::dim(),
    )];
    let chart = ratatui::widgets::Chart::new(datasets)
        .block(block_with(app, name, summary))
        .x_axis(time_axis(since, now))
        .y_axis(value_axis(y, format::price))
        .legend_position(Some(LegendPosition::TopLeft))
        .hidden_legend_constraints((Constraint::Percentage(90), Constraint::Percentage(90)));
    frame.render_widget(chart, area);
}

fn row(name: &str, value: String, style: Style) -> Row<'static> {
    Row::new(vec![
        Cell::from(Span::styled(name.to_string(), theme::dim())),
        Cell::from(Line::from(Span::styled(value, style)).right_aligned()),
    ])
}

/// The figures of the pair: the book, the range, the trend, the managers
fn render_quote(frame: &mut Frame, app: &App, area: Rect, index: usize, market: &Market) {
    let (base, quote) = split(&market.key);
    let last = to_f64(market.last);
    let bid = to_f64(market.bid);
    let ask = to_f64(market.ask);
    let middle = (bid + ask) / 2.0;
    let spread = if middle > 0.0 {
        (ask - bid) / middle * 10_000.0
    } else {
        0.0
    };
    let high = market.candles.iter().map(|it| it.high).fold(last, f64::max);
    let low = market.candles.iter().map(|it| it.low).fold(last, f64::min);
    let change = change(market);
    let ids = app.model.managers_of_market(index);
    let managers: Vec<_> = app
        .model
        .data
        .managers
        .iter()
        .filter(|it| ids.contains(&it.id))
        .collect();
    let budget: f64 = managers.iter().map(|it| to_f64(it.budget)).sum();
    let held: f64 = managers.iter().map(|it| to_f64(it.base())).sum();

    let mut rows = vec![
        row("LAST", format::price(last), theme::bold(TEXT)),
        row("BID", format::price(bid), Style::new().fg(UP)),
        row("ASK", format::price(ask), Style::new().fg(DOWN)),
        row("SPREAD", format!("{spread:.1} bps"), theme::text()),
        row(
            "CHANGE",
            format!("{} {}", theme::arrow(change), format::percent(change)),
            theme::bold(theme::trend(change)),
        ),
        row("HIGH", format::price(high), theme::text()),
        row("LOW", format::price(low), theme::text()),
        row(
            "VWAP 24H",
            format::price(to_f64(market.vwap_24h)),
            theme::text(),
        ),
        row(
            "VOLUME 24H",
            format!("{:.2} {base}", to_f64(market.volume_24h)),
            theme::text(),
        ),
    ];
    if let Some(it) = market.indicators {
        let (trend, color) = match it.trend() {
            1 => ("▲ RISING", UP),
            -1 => ("▼ FALLING", DOWN),
            _ => ("◆ MIXED", GOLD),
        };
        rows.push(row("TREND", trend.into(), theme::bold(color)));
        rows.push(row(
            "24H",
            format!(
                "{} {}",
                theme::arrow(it.change_24h_percent),
                format::percent(it.change_24h_percent)
            ),
            Style::new().fg(theme::trend(it.change_24h_percent)),
        ));
        rows.push(row("ATR", format!("{:.2}%", it.atr_percent), theme::text()));
    }
    rows.push(row(
        "MANAGERS",
        format!("{} · {} {quote}", managers.len(), format::money(budget)),
        Style::new().fg(CYAN),
    ));
    rows.push(row(
        "HELD",
        format!("{} {base}", format::volume(held)),
        Style::new().fg(GOLD),
    ));

    let block = panel(
        title(
            "QUOTE",
            [Span::styled(format!("· {base}/{quote}"), theme::dim())],
        ),
        false,
    );
    frame.render_widget(
        Table::new(rows, [Constraint::Length(10), Constraint::Fill(1)])
            .column_spacing(1)
            .block(block),
        area,
    );
}

/// Where the price is in its range, and what the managers hold of their budget
fn render_position(frame: &mut Frame, app: &App, area: Rect, index: usize, market: &Market) {
    let block = panel(title("POSITION", []), false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [range_text, range, exposure_text, exposure] =
        Layout::vertical([Constraint::Length(1); 4]).areas(inner);

    let last = to_f64(market.last);
    let high = market.candles.iter().map(|it| it.high).fold(last, f64::max);
    let low = market.candles.iter().map(|it| it.low).fold(last, f64::min);
    let ratio = if high > low {
        ((last - low) / (high - low)).clamp(0.0, 1.0)
    } else {
        0.5
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("IN RANGE ", theme::dim()),
            Span::styled(format::price(low), Style::new().fg(DOWN)),
            Span::styled(" … ", theme::faint()),
            Span::styled(format::price(high), Style::new().fg(UP)),
        ])),
        range_text,
    );
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label(Span::styled(
                format!("{:>3.0}%", ratio * 100.0),
                theme::bold(TEXT),
            ))
            .filled_style(Style::new().fg(theme::trend(ratio - 0.5)))
            .unfilled_style(Style::new().fg(FAINT)),
        range,
    );

    let ids = app.model.managers_of_market(index);
    let (budget, exposed) = app
        .model
        .data
        .managers
        .iter()
        .filter(|it| ids.contains(&it.id))
        .fold((0.0, 0.0), |(budget, exposed), it| {
            (
                budget + to_f64(it.budget),
                exposed + to_f64(it.base()) * last,
            )
        });
    let share = if budget > 0.0 {
        (exposed / budget).clamp(0.0, 1.0)
    } else {
        0.0
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("EXPOSURE ", theme::dim()),
            Span::styled(format::money(exposed), Style::new().fg(GOLD)),
            Span::styled(format!(" of {}", format::money(budget)), theme::dim()),
        ])),
        exposure_text,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(share)
            .label(Span::styled(
                format!("{:.0}%", share * 100.0),
                Style::new().fg(Color::Black).bold(),
            ))
            .gauge_style(Style::new().fg(GOLD).bg(theme::SELECTED)),
        exposure,
    );
}

/// The row of the plot a price is on, 0 at the top
fn row_of(value: f64, [low, high]: [f64; 2], height: u16) -> u16 {
    let share = if high > low {
        (high - value) / (high - low)
    } else {
        0.5
    };
    ((share * f64::from(height)) as u16).min(height.saturating_sub(1))
}

/// The candles of the range, drawn cell by cell: the body from the open to the close, the wick
/// from the lowest to the highest
fn render_candles(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    const LABELS: u16 = 10;
    let block = block_with(app, "CANDLES".into(), Vec::new());
    let inner = block.inner(area);
    if market.candles.len() < 2 || inner.width <= LABELS + 2 || inner.height < 3 {
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }
    // a column for each candle, one between two
    let plot_width = inner.width - LABELS;
    let candles = pairs::merged(&market.candles, usize::from(plot_width / 2));
    let ups = candles.iter().filter(|it| it.close >= it.open).count();
    let block = block_with(
        app,
        "CANDLES".into(),
        vec![
            Span::styled(format!("{} ", candles.len()), theme::text()),
            Span::styled(format!("{ups}▲ "), Style::new().fg(UP)),
            Span::styled(format!("{}▼", candles.len() - ups), Style::new().fg(DOWN)),
        ],
    );
    frame.render_widget(block, area);

    let low = candles.iter().map(|it| it.low).fold(f64::MAX, f64::min);
    let high = candles.iter().map(|it| it.high).fold(f64::MIN, f64::max);
    let range = [low, high];
    let height = inner.height;
    for (y, value) in [
        (0, high),
        (height / 2, (low + high) / 2.0),
        (height - 1, low),
    ] {
        frame.render_widget(
            Paragraph::new(Span::styled(format::price(value), theme::dim())),
            Rect::new(inner.x, inner.y + y, LABELS - 1, 1),
        );
    }
    let start = inner.x + LABELS + plot_width.saturating_sub(candles.len() as u16 * 2);
    let buffer = frame.buffer_mut();
    for (column, candle) in candles.iter().enumerate() {
        let x = start + column as u16 * 2;
        let color = if candle.close >= candle.open {
            UP
        } else {
            DOWN
        };
        let wick = row_of(candle.high, range, height)..=row_of(candle.low, range, height);
        let body = row_of(candle.open.max(candle.close), range, height)
            ..=row_of(candle.open.min(candle.close), range, height);
        for row in wick {
            let symbol = if body.contains(&row) { "█" } else { "│" };
            if let Some(cell) = buffer.cell_mut((x, inner.y + row)) {
                cell.set_symbol(symbol).set_fg(color);
            }
        }
    }
}

/// How the price moved from a candle to the next: how many times each move
fn render_returns(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    let returns = pairs::returns(&market.candles);
    let bins = usize::from(inner.width / 3).clamp(3, 25);
    let Some(histogram) = pairs::histogram(&returns, bins) else {
        let block = block_with(app, "RETURNS".into(), Vec::new());
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    };
    let count = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / count;
    let deviation = (returns.iter().map(|it| (it - mean).powi(2)).sum::<f64>() / count).sqrt();
    let ups = returns.iter().filter(|it| **it > 0.0).count();
    let block = block_with(
        app,
        "RETURNS".into(),
        vec![
            Span::styled(format!("σ {deviation:.2}% "), theme::text()),
            Span::styled(
                format!("{:.0}%▲", ups as f64 / count * 100.0),
                Style::new().fg(UP),
            ),
        ],
    )
    .title_bottom(Line::from(vec![
        Span::styled(
            format!(" {} ", format::percent(histogram.low)),
            Style::new().fg(DOWN),
        ),
        Span::styled("… ", theme::faint()),
        Span::styled(
            format!(
                "{} ",
                format::percent(histogram.low + histogram.width * bins as f64)
            ),
            Style::new().fg(UP),
        ),
    ]));
    let bars: Vec<Bar> = histogram
        .counts
        .iter()
        .enumerate()
        .map(|(bin, count)| {
            let middle = histogram.middle(bin);
            let color = if middle.abs() < histogram.width / 2.0 {
                GOLD
            } else {
                theme::trend(middle)
            };
            Bar::default()
                .value(*count as u64)
                .text_value(String::new())
                .style(Style::new().fg(color))
        })
        .collect();
    let width = (inner.width / bins as u16).saturating_sub(1).max(1);
    frame.render_widget(
        BarChart::default()
            .block(block)
            .data(BarGroup::default().bars(&bars))
            .bar_width(width)
            .bar_gap(1),
        area,
    );
}

/// The prices of the managers around the last one: their open orders, what their positions cost
fn render_levels(frame: &mut Frame, app: &App, area: Rect, index: usize, market: &Market) {
    let levels = app.model.levels(index);
    let last = to_f64(market.last);
    let block = panel(
        title(
            "LEVELS",
            [Span::styled(format!("· {} ", levels.len()), theme::dim())],
        ),
        false,
    );
    if levels.is_empty() {
        empty(frame, area, block, "NO ORDER, NO POSITION");
        return;
    }
    let inner = block.inner(area);
    let largest = levels.iter().map(|it| it.amount).fold(1e-9, f64::max);
    // name, price, distance: the bar gets what's left
    let room = usize::from(inner.width).saturating_sub(28).max(1);
    let mut lines: Vec<Line> = Vec::new();
    let mut last_at = None;
    for level in &levels {
        if last_at.is_none() && level.price < last {
            last_at = Some(lines.len());
            lines.push(last_line(last, inner.width));
        }
        let (name, color) = match level.side {
            Some(Side::Buy) => ("BUY  ", UP),
            Some(Side::Sell) => ("SELL ", DOWN),
            None => ("ENTRY", GOLD),
        };
        let distance = (level.price - last) / last * 100.0;
        let bar = ((level.amount / largest) * room as f64).round().max(1.0) as usize;
        lines.push(Line::from(vec![
            Span::styled(name, theme::bold(color)),
            Span::styled(
                format!(" {:<6}", app.manager_name(level.manager_id)),
                theme::dim(),
            ),
            Span::styled(format!("{:>10}", format::price(level.price)), theme::text()),
            Span::styled(format!(" {:>6} ", format::percent(distance)), theme::dim()),
            Span::styled("━".repeat(bar.min(room)), Style::new().fg(color)),
        ]));
    }
    let last_at = last_at.unwrap_or_else(|| {
        lines.push(last_line(last, inner.width));
        lines.len() - 1
    });
    // the lines around the last price, when they don't all fit
    let height = usize::from(inner.height);
    let first = last_at
        .saturating_sub(height / 2)
        .min(lines.len().saturating_sub(height));
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(first).collect::<Vec<_>>()).block(block),
        area,
    );
}

fn last_line(last: f64, width: u16) -> Line<'static> {
    let text = format!(" LAST {} ", format::price(last));
    let side = usize::from(width).saturating_sub(text.chars().count()) / 2;
    Line::from(vec![
        Span::styled("─".repeat(side), Style::new().fg(CYAN)),
        Span::styled(text, theme::bold(CYAN)),
        Span::styled("─".repeat(side), Style::new().fg(CYAN)),
    ])
}

/// A value for each column of the plot, taken at the same distance from each other
fn resampled(values: &[f64], width: usize) -> Vec<f64> {
    if values.len() <= width {
        return values.to_vec();
    }
    (1..=width)
        .map(|column| values[column * values.len() / width - 1])
        .collect()
}

/// What was traded over 24 hours, tick after tick: the lowest at the bottom
fn render_volume(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let (base, _) = split(&market.key);
    let volumes: Vec<f64> = market.candles.iter().map(|it| it.volume_24h).collect();
    let Some(last) = volumes.last().copied() else {
        let block = block_with(app, "VOLUME 24H".into(), Vec::new());
        empty(frame, area, block, "NO TICK IN THE RANGE");
        return;
    };
    let block = block_with(
        app,
        "VOLUME 24H".into(),
        vec![Span::styled(format!("{last:.1} {base}"), theme::bold(CYAN))],
    );
    let width = usize::from(block.inner(area).width);
    let points = resampled(&volumes, width);
    let low = points.iter().copied().fold(f64::MAX, f64::min);
    let high = points.iter().copied().fold(f64::MIN, f64::max);
    // from a little under the lowest: the moves, not the level
    let floor = low - (high - low) * 0.1;
    let data: Vec<u64> = points
        .iter()
        .map(|it| ((it - floor) / (high - floor).max(1e-9) * 1000.0) as u64)
        .collect();
    frame.render_widget(
        Sparkline::default()
            .block(block)
            .data(&data)
            .max(1000)
            .style(Style::new().fg(CYAN)),
        area,
    );
}

/// A small chart of a series of the range, the time without labels
fn render_series(
    frame: &mut Frame,
    block: Block,
    area: Rect,
    points: &[(f64, f64)],
    y: [f64; 2],
    (graph, color): (GraphType, Color),
    show: fn(f64) -> String,
) {
    let since = points.first().map_or(0.0, |it| it.0);
    let now = points.last().map_or(1.0, |it| it.0);
    let chart = ratatui::widgets::Chart::new(vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(graph)
            .style(Style::new().fg(color))
            .data(points),
    ])
    .block(block)
    .x_axis(
        Axis::default()
            .style(Style::new().fg(FAINT))
            .bounds([since, now.max(since + 1.0)]),
    )
    .y_axis(value_axis(y, show));
    frame.render_widget(chart, area);
}

/// The gap between the ask and the bid
fn render_spread(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let points: Vec<(f64, f64)> = market
        .candles
        .iter()
        .map(|it: &Candle| (it.at as f64, it.spread_bps()))
        .collect();
    let Some(last) = points.last().map(|it| it.1) else {
        let block = block_with(app, "SPREAD".into(), Vec::new());
        empty(frame, area, block, "NO TICK IN THE RANGE");
        return;
    };
    let block = block_with(
        app,
        "SPREAD".into(),
        vec![Span::styled(format!("{last:.2} bps"), theme::bold(GOLD))],
    );
    let y = bounds(points.iter().map(|it| it.1));
    render_series(
        frame,
        block,
        area,
        &points,
        y,
        (GraphType::Line, GOLD),
        |it| format!("{it:.2}"),
    );
}

/// The average true range of the bot, in % of the price
fn render_volatility(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let points: Vec<(f64, f64)> = market
        .trend
        .iter()
        .map(|it| (it.at as f64, it.atr_percent))
        .collect();
    let Some(last) = points.last().map(|it| it.1) else {
        let block = block_with(app, "VOLATILITY".into(), Vec::new());
        empty(frame, area, block, "NO INDICATORS YET");
        return;
    };
    let block = block_with(
        app,
        "VOLATILITY".into(),
        vec![Span::styled(format!("ATR {last:.2}%"), theme::bold(VIOLET))],
    );
    let high = points.iter().map(|it| it.1).fold(0.0, f64::max);
    render_series(
        frame,
        block,
        area,
        &points,
        [0.0, (high * 1.1).max(1e-6)],
        (GraphType::Bar, VIOLET),
        |it| format!("{it:.2}%"),
    );
}

/// How far the price is under the highest one of the range before it
fn render_drawdown(frame: &mut Frame, app: &App, area: Rect, market: &Market) {
    let points: Vec<(f64, f64)> = pairs::drawdown(&market.candles)
        .into_iter()
        .map(|(at, it)| (at as f64, it))
        .collect();
    let Some(last) = points.last().map(|it| it.1) else {
        let block = block_with(app, "DRAWDOWN".into(), Vec::new());
        empty(frame, area, block, "NO TICK IN THE RANGE");
        return;
    };
    let deepest = points.iter().map(|it| it.1).fold(0.0, f64::min);
    let block = block_with(
        app,
        "DRAWDOWN".into(),
        vec![
            Span::styled(format!("{} ", format::percent(last)), theme::bold(DOWN)),
            Span::styled(format!("max {}", format::percent(deepest)), theme::dim()),
        ],
    );
    render_series(
        frame,
        block,
        area,
        &points,
        [(deepest * 1.1).min(-1e-6), 0.0],
        (GraphType::Bar, DOWN),
        format::percent,
    );
}
