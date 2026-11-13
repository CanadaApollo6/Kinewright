//! Real Core/store boundaries and the production floor prefilter.
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
#[cfg(target_os = "linux")]
use kinewright_media::test_support::TempDirectory;
#[cfg(target_os = "linux")]
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
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
fn core(tracks: u64) -> Core {
    Core::spawn(Document {
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
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
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
    if policy.check(history).is_ok() {
        return Some(history.clone());
    }
    let mut first: Option<HistorySnapshot> = None;
    let mut maximum_depth = 0;
    for cut in 1..history.batches.len() {
        let Ok(candidate) = HistoryTransition::floor_after(history, cut) else {
            continue;
        };
        if policy.check(&candidate).is_err() {
            continue;
        }
        if first.is_none() {
            maximum_depth = candidate
                .cursor
                .undo
                .len()
                .saturating_sub(candidate.cursor.undo.len().div_ceil(8))
                .max(1);
        }
        if candidate.cursor.undo.len() <= maximum_depth {
            return Some(candidate);
        }
        first.get_or_insert(candidate);
    }
    first
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
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let branch = if specialist {
        owner
            .fork_branch(
                &owner.manual_branch_id().clone(),
                kinewright_project::history_record::HistoryRole::Ordinary,
            )
            .unwrap()
    } else {
        owner.manual_branch_id().clone()
    };
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    let before = core.export_history().unwrap();
    let selected = bound_history_probe(&before, &limits()).unwrap();
    assert!(selected.floor_offset > 0);
    let result = io_observer::fail_once("root_parent_sync", || {
        if specialist {
            owner.save_branch(&branch, &core)
        } else {
            owner.save(&core, &inputs)
        }
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnknown)));
    assert_eq!(core.export_history().unwrap(), before);
    edit(&core, false);
    apply(&core, Command::Undo);
    apply(&core, Command::Undo);
    let current = core.export_history().unwrap();
    let expected =
        HistoryTransition::floor(current, usize::try_from(selected.floor_offset).unwrap())
            .unwrap()
            .after()
            .clone();
    owner.reconcile().expect("moving Core must settle");
    assert_eq!(core.export_history().unwrap(), expected);
    if specialist {
        owner.save_branch(&branch, &core).unwrap();
    } else {
        owner.save(&core, &inputs).unwrap();
    }
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(&branch).unwrap()
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
    let selected = bound_history_probe(&core.export_history().unwrap(), &limits()).unwrap();
    let mut copy = io_observer::fail_once("root_parent_sync", || {
        session.first_save_with_limits(&path, &core, &inputs.reference, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    edit(&core, false);
    apply(&core, Command::Undo);
    let expected = HistoryTransition::floor(
        core.export_history().unwrap(),
        usize::try_from(selected.floor_offset).unwrap(),
    )
    .unwrap()
    .after()
    .clone();
    copy.reconcile()
        .expect("moving first-save Core must settle");
    assert_eq!(core.export_history().unwrap(), expected);
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
        *owner.branch_history(owner.manual_branch_id()).unwrap()
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
    assert!(
        bound_history_probe(&before, &limits())
            .unwrap()
            .floor_offset
            > 0
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
    let copy = session
        .first_save_with_limits(&path, &core, &inputs.reference, limits())
        .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("retry unsettled"));
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(owner.manual_branch_id()).unwrap()
    );
}
#[cfg(target_os = "linux")]
fn check_all_entries(core: &Core, before: &HistorySnapshot) {
    for entry in before.cursor.undo.iter().rev() {
        apply(core, Command::Undo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Undo target lost"
        );
    }
    for _ in &before.cursor.undo {
        apply(core, Command::Redo);
    }
    assert_eq!(core.export_history().unwrap().document, before.document);
    for entry in before.cursor.redo.iter().rev() {
        apply(core, Command::Redo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Redo target lost"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn older_root_releases_every_entry() {
    let temp = temp("r4-older");
    let path = temp.path("edit.kinewright");
    let core = core(200);
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let old = std::fs::read(&path).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    assert!(
        bound_history_probe(&before, &limits())
            .unwrap()
            .floor_offset
            > 0
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
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    grow(&core);
    let before = core.export_history().unwrap();
    let selected = bound_history_probe(&before, &limits()).unwrap();
    let dropped = before.cursor.undo.len() - selected.cursor.undo.len();
    assert!(dropped > 0);
    let witnessed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = witnessed.clone();
    let moving = core.clone();
    let floor_offset = selected.floor_offset;
    io_observer::at_barrier(
        "bundle_B0",
        move || {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            for _ in 0..selected.cursor.undo.len() {
                apply(&moving, Command::Undo);
            }
            let boundary = moving.export_history().unwrap();
            assert!(matches!(
                moving.request(Command::Undo).unwrap(),
                Event::UndoBlockedBySave { .. }
            ));
            assert_eq!(
                moving.export_history().unwrap(),
                boundary,
                "Undo crossed in-flight floor"
            );
            assert_eq!(
                boundary.cursor.undo.len(),
                dropped,
                "entries were dropped eagerly"
            );
            edit(&moving, false);
        },
        || owner.save(&core, &inputs),
    )
    .unwrap();
    assert!(witnessed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(core.export_history().unwrap().floor_offset, floor_offset);
    owner.save(&core, &inputs).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(owner.manual_branch_id()).unwrap()
    );
}

fn excursion(edits: usize, undos: usize, tail: usize) -> HistorySnapshot {
    let core = core(1);
    for i in 0..edits {
        edit(&core, i % 2 == 0);
    }
    for _ in 0..undos {
        apply(&core, Command::Undo);
    }
    for _ in 0..undos {
        apply(&core, Command::Redo);
    }
    for i in 0..tail {
        edit(&core, i % 2 == 0);
    }
    core.export_history().unwrap()
}
#[test]
fn prefilter_small_excursion_keeps_low_water_cut() {
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
    assert_eq!(compare(&history, &limits).unwrap().floor_offset, 2);
}
#[test]
fn prefilter_large_excursion_keeps_low_water_suffix() {
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
    println!(
        "R4_EXCURSION bound_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(actual, exhaustive(&history, &limits).unwrap());
    assert_eq!(actual.floor_offset, 88);
    assert_eq!(actual.cursor.undo.len(), 17);
}

#[test]
fn production_prefilter_matches_six_hundred_seeded_histories_fast() {
    seeded_prefilter_histories(600);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn production_prefilter_matches_ten_thousand_seeded_histories() {
    seeded_prefilter_histories(10_000);
}

fn seeded_prefilter_histories(cases: usize) {
    const SEED: u64 = 0x4a41_3153_3152_3401;
    let mut rng = SEED;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    for case in 0..cases {
        let core = core(1);
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
            apply(&core, command);
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
        if case + 1 == cases {
            println!("R4_DIFFERENTIAL seed={SEED:#x} histories={cases}");
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn exact_cap_and_one_byte_below_publish_the_shared_low_water_floor() {
    let history = excursion(3, 0, 0);
    let first = HistoryTransition::floor(history.clone(), 1)
        .unwrap()
        .after()
        .clone();
    let cap = history_cost_probe(&first, &StorageLimits::default()).unwrap();
    for allowance in [cap, cap - 1] {
        let temp = temp("r4-exact-cap");
        let path = temp.path("edit.kinewright");
        let core = core(1);

        let limits = StorageLimits {
            branch_history_bytes: allowance,
            ..StorageLimits::default()
        };
        let selected = compare(&history, &limits).unwrap();
        assert_eq!(selected.floor_offset, 2);
        assert_eq!(
            kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits)
                .unwrap()
                .check(&first)
                .is_ok(),
            allowance == cap,
            "EXACT_CAP_ADMISSION_REMAINS_STRICT_BEFORE_LOW_WATER_SELECTION"
        );
        let mut owner =
            ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner.save(&core, &inputs).unwrap();
        owner.restrict_limits_probe(limits).unwrap();
        owner
            .attach_core(&core)
            .unwrap()
            .restore_history(history.clone())
            .unwrap();
        owner
            .save(&core, &inputs)
            .expect("exact-cap bounded history must save");
        assert_eq!(core.export_history().unwrap(), selected);
        drop(owner);
        let reopened = self::core(1);
        LoadedBundle::read(&path)
            .unwrap()
            .restore_manual(&reopened)
            .unwrap();
        assert_eq!(reopened.export_history().unwrap(), selected);
    }
}

#[test]
fn prefilter_zero_undo_counters_cross_digit_boundaries() {
    for count in [12, 102, 1002, 10002] {
        let core = core(1);
        for _ in 0..count {
            apply(&core, Command::Undo);
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
