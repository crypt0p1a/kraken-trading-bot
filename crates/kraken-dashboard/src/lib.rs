//! Terminal dashboard of kraken-trading-bot: its managers, their orders and their P&L, live from
//! its SQLite database, opened read only while the bot runs
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`data`] | what's read from the database |
//! | [`stats`] | the P&L realized by the sells, the totals, the P&L over time |
//! | [`app`] | what's selected and shown, the keys |
//! | [`ui`] | the screen |
//! | [`format`], [`theme`] | the numbers and the times, the colors |

pub mod app;
pub mod data;
pub mod format;
pub mod stats;
pub mod theme;
pub mod ui;
