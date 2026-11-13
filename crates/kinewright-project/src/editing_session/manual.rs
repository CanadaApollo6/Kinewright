use super::{BundleError, BundleSaveReport, Ja1EditingSession};
use crate::{
    manual_history::{AcceptedManualCommand, ManualLease, history_sha256},
    receipts::ReceiptLookup,
};
use kinewright_core::AcceptedHistory;

impl Ja1EditingSession {
    pub fn active_manual_project(&self) -> Result<crate::bundle_records::ProjectId, BundleError> {
        Ok(self.active_bundle()?.project_id().clone())
    }

    pub fn acknowledge_manual(
        &self,
        permit: &kinewright_core::ManualPermit,
        prefix: Vec<AcceptedHistory>,
    ) -> Result<(), BundleError> {
        self.attachment
            .acknowledge_manual(permit, prefix)
            .map_err(crate::bundle::history_error)
    }
    /// Test a producer whose floor policy differs while using real actor stamping.
    #[cfg(feature = "test-util")]
    pub fn bind_manual_encoder_probe(
        &self,
        encoder: Box<dyn kinewright_core::ManualHistoryEncoder>,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        self.attachment
            .manual_permit(encoder)
            .map_err(crate::bundle::history_error)
    }
    pub fn bind_manual_producer(
        &self,
        lease: ManualLease,
        inputs: crate::owned_inputs::OwnedInputManifest,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        if history_sha256(
            &self
                .core
                .export_history_for_publication()
                .map_err(crate::bundle::history_error)?,
        )? != lease.history_sha256
        {
            return Err(BundleError::HistoryChanged);
        }
        self.attachment
            .manual_permit(lease.into_encoder(inputs)?)
            .map_err(crate::bundle::history_error)
    }
    pub fn manual_input_context(
        &self,
    ) -> Result<crate::owned_inputs::OwnedInputManifest, BundleError> {
        let bundle = self.active_bundle()?;
        let inputs = &bundle
            .checkpoint(bundle.manual_branch_id())?
            .input_manifest_ref;
        Ok((*bundle.store.read_input_manifest(inputs)?).clone())
    }
    pub fn issue_manual_lease(
        &mut self,
        client: String,
        token: String,
    ) -> Result<ManualLease, BundleError> {
        self.require_local_writer()?;
        if self
            .core
            .ordinary_floor_source()
            .map_err(crate::bundle::history_error)?
            .is_some()
        {
            // This endpoint owns its destination. Settle the bridge itself
            // before issuing a successor lease; no person action is needed.
            self.save_active()?;
        }
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let history = core
            .export_history_for_publication()
            .map_err(crate::bundle::history_error)?;
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        bundle.with_person_publication(|bundle| {
            let branch = bundle.manual_branch_id().clone();
            let matches = if let Some(cached) = bundle.verified_cached_history(&branch)? {
                ***cached == history
            } else {
                *bundle.branch_history(&branch)? == history
            };
            if !matches {
                return Err(BundleError::HistoryChanged);
            }
            let issuer = bundle.issue_issuer(&core, &inputs, 1)?;
            let namespace_name = bundle.store.with_namespace(|n| {
                n.identity()
                    .resolved_root
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_owned)
                    .ok_or(BundleError::Unsupported("manual namespace name".into()))
            })?;
            let checkpoint = bundle.checkpoint(&branch)?;
            let parent_identity = bundle
                .store
                .with_namespace(|n| Ok((n.identity().parent_device, n.identity().parent_inode)))?;
            Ok(ManualLease {
                history_limits: crate::manual_history::OrdinaryHistoryLimits::from_limits(
                    bundle.store.limits(),
                )?,
                parent_identity: Some(parent_identity),
                namespace_name: Some(namespace_name),
                client,
                token,
                project: bundle.project_id().clone(),
                branch: branch.clone(),
                switch_generation: bundle
                    .root()
                    .ok_or(BundleError::HistoryChanged)?
                    .manual_switch_generation,
                owner_epoch: bundle.store.owner_epoch(),
                generation: 1,
                issuer,
                anchor_head: checkpoint.head_id.clone(),
                predecessor: checkpoint.batch_ids.last().cloned(),
                history_sha256: checkpoint.history_digest.clone(),
                inputs: checkpoint.input_manifest_ref.clone(),
                lineage: checkpoint.lineage_ref.clone(),
            })
        })
    }
    pub fn commit_manual_history(
        &mut self,
        lease: &ManualLease,
        accepted: &AcceptedHistory,
    ) -> Result<(ReceiptLookup, Option<BundleSaveReport>), BundleError> {
        self.require_local_writer()?;
        let envelope = AcceptedManualCommand::decode(accepted)?.envelope;
        if envelope.manual.as_ref().map(|m| &m.binding) != Some(lease) {
            return Err(BundleError::HistoryChanged);
        }
        let core = self.core.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let result = bundle.with_person_publication(|bundle| {
            crate::bundle_commit::BundleCommit::admitted_history(
                bundle,
                &envelope,
                &core,
                &lease.inputs,
            )
        })?;
        self.inputs = bundle
            .checkpoint(bundle.manual_branch_id())?
            .input_manifest_ref
            .clone();
        if let Some(report) = &result.1 {
            self.association
                .as_mut()
                .ok_or(BundleError::HistoryChanged)?
                .saved = Some(report.history_sha256.clone());
        }
        Ok(result)
    }
}

