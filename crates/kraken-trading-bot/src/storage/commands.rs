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
            .query_row("SELECT state FROM commands WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        state.as_deref().map(CommandState::parse).transpose()
    }

    /// Asks for the command, unless it's under way
    pub fn request_command(&self, key: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "INSERT INTO commands (key, state, updated_at) VALUES (?1, 'refresh', ?2)
             ON CONFLICT (key) DO UPDATE SET state = 'refresh', updated_at = excluded.updated_at
             WHERE state <> 'pending'",
            params![key, at],
        )?;
        Ok(())
    }

    /// Starts the command if it was asked for: `refresh` becomes `pending`. False when it wasn't
    pub fn start_command(&self, key: &str, at: i64) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE commands SET state = 'pending', updated_at = ?2
             WHERE key = ?1 AND state = 'refresh'",
            params![key, at],
        )?;
        Ok(changed > 0)
    }

    /// Ends the command under way: `pending` becomes `idle`. A command asked for meanwhile stays
    pub fn end_command(&self, key: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE commands SET state = 'idle', updated_at = ?2
             WHERE key = ?1 AND state = 'pending'",
            params![key, at],
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
}
