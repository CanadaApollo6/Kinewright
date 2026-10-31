#![cfg(target_os = "linux")]
use super::*;
use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_copy::SaveAsOutcome,
    bundle_records::{ObjectRef, RecordKind},
    bundle_recovery::RecoverySelection,
    editing_session::Ja1EditingSession,
    io_observer,
    receipts::{CommandReceipt, ReceiptLookup, ReceiptResult},
    storage_budget::StorageLimits,
    store::SealedObject,
};
use std::path::Path;
fn run_record(run_id: RunId) -> RunRecord {
    RunRecord {
        run_id: run_id.clone(),
        projection: view(run_id),
        snapshot_refs: vec![],
        event_segment_refs: vec![],
        effect_refs: vec![],
        request_refs: vec![],
        delivery_refs: vec![],
        result_refs: vec![],
        usage_refs: vec![],
    }
}
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let mut document = Document::default();
    document.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    let core = Core::spawn(document).unwrap();
    let mut b = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = b.empty_inputs().unwrap();
    let r = inputs.reference.clone();
    b.save(&core, &r).unwrap();
    (core, b, r)
}
fn envelope(
    b: &ProjectBundle,
    issuer: &kinewright_project::receipts::IssuerId,
    sequence: u64,
) -> MetadataEnvelope {
    MetadataEnvelope {
        id: CommandId {
            issuer: issuer.clone(),
            sequence,
        },
        project_id: b.project_id().clone(),
        target_branch: b.manual_branch_id().clone(),
        owner_epoch: b.store().owner_epoch(),
        expected_head: b.manual_head().cloned(),
    }
}
fn core_revision(core: &Core) -> kinewright_core::TimelineRevision {
    let Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision, .. }) = core
        .request(Command::Query(kinewright_core::Query::Snapshot))
        .unwrap()
    else {
        panic!("snapshot")
    };
    revision
}
fn committed(lookup: ReceiptLookup) -> CommandReceipt {
    match lookup {
        ReceiptLookup::Committed(r) | ReceiptLookup::Retired(r) => r,
        r => panic!("unexpected {r:?}"),
    }
}
fn update(b: &ProjectBundle, change: MetadataChange) -> MetadataUpdate {
    MetadataUpdate {
        expected_root_generation: b.root().unwrap().generation,
        change,
    }
}
fn blob(b: &mut ProjectBundle, bytes: &[u8]) -> SealedObject {
    let r = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    b.store_mut()
        .unwrap()
        .admit_blob(&mut &*bytes, &r, &[])
        .unwrap()
}
fn snapshot(b: &mut ProjectBundle) -> SnapshotKey {
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(b, &issuer, 1);
    let branch = b.manual_branch_id().clone();
    let u = update(
        b,
        MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: b.manual_head().unwrap().clone(),
            checkpoint_ref: b.checkpoint_reference(&branch).unwrap(),
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = committed(b.metadata_command(&e, MetadataScope::Snapshot, &u).unwrap()).result
    else {
        panic!("snapshot result")
    };
    key
}
fn feedback(b: &mut ProjectBundle, key: SnapshotKey) -> (FeedbackRevision, Vec<SealedObject>) {
    let text = blob(b, b"original feedback");
    let strokes = blob(b, b"[[0.25,0.75]]");
    let proof = blob(b, b"exact displayed frame bytes");
    let r = FeedbackRevision {
        comment_id: CommentId::fresh().unwrap(),
        revision: 1,
        predecessor: None,
        anchor: FeedbackAnchor {
            snapshot: key,
            frame: 17,
            fps: FrameRate {
                numerator: 30000,
                denominator: 1001,
            },
            raster_size: [1920, 1080],
            orientation: Orientation::Upright,
            target: None,
            source_ref: None,
            script_beat_id: None,
        },
        original_text_ref: text.reference.clone(),
        strokes_ref: strokes.reference.clone(),
        frame_proof_ref: proof.reference.clone(),
        voice_ref: None,
        transcript_revision: None,
    };
    (r, vec![text, strokes, proof])
}
fn save_feedback(b: &mut ProjectBundle, r: FeedbackRevision) -> ObjectRef {
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(b, &issuer, 1);
    let u = update(b, MetadataChange::SaveFeedback(r));
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(r),
    } = committed(b.metadata_command(&e, MetadataScope::Feedback, &u).unwrap()).result
    else {
        panic!("feedback result")
    };
    r
}
fn seed(b: &mut ProjectBundle) -> (SnapshotKey, ObjectRef, FeedbackBatchId, RunId) {
    let key = snapshot(b);
    let (revision, _blobs) = feedback(b, key.clone());
    let revision_ref = save_feedback(b, revision);
    let batch_id = FeedbackBatchId::fresh().unwrap();
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(b, &issuer, 1);
    let u = update(
        b,
        MetadataChange::SubmitBatch {
            batch: FeedbackBatch {
                batch_id: batch_id.clone(),
                revision_refs: vec![revision_ref.clone()],
            },
            request: ReviseRequest {
                command_id: e.id.clone(),
                batch_id: batch_id.clone(),
                expected_candidate_head: key.head.clone(),
            },
        },
    );
    committed(b.metadata_command(&e, MetadataScope::Feedback, &u).unwrap());
    let run = RunId::fresh().unwrap();
    let mut record = run_record(run.clone());
    let event = RunEventSegment {
        version: 1,
        run_id: run.clone(),
        events: vec![RunEvent {
            run_id: run.clone(),
            sequence: 1,
            delta: RunViewDelta {
                view: record.projection.clone(),
            },
        }],
    };
    let segment = b.admit_orchestration_record(&event).unwrap();
    record.event_segment_refs.push(segment.reference.clone());
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(b, &issuer, 1);
    let u = update(
        b,
        MetadataChange::Orchestration {
            runs: vec![record],
            consume_batch: Some((batch_id.clone(), run.clone())),
        },
    );
    let validated = b.validate_run_update(&e, &u).unwrap();
    let receipt = b.commit_orchestration(&e, validated).unwrap();
    assert_eq!(
        receipt.consumed_batch,
        Some((batch_id.clone(), run.clone())),
        "S3_ORCHESTRATION_RECEIPT"
    );
    assert_eq!(
        b.read_submitted_batch(&batch_id).unwrap().consumed_by,
        Some(run.clone()),
        "S3_ATOMIC_RUN_BATCH"
    );
    (key, revision_ref, batch_id, run)
}
fn assert_retained(
    path: &Path,
    key: &SnapshotKey,
    revision: &ObjectRef,
    batch: &FeedbackBatchId,
    run: &RunId,
    expected_root: &kinewright_project::bundle::StudioRoot,
) {
    let loaded = LoadedBundle::read(path).unwrap();
    let root = loaded.root().unwrap();
    assert_eq!(
        root.version_index_ref, expected_root.version_index_ref,
        "S3_EXACT_VERSION_INDEX"
    );
    let copied = root.project_id != expected_root.project_id;
    if copied {
        assert_ne!(
            root.feedback_index_ref, expected_root.feedback_index_ref,
            "S3_COPY_DETACHES_RUN_CONSUMPTION"
        );
    } else {
        assert_eq!(
            root.feedback_index_ref, expected_root.feedback_index_ref,
            "S3_EXACT_FEEDBACK_INDEX"
        );
    }
    if copied {
        assert!(root.run_index_ref.is_none(), "S3_COPY_HAS_NO_INHERITED_RUN");
    } else {
        assert_eq!(
            root.run_index_ref, expected_root.run_index_ref,
            "S3_EXACT_RUN_INDEX"
        );
    }
    assert!(
        root.version_index_ref.is_some()
            && root.feedback_index_ref.is_some()
            && (copied || root.run_index_ref.is_some()),
        "S3_ROOT_INDEX_PRESERVATION"
    );
    drop(loaded);
    let mut b = ProjectBundle::activate(path, BundleWriteAdmission::read(path).unwrap()).unwrap();
    assert_eq!(
        b.read_snapshot(key).unwrap().descriptor.key,
        *key,
        "S3_SNAPSHOT_IDENTITY"
    );
    let feedback = b.read_feedback(revision).unwrap();
    assert_eq!(feedback.revision.anchor.frame, 17);
    assert_eq!(
        &*b.store()
            .read_evidence(&feedback.revision.original_text_ref)
            .unwrap(),
        b"original feedback",
        "S3_FEEDBACK_BYTES"
    );
    let proof = b.store().read_evidence(&feedback.revision.frame_proof_ref);
    assert!(proof.is_ok(), "S3_FRAME_PROOF_RETAINED");
    assert_eq!(
        &*proof.unwrap(),
        b"exact displayed frame bytes",
        "S3_FRAME_PROOF_BYTES"
    );
    let submitted = b.read_submitted_batch(batch).unwrap();
    assert_eq!(submitted.batch.revision_refs, vec![revision.clone()]);
    assert_eq!(
        submitted.consumed_by.as_ref(),
        if copied { None } else { Some(run) },
        "S3_RUN_BATCH_PRESERVATION"
    );
    let index = b.run_index().unwrap();
    if copied {
        assert!(
            index.runs.is_empty() && index.orchestration.is_empty(),
            "S3_COPY_HAS_NO_RUN_BINDINGS"
        );
    } else {
        let record = &index.runs[run];
        let segment = b
            .store()
            .read_record::<RunEventSegment>(&record.event_segment_refs[0])
            .unwrap();
        assert_eq!(segment.run_id, *run, "S3_RUN_GRAPH_PRESERVATION");
    }
    b.reconcile_metadata().unwrap();
}
fn index_bytes(path: &Path, root: &kinewright_project::bundle::StudioRoot) -> Vec<Option<Vec<u8>>> {
    [
        &root.version_index_ref,
        &root.feedback_index_ref,
        &root.run_index_ref,
    ]
    .into_iter()
    .map(|reference| {
        reference.as_ref().map(|reference| {
            std::fs::read(
                path.with_extension("kinewright-assets")
                    .join("studio/objects")
                    .join(&reference.sha256),
            )
            .unwrap()
        })
    })
    .collect()
}

