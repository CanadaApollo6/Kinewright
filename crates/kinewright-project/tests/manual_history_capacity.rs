//! Person-visible acceptance uses the same history admission as Save and cold open.
#![allow(
    clippy::too_many_lines,
    reason = "store witnesses keep acceptance, publication and cold equality together"
)]
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
    heavy_document_with_keys(clips, 1000)
}
fn heavy_document_with_keys(clips: i64, keys: i64) -> Document {
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
                    keyframes: (0..keys)
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

fn cold_manual_cursor_roundtrip(
    root: &std::path::Path,
    path: &std::path::Path,
    limits: StorageLimits,
    expected: &HistorySnapshot,
) {
    let loaded = LoadedBundle::read(path).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let host = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&host).unwrap();
    assert_eq!(host.export_history().unwrap(), *expected);
    let bundle = loaded
        .into_writer_with_limits_probe(limits.clone())
        .unwrap();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &host, inputs).unwrap();
    let lease = owner
        .issue_manual_lease("cold-cursor-person".into(), "cold-cursor-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(root, &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let mut undos = 0;
    let mut refused = false;
    while !gui.export_history().unwrap().cursor.undo.is_empty() {
        let before = gui.export_history().unwrap();
        let bytes = serde_json::to_vec(&before).unwrap();
        let rev = revision(&gui);
        let root_bytes = std::fs::read(path).unwrap();
        let event = permit.request(Command::Undo).unwrap();
        if let Event::MutationRefused {
            reason: CoreMutationRefusal::ManualUndoCapacity,
            ..
        } = event
        {
            assert_eq!(
                CoreMutationRefusal::ManualUndoCapacity.to_string(),
                "Can't undo further. This project's undo history is full."
            );
            assert_eq!(
                serde_json::to_vec(&gui.export_history().unwrap()).unwrap(),
                bytes
            );
            assert_eq!(revision(&gui), rev);
            assert_eq!(std::fs::read(path).unwrap(), root_bytes);
            assert!(permit.pending().unwrap().is_empty());
            refused = true;
            break;
        }
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "UNDO_MAY_ONLY_REFUSE_WITH_ITS_TYPED_CAPACITY: {event:?}"
        );
        let messages = permit.pending().unwrap();
        owner.commit_manual_history(&lease, &messages[0]).unwrap();
        replica.acknowledge_manual(&permit, messages).unwrap();
        undos += 1;
    }
    let middle = gui.export_history().unwrap();
    assert_eq!(host.export_history().unwrap(), middle);
    drop(owner);
    drop(replica);
    drop(permit);
    // Reopen the Redo cut through the real cold resolver, then bind a fresh
    // guarded producer. Runtime reservation caches cannot stand in for disk.
    let loaded = LoadedBundle::read(path).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let host = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&host).unwrap();
    assert_eq!(host.export_history().unwrap(), middle);
    let bundle = loaded.into_writer_with_limits_probe(limits).unwrap();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &host, inputs).unwrap();
    let lease = owner
        .issue_manual_lease("cold-redo-person".into(), "cold-redo-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(middle).unwrap();
    let replica = Ja1EditingSession::new(root, &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let mut redos = 0;
    while !gui.export_history().unwrap().cursor.redo.is_empty() {
        let event = permit.request(Command::Redo).unwrap();
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "ADMITTED_UNDO_MUST_NEVER_REFUSE_REDO: {event:?}"
        );
        let messages = permit.pending().unwrap();
        owner.commit_manual_history(&lease, &messages[0]).unwrap();
        replica.acknowledge_manual(&permit, messages).unwrap();
        redos += 1;
    }
    let after = gui.export_history().unwrap();
    assert_eq!(after.document, expected.document);
    let discard = usize::try_from(after.floor_offset - expected.floor_offset).unwrap();
    let kept = if discard == 0 {
        expected.clone()
    } else {
        HistoryTransition::floor_after(expected, discard).unwrap()
    };
    assert_eq!(
        after.cursor, kept.cursor,
        "CURSOR_RETURNS_TO_THE_COMPLETE_KEPT_HISTORY"
    );
    assert_eq!(
        &after.batches[..kept.batches.len()],
        kept.batches,
        "ACCEPTED_NAVIGATION_APPENDS_AFTER_THE_KEPT_JOURNAL"
    );
    assert_eq!(redos, undos);
    println!(
        "COLD_GUARDED_CURSOR undos={undos} redos={redos} undo_refused={refused} original_depth={} kept_depth={} floor={}",
        expected.cursor.undo.len(),
        kept.cursor.undo.len(),
        after.floor_offset
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_full_undo_keeps_editing_through_repeated_floors() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-continuous-floor");
    let path = temp.path("continuous.kinewright");
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    };
    let host = Core::spawn(document).unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 512 << 10,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let lease = owner
        .issue_manual_lease("continuous-person".into(), "continuous-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let mut floors = 0;
    let mut offset = 0;
    let mut maximum_depth = 0;
    let mut last_floor = None;
    let mut low_water = None;
    for index in 0_usize..180 {
        let event = permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index.is_multiple_of(2),
            }))
            .unwrap();
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "FULL_HISTORY_MUST_FLOOR_BEFORE_ACK index={index}: {event:?}"
        );
        let pending = permit.pending().unwrap();
        owner.commit_manual_history(&lease, &pending[0]).unwrap();
        replica.acknowledge_manual(&permit, pending).unwrap();
        let history = gui.export_history().unwrap();
        assert_eq!(
            host.export_history().unwrap(),
            history,
            "DOOR_SAVE_AGREEMENT"
        );
        maximum_depth = maximum_depth.max(history.cursor.undo.len());
        if history.floor_offset != offset {
            floors += 1;
            offset = history.floor_offset;
            let minimum = *low_water.get_or_insert(history.cursor.undo.len());
            assert!(
                history.cursor.undo.len() >= minimum,
                "LOW_WATER_UNDO_IS_KEPT"
            );
            if let Some(previous) = last_floor {
                assert!(
                    index - previous >= maximum_depth / 8,
                    "LOW_WATER_LEAVES_MANY_EDITS_BETWEEN_FLOORS"
                );
            }
            last_floor = Some(index);
        }
        if let Some(minimum) = low_water {
            assert!(
                history.cursor.undo.len() >= minimum,
                "UNDO_NEVER_DROPS_BELOW_LOW_WATER"
            );
        }
    }
    assert!(floors >= 3, "CONTINUOUS_EDITING_MUST_CROSS_MULTIPLE_FLOORS");
    assert!(180 >= 3 * maximum_depth, "THREE_TIMES_RETAINED_CAPACITY");
    let expected = gui.export_history().unwrap();
    drop(owner);
    drop(replica);
    drop(permit);
    cold_manual_cursor_roundtrip(temp.root(), &path, limits, &expected);
    println!("ORDINARY_CONTINUOUS_FLOOR edits=180 floors={floors} maximum_depth={maximum_depth}");
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
    assert_eq!(
        CoreMutationRefusal::ManualHistoryCapacity.to_string(),
        "This edit is too large for one step. Try it in smaller parts."
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

#[test]
fn ordinary_floor_source_saves_unsaved_tail_after_the_final_save_instruction() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "ordinary-floor-source");
    let path = temp.path("pending.kinewright");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 512 << 10,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    let mut accepted = 0;
    let mut retained_source_seen = false;
    for index in 0_usize..80 {
        let command = Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index.is_multiple_of(2),
        });
        let before = core.export_history_for_publication().unwrap();
        let event = admission.request(&core, command.clone()).unwrap();
        if matches!(
            event,
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualSaveRequired,
                ..
            }
        ) {
            assert!(
                before.ordinary_floor_source.is_some(),
                "ONLY_THE_UNPUBLISHED_SOURCE_REQUIRES_A_SAVE"
            );
            assert_eq!(core.export_history_for_publication().unwrap(), before);
            owner.save_active().unwrap();
            assert!(
                core.export_history_for_publication()
                    .unwrap()
                    .ordinary_floor_source
                    .is_none()
            );
            assert!(
                matches!(
                    admission.request(&core, command).unwrap(),
                    Event::DocumentChanged { .. }
                ),
                "SAVE_RELEASES_THE_IDENTICAL_RETRY"
            );
        } else {
            assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
        }
        accepted += 1;
        let current = core.export_history_for_publication().unwrap();
        if current.ordinary_floor_source.is_some() {
            retained_source_seen = true;
            // The next Save must include edits accepted after the floor, too.
            if index.is_multiple_of(6) {
                let expected = current;
                owner.save_active().unwrap();
                assert_eq!(core.export_history_for_publication().unwrap(), expected);
                assert!(
                    core.export_history_for_publication()
                        .unwrap()
                        .ordinary_floor_source
                        .is_none()
                );
            }
        }
    }
    assert_eq!(accepted, 80);
    assert!(retained_source_seen);
    let expected = core.export_history_for_publication().unwrap();
    owner.save_active().unwrap();
    drop(owner);
    let loaded = LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history_for_publication().unwrap(), expected);
}

