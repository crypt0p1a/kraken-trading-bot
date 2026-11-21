//! The journal of the managers: what each one decided, in the `actions` table

use super::Bot;
use crate::BotResult;
use crate::clock::now_millis;
use crate::exchange::Exchange;
use crate::storage::{ActionKind, ActionRecord};

impl<E: Exchange> Bot<E> {
    /// An action of the manager now, in the mode of the bot (dry run or not)
    pub(super) fn action(&self, manager_id: i32, kind: ActionKind) -> ActionRecord {
        ActionRecord::new(manager_id, now_millis(), kind, self.config.dry_run)
    }

    /// Records the action, unless the last one of its manager is of the same kind and `same` as
    /// it: what is repeated at each tick is recorded once, when it changes
    pub(super) fn record_unless_repeated(
        &self,
        record: &ActionRecord,
        same: impl Fn(&ActionRecord) -> bool,
    ) -> BotResult<()> {
        let repeated = self
            .storage
            .last_action(record.manager_id)?
            .is_some_and(|last| last.kind == record.kind && same(&last));
        if !repeated {
            self.storage.record_action(record)?;
        }
        Ok(())
    }
}
