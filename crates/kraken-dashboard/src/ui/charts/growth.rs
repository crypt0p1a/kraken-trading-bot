//! The growth of each period

use super::bars::{plot_columns, render_bars};
use super::{block, block_with, empty};
use crate::app::App;
use crate::format;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

/// The growth of each period in % of the portfolio, the latest at the right: the period still going
/// on included
pub(super) fn render_growth(frame: &mut Frame, app: &App, area: Rect) {
    let name = format!("GROWTH PER {}", app.range.period_label());
    let ids = app.charted();
    // a manager against itself, the whole account else
    let quote = app.model.totals.first().map(|it| it.quote.clone());
    let quote = quote.as_deref().filter(|_| app.focus().is_none());
    let growth = app.model.growth_of(&ids, quote, app.range.period());
    // as many bars as the width holds, the most recent
    let columns = plot_columns(area);
    let growth = &growth[growth.len().saturating_sub(columns / 2)..];
    let Some(last) = growth.last() else {
        empty(
            frame,
            area,
            block(app, name),
            "NOT ENOUGH PERIODS IN THE RANGE",
        );
        return;
    };

    let values: Vec<f64> = growth.iter().map(|it| it.percent).collect();
    let average = values.iter().sum::<f64>() / values.len() as f64;
    let mut summary = vec![Span::styled(
        format!("{} ", format::percent(last.percent)),
        theme::bold(theme::trend(last.percent)),
    )];
    if let Some(previous) = growth.len().checked_sub(2).map(|it| growth[it]) {
        summary.push(Span::styled(
            format!("prev {} ", format::percent(previous.percent)),
            Style::new().fg(theme::trend(previous.percent)),
        ));
    }
    summary.push(Span::styled(
        format!("avg {}", format::percent(average)),
        theme::dim(),
    ));

    let short = app.range.period() < 24 * 3_600_000;
    let label = |at: f64| {
        let at = at as i64;
        if short {
            format::clock(at)[..5].to_string()
        } else {
            format::date(at)[5..].to_string()
        }
    };
    let oldest = growth.first().map_or(last.at, |it| it.at);
    render_bars(
        frame,
        area,
        block_with(app, name, summary),
        &values,
        [label(oldest), label(last.at)],
        format::percent,
    );
}