#[test]
fn manual_history_floors_after_undo_when_the_new_edit_clears_redo() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-redo-floor");
    let path = temp.path("redo.kinewright");
    let host = Core::spawn(heavy_document_with_keys(2, 64)).unwrap();
    for index in 0..24 {
        assert!(matches!(
            host.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index % 2 == 0
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    assert!(matches!(
        host.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let prefix = host.export_history().unwrap();
    let mut low = 1;
    let mut high = StorageLimits::default().branch_history_bytes;
    while low < high {
        let middle = low + (high - low) / 2;
        let limits = StorageLimits {
            branch_history_bytes: middle,
            ..StorageLimits::default()
        };
        let policy =
            kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits)
                .unwrap();
        if policy.check(&prefix).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let cap = low;
    let limits = StorageLimits {
        branch_history_bytes: cap,
        ..StorageLimits::default()
    };
    let policy =
        kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits).unwrap();
    policy.check(&prefix).unwrap();
    let alone = Core::spawn(std::sync::Arc::clone(&prefix.document)).unwrap();
    alone
        .request(Command::DoBatch(vec![
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            },
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            },
        ]))
        .unwrap();
    policy.check(&alone.export_history().unwrap()).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits).unwrap();
    let lease = owner
        .issue_manual_lease("redo-person".into(), "redo-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let event = permit
        .request(Command::DoBatch(vec![
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            },
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            },
        ]))
        .unwrap();
    assert!(
        matches!(event, Event::DocumentChanged { .. }),
        "ONE_STEP_FITS_AFTER_CLEARING_REDO: {event:?}"
    );
    let expected = gui.export_history().unwrap();
    assert!(expected.floor_offset > 0, "DEAD_REDO_PREFIX_CAN_BE_FLOORED");
    assert!(!expected.cursor.undo.is_empty());
    assert_eq!(
        expected.cursor.undo.last().unwrap().document,
        prefix.document
    );
    assert!(expected.cursor.redo.is_empty());
    let messages = permit.pending().unwrap();
    owner.commit_manual_history(&lease, &messages[0]).unwrap();
    replica.acknowledge_manual(&permit, messages).unwrap();
    drop(owner);
    drop(replica);
    drop(permit);
    let loaded = LoadedBundle::read(&path).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&reopened).unwrap();
    assert_eq!(reopened.export_history().unwrap(), expected);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_realistic_400_clip_edits_keep_continuing_and_reopen_exactly() {
    document_continuity(400, true);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_plain_300_clip_edits_keep_continuing_and_reopen_exactly() {
    document_continuity(300, false);
}

fn document_continuity(clips: i64, effects: bool) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-realistic-floor");
    let path = temp.path("realistic.kinewright");
    let mut document = heavy_document_with_keys(clips, 8);
    if !effects {
        document.tracks[0].clips[0].effects.clear();
    }
    for clip in 2..=u64::try_from(clips).unwrap() {
        if !effects {
            break;
        }
        Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(clip),
            names: None,
            include_keyframes: true,
        }
        .apply(&mut document)
        .unwrap();
    }
    document.validate().unwrap();
    let host = Core::spawn(document).unwrap();
    let mut owner =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &host, None)
            .unwrap();
    owner.save_active().unwrap();
    let lease = owner
        .issue_manual_lease("realistic-person".into(), "realistic-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let mut first_floor = None;
    let mut previous_floor = 0;
    let mut floor_count = 0;
    let mut maximum = 0_usize;
    let mut low_water = 0;
    let mut accepted = 0;
    for index in 0_usize..1200 {
        let event = permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index.is_multiple_of(2),
            }))
            .unwrap();
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "REALISTIC_FULL_HISTORY_NEVER_REFUSES_A_SMALL_STEP {index}: {event:?}"
        );
        let messages = permit.pending().unwrap();
        owner.commit_manual_history(&lease, &messages[0]).unwrap();
        replica.acknowledge_manual(&permit, messages).unwrap();
        accepted += 1;
        let history = gui.export_history().unwrap();
        assert_eq!(host.export_history().unwrap(), history);
        maximum = maximum.max(history.cursor.undo.len());
        if history.floor_offset != previous_floor {
            previous_floor = history.floor_offset;
            floor_count += 1;
            if first_floor.is_none() {
                first_floor = Some(accepted);
                low_water = history.cursor.undo.len();
            }
        }
        if first_floor.is_some() {
            assert!(
                history.cursor.undo.len() >= low_water,
                "REALISTIC_LOW_WATER_RETAINED"
            );
        }
        if first_floor.is_some_and(|first| accepted >= first * 3) {
            break;
        }
    }
    let first = first_floor.expect("realistic witness reaches the ordinary bound");
    assert!(accepted >= 3 * maximum);
    assert!(floor_count >= 3);
    let expected = gui.export_history().unwrap();
    drop(owner);
    drop(replica);
    drop(permit);
    cold_manual_cursor_roundtrip(temp.root(), &path, StorageLimits::default(), &expected);
    println!(
        "REALISTIC_ORDINARY_CONTINUITY={}",
        serde_json::json!({"clips":clips,"effects":if effects {clips} else {0},"keyframes":if effects {clips*8} else {0},"first_floor_edit":first,"accepted":accepted,"refused":0,"maximum_undo":maximum,"low_water_undo":low_water,"floors":floor_count,"final_undo":expected.cursor.undo.len()})
    );
}

fn replay_peak(history: &HistorySnapshot) -> u64 {
    kinewright_core::history_size::measure_replay_peak(
        &HistoryJournal::capture(history).unwrap(),
        &history.baseline,
        &history.document,
    )
    .unwrap()
}

