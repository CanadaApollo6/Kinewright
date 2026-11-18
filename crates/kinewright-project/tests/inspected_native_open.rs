//! A cold native owner replaces an inspection, never accepted work.
#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::{
    AttachmentState, Command, Document, Event, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::{BatchId, BranchId},
    editing_session::Ja1EditingSession,
    operation_actor::Core,
    operation_history::{CreationOrigin, HistoryPool, PoolLimits, Role},
    storage_budget::StorageLimits,
};

struct TestCleanup;

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one validation, accepted-edit refusal and two-tab replacement witness"
)]
fn inspected_native_open_validates_before_retirement_and_preserves_every_accepted_edit() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "inspected-native-open");
    let path = temp.path("project.kinewright");
    let source = Core::for_test(Document::default()).unwrap();
    let mut source_owner =
        Ja1EditingSession::new(temp.root(), &source, StorageLimits::default()).unwrap();
    source_owner
        .first_save(&path, StorageLimits::default())
        .unwrap();
    let expected = source.snapshot_identity().unwrap();
    drop(source_owner);
    let root = std::fs::read(&path).unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let make_view = || {
        Core::spawn(
            &pool,
            Role::Ordinary,
            CreationOrigin {
                branch: BranchId::fresh().unwrap(),
                batch: BatchId::fresh().unwrap(),
            },
            Document::default(),
        )
        .unwrap()
    };
    let _other_tab = make_view();
    let view = make_view();
    let before = view.change_id().unwrap();
    std::fs::write(&path, b"invalid project").unwrap();
    assert!(Ja1EditingSession::open_document_view(&path, &view, &Document::default()).is_err());
    assert_eq!(
        view.change_id().unwrap(),
        before,
        "INVALID_OWNER_DOES_NOT_RETIRE_INSPECTION"
    );
    assert_eq!(view.attachment_state().unwrap(), AttachmentState::Live);
    std::fs::write(&path, root).unwrap();

    assert!(matches!(
        view.request(Command::Do(Operation::AddTrack {
            track: Track {
                name: None,
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let accepted = view.snapshot_identity().unwrap();
    assert!(
        matches!(
            Ja1EditingSession::open_document_view(&path, &view, &Document::default()),
            Err(BundleError::HistoryChanged)
        ),
        "INSPECTION_WITH_ACCEPTED_WORK_IS_NEVER_RETIRED"
    );
    assert_eq!(
        view.snapshot_identity().unwrap(),
        accepted,
        "INSPECTION_REFUSAL_PRESERVES_ACCEPTED_WORK"
    );
    assert_eq!(view.attachment_state().unwrap(), AttachmentState::Live);
    let owner = view.attach(&TestCleanup).unwrap();
    let cut = owner.freeze(&TestCleanup).unwrap();
    owner.retire(&TestCleanup, cut).unwrap();

    let fresh = make_view();
    let opened =
        Ja1EditingSession::open_document_view(&path, &fresh, &Document::default()).unwrap();
    assert_eq!(
        opened.core().snapshot_identity().unwrap(),
        expected,
        "INSPECTION_OPEN_PRESERVES_STORED_COMBINED_IDENTITY"
    );
    assert_eq!(fresh.attachment_state().unwrap(), AttachmentState::Sealed);
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap(),
        (2, 3),
        "INSPECTION_REPLACEMENT_PEAK_IS_THREE"
    );
}
