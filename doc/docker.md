# Docker

The bot runs from one image (distroless, non-root, amd64 and arm64) holding its two binaries: the
bot and its [dashboard](dashboard.md). Docker Compose starts the bot; the dashboard runs in its
container when wanted.

## Getting started

```sh
cp managers.example.yaml managers.yaml   # the managers: a dry run until dry_run: false
cp .env.example .env                     # the Kraken API key, never with Withdraw Funds
docker compose run --rm kraken-trading-bot --check   # checks the configuration, the pairs and the key
docker compose up -d --build
docker compose exec kraken-trading-bot kraken-dashboard   # the dashboard, q to quit
```

What goes in `managers.yaml` and `.env`: [the configuration](trading-bot.md#configuration). The
bot is a dry run, its orders only validated by Kraken, until `managers.yaml` says `dry_run: false`.

## Every day

| Command | |
|---------|-|
| `docker compose exec kraken-trading-bot kraken-dashboard` | the [dashboard](dashboard.md), on the database of the bot; `q` to quit |
| `docker compose logs -f` | the logs of the bot |
| `docker compose ps` | whether it runs |
| `docker compose run --rm kraken-trading-bot --check` | checks the configuration, the pairs and the API keys on Kraken (their permissions: none that withdraws), then stops. Nothing is placed |
| `docker compose restart` | after a change of `managers.yaml` or `.env` |
| `docker compose stop` | stops the bot once the tick in progress ended |
| `git pull && docker compose up -d --build` | a new version: the image built again, the bot started again on its database |

## What's where

| On the machine | In the container | What |
|----------------|------------------|------|
| `./managers.yaml` | `/config/managers.yaml`, read only | the managers |
| `.env` | the environment of the bot | the keys, the [notifications](trading-bot.md#notifications), the [web backend](trading-bot.md#web-backend) |
| volume `kraken-bot-data` | `/data` | the [database](trading-bot.md#database) `kraken-bot.sqlite`, and the screen of the dashboard next to it |

The image sets `KRAKEN_BOT_CONFIG=/config/managers.yaml` and
`KRAKEN_BOT_DATABASE=/data/kraken-bot.sqlite`.

## The database

Read with SQLite from another container (`docker volume ls` tells the name of the volume, after
the folder of the compose):

```sh
docker run --rm -v kraken-trading-bot_kraken-bot-data:/data -it keinos/sqlite3 sqlite3 /data/kraken-bot.sqlite \
  "SELECT id, pair, budget, value, quote_available, base_available FROM managers"
```

Kept elsewhere, a copy of the volume:

```sh
docker run --rm -v kraken-trading-bot_kraken-bot-data:/data -v "$PWD:/backup" busybox \
  tar czf /backup/kraken-bot-data.tar.gz -C /data .
```

## Following the bot from elsewhere

With `KRAKEN_BOT_PUSH_URL` in `.env`, the bot pushes what its dashboard shows to its web backend,
[kraken-dashboard-web](https://github.com/crypt0p1a/kraken-dashboard-web), at each tick: see
[the web backend](trading-bot.md#web-backend). Without it, nothing goes on the network but the
calls to Kraken and the notifications.

## Keeping the machine awake

The bot runs in Docker: while the machine sleeps, it's frozen and places nothing, its orders
already on Kraken staying there. It catches up at the first tick after the wake up. To keep the
machine awake while it runs:

```sh
# macOS: no sleep as long as the command runs (-i: idle, -s: on mains power), the screen may turn off
caffeinate -is docker compose logs -f
caffeinate -is docker compose exec kraken-trading-bot kraken-dashboard

# macOS: never on mains power, until it's set back (`pmset -g` shows the current value first)
sudo pmset -c sleep 0

# Linux (systemd): no sleep as long as the command runs
systemd-inhibit --what=idle:sleep --why="kraken-trading-bot" docker compose logs -f

# Windows (PowerShell, as administrator): never on mains power, until it's set back
powercfg /change standby-timeout-ac 0
```

Closing the lid of a laptop still puts it to sleep, unless it's on mains power with an external
screen. A machine that never sleeps, a small server, is the right place for a bot trading real
money.
