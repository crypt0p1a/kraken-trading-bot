//! The heatmaps of the pairs and of the accounts

use super::{block, block_with, empty};
use crate::app::App;
use crate::format;
use crate::stats::Tile;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// Under this, a variation stays pale: a calm market isn't painted as a big move
const HEAT_SCALE: f64 = 0.5;

pub(super) fn render_heat_pairs(frame: &mut Frame, app: &App, area: Rect) {
    let quote = app.model.totals.first().map_or("", |it| it.quote.as_str());
    let tiles = app.model.pair_tiles(quote);
    render_heatmap(frame, app, area, "HEATMAP PAIRS", &tiles);
}

pub(super) fn render_heat_accounts(frame: &mut Frame, app: &App, area: Rect) {
    let quote = app.model.totals.first().map_or("", |it| it.quote.as_str());
    let tiles = app.model.account_tiles(quote);
    render_heatmap(frame, app, area, "HEATMAP ACCOUNTS", &tiles);
}

/// Where each weight goes in the area, its share of it, in the order of the weights: the
/// heaviest half on one side, the rest on the other, along the longest side. A cell is about twice
/// as high as wide. A place too small for what's left is empty
fn treemap(area: Rect, weights: &[f64]) -> Vec<Rect> {
    fn place(area: Rect, items: &[(usize, f64)], places: &mut [Rect]) {
        let [(index, _)] = items else {
            let total: f64 = items.iter().map(|it| it.1).sum();
            if items.is_empty() {
                return;
            }
            // the first items up to half of the weight, at least one on each side
            let mut before = 0.0;
            let mut count = 0;
            while count < items.len() - 1 && (count == 0 || before < total / 2.0) {
                before += items[count].1;
                count += 1;
            }
            let share = before / total;
            let cut =
                |size: u16| ((f64::from(size) * share).round() as u16).clamp(1, size.max(2) - 1);
            let [first, second] = if area.width >= area.height.saturating_mul(2) && area.width > 1 {
                Layout::horizontal([Constraint::Length(cut(area.width)), Constraint::Fill(1)])
                    .areas(area)
            } else {
                Layout::vertical([Constraint::Length(cut(area.height)), Constraint::Fill(1)])
                    .areas(area)
            };
            place(first, &items[..count], places);
            place(second, &items[count..], places);
            return;
        };
        places[*index] = area;
    }

    let items: Vec<(usize, f64)> = weights.iter().copied().enumerate().collect();
    let mut places = vec![Rect::default(); weights.len()];
    place(area, &items, &mut places);
    places
}

/// From nothing to the full color of its sign, at `scale`, and what's readable on it. The root
/// makes the small moves visible
fn heat_colors(value: f64, scale: f64) -> (Color, Color) {
    let (red, green, blue) = if value >= 0.0 {
        (0.0, 230.0, 118.0)
    } else {
        (255.0, 69.0, 58.0)
    };
    let strength = (value.abs() / scale).min(1.0).sqrt();
    let mix = |from: f64, to: f64| (from + (to - from) * strength) as u8;
    (
        Color::Rgb(mix(30.0, red), mix(38.0, green), mix(52.0, blue)),
        if strength > 0.55 {
            Color::Black
        } else {
            theme::TEXT
        },
    )
}

