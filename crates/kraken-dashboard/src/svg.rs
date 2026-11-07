//! A screen of the dashboard as an SVG image, its colors kept: the screenshots of the README,
//! drawn from a buffer of the tests rather than taken from a real terminal
//! (`examples/screenshots.rs`)

use crate::theme;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::fmt::Write;

/// Pixels of a cell: the advance of a monospace font of 14 pixels, its line height
const WIDTH: f64 = 8.4;
const HEIGHT: f64 = 18.0;
const FONT_SIZE: f64 = 14.0;
/// The side of a dot of braille, in pixels: 2 columns of 4 dots in a cell
const DOT: f64 = 2.6;
const FONTS: &str = "Menlo, Monaco, Consolas, 'DejaVu Sans Mono', 'Liberation Mono', monospace";

/// The color of the image: Reset is the background of the desk, or its text
fn hex(color: Color, reset: Color) -> String {
    let rgb = |red: u8, green: u8, blue: u8| format!("#{red:02x}{green:02x}{blue:02x}");
    match color {
        Color::Reset => hex(reset, theme::TEXT),
        Color::Rgb(red, green, blue) => rgb(red, green, blue),
        Color::Black => rgb(0, 0, 0),
        Color::Red => rgb(205, 49, 49),
        Color::Green => rgb(13, 188, 121),
        Color::Yellow => rgb(229, 229, 16),
        Color::Blue => rgb(36, 114, 200),
        Color::Magenta => rgb(188, 63, 188),
        Color::Cyan => rgb(17, 168, 205),
        Color::Gray => rgb(204, 204, 204),
        Color::DarkGray => rgb(118, 118, 118),
        Color::LightRed => rgb(241, 76, 76),
        Color::LightGreen => rgb(35, 209, 139),
        Color::LightYellow => rgb(245, 245, 67),
        Color::LightBlue => rgb(59, 142, 234),
        Color::LightMagenta => rgb(214, 112, 214),
        Color::LightCyan => rgb(41, 184, 219),
        Color::White => rgb(229, 229, 229),
        Color::Indexed(index) => {
            let level = 8 + u16::from(index % 24) * 10;
            let level = level.min(255) as u8;
            rgb(level, level, level)
        }
    }
}

