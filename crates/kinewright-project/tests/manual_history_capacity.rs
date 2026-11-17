//! The executor and the accepted-message arena refuse before a person sees acceptance.
#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::*;
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::operation_actor::{Core, ManualPermit};
use kinewright_project::{
    editing_session::Ja1EditingSession,
    operation_history::HistoryPool,
    operation_protocol::{Lease, PENDING_COMMANDS},
    storage_budget::StorageLimits,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
fn heavy_document(clips: i64) -> Document {
    let mut document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    };
    for clip in 0..clips {
        Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(clip * 90),
            duration: TimeCode(90),
            color: SolidColor { r: 0, g: 0, b: 0 },
        }
        .apply(&mut document)
        .unwrap();
    }
    Operation::AddEffect {
        clip: ClipId(1),
        effect: Effect {
            id: EffectId(1),
            name: "primary_correction".into(),
            enabled: true,
            enabled_curve: None,
            parameters: BTreeMap::from([("exposure_milli_stops".into(), ParamValue::Integer(0))]),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".into(),
                AutomationCurve {
                    keyframes: (0..1000)
                        .map(|at| Keyframe {
                            at: TimeCode(at),
                            value: 0,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        })
                        .collect(),
                },
            )]),
        },
    }
    .apply(&mut document)
    .unwrap();
    document
}
fn copies(clips: u64) -> Command {
    Command::DoBatch(
        (2..=clips)
            .map(|to_clip| Operation::CopyClipAttributes {
                from_clip: ClipId(1),
                to_clip: ClipId(to_clip),
                names: None,
                include_keyframes: true,
            })
            .collect(),
    )
}

fn fixture(
    clips: i64,
) -> (
    TempDirectory,
    Ja1EditingSession,
    Ja1EditingSession,
    Lease,
    ManualPermit,
) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-history-owner");
    let core = Core::for_test(heavy_document(clips)).unwrap();
    let mut writer = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    writer
        .first_save(&temp.path("project.kinewright"), StorageLimits::default())
        .unwrap();
    let lease = writer
        .issue_manual_lease("person".repeat(12), "local".repeat(12))
        .unwrap();
    let wire = core.portable_history().unwrap();
    let branch = wire.branch.clone();
    let revision = wire.revision;
    let pool = HistoryPool::new(kinewright_project::operation_history::PoolLimits::default());
    let gui = Core::open(wire.validate(&pool).unwrap(), branch, revision).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(
            lease.clone(),
            writer.manual_input_context().unwrap(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    (temp, writer, replica, lease, permit)
}
fn publish(
    writer: &mut Ja1EditingSession,
    replica: &Ja1EditingSession,
    lease: &Lease,
    permit: &ManualPermit,
) {
    let authority = writer.manual_authority(lease.clone()).unwrap();
    let mut receipts = vec![];
    for message in permit.pending().unwrap() {
        let (receipt, report) = writer.commit_manual_history(&authority, &message).unwrap();
        assert!(report.is_some(), "PERSON_ACK_FOLLOWS_ROOT_PUBLICATION");
        receipts.push(receipt);
    }
    replica.acknowledge_manual(permit, receipts).unwrap();
    assert!(
        !writer.dirty().unwrap(),
        "PUBLISHED_STREAM_ADVANCES_THE_WRITERS_SAVED_POSITION"
    );
    assert_eq!(
        writer.core().snapshot_identity().unwrap(),
        replica.core().snapshot_identity().unwrap(),
        "DRAIN_EQUALS_THE_FULL_OWNER_IDENTITY"
    );
}
#[test]
fn manual_history_capacity_refuses_heavy_batch_before_ack_and_retries_smaller_edit_exactly() {
    let (temp, mut writer, replica, lease, permit) = fixture(20);
    permit
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    permit.request(Command::Undo).unwrap();
    publish(&mut writer, &replica, &lease, &permit);
    let before = replica.core().snapshot_identity().unwrap();
    let change = replica.core().change_id().unwrap();
    let root = std::fs::read(temp.path("project.kinewright")).unwrap();
    let oversized = Operation::AddMarker {
        marker: Marker {
            id: MarkerId(999),
            position: TimeCode(0),
            label: "x".repeat(1 << 20),
            color_token: 0,
        },
    };
    let refusal = permit
        .request(Command::DoBatch(vec![oversized]))
        .unwrap_err();
    assert_eq!(
        refusal.error,
        kinewright_project::operation_actor::Error::MutationRefused(
            CoreMutationRefusal::ManualHistoryCapacity
        ),
        "MANUAL_REFUSAL_BEFORE_ACK"
    );
    assert_eq!(
        refusal.error.to_string(),
        CoreMutationRefusal::ManualHistoryCapacity.to_string(),
        "MANUAL_CAPACITY_REFUSAL_HAS_ITS_TRUTHFUL_PERSON_MESSAGE"
    );
    assert_eq!(
        replica.core().snapshot_identity().unwrap(),
        before,
        "REFUSED_STEP_PRESERVES_REDO_LINEAGE_LEDGER_AND_RESOURCES"
    );
    assert_eq!(replica.core().change_id().unwrap(), change);
    assert!(permit.pending().unwrap().is_empty());
    assert_eq!(
        std::fs::read(temp.path("project.kinewright")).unwrap(),
        root
    );
    permit.request(Command::Redo).unwrap();
    assert_eq!(
        permit.pending().unwrap()[0].decode().unwrap().sequence,
        3,
        "REFUSAL_DOES_NOT_ADVANCE_ENCODER"
    );
    publish(&mut writer, &replica, &lease, &permit);
    let expected = replica.core().snapshot_identity().unwrap();
    drop(writer);
    let reopened = Ja1EditingSession::open(
        &temp.path("project.kinewright"),
        &HistoryPool::new(kinewright_project::operation_history::PoolLimits::default()),
    )
    .unwrap();
    assert_eq!(
        reopened.core().snapshot_identity().unwrap(),
        expected,
        "MANUAL_ACCEPTED_STATE_COLD_OPENS_EXACTLY"
    );
}
#[test]
fn manual_history_capacity_ten_clip_control_publishes_and_cold_reopens_full_undo() {
    let (temp, mut writer, replica, lease, permit) = fixture(10);
    permit.request(copies(10)).unwrap();
    publish(&mut writer, &replica, &lease, &permit);
    let expected = replica.core().snapshot_identity().unwrap();
    drop(writer);
    let reopened = Ja1EditingSession::open(
        &temp.path("project.kinewright"),
        &HistoryPool::new(kinewright_project::operation_history::PoolLimits::default()),
    )
    .unwrap();
    assert_eq!(reopened.core().snapshot_identity().unwrap(), expected);
    reopened.core().request(Command::Undo).unwrap();
    let undone = reopened.core().export_history().unwrap();
    assert!(undone.history().undo.is_empty());
    assert_eq!(undone.history().redo.len(), 1);
    drop(undone);
    reopened.core().request(Command::Redo).unwrap();
    assert_eq!(
        reopened
            .core()
            .export_history()
            .unwrap()
            .history()
            .document
            .tracks[0]
            .clips[1]
            .effects
            .len(),
        1
    );
}
#[test]
fn manual_history_capacity_encoder_limit_rolls_back_the_staged_replay_proof() {
    let (_temp, mut writer, replica, lease, permit) = fixture(10);
    for index in 0..PENDING_COMMANDS {
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index % 2 == 0,
            }))
            .unwrap();
    }
    let before = replica.core().snapshot_identity().unwrap();
    let change = replica.core().change_id().unwrap();
    assert!(
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .is_err(),
        "FULL_ENCODER_REFUSES_BEFORE_ACCEPTANCE"
    );
    assert_eq!(replica.core().snapshot_identity().unwrap(), before);
    assert_eq!(replica.core().change_id().unwrap(), change);
    publish(&mut writer, &replica, &lease, &permit);
    permit.request(Command::Undo).unwrap();
    assert_eq!(
        permit.pending().unwrap()[0].decode().unwrap().sequence,
        65,
        "DRAIN_REOPENS_THE_ORIGINAL_FIXED_ENCODER"
    );
    publish(&mut writer, &replica, &lease, &permit);
}

