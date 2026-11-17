//! The GUI's bounded stream and the writer's durable shadow share one protocol.
use super::{BundleError, BundleSaveReport, Ja1EditingSession};
use crate::{
    operation_actor::{AcceptedCut, ManualPermit},
    operation_inputs::ManualInputs,
    operation_protocol::{Action, EncodedMessage, Lease, Receipt, Seed, StreamState},
};
use std::sync::{Arc, atomic::AtomicBool};
fn protocol(error: crate::operation_protocol::Conflict) -> BundleError {
    crate::bundle::operation_error(crate::operation_actor::Error::Protocol(error))
}
pub struct ManualAuthority {
    lease: Lease,
    inputs: ManualInputs,
}
impl std::ops::Deref for ManualAuthority {
    type Target = Lease;
    fn deref(&self) -> &Lease {
        &self.lease
    }
}
impl ManualAuthority {
    #[must_use]
    pub fn lease(&self) -> &Lease {
        &self.lease
    }
}
impl Ja1EditingSession {
    pub fn active_manual_project(&self) -> Result<crate::bundle_records::ProjectId, BundleError> {
        Ok(self.active_bundle()?.project_id().clone())
    }
    pub fn acknowledge_manual(
        &self,
        permit: &ManualPermit,
        prefix: Vec<Receipt>,
    ) -> Result<(), BundleError> {
        self.attachment
            .acknowledge_manual(permit, prefix)
            .map_err(crate::bundle::operation_error)
    }
    pub fn bind_manual_producer(
        &self,
        lease: Lease,
        inputs: crate::owned_inputs::OwnedInputManifest,
        live: Arc<AtomicBool>,
    ) -> Result<ManualPermit, BundleError> {
        let inputs = ManualInputs::from_wire(lease.inputs.clone(), inputs, &self.input_usage)?;
        self.attachment
            .manual_permit(lease, inputs, live)
            .map_err(crate::bundle::operation_error)
    }
    pub fn manual_input_context(
        &self,
    ) -> Result<crate::owned_inputs::OwnedInputManifest, BundleError> {
        let bundle = self.active_bundle()?;
        Ok((*bundle.store.read_input_manifest(
            &bundle
                .checkpoint(bundle.manual_branch_id())?
                .input_manifest_ref,
        )?)
        .clone())
    }
    pub fn issue_manual_lease(
        &mut self,
        client: String,
        token: String,
    ) -> Result<Lease, BundleError> {
        self.require_local_writer()?;
        let core = self.core.clone();
        let inputs = self.inputs.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let mut published_base = None;
        let result = bundle.with_person_publication(|bundle| {
            let branch = bundle.manual_branch_id().clone();
            // The writer shadow has no other mutator. The full combined state
            // is checked at the boundary where authority is issued.
            let cut = bundle
                .attach_core(&core)?
                .capture()
                .map_err(crate::bundle::operation_error)?;
            if bundle.checkpoint(&branch)?.identity != cut.identity() {
                // The GUI handover installed a new epoch without claiming a
                // save. Publish its exact owner and empty chain atomically
                // before granting the first stream authority.
                let capture = core
                    .capture_state()
                    .map_err(crate::bundle::operation_error)?;
                if capture.change != cut.change() {
                    return Err(BundleError::HistoryChanged);
                }
                let stream = StreamState::installed(Seed {
                    project: bundle.project_id().clone(),
                    branch: branch.clone(),
                    attachment_epoch: capture.snapshot.history().epoch,
                    installed: cut.identity(),
                })
                .map_err(protocol)?;
                published_base = Some(bundle.publish_manual_base(&core, capture, &inputs, stream)?);
            }
            let issuer = bundle.issue_issuer(&core, &inputs, 1)?;
            let checkpoint = bundle.checkpoint(&branch)?;
            let stream = if let Some(stream) = &checkpoint.stream {
                stream.clone()
            } else {
                StreamState::installed(Seed {
                    project: bundle.project_id().clone(),
                    branch: branch.clone(),
                    attachment_epoch: checkpoint.position.epoch,
                    installed: checkpoint.identity,
                })
                .map_err(protocol)?
            };
            crate::bundle_commit::BundleCommit::publish_stream(bundle, &inputs, stream)?;
            let (parent_identity, namespace_name) = bundle.store.with_namespace(|n| {
                let identity = n.identity();
                Ok((
                    (identity.parent_device, identity.parent_inode),
                    identity
                        .resolved_root
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .ok_or(BundleError::HistoryChanged)?
                        .to_owned(),
                ))
            })?;
            let checkpoint = bundle.checkpoint(&branch)?;
            Ok(Lease {
                state: checkpoint
                    .stream
                    .clone()
                    .ok_or(BundleError::HistoryChanged)?,
                snapshot: checkpoint.identity,
                next_key: checkpoint.position,
                parent_identity: Some(parent_identity),
                namespace_name: Some(namespace_name),
                client,
                token,
                switch_generation: bundle
                    .root()
                    .ok_or(BundleError::HistoryChanged)?
                    .manual_switch_generation,
                owner_epoch: bundle.store.owner_epoch(),
                generation: 1,
                issuer,
                next_issuer_sequence: 1,
                anchor_head: checkpoint.head_id.clone(),
                inputs: checkpoint.input_manifest_ref.clone(),
            })
        });
        if let Some(report) = published_base
            && let Some(association) = &mut self.association
        {
            association.saved = Some(report.history_sha256);
            association.saved_position = Some(report.position);
        }
        result
    }
    pub fn manual_authority(&self, lease: Lease) -> Result<ManualAuthority, BundleError> {
        let inputs = ManualInputs::read(self.store()?, &lease.inputs)?;
        Ok(ManualAuthority { lease, inputs })
    }
    pub fn commit_manual_history(
        &mut self,
        authority: &ManualAuthority,
        accepted: &EncodedMessage,
    ) -> Result<(Receipt, Option<BundleSaveReport>), BundleError> {
        self.require_local_writer()?;
        let lease = &authority.lease;
        let message = accepted.decode().map_err(protocol)?;
        // Lookup comes first; a retired issuer must still get its saved receipt.
        if let Some(receipt) = self.manual_receipt(accepted)? {
            let core = self.core.clone();
            let bundle = self.active_bundle_mut()?;
            let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
            let matching = checkpoint
                .stream
                .as_ref()
                .and_then(|state| state.receipts().last())
                .is_some_and(|last| last.id == receipt.id)
                && bundle
                    .pending_publication
                    .as_ref()
                    .is_some_and(|pending| pending.identity == checkpoint.identity);
            if matching {
                bundle.reconcile_command(&core)?;
            }
            if !bundle.unsettled {
                let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
                let identity = checkpoint.identity;
                let position = checkpoint.position;
                if let Some(association) = &mut self.association {
                    association.saved = Some(identity.to_string());
                    association.saved_position = Some(position);
                }
            }
            return Ok((receipt, None));
        }
        let core = self.core.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        if lease.state.seed()
            != checkpoint
                .stream
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?
                .seed()
            || lease.owner_epoch != bundle.store.owner_epoch()
            || lease.switch_generation
                != bundle
                    .root()
                    .ok_or(BundleError::HistoryChanged)?
                    .manual_switch_generation
            || message.id.issuer != lease.issuer
            || lease.inputs != checkpoint.input_manifest_ref
        {
            return Err(BundleError::HistoryChanged);
        }
        // The host retains this certificate while the lease is live, including
        // its pin. Admission also validates the precise operation payload here.
        if let Action::Do { operations, .. } = &message.action {
            authority.inputs.check(operations)?;
        }
        let result = bundle.with_person_publication(|bundle| {
            crate::bundle_commit::BundleCommit::admitted_history(
                bundle,
                &message,
                &core,
                &lease.inputs,
            )
        })?;
        if let Some(report) = &result.1 {
            let association = self
                .association
                .as_mut()
                .ok_or(BundleError::HistoryChanged)?;
            association.saved = Some(report.history_sha256.clone());
            association.saved_position = Some(report.position);
        }
        Ok(result)
    }
    pub fn command_receipt(
        &self,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<crate::receipts::ReceiptLookup, BundleError> {
        self.active_bundle()?.receipt_lookup(envelope)
    }
    pub fn manual_receipt(
        &self,
        accepted: &EncodedMessage,
    ) -> Result<Option<Receipt>, BundleError> {
        let message = accepted.decode().map_err(protocol)?;
        self.validate_manual_publisher(message.publisher)?;
        self.active_bundle()?
            .checkpoint(self.active_bundle()?.manual_branch_id())?
            .stream
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?
            .lookup(&message)
            .map(Option::<&Receipt>::cloned)
            .map_err(protocol)
    }
    pub fn validate_manual_message_publisher(
        &self,
        accepted: &EncodedMessage,
    ) -> Result<(), BundleError> {
        self.validate_manual_publisher(accepted.decode().map_err(protocol)?.publisher)
    }
    pub fn manual_publisher(&self) -> Result<crate::operation_protocol::Publisher, BundleError> {
        self.store()?.with_namespace(|namespace| {
            let identity = namespace.identity();
            let name = identity
                .resolved_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or(BundleError::HistoryChanged)?;
            crate::operation_protocol::Publisher::native(
                (identity.parent_device, identity.parent_inode),
                name,
            )
            .map_err(protocol)
        })
    }
    pub fn validate_manual_publisher(
        &self,
        publisher: crate::operation_protocol::Publisher,
    ) -> Result<(), BundleError> {
        let expected = self.manual_publisher()?;
        if publisher != expected {
            return Err(BundleError::Identity(
                "accepted command belongs to a different native publisher namespace".into(),
            ));
        }
        Ok(())
    }
    /// A fresh authenticated recovery owner may apply only this bounded original
    /// chain, with its original ids and tips. No retired issuer is revived.
    pub fn reconcile_manual_chain(
        &mut self,
        _client: &str,
        pending: &[EncodedMessage],
        expected: Option<crate::operation_history::SnapshotIdentity>,
    ) -> Result<(), BundleError> {
        self.require_local_writer()?;
        if pending.len() > crate::operation_protocol::PENDING_COMMANDS
            || pending
                .iter()
                .try_fold(0_u64, |n, p| n.checked_add(p.byte_len()))
                .is_none_or(|n| n > crate::operation_protocol::PENDING_BYTES)
            || (!pending.is_empty() && expected.is_none())
        {
            return Err(BundleError::Capacity("manual recovery chain"));
        }
        for message in pending {
            self.validate_manual_message_publisher(message)?;
        }
        let core = self.core.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let result = bundle.with_person_publication(|bundle| {
            for accepted in pending {
                let message = accepted.decode().map_err(protocol)?;
                let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
                let stream = checkpoint
                    .stream
                    .as_ref()
                    .ok_or(BundleError::HistoryChanged)?;
                if stream.lookup(&message).map_err(protocol)?.is_some() {
                    continue;
                }
                let current = &checkpoint.input_manifest_ref;
                let inputs = match message.context.inputs.as_slice() {
                    [] => current.clone(),
                    [inputs] => {
                        if inputs != current {
                            crate::bundle_commit::validate_manifest_extension(
                                &bundle.store,
                                current,
                                inputs,
                            )?;
                        }
                        inputs.clone()
                    }
                    _ => return Err(BundleError::HistoryChanged),
                };
                let certificate = ManualInputs::read(&bundle.store, &inputs)?;
                if let Action::Do { operations, .. } = &message.action {
                    certificate.check(operations)?;
                }
                crate::bundle_commit::BundleCommit::admitted_history(
                    bundle, &message, &core, &inputs,
                )?;
            }
            if expected.is_some_and(|identity| {
                bundle
                    .checkpoint(bundle.manual_branch_id())
                    .is_ok_and(|checkpoint| checkpoint.identity != identity)
            }) {
                return Err(BundleError::HistoryChanged);
            }
            Ok(())
        });
        // A later suffix may refuse after an earlier message committed. Keep
        // the input identity of that exact durable prefix in either outcome.
        self.inputs = bundle
            .checkpoint(bundle.manual_branch_id())?
            .input_manifest_ref
            .clone();
        if !bundle.unsettled {
            let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
            if let Some(association) = &mut self.association {
                association.saved = Some(checkpoint.identity.to_string());
                association.saved_position = Some(checkpoint.position);
            }
        }
        result
    }
    pub fn rotate_manual_epoch(
        &mut self,
        source: crate::operation_history::SnapshotIdentity,
    ) -> Result<crate::bundle_records::ObjectRef, BundleError> {
        self.require_local_writer()?;
        let checkpoint = self
            .active_bundle()?
            .checkpoint(self.active_bundle()?.manual_branch_id())?
            .clone();
        if let Some(install) = &checkpoint.install
            && install.source == source
        {
            self.validate_manual_publisher(install.publisher)?;
            return self
                .active_bundle()?
                .checkpoint_reference(self.active_bundle()?.manual_branch_id());
        }
        if checkpoint.identity != source {
            return Err(BundleError::HistoryChanged);
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        let update = self
            .attachment
            .prepare_reinstall(
                &cut,
                crate::operation_actor::CommandContext {
                    creation: crate::operation_history::CreationOrigin {
                        branch: checkpoint.branch_id.clone(),
                        batch: crate::bundle_records::BatchId::fresh()?,
                    },
                    inputs: vec![],
                    approval: None,
                    transition: None,
                },
            )
            .map_err(crate::bundle::operation_error)?;
        let install = crate::operation_checkpoint::InstallRecord {
            publisher: self.manual_publisher()?,
            source,
            source_position: checkpoint.position,
            stream: checkpoint.stream.clone(),
        };
        let core = self.core.clone();
        let bundle = self
            .destination
            .as_mut()
            .ok_or(BundleError::HistoryChanged)?
            .bundle_mut()?;
        let report = bundle.with_person_publication(|bundle| {
            crate::bundle_commit::BundleCommit::publish_update(
                bundle,
                &core,
                cut,
                update,
                &checkpoint.input_manifest_ref,
                &checkpoint.branch_id,
                None,
                false,
                Some(install),
            )
        })?;
        let reference = bundle.checkpoint_reference(&checkpoint.branch_id)?;
        if let Some(association) = &mut self.association {
            association.saved = Some(report.history_sha256);
            association.saved_position = Some(report.position);
        }
        Ok(reference)
    }
    pub fn installed_manual_receipts(
        &self,
        source: crate::operation_history::SnapshotIdentity,
    ) -> Result<Option<Vec<Receipt>>, BundleError> {
        let bundle = self.active_bundle()?;
        let Some(install) = bundle
            .checkpoint(bundle.manual_branch_id())?
            .install
            .as_ref()
            .filter(|install| install.source == source)
        else {
            return Ok(None);
        };
        self.validate_manual_publisher(install.publisher)?;
        Ok(Some(
            install
                .stream
                .as_ref()
                .map_or_else(Vec::new, |stream| stream.receipts().cloned().collect()),
        ))
    }
    pub fn published_manual_history(
        &self,
    ) -> Result<crate::operation_actor::PortableHistory, BundleError> {
        let bundle = self.active_bundle()?;
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id())?;
        Ok(crate::operation_actor::PortableHistory {
            snapshot: Arc::new(
                bundle
                    .branch_history(bundle.manual_branch_id())?
                    .into_capture()?
                    .into_transport()?,
            ),
            branch: checkpoint.branch_id.clone(),
            revision: checkpoint.revision,
        })
    }
    pub fn manual_namespace_name(&self) -> Result<String, BundleError> {
        self.active_bundle()?.store.with_namespace(|n| {
            n.identity()
                .resolved_root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned)
                .ok_or(BundleError::HistoryChanged)
        })
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn restrict_manual_limits_probe(
        &mut self,
        limits: crate::storage_budget::StorageLimits,
    ) -> Result<(), BundleError> {
        self.active_bundle_mut()?.restrict_limits_probe(limits)
    }
    pub fn manual_parent_identity(&self) -> Result<(u64, u64), BundleError> {
        self.active_bundle()?
            .store
            .with_namespace(|n| Ok((n.identity().parent_device, n.identity().parent_inode)))
    }
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
        Ok((
            bundle.checkpoint_reference(bundle.manual_branch_id())?,
            bundle.checkpoint(bundle.manual_branch_id())?.clone(),
        ))
    }
    pub fn freeze_manual_barrier(&self) -> Result<AcceptedCut, BundleError> {
        self.attachment
            .freeze()
            .map_err(crate::bundle::operation_error)
    }
    pub fn resume_manual_barrier(&mut self, cut: &AcceptedCut) -> Result<(), BundleError> {
        self.attachment
            .resume(cut)
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = None;
        Ok(())
    }
    pub fn verify_manual_predecessor(
        &self,
        reference: &crate::bundle_records::ObjectRef,
        identity: crate::operation_history::SnapshotIdentity,
    ) -> Result<(), BundleError> {
        let checkpoint = self
            .active_bundle()?
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(reference)?;
        if checkpoint.branch_id != *self.active_bundle()?.manual_branch_id()
            || checkpoint.identity != identity
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
    pub fn recapture_host_recovery_cut(&mut self) -> Result<AcceptedCut, BundleError> {
        if !self.host_attached || self.host_retention_lost {
            return Err(BundleError::HistoryChanged);
        }
        let cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        self.host_cut = Some(cut);
        Ok(cut)
    }
    pub fn manual_recovery_endpoint(
        &self,
        cut: &AcceptedCut,
        pending: Vec<EncodedMessage>,
    ) -> Result<ManualRecoveryEndpoint, BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.host_cut.as_ref() != Some(cut)
            || self
                .core
                .change_id()
                .map_err(crate::bundle::operation_error)?
                != cut.change()
            || !self.rescue.attempts.is_empty()
            || !self.retained_destinations.is_empty()
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(ManualRecoveryEndpoint {
            attachment: crate::ProjectAttachment::new(&self.core)
                .map_err(crate::bundle::operation_error)?,
            core: self.core.clone(),
            cut: *cut,
            pending,
            input_usage: std::sync::Arc::clone(&self.input_usage),
        })
    }
    pub fn finish_host_manual_recovery(
        &mut self,
        publisher_digest: &str,
    ) -> Result<(), BundleError> {
        if !self.host_attached
            || self.host_retention_lost
            || self.host_cut.is_none()
            || self
                .attachment
                .capture()
                .map_err(crate::bundle::operation_error)?
                .identity()
                .to_string()
                != publisher_digest
        {
            return Err(BundleError::HistoryChanged);
        }
        self.host_cut = None;
        self.host_published_digest = Some(publisher_digest.into());
        if let Some(a) = &mut self.association {
            a.saved_position = Some(
                self.core
                    .change_id()
                    .map_err(crate::bundle::operation_error)?
                    .position,
            );
            a.saved = Some(publisher_digest.into());
        }
        Ok(())
    }
    pub fn manual_switch_generation(&self) -> Result<u64, BundleError> {
        Ok(self
            .active_bundle()?
            .root()
            .ok_or(BundleError::Unsettled)?
            .manual_switch_generation)
    }
}
pub struct ManualRecoveryEndpoint {
    attachment: crate::ProjectAttachment,
    core: crate::operation_actor::Core,
    cut: AcceptedCut,
    pending: Vec<EncodedMessage>,
    input_usage: std::sync::Arc<std::sync::Mutex<crate::storage_budget::StorageUsage>>,
}
impl ManualRecoveryEndpoint {
    pub fn install(
        &mut self,
        transport: crate::operation_actor::PortableHistory,
        receipts: Vec<Receipt>,
    ) -> Result<(), BundleError> {
        let crate::operation_actor::PortableHistory {
            snapshot,
            branch,
            revision,
        } = transport;
        let role = snapshot.role();
        let snapshot = snapshot.receive(self.core.history_pool(), role)?;
        self.attachment
            .acknowledge_frozen(&self.cut, receipts)
            .map_err(crate::bundle::operation_error)?;
        // The actor retains any unacknowledged suffix. This duplicate request
        // buffer has finished its job and must not keep the old arena's entire
        // reservation through replacement. Install still requires a drained actor.
        self.pending.clear();
        // Every pending command must have a durable receipt before replacement.
        self.attachment
            .install_peer_snapshot(&self.cut, snapshot, branch, revision)
            .map_err(crate::bundle::operation_error)?;
        self.cut = self
            .attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        Ok(())
    }
    pub fn snapshot(&self) -> Result<crate::operation_actor::PortableHistory, BundleError> {
        self.core
            .portable_history()
            .map_err(crate::bundle::operation_error)
    }
    pub fn finish(
        self,
        lease: Lease,
        inputs: crate::owned_inputs::OwnedInputManifest,
        live: Arc<AtomicBool>,
    ) -> Result<ManualPermit, BundleError> {
        if self.cut.identity() != lease.snapshot || !self.pending.is_empty() {
            return Err(BundleError::HistoryChanged);
        }
        let inputs = ManualInputs::from_wire(lease.inputs.clone(), inputs, &self.input_usage)?;
        self.attachment
            .resume_manual(&self.cut, lease, inputs, live)
            .map_err(crate::bundle::operation_error)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod c0_fix1_tests {
    use super::*;
    use crate::{bundle::BundleWriteAdmission, operation_actor::Core};
    use kinewright_core::{Command, Document, Operation, Track, TrackId, TrackKind};
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

    fn owner() -> (TempDirectory, Ja1EditingSession) {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "owner-manual-protocol");
        let core = Core::for_test(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner = Ja1EditingSession::activate(
            &temp.path("project.kinewright"),
            BundleWriteAdmission::new_document(),
            &core,
            None,
        )
        .unwrap();
        owner.save_active().unwrap();
        (temp, owner)
    }
    fn replica(
        temp: &TempDirectory,
        owner: &Ja1EditingSession,
        lease: Lease,
    ) -> (Ja1EditingSession, ManualPermit) {
        // Two processes have distinct explicit pools. This test exercises the
        // same portable receiver and full validation as the GUI bootstrap.
        let pool = crate::operation_history::HistoryPool::new(
            crate::operation_history::PoolLimits::default(),
        );
        let wire = owner.core().portable_history().unwrap();
        let branch = wire.branch.clone();
        let revision = wire.revision;
        let core = Core::open(wire.validate(&pool).unwrap(), branch, revision).unwrap();
        let replica = Ja1EditingSession::new(
            temp.root(),
            &core,
            crate::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        let permit = replica
            .bind_manual_producer(
                lease,
                owner.manual_input_context().unwrap(),
                Arc::new(AtomicBool::new(true)),
            )
            .unwrap();
        (replica, permit)
    }
    #[test]
    fn handover_pool_refusal_preserves_the_live_source_and_retries_after_release() {
        let (_temp, mut gui) = owner();
        let first = gui.core().export_history().unwrap();
        let second = gui.core().export_history().unwrap();
        let before = gui.core().change_id().unwrap();
        let identity = gui.core().snapshot_identity().unwrap();
        let leases = gui
            .core()
            .history_pool()
            .lease_counts(crate::operation_history::Role::Ordinary)
            .unwrap()
            .0;
        assert!(
            matches!(
                gui.prepare_host_attachment(),
                Err(BundleError::Operation(
                    crate::operation_actor::Error::History(
                        crate::operation_history::Error::PoolFull(
                            crate::operation_history::Role::Ordinary
                        )
                    )
                ))
            ),
            "HANDOVER_FULL_POOL_IS_A_TYPED_RETRYABLE_REFUSAL"
        );
        assert_eq!(
            gui.core().attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live,
            "HOST_HANDOVER_POOL_REFUSAL_PRESERVES_THE_LIVE_SOURCE"
        );
        assert_eq!(gui.core().change_id().unwrap(), before);
        assert_eq!(gui.core().snapshot_identity().unwrap(), identity);
        assert_eq!(
            gui.core()
                .history_pool()
                .lease_counts(crate::operation_history::Role::Ordinary)
                .unwrap()
                .0,
            leases,
            "HOST_HANDOVER_REFUSAL_NEVER_CHANGES_ITS_SOURCE_RESERVATION"
        );
        drop(second);
        let prepared = gui
            .prepare_host_attachment()
            .expect("HOST_HANDOVER_RETRIES_AFTER_A_CAPTURE_IS_RELEASED");
        assert_ne!(prepared.history.identity(), identity);
        drop(first);
    }
    #[test]
    fn first_save_reconciliation_preserves_a_later_local_suffix() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "first-save-later-suffix");
        let core = Core::for_test(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut session = Ja1EditingSession::new(
            temp.root(),
            &core,
            crate::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        let path = temp.path("project.kinewright");
        assert!(matches!(
            crate::io_observer::fail_once("root_parent_sync", || session
                .first_save(&path, crate::storage_budget::StorageLimits::default()))
            .unwrap(),
            crate::bundle_copy::CopyStatus::DurabilityUnknown
        ));
        // An unavailable first-save acknowledgement leaves the local producer
        // live. Its later accepted commands must survive reconciliation.
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
        let before = core.export_history().unwrap();
        session.reconcile_active().unwrap();
        let after = core.export_history().unwrap();
        assert_eq!(
            &*after.history().document,
            &*before.history().document,
            "FIRST_SAVE_RECONCILIATION_PRESERVES_THE_LATER_DOCUMENT"
        );
        assert_eq!(
            serde_json::to_value(&after.history().undo).unwrap(),
            serde_json::to_value(&before.history().undo).unwrap(),
            "FIRST_SAVE_RECONCILIATION_PRESERVES_EVERY_LATER_COMMAND"
        );
        assert!(
            session.dirty().unwrap(),
            "SAVED_PREFIX_NEVER_ACKNOWLEDGES_THE_LATER_SUFFIX"
        );
        drop(before);
        drop(after);
        session.save_active().unwrap();
        assert!(!session.dirty().unwrap());
        let expected = core.snapshot_identity().unwrap();
        let mut cold = crate::bundle::LoadedBundle::read(
            &path,
            &crate::operation_history::HistoryPool::new(
                crate::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        assert_eq!(
            cold.open_manual().unwrap().snapshot_identity().unwrap(),
            expected,
            "FIRST_SAVE_SUFFIX_SURVIVES_ITS_NEXT_SAVE_AND_COLD_OPEN"
        );
    }
    #[test]
    fn refused_peer_install_preserves_the_live_destination() {
        let (_left, mut destination) = owner();
        let (_right, mut sibling) = owner();
        for (session, locked) in [(&destination, false), (&sibling, true)] {
            session
                .core()
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked,
                }))
                .unwrap();
        }
        let before = destination.core().change_id().unwrap();
        let identity = destination.core().snapshot_identity().unwrap();
        let incoming = sibling.prepare_host_attachment().unwrap();
        assert!(
            matches!(
                destination.install_host_snapshot(incoming.history),
                Err(BundleError::Operation(
                    crate::operation_actor::Error::History(
                        crate::operation_history::Error::CommandConflict
                    )
                ))
            ),
            "PEER_INSTALL_RETAINED_KEY_CONFLICT_IS_TYPED"
        );
        assert_eq!(
            destination.core().change_id().unwrap(),
            before,
            "PEER_INSTALL_REFUSAL_PRESERVES_THE_ACCEPTED_CUT"
        );
        assert_eq!(
            destination.core().snapshot_identity().unwrap(),
            identity,
            "PEER_INSTALL_REFUSAL_PRESERVES_THE_COMBINED_IDENTITY"
        );
        assert_eq!(
            destination.core().attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live,
            "PEER_INSTALL_REFUSAL_PRESERVES_THE_LIVE_DESTINATION"
        );
    }
    #[test]
    fn manual_lease_publishes_the_validated_handover_snapshot_before_authority() {
        let (temp, mut gui) = owner();
        gui.core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }))
            .unwrap();
        let transfer = gui.prepare_host_attachment().unwrap();
        let expected = transfer.history.identity();
        let expected_revision = transfer.history.revision;
        let expected_branch = transfer.history.branch.clone();
        let path = temp.path("project.kinewright");
        let pool = crate::operation_history::HistoryPool::new(
            crate::operation_history::PoolLimits::default(),
        );
        let mut writer = Ja1EditingSession::open(&path, &pool).unwrap();
        writer.install_host_snapshot(transfer.history).unwrap();
        assert_eq!(
            writer.core().change_id().unwrap().revision,
            expected_revision,
            "HANDOVER_PEERS_SHARE_THE_GUI_REVISION"
        );
        assert_eq!(
            writer.core().creation_branch().unwrap(),
            expected_branch,
            "HANDOVER_PEERS_SHARE_THE_GUI_BRANCH"
        );
        let lease = writer
            .issue_manual_lease("h".repeat(64), "t".repeat(64))
            .expect("HANDOVER_LEASE_PUBLISHES_THE_INSTALLED_GUI_BASE_BEFORE_AUTHORITY");
        assert_eq!(lease.snapshot, expected);
        assert_eq!(lease.state.seed().installed, expected);
        assert_eq!(
            writer.manual_checkpoint().unwrap().1.identity,
            expected,
            "HANDOVER_AUTHORITY_NAMES_A_DURABLE_COMBINED_SNAPSHOT"
        );
        let mut cold = crate::bundle::LoadedBundle::read(
            &path,
            &crate::operation_history::HistoryPool::new(
                crate::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        assert_eq!(
            cold.open_manual().unwrap().snapshot_identity().unwrap(),
            expected,
            "HANDOVER_LEASE_COLD_OPEN_EQUALS_THE_GUI_BASE"
        );
    }
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one receipt lifecycle includes the lost acknowledgement, restart and fresh installation"
    )]
    fn production_manual_receipts_survive_restart_and_a_fresh_install_epoch() {
        let (temp, mut writer) = owner();
        let lease = writer
            .issue_manual_lease("g".repeat(64), "t".repeat(64))
            .unwrap();
        let authority = writer.manual_authority(lease.clone()).unwrap();
        let (gui, permit) = replica(&temp, &writer, lease.clone());
        for command in [
            Command::DoBatchCoalesced {
                operations: vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                }],
                coalesce_key: "gesture".into(),
            },
            Command::DoBatchCoalesced {
                operations: vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true,
                }],
                coalesce_key: "gesture".into(),
            },
            Command::Undo,
            Command::Redo,
        ] {
            permit.request(command).unwrap();
        }
        permit.close_gesture().unwrap();
        let pending = permit.pending().unwrap();
        let mut receipts = Vec::new();
        for message in &pending {
            let (receipt, report) = writer.commit_manual_history(&authority, message).unwrap();
            let report = report.expect("ACK_FOLLOWS_DURABLE_ROOT_PUBLICATION");
            assert_eq!(
                kinewright_media::sha256_bytes(&std::fs::read(&report.path).unwrap()),
                report.root_sha256,
                "ACK_FOLLOWS_DURABLE_ROOT_PUBLICATION"
            );
            let position = writer.core().change_id().unwrap();
            let (repeated, duplicate) = writer.commit_manual_history(&authority, message).unwrap();
            assert_eq!(repeated, receipt, "RETRY_RETURNS_THE_ORIGINAL_RECEIPT");
            assert!(duplicate.is_none());
            assert_eq!(
                writer.core().change_id().unwrap(),
                position,
                "RECEIPT_RETRY_NEVER_REAPPLIES_COMMAND"
            );
            receipts.push(receipt);
        }
        let identity = gui.core().snapshot_identity().unwrap();
        assert_eq!(
            writer.core().snapshot_identity().unwrap(),
            identity,
            "GUI_AND_PUBLISHED_SHADOW_HAVE_IDENTICAL_OWNER_STATE"
        );
        let path = temp.path("project.kinewright");
        let pool = writer.core().history_pool().clone();
        drop(authority);
        drop(writer);
        let mut reopened = Ja1EditingSession::open(&path, &pool).unwrap();
        let retry_authority = reopened.manual_authority(lease).unwrap();
        for (message, receipt) in pending.iter().zip(&receipts) {
            assert_eq!(
                reopened.manual_receipt(message).unwrap().as_ref(),
                Some(receipt),
                "RESTART_RECOGNISES_EVERY_ORIGINAL_RECEIPT"
            );
            let before = reopened.core().change_id().unwrap();
            let (retried, report) = reopened
                .commit_manual_history(&retry_authority, message)
                .unwrap();
            assert_eq!(&retried, receipt, "RESTART_RETURNS_ORIGINAL_RECEIPT");
            assert!(report.is_none(), "RESTART_RETRY_PUBLISHES_NOTHING");
            assert_eq!(
                reopened.core().change_id().unwrap(),
                before,
                "RESTART_RECEIPT_RETRY_NEVER_REAPPLIES_COMMAND"
            );
        }
        drop(retry_authority);
        assert_eq!(
            reopened.core().snapshot_identity().unwrap(),
            identity,
            "WHAT_WAS_ACKNOWLEDGED_REOPENS_IDENTICALLY"
        );
        let installation = reopened.rotate_manual_epoch(identity);
        assert!(
            installation.is_ok(),
            "INSTALL_RETIRES_PREVIOUS_EPOCH_CHAIN: {:?}",
            installation.as_ref().err()
        );
        installation.unwrap();
        assert!(
            reopened.manual_checkpoint().unwrap().1.stream.is_none(),
            "INSTALL_RETIRES_PREVIOUS_EPOCH_CHAIN"
        );
        let after = reopened.core().snapshot_identity().unwrap();
        assert_ne!(after, identity, "RECOVERY_INSTALL_STARTS_A_NEW_EPOCH");
        let saved_position = reopened.core().change_id().unwrap().position;
        reopened.rotate_manual_epoch(identity).unwrap();
        assert_eq!(
            reopened.core().change_id().unwrap().position,
            saved_position,
            "LOST_INSTALL_REPLY_DOES_NOT_INSTALL_AGAIN"
        );
        assert_eq!(
            reopened
                .installed_manual_receipts(identity)
                .unwrap()
                .unwrap(),
            receipts,
            "INSTALL_RETAINS_ITS_ORIGINAL_RECEIPT_WINDOW"
        );
        let attachment = crate::ProjectAttachment::new(gui.core()).unwrap();
        let cut = attachment.freeze().unwrap();
        attachment
            .acknowledge_frozen(&cut, receipts.clone())
            .unwrap();
        let wire = reopened.published_manual_history().unwrap();
        let snapshot = wire.validate(gui.core().history_pool()).unwrap();
        let epoch = snapshot.position().epoch;
        attachment
            .install_snapshot(&cut, snapshot, Some(epoch))
            .unwrap();
        assert_eq!(
            gui.core().snapshot_identity().unwrap(),
            after,
            "RECOVERY_PEERS_ADOPT_THE_SAME_COMBINED_IDENTITY"
        );
        drop(reopened);
        let reopened = Ja1EditingSession::open(&path, &pool).unwrap();
        assert_eq!(
            reopened
                .installed_manual_receipts(identity)
                .unwrap()
                .unwrap(),
            receipts,
            "RESTART_RECOGNISES_THE_COMMITTED_INSTALL"
        );
        assert!(
            reopened.manual_receipt(&pending[0]).is_err(),
            "OLD_EPOCH_CHAIN_CANNOT_BE_CONTINUED"
        );
    }
    #[test]
    fn s3_fix1_recovery_checks_inputs_before_replacement_authority() {
        for fault in ["branch", "predecessor", "payload", "context"] {
            let (_temp, mut writer) = owner();
            let lease = writer
                .issue_manual_lease("g".repeat(64), "t".repeat(64))
                .unwrap();
            let authority = writer.manual_authority(lease.clone()).unwrap();
            let context = crate::operation_actor::CommandContext {
                creation: crate::operation_history::CreationOrigin {
                    branch: lease.state.seed().branch.clone(),
                    batch: crate::bundle_records::BatchId::fresh().unwrap(),
                },
                inputs: vec![],
                approval: None,
                transition: None,
            };
            let mut message = lease
                .state
                .message(
                    crate::receipts::CommandId {
                        issuer: lease.issuer.clone(),
                        sequence: lease.next_issuer_sequence,
                    },
                    lease.next_key,
                    Action::Do {
                        operations: vec![Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: false,
                        }],
                        gesture: None,
                    },
                    context,
                )
                .unwrap();
            match fault {
                "branch" => {
                    message.context.creation.branch =
                        crate::bundle_records::BranchId::fresh().unwrap();
                }
                "predecessor" => message.predecessor = [99; 32],
                "payload" => {
                    message.action = Action::Do {
                        operations: vec![Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: true,
                        }],
                        gesture: None,
                    }
                }
                "context" => message
                    .context
                    .inputs
                    .push(crate::bundle_records::ObjectRef {
                        sha256: "0".repeat(64),
                        byte_length: 1,
                        record_kind: crate::bundle_records::RecordKind::InputManifest,
                    }),
                _ => unreachable!(),
            }
            let accepted: EncodedMessage =
                serde_json::from_value(serde_json::json!(serde_json::to_string(&message).unwrap()))
                    .unwrap();
            let root = writer.root_sha256().unwrap();
            let before = writer.core().change_id().unwrap();
            assert!(
                writer.commit_manual_history(&authority, &accepted).is_err(),
                "C0_FIX1_RECOVERY_INPUT_REFUSED {fault}"
            );
            assert_eq!(
                writer.root_sha256().unwrap(),
                root,
                "C0_FIX1_NO_REPLACEMENT_AUTHORITY {fault}"
            );
            assert_eq!(
                writer.core().change_id().unwrap(),
                before,
                "MALFORMED_INGRESS_CHANGES_NO_OWNER_STATE {fault}"
            );
        }
    }
}
