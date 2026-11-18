use super::tests::{document, origin};
use super::*;
use kinewright_core::{
    Command, Operation, TrackId,
    undo::{Charge, RetentionPolicy, RoleBounds},
};

fn make(pool: &HistoryPool, role: Role) -> OwnerHistory {
    OwnerHistory::new(pool, role, role.policy(), &origin(1), || Ok(document())).unwrap()
}
#[test]
fn creation_sizing_is_fixed_and_ordinary_captures_hold_two_leases() {
    let mut limits = PoolLimits::default();
    limits.retained[0] = 208 << 20;
    limits.peak[0] = limits.retained[0] + limits.workspace;
    let pool = HistoryPool::new(limits);
    let mut owner = make(&pool, Role::Ordinary);
    let capture = owner.capture().unwrap();
    assert_eq!(
        owner.reservation.limits().envelope.retained,
        104 << 20,
        "ORDINARY_FIXED_104_MIB"
    );
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap(),
        (2, 2),
        "ORDINARY_CAPTURE_SECOND_FULL_LEASE"
    );
    let before = capture.snapshot_identity().unwrap();
    assert!(
        matches!(owner.capture(), Err(Error::PoolFull(Role::Ordinary))),
        "THIRD_ORDINARY_LEASE_REFUSED"
    );
    drop(capture);
    assert_eq!(
        owner.capture().unwrap().snapshot_identity().unwrap(),
        before,
        "CAPTURE_REFUSAL_PRESERVES_UNSAVED_STATE"
    );
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let profile = owner.reservation.limits();
    let snapshot = owner.capture().unwrap();
    let sizing = snapshot.data.sizing;
    drop(owner);
    let opened = snapshot.validate().unwrap().open().unwrap();
    assert_eq!(
        opened.reservation.limits(),
        profile,
        "OPEN_NEEDS_EXACTLY_CAPTURE_LEASE"
    );
    assert_eq!(
        opened.reservation.sizing(),
        sizing,
        "CREATION_DESCRIPTOR_SURVIVES_OPEN"
    );
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap().1,
        2,
        "ORDINARY_PEAK_TWO"
    );
}
#[test]
fn sized_tasks_refuse_before_start_and_release_when_settled() {
    let descriptor = CreationSizing {
        kind: CreationKind::Task,
        baseline_live: 500,
        baseline_owners: 4096,
        initial_batch: Charge::default(),
    };
    let bytes = descriptor.limits().unwrap().envelope.retained;
    let pool = HistoryPool::new(PoolLimits {
        retained: [0, 3 * bytes, 0],
        peak: [0, 3 * bytes + (168 << 20), 0],
        workspace: 168 << 20,
    });
    let first = pool.admit_sized(descriptor).unwrap().into_core().unwrap();
    let second = pool.admit_sized(descriptor).unwrap().into_core().unwrap();
    let before = pool.usage(Role::Task).unwrap();
    assert!(
        matches!(
            pool.admit_sized(descriptor)
                .and_then(HistoryReservation::into_core),
            Err(Error::PoolFull(Role::Task))
        ),
        "TASK_POOL_FULL_IS_RETRYABLE_BEFORE_START"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        before,
        "TASK_ADMISSION_REFUSAL_ATOMIC"
    );
    drop(first);
    let next = pool
        .admit_sized(descriptor)
        .and_then(HistoryReservation::into_core)
        .expect("SETTLED_TASK_DROP_FREES_NEXT_ADMISSION");
    drop((second, next));
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (0, 0),
        "ALL_SIZED_LEASES_RELEASED"
    );
}
#[test]
fn creation_descriptor_is_required_bounded_and_part_of_identity() {
    let pool = HistoryPool::new(PoolLimits::default());
    let owner = make(&pool, Role::Task);
    let mut snapshot = owner.capture().unwrap();
    let original = snapshot.snapshot_identity().unwrap();
    snapshot.data.sizing.baseline_live += 1;
    assert_ne!(
        snapshot.snapshot_identity().unwrap(),
        original,
        "CREATION_SIZING_IS_SNAPSHOT_IDENTITY"
    );
    assert!(
        matches!(snapshot.validate(), Err(Error::InvalidRecord)),
        "CAPTURE_DESCRIPTOR_MUST_MATCH_ITS_LEASE"
    );
    let snapshot = owner.capture().unwrap();
    let encoded = super::stored::codec_tests::encoded(&snapshot);
    let mut wire: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    wire.as_object_mut().unwrap().remove("sizing");
    assert!(
        serde_json::from_value::<StoredOwnerRecord>(wire).is_err(),
        "NO_READER_WITHOUT_CREATION_DESCRIPTOR"
    );
    let bad = CreationSizing {
        kind: CreationKind::Task,
        baseline_live: (8 << 20) + 1,
        baseline_owners: 0,
        initial_batch: Charge::default(),
    };
    assert!(
        matches!(
            pool.admit_sized(bad),
            Err(Error::Capacity("creation sizing"))
        ),
        "CREATION_DESCRIPTOR_CANNOT_RAISE_PRODUCT_MAXIMUM"
    );
}
#[test]
fn admitted_task_streams_merge_as_one_batch_into_both_destinations() {
    // First case is the original 95,000-byte diagnostic. The other seeds vary
    // command and batch boundaries while forwarding exactly the applied delta.
    for seed in 0..9_u64 {
        let pool = HistoryPool::new(PoolLimits::default());
        let mut task = make(&pool, Role::Task);
        let count = if seed == 0 {
            2000
        } else {
            32 + usize::try_from(seed).unwrap() * 41
        };
        let mut random = seed + 1;
        for i in 0..count {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let operations: Vec<_> = (0..if seed == 0 {
                1
            } else {
                1 + (random >> 60) as usize
            })
                .map(|j| Operation::SetTrackSyncLock {
                    track: TrackId(81),
                    locked: (i + j) % 2 == 0,
                })
                .collect();
            task.execute(
                Command::DoBatch(operations),
                origin(i as u64 + 2),
                vec![],
                None,
            )
            .unwrap();
        }
        if seed == 0 {
            assert_eq!(
                task.history().charge().encoded,
                95_000,
                "ORIGINAL_MERGE_DIAGNOSTIC_LITERAL_BYTES"
            );
        }
        // Each receiving actor owns its batch; these are independent command payloads.
        let delta: Vec<_> = task.history().applied_delta().cloned().collect();
        let need = kinewright_core::undo::batch_charge(&delta).unwrap();
        assert!(
            need.encoded <= RoleBounds::TASK.operation_bytes
                && need.decoded <= RoleBounds::TASK.decoded_history_bytes,
            "PLAIN_BATCH_NEVER_COSTS_MORE_THAN_SOURCE_COMMANDS"
        );
        let mut candidate = make(&pool, Role::Candidate);
        candidate
            .execute(Command::DoBatch(delta), origin(3000), vec![], None)
            .expect("ADMITTED_TASK_DELTA_MUST_FIT_LIVE_CANDIDATE");
        let forwarded: Vec<_> = candidate.history().applied_delta().cloned().collect();
        let mut ordinary = make(&pool, Role::Ordinary);
        ordinary
            .execute(Command::DoBatch(forwarded), origin(4000), vec![], None)
            .expect("ADMITTED_CANDIDATE_BATCH_MUST_FIT_ORDINARY");
        assert_eq!(
            task.history().document(),
            ordinary.history().document(),
            "MERGE_FIT_PRESERVES_RESULT"
        );
        assert_eq!(candidate.history().policy(), RetentionPolicy::NeverFold);
        drop(candidate);
        let delta: Vec<_> = task.history().applied_delta().cloned().collect();
        let mut staging =
            OwnerHistory::staging(&pool, &origin(1), task.reservation.sizing(), &delta, || {
                Ok(document())
            })
            .unwrap();
        staging
            .execute(Command::DoBatch(delta), origin(5000), vec![], None)
            .expect("MEASURED_FROZEN_DELTA_FITS_STAGING_CANDIDATE");
    }
}

