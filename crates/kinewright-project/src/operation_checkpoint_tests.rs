use super::*;
#[cfg(target_os = "linux")]
use crate::operation_history::{Approval, ApprovalAnswer, OwnerHistory, PoolLimits, RemovedTarget};
use crate::{
    bundle_records::BatchId,
    operation_actor::CommandContext,
    operation_history::CreationOrigin,
    operation_protocol::{Action, Seed},
    receipts::IssuerId,
};
#[cfg(target_os = "linux")]
use kinewright_core::{Document, Operation, Track, TrackId, TrackKind};

fn reference(kind: RecordKind, digit: &str) -> ObjectRef {
    ObjectRef {
        sha256: digit.repeat(64),
        byte_length: 1,
        record_kind: kind,
    }
}
fn checkpoint() -> Checkpoint {
    let identity = SnapshotIdentity {
        core: [1; 32],
        owner: [2; 32],
    };
    let branch_id = BranchId::fresh().unwrap();
    let stream = StreamState::installed(Seed {
        project: ProjectId::fresh().unwrap(),
        branch: branch_id.clone(),
        attachment_epoch: 1,
        installed: identity,
    })
    .unwrap();
    Checkpoint {
        branch_id,
        head_id: HeadId::fresh().unwrap(),
        owner_ref: reference(RecordKind::OperationHistory, "1"),
        identity,
        role: Role::Ordinary,
        document_ref: reference(RecordKind::Document, "2"),
        revision: TimelineRevision(0),
        position: CommandKey {
            epoch: 0,
            position: 0,
        },
        input_manifest_ref: reference(RecordKind::InputManifest, "3"),
        generation: 1,
        predecessor: None,
        command_cut: None,
        take_map_revision_ref: None,
        stream: Some(stream),
        install: None,
    }
}
#[test]
fn checkpoint_codec_requires_the_new_owner_and_combined_install_identity() {
    let value = checkpoint();
    value.validate(&StorageLimits::default()).unwrap();
    let encoded = serde_json::to_vec(&value).unwrap();
    let decoded: Checkpoint = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded, value,
        "CHECKPOINT_CODEC_KEEPS_THE_COMPLETE_PUBLICATION_CUT"
    );
    let mut wrong = value.clone();
    wrong.identity.owner[0] ^= 1;
    assert!(
        wrong.validate(&StorageLimits::default()).is_err(),
        "FRESH_STREAM_ROOT_IS_ITS_COMBINED_INSTALLED_SNAPSHOT"
    );
    wrong = value.clone();
    wrong.owner_ref.record_kind = RecordKind::Document;
    assert!(
        wrong.validate(&StorageLimits::default()).is_err(),
        "CHECKPOINT_REQUIRES_THE_OPERATION_OWNER_RECORD_KIND"
    );
    let mut old = serde_json::to_value(value).unwrap();
    let fields = old.as_object_mut().unwrap();
    let old_history = fields.remove("owner_ref").unwrap();
    fields.insert("history_ref".into(), old_history);
    assert!(
        serde_json::from_value::<Checkpoint>(old).is_err(),
        "CHECKPOINT_CODEC_HAS_NO_OLD_RECORD_READER"
    );
}
#[test]
fn checkpoint_receipts_name_the_current_epoch_position_and_command_cut() {
    let mut value = checkpoint();
    let mut stream = value.stream.take().unwrap();
    let context = CommandContext {
        creation: CreationOrigin {
            branch: value.branch_id.clone(),
            batch: BatchId::fresh().unwrap(),
        },
        inputs: vec![],
        approval: None,
        transition: None,
    };
    let message = stream
        .message(
            CommandId {
                issuer: IssuerId("1".repeat(32)),
                sequence: 1,
            },
            value.position,
            Action::Close,
            context,
        )
        .unwrap();
    stream = stream.advanced(&message, value.position).unwrap().0;
    value.position.position = 1;
    value.command_cut = Some(message.id);
    value.stream = Some(stream);
    value.validate(&StorageLimits::default()).unwrap();
    let mut wrong = value.clone();
    wrong.command_cut = None;
    assert!(
        wrong.validate(&StorageLimits::default()).is_err(),
        "CHECKPOINT_RECEIPT_MATCHES_ITS_COMMAND_CUT"
    );
    wrong = value.clone();
    wrong.position.epoch += 1;
    assert!(
        wrong.validate(&StorageLimits::default()).is_err(),
        "CHECKPOINT_RECEIPTS_STAY_IN_THE_INSTALLED_EPOCH"
    );
    wrong = value;
    wrong.position.position += 1;
    assert!(
        wrong.validate(&StorageLimits::default()).is_err(),
        "CHECKPOINT_RECEIPT_CUT_MATCHES_ACCEPTED_POSITION"
    );
}

