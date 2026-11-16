//! Real Core/store boundaries and the production floor prefilter.
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
#[cfg(target_os = "linux")]
use kinewright_media::test_support::TempDirectory;
#[cfg(target_os = "linux")]
use kinewright_project::operation_actor::Core;
#[cfg(target_os = "linux")]
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, ProjectBundle},
    bundle_copy::{CopyStatus, SessionStore},
    io_observer,
    owned_inputs::OwnedInputManifest,
};
use kinewright_project::{
    bundle_commit::{bound_history_probe, history_cost_probe as live_history_cost_probe},
    storage_budget::StorageLimits,
};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::time::Instant;

fn history_cost_probe(
    history: &HistorySnapshot,
    limits: &StorageLimits,
) -> Result<u64, kinewright_project::bundle::BundleError> {
    let journal = HistoryJournal::capture(history).unwrap();
    let replay = kinewright_core::history_size::measure_replay_peak(
        &journal,
        &history.baseline,
        &history.document,
    )
    .unwrap();
    Ok(live_history_cost_probe(history, limits)?
        .max(
            kinewright_project::bundle_commit::ordinary_history_admission_cost_probe(
                history, limits,
            )?,
        )
        .max(replay))
}

#[cfg(target_os = "linux")]
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
#[cfg(target_os = "linux")]
fn core(tracks: u64) -> Core {
    Core::for_test(Document {
        tracks: (1..=tracks)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    })
    .unwrap()
}
#[cfg(target_os = "linux")]
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[cfg(target_os = "linux")]
fn edit(core: &Core, locked: bool) {
    apply(
        core,
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }),
    );
}
#[cfg(target_os = "linux")]
fn grow(core: &Core) {
    for i in 0..40 {
        edit(core, i % 2 == 0);
    }
}
#[cfg(target_os = "linux")]
fn limits() -> StorageLimits {
    StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    }
}
fn exhaustive(history: &HistorySnapshot, limits: &StorageLimits) -> Option<HistorySnapshot> {
    let policy =
        kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(limits).unwrap();
    let fits = |h: &HistorySnapshot| policy.check(h).is_ok();
    if fits(history) {
        return Some(history.clone());
    }
    (1..=history.batches.len()).find_map(|cut| {
        HistoryTransition::floor(history.clone(), cut)
            .ok()
            .map(|t| t.after().clone())
            .filter(fits)
    })
}
fn compare(history: &HistorySnapshot, limits: &StorageLimits) -> Option<HistorySnapshot> {
    let expected = exhaustive(history, limits);
    let actual = bound_history_probe(history, limits).ok();
    assert_eq!(
        actual, expected,
        "production prefilter disagrees at cap {}",
        limits.branch_history_bytes
    );
    actual
}

#[cfg(target_os = "linux")]
fn moving_core(specialist: bool) {
    let temp = temp("r4-moving");
    let path = temp.path("edit.kinewright");
    let core = core(200);
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let branch = if specialist {
        owner
            .fork_branch(
                &owner.manual_branch_id().clone(),
                kinewright_project::operation_history::Role::Ordinary,
            )
            .unwrap()
    } else {
        owner.manual_branch_id().clone()
    };
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    let before = core.snapshot_identity().unwrap();
    let result = io_observer::fail_once("root_parent_sync", || {
        if specialist {
            owner.save_branch(&branch, &core)
        } else {
            owner.save(&core, &inputs)
        }
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnknown)));
    assert_eq!(core.snapshot_identity().unwrap(), before);
    edit(&core, false);
    apply(&core, Command::Undo);
    apply(&core, Command::Undo);
    let expected = core.snapshot_identity().unwrap();
    owner.reconcile().expect("moving Core must settle");
    assert_eq!(core.snapshot_identity().unwrap(), expected);
    if specialist {
        owner.save_branch(&branch, &core).unwrap();
    } else {
        owner.save(&core, &inputs).unwrap();
    }
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(&branch)
            .unwrap()
            .into_capture()
            .unwrap()
    );
}
#[test]
#[cfg(target_os = "linux")]
fn moving_manual_core_reconciles_and_saves() {
    moving_core(false);
}
#[test]
#[cfg(target_os = "linux")]
fn moving_specialist_core_reconciles_and_saves() {
    moving_core(true);
}

#[test]
#[cfg(target_os = "linux")]
fn first_copy_moving_core_reconciles_and_saves() {
    let temp = temp("r4-copy-moving");
    let core = core(200);
    grow(&core);
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let inputs = session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries: vec![],
        })
        .unwrap();
    let path = temp.path("first.kinewright");
    let _frontier = core.export_history().unwrap().fold_frontier();
    let mut copy = io_observer::fail_once("root_parent_sync", || {
        session.first_save_with_limits(&path, &core, &inputs.reference, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    edit(&core, false);
    apply(&core, Command::Undo);
    let expected = core.snapshot_identity().unwrap();
    copy.reconcile()
        .expect("moving first-save Core must settle");
    assert_eq!(core.snapshot_identity().unwrap(), expected);
    let mut owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("copy unsettled"));
    let inputs = owner
        .checkpoint(owner.manual_branch_id())
        .unwrap()
        .input_manifest_ref
        .clone();
    owner.save(&core, &inputs).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap()
    );
}

