//! The notifications of a trading bot, in the language of their [`Catalog`]

use crate::EventKind;
use crate::i18n::{Catalog, render};
use crate::notifier::Notification;
use ntfy::Priority;
use rust_decimal::Decimal;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderState {
    /// Open, possibly partially filled
    Open,
    /// Entirely filled
    Filled,
    Expired,
    Canceled,
}

/// An order, as the notifications describe it. The fees are in the quote currency
#[derive(Debug, Clone, PartialEq)]
pub struct OrderView {
    pub side: Side,
    pub state: OrderState,
    /// Volume ordered, in the base
    pub volume: Decimal,
    /// Volume executed, in the base
    pub executed: Decimal,
    /// Quote paid or received, fees excluded
    pub cost: Decimal,
    pub fee: Decimal,
    pub average_price: Option<Decimal>,
}

/// The market of a manager, as shown in the notifications
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub manager_id: i32,
    /// e.g. "XBTUSD"
    pub pair: String,
    /// e.g. "XBT"
    pub base: String,
    /// e.g. "USD"
    pub quote: String,
}

impl Label {
    fn title(&self, what: &str) -> String {
        format!("{what} · #{} {}", self.manager_id, self.pair)
    }
}

fn notification(
    event: EventKind,
    title: String,
    message: String,
    tag: &'static str,
    priority: Priority,
) -> Notification {
    Notification {
        event,
        title,
        message,
        tags: vec![tag],
        priority,
    }
}

pub fn started(texts: &Catalog, managers: usize, dry_run: bool) -> Notification {
    let mode = if dry_run {
        texts.mode_dry_run
    } else {
        texts.mode_live
    };
    notification(
        EventKind::Started,
        texts.started_title.into(),
        render(
            texts.started_message,
            &[("managers", &managers.to_string()), ("mode", mode)],
        ),
        "robot",
        Priority::Default,
    )
}

pub fn placed(
    texts: &Catalog,
    label: &Label,
    side: Side,
    amount: Decimal,
    price: Decimal,
    dry_run: bool,
    expires_in: Option<Duration>,
) -> Notification {
    let (what, tag) = match side {
        Side::Buy => (texts.buy_placed, "green_circle"),
        Side::Sell => (texts.sell_placed, "red_circle"),
    };
    let mut message = render(
        texts.placed_message,
        &[
            ("amount", &texts.number(amount)),
            ("base", &label.base),
            ("price", &texts.number(price)),
            ("quote", &label.quote),
            ("total", &texts.money(amount * price)),
        ],
    );
    if let Some(delay) = expires_in {
        let hours = texts.number(Decimal::from(delay.as_secs()) / Decimal::from(3600));
        message.push('\n');
        message.push_str(&render(texts.expires_in, &[("hours", &hours)]));
    }
    let prefix = if dry_run { texts.dry_run_prefix } else { "" };
    notification(
        EventKind::Placed,
        format!("{prefix}{}", label.title(what)),
        message,
        tag,
        Priority::Low,
    )
}