#[cfg(target_os = "linux")]
struct StoredFixture {
    _temp: kinewright_media::test_support::TempDirectory,
    store: crate::bundle_copy::SessionStore,
    object: crate::store::SealedObject,
    checkpoint: Checkpoint,
    approval: ObjectRef,
}
#[cfg(target_os = "linux")]
#[expect(
    clippy::too_many_lines,
    reason = "one admitted command, Undo, Close and sealed checkpoint fixture"
)]
fn stored_fixture() -> StoredFixture {
    use crate::owned_inputs::OwnedInputManifest;
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "hx-operation-checkpoint",
    );
    let mut store =
        crate::bundle_copy::SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let manifest = OwnedInputManifest {
        project_id: None,
        session_id: Some(store.session_id().clone()),
        owner_epoch: store.store().owner_epoch(),
        entries: vec![],
    };
    let inputs = store.admit_inputs(&manifest).unwrap();
    let evidence = b"approved removal";
    let approval = ObjectRef {
        sha256: kinewright_media::sha256_bytes(evidence),
        byte_length: evidence.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let _evidence_pin = store
        .store_mut()
        .admit_blob(&mut evidence.as_slice(), &approval, &[])
        .unwrap();
    let branch = BranchId::fresh().unwrap();
    let origin = CreationOrigin {
        branch: branch.clone(),
        batch: BatchId::fresh().unwrap(),
    };
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin,
        || {
            Ok(Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                }],
                ..Document::default()
            })
        },
    )
    .unwrap();
    let mut stream = StreamState::installed(Seed {
        project: ProjectId::fresh().unwrap(),
        branch: branch.clone(),
        attachment_epoch: 1,
        installed: owner.identity().unwrap(),
    })
    .unwrap();
    let approved = Approval {
        reference: approval.clone(),
        targets: vec![RemovedTarget::Track(TrackId(1))],
    };
    for (index, (action, approval)) in [
        (
            Action::Do {
                operations: vec![Operation::RemoveTrack { track: TrackId(1) }],
                gesture: None,
            },
            Some(approved),
        ),
        (Action::Undo, None),
        (Action::Close, None),
    ]
    .into_iter()
    .enumerate()
    {
        let inputs = if matches!(action, Action::Do { .. }) {
            vec![inputs.reference.clone()]
        } else {
            vec![]
        };
        let context = CommandContext {
            creation: CreationOrigin {
                branch: branch.clone(),
                batch: BatchId::fresh().unwrap(),
            },
            inputs,
            approval,
            transition: None,
        };
        let accepted = owner.history().position();
        let message = stream
            .message(
                CommandId {
                    issuer: IssuerId("1".repeat(32)),
                    sequence: index as u64 + 1,
                },
                accepted,
                action.clone(),
                context.clone(),
            )
            .unwrap();
        let effects = action.execute(context, &mut owner).unwrap();
        stream = stream.advanced(&message, effects.accepted).unwrap().0;
    }
    let capture = owner.capture().unwrap();
    let sealed = capture
        .persist_checkpoint(store.store_mut(), |_| Ok(None))
        .unwrap();
    let header = Header {
        branch,
        head: HeadId::fresh().unwrap(),
        revision: TimelineRevision(2),
        inputs: inputs.reference.clone(),
        generation: 1,
        predecessor: None,
        command_cut: Some(stream.receipts().last().unwrap().id.clone()),
        take_map: None,
        stream: Some(stream),
    };
    let checkpoint = Checkpoint::from_sealed(header, &sealed).unwrap();
    let object = store.store_mut().admit_record(&checkpoint).unwrap();
    StoredFixture {
        _temp: temp,
        store,
        object,
        checkpoint,
        approval,
    }
}
#[cfg(target_os = "linux")]
#[test]
fn sealed_checkpoint_reopens_its_exact_owner_and_receipt_cut() {
    let fixture = stored_fixture();
    let checkpoint = fixture
        .store
        .store()
        .read_record::<Checkpoint>(&fixture.object.reference)
        .unwrap();
    let project = &checkpoint.stream.as_ref().unwrap().seed().project;
    checkpoint.validate_scope(project).unwrap();
    let opened = checkpoint
        .open(
            fixture.store.store(),
            &HistoryPool::new(PoolLimits::default()),
        )
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.identity().unwrap(),
        checkpoint.identity,
        "CHECKPOINT_OPEN_KEEPS_THE_COMBINED_OWNER_IDENTITY"
    );
    assert_eq!(opened.history().position(), checkpoint.position);
    assert_eq!(
        opened.history().document().tracks.len(),
        1,
        "CHECKPOINT_OPEN_KEEPS_THE_SAVED_DOCUMENT"
    );
    assert!(opened.history().undo().is_empty());
    assert_eq!(
        opened.history().redo().len(),
        1,
        "CHECKPOINT_OPEN_KEEPS_THE_REDO_STEP"
    );
    assert_eq!(
        opened.approval_answer(&fixture.approval).unwrap(),
        Some(ApprovalAnswer::Missing),
        "CHECKPOINT_OPEN_KEEPS_THE_APPROVED_COMMAND_FATE"
    );
    assert_eq!(
        checkpoint.stream.as_ref().unwrap().sequence(),
        3,
        "CHECKPOINT_SAVES_CLOSE_WITH_THE_COMMAND_STREAM"
    );
    assert_eq!(
        checkpoint.revision,
        TimelineRevision(2),
        "SAVED_REVISION_IS_INDEPENDENT_OF_ACCEPTED_POSITION"
    );
    let closure = fixture
        .store
        .store()
        .closure(std::slice::from_ref(&fixture.object.reference))
        .unwrap();
    assert!(
        closure.contains(&fixture.approval),
        "CHECKPOINT_GRAPH_RETAINS_ITS_LEDGER_EVIDENCE"
    );
}
#[cfg(target_os = "linux")]
#[test]
fn checkpoint_projection_refusals_preserve_the_pool_and_are_retryable_for_room() {
    let fixture = stored_fixture();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut wrong = fixture.checkpoint.clone();
    wrong.identity.owner[0] ^= 1;
    assert!(
        matches!(
            wrong.open(fixture.store.store(), &pool),
            Err(crate::operation_history::Error::IdentityMismatch)
        ),
        "CHECKPOINT_OPEN_USES_THE_COMBINED_OWNER_IDENTITY"
    );
    wrong = fixture.checkpoint.clone();
    wrong.document_ref.sha256 = "f".repeat(64);
    assert!(
        matches!(
            wrong.open(fixture.store.store(), &pool),
            Err(crate::operation_history::Error::IdentityMismatch)
        ),
        "CHECKPOINT_PROJECTION_NAMES_THE_OWNERS_EXACT_DOCUMENT"
    );
    assert_eq!(
        pool.usage(Role::Ordinary).unwrap(),
        (0, 0),
        "REFUSED_CHECKPOINT_OPEN_RELEASES_ALL_HISTORY_ROOM"
    );
    let mut limits = PoolLimits::default();
    limits.retained[0] = 0;
    let full = HistoryPool::new(limits);
    assert!(
        matches!(
            fixture.checkpoint.open(fixture.store.store(), &full),
            Err(crate::operation_history::Error::PoolFull(Role::Ordinary))
        ),
        "CHECKPOINT_POOL_FULL_IS_A_TYPED_RETRYABLE_REFUSAL"
    );
    fixture
        .checkpoint
        .open(fixture.store.store(), &pool)
        .unwrap();
}
