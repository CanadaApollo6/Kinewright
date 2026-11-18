//! Release native actors only after a fresh, exact persisted replay.
#![allow(clippy::missing_errors_doc)]
use super::{BundleError, DurableBranchSessionGuard, Ja1EditingSession};
use crate::{bundle_records::BranchId, operation_history::Role};

impl Ja1EditingSession {
    /// Diagnose a retained task without releasing it or relying on a readable
    /// run projection. Damaged run dependencies must not hide its live work.
    pub fn verify_registered_task_checkpoint(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<bool, BundleError> {
        let bundle = self.registered_bundle(guard)?;
        let checkpoint = bundle.checkpoint(guard.branch_id())?;
        if guard
            .core()
            .snapshot_identity()
            .map_err(crate::bundle::operation_error)?
            != checkpoint.identity
        {
            return Ok(false);
        }
        let reference = bundle.checkpoint_reference(guard.branch_id())?;
        let recorded = bundle
            .store()
            .read_record::<crate::bundle_records::BranchCheckpoint>(&reference)?;
        if *recorded != *checkpoint {
            return Err(BundleError::Corrupt(
                "saved native checkpoint differs from its accepted cut".into(),
            ));
        }
        let opened = checkpoint.open(bundle.store(), bundle.history_pool())?;
        if opened.identity() != checkpoint.identity {
            return Err(BundleError::HistoryChanged);
        }
        Ok(true)
    }
    /// The caller owns one guard; the registration owns the other Core handle.
    /// Additional handles, pending checkpoints and dirty work postpone release.
    pub fn release_checkpointed_native_branch(
        &mut self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<bool, BundleError> {
        let index = self.registered(guard)?;
        self.release_native_registration(index, 2)
    }

    #[must_use]
    pub fn is_registered_run_candidate(&self, branch: &BranchId) -> bool {
        self.durable_sessions.iter().any(|session| {
            session.guard.branch_id() == branch
                && session.guard.base().history_role == Role::Candidate
        })
    }

    /// A settled candidate has no caller-owned guard or Core clone.
    pub fn release_checkpointed_run_candidate(
        &mut self,
        branch: &BranchId,
    ) -> Result<bool, BundleError> {
        let Some(index) = self.durable_sessions.iter().position(|session| {
            session.guard.branch_id() == branch
                && session.guard.base().history_role == Role::Candidate
        }) else {
            return Ok(true);
        };
        self.release_native_registration(index, 1)
    }

    fn release_native_registration(
        &mut self,
        index: usize,
        handles: usize,
    ) -> Result<bool, BundleError> {
        let released = self.verify_native_release(index, handles)?;
        if released {
            self.durable_sessions.remove(index);
        }
        Ok(released)
    }

    fn verify_native_release(&self, index: usize, handles: usize) -> Result<bool, BundleError> {
        let guard = &self.durable_sessions[index].guard;
        if guard.core().handle_count() != handles
            || guard.base().history_role == Role::Ordinary
            || self.durable_sessions.iter().any(|session| {
                session.guard.branch_id() != guard.branch_id()
                    && &session.guard.base().target_branch == guard.branch_id()
            })
        {
            return Ok(false);
        }
        let bundle = self.registered_bundle(guard)?;
        if bundle.unsettled
            || bundle.task_checkpoints.contains_key(guard.branch_id())
            || self
                .core
                .attachment_state()
                .map_err(crate::bundle::operation_error)?
                != kinewright_core::AttachmentState::Live
        {
            return Ok(false);
        }
        let owner = bundle.attach_core(guard.core())?;
        let cut = owner.capture().map_err(crate::bundle::operation_error)?;
        let checkpoint = bundle.checkpoint(guard.branch_id())?;
        if cut.identity() != checkpoint.identity {
            return Ok(false);
        }
        let reference = bundle.checkpoint_reference(guard.branch_id())?;
        let verified = bundle
            .store()
            .read_record::<crate::bundle_records::BranchCheckpoint>(&reference)?;
        if *verified != *checkpoint {
            return Err(BundleError::Corrupt(
                "saved native checkpoint differs from its accepted cut".into(),
            ));
        }
        // Opening validates replay, lineage, fates and the exact resource union.
        // Drop this independent cold reader before dropping the live role lease.
        let replayed = checkpoint.open(bundle.store(), bundle.history_pool())?;
        if replayed.identity() != cut.identity() {
            return Err(BundleError::HistoryChanged);
        }
        drop(replayed);
        let frozen = owner.freeze().map_err(crate::bundle::operation_error)?;
        if frozen != cut {
            owner
                .resume(&frozen)
                .map_err(crate::bundle::operation_error)?;
            return Err(BundleError::HistoryChanged);
        }
        owner
            .retire(&frozen)
            .map_err(crate::bundle::operation_error)?;
        Ok(true)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
