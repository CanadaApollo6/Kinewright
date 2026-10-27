// Original Kinewright dispatcher witnesses; no upstream source was translated.
fn s4_id<T: serde::de::DeserializeOwned>(number: u64) -> T {
    serde_json::from_value(json!(format!("{number:032x}"))).unwrap()
}

fn s4_authority() -> crate::orchestration::tasks::RuntimeTaskAuthority {
    crate::orchestration::tasks::RuntimeTaskAuthority {
        project: s4_id(5),
        run: s4_id(1),
        task: s4_id(2),
        attempt: s4_id(3),
        endpoint: s4_id(4),
        owner_epoch: 1,
        expires_at: Instant::now() + Duration::from_secs(300),
    }
}

#[cfg(target_os = "linux")]
fn s4_owner_authority(
    owner: &kinewright_project::editing_session::Ja1EditingSession,
) -> crate::orchestration::tasks::RuntimeTaskAuthority {
    let mut authority = s4_authority();
    authority.project = owner.active_manual_project().unwrap();
    authority.owner_epoch = owner.store().unwrap().owner_epoch();
    authority
}

fn s4_scope(
    base: kinewright_project::editing_session::BranchBinding,
) -> crate::orchestration::orchestration_records::TaskScope {
    use std::collections::BTreeSet;
    let strings = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<BTreeSet<_>>()
    };
    crate::orchestration::orchestration_records::TaskScope {
        operations: strings(&[
            "split_clip",
            "set_track_mix",
            "set_track_sync_lock",
            "ripple_delete_clip",
            "set_color_context",
            "add_asset",
            "relink_asset",
            "upsert_audio_bus",
        ]),
        objects: strings(&[
            "document", "track:1", "track:2", "clip:1", "clip:2", "mix:2", "bus:1",
        ]),
        tracks: strings(&["track:1", "track:2"]),
        bus_sidechain_closure: strings(&["track:1", "track:2", "bus:1"]),
        fields: strings(&[
            "track.clips",
            "track.sync_lock",
            "document.duration",
            "document.mixes",
            "document.buses",
            "document.color_context",
            "mix.track",
            "mix.gain_tenth_db",
            "mix.pan_percent",
            "mix.mute",
            "mix.solo",
            "mix.gain_curve",
            "mix.pan_curve",
            "clip.id",
            "clip.asset",
            "clip.source_range",
            "clip.content",
            "clip.timeline_start",
            "clip.effects",
            "clip.transition_in",
            "clip.link",
            "clip.audio_gain_tenth_db",
            "clip.audio_fade_in_frames",
            "clip.audio_fade_out_frames",
            "clip.audio_gain_curve",
            "clip.speed_percent",
            "clip.enabled",
            "clip.enabled_curve",
            "clip.blend_mode",
        ]),
        asset_roots: vec![],
        base,
        consent_refs: vec![],
    }
}

fn s4_handler(core: Core, endpoint: crate::orchestration::tasks::TaskEndpoint) -> KinewrightMcp {
    let media = Arc::new(NoopMedia::default());
    KinewrightMcp::new(core, media.clone(), media, ConfirmationBroker::default())
        .with_task_endpoint(endpoint)
}

fn s4_call(handler: &KinewrightMcp, name: &str, arguments: serde_json::Value) -> CallToolResult {
    handler
        .call_exposed_blocking(CallToolRequestParams::new(name.to_owned()).with_arguments(
            match arguments {
                serde_json::Value::Object(map) => map,
                _ => panic!("fixture arguments"),
            },
        ))
        .unwrap()
}

