#![cfg(all(feature = "eval-harness", target_os = "linux"))]
use kinewright_agent::{BranchApplyOutcome, DurableTimelineBranch, TimelineBranch};
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::ObjectRef,
    io_observer,
    receipts::{CommandEnvelope, CommandId, CommandReceipt, HistoryRequest, ReceiptLookup},
};
use std::path::Path;

fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn lock(locked: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    }
}
fn revision(core: &Core) -> TimelineRevision {
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    revision
}
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let core = Core::spawn(Document::default()).unwrap();
    apply(
        &core,
        Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }),
    );
    apply(&core, Command::Do(lock(false)));
    apply(&core, Command::Undo);
    apply(&core, Command::Redo);
    assert_eq!(revision(&core), TimelineRevision(4));
    let mut bundle = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let reference = inputs.reference.clone();
    bundle
        .save(&core.export_history().unwrap(), &reference)
        .unwrap();
    (core, bundle, reference)
}
fn reopen(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let loaded = LoadedBundle::read(path).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    (core, loaded.into_writer().unwrap(), inputs)
}
fn committed(result: ReceiptLookup) -> CommandReceipt {
    match result {
        ReceiptLookup::Committed(r) | ReceiptLookup::Retired(r) => r,
        other => panic!("{other:?}"),
    }
}
fn command(
    bundle: &mut ProjectBundle,
    core: &Core,
    inputs: &ObjectRef,
    command: JournalCommand,
) -> CommandEnvelope {
    let issuer = bundle.issue_issuer(core, inputs, 1).unwrap();
    CommandEnvelope {
        version: 1,
        action: "edit".into(),
        id: CommandId {
            issuer,
            sequence: 1,
        },
        project_id: bundle.project_id().clone(),
        target_branch: bundle.manual_branch_id().clone(),
        owner_epoch: bundle.store().owner_epoch(),
        lease: None,
        expected_head: bundle.manual_head().cloned(),
        expected_revision: revision(core),
        command: Some(command),
        comment_text: None,
        comment_strokes: None,
        evidence: vec![],
        transition: None,
    }
}
fn prepare_merge(
    branch: &DurableTimelineBranch,
    bundle: &mut ProjectBundle,
    core: &Core,
    inputs: &ObjectRef,
) -> CommandEnvelope {
    let issuer = bundle.issue_issuer(core, inputs, 1).unwrap();
    branch
        .merge_envelope(
            bundle,
            core,
            CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap()
}

#[test]
fn reopened_exact_base_merges_once_with_fresh_runtime_revision() {
    let temp = TempDirectory::new("ja-s1-durable-base");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let original = core.export_history().unwrap();
    let legacy =
        TimelineBranch::new("counter-only", revision(&core), original.document.clone()).unwrap();
    apply(&legacy.core(), Command::Do(lock(true)));
    let mut branch = DurableTimelineBranch::fork(&mut bundle, &core, "durable").unwrap();
    apply(&branch.core(), Command::Do(lock(true)));
    apply(&branch.core(), Command::Do(lock(false)));
    apply(&branch.core(), Command::Undo);
    let proposed = branch.core().export_history().unwrap().document;
    let branch_id = branch.branch_id().clone();
    let base_ref = branch.persist(&mut bundle).unwrap();
    let frozen_delta = branch.base().full_applied_delta.clone();
    assert_eq!(
        frozen_delta,
        vec![lock(true)],
        "undone work is not a merge delta"
    );
    drop(branch);
    drop(bundle);
    drop(core);
    let (core, mut bundle, reopened_inputs) = reopen(&path);
    assert_eq!(inputs, reopened_inputs);
    assert_eq!(revision(&core), TimelineRevision(1));
    assert!(
        matches!(
            legacy.merge_into(&core).unwrap(),
            BranchApplyOutcome::Conflict {
                expected: TimelineRevision(4),
                actual: TimelineRevision(1)
            }
        ),
        "literal old numeric-base control"
    );
    assert_eq!(core.export_history().unwrap(), original);
    let branch = DurableTimelineBranch::reopen(&bundle, branch_id, &base_ref, "durable").unwrap();
    assert_eq!(branch.base().full_applied_delta, frozen_delta);
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    assert_eq!(envelope.expected_revision, revision(&core));
    committed(branch.merge(&mut bundle, &core, &envelope).unwrap());
    assert_eq!(core.export_history().unwrap().document, proposed);
    assert_eq!(
        core.export_history().unwrap().cursor.undo.len(),
        original.cursor.undo.len() + 1
    );
    let loaded = LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(
        restored.export_history().unwrap(),
        core.export_history().unwrap()
    );
    for command in [Command::Undo, Command::Redo] {
        apply(&core, command.clone());
        apply(&restored, command);
        assert_eq!(
            core.export_history().unwrap(),
            restored.export_history().unwrap()
        );
    }
}

#[test]
fn equal_bytes_new_head_and_equal_counter_other_head_conflict() {
    let temp = TempDirectory::new("ja-s1-head-conflict");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut branch = DurableTimelineBranch::fork(&mut bundle, &core, "durable").unwrap();
    apply(&branch.core(), Command::Do(lock(true)));
    branch.persist(&mut bundle).unwrap();
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    let original = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    let old_counter = revision(&core);
    let same_document = core.export_history().unwrap().document;
    let legacy = TimelineBranch::new("counter-only", old_counter, same_document.clone()).unwrap();
    apply(&legacy.core(), Command::Do(lock(true)));
    apply(&core, Command::Do(lock(false)));
    bundle
        .save(&core.export_history().unwrap(), &inputs)
        .unwrap();
    let current = bundle.checkpoint(bundle.manual_branch_id()).unwrap();
    assert_eq!(current.document_hash, original.document_hash);
    assert_ne!(current.head_id, original.head_id);
    let other_runtime = Core::spawn_at((*same_document).clone(), old_counter).unwrap();
    assert_eq!(revision(&other_runtime), envelope.expected_revision);
    assert!(
        matches!(
            legacy.merge_into(&other_runtime).unwrap(),
            BranchApplyOutcome::Applied { .. }
        ),
        "old counter-only path accepts another lineage"
    );
    let before = core.export_history().unwrap();
    let root = std::fs::read(&path).unwrap();
    let (result, trace) = io_observer::observe(|| branch.merge(&mut bundle, &core, &envelope));
    assert!(
        matches!(result,Err(BundleError::Unsupported(message)) if message=="durable base conflict")
    );
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(std::fs::read(&path).unwrap(), root);
    let fresh = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&path)
        .unwrap()
        .restore_manual(&fresh)
        .unwrap();
    assert!(
        branch
            .merge_envelope(&bundle, &fresh, envelope.id.clone())
            .is_err(),
        "equal document bytes at another HeadId must not bind even to a freshly restored Core"
    );
    assert!(
        branch
            .merge_envelope(&bundle, &other_runtime, envelope.id)
            .is_err()
    );
}

