//! Push notifications of a trading bot, through [ntfy](https://ntfy.sh): the orders placed,
//! filled or expired, the failures, a daily summary, in the language of the reader.
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`event`] | the kinds of events, to choose the ones notified |
//! | [`i18n`] | the languages: catalogs of templates, English by default |
//! | [`messages`] | the notifications, built from plain descriptions of the orders |
//! | [`notifier`] | sending them with the `ntfy` crate, or recording them for the tests |
//!
//! Independent of the exchange: the orders are described by [`messages::OrderView`].

pub mod event;
pub mod i18n;
pub mod messages;
pub mod notifier;

pub use event::EventKind;
pub use i18n::{Catalog, Language};
pub use notifier::{Notification, Notifier, Recorded};
