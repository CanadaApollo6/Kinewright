use super::*;
use crate::operation_actor::Core;
use crate::{editing_session::DurableBranchRole, storage_budget::StorageLimits};
use kinewright_core::{Command, Document, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

fn fixture() -> (
    TempDirectory,
    Ja1EditingSession,
    DurableBranchSessionGuard,
    DurableBranchSessionGuard,
) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "native-actor-release");
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
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&temp.path("project.kinewright"), StorageLimits::default())
        .unwrap();
    let mut candidate = owner
        .fork_durable_branch("candidate", DurableBranchRole::RunCandidate)
        .unwrap();
    owner.persist_durable_branch(&mut candidate).unwrap();
    let base = owner.candidate_binding(candidate.branch_id()).unwrap();
    let task = owner.fork_registered_from_candidate(&base, "task").unwrap();
    (temp, owner, candidate, task)
}

#[test]
fn native_release_waits_for_dirty_work_handles_and_capacity_then_rehydrates_exactly() {
    let (_temp, mut owner, candidate, mut task) = fixture();
    task.core()
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    task.core().request(Command::Undo).unwrap();
    let expected = task.core().export_history().unwrap();
    assert!(!expected.history().redo.is_empty());
    assert!(
        !owner.release_checkpointed_native_branch(&task).unwrap(),
        "DIRTY_NOT_RELEASED"
    );
    let base_ref = owner.persist_durable_branch(&mut task).unwrap();
    let extra = task.core().clone();
    assert!(
        !owner.release_checkpointed_native_branch(&task).unwrap(),
        "LIVE_CORE_HANDLE_NOT_RELEASED"
    );
    drop(extra);
    assert!(
        !owner
            .release_checkpointed_native_branch(&candidate)
            .unwrap(),
        "CANDIDATE_IN_USE_NOT_RELEASED"
    );
    let store = owner.store().unwrap();
    let usage = store.usage().unwrap();
    let mut available = store.limits().metadata_bytes
        - usage.metadata_bytes
        - usage.decoded_bytes
        - usage.reserved_bytes;
    let mut held = Vec::new();
    while available != 0 {
        let bytes = available.min(store.limits().decoded_record_bytes);
        held.push(store.reserve_copy_read(0, bytes, false, true).unwrap());
        available -= bytes;
    }
    assert!(
        matches!(
            owner.release_checkpointed_native_branch(&task),
            Err(BundleError::Capacity(_))
        ),
        "CAPACITY_NOT_RELEASED"
    );
    assert_eq!(
        task.core().attachment_state().unwrap(),
        kinewright_core::AttachmentState::Live
    );
    assert_eq!(task.core().export_history().unwrap(), expected);
    drop(held);
    let branch = task.branch_id().clone();
    let checkpoint = owner.candidate_binding(&branch).unwrap();
    assert!(owner.release_checkpointed_native_branch(&task).unwrap());
    drop(task);
    assert!(
        owner
            .release_checkpointed_native_branch(&candidate)
            .unwrap()
    );
    drop(candidate);
    assert!(owner.durable_sessions().is_empty());
    let restored = owner
        .reopen_registered_candidate(branch.clone(), &base_ref)
        .unwrap();
    assert_eq!(
        restored.core().export_history().unwrap(),
        expected,
        "RELEASE_REPLAY_EXACT_WITH_REDO"
    );
    assert_eq!(
        owner.candidate_binding(&branch).unwrap(),
        checkpoint,
        "RELEASE_CHECKPOINT_IDENTITY_KEPT"
    );
}