fn exhaustive_ordinary_floor(
    history: &HistorySnapshot,
    limits: &StorageLimits,
) -> Option<HistorySnapshot> {
    let policy =
        kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(limits).unwrap();
    if policy.check(history).is_ok() {
        return Some(history.clone());
    }
    let fitting: Vec<_> = (1..history.batches.len())
        .filter_map(|cut| {
            HistoryTransition::floor_after(history, cut)
                .ok()
                .filter(|h| policy.check(h).is_ok())
        })
        .collect();
    let first = fitting.first()?;
    let depth = first
        .cursor
        .undo
        .len()
        .saturating_sub(first.cursor.undo.len().div_ceil(8))
        .max(1);
    Some(
        fitting
            .iter()
            .find(|h| h.cursor.undo.len() <= depth)
            .unwrap_or(first)
            .clone(),
    )
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_payload_cuts_agree_with_save_and_cold_open() {
    use kinewright_project::{
        bundle_commit::history_cost_probe, manual_history::OrdinaryHistoryLimits,
    };
    const SEED: u64 = 0x4a41_4658_3350_4159;
    let mut rng = SEED;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut accepted = 0;
    let mut refused = 0;
    let mut floors = 0;
    let mut setup_capacity = vec![];
    let mut minimum_setup_limits = BTreeMap::new();
    let mut setup_disagreements = vec![];
    for case in 0..60 {
        let fanout = [2, 4, 8, 16][(next() % 4) as usize];
        let builder = Core::spawn(heavy_document_with_keys(
            16,
            4 + i64::try_from(next() % 29).unwrap(),
        ))
        .unwrap();
        let command = match case % 3 {
            0 => copies(fanout),
            1 => {
                let splits = 2 + next() % 6;
                for split in 0..splits {
                    assert!(matches!(
                        builder
                            .request(Command::Do(Operation::SplitClip {
                                clip: ClipId(1),
                                at: TimeCode(89 - i64::try_from(split).unwrap()),
                            }))
                            .unwrap(),
                        Event::DocumentChanged { .. }
                    ));
                }
                Command::DoBatch(
                    (17..17 + splits)
                        .map(|id| Operation::DeleteClip { clip: ClipId(id) })
                        .collect(),
                )
            }
            _ => {
                let Command::DoBatch(mut operations) = copies(fanout) else {
                    unreachable!()
                };
                operations.extend((0..2).map(|n| Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: n == 0,
                }));
                Command::DoBatch(operations)
            }
        };
        assert!(matches!(
            builder.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
        for edit in 0..2 + next() % 7 {
            assert!(matches!(
                builder
                    .request(Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: edit % 2 == 0,
                    }))
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        let prefix = builder.export_history().unwrap();
        builder
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }))
            .unwrap();
        let original = builder.export_history().unwrap();
        let target =
            1 + usize::try_from(next() % u64::try_from(original.batches.len() - 1).unwrap())
                .unwrap();
        let candidate = HistoryTransition::floor_after(&original, target).unwrap();
        let charge = history_cost_probe(&candidate, &StorageLimits::default()).unwrap();
        for cap in [charge - 1, charge, charge + 1] {
            let limits = StorageLimits {
                branch_history_bytes: cap,
                ..StorageLimits::default()
            };
            let policy = OrdinaryHistoryLimits::from_limits(&limits).unwrap();
            assert_eq!(
                policy.select(&original).ok(),
                exhaustive_ordinary_floor(&original, &limits),
                "PAYLOAD_FLOOR_EXHAUSTIVE case={case} cap={cap}"
            );
            let temp = TempDirectory::new_in(&workspace_target_dir(), "payload-floor-agreement");
            let path = temp.path("payload.kinewright");
            let host = Core::spawn(Document::default()).unwrap();
            host.restore_history(prefix.clone()).unwrap();
            let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
            // Prefix publication itself uses the same selector; start the real
            // lease at the cut actually accepted by this store.
            if let Err(error) = owner.first_save(&path, limits) {
                let fitting = policy.select(&prefix).is_ok();
                match error {
                    kinewright_project::bundle::BundleError::Capacity(reason) => {
                        let minimum = *minimum_setup_limits.entry(case).or_insert_with(|| {
                            let mut low = cap + 1;
                            let mut high = StorageLimits::default().branch_history_bytes;
                            while low < high {
                                let middle = low + (high - low) / 2;
                                let limits = StorageLimits {
                                    branch_history_bytes: middle,
                                    ..StorageLimits::default()
                                };
                                if OrdinaryHistoryLimits::from_limits(&limits)
                                    .unwrap()
                                    .select(&prefix)
                                    .is_ok()
                                {
                                    high = middle;
                                } else {
                                    low = middle + 1;
                                }
                            }
                            low
                        });
                        setup_capacity.push(serde_json::json!({"case":case,"shape":case%3,"cap":cap,"reason":reason,"prefix_has_fitting_floor":fitting,"minimum_prefix_allowance":minimum,"short_by":minimum-cap,"prefix_locked":prefix.document.tracks[0].sync_lock,"prospective_locked":original.document.tracks[0].sync_lock}));
                        if fitting {
                            setup_disagreements.push((case, cap, reason));
                        }
                    }
                    other => panic!(
                        "GENERATED_SETUP_SAVE_IS_NOT_CAPACITY case={case} cap={cap}: {other:?}"
                    ),
                }
                continue;
            }
            let saved = host.export_history().unwrap();
            let proposed = Core::spawn(Document::default()).unwrap();
            proposed.restore_history(saved.clone()).unwrap();
            proposed
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true,
                }))
                .unwrap();
            let proposed = proposed.export_history().unwrap();
            let selected = policy.select(&proposed).ok();
            assert_eq!(
                selected,
                exhaustive_ordinary_floor(
                    &proposed,
                    &StorageLimits {
                        branch_history_bytes: cap,
                        ..StorageLimits::default()
                    }
                )
            );
            let alone = Core::spawn(std::sync::Arc::clone(&saved.document)).unwrap();
            alone
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true,
                }))
                .unwrap();
            let mut alone_cut = alone.export_history().unwrap();
            // A step on an empty history still has this project's absolute
            // position. Decimal offset digits are real bytes at an exact cap.
            alone_cut.floor_offset = saved.floor_offset + saved.batches.len() as u64;
            if policy.check(&alone_cut).is_ok() {
                assert!(
                    selected.is_some(),
                    "ONE_STEP_FITS_BUT_NO_FLOOR case={case} cap={cap}"
                );
            }
            let lease = owner
                .issue_manual_lease(format!("generated-{case}"), "generated-local".into())
                .unwrap();
            let gui = Core::spawn(Document::default()).unwrap();
            gui.restore_history(saved.clone()).unwrap();
            let replica =
                Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
            let permit = replica
                .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
                .unwrap();
            let bytes = serde_json::to_vec(&saved).unwrap();
            let event = permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true,
                }))
                .unwrap();
            if let Some(expected) = selected {
                assert!(
                    matches!(event, Event::DocumentChanged { .. }),
                    "PAYLOAD_DOOR_SAVE_AGREEMENT: {event:?}"
                );
                let actual = gui.export_history().unwrap();
                assert_eq!(actual, expected);
                floors += usize::from(actual.floor_offset != saved.floor_offset);
                let messages = permit.pending().unwrap();
                owner.commit_manual_history(&lease, &messages[0]).unwrap();
                replica.acknowledge_manual(&permit, messages).unwrap();
                drop(owner);
                let loaded = LoadedBundle::read(&path).unwrap();
                let cold = Core::spawn(Document::default()).unwrap();
                loaded.restore_manual(&cold).unwrap();
                assert_eq!(
                    cold.export_history().unwrap(),
                    actual,
                    "PAYLOAD_COLD_CUT_EXACT"
                );
                accepted += 1;
            } else {
                assert!(matches!(
                    event,
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualHistoryCapacity,
                        ..
                    }
                ));
                assert_eq!(
                    serde_json::to_vec(&gui.export_history().unwrap()).unwrap(),
                    bytes
                );
                assert!(permit.pending().unwrap().is_empty());
                refused += 1;
            }
        }
    }
    println!(
        "PAYLOAD_SETUP_CAPACITY={}",
        serde_json::to_string(&setup_capacity).unwrap()
    );
    assert!(
        setup_disagreements.is_empty(),
        "GENERATED_SETUP_SAVE_DISAGREES_WITH_ADMISSION: {setup_disagreements:?}"
    );
    assert!(
        accepted >= 100 && floors > 0,
        "PAYLOAD_CASES_MUST_REACH_THE_FLOOR"
    );
    println!(
        "PAYLOAD_FLOOR_AGREEMENT seed={SEED:#x} histories=60 caps=180 accepted={accepted} refused={refused} floors={floors}"
    );
    for clips in [2, 4, 8, 16] {
        let core = Core::spawn(heavy_document(clips)).unwrap();
        core.request(copies(u64::try_from(clips).unwrap())).unwrap();
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
        let full = core.export_history().unwrap();
        let floored = HistoryTransition::floor_after(&full, 1).unwrap();
        println!(
            "COPY_FLOOR_PEAK clips={clips} full={} floored={}",
            replay_peak(&full),
            replay_peak(&floored)
        );
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_400_clip_drag_keeps_accepted_sub_batches_while_save_catches_up() {
    live_drag_120(false);
}

#[test]
#[ignore = "GUI resend follow-up: retry the latest refused drag value when the publisher catches up"]
fn manual_history_400_clip_drag_accepts_all_120_sub_batches_gui_resend_followup() {
    live_drag_120(true);
}

fn live_drag_120(strict: bool) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-live-drag");
    let path = temp.path("drag.kinewright");
    let mut document = heavy_document_with_keys(400, 8);
    for clip in 2..=400 {
        Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(clip),
            names: None,
            include_keyframes: true,
        }
        .apply(&mut document)
        .unwrap();
    }
    let host = Core::spawn(document).unwrap();
    let mut owner =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &host, None)
            .unwrap();
    owner.save_active().unwrap();
    for index in 0..5 {
        host.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index % 2 == 0,
        }))
        .unwrap();
    }
    owner.save_active().unwrap();
    let before = host.export_history().unwrap();
    assert_eq!(
        before.cursor.undo.len(),
        5,
        "DRAG_STARTS_AT_THE_HEAVY_HISTORY_BOUND"
    );
    let lease = owner
        .issue_manual_lease("drag-person".into(), "drag-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let (publish, incoming) = std::sync::mpsc::channel::<AcceptedHistory>();
    let (completed, acknowledgements) = std::sync::mpsc::channel::<AcceptedHistory>();
    let mut times = vec![];
    let mut refused = vec![];
    let mut accepted_values = vec![];
    let mut peak_pending = 0;
    let mut peak_queued_bytes = 0;
    let mut last_message_bytes = 0;
    let mut first_queue_refusal = None;
    let (owner, save_times) = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let mut save_times = vec![];
            for item in incoming {
                let start = std::time::Instant::now();
                owner.commit_manual_history(&lease, &item).unwrap();
                save_times.push(start.elapsed().as_secs_f64() * 1000.0);
                completed.send(item).unwrap();
            }
            (owner, save_times)
        });
        for index in 0..120 {
            for item in acknowledgements.try_iter() {
                replica.acknowledge_manual(&permit, vec![item]).unwrap();
            }
            let started = std::time::Instant::now();
            let event = permit
                .request(Command::DoBatchCoalesced {
                    operations: vec![Operation::SetEffectParam {
                        clip: ClipId(1),
                        effect: EffectId(1),
                        name: "exposure_milli_stops".into(),
                        value: ParamValue::Integer(index),
                    }],
                    coalesce_key: "one-120-part-drag".into(),
                })
                .unwrap();
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            if matches!(event, Event::DocumentChanged { .. }) {
                accepted_values.push(index);
                let messages = permit.pending().unwrap();
                peak_pending = peak_pending.max(messages.len());
                let queued: usize = messages
                    .iter()
                    .map(|item| serde_json::to_vec(item).unwrap().len())
                    .sum();
                peak_queued_bytes = peak_queued_bytes.max(queued);
                last_message_bytes = serde_json::to_vec(messages.last().unwrap()).unwrap().len();
                publish.send(messages.last().unwrap().clone()).unwrap();
            } else {
                assert!(
                    matches!(
                        event,
                        Event::MutationRefused {
                            reason: CoreMutationRefusal::ManualBackpressure,
                            ..
                        }
                    ),
                    "GESTURE_MUST_NOT_REFUSE_HISTORY_CAPACITY: {event:?}"
                );
                if first_queue_refusal.is_none() {
                    let pending = permit.pending().unwrap();
                    let queued: usize = pending
                        .iter()
                        .map(|item| serde_json::to_vec(item).unwrap().len())
                        .sum();
                    first_queue_refusal = Some(
                        serde_json::json!({"sub_batch":index+1,"pending":pending.len(),"queued_bytes":queued,"free_bytes":kinewright_core::MANUAL_BYTES-queued,"last_accepted_message_bytes":last_message_bytes,"pending_floor_source":gui.ordinary_floor_source().unwrap().is_some()}),
                    );
                }
                refused.push((index, format!("{event:?}")));
            }
        }
        drop(publish);
        let (owner, saves) = worker.join().unwrap();
        for item in acknowledgements.try_iter() {
            replica.acknowledge_manual(&permit, vec![item]).unwrap();
        }
        (owner, saves)
    });
    assert!(permit.pending().unwrap().is_empty());
    let expected = gui.export_history().unwrap();
    assert_eq!(host.export_history().unwrap(), expected);
    let gesture = &expected.cursor.undo.last().unwrap().operations;
    let observed: Vec<_> = gesture
        .iter()
        .map(|operation| match operation {
            Operation::SetEffectParam {
                value: ParamValue::Integer(value),
                ..
            } => *value,
            other => panic!("unexpected gesture operation: {other:?}"),
        })
        .collect();
    assert_eq!(observed, accepted_values, "ACCEPTED_GESTURE_ORDER_IS_EXACT");
    drop(owner);
    drop(replica);
    drop(permit);
    let loaded = LoadedBundle::read(&path).unwrap();
    let cold = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&cold).unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
    let actual_exposure = match &expected.document.clip(ClipId(1)).unwrap().effects[0].parameters["exposure_milli_stops"]
    {
        ParamValue::Integer(value) => *value,
        _ => panic!("integer exposure fixture"),
    };
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let mut saves = save_times.clone();
    saves.sort_by(f64::total_cmp);
    println!(
        "LIVE_DRAG_120={}",
        serde_json::json!({
            "clips":400,"effects":400,"keyframes":3200,"intended_exposure":119,"actual_exposure":actual_exposure,"accepted":120-refused.len(),"refused":refused,
            "peak_pending":peak_pending,"peak_queued_bytes":peak_queued_bytes,"first_queue_refusal":first_queue_refusal,"accepted_cut_cold_reopened_exactly":true,"sub_batch_ms":times,"median_ms":f64::midpoint(sorted[59],sorted[60]),"worst_ms":sorted[119],
            "save_median_ms":saves[saves.len()/2],"save_worst_ms":saves.last(),
            "initial_undo":before.cursor.undo.len(),"final_undo":gui.export_history().unwrap().cursor.undo.len(),
            "floor_before":before.floor_offset,"floor_after":gui.export_history().unwrap().floor_offset,
        })
    );
    if strict {
        assert!(
            refused.is_empty(),
            "LIVE_GESTURE_MUST_NOT_REFUSE_OR_DROP_SUB_BATCHES"
        );
        assert_eq!(actual_exposure, 119);
    }
}

