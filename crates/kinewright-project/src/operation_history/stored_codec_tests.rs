use super::super::{
    OwnerHistory,
    tests::{document, origin},
};
use super::*;
use kinewright_core::{Command, Operation, TrackId};

fn owner() -> OwnerHistory {
    OwnerHistory::new(
        &super::super::tests::story_pool(),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap()
}

pub(in crate::operation_history) fn encoded(snapshot: &OwnerSnapshot) -> Vec<u8> {
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
        seeds: SeedValues::Captured(&data.seeds),
        origins: OriginContexts::Captured(&data.origins),
        resources: ResourceValues::Captured(&data.resources),
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
        sizing: data.sizing,
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

#[test]
fn unhashed_media_is_not_an_owned_blob_binding() {
    let resource = Resource::Media {
        asset: 1,
        sha256: None,
        byte_len: Some(1),
    };
    let blob = ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedMedia,
    };
    assert!(
        !blob_matches(&resource, &blob),
        "PORTABLE_UNHASHED_MEDIA_IS_NOT_AN_OWNED_BINDING"
    );
}

#[test]
fn oversize_baseline_is_refused_before_loading_the_second_document() {
    use std::cell::Cell;
    struct Source {
        limits: StorageLimits,
        loads: Cell<usize>,
    }
    impl SnapshotSource for Source {
        fn limits(&self) -> &StorageLimits {
            &self.limits
        }
        fn document(&self, _: &ObjectRef) -> Result<kinewright_core::Document, Error> {
            let count = self.loads.get() + 1;
            self.loads.set(count);
            assert_eq!(count, 1, "OVERSIZE_BASELINE_NEVER_LOADS_SECOND_DOCUMENT");
            Ok(kinewright_core::Document {
                tracks: (0..100_000)
                    .map(|id| kinewright_core::Track {
                        id: TrackId(id),
                        kind: kinewright_core::TrackKind::Video,
                        sync_lock: false,
                        clips: vec![],
                    })
                    .collect(),
                ..kinewright_core::Document::default()
            })
        }
        fn binding_matches(&self, _: &Resource, _: &ObjectRef) -> Result<bool, Error> {
            Ok(true)
        }
    }
    let pool = super::super::HistoryPool::new(super::super::PoolLimits::default());
    let owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    let snapshot = owner.capture().unwrap();
    let record: StoredOwnerRecord = serde_json::from_slice(&encoded(&snapshot)).unwrap();
    drop(snapshot);
    let reservation = pool.admit_sized(record.sizing).unwrap();
    let bounds = reservation.limits().bounds;
    let window = reservation.workspace().unwrap();
    let source = Source {
        limits: StorageLimits::default(),
        loads: Cell::new(0),
    };
    assert!(
        matches!(
            record.resolve_reserved(&source, Role::Task, reservation, window, bounds),
            Err(Error::History(
                kinewright_core::undo::Error::DocumentCapacity
            ))
        ),
        "UNTRUSTED_BASELINE_LIVE_LIMIT_IS_CHECKED_BEFORE_NEXT_READ"
    );
    assert_eq!(
        source.loads.get(),
        1,
        "EXACTLY_ONE_UNTRUSTED_DOCUMENT_IN_INGRESS"
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one complete spare-capacity stream, stored round trip and fresh merge"
)]
#[test]
#[ignore = "on-demand: release qualification of spare baseline capacity across a long split stream"]
fn spare_baseline_capacity_preserves_save_open_and_fresh_merge_fit() {
    use super::super::sizing_tests::fixture;
    use std::{cell::Cell, collections::BTreeMap, sync::Arc};
    struct Source {
        limits: StorageLimits,
        documents: BTreeMap<ObjectRef, Vec<u8>>,
        loads: Cell<usize>,
    }
    impl SnapshotSource for Source {
        fn limits(&self) -> &StorageLimits {
            &self.limits
        }
        fn document(&self, reference: &ObjectRef) -> Result<kinewright_core::Document, Error> {
            self.loads.set(self.loads.get() + 1);
            let bytes = &self.documents[reference];
            assert_eq!(
                kinewright_media::sha256_bytes(bytes),
                reference.sha256,
                "SPARE_SOURCE_VERIFIES_DOCUMENT_BYTES"
            );
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidRecord)
        }
        fn binding_matches(&self, _: &Resource, _: &ObjectRef) -> Result<bool, Error> {
            Ok(false)
        }
    }
    let mut baseline = fixture::document(2500, 1, 90);
    baseline.tracks[0].clips.reserve_exact(500);
    assert_eq!(
        baseline.tracks[0].clips.capacity(),
        3000,
        "SPARE_BASELINE_FIXED_CAPACITY"
    );
    let pool = super::super::HistoryPool::new(super::super::PoolLimits::default());
    let mut owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(baseline)
    })
    .unwrap();
    let operations: Vec<_> = (0..7501)
        .map(|i| {
            let (parent, within) = if i == 7500 {
                (2499, 4)
            } else {
                (i / 3, i % 3 + 1)
            };
            Operation::SplitClip {
                clip: kinewright_core::ClipId(if within == 1 { parent + 1 } else { 2500 + i }),
                at: kinewright_core::TimeCode(i64::try_from(parent * 90 + within).unwrap()),
            }
        })
        .collect();
    for (i, batch) in operations.chunks(32).enumerate() {
        let before = owner.capture().unwrap().snapshot_identity().unwrap();
        match owner.execute(
            Command::DoBatch(batch.to_vec()),
            origin(i as u64 + 2),
            vec![],
            None,
        ) {
            Ok(_) => {}
            Err(Error::History(kinewright_core::undo::Error::DocumentCapacity)) => {
                assert_eq!(
                    owner.capture().unwrap().snapshot_identity().unwrap(),
                    before,
                    "SPARE_SOURCE_LIMIT_REFUSAL_IS_ATOMIC"
                );
                break;
            }
            Err(error) => panic!("SPARE_SOURCE_ACCEPTS_WITHIN_EVERY_LIMIT batch={i}: {error}"),
        }
    }
    let snapshot = owner.capture().unwrap();
    let expected = snapshot.snapshot_identity().unwrap();
    let mut source = Source {
        limits: StorageLimits::default(),
        documents: BTreeMap::new(),
        loads: Cell::new(0),
    };
    let mut put = |document: &kinewright_core::Document| {
        let bytes = serde_json::to_vec(document).unwrap();
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::Document,
        };
        source.documents.insert(reference.clone(), bytes);
        reference
    };
    let baseline = put(&snapshot.data.history.baseline);
    let current = put(&snapshot.data.history.document);
    let bytes =
        serde_json::to_vec(&snapshot.stored_record(baseline, current, &[]).unwrap()).unwrap();
    drop((snapshot, owner));
    let record: StoredOwnerRecord = serde_json::from_slice(&bytes).unwrap();
    let reservation = pool.admit_sized(record.sizing).unwrap();
    let bounds = reservation.limits().bounds;
    let window = reservation.workspace().unwrap();
    let reopened = record
        .resolve_reserved(&source, Role::Task, reservation, window, bounds)
        .expect("SPARE_CAPACITY_WHAT_CAN_BE_SAVED_CAN_BE_OPENED")
        .open()
        .unwrap();
    assert_eq!(
        reopened.capture().unwrap().snapshot_identity().unwrap(),
        expected,
        "SPARE_CAPACITY_OPEN_PRESERVES_COMBINED_IDENTITY"
    );
    let delta: Vec<_> = reopened.history().applied_delta().cloned().collect();
    let mut stage = OwnerHistory::staging(
        &pool,
        &origin(1),
        reopened.reservation.sizing(),
        &delta,
        || Ok(Arc::clone(reopened.history().baseline())),
    )
    .unwrap();
    stage
        .execute(Command::DoBatch(delta), origin(9000), vec![], None)
        .expect("SPARE_CAPACITY_SOURCE_FITS_FRESH_STAGING");
    assert_eq!(
        stage.history().document(),
        reopened.history().document(),
        "SPARE_CAPACITY_MERGE_PRESERVES_DOCUMENT"
    );
}

#[test]
fn an_admitted_large_gesture_always_has_a_decodable_owner_body() {
    let pool = super::super::HistoryPool::new(super::super::PoolLimits::default());
    let mut owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(super::super::sizing_tests::fixture::document(5000, 8, 90))
    })
    .unwrap();
    let before = owner.capture().unwrap().snapshot_identity().unwrap();
    let result = owner.execute(
        Command::DoBatchCoalesced {
            operations: vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }],
            coalesce_key: "x".repeat(21 << 20),
        },
        origin(2),
        vec![],
        None,
    );
    match result {
        Ok(_) => {
            let snapshot = owner.capture().unwrap();
            let record: StoredOwnerRecord = serde_json::from_slice(&encoded(&snapshot)).unwrap();
            record
                .decode_body()
                .expect("ADMITTED_GESTURE_BODY_MUST_DECODE");
        }
        Err(Error::Capacity("owner decode")) => {
            assert_eq!(
                owner.capture().unwrap().snapshot_identity().unwrap(),
                before,
                "AGGREGATE_DECODE_REFUSAL_LEAVES_HISTORY_UNCHANGED"
            );
        }
        Err(error) => panic!("LARGE_GESTURE_HAS_A_TYPED_DECODE_RESULT: {error}"),
    }
}
