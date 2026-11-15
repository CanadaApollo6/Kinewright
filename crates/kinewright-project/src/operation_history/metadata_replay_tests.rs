use super::tests::{document, origin};
use super::*;
use kinewright_core::{Command, Operation, TrackId};

#[test]
fn admitted_manifest_context_history_reopens_at_the_owner_bound() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    let input = crate::bundle_records::ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 1,
        record_kind: crate::bundle_records::RecordKind::InputManifest,
    };
    let mut accepted = 0;
    for index in 0..4096 {
        let before = owner.history().position();
        match owner.execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: index % 2 == 0,
            }),
            origin(index + 2),
            vec![input.clone()],
            None,
        ) {
            Ok(_) => accepted += 1,
            Err(Error::Capacity("owner metadata")) => {
                assert_eq!(
                    owner.history().position(),
                    before,
                    "MANIFEST_METADATA_REFUSAL_PRESERVES_COUNTER"
                );
                break;
            }
            Err(error) => panic!("MANIFEST_CONTEXT_CAPACITY_IS_A_TYPED_OWNER_LIMIT: {error}"),
        }
    }
    assert!(
        accepted >= 2000,
        "MANIFEST_CONTEXT_STILL_HOLDS_TWO_THOUSAND_COMMANDS"
    );
    let captured = owner.capture().unwrap();
    let identity = captured.identity().unwrap();
    let OwnerSnapshot { data, reservation } = captured;
    let bytes = serde_json::to_vec(&data).unwrap();
    drop((data, reservation));
    let data: super::snapshot::SnapshotData = serde_json::from_slice(&bytes).unwrap();
    assert!(
        data.origins
            .iter()
            .all(|origin| origin.inputs.len() == 1 && origin.inputs.capacity() >= 4),
        "WIRE_MANIFEST_VECTOR_GEOMETRY_EXERCISED"
    );
    let reader_pool = HistoryPool::new(PoolLimits::default());
    let reservation = reader_pool.admit_sized(data.sizing).unwrap();
    let opened = OwnerSnapshot { data, reservation }
        .validate()
        .expect("ADMITTED_MANIFEST_CONTEXT_HISTORY_REOPENS")
        .open()
        .unwrap();
    assert_eq!(
        opened.capture().unwrap().identity().unwrap(),
        identity,
        "MANIFEST_CONTEXT_OPEN_PRESERVES_IDENTITY"
    );
    assert_eq!(
        opened.history().undo().len(),
        accepted,
        "MANIFEST_CONTEXT_OPEN_PRESERVES_EVERY_ACCEPTED_COMMAND"
    );
}
