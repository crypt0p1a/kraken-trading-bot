//! The state of the dashboard: what's selected, what's shown, and the keys that change it

use crate::data::{Action, Database, Manager, ManagerKey, Order, account_of, manager_number};
use crate::stats::Model;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Managers,
    Orders,
    Tape,
}

impl Panel {
    fn next(self) -> Self {
        match self {
            Self::Managers => Self::Orders,
            Self::Orders => Self::Tape,
            Self::Tape => Self::Managers,
        }
    }

    fn previous(self) -> Self {
        self.next().next()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chart {
    /// The P&L over time
    Pnl,
    /// What the whole account is worth over time, with its target and the orders filled
    Portfolio,
    /// The price of the pair, with the entry price and the open orders
    Price,
    /// The P&L of each sell
    Trades,
    /// What the portfolio would be worth if the market moved
    Scenarios,
    /// What each period earned, in % of what the portfolio was worth before it
    Growth,
}

impl Chart {
    fn next(self) -> Self {
        match self {
            Self::Pnl => Self::Portfolio,
            Self::Portfolio => Self::Price,
            Self::Price => Self::Trades,
            Self::Trades => Self::Scenarios,
            Self::Scenarios => Self::Growth,
            Self::Growth => Self::Pnl,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hour,
    Day,
    Week,
    Month,
    All,
}

impl Range {
    pub const ALL: [Range; 5] = [Self::Hour, Self::Day, Self::Week, Self::Month, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hour => "1H",
            Self::Day => "24H",
            Self::Week => "7D",
            Self::Month => "30D",
            Self::All => "ALL",
        }
    }

    /// The periods the growth compares in this range, UTC milliseconds: a tick of the bot at most
    pub fn period(self) -> i64 {
        const HOUR: i64 = 3_600_000;
        match self {
            Self::Hour => 5 * 60_000,
            Self::Day => HOUR,
            Self::Week | Self::Month | Self::All => 24 * HOUR,
        }
    }

    pub fn period_label(self) -> &'static str {
        match self {
            Self::Hour => "5 MIN",
            Self::Day => "HOUR",
            Self::Week | Self::Month | Self::All => "DAY",
        }
    }

    /// Start of the range, UTC milliseconds: 0 for everything
    pub fn since(self, now: i64) -> i64 {
        const HOUR: i64 = 3_600_000;
        match self {
            Self::Hour => now - HOUR,
            Self::Day => now - 24 * HOUR,
            Self::Week => now - 7 * 24 * HOUR,
            Self::Month => now - 30 * 24 * HOUR,
            Self::All => 0,
        }
    }
}

/// Letters of the name of an account before its managers, when the accounts are together
const ACCOUNT_TAG: usize = 5;

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|it| it.as_millis() as i64)
        .unwrap_or_default()
}

pub struct App {
    pub model: Model,
    /// Where the database is, as given
    pub source: String,
    /// The row of the table of the managers
    pub cursor: usize,
    /// Only the manager of the cursor, instead of all of them
    pub drilled: bool,
    pub panel: Panel,
    /// The charts of the column, from the top
    pub charts: [Chart; 3],
    pub range: Range,
    /// The account shown, all of them added up when None
    pub view: Option<i64>,
    pub orders_row: usize,
    pub tape_row: usize,
    /// The idle decisions in the tape: one per manager and tick
    pub show_idle: bool,
    pub open_only: bool,
    pub help: bool,
    pub error: Option<String>,
    /// UTC milliseconds of the last read
    pub loaded_at: i64,
    data_version: Option<i64>,
    started: Instant,
    pub quit: bool,
    /// `t` pressed: a tick of the bot to ask for
    pub tick_asked: bool,
}