#[path = "../../../kinewright-core/tests/support/merge_fixture.rs"]
pub(super) mod fixture;
#[test]
fn twenty_thousand_clip_cut_forks_merges_and_reopens() {
    let mut baseline = fixture::document(20_000, 8, 90);
    for (i, track) in baseline.tracks.iter_mut().enumerate() {
        track.sync_lock = i < 4;
        if track.sync_lock {
            track.clips[0].source_range.end = kinewright_core::TimeCode(120_120);
            for clip in track.clips.iter_mut().skip(1) {
                clip.timeline_start.0 += 120_030;
            }
        }
    }
    baseline.duration.0 += 120_030;
    let make_baseline = || {
        // Each actor below owns an independent initial value in the fixture.
        Ok(baseline.clone())
    };
    let pool = HistoryPool::new(PoolLimits::default());
    let mut task = OwnerHistory::new(
        &pool,
        Role::Task,
        Role::Task.policy(),
        &origin(1),
        make_baseline,
    )
    .unwrap();
    let mut operations = vec![];
    for track in 0..4 {
        operations.push(Operation::SplitClip {
            clip: kinewright_core::ClipId(track * 2500 + 1),
            at: kinewright_core::TimeCode(60),
        });
        let middle = kinewright_core::ClipId(20_001 + track * 2);
        operations.push(Operation::SplitClip {
            clip: middle,
            at: kinewright_core::TimeCode(66),
        });
        if track != 0 {
            operations.push(Operation::DeleteClip { clip: middle });
        }
    }
    operations.push(Operation::RippleDeleteClip {
        clip: kinewright_core::ClipId(20_001),
    });
    task.execute(Command::DoBatch(operations), origin(2), vec![], None)
        .expect("PRODUCT_20000_CLIP_TASK_CUT_ACCEPTED");
    let mut candidate = OwnerHistory::new(
        &pool,
        Role::Candidate,
        Role::Candidate.policy(),
        &origin(1),
        make_baseline,
    )
    .unwrap();
    candidate
        .execute(
            Command::DoBatch(task.history().applied_delta().cloned().collect()),
            origin(3),
            vec![],
            None,
        )
        .expect("PRODUCT_20000_CLIP_TASK_MERGES");
    let mut ordinary = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin(1),
        make_baseline,
    )
    .unwrap();
    ordinary
        .execute(
            Command::DoBatch(candidate.history().applied_delta().cloned().collect()),
            origin(4),
            vec![],
            None,
        )
        .expect("PRODUCT_20000_CLIP_CANDIDATE_PUBLISHES");
    super::roundtrip_tests::saved_open(&ordinary);
}