/// `entry_price`: the average price the base sold had cost, fees included
pub fn filled(
    texts: &Catalog,
    label: &Label,
    order: &OrderView,
    entry_price: Option<Decimal>,
) -> Notification {
    let complete = order.state == OrderState::Filled;
    let executed = order.executed;
    let average = order.average_price.unwrap_or_else(|| {
        if executed.is_zero() {
            Decimal::ZERO
        } else {
            order.cost / executed
        }
    });
    let volume = if complete {
        format!("{} {}", texts.number(executed), label.base)
    } else {
        render(
            texts.partial_volume,
            &[
                ("executed", &texts.number(executed)),
                ("volume", &texts.number(order.volume)),
                ("base", &label.base),
            ],
        )
    };
    let price = texts.number(average.round_dp(8));

    match order.side {
        Side::Buy => notification(
            EventKind::Filled,
            label.title(if complete {
                texts.buy_filled
            } else {
                texts.buy_partially_filled
            }),
            render(
                texts.buy_filled_message,
                &[
                    ("volume", &volume),
                    ("price", &price),
                    ("quote", &label.quote),
                    ("cost", &texts.money(order.cost + order.fee)),
                ],
            ),
            "white_check_mark",
            Priority::Default,
        ),
        Side::Sell => {
            let received = order.cost - order.fee;
            let mut message = render(
                texts.sell_filled_message,
                &[
                    ("volume", &volume),
                    ("price", &price),
                    ("quote", &label.quote),
                    ("received", &texts.money(received)),
                ],
            );
            if let Some(entry) = entry_price.filter(|it| *it > Decimal::ZERO) {
                let cost = entry * executed;
                let gain = received - cost;
                message.push('\n');
                message.push_str(&render(
                    texts.gain,
                    &[
                        ("gain", &texts.signed(gain)),
                        ("quote", &label.quote),
                        ("percent", &texts.signed(gain / cost * Decimal::ONE_HUNDRED)),
                    ],
                ));
            }
            notification(
                EventKind::Filled,
                label.title(if complete {
                    texts.sell_filled
                } else {
                    texts.sell_partially_filled
                }),
                message,
                "moneybag",
                Priority::High,
            )
        }
    }
}

/// An order expired or canceled
pub fn ended(texts: &Catalog, label: &Label, order: &OrderView) -> Notification {
    let what = match (order.side, order.state) {
        (Side::Buy, OrderState::Expired) => texts.buy_expired,
        (Side::Buy, _) => texts.buy_canceled,
        (Side::Sell, OrderState::Expired) => texts.sell_expired,
        (Side::Sell, _) => texts.sell_canceled,
    };
    let message = if order.executed.is_zero() {
        match order.side {
            Side::Buy => texts.buy_not_executed.to_string(),
            Side::Sell => texts.sell_not_executed.to_string(),
        }
    } else {
        render(
            texts.partially_executed,
            &[
                ("executed", &texts.number(order.executed)),
                ("volume", &texts.number(order.volume)),
                ("base", &label.base),
            ],
        )
    };
    notification(
        EventKind::Expired,
        label.title(what),
        message,
        "hourglass",
        Priority::Low,
    )
}

/// A failure: of a manager when `label` is given, of the bot otherwise
pub fn failed(texts: &Catalog, label: Option<&Label>, error: &str) -> Notification {
    let title = match label {
        Some(label) => label.title(texts.manager_error),
        None => texts.bot_error.into(),
    };
    notification(
        EventKind::Failed,
        title,
        error.to_string(),
        "warning",
        Priority::High,
    )
}

/// The value of a manager for the daily summary
#[derive(Debug, Clone, PartialEq)]
pub struct ManagerSummary {
    pub label: Label,
    pub budget: Decimal,
    pub value: Decimal,
    /// Sales filled during the last 24 hours
    pub sales: usize,
}

