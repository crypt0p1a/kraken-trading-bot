//! The charts of bars: green and red from zero

use super::{bounds, value_axis};
use crate::theme::{self, DOWN, FAINT, UP};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::Span;
use ratatui::widgets::{Axis, Block, Dataset, GraphType};

/// The columns of the plot of a chart of bars, without the borders and the labels of the values
pub(super) fn plot_columns(area: Rect) -> usize {
    area.width.saturating_sub(12).max(1) as usize
}

/// Green and red bars from zero, from left to right
pub(super) fn render_bars(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    values: &[f64],
    ends: [String; 2],
    show: fn(f64) -> String,
) {
    // a bar is a vertical line per column it covers: 60% of the room of a value
    let per_value = plot_columns(area) as f64 / values.len() as f64;
    let width = (per_value * 0.6).round().max(1.0) as usize;
    let bars = |positive: bool| -> Vec<(f64, f64)> {
        values
            .iter()
            .enumerate()
            .filter(|(_, it)| (**it > 0.0) == positive)
            .flat_map(|(index, it)| {
                (0..width).map(move |column| (index as f64 + column as f64 / per_value, *it))
            })
            .collect()
    };
    let gains = bars(true);
    let losses = bars(false);
    let y = bounds(values.iter().copied().chain([0.0]));

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
                .bounds([-0.2, values.len() as f64])
                .labels(ends.map(|it| Span::styled(it, theme::dim())).to_vec()),
        )
        .y_axis(value_axis(y, show));
    frame.render_widget(chart, area);
}
