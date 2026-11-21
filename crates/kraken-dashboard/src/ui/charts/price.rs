//! The price, with the levels of the orders

use super::{Level, block, bounds, empty, level, time_axis, value_axis};
use crate::app::App;
use crate::data::{Side, to_f64};
use crate::format;
use crate::stats::canonical;
use crate::theme::{self, CYAN, DOWN, GOLD, UP};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Dataset, GraphType, LegendPosition};

pub(super) fn render_price(frame: &mut Frame, app: &App, area: Rect) {
    let Some(manager) = app.selected() else {
        empty(frame, area, block(app, "PRICE".into()), "NO MANAGER");
        return;
    };
    let pair = canonical(&manager.pair);
    let block = block(app, format!("{pair} · {}", app.manager_name(manager.id)));
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
    // from the first price known: a ticker of 0 says nothing of the price
    let change = prices
        .iter()
        .map(|it| it.1)
        .find(|it| *it > 0.0)
        .map_or(0.0, |first| (last - first) / first * 100.0);

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