fn s4_prepare(handler: &KinewrightMcp, operations: serde_json::Value) -> CallToolResult {
    let revision = handler.snapshot().unwrap().0;
    s4_call(
        handler,
        "prepare_edit_plan",
        serde_json::Value::Object(serde_json::Map::from_iter([
            ("expected_revision".into(), json!(revision)),
            ("operations".into(), operations),
        ])),
    )
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one server-level scope and side-effect closure oracle"
)]
fn s4_role_scope_closure() {
    use crate::orchestration::{
        orchestration_records::Role, policy::TaskPolicy, tasks::TaskEndpoint,
    };
    let core = Core::spawn(Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: false,
                clips: vec![],
            },
        ],
        ..Document::default()
    })
    .unwrap();
    core.request(Command::Do(Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(0),
        duration: TimeCode(100),
        color: kinewright_core::SolidColor::default(),
    }))
    .unwrap();
    let base = kinewright_project::editing_session::BranchBinding {
        branch: s4_id(10),
        head: s4_id(11),
        checkpoint: kinewright_project::bundle_records::ObjectRef {
            sha256: "1".repeat(64),
            byte_length: 1,
            record_kind: kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
        },
        document_hash: "2".repeat(64),
    };
    let mut scope = s4_scope(base);
    scope.tracks.remove("track:1");
    scope.bus_sidechain_closure.remove("track:1");
    let endpoint = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Audio,
            scope: scope.clone(),
        },
        s4_authority(),
        &core,
    );
    let handler = s4_handler(core.clone(), endpoint.clone());
    assert!(!endpoint.is_closed_for_fixture());
    let prepared = s4_prepare(
        &handler,
        json!([{"op":"set_track_mix", "track":2, "gain_tenth_db":-60, "pan_percent":0, "mute":false, "solo":false}]),
    );
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let plan = prepared.structured_content.unwrap();
    let result = s4_call(
        &handler,
        "commit_edit_plan",
        json!({"plan_id":plan["plan_id"], "expected_revision":handler.snapshot().unwrap().0}),
    );
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_eq!(
        core.export_history().unwrap().document.audio_mix.tracks[0].gain_tenth_db,
        -60
    );
    let before = core.export_history().unwrap();
    let bypass = handler
        .apply_edit_plan(
            handler.snapshot().unwrap().0,
            &[Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(50),
            }],
        )
        .unwrap();
    assert_eq!(bypass.is_error, Some(true));
    assert_eq!(core.export_history().unwrap(), before);
    for operations in [
        json!([{"op":"set_track_sync_lock","track":2,"locked":false}]),
        json!([{"op":"set_track_mix", "track":2, "gain_tenth_db":-120, "pan_percent":0, "mute":false, "solo":false},{"op":"split_clip","clip":1,"at":50}]),
        json!([{"op":"set_track_sync_lock","track":1,"locked":false}]),
        json!([{"op":"ripple_delete_clip","clip":1}]),
        json!([{"op":"set_color_context","color_context":before.document.color_context}]),
        json!([{"op":"upsert_audio_bus","bus":{"id":1,"name":"shared","tracks":[2],"gain_tenth_db":0,"gain_curve":null,"effects":[],"ducking_sidechain_tracks":[1]}}]),
    ] {
        assert_eq!(s4_prepare(&handler, operations).is_error, Some(true));
        assert_eq!(core.export_history().unwrap(), before);
    }
    let planner = s4_call(
        &handler,
        "invoke_capability",
        json!({"name":"plan_motion","arguments":{"expected_revision":handler.snapshot().unwrap().0,"clip_id":1,"preset":"push_in","replace":false}}),
    );
    assert_eq!(planner.is_error, Some(true));
    assert!(
        planner.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("operation outside scope"),
        "{planner:?}"
    );
    let media = Arc::new(NoopMedia::default());
    let guarded = KinewrightMcp::new(
        core.clone(),
        media.clone(),
        media.clone(),
        ConfirmationBroker::default(),
    )
    .with_task_endpoint(endpoint.clone());
    let import = s4_call(
        &guarded,
        "invoke_capability",
        json!({"name":"import_media","arguments":{"expected_revision":handler.snapshot().unwrap().0,"path":"unrelated-fixture.mp4"}}),
    );
    assert_eq!(import.is_error, Some(true));
    assert!(
        media.probe_paths.lock().unwrap().is_empty(),
        "scope must refuse before probing unrelated input"
    );
    let other = Core::spawn((*before.document).clone()).unwrap();
    let mismatched = s4_handler(other, endpoint.clone());
    assert_eq!(
        s4_call(&mismatched, "get_timeline_state", json!({})).is_error,
        Some(true)
    );
    for name in [
        "import_media",
        "relink_media",
        "cancel_export",
        "delegate_task",
        "set_track_mix",
    ] {
        let result = s4_call(
            &handler,
            "invoke_capability",
            json!({"name":name,"arguments":{}}),
        );
        assert_eq!(result.is_error, Some(true));
        assert_eq!(core.export_history().unwrap(), before);
        let direct = handler
            .call_blocking(CallToolRequestParams::new(name))
            .unwrap();
        assert_eq!(direct.is_error, Some(true));
    }
    let reviewer = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Reviewer,
            scope: scope.clone(),
        },
        s4_authority(),
        &core,
    );
    let review_handler = s4_handler(core.clone(), reviewer);
    assert_eq!(s4_prepare(&review_handler, json!([{"op":"set_track_mix","track":2,"gain_tenth_db":-20,"pan_percent":0,"mute":false,"solo":false}])).is_error, Some(true));
    assert_eq!(
        s4_call(
            &review_handler,
            "invoke_capability",
            json!({"name":"delegate_task","arguments":{}})
        )
        .is_error,
        Some(true)
    );
    let pending = s4_prepare(&handler, json!([{"op":"set_track_mix","track":2,"gain_tenth_db":-40,"pan_percent":0,"mute":false,"solo":false}])).structured_content.unwrap();
    endpoint.revoke();
    assert_eq!(
        s4_call(
            &handler,
            "commit_edit_plan",
            json!({"plan_id":pending["plan_id"],"expected_revision":handler.snapshot().unwrap().0})
        )
        .is_error,
        Some(true)
    );
    assert_eq!(core.export_history().unwrap(), before);
    let mut expired = s4_authority();
    expired.expires_at = Instant::now();
    let expired = s4_handler(
        core.clone(),
        TaskEndpoint::for_fixture(
            TaskPolicy {
                role: Role::Audio,
                scope,
            },
            expired,
            &core,
        ),
    );
    assert_eq!(
        s4_call(&expired, "get_timeline_state", json!({})).is_error,
        Some(true)
    );
}

