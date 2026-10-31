use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the bot reads from the environment, or else from a `.env` file of the current directory:
/// the credentials never go in the YAML configuration
#[derive(Clone, PartialEq)]
pub struct Environment {
    pub api_key: String,
    pub api_secret: String,
    /// KRAKEN_BOT_CONFIG, `managers.yaml` by default
    pub config_path: PathBuf,
    /// KRAKEN_BOT_DATABASE, `kraken-bot.sqlite` by default
    pub database_path: PathBuf,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Environment")
            .field("api_key", &"***")
            .field("api_secret", &"***")
            .field("config_path", &self.config_path)
            .field("database_path", &self.database_path)
            .finish()
    }
}

impl Environment {
    pub fn load() -> Result<Self, String> {
        let mut variables: HashMap<String, String> = std::fs::read_to_string(".env")
            .map(|content| parse_dotenv(&content))
            .unwrap_or_default();
        // the real environment wins over the file
        variables.extend(std::env::vars());
        Self::from_variables(&variables)
    }

    pub fn from_variables(variables: &HashMap<String, String>) -> Result<Self, String> {
        let get = |name: &str| {
            variables
                .get(name)
                .map(|it| it.trim())
                .filter(|it| !it.is_empty())
        };
        let required = |name: &str| {
            get(name)
                .map(str::to_string)
                .ok_or(format!("{name} is required"))
        };

        Ok(Self {
            api_key: required("KRAKEN_API_KEY")?,
            api_secret: required("KRAKEN_API_SECRET")?,
            config_path: Path::new(get("KRAKEN_BOT_CONFIG").unwrap_or("managers.yaml"))
                .to_path_buf(),
            database_path: Path::new(get("KRAKEN_BOT_DATABASE").unwrap_or("kraken-bot.sqlite"))
                .to_path_buf(),
        })
    }
}

/// KEY=VALUE lines, # comments, values possibly quoted and holding '=' (base64)
pub(crate) fn parse_dotenv(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|it| it.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|it| it.strip_suffix('\''))
                })
                .unwrap_or(value);
            (key.trim().to_string(), unquoted.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variables(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn reads_the_credentials_and_the_paths() {
        let environment = Environment::from_variables(&variables(&[
            ("KRAKEN_API_KEY", "key"),
            ("KRAKEN_API_SECRET", "c2VjcmV0"),
            ("KRAKEN_BOT_CONFIG", "/config/managers.yaml"),
            ("KRAKEN_BOT_DATABASE", "/data/bot.sqlite"),
        ]))
        .unwrap();

        assert_eq!(environment.config_path, Path::new("/config/managers.yaml"));
        assert_eq!(environment.database_path, Path::new("/data/bot.sqlite"));
    }

    #[test]
    fn has_default_paths_and_requires_the_credentials() {
        let environment = Environment::from_variables(&variables(&[
            ("KRAKEN_API_KEY", "key"),
            ("KRAKEN_API_SECRET", "c2VjcmV0"),
        ]))
        .unwrap();
        assert_eq!(environment.config_path, Path::new("managers.yaml"));
        assert_eq!(environment.database_path, Path::new("kraken-bot.sqlite"));

        assert!(Environment::from_variables(&variables(&[("KRAKEN_API_KEY", "key")])).is_err());
        assert!(
            Environment::from_variables(&variables(&[
                ("KRAKEN_API_KEY", "key"),
                ("KRAKEN_API_SECRET", " ")
            ]))
            .is_err()
        );
    }

    #[test]
    fn never_prints_the_credentials() {
        let environment = Environment::from_variables(&variables(&[
            ("KRAKEN_API_KEY", "the-key"),
            ("KRAKEN_API_SECRET", "the-secret"),
        ]))
        .unwrap();
        let printed = format!("{environment:?}");
        assert!(
            !printed.contains("the-key") && !printed.contains("the-secret"),
            "{printed}"
        );
    }

    #[test]
    fn parses_the_dotenv_files() {
        let parsed = parse_dotenv(
            "# comment\nKRAKEN_API_KEY = abc\nKRAKEN_API_SECRET=\"c2Vj=cmV0==\"\nQUOTED='x'\n\nNO_VALUE\n",
        );
        assert_eq!(parsed["KRAKEN_API_KEY"], "abc");
        assert_eq!(parsed["KRAKEN_API_SECRET"], "c2Vj=cmV0==");
        assert_eq!(parsed["QUOTED"], "x");
        assert!(!parsed.contains_key("NO_VALUE"));
    }
}