struct WalkEditor {
    owner: Ja1EditingSession,
    host: Core,
    gui: Core,
    replica: Ja1EditingSession,
    permit: ManualPermit,
    lease: kinewright_project::manual_history::ManualLease,
}
fn open_walk_editor(
    root: &std::path::Path,
    path: &std::path::Path,
    limits: StorageLimits,
) -> WalkEditor {
    let loaded = LoadedBundle::read(path).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let host = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&host).unwrap();
    let bundle = loaded.into_writer_with_limits_probe(limits).unwrap();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &host, inputs).unwrap();
    let lease = owner
        .issue_manual_lease("walk-person".into(), "walk-local".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(root, &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    WalkEditor {
        owner,
        host,
        gui,
        replica,
        permit,
        lease,
    }
}
impl WalkEditor {
    fn navigate(&mut self, redo: bool, path: &std::path::Path) -> bool {
        let before = self.gui.export_history().unwrap();
        let bytes = serde_json::to_vec(&before).unwrap();
        let rev = revision(&self.gui);
        let disk = std::fs::read(path).unwrap();
        let event = self
            .permit
            .request(if redo { Command::Redo } else { Command::Undo })
            .unwrap();
        if matches!(
            event,
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualUndoCapacity,
                ..
            }
        ) {
            assert!(!redo, "REDO_CAN_NEVER_REFUSE_CAPACITY");
            assert_eq!(
                serde_json::to_vec(&self.gui.export_history().unwrap()).unwrap(),
                bytes,
                "REFUSED_UNDO_IS_BYTE_IDENTICAL"
            );
            assert_eq!(revision(&self.gui), rev);
            assert_eq!(std::fs::read(path).unwrap(), disk);
            assert!(self.permit.pending().unwrap().is_empty());
            return false;
        }
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "REDO_CAN_NEVER_REFUSE_CAPACITY redo={redo}: {event:?}"
        );
        let messages = self.permit.pending().unwrap();
        self.owner
            .commit_manual_history(&self.lease, &messages[0])
            .unwrap();
        self.replica
            .acknowledge_manual(&self.permit, messages)
            .unwrap();
        assert_eq!(
            self.host.export_history().unwrap(),
            self.gui.export_history().unwrap()
        );
        true
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_never_refuse_redo_after_cold_reopen() {
    seeded_excursion_pair(0);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_pair_2() {
    seeded_excursion_pair(1);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_pair_3() {
    seeded_excursion_pair(2);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_pair_4() {
    seeded_excursion_pair(3);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_pair_5() {
    seeded_excursion_pair(4);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_seeded_excursions_pair_6() {
    seeded_excursion_pair(5);
}

fn seeded_excursion_pair(pair: usize) {
    let mut seed = 0x4a41_334e_4156_5eed_u64 ^ (pair as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut accepted_undos = 0;
    let mut refused_undos = 0;
    let mut redos = 0;
    let mut reopens = 0;
    for case in pair * 2..pair * 2 + 2 {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "seeded-navigation");
        let path = temp.path("walk.kinewright");
        let heavy = case % 2 == 1;
        let document = if heavy {
            let mut document = heavy_document_with_keys(40, 16);
            for clip in 2..=40 {
                Operation::CopyClipAttributes {
                    from_clip: ClipId(1),
                    to_clip: ClipId(clip),
                    names: None,
                    include_keyframes: true,
                }
                .apply(&mut document)
                .unwrap();
            }
            document
        } else {
            Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            }
        };
        let core = Core::spawn(document).unwrap();
        let limits = StorageLimits {
            branch_history_bytes: if heavy { 8 << 20 } else { 256 << 10 },
            ..StorageLimits::default()
        };
        let mut initial = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
        initial.first_save(&path, limits.clone()).unwrap();
        let policy = std::sync::Arc::new(
            kinewright_project::manual_history::OrdinaryHistoryAdmission::new(
                kinewright_project::manual_history::OrdinaryHistoryLimits::from_limits(&limits)
                    .unwrap(),
            ),
        );
        for edit in 0..36 {
            let event = policy
                .request(
                    &core,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: edit % 2 == 0,
                    }),
                )
                .unwrap();
            assert!(
                matches!(event, Event::DocumentChanged { .. }),
                "SMALL_STEP_ALWAYS_FITS: {event:?}"
            );
            initial.save_active().unwrap();
        }
        let top = core.export_history().unwrap().document;
        drop(initial);
        let mut editor = open_walk_editor(temp.root(), &path, limits.clone());
        for step in 0..36 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let history = editor.gui.export_history().unwrap();
            // Force the ruled Undo 5 / Redo 3 / Undo 3 zigzag before the random walk.
            let redo = !history.cursor.redo.is_empty()
                && if step < 5 {
                    false
                } else if step < 8 {
                    true
                } else if step < 11 {
                    false
                } else {
                    seed.is_multiple_of(3)
                };
            if editor.navigate(redo, &path) {
                if redo {
                    redos += 1;
                } else {
                    accepted_undos += 1;
                }
            } else {
                refused_undos += 1;
            }
            if step % 7 == 6 {
                let expected = editor.gui.export_history().unwrap();
                drop(editor);
                editor = open_walk_editor(temp.root(), &path, limits.clone());
                assert_eq!(
                    editor.gui.export_history().unwrap(),
                    expected,
                    "EXCURSION_COLD_CUT_IS_EXACT"
                );
                reopens += 1;
            }
        }
        while !editor.gui.export_history().unwrap().cursor.redo.is_empty() {
            assert!(editor.navigate(true, &path));
            redos += 1;
        }
        assert_eq!(
            editor.gui.export_history().unwrap().document,
            top,
            "ALL_ACCEPTED_WORK_REMAINS_REACHABLE"
        );
    }
    assert!(
        accepted_undos > 0 && redos > 0 && reopens > 0,
        "EXCURSIONS_EXERCISE_BOTH_CAPACITY_AND_RETURN"
    );
    println!(
        "SEEDED_GUARDED_EXCURSIONS pair={pair} cases=2 steps=72 undos={accepted_undos} refused_undos={refused_undos} redos={redos} reopens={reopens}"
    );
}

#[test]
fn manual_history_spare_baseline_floor_publishes_exactly() {
    use kinewright_project::manual_history::OrdinaryHistoryLimits;
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-allocation-floor-parity");
    let path = temp.path("parity.kinewright");
    let host = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    for index in 0_usize..4 {
        assert!(matches!(
            host.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: !index.is_multiple_of(2)
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let before = host.export_history().unwrap();
    let mut enlarged = before.clone();
    enlarged.replay_proof = None;
    let mut baseline = (*before.baseline).clone();
    baseline.tracks.reserve(4000);
    let replacement = std::sync::Arc::new(baseline);
    for boundary in enlarged
        .cursor
        .undo
        .iter_mut()
        .chain(&mut enlarged.cursor.redo)
    {
        if std::sync::Arc::ptr_eq(&boundary.document, &before.baseline) {
            boundary.document = std::sync::Arc::clone(&replacement);
        }
    }
    enlarged.baseline = replacement;
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(enlarged).unwrap();
    let enlarged = gui.export_history().unwrap();
    assert!(enlarged.baseline.tracks.capacity() >= 4001);
    let mut low = 1;
    let mut high = StorageLimits::default().branch_history_bytes;
    while low < high {
        let middle = low + (high - low) / 2;
        let limits = StorageLimits {
            branch_history_bytes: middle,
            ..StorageLimits::default()
        };
        if OrdinaryHistoryLimits::from_limits(&limits)
            .unwrap()
            .check(&enlarged)
            .is_ok()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let limits = StorageLimits {
        branch_history_bytes: low + 128,
        ..StorageLimits::default()
    };
    let policy = OrdinaryHistoryLimits::from_limits(&limits).unwrap();
    policy.check(&enlarged).unwrap();
    let shadow = Core::spawn(Document::default()).unwrap();
    shadow.restore_history(before).unwrap();
    shadow
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    let _ = policy.select(&shadow.export_history().unwrap()).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits).unwrap();
    let lease = owner
        .issue_manual_lease("allocation-person".into(), "allocation-local".into())
        .unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let event = permit
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    let expected = gui.export_history().unwrap();
    assert!(
        expected.floor_offset > enlarged.floor_offset,
        "ENLARGED_BASELINE_REQUIRES_A_FLOOR"
    );
    let pending = permit.pending().unwrap();
    assert_eq!(pending.len(), 1);
    let result = owner.commit_manual_history(&lease, &pending[0]);
    println!(
        "MANUAL_ALLOCATION_FLOOR_PARITY cap={} gui_floor={} host_floor={} publication={result:?}",
        low + 128,
        expected.floor_offset,
        host.export_history().unwrap().floor_offset
    );
    result.expect("DOOR_ADMITTED_ALLOCATION_FLOOR_MUST_PUBLISH");
    replica.acknowledge_manual(&permit, pending).unwrap();
    assert_eq!(host.export_history().unwrap(), expected);
    drop(owner);
    let loaded = LoadedBundle::read(&path).unwrap();
    let cold = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&cold).unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
}

#[test]
fn manual_history_allocation_geometries_keep_canonical_floors_and_real_bounds() {
    use kinewright_core::HistoryAllocationProbe;
    use kinewright_project::manual_history::OrdinaryHistoryLimits;
    for (shape, target) in [
        ("baseline", HistoryAllocationProbe::Baseline),
        ("cursor", HistoryAllocationProbe::Cursor),
        ("current", HistoryAllocationProbe::Current),
        ("vectors", HistoryAllocationProbe::Metadata),
    ] {
        for side in 1..=3 {
            let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-geometries");
            let path = temp.path("geometry.kinewright");
            let host = Core::spawn(Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            })
            .unwrap();
            for n in 0_usize..4 {
                host.request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: !n.is_multiple_of(2),
                }))
                .unwrap();
            }
            let before = host.export_history().unwrap();
            let mut low = 1;
            let mut high = StorageLimits::default().branch_history_bytes;
            while low < high {
                let middle = low + (high - low) / 2;
                let limits = StorageLimits {
                    branch_history_bytes: middle,
                    ..StorageLimits::default()
                };
                if OrdinaryHistoryLimits::from_limits(&limits)
                    .unwrap()
                    .check(&before)
                    .is_ok()
                {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            let limits = StorageLimits {
                branch_history_bytes: low + 128,
                ..StorageLimits::default()
            };
            let policy = OrdinaryHistoryLimits::from_limits(&limits).unwrap();
            let expected_core = Core::spawn(Document::default()).unwrap();
            expected_core.restore_history(before.clone()).unwrap();
            expected_core
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                }))
                .unwrap();
            let expected = policy
                .select(&expected_core.export_history().unwrap())
                .unwrap();
            assert!(expected.floor_offset > before.floor_offset);
            let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
            owner.first_save(&path, limits.clone()).unwrap();
            let lease = owner
                .issue_manual_lease("geometry-person".into(), "geometry-client".into())
                .unwrap();
            let gui = Core::spawn(Document::default()).unwrap();
            gui.restore_history(before).unwrap();
            let replica = Ja1EditingSession::new(temp.root(), &gui, limits.clone()).unwrap();
            let permit = replica
                .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
                .unwrap();
            let mut inflated = 0;
            if side & 1 != 0 {
                inflated = gui
                    .inflate_history_allocation_probe(target, 100_000)
                    .unwrap();
            }
            if side & 2 != 0 {
                inflated = inflated.max(
                    host.inflate_history_allocation_probe(target, 100_000)
                        .unwrap(),
                );
            }
            assert!(
                inflated > limits.branch_history_bytes,
                "MAXIMAL_SPARE_REALLY_EXCEEDS_BOUND"
            );
            let event = permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                }))
                .unwrap();
            assert!(
                matches!(event, Event::DocumentChanged { .. }),
                "CANONICAL_GEOMETRY_SELECTION admission {shape}/{side}: {event:?}"
            );
            let accepted = gui.export_history().unwrap();
            assert_eq!(
                accepted, expected,
                "CANONICAL_GEOMETRY_SELECTION {shape}/{side}"
            );
            let items = permit.pending().unwrap();
            owner
                .commit_manual_history(&lease, &items[0])
                .expect("GEOMETRY_DOOR_SAVE_PARITY");
            replica.acknowledge_manual(&permit, items).unwrap();
            assert_eq!(host.export_history().unwrap(), expected);
            let gui_actual = gui
                .inflate_history_allocation_probe(HistoryAllocationProbe::Metadata, 0)
                .unwrap();
            let host_actual = host
                .inflate_history_allocation_probe(HistoryAllocationProbe::Metadata, 0)
                .unwrap();
            assert!(
                gui_actual <= limits.branch_history_bytes
                    && host_actual <= limits.branch_history_bytes,
                "REAL_MEMORY_CURED gui={gui_actual} host={host_actual}"
            );
            drop(owner);
            let cold = Core::spawn(Document::default()).unwrap();
            LoadedBundle::read_with_limits_probe(&path, limits.clone())
                .unwrap()
                .restore_manual(&cold)
                .unwrap();
            assert_eq!(cold.export_history().unwrap(), expected);
            println!(
                "MANUAL_CANONICAL_GEOMETRY shape={shape} side={side} inflated={inflated} gui_actual={gui_actual} host_actual={host_actual} cap={} floor={}",
                limits.branch_history_bytes, expected.floor_offset
            );
        }
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn manual_history_natural_vector_growth_keeps_publishing_across_floors() {
    use kinewright_project::manual_history::OrdinaryHistoryLimits;
    let mut document = Document::default();
    for track in 1..=9 {
        Operation::AddTrack {
            track: Track {
                id: TrackId(track),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }
        .apply(&mut document)
        .unwrap();
        for clip in 0..9 {
            Operation::AddSolidClip {
                track: TrackId(track),
                timeline_start: TimeCode(clip * 90),
                duration: TimeCode(90),
                color: SolidColor { r: 0, g: 0, b: 0 },
            }
            .apply(&mut document)
            .unwrap();
        }
    }
    Operation::RemoveTrack { track: TrackId(9) }
        .apply(&mut document)
        .unwrap();
    for clip in [ClipId(2), ClipId(4), ClipId(6)] {
        Operation::DeleteClip { clip }.apply(&mut document).unwrap();
    }
    let natural_tracks = document.tracks.capacity();
    let natural_clips = document.tracks[0].clips.capacity();
    assert!(
        natural_tracks > document.tracks.len() || natural_clips > document.tracks[0].clips.len()
    );
    let host = Core::spawn(document).unwrap();
    for n in 0_usize..4 {
        host.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: !n.is_multiple_of(2),
        }))
        .unwrap();
    }
    let before = host.export_history().unwrap();
    let old_geometry = Core::spawn(Document::default()).unwrap();
    old_geometry.restore_history(before.clone()).unwrap();
    let journal = HistoryJournal::capture(&before).unwrap();
    let canonical = journal
        .resolve_canonical_with_budget(&before.baseline, &before.document, 32 << 20, None)
        .unwrap();
    let allocation_sensitive = journal
        .resolve_verified_with_budget(
            std::sync::Arc::clone(&before.baseline),
            &before.document,
            32 << 20,
            None,
        )
        .unwrap();
    // This is the old executor, on real geometry, without checking out an old head.
    println!(
        "MANUAL_NATURAL_GEOMETRY tracks_cap={natural_tracks} clips_cap={natural_clips} canonical_peak={} allocation_sensitive_peak={} old_charge_disagrees={}",
        canonical.replay_peak(),
        allocation_sensitive.replay_peak(),
        canonical.replay_peak() != allocation_sensitive.replay_peak()
    );
    let limits = StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    };
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-natural-geometry");
    let path = temp.path("natural.kinewright");
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let old_initial =
        allocation_sensitive_floor_probe(&old_geometry.export_history().unwrap(), &limits);
    println!(
        "MANUAL_NATURAL_FIRST_SAVE canonical_floor={} old_floor={}",
        host.export_history().unwrap().floor_offset,
        old_initial.floor_offset
    );
    old_geometry.restore_history(old_initial).unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let lease = owner
        .issue_manual_lease("natural-person".into(), "natural-client".into())
        .unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, limits.clone()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let policy = OrdinaryHistoryLimits::from_limits(&limits).unwrap();
    let mut floors = 0;
    let mut old_floor_disagreements = 0;
    let mut offset = gui.export_history().unwrap().floor_offset;
    for edit in 0_usize..120 {
        let event = permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit.is_multiple_of(2),
            }))
            .unwrap();
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "NATURAL_EDIT_MUST_CONTINUE {event:?}"
        );
        let history = gui.export_history().unwrap();
        old_geometry
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit.is_multiple_of(2),
            }))
            .unwrap();
        let old_before = old_geometry.export_history().unwrap();
        let old_selected = allocation_sensitive_floor_probe(&old_before, &limits);
        if old_selected.floor_offset != history.floor_offset {
            old_floor_disagreements += 1;
        }
        if old_selected.floor_offset != old_before.floor_offset {
            old_geometry.restore_history(old_selected).unwrap();
        }
        if history.floor_offset > offset {
            floors += 1;
            offset = history.floor_offset;
        }
        let pending = permit.pending().unwrap();
        owner
            .commit_manual_history(&lease, &pending[0])
            .expect("NATURAL_DOOR_SAVE_PARITY");
        replica.acknowledge_manual(&permit, pending).unwrap();
        assert_eq!(host.export_history().unwrap(), history);
        policy.check(&history).unwrap();
    }
    assert!(floors >= 3);
    let expected = gui.export_history().unwrap();
    drop(owner);
    let cold = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read_with_limits_probe(&path, limits)
        .unwrap()
        .restore_manual(&cold)
        .unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
    println!(
        "MANUAL_NATURAL_CONTINUITY edits=120 refusals=0 floors={floors} undo={} floor={offset} old_floor_disagreements={old_floor_disagreements}",
        expected.cursor.undo.len()
    );
}

