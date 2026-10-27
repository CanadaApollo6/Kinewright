//! Validated, actor-installed transitions of a frozen history cut.
#![allow(clippy::missing_errors_doc)]
use crate::{Command, Core, Event, HistoryError, HistorySnapshot};

/// Runtime reconciliation identity supplied by the persistence owner. Core does no IO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySaveTag {
    pub destination: std::path::PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub root_sha256: String,
    pub previous_root_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHistorySave {
    pub actor: crate::ActorIdentity,
    pub id: u64,
    pub tag: Option<HistorySaveTag>,
    pub ready: bool,
}

/// Persistence capability bound to an owner's private proof type.
/// Merely holding a Core or a copied ticket cannot settle that owner's save.
///
/// ```compile_fail
/// use kinewright_core::{Core, PendingHistorySave};
/// fn forge(core: &Core, ticket: &PendingHistorySave) {
///     core.settle_history_save(ticket, true).unwrap();
/// }
/// ```
/// ```compile_fail
/// use kinewright_core::{Core, HistoryTransition};
/// fn forge(core: &Core, transition: HistoryTransition) {
///     core.begin_history_floor(transition).unwrap();
/// }
/// ```
pub type HistorySaveOwner<P> = crate::CoreAttachment<P>;
impl<P: 'static> crate::CoreAttachment<P> {
    pub fn begin_history_floor(
        &self,
        _proof: &P,
        transition: HistoryTransition,
    ) -> Result<HistoryFloorBarrier<P>, HistoryError> {
        let id = self.core.begin_history_floor::<P>(transition)?;
        Ok(HistoryFloorBarrier::new(self.core.clone(), id))
    }
    pub fn settle_history_save(
        &self,
        _proof: &P,
        pending: &PendingHistorySave,
        commit: bool,
    ) -> Result<(), HistoryError> {
        self.core.settle_history_save::<P>(pending, commit)
    }
}

/// Revocable save barrier; only the bound owner's proof authorizes commit or retention.
pub struct HistoryFloorBarrier<P: 'static> {
    core: Core,
    id: u64,
    retain: bool,
    tag: Option<HistorySaveTag>,
    settled: bool,
    proof: std::marker::PhantomData<fn(P)>,
}
impl<P: 'static> HistoryFloorBarrier<P> {
    pub(crate) const fn new(core: Core, id: u64) -> Self {
        Self {
            core,
            id,
            retain: false,
            tag: None,
            settled: false,
            proof: std::marker::PhantomData,
        }
    }
    pub fn retain_until_reconcile(&mut self, _proof: &P) -> Result<(), HistoryError> {
        self.core.tag_history_floor::<P>(self.id, None, true)?;
        self.retain = true;
        Ok(())
    }
    pub fn tag_publication(&mut self, _proof: &P, tag: HistorySaveTag) -> Result<(), HistoryError> {
        self.core
            .tag_history_floor::<P>(self.id, Some(tag.clone()), false)?;
        self.tag = Some(tag);
        Ok(())
    }
    #[must_use]
    pub fn ticket(&self) -> PendingHistorySave {
        PendingHistorySave {
            actor: self.core.actor_identity(),
            id: self.id,
            tag: self.tag.clone(),
            ready: self.retain,
        }
    }
    pub fn commit(&mut self, _proof: &P) -> Result<(), HistoryError> {
        if self.settled {
            return Err(HistoryError::BarrierInvariant);
        }
        // The actor releases losslessly on an internal commit failure.
        self.core.settle_history_floor::<P>(self.id, true)?;
        self.settled = true;
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), HistoryError> {
        if self.settled {
            return Err(HistoryError::BarrierInvariant);
        }
        self.core.settle_history_floor::<P>(self.id, false)?;
        self.settled = true;
        Ok(())
    }
}
impl<P: 'static> Drop for HistoryFloorBarrier<P> {
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
        struct RecoveryReplayProof;
        if before != *reconciled {
            return Err(HistoryError::Corrupt);
        }
        let candidate =
            Core::spawn((*before.baseline).clone()).map_err(|_| HistoryError::Corrupt)?;
        candidate
            .history_save_owner(&RecoveryReplayProof)?
            .restore_history(&RecoveryReplayProof, before.clone())?;
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
