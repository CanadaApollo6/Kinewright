//! A namespace owner grants the actor's attachment capability. Publications
//! retain their immutable captured state; they do not cut or protect a prefix.
#![allow(clippy::missing_errors_doc)]
use crate::{
    operation_actor::{
        AcceptedCut, ActorIdentity, Attachment, ChangeId, CommandContext, Core, Error,
        ManualPermit, PreparedUpdate,
    },
    operation_history::{SnapshotIdentity, ValidatedOwnerSnapshot},
    operation_inputs::ManualInputs,
    operation_protocol::{Action, Lease, Receipt},
};
use kinewright_core::HistorySaveTag;
use std::sync::{Arc, atomic::AtomicBool};

struct ProjectHistoryProof;

pub(crate) fn bind(core: &Core) -> Result<(), Error> {
    core.attach(&ProjectHistoryProof).map(|_| ())
}

/// The source actor and exact immutable state bound to one namespace outcome.
/// No independent document, history, or pool reservation is kept by this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptBinding {
    change: ChangeId,
    identity: SnapshotIdentity,
    tag: HistorySaveTag,
}
impl AttemptBinding {
    pub(crate) const fn new(
        change: ChangeId,
        identity: SnapshotIdentity,
        tag: HistorySaveTag,
    ) -> Self {
        Self {
            change,
            identity,
            tag,
        }
    }
    #[must_use]
    pub const fn actor(&self) -> ActorIdentity {
        self.change.actor
    }
    #[must_use]
    pub const fn change(&self) -> ChangeId {
        self.change
    }
    #[must_use]
    pub const fn identity(&self) -> SnapshotIdentity {
        self.identity
    }
    #[must_use]
    pub const fn tag(&self) -> &HistorySaveTag {
        &self.tag
    }
}

/// Constructors require an actual validated project/session owner. A plain
/// actor handle cannot install or resume that owner's frozen state.
pub struct ProjectAttachment {
    core: Core,
    authority: Attachment<ProjectHistoryProof>,
}
impl ProjectAttachment {
    pub(crate) fn new(core: &Core) -> Result<Self, Error> {
        let authority = core.attach(&ProjectHistoryProof)?;
        Ok(Self {
            core: core.clone(),
            authority,
        })
    }
    pub fn manual_permit(
        &self,
        lease: Lease,
        inputs: ManualInputs,
        live: Arc<AtomicBool>,
    ) -> Result<ManualPermit, Error> {
        self.authority
            .manual_permit(&ProjectHistoryProof, None, lease, inputs, live)
    }
    pub(crate) fn resume_manual(
        &self,
        cut: &AcceptedCut,
        lease: Lease,
        inputs: ManualInputs,
        live: Arc<AtomicBool>,
    ) -> Result<ManualPermit, Error> {
        self.authority
            .manual_permit(&ProjectHistoryProof, Some(*cut), lease, inputs, live)
    }
    pub(crate) fn acknowledge_frozen(
        &self,
        cut: &AcceptedCut,
        receipts: Vec<Receipt>,
    ) -> Result<(), Error> {
        self.authority
            .acknowledge_frozen(&ProjectHistoryProof, *cut, receipts)
    }
    pub fn acknowledge_manual(
        &self,
        permit: &ManualPermit,
        receipts: Vec<Receipt>,
    ) -> Result<(), Error> {
        self.authority
            .acknowledge_manual(&ProjectHistoryProof, permit, receipts)
    }
    pub fn capture(&self) -> Result<AcceptedCut, Error> {
        self.authority.capture(&ProjectHistoryProof)
    }
    /// The writer has already proved this actor at install/publication and is
    /// its sole mutator. Freeze that committed cut without rehashing a warm edit.
    pub(crate) fn freeze_published(
        &self,
        identity: SnapshotIdentity,
        position: kinewright_core::undo::CommandKey,
        revision: kinewright_core::TimelineRevision,
    ) -> Result<AcceptedCut, Error> {
        self.authority
            .freeze_published(&ProjectHistoryProof, identity, position, revision)
    }
    pub fn freeze(&self) -> Result<AcceptedCut, Error> {
        self.authority.freeze(&ProjectHistoryProof)
    }
    pub fn resume(&self, cut: &AcceptedCut) -> Result<(), Error> {
        self.authority.resume(&ProjectHistoryProof, *cut)
    }
    pub fn seal(&self, cut: &AcceptedCut) -> Result<(), Error> {
        self.authority.seal(&ProjectHistoryProof, *cut)
    }
    pub fn retire(&self, cut: &AcceptedCut) -> Result<(), Error> {
        self.authority.retire(&ProjectHistoryProof, *cut)
    }
    pub fn install_snapshot(
        &self,
        cut: &AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
        epoch: Option<u64>,
    ) -> Result<ChangeId, Error> {
        match epoch {
            Some(epoch) => self.authority.install_snapshot_at_epoch(
                &ProjectHistoryProof,
                *cut,
                snapshot,
                epoch,
            ),
            None => self
                .authority
                .install_snapshot(&ProjectHistoryProof, *cut, snapshot),
        }
    }
    pub(crate) fn install_peer_snapshot(
        &self,
        cut: &crate::operation_actor::AcceptedCut,
        snapshot: crate::operation_history::ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
        revision: kinewright_core::TimelineRevision,
    ) -> Result<crate::operation_actor::ChangeId, crate::operation_actor::Error> {
        self.authority
            .install_peer_snapshot(&ProjectHistoryProof, *cut, snapshot, branch, revision)
    }

