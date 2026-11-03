//! The colors of the desk: amber labels on black, green up, red down

use ratatui::style::{Color, Modifier, Style};

pub const BACKGROUND: Color = Color::Rgb(7, 9, 13);
pub const PANEL: Color = Color::Rgb(12, 15, 21);
pub const BORDER: Color = Color::Rgb(44, 52, 66);
pub const BORDER_FOCUS: Color = Color::Rgb(255, 176, 0);
pub const AMBER: Color = Color::Rgb(255, 176, 0);
pub const TEXT: Color = Color::Rgb(222, 226, 232);
pub const DIM: Color = Color::Rgb(112, 121, 136);
pub const FAINT: Color = Color::Rgb(62, 70, 84);
pub const UP: Color = Color::Rgb(0, 230, 118);
pub const DOWN: Color = Color::Rgb(255, 69, 58);
/// The areas under the lines
pub const UP_SHADE: Color = Color::Rgb(0, 72, 40);
pub const DOWN_SHADE: Color = Color::Rgb(92, 22, 20);
pub const CYAN: Color = Color::Rgb(0, 210, 255);
pub const GOLD: Color = Color::Rgb(255, 214, 10);
pub const VIOLET: Color = Color::Rgb(191, 90, 242);
pub const SELECTED: Color = Color::Rgb(30, 38, 52);

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn faint() -> Style {
    Style::new().fg(FAINT)
}

pub fn label() -> Style {
    Style::new().fg(AMBER).add_modifier(Modifier::BOLD)
}

pub fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// Green when positive, red when negative
pub fn trend(value: f64) -> Color {
    if value > 0.0 {
        UP
    } else if value < 0.0 {
        DOWN
    } else {
        TEXT
    }
}

/// ▲ ▼ or ■
pub fn arrow(value: f64) -> &'static str {
    if value > 0.0 {
        "▲"
    } else if value < 0.0 {
        "▼"
    } else {
        "■"
    }
}
