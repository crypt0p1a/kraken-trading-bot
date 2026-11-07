use crate::storage::MAIN_ACCOUNT;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the bot reads from the environment, or else from a `.env` file of the current directory:
/// the credentials never go in the YAML configuration
#[derive(Clone, PartialEq)]
pub struct Environment {
    /// The Kraken accounts the bot trades on, each with its key: KRAKEN_BOT_ACCOUNTS
    pub accounts: Vec<AccountKey>,
    /// KRAKEN_BOT_CONFIG, `managers.yaml` by default
    pub config_path: PathBuf,
    /// KRAKEN_BOT_DATABASE, `kraken-bot.sqlite` by default
    pub database_path: PathBuf,
    /// KRAKEN_BOT_NTFY_TOPIC: whoever knows it reads the notifications
    pub ntfy_topic: Option<String>,
    /// KRAKEN_BOT_NTFY_TOKEN: access token of a protected topic
    pub ntfy_token: Option<String>,
    /// KRAKEN_BOT_API_HOST, the real Kraken (https://api.kraken.com) by default: another host
    /// speaking its API, a mocked Kraken in the tests
    pub api_host: Option<String>,
}

/// A Kraken account and its API key. All of them run the same managers
#[derive(Clone, PartialEq)]
pub struct AccountKey {
    pub name: String,
    pub api_key: String,
    pub api_secret: String,
}

impl std::fmt::Debug for AccountKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountKey")
            .field("name", &self.name)
            .field("api_key", &"***")
            .field("api_secret", &"***")
            .finish()
    }
}

/// The longest name of an account: it's shown in the columns of the dashboard
const ACCOUNT_NAME_LENGTH: usize = 12;

