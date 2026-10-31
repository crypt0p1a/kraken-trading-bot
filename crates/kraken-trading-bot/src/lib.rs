//! Trading bot on Kraken Spot, port of `cex-trading-bot`
//!
//! Each manager trades its own budget on one pair, with limit orders only, in a loop: it buys at
//! `-buy_below`% of the market, then sells everything it bought at `+sell_above`% of what it cost,
//! fees included. Its orders carry its id as Kraken `userref`, which ties them to it.
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`config`] | the YAML configuration of the managers, the environment |

pub mod config;