    pub(crate) fn prepare_selection(
        &self,
        cut: &AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
    ) -> Result<Core, Error> {
        self.authority
            .prepare_selection(&ProjectHistoryProof, *cut, snapshot, branch)
    }
    pub(crate) fn open_validated_selection(
        &self,
        cut: &AcceptedCut,
        snapshot: ValidatedOwnerSnapshot,
        branch: crate::bundle_records::BranchId,
        revision: kinewright_core::TimelineRevision,
    ) -> Result<Core, Error> {
        self.authority.open_validated_selection(
            &ProjectHistoryProof,
            *cut,
            snapshot,
            branch,
            revision,
        )
    }
    pub(crate) fn retarget_selection(
        &self,
        cut: &AcceptedCut,
        branch: crate::bundle_records::BranchId,
    ) -> Result<AcceptedCut, Error> {
        self.authority
            .retarget_selection(&ProjectHistoryProof, *cut, branch)
    }
    pub(crate) fn seal_selection(
        &self,
        cut: &AcceptedCut,
        store: &crate::store::ProjectStore,
        bindings: crate::storage_budget::StorageReservation<
            std::collections::BTreeMap<
                crate::operation_history::Resource,
                crate::bundle_records::ObjectRef,
            >,
        >,
    ) -> Result<crate::operation_history::SealedOwner, Error> {
        self.authority
            .seal_selection(&ProjectHistoryProof, *cut, store, bindings)
    }
    pub(crate) fn prepare_install(
        &self,
        cut: &AcceptedCut,
        action: Action,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.authority
            .prepare_install(&ProjectHistoryProof, *cut, action, context)
    }
    pub(crate) fn prepare(
        &self,
        cut: &AcceptedCut,
        action: Action,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.authority
            .prepare(&ProjectHistoryProof, *cut, action, context)
    }
    pub(crate) fn prepare_reinstall(
        &self,
        cut: &AcceptedCut,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.authority
            .prepare_reinstall(&ProjectHistoryProof, *cut, context)
    }
    pub(crate) fn prepare_merge(
        &self,
        cut: &AcceptedCut,
        operations: Vec<kinewright_core::Operation>,
        context: CommandContext,
    ) -> Result<PreparedUpdate, Error> {
        self.authority
            .prepare_merge(&ProjectHistoryProof, *cut, operations, context)
    }
    pub(crate) fn commit_prepared(
        &self,
        cut: &AcceptedCut,
        update: PreparedUpdate,
        published: SnapshotIdentity,
    ) -> Result<AcceptedCut, Error> {
        let change = self
            .authority
            .commit_prepared(&ProjectHistoryProof, *cut, update)?;
        // The immutable update's identity was already computed for this exact
        // durable root. Finishing the actor needs no fallible post-commit hash.
        Ok(AcceptedCut::committed(change, published))
    }
    /// Verify the original destination outcome; the publication owner applies
    /// its retained update. No save settlement mutates an executor prefix.
    pub fn settle_attempt(
        &self,
        attempt: &AttemptBinding,
    ) -> Result<bool, crate::bundle::BundleError> {
        if self.core.actor_identity() != attempt.actor() {
            return Err(crate::bundle::BundleError::HistoryBarrierInvariant);
        }
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Sealed
        {
            return Ok(false);
        }
        crate::bundle::reconcile_save_tag_continuous(attempt.tag(), self.core.history_pool())
    }
}
