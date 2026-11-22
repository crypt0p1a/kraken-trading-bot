//! The keys the dashboard understands, as the terminal and the browser send them: crossterm in the
//! terminal (the feature `terminal`), the events of the page on the web. The same names as
//! crossterm, which doesn't compile to WebAssembly

/// A key pressed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    /// Shift and tab
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// Any other: nothing to do
    Other,
}

/// The keys held with it: control only matters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    control: bool,
}

impl KeyModifiers {
    pub const NONE: Self = Self { control: false };
    pub const CONTROL: Self = Self { control: true };

    pub fn contains(self, other: Self) -> bool {
        !other.control || self.control
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }
}

impl From<KeyCode> for KeyEvent {
    fn from(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }
}

#[cfg(feature = "terminal")]
impl From<ratatui::crossterm::event::KeyEvent> for KeyEvent {
    fn from(key: ratatui::crossterm::event::KeyEvent) -> Self {
        use ratatui::crossterm::event::{KeyCode as Terminal, KeyModifiers as Held};
        let code = match key.code {
            Terminal::Char(char) => KeyCode::Char(char),
            Terminal::Enter => KeyCode::Enter,
            Terminal::Esc => KeyCode::Esc,
            Terminal::Tab => KeyCode::Tab,
            Terminal::BackTab => KeyCode::BackTab,
            Terminal::Left => KeyCode::Left,
            Terminal::Right => KeyCode::Right,
            Terminal::Up => KeyCode::Up,
            Terminal::Down => KeyCode::Down,
            Terminal::Home => KeyCode::Home,
            Terminal::End => KeyCode::End,
            Terminal::PageUp => KeyCode::PageUp,
            Terminal::PageDown => KeyCode::PageDown,
            _ => KeyCode::Other,
        };
        let modifiers = if key.modifiers.contains(Held::CONTROL) {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        Self::new(code, modifiers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_control_or_not() {
        assert!(KeyModifiers::CONTROL.contains(KeyModifiers::CONTROL));
        assert!(!KeyModifiers::NONE.contains(KeyModifiers::CONTROL));
        assert!(KeyModifiers::NONE.contains(KeyModifiers::NONE));
        assert_eq!(KeyEvent::from(KeyCode::Tab).modifiers, KeyModifiers::NONE);
    }

    #[cfg(feature = "terminal")]
    #[test]
    fn reads_the_keys_of_the_terminal() {
        use ratatui::crossterm::event::{
            KeyCode as Terminal, KeyEvent as Event, KeyModifiers as Held,
        };
        assert_eq!(
            KeyEvent::from(Event::new(Terminal::Char('c'), Held::CONTROL)),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            KeyEvent::from(Event::from(Terminal::BackTab)),
            KeyEvent::from(KeyCode::BackTab)
        );
        assert_eq!(
            KeyEvent::from(Event::from(Terminal::F(5))).code,
            KeyCode::Other
        );
    }
}