#[test]
fn manual_history_writer_verifies_any_legal_admitted_floor_without_reselecting() {
    struct DeeperFloor(Box<dyn ManualHistoryEncoder>);
    impl HistoryAdmission for DeeperFloor {
        fn check_history(
            &mut self,
            history: &HistorySnapshot,
        ) -> Result<Option<ResolvedHistory>, HistoryError> {
            let Some(proof) = self.0.check_history(history)? else {
                return Ok(None);
            };
            if proof.snapshot().floor_offset > history.floor_offset {
                let selected = HistoryTransition::floor_after(proof.snapshot(), 1)?;
                let journal = HistoryJournal::capture(&selected)?;
                Ok(Some(journal.resolve_canonical_with_budget(
                    &selected.baseline,
                    &selected.document,
                    87_406,
                    None,
                )?))
            } else {
                Ok(Some(proof))
            }
        }
        fn finish_history_check(&mut self, accepted: bool) {
            self.0.finish_history_check(accepted);
        }
    }
    impl ManualHistoryEncoder for DeeperFloor {
        fn encode(
            &self,
            sequence: u64,
            revision: TimelineRevision,
            predecessor: Option<&str>,
            candidate: ManualHistoryCandidate<'_>,
        ) -> Result<AcceptedHistory, HistoryError> {
            self.0.encode(sequence, revision, predecessor, candidate)
        }
    }
    let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-chosen-floor");
    let path = temp.path("chosen.kinewright");
    let host = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    for n in 0_usize..4 {
        host.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: !n.is_multiple_of(2),
        }))
        .unwrap();
    }
    let limits = StorageLimits {
        branch_history_bytes: 87_406,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let lease = owner
        .issue_manual_lease("chosen-person".into(), "chosen-client".into())
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, limits.clone()).unwrap();
    let encoder = lease
        .clone()
        .into_encoder(owner.manual_input_context().unwrap())
        .unwrap();
    let permit = replica
        .bind_manual_encoder_probe(Box::new(DeeperFloor(encoder)))
        .unwrap();
    assert!(matches!(
        permit
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let expected = gui.export_history().unwrap();
    assert_eq!(
        expected.floor_offset, 4,
        "PRODUCER_CHOOSES_A_DIFFERENT_VALID_FLOOR"
    );
    let pending = permit.pending().unwrap();
    owner
        .commit_manual_history(&lease, &pending[0])
        .expect("WRITER_VERIFIES_CARRIED_FLOOR");
    replica.acknowledge_manual(&permit, pending).unwrap();
    assert_eq!(host.export_history().unwrap(), expected);
    drop(owner);
    let cold = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read_with_limits_probe(&path, limits)
        .unwrap()
        .restore_manual(&cold)
        .unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
}

// Executable old allocation-sensitive predicate, for the natural-geometry
// comparison only. No old head is checked out and production never calls it.
fn allocation_sensitive_floor_probe(
    history: &HistorySnapshot,
    limits: &StorageLimits,
) -> HistorySnapshot {
    let fits = |cut: &HistorySnapshot| {
        use kinewright_project::{
            history_record::{HistoryRecord, HistoryRole},
            store::BundleRecord,
        };
        let role = HistoryRole::Ordinary;
        let Ok(record) = HistoryRecord::capture(cut, role) else {
            return false;
        };
        if record.validate(limits).is_err() {
            return false;
        }
        let Ok(resolved) = record.journal.resolve_verified_with_budget(
            std::sync::Arc::clone(&cut.baseline),
            &cut.document,
            role.live_limit(limits).min(limits.decoded_record_bytes),
            None,
        ) else {
            return false;
        };
        let history = resolved.snapshot();
        history == cut
            && history.ordinary_admission_bytes().is_ok()
            && kinewright_project::bundle_commit::ordinary_history_admission_cost_probe(
                history, limits,
            )
            .is_ok_and(|n| n <= limits.branch_history_bytes)
            && kinewright_core::history_size::restored_history_live_bytes(history)
                .is_ok_and(|n| n <= role.live_limit(limits))
    };
    if fits(history) {
        return history.clone();
    }
    let first = (1..history.batches.len())
        .find_map(|discard| {
            let cut = HistoryTransition::floor_after(history, discard).ok()?;
            (!cut.cursor.undo.is_empty() && fits(&cut)).then_some(cut)
        })
        .expect("natural old selector has a fitting newest step");
    let retained = first.cursor.undo.len();
    let maximum = retained.saturating_sub(retained.div_ceil(8)).max(1);
    (1..history.batches.len())
        .find_map(|discard| {
            let cut = HistoryTransition::floor_after(history, discard).ok()?;
            (!cut.cursor.undo.is_empty() && cut.cursor.undo.len() <= maximum && fits(&cut))
                .then_some(cut)
        })
        .unwrap_or(first)
}

// Independent review witness: delayed acknowledgements across floors, including
// coalesced updates and navigation, with exact retries after late writer failure.
#[test]
fn astra_review_delayed_ack_carried_floor_and_publish_rollback() {
    use kinewright_project::io_observer;
    let temp = TempDirectory::new_in(&workspace_target_dir(), "astra-fix3-delayed");
    let path = temp.path("delayed.kinewright");
    let host = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 180_000,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let lease = owner
        .issue_manual_lease("a".repeat(64), "d".repeat(64))
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
        .unwrap();
    let mut expected = std::collections::VecDeque::new();
    let mut floors = 0;
    let mut offset = 0;
    let mut failures = 0;
    let mut redo_count = 0;
    for index in 0_usize..90 {
        let lock = Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index.is_multiple_of(2),
        };
        let command = match index % 9 {
            3 | 4 => Command::DoBatchCoalesced {
                operations: vec![lock],
                coalesce_key: format!("drag-{}", index / 9),
            },
            5 => Command::Undo,
            6 => Command::Redo,
            _ => Command::Do(lock),
        };
        let before = gui.export_history().unwrap();
        let rev = revision(&gui);
        let event = permit.request(command.clone()).unwrap();
        if matches!(
            event,
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualUndoCapacity,
                ..
            }
        ) {
            assert_eq!(before, gui.export_history().unwrap());
            assert_eq!(rev, revision(&gui));
            continue;
        }
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "index={index}: {event:?}"
        );
        if matches!(command, Command::Redo) && !before.cursor.redo.is_empty() {
            redo_count += 1;
        }
        let cut = gui.export_history().unwrap();
        if cut.floor_offset != offset {
            floors += 1;
            offset = cut.floor_offset;
        }
        expected.push_back(cut);
        // Three accepted messages can have different floors before any ack.
        if expected.len() >= 3 || index == 89 {
            let pending = permit.pending().unwrap();
            for message in pending {
                let before = host.export_history().unwrap();
                let root_bytes = std::fs::read(&path).unwrap();
                if message.sequence % 7 == 0 {
                    let failure = io_observer::fail_once("root_file_sync", || {
                        owner.commit_manual_history(&lease, &message)
                    });
                    assert!(failure.is_err(), "late fault must execute");
                    assert_eq!(host.export_history().unwrap(), before, "writer rollback");
                    assert_eq!(std::fs::read(&path).unwrap(), root_bytes, "disk rollback");
                    failures += 1;
                }
                owner
                    .commit_manual_history(&lease, &message)
                    .expect("exact retry publishes carried floor");
                assert_eq!(
                    host.export_history().unwrap(),
                    expected.pop_front().unwrap()
                );
                // Acknowledge the old floor only after the producer already moved further.
                replica.acknowledge_manual(&permit, vec![message]).unwrap();
            }
        }
    }
    assert!(expected.is_empty());
    assert!(
        floors > 5 && failures > 5 && redo_count > 0,
        "floors={floors} failures={failures} redos={redo_count}"
    );
    let expected = gui.export_history().unwrap();
    drop(owner);
    let cold = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read_with_limits_probe(&path, limits)
        .unwrap()
        .restore_manual(&cold)
        .unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
    println!("ASTRA_DELAYED_ACK floors={floors} injected_failures={failures} redos={redo_count}");
}