#[cfg(target_os = "linux")]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one registered owner/server drain and durable-base oracle"
)]
fn s4_drain_and_durable_base() {
    use crate::orchestration::{orchestration_records::*, tasks::EditorTask};
    use kinewright_project::{
        editing_session::Ja1EditingSession, receipts::CommandId, storage_budget::StorageLimits,
    };
    let root = kinewright_media::test_support::TempDirectory::new_in(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .unwrap()
            .as_path(),
        "s4-task",
    );
    let path = root.path("task.kinewright");
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
    let mut owner = Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
    owner.first_save(&path, StorageLimits::default()).unwrap();
    let mut candidate = owner.fork_durable_branch("candidate").unwrap();
    candidate
        .core()
        .request(Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(0),
            duration: TimeCode(100),
            color: kinewright_core::SolidColor::default(),
        }))
        .unwrap();
    let candidate_base = owner.persist_durable_branch(&mut candidate).unwrap();
    let issuer = owner
        .issue_manual_lease("s4-fixture".into(), "s4-candidate".into())
        .unwrap()
        .issuer;
    let lease = owner
        .issue_manual_lease("person".into(), "s4-manual".into())
        .unwrap();
    let permit = owner
        .bind_manual_producer(lease, owner.manual_input_context().unwrap())
        .unwrap();
    permit
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    let manual_pending = permit.pending().unwrap();
    let manual = core.export_history().unwrap();
    let base = owner.candidate_binding(candidate.branch_id()).unwrap();
    let reference = base.checkpoint.clone();
    let brief = TaskBrief {
        plan: reference.clone(),
        script: reference.clone(),
        inputs: reference.clone(),
        assignment: reference,
        role: Role::Editor,
        provider: ProviderSelectionRecord {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        },
        scope: s4_scope(base.clone()),
    };
    let before_refusal = owner.root_sha256().unwrap();
    let mut expired = s4_owner_authority(&owner);
    expired.expires_at = Instant::now();
    assert!(EditorTask::prepare_fixture(&mut owner, brief.clone(), expired).is_err());
    let mut stale = s4_owner_authority(&owner);
    stale.owner_epoch += 1;
    assert!(EditorTask::prepare_fixture(&mut owner, brief.clone(), stale).is_err());
    assert_eq!(owner.root_sha256().unwrap(), before_refusal);
    let mut foreign = s4_owner_authority(&owner);
    foreign.project = s4_id(999);
    assert!(EditorTask::prepare_fixture(&mut owner, brief.clone(), foreign).is_err());
    assert_eq!(owner.root_sha256().unwrap(), before_refusal);
    let authority = s4_owner_authority(&owner);
    let mut task = EditorTask::prepare_fixture(&mut owner, brief.clone(), authority).unwrap();
    let handler = s4_handler(task.core().clone(), task.endpoint());
    let prepared = s4_prepare(&handler, json!([{"op":"split_clip","clip":1,"at":50}]));
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let plan = prepared.structured_content.unwrap();
    let plan_id = plan["plan_id"].clone();
    let result = s4_call(
        &handler,
        "commit_edit_plan",
        json!({"plan_id":plan_id,"expected_revision":handler.snapshot().unwrap().0}),
    );
    assert_eq!(result.is_error, Some(false), "{result:?}");
    task.core().request(Command::Undo).unwrap();
    task.core().request(Command::Redo).unwrap();
    assert!(handler.snapshot().unwrap().0.0 > 0);
    task.core().request(Command::Undo).unwrap();
    let prepared = s4_prepare(&handler, json!([{"op":"split_clip","clip":1,"at":50}]))
        .structured_content
        .unwrap();
    let revision = handler.snapshot().unwrap().0;
    let endpoint = task.endpoint();
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let mut admitted_handler = handler.clone();
    let (inside, resume) = (entered.clone(), release.clone());
    admitted_handler.task_admission_hook = Some(Arc::new(move || {
        inside.wait();
        resume.wait();
    }));
    let admitted_call = std::thread::spawn(move || {
        admitted_handler
            .call_exposed_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({"plan_id":prepared["plan_id"],"expected_revision":revision})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap()
    });
    entered.wait();
    let freezing = std::thread::spawn(move || {
        let frozen = task.freeze(&mut owner).unwrap().clone();
        (task, owner, frozen)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !endpoint.is_closed_for_fixture() {
        assert!(Instant::now() < deadline, "freeze did not close ingress");
        std::thread::yield_now();
    }
    std::thread::sleep(Duration::from_secs(1));
    let froze_before_release = freezing.is_finished();
    release.wait();
    let result = admitted_call.join().unwrap();
    let (task, owner, frozen) = freezing.join().unwrap();
    assert!(
        !froze_before_release,
        "freeze must wait for production admission"
    );
    assert_eq!(
        result.is_error,
        Some(false),
        "admitted work drains after closure: {result:?}"
    );
    assert_eq!(
        frozen.delta().len(),
        1,
        "only the currently applied split survives Undo/Redo"
    );
    assert_eq!(
        s4_call(
            &handler,
            "commit_edit_plan",
            json!({"plan_id":plan_id,"expected_revision":handler.snapshot().unwrap().0})
        )
        .is_error,
        Some(true)
    );
    assert!(matches!(
        task.core().request(Command::Redo).unwrap(),
        Event::MutationRefused { .. }
    ));
    assert_eq!(core.export_history().unwrap(), manual);
    assert_eq!(permit.pending().unwrap(), manual_pending);
    let source = frozen.source().branch.clone();
    let source_base = frozen.base_ref().clone();
    drop(task);
    drop(handler);
    drop(owner);
    let mut owner = Ja1EditingSession::open(&path).unwrap();
    let candidate = owner
        .reopen_registered_candidate(candidate.branch_id().clone(), &candidate_base)
        .unwrap();
    let mut authority = s4_owner_authority(&owner);
    authority.expires_at = Instant::now() + Duration::from_secs(3600);
    let mut reopened = EditorTask::reopen(
        &mut owner,
        brief.clone(),
        authority,
        source,
        &source_base,
        None,
    )
    .unwrap();
    assert_eq!(reopened.freeze(&mut owner).unwrap().delta().len(), 1);
    let mut wrong = base.clone();
    wrong.head = s4_id(999);
    assert!(
        owner
            .fork_registered_from_candidate(&wrong, "same counter/different head")
            .is_err()
    );
    let id = CommandId {
        issuer,
        sequence: 1,
    };
    let receipt = reopened.merge(&mut owner, id.clone()).unwrap();
    reopened.endpoint().revoke();
    assert_eq!(reopened.merge(&mut owner, id.clone()).unwrap(), receipt);
    let mut changed = id;
    changed.sequence += 1;
    assert!(reopened.merge(&mut owner, changed).is_err());
    let history = owner.durable_branch_history(&candidate).unwrap();
    assert_eq!(history.document.tracks[0].clips.len(), 2);
    assert_eq!(history.cursor.undo.len(), 2);
    drop(history);
    let registrations = owner.durable_sessions().len();
    let retired_core = reopened.core().clone();
    reopened.close(&mut owner).unwrap();
    assert_eq!(owner.durable_sessions().len(), registrations - 1);
    assert_eq!(
        retired_core.attachment_state().unwrap(),
        kinewright_core::AttachmentState::Sealed
    );
    assert!(
        owner
            .fork_registered_from_candidate(&base, "changed durable target")
            .is_err()
    );
    let mut invalid_brief = brief;
    invalid_brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
    invalid_brief.role = Role::Audio;
    let authority = s4_owner_authority(&owner);
    let mut invalid_task =
        EditorTask::prepare_fixture(&mut owner, invalid_brief, authority).unwrap();
    invalid_task
        .core()
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    invalid_task.freeze(&mut owner).unwrap();
    let issuer = owner
        .issue_manual_lease("second-issuer".into(), "second-private".into())
        .unwrap()
        .issuer;
    let before_refusal = owner.root_sha256().unwrap();
    assert!(
        invalid_task
            .merge(
                &mut owner,
                CommandId {
                    issuer,
                    sequence: 1
                }
            )
            .is_err()
    );
    assert_eq!(owner.root_sha256().unwrap(), before_refusal);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "H3 resolved attachment dispatch and C2 contained connector integration are not merged"]
#[allow(
    clippy::too_many_lines,
    reason = "one deferred Editor-to-Audio integration scenario"
)]
fn s4_editor_result_to_candidate() {
    use crate::orchestration::{orchestration_records::*, tasks::EditorTask};
    use kinewright_project::{
        editing_session::Ja1EditingSession, receipts::CommandId, storage_budget::StorageLimits,
    };
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()
        .unwrap();
    let root =
        kinewright_media::test_support::TempDirectory::new_in(&directory, "s4-editor-result");
    let path = root.path("studio.kinewright");
    let manual = Core::spawn(Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: false,
                clips: vec![],
            },
        ],
        ..Document::default()
    })
    .unwrap();
    let mut owner = Ja1EditingSession::new(root.root(), &manual, StorageLimits::default()).unwrap();
    owner.first_save(&path, StorageLimits::default()).unwrap();
    let mut candidate = owner.fork_durable_branch("candidate").unwrap();
    candidate
        .core()
        .request(Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(0),
            duration: TimeCode(100),
            color: kinewright_core::SolidColor::default(),
        }))
        .unwrap();
    owner.persist_durable_branch(&mut candidate).unwrap();
    let issuer = owner
        .issue_manual_lease("run".into(), "run-private".into())
        .unwrap()
        .issuer;
    let lease = owner
        .issue_manual_lease("person".into(), "person-private".into())
        .unwrap();
    let permit = owner
        .bind_manual_producer(lease, owner.manual_input_context().unwrap())
        .unwrap();
    permit
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap();
    let manual_history = manual.export_history().unwrap();
    let manual_pending = permit.pending().unwrap();
    let manual_attachment = owner.current_switch_attachment().unwrap();
    let flattened: Document = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (index, (role, operations)) in [
        (Role::Editor,json!([{"op":"split_clip","clip":1,"at":50}])),
        (Role::Audio,json!([{"op":"set_track_mix","track":2,"gain_tenth_db":-60,"pan_percent":0,"mute":false,"solo":false}])),
    ].into_iter().enumerate() {
        let base = owner.candidate_binding(candidate.branch_id()).unwrap();
        let reference = base.checkpoint.clone();
        let brief = TaskBrief { plan:reference.clone(),script:reference.clone(),inputs:reference.clone(),assignment:reference,
            role,provider:ProviderSelectionRecord {connector:"scripted".into(),model:"fixture".into(),options:vec![]},scope:s4_scope(base.clone()) };
        let mut authority = s4_owner_authority(&owner);
        authority.task=s4_id(20+index as u64); authority.attempt=s4_id(30+index as u64);authority.endpoint=s4_id(40+index as u64);
        let mut task=EditorTask::prepare_fixture(&mut owner,brief,authority).unwrap();
        let handler=s4_handler(task.core().clone(),task.endpoint());
        let prepared=s4_prepare(&handler,operations);
        assert_eq!(prepared.is_error,Some(false),"{prepared:?}");
        let plan=prepared.structured_content.unwrap();
        let applied=s4_call(&handler,"commit_edit_plan",json!({"plan_id":plan["plan_id"],"expected_revision":handler.snapshot().unwrap().0}));
        assert_eq!(applied.is_error,Some(false),"{applied:?}");
        let frozen=task.freeze(&mut owner).unwrap().clone();
        assert_eq!(frozen.base(),&base);
        let reopened=kinewright_project::bundle::LoadedBundle::read(&path).unwrap();
        assert_eq!(reopened.root().unwrap().manual_branch_id,manual_attachment.current.branch);
        drop(reopened);
        let id=CommandId {issuer:issuer.clone(),sequence:index as u64+1};
        task.merge(&mut owner,id.clone()).unwrap();
        let original=task.original_merge_envelope().unwrap().clone();
        let receipt=task.merge(&mut owner,id).unwrap();
        assert_eq!(task.original_merge_envelope().unwrap(),&original);
        assert!(matches!(receipt,kinewright_project::receipts::ReceiptLookup::Committed(_)));
        let history=owner.durable_branch_history(&candidate).unwrap();
        assert_eq!(history.cursor.undo.len(),index+2);
        assert_eq!(history.document.tracks[0].clips.len(),2);
        if role==Role::Audio {assert_eq!(history.document.audio_mix.tracks[0].gain_tenth_db,-60);}
        drop(history);
        assert_eq!(manual.export_history().unwrap(),manual_history);
        assert_eq!(permit.pending().unwrap(),manual_pending);
        assert_eq!(owner.current_switch_attachment().unwrap().current,manual_attachment.current);
        assert_eq!(serde_json::from_slice::<Document>(&std::fs::read(&path).unwrap()).unwrap(),flattened);
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "two scope refusals bind audible collateral to exact Core history"
)]
fn s4_solo_removal_and_director_routing() {
    use crate::orchestration::{
        orchestration_records::Role, policy::TaskPolicy, tasks::TaskEndpoint,
    };
    let mut document = mo1_motion_document(60, Vec::new());
    document.media_pool[0].kind = MediaKind::AudioVideo;
    document.tracks[0].clips[0].content = kinewright_core::ClipContent::Media;
    kinewright_core::apply_batch(
        &mut document,
        &[
            Operation::AddTrack {
                track: Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: false,
                    clips: vec![],
                },
            },
            Operation::AddClip {
                track: TrackId(2),
                asset: AssetId(1),
                at: TimeCode(0),
                source: TimeCode(0)..TimeCode(60),
            },
            Operation::SetTrackMix {
                track: TrackId(2),
                gain_tenth_db: 0,
                pan_percent: 0,
                mute: false,
                solo: true,
            },
        ],
    )
    .unwrap();
    assert!(!document.track_audible(TrackId(1)));
    let mut removed = document.clone();
    kinewright_core::apply_batch(
        &mut removed,
        &[Operation::RemoveTrack { track: TrackId(2) }],
    )
    .unwrap();
    assert!(
        removed.track_audible(TrackId(1)),
        "removing the sole soloed track unmutes a surviving track"
    );
    let core = Core::spawn(document).unwrap();
    let base = kinewright_project::editing_session::BranchBinding {
        branch: s4_id(10),
        head: s4_id(11),
        checkpoint: kinewright_project::bundle_records::ObjectRef {
            sha256: "1".repeat(64),
            byte_length: 1,
            record_kind: kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
        },
        document_hash: "2".repeat(64),
    };
    let mut scope = s4_scope(base);
    scope
        .operations
        .extend(["remove_track", "set_clip_audio"].map(str::to_owned));
    scope.objects.insert("asset:1".into());
    scope
        .fields
        .extend(["document.tracks", "track.id", "track.kind"].map(str::to_owned));
    let full = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Editor,
            scope: scope.clone(),
        },
        s4_authority(),
        &core,
    );
    let full = s4_handler(core.clone(), full);
    let control = s4_prepare(&full, json!([{"op":"remove_track","track":2}]));
    assert_eq!(control.is_error, Some(false), "{control:?}");
    let mut narrow = scope.clone();
    narrow.tracks.remove("track:1");
    narrow.bus_sidechain_closure.remove("track:1");
    let narrow = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Editor,
            scope: narrow,
        },
        s4_authority(),
        &core,
    );
    let narrow = s4_handler(core.clone(), narrow);
    let before = core.export_history().unwrap();
    assert_eq!(
        s4_prepare(&narrow, json!([{"op":"remove_track","track":2}])).is_error,
        Some(true)
    );
    assert_eq!(core.export_history().unwrap(), before);
    let mut director = scope;
    director.bus_sidechain_closure.clear();
    let endpoint = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Director,
            scope: director.clone(),
        },
        s4_authority(),
        &core,
    );
    let handler = s4_handler(core.clone(), endpoint);
    let operations = json!([{"op":"set_clip_audio","clip":1,"gain_tenth_db":-20,"fade_in_frames":0,"fade_out_frames":0}]);
    assert_eq!(
        s4_prepare(&handler, operations.clone()).is_error,
        Some(true)
    );
    assert_eq!(core.export_history().unwrap(), before);
    director.bus_sidechain_closure.insert("track:1".into());
    let endpoint = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Director,
            scope: director,
        },
        s4_authority(),
        &core,
    );
    let handler = s4_handler(core.clone(), endpoint);
    let result = s4_prepare(&handler, operations);
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let plan = result.structured_content.unwrap();
    assert_eq!(
        s4_call(
            &handler,
            "commit_edit_plan",
            json!({"plan_id":plan["plan_id"],"expected_revision":handler.snapshot().unwrap().0})
        )
        .is_error,
        Some(false)
    );
    assert_eq!(
        core.export_history()
            .unwrap()
            .document
            .clip(ClipId(1))
            .unwrap()
            .audio_gain_tenth_db,
        -20
    );
}