pub fn daily_summary(texts: &Catalog, managers: &[ManagerSummary]) -> Notification {
    let mut lines: Vec<String> = managers
        .iter()
        .map(|it| {
            let gain = it.value - it.budget;
            let percent = if it.budget.is_zero() {
                Decimal::ZERO
            } else {
                gain / it.budget * Decimal::ONE_HUNDRED
            };
            render(
                texts.daily_line,
                &[
                    ("id", &it.label.manager_id.to_string()),
                    ("pair", &it.label.pair),
                    ("value", &texts.money(it.value)),
                    ("quote", &it.label.quote),
                    ("gain", &texts.signed(gain)),
                    ("percent", &texts.signed(percent)),
                    ("sales", &it.sales.to_string()),
                ],
            )
        })
        .collect();

    // the totals, per quote currency
    let mut quotes: Vec<&str> = managers.iter().map(|it| it.label.quote.as_str()).collect();
    quotes.sort();
    quotes.dedup();
    for quote in quotes {
        let (budget, value) = managers
            .iter()
            .filter(|it| it.label.quote == quote)
            .fold((Decimal::ZERO, Decimal::ZERO), |(budget, value), it| {
                (budget + it.budget, value + it.value)
            });
        lines.push(render(
            texts.daily_total,
            &[
                ("quote", quote),
                ("value", &texts.money(value)),
                ("gain", &texts.signed(value - budget)),
            ],
        ));
    }

    notification(
        EventKind::DailySummary,
        texts.daily_title.into(),
        lines.join("\n"),
        "chart_with_upwards_trend",
        Priority::Low,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{ENGLISH, FRENCH};
    use std::str::FromStr;

    fn d(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    fn label() -> Label {
        Label {
            manager_id: 1,
            pair: "XBTUSD".into(),
            base: "XBT".into(),
            quote: "USD".into(),
        }
    }

    fn order(
        side: Side,
        state: OrderState,
        volume: &str,
        executed: &str,
        cost: &str,
        fee: &str,
        average: &str,
    ) -> OrderView {
        OrderView {
            side,
            state,
            volume: d(volume),
            executed: d(executed),
            cost: d(cost),
            fee: d(fee),
            average_price: Some(d(average)),
        }
    }

    #[test]
    fn notifies_the_start() {
        let live = started(&ENGLISH, 3, false);
        assert_eq!(live.title, "Bot started");
        assert_eq!(
            live.message,
            "3 manager(s) · live: the orders are placed on Kraken"
        );
        assert!(started(&ENGLISH, 1, true).message.contains("dry run"));
    }

    #[test]
    fn notifies_an_order_placed() {
        let buy = placed(
            &ENGLISH,
            &label(),
            Side::Buy,
            d("0.00201516"),
            d("49500"),
            false,
            Some(Duration::from_secs(12 * 3600)),
        );
        assert_eq!(buy.title, "Buy placed · #1 XBTUSD");
        assert_eq!(
            buy.message,
            "0.00201516 XBT at 49500 USD (≈ 99.75 USD)\nexpires in 12 h if not filled"
        );
        assert_eq!(buy.event, EventKind::Placed);
        assert_eq!(buy.priority, Priority::Low);

        let sell = placed(
            &ENGLISH,
            &label(),
            Side::Sell,
            d("0.002"),
            d("51000.5"),
            true,
            None,
        );
        assert_eq!(sell.title, "[dry run] Sell placed · #1 XBTUSD");
        assert_eq!(sell.message, "0.002 XBT at 51000.5 USD (≈ 102 USD)");
        assert_eq!(sell.tags, ["red_circle"]);
    }

    #[test]
    fn notifies_a_buy_filled() {
        let buy = order(
            Side::Buy,
            OrderState::Filled,
            "0.002",
            "0.002",
            "99",
            "0.2475",
            "49500",
        );
        let notification = filled(&ENGLISH, &label(), &buy, None);
        assert_eq!(notification.title, "Buy filled · #1 XBTUSD");
        assert_eq!(
            notification.message,
            "0.002 XBT at 49500 USD · cost 99.25 USD fees included"
        );

        let partial = order(
            Side::Buy,
            OrderState::Open,
            "0.002",
            "0.0005",
            "24.75",
            "0.06",
            "49500",
        );
        let notification = filled(&ENGLISH, &label(), &partial, None);
        assert_eq!(notification.title, "Buy partially filled · #1 XBTUSD");
        assert!(
            notification
                .message
                .starts_with("0.0005 / 0.002 XBT at 49500 USD"),
            "{}",
            notification.message
        );

        // without average price, it's computed
        let without = OrderView {
            average_price: None,
            ..buy
        };
        assert!(
            filled(&ENGLISH, &label(), &without, None)
                .message
                .contains("at 49500 USD")
        );
    }

    #[test]
    fn notifies_the_gain_of_a_sale() {
        let sell = order(
            Side::Sell,
            OrderState::Filled,
            "0.002",
            "0.002",
            "101",
            "0.2525",
            "50500",
        );
        // bought 0.002 for 99.2475, fees included
        let notification = filled(&ENGLISH, &label(), &sell, Some(d("49623.75")));
        assert_eq!(notification.title, "Sell filled · #1 XBTUSD");
        assert_eq!(
            notification.message,
            "0.002 XBT at 50500 USD · received 100.75 USD after fees\ngain +1.5 USD (+1.51 %)"
        );
        assert_eq!(notification.priority, Priority::High);
        assert_eq!(notification.tags, ["moneybag"]);

        // without the entry price, no gain
        assert!(
            !filled(&ENGLISH, &label(), &sell, None)
                .message
                .contains("gain")
        );
    }

    #[test]
    fn notifies_an_order_expired_or_canceled() {
        let expired = order(Side::Buy, OrderState::Expired, "0.002", "0", "0", "0", "0");
        let notification = ended(&ENGLISH, &label(), &expired);
        assert_eq!(notification.title, "Buy expired · #1 XBTUSD");
        assert!(notification.message.starts_with("not filled"));

        let partially = order(
            Side::Buy,
            OrderState::Expired,
            "0.002",
            "0.0005",
            "24.75",
            "0.06",
            "49500",
        );
        assert_eq!(
            ended(&ENGLISH, &label(), &partially).message,
            "partially filled: 0.0005 / 0.002 XBT, the rest is handled at the next tick"
        );

        let canceled = order(
            Side::Sell,
            OrderState::Canceled,
            "0.002",
            "0",
            "0",
            "0",
            "0",
        );
        assert_eq!(
            ended(&ENGLISH, &label(), &canceled).title,
            "Sell canceled · #1 XBTUSD"
        );
        assert!(
            ended(&ENGLISH, &label(), &canceled)
                .message
                .starts_with("not filled: a new sell")
        );
    }

    #[test]
    fn notifies_the_failures() {
        let manager = failed(&ENGLISH, Some(&label()), "EOrder:Insufficient funds");
        assert_eq!(manager.title, "Error · #1 XBTUSD");
        assert_eq!(manager.message, "EOrder:Insufficient funds");
        assert_eq!(manager.priority, Priority::High);
        assert_eq!(failed(&ENGLISH, None, "timeout").title, "Bot error");
    }

    #[test]
    fn summarizes_the_day_with_totals_per_currency() {
        let eur = Label {
            manager_id: 3,
            pair: "ETHEUR".into(),
            base: "ETH".into(),
            quote: "EUR".into(),
        };
        let summary = daily_summary(
            &ENGLISH,
            &[
                ManagerSummary {
                    label: label(),
                    budget: d("100"),
                    value: d("102.5"),
                    sales: 3,
                },
                ManagerSummary {
                    label: Label {
                        manager_id: 2,
                        ..label()
                    },
                    budget: d("50"),
                    value: d("49.6"),
                    sales: 0,
                },
                ManagerSummary {
                    label: eur,
                    budget: d("40"),
                    value: d("41"),
                    sales: 1,
                },
            ],
        );
        assert_eq!(summary.title, "Daily summary");
        assert_eq!(
            summary.message,
            "#1 XBTUSD: 102.5 USD (+2.5 · +2.5 %) · 3 sale(s) in 24 h
#2 XBTUSD: 49.6 USD (-0.4 · -0.8 %) · 0 sale(s) in 24 h
#3 ETHEUR: 41 EUR (+1 · +2.5 %) · 1 sale(s) in 24 h
total EUR: 41 (+1)
total USD: 152.1 (+2.1)"
        );
    }

    #[test]
    fn speaks_french_too() {
        let sell = order(
            Side::Sell,
            OrderState::Filled,
            "0.002",
            "0.002",
            "101",
            "0.2525",
            "50500",
        );
        let notification = filled(&FRENCH, &label(), &sell, Some(d("49623.75")));
        assert_eq!(notification.title, "Vente exécutée · #1 XBTUSD");
        assert_eq!(
            notification.message,
            "0,002 XBT à 50500 USD · reçu 100,75 USD frais déduits\ngain +1,5 USD (+1,51 %)"
        );
        let expired = order(Side::Sell, OrderState::Expired, "1", "0", "0", "0", "0");
        assert_eq!(
            ended(&FRENCH, &label(), &expired).title,
            "Vente expirée · #1 XBTUSD"
        );
    }
}