#[test]
fn astra_review_recover_absent_floored_edit() {
    let mut refused_cases = 0;
    for production in [false, true] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "astra-fix3-recover");
        let path = temp.path("recover.kinewright");
        let mut document = if production {
            heavy_document_with_keys(400, 8)
        } else {
            Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            }
        };
        if production {
            for clip in 2..=400 {
                Operation::CopyClipAttributes {
                    from_clip: ClipId(1),
                    to_clip: ClipId(clip),
                    names: None,
                    include_keyframes: true,
                }
                .apply(&mut document)
                .unwrap();
            }
        }
        let limits = if production {
            StorageLimits::default()
        } else {
            StorageLimits {
                branch_history_bytes: 87_406,
                ..StorageLimits::default()
            }
        };
        let host = Core::spawn(document).unwrap();
        let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
        owner.first_save(&path, limits.clone()).unwrap();
        let lease = owner
            .issue_manual_lease("a".repeat(64), "r".repeat(64))
            .unwrap();
        let gui = Core::spawn(Document::default()).unwrap();
        gui.restore_history(host.export_history().unwrap()).unwrap();
        let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
        let permit = replica
            .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
            .unwrap();
        let mut found = false;
        for index in 0_usize..80 {
            let before = gui.export_history().unwrap();
            assert!(matches!(
                permit
                    .request(Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: !index.is_multiple_of(2)
                    }))
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
            let expected = gui.export_history().unwrap();
            let pending = permit.pending().unwrap();
            if expected.floor_offset == before.floor_offset {
                owner.commit_manual_history(&lease, &pending[0]).unwrap();
                replica.acknowledge_manual(&permit, pending).unwrap();
                continue;
            }
            // Accepted by the GUI; the writer exits before receiving this envelope.
            drop(owner);
            let loaded = LoadedBundle::read_with_limits_probe(&path, limits.clone()).unwrap();
            let inputs = loaded.root().unwrap().input_manifest_ref.clone();
            let reopened = Core::spawn(Document::default()).unwrap();
            loaded.restore_manual(&reopened).unwrap();
            assert_eq!(reopened.export_history().unwrap(), before);
            let bundle = loaded
                .into_writer_with_limits_probe(limits.clone())
                .unwrap();
            owner = Ja1EditingSession::from_bundle(bundle, &reopened, inputs).unwrap();
            let first = owner.reconcile_manual_chain(&"a".repeat(64), &pending, Some(&expected));
            let second = owner.reconcile_manual_chain(&"a".repeat(64), &pending, Some(&expected));
            println!(
                "ASTRA_ABSENT_FLOOR production={production} edit={} floor={}->{} first={first:?} second={second:?} live_matches_old={} pending={}",
                index + 1,
                before.floor_offset,
                expected.floor_offset,
                reopened.export_history().unwrap() == before,
                pending.len()
            );
            // Record both fixtures before making the failure assertion below.
            let refused = first.is_err() || second.is_err();
            if refused {
                refused_cases += 1;
                assert!(matches!(
                    first,
                    Err(kinewright_project::bundle::BundleError::HistoryChanged)
                ));
                assert!(matches!(
                    second,
                    Err(kinewright_project::bundle::BundleError::HistoryChanged)
                ));
                assert_eq!(reopened.export_history().unwrap(), before);
            }
            drop(owner);
            let cold = Core::spawn(Document::default()).unwrap();
            LoadedBundle::read_with_limits_probe(&path, limits.clone())
                .unwrap()
                .restore_manual(&cold)
                .unwrap();
            if refused {
                assert_eq!(cold.export_history().unwrap(), before);
                assert_ne!(cold.export_history().unwrap().document, expected.document);
            } else {
                assert_eq!(cold.export_history().unwrap(), expected);
            }
            found = true;
            break;
        }
        assert!(found, "must reach a floor");
    }
    assert_eq!(
        refused_cases, 0,
        "accepted floored edits must recover after writer restart"
    );
}

#[test]
fn astra_review_pending_ordinary_floor_survives_named_host_handoff() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "astra-fix3-handoff");
    let path = temp.path("handoff.kinewright");
    let mut document = heavy_document_with_keys(400, 8);
    for clip in 2..=400 {
        Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(clip),
            names: None,
            include_keyframes: true,
        }
        .apply(&mut document)
        .unwrap();
    }
    let core = Core::spawn(document).unwrap();
    let limits = StorageLimits::default();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    for index in 0_usize..80 {
        assert!(matches!(
            admission
                .request(
                    &core,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: !index.is_multiple_of(2)
                    })
                )
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        if core.ordinary_floor_source().unwrap().is_some() {
            break;
        }
        // Leave this accepted tail unpublished before the handoff.
    }
    assert!(core.ordinary_floor_source().unwrap().is_some());
    let expected = core.export_history_for_publication().unwrap();
    let prepared = owner.prepare_host_attachment().unwrap();
    println!(
        "ASTRA_HANDOFF before_source={} prepared_source={} actor_still_has_source={}",
        expected.ordinary_floor_source.is_some(),
        prepared.history.ordinary_floor_source.is_some(),
        core.ordinary_floor_source().unwrap().is_some()
    );
    let loaded = LoadedBundle::read_with_limits_probe(&path, limits.clone()).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let shadow = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&shadow).unwrap();
    let disk = shadow.export_history().unwrap();
    println!(
        "ASTRA_HANDOFF disk_floor={} accepted_floor={} disk_matches_accepted={}",
        disk.floor_offset,
        expected.floor_offset,
        disk == expected
    );
    let bundle = loaded.into_writer_with_limits_probe(limits).unwrap();
    let mut successor = Ja1EditingSession::from_bundle(bundle, &shadow, inputs).unwrap();
    successor.install_host_snapshot(prepared.history).unwrap();
    let first = successor.save_active();
    let second = successor.save_active();
    println!("ASTRA_HANDOFF first={first:?} second={second:?}");
    assert!(
        first.is_ok(),
        "named handoff must keep accepted floor publishable"
    );
    assert!(second.is_ok());
}

