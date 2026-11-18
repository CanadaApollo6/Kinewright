use super::*;
use crate::{Track, TrackId, TrackKind};
fn document() -> Document {
    Document {
        tracks: vec![Track {
            name: None,
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

#[test]
fn encoded_stream_holder_uses_one_anchor_allowance_without_a_document_copy() {
    let mut history =
        History::new(document(), RetentionPolicy::Fold, RoleBounds::ORDINARY).unwrap();
    let identity = history.snapshot_identity().unwrap();
    let before = PhaseCounters::read();
    history
        .reserve_stream_buffer((4 << 20) + (128 << 10))
        .unwrap();
    assert_eq!(
        history.snapshot_identity().unwrap(),
        identity,
        "OUTBOX_RESERVATION_DOES_NOT_CHANGE_HISTORY_IDENTITY"
    );
    assert_eq!(
        PhaseCounters::read().since(before).document_copies,
        0,
        "OUTBOX_RESERVATION_COPIES_NO_DOCUMENT"
    );
    for index in 0..128 {
        history.execute(lock(index % 2 == 0)).unwrap();
        assert!(
            history.cache_depths().len() < RoleBounds::ORDINARY.checkpoint_documents,
            "ENCODED_OUTBOX_REPLACES_ONE_ANCHOR_ALLOWANCE"
        );
    }
    history.release_stream_buffer().unwrap();
    assert_eq!(
        history.bounds().checkpoint_documents,
        4,
        "OUTBOX_RELEASE_PRESERVES_FIXED_CREATION_PROFILE"
    );
}

#[test]
fn installing_a_snapshot_preserves_the_live_stream_buffer_allowance() {
    let mut history = History::new(
        Document::default(),
        RetentionPolicy::Fold,
        RoleBounds::ORDINARY,
    )
    .unwrap();
    let selected = history.record().validate(RoleBounds::ORDINARY).unwrap();
    history
        .reserve_stream_buffer(RoleBounds::ORDINARY.document_live_bytes)
        .unwrap();
    assert_eq!(history.cache.slots(), 3);
    history.install_snapshot(selected).unwrap();
    assert_eq!(
        history.cache.slots(),
        3,
        "INSTALL_NEVER_SPENDS_THE_OUTBOX_ANCHOR_ALLOWANCE_TWICE"
    );
    history.release_stream_buffer().unwrap();
    assert_eq!(history.cache.slots(), 4);
}

#[test]
fn a_forwarded_install_adopts_the_selected_epoch_without_rekeying_commands() {
    let mut selected = History::new(
        Document::default(),
        RetentionPolicy::Fold,
        RoleBounds::ORDINARY,
    )
    .unwrap();
    let mut peer = History::new(
        Document::default(),
        RetentionPolicy::Fold,
        RoleBounds::ORDINARY,
    )
    .unwrap();
    let original = selected
        .execute(crate::Command::Do(crate::Operation::SetPanLaw {
            law: crate::PanLaw::default(),
        }))
        .unwrap()
        .accepted;
    let captured = selected.record().validate(RoleBounds::ORDINARY).unwrap();
    selected.install_snapshot(captured).unwrap();
    let forwarded = selected.record().validate(RoleBounds::ORDINARY).unwrap();
    peer.install_snapshot_at_epoch(forwarded, selected.position().epoch)
        .unwrap();
    assert_eq!(
        peer.snapshot_identity().unwrap(),
        selected.snapshot_identity().unwrap(),
        "FORWARDED_INSTALL_USES_THE_SELECTED_EPOCH"
    );
    assert_eq!(
        peer.undo().back().unwrap().commands()[0].id,
        original,
        "FORWARDED_INSTALL_PRESERVES_ORIGINAL_COMMAND_KEYS"
    );
    let before = peer.snapshot_identity().unwrap();
    let invalid = peer.record().validate(RoleBounds::ORDINARY).unwrap();
    assert_eq!(
        peer.install_snapshot_at_epoch(invalid, peer.position().epoch),
        Err(Error::InvalidMetadata),
        "INSTALL_EPOCH_MUST_ADVANCE_THE_DESTINATION"
    );
    assert_eq!(
        peer.snapshot_identity().unwrap(),
        before,
        "REFUSED_INSTALL_EPOCH_PRESERVES_THE_DESTINATION"
    );
}
