use super::*;
use crate::{
    bundle_records::{BatchId, BranchId, ProjectId, RecordKind},
    operation_history::{CreationOrigin, HistoryPool, PoolLimits, Role},
    operation_protocol::Seed,
};
use kinewright_core::{
    Document, Marker, MarkerId, TimeCode, Track, TrackId, TrackKind,
    undo::{Error as UndoError, PhaseCounters},
};
fn context(index: u64) -> CommandContext {
    CommandContext {
        creation: CreationOrigin {
            branch: serde_json::from_str::<BranchId>(&format!("\"{:032x}\"", 1)).unwrap(),
            batch: serde_json::from_str::<BatchId>(&format!("\"{index:032x}\"")).unwrap(),
        },
        inputs: vec![],
        approval: None,
        transition: None,
    }
}
fn bound() -> (OwnerHistory, Producer) {
    let pool = HistoryPool::new(PoolLimits::default());
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    };
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &context(1).creation,
        || Ok(document),
    )
    .unwrap();
    let identity = owner.identity().unwrap();
    let state = StreamState::installed(Seed {
        project: ProjectId::fresh().unwrap(),
        branch: context(1).creation.branch,
        attachment_epoch: 1,
        installed: identity,
    })
    .unwrap();
    let lease = Lease {
        state,
        snapshot: identity,
        next_key: owner.history().position(),
        issuer: IssuerId(format!("{:032x}", 1)),
        next_issuer_sequence: 1,
        generation: 1,
        client: "gui".into(),
        token: "test-only-permit".into(),
        switch_generation: 0,
        owner_epoch: 1,
        anchor_head: HeadId::fresh().unwrap(),
        inputs: ObjectRef {
            sha256: "0".repeat(64),
            byte_length: 1,
            record_kind: RecordKind::InputManifest,
        },
        parent_identity: None,
        namespace_name: None,
    };
    let producer = Producer::bind(lease, &mut owner).unwrap();
    (owner, producer)
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
fn borrowed_encoding_matches_owned_protocol_and_retains_no_typed_payload_copy() {
    let (mut owner, mut producer) = bound();
    let action = edit(true);
    let context = context(2);
    let expected = producer
        .state()
        .message(
            CommandId {
                issuer: producer.lease().issuer.clone(),
                sequence: 1,
            },
            owner.history().position(),
            action.clone(),
            context.clone(),
        )
        .unwrap();
    let before = PhaseCounters::read();
    producer.execute(&mut owner, action, context).unwrap();
    let encoded = producer.pending().remove(0);
    assert_eq!(
        encoded.decode().unwrap(),
        expected,
        "BORROWED_OUTBOX_MATCHES_OWNED_NORMALISED_MESSAGE"
    );
    assert_eq!(
        serde_json::from_str::<String>(&serde_json::to_string(&encoded).unwrap()).unwrap(),
        serde_json::to_string(&expected).unwrap(),
        "RAW_OUTBOX_BYTES_ARE_CANONICAL_MESSAGE_BYTES"
    );
    assert_eq!(
        producer.state().tip(),
        expected.tip,
        "BORROWED_OUTBOX_MATCHES_PROTOCOL_CHAIN"
    );
    let phases = PhaseCounters::read().since(before);
    assert_eq!(
        phases.document_copies, 1,
        "PRODUCER_WARM_EDIT_ONE_DOCUMENT_COPY"
    );
    assert_eq!(
        phases.exact_measurements, 0,
        "PRODUCER_WARM_EDIT_ZERO_EXACT_MEASUREMENTS"
    );
}
#[test]
fn full_outbox_refuses_before_mutation_and_empty_batch_still_answers_empty() {
    let (mut owner, mut producer) = bound();
    for index in 0..PENDING_COMMANDS {
        producer
            .execute(&mut owner, edit(index % 2 == 0), context(index as u64 + 2))
            .unwrap();
    }
    let before = owner.identity().unwrap();
    let stream = producer.state().clone();
    let bytes = producer.pending_bytes();
    assert_eq!(
        producer.execute(&mut owner, edit(false), context(100)),
        Err(ProducerError::Protocol(Conflict::Backpressure)),
        "FULL_OUTBOX_REFUSES_BEFORE_EDIT"
    );
    assert_eq!(
        producer.execute(
            &mut owner,
            Action::Do {
                operations: vec![],
                gesture: None
            },
            context(101)
        ),
        Err(ProducerError::History(HistoryError::History(
            UndoError::Batch(kinewright_core::BatchError::Empty)
        ))),
        "EMPTY_BATCH_PRECEDES_OUTBOX_CAPACITY"
    );
    assert_eq!(
        owner.identity().unwrap(),
        before,
        "OUTBOX_REFUSAL_PRESERVES_COMPLETE_OWNER_STATE"
    );
    assert_eq!(
        producer.state(),
        &stream,
        "OUTBOX_REFUSAL_PRESERVES_SEQUENCE_AND_CHAIN"
    );
    assert_eq!(
        producer.pending_bytes(),
        bytes,
        "OUTBOX_REFUSAL_PRESERVES_PENDING_BYTES"
    );
}
#[test]
fn acknowledgement_is_atomic_idempotent_and_fenced_after_detach() {
    let (mut owner, mut producer) = bound();
    for index in 0..4 {
        producer
            .execute(&mut owner, edit(index % 2 == 0), context(index + 2))
            .unwrap();
    }
    let receipts: Vec<_> = producer.state().receipts().cloned().collect();
    let bytes = producer.pending_bytes();
    let mut wrong = receipts[1].clone();
    wrong.command[0] ^= 1;
    assert_eq!(
        producer.acknowledge(&[receipts[0].clone(), wrong]),
        Err(Conflict::CommandIdentity),
        "BAD_ACK_REFUSES_THE_WHOLE_PREFIX"
    );
    assert_eq!(
        producer.pending_count(),
        4,
        "BAD_ACK_KEEPS_EVERY_PENDING_MESSAGE"
    );
    assert_eq!(
        producer.pending_bytes(),
        bytes,
        "BAD_ACK_KEEPS_PENDING_BYTE_CHARGE"
    );
    assert_eq!(
        producer.detach(&mut owner),
        Err(ProducerError::Protocol(Conflict::Backpressure)),
        "DETACH_CANNOT_DROP_UNACKNOWLEDGED_WORK"
    );
    producer.acknowledge(&receipts).unwrap();
    producer
        .acknowledge(&receipts)
        .expect("REPEATED_DURABLE_ACK_IS_IDEMPOTENT");
    assert_eq!(
        producer.pending_bytes(),
        0,
        "ACK_RELEASES_ENCODED_QUEUE_BYTES"
    );
    producer.detach(&mut owner).unwrap();
    assert_eq!(
        producer.execute(&mut owner, edit(true), context(9)),
        Err(ProducerError::Protocol(Conflict::Detached)),
        "DETACHED_PRODUCER_CANNOT_REUSE_RELEASED_BUFFER_ALLOWANCE"
    );
}
#[test]
fn encoded_byte_limit_is_independent_of_command_count_and_atomic() {
    let (mut owner, mut producer) = bound();
    for index in 1..=2 {
        producer
            .execute(
                &mut owner,
                Action::Do {
                    operations: vec![Operation::AddMarker {
                        marker: Marker {
                            id: MarkerId(index),
                            position: TimeCode(0),
                            label: "x".repeat((1 << 20) - 8192),
                            color_token: 0,
                        },
                    }],
                    gesture: None,
                },
                context(index + 1),
            )
            .unwrap();
    }
    assert_eq!(
        producer.pending_count(),
        2,
        "BYTE_WINDOW_WITNESS_HAS_ONLY_TWO_COMMANDS"
    );
    let before = owner.identity().unwrap();
    let operation = Operation::AddMarker {
        marker: Marker {
            id: MarkerId(3),
            position: TimeCode(0),
            label: "z".repeat(64 << 10),
            color_token: 0,
        },
    };
    assert_eq!(
        producer.execute(
            &mut owner,
            Action::Do {
                operations: vec![operation],
                gesture: None
            },
            context(4)
        ),
        Err(ProducerError::Protocol(Conflict::Backpressure)),
        "RAW_BYTE_WINDOW_REFUSES_BEFORE_EDIT"
    );
    assert_eq!(
        owner.identity().unwrap(),
        before,
        "RAW_BYTE_WINDOW_REFUSAL_PRESERVES_STATE"
    );
    assert!(
        producer.pending_bytes() <= PENDING_BYTES,
        "ENCODED_OUTBOX_STAYS_INSIDE_FIXED_WINDOW"
    );
}