#[test]
#[cfg(target_os = "linux")]
fn first_failure_releases_every_original_undo_and_redo_then_retries() {
    let temp = temp("r4-first-failure");
    let core = core(200);
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    assert_eq!(
        before.history().undo.len() + before.history().redo.len(),
        40
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let inputs = session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries: vec![],
        })
        .unwrap();
    let path = temp.path("first.kinewright");
    assert!(
        io_observer::fail_once("copy_verify", || session.first_save_with_limits(
            &path,
            &core,
            &inputs.reference,
            limits()
        ))
        .is_err()
    );
    assert_eq!(core.export_history().unwrap(), before);
    assert!(!path.exists());
    check_all_entries(&core, &before);
    drop(before);
    let copy = session
        .first_save_with_limits(&path, &core, &inputs.reference, limits())
        .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("retry unsettled"));
    super::round_two::assert_selected_epoch(
        &core.export_history().unwrap(),
        &owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap(),
    );
}
#[cfg(target_os = "linux")]
fn check_all_entries(core: &Core, before: &kinewright_project::operation_history::OwnerSnapshot) {
    // Independent operation replay, including the saved Redo tail.
    let mut document = before.history().baseline.as_ref().clone();
    let mut boundaries = vec![document.clone()];
    for step in before
        .history()
        .undo
        .iter()
        .chain(before.history().redo.iter().rev())
    {
        for command in step.commands() {
            for operation in &command.operations {
                operation.apply(&mut document).unwrap();
            }
        }
        boundaries.push(document.clone());
    }
    let depth = before.history().undo.len();
    for at in (0..depth).rev() {
        apply(core, Command::Undo);
        assert_eq!(
            core.export_history().unwrap().history().document.as_ref(),
            &boundaries[at],
            "ORIGINAL_UNDO_TARGET_SURVIVES_PUBLICATION_FAILURE"
        );
    }
    for _ in 0..depth {
        apply(core, Command::Redo);
    }
    assert_eq!(
        core.export_history().unwrap().history().document,
        before.history().document
    );
    for document in &boundaries[depth + 1..] {
        apply(core, Command::Redo);
        assert_eq!(
            core.export_history().unwrap().history().document.as_ref(),
            document,
            "ORIGINAL_REDO_TARGET_SURVIVES_PUBLICATION_FAILURE"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn older_root_releases_every_entry() {
    let temp = temp("r4-older");
    let path = temp.path("edit.kinewright");
    let core = core(200);
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let old = std::fs::read(&path).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    assert_eq!(
        before.history().undo.len() + before.history().redo.len(),
        40
    );
    assert!(matches!(
        io_observer::fail_once("root_rename_io", || owner.save(&core, &inputs)),
        Err(BundleError::DurabilityUnknown)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), old);
    owner.reconcile().unwrap();
    assert_eq!(core.export_history().unwrap(), before);
    check_all_entries(&core, &before);
    owner.save(&core, &inputs).unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn command_lands_mid_save_above_pending_floor() {
    let temp = temp("r4-mid-save");
    let path = temp.path("edit.kinewright");
    let core = core(200);
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    let before = core.export_history().unwrap();
    let depth = before.history().undo.len();
    let before_identity = before.identity().unwrap();
    drop(before);
    let witnessed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::clone(&witnessed);
    let moving = core.clone();
    let position = core.change_id().unwrap().position;
    io_observer::at_barrier(
        "bundle_B0",
        move || {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            for _ in 0..=depth {
                apply(&moving, Command::Undo);
            }
            edit(&moving, false);
        },
        || owner.save(&core, &inputs),
    )
    .unwrap();
    assert!(witnessed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        core.change_id().unwrap().position.position,
        position.position + depth as u64 + 2,
        "CONCURRENT_SAVE_DOES_NOT_BLOCK_OR_DROP_LATER_COMMANDS"
    );
    assert_eq!(
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .identity()
            .unwrap(),
        before_identity,
        "SAVE_PUBLISHES_THE_COHERENT_CAPTURED_CUT"
    );
    owner.save(&core, &inputs).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap()
    );
}

fn legacy_core() -> kinewright_core::Core {
    kinewright_core::Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap()
}
fn legacy_apply(core: &kinewright_core::Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn legacy_edit(core: &kinewright_core::Core, locked: bool) {
    legacy_apply(
        core,
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }),
    );
}
fn excursion(edits: usize, undos: usize, tail: usize) -> HistorySnapshot {
    let core = legacy_core();
    for i in 0..edits {
        legacy_edit(&core, i % 2 == 0);
    }
    for _ in 0..undos {
        legacy_apply(&core, Command::Undo);
    }
    for _ in 0..undos {
        legacy_apply(&core, Command::Redo);
    }
    for i in 0..tail {
        legacy_edit(&core, i % 2 == 0);
    }
    core.export_history().unwrap()
}
#[test]
fn prefilter_small_excursion_keeps_first_valid_cut() {
    let history = excursion(2, 2, 1);
    let valid: Vec<_> = (1..=7)
        .filter(|cut| HistoryTransition::floor(history.clone(), *cut).is_ok())
        .collect();
    assert_eq!(valid, [1, 2, 6, 7]);
    let first = HistoryTransition::floor(history.clone(), 1)
        .unwrap()
        .after()
        .clone();
    let limits = StorageLimits {
        branch_history_bytes: history_cost_probe(&first, &StorageLimits::default()).unwrap(),
        ..StorageLimits::default()
    };
    assert_eq!(compare(&history, &limits).unwrap().floor_offset, 1);
}
#[test]
fn prefilter_large_excursion_keeps_twenty_undo_steps() {
    let history = excursion(100, 30, 5);
    let expected = HistoryTransition::floor(history.clone(), 85)
        .unwrap()
        .after()
        .clone();
    let limits = StorageLimits {
        branch_history_bytes: history_cost_probe(&expected, &StorageLimits::default()).unwrap(),
        ..StorageLimits::default()
    };
    let started = Instant::now();
    let actual = bound_history_probe(&history, &limits).unwrap();
    eprintln!(
        "R4_EXCURSION bound_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(actual, exhaustive(&history, &limits).unwrap());
    assert_eq!(actual.floor_offset, 85);
    assert_eq!(actual.cursor.undo.len(), 20);
}

#[test]
fn production_prefilter_matches_ten_thousand_seeded_histories() {
    const SEED: u64 = 0x4a41_3153_3152_3401;
    let mut rng = SEED;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    for case in 0..10_000 {
        let core = legacy_core();
        let count = 1 + next() % 18;
        for _ in 0..count {
            let value = next();
            let operation = Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: value & 1 == 0,
            };
            let command = match value % 6 {
                0 => Command::Undo,
                1 => Command::Redo,
                2 | 3 => Command::DoBatchCoalesced {
                    operations: vec![operation],
                    coalesce_key: format!("gesture-{}", value % 3),
                },
                _ => Command::Do(operation),
            };
            legacy_apply(&core, command);
        }
        let history = core.export_history().unwrap();
        let target = 1 + usize::try_from(next()).unwrap() % history.batches.len();
        let candidate = HistoryTransition::floor(history.clone(), target).ok();
        let charge = history_cost_probe(
            candidate
                .as_ref()
                .map_or(&history, HistoryTransition::after),
            &StorageLimits::default(),
        )
        .unwrap();
        let limits = StorageLimits {
            branch_history_bytes: charge.saturating_sub(next() % 17),
            ..StorageLimits::default()
        };
        compare(&history, &limits);
        if case == 9_999 {
            eprintln!("R4_DIFFERENTIAL seed={SEED:#x} histories=10000");
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn exact_cap_saves_and_one_byte_over_floors_one_more_step() {
    let history = excursion(3, 0, 0);
    let first = HistoryTransition::floor(history.clone(), 1)
        .unwrap()
        .after()
        .clone();
    let cap = history_cost_probe(&first, &StorageLimits::default()).unwrap();
    for (allowance, floor) in [(cap, 1), (cap - 1, 2)] {
        let temp = temp("r4-exact-cap");
        let _path = temp.path("edit.kinewright");
        let core = legacy_core();

        let limits = StorageLimits {
            branch_history_bytes: allowance,
            ..StorageLimits::default()
        };
        let selected = compare(&history, &limits).unwrap();
        assert_eq!(selected.floor_offset, floor);
        core.restore_history(selected.clone()).unwrap();
        assert_eq!(
            core.export_history().unwrap(),
            selected,
            "LEGACY_EXACT_CAP_TRANSFORMATION_ORACLE"
        );
    }
}

#[test]
fn prefilter_zero_undo_counters_cross_digit_boundaries() {
    for count in [12, 102, 1002, 10002] {
        let core = legacy_core();
        for _ in 0..count {
            legacy_apply(&core, Command::Undo);
        }
        let history = core.export_history().unwrap();
        let expected = HistoryTransition::floor(history.clone(), 1)
            .unwrap()
            .after()
            .clone();
        let limits = StorageLimits {
            branch_history_bytes: history_cost_probe(&expected, &StorageLimits::default()).unwrap(),
            ..StorageLimits::default()
        };
        let actual = compare(&history, &limits).unwrap();
        assert_eq!(
            actual.floor_offset, 1,
            "decimal boundary lost a valid first cut"
        );
        assert_eq!(actual.batches.len(), count - 1);
    }
}
