//! What the dashboard shows, kept from a run to the next: the charts, the range, the account, the
//! order of the tables… in a JSON file next to the database, in the volume `/data` with Docker.
//! The database itself stays the bot's: the dashboard never changes its schema

use crate::app::{Chart, Range};
use crate::sort::Sort;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The choices of the screen; what's missing from the file keeps its default, a file of an older
/// version included
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The column of the desk, from the top
    pub charts: [Chart; 3],
    pub range: Range,
    /// The account shown, all of them added up when None
    pub account: Option<i64>,
    /// The order of the managers, the orders and the tape
    pub sorts: [Sort; 3],
    pub all_charts: bool,
    pub show_idle: bool,
    pub open_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        crate::app::App::new(String::new()).settings()
    }
}

/// `kraken-bot.sqlite.dashboard.json` next to `kraken-bot.sqlite`: each database its own
pub fn path_of(database: &Path) -> PathBuf {
    let mut name = database
        .file_name()
        .map(|it| it.to_os_string())
        .unwrap_or_else(|| "kraken-bot.sqlite".into());
    name.push(".dashboard.json");
    database.with_file_name(name)
}

/// The settings of the file, the defaults when it's missing or unreadable: never a reason not to
/// start
pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|it| serde_json::from_str(&it).ok())
        .unwrap_or_default()
}

/// Written aside, then moved over the file: a dashboard stopped meanwhile never leaves half a file
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    let mut partial = path.as_os_str().to_os_string();
    partial.push(".partial");
    std::fs::write(&partial, json + "\n")?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kraken-dashboard-settings-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn lives_next_to_its_database() {
        assert_eq!(
            path_of(Path::new("/data/kraken-bot.sqlite")),
            Path::new("/data/kraken-bot.sqlite.dashboard.json")
        );
        assert_eq!(
            path_of(Path::new("copy.sqlite")),
            Path::new("copy.sqlite.dashboard.json")
        );
    }

    #[test]
    fn reads_what_it_wrote() {
        let folder = folder("round-trip");
        let path = folder.join("bot.sqlite.dashboard.json");
        let settings = Settings {
            charts: [Chart::HeatPairs, Chart::Growth, Chart::Pnl],
            range: Range::Week,
            account: Some(2),
            all_charts: true,
            show_idle: true,
            open_only: true,
            ..Settings::default()
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), settings);
        // readable by a human: the names of the charts and of the range
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"heat_pairs\"") && text.contains("\"week\""),
            "{text}"
        );
        assert!(!folder.join("bot.sqlite.dashboard.json.partial").exists());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn starts_with_the_defaults_rather_than_failing() {
        let folder = folder("defaults");
        // no file, a broken one, one of an older version without the latest settings
        assert_eq!(load(&folder.join("missing.json")), Settings::default());
        let broken = folder.join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        assert_eq!(load(&broken), Settings::default());
        let older = folder.join("older.json");
        std::fs::write(&older, r#"{"range": "month", "unknown": 1}"#).unwrap();
        assert_eq!(
            load(&older),
            Settings {
                range: Range::Month,
                ..Settings::default()
            }
        );
        // a folder that can't be written: an error to tell, not a panic
        assert!(save(&folder.join("missing/settings.json"), &Settings::default()).is_err());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
