use super::Ja1EditingSession;
use kinewright_core::{
    Command, Core, Document, Event, HistoryTransition, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_commit::history_cost_probe,
    bundle_records::BranchId,
    bundle_switch::ProjectSelectionResult,
    io_observer,
    manual_history::OrdinaryHistoryLimits,
    storage_budget::StorageLimits,
};
fn edit(core: &Core, locked: bool) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn grow(core: &Core, n: usize) {
    for i in 0..n {
        edit(core, i % 2 == 0);
    }
}
fn source(profile: StorageLimits) -> (TempDirectory, Ja1EditingSession, Vec<BranchId>, BranchId) {
    let temp = TempDirectory::new_in(super::durable_test_root(), "c5-pressure");
    let a = temp.path("drive/a.kinewright");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    let doc = Document {
        tracks: (1..=8)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::spawn(doc).unwrap();
    grow(&core, 8);
    let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
    let input = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &input).unwrap();
    let left = bundle.manual_branch_id().clone();
    let mut ids = vec![];
    for n in 0..24 {
        let id = bundle
            .fork_branch(
                &left,
                kinewright_project::history_record::HistoryRole::Ordinary,
            )
            .unwrap();
        let other = Core::spawn((*bundle.branch_history(&id).unwrap().document).clone()).unwrap();
        grow(&other, 24 + n);
        bundle.save_branch(&id, &other).unwrap();
        ids.push(id);
    }
    let mut session = Ja1EditingSession::from_bundle(bundle, &core, input).unwrap();
    session.save_active().unwrap();
    edit(&core, true);
    io_observer::fail_once("root_parent_sync", || session.save_active()).unwrap_err();
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let selection = session.preview_rescue(capture, profile).unwrap();
    let consent = selection.consent();
    let inspection = session
        .inspect_rescue_destination(&temp.path("b.kinewright"))
        .unwrap();
    assert!(
        session
            .rescue(selection, &consent, &inspection)
            .unwrap()
            .activated
    );
    let point = session.list_recovery_points().unwrap()[0].clone();
    edit(session.core(), false);
    session.prepare_recovery(point).unwrap();
    assert!(matches!(
        session.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    grow(session.core(), 20);
    session.save_active().unwrap();
    (temp, session, ids, left)
}
fn large_edit(core: &Core, label_bytes: usize) {
    // Grow the immutable document, independently of journal representation.
    let ops = vec![Operation::AddMarker {
        marker: kinewright_core::Marker {
            id: kinewright_core::MarkerId(9000),
            position: kinewright_core::TimeCode(0),
            label: "m".repeat(label_bytes),
            color_token: 0,
        },
    }];
    assert!(matches!(
        core.request(Command::DoBatch(ops)).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[test]
fn w13_sparse_cache_capacity_smoke() {
    // Its cap must also reopen all 24 retained branches. A 64 KiB marker
    // left only 27 KiB between the source and candidate roots; retain a wider
    // separation so random branch traversal order cannot decide this fixture.
    pressure_witness(1 << 17);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
pub(super) fn w13_definite_rescue_sparse_cache_growing_saves_and_same_pin_quota_retry() {
    pressure_witness(1 << 19);
}

fn pressure_witness(label_bytes: usize) {
    let ((), sync) = io_observer::profile_sync(|| pressure_witness_inner(label_bytes));
    println!(
        "PRESSURE_SYNC={}",
        serde_json::json!({"label_bytes":label_bytes,"profile":sync})
    );
}

fn pressure_witness_inner(label_bytes: usize) {
    let total = std::time::Instant::now();
    let mut phase = std::time::Instant::now();
    let mut timed = |name: &str| {
        println!(
            "PRESSURE_PHASE={}",
            serde_json::json!({"phase":name,"label_bytes":label_bytes,"milliseconds":phase.elapsed().as_secs_f64()*1000.0,"total_ms":total.elapsed().as_secs_f64()*1000.0,"history_bound":io_observer::history_bound_profile(),"publication_fit":io_observer::publication_fit_profile(),"sync":io_observer::sync_profile()})
        );
        phase = std::time::Instant::now();
    };
    let (model_temp, mut model, model_ids, _) = source(StorageLimits::default());
    timed("model-source");
    large_edit(model.core(), label_bytes);
    let candidate = model.core().export_history().unwrap();
    let mut minimum = u64::MAX;
    let mut minimum_retained = u64::MAX;
    for cut in 0..=candidate.batches.len() {
        let history = if cut == 0 {
            candidate.clone()
        } else {
            let Ok(t) = HistoryTransition::floor(candidate.clone(), cut) else {
                continue;
            };
            t.after().clone()
        };
        // Size every legal cut once, keeping the newest accepted step. Repeated
        // selection in a binary search replays these same marker-bearing cuts
        // about 25 times and dominates the unrelated App test binary.
        if !history.batches.is_empty() {
            minimum = minimum.min(history_cost_probe(&history, &StorageLimits::default()).unwrap());
        }
        minimum_retained = minimum_retained
            .min(kinewright_core::history_size::restored_history_live_bytes(&history).unwrap());
    }
    timed("every-legal-floor-calibration");
    // Confirm the independent sizing walk against the real whole predicate at
    // both sides of its boundary. No fixture limit or pressure assertion moves.
    for (cap, fits) in [(minimum, true), (minimum - 1, false)] {
        let profile = StorageLimits {
            branch_history_bytes: cap,
            ..StorageLimits::default()
        };
        assert_eq!(
            OrdinaryHistoryLimits::from_limits(&profile)
                .unwrap()
                .select(&candidate)
                .is_ok(),
            fits,
            "exact canonical cap {cap}"
        );
    }
    timed("boundary-predicate-confirmation");
    // Compare both cuts under the same replay window. The default branch cap
    // otherwise reserves a larger cold-open overlap than this destination profile.
    let before = LoadedBundle::read_with_limits_probe(
        &model_temp.path("b.kinewright"),
        StorageLimits {
            branch_history_bytes: minimum,
            ..StorageLimits::default()
        },
    )
    .unwrap()
    .usage_probe()
    .unwrap()
    .peak_live_history_bytes;
    model
        .restrict_limits_probe(StorageLimits {
            branch_history_bytes: minimum,
            ..StorageLimits::default()
        })
        .unwrap();
    model.save_active().unwrap();
    timed("initial-open-and-flooring-save");
    let candidate = model.core().export_history().unwrap();
    let loaded = LoadedBundle::read(&model_temp.path("b.kinewright")).unwrap();
    let root = loaded.root().unwrap();
    let index=model.store().unwrap().read_record::<std::collections::BTreeMap<BranchId,kinewright_project::bundle_records::ObjectRef>>(&root.branch_index_ref).unwrap();
    let mut costs = std::collections::BTreeMap::new();
    for (id, r) in index.iter() {
        if id == &root.manual_branch_id {
            continue;
        }
        let cp = model
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(r)
            .unwrap();
        let h = model
            .store()
            .unwrap()
            .resolve_history(&cp.history_ref)
            .unwrap();
        costs
            .entry(cp.history_ref.sha256.clone())
            .or_insert(kinewright_core::history_size::restored_history_live_bytes(&h).unwrap());
    }
    let manifest = model
        .store()
        .unwrap()
        .read_record::<kinewright_project::store::StoreManifest>(&root.store_manifest_ref)
        .unwrap();
    assert!(!manifest.recovery.predecessors.is_empty());
    assert!(manifest.objects.iter().any(|reference| {
        reference.record_kind == kinewright_project::bundle_records::RecordKind::BranchCheckpoint
            && !index.values().any(|current| current == reference)
    }));
    drop(manifest);
    // Archived ordinary checkpoints replay after all current histories remain
    // resident. Count those retained histories once and the shared replay
    // window once; summing individual replay peaks would overstate this.
    let irreducible = costs.values().sum::<u64>() + minimum_retained + minimum;
    println!(
        "C5 fresh-open setup peak={before}, no-floor resident lower bound={irreducible}, minimum target={minimum}"
    );
    assert!(
        irreducible > before,
        "before={before} irreducible={irreducible}"
    );
    assert_eq!(model_ids.len(), 24);
    drop(index);
    drop(loaded);
    // Calibrate and publish with this writer's actual branch identities. A new fixture's
    // random BranchId order changes the reader's temporary overlap, even for equal histories.
    for _ in 0..2 {
        model.save_active().unwrap();
    }
    let calibrated = LoadedBundle::read_with_limits_probe(
        &model_temp.path("b.kinewright"),
        StorageLimits {
            branch_history_bytes: minimum,
            ..StorageLimits::default()
        },
    )
    .unwrap();
    let exact = calibrated.usage_probe().unwrap().peak_live_history_bytes;
    drop(calibrated);
    let exact_profile = StorageLimits {
        live_history_bytes: exact,
        branch_history_bytes: minimum,
        ..StorageLimits::default()
    };
    model.restrict_limits_probe(exact_profile.clone()).unwrap();
    let (saved, trace) = io_observer::observe(|| model.save_active());
    saved.unwrap();
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation == "bundle_reconcile")
    );
    assert_eq!(model.core().export_history().unwrap(), candidate);
    let exact_open = LoadedBundle::read_with_limits_probe(
        &model_temp.path("b.kinewright"),
        exact_profile.clone(),
    )
    .unwrap();
    assert_eq!(
        exact_open.usage_probe().unwrap().peak_live_history_bytes,
        exact
    );
    drop(exact_open);
    let over_profile = StorageLimits {
        live_history_bytes: exact - 1,
        ..exact_profile
    };
    assert!(matches!(
        LoadedBundle::read_with_limits_probe(&model_temp.path("b.kinewright"), over_profile),
        Err(kinewright_project::bundle::BundleError::Capacity(
            "live history bytes"
        ))
    ));
    println!("C5 same-writer exact fresh-open peak={exact}");
    drop(model);
    timed("inventory-calibrated-open-and-exact-limit-reads");
    for (lane, limit) in [("over", irreducible - 1), ("existing", irreducible - 1)] {
        let profile = StorageLimits {
            live_history_bytes: limit,
            branch_history_bytes: minimum,
            ..StorageLimits::default()
        };
        let (temp, mut session, ids, left) = source(profile.clone());
        timed(&format!("{lane}-source"));
        large_edit(session.core(), label_bytes);
        let history = session.core().export_history().unwrap();
        let previous = std::fs::read(temp.path("b.kinewright")).unwrap();
        let (result, trace) = io_observer::observe(|| session.save_active());
        assert!(
            !trace
                .events
                .iter()
                .any(|e| e.operation == "bundle_reconcile")
        );
        if result.is_ok() {
            LoadedBundle::read_with_limits_probe(&temp.path("b.kinewright"), profile.clone())
                .expect("published candidate cannot exceed independent fresh-open capacity");
            panic!("one-byte-over candidate published");
        }
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "publication_reopen_fit"),
            "actual writer reservations masked the fresh-reader bound: {result:?}"
        );
        assert_eq!(std::fs::read(temp.path("b.kinewright")).unwrap(), previous);
        assert_eq!(session.core().export_history().unwrap(), history);
        let points = session.list_recovery_points().unwrap();
        let target = if lane == "existing" {
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(left.clone())
        } else {
            points[1].clone()
        };
        let loaded = LoadedBundle::read(&temp.path("b.kinewright")).unwrap();
        let (index_ref, branch, root) = match &target {
            kinewright_project::bundle_recovery::RecoverySelection::Checkpoint { root, branch } => {
                let raw: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(temp.path(&format!(
                        "b.kinewright-assets/studio/objects/{}",
                        root.sha256
                    )))
                    .unwrap(),
                )
                .unwrap();
                (
                    serde_json::from_value(raw["studio"]["branch_index_ref"].clone()).unwrap(),
                    branch.clone(),
                    Some(root.clone()),
                )
            }
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(branch) => (
                loaded.root().unwrap().branch_index_ref.clone(),
                branch.clone(),
                None,
            ),
        };
        let index = session.store().unwrap().read_record::<std::collections::BTreeMap<BranchId,kinewright_project::bundle_records::ObjectRef>>(&index_ref).unwrap();
        let root = root.unwrap_or_else(|| index[&branch].clone());
        let cp = session
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(&index[&branch])
            .unwrap();
        let target_oracle = (*session
            .store()
            .unwrap()
            .resolve_history(&cp.history_ref)
            .unwrap())
        .clone();
        drop(cp);
        drop(index);
        drop(loaded);
        session.prepare_recovery(target.clone()).unwrap();
        timed(&format!("{lane}-refusal-and-prepare"));
        let original_target_pin = session
            .store()
            .unwrap()
            .retention_probe()
            .unwrap()
            .1
            .get(&root.sha256)
            .copied()
            .unwrap();
        assert!(original_target_pin > 0);
        let result = session.select_local();
        assert!(
            matches!(result, ProjectSelectionResult::Refused(_)),
            "{result:?}"
        );
        if lane == "existing" {
            assert!(!session.retirement_candidates().unwrap().contains(&left));
            let before = session.root_sha256().unwrap();
            assert!(session.retire_working_branch(&left, &before, true).is_err());
            assert_eq!(session.root_sha256().unwrap(), before);
        }
        let mut activated = false;
        for id in ids {
            if !session.retirement_candidates().unwrap().contains(&id) {
                continue;
            }
            let disclosure = session.retirement_disclosure(&id).unwrap();
            assert!(disclosure.contains("Current-history residency relief"));
            let expected = session.root_sha256().unwrap();
            session.retire_working_branch(&id, &expected, true).unwrap();
            let result = session.select_local();
            if matches!(result, ProjectSelectionResult::Activated(_)) {
                activated = true;
                break;
            }
            assert!(
                matches!(result, ProjectSelectionResult::Refused(_)),
                "{result:?}"
            );
            assert!(session.store().unwrap().verify(&root).is_ok());
        }
        assert!(
            activated,
            "explicit unrelated branch retirement must make retry usable"
        );
        assert_eq!(session.core().export_history().unwrap(), target_oracle);
        assert!(
            !session.list_recovery_points().unwrap().contains(&target),
            "real retirement/Save rotation must remove P1 from inventory"
        );
        let expected = session.core().export_history().unwrap();
        timed(&format!("{lane}-retirement-retry"));
        drop(session);
        std::fs::remove_dir_all(temp.path("hidden-a")).unwrap();
        let loaded =
            LoadedBundle::read_with_limits_probe(&temp.path("b.kinewright"), profile).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), expected);
        timed(&format!("{lane}-cold-open"));
    }
}
