//! Configuration: the managers in a YAML file, the credentials and paths in the environment

mod environment;
mod managers;

pub use environment::Environment;
pub use managers::{BotConfig, ConfigError, ManagerConfig, ManagerMode};
