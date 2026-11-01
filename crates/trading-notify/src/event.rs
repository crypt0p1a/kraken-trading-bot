use serde::Deserialize;

/// What is notified
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The bot started
    Started,
    /// An order was placed
    Placed,
    /// An order was filled, entirely or partially
    Filled,
    /// An order expired or was canceled
    Expired,
    /// An order was refused, the bot or a manager failed
    Failed,
    /// Every day, the value of the managers
    DailySummary,
}

impl EventKind {
    pub const ALL: [EventKind; 6] = [
        EventKind::Started,
        EventKind::Placed,
        EventKind::Filled,
        EventKind::Expired,
        EventKind::Failed,
        EventKind::DailySummary,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_events_in_snake_case() {
        let events: Vec<EventKind> =
            serde_norway::from_str("[started, placed, filled, expired, failed, daily_summary]")
                .unwrap();
        assert_eq!(events, EventKind::ALL);
        assert!(serde_norway::from_str::<EventKind>("everything").is_err());
    }
}
