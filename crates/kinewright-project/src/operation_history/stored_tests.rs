use super::super::{
    ApprovalAnswer, HistoryPool, OwnerHistory, PoolLimits, Role,
    tests::{approval, document, origin, remove},
};
use super::*;
use kinewright_core::{Command, Operation, TrackId, undo::RetentionPolicy};
fn session(
    name: &str,
) -> (
    kinewright_media::test_support::TempDirectory,
    crate::bundle_copy::SessionStore,
) {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        name,
    );
    let store =
        crate::bundle_copy::SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    (temp, store)
}

fn stored_older_install(case: super::super::roundtrip_tests::InstallCase) {
    let (_temp, mut session) = session("hx-old-epoch-reopen");
    let store = session.store_mut();
    let bytes = b"old epoch approval";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let _pin = store
        .admit_blob(&mut bytes.as_slice(), &reference, &[])
        .unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let owner = super::super::roundtrip_tests::older_install(&pool, case, reference);
    let capture = owner.capture().unwrap();
    let sealed = capture.persist(store, |_| Ok(None)).unwrap();
    drop(capture);
    let opened = StoredOwnerRecord::read(store, &sealed.reference, &pool, Role::Task)
        .expect("SUCCESSFULLY_PERSISTED_HISTORY_MUST_REOPEN")
        .open()
        .unwrap();
    super::super::roundtrip_tests::assert_same(&owner, &opened);
}

#[test]
fn stored_older_snapshot_install_with_later_do_can_reopen() {
    stored_older_install(super::super::roundtrip_tests::InstallCase::Single);
}
#[test]
fn stored_two_older_snapshot_installs_with_later_do_can_reopen() {
    stored_older_install(super::super::roundtrip_tests::InstallCase::Twice);
}
#[test]
fn stored_older_snapshot_install_with_approved_undo_can_reopen() {
    stored_older_install(super::super::roundtrip_tests::InstallCase::Undo);
}
#[test]
fn compact_record_roundtrip_at_ten_thousand_commands_is_openable() {
    let (_temp, mut session) = session("hx-compact-record");
    let store = session.store_mut();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
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
    let expected = owner.history().snapshot_identity().unwrap();
    let capture = owner.capture().unwrap();
    let sealed = capture.persist(store, |_| Ok(None)).unwrap();
    let record = store
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take();
    assert_eq!(
        record.identity,
        digest_text(&expected),
        "STORED_CORE_CANONICAL_IDENTITY"
    );
    let actual = kinewright_core::undo::payload_memory_bound(record.body.get().as_bytes()).unwrap()
        + record.body.get().len() as u64;
    assert!(
        actual <= owner.decode_bound,
        "ADMISSION_COVERS_STORED_DECODE actual={actual} admitted={}",
        owner.decode_bound
    );
    let encoded = sealed.reference.byte_length;
    let decoded = compact_decode_charge(&serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(
        encoded / 10_000 < 400,
        "COMPACT_BYTES_PER_COMMAND {encoded}"
    );
    assert!(
        decoded <= encoded + 8192,
        "COMPACT_DECODE_STAYS_NEAR_ENCODED"
    );
    let opened = record
        .resolve(store, &pool, Role::Ordinary)
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.history().snapshot_identity().unwrap(),
        expected,
        "STORED_OPEN_IDENTITY"
    );
    assert_eq!(
        opened.history().document(),
        owner.history().document(),
        "STORED_OPEN_DOCUMENT"
    );
}
#[test]
fn task_holds_256_approve_delete_undo_cycles_in_its_role() {
    let (_temp, mut session) = session("hx-task-256-cycles");
    let store = session.store_mut();
    let mut evidence_pins = Vec::new();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    for i in 0..256 {
        let bytes = format!("approval evidence {i}");
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes.as_bytes()),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        evidence_pins.push(
            store
                .admit_blob(&mut bytes.as_bytes(), &reference, &[])
                .unwrap(),
        );
        let mut a = approval();
        a.reference = reference;
        owner
            .execute(remove(), origin(i * 2 + 2), vec![], Some(a))
            .unwrap();
        owner
            .execute(Command::Undo, origin(i * 2 + 3), vec![], None)
            .unwrap();
    }
    assert_eq!(owner.ledger().entries().count(), 256, "TASK_256_APPROVALS");
    assert!(
        owner
            .ledger()
            .entries()
            .all(|entry| entry.answer() == ApprovalAnswer::Missing),
        "TASK_256_COMMAND_FATES"
    );
    assert!(
        owner.owner_bytes() <= 4 << 20,
        "TASK_LEDGER_INSIDE_FOUR_MIB"
    );
    assert!(
        owner.history().retained_live_bound().unwrap() + owner.owner_bytes() <= 36 << 20,
        "TASK_RETAINED_INSIDE_36_MIB"
    );
    let capture = owner.capture().unwrap();
    let sealed = capture.persist(store, |_| Ok(None)).unwrap();
    assert!(
        sealed.reference.byte_length <= Role::Task.persisted_shell_cap(),
        "TASK_256_RECORD_INSIDE_PERSISTED_CAP"
    );
    let opened = store
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take()
        .resolve(store, &pool, Role::Task)
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(opened.ledger(), owner.ledger(), "TASK_256_LEDGER_ROUNDTRIP");
    println!(
        "HX_TASK_256={}",
        serde_json::json!({"approvals":owner.ledger().len(),"owner_bytes":owner.owner_bytes(),"retained_bound":owner.history().retained_live_bound().unwrap()+owner.owner_bytes(),"record_bytes":sealed.reference.byte_length,"decode_forecast":owner.decode_bound,"unique_approval_edges":evidence_pins.len()})
    );
    drop(evidence_pins);
}
#[test]
fn redo_payload_owns_a_real_blob_through_collection_and_open() {
    let (_temp, mut session) = session("hx-redo-blob");
    let store = session.store_mut();
    let bytes = b"owned LUT bytes";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let blob = store
        .admit_blob(&mut bytes.as_slice(), &reference, &[])
        .unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let document = serde_json::from_value(cases["AddLutAsset"]["document"].clone()).unwrap();
    let mut operation: Operation =
        serde_json::from_value(cases["AddLutAsset"]["operation"].clone()).unwrap();
    if let Operation::AddLutAsset { asset } = &mut operation {
        asset.sha256 = reference.sha256.clone();
        asset.byte_len = reference.byte_length;
    } else {
        unreachable!()
    }
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document),
    )
    .unwrap();
    owner
        .execute(Command::Do(operation), origin(2), vec![], None)
        .unwrap();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    let capture = owner.capture().unwrap();
    let sealed = capture
        .persist(store, |r| {
            Ok(if matches!(r, Resource::Lut { .. }) {
                Some(reference.clone())
            } else {
                None
            })
        })
        .unwrap();
    drop(blob);
    store.collect_unpublished().unwrap();
    assert!(
        store
            .closure(std::slice::from_ref(&sealed.reference))
            .unwrap()
            .contains(&reference),
        "REDO_BLOB_GRAPH_EDGE"
    );
    let mut opened = store
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take()
        .resolve(store, &pool, Role::Task)
        .unwrap()
        .open()
        .unwrap();
    opened
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    assert_eq!(
        opened.history().document().lut_assets.len(),
        1,
        "REDO_BLOB_SURVIVES_COLLECTION"
    );
    let roots = store
        .closure(std::slice::from_ref(&sealed.reference))
        .unwrap();
    drop(sealed);
    drop(capture);
    drop(opened);
    store.collect_unpublished().unwrap();
    assert!(
        store.closure(&roots).is_err(),
        "UNOWNED_RECORD_GRAPH_COLLECTED"
    );
}