impl App {
    pub fn new(source: String) -> Self {
        Self {
            model: Model::default(),
            source,
            cursor: 0,
            drilled: false,
            panel: Panel::Managers,
            charts: [Chart::Portfolio, Chart::Scenarios, Chart::Price],
            range: Range::Day,
            view: None,
            orders_row: 0,
            tape_row: 0,
            show_idle: false,
            open_only: false,
            help: false,
            error: None,
            loaded_at: 0,
            data_version: None,
            started: Instant::now(),
            quit: false,
            tick_asked: false,
        }
    }

    /// Asks the bot for a tick now, if `t` was pressed
    pub fn ask_tick(&mut self, database: &Database) {
        if !std::mem::take(&mut self.tick_asked) {
            return;
        }
        match database.request_tick(now_millis(), self.view) {
            Ok(()) => self.refresh(database, true),
            Err(error) => self.error = Some(error),
        }
    }

    /// Reads the database again when the bot wrote it, or when `force`
    pub fn refresh(&mut self, database: &Database, force: bool) {
        let version = match database.data_version() {
            Ok(version) => version,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        if !force && self.data_version == Some(version) {
            return;
        }
        let now = now_millis();
        match database.load(self.range.since(now), now, self.view) {
            Ok(data) => {
                self.model = Model::new(data);
                self.data_version = Some(version);
                self.loaded_at = now;
                self.error = None;
                self.cursor = self
                    .cursor
                    .min(self.model.data.managers.len().saturating_sub(1));
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// There's something to tell apart: more than one account in the database
    pub fn several_accounts(&self) -> bool {
        self.model.data.names.len() > 1
    }

    /// The accounts are added up on the screen, the managers of each next to each other
    fn together(&self) -> bool {
        self.view.is_none() && self.several_accounts()
    }

    /// What's before a manager in the tables when the accounts are together: "alice·"
    pub fn account_tag(&self, key: ManagerKey) -> String {
        if !self.together() {
            return String::new();
        }
        let name = self
            .model
            .data
            .names
            .iter()
            .find(|(id, _)| *id == account_of(key))
            .map_or("?", |(_, name)| name.as_str());
        let short: String = name.chars().take(ACCOUNT_TAG).collect();
        format!("{short}·")
    }

    /// The manager, as the tables name it: "#7", "alice·#7" when the accounts are together
    pub fn manager_name(&self, key: ManagerKey) -> String {
        format!("{}#{}", self.account_tag(key), manager_number(key))
    }

    /// What the account tags add to the width of the columns of the managers
    pub fn tag_width(&self) -> u16 {
        if !self.together() {
            return 0;
        }
        let longest = self
            .model
            .data
            .names
            .iter()
            .map(|(_, name)| name.chars().count().min(ACCOUNT_TAG))
            .max()
            .unwrap_or_default();
        longest as u16 + 1
    }

    /// The name of what's shown, when there's something to tell apart
    pub fn view_name(&self) -> Option<String> {
        if !self.several_accounts() {
            return None;
        }
        let names = &self.model.data.names;
        Some(match self.view {
            None => format!("ALL ACCOUNTS · {}", names.len()),
            Some(view) => {
                let index = names.iter().position(|(id, _)| *id == view);
                format!(
                    "{} · {}/{}",
                    index.map_or("?".to_string(), |it| names[it].1.to_uppercase()),
                    index.map_or(0, |it| it + 1),
                    names.len()
                )
            }
        })
    }

    /// The next one, in a circle: all of them added up, the first account, the second…
    fn next_view(&mut self) {
        let names = &self.model.data.names;
        self.view = match self.view {
            None => names.first().map(|(id, _)| *id),
            Some(view) => names
                .iter()
                .position(|(id, _)| *id == view)
                .and_then(|it| names.get(it + 1))
                .map(|(id, _)| *id),
        };
        self.cursor = 0;
        self.drilled = false;
        self.orders_row = 0;
        self.tape_row = 0;
    }

    /// Milliseconds since the start, for the animations
    pub fn elapsed(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    pub fn selected(&self) -> Option<&Manager> {
        self.model.data.managers.get(self.cursor)
    }

    /// The manager everything is filtered on, when drilled into
    pub fn focus(&self) -> Option<&Manager> {
        self.drilled.then(|| self.selected()).flatten()
    }

    fn shown(&self, manager_id: ManagerKey) -> bool {
        self.focus().is_none_or(|it| it.id == manager_id)
    }

    /// The orders of the blotter, the most recent first
    pub fn orders(&self) -> Vec<&Order> {
        let mut orders: Vec<&Order> = self
            .model
            .data
            .orders
            .iter()
            .filter(|it| self.shown(it.manager_id) && (!self.open_only || !it.is_final))
            .collect();
        orders.sort_by(|a, b| b.open_time.total_cmp(&a.open_time));
        orders
    }

    /// The actions of the tape, the most recent first
    pub fn actions(&self) -> Vec<&Action> {
        self.model
            .data
            .actions
            .iter()
            .filter(|it| self.shown(it.manager_id) && (self.show_idle || it.kind != "idle"))
            .collect()
    }

    /// The managers the charts are about
    pub fn charted(&self) -> Vec<ManagerKey> {
        match self.focus() {
            Some(manager) => vec![manager.id],
            None => {
                // the managers of the main quote currency: their amounts add up
                let quote = self.model.totals.first().map(|it| it.quote.as_str());
                self.model
                    .data
                    .managers
                    .iter()
                    .filter(|it| self.model.quotes.get(&it.id).map(String::as_str) == quote)
                    .map(|it| it.id)
                    .collect()
            }
        }
    }

    /// Handles a key; returns true when the database must be read again
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if self.help {
            self.help = false;
            return false;
        }
        let mut reload = false;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('?') | KeyCode::Char('h') => self.help = true,
            KeyCode::Tab => self.panel = self.panel.next(),
            KeyCode::BackTab => self.panel = self.panel.previous(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::PageUp => self.scroll(-10),
            KeyCode::PageDown => self.scroll(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll(-1_000_000),
            KeyCode::End | KeyCode::Char('G') => self.scroll(1_000_000),
            KeyCode::Enter => {
                self.drilled = !self.drilled;
                self.orders_row = 0;
                self.tape_row = 0;
            }
            KeyCode::Esc => {
                self.drilled = false;
                self.orders_row = 0;
                self.tape_row = 0;
            }
            KeyCode::Char('c') => self.next_chart(0),
            KeyCode::Char('v') => self.next_chart(1),
            KeyCode::Char('b') => self.next_chart(2),
            KeyCode::Char('i') => {
                self.show_idle = !self.show_idle;
                self.tape_row = 0;
            }
            KeyCode::Char('o') => {
                self.open_only = !self.open_only;
                self.orders_row = 0;
            }
            KeyCode::Char('a') if self.several_accounts() => {
                self.next_view();
                reload = true;
            }
            KeyCode::Char('r') => reload = true,
            KeyCode::Char('t') => self.tick_asked = true,
            KeyCode::Char(digit @ '1'..='5') => {
                self.range = Range::ALL[digit as usize - '1' as usize];
                reload = true;
            }
            _ => {}
        }
        reload
    }

    /// The next chart at this place, never one shown at another
    fn next_chart(&mut self, place: usize) {
        let mut chart = self.charts[place].next();
        while self.charts.contains(&chart) {
            chart = chart.next();
        }
        self.charts[place] = chart;
    }

    fn scroll(&mut self, by: i64) {
        let (row, count) = match self.panel {
            Panel::Managers => (&mut self.cursor, self.model.data.managers.len()),
            Panel::Orders => {
                let count = self.orders().len();
                (&mut self.orders_row, count)
            }
            Panel::Tape => {
                let count = self.actions().len();
                (&mut self.tape_row, count)
            }
        };
        let last = count.saturating_sub(1) as i64;
        *row = (*row as i64 + by).clamp(0, last) as usize;
        if self.panel == Panel::Managers && self.drilled {
            self.orders_row = 0;
            self.tape_row = 0;
        }
    }
}
