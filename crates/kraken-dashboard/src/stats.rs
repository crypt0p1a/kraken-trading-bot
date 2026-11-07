//! What the dashboard computes from the data of the bot: the P&L realized by the sells, the
//! totals by quote currency, the P&L over time, the pair of each manager

use crate::data::{
    Account, AccountPoint, Data, Manager, ManagerKey, Market, Order, Side, Snapshot, percent,
    to_f64,
};
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The quote currencies Kraken names its pairs with, the longest first: USDC before USD
const QUOTES: [&str; 13] = [
    "PYUSD", "USDT", "USDC", "USD", "EUR", "GBP", "CHF", "CAD", "JPY", "AUD", "XBT", "ETH", "DAI",
];

/// The pair as the orders name it, whatever names it: "XBT/USD", "xbtusd" and the Kraken key
/// "XXBTZUSD" are all "XBTUSD"
pub fn canonical(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|it| *it != '/')
        .collect::<String>()
        .to_ascii_uppercase();
    let bytes = name.as_bytes();
    // the legacy keys of Kraken: X + base of 3 letters, then Z (fiat) or X (crypto) + quote
    if bytes.len() == 8 && bytes[0] == b'X' && matches!(bytes[4], b'Z' | b'X') {
        return format!("{}{}", &name[1..4], &name[5..8]);
    }
    name
}

/// An asset as the pairs name it: the legacy "ZEUR" and "XXBT" of Kraken are "EUR" and "XBT"
pub fn asset(name: &str) -> &str {
    match name.as_bytes() {
        [b'X' | b'Z', _, _, _] => &name[1..],
        _ => name,
    }
}

/// The base and the quote of a pair: ("XBT", "USD") for "XBTUSD"
pub fn split(pair: &str) -> (String, String) {
    let pair = canonical(pair);
    for quote in QUOTES {
        if let Some(base) = pair.strip_suffix(quote).filter(|it| !it.is_empty()) {
            return (base.to_string(), quote.to_string());
        }
    }
    (pair, String::new())
}

/// A tile of a heatmap: what something weighs, and how it moved over the range
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub name: String,
    /// In the quote
    pub value: f64,
    /// In %, None when the range doesn't have two points to compare
    pub percent: Option<f64>,
}

/// Under a cent there's nothing to show: what's left of a sale
const NOTHING: f64 = 0.01;

/// The tiles worth something, the heaviest first, then by name
fn tiles(mut tiles: Vec<Tile>) -> Vec<Tile> {
    tiles.retain(|it| it.value >= NOTHING);
    tiles.sort_by(|a, b| {
        b.value
            .total_cmp(&a.value)
            .then_with(|| a.name.cmp(&b.name))
    });
    tiles
}

/// What the account was made of over time, a layer per crypto, then its quote
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Allocation {
    /// The cryptos the managers held, by name, then the quote, then [`OTHER`] if there's some
    pub names: Vec<String>,
    /// From the oldest: (UTC milliseconds, what each name is worth)
    pub points: Vec<(f64, Vec<f64>)>,
}

/// What the account holds that no manager bought, a layer of its own
pub const OTHER: &str = "OTHER";

/// A share of the account under this isn't something held: the managers and the account aren't
/// valued at the same instant, nor exactly at the same price
const OTHER_NOISE: f64 = 0.005;

/// A sell, and what it earned against what its base cost
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub manager_id: ManagerKey,
    /// UTC seconds of the sell
    pub at: f64,
    pub pnl: Decimal,
    pub percent: f64,
}

/// What a manager did, from its orders
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Performance {
    /// What the sells earned, fees included, against what their base cost
    pub realized: Decimal,
    pub fees: Decimal,
    /// Quote exchanged, buys and sells
    pub volume: Decimal,
    /// Oldest first
    pub trades: Vec<Trade>,
}

impl Performance {
    pub fn wins(&self) -> usize {
        self.trades
            .iter()
            .filter(|it| it.pnl > Decimal::ZERO)
            .count()
    }
}