/// "alice" reads KRAKEN_API_KEY_ALICE: letters and digits, - becoming _
fn suffix(name: &str) -> String {
    name.to_ascii_uppercase().replace('-', "_")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= ACCOUNT_NAME_LENGTH
        && name
            .chars()
            .all(|it| it.is_ascii_lowercase() || it.is_ascii_digit() || it == '-' || it == '_')
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Environment")
            .field("accounts", &self.accounts)
            .field("config_path", &self.config_path)
            .field("database_path", &self.database_path)
            .field("ntfy_topic", &self.ntfy_topic.as_ref().map(|_| "***"))
            .field("ntfy_token", &self.ntfy_token.as_ref().map(|_| "***"))
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
        // the key of an account, KRAKEN_API_KEY_ALICE; `main` also reads KRAKEN_API_KEY
        let credential = |prefix: &str, account: &str| {
            let name = format!("{prefix}_{}", suffix(account));
            get(&name)
                .or_else(|| (account == MAIN_ACCOUNT).then(|| get(prefix)).flatten())
                .map(str::to_string)
                .ok_or(if account == MAIN_ACCOUNT {
                    format!("{prefix} is required")
                } else {
                    format!("{name} is required")
                })
        };

        let names: Vec<&str> = get("KRAKEN_BOT_ACCOUNTS")
            .map(|it| it.split(',').map(str::trim).collect())
            .unwrap_or_else(|| vec![MAIN_ACCOUNT]);
        let mut accounts: Vec<AccountKey> = Vec::new();
        for name in names {
            if !valid_name(name) {
                return Err(format!(
                    "KRAKEN_BOT_ACCOUNTS: \"{name}\" isn't a name: 1 to {ACCOUNT_NAME_LENGTH} of a-z, 0-9, - and _"
                ));
            }
            if accounts.iter().any(|it| it.name == name) {
                return Err(format!("KRAKEN_BOT_ACCOUNTS: {name} is there twice"));
            }
            let api_key = credential("KRAKEN_API_KEY", name)?;
            // twice the same key: the managers of both would place their orders on one account
            if let Some(other) = accounts.iter().find(|it| it.api_key == api_key) {
                return Err(format!(
                    "the accounts {} and {name} have the same API key",
                    other.name
                ));
            }
            accounts.push(AccountKey {
                name: name.to_string(),
                api_key,
                api_secret: credential("KRAKEN_API_SECRET", name)?,
            });
        }

        Ok(Self {
            accounts,
            config_path: Path::new(get("KRAKEN_BOT_CONFIG").unwrap_or("managers.yaml"))
                .to_path_buf(),
            database_path: Path::new(get("KRAKEN_BOT_DATABASE").unwrap_or("kraken-bot.sqlite"))
                .to_path_buf(),
            ntfy_topic: get("KRAKEN_BOT_NTFY_TOPIC").map(str::to_string),
            ntfy_token: get("KRAKEN_BOT_NTFY_TOKEN").map(str::to_string),
            api_host: get("KRAKEN_BOT_API_HOST").map(str::to_string),
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
            ("KRAKEN_BOT_API_HOST", "http://127.0.0.1:8080"),
        ]))
        .unwrap();

        assert_eq!(environment.config_path, Path::new("/config/managers.yaml"));
        assert_eq!(environment.database_path, Path::new("/data/bot.sqlite"));
        assert_eq!(
            environment.api_host.as_deref(),
            Some("http://127.0.0.1:8080")
        );
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
        // the real Kraken
        assert_eq!(environment.api_host, None);

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
            ("KRAKEN_BOT_ACCOUNTS", "main,alice"),
            ("KRAKEN_API_KEY", "the-key"),
            ("KRAKEN_API_SECRET", "the-secret"),
            ("KRAKEN_API_KEY_ALICE", "the-alice-key"),
            ("KRAKEN_API_SECRET_ALICE", "the-alice-secret"),
            ("KRAKEN_BOT_NTFY_TOPIC", "the-topic"),
            ("KRAKEN_BOT_NTFY_TOKEN", "the-token"),
        ]))
        .unwrap();
        assert_eq!(environment.ntfy_topic.as_deref(), Some("the-topic"));
        assert_eq!(environment.ntfy_token.as_deref(), Some("the-token"));
        let printed = format!("{environment:?}");
        for secret in [
            "the-key",
            "the-secret",
            "the-alice-key",
            "the-alice-secret",
            "the-topic",
            "the-token",
        ] {
            assert!(!printed.contains(secret), "{printed}");
        }
    }

    #[test]
    fn reads_a_single_account_without_any_list() {
        let environment = Environment::from_variables(&variables(&[
            ("KRAKEN_API_KEY", "key"),
            ("KRAKEN_API_SECRET", "secret"),
        ]))
        .unwrap();
        assert_eq!(environment.accounts.len(), 1);
        assert_eq!(environment.accounts[0].name, MAIN_ACCOUNT);
        assert_eq!(environment.accounts[0].api_key, "key");
        assert_eq!(environment.accounts[0].api_secret, "secret");
    }

    #[test]
    fn reads_the_key_of_each_account() {
        let environment = Environment::from_variables(&variables(&[
            ("KRAKEN_BOT_ACCOUNTS", "main, alice,the-other"),
            ("KRAKEN_API_KEY", "key"),
            ("KRAKEN_API_SECRET", "secret"),
            ("KRAKEN_API_KEY_ALICE", "key-a"),
            ("KRAKEN_API_SECRET_ALICE", "secret-a"),
            ("KRAKEN_API_KEY_THE_OTHER", "key-o"),
            ("KRAKEN_API_SECRET_THE_OTHER", "secret-o"),
        ]))
        .unwrap();
        let keys: Vec<(&str, &str, &str)> = environment
            .accounts
            .iter()
            .map(|it| {
                (
                    it.name.as_str(),
                    it.api_key.as_str(),
                    it.api_secret.as_str(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("main", "key", "secret"),
                ("alice", "key-a", "secret-a"),
                ("the-other", "key-o", "secret-o"),
            ]
        );
    }

    #[test]
    fn refuses_what_would_mix_the_accounts() {
        let error =
            |pairs: &[(&str, &str)]| Environment::from_variables(&variables(pairs)).unwrap_err();
        // the same key twice: two sets of managers on one account
        assert!(
            error(&[
                ("KRAKEN_BOT_ACCOUNTS", "main,alice"),
                ("KRAKEN_API_KEY", "same"),
                ("KRAKEN_API_SECRET", "s"),
                ("KRAKEN_API_KEY_ALICE", "same"),
                ("KRAKEN_API_SECRET_ALICE", "s2"),
            ])
            .contains("same API key")
        );
        assert!(
            error(&[
                ("KRAKEN_BOT_ACCOUNTS", "main,main"),
                ("KRAKEN_API_KEY", "k"),
                ("KRAKEN_API_SECRET", "s"),
            ])
            .contains("twice")
        );
        // never the key of the main account for another one
        assert!(
            error(&[
                ("KRAKEN_BOT_ACCOUNTS", "alice"),
                ("KRAKEN_API_KEY", "k"),
                ("KRAKEN_API_SECRET", "s"),
            ])
            .contains("KRAKEN_API_KEY_ALICE")
        );
        for name in ["main,,alice", "Alice", "a b", "name-much-too-long"] {
            assert!(
                error(&[
                    ("KRAKEN_BOT_ACCOUNTS", name),
                    ("KRAKEN_API_KEY", "k"),
                    ("KRAKEN_API_SECRET", "s"),
                ])
                .contains("isn't a name")
            );
        }
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
