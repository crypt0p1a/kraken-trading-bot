//! kraken-dashboard: the trading desk of kraken-trading-bot, in the terminal
//!
//! ```text
//! kraken-dashboard [DATABASE] [--refresh SECONDS]
//! ```
//!
//! DATABASE: the SQLite database of the bot, KRAKEN_BOT_DATABASE else ./kraken-bot.sqlite. It's
//! opened read only, and read again when the bot writes it: checked every SECONDS (default 2).

use kraken_dashboard::app::App;
use kraken_dashboard::data::Database;
use kraken_dashboard::desk;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "kraken-dashboard [DATABASE] [--refresh SECONDS]

DATABASE: the database of the bot, KRAKEN_BOT_DATABASE else ./kraken-bot.sqlite
SECONDS: how often it's checked for a change, 2 by default";

struct Options {
    database: PathBuf,
    refresh: Duration,
}

fn options(arguments: &[String]) -> Result<Options, String> {
    let mut database = None;
    let mut refresh = Duration::from_secs(2);
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--refresh" => {
                let seconds = arguments
                    .next()
                    .and_then(|it| it.parse::<f64>().ok())
                    .filter(|it| *it > 0.0)
                    .ok_or("--refresh expects a number of seconds")?;
                refresh = Duration::from_secs_f64(seconds);
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if database.is_none() => database = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    let database = database
        .or_else(|| std::env::var_os("KRAKEN_BOT_DATABASE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("kraken-bot.sqlite"));
    Ok(Options { database, refresh })
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|it| it == "-h" || it == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match options(&arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("kraken-dashboard: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // before the screen is taken: an error stays readable
    let database = match Database::open(&options.database) {
        Ok(database) => database,
        Err(error) => {
            eprintln!("kraken-dashboard: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = App::new(options.database.display().to_string());
    app.refresh(&database, true);

    let mut terminal = ratatui::init();
    let result = desk::run(
        &mut terminal,
        &database,
        &mut app,
        options.refresh,
        |frame| {
            // the presses only: the terminals sending the releases too would count each key twice
            if event::poll(frame)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                return Ok(Some(key));
            }
            Ok(None)
        },
    );
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kraken-dashboard: {error}");
            ExitCode::FAILURE
        }
    }
}
