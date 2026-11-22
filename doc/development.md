# Development

Everything without Docker, with cargo: [rustup](https://rustup.rs), Rust 1.89 at least (the
`rust-version` of `Cargo.toml`).

## The crates

```text
crates/
├── kraken-trading-bot     the bot: managers, strategy, SQLite database (executable, Docker)
├── kraken-dashboard       its dashboard in the terminal (ratatui), live from its database
├── kraken-dashboard-data  what the dashboard shows, read from the database (shared, serializable)
└── trading-notify         notifications of a trading bot through ntfy, in several languages
```

| Crate | What's in it |
|-------|--------------|
| `kraken-trading-bot` | the [managers](trading-bot.md#configuration), [several accounts](trading-bot.md#several-accounts), [virtual managers](trading-bot.md#virtual-managers), the [indicators of the market](trading-bot.md#indicators-of-the-market), the [database](trading-bot.md#database) and what's pushed to the [web backend](trading-bot.md#web-backend) |
| `kraken-dashboard` | the [dashboard](dashboard.md): P&L, managers, orders, markets, decisions, charts and heatmaps |
| `kraken-dashboard-data` | what the dashboard shows, read from the database of the bot, as JSON too: shared by the dashboard and the bot |
| `trading-notify` | the [notifications](trading-bot.md#notifications): languages, texts, sending through ntfy |


## Running it with cargo

The bot reads its environment, or else a `.env` file of the current directory, as with Docker:

```sh
cp managers.example.yaml managers.yaml
cp .env.example .env
cargo run --release -p kraken-trading-bot              # ticks until Ctrl-C
cargo run --release -p kraken-trading-bot -- --once    # a single tick
cargo run --release -p kraken-trading-bot -- --check   # the configuration, the pairs and the API keys, nothing placed
```

The database is `kraken-bot.sqlite` in the current directory, unless `KRAKEN_BOT_DATABASE` says
otherwise. The dashboard, on it or on a copy:

```sh
cargo run --release -p kraken-dashboard -- kraken-bot.sqlite
cargo run --release -p kraken-dashboard -- kraken-bot.sqlite --refresh 5   # checked every 5 seconds
```

## The demo and the screenshots

A demo database, 7 days of a simulated market traded by the strategy, written by the storage of
the bot, and the dashboard on it:

```sh
cargo run -p kraken-dashboard --example demo -- demo.sqlite
cargo run -p kraken-dashboard -- demo.sqlite
```

The screenshots of the README are drawn, not taken: `scripts/screenshots.sh` renders the screens of
the dashboard as SVG images in `assets/screenshots`, from the demo database. Run it before a
release, and whenever a screen changes. A change of the storage of the bot updates the demo in the
same change (see `CLAUDE.md`).

## Checks

```sh
git config core.hooksPath .githooks   # refuses a commit holding a secret (gitleaks)
cargo test --workspace                # the offline tests
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check advisories licenses bans sources
```

The [CI](ci.md) runs them on Linux, macOS and Windows, with the coverage, the secret scanners and
the Docker image.

## The tests

- the decisions (planner, ledger, strategy) with the values of the tests of `cex-trading-bot`,
  including 500 random cycles which must all make money
- the database, the configuration, the synchronization
- `scenarios.rs`: the bot tick after tick on a simulated Kraken (fills, expiry, partial fills,
  closing managers, missing funds, orphans, restart without database, refused orders, dry run,
  600 ticks of a random market where every cycle makes money, only limit orders)
- `trailing_buy.rs`: the buys placed again when the price rises, only the ones not filled at all,
  from the price they were placed from, never in a dry run, a refused cancellation notified
- `indicators.rs`: the indicators recorded once 12 hours of candles exist
- `virtual_managers.rs`: the virtual managers on a simulated Kraken (added with the margin,
  deleted without money or once closed, rebuilt after the loss of the database)
- `kraken_wiring.rs`: the requests sent to a mocked Kraken
- `live_dry_run.rs`: one tick on the real Kraken, as a dry run, the history of the account
  searched for virtual managers
