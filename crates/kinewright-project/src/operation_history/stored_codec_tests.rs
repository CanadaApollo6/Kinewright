use super::super::{
    HistoryPool, OwnerHistory, PoolLimits,
    tests::{document, origin},
};
use super::*;
use kinewright_core::{Command, Operation, TrackId};

fn owner() -> OwnerHistory {
    OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap()
}

fn encoded(snapshot: &OwnerSnapshot) -> Vec<u8> {
    let data = &snapshot.data;
    let history = &data.history;
    let body = serde_json::value::to_raw_value(&BorrowedBody {
        undo: &history.undo,
        redo: &history.redo,
        position: history.position,
        epoch: history.epoch,
        policy: history.policy,
        fold_frontier: data.fold_frontier,
        terminal_facts: &data.terminal_facts,
        seeds: &data.seeds,
        origins: &data.origins,
        resources: &data.resources,
        bindings: &[],
        ledger: &data.ledger,
        folded_approvals: &data.folded_approvals,
    })
    .unwrap();
    let document_ref = || ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::Document,
    };
    serde_json::to_vec(&StoredOwnerRecord {
        role: data.role,
        baseline: document_ref(),
        document: document_ref(),
        identity: digest_text(&snapshot.snapshot_identity().unwrap()),
        edges: vec![],
        body,
    })
    .unwrap()
}

fn roundtrip(owner: &OwnerHistory) {
    let snapshot = owner.capture().unwrap();
    let bytes = encoded(&snapshot);
    assert_eq!(
        compact_decode_charge(&bytes).unwrap(),
        bytes.len() as u64 + 4096,
        "PORTABLE_COMPACT_DECODE_LITERAL_CHARGE"
    );
    let record: StoredOwnerRecord = serde_json::from_slice(&bytes).unwrap();
    record.validate(&StorageLimits::default()).unwrap();
    let body = record.decode_body().unwrap();
    let actual = kinewright_core::undo::payload_memory_bound(record.body.get().as_bytes()).unwrap()
        + record.body.get().len() as u64;
    assert!(
        actual <= owner.decode_bound,
        "PORTABLE_CODEC_DECODE_FORECAST"
    );
    assert_eq!(body.position, owner.history().position().position);
    assert_eq!(body.epoch, owner.history().position().epoch);
    assert_eq!(body.origins.len(), snapshot.data.origins.len());
    let steps = body.undo.into_iter().chain(body.redo.into_iter().rev());
    let expected = snapshot
        .data
        .history
        .undo
        .iter()
        .chain(snapshot.data.history.redo.iter().rev());
    assert_eq!(steps.size_hint(), expected.size_hint());
    for (wire, expected) in steps.zip(expected) {
        let step = wire.into_core();
        assert_eq!(
            step.commands(),
            expected.commands(),
            "PORTABLE_CODEC_COMMAND_IDENTITY"
        );
        assert_eq!(
            step.coalesce_key(),
            expected.coalesce_key(),
            "PORTABLE_CODEC_GESTURE_KEY"
        );
    }
}

#[test]
fn compact_codec_roundtrips_ten_thousand_commands_without_a_store() {
    let mut owner = owner();
    for i in 0..10_000 {
        owner
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(81),
                    locked: i % 2 == 0,
                }),
                origin(i + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    roundtrip(&owner);
}

#[test]
fn compact_codec_preserves_closed_and_older_gesture_keys_without_a_store() {
    let mut owner = owner();
    for i in 0..4 {
        owner
            .execute(
                Command::DoBatchCoalesced {
                    operations: vec![Operation::SetTrackSyncLock {
                        track: TrackId(81),
                        locked: i % 2 == 0,
                    }],
                    coalesce_key: format!("{}-{i}", "g".repeat(8192)),
                },
                origin(i + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    roundtrip(&owner);
    owner.close_gesture().unwrap();
    owner
        .execute(Command::Undo, origin(6), vec![], None)
        .unwrap();
    roundtrip(&owner);
}

#[test]
fn malformed_stored_bytes_are_refused_without_a_store() {
    let mut owner = owner();
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
    let snapshot = owner.capture().unwrap();
    let bytes = encoded(&snapshot);
    let before = owner.history().snapshot_identity().unwrap();
    for change in [
        (|body: &mut serde_json::Value| {
            body["undo"][0][1][0][2] = serde_json::json!("g".repeat(64));
        }) as fn(&mut serde_json::Value),
        |body| body["undo"][0][1][0] = serde_json::json!([0, 1]),
        |body| body["unknown"] = serde_json::json!(true),
    ] {
        let mut record: StoredOwnerRecord = serde_json::from_slice(&bytes).unwrap();
        let mut body = serde_json::from_str(record.body.get()).unwrap();
        change(&mut body);
        record.body = serde_json::value::to_raw_value(&body).unwrap();
        assert_eq!(
            record.decode_body().err(),
            Some(Error::InvalidRecord),
            "PORTABLE_MALFORMED_BODY_TYPED_REFUSAL"
        );
        assert_eq!(
            owner.history().snapshot_identity().unwrap(),
            before,
            "PORTABLE_MALFORMED_BYTES_DESTINATION_UNCHANGED"
        );
    }
    let mut record: StoredOwnerRecord = serde_json::from_slice(&bytes).unwrap();
    record.identity = "bad".into();
    assert!(
        matches!(
            record.validate(&StorageLimits::default()),
            Err(BundleError::Corrupt(_))
        ),
        "PORTABLE_MALFORMED_SHELL_REFUSED"
    );
    assert!(
        serde_json::from_slice::<StoredOwnerRecord>(&bytes[..bytes.len() - 1]).is_err(),
        "PORTABLE_TRUNCATED_SHELL_REFUSED"
    );
    assert!(
        compact_decode_charge(&bytes[..bytes.len() - 1]).is_err(),
        "PORTABLE_TRUNCATED_SHELL_CHARGE_REFUSED"
    );
}