/// Replays the orders of a manager as the ledger of the bot does: the cost of the position is
/// shared by what's sold of it
pub fn performance(orders: &[&Order]) -> Performance {
    let mut sorted = orders.to_vec();
    sorted.sort_by(|a, b| {
        a.open_time
            .total_cmp(&b.open_time)
            .then_with(|| a.txid.cmp(&b.txid))
    });

    let mut result = Performance::default();
    let mut position_base = Decimal::ZERO;
    let mut position_cost = Decimal::ZERO;

    for order in sorted {
        let executed = order.volume_executed;
        result.fees += order.fee;
        result.volume += order.cost;

        match order.side {
            Side::Buy => {
                position_base += executed;
                position_cost += order.cost + order.fee;
            }
            Side::Sell if executed > Decimal::ZERO => {
                let sold = executed.min(position_base);
                let basis = if position_base > Decimal::ZERO {
                    position_cost * sold / position_base
                } else {
                    Decimal::ZERO
                };
                let pnl = order.cost - order.fee - basis;
                result.realized += pnl;
                result.trades.push(Trade {
                    manager_id: order.manager_id,
                    at: order.close_time.unwrap_or(order.open_time),
                    pnl,
                    percent: percent(pnl, basis),
                });

                if executed >= position_base {
                    position_base = Decimal::ZERO;
                    position_cost = Decimal::ZERO;
                } else {
                    position_cost -= basis;
                    position_base -= executed;
                }
            }
            Side::Sell => {}
        }
    }
    result
}

/// What a period earned against the one before it
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Growth {
    /// UTC milliseconds of the last point of the period
    pub at: f64,
    /// In the quote
    pub earned: f64,
    /// In % of what the portfolio was worth at the end of the period before
    pub percent: f64,
}

