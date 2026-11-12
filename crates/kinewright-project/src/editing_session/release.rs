//! Release native actors only after a fresh, exact persisted replay.
#![allow(clippy::missing_errors_doc)]
use super::{BundleError, DurableBranchSessionGuard, Ja1EditingSession};
use crate::{bundle_records::BranchId, history_record::HistoryRecord};

impl Ja1EditingSession {
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
                && session.guard.base().history_role
                    == crate::history_record::HistoryRole::Candidate
        })
    }

    /// A settled candidate has no caller-owned guard or Core clone.
    pub fn release_checkpointed_run_candidate(
        &mut self,
        branch: &BranchId,
    ) -> Result<bool, BundleError> {
        let Some(index) = self.durable_sessions.iter().position(|session| {
            session.guard.branch_id() == branch
                && session.guard.base().history_role
                    == crate::history_record::HistoryRole::Candidate
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
            || guard.base().history_role == crate::history_record::HistoryRole::Ordinary
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
                .map_err(crate::bundle::history_error)?
                != kinewright_core::AttachmentState::Live
        {
            return Ok(false);
        }
        bundle.store().replay_gate().with(|| {
            let owner = bundle.attach_core(guard.core())?;
            let cut = owner.capture().map_err(crate::bundle::history_error)?;
            let checkpoint = bundle.checkpoint(guard.branch_id())?;
            // This detects dirty or behind-checkpoint work; a matching digest
            // never substitutes for decoding the persisted journal below.
            if crate::manual_history::history_sha256(cut.history())? != checkpoint.history_digest {
                return Ok(false);
            }
            let verified = bundle
                .store()
                .read_record::<crate::bundle_records::BranchCheckpoint>(
                    &bundle.checkpoint_reference(guard.branch_id())?,
                )?;
            if *verified != *checkpoint {
                return Err(BundleError::Corrupt(
                    "saved native checkpoint differs from its accepted cut".into(),
                ));
            }
            let replayed = bundle.store().resolve_history(&checkpoint.history_ref)?;
            HistoryRecord::verify_checkpoint(checkpoint, &replayed)?;
            if *replayed != *cut.history() || checkpoint.batch_ids.len() != replayed.batches.len() {
                return Err(BundleError::Corrupt(
                    "saved native history differs from its live actor".into(),
                ));
            }
            let lineage = bundle.branch_lineage(guard.branch_id())?;
            crate::history_size::verify_history_lineage(
                &replayed,
                &lineage,
                checkpoint.history_role,
                bundle.store().limits(),
            )?;
            let lineage_shadow = crate::lineage_journal::live_bytes(&lineage)?;
            if replayed
                .decoded_charge()
                .checked_add(lineage.decoded_charge())
                .and_then(|bytes| bytes.checked_add(lineage_shadow))
                .is_none_or(|bytes| bytes > bundle.store().limits().decoded_record_bytes)
            {
                return Err(BundleError::Capacity("history replay memory"));
            }
            let _lineage_shadow =
                bundle
                    .store()
                    .reserve_copy_read(0, lineage_shadow, 0, false, true)?;
            let rebuilt = crate::bundle_commit::lineage(
                guard.branch_id(),
                &replayed,
                &checkpoint.batch_ids,
                Some(&lineage),
                Some(&lineage),
            )?;
            if rebuilt != *lineage {
                return Err(BundleError::Corrupt(
                    "saved native lineage differs from replay".into(),
                ));
            }
            let frozen = owner.freeze().map_err(crate::bundle::history_error)?;
            if frozen != cut {
                owner
                    .resume(&frozen)
                    .map_err(crate::bundle::history_error)?;
                return Err(BundleError::HistoryChanged);
            }
            owner.seal(&frozen).map_err(crate::bundle::history_error)?;
            Ok(true)
        })?
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
