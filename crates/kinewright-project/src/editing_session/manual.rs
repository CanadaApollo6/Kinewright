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
    pub fn bind_manual_producer(
        &self,
        lease: ManualLease,
        inputs: crate::owned_inputs::OwnedInputManifest,
    ) -> Result<kinewright_core::ManualPermit, BundleError> {
        if history_sha256(
            &self
                .core
                .export_history()
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
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let history = core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let branch = bundle.manual_branch_id().clone();
        if *bundle.branch_history(&branch)? != history {
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
            history_sha256: checkpoint.history_ref.sha256.clone(),
            inputs: checkpoint.input_manifest_ref.clone(),
            lineage: checkpoint.lineage_ref.clone(),
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
        let result = crate::bundle_commit::BundleCommit::admitted_history(
            bundle,
            &envelope,
            &core,
            &lease.inputs,
        )?;
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
                .map_err(|e| BundleError::Corrupt(e.to_string()))?
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
            self.publish_absent_manual_suffix(client, &absent)?;
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
    ) -> Result<(), BundleError> {
        use kinewright_core::{AttachmentState, Command, Event};
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
        let lease = self.issue_manual_lease(
            client.into(),
            format!(
                "{}{}",
                crate::bundle_records::SessionId::fresh()?.as_str(),
                crate::bundle_records::SessionId::fresh()?.as_str()
            ),
        )?;
        let before = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let original_issuer = self.active_bundle()?.issuers[&lease.issuer].clone();
        let _overlap = self.active_bundle()?.store.reserve_history_shadow(
            before
                .encoded_bytes()
                .map_err(crate::bundle::history_error)?,
        )?;
        let mut attempted = false;
        let outcome = (|| {
            let mut envelopes = Vec::with_capacity(originals.len());
            let mut encoded = 0_u64;
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
                let manual = renewed.manual.as_mut().ok_or(BundleError::HistoryChanged)?;
                manual.binding = lease.clone();
                manual.recovery_origin = Some(crate::receipts::OriginalCommand {
                    id: original.id.clone(),
                    digest: original.digest()?,
                });
                encoded = encoded
                    .checked_add(renewed.encoded_bytes()?)
                    .ok_or(BundleError::Capacity("manual recovery controls"))?;
                envelopes.push(renewed);
            }
            let reservation = self.active_bundle()?.store.reserve(
                0,
                encoded
                    .checked_mul(3)
                    .ok_or(BundleError::Capacity("manual recovery overlap"))?,
                0,
                false,
            )?;
            let mut predecessor = lease.predecessor.clone();
            for original in originals {
                let metadata = original
                    .manual
                    .as_ref()
                    .ok_or(BundleError::HistoryChanged)?;
                if metadata.recovery_origin.is_some()
                    || original.project_id != lease.project
                    || original.target_branch != lease.branch
                    || metadata.binding.switch_generation != lease.switch_generation
                    || metadata.binding.inputs != lease.inputs
                    || metadata.predecessor != predecessor
                    || history_sha256(
                        &self
                            .core
                            .export_history()
                            .map_err(crate::bundle::history_error)?,
                    )? != metadata.before_sha256
                {
                    return Err(BundleError::HistoryChanged);
                }
                if !matches!(
                    self.core
                        .request(Command::from(
                            original
                                .command
                                .clone()
                                .ok_or(BundleError::HistoryChanged)?
                        ))
                        .map_err(|_| BundleError::HistoryChanged)?,
                    Event::DocumentChanged { .. }
                ) {
                    return Err(BundleError::HistoryChanged);
                }
                let after = self
                    .core
                    .export_history()
                    .map_err(crate::bundle::history_error)?;
                if history_sha256(&after)? != metadata.after_sha256
                    || after.batches.last() != Some(&metadata.batch)
                    || kinewright_media::sha256_bytes(
                        &serde_json::to_vec(&after.cursor)
                            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                    ) != metadata.cursor_sha256
                {
                    return Err(BundleError::HistoryChanged);
                }
                predecessor = Some(metadata.batch_id.clone());
            }
            let after = self
                .core
                .export_history()
                .map_err(crate::bundle::history_error)?;
            let selected = crate::bundle_commit::bound_history(&after, &limits)?;
            let bundle = self
                .destination
                .as_mut()
                .ok_or(BundleError::Unsettled)?
                .bundle_mut()?;
            bundle.pending_command = Some(
                reservation.hold(envelopes.last().ok_or(BundleError::HistoryChanged)?.clone()),
            );
            bundle
                .issuers
                .get_mut(&lease.issuer)
                .ok_or(BundleError::HistoryChanged)?
                .next_sequence = u64::try_from(originals.len())
                .map_err(|_| BundleError::Capacity("manual recovery sequence"))?
                .checked_add(1)
                .ok_or(BundleError::Capacity("manual recovery sequence"))?;
            attempted = true;
            let report = crate::bundle_commit::BundleCommit::publish_recovery_batch(
                bundle,
                &after,
                &selected,
                &lease.inputs,
                &envelopes,
            )?;
            bundle.pending_command = None;
            Ok(report)
        })();
        match outcome {
            Ok(report) => {
                if let Some(association) = &mut self.association {
                    association.saved = Some(report.history_sha256);
                }
                Ok(())
            }
            Err(error) => {
                let bundle = self
                    .destination
                    .as_mut()
                    .ok_or(BundleError::Unsettled)?
                    .bundle_mut()?;
                if !attempted
                    || !matches!(
                        bundle.publication_state,
                        crate::bundle::PublicationState::Unknown
                            | crate::bundle::PublicationState::Committed
                    )
                {
                    bundle.pending_command = None;
                    bundle.issuers.insert(lease.issuer, original_issuer);
                    self.attachment
                        .restore_history(before)
                        .map_err(crate::bundle::history_error)?;
                } else {
                    bundle.unsettled = true;
                }
                Err(error)
            }
        }
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
            || *bundle
                .store
                .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)?
                != *history
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
            .export_history()
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
                    .export_history()
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