#[test]
fn restored_lineage_tracks_siblings_reuse_and_undone_splits() {
    let temp = TempDirectory::new("ja-s1-lineage");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let add = Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(0),
        duration: TimeCode(30),
        color: SolidColor::default(),
    };
    apply(&core, Command::Do(add.clone()));
    bundle
        .save(&core.export_history().unwrap(), &inputs)
        .unwrap();
    let manual = bundle.manual_branch_id().clone();
    let origin = bundle.branch_lineage(&manual).unwrap().current[&ClipId(1)].clone();
    let a = bundle.fork_branch(&manual).unwrap();
    let b = bundle.fork_branch(&manual).unwrap();
    let c = bundle.fork_branch(&manual).unwrap();
    let d = bundle.fork_branch(&manual).unwrap();
    let make = |id: &_| {
        let core = Core::spawn(Document::default()).unwrap();
        core.restore_history((*bundle.branch_history(id).unwrap()).clone())
            .unwrap();
        core
    };
    let first = make(&a);
    let second = make(&b);
    apply(
        &first,
        Command::DoBatch(vec![Operation::DeleteClip { clip: ClipId(1) }, add]),
    );
    bundle
        .save_branch(&a, &first.export_history().unwrap())
        .unwrap();
    let reused = bundle.branch_lineage(&a).unwrap().current[&ClipId(1)].clone();
    assert_ne!(reused, origin);
    let split = Command::Do(Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(15),
    });
    apply(&first, split.clone());
    bundle
        .save_branch(&a, &first.export_history().unwrap())
        .unwrap();
    let child = bundle.branch_lineage(&a).unwrap().current[&ClipId(2)].clone();
    apply(&second, split);
    bundle
        .save_branch(&b, &second.export_history().unwrap())
        .unwrap();
    assert_ne!(
        child,
        bundle.branch_lineage(&b).unwrap().current[&ClipId(2)]
    );
    apply(&first, Command::Undo);
    bundle
        .save_branch(&a, &first.export_history().unwrap())
        .unwrap();
    let undone = bundle.branch_lineage(&a).unwrap();
    assert_eq!(undone.current[&ClipId(1)], reused);
    assert!(undone.split_ancestry.is_empty());
    drop(undone);
    drop(bundle);
    let (_, mut bundle, _) = reopen(&path);
    assert_eq!(
        *bundle.branch_history(&c).unwrap(),
        *bundle.branch_history(&d).unwrap(),
        "shared immutable history decodes once"
    );
    let before = bundle.branch_lineage(&a).unwrap().current.clone();
    assert_eq!(before[&ClipId(1)], reused);
    apply(&first, Command::Redo);
    bundle
        .save_branch(&a, &first.export_history().unwrap())
        .unwrap();
    assert_eq!(
        bundle.branch_lineage(&a).unwrap().current[&ClipId(2)],
        child
    );
    let lineage = bundle.branch_lineage(&a).unwrap();
    assert_eq!(lineage.split_ancestry.len(), 2);
    assert!(
        lineage
            .split_ancestry
            .iter()
            .all(|(_, parent, _)| *parent == reused)
    );
}

