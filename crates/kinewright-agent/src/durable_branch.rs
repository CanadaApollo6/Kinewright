//! Frozen specialist bases bind durable identity before using a fresh Core revision.
#![allow(clippy::missing_errors_doc)]
use crate::TimelineBranch;
use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Query, QueryResult, TimelineRevision,
};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchId, ObjectRef, SpecialistBaseRecord},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    store::StorePin,
};
use std::sync::Arc;

pub struct DurableTimelineBranch {
    branch_id: BranchId,
    branch: TimelineBranch,
    base: SpecialistBaseRecord,
    baseline_delta: Vec<kinewright_core::Operation>,
    pin: Option<StorePin>,
    registration: Option<kinewright_project::editing_session::DurableBranchSessionGuard>,
}

fn snapshot(core: &Core) -> Result<(TimelineRevision, Arc<Document>), BundleError> {
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) = core
        .request(Command::Query(Query::Snapshot))
        .map_err(|e| BundleError::Corrupt(e.to_string()))?
    else {
        return Err(BundleError::Corrupt("Core snapshot response".into()));
    };
    Ok((revision, document))
}

impl DurableTimelineBranch {
    /// The live Core is quiescent and exactly the acknowledged target; S4 owns task quiescence.
    #[cfg(any(test, feature = "eval-harness"))]
    pub(crate) fn fork(
        bundle: &mut ProjectBundle,
        live: &Core,
        name: &str,
    ) -> Result<Self, BundleError> {
        let source = bundle.manual_branch_id().clone();
        let checkpoint = bundle.checkpoint(&source)?.clone();
        let frozen = bundle.branch_history(&source)?;
        if live
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
            != *frozen
        {
            return Err(BundleError::Unsupported(
                "live Core is not the durable base".into(),
            ));
        }
        let base = SpecialistBaseRecord {
            target_branch: source.clone(),
            base_head: checkpoint.head_id,
            base_document_hash: checkpoint.document_hash,
            original_baseline_ref: checkpoint.document_ref,
            frozen_checkpoint_ref: bundle.checkpoint_reference(&source)?,
            full_applied_delta: vec![],
        };
        let pin = bundle
            .store()
            .pin(std::slice::from_ref(&base.frozen_checkpoint_ref))?;
        let branch_id = bundle.fork_branch(&source)?;
        let (revision, _) = snapshot(live)?;
        let branch = TimelineBranch::new(name, revision, std::sync::Arc::clone(&frozen.document))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        bundle.attach_core(&branch.core())?;
        Ok(Self {
            branch_id,
            branch,
            base,
            baseline_delta: vec![],
            pin: Some(pin),
            registration: None,
        })
    }
    #[cfg(feature = "eval-harness")]
    pub fn fork_probe(
        bundle: &mut ProjectBundle,
        live: &Core,
        name: &str,
    ) -> Result<Self, BundleError> {
        Self::fork(bundle, live, name)
    }
    #[cfg(feature = "eval-harness")]
    pub fn reopen_probe(
        bundle: &ProjectBundle,
        branch: BranchId,
        base: &ObjectRef,
        name: &str,
    ) -> Result<Self, BundleError> {
        Self::reopen(bundle, branch, base, name)
    }
    #[must_use]
    pub fn core(&self) -> Core {
        self.branch.core()
    }
    #[must_use]
    pub const fn branch_id(&self) -> &BranchId {
        &self.branch_id
    }
    #[must_use]
    pub const fn base(&self) -> &SpecialistBaseRecord {
        &self.base
    }

