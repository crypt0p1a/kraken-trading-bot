# Trading bot

`kraken-trading-bot` is the port of `cex-trading-bot` on Kraken. Each **manager** trades its own
budget on one pair, with **limit orders only**, in a loop:

1. buys with everything it has at **-`buy_below`%** of the last price (GTD order: not filled
   after `buy_order_timeout`, it expires and is placed again from the new price; with
   `buy_trailing`, it's placed again as soon as the price rose `buy_trailing`% above the one it was
   placed from)
2. sells everything it bought for **what it cost + `sell_above`%**, fees included (GTC order)
3. starts again with what the sale brought: every cycle ends with more than it started with

Its orders carry its id as Kraken **`userref`**: Kraken knows which order belongs to which
manager, whatever happens to the database. The fees are taken in the quote currency (`fciq`). A
manager only spends its own money, and only what the account has.

## Prerequisites

- Docker with Compose ([Docker](docker.md)), or [rustup](https://rustup.rs) to run it with cargo ([development](development.md))
- a Kraken API key with the Query Funds, Query Open Orders & Trades, Query Closed Orders & Trades
  and Create & Modify Orders permissions, never Withdraw Funds

## Configuration

The managers are in a YAML file ([example](../managers.example.yaml)),
the credentials in the environment:

```yaml
dry_run: true              # orders only validated by Kraken, never placed: false to trade
tick_interval: 5m
buy_order_timeout: 12h
buy_trailing: 0.5          # optional: a buy left behind by a rise of 0.5% is placed again
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
- `buy_trailing` compares the price with the one each buy was placed from, recorded with it:
  editing `buy_below` doesn't cancel the open buys. A dry run never cancels. A cancellation Kraken
  refuses (permission, rate limit) is a failure of the manager, notified once; the buy placed again
  is notified, not the cancelled one.
- The configuration is checked at start: unique ids, known pairs, valid percentages.

| Variable | Default | |
|----------|---------|-|
| `KRAKEN_API_KEY`, `KRAKEN_API_SECRET` | | the API key, see the permissions above |
| `KRAKEN_BOT_ACCOUNTS` | `main` | the [Kraken accounts](#several-accounts) to trade on, separated by commas |
| `KRAKEN_API_KEY_<NAME>`, `KRAKEN_API_SECRET_<NAME>` | | the API key of each account of the list |
| `KRAKEN_BOT_CONFIG` | `managers.yaml` | the YAML file |
| `KRAKEN_BOT_DATABASE` | `kraken-bot.sqlite` | the SQLite database |
| `KRAKEN_BOT_NTFY_TOPIC`, `KRAKEN_BOT_NTFY_TOKEN` | | the [notifications](#notifications) |
| `KRAKEN_BOT_API_HOST` | `https://api.kraken.com` | another host speaking the API of Kraken: a mocked Kraken in the tests |
| `KRAKEN_BOT_PUSH_URL` | | its [web backend](#web-backend): what the dashboard shows is pushed there at each tick |
| `KRAKEN_BOT_PUSH_KEY` | | needed with `KRAKEN_BOT_PUSH_URL`: the key the pushes are signed with, the backend's too |
| `RUST_LOG` | `info` | |

## Several accounts

One bot can trade on several Kraken accounts at once, each with its own key and its own money. They
all run the managers of the same `managers.yaml`, and each has its own orders, positions and virtual
managers. In `.env`:

```sh
KRAKEN_BOT_ACCOUNTS=main,alice
KRAKEN_API_KEY=...            # main: KRAKEN_API_KEY_MAIN works too
KRAKEN_API_SECRET=...
KRAKEN_API_KEY_ALICE=...
KRAKEN_API_SECRET_ALICE=...
```

- A name is 1 to 12 of `a-z`, `0-9`, `-` and `_`; `-` becomes `_` in the variables.
- **One key per account.** Two keys of one Kraken account would run its managers twice with the same
  ids, mixing their orders. The bot refuses the same key twice, but can't tell two keys of one account
  apart: that one is on you.
- The budgets are per account: each needs the money of all the managers of the file.
- History before several accounts belongs to `main`, so keep that name for the first account.
- An account taken off the list stops trading: its open orders stay on Kraken, its last values stay in
  the dashboard, added to the others.
- The accounts tick at the same time and share a limit of calls to Kraken: the ticks take longer. A
  refused key fails the ticks of its account only. The notifications start with `[name]`.

## Virtual managers

With `virtual_managers: true` (`false` by default), the money of the account no manager uses
isn't left idle:

```yaml
virtual_managers: true
```

- **The margin**, per quote currency: what the account has available, minus what each manager
  has to buy with (its budget, then what its sales brought). The quote held by open buy orders
  isn't available, so it isn't counted twice. The gains of a manager stay its own: they are never
  a margin.
- **At each tick**, if the margin pays for the budget of an active manager of the YAML file, the
  bot adds a copy of one of them, picked at random: same pair, budget, `buy_below` and
  `sell_above`. One at most per tick, as long as the margin lasts: the money of the account is the only limit.
  It trades from that tick, like any manager.
- **Their ids** are `1000000 + 1000 × order of creation + id of the copied manager`:
  `1003002` is the 4th virtual manager added, a copy of manager 2. The ids of the YAML file must
  then be below 1000, which the check of the configuration enforces. An id is never reused.
- **Deleted** when the account can't pay for their buy 3 ticks in a row, money withdrawn for
  instance: a virtual manager buys only when it holds nothing, so it's deleted with nothing but
  its quote, which goes back to the margin. One which holds a position waits for its sale, one
  which keeps an amount too small to be sold is kept with it. The managers of the YAML file are
  never deleted.
- **Rebuilt** when the database doesn't know them, lost for instance: at the first tick, the bot
  searches every order of the account (open and closed) for the userrefs of virtual managers. The
  id tells the copied manager, the prices of the orders check its parameters: the budget from the
  first buy, `sell_above` from the last sell and the cost of what it sold, `buy_below` from the
  last buy and the price of the market when it was placed (the public trades of Kraken). A
  parameter the orders contradict, the copied manager having changed, is taken from the orders.
  A virtual manager which holds nothing (no open order, nothing to sell) is rebuilt deleted, its
  id staying taken. One whose copied manager is gone is rebuilt `closing`: it ends its open
  order, sells what it holds, then is deleted. Without its copied manager nor any sell, it sells
  for what its buy cost + 1.5%, fees included.
- They are stored in the `virtual_managers` table of the database, deleted ones included.

### The crypto no manager holds

With `sell_free_base: true` too (`false` by default: the crypto of the account is left alone), the
crypto of the account no manager holds, a deposit of XBT for instance, is sold by virtual managers:

```yaml
virtual_managers: true
sell_free_base: true
```

- **The free crypto**, per base asset: what the account has available, minus what each manager
  holds and hasn't put on sale yet. The crypto of the open sell orders isn't available.
- **At each tick**, the first active manager of the YAML file on a pair of that crypto, among the
  ones with the fewest such copies (so each gets one in turn), gets a copy which starts **as if
  its buy had just been filled**: the price of its buy, `buy_below`% under the price of the
  market, and what its budget buys at that price, fees included. If the free crypto pays for it,
  the copy holds that amount, which cost its budget. One at most per tick, as long as the free
  crypto lasts.
- **It sells it** like any manager: for its budget + `sell_above`%, fees deducted. So from the
  price its buy would have had, not from the price of the market: about `sell_above - buy_below`%
  above it, plus the fees. Then it buys again with what it brought, and trades like the others.
  With `buy_below` above `sell_above`, the sell is under the price of the market: filled at once.
- **Deleted** when its crypto leaves the account before it sold anything (withdrawn), 3 ticks in a
  row, like a virtual manager which can't buy.
- **Rebuilt** from its orders as the others: a first order which is a sell tells a copy which
  started with crypto, the amount of that sell, its budget what the sell brings back less
  `sell_above`%.
- The amount it started with is the `holding` column of `virtual_managers`.

## Indicators of the market

At each tick, the bot reads the candles of 15 minutes of the last 24 hours of each pair (public
OHLC of Kraken) and computes, with [`yata`](https://crates.io/crates/yata):

| Indicator | |
|-----------|-|
| `atr_percent` | average true range of 14 candles, in % of the close: how much a candle moves |
| `ema_fast`, `ema_slow` | EMAs of the closes, 3 and 12 hours |
| `slope_24h_percent` | slope of the linear regression of the closes over 24 hours, in % per 24 hours |
| `range_24h_percent`, `change_24h_percent` | highest high minus lowest low, and the change, over 24 hours |

They're logged (`indicators`, with a trend: rising, falling or mixed) and recorded in the
`indicators` table, once 12 hours of candles exist. **Nothing decides on them yet**: they're
there to see what they say before tying `buy_below` to the volatility or pausing the buys in a
falling market. A failure to read the candles is only logged.

## Web backend

To follow the bot from elsewhere than its machine, it pushes what its dashboard shows to
[kraken-dashboard-web](https://github.com/crypt0p1a/kraken-dashboard-web), its web backend, when
`.env` sets `KRAKEN_BOT_PUSH_URL` (e.g. `http://dashboard-web:8080`). Without it, nothing is
pushed, nothing goes on the network.

- **What**: every screen of the dashboard, read from the database as the dashboard reads it
  and bucketed: each range (1H, 24H, 7D, 30D, ALL) of all the accounts added up, and of each
  account when there are several. `PUT /state`, JSON compressed with gzip
  (`Content-Encoding: gzip`): for the demo database (8 managers, 3 pairs, 7 days), 3.1 MB of JSON,
  about 720 KB pushed.
- **When**: as soon as the bot starts (the last state of its database), at the end of each tick,
  and when a forced tick starts.
- **Signed**: with `KRAKEN_BOT_PUSH_KEY`, 32 characters at least (`openssl rand -hex 32`), the
  `KRAKEN_DASHBOARD_WEB_PUSH_KEY` of the backend: the same key, never sent. Each push carries when
  it was signed (`X-Kraken-Push-At`, UTC milliseconds) and an HMAC-SHA256 of that time and of the
  body as sent (`X-Kraken-Push-Signature: sha256=<hex>`). The backend takes the pushes of the bot
  only, never one heard on the way and sent again, nor one older than the last it took. With a URL
  and no key (or one too short), the bot trades and pushes nothing, which it tells in its logs; a
  push refused, it logs why, as the backend tells it (not its key, a clock too far from its own).
- **Read only**: nothing comes back from the backend to the bot.
- **Never in the way**: the pusher has a thread and a connection of its own. The backend away or
  slow, the bot goes on trading, says so once in its logs, and pushes its last state again every 30
  seconds until the backend is back. Stopping, it pushes its last tick, 25 seconds at most.

The backend has nothing authenticated yet: run it on a network of your own only.

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
| `indicators` | the [indicators](#indicators-of-the-market) of the pairs, at each tick |
| `account_snapshots` | what the whole account is worth at each tick, per quote currency of the managers: the quote, plus the bases of their pairs at the last price |
| `accounts` | the Kraken accounts, by name. Every table but `tickers` and `indicators` has an `account` column |
| `commands` | what is asked of the bot from outside: `forced_tick`, `refresh` when the dashboard asks for a tick, `pending` while the bot does it, `idle` once done. The bot looks for it every 10 seconds between two ticks |
| `sync_state` | where the synchronization of the orders is |
| `virtual_managers` | the [virtual managers](#virtual-managers), deleted ones included |

The amounts are stored as text, to stay exact.

Read with SQLite from another container: [Docker](docker.md#the-database).