#[test]
fn merge_lost_reply_preserves_one_undo_entry() {
    let temp = TempDirectory::new("ja-s1-merge-retry");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut branch = DurableTimelineBranch::fork(&mut bundle, &core, "durable").unwrap();
    apply(
        &branch.core(),
        Command::DoBatch(vec![lock(true), lock(false)]),
    );
    let base_ref = branch.persist(&mut bundle).unwrap();
    let id = branch.branch_id().clone();
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    let original = committed(branch.merge(&mut bundle, &core, &envelope).unwrap());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 3);
    drop(branch);
    drop(bundle);
    drop(core);
    let (core, mut bundle, inputs) = reopen(&path);
    let before = core.export_history().unwrap();
    let branch = DurableTimelineBranch::reopen(&bundle, id, &base_ref, "durable").unwrap();
    let (result, trace) = io_observer::observe(|| branch.merge(&mut bundle, &core, &envelope));
    assert_eq!(committed(result.unwrap()), original);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
    drop(branch);
    drop(bundle);
    drop(core);
    exercise_floor_and_close(&temp.path("floor.kinewright"));
    let _ = inputs;
}

#[allow(
    clippy::too_many_lines,
    reason = "one ordered floor, suffix, retry and closure witness"
)]
fn exercise_floor_and_close(path: &Path) {
    let (core, mut bundle, inputs) = setup(path);
    let drag = |locked| JournalCommand::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "K".into(),
    };
    let b = command(&mut bundle, &core, &inputs, drag(true));
    committed(bundle.command(&b, &core, &inputs, &[]).unwrap());
    let c = command(&mut bundle, &core, &inputs, drag(false));
    let committed_c = committed(bundle.command(&c, &core, &inputs, &[]).unwrap());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 3);
    assert_eq!(
        core.export_history().unwrap().cursor.undo[2]
            .operations
            .len(),
        2
    );
    let mut floor = command(&mut bundle, &core, &inputs, JournalCommand::Undo);
    let previous = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    floor.action = "floor".into();
    floor.command = None;
    floor.transition = Some(HistoryRequest::Floor {
        discard_batches: 4,
        preceding_checkpoint_generation: previous.checkpoint_generation,
        preceding_floor_generation: previous.floor_generation,
        command_cut: previous.command_cut.clone().unwrap(),
    });
    let floor_receipt = committed(bundle.command(&floor, &core, &inputs, &[]).unwrap());
    assert_ne!(floor_receipt.head_id, committed_c.head_id);
    assert_eq!(
        bundle
            .checkpoint(bundle.manual_branch_id())
            .unwrap()
            .floor_generation,
        1
    );
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    assert_eq!(
        core.export_history().unwrap().cursor.undo[0]
            .coalesce_key
            .as_deref(),
        Some("K")
    );
    let (retried, trace) = io_observer::observe(|| bundle.command(&c, &core, &inputs, &[]));
    assert_eq!(committed(retried.unwrap()), committed_c);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    let mut close = command(&mut bundle, &core, &inputs, JournalCommand::Undo);
    let previous = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    close.action = "recovery_close".into();
    close.command = None;
    close.transition = Some(HistoryRequest::RecoveryClose {
        preceding_checkpoint_generation: previous.checkpoint_generation,
        reconciled_cut: previous.command_cut.clone().unwrap(),
        suffix_digest: previous.history_ref.sha256.clone(),
    });
    let closed = committed(bundle.command(&close, &core, &inputs, &[]).unwrap());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    assert_eq!(
        core.export_history().unwrap().cursor.undo[0].coalesce_key,
        None
    );
    drop(bundle);
    drop(core);
    let (core, mut bundle, inputs) = reopen(path);
    let (retried, trace) = io_observer::observe(|| bundle.command(&close, &core, &inputs, &[]));
    assert_eq!(committed(retried.unwrap()), closed);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    let edit = command(&mut bundle, &core, &inputs, JournalCommand::Do(lock(true)));
    committed(bundle.command(&edit, &core, &inputs, &[]).unwrap());
    let mut invalid = command(&mut bundle, &core, &inputs, JournalCommand::Undo);
    let previous = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    invalid.action = "floor".into();
    invalid.command = None;
    let mut cut = previous.command_cut.clone().unwrap();
    cut.sequence += 1;
    invalid.transition = Some(HistoryRequest::Floor {
        discard_batches: 4,
        preceding_checkpoint_generation: previous.checkpoint_generation,
        preceding_floor_generation: previous.floor_generation,
        command_cut: cut,
    });
    let before = core.export_history().unwrap();
    assert!(
        HistoryTransition::floor(before.clone(), 4).is_ok(),
        "the proposed floor itself is valid; only its cut is wrong"
    );
    let (result, trace) = io_observer::observe(|| bundle.command(&invalid, &core, &inputs, &[]));
    assert!(result.is_err());
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
    let mut valid = invalid.clone();
    valid.transition = Some(HistoryRequest::Floor {
        discard_batches: 4,
        preceding_checkpoint_generation: previous.checkpoint_generation,
        preceding_floor_generation: previous.floor_generation,
        command_cut: previous.command_cut.clone().unwrap(),
    });
    let mut wrong_generation = valid.clone();
    if let Some(HistoryRequest::Floor {
        preceding_floor_generation,
        ..
    }) = &mut wrong_generation.transition
    {
        *preceding_floor_generation += 1;
    }
    assert!(
        bundle
            .command(&wrong_generation, &core, &inputs, &[])
            .is_err()
    );
    committed(bundle.command(&valid, &core, &inputs, &[]).unwrap());
    assert_eq!(
        bundle
            .checkpoint(bundle.manual_branch_id())
            .unwrap()
            .floor_generation,
        2
    );
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
}