fn assert_projection_oracle(bundle: &mut ProjectBundle, path: &Path, source: &ObjectRef) {
    use kinewright_project::bundle_recovery::verify_projection_payloads;
    let bytes = std::fs::read(
        path.with_extension("kinewright-assets")
            .join("studio/objects")
            .join(&source.sha256),
    )
    .unwrap();
    for field in ["version_index_ref", "feedback_index_ref", "run_index_ref"] {
        let mut file: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        file["studio"]
            .as_object_mut()
            .unwrap()
            .remove(field)
            .unwrap();
        let root: kinewright_project::bundle::StudioRoot =
            serde_json::from_value(file["studio"].clone()).unwrap();
        let dependencies = [
            Some(root.branch_index_ref),
            Some(root.input_manifest_ref),
            Some(root.store_manifest_ref),
            root.version_index_ref,
            root.feedback_index_ref,
            root.run_index_ref,
            root.claim_index_ref,
            root.receipt_index_ref,
            root.issuer_state_ref,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let changed = serde_json::to_vec(&file).unwrap();
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&changed),
            byte_length: changed.len() as u64,
            record_kind: RecordKind::RootCheckpoint,
        };
        let hold = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &*changed, &reference, &dependencies)
            .unwrap();
        assert!(
            matches!(
                verify_projection_payloads(bundle, source, &hold.reference),
                Err(BundleError::Corrupt(message)) if message == "L1 non-linkage payload changed"
            ),
            "S3_RESCUE_INDEX_ORACLE: {field}"
        );
    }
}
#[test]
fn s3_indexes_survive_all_root_producers() {
    let version_only_path = temp("s3-version-only");
    let version_only_root = version_only_path.path("studio.kinewright");
    let (_core, mut version_only, _inputs) = setup(&version_only_root);
    snapshot(&mut version_only);
    let expected = version_only.root().unwrap().version_index_ref.clone();
    let branch = version_only.manual_branch_id().clone();
    version_only.fork_branch(&branch).unwrap();
    assert_eq!(
        version_only.root().unwrap().version_index_ref,
        expected,
        "S3_VERSION_ONLY_CANDIDATE_PRESERVATION"
    );
    let issuer = version_only.issue_metadata_issuer().unwrap();
    let run_update = update(
        &version_only,
        MetadataChange::Orchestration {
            runs: vec![run_record(RunId::fresh().unwrap())],
            consume_batch: None,
        },
    );
    let command = envelope(&version_only, &issuer, 1);
    let validated = version_only
        .validate_run_update(&command, &run_update)
        .unwrap();
    version_only
        .commit_orchestration(&command, validated)
        .unwrap();
    let expected_run = version_only.root().unwrap().run_index_ref.clone();
    version_only.fork_branch(&branch).unwrap();
    assert_eq!(
        version_only.root().unwrap().run_index_ref,
        expected_run,
        "S3_RUN_ONLY_CANDIDATE_PRESERVATION"
    );
    drop(version_only);
    let tmp = temp("s3-index-producers");
    let drive = tmp.path("drive");
    std::fs::create_dir(&drive).unwrap();
    let path = drive.join("studio.kinewright");
    let (core, mut b, inputs) = setup(&path);
    let (key, revision, batch, run) = seed(&mut b);
    assert!(
        matches!(
            kinewright_project::bundle_recovery::without_api_probe("orchestration", || {
                LoadedBundle::read(&path)
            }),
            Err(BundleError::Unsupported(_))
        ),
        "S3_ORCHESTRATION_REQUIRED_SUPPORT"
    );

    let old = b.root().unwrap().clone();
    b.save(&core, &inputs).unwrap();
    assert_eq!(
        b.root().unwrap().version_index_ref,
        old.version_index_ref,
        "S3_SHARED_PUBLISHER_PRESERVES_VERSION"
    );
    let branch = b.manual_branch_id().clone();
    let candidate = b.fork_branch(&branch).unwrap();
    let candidate_core =
        Core::spawn((*b.branch_history(&candidate).unwrap().document).clone()).unwrap();
    assert!(matches!(
        candidate_core
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    b.save_branch(&candidate, &candidate_core).unwrap();
    assert_eq!(
        b.root().unwrap().version_index_ref,
        old.version_index_ref,
        "S3_CANDIDATE_VERSION_PRESERVATION"
    );
    assert_eq!(
        b.root().unwrap().run_index_ref,
        old.run_index_ref,
        "S3_CANDIDATE_RUN_PRESERVATION"
    );
    assert_eq!(
        b.root().unwrap().feedback_index_ref,
        old.feedback_index_ref,
        "S3_CANDIDATE_PRESERVATION"
    );
    let issuer = b.issue_issuer(&core, &inputs, 1).unwrap();
    let mut edit = kinewright_project::receipts::CommandEnvelope {
        switch: None,
        manual: None,
        version: 1,
        action: "edit".into(),
        id: CommandId {
            issuer,
            sequence: 1,
        },
        project_id: b.project_id().clone(),
        target_branch: b.manual_branch_id().clone(),
        owner_epoch: b.store().owner_epoch(),
        lease: None,
        expected_head: b.manual_head().cloned(),
        expected_revision: core_revision(&core),
        command: Some(kinewright_core::JournalCommand::Do(
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            },
        )),
        comment_text: None,
        comment_strokes: None,
        evidence: vec![],
        transition: None,
    };
    committed(b.command(&edit, &core, &inputs, &[]).unwrap());
    let checkpoint = b.checkpoint(b.manual_branch_id()).unwrap().clone();
    edit.action = "floor".into();
    edit.command = None;
    edit.transition = Some(kinewright_project::receipts::HistoryRequest::Floor {
        discard_batches: 1,
        preceding_checkpoint_generation: checkpoint.checkpoint_generation,
        preceding_floor_generation: checkpoint.floor_generation,
        command_cut: edit.id.clone(),
    });
    edit.id.sequence = 2;
    edit.expected_head = b.manual_head().cloned();
    edit.expected_revision = core_revision(&core);
    committed(b.command(&edit, &core, &inputs, &[]).unwrap());
    assert_eq!(
        b.root().unwrap().run_index_ref,
        old.run_index_ref,
        "S3_FLOOR_PRESERVATION"
    );
    assert_eq!(
        b.root().unwrap().feedback_index_ref,
        old.feedback_index_ref,
        "S3_FLOOR_FEEDBACK_PRESERVATION"
    );
    let ephemeral = blob(&mut b, b"reader-only run evidence");
    let mut projection = view(RunId::fresh().unwrap());
    projection
        .review_state
        .evidence_refs
        .push(ephemeral.reference.clone());
    let record = b
        .admit_orchestration_record(&RunSnapshotRecord {
            version: 1,
            view: projection,
        })
        .unwrap();
    let retained = record.reference.clone();
    let evidence = ephemeral.reference.clone();
    let reader = b.store().pin(std::slice::from_ref(&retained)).unwrap();
    drop((record, ephemeral));
    b.save(&core, &inputs).unwrap();
    b.collect_after_ack().unwrap();
    assert!(
        b.store().read_evidence(&evidence).is_ok(),
        "S3_READER_DEPENDENCY_PIN"
    );
    assert!(
        b.store()
            .read_record::<RunSnapshotRecord>(&retained)
            .is_ok(),
        "S3_READER_RECORD_PIN"
    );
    drop(reader);
    b.collect_after_ack().unwrap();
    assert!(
        !path
            .with_extension("kinewright-assets")
            .join("studio/objects")
            .join(&evidence.sha256)
            .exists(),
        "S3_READER_RETIREMENT"
    );
    let copy_path = tmp.path("copy.kinewright");
    let SaveAsOutcome::Copied(copy) = b.save_as(&copy_path).unwrap() else {
        panic!("copy")
    };
    drop(copy);
    drop(b);
    assert_retained(&path, &key, &revision, &batch, &run, &old);
    assert_retained(&copy_path, &key, &revision, &batch, &run, &old);
    let mut session = Ja1EditingSession::open(&path).unwrap();
    let mut guard = session.fork_durable_branch("candidate witness").unwrap();
    assert!(matches!(
        guard
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    session.persist_durable_branch(&mut guard).unwrap();
    let binding = session.candidate_binding(guard.branch_id()).unwrap();
    let mut specialist = session
        .fork_registered_from_candidate(&binding, "S4 candidate merge")
        .unwrap();
    assert!(matches!(
        specialist
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let frozen = session
        .freeze_registered_candidate(&mut specialist)
        .unwrap();
    let issuer = session
        .issue_manual_lease("S4 candidate".into(), "candidate-private".into())
        .unwrap()
        .issuer;
    let command = session
        .candidate_merge_envelope(
            &frozen,
            CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap();
    committed(session.commit_candidate(&frozen, &command).unwrap());
    assert!(guard.core().export_history().unwrap().document.tracks[0].sync_lock);
    let published = LoadedBundle::read(&path).unwrap();
    let published = published.root().unwrap();
    assert_eq!(
        (
            &published.version_index_ref,
            &published.feedback_index_ref,
            &published.run_index_ref,
            &published.claim_index_ref,
        ),
        (
            &old.version_index_ref,
            &old.feedback_index_ref,
            &old.run_index_ref,
            &old.claim_index_ref,
        ),
        "S3_S4_CANDIDATE_INDEX_PRESERVATION"
    );
    assert!(
        session
            .read_submitted_batch(&batch)
            .unwrap()
            .consumed_by
            .as_ref()
            == Some(&run),
        "S3_REGISTERED_CANDIDATE_PRESERVATION"
    );
    session
        .prepare_recovery(RecoverySelection::WorkingBranch(candidate))
        .unwrap();
    assert!(matches!(
        session.select_local(),
        kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(
        session.read_snapshot(&key).unwrap().descriptor.key,
        key,
        "S3_SWITCH_PRESERVATION"
    );
    drop(session);
    assert_retained(&path, &key, &revision, &batch, &run, &old);
    let mut session = Ja1EditingSession::open(&path).unwrap();
    let recovery = session
        .list_recovery_points()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    session.prepare_recovery(recovery).unwrap();
    assert!(
        matches!(
            session.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ),
        "S3_RECOVERY_ACTIVATED"
    );
    let tentative_id = CommentId::fresh().unwrap();
    let mut tentative_revision = session.read_feedback(&revision).unwrap().revision;
    tentative_revision.comment_id = tentative_id.clone();
    let issuer = session.issue_metadata_issuer().unwrap();
    let tentative_envelope = session
        .metadata_envelope(CommandId {
            issuer,
            sequence: 1,
        })
        .unwrap();
    let tentative_update = MetadataUpdate {
        expected_root_generation: session.metadata_generation().unwrap(),
        change: MetadataChange::SaveFeedback(tentative_revision),
    };
    assert!(
        io_observer::fail_once("root_parent_sync", || session.metadata_command(
            &tentative_envelope,
            MetadataScope::Feedback,
            &tentative_update
        ))
        .is_err()
    );
    std::fs::rename(&drive, tmp.path("hidden-drive")).unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let selected = session
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selected.consent();
    let rescued_drive = tmp.path("rescued-drive");
    std::fs::create_dir(&rescued_drive).unwrap();
    let rescued = rescued_drive.join("rescued.kinewright");
    let destination = session.inspect_rescue_destination(&rescued).unwrap();
    let outcome = session.rescue(selected, &consent, &destination).unwrap();
    assert!(outcome.activated, "S3_RESCUE_ACTIVATED");
    drop(session);
    assert_retained(&rescued, &key, &revision, &batch, &run, &old);
    let mut rescued_owner = LoadedBundle::read(&rescued).unwrap().into_writer().unwrap();
    assert!(
        !rescued_owner
            .feedback_index()
            .unwrap()
            .revisions
            .contains_key(&tentative_id),
        "S3_A1_TENTATIVE_EXCLUDED"
    );
    assert_eq!(
        rescued_owner
            .receipt_lookup(
                &tentative_envelope
                    .receipt_envelope(&tentative_update)
                    .unwrap()
            )
            .unwrap(),
        ReceiptLookup::UnresolvedOrigin,
        "S3_A1_TENTATIVE_RECEIPT"
    );
    let (fresh_key, fresh_revision, fresh_batch, fresh_run) = seed(&mut rescued_owner);
    let expected_root = rescued_owner.root().unwrap().clone();
    for (before, after) in [
        (&old.version_index_ref, &expected_root.version_index_ref),
        (&old.feedback_index_ref, &expected_root.feedback_index_ref),
        (&old.run_index_ref, &expected_root.run_index_ref),
    ] {
        assert_ne!(before, after, "S3_RESCUE_FRESH_INDEX_CONTROL");
    }
    let expected_bytes = index_bytes(&rescued, &expected_root);
    drop(rescued_owner);
    let mut session = Ja1EditingSession::open(&rescued).unwrap();
    let locked = !session.core().export_history().unwrap().document.tracks[0].sync_lock;
    assert!(matches!(
        session
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked,
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
    std::fs::rename(&rescued_drive, tmp.path("hidden-rescued-drive")).unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let selected = session
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selected.consent();
    let twice = tmp.path("rescued-twice.kinewright");
    let destination = session.inspect_rescue_destination(&twice).unwrap();
    assert!(
        session
            .rescue(selected, &consent, &destination)
            .expect("S3_RESCUE_OF_RESCUE_PUBLISHED")
            .activated,
        "S3_RESCUE_OF_RESCUE_ACTIVATED"
    );
    drop(session);
    let mut owner = LoadedBundle::read(&twice)
        .expect("S3_RESCUE_PROJECTED_ROOT_READABLE")
        .into_writer()
        .unwrap();
    let records = owner.rescue_records().unwrap();
    let record = records
        .iter()
        .find(|record| record.recovery_origin.as_ref().is_some_and(|o| o.rebound))
        .unwrap();
    let acknowledged = record.acknowledged_root.clone().unwrap();
    let projected: kinewright_project::ProjectFile = serde_json::from_slice(
        &std::fs::read(
            twice
                .with_extension("kinewright-assets")
                .join("studio/objects")
                .join(&acknowledged.sha256),
        )
        .unwrap(),
    )
    .unwrap();
    let projected_root = projected.studio.unwrap();
    assert_eq!(
        index_bytes(&twice, &projected_root),
        expected_bytes,
        "S3_ARCHIVED_SOURCE_INDEX_BYTES"
    );
    let destination_bytes = index_bytes(&twice, owner.root().unwrap());
    assert!(
        owner.root().unwrap().run_index_ref.is_none(),
        "S3_RESCUE_HAS_NO_INHERITED_RUN"
    );
    let manifest = owner
        .store()
        .read_record::<kinewright_project::store::StoreManifest>(&projected_root.store_manifest_ref)
        .unwrap();
    for reference in [
        &projected_root.version_index_ref,
        &projected_root.feedback_index_ref,
        &projected_root.run_index_ref,
    ] {
        let reference = reference.as_ref().expect("S3_RESCUE_NONEMPTY_INDEX");
        assert!(
            manifest.objects.contains(reference),
            "S3_RESCUE_PROJECTED_INDEX_DECLARED: {:?}",
            reference.record_kind
        );
    }
    drop(manifest);
    assert_projection_oracle(&mut owner, &twice, &acknowledged);
    assert_projection_manifest_oracle(&mut owner, &twice, &acknowledged);
    drop(owner);
    let mut session = Ja1EditingSession::open(&twice).unwrap();
    let point = session
        .list_recovery_points()
        .unwrap()
        .into_iter()
        .find(|point| matches!(point, RecoverySelection::Checkpoint { root, .. } if root == &acknowledged))
        .expect("S3_RESCUE_PROJECTED_ACK_LISTED");
    session
        .prepare_recovery(point)
        .expect("S3_RESCUE_PROJECTED_ACK_PREPARED");
    assert!(
        matches!(
            session.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ),
        "S3_RESCUE_PROJECTED_ACK_RESTORED"
    );
    drop(session);
    let restored = LoadedBundle::read(&twice).unwrap();
    assert_eq!(
        index_bytes(&twice, restored.root().unwrap()),
        destination_bytes,
        "S3_RECOVERY_SELECTION_PRESERVES_DESTINATION_INDEX_BYTES"
    );
    drop(restored);
    assert_retained(&twice, &key, &revision, &batch, &run, &expected_root);
    assert_retained(
        &twice,
        &fresh_key,
        &fresh_revision,
        &fresh_batch,
        &fresh_run,
        &expected_root,
    );
}

#[test]
fn s3_late_asr_preserves_audio_only_feedback_and_note_duration() {
    let tmp = temp("s3-late-asr");
    let path = tmp.path("studio.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let key = snapshot(&mut bundle);
    let audio = blob(&mut bundle, b"acknowledged voice audio");
    let mut voice = VoiceRef {
        note_id: NoteId::fresh().unwrap(),
        audio_ref: audio.reference.clone(),
        duration_micros: 1_500_000,
        range: Some(AudioRange {
            start_micros: 0,
            end_micros: 1_500_000,
        }),
        playhead_log_ref: None,
        original_asr_ref: None,
        chosen_transcript_revision: None,
    };
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let save = update(&bundle, MetadataChange::SaveVoice(voice.clone()));
    committed(
        bundle
            .metadata_command(
                &envelope(&bundle, &issuer, 1),
                MetadataScope::Feedback,
                &save,
            )
            .unwrap(),
    );
    let (mut revision, blobs) = feedback(&mut bundle, key);
    revision.voice_ref = Some(voice.clone());
    let original_ref = save_feedback(&mut bundle, revision.clone());
    let original_path = path
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(&original_ref.sha256);
    let original_bytes = std::fs::read(&original_path).unwrap();
    let asr = blob(&mut bundle, b"original transcription arrived later");
    voice.original_asr_ref = Some(asr.reference.clone());
    let save = update(&bundle, MetadataChange::SaveVoice(voice.clone()));
    committed(
        bundle
            .metadata_command(
                &envelope(&bundle, &issuer, 2),
                MetadataScope::Feedback,
                &save,
            )
            .expect("S3_LATE_ASR_ACKNOWLEDGED"),
    );
    bundle.save(&core, &inputs).expect("S3_LATE_ASR_PUBLISHED");
    drop((audio, asr, blobs));
    drop(bundle);
    let mut bundle = LoadedBundle::read(&path)
        .expect("S3_LATE_ASR_REOPENED")
        .into_writer()
        .unwrap();
    assert_eq!(
        bundle.read_feedback(&original_ref).unwrap().revision,
        revision,
        "S3_LATE_ASR_ORIGINAL_REVISION"
    );
    assert_eq!(
        std::fs::read(&original_path).unwrap(),
        original_bytes,
        "S3_LATE_ASR_ORIGINAL_BYTES"
    );
    assert_eq!(
        bundle.feedback_index().unwrap().notes[&voice.note_id],
        voice
    );
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let mut later = revision.clone();
    later.comment_id = CommentId::fresh().unwrap();
    save_feedback(&mut bundle, later);
    let mut with_asr = revision.clone();
    with_asr.comment_id = CommentId::fresh().unwrap();
    with_asr.voice_ref = Some(voice.clone());
    save_feedback(&mut bundle, with_asr.clone());
    let other = blob(&mut bundle, b"different ASR is never the original");
    with_asr.comment_id = CommentId::fresh().unwrap();
    with_asr.voice_ref.as_mut().unwrap().original_asr_ref = Some(other.reference.clone());
    let bad = update(&bundle, MetadataChange::SaveFeedback(with_asr));
    let e = envelope(&bundle, &issuer, 1);
    assert!(
        bundle
            .metadata_command(&e, MetadataScope::Feedback, &bad)
            .is_err(),
        "S3_LATE_ASR_MISMATCH_REFUSED"
    );
    let before = std::fs::read(&path).unwrap();
    let mut changed_revision = revision.clone();
    changed_revision.comment_id = CommentId::fresh().unwrap();
    changed_revision.voice_ref = Some(voice.clone());
    changed_revision.voice_ref.as_mut().unwrap().duration_micros += 1;
    let changed = update(&bundle, MetadataChange::SaveFeedback(changed_revision));
    assert!(
        bundle
            .metadata_command(&e, MetadataScope::Feedback, &changed)
            .is_err(),
        "C1_FEEDBACK_VOICE_DURATION_MATCHES_NOTE"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        bundle.feedback_index().unwrap().notes[&voice.note_id].duration_micros,
        voice.duration_micros
    );
    for change in ["duration", "replace_asr", "remove_asr"] {
        let mut changed = voice.clone();
        match change {
            "duration" => changed.duration_micros += 1,
            "replace_asr" => changed.original_asr_ref = Some(other.reference.clone()),
            _ => changed.original_asr_ref = None,
        }
        let bad = update(&bundle, MetadataChange::SaveVoice(changed));
        assert!(
            bundle
                .metadata_command(&e, MetadataScope::Feedback, &bad)
                .is_err(),
            "S3_IMMUTABLE_VOICE: {change}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "S3_IMMUTABLE_VOICE_REFUSAL_ROOT"
        );
    }
}
#[test]
fn s3_metadata_retry_and_admission() {
    let tmp = temp("s3-metadata-retry");
    let path = tmp.path("studio.kinewright");
    let (core, mut b, inputs) = setup(&path);
    let key = snapshot(&mut b);
    let (revision, blobs) = feedback(&mut b, key.clone());
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(&b, &issuer, 1);
    let u = update(&b, MetadataChange::SaveFeedback(revision.clone()));
    let root = std::fs::read(&path).unwrap();
    let history = core.export_history().unwrap();
    assert!(
        b.command(&e.receipt_envelope(&u).unwrap(), &core, &inputs, &[])
            .is_err(),
        "S3_TYPED_METADATA_INGRESS"
    );

    assert!(
        b.metadata_command(&e, MetadataScope::Snapshot, &u).is_err(),
        "S3_METADATA_SCOPE"
    );
    let mut wrong = e.clone();
    wrong.project_id = kinewright_project::bundle_records::ProjectId::fresh().unwrap();
    assert!(
        b.metadata_command(&wrong, MetadataScope::Feedback, &u)
            .is_err(),
        "S3_METADATA_PROJECT"
    );
    wrong = e.clone();
    wrong.owner_epoch ^= 1;
    assert!(
        b.metadata_command(&wrong, MetadataScope::Feedback, &u)
            .is_err(),
        "S3_METADATA_EPOCH"
    );
    let mut stale = u.clone();
    stale.expected_root_generation -= 1;
    assert!(
        b.metadata_command(&e, MetadataScope::Feedback, &stale)
            .is_err(),
        "S3_METADATA_GENERATION"
    );
    let mut bad_revision = revision.clone();
    bad_revision.revision = 2;
    stale = update(&b, MetadataChange::SaveFeedback(bad_revision));
    assert!(
        b.metadata_command(&e, MetadataScope::Feedback, &stale)
            .is_err(),
        "S3_METADATA_REVISION"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        root,
        "S3_METADATA_REFUSAL_ROOT"
    );
    assert_eq!(core.export_history().unwrap(), history);
    assert!(
        io_observer::fail_once("root_parent_sync", || b.metadata_command(
            &e,
            MetadataScope::Feedback,
            &u
        ))
        .is_err()
    );
    drop(blobs);
    drop(b);
    let mut b = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let original = committed(b.metadata_command(&e, MetadataScope::Feedback, &u).unwrap());
    assert_eq!(
        committed(b.metadata_command(&e, MetadataScope::Feedback, &u).unwrap()),
        original,
        "S3_METADATA_REPLAY"
    );
    let mut changed = u.clone();
    if let MetadataChange::SaveFeedback(r) = &mut changed.change {
        r.anchor.frame += 1;
    }
    assert_eq!(
        b.metadata_command(&e, MetadataScope::Feedback, &changed)
            .unwrap(),
        ReceiptLookup::IdPayloadConflict,
        "S3_METADATA_DIGEST"
    );
    assert!(
        b.metadata_command(&e, MetadataScope::Snapshot, &u).is_err(),
        "S3_METADATA_REPLAY_SCOPE"
    );
    assert_eq!(
        b.feedback_index().unwrap().revisions.len(),
        1,
        "S3_METADATA_SINGLE_REVISION"
    );
    let issuer = b.issue_metadata_issuer().unwrap();
    let e2 = envelope(&b, &issuer, 1);
    let (mut exact, hold) = feedback(&mut b, key.clone());
    let text = blob(&mut b, &vec![b'x'; 16 << 10]);
    exact.original_text_ref = text.reference.clone();
    let strokes = blob(
        &mut b,
        serde_json::to_vec(&vec![[0.0, 1.0]; 512])
            .unwrap()
            .as_slice(),
    );
    exact.strokes_ref = strokes.reference.clone();
    let exact_update = update(&b, MetadataChange::SaveFeedback(exact.clone()));
    committed(
        b.metadata_command(&e2, MetadataScope::Feedback, &exact_update)
            .unwrap(),
    );
    let mut too_long = exact.clone();
    too_long.comment_id = CommentId::fresh().unwrap();
    let over = blob(&mut b, &vec![b'x'; (16 << 10) + 1]);
    too_long.original_text_ref = over.reference.clone();
    let e3 = envelope(&b, &issuer, 2);
    let over_update = update(&b, MetadataChange::SaveFeedback(too_long));
    assert!(
        b.metadata_command(&e3, MetadataScope::Feedback, &over_update)
            .is_err(),
        "S3_METADATA_TEXT_LIMIT"
    );
    let mut points = exact;
    points.comment_id = CommentId::fresh().unwrap();
    let over_points = blob(
        &mut b,
        serde_json::to_vec(&vec![[0.0, 1.0]; 513])
            .unwrap()
            .as_slice(),
    );
    points.strokes_ref = over_points.reference.clone();
    let over_update = update(&b, MetadataChange::SaveFeedback(points));
    assert!(
        b.metadata_command(&e3, MetadataScope::Feedback, &over_update)
            .is_err(),
        "S3_METADATA_POINTS_LIMIT"
    );
    drop((text, strokes, over, over_points, hold));
    let root = std::fs::read(&path).unwrap();
    b.restrict_limits_probe(StorageLimits {
        comments: 2,
        ..StorageLimits::default()
    })
    .unwrap();
    let (over_count, _hold) = feedback(&mut b, key);
    let over_update = update(&b, MetadataChange::SaveFeedback(over_count));
    assert!(
        b.metadata_command(&e3, MetadataScope::Feedback, &over_update)
            .is_err(),
        "S3_METADATA_COMMENT_LIMIT"
    );
    assert_eq!(std::fs::read(&path).unwrap(), root);
}
#[test]
fn s3_orchestration_admission() {
    let tmp = temp("s3-orchestration-admission");
    let path = tmp.path("studio.kinewright");
    let (_core, mut b, _inputs) = setup(&path);
    let issuer = b.issue_metadata_issuer().unwrap();
    let e = envelope(&b, &issuer, 1);
    let run = run_record(RunId::fresh().unwrap());
    let before = std::fs::read(&path).unwrap();
    assert!(
        matches!(
            b.admit_orchestration_record(&RunSnapshotRecord {
                version: 2,
                view: run.projection.clone()
            }),
            Err(BundleError::Unsupported(_))
        ),
        "S3_ORCHESTRATION_SCHEMA"
    );
    let mut wrong = run.clone();
    wrong.projection.run_id = RunId::fresh().unwrap();
    let invalid = update(
        &b,
        MetadataChange::Orchestration {
            runs: vec![wrong],
            consume_batch: None,
        },
    );
    assert!(
        b.validate_run_update(&e, &invalid).is_err(),
        "S3_ORCHESTRATION_RUN_BINDING"
    );
    let invalid = update(
        &b,
        MetadataChange::Orchestration {
            runs: vec![run.clone()],
            consume_batch: Some((FeedbackBatchId::fresh().unwrap(), run.run_id.clone())),
        },
    );
    assert!(
        b.validate_run_update(&e, &invalid).is_err(),
        "S3_ORCHESTRATION_UNSUBMITTED_BATCH"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "S3_ORCHESTRATION_REFUSAL_ROOT"
    );
    let update = update(
        &b,
        MetadataChange::Orchestration {
            runs: vec![run.clone()],
            consume_batch: None,
        },
    );
    let validated = b.validate_run_update(&e, &update).unwrap();
    let mut changed = e.clone();
    changed.owner_epoch ^= 1;
    assert!(
        b.commit_orchestration(&changed, validated).is_err(),
        "S3_ORCHESTRATION_VALIDATED_ENVELOPE"
    );
    let validated = b.validate_run_update(&e, &update).unwrap();
    let receipt = b.commit_orchestration(&e, validated).unwrap();
    assert_eq!(receipt.run_ids, vec![run.run_id.clone()]);
    drop(b);
    let mut b = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let replay = b.validate_run_update(&e, &update).unwrap();
    assert_eq!(
        b.commit_orchestration(&e, replay).unwrap(),
        receipt,
        "S3_ORCHESTRATION_REOPEN_REPLAY"
    );
    assert!(
        b.run_index().unwrap().runs.contains_key(&run.run_id),
        "S3_ORCHESTRATION_REOPEN_INDEX"
    );
}

fn assert_projection_manifest_oracle(bundle: &mut ProjectBundle, path: &Path, source: &ObjectRef) {
    use kinewright_project::{bundle_recovery::verify_projection_payloads, store::StoreManifest};
    let bytes = std::fs::read(
        path.with_extension("kinewright-assets")
            .join("studio/objects")
            .join(&source.sha256),
    )
    .unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let original_root: kinewright_project::bundle::StudioRoot =
        serde_json::from_value(original["studio"].clone()).unwrap();
    let manifest = (*bundle
        .store()
        .read_record::<StoreManifest>(&original_root.store_manifest_ref)
        .unwrap())
    .clone();
    for reference in [
        &original_root.version_index_ref,
        &original_root.feedback_index_ref,
        &original_root.run_index_ref,
    ] {
        let reference = reference.as_ref().unwrap();
        let mut changed_manifest = manifest.clone();
        assert!(changed_manifest.objects.contains(reference));
        changed_manifest
            .objects
            .retain(|object| object != reference);
        let manifest_hold = bundle
            .store_mut()
            .unwrap()
            .admit_manifest_probe(&changed_manifest)
            .unwrap();
        let manifest_ref = manifest_hold.reference.clone();
        let mut root = original_root.clone();
        root.store_manifest_ref = manifest_ref;
        let dependencies = [
            Some(root.branch_index_ref.clone()),
            Some(root.input_manifest_ref.clone()),
            Some(root.store_manifest_ref.clone()),
            root.version_index_ref.clone(),
            root.feedback_index_ref.clone(),
            root.run_index_ref.clone(),
            root.claim_index_ref.clone(),
            root.receipt_index_ref.clone(),
            root.issuer_state_ref.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let mut file = original.clone();
        file["studio"] = serde_json::to_value(&root).unwrap();
        let changed = serde_json::to_vec(&file).unwrap();
        let checkpoint_ref = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&changed),
            byte_length: changed.len() as u64,
            record_kind: RecordKind::RootCheckpoint,
        };
        let checkpoint = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &*changed, &checkpoint_ref, &dependencies)
            .unwrap();
        assert!(
            matches!(verify_projection_payloads(bundle, source, &checkpoint.reference), Err(BundleError::Corrupt(message)) if message == "L1 projected manifest omits root reference"),
            "C1_PROJECTED_MANIFEST_ORACLE: {:?}",
            reference.record_kind
        );
        drop((checkpoint, manifest_hold));
    }
}

#[test]
fn c6b_run_update_checks_sequence_and_consumes_admission() {
    let tmp = temp("c6b-run-admission");
    let path = tmp.path("studio.kinewright");
    let (_core, mut bundle, _inputs) = setup(&path);
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let request = update(
        &bundle,
        MetadataChange::Orchestration {
            runs: vec![run_record(RunId::fresh().unwrap())],
            consume_batch: None,
        },
    );
    let before = bundle.store().usage().unwrap().objects;
    let (refused, trace) = io_observer::observe(|| {
        bundle.validate_run_update(&envelope(&bundle, &issuer, 2), &request)
    });
    assert!(
        matches!(refused, Err(BundleError::Identity(ref message)) if message == "metadata issuer sequence"),
        "C6B_RUN_SEQUENCE_CHECKED_BEFORE_ADMISSION"
    );
    assert!(
        trace.events.is_empty(),
        "invalid issuer must not stage records"
    );
    assert_eq!(bundle.store().usage().unwrap().objects, before);
    let command = envelope(&bundle, &issuer, 1);
    let (validated, staged) =
        io_observer::observe(|| bundle.validate_run_update(&command, &request));
    let staged_hashes: Vec<_> = staged
        .events
        .iter()
        .filter(|event| event.operation == "object_stage")
        .map(|event| event.attempted_sha256.clone().unwrap())
        .collect();
    assert!(
        !staged_hashes.is_empty(),
        "the update admitted real records"
    );
    let validations = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = std::rc::Rc::clone(&validations);
    let receipt = io_observer::at_barrier(
        "metadata_admission",
        move || observed.set(observed.get() + 1),
        || bundle.commit_orchestration(&command, validated.unwrap()),
    )
    .unwrap();
    assert_eq!(validations.get(), 0, "C6B_COMMIT_CONSUMES_ADMITTED_SEALS");
    let replay = bundle.validate_run_update(&command, &request).unwrap();
    assert_eq!(
        bundle.commit_orchestration(&command, replay).unwrap(),
        receipt
    );
}

#[test]
fn c6b_refused_voice_requests_do_not_exhaust_slots() {
    let tmp = temp("c6b-voice-slots");
    let path = tmp.path("studio.kinewright");
    let (_core, mut bundle, _inputs) = setup(&path);
    let limits = StorageLimits {
        comments: 1,
        ..StorageLimits::default()
    };
    bundle.restrict_limits_probe(limits).unwrap();
    let issuer = bundle.issue_metadata_issuer().unwrap();
    let audio = blob(&mut bundle, b"retained voice audio");
    let mut voice = VoiceRef {
        note_id: NoteId::fresh().unwrap(),
        audio_ref: audio.reference.clone(),
        duration_micros: 1_000_000,
        range: None,
        playhead_log_ref: None,
        original_asr_ref: None,
        chosen_transcript_revision: None,
    };
    for _ in 0..8 {
        voice.note_id = NoteId::fresh().unwrap();
        let request = update(&bundle, MetadataChange::SaveVoice(voice.clone()));
        let refused = bundle.metadata_command(
            &envelope(&bundle, &issuer, 1),
            MetadataScope::Snapshot,
            &request,
        );
        assert!(
            matches!(refused, Err(BundleError::Identity(ref message)) if message == "metadata scope/project"),
            "C6B_REFUSALS_DO_NOT_EXHAUST_VOICE_SLOTS"
        );
    }
    voice.note_id = NoteId::fresh().unwrap();
    let request = update(&bundle, MetadataChange::SaveVoice(voice.clone()));
    committed(
        bundle
            .metadata_command(
                &envelope(&bundle, &issuer, 1),
                MetadataScope::Feedback,
                &request,
            )
            .unwrap(),
    );
    assert_eq!(
        bundle.feedback_index().unwrap().notes.get(&voice.note_id),
        Some(&voice)
    );
}