#[test]
fn astra_review_floored_branch_applied_operations_still_reconstruct_head() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let mut document = heavy_document_with_keys(400, 8);
    for clip in 2..=400 {
        Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(clip),
            names: None,
            include_keyframes: true,
        }
        .apply(&mut document)
        .unwrap();
    }
    let base = document.clone();
    let branch = Core::spawn_delta(document, TimelineRevision::default()).unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::default(),
    ));
    assert!(matches!(
        admission
            .request(
                &branch,
                Command::Do(Operation::AddMarker {
                    marker: Marker {
                        id: MarkerId(999),
                        position: TimeCode(0),
                        label: "kept agent edit".into(),
                        color_token: 0
                    }
                })
            )
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let mut refused = false;
    for index in 0_usize..80 {
        let before = branch.export_history().unwrap();
        let event = admission
            .request(
                &branch,
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index.is_multiple_of(2),
                }),
            )
            .unwrap();
        if let Event::MutationRefused {
            reason: CoreMutationRefusal::ThreadHistoryCapacity,
            ..
        } = event
        {
            assert_eq!(before, branch.export_history().unwrap());
            refused = true;
            break;
        }
        assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    }
    let accepted = branch.export_history().unwrap();
    assert!(refused);
    assert_eq!(accepted.floor_offset, 0);
    assert_eq!(accepted.document.markers.len(), 1);
    // TimelineBranch::compare and merge_into use exactly this query and batch.
    let Event::QueryResult(QueryResult::AppliedOperations(operations)) = branch
        .request(Command::Query(Query::AppliedOperations))
        .unwrap()
    else {
        panic!("applied operations");
    };
    let live = Core::spawn(base).unwrap();
    assert!(matches!(
        OrdinaryHistoryLimits::default()
            .request(
                &live,
                Command::DoBatchIfRevision {
                    expected: TimelineRevision(0),
                    operations: (*operations).clone()
                }
            )
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let merged = live.export_history().unwrap();
    println!(
        "ASTRA_BRANCH floor={} accepted_markers={} applied_operations={} merged_markers={}",
        accepted.floor_offset,
        accepted.document.markers.len(),
        operations.len(),
        merged.document.markers.len()
    );
    assert!(
        merged.document == accepted.document,
        "legacy merge must retain every accepted branch edit"
    );
}

#[test]
fn manual_recovery_publishes_two_pending_floors_in_order_and_retries_exactly() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "fix3fix1-recovery-floors");
    let path = temp.path("recover.kinewright");
    let host = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 180_000,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &host, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let lease = owner
        .issue_manual_lease("a".repeat(64), "f".repeat(64))
        .unwrap();
    let gui = Core::spawn(Document::default()).unwrap();
    gui.restore_history(host.export_history().unwrap()).unwrap();
    let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(lease, owner.manual_input_context().unwrap())
        .unwrap();
    let mut floors = 0;
    let mut offset = 0;
    for index in 0..60 {
        assert!(matches!(
            permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index % 2 == 0
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let cut = gui.export_history().unwrap();
        if cut.floor_offset > offset {
            floors += 1;
            offset = cut.floor_offset;
        }
        if floors >= 2 {
            break;
        }
    }
    assert!(floors >= 2);
    let expected = gui.export_history().unwrap();
    let pending = permit.pending().unwrap();
    drop(owner);
    let loaded = LoadedBundle::read_with_limits_probe(&path, limits.clone()).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let reopened = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&reopened).unwrap();
    owner = Ja1EditingSession::from_bundle(
        loaded
            .into_writer_with_limits_probe(limits.clone())
            .unwrap(),
        &reopened,
        inputs,
    )
    .unwrap();
    let interrupted = kinewright_project::io_observer::fail_nth("root_file_sync", 4, || {
        owner.reconcile_manual_chain(&"a".repeat(64), &pending, Some(&expected))
    });
    assert!(interrupted.is_err());
    let committed = pending
        .iter()
        .take_while(|message| {
            matches!(
                owner.manual_receipt(message).unwrap(),
                kinewright_project::receipts::ReceiptLookup::Committed(_)
                    | kinewright_project::receipts::ReceiptLookup::Retired(_)
            )
        })
        .count();
    assert!(
        committed > 0 && committed < pending.len(),
        "must fail after a durable prefix: {committed}/{}",
        pending.len()
    );
    let original =
        kinewright_project::manual_history::AcceptedManualCommand::decode(&pending[committed - 1])
            .unwrap();
    assert_eq!(
        kinewright_project::manual_history::history_sha256(&reopened.export_history().unwrap())
            .unwrap(),
        original.envelope.manual.unwrap().after_sha256,
        "a later failure must keep the committed prefix installed"
    );
    owner
        .reconcile_manual_chain(&"a".repeat(64), &pending, Some(&expected))
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    owner
        .reconcile_manual_chain(&"a".repeat(64), &pending, Some(&expected))
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(reopened.export_history().unwrap(), expected);
    for message in &pending {
        assert!(matches!(
            owner.manual_receipt(message).unwrap(),
            kinewright_project::receipts::ReceiptLookup::Committed(_)
                | kinewright_project::receipts::ReceiptLookup::Retired(_)
        ));
    }
    drop(owner);
    let cold = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read_with_limits_probe(&path, limits)
        .unwrap()
        .restore_manual(&cold)
        .unwrap();
    assert_eq!(cold.export_history().unwrap(), expected);
    println!(
        "RECOVERED_PENDING_FLOORS={floors};messages={}",
        pending.len()
    );
}

#[test]
fn never_saved_ordinary_endpoint_floors_repeatedly_without_waiting_for_an_ack() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 100_000,
        ..StorageLimits::default()
    };
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    let mut floors = 0;
    let mut offset = 0;
    for index in 0..60 {
        assert!(matches!(
            admission
                .request(
                    &core,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0
                    })
                )
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let history = core.export_history_for_publication().unwrap();
        assert!(history.ordinary_floor_source.is_none());
        if history.floor_offset > offset {
            floors += 1;
            offset = history.floor_offset;
        }
    }
    assert!(floors > 2);
    println!("UNSAVED_ENDPOINT_FLOORS={floors}");
    // First Save may have landed even when its durable acknowledgement is lost.
    // A later runtime floor must remain publishable from that exact root too.
    for landed in [false, true] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "first-save-runtime-floor");
        let path = temp.path("first.kinewright");
        let branch = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner = Ja1EditingSession::new(temp.root(), &branch, limits.clone()).unwrap();
        let status = kinewright_project::io_observer::fail_once(
            if landed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            },
            || owner.first_save(&path, limits.clone()),
        )
        .unwrap();
        assert!(matches!(
            status,
            kinewright_project::bundle_copy::CopyStatus::DurabilityUnknown
        ));
        let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
            OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
        ));
        assert!(branch.pending_history_save().unwrap().is_some());
        let mut accepted = 0;
        let mut refused = false;
        for index in 0..30 {
            let before = branch.export_history().unwrap();
            match admission
                .request(
                    &branch,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0,
                    }),
                )
                .unwrap()
            {
                Event::DocumentChanged { .. } => accepted += 1,
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualSaveRequired,
                    ..
                } => {
                    assert_eq!(branch.export_history().unwrap(), before);
                    refused = true;
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        let expected = branch.export_history().unwrap();
        assert!(accepted > 0);
        assert!(refused, "an uncertain first root must stop a moving floor");
        assert_eq!(expected.floor_offset, 0);
        let saved = owner.save_active();
        println!(
            "FIRST_SAVE_RUNTIME_FLOOR landed={landed} accepted={accepted} floor={} saved={saved:?}",
            expected.floor_offset
        );
        saved.unwrap();
        assert!(branch.pending_history_save().unwrap().is_none());
        assert_eq!(branch.export_history().unwrap(), expected);
        drop(owner);
        let cold = Core::spawn(Document::default()).unwrap();
        LoadedBundle::read_with_limits_probe(&path, limits.clone())
            .unwrap()
            .restore_manual(&cold)
            .unwrap();
        assert_eq!(cold.export_history().unwrap(), expected);
    }
}

#[test]
fn named_core_only_endpoint_requires_save_instead_of_promising_an_automatic_ack() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "ordinary-final-save-instruction");
    let path = temp.path("named.kinewright");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 100_000,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    let mut refused = false;
    for index in 0..60 {
        let before = core.export_history_for_publication().unwrap();
        let command = Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index % 2 == 0,
        });
        match admission.request(&core, command.clone()).unwrap() {
            Event::DocumentChanged { .. } => {}
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualSaveRequired,
                ..
            } => {
                assert!(before.ordinary_floor_source.is_some());
                assert_eq!(core.export_history_for_publication().unwrap(), before);
                assert_eq!(
                    CoreMutationRefusal::ManualSaveRequired.to_string(),
                    "Save the project to keep editing."
                );
                // An identical retry, with no person action, remains final.
                assert!(matches!(
                    admission.request(&core, command).unwrap(),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualSaveRequired,
                        ..
                    }
                ));
                refused = true;
                break;
            }
            other => panic!("no automatic publisher: {other:?}"),
        }
    }
    assert!(refused);
}

