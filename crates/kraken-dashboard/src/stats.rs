//! What the dashboard computes from the data of the bot: the P&L realized by the sells, the
//! totals by quote currency, the P&L over time, the pair of each manager

use crate::data::{Account, Data, Manager, Market, Order, Side, Snapshot, percent, to_f64};
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

/// A sell, and what it earned against what its base cost
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub manager_id: i32,
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

    pub fn win_rate(&self) -> Option<f64> {
        (self.trades > 0).then(|| self.wins as f64 * 100.0 / self.trades as f64)
    }
}

/// The data, with what's computed from it
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub data: Data,
    pub performances: HashMap<i32, Performance>,
    /// The quote currency of each manager
    pub quotes: HashMap<i32, String>,
    /// The index in `data.markets` of the pair of each manager
    pub markets: HashMap<i32, usize>,
    /// By quote, the one with the most budget first
    pub totals: Vec<Totals>,
    /// The sells of every manager, oldest first
    pub trades: Vec<Trade>,
}

impl Model {
    pub fn new(data: Data) -> Self {
        let mut by_manager: HashMap<i32, Vec<&Order>> = HashMap::new();
        for order in &data.orders {
            by_manager.entry(order.manager_id).or_default().push(order);
        }
        let performances: HashMap<i32, Performance> = by_manager
            .iter()
            .map(|(id, orders)| (*id, performance(orders)))
            .collect();

        let quotes: HashMap<i32, String> = data
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

        let mut by_quote: BTreeMap<&str, Vec<i32>> = BTreeMap::new();
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
    pub fn totals_of(&self, ids: &[i32]) -> Totals {
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

    pub fn manager(&self, id: i32) -> Option<&Manager> {
        self.data.managers.iter().find(|it| it.id == id)
    }

    pub fn market_of(&self, id: i32) -> Option<&Market> {
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
    pub fn manager_series(&self, id: i32) -> Vec<(f64, f64)> {
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

    pub fn open_orders_of(&self, id: i32) -> impl Iterator<Item = &Order> {
        self.data
            .orders
            .iter()
            .filter(move |it| it.manager_id == id && !it.is_final)
    }

    /// The last values of a manager, oldest first, for its sparkline
    pub fn values_of(&self, id: i32) -> Vec<f64> {
        self.data
            .snapshots
            .iter()
            .filter(|it| it.manager_id == id)
            .map(|it| it.value)
            .collect()
    }

    /// The P&L over time of the managers of `ids`: (UTC milliseconds, quote)
    pub fn pnl_series(&self, ids: &[i32]) -> Vec<(f64, f64)> {
        let budgets: HashMap<i32, f64> = self
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
pub fn pnl_series(snapshots: &[&Snapshot], budgets: &HashMap<i32, f64>) -> Vec<(f64, f64)> {
    // the managers of a tick are saved a few milliseconds apart: together, they're one point
    const TICK: i64 = 5_000;
    let times: BTreeSet<i64> = snapshots.iter().map(|it| it.at / TICK).collect();

    let mut last: HashMap<i32, f64> = HashMap::new();
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
