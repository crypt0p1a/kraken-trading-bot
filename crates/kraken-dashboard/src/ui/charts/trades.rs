//! The P&L of each sell

use super::bars::{plot_columns, render_bars};
use super::{block, block_with, empty};
use crate::app::App;
use crate::data::to_f64;
use crate::format;
use crate::theme::{self, DOWN, UP};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

pub(super) fn render_trades(frame: &mut Frame, app: &App, area: Rect) {
    let ids = app.charted();
    let since = app.model.data.since as f64 / 1000.0;
    let trades: Vec<_> = app
        .model
        .trades
        .iter()
        .filter(|it| ids.contains(&it.manager_id) && it.at >= since)
        .collect();
    let columns = plot_columns(area);
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

    let values: Vec<f64> = trades.iter().map(|it| to_f64(it.pnl)).collect();
    render_bars(
        frame,
        area,
        block,
        &values,
        ["older".into(), "latest".into()],
        format::signed,
    );
}
