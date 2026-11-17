#![cfg(all(feature = "eval-harness", target_os = "linux"))]
use kinewright_agent::{BranchApplyOutcome, DurableTimelineBranch, TimelineBranch};
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::ObjectRef,
    io_observer,
    receipts::{CommandEnvelope, CommandId, CommandReceipt, ReceiptLookup},
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
    let core = Core::for_test(Document::default()).unwrap();
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
    let mut bundle = ProjectBundle::activate(
        path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let reference = inputs.reference.clone();
    bundle.save(&core, &reference).unwrap();
    (core, bundle, reference)
}
fn reopen(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let mut loaded = LoadedBundle::read(
        path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let core = loaded.open_manual().unwrap();
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
        switch: None,
        context: None,
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-durable-base",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let original = core.export_history().unwrap();
    let legacy = TimelineBranch::new(
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
        "counter-only",
        revision(&core),
        original.history().document.clone(),
    )
    .unwrap();
    apply(&legacy.core(), Command::Do(lock(true)));
    let mut branch = DurableTimelineBranch::fork_probe(&mut bundle, &core, "durable").unwrap();
    apply(&branch.core(), Command::Do(lock(true)));
    apply(&branch.core(), Command::Do(lock(false)));
    apply(&branch.core(), Command::Undo);
    let proposed =
        serde_json::to_value(&branch.core().export_history().unwrap().history().document).unwrap();
    let branch_id = branch.branch_id().clone();
    let base_ref = branch.persist(&mut bundle).unwrap();
    let frozen_delta = vec![lock(true)];
    assert_eq!(
        &*branch.core().operations(false).unwrap(),
        frozen_delta,
        "undone work is not a merge delta"
    );
    drop(branch);
    drop(bundle);
    drop(core);
    let (core, mut bundle, reopened_inputs) = reopen(&path);
    assert_eq!(inputs, reopened_inputs);
    assert_eq!(
        revision(&core),
        TimelineRevision(4),
        "COLD_OPEN_PRESERVES_STORED_REVISION"
    );
    let fresh_runtime =
        Core::for_test_at_revision(original.history().document.clone(), TimelineRevision(1))
            .unwrap();
    assert!(
        matches!(
            legacy.merge_into(&fresh_runtime).unwrap(),
            BranchApplyOutcome::Conflict {
                expected: TimelineRevision(4),
                actual: TimelineRevision(1)
            }
        ),
        "literal old numeric-base control"
    );
    assert_eq!(core.export_history().unwrap(), original);
    let branch =
        DurableTimelineBranch::reopen_probe(&bundle, branch_id, &base_ref, "durable").unwrap();
    assert_eq!(&*branch.core().operations(false).unwrap(), &frozen_delta);
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    assert_eq!(envelope.expected_revision, revision(&core));
    committed(branch.merge(&mut bundle, &core, &envelope).unwrap());
    assert_eq!(
        serde_json::to_value(&core.export_history().unwrap().history().document).unwrap(),
        proposed
    );
    assert_eq!(
        core.export_history().unwrap().history().undo.len(),
        original.history().undo.len() + 1
    );
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let restored = loaded.open_manual().unwrap();
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
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-head-conflict",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut branch = DurableTimelineBranch::fork_probe(&mut bundle, &core, "durable").unwrap();
    apply(&branch.core(), Command::Do(lock(true)));
    branch.persist(&mut bundle).unwrap();
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    let original = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    let old_counter = revision(&core);
    let same_document_capture = core.export_history().unwrap();
    let same_document = same_document_capture.history().document.clone();
    drop(same_document_capture);
    let legacy = TimelineBranch::new(
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
        "counter-only",
        old_counter,
        same_document.clone(),
    )
    .unwrap();
    apply(&legacy.core(), Command::Do(lock(true)));
    apply(&core, Command::Do(lock(false)));
    bundle.save(&core, &inputs).unwrap();
    let current = bundle.checkpoint(bundle.manual_branch_id()).unwrap();
    assert_eq!(current.document_ref.sha256, original.document_ref.sha256);
    assert_ne!(current.head_id, original.head_id);
    let other_runtime = Core::for_test_at_revision(same_document, old_counter).unwrap();
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
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let fresh = loaded.open_manual().unwrap();
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
#[allow(
    clippy::too_many_lines,
    reason = "the cold branch witness checks complete native history and the first edit in one lifecycle"
)]
fn restored_lineage_tracks_siblings_reuse_and_undone_splits() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-lineage",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let add = Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(0),
        duration: TimeCode(30),
        color: SolidColor::default(),
    };
    apply(&core, Command::Do(add.clone()));
    bundle.save(&core, &inputs).unwrap();
    let manual = bundle.manual_branch_id().clone();
    let origin = bundle
        .branch_lineage(&manual)
        .unwrap()
        .lineage()
        .occurrences[&ClipId(1)]
        .clone();
    let a = bundle
        .fork_branch(
            &manual,
            kinewright_project::operation_history::Role::Ordinary,
        )
        .unwrap();
    let b = bundle
        .fork_branch(
            &manual,
            kinewright_project::operation_history::Role::Ordinary,
        )
        .unwrap();
    let c = bundle
        .fork_branch(
            &manual,
            kinewright_project::operation_history::Role::Ordinary,
        )
        .unwrap();
    let d = bundle
        .fork_branch(
            &manual,
            kinewright_project::operation_history::Role::Ordinary,
        )
        .unwrap();
    let source_attachment = bundle.attach_core(&core).unwrap();
    let source_cut = source_attachment.freeze().unwrap();
    source_attachment.retire(&source_cut).unwrap();
    let make = |id: &_| {
        let checkpoint = bundle.checkpoint(id).unwrap();
        Core::open(
            checkpoint
                .open(bundle.store(), bundle.history_pool())
                .unwrap(),
            checkpoint.branch_id.clone(),
            checkpoint.revision,
        )
        .unwrap()
    };
    let first = make(&a);
    let second = make(&b);
    apply(
        &first,
        Command::DoBatch(vec![Operation::DeleteClip { clip: ClipId(1) }, add]),
    );
    bundle.save_branch(&a, &first).unwrap();
    let reused = bundle.branch_lineage(&a).unwrap().lineage().occurrences[&ClipId(1)].clone();
    assert_ne!(reused, origin);
    let split = Command::Do(Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(15),
    });
    apply(&first, split.clone());
    bundle.save_branch(&a, &first).unwrap();
    let child = bundle.branch_lineage(&a).unwrap().lineage().occurrences[&ClipId(2)].clone();
    apply(&second, split);
    bundle.save_branch(&b, &second).unwrap();
    assert_ne!(
        child,
        bundle.branch_lineage(&b).unwrap().lineage().occurrences[&ClipId(2)]
    );
    apply(&first, Command::Undo);
    bundle.save_branch(&a, &first).unwrap();
    let undone = bundle.branch_lineage(&a).unwrap();
    assert_eq!(undone.lineage().occurrences[&ClipId(1)], reused);
    assert!(undone.lineage().split_ancestry.is_empty());
    drop(undone);
    drop(bundle);
    let (_, mut bundle, _) = reopen(&path);
    assert_eq!(
        bundle.branch_history(&c).unwrap().into_capture().unwrap(),
        bundle.branch_history(&d).unwrap().into_capture().unwrap(),
        "shared immutable history decodes once"
    );
    let before = bundle
        .branch_lineage(&a)
        .unwrap()
        .lineage()
        .occurrences
        .clone();
    assert_eq!(before[&ClipId(1)], reused);
    apply(&first, Command::Redo);
    bundle.save_branch(&a, &first).unwrap();
    assert_eq!(
        bundle.branch_lineage(&a).unwrap().lineage().occurrences[&ClipId(2)],
        child
    );
    let lineage = bundle.branch_lineage(&a).unwrap();
    assert_eq!(lineage.lineage().split_ancestry.len(), 2);
    assert!(
        lineage
            .lineage()
            .split_ancestry
            .iter()
            .all(|(_, parent, _)| *parent == reused)
    );
}

