//! The loop of the dashboard: it draws the screen, handles a key, and reads the database again when
//! the bot wrote it. The keys come from a function: the terminal in `main`, a script in the tests

use crate::app::App;
use crate::data::Database;
use crate::settings;
use crate::ui;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::KeyEvent;
use std::path::Path;
use std::time::{Duration, Instant};

/// The screen is drawn again this often, for the clock and the ticker tape
pub const FRAME: Duration = Duration::from_millis(100);

/// Until `q`: `next_key` waits a frame at most for a key, None when there's none. The database is
/// checked every `refresh`. What the screen shows is written to `settings` as soon as it changes
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    database: &Database,
    app: &mut App,
    refresh: Duration,
    settings: Option<&Path>,
    mut next_key: impl FnMut(Duration) -> std::io::Result<Option<KeyEvent>>,
) -> std::io::Result<()> {
    let mut checked = Instant::now();
    let mut saved = app.settings();
    while !app.quit {
        terminal
            .draw(|frame| ui::render(frame, app))
            .map_err(|error| std::io::Error::other(error.to_string()))?;

        if let Some(key) = next_key(FRAME)?
            && app.on_key(key)
        {
            app.refresh(database, true);
            checked = Instant::now();
        }
        let current = app.settings();
        if let Some(path) = settings
            && current != saved
        {
            // told once, not at every key: the dashboard works the same without its settings
            if let Err(error) = settings::save(path, &current) {
                app.error = Some(format!("settings not kept: {error}"));
            }
            saved = current;
        }
        app.ask_tick(database);
        if checked.elapsed() >= refresh {
            app.refresh(database, false);
            checked = Instant::now();
        }
    }
    Ok(())
}
