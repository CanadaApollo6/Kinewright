//! H2 exercises the existing owner and sealed publisher with a dirty manual producer.
#![cfg(all(target_os = "linux", feature = "test-util"))]
use kinewright_core::{
    Command, Core, Document, Event, Operation, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::LoadedBundle,
    editing_session::Ja1EditingSession,
    receipts::{CommandId, ReceiptLookup, ReceiptResult},
    storage_budget::StorageLimits,
};

fn flattened_document(path: &std::path::Path) -> Document {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn h2_candidate_commit_preserves_pending_manual_and_replays_after_reopen() {
    run_case(false);
}

#[test]
fn h2_candidate_unknown_publication_reconciles_only_its_registered_core() {
    run_case(true);
}

#[allow(
    clippy::too_many_lines,
    reason = "one exact owner and manual identity oracle across publication and reopen"
)]
fn run_case(drop_ack: bool) {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("h2-roots");
    let temp = TempDirectory::new_in(&root, "candidate");
    let path = temp.path("project.kinewright");
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
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner.first_save(&path, StorageLimits::default()).unwrap();
    let mut candidate = owner.fork_durable_branch("candidate").unwrap();
    assert!(matches!(
        candidate
            .core()
            .request(Command::Do(Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode(0),
                duration: TimeCode(100),
                color: kinewright_core::SolidColor::default()
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let candidate_base = owner.persist_durable_branch(&mut candidate).unwrap();
    let issuer = owner
        .issue_manual_lease("candidate-issuer".into(), "candidate-private".into())
        .unwrap()
        .issuer;
    let lease = owner
        .issue_manual_lease("person".into(), "manual-private".into())
        .unwrap();
    let permit = owner
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    assert!(matches!(
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let pending = permit.pending().unwrap();
    let manual = core.export_history().unwrap();
    let attachment = owner.current_switch_attachment().unwrap();
    let flattened = flattened_document(&path);
    let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
    let mut specialist = owner
        .fork_registered_from_candidate(&binding, "Editor")
        .unwrap();
    assert!(matches!(
        specialist
            .core()
            .request(Command::Do(Operation::SplitClip {
                clip: kinewright_core::ClipId(1),
                at: TimeCode(50)
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let retained = specialist.core().clone();
    let frozen = kinewright_project::io_observer::at_barrier(
        "candidate_frozen_persisted",
        move || {
            for command in [
                Command::Undo,
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                }),
            ] {
                assert!(matches!(
                    retained.request(command).unwrap(),
                    Event::MutationRefused { .. }
                ));
            }
        },
        || owner.freeze_registered_candidate(&mut specialist),
    )
    .unwrap();
    assert!(matches!(
        specialist.core().request(Command::Undo).unwrap(),
        Event::MutationRefused { .. }
    ));
    let envelope = owner
        .candidate_merge_envelope(
            &frozen,
            CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap();
    let mut invalid = envelope.clone();
    invalid.command = Some(kinewright_core::JournalCommand::DoBatch(vec![
        Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        },
    ]));
    let before_refusal = owner.root_sha256().unwrap();
    assert!(owner.commit_candidate(&frozen, &invalid).is_err());
    assert_eq!(owner.root_sha256().unwrap(), before_refusal);
    if drop_ack {
        let result = kinewright_project::io_observer::fail_once("root_parent_sync", || {
            owner.commit_candidate(&frozen, &envelope)
        });
        assert!(matches!(
            result,
            Err(kinewright_project::bundle::BundleError::DurabilityUnknown)
        ));
        assert_eq!(core.export_history().unwrap(), manual);
        assert_eq!(permit.pending().unwrap(), pending);
        for command in [
            Command::Undo,
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
        ] {
            assert!(matches!(
                candidate.core().request(command).unwrap(),
                Event::MutationRefused { .. }
            ));
        }
        owner.reconcile_active().unwrap();
        assert_eq!(
            candidate.core().export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            2
        );
    }
    let (receipt, trace) =
        kinewright_project::io_observer::observe(|| owner.commit_candidate(&frozen, &envelope));
    let receipt = receipt.unwrap();
    if !drop_ack {
        let barriers: Vec<_> = trace
            .events
            .iter()
            .filter_map(|e| e.operation.strip_prefix("bundle_B"))
            .collect();
        assert_eq!(barriers, ["0", "1", "2", "3", "4", "5"]);
    }
    let ReceiptLookup::Committed(record) = &receipt else {
        panic!("candidate receipt {receipt:?}")
    };
    let after = owner.durable_branch_history(&candidate).unwrap();
    assert_eq!(after.document.tracks[0].clips.len(), 2);
    assert_eq!(candidate.core().export_history().unwrap(), *after);
    assert_eq!(
        after.cursor.undo.len(),
        2,
        "one merge undo entry follows candidate's own edit"
    );
    assert_eq!(record.target_branch, *candidate.branch_id());
    let new_binding = owner.candidate_binding(candidate.branch_id()).unwrap();
    let checkpoint = owner
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &new_binding.checkpoint,
        )
        .unwrap();
    let ReceiptResult::Edit { batch_id, .. } = &record.result else {
        panic!("edit result")
    };
    assert_eq!(batch_id.as_ref(), checkpoint.batch_ids.last());
    drop(checkpoint);
    assert!(
        batch_id.is_some(),
        "candidate batch identity must be retained"
    );
    assert_eq!(core.export_history().unwrap(), manual);
    assert_eq!(permit.pending().unwrap(), pending);
    assert_eq!(
        owner.current_switch_attachment().unwrap().current,
        attachment.current
    );
    assert_eq!(flattened_document(&path), flattened);
    let accepted: kinewright_project::receipts::CommandEnvelope =
        serde_json::from_slice(&pending[0].envelope).unwrap();
    assert_eq!(accepted.manual.unwrap().binding, lease);
    assert_eq!(owner.commit_candidate(&frozen, &envelope).unwrap(), receipt);
    let head_after_merge = owner.root_sha256().unwrap();
    assert!(
        owner
            .fork_registered_from_candidate(&binding, "stale fork")
            .is_err()
    );
    assert_eq!(owner.root_sha256().unwrap(), head_after_merge);
    let mut next = envelope.clone();
    next.id.sequence += 1;
    assert!(owner.commit_candidate(&frozen, &next).is_err());
    assert!(matches!(
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert_eq!(
        permit.pending().unwrap().len(),
        pending.len() + 1,
        "the original manual lease remains usable"
    );
    let candidate_id = candidate.branch_id().clone();
    drop(after);
    drop(owner);
    let loaded = LoadedBundle::read(&path).unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&fresh).unwrap();
    assert_eq!(
        fresh.export_history().unwrap().document.as_ref(),
        &flattened
    );
    drop(loaded);
    let mut reopened = Ja1EditingSession::open(&path).unwrap();
    let registered = reopened
        .reopen_registered_candidate(candidate_id, &candidate_base)
        .unwrap();
    assert_eq!(
        registered.core().export_history().unwrap().document.tracks[0]
            .clips
            .len(),
        2
    );
    assert_eq!(
        reopened.commit_candidate(&frozen, &envelope).unwrap(),
        receipt
    );
    let mut changed = envelope.clone();
    changed.expected_revision.0 += 1;
    assert_ne!(
        reopened.commit_candidate(&frozen, &changed).unwrap(),
        receipt
    );
}
