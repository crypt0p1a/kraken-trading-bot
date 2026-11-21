//! What is asked of the bot from outside, the dashboard: a row per command, whose state goes from
//! `refresh` (asked for) to `pending` (under way) to `idle` (done, or never asked for)

use super::{Result, Storage, StorageError};
use rusqlite::{OptionalExtension, params};

/// A tick now, without waiting for the end of the interval
pub const FORCED_TICK: &str = "forced_tick";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandState {
    /// Nothing asked for, or done
    Idle,
    /// Asked for, not started yet
    Refresh,
    /// Under way
    Pending,
}

impl CommandState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Refresh => "refresh",
            Self::Pending => "pending",
        }
    }

    fn parse(text: &str) -> Result<Self> {
        match text {
            "idle" => Ok(Self::Idle),
            "refresh" => Ok(Self::Refresh),
            "pending" => Ok(Self::Pending),
            other => Err(StorageError::Invalid(format!("command state {other}"))),
        }
    }
}

impl Storage {
    pub fn command(&self, key: &str) -> Result<Option<CommandState>> {
        let state: Option<String> = self
            .connection
            .query_row(
                "SELECT state FROM commands WHERE account = ?1 AND key = ?2",
                params![self.account, key],
                |row| row.get(0),
            )
            .optional()?;
        state.as_deref().map(CommandState::parse).transpose()
    }

    /// Asks for the command, unless it's under way
    pub fn request_command(&self, key: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "INSERT INTO commands (account, key, state, updated_at) VALUES (?1, ?2, ?4, ?3)
             ON CONFLICT (account, key) DO UPDATE SET state = ?4,
                updated_at = excluded.updated_at
             WHERE state <> ?5",
            params![
                self.account,
                key,
                at,
                CommandState::Refresh.name(),
                CommandState::Pending.name()
            ],
        )?;
        Ok(())
    }

    /// Starts the command if it was asked for: `refresh` becomes `pending`. False when it wasn't
    pub fn start_command(&self, key: &str, at: i64) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE commands SET state = ?4, updated_at = ?2
             WHERE account = ?3 AND key = ?1 AND state = ?5",
            params![
                key,
                at,
                self.account,
                CommandState::Pending.name(),
                CommandState::Refresh.name()
            ],
        )?;
        Ok(changed > 0)
    }

    /// Ends the command under way: `pending` becomes `idle`. A command asked for meanwhile stays
    pub fn end_command(&self, key: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE commands SET state = ?4, updated_at = ?2
             WHERE account = ?3 AND key = ?1 AND state = ?5",
            params![
                key,
                at,
                self.account,
                CommandState::Idle.name(),
                CommandState::Pending.name()
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goes_from_refresh_to_pending_to_idle() {
        let storage = Storage::open_in_memory().unwrap();
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Idle)
        );
        assert!(!storage.start_command(FORCED_TICK, 1).unwrap());

        storage.request_command(FORCED_TICK, 2).unwrap();
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Refresh)
        );
        assert!(storage.start_command(FORCED_TICK, 3).unwrap());
        assert!(!storage.start_command(FORCED_TICK, 4).unwrap());

        // asked again while under way: it's the same tick
        storage.request_command(FORCED_TICK, 5).unwrap();
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Pending)
        );

        storage.end_command(FORCED_TICK, 6).unwrap();
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Idle)
        );
        // an end without a start changes nothing
        storage.request_command(FORCED_TICK, 7).unwrap();
        storage.end_command(FORCED_TICK, 8).unwrap();
        assert_eq!(
            storage.command(FORCED_TICK).unwrap(),
            Some(CommandState::Refresh)
        );
        assert_eq!(storage.command("unknown").unwrap(), None);
    }

    #[test]
    fn refuses_a_state_it_doesnt_know() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .connection
            .execute("UPDATE commands SET state = 'paused'", [])
            .unwrap();
        assert!(matches!(
            storage.command(FORCED_TICK),
            Err(StorageError::Invalid(_))
        ));
    }

    #[test]
    fn fails_when_its_writes_are_refused() {
        let storage = Storage::open_in_memory().unwrap();
        storage.refuse_writes();
        assert!(storage.request_command(FORCED_TICK, 1).is_err());
        assert!(storage.start_command(FORCED_TICK, 2).is_err());
        assert!(storage.end_command(FORCED_TICK, 3).is_err());
    }
}
