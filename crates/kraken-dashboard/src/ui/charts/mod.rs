//! The chart, one of nine: the P&L over time, the portfolio, the price with the levels of the
//! orders, the P&L of each sell, the portfolio if the market moves, the growth of each period, the
//! portfolio crypto by crypto, the heatmaps of the pairs and of the accounts

mod allocation;
mod bars;
mod flags;
mod growth;
mod heatmap;
mod pnl;
mod portfolio;
mod price;
mod trades;

use super::{panel, title};
use crate::app::{App, Chart};
use crate::format;
use crate::theme::{self, FAINT};
use allocation::render_allocation;
use growth::render_growth;
use heatmap::{render_heat_accounts, render_heat_pairs};
use pnl::{render_pnl, render_scenarios};
use portfolio::render_portfolio;
use price::render_price;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Paragraph};
use trades::render_trades;

fn block(app: &App, name: String) -> Block<'static> {
    block_with(app, name, Vec::new())
}

/// The chart, its range, what it sums up
pub(super) fn block_with(app: &App, name: String, summary: Vec<Span<'static>>) -> Block<'static> {
    let mut details = vec![Span::styled(
        format!("· {} ", app.range.label()),
        theme::dim(),
    )];
    details.extend(summary);
    panel(title(&name, details), false)
}

pub(super) fn empty(frame: &mut Frame, area: Rect, block: Block, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), theme::faint())))
            .centered()
            .block(block),
        area,
    );
}

/// The bounds of the values, a little wider than them
pub(super) fn bounds(values: impl Iterator<Item = f64>) -> [f64; 2] {
    let (low, high) = values.fold((f64::MAX, f64::MIN), |(low, high), it| {
        (low.min(it), high.max(it))
    });
    if low > high {
        return [0.0, 1.0];
    }
    let margin = ((high - low) * 0.08).max(high.abs() * 0.0005).max(1e-9);
    [low - margin, high + margin]
}

pub(super) fn time_axis(since: i64, now: i64) -> Axis<'static> {
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

pub(super) fn value_axis(bounds: [f64; 2], show: fn(f64) -> String) -> Axis<'static> {
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

/// What a chart wants on the screen with all of them, before they get narrower
const GRID_CHART_WIDTH: u16 = 40;
const GRID_CHART_HEIGHT: u16 = 9;
/// However wide the screen: more would leave rows too low and charts too wide
const GRID_COLUMNS: u16 = 4;

/// The charts of the column, one above the other
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let count = (area.height / CHART_HEIGHT).clamp(1, app.charts.len() as u16) as usize;
    let places = Layout::vertical(vec![Constraint::Fill(1); count]).split(area);
    for (place, chart) in places.iter().zip(app.charts) {
        render_chart(frame, app, *place, chart);
    }
}

/// How many charts on each row of a screen of this size, the first rows having one less: the
/// curves are first, and want the room
fn grid(width: u16, height: u16, count: usize) -> Vec<usize> {
    let columns = (width / GRID_CHART_WIDTH).clamp(1, GRID_COLUMNS) as usize;
    let mut rows = count.div_ceil(columns);
    // wide: the curves, then the four charts that hold in a quarter, then the heatmaps, which want
    // the width
    if columns == GRID_COLUMNS as usize && count == 9 && height / (rows as u16) >= GRID_CHART_HEIGHT
    {
        return vec![3, 4, 2];
    }
    // too low for that: narrower charts, on fewer rows
    if height / (rows as u16) < GRID_CHART_HEIGHT {
        rows = ((height / GRID_CHART_HEIGHT).max(1) as usize).min(count);
    }
    (0..rows)
        .map(|row| count / rows + usize::from(row >= rows - count % rows))
        .collect()
}

/// Every chart, on the whole area
pub fn render_all(frame: &mut Frame, app: &App, area: Rect) {
    let rows = grid(area.width, area.height, Chart::ALL.len());
    let places = Layout::vertical(vec![Constraint::Fill(1); rows.len()]).split(area);
    let mut charts = Chart::ALL.into_iter();
    for (place, count) in places.iter().zip(rows) {
        let columns = Layout::horizontal(vec![Constraint::Fill(1); count]).split(*place);
        for (column, chart) in columns.iter().zip(charts.by_ref()) {
            render_chart(frame, app, *column, chart);
        }
    }
}

fn render_chart(frame: &mut Frame, app: &App, area: Rect, chart: Chart) {
    match chart {
        Chart::Pnl => render_pnl(frame, app, area),
        Chart::Portfolio => render_portfolio(frame, app, area),
        Chart::Price => render_price(frame, app, area),
        Chart::Trades => render_trades(frame, app, area),
        Chart::Scenarios => render_scenarios(frame, app, area),
        Chart::Growth => render_growth(frame, app, area),
        Chart::Allocation => render_allocation(frame, app, area),
        Chart::HeatPairs => render_heat_pairs(frame, app, area),
        Chart::HeatAccounts => render_heat_accounts(frame, app, area),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_every_chart_out_for_the_size_of_the_screen() {
        // wide, however wide: the curves, the four others, the heatmaps
        assert_eq!(grid(160, 44, 9), [3, 4, 2]);
        assert_eq!(grid(200, 44, 9), [3, 4, 2]);
        assert_eq!(grid(300, 44, 9), [3, 4, 2]);
        // less wide: the fuller rows at the bottom
        assert_eq!(grid(159, 44, 9), [3, 3, 3]);
        assert_eq!(grid(100, 44, 9), [2, 2, 2, 3]);
        // narrow and high: one above the other, as long as there's room
        assert_eq!(grid(39, 90, 9), [1; 9]);
        // low: narrower charts rather than charts without room for a curve
        assert_eq!(grid(100, 25, 9), [4, 5]);
        assert_eq!(grid(200, 25, 9), [4, 5]);
        // nothing lost, whatever the size
        for (width, height) in [(100, 22), (120, 30), (200, 50), (400, 100), (1, 1)] {
            assert_eq!(grid(width, height, 9).iter().sum::<usize>(), 9);
        }
    }
}