    /// Store the full applied delta and its original base independently of manual floors.
    pub fn persist(&mut self, bundle: &mut ProjectBundle) -> Result<ObjectRef, BundleError> {
        let frozen = self
            .branch
            .core()
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let mut full_delta = self.baseline_delta.clone();
        full_delta.extend(frozen.applied_operations());
        bundle.save_branch(&self.branch_id, &self.branch.core())?;
        self.base.full_applied_delta = full_delta;
        self.baseline_delta = specialist_floor_delta(
            bundle,
            &self.base,
            &self
                .branch
                .core()
                .export_history()
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        )?;
        let reference = bundle.retain_specialist_base(&self.base)?;
        self.pin = Some(bundle.store().pin(&[
            reference.clone(),
            bundle.checkpoint_reference(&self.branch_id)?,
        ])?);
        Ok(reference)
    }
    #[cfg(any(test, feature = "eval-harness"))]
    pub(crate) fn reopen(
        bundle: &ProjectBundle,
        branch_id: BranchId,
        base_ref: &ObjectRef,
        name: &str,
    ) -> Result<Self, BundleError> {
        let base = bundle
            .store()
            .read_record::<SpecialistBaseRecord>(base_ref)?;
        let checkpoint = bundle
            .store()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &base.frozen_checkpoint_ref,
            )?;
        if checkpoint.branch_id != base.target_branch
            || checkpoint.head_id != base.base_head
            || checkpoint.document_hash != base.base_document_hash
            || base.original_baseline_ref != checkpoint.document_ref
        {
            return Err(BundleError::Corrupt(
                "specialist base identity mismatch".into(),
            ));
        }
        let baseline = bundle
            .store()
            .read_record::<Document>(&base.original_baseline_ref)?;
        let history = bundle.branch_history(&branch_id)?;
        let baseline_delta = specialist_floor_delta(bundle, &base, &history)?;
        let branch = TimelineBranch::new(name, TimelineRevision(0), Arc::new((*baseline).clone()))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        bundle
            .attach_core(&branch.core())?
            .restore_history((*history).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let pin = bundle
            .store()
            .pin(&[base_ref.clone(), bundle.checkpoint_reference(&branch_id)?])?;
        Ok(Self {
            branch_id,
            branch,
            base: (*base).clone(),
            baseline_delta,
            pin: Some(pin),
            registration: None,
        })
    }
    pub fn fork_registered(
        session: &mut kinewright_project::editing_session::Ja1EditingSession,
        name: &str,
    ) -> Result<Self, BundleError> {
        let registration = session.fork_durable_branch(name)?;
        let baseline = session.durable_branch_baseline(&registration)?;
        let branch = TimelineBranch::from_registered(name, baseline, registration.core().clone());
        Ok(Self {
            branch_id: registration.branch_id().clone(),
            base: registration.base().clone(),
            branch,
            baseline_delta: vec![],
            pin: None,
            registration: Some(registration),
        })
    }
    pub fn reopen_registered(
        session: &mut kinewright_project::editing_session::Ja1EditingSession,
        branch_id: BranchId,
        base_ref: &ObjectRef,
        name: &str,
    ) -> Result<Self, BundleError> {
        let registration = session.reopen_durable_branch(branch_id.clone(), base_ref, name)?;
        let core = registration.core().clone();
        let base = registration.base().clone();
        let baseline = session.durable_branch_baseline(&registration)?;
        let history = session.durable_branch_history(&registration)?;
        let baseline_delta = specialist_floor_delta_from_baseline(&base, &history, &baseline)?;
        let branch = TimelineBranch::from_registered(name, baseline, core);
        Ok(Self {
            branch_id,
            branch,
            base,
            baseline_delta,
            pin: None,
            registration: Some(registration),
        })
    }
    pub fn persist_registered(
        &mut self,
        session: &mut kinewright_project::editing_session::Ja1EditingSession,
    ) -> Result<ObjectRef, BundleError> {
        let registration = self
            .registration
            .as_mut()
            .ok_or_else(|| BundleError::Identity("unregistered durable session".into()))?;
        let reference = session.persist_durable_branch(registration)?;
        self.base = registration.base().clone();
        let history = session.durable_branch_history(registration)?;
        self.baseline_delta = specialist_floor_delta_from_baseline(
            &self.base,
            &history,
            &*session.durable_branch_baseline(registration)?,
        )?;
        Ok(reference)
    }
    pub fn close_registered(
        &mut self,
        session: &mut kinewright_project::editing_session::Ja1EditingSession,
    ) -> Result<(), BundleError> {
        self.persist_registered(session)?;
        session.close_durable_branch(
            self.registration
                .as_ref()
                .ok_or_else(|| BundleError::Identity("unregistered durable session".into()))?,
        )
    }
    fn bind(&self, bundle: &ProjectBundle, live: &Core) -> Result<TimelineRevision, BundleError> {
        let checkpoint = bundle.checkpoint(&self.base.target_branch)?;
        if checkpoint.branch_id != self.base.target_branch
            || checkpoint.head_id != self.base.base_head
            || checkpoint.document_hash != self.base.base_document_hash
        {
            return Err(BundleError::Unsupported("durable base conflict".into()));
        }
        let current = live
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if current != *bundle.branch_history(&self.base.target_branch)? {
            return Err(BundleError::Unsupported(
                "runtime Core is not the bound durable head".into(),
            ));
        }
        Ok(snapshot(live)?.0)
    }
    pub fn merge_envelope(
        &self,
        bundle: &ProjectBundle,
        live: &Core,
        id: CommandId,
    ) -> Result<CommandEnvelope, BundleError> {
        let revision = self.bind(bundle, live)?;
        let frozen = self
            .branch
            .core()
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let mut full_delta = self.baseline_delta.clone();
        full_delta.extend(frozen.applied_operations());
        if full_delta != self.base.full_applied_delta {
            return Err(BundleError::Unsupported(
                "specialist changed after freezing".into(),
            ));
        }
        Ok(CommandEnvelope {
            switch: None,
            manual: None,
            version: 1,
            action: "edit".into(),
            id,
            project_id: bundle.project_id().clone(),
            target_branch: self.base.target_branch.clone(),
            owner_epoch: bundle.store().owner_epoch(),
            lease: None,
            expected_head: Some(self.base.base_head.clone()),
            expected_revision: revision,
            command: Some(JournalCommand::DoBatch(
                self.base.full_applied_delta.clone(),
            )),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        })
    }
    /// Lookup the original result before rebinding; a completed merge has already changed its base head.
    pub fn merge(
        &self,
        bundle: &mut ProjectBundle,
        live: &Core,
        envelope: &CommandEnvelope,
    ) -> Result<ReceiptLookup, BundleError> {
        let previous = bundle.receipt_lookup(envelope)?;
        if previous != ReceiptLookup::Absent {
            return Ok(previous);
        }
        let revision = self.bind(bundle, live)?;
        if envelope.expected_revision != revision
            || envelope.expected_head.as_ref() != Some(&self.base.base_head)
            || envelope.target_branch != self.base.target_branch
            || envelope.command
                != Some(JournalCommand::DoBatch(
                    self.base.full_applied_delta.clone(),
                ))
        {
            return Err(BundleError::Unsupported(
                "merge envelope differs from frozen durable binding".into(),
            ));
        }
        let inputs = bundle
            .checkpoint(&self.base.target_branch)?
            .input_manifest_ref
            .clone();
        bundle.command(envelope, live, &inputs, &[])
    }
}

