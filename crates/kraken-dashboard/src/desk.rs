//! The loop of the dashboard: it draws the screen, handles a key, and reads the database again when
//! the bot wrote it. The keys come from a function: the terminal in `main`, a script in the tests

use crate::app::App;
use crate::data::Database;
use crate::ui;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::KeyEvent;
use std::time::{Duration, Instant};

/// The screen is drawn again this often, for the clock and the ticker tape
pub const FRAME: Duration = Duration::from_millis(100);

/// Until `q`: `next_key` waits a frame at most for a key, None when there's none. The database is
/// checked every `refresh`
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    database: &Database,
    app: &mut App,
    refresh: Duration,
    mut next_key: impl FnMut(Duration) -> std::io::Result<Option<KeyEvent>>,
) -> std::io::Result<()> {
    let mut checked = Instant::now();
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
        app.ask_tick(database);
        if checked.elapsed() >= refresh {
            app.refresh(database, false);
            checked = Instant::now();
        }
    }
    Ok(())
}