#[test]
fn merge_lost_reply_preserves_one_undo_entry() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-merge-retry",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut branch = DurableTimelineBranch::fork_probe(&mut bundle, &core, "durable").unwrap();
    apply(
        &branch.core(),
        Command::DoBatch(vec![lock(true), lock(false)]),
    );
    let base_ref = branch.persist(&mut bundle).unwrap();
    let id = branch.branch_id().clone();
    let envelope = prepare_merge(&branch, &mut bundle, &core, &inputs);
    let original = committed(branch.merge(&mut bundle, &core, &envelope).unwrap());
    assert_eq!(core.export_history().unwrap().history().undo.len(), 3);
    drop(branch);
    drop(bundle);
    drop(core);
    let (core, mut bundle, inputs) = reopen(&path);
    let before = core.export_history().unwrap();
    let branch = DurableTimelineBranch::reopen_probe(&bundle, id, &base_ref, "durable").unwrap();
    let (result, trace) = io_observer::observe(|| branch.merge(&mut bundle, &core, &envelope));
    assert_eq!(committed(result.unwrap()), original);
    assert!(trace.events.is_empty());
    assert_eq!(core.export_history().unwrap(), before);
    drop(branch);
    drop(bundle);
    drop(core);
    exercise_native_gesture_close(&temp.path("close.kinewright"));
    let _ = inputs;
}

