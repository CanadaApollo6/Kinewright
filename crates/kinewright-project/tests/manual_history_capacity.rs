//! Person-visible acceptance uses the same history admission as Save and cold open.
#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::*;
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle},
    editing_session::Ja1EditingSession,
    storage_budget::StorageLimits,
};
use std::collections::BTreeMap;

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
fn revision(core: &Core) -> TimelineRevision {
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot query")
    };
    revision
}
#[test]
fn manual_history_capacity_refuses_heavy_batch_before_ack_and_retries_smaller_edit_exactly() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-pre-edit-capacity");
    let path = temp.path("source.kinewright");
    let host = Core::spawn(heavy_document(20)).unwrap();
    let mut owner =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &host, None)
            .unwrap();
    owner.save_active().unwrap();
    let lease = owner
        .issue_manual_lease("capacity-person".into(), "capacity-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let before = gui.export_history().unwrap();
    let encoded = serde_json::to_vec(&before).unwrap();
    let before_revision = revision(&gui);
    let before_change = gui.history_change_id().unwrap();
    let root = std::fs::read(&path).unwrap();
    let event = permit.request(copies(20)).unwrap();
    assert!(
        matches!(
            event,
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualHistoryCapacity,
                ..
            }
        ),
        "MANUAL_REFUSAL_BEFORE_ACK"
    );
    assert_eq!(gui.export_history().unwrap(), before);
    assert_eq!(
        serde_json::to_vec(&gui.export_history().unwrap()).unwrap(),
        encoded
    );
    assert_eq!(revision(&gui), before_revision);
    assert_eq!(gui.history_change_id().unwrap(), before_change);
    assert!(permit.pending().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), root);
    let next = permit
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    assert!(matches!(next, Event::DocumentChanged { .. }));
    let pending = permit.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].sequence, 1, "REFUSAL_DOES_NOT_ADVANCE_ENCODER");
    owner.commit_manual_history(&lease, &pending[0]).unwrap();
    let expected = gui.export_history().unwrap();
    assert_eq!(host.export_history().unwrap(), expected);
    drop(owner);
    drop(replica);
    drop(permit);
    let loaded = LoadedBundle::read(&path).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&reopened).unwrap();
    assert_eq!(reopened.export_history().unwrap(), expected);
}
#[test]
fn manual_history_capacity_ten_clip_control_publishes_and_cold_reopens_full_undo() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-heavy-control");
    let path = temp.path("source.kinewright");
    let host = Core::spawn(heavy_document(10)).unwrap();
    let mut owner =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &host, None)
            .unwrap();
    owner.save_active().unwrap();
    let lease = owner
        .issue_manual_lease("capacity-person".into(), "capacity-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let event = permit.request(copies(10)).unwrap();
    assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    let expected = gui.export_history().unwrap();
    let pending = permit.pending().unwrap();
    owner.commit_manual_history(&lease, &pending[0]).unwrap();
    assert_eq!(host.export_history().unwrap(), expected);
    drop(owner);
    drop(replica);
    drop(permit);
    let loaded = LoadedBundle::read(&path).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&reopened).unwrap();
    assert_eq!(reopened.export_history().unwrap(), expected);
    assert!(matches!(
        reopened.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert_eq!(
        reopened.export_history().unwrap().document,
        expected.baseline
    );
    assert!(matches!(
        reopened.request(Command::Redo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert_eq!(
        reopened.export_history().unwrap().document,
        expected.document
    );
}

#[test]
#[ignore = "on-demand: heavy-effect ordinary copy peak and largest accepted batch"]
fn manual_history_heavy_copy_capacity_census() {
    use kinewright_project::manual_history::OrdinaryHistoryLimits;
    for total in [10_u64, 20] {
        let mut largest = 0;
        for targets in 1..total {
            let core = Core::spawn(heavy_document(i64::try_from(total).unwrap())).unwrap();
            assert!(matches!(
                core.request(copies(targets + 1)).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let history = core.export_history().unwrap();
            let mut one = (*history.baseline).clone();
            one.tracks[0].clips.truncate(1);
            let mut empty = one.clone();
            empty.tracks[0].clips.clear();
            let empty_charge =
                kinewright_core::history_size::replay_document_bytes(&empty).unwrap();
            let source_charge =
                kinewright_core::history_size::replay_document_bytes(&one).unwrap() - empty_charge;
            one.tracks[0].clips[0] = history.baseline.tracks[0].clips[1].clone();
            let destination_charge =
                kinewright_core::history_size::replay_document_bytes(&one).unwrap() - empty_charge;
            let operation_copy_allowance = 3 * source_charge + 2 * destination_charge;
            let journal = HistoryJournal::capture(&history).unwrap();
            let peak = kinewright_core::history_size::measure_replay_peak(
                &journal,
                &history.baseline,
                &history.document,
            )
            .unwrap();
            let retained =
                kinewright_core::history_size::restored_history_live_bytes(&history).unwrap();
            let admission = OrdinaryHistoryLimits::default().check(&history);
            if admission.is_ok() {
                largest = targets;
            }
            println!(
                "HEAVY_COPY_CAPACITY={}",
                serde_json::json!({"total_clips":total,"targets":targets,"operation_copy_allowance":operation_copy_allowance,"operation_allowance_sum_if_held":operation_copy_allowance*targets,"encoded":history.encoded_bytes().unwrap(),"retained":retained,"peak":peak,"ordinary_limit":32_u64<<20,"admission":format!("{admission:?}")})
            );
        }
        println!("HEAVY_COPY_LARGEST={total}:{largest}");
    }
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
    let core = Core::spawn(Document::default()).unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 1,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits).unwrap();
    let before = core.export_history().unwrap();
    let inputs = owner.current_inputs_ref().clone();
    let result = owner.import_owned_media(&source);
    assert!(
        matches!(
            result,
            Err(kinewright_project::bundle::BundleError::Capacity(_))
        ),
        "import must run the ordinary history predicate"
    );
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(
        owner.current_inputs_ref(),
        &inputs,
        "REFUSED_IMPORT_DOES_NOT_INSTALL_INPUT_MANIFEST"
    );
}

#[test]
fn manual_history_capacity_gui_lease_keeps_the_writers_lower_effective_bound() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-lower-policy");
    let host = Core::spawn(heavy_document(10)).unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 10 << 20,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner
        .first_save(&temp.path("lower.kinewright"), limits.clone())
        .unwrap();
    let lease = owner
        .issue_manual_lease("lower-person".into(), "lower-local".into())
        .unwrap();
    assert_eq!(
        lease.history_limits,
        kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits).unwrap()
    );
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease, owner.manual_input_context().unwrap())
        .unwrap();
    let before = gui.export_history().unwrap();
    assert!(matches!(
        permit.request(copies(10)).unwrap(),
        Event::MutationRefused {
            reason: CoreMutationRefusal::ManualHistoryCapacity,
            ..
        }
    ));
    assert_eq!(gui.export_history().unwrap(), before);
    assert!(permit.pending().unwrap().is_empty());
}

#[test]
fn manual_history_capacity_encoder_limit_rolls_back_the_staged_replay_proof() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-encoder-capacity");
    let host = Core::spawn(heavy_document(2)).unwrap();
    let limits = StorageLimits {
        command_bytes: 16 << 10,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner
        .first_save(&temp.path("source.kinewright"), limits.clone())
        .unwrap();
    let lease = owner
        .issue_manual_lease("encoder-person".into(), "encoder-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let before = gui.export_history().unwrap();
    let mut effect = before.document.tracks[0].clips[0].effects[0].clone();
    effect.id = EffectId(2);
    let operation = Operation::AddEffect {
        clip: ClipId(2),
        effect,
    };
    let shadow = Core::spawn(std::sync::Arc::clone(&before.baseline)).unwrap();
    shadow.restore_history(before.clone()).unwrap();
    assert!(matches!(
        shadow.request(Command::Do(operation.clone())).unwrap(),
        Event::DocumentChanged { .. }
    ));
    kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits)
        .unwrap()
        .check(&shadow.export_history().unwrap())
        .unwrap();
    assert!(matches!(
        permit.request(Command::Do(operation)).unwrap(),
        Event::MutationRefused {
            reason: CoreMutationRefusal::ManualHistoryCapacity,
            ..
        }
    ));
    assert_eq!(gui.export_history().unwrap(), before);
    assert!(permit.pending().unwrap().is_empty());
    assert!(matches!(
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let pending = permit.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].sequence, 1);
    owner.commit_manual_history(&lease, &pending[0]).unwrap();
    assert_eq!(
        host.export_history().unwrap(),
        gui.export_history().unwrap()
    );
}