impl Ja1EditingSession {
    /// Recovery lookup never revives the original lease or issuer.
    pub fn manual_receipt(&self, accepted: &AcceptedHistory) -> Result<ReceiptLookup, BundleError> {
        self.active_bundle()?
            .receipt_lookup(&AcceptedManualCommand::decode(accepted)?.envelope)
    }

    /// The complete bounded frozen chain is the only automatic reauthorization entrypoint.
    pub fn reconcile_manual_chain(
        &mut self,
        client: &str,
        pending: &[AcceptedHistory],
        expected: Option<&kinewright_core::HistorySnapshot>,
    ) -> Result<(), BundleError> {
        if pending.len() > kinewright_core::MANUAL_COMMANDS
            || serde_json::to_vec(pending)
                .map_err(crate::bundle::encoding_error)?
                .len()
                > kinewright_core::MANUAL_BYTES + 128
            || (expected.is_none() && !pending.is_empty())
        {
            return Err(BundleError::Capacity("manual recovery chain"));
        }
        self.require_local_writer()?;
        let mut previous = None;
        let mut absent = Vec::new();
        for accepted in pending {
            let original = AcceptedManualCommand::decode(accepted)?.envelope;
            let metadata = original
                .manual
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?;
            let parent = self
                .active_bundle()?
                .store
                .with_namespace(|n| Ok((n.identity().parent_device, n.identity().parent_inode)))?;
            if metadata.binding.parent_identity != Some(parent)
                || metadata.binding.namespace_name.as_deref()
                    != Some(self.manual_namespace_name()?.as_str())
            {
                return Err(BundleError::HistoryChanged);
            }
            if let Some((binding, sequence, batch, after)) = previous.as_ref()
                && (&metadata.binding != binding
                    || original.id.sequence != sequence + 1
                    || metadata.predecessor.as_ref() != Some(batch)
                    || &metadata.before_sha256 != after)
            {
                return Err(BundleError::HistoryChanged);
            }
            previous = Some((
                metadata.binding.clone(),
                original.id.sequence,
                metadata.batch_id.clone(),
                metadata.after_sha256.clone(),
            ));
            match self.manual_receipt(accepted)? {
                ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_) => {
                    if !absent.is_empty() {
                        return Err(BundleError::HistoryChanged);
                    }
                }
                ReceiptLookup::Absent => absent.push(original),
                _ => return Err(BundleError::Unsettled),
            }
        }
        if let (Some(history), Some((_, _, _, after))) = (expected, previous.as_ref())
            && history_sha256(history)? != *after
        {
            return Err(BundleError::HistoryChanged);
        }
        if !absent.is_empty() {
            self.publish_absent_manual_suffix(
                client,
                &absent,
                expected.ok_or(BundleError::HistoryChanged)?,
            )?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one bounded absent-suffix validation and existing-root publication"
    )]
    fn publish_absent_manual_suffix(
        &mut self,
        client: &str,
        originals: &[crate::receipts::CommandEnvelope],
        expected: &kinewright_core::HistorySnapshot,
    ) -> Result<(), BundleError> {
        use kinewright_core::AttachmentState;
        let bundle = self.active_bundle()?;
        if bundle.unsettled || bundle.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let limits = bundle.store.limits().clone();
        if originals.len() as u64 > limits.pending_commands
            || (bundle.receipts.detailed.len() + bundle.receipts.retired.len() + originals.len())
                as u64
                > limits.receipts
        {
            return Err(BundleError::Capacity("manual recovery receipts"));
        }
        let inputs = originals
            .first()
            .and_then(|original| original.manual.as_ref())
            .ok_or(BundleError::HistoryChanged)?
            .binding
            .inputs
            .clone();
        crate::bundle_commit::validate_manifest_extension(
            &bundle.store,
            &bundle
                .checkpoint(bundle.manual_branch_id())?
                .input_manifest_ref,
            &inputs,
        )?;
        let manifest = bundle.store.read_input_manifest(&inputs)?;
        crate::bundle_commit::validate_inputs(expected, &manifest)?;
        for reference in bundle.store.closure(std::slice::from_ref(&inputs))? {
            bundle.store.verify(&reference)?;
        }
        crate::store::BundleRecord::validate_admission(&*manifest, &bundle.store)?;
        let _inputs_pin = bundle.store.pin(std::slice::from_ref(&inputs))?;
        drop(manifest);
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            == AttachmentState::Frozen
        {
            let cut = self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?;
            self.attachment
                .resume(&cut)
                .map_err(crate::bundle::history_error)?;
        }
        let mut lease = self.issue_manual_lease(
            client.into(),
            format!(
                "{}{}",
                crate::bundle_records::SessionId::fresh()?.as_str(),
                crate::bundle_records::SessionId::fresh()?.as_str()
            ),
        )?;
        lease.inputs = inputs;
        // Commit each accepted cut in order. Each message uses the ordinary
        // publisher's exact-floor transaction and its own original receipt.
        // A later failure cannot roll back an earlier durable acknowledgement.
        for (index, original) in originals.iter().enumerate() {
            let mut renewed = original.clone();
            renewed.owner_epoch = lease.owner_epoch;
            renewed.id = crate::receipts::CommandId {
                issuer: lease.issuer.clone(),
                sequence: u64::try_from(index + 1)
                    .map_err(|_| BundleError::Capacity("manual recovery sequence"))?,
            };
            renewed.lease = Some(lease.token.clone());
            renewed.expected_head = Some(lease.anchor_head.clone());
            renewed.expected_revision = self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?
                .revision();
            let metadata = renewed.manual.as_mut().ok_or(BundleError::HistoryChanged)?;
            metadata.binding = lease.clone();
            metadata.recovery_origin = Some(crate::receipts::OriginalCommand {
                id: original.id.clone(),
                digest: original.digest()?,
            });
            let bundle = self
                .destination
                .as_mut()
                .ok_or(BundleError::Unsettled)?
                .bundle_mut()?;
            let (_, report) = crate::bundle_commit::BundleCommit::recovered_history(
                bundle,
                &renewed,
                &self.core,
                &lease.inputs,
            )?;
            if let Some(report) = report
                && let Some(association) = &mut self.association
            {
                association.saved = Some(report.history_sha256);
            }
        }
        self.inputs.clone_from(&lease.inputs);
        Ok(())
    }

    pub fn published_manual_history(
        &self,
    ) -> Result<kinewright_core::HistorySnapshot, BundleError> {
        let bundle = self.active_bundle()?;
        Ok((*bundle.branch_history(bundle.manual_branch_id())?).clone())
    }
    pub fn manual_namespace_name(&self) -> Result<String, BundleError> {
        self.active_bundle()?.store.with_namespace(|n| {
            n.identity()
                .resolved_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned)
                .ok_or(BundleError::Unsupported("manual namespace name".into()))
        })
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn restrict_manual_limits_probe(
        &mut self,
        limits: crate::storage_budget::StorageLimits,
    ) -> Result<(), BundleError> {
        self.destination
            .as_mut()
            .ok_or(BundleError::Unsettled)?
            .bundle_mut()?
            .restrict_limits_probe(limits)
    }
    pub fn manual_parent_identity(&self) -> Result<(u64, u64), BundleError> {
        self.active_bundle()?
            .store
            .with_namespace(|n| Ok((n.identity().parent_device, n.identity().parent_inode)))
    }
    /// The manual branch's current checkpoint reference, without reading it.
    pub fn manual_checkpoint_ref(&self) -> Result<crate::bundle_records::ObjectRef, BundleError> {
        let bundle = self.active_bundle()?;
        bundle.checkpoint_reference(bundle.manual_branch_id())
    }
    pub fn manual_checkpoint(
        &self,
    ) -> Result<
        (
            crate::bundle_records::ObjectRef,
            crate::bundle_records::BranchCheckpoint,
        ),
        BundleError,
    > {
        let bundle = self.active_bundle()?;
        let id = bundle.manual_branch_id();
        Ok((
            bundle.checkpoint_reference(id)?,
            bundle.checkpoint(id)?.clone(),
        ))
    }

    /// Durable issuance precedes returning an immutable transition envelope.
    pub fn prepare_manual_transition(
        &mut self,
        discard: Option<usize>,
    ) -> Result<crate::receipts::CommandEnvelope, BundleError> {
        self.require_local_writer()?;
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::Unsettled)?
            .bundle_mut()?;
        let cut = self
            .attachment
            .capture()
            .map_err(crate::bundle::history_error)?;
        let frozen = (core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            == kinewright_core::AttachmentState::Frozen)
            .then_some(&cut);
        let issuer = bundle.issue_issuer_at_cut(&core, &inputs, 1, frozen)?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        let cut = checkpoint.command_cut.clone();
        let request = match discard {
            Some(discard_batches) => crate::receipts::HistoryRequest::Floor {
                discard_batches,
                preceding_checkpoint_generation: checkpoint.checkpoint_generation,
                preceding_floor_generation: checkpoint.floor_generation,
                command_cut: cut.ok_or(BundleError::HistoryChanged)?,
            },
            None => crate::receipts::HistoryRequest::RecoveryClose {
                preceding_checkpoint_generation: checkpoint.checkpoint_generation,
                reconciled_cut: cut,
                suffix_digest: checkpoint.history_ref.sha256.clone(),
            },
        };
        let action = match request {
            crate::receipts::HistoryRequest::Floor { .. } => "floor",
            crate::receipts::HistoryRequest::RecoveryClose { .. } => "recovery_close",
        };
        Ok(crate::receipts::CommandEnvelope {
            switch: None,
            manual: None,
            version: 1,
            action: action.into(),
            id: crate::receipts::CommandId {
                issuer,
                sequence: 1,
            },
            project_id: bundle.project_id().clone(),
            target_branch: bundle.manual_branch_id().clone(),
            owner_epoch: bundle.store.owner_epoch(),
            lease: None,
            expected_head: bundle.manual_head().cloned(),
            expected_revision: self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?
                .revision(),
            command: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: Some(request),
        })
    }

    /// Reuses S1 transition validation/publication; the caller installs on each frozen actor.
    pub fn publish_manual_transition(
        &mut self,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<ReceiptLookup, BundleError> {
        self.require_local_writer()?;
        if envelope.transition.is_none() {
            return Err(BundleError::HistoryChanged);
        }
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::Unsettled)?
            .bundle_mut()?;
        crate::bundle_commit::BundleCommit::command_inner(
            bundle,
            envelope,
            &core,
            &inputs,
            &[],
            false,
        )
    }

    pub fn freeze_manual_barrier(&self) -> Result<kinewright_core::AcceptedCut, BundleError> {
        self.attachment
            .freeze()
            .map_err(crate::bundle::history_error)
    }

    pub fn install_manual_barrier(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
        history: kinewright_core::HistorySnapshot,
        pending: Vec<AcceptedHistory>,
    ) -> Result<kinewright_core::AcceptedCut, BundleError> {
        let digest = history_sha256(&history)?;
        let cut = self
            .attachment
            .install_frozen_transition(cut, history, pending)
            .map_err(crate::bundle::history_error)?;
        if self.host_attached {
            self.host_cut = Some(cut.clone());
        }
        if let Some(association) = &mut self.association {
            association.saved = Some(digest);
        }
        Ok(cut)
    }

    pub fn resume_manual_barrier(
        &mut self,
        cut: &kinewright_core::AcceptedCut,
    ) -> Result<(), BundleError> {
        self.attachment
            .resume(cut)
            .map_err(crate::bundle::history_error)?;
        self.host_cut = None;
        Ok(())
    }
}