#[test]
fn retained_sender_bytes_keep_the_source_budget_until_the_last_drop() {
    let (mut owner, mut producer) = bound();
    let pool = owner.pool();
    let reservation = pool.usage(Role::Ordinary).unwrap().0;
    producer
        .execute(&mut owner, edit(true), context(2))
        .unwrap();
    let sender = producer.pending();
    let receipts: Vec<_> = producer.state().receipts().cloned().collect();
    producer.acknowledge(&receipts).unwrap();
    assert_eq!(
        producer.detach(&mut owner),
        Err(ProducerError::Protocol(Conflict::Backpressure)),
        "LIVE_SENDER_BYTES_KEEP_THE_ANCHOR_ALLOWANCE_RESERVED"
    );
    drop(producer);
    drop(owner);
    assert_eq!(
        pool.usage(Role::Ordinary).unwrap().0,
        reservation,
        "ENCODED_SENDER_RETAINS_SOURCE_RESERVATION"
    );
    drop(sender);
    assert_eq!(
        pool.usage(Role::Ordinary).unwrap(),
        (0, 0),
        "LAST_ENCODED_SENDER_DROP_RELEASES_SOURCE_RESERVATION"
    );
}
#[test]
fn sender_release_reopens_the_fixed_arena_without_growing_the_lease() {
    let (mut owner, mut producer) = bound();
    let pool = owner.pool();
    let before = pool.usage(Role::Ordinary).unwrap();
    let mut retained = Vec::new();
    let mut blocked = false;
    for index in 1..=16 {
        let action = Action::Do {
            operations: vec![Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(index),
                    position: TimeCode(0),
                    label: "x".repeat((1 << 20) - 8192),
                    color_token: 0,
                },
            }],
            gesture: None,
        };
        match producer.execute(&mut owner, action, context(index + 2)) {
            Ok(_) => {
                let receipt = producer.state().receipts().last().unwrap().clone();
                retained.extend(producer.pending());
                producer.acknowledge(&[receipt]).unwrap();
            }
            Err(ProducerError::Protocol(Conflict::Backpressure)) => {
                blocked = true;
                break;
            }
            other => panic!("UNEXPECTED_ARENA_WITNESS_RESULT: {other:?}"),
        }
    }
    assert!(blocked, "RETAINED_SENDERS_CANNOT_EXCEED_FIXED_ARENA");
    assert!(
        producer.arena.used.load(Ordering::Relaxed) <= producer.arena.limit,
        "ENCODED_ARENA_CHARGE_NEVER_EXCEEDS_LIMIT"
    );
    assert_eq!(
        pool.usage(Role::Ordinary).unwrap(),
        before,
        "ENCODED_ARENA_NEVER_GROWS_HISTORY_LEASE"
    );
    drop(retained);
    producer
        .detach(&mut owner)
        .expect("DROPPED_SENDERS_REOPEN_RESERVED_BUFFER");
    assert_eq!(
        producer.arena.used.load(Ordering::Relaxed),
        0,
        "LAST_RAW_DROP_RETURNS_EVERY_BYTE"
    );
}
#[test]
fn encoded_header_charge_covers_its_typed_allocation() {
    assert!(
        std::mem::size_of::<EncodedPayload>() as u64 + 2 * std::mem::size_of::<usize>() as u64 + 32
            <= ENCODED_HEADER_BYTES,
        "ENCODED_RAW_HEADER_CHARGE_COVERS_TYPED_ALLOCATION"
    );
}

