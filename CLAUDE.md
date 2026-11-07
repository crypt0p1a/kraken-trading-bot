# kraken-trading-bot

## The database and the demo

A change of the storage of the bot (`crates/kraken-trading-bot/src/storage`: a table, a column, a
migration, what's recorded) must update the demo database too, in the same change:

- `crates/kraken-dashboard/examples/demo.rs` writes the demo through the storage of the bot: make it
  fill what was added, so the dashboard shows it on the demo
- the tests of the dashboard (`crates/kraken-dashboard/tests/dashboard.rs`) read a database written
  by that same storage: they must still pass
- regenerate the screenshots of the README when a screen changes: `scripts/screenshots.sh`, then
  commit `assets/screenshots`