impl Ja1EditingSession {
    pub fn manual_transition_receipt(
        &self,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<ReceiptLookup, BundleError> {
        self.active_bundle()?.receipt_lookup(envelope)
    }
    pub fn verify_manual_predecessor(
        &self,
        reference: &crate::bundle_records::ObjectRef,
        history: &kinewright_core::HistorySnapshot,
    ) -> Result<(), BundleError> {
        let bundle = self.active_bundle()?;
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(reference)?;
        if checkpoint.branch_id != *bundle.manual_branch_id()
            || *bundle.store.resolve_history(&checkpoint.history_ref)? != *history
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
}

/// Same-process GUI actor control. It cannot execute edits or install arbitrary snapshots.
pub struct ManualRecoveryEndpoint {
    attachment: crate::ProjectAttachment,
    core: kinewright_core::Core,
    cut: kinewright_core::AcceptedCut,
    pending: Vec<AcceptedHistory>,
}
impl ManualRecoveryEndpoint {
    pub fn install(
        &mut self,
        history: kinewright_core::HistorySnapshot,
    ) -> Result<(), BundleError> {
        self.cut = self
            .attachment
            .install_frozen_transition(&self.cut, history, self.pending.clone())
            .map_err(crate::bundle::history_error)?;
        self.pending.clear();
        Ok(())
    }
    pub fn snapshot(&self) -> Result<kinewright_core::HistorySnapshot, BundleError> {
        self.core
            .export_history_for_publication()
            .map_err(crate::bundle::history_error)
    }
    pub fn finish(
        self,
        lease: ManualLease,
        inputs: crate::owned_inputs::OwnedInputManifest,
        live: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        if history_sha256(self.cut.history())? != lease.history_sha256 {
            return Err(BundleError::HistoryChanged);
        }
        crate::bundle_commit::validate_inputs(self.cut.history(), &inputs)?;
        self.attachment
            .resume_manual(&self.cut, lease.into_encoder(inputs)?, live)
            .map_err(crate::bundle::history_error)
    }
}
impl Ja1EditingSession {
    pub fn recapture_host_recovery_cut(
        &mut self,
    ) -> Result<kinewright_core::AcceptedCut, BundleError> {
        if !self.host_attached || self.host_retention_lost {
            return Err(BundleError::HistoryChanged);
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::history_error)?;
        self.host_cut = Some(cut.clone());
        Ok(cut)
    }
    pub fn manual_recovery_endpoint(
        &self,
        cut: &kinewright_core::AcceptedCut,
        pending: Vec<AcceptedHistory>,
    ) -> Result<ManualRecoveryEndpoint, BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.host_cut.as_ref() != Some(cut)
            || self
                .attachment
                .capture()
                .map_err(crate::bundle::history_error)?
                != *cut
            || !self.rescue.attempts.is_empty()
            || !self.retained_destinations.is_empty()
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(ManualRecoveryEndpoint {
            attachment: crate::ProjectAttachment::new(&self.core)
                .map_err(crate::bundle::history_error)?,
            core: self.core.clone(),
            cut: cut.clone(),
            pending,
        })
    }
    pub fn finish_host_manual_recovery(
        &mut self,
        publisher_digest: &str,
    ) -> Result<(), BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.host_cut.is_none()
            || history_sha256(
                &self
                    .core
                    .export_history_for_publication()
                    .map_err(crate::bundle::history_error)?,
            )? != publisher_digest
        {
            return Err(BundleError::HistoryChanged);
        }
        self.host_cut = None;
        self.host_published_digest = Some(publisher_digest.into());
        if let Some(association) = &mut self.association {
            association.saved = Some(publisher_digest.into());
        }
        Ok(())
    }
}

