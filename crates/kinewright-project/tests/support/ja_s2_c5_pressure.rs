use super::Ja1EditingSession;
use kinewright_core::{Command, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::BranchId,
    bundle_switch::ProjectSelectionResult,
    io_observer,
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
                name: None,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::for_test(doc).unwrap();
    grow(&core, 8);
    let mut bundle = ProjectBundle::activate(
        &a,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let input = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &input).unwrap();
    let left = bundle.manual_branch_id().clone();
    let attachment = bundle.attach_core(&core).unwrap();
    let cut = attachment.freeze().unwrap();
    attachment.retire(&cut).unwrap();
    let mut ids = vec![];
    for n in 0..24 {
        let id = bundle
            .fork_branch(&left, kinewright_project::operation_history::Role::Ordinary)
            .unwrap();
        let checkpoint = bundle.checkpoint(&id).unwrap();
        let other = Core::open(
            checkpoint
                .open(bundle.store(), bundle.history_pool())
                .unwrap(),
            id.clone(),
            checkpoint.revision,
        )
        .unwrap();
        grow(&other, 24 + n);
        bundle.save_branch(&id, &other).unwrap();
        let attachment = bundle.attach_core(&other).unwrap();
        let cut = attachment.freeze().unwrap();
        attachment.retire(&cut).unwrap();
        ids.push(id);
    }
    let checkpoint = bundle.checkpoint(&left).unwrap();
    let core = Core::open(
        checkpoint
            .open(bundle.store(), bundle.history_pool())
            .unwrap(),
        left.clone(),
        checkpoint.revision,
    )
    .unwrap();
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
fn large_edit(core: &Core) {
    // Grow the immutable document, independently of journal representation.
    let ops = vec![Operation::AddMarker {
        marker: kinewright_core::Marker {
            id: kinewright_core::MarkerId(9000),
            position: kinewright_core::TimeCode(0),
            label: "m".repeat(1 << 19),
            color_token: 0,
        },
    }];
    assert!(matches!(
        core.request(Command::DoBatch(ops)).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[test]
pub(super) fn w13_definite_rescue_sparse_cache_growing_saves_and_same_pin_quota_retry() {
    use kinewright_project::operation_history::{HistoryPool, PoolLimits, Role};
    let (temp, mut session, ids, _) = source(StorageLimits::default());
    assert_eq!(ids.len(), 24);
    large_edit(session.core());
    let expected = session.core().snapshot_identity().unwrap();
    let root = std::fs::read(temp.path("b.kinewright")).unwrap();
    let pool = session.core().history_pool();
    let first = session.core().export_history().unwrap();
    let second = session.core().export_history().unwrap();
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap().0,
        3,
        "RESCUE_CAPACITY_FIXTURE_FILLS_THE_THREE_ORDINARY_HOLDERS"
    );
    assert!(
        session.save_active().is_err(),
        "FULL_HISTORY_POOL_REFUSES_SAVE_BEFORE_PUBLICATION"
    );
    assert_eq!(
        std::fs::read(temp.path("b.kinewright")).unwrap(),
        root,
        "POOL_REFUSAL_CANNOT_REPLACE_THE_ACKNOWLEDGED_ROOT"
    );
    assert_eq!(
        session.core().snapshot_identity().unwrap(),
        expected,
        "POOL_REFUSAL_KEEPS_THE_ACCEPTED_UNSAVED_OWNER"
    );
    drop(first);
    drop(second);
    session.save_active().unwrap();
    assert_eq!(
        session.core().snapshot_identity().unwrap(),
        expected,
        "POOL_ROOM_RELEASE_MAKES_THE_SAME_SAVE_RETRYABLE"
    );
    let retired = session
        .retirement_candidates()
        .unwrap()
        .into_iter()
        .find(|id| ids.contains(id))
        .expect("unrelated ordinary branches can be explicitly retired");
    let root = session.root_sha256().unwrap();
    session
        .retire_working_branch(&retired, &root, true)
        .unwrap();
    assert_eq!(
        session.core().snapshot_identity().unwrap(),
        expected,
        "RETIREMENT_NEVER_ALTERS_THE_ACTIVE_HISTORY"
    );
    drop(session);
    let ordinary = Role::Ordinary.envelope().retained;
    let mut limits = PoolLimits::default();
    limits.retained[0] = 2 * ordinary;
    limits.peak[0] = limits.retained[0] + limits.workspace;
    let pool = HistoryPool::new(limits);
    let mut loaded = LoadedBundle::read(&temp.path("b.kinewright"), &pool).unwrap();
    let reopened = loaded.open_manual().unwrap();
    assert_eq!(
        reopened.snapshot_identity().unwrap(),
        expected,
        "SPARSE_RESCUE_GRAPH_COLD_OPENS_THE_EXACT_RETRIED_OWNER"
    );
    assert!(
        pool.lease_counts(Role::Ordinary).unwrap().1 <= 2,
        "ARCHIVED_RESCUE_BRANCHES_USE_ONE_SHARED_REPLAY_WINDOW"
    );
}
