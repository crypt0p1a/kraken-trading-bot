# Trading bot

`kraken-trading-bot` is the port of `cex-trading-bot` on Kraken. Each **manager** trades its own
budget on one pair, with **limit orders only**, in a loop:

1. buys with everything it has at **-`buy_below`%** of the last price (GTD order: not filled
   after `buy_order_timeout`, it expires and is placed again from the new price)
2. sells everything it bought for **what it cost + `sell_above`%**, fees included (GTC order)
3. starts again with what the sale brought: every cycle ends with more than it started with

Its orders carry its id as Kraken **`userref`**: Kraken knows which order belongs to which
manager, whatever happens to the database. The fees are taken in the quote currency (`fciq`). A
manager only spends its own money, and only what the account has.

## Prerequisites

- Docker with Compose, or [rustup](https://rustup.rs) to run it with cargo
- a Kraken API key with the Query Funds, Query Open Orders & Trades, Query Closed Orders & Trades
  and Create & Modify Orders permissions, never Withdraw Funds

## Configuration

The managers are in a YAML file ([example](../managers.example.yaml)),
the credentials in the environment:

```yaml
dry_run: true              # orders only validated by Kraken, never placed: false to trade
tick_interval: 5m
buy_order_timeout: 12h
managers:
  - id: 1                  # userref of its orders: 1 to 2147483647, unique, never reused
    pair: XBTUSD           # or XBT/USD, XXBTZUSD
    budget: 30             # in the quote currency (USD)
    buy_below: 1.0         # buys at -1% of the last price
    sell_above: 1.5        # sells for its cost + 1.5%, fees included
  - id: 2
    pair: ETHUSD
    budget: 30
    buy_below: 0.8
    sell_above: 1.2
    mode: closing          # active (default) | closing: sells what it holds, then stops
```

- To stop a manager, set `mode: closing`, and remove it once it holds nothing: the open orders of
  a userref no manager has are reported in the logs and left untouched.
- Changing the budget or the percentages of a manager applies from its next order.
- The configuration is checked at start: unique ids, known pairs, valid percentages.

| Variable | Default | |
|----------|---------|-|
| `KRAKEN_API_KEY`, `KRAKEN_API_SECRET` | | the API key, see the permissions above |
| `KRAKEN_BOT_CONFIG` | `managers.yaml` | the YAML file |
| `KRAKEN_BOT_DATABASE` | `kraken-bot.sqlite` | the SQLite database |
| `KRAKEN_BOT_NTFY_TOPIC`, `KRAKEN_BOT_NTFY_TOKEN` | | the [notifications](#notifications) |
| `RUST_LOG` | `info` | |

## Virtual managers

With a `virtual_managers` section, the money of the account no manager uses isn't left idle:

```yaml
virtual_managers:
  min_virtual_id: 1000000  # first id of the virtual managers
  max: 5                   # most virtual managers at the same time (5 by default)
```

- **The margin**, per quote currency: what the account has available, minus what each manager
  has to buy with (its budget, then what its sales brought). The quote held by open buy orders
  isn't available, so it isn't counted twice. The gains of a manager stay its own: they are never
  a margin.
- **At each tick**, if the margin pays for the budget of an active manager of the YAML file, the
  bot adds a copy of one of them, picked at random: same pair, budget, `buy_below` and
  `sell_above`. One at most per tick, up to `max`. It trades from that tick, like any manager.
- **Their ids** are `min_virtual_id + 1000 × order of creation + id of the copied manager`:
  `1003002` is the 4th virtual manager added, a copy of manager 2. The ids of the YAML file must
  then be below 1000, which the check of the configuration enforces. An id is never reused.
- They are stored in the `virtual_managers` table of the database, deleted ones included.

## Running it

```sh
cp managers.example.yaml managers.yaml   # then edit it
cp .env.example .env                     # then fill the key and secret
docker compose up -d --build
docker compose logs -f
```

The image holds only the binary (distroless, non-root, ~70 MB): `managers.yaml` is mounted
read-only in `/config`, the database lives in the `kraken-bot-data` volume (`/data`). `docker
compose stop` lets the tick in progress end.

Without Docker: `cargo run --release -p kraken-trading-bot` (or `-- --once` for a single tick,
`-- --check` to only check the configuration and the pairs).

## Notifications

The bot notifies the phone through [ntfy](https://ntfy.sh) (the `trading-notify` crate, on the
[`ntfy`](https://crates.io/crates/ntfy) crate):

| Event | Notification |
|-------|--------------|
| `started` | the bot started, dry run or live |
| `placed` | a buy or a sell placed: volume, price, total, expiry |
| `filled` | a buy or a sell filled, entirely or partially; the gain of each sale, fees included |
| `expired` | an order expired or canceled, partially filled or not |
| `failed` | an order refused, a manager or a tick failing: once, until it works again |
| `daily_summary` | every day after `daily_summary_hour` (UTC): the value of each manager, its gain, its sales |

1. install the ntfy app (Android, iOS) and subscribe to a long random topic
   (`openssl rand -hex 16`): whoever knows it reads the notifications
2. put it in `.env`: `KRAKEN_BOT_NTFY_TOPIC=...` (and `KRAKEN_BOT_NTFY_TOKEN` for a protected
   topic)
3. add to `managers.yaml`:

```yaml
notifications:
  ntfy:
    server: https://ntfy.sh   # or a self-hosted server
  language: en                # en (default) or fr
  events: [started, placed, filled, expired, failed, daily_summary]   # all by default
  dry_run: false              # true: the orders of a dry run too (one per tick and manager)
  daily_summary_hour: 20      # UTC
```

The fills are found when the orders are synchronized: each change of an order is notified once,
restarts included, and the history found at the first start isn't. A notification which can't be
sent is logged, the bot goes on. Adding a language: a catalog in `trading-notify/src/i18n.rs`.

## Database

The SQLite database (`/data/kraken-bot.sqlite`) is there to work with what the bot did:

| Table | Content |
|-------|---------|
| `orders` | the orders of the managers, synchronized from Kraken (status, volumes, cost, fees, prices, times) |
| `managers` | the current state of each manager: configuration, balances, entry price, value |
| `manager_snapshots` | the state of each manager at each tick |
| `actions` | the decisions: orders placed or validated (dry run), idle reasons, errors |
| `tickers` | the prices of the pairs of the managers, at each tick |
| `sync_state` | where the synchronization of the orders is |
| `virtual_managers` | the [virtual managers](#virtual-managers), deleted ones included |

The amounts are stored as text, to stay exact.

```sh
docker run --rm -v kraken-rust_kraken-bot-data:/data -it keinos/sqlite3 sqlite3 /data/kraken-bot.sqlite \
  "SELECT id, pair, budget, value, quote_available, base_available FROM managers"
```

## Tests

- the decisions (planner, ledger, strategy) with the values of the tests of `cex-trading-bot`,
  including 500 random cycles which must all make money
- the database, the configuration, the synchronization
- `scenarios.rs`: the bot tick after tick on a simulated Kraken (fills, expiry, partial fills,
  closing managers, missing funds, orphans, restart without database, refused orders, dry run,
  600 ticks of a random market where every cycle makes money, only limit orders)
- `virtual_managers.rs`: the virtual managers on a simulated Kraken
- `kraken_wiring.rs`: the requests sent to a mocked Kraken
- `live_dry_run.rs`: one tick on the real Kraken, as a dry run
