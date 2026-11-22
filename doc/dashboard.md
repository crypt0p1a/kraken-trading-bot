# Dashboard

`kraken-dashboard` follows the bot in the terminal (ratatui), live from its
[database](trading-bot.md#database), opened **read only**: it never touches Kraken. Its only write is `t`, a
tick of the bot asked for in the `commands` table.

```sh
docker compose exec kraken-trading-bot kraken-dashboard   # in the container of the bot, q to quit
cargo run --release -p kraken-dashboard -- kraken-bot.sqlite   # on a copy of the database
```

| Panel | Content |
|-------|---------|
| header | LIVE or DRY RUN (from the last order), TICK ASKED then TICKING once `t` is pressed, the time since the last tick (STALE after 30 min), the UTC clock, the prices of the pairs scrolling |
| tiles | portfolio: the whole account, what the managers are worth and the money they don't use (equity of the manager when focused on one), target: what the account would be worth once every open buy filled and every position sold for what it cost + `sell_above`%, fees included, P&L (value - budget), realized by the sells, unrealized, 1 month, 3 months and 1 year: what the sells earned per day since the first order carried on as it is, without compounding, the gains of a manager not funding others. For fun rather than a forecast: unrealistic on a short history, and a falling market stops the sells. Fees and volume, open orders; the other quote currencies apart |
| managers | value, P&L, position, entry price and the last price against it, open order, how far the market is from it (`TO FILL`, ▼ the drop a buy waits for, ▲ the rise a sell waits for) and its fill, the trend of the value |
| charts | a column on the last third of the screen, three charts one above the other (fewer in a low terminal), `c`, `v` and `b` switch the one at the top, in the middle and at the bottom: the P&L over time, the value of the whole account (of the manager when focused on one) with its target, the buys filled (▲), and each sell (▼) with a flag saying what it earned, the sells too close sharing one (`3× +1.02`), the price of the pair with the entry price and the open orders, the P&L of each sell, what the portfolio would be worth if every price moved at once by -10% to +10%: the buys the move reaches filled at their limit, the sells at their target, what the bot would place next not played. Not a forecast |
| orders | the blotter, without the cost, the fee and then the volume when narrow: side, price, how far the market is from an open order, volume, fill, cost, fee, status |
| markets | the last price (the range and the spread left out when narrow), 24 hours change, ATR, range and trend of the [indicators](trading-bot.md#indicators-of-the-market), the spread |
| tape | the decisions: orders placed, refused (✗), trailing, virtual managers; `i` shows the idle ones |

The page of the markets, after every chart with `d`, shows one pair at a time, in the order of the YAML file (where its
first manager is; a pair traded only by virtual managers last; a pair no manager trades anymore, ticks
left from an older file, left out),
`←` `→` to the next one:

| Panel | Content |
|-------|---------|
| pairs | every pair, its price, its change over the range and its sparkline; the one shown framed |
| price | the price with the EMAs of the [indicators](trading-bot.md#indicators-of-the-market), the VWAP of 24 hours and the buys and the sells filled |
| quote | last, bid, ask, spread, change, highest and lowest of the range, VWAP, volume, trend, ATR, the managers of the pair and what they hold |
| position | where the price is between the lowest and the highest of the range, what the managers hold of their budget |
| candles | open, highest, lowest and close of each period, as many as the width holds |
| returns | how many times the price moved by how much from a period to the next |
| levels | the open orders of the managers and what their positions cost, around the last price |
| volume 24h, spread, volatility, drawdown | over the range: the volume, the spread in bps, the ATR, how far the price is under the highest before it |

## The screen kept

What the screen shows is kept from a run to the next: the charts, the range, the account, the
order of the tables, the screen of every chart, the page of the markets and its pair, the idle decisions and the open orders only. They're
in `kraken-bot.sqlite.dashboard.json`, next to the database (in the volume `/data` with Docker):
removing it brings the defaults back. A setting that doesn't fit the database anymore, an account
gone, keeps its default.

## Keys

`d` the next screen, in a circle: every chart, the page of the markets (`←` `→` the pair), the desk again (`esc` back to the desk), `tab` the panel the arrows move in, `←` `→` the column its table is sorted on, `s` the other
way (the positions and the volumes by what they're worth, a `—` always at the end), `⏎` focus on the manager of the cursor (tiles, charts,
orders and tape on it alone), `esc` back to all of them, `1`…`5` the range of the charts (1H, 24H,
7D, 30D, ALL), `o` the open orders only, `r` read the database again, `t` a tick of the bot now,
`?` the help, `q` quit. The database is checked every 2
seconds (`--refresh SECONDS`), read again only when the bot wrote it. The P&L realized replays the
orders as the ledger of the bot does: each sell against the cost of the base it sold, fees included.

With several accounts, `a` goes through them in a circle: all of them added up (the managers named
`alice·#7`), then each one alone. `t` asks for a tick of what's shown. An account joining makes the
curve of the portfolio jump, as a deposit does.

## The demo

A demo database, 7 days of a simulated market traded by the strategy, written by the storage of
the bot:

```sh
cargo run -p kraken-dashboard --example demo -- demo.sqlite
cargo run -p kraken-dashboard -- demo.sqlite
```
