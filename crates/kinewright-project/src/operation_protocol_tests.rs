use super::*;
use crate::{
    bundle_records::BatchId,
    operation_history::{CreationOrigin, HistoryPool, PoolLimits, Role},
    receipts::IssuerId,
};
use kinewright_core::{Document, Operation, Track, TrackId, TrackKind};
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
fn context(index: u64) -> CommandContext {
    CommandContext {
        creation: CreationOrigin {
            branch: serde_json::from_str(&format!("\"{:032x}\"", 1)).unwrap(),
            batch: serde_json::from_str::<BatchId>(&format!("\"{index:032x}\"")).unwrap(),
        },
        inputs: vec![],
        approval: None,
        transition: None,
    }
}
fn state() -> StreamState {
    StreamState::installed(Seed {
        project: ProjectId::fresh().unwrap(),
        branch: BranchId::fresh().unwrap(),
        attachment_epoch: 1,
        installed: SnapshotIdentity {
            core: [1; 32],
            owner: [2; 32],
        },
    })
    .unwrap()
}
fn id(index: u64) -> CommandId {
    CommandId {
        issuer: IssuerId(format!("{:032x}", 1)),
        sequence: index,
    }
}
fn edit(locked: bool) -> Action {
    Action::Do {
        operations: vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }],
        gesture: None,
    }
}
#[test]
fn accepted_stream_keeps_a_complete_pending_window_through_codec_roundtrips() {
    let mut stream = state();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &context(1).creation,
        || Ok(document()),
    )
    .unwrap();
    let mut messages = VecDeque::new();
    for index in 1..=192 {
        let message = stream
            .message(
                id(index),
                owner.history().position(),
                edit(index % 2 == 0),
                context(index),
            )
            .unwrap();
        assert!(stream.lookup(&message).unwrap().is_none());
        let effects = message.action.apply(&message.context, &mut owner).unwrap();
        let (next, receipt) = stream.advanced(&message, effects.accepted).unwrap();
        assert_eq!(
            stream.sequence(),
            index - 1,
            "PREPARING_ROOT_DOES_NOT_ACKNOWLEDGE_OR_CHANGE_COMMITTED_STREAM"
        );
        stream = serde_json::from_slice(&serde_json::to_vec(&next).unwrap()).unwrap();
        stream.validate().unwrap();
        assert_eq!(
            stream.lookup(&message).unwrap(),
            Some(&receipt),
            "RECEIPTED_MESSAGE_IS_RECOGNISED_WITHOUT_REAPPLY"
        );
        messages.push_back(message);
        if messages.len() > PENDING_COMMANDS {
            messages.pop_front();
        }
        for pending in &messages {
            assert!(
                stream.lookup(pending).unwrap().is_some(),
                "ENTIRE_PENDING_WINDOW_SURVIVES_RESTART"
            );
        }
    }
    assert_eq!(
        stream.receipts.len(),
        PENDING_COMMANDS,
        "STREAM_RECEIPTS_REMAIN_BOUNDED"
    );
}
#[test]
fn sequence_chain_payload_and_accepted_key_conflicts_are_typed() {
    let stream = state();
    let original = stream
        .message(
            id(1),
            CommandKey {
                epoch: 0,
                position: 0,
            },
            edit(true),
            context(1),
        )
        .unwrap();
    let mut wrong = original.clone();
    wrong.sequence += 1;
    assert_eq!(
        stream.lookup(&wrong),
        Err(Conflict::Sequence),
        "STREAM_REJECTS_SEQUENCE_GAP"
    );
    let mut wrong = original.clone();
    wrong.predecessor[0] ^= 1;
    assert_eq!(
        stream.lookup(&wrong),
        Err(Conflict::Chain),
        "STREAM_REJECTS_WRONG_PREDECESSOR"
    );
    let mut wrong = original.clone();
    wrong.action = edit(false);
    assert_eq!(
        stream.lookup(&wrong),
        Err(Conflict::Chain),
        "STREAM_REJECTS_MUTATED_NORMALISED_COMMAND"
    );
    assert_eq!(
        stream.advanced(
            &original,
            CommandKey {
                epoch: 1,
                position: 0
            }
        ),
        Err(Conflict::AcceptedKey),
        "STREAM_REJECTS_DIFFERENT_EXECUTOR_KEY"
    );
    let (committed, _) = stream.advanced(&original, original.accepted).unwrap();
    assert_eq!(
        committed.lookup(&wrong),
        Err(Conflict::CommandIdentity),
        "RECEIPTED_ID_NEVER_NAMES_ANOTHER_PAYLOAD"
    );
}
#[test]
fn install_seeds_a_new_chain_from_the_combined_snapshot_identity() {
    let initial = state();
    let message = initial
        .message(
            id(1),
            CommandKey {
                epoch: 0,
                position: 0,
            },
            edit(true),
            context(1),
        )
        .unwrap();
    let (committed, _) = initial.advanced(&message, message.accepted).unwrap();
    let mut selected = committed.seed().clone();
    selected.attachment_epoch += 1;
    let installed = StreamState::installed(selected.clone()).unwrap();
    assert_eq!(installed.sequence(), 0, "INSTALL_RESETS_STREAM_SEQUENCE");
    assert_ne!(
        installed.tip(),
        committed.tip(),
        "INSTALL_RESETS_PREVIOUS_EPOCH_CHAIN"
    );
    selected.installed.owner[0] ^= 1;
    let owner_changed = StreamState::installed(selected.clone()).unwrap();
    assert_ne!(
        installed.tip(),
        owner_changed.tip(),
        "STREAM_INSTALL_SEED_BINDS_OWNER_DIGEST"
    );
    assert_eq!(
        installed.verify_installed_snapshot(selected.installed),
        Err(Conflict::Snapshot),
        "INSTALL_EQUALITY_USES_COMBINED_SNAPSHOT_IDENTITY"
    );
}
#[test]
fn close_is_an_explicit_accepted_transition_in_the_stream() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &context(1).creation,
        || Ok(document()),
    )
    .unwrap();
    let stream = state();
    let message = stream
        .message(id(1), owner.history().position(), Action::Close, context(1))
        .unwrap();
    let effects = message.action.apply(&message.context, &mut owner).unwrap();
    let (closed, _) = stream.advanced(&message, effects.accepted).unwrap();
    assert_eq!(
        owner.history().position().position,
        1,
        "STREAM_CLOSE_ADVANCES_EXECUTOR_POSITION"
    );
    assert_eq!(closed.sequence(), 1, "STREAM_CLOSE_ADVANCES_SEQUENCE");
    assert_ne!(closed.tip(), stream.tip(), "STREAM_CLOSE_ADVANCES_CHAIN");
}