/// A tile per pair or account, in rows and columns: its size says what it's worth, its color how it
/// moved over the range
fn render_heatmap(frame: &mut Frame, app: &App, area: Rect, name: &str, tiles: &[Tile]) {
    let name = name.to_string();
    if tiles.is_empty() {
        empty(frame, area, block(app, name), "NOTHING WORTH ANYTHING YET");
        return;
    }
    let scale = tiles
        .iter()
        .filter_map(|it| it.percent)
        .fold(HEAT_SCALE, |scale, it| scale.max(it.abs()));
    let block = block_with(
        app,
        name,
        vec![Span::styled(format!("±{scale:.2}%"), theme::dim())],
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let weights: Vec<f64> = tiles.iter().map(|it| it.value).collect();
    let places = treemap(inner, &weights);
    for (tile, place) in tiles.iter().zip(places) {
        if place.is_empty() {
            continue;
        }
        // a line of the panel between two tiles, none on the edges
        let shown = Rect {
            width: place.width - u16::from(place.width > 2 && place.right() < inner.right()),
            height: place.height - u16::from(place.height > 2 && place.bottom() < inner.bottom()),
            ..place
        };
        let (background, text) = match tile.percent {
            Some(percent) => heat_colors(percent, scale),
            None => (theme::SELECTED, theme::TEXT),
        };
        let percent = tile.percent.map_or("—".to_string(), format::percent);
        let mut lines = vec![
            Line::from(tile.name.clone()).bold(),
            Line::from(format::money(tile.value)),
            Line::from(percent.clone()).bold(),
        ];
        match shown.height {
            0 | 1 => lines = vec![Line::from(format!("{} {percent}", tile.name)).bold()],
            2 => {
                lines.remove(1);
            }
            _ => {}
        }
        let top = shown.height.saturating_sub(lines.len() as u16) / 2;
        frame.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .style(Style::new().fg(text).bg(background))
                .block(Block::new().padding(ratatui::widgets::Padding::top(top))),
            shown,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_a_move_by_its_sign_stronger_as_it_grows() {
        // nothing: the background of the panel, the text readable on it
        assert_eq!(heat_colors(0.0, 0.5), (Color::Rgb(30, 38, 52), theme::TEXT));
        // at the scale or over: the full color, black on it
        assert_eq!(
            heat_colors(0.5, 0.5),
            (Color::Rgb(0, 230, 118), Color::Black)
        );
        assert_eq!(heat_colors(3.0, 0.5), heat_colors(0.5, 0.5));
        assert_eq!(
            heat_colors(-0.5, 0.5),
            (Color::Rgb(255, 69, 58), Color::Black)
        );
        // a small move already shows: the root of its share of the scale, brighter than its share
        let (Color::Rgb(_, green, _), text) = heat_colors(0.05, 0.5) else {
            panic!("not a color")
        };
        let linear = 38.0 + (230.0 - 38.0) * 0.1;
        assert!(f64::from(green) > linear + 30.0, "{green}");
        assert_eq!(text, theme::TEXT);
    }

    #[test]
    fn gives_each_tile_its_share_of_the_area() {
        let area = Rect::new(2, 3, 80, 20);
        let weights = [232.0, 95.0, 19.0, 4.0];
        let places = treemap(area, &weights);
        let surface = |place: &Rect| u32::from(place.width) * u32::from(place.height);

        // the whole area, no tile over another, each inside
        assert_eq!(places.iter().map(surface).sum::<u32>(), surface(&area));
        for (index, place) in places.iter().enumerate() {
            assert!(area.contains(place.as_position()), "{place:?}");
            assert!(place.right() <= area.right() && place.bottom() <= area.bottom());
            for other in &places[index + 1..] {
                assert!(!place.intersects(*other), "{place:?} {other:?}");
            }
        }
        // the heavier, the larger, about in proportion
        assert!(surface(&places[0]) > surface(&places[1]));
        assert!(surface(&places[1]) > surface(&places[2]));
        let share = f64::from(surface(&places[0])) / f64::from(surface(&area));
        assert!((share - 232.0 / 350.0).abs() < 0.1, "{share}");

        // alone: everything
        assert_eq!(treemap(area, &[1.0]), [area]);
        assert!(treemap(area, &[]).is_empty());
        // no room: never panics, nothing outside
        for size in [
            Rect::new(0, 0, 1, 1),
            Rect::new(0, 0, 2, 1),
            Rect::new(0, 0, 1, 2),
        ] {
            for place in treemap(size, &weights) {
                assert!(place.right() <= size.right() && place.bottom() <= size.bottom());
            }
        }
    }
}
