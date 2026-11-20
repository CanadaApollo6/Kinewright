//! Synchronous B0–B5 publication through the one store/namespace owner.
#![allow(clippy::missing_errors_doc)]
use crate::operation_actor::Core;
use crate::{
    bundle::{
        BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle, StudioRoot,
        SupportInventory,
    },
    bundle_records::{BatchId, BranchCheckpoint, HeadId, ObjectRef, RecordKind},
    owned_inputs::OwnedInputManifest,
    receipts::{CommandEnvelope, CommandReceipt, ReceiptLookup, ReceiptResult},
    store::SealedObject,
};

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitBarrier {
    B0,
    B1,
    B2,
    B3,
    B4,
    B5,
}
#[derive(Debug)]
pub enum CommitOutcome {
    Committed(BundleSaveReport),
    DefinitelyUnpublished(BundleError),
    DurabilityUnknown,
    CommittedWithError(BundleError),
}
enum BranchPublication {
    Published(BundleSaveReport),
    Staged(Box<crate::task_checkpoint::StagedTaskCheckpoint>),
    Prepared(Box<PreparedMaintenancePublication>),
}
impl BranchPublication {
    fn published(self) -> Result<BundleSaveReport, BundleError> {
        match self {
            Self::Published(report) => Ok(report),
            Self::Staged(_) | Self::Prepared(_) => Err(BundleError::HistoryBarrierInvariant),
        }
    }
}
pub struct BundleCommit;
#[derive(Clone, Copy)]
pub(crate) enum PublicationView<'a> {
    Capture(&'a crate::operation_actor::CapturedHistory),
    Continuity(
        &'a crate::operation_actor::CapturedHistory,
        crate::operation_history::SnapshotIdentity,
    ),
    Snapshot(
        &'a crate::operation_history::OwnerSnapshot,
        kinewright_core::TimelineRevision,
    ),
    Prepared(&'a crate::operation_actor::PreparedUpdate),
    Reinstall(
        &'a crate::operation_actor::PreparedUpdate,
        &'a crate::operation_checkpoint::InstallRecord,
    ),
    Selection(&'a crate::bundle_switch::PreparedProjectSelection),
    Frozen(
        &'a crate::ProjectAttachment,
        &'a crate::operation_actor::AcceptedCut,
        &'a BTreeSet<ObjectRef>,
    ),
}
impl PublicationView<'_> {
    fn revision(self) -> kinewright_core::TimelineRevision {
        match self {
            Self::Capture(capture) | Self::Continuity(capture, _) => capture.change.revision,
            Self::Snapshot(_, revision) => revision,
            Self::Prepared(update) | Self::Reinstall(update, _) => update.revision(),
            Self::Selection(selection) => selection.restore.cut.change().revision,
            Self::Frozen(_, cut, _) => cut.change().revision,
        }
    }
    fn seal(
        self,
        bundle: &mut ProjectBundle,
        inputs: &ObjectRef,
        prior: Option<&BranchCheckpoint>,
    ) -> Result<crate::operation_history::SealedOwner, BundleError> {
        let mut sources = std::collections::BTreeSet::from([inputs.clone()]);
        if let Some(prior) = prior {
            sources.insert(prior.owner_ref.clone());
        }
        if let Self::Capture(capture) | Self::Continuity(capture, _) = self {
            sources.extend(
                capture
                    .snapshot
                    .origins()
                    .iter()
                    .flat_map(|origin| origin.inputs.iter().cloned()),
            );
        }
        if let Self::Snapshot(snapshot, _) = self {
            sources.extend(
                snapshot
                    .origins()
                    .iter()
                    .flat_map(|origin| origin.inputs.iter().cloned()),
            );
        }
        if let Self::Prepared(update) | Self::Reinstall(update, _) = self {
            sources.extend(update.input_contexts().cloned());
        }
        if let Self::Selection(selection) = self {
            sources.insert(selection.restore.checkpoint.owner_ref.clone());
        }
        if let Self::Frozen(_, _, references) = self {
            sources.extend(references.iter().cloned());
        }
        let bindings = crate::operation_publication::bindings(&bundle.store, &sources, inputs)?;
        match self {
            Self::Frozen(attachment, cut, _) => attachment
                .seal_selection(cut, &bundle.store, bindings)
                .map_err(crate::bundle::operation_error),

            Self::Capture(capture) | Self::Continuity(capture, _) => capture
                .snapshot
                .persist_checkpoint(&mut bundle.store, |resource| {
                    Ok(bindings.get(resource).cloned())
                })
                .map_err(BundleError::from),
            Self::Snapshot(snapshot, _) => snapshot
                .persist_checkpoint(&mut bundle.store, |resource| {
                    Ok(bindings.get(resource).cloned())
                })
                .map_err(BundleError::from),
            Self::Prepared(update) | Self::Reinstall(update, _) => update
                .persist_checkpoint(&mut bundle.store, |resource| {
                    Ok(bindings.get(resource).cloned())
                })
                .map_err(crate::bundle::operation_error),
            Self::Selection(selection) => selection
                .restore
                .attachment
                .seal_selection(&selection.restore.cut, &bundle.store, bindings)
                .map_err(crate::bundle::operation_error),
        }
    }
}

impl BundleCommit {
    /// Stage through Core, then publish the result and issuer sequence with the same root.
    #[allow(clippy::too_many_lines)]
    pub fn command(
        bundle: &mut ProjectBundle,
        envelope: &CommandEnvelope,
        core: &Core,
        inputs: &ObjectRef,
        blobs: &[(&ObjectRef, &[u8])],
    ) -> Result<ReceiptLookup, BundleError> {
        Self::command_inner(bundle, envelope, core, inputs, blobs, true)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one existing transactional command publisher with optional frozen install"
    )]
    pub(crate) fn command_inner(
        bundle: &mut ProjectBundle,
        envelope: &CommandEnvelope,
        core: &Core,
        inputs: &ObjectRef,
        blobs: &[(&ObjectRef, &[u8])],
        install: bool,
    ) -> Result<ReceiptLookup, BundleError> {
        let target = bundle.manual_branch_id.clone();
        Self::command_target(bundle, envelope, core, inputs, blobs, install, &target)
    }
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(crate) fn command_target(
        bundle: &mut ProjectBundle,
        envelope: &CommandEnvelope,
        core: &Core,
        inputs: &ObjectRef,
        blobs: &[(&ObjectRef, &[u8])],
        install: bool,
        target: &crate::bundle_records::BranchId,
    ) -> Result<ReceiptLookup, BundleError> {
        // Durable results precede fresh authority, including after a restart.
        let lookup = bundle.receipt_lookup(envelope)?;
        if lookup != ReceiptLookup::Absent {
            if matches!(
                lookup,
                ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
            ) {
                bundle.settle_matching_receipt(envelope)?;
            }
            return Ok(lookup);
        }
        if envelope.action == "metadata" {
            return Err(BundleError::Unsupported(
                "typed owner ingress required".into(),
            ));
        }
        if bundle.unsettled
            || bundle.pending_command.is_some()
            || bundle.pending_publication.is_some()
        {
            return Err(BundleError::Unsettled);
        }
        let retained_candidate = !install
            && target != &bundle.manual_branch_id
            && envelope.action == "edit"
            && bundle.retained_candidate_merge(envelope)?;
        let checkpoint = bundle.checkpoint(target)?.clone();
        if envelope.project_id != bundle.project_id
            || envelope.target_branch != *target
            || (envelope.owner_epoch != bundle.store.owner_epoch() && !retained_candidate)
            || envelope.lease.is_some()
            || envelope.expected_head.as_ref() != Some(&checkpoint.head_id)
        {
            return Err(BundleError::Unsupported(
                "stale command authority/head".into(),
            ));
        }
        let change = core.change_id().map_err(crate::bundle::operation_error)?;
        if change.revision != envelope.expected_revision && !retained_candidate {
            return Err(BundleError::HistoryChanged);
        }
        let issuer = bundle
            .issuers
            .get(&envelope.id.issuer)
            .cloned()
            .ok_or(BundleError::HistoryChanged)?;
        if issuer.retired || issuer.next_sequence != envelope.id.sequence {
            return Err(BundleError::HistoryChanged);
        }
        let next_sequence = issuer
            .next_sequence
            .checked_add(1)
            .ok_or(BundleError::Capacity("issuer sequence"))?;
        if bundle.store.admission_limits().pending_commands == 0
            || (bundle.receipts.detailed.len() + bundle.receipts.retired.len()) as u64
                >= bundle.store.admission_limits().receipts
        {
            return Err(bundle
                .store
                .run_error(BundleError::Capacity("pending command/receipt entries")));
        }
        let encoded = envelope.encoded_bytes()?;
        if encoded > bundle.store.limits().command_bytes {
            return Err(BundleError::Capacity("command bytes"));
        }
        let reservation = bundle.store.reserve(0, encoded, false)?;
        if envelope
            .context
            .as_ref()
            .is_some_and(|context| context.creation.branch != *target)
        {
            return Err(BundleError::HistoryChanged);
        }
        let owner = bundle.attach_core(core)?;
        let was_live = core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            == kinewright_core::AttachmentState::Live;
        if install && !was_live {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        let cut = if envelope.action == "comment" && envelope.command.is_none() {
            // Comments publish metadata over the saved checkpoint. Freeze the
            // current actor without pretending its unsaved suffix is that cut.
            owner.freeze()
        } else {
            owner.freeze_published(
                checkpoint.identity,
                checkpoint.position,
                checkpoint.revision,
            )
        }
        .map_err(crate::bundle::operation_error)?;
        let cut = if core
            .creation_branch()
            .map_err(crate::bundle::operation_error)?
            == *target
        {
            cut
        } else {
            owner
                .retarget_selection(&cut, target.clone())
                .map_err(crate::bundle::operation_error)?
        };
        if let Some(command) = &envelope.command {
            let action = crate::operation_protocol::Action::from_normalised(
                kinewright_core::Command::from(command.clone()),
            )
            .map_err(|error| {
                crate::bundle::operation_error(crate::operation_actor::Error::Protocol(error))
            })?;
            let context = if let Some(context) = &envelope.context {
                if context.creation.branch != *target {
                    return Err(BundleError::HistoryChanged);
                }
                context.clone()
            } else {
                crate::operation_actor::CommandContext {
                    creation: crate::operation_history::CreationOrigin {
                        branch: target.clone(),
                        batch: BatchId::command(&envelope.id)?,
                    },
                    inputs: if matches!(action, crate::operation_protocol::Action::Do { .. }) {
                        vec![inputs.clone()]
                    } else {
                        vec![]
                    },
                    approval: None,
                    transition: None,
                }
            };
            let update = if target != &bundle.manual_branch_id
                && checkpoint.role == crate::operation_history::Role::Candidate
            {
                match action {
                    crate::operation_protocol::Action::Do {
                        operations,
                        gesture: None,
                    } => owner.prepare_merge(&cut, operations, context),
                    action => owner.prepare(&cut, action, context),
                }
            } else if install {
                owner.prepare_install(&cut, action, context)
            } else {
                owner.prepare(&cut, action, context)
            };
            let update = match update {
                Ok(update) => update,
                Err(error) => {
                    if was_live {
                        owner.resume(&cut).map_err(crate::bundle::operation_error)?;
                    }
                    return Err(crate::bundle::operation_error(error));
                }
            };
            bundle.pending_command = Some(reservation.hold(envelope.clone()));
            bundle
                .issuers
                .get_mut(&envelope.id.issuer)
                .ok_or(BundleError::HistoryChanged)?
                .next_sequence = next_sequence;
            let result = Self::publish_update(
                bundle, core, cut, update, inputs, target, None, was_live, None,
            );
            if let Err(error) = result {
                if !bundle.unsettled {
                    bundle.issuers.insert(envelope.id.issuer.clone(), issuer);
                    bundle.pending_command = None;
                }
                return Err(error);
            }
        } else {
            if envelope.action != "comment" {
                if was_live {
                    owner.resume(&cut).map_err(crate::bundle::operation_error)?;
                }
                return Err(BundleError::HistoryChanged);
            }
            let result = crate::receipts::stage_comment(bundle, envelope, blobs);
            if let Err(error) = result {
                if was_live {
                    owner.resume(&cut).map_err(crate::bundle::operation_error)?;
                }
                return Err(error);
            }
            bundle.pending_result = Some(result?);
            bundle.pending_command = Some(reservation.hold(envelope.clone()));
            bundle
                .issuers
                .get_mut(&envelope.id.issuer)
                .ok_or(BundleError::HistoryChanged)?
                .next_sequence = next_sequence;
            let result = Self::publish_maintenance(bundle);
            if was_live {
                owner.resume(&cut).map_err(crate::bundle::operation_error)?;
            }
            result?;
        }
        bundle.pending_command = None;
        bundle.pending_result = None;
        bundle.receipt_lookup(envelope)
    }
    /// The host validates the lease and input certificate before this ingress.
    /// Stream receipts remain the authoritative duplicate check after restart.
    pub fn admitted_history(
        bundle: &mut ProjectBundle,
        message: crate::operation_protocol::Message,
        shadow: &Core,
        inputs: &ObjectRef,
    ) -> Result<(crate::operation_protocol::Receipt, Option<BundleSaveReport>), BundleError> {
        let branch = bundle.manual_branch_id.clone();
        let checkpoint = bundle.checkpoint(&branch)?.clone();
        let stream = checkpoint
            .stream
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?;
        if let Some(receipt) = stream.lookup(&message).map_err(|e| {
            crate::bundle::operation_error(crate::operation_actor::Error::Protocol(e))
        })? {
            return Ok((receipt.clone(), None));
        }
        if bundle.unsettled || bundle.pending_publication.is_some() {
            return Err(BundleError::Unsettled);
        }
        if bundle
            .issuers
            .get(&message.id.issuer)
            .is_none_or(|issuer| issuer.next_sequence != message.id.sequence)
        {
            return Err(BundleError::HistoryChanged);
        }
        if message.context.creation.branch != branch
            || message.context.approval.is_some()
            || message.context.transition.is_some()
            || message
                .context
                .inputs
                .iter()
                .any(|reference| reference != inputs)
        {
            return Err(BundleError::HistoryChanged);
        }
        let (stream, receipt) = stream
            .advanced(&message, checkpoint.position)
            .map_err(|e| {
                crate::bundle::operation_error(crate::operation_actor::Error::Protocol(e))
            })?;
        let owner = bundle.attach_core(shadow)?;
        let cut = owner
            .freeze_published(
                checkpoint.identity,
                checkpoint.position,
                checkpoint.revision,
            )
            .map_err(crate::bundle::operation_error)?;
        let update = match owner.preview_manual(cut).and_then(|mut preview| {
            preview.apply_message(message)?;
            preview
                .finish()?
                .ok_or(crate::operation_actor::Error::CaptureChanged)
        }) {
            Ok(update) => update,
            Err(error) => {
                owner.resume(&cut).map_err(crate::bundle::operation_error)?;
                return Err(crate::bundle::operation_error(error));
            }
        };
        let report = Self::publish_update(
            bundle,
            shadow,
            cut,
            update,
            inputs,
            &branch,
            Some(stream),
            true,
            None,
        )?;
        // This answer is constructed only after root publication and shadow commit.
        Ok((receipt, Some(report)))
    }
    #[allow(clippy::too_many_arguments)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the publication consumes the original bounded install evidence with its attempt"
    )]
    pub(crate) fn publish_update(
        bundle: &mut ProjectBundle,
        core: &Core,
        cut: crate::operation_actor::AcceptedCut,
        update: crate::operation_actor::PreparedUpdate,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
        stream: Option<crate::operation_protocol::StreamState>,
        resume: bool,
        install: Option<crate::operation_checkpoint::InstallRecord>,
    ) -> Result<BundleSaveReport, BundleError> {
        let identity = match update.identity() {
            Ok(identity) => identity,
            Err(error) => {
                if resume {
                    bundle
                        .attach_core(core)?
                        .resume(&cut)
                        .map_err(crate::bundle::operation_error)?;
                }
                return Err(crate::bundle::operation_error(error));
            }
        };
        bundle.publication_state = crate::bundle::PublicationState::NeverAttempted;
        bundle.pending_publication = Some(crate::bundle::PendingPublication {
            candidate_base: None,
            core: core.clone(),
            change: cut.change(),
            identity,
            owner: None,
            root_sha256: None,
            seals: Vec::new(),
        });
        let view = install
            .as_ref()
            .map_or(PublicationView::Prepared(&update), |install| {
                PublicationView::Reinstall(&update, install)
            });
        let result = Self::publish_branch_impl(
            bundle,
            Some(view),
            inputs,
            branch,
            None,
            false,
            &[],
            stream,
            None,
            None,
        )
        .and_then(BranchPublication::published);
        match result {
            Ok(report) => {
                bundle
                    .pending_publication
                    .as_mut()
                    .ok_or(BundleError::HistoryBarrierInvariant)?
                    .owner = Some(crate::bundle::PublicationOwner::Update {
                    cut,
                    update,
                    resume,
                });
                if let Err(error) = bundle.settle_pending_publication(true) {
                    bundle.unsettled = true;
                    return Err(error);
                }
                Ok(report)
            }
            Err(error) => {
                if matches!(
                    bundle.publication_state,
                    crate::bundle::PublicationState::Unknown
                        | crate::bundle::PublicationState::Committed
                ) {
                    bundle
                        .pending_publication
                        .as_mut()
                        .ok_or(BundleError::HistoryBarrierInvariant)?
                        .owner = Some(crate::bundle::PublicationOwner::Update {
                        cut,
                        update,
                        resume,
                    });
                    bundle.unsettled = true;
                } else {
                    drop(update);
                    bundle.pending_publication = None;
                    if resume {
                        bundle
                            .attach_core(core)?
                            .resume(&cut)
                            .map_err(crate::bundle::operation_error)?;
                    }
                }
                Err(error)
            }
        }
    }
    pub fn run(bundle: &mut ProjectBundle, core: &Core, inputs: &ObjectRef) -> CommitOutcome {
        if bundle.pending_command.is_some() {
            return match bundle.publication_state {
                crate::bundle::PublicationState::Unknown => CommitOutcome::DurabilityUnknown,
                crate::bundle::PublicationState::Committed => {
                    CommitOutcome::CommittedWithError(BundleError::Unsettled)
                }
                _ => CommitOutcome::DefinitelyUnpublished(BundleError::Unsettled),
            };
        }
        match bundle.save(core, inputs) {
            Ok(report) => CommitOutcome::Committed(report),
            Err(error)
                if bundle.publication_state == crate::bundle::PublicationState::Committed =>
            {
                CommitOutcome::CommittedWithError(error)
            }
            Err(_) if bundle.publication_state == crate::bundle::PublicationState::Unknown => {
                CommitOutcome::DurabilityUnknown
            }
            Err(error) => CommitOutcome::DefinitelyUnpublished(error),
        }
    }
    pub(crate) fn publish_capture(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        Self::publish_capture_kind(bundle, capture, inputs, branch, None)
    }
    pub(crate) fn publish_continuity_capture(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
        source: crate::operation_history::SnapshotIdentity,
    ) -> Result<BundleSaveReport, BundleError> {
        let prior = bundle.checkpoint(branch)?;
        if prior.identity != source
            || prior.input_manifest_ref != *inputs
            || capture.change.position.epoch <= prior.position.epoch
        {
            return Err(BundleError::HistoryChanged);
        }
        Self::publish_capture_kind(bundle, capture, inputs, branch, Some(source))
    }
    fn publish_capture_kind(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
        continuity: Option<crate::operation_history::SnapshotIdentity>,
    ) -> Result<BundleSaveReport, BundleError> {
        let stream = match bundle.branches.get(branch) {
            Some(checkpoint) if checkpoint.stream.is_some() => {
                let identity = capture.snapshot.identity()?;
                if checkpoint.position.epoch != capture.change.position.epoch {
                    Some(
                        crate::operation_protocol::StreamState::installed(
                            crate::operation_protocol::Seed {
                                project: bundle.project_id.clone(),
                                branch: branch.clone(),
                                attachment_epoch: capture.change.position.epoch,
                                installed: identity,
                            },
                        )
                        .map_err(|error| BundleError::Corrupt(error.to_string()))?,
                    )
                } else if checkpoint.identity == identity {
                    checkpoint.stream.clone()
                } else {
                    return Err(BundleError::HistoryChanged);
                }
            }
            _ => None,
        };
        Self::publish_branch_impl(
            bundle,
            Some(
                continuity.map_or(PublicationView::Capture(capture), |source| {
                    PublicationView::Continuity(capture, source)
                }),
            ),
            inputs,
            branch,
            None,
            false,
            &[],
            stream,
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn publish_capture_with_stream(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
        stream: Option<crate::operation_protocol::StreamState>,
    ) -> Result<BundleSaveReport, BundleError> {
        Self::publish_branch_impl(
            bundle,
            Some(PublicationView::Capture(capture)),
            inputs,
            branch,
            None,
            false,
            &[],
            stream,
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn publish_branch(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        Self::publish_capture(bundle, capture, inputs, branch)
    }
    pub(crate) fn publish_selection(
        bundle: &mut ProjectBundle,
        selection: &crate::bundle_switch::PreparedProjectSelection,
    ) -> Result<BundleSaveReport, BundleError> {
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("switch_before_publish");
        let result = Self::publish_branch_impl(
            bundle,
            Some(PublicationView::Selection(selection)),
            &selection.restore.checkpoint.input_manifest_ref,
            &selection.target_branch,
            Some(selection),
            false,
            &[],
            None,
            None,
            None,
        )?
        .published();
        #[cfg(any(test, feature = "test-util"))]
        if result.is_ok() {
            crate::test_hook("switch_after_publish");
        }
        result
    }
    pub(crate) fn publish_snapshot(
        bundle: &mut ProjectBundle,
        snapshot: &crate::operation_history::OwnerSnapshot,
        revision: kinewright_core::TimelineRevision,
        inputs: &ObjectRef,
    ) -> Result<BundleSaveReport, BundleError> {
        let branch = bundle.manual_branch_id.clone();
        Self::publish_branch_impl(
            bundle,
            Some(PublicationView::Snapshot(snapshot, revision)),
            inputs,
            &branch,
            None,
            false,
            &[],
            None,
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn publish_frozen_branch(
        bundle: &mut ProjectBundle,
        attachment: &crate::ProjectAttachment,
        cut: &crate::operation_actor::AcceptedCut,
        sources: &BTreeSet<ObjectRef>,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
    ) -> Result<BundleSaveReport, BundleError> {
        Self::publish_branch_impl(
            bundle,
            Some(PublicationView::Frozen(attachment, cut, sources)),
            inputs,
            branch,
            None,
            false,
            &[],
            None,
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn publish_stream(
        bundle: &mut ProjectBundle,
        inputs: &ObjectRef,
        stream: crate::operation_protocol::StreamState,
    ) -> Result<BundleSaveReport, BundleError> {
        let branch = bundle.manual_branch_id.clone();
        Self::publish_branch_impl(
            bundle,
            None,
            inputs,
            &branch,
            None,
            true,
            &[],
            Some(stream),
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn publish_maintenance(
        bundle: &mut ProjectBundle,
    ) -> Result<BundleSaveReport, BundleError> {
        let branch = bundle.manual_branch_id.clone();
        let inputs = bundle.checkpoint(&branch)?.input_manifest_ref.clone();
        Self::publish_branch_impl(
            bundle,
            None,
            &inputs,
            &branch,
            None,
            true,
            &[],
            None,
            None,
            None,
        )?
        .published()
    }
    pub(crate) fn prepare_maintenance(
        bundle: &mut ProjectBundle,
        fence: &crate::store::PublicationFence,
    ) -> Result<PreparedMaintenancePublication, BundleError> {
        let branch = bundle.manual_branch_id.clone();
        let inputs = bundle.checkpoint(&branch)?.input_manifest_ref.clone();
        match Self::publish_branch_impl(
            bundle,
            None,
            &inputs,
            &branch,
            None,
            true,
            &[],
            None,
            None,
            Some(fence),
        )? {
            BranchPublication::Prepared(prepared) => Ok(*prepared),
            _ => Err(BundleError::HistoryBarrierInvariant),
        }
    }
    pub(crate) fn stage_task_checkpoint(
        bundle: &mut ProjectBundle,
        capture: &crate::operation_actor::CapturedHistory,
        inputs: &ObjectRef,
        branch: &crate::bundle_records::BranchId,
        base: &ObjectRef,
    ) -> Result<(), BundleError> {
        let BranchPublication::Staged(staged) = Self::publish_branch_impl(
            bundle,
            Some(PublicationView::Capture(capture)),
            inputs,
            branch,
            None,
            false,
            &[],
            None,
            Some(base),
            None,
        )?
        else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        bundle.task_checkpoints.insert(branch.clone(), *staged);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("task_checkpoint_staged");
        Ok(())
    }
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn publish_branch_impl(
        bundle: &mut ProjectBundle,
        view: Option<PublicationView<'_>>,
        inputs: &ObjectRef,
        branch_id: &crate::bundle_records::BranchId,
        selection: Option<&crate::bundle_switch::PreparedProjectSelection>,
        metadata_only: bool,
        manual_batch: &[CommandEnvelope],
        stream: Option<crate::operation_protocol::StreamState>,
        stage_base: Option<&ObjectRef>,
        preparation: Option<&crate::store::PublicationFence>,
    ) -> Result<BranchPublication, BundleError> {
        #[cfg(any(test, feature = "test-util"))]
        if matches!(view, Some(PublicationView::Capture(_))) {
            crate::test_hook("history_selected");
        }
        let copy_branch = bundle.prepare_plain_copy(branch_id)?;
        let branch_id = copy_branch.as_ref().unwrap_or(branch_id);
        let manual_branch_id = selection.map_or_else(
            || bundle.manual_branch_id.clone(),
            |s| s.target_branch.clone(),
        );
        let old_switch = bundle
            .root
            .as_ref()
            .map_or(0, |root| root.manual_switch_generation);
        let switch_generation = if selection.is_some() {
            old_switch
                .checked_add(1)
                .ok_or(BundleError::Capacity("manual switch generation"))?
        } else {
            old_switch
        };
        bundle.publication_state = crate::bundle::PublicationState::NeverAttempted;
        bundle.publication_tag = None;
        if bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        if stage_base.is_none() && !metadata_only {
            bundle.refuse_unpaired_task_checkpoint(branch_id)?;
        }
        bundle.store.with_namespace(|n| n.verify())?;
        if let Some(fence) = preparation {
            bundle.store.check_publication_fence(fence)?;
        } else if bundle.store.with_namespace(|n| n.read_root())?.as_deref()
            != bundle
                .admission
                .source()
                .map(|_| bundle.admission.raw_bytes())
        {
            return Err(BundleError::Identity(
                "root changed since bundle admission".into(),
            ));
        }
        if preparation.is_none()
            && !bundle
                .store
                .with_namespace(|n| Ok(n.object_batch_active()))?
            && let Err(error) = bundle.collect_after_ack()
        {
            bundle.store.with_namespace(|n| n.verify())?;
            if matches!(
                error,
                BundleError::Unsettled | BundleError::DurabilityUnknown
            ) {
                return Err(error);
            }
            crate::namespace::note("gc_retry_pending", &bundle.store.resolved_root()?);
        }
        let prior = bundle.branches.get(branch_id).cloned();
        let generation = bundle
            .generation
            .checked_add(1)
            .ok_or(BundleError::Capacity("root generation"))?;
        bundle.store.bind_history_pool(&bundle.history_pool)?;
        barrier(bundle, CommitBarrier::B0)?;
        bundle.store.begin_publication()?;
        let mut seals: Vec<SealedObject> = vec![];
        macro_rules! seal {
            ($value:expr) => {{
                let sealed = bundle.store.admit_record($value)?;
                let reference = sealed.reference.clone();
                seals.push(sealed);
                reference
            }};
        }
        let (checkpoint, checkpoint_ref) = if metadata_only {
            let mut checkpoint = prior
                .clone()
                .ok_or_else(|| BundleError::Corrupt("metadata checkpoint missing".into()))?;
            if stream
                .as_ref()
                .is_some_and(|next| checkpoint.stream.as_ref() != Some(next))
                || checkpoint.input_manifest_ref != *inputs
            {
                // Binding or renewing an empty stream accepts no edit. Keep the
                // person's submitted-note head while publishing the new stream.
                if checkpoint.input_manifest_ref != *inputs
                    || stream.as_ref().is_some_and(|next| next.sequence() != 0)
                {
                    checkpoint.head_id = HeadId::fresh()?;
                    checkpoint.predecessor = prior.as_ref().map(|c| c.head_id.clone());
                }
                checkpoint.generation = generation;
                checkpoint.input_manifest_ref = inputs.clone();
                if let Some(stream) = stream {
                    checkpoint.command_cut = stream.receipts().last().map(|r| r.id.clone());
                    checkpoint.stream = Some(stream);
                }
                let reference = seal!(&checkpoint);
                (checkpoint, reference)
            } else {
                (checkpoint, bundle.checkpoint_reference(branch_id)?)
            }
        } else {
            let view = view.ok_or(BundleError::HistoryBarrierInvariant)?;
            let owner = view.seal(bundle, inputs, prior.as_ref())?;
            let continuity = match view {
                PublicationView::Continuity(_, source) => {
                    prior.as_ref().is_some_and(|p| p.identity == source)
                }
                _ => false,
            };
            let head = if continuity
                || prior.as_ref().is_some_and(|p| {
                    p.identity == owner.identity
                        && p.input_manifest_ref == *inputs
                        && p.stream == stream
                }) {
                prior.as_ref().unwrap().head_id.clone()
            } else {
                HeadId::fresh()?
            };
            let mut checkpoint = crate::operation_checkpoint::Checkpoint::from_sealed(
                crate::operation_checkpoint::Header {
                    branch: branch_id.clone(),
                    head,
                    revision: view.revision(),
                    inputs: inputs.clone(),
                    generation,
                    predecessor: prior.as_ref().map(|p| p.head_id.clone()),
                    command_cut: stream
                        .as_ref()
                        .and_then(|state| state.receipts().last())
                        .map(|receipt| receipt.id.clone())
                        .or_else(|| manual_batch.last().map(|e| e.id.clone()))
                        .or_else(|| bundle.pending_command.as_ref().map(|e| e.id.clone()))
                        .or_else(|| prior.as_ref().and_then(|p| p.command_cut.clone())),
                    take_map: None,
                    stream,
                },
                &owner,
            )?;
            checkpoint.install = if let PublicationView::Reinstall(_, install) = view {
                Some(install.clone())
            } else {
                prior
                    .as_ref()
                    .filter(|p| p.position.epoch == checkpoint.position.epoch)
                    .and_then(|p| p.install.clone())
            };
            let reference = seal!(&checkpoint);
            seals.push(owner.into_object());
            (checkpoint, reference)
        };
        let head_id = checkpoint.head_id.clone();
        if let Some(base_ref) = stage_base {
            bundle
                .store
                .with_namespace(crate::namespace::ProjectNamespace::finish_object_batch)?;
            let pin = bundle
                .store
                .pin(&[checkpoint_ref.clone(), base_ref.clone()])?;
            return Ok(BranchPublication::Staged(Box::new(
                crate::task_checkpoint::StagedTaskCheckpoint {
                    before: bundle.checkpoint_reference(branch_id)?,
                    reference: checkpoint_ref,
                    checkpoint,
                    base_ref: base_ref.clone(),
                    _pin: pin,
                    _seals: seals,
                },
            )));
        }
        let mut candidate_recovery = bundle
            .root
            .as_ref()
            .map_or_else(BTreeMap::new, |root| root.candidate_recovery.clone());
        if let Some((candidate, base)) = bundle
            .pending_publication
            .as_ref()
            .and_then(|pending| pending.candidate_base.as_ref())
            .filter(|(candidate, _)| candidate == branch_id)
        {
            candidate_recovery.insert(
                candidate.clone(),
                crate::candidate_recovery::CandidateRecovery {
                    checkpoint: checkpoint_ref.clone(),
                    base_ref: base.clone(),
                },
            );
        }
        // The base is immutable, but its binding follows every committed cut of
        // this registered branch, including a merge prepared under a lost reply.
        if let Some(binding) = candidate_recovery.get_mut(branch_id) {
            binding.checkpoint = checkpoint_ref.clone();
        }
        let mut branches = bundle.branches.clone();
        branches.insert(branch_id.clone(), checkpoint);
        let mut branch_index: BTreeMap<_, _> = if let Some(root) = bundle.root.as_ref() {
            (*bundle
                .store
                .read_record::<BTreeMap<crate::bundle_records::BranchId, ObjectRef>>(
                    &root.branch_index_ref,
                )?)
            .clone()
        } else {
            BTreeMap::new()
        };
        branch_index.retain(|id, _| branches.contains_key(id));
        branch_index.insert(branch_id.clone(), checkpoint_ref.clone());
        let evidence = bundle.evidence.clone();
        if let Some(pending) = &bundle.pending_metadata {
            for branch in &pending.task_checkpoints {
                let staged = bundle
                    .task_checkpoints
                    .get(branch)
                    .ok_or(BundleError::HistoryChanged)?;
                if branch_index.get(branch) != Some(&staged.before) {
                    return Err(BundleError::HistoryChanged);
                }
                branches.insert(branch.clone(), staged.checkpoint.clone());
                branch_index.insert(branch.clone(), staged.reference.clone());
                candidate_recovery.insert(
                    branch.clone(),
                    crate::candidate_recovery::CandidateRecovery {
                        checkpoint: staged.reference.clone(),
                        base_ref: staged.base_ref.clone(),
                    },
                );
            }
        }
        candidate_recovery.retain(|branch, _| branches.contains_key(branch));
        let branch_index_ref = seal!(&branch_index);
        barrier(bundle, CommitBarrier::B1)?;
        let mut staged_inventory = None;
        let mut recovery = bundle.recovery.clone();
        if !bundle.admission.raw_bytes().is_empty() {
            let deps = bundle
                .admission
                .data
                .file
                .as_ref()
                .and_then(|f| f.studio.as_ref())
                .map_or_else(Vec::new, super::bundle::StudioRoot::references);
            let previous = bundle
                .store
                .admit_root_checkpoint(bundle.admission.raw_bytes(), &deps)?;
            let reference = previous.reference.clone();
            seals.push(previous);
            let mut staged = recovery.clone();
            staged.staging = Some(reference.clone());
            staged_inventory = Some(staged);
            recovery.predecessors.insert(0, reference);
            recovery.predecessors.truncate(2);
        }
        recovery.generation = generation;
        recovery.staging = None;
        let mut receipts = bundle.receipts.clone();
        let mut issuers = bundle.issuers.clone();
        if let Some(stream) = &branches[branch_id].stream {
            let previous = prior
                .as_ref()
                .and_then(|checkpoint| checkpoint.stream.as_ref())
                .filter(|before| before.seed() == stream.seed())
                .map_or(0, crate::operation_protocol::StreamState::sequence);
            for receipt in stream
                .receipts()
                .filter(|receipt| receipt.sequence > previous)
            {
                let issuer = issuers
                    .get_mut(&receipt.id.issuer)
                    .ok_or(BundleError::HistoryChanged)?;
                if issuer.next_sequence != receipt.id.sequence {
                    return Err(BundleError::HistoryChanged);
                }
                issuer.next_sequence = issuer
                    .next_sequence
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("issuer sequence"))?;
            }
        }
        for envelope in manual_batch
            .iter()
            .chain(
                bundle
                    .pending_command
                    .as_deref()
                    .filter(|_| manual_batch.is_empty()),
            )
            .chain(selection.and_then(|s| s.envelope.as_ref()))
        {
            let result = if let Some(request) = &envelope.switch {
                let selected = selection.ok_or(BundleError::HistoryChanged)?;
                let state = issuers
                    .get_mut(&envelope.id.issuer)
                    .filter(|s| !s.retired)
                    .ok_or(BundleError::HistoryChanged)?;
                if state.next_sequence != envelope.id.sequence {
                    return Err(BundleError::HistoryChanged);
                }
                state.next_sequence = state
                    .next_sequence
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("issuer sequence"))?;
                ReceiptResult::Switch {
                    result: Box::new(crate::bundle_switch::SwitchResult {
                        old: crate::bundle_switch::SwitchBranch::from_checkpoint(
                            bundle.checkpoint_reference(&selected.source_branch)?,
                            bundle.checkpoint(&selected.source_branch)?,
                        ),
                        new: crate::bundle_switch::SwitchBranch::from_checkpoint(
                            checkpoint_ref.clone(),
                            &branches[branch_id],
                        ),
                        target_version: request.target.version(),
                        switch_generation,
                        root_generation: generation,
                    }),
                }
            } else {
                bundle
                    .pending_result
                    .clone()
                    .unwrap_or_else(|| ReceiptResult::Edit {
                        checkpoint_ref: checkpoint_ref.clone(),
                        batch_id: match view {
                            Some(
                                PublicationView::Prepared(update)
                                | PublicationView::Reinstall(update, _),
                            ) => update.accepted_batch().cloned(),
                            _ => None,
                        },
                    })
            };
            if matches!(result, ReceiptResult::Comment { .. }) {
                receipts.comments = receipts
                    .comments
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("comment count"))?;
            }
            receipts.detailed.push(CommandReceipt {
                recovery_origin: None,
                id: envelope.id.clone(),
                digest: envelope.digest()?,
                project_id: bundle.project_id.clone(),
                target_branch: envelope.target_branch.clone(),
                original_epoch: envelope.owner_epoch,
                expected_head: envelope.expected_head.clone(),
                head_id: head_id.clone(),
                generation,
                result,
            });
        }
        receipts.validate(bundle.store.limits())?;
        let receipt_index_ref = if receipts.detailed.is_empty()
            && receipts.retired.is_empty()
            && receipts.comments == 0
        {
            None
        } else {
            Some(seal!(&receipts))
        };
        let issuer_state_ref = if issuers.is_empty() && receipt_index_ref.is_none() {
            None
        } else {
            Some(seal!(&issuers))
        };
        let mut roots = vec![
            branch_index_ref.clone(),
            branches[&manual_branch_id].input_manifest_ref.clone(),
        ];
        let mut metadata_root = bundle.root.clone();
        if let Some(pending) = &bundle.pending_metadata {
            pending.apply(metadata_root.as_mut().ok_or(BundleError::Unsettled)?);
        }
        if let Some(root) = &metadata_root {
            roots.extend(root.version_index_ref.clone());
            roots.extend(root.approved_rulebook_ref.clone());
            roots.extend(root.pending_rulebook_ref.clone());
            roots.extend(root.active_review_ref.clone());
            roots.extend(root.feedback_index_ref.clone());
            roots.extend(root.run_index_ref.clone());
            roots.extend(root.claim_index_ref.clone());
        }
        roots.extend(
            candidate_recovery
                .values()
                .flat_map(|binding| [binding.checkpoint.clone(), binding.base_ref.clone()]),
        );
        roots.extend(receipt_index_ref.clone());
        roots.extend(issuer_state_ref.clone());
        let mut inventory = SupportInventory {
            required_fields: BTreeSet::new(),
            required_kinds: BTreeSet::new(),
            required_apis: crate::bundle_recovery::base_apis(),
        };
        let mut retained = roots.clone();
        retained.extend(evidence.clone());
        retained.extend(recovery.references());
        inventory.required_fields = bundle.store.support_fields(&retained)?;
        let closure = bundle.store.closure(&retained)?;
        let roles = bundle.store.evidence_roles(&closure)?;
        let requires_switch =
            switch_generation != 0 || bundle.store.has_archived_manual_switch(&closure)?;
        inventory.required_apis =
            crate::rescue_record::required_apis(u64::from(requires_switch), !roles.is_empty());
        if bundle.store.has_host_switch_receipts(&closure)? {
            inventory.required_apis.insert("host-switch".into(), 1);
        }
        crate::studio_store::add_required_apis(
            &mut inventory.required_apis,
            closure.iter().map(|r| r.record_kind),
        );
        let store_manifest = bundle.store.selected_manifest(
            &roots,
            &evidence,
            recovery.clone(),
            &inventory,
            generation,
        )?;
        let store_manifest_ref = seal!(&store_manifest);
        let mut retained_task_reviews = bundle
            .root
            .as_ref()
            .map(|root| root.retained_task_reviews.clone())
            .unwrap_or_default();
        retained_task_reviews.retain(|fork, _| branches.contains_key(fork));
        if let Some(selected) = selection
            && selected.protects_branch(&selected.restore.checkpoint.branch_id)
            && !selected.reuses_checkpoint()
        {
            let source = &selected.restore.checkpoint;
            retained_task_reviews.insert(
                selected.target_branch.clone(),
                crate::orchestration_records::BranchBinding {
                    branch: source.branch_id.clone(),
                    head: source.head_id.clone(),
                    document_hash: source.document_ref.sha256.clone(),
                    checkpoint: bundle.checkpoint_reference(&source.branch_id)?,
                },
            );
        }
        let root = StudioRoot {
            project_id: bundle.project_id.clone(),
            origin_project_ids: bundle.origin_project_ids.clone(),
            origin_session_ids: bundle.origin_session_ids.clone(),
            generation,
            creation_brief_ref: None,
            manual_branch_id: manual_branch_id.clone(),
            manual_switch_generation: switch_generation,
            retained_task_reviews,
            candidate_branch_ids: branches
                .keys()
                .filter(|id| **id != manual_branch_id)
                .cloned()
                .collect(),
            branch_index_ref,
            candidate_recovery,
            version_index_ref: metadata_root
                .as_ref()
                .and_then(|r| r.version_index_ref.clone()),
            approved_rulebook_ref: metadata_root
                .as_ref()
                .and_then(|r| r.approved_rulebook_ref.clone()),
            pending_rulebook_ref: metadata_root
                .as_ref()
                .and_then(|r| r.pending_rulebook_ref.clone()),
            active_review_ref: metadata_root
                .as_ref()
                .and_then(|r| r.active_review_ref.clone()),
            stopped_review: metadata_root
                .as_ref()
                .and_then(|r| r.stopped_review.clone()),
            feedback_index_ref: metadata_root
                .as_ref()
                .and_then(|r| r.feedback_index_ref.clone()),
            run_index_ref: metadata_root.as_ref().and_then(|r| r.run_index_ref.clone()),
            claim_index_ref: metadata_root
                .as_ref()
                .and_then(|r| r.claim_index_ref.clone()),
            input_manifest_ref: branches[&manual_branch_id].input_manifest_ref.clone(),
            receipt_index_ref,
            issuer_state_ref,
            store_manifest_ref,
        };
        root.admit()?;
        bundle.validate_plain_copy(&root, &branch_index)?;
        bundle
            .store
            .with_namespace(crate::namespace::ProjectNamespace::finish_object_batch)?;
        let publication = PreparedBranchPublication {
            branch_id: branch_id.clone(),
            manual_branch_id,
            generation,
            head_id,
            root: Some(root),
            branches,
            evidence,
            receipts,
            issuers,
            inventory,
            recovery,
            seals,
            staged_inventory,
        };
        if preparation.is_some() {
            publication
                .prepare(bundle)
                .map(|ready| BranchPublication::Prepared(Box::new(ready)))
        } else {
            publication.publish(bundle)
        }
    }
}
/// Owns both the exact future root and every resource its publication consumes.
pub(crate) struct PreparedMaintenancePublication {
    before: String,
    root_write: crate::store::PreparedControlWrite,
    staging: Option<crate::store::PreparedInventoryWrite>,
    resources: Option<PreparedMaintenanceResources>,
}
struct PreparedMaintenanceResources {
    body: PreparedBranchPublication,
    admission: BundleWriteAdmission,
    _encoded_root: crate::storage_budget::StorageReservation,
    pin: crate::store::StorePin,
}
impl PreparedMaintenanceResources {
    fn swap_metadata(&mut self, bundle: &mut ProjectBundle) {
        std::mem::swap(&mut self.body.generation, &mut bundle.generation);
        std::mem::swap(&mut self.body.root, &mut bundle.root);
        std::mem::swap(&mut self.admission, &mut bundle.admission);
        std::mem::swap(&mut self.body.branches, &mut bundle.branches);
        std::mem::swap(&mut self.body.evidence, &mut bundle.evidence);
        std::mem::swap(&mut self.body.receipts, &mut bundle.receipts);
        std::mem::swap(&mut self.body.issuers, &mut bundle.issuers);
        std::mem::swap(&mut self.body.inventory, &mut bundle.inventory);
        std::mem::swap(&mut self.body.recovery, &mut bundle.recovery);
    }
}
impl PreparedMaintenancePublication {
    fn check_current(&self, bundle: &ProjectBundle) -> Result<(), BundleError> {
        if self.resources.is_none() {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        if kinewright_media::sha256_bytes(bundle.admission.raw_bytes()) != self.before {
            return Err(BundleError::HistoryChanged);
        }
        bundle
            .store
            .check_prepared_predecessor(&self.root_write, &self.before)
    }
    /// Lend cut1's metadata to the same publisher while the physical root stays put.
    /// The original durable pin and the prepared graph pins remain held throughout.
    pub(crate) fn with_preview<T>(
        &mut self,
        bundle: &mut ProjectBundle,
        prepare: impl FnOnce(&mut ProjectBundle) -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        struct Preview<'a> {
            resources: &'a mut PreparedMaintenanceResources,
            bundle: &'a mut ProjectBundle,
        }
        impl Drop for Preview<'_> {
            fn drop(&mut self) {
                self.resources.swap_metadata(self.bundle);
            }
        }
        self.check_current(bundle)?;
        let resources = self
            .resources
            .as_mut()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        if resources.body.manual_branch_id != bundle.manual_branch_id {
            return Err(BundleError::HistoryChanged);
        }
        resources.swap_metadata(bundle);
        let preview = Preview { resources, bundle };
        prepare(preview.bundle)
    }
    pub(crate) fn publish(
        &mut self,
        bundle: &mut ProjectBundle,
    ) -> Result<BundleSaveReport, BundleError> {
        self.check_current(bundle)?;
        let resources = self
            .resources
            .as_ref()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        barrier(bundle, CommitBarrier::B2)?;
        if let Some(staging) = &mut self.staging {
            bundle.store.publish_prepared_inventory(staging)?;
        }
        barrier(bundle, CommitBarrier::B3)?;
        let published =
            publish_root_from_admission(bundle, &resources.admission, Some(&mut self.root_write))?;
        let PreparedMaintenanceResources {
            body,
            admission,
            pin,
            _encoded_root,
        } = self
            .resources
            .take()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        body.adopt(bundle, admission, published, Some(pin))?
            .published()
    }
}

struct PreparedBranchPublication {
    branch_id: crate::bundle_records::BranchId,
    manual_branch_id: crate::bundle_records::BranchId,
    generation: u64,
    head_id: HeadId,
    root: Option<StudioRoot>,
    branches: BTreeMap<crate::bundle_records::BranchId, BranchCheckpoint>,
    evidence: Vec<ObjectRef>,
    receipts: crate::receipts::ReceiptLedger,
    issuers: BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>,
    inventory: SupportInventory,
    recovery: crate::store::RecoveryInventory,
    seals: Vec<SealedObject>,
    staged_inventory: Option<crate::store::RecoveryInventory>,
}
impl PreparedBranchPublication {
    fn prepare(
        self,
        bundle: &ProjectBundle,
    ) -> Result<PreparedMaintenancePublication, BundleError> {
        let (admission, encoded_root) = self.encode_root(bundle)?;
        let root_write = bundle.store.prepare_root_write(admission.raw_bytes())?;
        let staging = self
            .staged_inventory
            .as_ref()
            .map(|inventory| bundle.store.prepare_inventory(inventory))
            .transpose()?;
        let pin = bundle.store.pin(
            &self
                .root
                .as_ref()
                .ok_or(BundleError::HistoryBarrierInvariant)?
                .references(),
        )?;
        Ok(PreparedMaintenancePublication {
            before: kinewright_media::sha256_bytes(bundle.admission.raw_bytes()),
            root_write,
            staging,
            resources: Some(PreparedMaintenanceResources {
                body: self,
                admission,
                _encoded_root: encoded_root,
                pin,
            }),
        })
    }
    fn encode_root(
        &self,
        bundle: &ProjectBundle,
    ) -> Result<
        (
            BundleWriteAdmission,
            crate::storage_budget::StorageReservation,
        ),
        BundleError,
    > {
        encode_publication_root(
            bundle,
            &self.branch_id,
            &self.manual_branch_id,
            &self.branches,
            self.root
                .as_ref()
                .ok_or(BundleError::HistoryBarrierInvariant)?,
        )
    }
    fn publish(mut self, bundle: &mut ProjectBundle) -> Result<BranchPublication, BundleError> {
        barrier(bundle, CommitBarrier::B2)?;
        if let Some(staged) = self.staged_inventory.take() {
            bundle.store.publish_inventory(&staged)?;
        }
        barrier(bundle, CommitBarrier::B3)?;
        let (next_admission, _encoded_root) = self.encode_root(bundle)?;
        let published = match publish_root_from_admission(bundle, &next_admission, None) {
            Ok(published) => published,
            Err(error) => {
                if bundle.publication_state == crate::bundle::PublicationState::Unknown
                    && let Some(pending) = &mut bundle.pending_publication
                {
                    // Either root may still be authoritative. Its incoming
                    // graph stays pinned until the original attempt settles.
                    pending.seals = self.seals;
                }
                return Err(error);
            }
        };
        self.adopt(bundle, next_admission, published, None)
    }
    fn adopt(
        self,
        bundle: &mut ProjectBundle,
        next_admission: BundleWriteAdmission,
        published: PublishedRoot,
        prepared_pin: Option<crate::store::StorePin>,
    ) -> Result<BranchPublication, BundleError> {
        let Self {
            branch_id,
            manual_branch_id,
            generation,
            head_id,
            root,
            branches,
            evidence,
            receipts,
            issuers,
            inventory,
            recovery,
            seals,
            staged_inventory: _,
        } = self;
        let branch_id = &branch_id;
        bundle.publication_state = crate::bundle::PublicationState::Committed;
        // Publication is monotonic: any fallible tail is unsettled, never a definite rollback.
        bundle.unsettled = true;
        let tail = (|| -> Result<(), BundleError> {
            bundle.evidence = evidence;
            bundle.receipts = receipts;
            bundle.issuers = issuers;
            bundle.manual_branch_id = manual_branch_id.clone();
            bundle.store.set_review_budget(
                root.as_ref()
                    .is_some_and(|root| root.active_review_ref.is_some()),
            );
            bundle.root = root;
            bundle.branches = branches;
            bundle.copy_publication = None;
            if let Some(pending) = &bundle.pending_metadata {
                for branch in &pending.task_checkpoints {
                    bundle.task_checkpoints.remove(branch);
                }
            }
            bundle.generation = generation;
            bundle.inventory = inventory;
            bundle.recovery = recovery;
            bundle.admission = next_admission;
            let roots = bundle.root_references()?;
            bundle.durable = Some(match prepared_pin {
                Some(pin) => bundle.store.mark_prepared_committed(roots, pin)?,
                None => bundle.store.mark_committed(&roots)?,
            });
            bundle.refresh_run_budget()?;
            barrier(bundle, CommitBarrier::B5)?;
            bundle.pending_evidence.clear();
            Ok(())
        })();
        if tail.is_err() {
            return Err(BundleError::DurabilityUnknown);
        }
        bundle.unsettled = false;
        drop(seals);
        Ok(BranchPublication::Published(BundleSaveReport {
            position: bundle.checkpoint(branch_id)?.position,
            history_sha256: bundle.checkpoint(branch_id)?.identity.to_string(),
            collection: published.collection,
            path: published.saved_path,
            project_id: bundle.project_id.clone(),
            generation,
            head_id,
            root_sha256: kinewright_media::sha256_bytes(bundle.admission.raw_bytes()),
            byte_length: bundle.admission.raw_bytes().len() as u64,
        }))
    }
}

struct PublishedRoot {
    saved_path: std::path::PathBuf,
    collection: crate::store::CollectionStatus,
}
#[allow(
    clippy::too_many_lines,
    reason = "the original tagged root publication and failure classification stay together"
)]
fn publish_root_from_admission(
    bundle: &mut ProjectBundle,
    next_admission: &BundleWriteAdmission,
    prepared: Option<&mut crate::store::PreparedControlWrite>,
) -> Result<PublishedRoot, BundleError> {
    #[cfg(any(test, feature = "test-util"))]
    crate::io_observer::check_failure("root_before_publish", &bundle.store.resolved_root()?)?;
    let saved_path = bundle.store.resolved_root()?;
    let collection = bundle.store.collection_status()?;
    let identity = bundle.store.with_namespace(|n| Ok(n.identity().clone()))?;
    let tag = crate::history_save::PublicationTag {
        destination: identity.resolved_root,
        parent_device: identity.parent_device,
        parent_inode: identity.parent_inode,
        root_sha256: kinewright_media::sha256_bytes(next_admission.raw_bytes()),
        previous_root_sha256: if bundle.admission.raw_bytes().is_empty() {
            None
        } else {
            Some(kinewright_media::sha256_bytes(bundle.admission.raw_bytes()))
        },
    };
    // Publish the owner tag only after every fallible preparation step succeeds.
    bundle.publication_tag = Some(tag.clone());
    if let Some(pending) = &mut bundle.pending_publication {
        pending.root_sha256 = Some(tag.root_sha256);
    }
    #[cfg(any(test, feature = "test-util"))]
    if bundle
        .pending_metadata
        .as_ref()
        .is_some_and(|p| !p.task_checkpoints.is_empty())
    {
        crate::test_hook("task_checkpoint_before_publish");
    }
    let publication = match prepared {
        Some(prepared) => bundle
            .store
            .publish_prepared_root(prepared, next_admission.raw_bytes()),
        None => bundle.store.publish_root(next_admission.raw_bytes()),
    };
    if let Err(failure) = publication {
        if failure.boundary == crate::namespace::RootReplaceBoundary::Unknown {
            bundle.publication_state = crate::bundle::PublicationState::Unknown;
            bundle.unsettled = true;
            return Err(BundleError::DurabilityUnknown);
        }
        bundle.publication_state =
            if failure.boundary == crate::namespace::RootReplaceBoundary::DefinitelyRefused {
                crate::bundle::PublicationState::DefinitelyRefused
            } else {
                crate::bundle::PublicationState::NeverAttempted
            };
        // Cleanup may still be uncertain, but it cannot have published this root.
        bundle.publication_tag = None;
        if let Some(pending) = &mut bundle.pending_publication {
            pending.root_sha256 = None;
        }
        return Err(failure.error);
    }
    #[cfg(any(test, feature = "test-util"))]
    if bundle
        .pending_metadata
        .as_ref()
        .is_some_and(|p| !p.task_checkpoints.is_empty())
    {
        crate::test_hook("task_checkpoint_after_publish");
    }
    bundle.publication_state = crate::bundle::PublicationState::Committed;
    Ok(PublishedRoot {
        saved_path,
        collection,
    })
}

/// The production root encoding/admission used by immediate and prepared cuts.
fn encode_publication_root(
    bundle: &ProjectBundle,
    _branch_id: &crate::bundle_records::BranchId,
    manual_branch_id: &crate::bundle_records::BranchId,
    branches: &BTreeMap<crate::bundle_records::BranchId, BranchCheckpoint>,
    root: &StudioRoot,
) -> Result<
    (
        BundleWriteAdmission,
        crate::storage_budget::StorageReservation,
    ),
    BundleError,
> {
    let document = bundle
        .store
        .read_record::<kinewright_core::Document>(&branches[manual_branch_id].document_ref)?;
    let (bytes, encoded_root) = serialize_root(&bundle.store, &document, root)?;
    let next = BundleWriteAdmission::from_bytes_with_limits(
        Some(bundle.store.resolved_root()?),
        bytes,
        bundle.store.usage_arc(),
        bundle.store.limits(),
    )?;
    next.require_supported()?;
    bundle.store.closure(&root.references())?;
    Ok((next, encoded_root))
}

// Ordinary Save keeps its document-bearing decoded admission budget. Count
// one boundary at a time; the persisted/transport form stays compact and this
// check never builds a second complete document-bearing JSON history.

pub(crate) fn lut_entry_id_matches(id: &str, asset: &kinewright_core::LutAsset) -> bool {
    id == format!("lut:{}", asset.id.0)
        || id == format!("lut:{}:{}", asset.id.0, asset.sha256)
        || id == format!("lut:{}:{}:{}", asset.id.0, asset.sha256, asset.byte_len)
}
pub(crate) fn retain_document_inputs(
    document: &kinewright_core::Document,
    manifest: &mut OwnedInputManifest,
) {
    manifest.entries.retain(|entry| {
        document.media_pool.iter().any(|asset| {
            asset.source_fingerprint.content_sha256.is_some()
                && asset.source_fingerprint.byte_len.is_some()
                && entry.matches_media_asset(asset)
        }) || document.lut_assets.iter().any(|asset| {
            lut_entry_id_matches(&entry.entry_id, asset)
                && entry.object.sha256 == asset.sha256
                && entry.object.byte_length == asset.byte_len
        }) || (entry.entry_id == "font:Inter"
            && document
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .any(|clip| matches!(clip.content, kinewright_core::ClipContent::Title(_))))
    });
    manifest
        .entries
        .sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
    manifest.entries.dedup();
}
pub(crate) fn validate_document_inputs(
    document: &kinewright_core::Document,
    manifest: &OwnedInputManifest,
) -> Result<(), BundleError> {
    for asset in &document.media_pool {
        if asset.source_fingerprint.content_sha256.is_none()
            && asset.source_fingerprint.byte_len.is_none()
        {
            continue;
        }
        if !manifest
            .entries
            .iter()
            .any(|entry| entry.matches_media_asset(asset))
        {
            return Err(BundleError::Unsupported(format!(
                "retained asset media:{} lacks exact fingerprint/interpretation/table",
                asset.id.0
            )));
        }
    }
    if document
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .any(|c| matches!(c.content, kinewright_core::ClipContent::Title(_)))
    {
        let digest = kinewright_media::sha256_bytes(kinewright_core::title_font_bytes());
        if !manifest.entries.iter().any(|e| {
            e.entry_id == "font:Inter"
                && e.object.record_kind == RecordKind::OwnedFont
                && e.object.sha256 == digest
                && e.object.byte_length == kinewright_core::title_font_bytes().len() as u64
        }) {
            return Err(BundleError::Unsupported(
                "retained title requires its owned embedded font".into(),
            ));
        }
    }
    for asset in &document.lut_assets {
        if !manifest.entries.iter().any(|e| {
            lut_entry_id_matches(&e.entry_id, asset)
                && e.object.record_kind == RecordKind::OwnedLut
                && e.object.sha256 == asset.sha256
                && e.object.byte_length == asset.byte_len
        }) {
            return Err(BundleError::Unsupported(
                "retained LUT lacks owned bytes".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn serialize_root(
    store: &crate::store::ProjectStore,
    document: &kinewright_core::Document,
    root: &StudioRoot,
) -> Result<(Vec<u8>, crate::storage_budget::StorageReservation), BundleError> {
    #[derive(serde::Serialize)]
    struct BorrowedRoot<'a> {
        format_version: u32,
        studio: &'a StudioRoot,
        #[serde(flatten)]
        document: &'a kinewright_core::Document,
    }
    let file = BorrowedRoot {
        format_version: crate::project::required_format_version(document, Some(root)),
        studio: root,
        document,
    };
    let mut counter = crate::store::BoundedCounter {
        bytes: 0,
        limit: store.limits().control_bytes,
    };
    serde_json::to_writer_pretty(&mut counter, &file)
        .map_err(|_| BundleError::Capacity("root encoded bytes"))?;
    let encoded = store.reserve(0, counter.bytes, false)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            usize::try_from(counter.bytes)
                .map_err(|_| BundleError::Capacity("root address space"))?,
        )
        .map_err(|_| BundleError::Capacity("root allocation"))?;
    serde_json::to_writer_pretty(&mut bytes, &file)
        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    if bytes.len() as u64 != counter.bytes {
        return Err(BundleError::Corrupt("unstable root serialization".into()));
    }
    Ok((bytes, encoded))
}

pub(crate) fn validate_manifest_extension(
    store: &crate::store::ProjectStore,
    previous: &ObjectRef,
    next: &ObjectRef,
) -> Result<(), BundleError> {
    let before = store.read_input_manifest(previous)?;
    let after = store.read_input_manifest(next)?;
    if before.project_id != after.project_id
        || before.session_id != after.session_id
        || before.owner_epoch != after.owner_epoch
        || before
            .entries
            .iter()
            .any(|entry| !after.entries.contains(entry))
    {
        return Err(BundleError::Identity(
            "manual input manifest is not an exact superset".into(),
        ));
    }
    Ok(())
}

// G1-W: durable bundle activation and receipt publication are unsupported on Windows.
#[cfg(all(test, target_os = "linux"))]
mod fu1_tests {
    use super::*;
    use kinewright_core::JournalCommand;
    use kinewright_core::{
        Command, Document, Event, Operation, Query, QueryResult, Track, TrackId, TrackKind,
    };

    #[test]
    fn root_admission_refuses_an_unopenable_owner_graph_before_staging() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "native-root-admission",
        );
        let path = temp.path("project.kinewright");
        let core = Core::for_test(Document::default()).unwrap();
        let mut bundle = ProjectBundle::activate(
            &path,
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = bundle.empty_inputs().unwrap();
        bundle.save(&core, &inputs.reference).unwrap();
        let saved = std::fs::read(&path).unwrap();
        let identity = core.snapshot_identity().unwrap();
        drop(bundle.store.prepare_root_write(&saved).unwrap());
        let mut proposed: crate::ProjectFile = serde_json::from_slice(&saved).unwrap();
        proposed.studio.as_mut().unwrap().branch_index_ref.sha256 = "f".repeat(64);
        let bytes = serde_json::to_vec(&proposed).unwrap();
        assert!(
            bundle.store.prepare_root_write(&bytes).is_err(),
            "ROOT_ADMISSION_REFUSES_UNOPENABLE_OWNER_GRAPH"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            saved,
            "REFUSED_ROOT_LEAVES_THE_FILE_UNCHANGED"
        );
        assert_eq!(
            core.snapshot_identity().unwrap(),
            identity,
            "REFUSED_ROOT_LEAVES_THE_CORE_UNCHANGED"
        );
        assert!(
            crate::bundle::LoadedBundle::read(
                &path,
                &crate::operation_history::HistoryPool::new(
                    crate::operation_history::PoolLimits::default()
                ),
            )
            .is_ok(),
            "REFUSED_ROOT_LEAVES_THE_SAVED_STATE_OPENABLE"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one native candidate transaction proves the absent outcome, source preservation and exact retry"
    )]
    fn absent_candidate_publication_preserves_work_and_retries_once() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "absent-native-candidate",
        );
        let core = Core::for_test(Document {
            tracks: vec![Track {
                name: None,
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner = crate::editing_session::Ja1EditingSession::new(
            temp.root(),
            &core,
            crate::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        owner
            .first_save(
                &temp.path("project.kinewright"),
                crate::storage_budget::StorageLimits::default(),
            )
            .unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "candidate",
                crate::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut source = owner
            .fork_registered_from_candidate(&binding, "Editor")
            .unwrap();
        source
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }))
            .unwrap();
        let frozen = owner.freeze_registered_candidate(&mut source).unwrap();
        let source_identity = source.core().snapshot_identity().unwrap();
        let issuer = owner
            .issue_manual_lease("candidate".into(), "private".into())
            .unwrap()
            .issuer;
        let envelope = owner
            .candidate_merge_envelope(
                &frozen,
                crate::receipts::CommandId {
                    issuer,
                    sequence: 1,
                },
            )
            .unwrap();
        let original = candidate.core().snapshot_identity().unwrap();
        let original_change = candidate.core().change_id().unwrap();
        assert!(
            matches!(
                crate::io_observer::fail_once("root_rename_io", || owner
                    .commit_candidate(&frozen, &envelope)),
                Err(BundleError::DurabilityUnknown)
            ),
            "CANDIDATE_ROOT_OUTCOME_IS_UNKNOWN_BEFORE_RECONCILIATION"
        );
        owner.reconcile_active().unwrap();
        assert_eq!(
            candidate.core().snapshot_identity().unwrap(),
            original,
            "ABSENT_CANDIDATE_ROOT_NEVER_COMMITS_ITS_PREPARED_UPDATE"
        );
        assert_eq!(candidate.core().change_id().unwrap(), original_change);
        assert_eq!(
            source.core().snapshot_identity().unwrap(),
            source_identity,
            "ABSENT_PUBLICATION_PRESERVES_THE_SOURCE_WORK"
        );
        assert!(matches!(
            owner.commit_candidate(&frozen, &envelope).unwrap(),
            ReceiptLookup::Committed(_)
        ));
        let committed = candidate.core().change_id().unwrap();
        assert_eq!(
            committed.position.position,
            original_change.position.position + 1,
            "ABSENT_CANDIDATE_RETRY_APPLIES_EXACTLY_ONCE"
        );
        assert_eq!(
            candidate
                .core()
                .export_history()
                .unwrap()
                .history()
                .undo
                .len(),
            1
        );
        owner.commit_candidate(&frozen, &envelope).unwrap();
        assert_eq!(
            candidate.core().change_id().unwrap(),
            committed,
            "COMMITTED_CANDIDATE_RETRY_NEVER_REAPPLIES"
        );
    }

    #[test]
    fn fu1_unknown_branch_refuses_after_receipt_lookup() {
        // Receipt ordering needs a real durable publisher, outside tmpfs.
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let root = workspace.join("target").join("fu1-unit");
        let temp = kinewright_media::test_support::TempDirectory::new_in(&root, "unknown-branch");
        let core = Core::for_test(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut bundle = ProjectBundle::activate(
            &temp.path("project.kinewright"),
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &inputs).unwrap();
        let issuer = bundle.issue_issuer(&core, &inputs, 1).unwrap();
        let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
            core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("snapshot");
        };
        let target = crate::bundle_records::BranchId::fresh().unwrap();
        let mut envelope = CommandEnvelope {
            version: 1,
            action: "edit".into(),
            id: crate::receipts::CommandId {
                issuer,
                sequence: 1,
            },
            project_id: bundle.project_id().clone(),
            target_branch: target.clone(),
            owner_epoch: bundle.store.owner_epoch(),
            expected_head: None,
            expected_revision: revision,
            command: Some(JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            })),
            switch: None,
            context: None,
            lease: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
        };
        assert!(
            matches!(BundleCommit::command_target(&mut bundle, &envelope, &core, &inputs, &[], true, &target), Err(BundleError::Unsupported(message)) if message == "unknown branch")
        );
        envelope.target_branch = bundle.manual_branch_id.clone();
        envelope.expected_head = bundle.manual_head().cloned();
        let receipt = bundle.command(&envelope, &core, &inputs, &[]).unwrap();
        assert!(matches!(receipt, ReceiptLookup::Committed(_)));
        let target = envelope.target_branch.clone();
        bundle.branches.remove(&target).unwrap();
        assert_eq!(
            BundleCommit::command_target(
                &mut bundle,
                &envelope,
                &core,
                &inputs,
                &[],
                true,
                &target
            )
            .unwrap(),
            receipt
        );
    }
}

fn barrier(bundle: &ProjectBundle, barrier: CommitBarrier) -> Result<(), BundleError> {
    let path = bundle.store.resolved_root()?;
    let label = match barrier {
        CommitBarrier::B0 => "bundle_B0",
        CommitBarrier::B1 => "bundle_B1",
        CommitBarrier::B2 => "bundle_B2",
        CommitBarrier::B3 => "bundle_B3",
        CommitBarrier::B4 => "bundle_B4",
        CommitBarrier::B5 => "bundle_B5",
    };
    crate::namespace::note(label, &path);
    Ok(())
}