impl Ja1EditingSession {
    /// Resolve the exact receipt checkpoint, allowing only later metadata publications
    /// that preserve every payload reference, command cut and floor generation.
    pub fn manual_barrier_result(
        &self,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<crate::bundle_records::BranchCheckpoint, BundleError> {
        use crate::receipts::ReceiptResult;
        let bundle = self.active_bundle()?;
        let lookup = bundle.receipt_lookup(envelope)?;
        let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = lookup else {
            return Err(BundleError::Unsettled);
        };
        let ReceiptResult::Edit { checkpoint_ref, .. } = receipt.result else {
            return Err(BundleError::HistoryChanged);
        };
        let recorded = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&checkpoint_ref)?;
        let mut current = bundle.checkpoint(bundle.manual_branch_id())?.clone();
        if current.checkpoint_generation < recorded.checkpoint_generation {
            return Err(BundleError::HistoryChanged);
        }
        current.head_id.clone_from(&recorded.head_id);
        current.predecessor.clone_from(&recorded.predecessor);
        current.checkpoint_generation = recorded.checkpoint_generation;
        if current != *recorded {
            return Err(BundleError::HistoryChanged);
        }
        Ok((*recorded).clone())
    }
    pub fn manual_switch_generation(&self) -> Result<u64, BundleError> {
        Ok(self
            .active_bundle()?
            .root()
            .ok_or(BundleError::Unsettled)?
            .manual_switch_generation)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod c0_fix1_tests {
    use super::*;
    use crate::{
        bundle::BundleWriteAdmission,
        bundle_records::{ObjectRef, ProjectId, RecordKind},
        storage_budget::StorageLimits,
        store::BundleRecord,
    };
    use kinewright_core::{AssetId, Command, Core, Document, Operation};
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

    fn floor_fixture() -> (
        TempDirectory,
        std::path::PathBuf,
        Core,
        Ja1EditingSession,
        StorageLimits,
    ) {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-publication-routes");
        let path = temp.path("project.kinewright");
        let core = Core::spawn(Document {
            tracks: vec![kinewright_core::Track {
                id: kinewright_core::TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        for index in 0..4 {
            core.request(Command::Do(Operation::SetTrackSyncLock {
                track: kinewright_core::TrackId(1),
                locked: index % 2 == 0,
            }))
            .unwrap();
        }
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        let before = core.export_history().unwrap();
        let mut low = 1;
        let mut high = StorageLimits::default().branch_history_bytes;
        while low < high {
            let middle = low + (high - low) / 2;
            let limits = StorageLimits {
                branch_history_bytes: middle,
                ..StorageLimits::default()
            };
            if crate::manual_history::OrdinaryHistoryLimits::from_limits(&limits)
                .unwrap()
                .check(&before)
                .is_ok()
            {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        let limits = StorageLimits {
            branch_history_bytes: low + 128,
            ..StorageLimits::default()
        };
        owner.restrict_manual_limits_probe(limits.clone()).unwrap();
        (temp, path, core, owner, limits)
    }
    fn floor_producer(
        temp: &TempDirectory,
        owner: &mut Ja1EditingSession,
        host: &Core,
    ) -> (
        ManualLease,
        Core,
        Ja1EditingSession,
        kinewright_core::ManualPermit,
    ) {
        let lease = owner
            .issue_manual_lease("a".repeat(64), "b".repeat(64))
            .unwrap();
        let gui = Core::spawn(Document::default()).unwrap();
        gui.restore_history(host.export_history().unwrap()).unwrap();
        let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
        let permit = replica
            .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
            .unwrap();
        (lease, gui, replica, permit)
    }
    #[test]
    fn ordinary_source_pressure_is_retryable_only_inside_the_real_publication_scope() {
        use crate::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
        use kinewright_core::{CoreMutationRefusal, Event};
        for fail in [false, true] {
            let (temp, path, host, mut owner, limits) = floor_fixture();
            let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
                OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
            ));
            let mut waiting = None;
            for index in 0..30 {
                let command = Command::Do(Operation::SetTrackSyncLock {
                    track: kinewright_core::TrackId(1),
                    locked: index % 2 == 0,
                });
                match admission.request(&host, command.clone()).unwrap() {
                    Event::DocumentChanged { .. } => {}
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualSaveRequired,
                        ..
                    } => {
                        waiting = Some(command);
                        break;
                    }
                    other => panic!("{other:?}"),
                }
            }
            let waiting = waiting.expect("second unpublished floor");
            let before = host.export_history().unwrap();
            let disk = std::fs::read(&path).unwrap();
            let moving = host.clone();
            let checking = std::sync::Arc::clone(&admission);
            let command = waiting.clone();
            let seen = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed = std::rc::Rc::clone(&seen);
            let result = crate::io_observer::at_barrier(
                "bundle_B0",
                move || {
                    if observed.replace(true) {
                        return;
                    }
                    let before = moving.export_history().unwrap();
                    assert!(matches!(
                        checking.request(&moving, command.clone()).unwrap(),
                        Event::MutationRefused {
                            reason: CoreMutationRefusal::ManualBackpressure,
                            ..
                        }
                    ));
                    assert_eq!(moving.export_history().unwrap(), before);
                },
                || {
                    if fail {
                        crate::io_observer::fail_once("root_file_sync", || owner.save_active())
                    } else {
                        owner.save_active()
                    }
                },
            );
            assert!(seen.get());
            if fail {
                assert!(result.is_err());
                assert_eq!(std::fs::read(&path).unwrap(), disk);
                assert_eq!(host.export_history().unwrap(), before);
                assert!(matches!(
                    admission.request(&host, waiting.clone()).unwrap(),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualSaveRequired,
                        ..
                    }
                ));
                owner.save_active().unwrap();
            } else {
                result.unwrap();
            }
            assert!(host.ordinary_floor_source().unwrap().is_none());
            assert!(matches!(
                admission.request(&host, waiting).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let expected = host.export_history().unwrap();
            owner.save_active().unwrap();
            drop(owner);
            let cold = Core::spawn(Document::default()).unwrap();
            crate::bundle::LoadedBundle::read_with_limits_probe(&path, limits)
                .unwrap()
                .restore_manual(&cold)
                .unwrap();
            assert_eq!(cold.export_history().unwrap(), expected);
            drop(temp);
        }
    }

    #[test]
    fn lease_rebind_and_switch_publish_a_pending_floor_without_a_person_save() {
        use crate::{
            bundle_recovery::RecoverySelection,
            bundle_switch::SwitchTarget,
            manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits},
        };
        use kinewright_core::Event;
        for switch in [false, true] {
            let (temp, path, host, mut owner, limits) = floor_fixture();
            let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
                OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
            ));
            assert!(matches!(
                admission
                    .request(
                        &host,
                        Command::Do(Operation::SetTrackSyncLock {
                            track: kinewright_core::TrackId(1),
                            locked: true
                        })
                    )
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
            assert!(host.ordinary_floor_source().unwrap().is_some());
            let expected = host.export_history().unwrap();
            let old_branch = owner.active_bundle().unwrap().manual_branch_id().clone();
            let lease = owner
                .issue_manual_lease("a".repeat(64), "rebound".into())
                .unwrap();
            assert!(host.ordinary_floor_source().unwrap().is_none());
            assert_eq!(
                *owner
                    .active_bundle()
                    .unwrap()
                    .branch_history(&old_branch)
                    .unwrap(),
                expected
            );
            if switch {
                let selection = owner
                    .list_recovery_points()
                    .unwrap()
                    .into_iter()
                    .find_map(|point| {
                        if let RecoverySelection::Checkpoint { root, branch } = point {
                            Some(SwitchTarget::Snapshot {
                                root,
                                branch,
                                version: "1".repeat(32),
                            })
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let envelope = owner.switch_envelope(&lease, selection).unwrap();
                owner.prepare_host_switch(&envelope).unwrap();
                owner.publish_host_switch(&envelope).unwrap();
                assert_eq!(
                    *owner
                        .active_bundle()
                        .unwrap()
                        .branch_history(&old_branch)
                        .unwrap(),
                    expected
                );
            } else {
                let gui = Core::spawn(Document::default()).unwrap();
                gui.restore_history(expected.clone()).unwrap();
                let replica =
                    Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
                let permit = replica
                    .bind_manual_producer(lease, owner.manual_input_context().unwrap())
                    .unwrap();
                assert!(permit.pending().unwrap().is_empty());
            }
            let published = owner.core().export_history().unwrap();
            drop(owner);
            let cold = Core::spawn(Document::default()).unwrap();
            crate::bundle::LoadedBundle::read_with_limits_probe(&path, limits)
                .unwrap()
                .restore_manual(&cold)
                .unwrap();
            assert_eq!(cold.export_history().unwrap(), published);
        }
    }

    #[test]
    fn ordinary_save_ahead_of_the_published_end_is_typed_until_the_stream_drains() {
        use kinewright_core::{CoreMutationRefusal, Event};
        let (temp, path, host, mut owner, limits) = floor_fixture();
        let (lease, gui, replica, permit) = floor_producer(&temp, &mut owner, &host);
        let published_end = host.export_history().unwrap().batches.len() as u64;
        for index in 0..30 {
            assert!(matches!(
                permit
                    .request(Command::Do(Operation::SetTrackSyncLock {
                        track: kinewright_core::TrackId(1),
                        locked: index % 2 == 0
                    }))
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
            if gui.export_history().unwrap().floor_offset > published_end {
                break;
            }
        }
        let expected = gui.export_history().unwrap();
        assert!(expected.floor_offset > published_end);
        let disk = std::fs::read(&path).unwrap();
        let inputs = owner.inputs.clone();
        let result = owner.active_bundle_mut().unwrap().save(&gui, &inputs);
        assert!(
            matches!(
                result,
                Err(BundleError::CoreMutationRefused(
                    CoreMutationRefusal::ManualBackpressure
                ))
            ),
            "{result:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        assert_eq!(gui.export_history().unwrap(), expected);
        let pending = permit.pending().unwrap();
        for message in &pending {
            owner.commit_manual_history(&lease, message).unwrap();
        }
        replica.acknowledge_manual(&permit, pending).unwrap();
        owner.save_active().unwrap();
        assert_eq!(host.export_history().unwrap(), expected);
        drop(owner);
        let cold = Core::spawn(Document::default()).unwrap();
        crate::bundle::LoadedBundle::read_with_limits_probe(&path, limits)
            .unwrap()
            .restore_manual(&cold)
            .unwrap();
        assert_eq!(cold.export_history().unwrap(), expected);
    }
    #[test]
    fn writer_keeps_shadow_backpressure_typed_and_the_identical_message_retries() {
        use kinewright_core::{CoreMutationRefusal, Event};
        let (temp, path, host, mut owner, limits) = floor_fixture();
        let (lease, gui, replica, permit) = floor_producer(&temp, &mut owner, &host);
        assert!(matches!(
            permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: kinewright_core::TrackId(1),
                    locked: false
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let expected = gui.export_history().unwrap();
        assert!(expected.floor_offset > host.export_history().unwrap().floor_offset);
        let message = permit.pending().unwrap().remove(0);
        let disk = std::fs::read(&path).unwrap();
        let holder = std::rc::Rc::new(std::cell::RefCell::new(owner));
        let observed = std::rc::Rc::new(std::cell::Cell::new(false));
        let checking = std::rc::Rc::clone(&holder);
        let seen = std::rc::Rc::clone(&observed);
        let retry_lease = lease.clone();
        let retry_message = message.clone();
        let mut competing = crate::bundle::ProjectBundle::activate(
            &temp.path("competing.kinewright"),
            BundleWriteAdmission::new_document(),
        )
        .unwrap();
        let inputs = competing.empty_inputs().unwrap().reference.clone();
        competing
            .restrict_limits_probe(StorageLimits {
                branch_history_bytes: limits.branch_history_bytes - 129,
                ..limits
            })
            .unwrap();
        let result = crate::io_observer::at_barrier(
            "bundle_B0",
            move || {
                if seen.replace(true) {
                    return;
                }
                let result = checking
                    .borrow_mut()
                    .commit_manual_history(&retry_lease, &retry_message);
                assert!(
                    matches!(
                        result,
                        Err(BundleError::CoreMutationRefused(
                            CoreMutationRefusal::ManualBackpressure
                        ))
                    ),
                    "{result:?}"
                );
            },
            || crate::io_observer::fail_once("root_file_sync", || competing.save(&host, &inputs)),
        );
        assert!(observed.get());
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        holder
            .borrow_mut()
            .commit_manual_history(&lease, &message)
            .unwrap();
        replica.acknowledge_manual(&permit, vec![message]).unwrap();
        assert_eq!(host.export_history().unwrap(), expected);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn s3_fix1_recovery_checks_inputs_before_replacement_authority() {
        for fault in [
            "owner",
            "predecessor",
            "fingerprint",
            "interpretation",
            "table",
            "bytes",
            "stream",
        ] {
            let temp =
                TempDirectory::new_in(&workspace_target_dir(), "c0-fix1-recovery-validation");
            let path = temp.path("validation.kinewright");
            let core = Core::spawn(Document::default()).unwrap();
            let mut owner = Ja1EditingSession::activate(
                &path,
                BundleWriteAdmission::new_document(),
                &core,
                None,
            )
            .unwrap();
            let source = temp.path("source.wav");
            std::fs::write(
                &source,
                kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
            )
            .unwrap();
            owner.import_owned_media(&source).unwrap();
            std::fs::write(
                &source,
                kinewright_media::test_support::wav_f32(&vec![0.5; 4800], 48_000, 1),
            )
            .unwrap();
            let prepared = owner.prepare_owned_media(&source, AssetId(2)).unwrap();
            let lease = owner
                .issue_manual_lease_for_inputs("r".repeat(64), "t".repeat(64), &prepared)
                .unwrap();
            let gui = Core::spawn(Document::default()).unwrap();
            gui.restore_history(core.export_history_for_publication().unwrap())
                .unwrap();
            let replica = Ja1EditingSession::new(
                temp.root(),
                &gui,
                crate::storage_budget::StorageLimits::default(),
            )
            .unwrap();
            let permit = replica
                .bind_manual_producer(lease, prepared.manifest.clone())
                .unwrap();
            permit
                .request(Command::Do(Operation::AddAsset {
                    asset: prepared.asset.clone(),
                }))
                .unwrap();
            let mut original = AcceptedManualCommand::decode(&permit.pending().unwrap()[0])
                .unwrap()
                .envelope;
            let mut expected = gui.export_history_for_publication().unwrap();
            let mut manifest = prepared.manifest.clone();
            match fault {
                "owner" => manifest.project_id = Some(ProjectId::fresh().unwrap()),
                "predecessor" => manifest
                    .entries
                    .retain(|entry| entry.matches_media_asset(&prepared.asset)),
                "fingerprint" => {
                    std::sync::Arc::make_mut(&mut expected.document).media_pool[1]
                        .source_fingerprint
                        .content_sha256 = Some("0".repeat(64));
                }
                "interpretation" => {
                    std::sync::Arc::make_mut(&mut expected.document).media_pool[1]
                        .color_description
                        .provenance = kinewright_core::ColorProvenance::UserOverride;
                }
                "table" => {
                    let other = manifest
                        .entries
                        .iter()
                        .find(|entry| !entry.matches_media_asset(&prepared.asset))
                        .unwrap()
                        .chunk_table_ref()
                        .unwrap()
                        .clone();
                    manifest
                        .entries
                        .iter_mut()
                        .find(|entry| entry.matches_media_asset(&prepared.asset))
                        .unwrap()
                        .dependencies = vec![other];
                }
                "stream" => {
                    let mut entry = manifest
                        .entries
                        .iter()
                        .find(|entry| entry.matches_media_asset(&prepared.asset))
                        .unwrap()
                        .clone();
                    let mut interpretation = entry.media_interpretation().unwrap();
                    interpretation.audio.as_mut().unwrap().time_base = (1, 42);
                    let profile = interpretation.canonical().unwrap();
                    entry.entry_id = format!(
                        "media:2:{}:{}:{}",
                        entry.object.sha256,
                        entry.object.byte_length,
                        kinewright_media::sha256_bytes(profile.as_bytes())
                    );
                    entry.profile = Some(profile);
                    manifest.entries.push(entry);
                    manifest.entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
                }
                "bytes" => {
                    use std::io::{Seek, SeekFrom, Write};
                    let entry = manifest
                        .entries
                        .iter()
                        .find(|entry| entry.matches_media_asset(&prepared.asset))
                        .unwrap();
                    let assets = path.with_extension("kinewright-assets");
                    let mut writer = std::fs::OpenOptions::new()
                        .write(true)
                        .open(assets.join("studio/objects").join(&entry.object.sha256))
                        .unwrap();
                    writer.seek(SeekFrom::Start(1000)).unwrap();
                    writer.write_all(&[99]).unwrap();
                }
                _ => unreachable!(),
            }
            let bundle = owner.destination.as_mut().unwrap().bundle_mut().unwrap();
            let bytes = serde_json::to_vec(&manifest).unwrap();
            let reference = ObjectRef {
                sha256: kinewright_media::sha256_bytes(&bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::InputManifest,
            };
            let _pin = bundle
                .store
                .admit_recovered(&mut bytes.as_slice(), &reference, &manifest.dependencies())
                .unwrap();
            original.manual.as_mut().unwrap().binding.inputs = reference;
            let issuers = bundle.issuers.len();
            let root = owner.root_sha256().unwrap();
            let result =
                owner.publish_absent_manual_suffix(&"r".repeat(64), &[original], &expected);
            assert!(result.is_err(), "C0_FIX1_RECOVERY_INPUT_REFUSED {fault}");
            assert_eq!(
                owner.active_bundle().unwrap().issuers.len(),
                issuers,
                "C0_FIX1_NO_REPLACEMENT_AUTHORITY {fault}"
            );
            assert_eq!(owner.root_sha256().unwrap(), root, "{fault}");
            assert_eq!(
                core.export_history_for_publication()
                    .unwrap()
                    .document
                    .media_pool
                    .len(),
                1,
                "{fault}"
            );
        }
    }
}
