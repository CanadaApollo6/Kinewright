use super::*;
use crate::{Track, TrackId, TrackKind};
fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn lock(locked: bool) -> Command {
    Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    })
}
#[test]
fn trusted_preview_shares_documents_and_preserves_the_accepted_prefix() {
    let mut source =
        History::new(document(), RetentionPolicy::NeverFold, RoleBounds::TASK).unwrap();
    source.execute(lock(true)).unwrap();
    source.execute(lock(false)).unwrap();
    source.execute(Command::Undo).unwrap();
    let before = source.snapshot_identity().unwrap();
    let counters = PhaseCounters::read();
    let mut preview = source.fork_with_bounds(RoleBounds::TASK).unwrap();
    let phases = PhaseCounters::read().since(counters);
    assert_eq!(
        phases.document_copies, 0,
        "TRUSTED_PREVIEW_COPIES_NO_DOCUMENT"
    );
    assert_eq!(
        phases.exact_measurements, 0,
        "TRUSTED_PREVIEW_REUSES_RUNNING_BOUNDS"
    );
    assert_eq!(
        preview.snapshot_identity().unwrap(),
        before,
        "TRUSTED_PREVIEW_PRESERVES_KEYS_AND_BOTH_STACKS"
    );
    assert!(
        Arc::ptr_eq(source.document(), preview.document()),
        "TRUSTED_PREVIEW_SHARES_IMMUTABLE_CURRENT_DOCUMENT"
    );
    assert!(
        Arc::ptr_eq(source.baseline(), preview.baseline()),
        "TRUSTED_PREVIEW_SHARES_IMMUTABLE_BASELINE"
    );
    preview.execute(Command::Redo).unwrap();
    assert!(
        !preview.document().tracks[0].sync_lock,
        "PREVIEW_REDO_USES_VALIDATED_STEP"
    );
    assert_eq!(
        source.snapshot_identity().unwrap(),
        before,
        "PREVIEW_NEVER_MUTATES_SOURCE"
    );
}
#[test]
fn prepared_destination_profile_keeps_the_same_epoch_and_commands() {
    let mut history = History::new(
        document(),
        RetentionPolicy::NeverFold,
        RoleBounds::CANDIDATE,
    )
    .unwrap();
    history.execute(lock(true)).unwrap();
    let before = history.snapshot_identity().unwrap();
    let key = history.position();
    let history = history.into_bounds(RoleBounds::TASK).unwrap();
    assert_eq!(
        history.snapshot_identity().unwrap(),
        before,
        "PREPARED_PROFILE_IS_NOT_A_SNAPSHOT_INSTALL"
    );
    assert_eq!(
        history.position(),
        key,
        "PREPARED_PROFILE_DOES_NOT_MINT_AN_EPOCH"
    );
    let too_small = RoleBounds {
        decoded_history_bytes: 1,
        ..RoleBounds::TASK
    };
    assert_eq!(
        history.fork_with_bounds(too_small),
        Err(Error::HistoryFull),
        "PREVIEW_ADMISSION_PRECEDES_COMMAND_PAYLOAD_COPY"
    );
}
