//! Configuration: the managers in a YAML file, the credentials and paths in the environment

mod environment;
mod managers;

pub use environment::{AccountKey, Environment};
pub use managers::{
    BotConfig, ConfigError, EventKind, FALLBACK_SELL_ABOVE, Language, MIN_VIRTUAL_ID,
    ManagerConfig, ManagerMode, NotificationsConfig, NtfyConfig, TEMPLATE_IDS, VirtualId,
    is_virtual,
};
