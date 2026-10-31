//! Trading bot on Kraken Spot, port of `cex-trading-bot`
//!
//! Each manager trades its own budget on one pair, with limit orders only, in a loop: it buys at
//! `-buy_below`% of the market, then sells everything it bought at `+sell_above`% of what it cost,
//! fees included. Its orders carry its id as Kraken `userref`, which ties them to it.
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`config`] | the YAML configuration of the managers, the environment |
//! | [`pair`], [`planner`] | the constraints of a pair, the prices and amounts of the orders |
//! | [`order`], [`ledger`] | the orders of a manager, what it has from them |
//! | [`strategy`] | what a manager does next |
//! | [`storage`] | the SQLite database: orders, states and history of the managers, actions, prices |

pub mod clock;
pub mod config;
pub mod decimal;
pub mod ledger;
pub mod order;
pub mod pair;
pub mod planner;
pub mod storage;
pub mod strategy;