#[test]
fn durable_delta_session_saves_its_unpublished_tail_without_a_runtime_floor_gap() {
    use kinewright_project::{
        editing_session::DurableBranchRole,
        manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits},
    };
    let temp = TempDirectory::new_in(&workspace_target_dir(), "session-no-runtime-floor");
    let path = temp.path("session.kinewright");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 100_000,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
    owner.first_save(&path, limits.clone()).unwrap();
    let mut session = owner
        .fork_durable_branch("legacy session", DurableBranchRole::Session)
        .unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    let mut refused = false;
    for index in 0..60 {
        let before = session.core().export_history().unwrap();
        match admission
            .request(
                session.core(),
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index % 2 == 0,
                }),
            )
            .unwrap()
        {
            Event::DocumentChanged { .. } => {}
            Event::MutationRefused {
                reason: CoreMutationRefusal::ThreadHistoryCapacity,
                ..
            } => {
                assert_eq!(before, session.core().export_history().unwrap());
                refused = true;
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(refused);
    let expected = session.core().export_history().unwrap();
    assert_eq!(expected.floor_offset, 0);
    assert!(session.core().ordinary_floor_source().unwrap().is_none());
    let base = owner.persist_durable_branch(&mut session).unwrap();
    owner.persist_durable_branch(&mut session).unwrap();
    assert_eq!(*owner.durable_branch_history(&session).unwrap(), expected);
    let branch = session.branch_id().clone();
    drop(session);
    drop(owner);
    let loaded = LoadedBundle::read_with_limits_probe(&path, limits.clone()).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let live = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&live).unwrap();
    let mut owner = Ja1EditingSession::from_bundle(
        loaded.into_writer_with_limits_probe(limits).unwrap(),
        &live,
        inputs,
    )
    .unwrap();
    let restored = owner
        .reopen_durable_branch(branch, &base, "legacy session", DurableBranchRole::Session)
        .unwrap();
    assert_eq!(restored.core().export_history().unwrap(), expected);
}

#[test]
#[ignore = "on-demand: paired debug lease, edit and publication costs at 1000, 1980, floored 12000 edits and 400 clips"]
fn history_fix3_followon_costs() {
    use std::time::Instant;
    for (row, document, setup) in [
        (
            "1000",
            Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            },
            1_000,
        ),
        (
            "1980",
            Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            },
            1980,
        ),
        (
            "12000-floored",
            Document {
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                }],
                ..Document::default()
            },
            12000,
        ),
        (
            "400",
            {
                let mut document = heavy_document_with_keys(400, 8);
                for clip in 2..=400 {
                    Operation::CopyClipAttributes {
                        from_clip: ClipId(1),
                        to_clip: ClipId(clip),
                        names: None,
                        include_keyframes: true,
                    }
                    .apply(&mut document)
                    .unwrap();
                }
                document
            },
            0,
        ),
    ] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "fix3-followon-costs");
        let path = temp.path("cost.kinewright");
        let host = Core::spawn(document).unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &host, StorageLimits::default()).unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        for index in 0..setup {
            host.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index % 2 == 0,
            }))
            .unwrap();
        }
        let start = Instant::now();
        owner.save_active().unwrap();
        println!(
            "FOLLOWON_INITIAL_SAVE={}",
            serde_json::json!({"row":row,"milliseconds":start.elapsed().as_secs_f64()*1000.0})
        );
        let lease = owner
            .issue_manual_lease("cost-person".into(), "cost-local".into())
            .unwrap();
        let gui = Core::spawn(Document::default()).unwrap();
        gui.restore_history(host.export_history().unwrap()).unwrap();
        let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
        let start = Instant::now();
        let permit = replica
            .bind_manual_producer(lease.clone(), owner.manual_input_context().unwrap())
            .unwrap();
        println!(
            "FOLLOWON_LEASE={}",
            serde_json::json!({"row":row,"milliseconds":start.elapsed().as_secs_f64()*1000.0})
        );
        for index in 0..12 {
            let before = gui.export_history().unwrap();
            let start = Instant::now();
            let event = permit
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index % 2 == 0,
                }))
                .unwrap();
            let edit_ms = start.elapsed().as_secs_f64() * 1000.0;
            if !matches!(event, Event::DocumentChanged { .. }) {
                println!(
                    "FOLLOWON_REFUSAL={}",
                    serde_json::json!({"row":row,"index":index,"reason":format!("{event:?}"),"milliseconds":edit_ms})
                );
                break;
            }
            let after = gui.export_history().unwrap();
            let pending = permit.pending().unwrap();
            let start = Instant::now();
            owner.commit_manual_history(&lease, &pending[0]).unwrap();
            let save_ms = start.elapsed().as_secs_f64() * 1000.0;
            replica.acknowledge_manual(&permit, pending).unwrap();
            let start = Instant::now();
            owner.save_active().unwrap();
            let quiet_save_ms = start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "FOLLOWON_EDIT_SAVE={}",
                serde_json::json!({"row":row,"index":index,"moving":before.floor_offset != after.floor_offset,"floor":after.floor_offset,"depth":after.cursor.undo.len(),"hand_edit_ms":edit_ms,"save_ms":save_ms,"quiet_save_ms":quiet_save_ms,"batches_before":before.batches.len()})
            );
        }
        let expected = gui.export_history().unwrap();
        println!(
            "FOLLOWON_CUT={}",
            serde_json::json!({"row":row,"retained_bytes":history_size::history_live_bytes(&expected).unwrap(),"floor":expected.floor_offset,"depth":expected.cursor.undo.len()})
        );
        drop(owner);
        let cold = Core::spawn(Document::default()).unwrap();
        LoadedBundle::read(&path)
            .unwrap()
            .restore_manual(&cold)
            .unwrap();
        assert_eq!(cold.export_history().unwrap(), expected);
    }
}

#[test]
fn unsaved_host_handoff_keeps_a_floored_snapshot_without_a_disk_bridge() {
    use kinewright_project::manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits};
    let temp = TempDirectory::new_in(&workspace_target_dir(), "unsaved-floored-handoff");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let limits = StorageLimits {
        branch_history_bytes: 100_000,
        ..StorageLimits::default()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
    let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
        OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
    ));
    for index in 0..30 {
        assert!(matches!(
            admission
                .request(
                    &core,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0
                    })
                )
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let expected = core.export_history().unwrap();
    assert!(expected.floor_offset > 0);
    assert!(core.ordinary_floor_source().unwrap().is_none());
    let prepared = owner.prepare_host_attachment().unwrap();
    assert_eq!(prepared.history, expected);
    let descriptor = prepared.source.as_ref().unwrap();
    let service = prepared.source_service.as_ref().unwrap();
    let binding = service
        .ensure_snapshot(descriptor, &prepared.history)
        .unwrap();
    service.prove_snapshot(descriptor, &binding).unwrap();
    let encoded =
        serde_json::to_vec(&HistoryTransport::capture(&prepared.history).unwrap()).unwrap();
    let transport: HistoryTransport = serde_json::from_slice(&encoded).unwrap();
    let reopened = transport
        .journal
        .resolve_canonical_with_budget(
            &transport.baseline,
            &transport.document,
            limits.branch_history_bytes,
            None,
        )
        .unwrap();
    assert_eq!(reopened.snapshot(), &expected);
}

#[test]
fn uncertain_save_as_keeps_the_frozen_copy_and_the_active_floor_bridge_separate() {
    use kinewright_project::{
        bundle_copy::CopyStatus,
        manual_history::{OrdinaryHistoryAdmission, OrdinaryHistoryLimits},
    };
    for landed in [false, true] {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "unknown-copy-floor");
        let path = temp.path("original.kinewright");
        let copied = temp.path("copy.kinewright");
        let limits = StorageLimits {
            branch_history_bytes: 100_000,
            ..StorageLimits::default()
        };
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner = Ja1EditingSession::new(temp.root(), &core, limits.clone()).unwrap();
        owner.first_save(&path, limits.clone()).unwrap();
        let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
            OrdinaryHistoryLimits::from_limits(&limits).unwrap(),
        ));
        for index in 0..4 {
            assert!(matches!(
                admission
                    .request(
                        &core,
                        Command::Do(Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: index % 2 == 0
                        })
                    )
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
            owner.save_active().unwrap();
        }
        let frozen = core.export_history().unwrap();
        let status = kinewright_project::io_observer::fail_once(
            if landed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            },
            || owner.save_as(&copied, limits.clone()),
        )
        .unwrap();
        assert!(matches!(status, CopyStatus::DurabilityUnknown));
        let mut floors = 0;
        for index in 0..20 {
            let before = core.export_history().unwrap();
            match admission
                .request(
                    &core,
                    Command::Do(Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0,
                    }),
                )
                .unwrap()
            {
                Event::DocumentChanged { .. } => {}
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualSaveRequired,
                    ..
                } => {
                    assert_eq!(core.export_history().unwrap(), before);
                    owner.save_active().unwrap();
                    assert!(matches!(
                        admission
                            .request(
                                &core,
                                Command::Do(Operation::SetTrackSyncLock {
                                    track: TrackId(1),
                                    locked: index % 2 == 0
                                })
                            )
                            .unwrap(),
                        Event::DocumentChanged { .. }
                    ));
                }
                other => panic!("{other:?}"),
            }
            floors +=
                usize::from(core.export_history().unwrap().floor_offset > before.floor_offset);
        }
        assert!(floors > 1);
        owner.save_active().unwrap();
        owner.reconcile_retained_destination(&copied).unwrap();
        let expected = core.export_history().unwrap();
        drop(owner);
        let cold = Core::spawn(Document::default()).unwrap();
        LoadedBundle::read_with_limits_probe(&path, limits.clone())
            .unwrap()
            .restore_manual(&cold)
            .unwrap();
        assert_eq!(cold.export_history().unwrap(), expected);
        if landed {
            let copy = Core::spawn(Document::default()).unwrap();
            LoadedBundle::read_with_limits_probe(&copied, limits)
                .unwrap()
                .restore_manual(&copy)
                .unwrap();
            assert_eq!(copy.export_history().unwrap(), frozen);
        } else {
            assert!(!copied.exists());
        }
        println!("UNKNOWN_SAVE_AS_FLOOR landed={landed} floors={floors}");
    }
}