#[test]
#[ignore = "on-demand: release qualification of the fixed 2000-removal task and its complete merge"]
fn two_thousand_four_track_removals_fit_one_task_and_its_merges() {
    let pool = HistoryPool::new(PoolLimits::default());
    let baseline = || Ok(fixture::document(4, 4, 120_120));
    let mut task =
        OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), baseline).unwrap();
    let mut operations = fixture::removals(4, 2000).into_iter();
    for cut in 0..2000 {
        task.execute(
            Command::DoBatch(operations.by_ref().take(12).collect()),
            origin(cut + 2),
            vec![],
            None,
        )
        .unwrap_or_else(|error| panic!("TASK_HOLDS_2000_FOUR_TRACK_REMOVALS cut={cut}: {error}"));
    }
    assert_eq!(
        task.history().charge().encoded,
        850_911,
        "DIALOGUE_2000_LITERAL_ENCODED_CHARGE"
    );
    assert_eq!(
        task.history().charge().decoded,
        20_465_822,
        "DIALOGUE_2000_LITERAL_DECODED_CHARGE"
    );
    let boundary = task
        .boundary(task.history().undo().len())
        .expect("DIALOGUE_2000_CURRENT_LINEAGE_QUERY_FITS_ITS_LEASE");
    assert!(
        boundary.lineage().capacity_bytes().unwrap()
            + kinewright_core::undo::document_capacity_bytes(boundary.document()).unwrap()
            <= task.reservation.limits().envelope.retained,
        "DIALOGUE_2000_BOUNDARY_FITS_ITS_FULL_LEASE"
    );
    drop(boundary);
    let delta: Vec<_> = task.history().applied_delta().cloned().collect();
    let mut staging = OwnerHistory::staging(
        &pool,
        &origin(1),
        task.reservation.sizing(),
        &delta,
        baseline,
    )
    .unwrap();
    staging
        .execute(Command::DoBatch(delta), origin(3000), vec![], None)
        .expect("DIALOGUE_2000_STAGING_ACCEPTS_FULL_TASK");
    drop(staging);
    let mut live = OwnerHistory::new(
        &pool,
        Role::Candidate,
        Role::Candidate.policy(),
        &origin(1),
        baseline,
    )
    .unwrap();
    live.execute(
        Command::DoBatch(task.history().applied_delta().cloned().collect()),
        origin(3001),
        vec![],
        None,
    )
    .expect("DIALOGUE_2000_LIVE_CANDIDATE_ACCEPTS_FULL_TASK");
    let mut ordinary = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin(1),
        baseline,
    )
    .unwrap();
    ordinary
        .execute(
            Command::DoBatch(live.history().applied_delta().cloned().collect()),
            origin(4000),
            vec![],
            None,
        )
        .expect("DIALOGUE_2000_ORDINARY_ACCEPTS_FULL_CANDIDATE");
    assert_eq!(
        task.history().document(),
        ordinary.history().document(),
        "DIALOGUE_2000_MERGED_DOCUMENT"
    );
}

#[test]
fn two_project_tabs_can_capture_without_blocking_either_live_core() {
    let pool = HistoryPool::new(PoolLimits::default());
    // App::new_project retains the first ProjectSession's Core while pushing
    // the second; saving either tab then owns one independent capture.
    let first = make(&pool, Role::Ordinary);
    let second = make(&pool, Role::Ordinary);
    let captured = first.capture().unwrap();
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap(),
        (3, 3),
        "TWO_TABS_AND_SAVE_NEED_THREE_ORDINARY_LEASES"
    );
    drop(captured);
    assert!(
        OwnerHistory::new(
            &pool,
            Role::Ordinary,
            Role::Ordinary.policy(),
            &origin(1),
            || Ok(document())
        )
        .is_err(),
        "ORDINARY_ADMISSION_LEAVES_ONE_CAPTURE_FREE"
    );
    assert!(second.capture().is_ok(), "EITHER_OPEN_TAB_CAN_STILL_SAVE");
    let limits = PoolLimits::default();
    assert_eq!(
        limits.retained.iter().sum::<u64>() + limits.workspace,
        1024 << 20,
        "PRODUCT_POOL_TOTAL_1024_MIB"
    );
}

