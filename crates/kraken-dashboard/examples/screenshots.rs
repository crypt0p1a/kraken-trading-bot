//! The screenshots of the README: the screens of the dashboard on a database, as SVG images
//!
//! ```text
//! scripts/screenshots.sh                     # on the demo database, into assets/screenshots
//! cargo run -p kraken-dashboard --example screenshots -- DATABASE FOLDER
//! ```

use kraken_dashboard::app::{App, Chart};
use kraken_dashboard::data::Database;
use kraken_dashboard::{svg, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The size of a terminal of a large screen, in cells
const WIDTH: u16 = 200;
const HEIGHT: u16 = 50;

/// The keys pressed, then the screen saved
const SCREENS: [(&str, &str, &[KeyCode]); 5] = [
    (
        "desk",
        "the desk: the key figures, the managers, the orders, the markets, the tape",
        &[],
    ),
    (
        "focus",
        "a manager focused on: its figures, its chart, its orders",
        &[KeyCode::Enter],
    ),
    (
        "charts",
        "every chart, on 7 days",
        &[KeyCode::Char('3'), KeyCode::Char('d')],
    ),
    (
        "markets",
        "the markets, the first pair of the YAML file, on 7 days",
        &[KeyCode::Char('3'), KeyCode::Char('d'), KeyCode::Char('d')],
    ),
    ("help", "the keys", &[KeyCode::Char('?')]),
];

fn screen(database: &Database, source: &str, keys: &[KeyCode]) -> String {
    let mut app = App::new(source.to_string());
    app.refresh(database, true);
    // the column of the desk with its most telling charts
    app.charts = [Chart::Portfolio, Chart::Allocation, Chart::Price];
    for code in keys {
        if app.on_key(KeyEvent::from(*code)) {
            app.refresh(database, true);
        }
    }
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("a test terminal");
    terminal
        .draw(|frame| ui::render(frame, &app))
        .expect("a frame");
    svg::from_buffer(terminal.backend().buffer())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [database, folder] = arguments.as_slice() else {
        eprintln!("screenshots DATABASE FOLDER");
        return ExitCode::FAILURE;
    };
    let path = Path::new(database);
    let opened = match Database::open(path) {
        Ok(it) => it,
        Err(error) => {
            eprintln!("screenshots: {error}");
            return ExitCode::FAILURE;
        }
    };
    let folder = PathBuf::from(folder);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        eprintln!("screenshots: {}: {error}", folder.display());
        return ExitCode::FAILURE;
    }
    for (name, what, keys) in SCREENS {
        let file = folder.join(format!("{name}.svg"));
        if let Err(error) = std::fs::write(&file, screen(&opened, "demo.sqlite", keys)) {
            eprintln!("screenshots: {}: {error}", file.display());
            return ExitCode::FAILURE;
        }
        println!("{}: {what}", file.display());
    }
    ExitCode::SUCCESS
}