#[test]
fn manual_history_capacity_unsaved_import_refuses_before_installing_document_or_inputs() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "import-pre-edit-capacity");
    let source = temp.path("source.wav");
    std::fs::write(
        &source,
        kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
    )
    .unwrap();
    let core = Core::for_test(Document::default()).unwrap();
    let limits = StorageLimits {
        owned_bytes: 1,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits).unwrap();
    let before = core.snapshot_identity().unwrap();
    let inputs = owner.current_inputs_ref().clone();
    assert!(
        owner.import_owned_media(&source).is_err(),
        "OWNED_BYTE_REFUSAL_PRECEDES_DOCUMENT_ACCEPTANCE"
    );
    assert_eq!(core.snapshot_identity().unwrap(), before);
    assert_eq!(
        owner.current_inputs_ref(),
        &inputs,
        "REFUSED_IMPORT_DOES_NOT_INSTALL_INPUT_MANIFEST"
    );
}

#[test]
fn manual_history_capacity_gui_lease_cannot_override_the_installed_counter_or_profile() {
    let (temp, writer, replica, lease, _permit) = fixture(10);
    let writer_profile = writer.core().export_history().unwrap().creation_sizing();
    let gui_profile = replica.core().export_history().unwrap().creation_sizing();
    assert_eq!(
        writer_profile, gui_profile,
        "BOTH_PEERS_USE_THE_SAME_FIXED_CREATION_PROFILE"
    );
    let peer = Core::peer_for_test(writer.core().portable_history().unwrap()).unwrap();
    let receiver = Ja1EditingSession::new(temp.root(), &peer, StorageLimits::default()).unwrap();
    let before = peer.change_id().unwrap();
    let mut forged = lease;
    forged.next_key.position += 1;
    assert!(
        receiver
            .bind_manual_producer(
                forged,
                writer.manual_input_context().unwrap(),
                Arc::new(AtomicBool::new(true))
            )
            .is_err(),
        "LEASE_CANNOT_OVERRIDE_THE_INSTALLED_COMMAND_COUNTER"
    );
    assert_eq!(peer.change_id().unwrap(), before);
}