#[test]
fn admitted_source_step_fits_the_manual_transport() {
    use kinewright_core::{Marker, MarkerId, TimeCode};
    let pool = HistoryPool::new(PoolLimits::default());
    let mut source = OwnerHistory::new(
        &pool,
        Role::Task,
        Role::Task.policy(),
        &context(1).creation,
        || Ok(document()),
    )
    .unwrap();
    let operation = Operation::AddMarker {
        marker: Marker {
            id: MarkerId(1),
            position: TimeCode(0),
            label: "x".repeat((1 << 20) - 8192),
            color_token: 0,
        },
    };
    let accepted = source
        .execute(Command::Do(operation), context(2).creation, vec![], None)
        .expect("SOURCE_ADMITS_MAXIMAL_TRANSPORT_WITNESS")
        .accepted;
    // This independent message owns the payload being forwarded by the source.
    let action = Action::Do {
        operations: source.history().applied_delta().cloned().collect(),
        gesture: None,
    };
    let message = state()
        .message(id(1), accepted, action, context(2))
        .unwrap();
    let bytes = serde_json::to_vec(&message).unwrap().len() as u64;
    assert!(bytes > 256 << 10, "TRANSPORT_WITNESS_EXCEEDS_OLD_WINDOW");
    assert!(
        bytes <= PENDING_BYTES,
        "ADMITTED_SOURCE_STEP_FITS_MANUAL_TRANSPORT bytes={bytes} cap={PENDING_BYTES}"
    );
}