#[test]
fn rebinding_keeps_all_retained_senders_in_the_original_arena() {
    let (mut owner, mut producer) = bound();
    let mut retained = Vec::new();
    let mut blocked = false;
    let action = |index| Action::Do {
        operations: vec![Operation::AddMarker {
            marker: Marker {
                id: MarkerId(index),
                position: TimeCode(0),
                label: "x".repeat((1 << 20) - 8192),
                color_token: 0,
            },
        }],
        gesture: None,
    };
    for index in 1..=16 {
        match producer.execute(&mut owner, action(index), context(index + 2)) {
            Ok(_) => {
                retained.extend(producer.pending());
                producer
                    .acknowledge(&[producer.state().receipts().last().unwrap().clone()])
                    .unwrap();
            }
            Err(ProducerError::Protocol(Conflict::Backpressure)) => {
                blocked = true;
                break;
            }
            other => panic!("UNEXPECTED_REBIND_ARENA_WITNESS: {other:?}"),
        }
    }
    assert!(blocked);
    let charged = producer.arena.used.load(Ordering::Relaxed);
    let mut lease = producer.lease().clone();
    lease.snapshot = owner.identity().unwrap();
    lease.next_key = owner.history().position();
    lease.generation += 1;
    lease.state = producer.state().clone();
    lease.next_issuer_sequence = producer.next_issuer;
    producer.rebind(lease, &owner).unwrap();
    assert_eq!(
        producer.arena.used.load(Ordering::Relaxed),
        charged,
        "REBIND_RETAINS_EVERY_OLD_SENDER_CHARGE"
    );
    assert_eq!(
        producer.execute(&mut owner, action(99), context(99)),
        Err(ProducerError::Protocol(Conflict::Backpressure)),
        "REBIND_CANNOT_MINT_A_SECOND_BUFFER_ALLOWANCE"
    );
    drop(retained);
    producer
        .execute(&mut owner, action(99), context(99))
        .unwrap();
}

#[test]
fn pending_message_roundtrips_inside_an_internally_tagged_command() {
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "command")]
    enum Envelope {
        Commit { pending: EncodedMessage },
    }
    let (mut owner, mut producer) = bound();
    producer
        .execute(&mut owner, edit(true), context(2))
        .unwrap();
    let pending = producer.pending().remove(0);
    let expected = pending.decode().unwrap();
    let wire = serde_json::to_vec(&Envelope::Commit { pending }).unwrap();
    let Envelope::Commit { pending } = serde_json::from_slice(&wire)
        .expect("PENDING_MESSAGE_DECODES_INSIDE_TAGGED_FOREGROUND_COMMAND");
    assert_eq!(
        pending.decode().unwrap(),
        expected,
        "TAGGED_TRANSPORT_PRESERVES_ORIGINAL_COMMAND_AND_CHAIN"
    );
}