/// The growth of each period, from `pnl`, the P&L over time. What's earned comes from the P&L, so
/// money deposited or an account joining isn't growth. It's compared to `capital`, what the
/// portfolio was worth then, else to `budget` and the P&L when `capital` doesn't go back that far.
/// A period without any point, the bot being stopped, is skipped: the next one covers it
pub fn growth(pnl: &[(f64, f64)], capital: &[(f64, f64)], budget: f64, period: i64) -> Vec<Growth> {
    let mut closes: Vec<(f64, f64)> = Vec::new();
    let mut current = None;
    for point in pnl {
        let bucket = (point.0 / period as f64).floor() as i64;
        if current == Some(bucket) {
            if let Some(last) = closes.last_mut() {
                *last = *point;
            }
        } else {
            closes.push(*point);
            current = Some(bucket);
        }
    }
    closes
        .windows(2)
        .map(|it| {
            let (before, now) = (it[0], it[1]);
            let index = capital.partition_point(|point| point.0 <= before.0);
            let worth = match index {
                0 => budget + before.1,
                _ => capital[index - 1].1,
            };
            let earned = now.1 - before.1;
            Growth {
                at: now.0,
                earned,
                percent: if worth > 0.0 {
                    earned / worth * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// The moves of the market the scenarios look at, in %
pub const MOVES: [i32; 7] = [-10, -5, -2, 0, 2, 5, 10];

/// What the portfolio would be worth if every price moved at once by a percentage
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    /// In %
    pub moved: i32,
    pub value: Decimal,
    /// The open buys the move fills, at their limit
    pub buys: usize,
    /// The open sells the move fills, for what they cost + `sell_above`%, fees included
    pub sells: usize,
}

/// An amount earned from a time until another (UTC milliseconds), per day. A first day counts as a
/// whole one: what a first hour earned isn't 24 times that
fn per_day(earned: f64, since: f64, now: f64) -> f64 {
    earned / ((now - since) / 86_400_000.0).max(1.0)
}

/// The managers of a quote currency, together
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Totals {
    pub quote: String,
    pub managers: usize,
    pub budget: Decimal,
    pub value: Decimal,
    /// The value once every open buy filled and every position sold at its target
    pub target: Decimal,
    pub realized: Decimal,
    pub fees: Decimal,
    pub volume: Decimal,
    /// Value of the base held, at the last prices
    pub exposure: Decimal,
    pub open_orders: usize,
    pub trades: usize,
    pub wins: usize,
}

impl Totals {
    /// What the managers and the account are worth now, then once every open buy filled and every
    /// position sold at its target. The rest of the account stays as it is
    pub fn current_and_target(&self, account: Option<&Account>) -> (Decimal, Decimal) {
        let current = account.map_or(self.value, Account::value);
        (current, current - self.value + self.target)
    }

    pub fn pnl(&self) -> Decimal {
        self.value - self.budget
    }

    pub fn pnl_percent(&self) -> f64 {
        percent(self.pnl(), self.budget)
    }

    /// What the base held would earn or lose if sold at the last prices
    pub fn unrealized(&self) -> Decimal {
        self.pnl() - self.realized
    }

    pub fn exposure_percent(&self) -> f64 {
        percent(self.exposure, self.value)
    }
}

/// The data, with what's computed from it
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub data: Data,
    pub performances: HashMap<ManagerKey, Performance>,
    /// The quote currency of each manager
    pub quotes: HashMap<ManagerKey, String>,
    /// The index in `data.markets` of the pair of each manager
    pub markets: HashMap<ManagerKey, usize>,
    /// By quote, the one with the most budget first
    pub totals: Vec<Totals>,
    /// The sells of every manager, oldest first
    pub trades: Vec<Trade>,
}

impl Model {
    pub fn new(data: Data) -> Self {
        let mut by_manager: HashMap<ManagerKey, Vec<&Order>> = HashMap::new();
        for order in &data.orders {
            by_manager.entry(order.manager_id).or_default().push(order);
        }
        let performances: HashMap<ManagerKey, Performance> = by_manager
            .iter()
            .map(|(id, orders)| (*id, performance(orders)))
            .collect();

        let quotes: HashMap<ManagerKey, String> = data
            .managers
            .iter()
            .map(|it| (it.id, split(&it.pair).1))
            .collect();
        let markets = data
            .managers
            .iter()
            .filter_map(|manager| Some((manager.id, market_of(manager, &data.markets)?)))
            .collect();

        let mut trades: Vec<Trade> = performances
            .values()
            .flat_map(|it| it.trades.iter().cloned())
            .collect();
        trades.sort_by(|a, b| a.at.total_cmp(&b.at));

        let mut model = Self {
            data,
            performances,
            quotes,
            markets,
            totals: Vec::new(),
            trades,
        };

        let mut by_quote: BTreeMap<&str, Vec<ManagerKey>> = BTreeMap::new();
        for manager in &model.data.managers {
            by_quote
                .entry(model.quotes[&manager.id].as_str())
                .or_default()
                .push(manager.id);
        }
        let mut totals: Vec<Totals> = by_quote
            .into_values()
            .map(|ids| model.totals_of(&ids))
            .collect();
        totals.sort_by(|a, b| b.budget.cmp(&a.budget).then_with(|| a.quote.cmp(&b.quote)));
        model.totals = totals;
        model
    }

    /// The managers of `ids` together, all of one quote currency
    pub fn totals_of(&self, ids: &[ManagerKey]) -> Totals {
        let mut total = Totals::default();
        for manager in self.data.managers.iter().filter(|it| ids.contains(&it.id)) {
            total.quote.clone_from(&self.quotes[&manager.id]);
            total.managers += 1;
            total.budget += manager.budget;
            total.value += manager.value;
            total.target += manager.target();
            total.exposure += manager.base() * manager.last_price;
            total.open_orders += self.open_orders_of(manager.id).count();
            if let Some(performance) = self.performances.get(&manager.id) {
                total.realized += performance.realized;
                total.fees += performance.fees;
                total.volume += performance.volume;
                total.trades += performance.trades.len();
                total.wins += performance.wins();
            }
        }
        total
    }

    pub fn manager(&self, id: ManagerKey) -> Option<&Manager> {
        self.data.managers.iter().find(|it| it.id == id)
    }

    pub fn market_of(&self, id: ManagerKey) -> Option<&Market> {
        self.markets
            .get(&id)
            .map(|index| &self.data.markets[*index])
    }

    /// How far the market is from an open order, in % of the last price: what the price has to
    /// move for the order to fill, negative for a buy below the market. None for a closed order
    pub fn to_fill(&self, order: &Order) -> Option<f64> {
        if order.is_final {
            return None;
        }
        let limit = to_f64(order.limit_price?);
        let last = self
            .market_of(order.manager_id)
            .map(|it| it.last)
            .or_else(|| self.manager(order.manager_id).map(|it| it.last_price))
            .map(to_f64)
            .filter(|it| *it > 0.0)?;
        Some((limit - last) / last * 100.0)
    }

    /// What the sells of these managers earned per day, since their first order
    pub fn realized_per_day(&self, ids: &[ManagerKey], now: i64) -> f64 {
        let first = self
            .data
            .orders
            .iter()
            .filter(|it| ids.contains(&it.manager_id))
            .map(|it| it.open_time)
            .fold(f64::MAX, f64::min);
        per_day(
            to_f64(self.totals_of(ids).realized),
            first * 1000.0,
            now as f64,
        )
    }

    /// The portfolio of these managers, and of the account when given, for each move of the market.
    /// One move at once, from the last prices: what the bot would place next isn't played. A cash
    /// no manager owns stays as it is, a base no manager owns moves with the market
    pub fn scenarios(&self, ids: &[ManagerKey], account: Option<&Account>) -> Vec<Scenario> {
        let totals = self.totals_of(ids);
        let (rest, held_outside) = account.map_or((Decimal::ZERO, Decimal::ZERO), |it| {
            (
                it.value() - totals.value,
                (it.holdings - totals.exposure).max(Decimal::ZERO),
            )
        });
        MOVES
            .iter()
            .map(|moved| {
                let move_ = Decimal::from(*moved) / Decimal::ONE_HUNDRED;
                let mut scenario = Scenario {
                    moved: *moved,
                    value: rest + held_outside * move_,
                    buys: 0,
                    sells: 0,
                };
                for manager in self.data.managers.iter().filter(|it| ids.contains(&it.id)) {
                    self.play(manager, move_, &mut scenario);
                }
                scenario
            })
            .collect()
    }

    /// Adds to the scenario what the manager would be worth after the move
    fn play(&self, manager: &Manager, move_: Decimal, scenario: &mut Scenario) {
        let price = manager.last_price * (Decimal::ONE + move_);
        let mut quote = manager.quote_available + manager.quote_in_buy_order;
        let mut base = manager.base();
        // a manager has one open order at most: it waits for its end before placing another
        if let Some(order) = self.open_orders_of(manager.id).last()
            && let Some(limit) = order.limit_price
        {
            let remaining = order.volume - order.volume_executed;
            match order.side {
                Side::Buy if move_ < Decimal::ZERO && price <= limit => {
                    quote -= manager.quote_in_buy_order;
                    base += remaining;
                    scenario.buys += 1;
                }
                Side::Sell if move_ > Decimal::ZERO && price >= limit && !base.is_zero() => {
                    let sold = remaining.min(base);
                    let cost = manager.position_cost * sold / base;
                    quote += cost * (Decimal::ONE + manager.sell_above / Decimal::ONE_HUNDRED);
                    base -= sold;
                    scenario.sells += 1;
                }
                _ => {}
            }
        }
        scenario.value += quote + base * price;
    }

    /// The growth of these managers per period, against the whole account in the quote when given,
    /// against what they're worth together else
    pub fn growth_of(&self, ids: &[ManagerKey], quote: Option<&str>, period: i64) -> Vec<Growth> {
        let capital = quote.map_or_else(Vec::new, |it| self.account_series(it));
        growth(
            &self.pnl_series(ids),
            &capital,
            to_f64(self.totals_of(ids).budget),
            period,
        )
    }

    /// What the whole account was worth in the quote ("EUR") over the range, oldest first
    pub fn account_series(&self, quote: &str) -> Vec<(f64, f64)> {
        self.data
            .account_series
            .iter()
            .filter(|it| asset(&it.quote) == quote)
            .map(|it| (it.at as f64, it.value))
            .collect()
    }

    /// What the manager was worth over the range, oldest first
    pub fn manager_series(&self, id: ManagerKey) -> Vec<(f64, f64)> {
        self.data
            .snapshots
            .iter()
            .filter(|it| it.manager_id == id)
            .map(|it| (it.at as f64, it.value))
            .collect()
    }

    /// The last value of the whole account in the quote ("EUR"), if the bot records it
    pub fn account(&self, quote: &str) -> Option<&Account> {
        self.data
            .accounts
            .iter()
            .find(|it| asset(&it.quote) == quote)
    }

    /// What the managers shown hold of each pair, with how its price moved over the range
    pub fn pair_tiles(&self, quote: &str) -> Vec<Tile> {
        let mut held: BTreeMap<String, f64> = BTreeMap::new();
        for manager in &self.data.managers {
            if self.quotes.get(&manager.id).is_some_and(|it| it == quote) {
                *held.entry(canonical(&manager.pair)).or_default() +=
                    to_f64(manager.base() * manager.last_price);
            }
        }
        tiles(
            held.into_iter()
                .map(|(name, value)| {
                    let prices = self
                        .data
                        .markets
                        .iter()
                        .find(|it| canonical(&it.key) == name)
                        .map(|it| it.prices.as_slice())
                        .unwrap_or_default();
                    let percent = match (prices.first(), prices.last()) {
                        (Some(first), Some(last)) if prices.len() > 1 && first.1 > 0.0 => {
                            Some((last.1 / first.1 - 1.0) * 100.0)
                        }
                        _ => None,
                    };
                    Tile {
                        name,
                        value,
                        percent,
                    }
                })
                .collect(),
        )
    }

    /// What each account is worth, whatever account is shown, with how it grew over the range. From
    /// its P&L, like the growth: money deposited isn't growth
    pub fn account_tiles(&self, quote: &str) -> Vec<Tile> {
        tiles(
            self.data
                .account_lines
                .iter()
                .map(|account| {
                    let budgets: HashMap<ManagerKey, f64> = account
                        .managers
                        .iter()
                        .filter(|(_, pair, _)| split(pair).1 == quote)
                        .map(|(id, _, budget)| (*id, *budget))
                        .collect();
                    let snapshots: Vec<&Snapshot> = account
                        .snapshots
                        .iter()
                        .filter(|it| budgets.contains_key(&it.manager_id))
                        .collect();
                    let capital: Vec<(f64, f64)> = account
                        .series
                        .iter()
                        .filter(|it| asset(&it.quote) == quote)
                        .map(|it| (it.at as f64, it.value))
                        .collect();
                    let pnl = pnl_series(&snapshots, &budgets);
                    // the first point against the last one: a single period
                    let ends: Vec<(f64, f64)> = match (pnl.first(), pnl.last()) {
                        (Some(first), Some(last)) if pnl.len() > 1 => vec![*first, *last],
                        _ => Vec::new(),
                    };
                    let moved = growth(&ends, &capital, budgets.values().sum(), 1);
                    Tile {
                        name: account.name.clone(),
                        value: capital.last().map_or(0.0, |it| it.1),
                        percent: moved.first().map(|it| it.percent),
                    }
                })
                .collect(),
        )
    }

    /// What the whole account was made of, at `columns` moments from `since` to `now`
    pub fn allocation(&self, quote: &str, since: i64, now: i64, columns: usize) -> Allocation {
        let bases: HashMap<ManagerKey, String> = self
            .data
            .managers
            .iter()
            .filter(|it| self.quotes.get(&it.id).is_some_and(|it| it == quote))
            .map(|it| (it.id, split(&it.pair).0))
            .collect();
        let mut cryptos: Vec<&str> = bases.values().map(String::as_str).collect();
        cryptos.sort_unstable();
        cryptos.dedup();

        let accounts: Vec<&AccountPoint> = self
            .data
            .account_series
            .iter()
            .filter(|it| asset(&it.quote) == quote)
            .collect();
        let (mut next_snapshot, mut next_account) = (0, 0_usize);
        let mut held: HashMap<ManagerKey, f64> = HashMap::new();
        let mut points = Vec::new();
        for column in 0..columns {
            let at = since + (now - since) * (column as i64 + 1) / columns as i64;
            while let Some(snapshot) = self
                .data
                .snapshots
                .get(next_snapshot)
                .filter(|it| it.at <= at)
            {
                held.insert(snapshot.manager_id, snapshot.held);
                next_snapshot += 1;
            }
            while accounts.get(next_account).is_some_and(|it| it.at <= at) {
                next_account += 1;
            }
            // nothing recorded yet at this moment, and each counts for its last value known
            let Some(account) = next_account.checked_sub(1).map(|it| accounts[it]) else {
                continue;
            };
            let mut values: Vec<f64> = cryptos
                .iter()
                .map(|crypto| {
                    held.iter()
                        .filter(|(id, _)| bases.get(*id).is_some_and(|it| it == crypto))
                        .map(|(_, it)| it)
                        .sum()
                })
                .collect();
            let other = account.value - account.cash - values.iter().sum::<f64>();
            values.push(account.cash);
            values.push(if other > account.value * OTHER_NOISE {
                other
            } else {
                0.0
            });
            points.push((at as f64, values));
        }

        let names = cryptos
            .into_iter()
            .chain([quote, OTHER])
            .map(str::to_string);
        // the layers never seen in the range say nothing
        let kept: Vec<(usize, String)> = names
            .enumerate()
            .filter(|(index, _)| points.iter().any(|(_, it)| it[*index] > 0.0))
            .collect();
        Allocation {
            points: points
                .into_iter()
                .map(|(at, values)| (at, kept.iter().map(|(it, _)| values[*it]).collect()))
                .collect(),
            names: kept.into_iter().map(|(_, it)| it).collect(),
        }
    }

    /// The quote of the account no open order holds: what can be spent right now
    pub fn free_cash(&self, account: &Account) -> Decimal {
        let quote = asset(&account.quote);
        let in_buy_orders: Decimal = self
            .data
            .managers
            .iter()
            .filter(|it| self.quotes.get(&it.id).is_some_and(|it| it == quote))
            .map(|it| it.quote_in_buy_order)
            .sum();
        (account.cash - in_buy_orders).max(Decimal::ZERO)
    }

    pub fn open_orders_of(&self, id: ManagerKey) -> impl Iterator<Item = &Order> {
        self.data
            .orders
            .iter()
            .filter(move |it| it.manager_id == id && !it.is_final)
    }

    /// The last values of a manager, oldest first, for its sparkline
    pub fn values_of(&self, id: ManagerKey) -> Vec<f64> {
        self.data
            .snapshots
            .iter()
            .filter(|it| it.manager_id == id)
            .map(|it| it.value)
            .collect()
    }

    /// The P&L over time of the managers of `ids`: (UTC milliseconds, quote)
    pub fn pnl_series(&self, ids: &[ManagerKey]) -> Vec<(f64, f64)> {
        let budgets: HashMap<ManagerKey, f64> = self
            .data
            .managers
            .iter()
            .filter(|it| ids.contains(&it.id))
            .map(|it| (it.id, to_f64(it.budget)))
            .collect();
        let snapshots: Vec<&Snapshot> = self
            .data
            .snapshots
            .iter()
            .filter(|it| budgets.contains_key(&it.manager_id))
            .collect();
        pnl_series(&snapshots, &budgets)
    }
}

/// The market of the manager: by its name, else by its last price, the one of the same tick
fn market_of(manager: &Manager, markets: &[Market]) -> Option<usize> {
    let pair = canonical(&manager.pair);
    markets
        .iter()
        .position(|it| canonical(&it.key) == pair)
        .or_else(|| markets.iter().position(|it| it.last == manager.last_price))
}

/// Sums the P&L of the managers at each time: a manager counts from its first point, with its last
/// value known since
pub fn pnl_series(snapshots: &[&Snapshot], budgets: &HashMap<ManagerKey, f64>) -> Vec<(f64, f64)> {
    // the managers of a tick are saved a few milliseconds apart: together, they're one point
    const TICK: i64 = 5_000;
    let times: BTreeSet<i64> = snapshots.iter().map(|it| it.at / TICK).collect();

    let mut last: HashMap<ManagerKey, f64> = HashMap::new();
    let mut index = 0;
    let mut sorted = snapshots.to_vec();
    sorted.sort_by_key(|it| it.at);

    let mut series = Vec::with_capacity(times.len());
    for time in times {
        let mut at = 0;
        while index < sorted.len() && sorted[index].at / TICK <= time {
            last.insert(sorted[index].manager_id, sorted[index].value);
            at = sorted[index].at;
            index += 1;
        }
        let pnl: f64 = last
            .iter()
            .map(|(id, value)| value - budgets.get(id).copied().unwrap_or_default())
            .sum();
        series.push((at as f64, pnl));
    }
    series
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    fn at(hours: f64) -> f64 {
        hours * HOUR as f64
    }

    #[test]
    fn compares_each_period_to_the_one_before() {
        // the last point of each hour counts
        let pnl = [
            (at(0.5), 0.0),
            (at(1.2), 5.0),
            (at(1.9), 10.0),
            (at(2.5), 4.0),
        ];
        let capital = [(at(0.5), 100.0), (at(1.9), 110.0)];
        let growth = growth(&pnl, &capital, 0.0, HOUR);
        assert_eq!(growth.len(), 2);
        assert_eq!(growth[0].earned, 10.0);
        assert_eq!(growth[0].percent, 10.0);
        assert_eq!(growth[0].at, at(1.9));
        // against what it was worth at the end of the hour before
        assert_eq!(growth[1].earned, -6.0);
        assert!((growth[1].percent + 6.0 / 110.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_deposit_is_not_growth() {
        // 100 deposited during the second hour: the P&L doesn't move
        let pnl = [(at(0.5), 2.0), (at(1.5), 2.0), (at(2.5), 3.0)];
        let capital = [(at(0.5), 100.0), (at(1.5), 200.0), (at(2.5), 201.0)];
        let growth = growth(&pnl, &capital, 0.0, HOUR);
        assert_eq!(growth[0].percent, 0.0);
        // and what's earned after it counts against the new amount
        assert_eq!(growth[1].percent, 0.5);
    }

    #[test]
    fn falls_back_on_the_budget_and_skips_the_empty_periods() {
        // nothing during the second and third hours; no value of the account that old
        let pnl = [(at(0.5), 0.0), (at(3.5), 3.0)];
        let growth = growth(&pnl, &[(at(3.5), 500.0)], 30.0, HOUR);
        assert_eq!(growth.len(), 1);
        assert_eq!(growth[0].percent, 10.0);
        assert!(super::growth(&pnl[..1], &[], 30.0, HOUR).is_empty());
        // nothing to compare to
        assert_eq!(super::growth(&pnl, &[], 0.0, HOUR)[0].percent, 0.0);
    }

    #[test]
    fn says_what_was_earned_per_day() {
        const DAY: f64 = 86_400_000.0;
        assert_eq!(per_day(6.0, 0.0, 2.0 * DAY), 3.0);
        // a first hour counts as a whole day, no order counts for nothing
        assert_eq!(per_day(6.0, 0.0, DAY / 24.0), 6.0);
        assert_eq!(per_day(0.0, f64::MAX, DAY), 0.0);
    }

    #[test]
    fn names_the_assets_as_the_pairs() {
        assert_eq!(asset("ZEUR"), "EUR");
        assert_eq!(asset("XXBT"), "XBT");
        assert_eq!(asset("USDC"), "USDC");
        assert_eq!(asset("SOL"), "SOL");
    }
    use std::str::FromStr;

    fn d(text: &str) -> Decimal {
        Decimal::from_str(text).unwrap()
    }

    fn order(time: f64, side: Side, executed: &str, cost: &str, fee: &str) -> Order {
        Order {
            txid: format!("O{time}"),
            manager_id: 1,
            pair: "XBTUSD".into(),
            side,
            status: "closed".into(),
            is_final: true,
            volume: d(executed),
            volume_executed: d(executed),
            cost: d(cost),
            fee: d(fee),
            limit_price: None,
            average_price: None,
            open_time: time,
            close_time: Some(time + 1.0),
        }
    }

    #[test]
    fn names_the_pairs_as_the_orders() {
        assert_eq!(canonical("XXBTZUSD"), "XBTUSD");
        assert_eq!(canonical("XETHXXBT"), "ETHXBT");
        assert_eq!(canonical("XBT/USD"), "XBTUSD");
        assert_eq!(canonical("solusd"), "SOLUSD");
        assert_eq!(canonical("XBTUSDC"), "XBTUSDC");
    }

    #[test]
    fn splits_the_base_and_the_quote() {
        assert_eq!(split("XXBTZUSD"), ("XBT".into(), "USD".into()));
        assert_eq!(split("XBTUSDC"), ("XBT".into(), "USDC".into()));
        assert_eq!(split("ETH/EUR"), ("ETH".into(), "EUR".into()));
        assert_eq!(split("SOLXBT"), ("SOL".into(), "XBT".into()));
        assert_eq!(split("USD"), ("USD".into(), String::new()));
    }

    #[test]
    fn realizes_the_pnl_of_the_sells() {
        let orders = [
            order(1.0, Side::Buy, "0.002", "100", "0.4"),
            // half of the position, which cost 50.2
            order(2.0, Side::Sell, "0.001", "52", "0.2"),
            order(3.0, Side::Sell, "0.001", "49", "0.2"),
        ];
        let performance = performance(&orders.iter().collect::<Vec<_>>());

        assert_eq!(performance.trades.len(), 2);
        assert_eq!(performance.trades[0].pnl, d("1.6"));
        assert_eq!(performance.trades[1].pnl, d("-1.4"));
        assert_eq!(performance.realized, d("0.2"));
        assert_eq!(performance.fees, d("0.8"));
        assert_eq!(performance.volume, d("201"));
        assert_eq!(performance.wins(), 1);
    }

    #[test]
    fn ignores_the_sells_not_executed() {
        let mut canceled = order(2.0, Side::Sell, "0", "0", "0");
        canceled.status = "canceled".into();
        let orders = [order(1.0, Side::Buy, "0.001", "50", "0"), canceled];
        let performance = performance(&orders.iter().collect::<Vec<_>>());
        assert!(performance.trades.is_empty());
        assert_eq!(performance.realized, Decimal::ZERO);
    }

    #[test]
    fn sums_the_pnl_with_the_last_values() {
        let snapshot = |manager_id, at, value| Snapshot {
            manager_id,
            at,
            value,
            held: 0.0,
        };
        let snapshots = [
            snapshot(1, 0, 100.0),
            // the same tick, a few milliseconds later
            snapshot(2, 3, 50.0),
            snapshot(1, 60_000, 101.0),
            // manager 2 not saved at this tick: its last value counts
            snapshot(1, 120_000, 99.0),
            snapshot(2, 120_002, 52.0),
        ];
        let budgets = HashMap::from([(1, 100.0), (2, 50.0)]);
        let series = pnl_series(&snapshots.iter().collect::<Vec<_>>(), &budgets);
        assert_eq!(series, vec![(3.0, 0.0), (60_000.0, 1.0), (120_002.0, 1.0)]);
    }
}