#[test]
fn native_release_freshly_reads_the_journal_and_keeps_a_failed_actor_usable() {
    let (temp, mut owner, _candidate, mut task) = fixture();
    task.core()
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    owner.persist_durable_branch(&mut task).unwrap();
    let expected = task.core().export_history().unwrap();
    let checkpoint = owner
        .active_bundle()
        .unwrap()
        .checkpoint(task.branch_id())
        .unwrap();
    let path = temp.path(&format!(
        "project.kinewright-assets/studio/objects/{}",
        checkpoint.owner_ref.sha256
    ));
    let original = std::fs::read(&path).unwrap();
    let root = std::fs::read(temp.path("project.kinewright")).unwrap();
    let mut damaged = original.clone();
    damaged[0] = b'!';
    std::fs::write(&path, &damaged).unwrap();
    let refused = owner.release_checkpointed_native_branch(&task);
    assert!(
        matches!(
            refused,
            Err(BundleError::Corrupt(_)
                | BundleError::Operation(crate::operation_actor::Error::History(
                    crate::operation_history::Error::Store(_)
                )))
        ),
        "FRESH_REPLAY_REFUSES_DAMAGED_BYTES: {refused:?}"
    );
    assert_eq!(
        owner.durable_sessions().len(),
        2,
        "MISMATCH_RETAINS_BOTH_ACTORS"
    );
    assert_eq!(
        task.core().attachment_state().unwrap(),
        kinewright_core::AttachmentState::Live
    );
    assert_eq!(task.core().export_history().unwrap(), expected);
    assert!(
        matches!(
            task.core().request(Command::Undo).unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ),
        "MISMATCH_ACTOR_STAYS_USABLE"
    );
    task.core().request(Command::Redo).unwrap();
    assert_eq!(
        task.core().export_history().unwrap().history().document,
        expected.history().document
    );
    assert_eq!(
        std::fs::read(temp.path("project.kinewright")).unwrap(),
        root
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        damaged,
        "VERIFICATION_DOES_NOT_REWRITE_FAILED_HISTORY"
    );
    std::fs::write(path, original).unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one lifecycle checks the released result, review and merge without retaining extra history leases"
)]
fn native_release_frozen_review_and_merge_preserve_the_exact_result() {
    let (_temp, mut owner, candidate, mut task) = fixture();
    task.core()
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    let expected = task.core().export_history().unwrap();
    let frozen = owner.freeze_registered_candidate(&mut task).unwrap();
    let task_branch = task.branch_id().clone();
    let candidate_branch = candidate.branch_id().clone();
    let candidate_base = owner
        .active_bundle()
        .unwrap()
        .root()
        .unwrap()
        .candidate_recovery[&candidate_branch]
        .base_ref
        .clone();
    assert!(owner.release_checkpointed_native_branch(&task).unwrap());
    drop(task);
    assert!(
        owner
            .release_checkpointed_native_branch(&candidate)
            .unwrap()
    );
    drop(candidate);
    owner
        .active_bundle_mut()
        .unwrap()
        .collect_after_ack()
        .unwrap();
    owner
        .reopen_native_task_pair(
            task_branch.clone(),
            frozen.base_ref(),
            candidate_branch.clone(),
            &candidate_base,
        )
        .unwrap();
    let mut restored = owner
        .durable_sessions()
        .into_iter()
        .find(|(_, guard)| guard.branch_id() == &task_branch)
        .unwrap()
        .1;
    assert_eq!(restored.core().export_history().unwrap(), expected);
    let after = owner
        .freeze_registered_task_checkpoint(&mut restored)
        .unwrap();
    assert_eq!(after.source(), frozen.source());
    assert_eq!(after.base(), frozen.base());
    assert_eq!(
        after.delta(&owner).unwrap().operations(),
        frozen.delta(&owner).unwrap().operations()
    );
    assert_eq!(
        after.captured_history().position,
        frozen.captured_history().position
    );
    let review = owner
        .active_bundle()
        .unwrap()
        .prepare_checkpoint_restore(
            owner.core(),
            &crate::bundle_recovery::RecoverySelection::WorkingBranch(task_branch),
            crate::bundle_records::BranchId::fresh().unwrap(),
        )
        .unwrap();
    let installed = review.core().export_history().unwrap();
    assert_eq!(
        installed.history().document,
        expected.history().document,
        "REVIEW_INSTALL_PRESERVES_DOCUMENT"
    );
    assert_eq!(
        installed.history().undo,
        expected.history().undo,
        "REVIEW_INSTALL_PRESERVES_COMMAND_KEYS"
    );
    assert_eq!(
        installed.history().redo,
        expected.history().redo,
        "REVIEW_INSTALL_PRESERVES_REDO"
    );
    assert_eq!(
        installed.resources(),
        expected.resources(),
        "REVIEW_INSTALL_PRESERVES_RESOURCES"
    );
    drop(installed);
    drop(review);
    let issuer = owner
        .issue_manual_lease("candidate".into(), "candidate-private".into())
        .unwrap()
        .issuer;
    let envelope = owner
        .candidate_merge_envelope(
            &after,
            crate::receipts::CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap();
    let result = owner.commit_candidate(&after, &envelope).unwrap();
    assert!(matches!(
        result,
        crate::receipts::ReceiptLookup::Committed(_)
    ));
    let merged = owner
        .durable_sessions()
        .into_iter()
        .find(|(_, guard)| guard.branch_id() == &candidate_branch)
        .unwrap()
        .1;
    assert_eq!(
        merged.core().export_history().unwrap().history().document,
        expected.history().document
    );
    assert_eq!(owner.commit_candidate(&after, &envelope).unwrap(), result);
}

#[test]
fn native_release_checks_split_occurrences_and_every_history_boundary() {
    let (_temp, mut owner, _candidate, mut task) = fixture();
    task.core()
        .request(Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: kinewright_core::TimeCode(0),
            duration: kinewright_core::TimeCode(100),
            color: kinewright_core::SolidColor::default(),
        }))
        .unwrap();
    let clip = task
        .core()
        .export_history()
        .unwrap()
        .history()
        .document
        .tracks[0]
        .clips[0]
        .id;
    task.core()
        .request(Command::Do(Operation::SplitClip {
            clip,
            at: kinewright_core::TimeCode(50),
        }))
        .unwrap();
    task.core().request(Command::Undo).unwrap();
    task.core().request(Command::Redo).unwrap();
    let expected = task.core().export_history().unwrap();
    let reference = owner.persist_durable_branch(&mut task).unwrap();
    let lineage = owner
        .active_bundle()
        .unwrap()
        .branch_lineage(task.branch_id())
        .unwrap();
    assert!(!lineage.lineage().split_ancestry.is_empty());
    let branch = task.branch_id().clone();
    assert!(owner.release_checkpointed_native_branch(&task).unwrap());
    drop(task);
    let restored = owner
        .reopen_registered_candidate(branch.clone(), &reference)
        .unwrap();
    assert_eq!(restored.core().export_history().unwrap(), expected);
    assert_eq!(
        owner
            .active_bundle()
            .unwrap()
            .branch_lineage(&branch)
            .unwrap()
            .lineage(),
        lineage.lineage()
    );
}
