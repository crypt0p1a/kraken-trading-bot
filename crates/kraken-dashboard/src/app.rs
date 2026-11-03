//! The state of the dashboard: what's selected, what's shown, and the keys that change it

use crate::data::{Action, Database, Manager, Order};
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
    /// The price of the pair, with the entry price and the open orders
    Price,
    /// The P&L of each sell
    Trades,
}

impl Chart {
    fn next(self) -> Self {
        match self {
            Self::Pnl => Self::Price,
            Self::Price => Self::Trades,
            Self::Trades => Self::Pnl,
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
    pub chart: Chart,
    pub range: Range,
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
}

impl App {
    pub fn new(source: String) -> Self {
        Self {
            model: Model::default(),
            source,
            cursor: 0,
            drilled: false,
            panel: Panel::Managers,
            chart: Chart::Pnl,
            range: Range::Day,
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
        match database.load(self.range.since(now), now) {
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

    fn shown(&self, manager_id: i32) -> bool {
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
    pub fn charted(&self) -> Vec<i32> {
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
            KeyCode::Char('c') => self.chart = self.chart.next(),
            KeyCode::Char('i') => {
                self.show_idle = !self.show_idle;
                self.tape_row = 0;
            }
            KeyCode::Char('o') => {
                self.open_only = !self.open_only;
                self.orders_row = 0;
            }
            KeyCode::Char('r') => reload = true,
            KeyCode::Char(digit @ '1'..='5') => {
                self.range = Range::ALL[digit as usize - '1' as usize];
                reload = true;
            }
            _ => {}
        }
        reload
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
