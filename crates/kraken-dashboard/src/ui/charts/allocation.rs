//! The portfolio crypto by crypto

use super::bars::plot_columns;
use super::{block, empty, time_axis, value_axis};
use crate::app::App;
use crate::format;
use crate::theme::{self, AMBER, CYAN, FAINT, GOLD};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Dataset, GraphType, Paragraph};

/// The color of a layer of the portfolio: always the same for a crypto, from a tick to the next
fn layer_color(name: &str, quote: &str) -> Color {
    match name {
        "XBT" | "BTC" => AMBER,
        "ETH" => theme::VIOLET,
        "SOL" => CYAN,
        "XRP" => Color::Rgb(90, 140, 255),
        crate::stats::OTHER => FAINT,
        _ if name == quote => theme::DIM,
        _ => GOLD,
    }
}

/// The whole account, focused on a manager or not: one layer above the other, each crypto then
/// the quote, what each is worth now above the plot
pub(super) fn render_allocation(frame: &mut Frame, app: &App, area: Rect) {
    let quote = app
        .model
        .totals
        .first()
        .map(|it| it.quote.clone())
        .unwrap_or_default();
    let block = block(app, format!("BY CRYPTO · {quote}"));
    let series = app.model.account_series(&quote);
    if series.len() < 2 {
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    }
    let now = app.loaded_at;
    let since = app.model.data.since.min(series[0].0 as i64);
    let allocation = app.model.allocation(&quote, since, now, plot_columns(area));
    let Some((_, last)) = allocation.points.last() else {
        empty(frame, area, block, "NOT ENOUGH TICKS IN THE RANGE");
        return;
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [legend, plot] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    let total: f64 = last.iter().sum();
    let spans: Vec<Span> = allocation
        .names
        .iter()
        .zip(last)
        // what the range had and is gone says nothing of now
        .filter(|(_, value)| **value >= 0.5)
        .flat_map(|(name, value)| {
            [
                Span::styled("■", Style::new().fg(layer_color(name, &quote))),
                Span::styled(
                    format!(
                        "{name} {value:.0} {:.0}%  ",
                        if total > 0.0 {
                            value / total * 100.0
                        } else {
                            0.0
                        }
                    ),
                    theme::text(),
                ),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), legend);

    // each layer up to its top, the highest drawn first: the next ones cover what's under them
    let tops: Vec<Vec<(f64, f64)>> = (0..allocation.names.len())
        .map(|layer| {
            allocation
                .points
                .iter()
                .map(|(at, values)| (*at, values[..=layer].iter().sum()))
                .collect()
        })
        .collect();
    let datasets: Vec<Dataset> = tops
        .iter()
        .zip(&allocation.names)
        .rev()
        .map(|(top, name)| {
            Dataset::default()
                .marker(Marker::HalfBlock)
                .graph_type(GraphType::Bar)
                .style(Style::new().fg(layer_color(name, &quote)))
                .data(top)
        })
        .collect();
    let high = tops
        .last()
        .into_iter()
        .flatten()
        .map(|it| it.1)
        .fold(1.0, f64::max);
    let chart = ratatui::widgets::Chart::new(datasets)
        .x_axis(time_axis(since, now))
        .y_axis(value_axis([0.0, high * 1.05], format::money));
    frame.render_widget(chart, plot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_each_layer_the_same_from_a_tick_to_the_next() {
        assert_eq!(layer_color("XBT", "USD"), AMBER);
        assert_eq!(layer_color("BTC", "USD"), AMBER);
        assert_eq!(layer_color("ETH", "USD"), theme::VIOLET);
        assert_eq!(layer_color("SOL", "USD"), CYAN);
        assert_eq!(layer_color(crate::stats::OTHER, "USD"), FAINT);
        // the quote, whatever it is
        assert_eq!(layer_color("USD", "USD"), theme::DIM);
        assert_eq!(layer_color("EUR", "EUR"), theme::DIM);
        // any other crypto
        assert_eq!(layer_color("ADA", "USD"), GOLD);
    }
}
