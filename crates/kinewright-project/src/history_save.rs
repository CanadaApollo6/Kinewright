//! The project owner is the only constructor of its Core persistence proof.
#![allow(clippy::missing_errors_doc)]
use kinewright_core::{Core, HistoryError, HistorySaveTag, HistoryTransition, PendingHistorySave};

struct ProjectHistoryProof;
pub(crate) struct FloorBarrier(kinewright_core::HistoryFloorBarrier<ProjectHistoryProof>);
impl FloorBarrier {
    pub(crate) fn retain_until_reconcile(&mut self) -> Result<(), HistoryError> {
        self.0.retain_until_reconcile(&ProjectHistoryProof)
    }
    pub(crate) fn tag_publication(&mut self, tag: HistorySaveTag) -> Result<(), HistoryError> {
        self.0.tag_publication(&ProjectHistoryProof, tag)
    }
    pub(crate) fn commit(&mut self) -> Result<(), HistoryError> {
        self.0.commit(&ProjectHistoryProof)
    }
    pub(crate) fn release(&mut self) -> Result<(), HistoryError> {
        self.0.release()
    }
}
pub(crate) fn bind(core: &Core) -> Result<(), HistoryError> {
    core.history_save_owner(&ProjectHistoryProof).map(|_| ())
}
pub(crate) fn begin(
    core: &Core,
    transition: HistoryTransition,
) -> Result<FloorBarrier, HistoryError> {
    core.history_save_owner(&ProjectHistoryProof)?
        .begin_history_floor(&ProjectHistoryProof, transition)
        .map(FloorBarrier)
}
pub(crate) fn settle(
    core: &Core,
    pending: &PendingHistorySave,
    commit: bool,
) -> Result<(), HistoryError> {
    core.history_save_owner(&ProjectHistoryProof)?
        .settle_history_save(&ProjectHistoryProof, pending, commit)
}

/// Opaque actor/attempt binding captured by the original publication owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptBinding {
    ticket: PendingHistorySave,
}
impl AttemptBinding {
    #[must_use]
    pub const fn actor(&self) -> kinewright_core::ActorIdentity {
        self.ticket.actor
    }
    #[must_use]
    pub const fn ticket(&self) -> &PendingHistorySave {
        &self.ticket
    }
}
impl FloorBarrier {
    pub(crate) fn attempt(&self) -> AttemptBinding {
        AttemptBinding {
            ticket: self.0.ticket(),
        }
    }
}

/// Authorized project attachment; constructors require a validated project/session owner.
pub struct ProjectAttachment {
    core: Core,
    authority: kinewright_core::CoreAttachment<ProjectHistoryProof>,
}
impl ProjectAttachment {
    pub(crate) fn new(core: &Core) -> Result<Self, HistoryError> {
        core.history_save_owner(&ProjectHistoryProof)
            .map(|authority| Self {
                core: core.clone(),
                authority,
            })
    }
    pub fn manual_permit(
        &self,
        encoder: Box<dyn kinewright_core::ManualHistoryEncoder>,
    ) -> Result<kinewright_core::ManualPermit, HistoryError> {
        self.authority.manual_permit(&ProjectHistoryProof, encoder)
    }
    pub fn acknowledge_manual(
        &self,
        permit: &kinewright_core::ManualPermit,
        prefix: Vec<kinewright_core::AcceptedHistory>,
    ) -> Result<(), HistoryError> {
        self.authority
            .acknowledge_manual(&ProjectHistoryProof, permit, prefix)
    }
    pub fn capture(&self) -> Result<kinewright_core::AcceptedCut, HistoryError> {
        self.authority.capture(&ProjectHistoryProof)
    }
    pub fn freeze(&self) -> Result<kinewright_core::AcceptedCut, HistoryError> {
        self.authority.freeze(&ProjectHistoryProof)
    }
    pub fn resume(&self, cut: &kinewright_core::AcceptedCut) -> Result<(), HistoryError> {
        self.authority.resume(&ProjectHistoryProof, cut)
    }
    pub fn seal(&self, cut: &kinewright_core::AcceptedCut) -> Result<(), HistoryError> {
        self.authority.seal(&ProjectHistoryProof, cut)
    }
    pub fn restore_history(
        &self,
        history: kinewright_core::HistorySnapshot,
    ) -> Result<(), HistoryError> {
        self.authority
            .restore_history(&ProjectHistoryProof, history)
    }
    pub fn install_checkpoint(
        &self,
        before: kinewright_core::HistorySnapshot,
        after: kinewright_core::HistorySnapshot,
    ) -> Result<(), HistoryError> {
        self.authority
            .install_checkpoint(&ProjectHistoryProof, before, after)
    }
    pub fn install_history_transition(
        &self,
        transition: HistoryTransition,
    ) -> Result<(), HistoryError> {
        self.authority
            .install_history_transition(&ProjectHistoryProof, transition)
    }
    /// Settle only the original attempt, after proof of its own destination's durable outcome.
    /// Sealed actors receive no settlement message. Callers supply the active attachment only.
    pub fn settle_attempt(
        &self,
        attempt: &AttemptBinding,
    ) -> Result<bool, crate::bundle::BundleError> {
        let core = &self.core;
        if core.actor_identity() != attempt.actor() {
            return Err(crate::bundle::BundleError::HistoryBarrierInvariant);
        }
        if core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(false);
        }
        let Some(pending) = core
            .pending_history_save()
            .map_err(crate::bundle::history_error)?
        else {
            return Ok(false);
        };
        if pending != attempt.ticket {
            return Err(crate::bundle::BundleError::HistoryBarrierInvariant);
        }
        if !pending.ready {
            return Err(crate::bundle::BundleError::SaveInProgress);
        }
        let tag = attempt
            .ticket
            .tag
            .as_ref()
            .ok_or(crate::bundle::BundleError::HistoryBarrierInvariant)?;
        let commit = crate::bundle::reconcile_save_tag_continuous(tag)?;
        settle(core, &attempt.ticket, commit).map_err(crate::bundle::history_error)?;
        Ok(true)
    }
}