/// The original base and full proposal are independent of the bounded undo checkpoint.
use kinewright_project::editing_session::specialist_floor_delta_from_baseline;
fn specialist_floor_delta(
    bundle: &ProjectBundle,
    base: &SpecialistBaseRecord,
    history: &kinewright_core::HistorySnapshot,
) -> Result<Vec<kinewright_core::Operation>, BundleError> {
    let baseline = bundle
        .store()
        .read_record::<Document>(&base.original_baseline_ref)?;
    specialist_floor_delta_from_baseline(base, history, &baseline)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "registered creation, reopen, caller drop, base and fenced close in one owner"
    )]
    fn w13_registered_session_blocks_branch_retirement() {
        use super::*;
        use kinewright_project::{
            bundle::BundleWriteAdmission, editing_session::Ja1EditingSession,
        };
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/c5-test-roots")
                .as_path(),
            "c5-registered",
        );
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
        let mut owner = Ja1EditingSession::activate(
            &temp.path("session.kinewright"),
            BundleWriteAdmission::new_document(),
            &core,
            None,
        )
        .unwrap();
        owner.save_active().unwrap();
        let mut branch =
            DurableTimelineBranch::fork_registered(&mut owner, "specialist X").unwrap();
        let id = branch.branch_id().clone();
        let edit = |core: &Core, locked| {
            assert!(matches!(
                core.request(Command::Do(kinewright_core::Operation::SetTrackSyncLock {
                    track: kinewright_core::TrackId(1),
                    locked
                }))
                .unwrap(),
                Event::DocumentChanged { .. }
            ));
        };
        edit(&branch.core(), false);
        let history = branch.core().export_history().unwrap();
        let root = owner.root_sha256().unwrap();
        let refusal = owner.retire_working_branch(&id, &root, true).unwrap_err();
        assert!(refusal.to_string().contains("specialist X"));
        assert_eq!(owner.root_sha256().unwrap(), root);
        assert_eq!(branch.core().export_history().unwrap(), history);
        let base = branch.persist_registered(&mut owner).unwrap();
        assert!(!owner.retirement_candidates().unwrap().contains(&id));
        let original_base = branch.base().target_branch.clone();
        let target = owner.list_recovery_points().unwrap()[0].clone();
        owner.prepare_recovery(target).unwrap();
        assert!(matches!(
            owner.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ));
        let root = owner.root_sha256().unwrap();
        let refusal = owner
            .retire_working_branch(&original_base, &root, true)
            .unwrap_err();
        assert!(refusal.to_string().contains("specialist X"));
        assert_eq!(owner.root_sha256().unwrap(), root);
        branch.close_registered(&mut owner).unwrap();
        let mut reopened =
            DurableTimelineBranch::reopen_registered(&mut owner, id.clone(), &base, "reopened X")
                .unwrap();
        edit(&reopened.core(), true);
        let root = owner.root_sha256().unwrap();
        assert!(owner.retire_working_branch(&id, &root, true).is_err());
        let latest_base = reopened.persist_registered(&mut owner).unwrap();
        reopened.close_registered(&mut owner).unwrap();
        let dropped = DurableTimelineBranch::reopen_registered(
            &mut owner,
            id.clone(),
            &latest_base,
            "dropped caller X",
        )
        .unwrap();
        let retained_core = dropped.core();
        edit(&retained_core, false);
        let retained_history = retained_core.export_history().unwrap();
        drop(dropped);
        assert!(
            owner
                .retire_working_branch(&id, &owner.root_sha256().unwrap(), true)
                .is_err()
        );
        let mut retained_guard = owner
            .durable_sessions()
            .into_iter()
            .find(|(_, guard)| guard.branch_id() == &id)
            .unwrap()
            .1;
        owner.persist_durable_branch(&mut retained_guard).unwrap();
        assert_eq!(retained_core.export_history().unwrap(), retained_history);
        owner.close_durable_branch(&retained_guard).unwrap();
        assert_eq!(
            retained_core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Sealed
        );
        assert!(owner.retirement_candidates().unwrap().contains(&id));
        owner
            .retire_working_branch(&id, &owner.root_sha256().unwrap(), true)
            .unwrap();
        assert!(
            DurableTimelineBranch::reopen_registered(&mut owner, id, &base, "retired X").is_err()
        );
    }

    #[cfg(all(target_os = "linux", feature = "eval-harness"))]
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "a live specialist keeps its original namespace and floor across manual rescue"
    )]
    fn w13_registered_session_keeps_original_namespace_after_rescue() {
        for floor in [false, true] {
            use super::*;
            use kinewright_core::{AttachmentState, Operation, Track, TrackId, TrackKind};
            use kinewright_project::{
                bundle::LoadedBundle, editing_session::Ja1EditingSession, io_observer,
                storage_budget::StorageLimits,
            };
            let temp = kinewright_media::test_support::TempDirectory::new_in(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/c5-test-roots")
                    .as_path(),
                "c5-session-namespace",
            );
            let private_root = std::fs::canonicalize(temp.root()).unwrap();
            let a = private_root.join("drive/a.kinewright");
            let b = private_root.join("b.kinewright");
            let core = Core::spawn(Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            })
            .unwrap();
            let cost = kinewright_project::bundle_commit::history_cost_probe(
                &core.export_history().unwrap(),
                &StorageLimits::default(),
            )
            .unwrap();
            let profile = StorageLimits {
                branch_history_bytes: if floor {
                    cost * 4
                } else {
                    StorageLimits::default().branch_history_bytes
                },
                ..StorageLimits::default()
            };
            let mut owner =
                Ja1EditingSession::new(&private_root, &core, StorageLimits::default()).unwrap();
            owner.first_save(&a, profile.clone()).unwrap();
            let mut specialist =
                DurableTimelineBranch::fork_registered(&mut owner, "original A specialist")
                    .unwrap();
            let x = specialist.branch_id().clone();
            let mut stale_guard = owner
                .durable_sessions()
                .into_iter()
                .find(|(_, guard)| guard.branch_id() == &x)
                .unwrap()
                .1;
            let worker = specialist.core();
            let edit = |actor: &Core, locked| {
                assert!(matches!(
                    actor
                        .request(Command::Do(Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked
                        }))
                        .unwrap(),
                    Event::DocumentChanged { .. }
                ));
            };
            for n in 0..if floor { 40 } else { 1 } {
                edit(&worker, if floor { n % 2 == 0 } else { false });
            }
            assert!(
                io_observer::fail_once("root_parent_sync", || specialist
                    .persist_registered(&mut owner))
                .is_err()
            );
            if floor {
                assert!(
                    worker.pending_history_save().unwrap().is_some(),
                    "real over-allowance own ticket"
                );
            }
            let attempted_proposal = worker.export_history().unwrap().applied_operations();
            edit(&worker, true);
            let full = worker.export_history().unwrap();
            let full_proposal = full.applied_operations();
            std::fs::rename(private_root.join("drive"), private_root.join("hidden-a")).unwrap();
            let offer = owner.probe_active().unwrap().unwrap();
            let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
            let selection = owner
                .preview_rescue(capture, StorageLimits::default())
                .unwrap();
            let consent = selection.consent();
            let destination = owner.inspect_rescue_destination(&b).unwrap();
            assert!(
                owner
                    .rescue(selection, &consent, &destination)
                    .unwrap()
                    .activated
            );
            let root = owner.root_sha256().unwrap();
            assert!(
                specialist.persist_registered(&mut owner).is_err(),
                "an A guard cannot write B while A is unavailable"
            );
            assert_eq!(owner.root_sha256().unwrap(), root);
            assert!(
                owner
                    .close_disclosure()
                    .unwrap()
                    .contains("original A specialist")
            );
            assert_eq!(worker.export_history().unwrap(), full);
            edit(owner.core(), false);
            owner.save_active().unwrap();
            let b_history = owner.core().export_history().unwrap();
            let b_root = owner.root_sha256().unwrap();
            std::fs::rename(private_root.join("hidden-a"), private_root.join("drive")).unwrap();
            let readable_a = LoadedBundle::read_with_limits_probe(&a, profile).unwrap();
            let manifest_ref = readable_a.root().unwrap().store_manifest_ref.clone();
            let manifest: kinewright_project::store::StoreManifest = serde_json::from_slice(
                &std::fs::read(private_root.join(format!(
                    "drive/a.kinewright-assets/studio/objects/{}",
                    manifest_ref.sha256
                )))
                .unwrap(),
            )
            .unwrap();
            assert!(
                manifest
                    .durable_pins
                    .iter()
                    .filter(|r| r.record_kind
                        == kinewright_project::bundle_records::RecordKind::SpecialistBase)
                    .any(|r| {
                        let record: SpecialistBaseRecord = serde_json::from_slice(
                            &std::fs::read(private_root.join(format!(
                                "drive/a.kinewright-assets/studio/objects/{}",
                                r.sha256
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        record.full_applied_delta == attempted_proposal
                    }),
                "the branch checkpoint and complete proposal publish in the same root"
            );
            drop(readable_a);
            owner.reconcile_retained_destination(&a).unwrap();
            assert_eq!(
                worker.export_history().unwrap().floor_offset > full.floor_offset,
                floor
            );
            assert!(worker.pending_history_save().unwrap().is_none());
            assert_eq!(owner.root_sha256().unwrap(), b_root);
            assert_eq!(owner.core().export_history().unwrap(), b_history);
            let base = specialist.persist_registered(&mut owner).unwrap();
            assert_eq!(specialist.base().full_applied_delta, full_proposal);
            owner.persist_durable_branch(&mut stale_guard).unwrap();
            assert_eq!(stale_guard.base().full_applied_delta, full_proposal);
            assert_eq!(owner.root_sha256().unwrap(), b_root);
            owner.retire_working_branch(&x, &b_root, true).unwrap();
            let after_retirement = owner.root_sha256().unwrap();
            specialist.close_registered(&mut owner).unwrap();
            assert_eq!(owner.root_sha256().unwrap(), after_retirement);
            assert_eq!(worker.attachment_state().unwrap(), AttachmentState::Sealed);
            assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
            drop(owner);
            let mut original = Ja1EditingSession::open(&a).unwrap();
            let reopened = DurableTimelineBranch::reopen_registered(
                &mut original,
                x,
                &base,
                "reopened A specialist",
            )
            .unwrap();
            assert_eq!(reopened.base().full_applied_delta, full_proposal);
            let loaded = LoadedBundle::read(&b).unwrap();
            let independent = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&independent).unwrap();
            assert_eq!(independent.export_history().unwrap(), b_history);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one end-to-end shared-prefix and floor identity oracle"
    )]
    fn w13_shared_prefix_occurrence_identity_survives_branch_saves() {
        use super::*;
        use kinewright_core::*;
        use kinewright_media::test_support::TempDirectory;
        use kinewright_project::{
            bundle::{BundleWriteAdmission, LoadedBundle},
            bundle_recovery::RecoverySelection,
            bundle_switch::ProjectSelectionResult,
        };
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/c4-test-roots")
                .as_path(),
            "c4-specialist",
        );
        let path = temp.path("branches.kinewright");
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let apply = |core: &Core, command| {
            assert!(matches!(
                core.request(command).unwrap(),
                Event::DocumentChanged { .. }
            ));
        };
        apply(
            &core,
            Command::Do(Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode(0),
                duration: TimeCode(30),
                color: SolidColor::default(),
            }),
        );
        apply(
            &core,
            Command::Do(Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(15),
            }),
        );
        apply(&core, Command::Undo);
        apply(&core, Command::Redo);
        let mut bundle =
            ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &inputs).unwrap();
        let left = bundle.manual_branch_id().clone();
        let original = (*bundle.branch_lineage(&left).unwrap()).clone();
        let ids = bundle.checkpoint(&left).unwrap().batch_ids.clone();
        let mut specialist = DurableTimelineBranch::fork(&mut bundle, &core, "writable").unwrap();
        apply(
            &specialist.core(),
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
        );
        specialist.persist(&mut bundle).unwrap();
        assert_eq!(
            bundle
                .branch_lineage(specialist.branch_id())
                .unwrap()
                .baseline,
            original.current
        );
        let manifest = bundle
            .store()
            .read_record::<kinewright_project::store::StoreManifest>(
                &bundle.root().unwrap().store_manifest_ref,
            )
            .unwrap();
        let target = manifest.recovery.predecessors[0].clone();
        drop(manifest);
        let mut prepared = bundle
            .prepare_project_selection(
                &core,
                RecoverySelection::Checkpoint {
                    root: target,
                    branch: left.clone(),
                },
            )
            .unwrap();
        assert!(matches!(
            bundle.publish_project_selection(&mut prepared, &core),
            ProjectSelectionResult::Activated(_)
        ));
        let right = prepared.target_branch().clone();
        let active = prepared.core();
        apply(
            &active,
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
        );
        bundle.save(&active, &inputs).unwrap();
        assert_eq!(
            &bundle.checkpoint(&right).unwrap().batch_ids[..ids.len()],
            &ids
        );
        let restored = bundle.branch_lineage(&right).unwrap();
        assert_eq!(restored.current, original.current);
        assert_eq!(restored.split_ancestry, original.split_ancestry);
        drop(restored);
        let history = active.export_history().unwrap();
        bundle
            .attach_core(&active)
            .unwrap()
            .install_history_transition(HistoryTransition::floor(history, 1).unwrap())
            .unwrap();
        bundle.save(&active, &inputs).unwrap();
        let floored = bundle.branch_lineage(&right).unwrap();
        assert_eq!(floored.baseline, original.undo[1]);
        assert_eq!(floored.current, original.current);
        assert_eq!(floored.split_ancestry, original.split_ancestry);
        drop(floored);
        let loaded = LoadedBundle::read(&path).unwrap();
        assert_eq!(loaded.root().unwrap().manual_branch_id, right);
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(
            fresh.export_history().unwrap(),
            active.export_history().unwrap()
        );
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn w13_shared_prefix_occurrence_identity_survives_branch_saves() {
        let root = std::env::temp_dir().join("ja1-c4-refusal.kinewright");
        assert!(matches!(
            kinewright_project::bundle::ProjectBundle::activate(
                &root,
                kinewright_project::bundle::BundleWriteAdmission::new_document()
            ),
            Err(kinewright_project::bundle::BundleError::DurabilityUnsupported)
        ));
    }
}
