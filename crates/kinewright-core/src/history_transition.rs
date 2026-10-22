//! Validated, actor-installed transitions of a frozen history cut.
use crate::{Command, Core, Event, HistoryError, HistorySnapshot};

/// Revocable save barrier. Definite failure releases on drop; uncertain publication
/// retains the barrier until its owner explicitly commits or releases it.
pub struct HistoryFloorBarrier {
    core: Core,
    offset: u64,
    retain: bool,
    settled: bool,
}
impl HistoryFloorBarrier {
    pub(crate) const fn new(core: Core, offset: u64) -> Self {
        Self {
            core,
            offset,
            retain: false,
            settled: false,
        }
    }
    pub fn retain_until_reconcile(&mut self) {
        self.retain = true;
    }
    /// Commit the floor against Core's current history.
    /// # Errors
    /// Returns an error if this barrier no longer matches or Core disconnected.
    pub fn commit(&mut self) -> Result<(), HistoryError> {
        if self.settled {
            return Err(HistoryError::Corrupt);
        }
        self.retain = true;
        self.core.settle_history_floor(self.offset, true)?;
        self.settled = true;
        Ok(())
    }
    /// Release without discarding any entries.
    /// # Errors
    /// Returns an error if this barrier no longer matches or Core disconnected.
    pub fn release(&mut self) -> Result<(), HistoryError> {
        if self.settled {
            return Err(HistoryError::Corrupt);
        }
        self.core.settle_history_floor(self.offset, false)?;
        self.settled = true;
        Ok(())
    }
}
impl Drop for HistoryFloorBarrier {
    fn drop(&mut self) {
        if !self.settled && !self.retain {
            let _ = self.release();
        }
    }
}

pub struct HistoryTransition {
    pub(crate) before: HistorySnapshot,
    pub(crate) after: HistorySnapshot,
}

fn replay(core: &Core, command: Command) -> Result<(), HistoryError> {
    if !matches!(
        core.request(command)
            .map_err(|_| HistoryError::Disconnected)?,
        Event::DocumentChanged { .. }
    ) {
        return Err(HistoryError::Corrupt);
    }
    Ok(())
}

impl HistoryTransition {
    /// Search in command order, replaying each prefix command once. The budget callback
    /// can skip candidates using a lower bound; every accepted cut still replays its suffix.
    /// # Errors
    /// Refuses corrupt history or a history with no equivalent fitting floor.
    pub fn find_floor(
        before: HistorySnapshot,
        may_fit: impl FnMut(usize, usize, &std::sync::Arc<crate::Document>) -> bool,
        accepts: impl FnMut(&HistorySnapshot) -> bool,
    ) -> Result<Self, HistoryError> {
        let after = crate::actor::find_floor_history(&before, may_fit, accepts)?;
        Ok(Self { before, after })
    }
    /// Discard a closed prefix only when the retained cursor is exactly equivalent.
    /// # Errors
    /// Refuses a corrupt cut, a prefix intersecting a retained gesture or a changed redo chain.
    pub fn floor(before: HistorySnapshot, discard_batches: usize) -> Result<Self, HistoryError> {
        let after = crate::actor::floor_history(&before, discard_batches)?;
        Ok(Self { before, after })
    }

    /// Close the final gesture only after the entire reconciled suffix equals the cut.
    /// Uses the ordinary Undo/Redo executor to close the key without another undo entry.
    /// # Errors
    /// Refuses an unmatched suffix, corrupt history or a live redo chain with an open gesture.
    pub fn recovery_close(
        before: HistorySnapshot,
        reconciled: &HistorySnapshot,
    ) -> Result<Self, HistoryError> {
        if before != *reconciled {
            return Err(HistoryError::Corrupt);
        }
        let candidate =
            Core::spawn((*before.baseline).clone()).map_err(|_| HistoryError::Corrupt)?;
        candidate.restore_history(before.clone())?;
        if before
            .cursor
            .undo
            .last()
            .is_some_and(|entry| entry.coalesce_key.is_some())
        {
            if !before.cursor.redo.is_empty() {
                return Err(HistoryError::Corrupt);
            }
            replay(&candidate, Command::Undo)?;
            replay(&candidate, Command::Redo)?;
        }
        let after = candidate.export_history()?;
        if before.document != after.document
            || before.applied_operations() != after.applied_operations()
            || before.cursor.undo.len() != after.cursor.undo.len()
            || before.cursor.redo != after.cursor.redo
        {
            return Err(HistoryError::Corrupt);
        }
        Ok(Self { before, after })
    }

    #[must_use]
    pub const fn before(&self) -> &HistorySnapshot {
        &self.before
    }
    #[must_use]
    pub const fn after(&self) -> &HistorySnapshot {
        &self.after
    }
}