#[allow(
    clippy::too_many_lines,
    reason = "one ordered gesture, receipt retry and closure witness"
)]
fn exercise_native_gesture_close(path: &Path) {
    let (core, mut bundle, inputs) = setup(path);
    let drag = |locked| JournalCommand::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "K".into(),
    };
    let b = command(&mut bundle, &core, &inputs, drag(true));
    committed(bundle.command(&b, &core, &inputs, &[]).unwrap());
    let c = command(&mut bundle, &core, &inputs, drag(false));
    let committed_c = committed(bundle.command(&c, &core, &inputs, &[]).unwrap());
    let before = core.export_history().unwrap();
    assert_eq!(before.history().undo.len(), 3);
    assert_eq!(before.history().undo[2].commands().len(), 2);
    drop(before);
    core.close_gesture().unwrap();
    bundle.save(&core, &inputs).unwrap();
    assert!(
        core.export_history().unwrap().history().undo[2]
            .coalesce_key()
            .is_none()
    );
    let (retried, trace) = io_observer::observe(|| bundle.command(&c, &core, &inputs, &[]));
    assert_eq!(committed(retried.unwrap()), committed_c);
    assert!(trace.events.is_empty());
    let expected = core.snapshot_identity().unwrap();
    drop(bundle);
    drop(core);
    let (core, mut bundle, inputs) = reopen(path);
    assert_eq!(core.snapshot_identity().unwrap(), expected);
    let d = command(&mut bundle, &core, &inputs, drag(true));
    committed(bundle.command(&d, &core, &inputs, &[]).unwrap());
    assert_eq!(
        core.export_history().unwrap().history().undo.len(),
        4,
        "EXPLICIT_CLOSE_SURVIVES_COLD_OPEN"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the cold branch witness checks complete native history and the first edit in one lifecycle"
)]
fn never_fold_specialist_reopens_original_full_delta_and_merges_once() {
    let temp = TempDirectory::new_in(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "r2-specialist-floor",
    );
    let path = temp.path("edit.kinewright");
    let document = Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let live = Core::for_test(document).unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        live.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&live, &inputs).unwrap();
    owner
        .restrict_limits_probe(kinewright_project::storage_budget::StorageLimits {
            ..Default::default()
        })
        .unwrap();
    let mut specialist =
        DurableTimelineBranch::fork_probe(&mut owner, &live, "floor specialist").unwrap();
    for i in 0..40 {
        apply(&specialist.core(), Command::Do(lock(i % 2 == 0)));
    }
    let proposal = serde_json::to_value(
        &specialist
            .core()
            .export_history()
            .unwrap()
            .history()
            .document,
    )
    .unwrap();
    let reference = specialist.persist(&mut owner).unwrap();
    let branch = specialist.branch_id().clone();
    let history = specialist.core().export_history().unwrap();
    assert!(
        history.fold_frontier().is_none(),
        "NEVER_FOLD_SPECIALIST_HAS_NO_FOLD_FRONTIER"
    );
    assert_eq!(
        history,
        owner
            .branch_history(&branch)
            .unwrap()
            .into_capture()
            .unwrap()
    );
    assert_eq!(
        specialist.core().operations(false).unwrap().len(),
        40,
        "floor truncated the merge delta"
    );
    drop(specialist);
    drop(owner);
    drop(live);
    let (live, mut owner, inputs) = reopen(&path);
    let mut specialist =
        DurableTimelineBranch::reopen_probe(&owner, branch, &reference, "reopened")
            .expect("floor made the persisted specialist unreopenable");
    assert_eq!(specialist.core().export_history().unwrap(), history);
    apply(&specialist.core(), Command::Do(lock(true)));
    apply(&specialist.core(), Command::Undo);
    let reference = specialist.persist(&mut owner).unwrap();
    assert_eq!(specialist.core().operations(false).unwrap().len(), 40);
    assert_eq!(
        serde_json::to_value(
            &specialist
                .core()
                .export_history()
                .unwrap()
                .history()
                .document
        )
        .unwrap(),
        proposal
    );
    let issuer = owner.issue_issuer(&live, &inputs, 1).unwrap();
    let envelope = specialist
        .merge_envelope(
            &owner,
            &live,
            CommandId {
                issuer,
                sequence: 1,
            },
        )
        .unwrap();
    let result = committed(specialist.merge(&mut owner, &live, &envelope).unwrap());
    assert_eq!(
        committed(specialist.merge(&mut owner, &live, &envelope).unwrap()),
        result
    );
    assert_eq!(
        serde_json::to_value(&live.export_history().unwrap().history().document).unwrap(),
        proposal
    );
    assert_eq!(live.export_history().unwrap().history().undo.len(), 1);
    owner.store().verify(&reference).unwrap();
}