/// The dots of a character of braille, the charts of the terminal: (column, row) in its cell
fn braille(symbol: &str) -> Option<Vec<(u8, u8)>> {
    let code = u32::from(symbol.chars().next()?);
    if !(0x2800..=0x28ff).contains(&code) || symbol.chars().count() != 1 {
        return None;
    }
    // the bits of the dots 1 to 8 of the standard, in the order of Unicode
    const DOTS: [(u8, u8); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let bits = code - 0x2800;
    Some(
        DOTS.iter()
            .enumerate()
            .filter(|(bit, _)| bits & (1 << bit) != 0)
            .map(|(_, dot)| *dot)
            .collect(),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The buffer as an SVG of its size in cells: a rectangle per run of a background, a text per run of
/// a style, stretched to its cells so that every font lines up
pub fn from_buffer(buffer: &Buffer) -> String {
    let area = buffer.area;
    let (width, height) = (
        f64::from(area.width) * WIDTH,
        f64::from(area.height) * HEIGHT,
    );
    let background = hex(theme::BACKGROUND, theme::BACKGROUND);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.1}\" height=\"{height:.1}\" \
         viewBox=\"0 0 {width:.1} {height:.1}\" font-family=\"{FONTS}\" font-size=\"{FONT_SIZE}\" \
         shape-rendering=\"crispEdges\">\n\
         <rect width=\"100%\" height=\"100%\" fill=\"{background}\"/>\n"
    );

    for y in 0..area.height {
        let top = f64::from(y) * HEIGHT;
        let cells: Vec<_> = (0..area.width)
            .map(|x| &buffer[(area.x + x, area.y + y)])
            .collect();

        // the backgrounds, the runs of a same color together
        let mut x = 0;
        while x < cells.len() {
            let color = hex(cells[x].bg, theme::BACKGROUND);
            let start = x;
            while x < cells.len() && hex(cells[x].bg, theme::BACKGROUND) == color {
                x += 1;
            }
            if color != background {
                let _ = writeln!(
                    svg,
                    "<rect x=\"{:.1}\" y=\"{top}\" width=\"{:.1}\" height=\"{HEIGHT}\" fill=\"{color}\"/>",
                    start as f64 * WIDTH,
                    (x - start) as f64 * WIDTH
                );
            }
        }

        // the braille drawn as its dots: the fonts draw it too small, or not at all
        for (x, cell) in cells.iter().enumerate() {
            if let Some(dots) = braille(cell.symbol()) {
                let color = hex(cell.fg, theme::TEXT);
                for (column, row) in dots {
                    let _ = writeln!(
                        svg,
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{DOT}\" height=\"{DOT}\" fill=\"{color}\"/>",
                        x as f64 * WIDTH + (f64::from(column) + 0.5) * WIDTH / 2.0 - DOT / 2.0,
                        top + (f64::from(row) + 0.5) * HEIGHT / 4.0 - DOT / 2.0
                    );
                }
            }
        }

        // the texts, the runs of a same style together, without their leading and trailing blanks
        let style = |cell: &ratatui::buffer::Cell| {
            (
                hex(cell.fg, theme::TEXT),
                cell.modifier.contains(Modifier::BOLD),
                cell.modifier.contains(Modifier::DIM),
            )
        };
        let mut x = 0;
        while x < cells.len() {
            let current = style(cells[x]);
            let start = x;
            let mut text = String::new();
            while x < cells.len() && style(cells[x]) == current {
                let symbol = cells[x].symbol();
                text.push_str(if braille(symbol).is_some() {
                    " "
                } else {
                    symbol
                });
                x += 1;
            }
            let trimmed = text.trim_end();
            let leading = trimmed.chars().take_while(|it| *it == ' ').count();
            let shown = &trimmed[leading..];
            if shown.is_empty() {
                continue;
            }
            let (color, bold, dim) = current;
            let _ = writeln!(
                svg,
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{color}\"{}{} textLength=\"{:.1}\" \
                 lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\">{}</text>",
                (start + leading) as f64 * WIDTH,
                top + HEIGHT * 0.75,
                if bold { " font-weight=\"bold\"" } else { "" },
                if dim { " opacity=\"0.6\"" } else { "" },
                shown.chars().count() as f64 * WIDTH,
                escape(shown)
            );
        }
    }
    svg.push_str("</svg>\n");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn draws_the_texts_and_the_backgrounds_of_the_cells() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 2));
        buffer.set_string(1, 0, "P&L <up>", Style::new().fg(theme::UP).bold());
        buffer.set_string(0, 1, "  ", Style::new().bg(theme::SELECTED));
        buffer.set_string(
            4,
            1,
            "ok",
            Style::new().fg(Color::Reset).add_modifier(Modifier::DIM),
        );
        let svg = from_buffer(&buffer);

        assert!(svg.starts_with("<svg "), "{svg}");
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("width=\"100.8\" height=\"36.0\""), "{svg}");
        // escaped, in its color, bold, from its first cell, stretched over its 8 cells
        assert!(
            svg.contains(
                "<text x=\"8.4\" y=\"13.5\" fill=\"#00e676\" font-weight=\"bold\" textLength=\"67.2\""
            ),
            "{svg}"
        );
        assert!(svg.contains(">P&amp;L &lt;up&gt;</text>"), "{svg}");
        // a background of its own, on 2 cells; the text of the terminal by default, dimmed
        assert!(
            svg.contains(
                "<rect x=\"0.0\" y=\"18\" width=\"16.8\" height=\"18\" fill=\"#1e2634\"/>"
            ),
            "{svg}"
        );
        assert!(svg.contains("fill=\"#dee2e8\" opacity=\"0.6\""), "{svg}");
        // nothing for the blank cells
        assert_eq!(svg.matches("<text").count(), 2);
    }

    #[test]
    fn draws_the_dots_of_the_braille() {
        // the dots 1 and 8: the top left and the bottom right
        assert_eq!(braille("\u{2881}"), Some(vec![(0, 0), (1, 3)]));
        assert_eq!(braille("\u{2800}"), Some(vec![]));
        assert_eq!(braille("a"), None);

        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer.set_string(1, 0, "\u{28ff}", Style::new().fg(theme::UP));
        let svg = from_buffer(&buffer);
        // its 8 dots, in its cell, and no text for it
        assert_eq!(svg.matches("fill=\"#00e676\"").count(), 8, "{svg}");
        assert!(
            svg.contains("<rect x=\"9.2\" y=\"0.9\" width=\"2.6\""),
            "{svg}"
        );
        assert!(!svg.contains("<text"), "{svg}");
    }

    #[test]
    fn has_a_color_for_every_color_of_the_terminal() {
        for color in [
            Color::Reset,
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
            Color::Indexed(250),
            Color::Rgb(1, 2, 3),
        ] {
            let it = hex(color, theme::BACKGROUND);
            assert!(it.len() == 7 && it.starts_with('#'), "{color:?}: {it}");
        }
        assert_eq!(hex(Color::Rgb(1, 2, 3), theme::TEXT), "#010203");
    }
}
