//! The order of the rows of the tables: the column they follow, and what each row has in it

use crate::app::Panel;
use crate::data::{Action, Manager, Order, Side, to_f64};
use crate::stats::{Model, canonical};
use std::cmp::Ordering;

/// A column rows can follow, named as in the header of its table
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    /// The direction it starts in: the largest amounts and the last dates first
    pub descending: bool,
}

const fn column(name: &'static str, descending: bool) -> Column {
    Column { name, descending }
}

const MANAGERS: &[Column] = &[
    column("MANAGER", false),
    column("PAIR", false),
    column("VALUE", true),
    column("P&L", true),
    column("P&L %", true),
    column("POSITION", true),
    column("ENTRY", true),
    column("VS ENTRY", true),
    column("OPEN ORDER", false),
    column("TO FILL", false),
    column("TREND", true),
];

const ORDERS: &[Column] = &[
    column("OPENED", true),
    column("MGR", false),
    column("SIDE", false),
    column("PAIR", false),
    column("PRICE", true),
    column("TO FILL", false),
    column("VOLUME", true),
    column("FILLED", true),
    column("COST", true),
    column("FEE", true),
    column("STATUS", false),
];

const TAPE: &[Column] = &[
    column("AT", true),
    column("MGR", false),
    column("WHAT", false),
];

/// The columns of a table: the first one is the order the table starts in
pub fn columns(panel: Panel) -> &'static [Column] {
    match panel {
        Panel::Managers => MANAGERS,
        Panel::Orders => ORDERS,
        Panel::Tape => TAPE,
    }
}

/// The column a table follows, and in which direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Sort {
    pub fn first(panel: Panel) -> Self {
        Self {
            column: 0,
            descending: columns(panel)[0].descending,
        }
    }

    /// The column before or after, in a circle, in the direction it starts in
    pub fn moved(self, panel: Panel, by: i64) -> Self {
        let columns = columns(panel);
        let column = (self.column as i64 + by).rem_euclid(columns.len() as i64) as usize;
        Self {
            column,
            descending: columns[column].descending,
        }
    }

    pub fn reversed(self) -> Self {
        Self {
            descending: !self.descending,
            ..self
        }
    }

    pub fn name(self, panel: Panel) -> &'static str {
        columns(panel)[self.column].name
    }
}

/// What a row has in the column it's sorted on
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Number(f64),
    Text(String),
    /// The "—" of the table: at the end, in both directions
    Nothing,
}

impl Key {
    fn number(value: Option<f64>) -> Self {
        value.map_or(Self::Nothing, Self::Number)
    }

    pub fn compare(&self, other: &Self, descending: bool) -> Ordering {
        let directed = |ordering: Ordering| {
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        };
        match (self, other) {
            (Self::Nothing, Self::Nothing) => Ordering::Equal,
            (Self::Nothing, _) => Ordering::Greater,
            (_, Self::Nothing) => Ordering::Less,
            (Self::Number(a), Self::Number(b)) => directed(a.total_cmp(b)),
            (Self::Text(a), Self::Text(b)) => directed(a.cmp(b)),
            _ => Ordering::Equal,
        }
    }
}

fn side(side: Side) -> Key {
    Key::Text(
        match side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
        .to_string(),
    )
}

/// What a manager has in a column of its table. The positions and the volumes are in different
/// currencies from a pair to the next: they're compared by what they're worth
pub fn manager(model: &Model, manager: &Manager, column: usize) -> Key {
    let position = (!manager.base().is_zero()).then(|| to_f64(manager.base()));
    let last = to_f64(manager.last_price);
    let entry = manager
        .entry_price
        .filter(|_| position.is_some())
        .map(to_f64);
    let order = || model.open_orders_of(manager.id).last();
    match column {
        0 => Key::Number(manager.id as f64),
        1 => Key::Text(canonical(&manager.pair)),
        2 => Key::Number(to_f64(manager.value)),
        3 => Key::Number(to_f64(manager.pnl())),
        4 => Key::Number(manager.pnl_percent()),
        5 => Key::number(position.map(|it| it * last)),
        6 => Key::number(entry),
        7 => Key::number(entry.map(|it| (last - it) / it * 100.0)),
        8 => order().map_or(Key::Nothing, |it| side(it.side)),
        // the closest to the market first, whatever the side it comes from
        9 => Key::number(order().and_then(|it| model.to_fill(it)).map(f64::abs)),
        _ => {
            let values = model.values_of(manager.id);
            match (values.first(), values.last()) {
                (Some(first), Some(last)) => Key::Number(last - first),
                _ => Key::Nothing,
            }
        }
    }
}

/// What an order has in a column of its table
pub fn order(model: &Model, order: &Order, column: usize) -> Key {
    let executed = !order.volume_executed.is_zero();
    match column {
        0 => Key::Number(order.open_time),
        1 => Key::Number(order.manager_id as f64),
        2 => side(order.side),
        3 => Key::Text(canonical(&order.pair)),
        4 => Key::Number(order.shown_price()),
        5 => Key::number(model.to_fill(order).map(f64::abs)),
        6 => Key::Number(to_f64(order.volume) * order.shown_price()),
        7 => Key::Number(order.filled()),
        8 => Key::number(executed.then(|| to_f64(order.cost))),
        9 => Key::number(executed.then(|| to_f64(order.fee))),
        _ => Key::Text(order.status.clone()),
    }
}

/// What an action has in a column of the tape
pub fn action(action: &Action, column: usize) -> Key {
    match column {
        0 => Key::Number(action.at as f64),
        1 => Key::Number(action.manager_id as f64),
        _ => Key::Text(action.kind.clone()),
    }
}

/// The rows in the order of their keys; those with the same key stay in the order they came
pub fn sorted<T>(rows: Vec<&T>, sort: Sort, key: impl Fn(&T) -> Key) -> Vec<&T> {
    let mut keyed: Vec<(Key, &T)> = rows.into_iter().map(|it| (key(it), it)).collect();
    keyed.sort_by(|a, b| a.0.compare(&b.0, sort.descending));
    keyed.into_iter().map(|(_, it)| it).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_has_nothing_at_the_end_in_both_directions() {
        let (one, two) = (Key::Number(1.0), Key::Number(2.0));
        for descending in [false, true] {
            assert_eq!(Key::Nothing.compare(&one, descending), Ordering::Greater);
            assert_eq!(two.compare(&Key::Nothing, descending), Ordering::Less);
        }
        assert_eq!(one.compare(&two, false), Ordering::Less);
        assert_eq!(one.compare(&two, true), Ordering::Greater);
    }

    #[test]
    fn goes_through_the_columns_in_a_circle_each_in_its_direction() {
        let first = Sort::first(Panel::Managers);
        assert_eq!(first.name(Panel::Managers), "MANAGER");
        assert!(!first.descending);

        let last = first.moved(Panel::Managers, -1);
        assert_eq!(last.name(Panel::Managers), "TREND");
        assert!(last.descending);
        assert_eq!(last.moved(Panel::Managers, 1), first);

        // a column reached again starts over in its own direction
        let value = first.moved(Panel::Managers, 2).reversed();
        assert!(!value.descending);
        assert!(
            value
                .moved(Panel::Managers, 1)
                .moved(Panel::Managers, -1)
                .descending
        );
    }
}
