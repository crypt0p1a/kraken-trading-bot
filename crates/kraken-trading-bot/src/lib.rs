//! Trading bot on Kraken Spot, port of `cex-trading-bot`
//!
//! Each manager trades its own budget on one pair, with limit orders only, in a loop: it buys at
//! `-buy_below`% of the market, then sells everything it bought at `+sell_above`% of what it cost,
//! fees included. Its orders carry its id as Kraken `userref`, which ties them to it.
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`config`] | the YAML configuration of the managers, the environment |
//! | [`market`] | the trend and the volatility of a pair over 24 hours, recorded only |
//! | [`pair`], [`planner`] | the constraints of a pair, the prices and amounts of the orders |
//! | [`order`], [`ledger`] | the orders of a manager, what it has from them |
//! | [`strategy`] | what a manager does next |
//! | [`storage`] | the SQLite database: orders, states and history of the managers, actions, prices |
//! | [`exchange`] | what the bot needs from Kraken, the real one and a simulated one |
//! | [`sync`] | the orders of the managers, from Kraken to the database |
//! | [`rebuild`] | the virtual managers found again in the orders of Kraken |
//! | [`bot`] | a tick: synchronization, then each manager decides and acts |
//! | [`notify`] | what changed in the orders, for the notifications of `trading-notify` |
//! | [`notify`] | the notifications on the phone, through ntfy |

pub mod bot;
pub mod clock;
pub mod config;
pub mod decimal;
pub mod exchange;
pub mod ledger;
pub mod market;
pub mod notify;
pub mod order;
pub mod pair;
pub mod planner;
pub mod rebuild;
pub mod storage;
pub mod strategy;
pub mod sync;

/// Errors of a tick
#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("Kraken: {0}")]
    Exchange(kraken_api::KrakenError),
    #[error("database: {0}")]
    Storage(String),
    #[error("configuration: {0}")]
    Config(String),
}

pub type BotResult<T> = Result<T, BotError>;