#[test]
fn media_binding_owns_its_manifest_and_chunk_table() {
    let (_temp, mut session) = session("hx-media-context");
    let length = 9600_u32;
    let mut wave = Vec::new();
    wave.extend(b"RIFF");
    wave.extend((length + 36).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16_u32.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(48000_u32.to_le_bytes());
    wave.extend(96000_u32.to_le_bytes());
    wave.extend(2_u16.to_le_bytes());
    wave.extend(16_u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(length.to_le_bytes());
    wave.resize(wave.len() + length as usize, 0);
    let (asset, entry, pins) = session
        .store_mut()
        .import_media(
            &mut wave.as_slice(),
            wave.len() as u64,
            kinewright_core::AssetId(1),
            std::path::Path::new("history.wav"),
            None,
        )
        .unwrap();
    let table = entry.chunk_table_ref().unwrap().clone();
    let media = entry.object.clone();
    let manifest = crate::owned_inputs::OwnedInputManifest {
        project_id: None,
        session_id: Some(session.session_id().clone()),
        owner_epoch: session.store().owner_epoch(),
        entries: vec![entry],
    };
    let context = session.admit_inputs(&manifest).unwrap();
    let context_ref = context.reference.clone();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut doc = document();
    doc.media_pool.push(asset);
    let owner = OwnerHistory::new(
        &pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(doc),
    )
    .unwrap();
    let capture = owner.capture().unwrap();
    let sealed = capture
        .persist(session.store_mut(), |resource| {
            Ok(
                if matches!(
                    resource,
                    Resource::Media {
                        sha256: Some(_),
                        ..
                    }
                ) {
                    Some(context_ref.clone())
                } else {
                    None
                },
            )
        })
        .unwrap();
    drop(pins);
    drop(context);
    session.store_mut().collect_unpublished().unwrap();
    let closure = session
        .store()
        .closure(std::slice::from_ref(&sealed.reference))
        .unwrap();
    assert!(
        closure.contains(&context_ref) && closure.contains(&table) && closure.contains(&media),
        "MEDIA_CONTEXT_TABLE_AND_BYTES_GRAPH"
    );
    let opened = session
        .store()
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take()
        .resolve(session.store(), &pool, Role::Task)
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.history().document().media_pool.len(),
        1,
        "MEDIA_CONTEXT_OPEN"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "all malformed variants share one unchanged destination and installed-state check"
)]
fn stored_owner_metadata_malformations_are_typed_and_atomic() {
    let (_temp, mut session) = session("hx-malformed-owner");
    let store = session.store_mut();
    let evidence = b"owner consent";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(evidence),
        byte_length: evidence.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let _pin = store
        .admit_blob(&mut evidence.as_slice(), &reference, &[])
        .unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    for n in 0..2 {
        owner
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(81),
                    locked: n == 0,
                }),
                origin(n + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    let mut approved = approval();
    approved.reference = reference;
    owner
        .execute(remove(), origin(4), vec![], Some(approved))
        .unwrap();
    owner
        .execute(Command::Undo, origin(5), vec![], None)
        .unwrap();
    let identity = owner.history().snapshot_identity().unwrap();
    let capture = owner.capture().unwrap();
    let sealed = capture.persist(store, |_| Ok(None)).unwrap();
    let corrupt = |change: fn(&mut serde_json::Value)| {
        let mut record = store
            .read_record::<StoredOwnerRecord>(&sealed.reference)
            .unwrap()
            .take();
        // An independent malformed wire value, not an estimator copy.
        let mut body: serde_json::Value = serde_json::from_str(record.body.get()).unwrap();
        change(&mut body);
        record.body = serde_json::value::to_raw_value(&body).unwrap();
        record
    };
    for (change, error, name) in [
        (
            (|b: &mut serde_json::Value| {
                b["origins"].as_array_mut().unwrap().pop();
            }) as fn(&mut serde_json::Value),
            Error::InvalidOrigin,
            "STORED_ORIGIN_BOUNDARY_REFUSED",
        ),
        (
            (|b: &mut serde_json::Value| {
                b["ledger"]["commands"][0]["key"]["position"] = serde_json::json!(3);
            }),
            Error::InvalidLedger,
            "STORED_ABSENT_APPROVAL_COMMAND_REFUSED",
        ),
        (
            (|b: &mut serde_json::Value| {
                b["ledger"]["commands"][0]["payload_digest"][0] = serde_json::json!(256);
            }),
            Error::InvalidRecord,
            "STORED_MALFORMED_DIGEST_REFUSED",
        ),
        (
            (|b: &mut serde_json::Value| {
                b["ledger"]["commands"][0]["fate"] = serde_json::json!("Folded");
            }),
            Error::InvalidLedger,
            "STORED_FORGED_FOLDED_FATE_REFUSED",
        ),
        (
            (|b: &mut serde_json::Value| {
                b["seeds"]["occurrences"]["999"] = serde_json::json!({"branch_origin":"1".repeat(32),"creation_batch_id":"2".repeat(32),"result_ordinal":0});
            }),
            Error::InvalidSeeds,
            "STORED_EXTRA_OCCURRENCE_REFUSED",
        ),
    ] {
        assert_eq!(
            corrupt(change).resolve(store, &pool, Role::Task).err(),
            Some(error),
            "{name}"
        );
        assert_eq!(
            owner.history().snapshot_identity().unwrap(),
            identity,
            "STORED_REFUSAL_DESTINATION_UNCHANGED {name}"
        );
    }
}

#[test]
fn folded_approval_requires_the_installed_baselines_exact_fact() {
    let (_temp, mut session) = session("hx-folded-approval");
    let store = session.store_mut();
    let bytes = b"fold consent";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let _pin = store
        .admit_blob(&mut bytes.as_slice(), &reference, &[])
        .unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    owner.core = kinewright_core::undo::History::new(
        document(),
        RetentionPolicy::Fold,
        kinewright_core::undo::RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
    )
    .unwrap();
    let mut a = approval();
    a.reference = reference;
    let key = owner
        .execute(remove(), origin(2), vec![], Some(a))
        .unwrap()
        .accepted;
    owner
        .execute(
            Command::Do(Operation::AddMarker {
                marker: kinewright_core::Marker {
                    id: kinewright_core::MarkerId(1),
                    position: kinewright_core::TimeCode(0),
                    label: "fold".into(),
                    color_token: 0,
                },
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    assert_eq!(
        owner.ledger().get(key).unwrap().fate,
        super::super::Fate::Folded,
        "FOLD_RECORDS_APPROVAL_FACT"
    );
    let capture = owner.capture().unwrap();
    let sealed = capture.persist(store, |_| Ok(None)).unwrap();
    let record = store
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take();
    let actual = kinewright_core::undo::payload_memory_bound(record.body.get().as_bytes()).unwrap()
        + record.body.get().len() as u64;
    assert!(
        actual <= owner.decode_bound,
        "FOLDED_PREFIX_PROOF_HAS_DECODE_RESERVATION actual={actual} bound={}",
        owner.decode_bound
    );
    let opened = record
        .resolve(store, &pool, Role::Ordinary)
        .expect("FOLDED_APPROVAL_RECORD_OPENS")
        .open()
        .unwrap();
    assert_eq!(
        opened.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Kept,
        "FOLDED_COMMAND_KEPT_BY_ITS_OWN_FACT"
    );
    drop(opened);
    let mut corrupt = store
        .read_record::<StoredOwnerRecord>(&sealed.reference)
        .unwrap()
        .take();
    let mut value: serde_json::Value = serde_json::from_str(corrupt.body.get()).unwrap();
    value["folded_approvals"] = serde_json::json!([]);
    corrupt.body = serde_json::value::to_raw_value(&value).unwrap();
    assert_eq!(
        corrupt.resolve(store, &pool, Role::Ordinary).err(),
        Some(Error::InvalidLedger),
        "BASELINE_EXACT_APPROVAL_FACT_REQUIRED"
    );
}

#[test]
fn closed_and_older_gesture_keys_roundtrip_inside_decode_forecast() {
    let (_temp, mut session) = session("hx-gesture-keys");
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
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
    assert!(
        owner
            .history()
            .undo()
            .iter()
            .take(3)
            .all(|step| step.coalesce_key().is_some()),
        "CORE_CAN_RETAIN_OLDER_GESTURE_KEYS"
    );
    for close in [false, true] {
        if close {
            owner.close_gesture().unwrap();
        }
        let identity = owner.history().snapshot_identity().unwrap();
        let snapshot = owner.capture().unwrap();
        let sealed = snapshot.persist(session.store_mut(), |_| Ok(None)).unwrap();
        let record = session
            .store_mut()
            .read_record::<StoredOwnerRecord>(&sealed.reference)
            .unwrap()
            .take();
        let actual = kinewright_core::undo::payload_memory_bound(record.body.get().as_bytes())
            .unwrap()
            + record.body.get().len() as u64;
        assert!(
            actual <= owner.decode_bound,
            "ALL_RETAINED_GESTURE_KEYS_HAVE_DECODE_RESERVATION actual={actual} bound={}",
            owner.decode_bound
        );
        let opened = record
            .resolve(session.store_mut(), &pool, Role::Ordinary)
            .unwrap()
            .open()
            .unwrap();
        assert_eq!(
            opened.history().snapshot_identity().unwrap(),
            identity,
            "OLDER_GESTURE_KEYS_ROUNDTRIP_IDENTITY"
        );
    }
}

#[test]
fn open_reserves_before_reading_the_owner_record() {
    let (_temp, session) = session("hx-open-reserve");
    let pool = HistoryPool::new(PoolLimits {
        retained: [0; 3],
        peak: [0; 3],
        workspace: 0,
    });
    let nonexistent = ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 999,
        record_kind: RecordKind::OperationHistory,
    };
    assert!(
        matches!(
            StoredOwnerRecord::read(session.store(), &nonexistent, &pool, Role::Task),
            Err(Error::Reservation)
        ),
        "OPEN_REFUSAL_PRECEDES_RECORD_ALLOCATION"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (0, 0),
        "OPEN_REFUSAL_HAS_NO_PARTIAL_LEASE"
    );
    let pool = HistoryPool::new(PoolLimits::default());
    let oversized = ObjectRef {
        byte_length: Role::Candidate.persisted_shell_cap() + 1,
        ..nonexistent
    };
    assert!(
        matches!(
            StoredOwnerRecord::read(session.store(), &oversized, &pool, Role::Candidate),
            Err(Error::Capacity("persisted owner shell"))
        ),
        "OPEN_REQUESTED_ROLE_CAP_PRECEDES_RECORD_ALLOCATION"
    );
    assert_eq!(
        pool.usage(Role::Candidate).unwrap(),
        (0, 0),
        "OVERSIZED_OPEN_HAS_NO_PARTIAL_LEASE"
    );
}
