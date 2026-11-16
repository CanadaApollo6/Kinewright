#![cfg(all(target_os = "linux", feature = "test-util"))]
use kinewright_core::{ClipId, Command, Document, Event, Operation, TrackId};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    editing_session::{DurableBranchRole, Ja1EditingSession},
    storage_budget::StorageLimits,
};
#[expect(
    dead_code,
    reason = "the native door witness shares the exact fixed sizing document"
)]
#[path = "../../kinewright-core/tests/support/merge_fixture.rs"]
mod merge_fixture;

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one real 20000-clip project crosses both native merge legs and cold open"
)]
fn twenty_thousand_clip_project_opens_edits_forks_merges_and_saves() {
    use kinewright_project::{
        operation_history::{HistoryPool, PoolLimits},
        receipts::{CommandId, ReceiptLookup},
    };
    use std::sync::{Arc, atomic::AtomicBool};
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "native-20000-clip-merge",
    );
    let mut document = merge_fixture::document(20_000, 8, 90);
    for (index, track) in document.tracks.iter_mut().enumerate() {
        track.sync_lock = index < 4;
    }
    let core = Core::for_test(document).unwrap();
    let mut writer = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let path = temp.path("project.kinewright");
    writer.first_save(&path, StorageLimits::default()).unwrap();
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(8),
        locked: false,
    }))
    .unwrap();
    writer.save_active().unwrap();
    let saved = core.snapshot_identity().unwrap();
    let mut opened = kinewright_project::bundle::LoadedBundle::read(
        &path,
        &HistoryPool::new(PoolLimits::default()),
    )
    .unwrap();
    assert_eq!(
        opened.open_manual().unwrap().snapshot_identity().unwrap(),
        saved,
        "PRODUCT_20000_CLIP_OPENS_AND_EDITS"
    );
    drop(opened);
    let mut candidate = writer
        .fork_durable_branch("candidate", DurableBranchRole::RunCandidate)
        .unwrap();
    writer.persist_durable_branch(&mut candidate).unwrap();
    let binding = writer.candidate_binding(candidate.branch_id()).unwrap();
    let mut task = writer
        .fork_registered_from_candidate(&binding, "dialogue cut")
        .unwrap();
    let mut cuts = vec![];
    for track in 0..4 {
        cuts.push(Operation::SplitClip {
            clip: ClipId(track * 2500 + 1),
            at: kinewright_core::TimeCode(30),
        });
        let middle = ClipId(20_001 + track * 2);
        cuts.push(Operation::SplitClip {
            clip: middle,
            at: kinewright_core::TimeCode(36),
        });
        if track != 0 {
            cuts.push(Operation::DeleteClip { clip: middle });
        }
    }
    cuts.push(Operation::RippleDeleteClip {
        clip: ClipId(20_001),
    });
    assert!(
        matches!(
            task.core().request(Command::DoBatch(cuts)).unwrap(),
            Event::DocumentChanged { .. }
        ),
        "PRODUCT_20000_CLIP_TASK_CUT_ADMITTED"
    );
    let frozen = writer.freeze_registered_candidate(&mut task).unwrap();
    assert_eq!(
        frozen.delta(&writer).unwrap().len(),
        12,
        "FROZEN_20000_CLIP_TASK_RETAINS_EVERY_OPERATION"
    );
    let issuer = writer
        .issue_manual_lease("candidate".into(), "candidate-private".into())
        .unwrap()
        .issuer;
    let envelope = writer
        .candidate_merge_envelope(
            &frozen,
            CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap();
    assert!(
        matches!(&envelope.command, Some(kinewright_core::JournalCommand::DoBatch(operations)) if operations.len() == 12),
        "CANDIDATE_ENVELOPE_FORWARDS_EVERY_OPERATION"
    );
    assert!(
        matches!(
            writer.commit_candidate(&frozen, &envelope).unwrap(),
            ReceiptLookup::Committed(_)
        ),
        "PRODUCT_20000_CLIP_TASK_MERGES_TO_CANDIDATE"
    );
    let operations = candidate.core().operations(false).unwrap();
    assert_eq!(
        operations.len(),
        12,
        "CANDIDATE_FORWARDS_THE_COMPLETE_FOUR_TRACK_CUT"
    );
    let delta = operations.iter().cloned().collect();
    drop(operations);
    let lease = writer
        .issue_manual_lease("person".into(), "person-private".into())
        .unwrap();
    let replica = Core::peer_for_test(core.portable_history().unwrap()).unwrap();
    let gui = Ja1EditingSession::new(temp.root(), &replica, StorageLimits::default()).unwrap();
    let permit = gui
        .bind_manual_producer(
            lease.clone(),
            writer.manual_input_context().unwrap(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    permit
        .request(Command::DoBatch(delta))
        .expect("PRODUCT_20000_CLIP_CANDIDATE_FITS_ORDINARY_BATCH");
    let authority = writer.manual_authority(lease).unwrap();
    let mut receipts = vec![];
    for message in permit.pending().unwrap() {
        let (receipt, report) = writer.commit_manual_history(&authority, &message).unwrap();
        assert!(report.is_some(), "PRODUCT_20000_CLIP_ACK_FOLLOWS_SAVE");
        receipts.push(receipt);
    }
    gui.acknowledge_manual(&permit, receipts).unwrap();
    let identity = core.snapshot_identity().unwrap();
    assert_eq!(
        identity,
        replica.snapshot_identity().unwrap(),
        "PRODUCT_20000_CLIP_PEERS_AGREE"
    );
    assert_eq!(core.operations(false).unwrap().len(), 13);
    writer.save_active().unwrap();
    let mut cold = kinewright_project::bundle::LoadedBundle::read(
        &path,
        &HistoryPool::new(PoolLimits::default()),
    )
    .unwrap();
    assert_eq!(
        cold.open_manual().unwrap().snapshot_identity().unwrap(),
        identity,
        "PRODUCT_20000_CLIP_MERGED_SAVE_COLD_OPENS_EXACTLY"
    );
}
fn document(clips: usize) -> Document {
    let mut document = kinewright_project::c0_fixtures::save_cost_document();
    let template = document.tracks.remove(0);
    document.tracks.clear();
    for index in 1..=clips / 10 {
        let mut track = template.clone();
        track.id = TrackId(index as u64);
        for (offset, clip) in track.clips.iter_mut().enumerate() {
            clip.id = ClipId(index as u64 * 100 + offset as u64 + 1);
        }
        document.tracks.push(track);
    }
    document.validate().unwrap();
    document
}
#[test]
fn native_task_history_capacity_table() {
    for clips in [40, 120, 400, 1200] {
        let temporary = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "history-capacity-matrix",
        );
        let core = Core::for_test(document(clips)).unwrap();
        let mut owner =
            Ja1EditingSession::new(temporary.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(
                &temporary.path("project.kinewright"),
                StorageLimits::default(),
            )
            .unwrap();
        let mut candidate = owner
            .fork_durable_branch("candidate", DurableBranchRole::RunCandidate)
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut task = owner
            .fork_registered_from_candidate(&binding, "task")
            .unwrap();
        for edit in 0..200 {
            let operations = if clips == 400 && edit == 100 {
                (1..=20)
                    .map(|track| Operation::RippleDeleteClip {
                        clip: ClipId(track * 100 + 10),
                    })
                    .collect()
            } else {
                vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: edit % 2 == 0,
                }]
            };
            assert!(
                matches!(
                    task.core().request(Command::DoBatch(operations)).unwrap(),
                    Event::DocumentChanged { .. }
                ),
                "TASK_ADMITS_THE_REQUIRED_200_EDITS clips={clips}"
            );
        }
        task.core().request(Command::Undo).unwrap();
        let expected = task.core().snapshot_identity().unwrap();
        let too_large = Operation::AddMarker {
            marker: kinewright_core::Marker {
                id: kinewright_core::MarkerId(900),
                position: kinewright_core::TimeCode(0),
                label: "x".repeat(1 << 20),
                color_token: 0,
            },
        };
        assert!(
            task.core().request(Command::Do(too_large)).is_err(),
            "EXECUTOR_REFUSES_OVERSIZED_STEP clips={clips}"
        );
        assert_eq!(
            task.core().snapshot_identity().unwrap(),
            expected,
            "REFUSAL_PRESERVES_DOCUMENT_BOTH_STACKS_LINEAGE_LEDGER_RESOURCES clips={clips}"
        );
        let frozen = owner.freeze_registered_task_checkpoint(&mut task).unwrap();
        let checkpoint = owner
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &frozen.source().checkpoint,
            )
            .unwrap();
        let restored = checkpoint
            .open(owner.store().unwrap(), task.core().history_pool())
            .unwrap();
        assert_eq!(
            restored.identity(),
            expected,
            "ADMITTED_NATIVE_TASK_FREEZES_AND_OPENS clips={clips}"
        );
        let reader = restored.into_reader().unwrap();
        assert_eq!(
            reader.creation_sizing(),
            task.core().export_history().unwrap().creation_sizing(),
            "REOPEN_USES_THE_IMMUTABLE_CREATION_SIZING clips={clips}"
        );
        assert_eq!(reader.history().undo().len(), 199);
        assert_eq!(reader.history().redo().len(), 1);
        println!(
            "NATIVE_HISTORY_CAPACITY clips={clips} accepted=200 undo=199 redo=1 identity={expected}"
        );
    }
}