#[test]
fn cold_source_original_document_room_reaches_both_fresh_merge_kinds() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut baseline = document();
    let mut label = String::with_capacity(2 << 20);
    label.push('x');
    baseline.markers.push(kinewright_core::Marker {
        id: kinewright_core::MarkerId(1),
        position: kinewright_core::TimeCode(0),
        label,
        color_token: 0,
    });
    let owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(baseline)
    })
    .unwrap();
    let expected = owner.history().bounds().document_live_bytes;
    let snapshot = owner.capture().unwrap();
    let bytes = serde_json::to_vec(&snapshot.data).unwrap();
    drop((snapshot, owner));
    let data: super::snapshot::SnapshotData = serde_json::from_slice(&bytes).unwrap();
    let reservation = pool.admit_sized(data.sizing).unwrap();
    let opened = OwnerSnapshot { data, reservation }
        .validate()
        .unwrap()
        .open()
        .unwrap();
    assert!(
        kinewright_core::undo::document_capacity_bytes(opened.history().baseline()).unwrap()
            < 1 << 20,
        "COLD_SOURCE_FIXTURE_COMPACTS_ALLOCATIONS"
    );
    assert_eq!(
        opened.history().bounds().document_live_bytes,
        expected,
        "COLD_SOURCE_RETAINS_ORIGINAL_DOCUMENT_ROOM"
    );
    let sizing = opened.reservation.sizing();
    let live = OwnerHistory::live_candidate(&pool, &origin(1), sizing, || {
        Ok(std::sync::Arc::clone(opened.history().baseline()))
    })
    .unwrap();
    assert_eq!(
        live.history().bounds().document_live_bytes,
        expected,
        "FRESH_LIVE_MERGE_DOCUMENT_ROOM_COVERS_SOURCE"
    );
    drop(live);
    let delta = vec![Operation::SetTrackSyncLock {
        track: TrackId(81),
        locked: true,
    }];
    let staging = OwnerHistory::staging(&pool, &origin(1), sizing, &delta, || {
        Ok(std::sync::Arc::clone(opened.history().baseline()))
    })
    .unwrap();
    assert_eq!(
        staging.history().bounds().document_live_bytes,
        expected,
        "FRESH_STAGING_DOCUMENT_ROOM_COVERS_SOURCE"
    );
}

#[test]
fn product_baseline_forks_keep_the_required_task_slots_and_candidate_room() {
    use std::sync::Arc;
    for (clips, minimum_tasks) in [(1000, 10), (5000, 8), (20_000, 4)] {
        let pool = HistoryPool::new(PoolLimits::default());
        let gui = OwnerHistory::new(
            &pool,
            Role::Ordinary,
            Role::Ordinary.policy(),
            &origin(1),
            || Ok(fixture::document(clips, 8, 90)),
        )
        .unwrap();
        let baseline = Arc::clone(gui.history().document());
        let task = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(2), || {
            Ok(Arc::clone(&baseline))
        })
        .unwrap();
        let candidate = OwnerHistory::new(
            &pool,
            Role::Candidate,
            Role::Candidate.policy(),
            &origin(3),
            || Ok(baseline),
        )
        .unwrap();
        let limits = task.reservation.limits();
        let candidate_bytes = candidate.reservation.limits().envelope.retained;
        let tasks = (400 << 20) / limits.envelope.retained - 1;
        let runs = (144 << 20) / candidate_bytes - 1;
        eprintln!(
            "PRODUCT_CREATION clips={clips} encoded={} live={} owners={} document_room={} owner_room={} task={} candidate={} tasks_with_capture={tasks} runs_with_staging={runs}",
            serde_json::to_vec(task.history().baseline()).unwrap().len(),
            task.reservation.sizing().baseline_live,
            task.reservation.sizing().baseline_owners,
            limits.bounds.document_live_bytes,
            limits.envelope.owners,
            limits.envelope.retained,
            candidate_bytes
        );
        assert!(
            tasks >= minimum_tasks,
            "PRODUCT_FORK_TASK_SLOTS_INCLUDE_CAPTURE_HEADROOM clips={clips}: {tasks} < {minimum_tasks}"
        );
        assert!(
            runs >= 1,
            "PRODUCT_LIVE_CANDIDATE_HAS_ONE_STAGING_LEASE clips={clips}"
        );
    }
}
