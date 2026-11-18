fn capacity_run(clips: usize) -> C5Fix10Run {
    capacity_run_document(&capacity_document(clips), Role::Editor)
}
fn capacity_run_document(document: &Document, role: Role) -> C5Fix10Run {
    capacity_run_document_with_objects(document, role, BTreeSet::new())
}
fn capacity_run_document_with_objects(
    document: &Document,
    role: Role,
    objects: BTreeSet<String>,
) -> C5Fix10Run {
    let clips = document
        .tracks
        .iter()
        .map(|track| track.clips.len())
        .sum::<usize>();
    let mut f = Fixture::with_document(false, document.clone());
    let mut scope = crate::orchestration::policy::director_scope(
        &Document::default(),
        f.brief.scope.base.clone(),
    )
    .unwrap();
    scope.objects.extend(objects);
    for track in &document.tracks {
        let name = format!("track:{}", track.id);
        scope.objects.insert(name.clone());
        scope.tracks.insert(name.clone());
        scope.bus_sidechain_closure.insert(name);
    }
    for track in 1..=20.min(clips / 10) {
        scope.objects.insert(format!("clip:{}", track * 100 + 10));
    }
    if role == Role::Colour {
        for clip in document.tracks.iter().flat_map(|track| &track.clips) {
            scope.objects.insert(format!("clip:{}", clip.id));
            scope.objects.insert(format!("effect:{}:1", clip.id));
        }
        scope
            .fields
            .insert("effect.parameters.exposure_milli_stops".into());
        scope
            .fields
            .insert("effect.keyframes.exposure_milli_stops".into());
    }
    f.brief.scope = scope;
    let (run, parent, producer) = reserve(&f);
    let (auth, _) = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .prepare_task_endpoint(
            &producer.bootstrap.auth,
            &run,
            &parent.task,
            &parent.attempt,
            Duration::from_secs(3600),
        )
        .unwrap();
    let _parent_client =
        HostClient::connect(f.host.endpoint.clone(), auth, f.host.roots.clone()).unwrap();
    let director = C4Mcp::new(&f, &run, &parent.task);
    let mut brief = f.brief.clone();
    brief.role = role;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (attempt, client, endpoint, core) =
        c4_reserve_task_with_ttl(&f, &run, &child, Duration::from_secs(3600));
    history_turn(&f, &attempt, &client, &endpoint, 1, true);
    let editor = C4Mcp::new(&f, &run, &child);
    C5Fix10Run {
        f,
        run,
        parent,
        child,
        attempt,
        director,
        editor,
        core,
    }
}
fn history_admission_native_copy_run() -> C5Fix10Run {
    use kinewright_core::{
        AutomationCurve, ClipId, Effect, EffectId, Keyframe, KeyframeInterpolation, ParamValue,
        TimeCode,
    };
    let mut document = capacity_document(10);
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(1),
        name: "primary_correction".into(),
        enabled: true,
        enabled_curve: None,
        parameters: std::collections::BTreeMap::from([(
            "exposure_milli_stops".into(),
            ParamValue::Integer(0),
        )]),
        keyframes: std::collections::BTreeMap::from([(
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
                    .collect::<Vec<_>>(),
            },
        )]),
    });
    let r = capacity_run_document(&document, Role::Colour);
    for operations in [
        (102..=110)
            .map(|id| kinewright_core::Operation::CopyClipAttributes {
                from_clip: ClipId(101),
                to_clip: ClipId(id),
                names: None,
                include_keyframes: true,
            })
            .collect::<Vec<_>>(),
        (102..=110)
            .map(|id| kinewright_core::Operation::RemoveEffect {
                clip: ClipId(id),
                effect: EffectId(1),
            })
            .collect::<Vec<_>>(),
    ] {
        crate::orchestration::policy::TaskPolicy {
            role: Role::Colour,
            scope: r.f.brief.scope.clone(),
        }
        .validate_batch(
            &r.core.export_history().unwrap().history().document,
            &operations,
        )
        .expect("COPY_WITNESS_HAS_THE_REAL_COLOUR_ROLE_AND_EFFECT_GRANTS");
        let event = r
            .core
            .request(kinewright_core::Command::DoBatch(operations))
            .unwrap();
        assert!(
            matches!(event, kinewright_core::Event::DocumentChanged { .. }),
            "{event:?}"
        );
    }
    capacity_check_task(&r.f, &r.core);
    r
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn history_admission_native_copy_remove_finish_crash_reopen() {
    capacity_lifecycle_crash_reopen_row(10);
}

// This helper is invoked by the slow parent in a separate process. With no
// handoff path it does no work in ordinary libtest runs.
#[test]
fn history_capacity_crash_child() {
    let Ok(handoff) = std::env::var("KINEWRIGHT_CAPACITY_HANDOFF") else {
        return;
    };
    let clips: usize = std::env::var("KINEWRIGHT_CAPACITY_CLIPS")
        .unwrap()
        .parse()
        .unwrap();
    if clips == 10 {
        capacity_finish_merge_stop(
            history_admission_native_copy_run(),
            Some(std::path::Path::new(&handoff)),
        );
        return;
    }
    let r = capacity_run(clips);
    let edits_started = Instant::now();
    let edits = if clips == 1200 { 68 } else { 200 };
    for edit in 0..edits {
        let reply = capacity_edit(&r.editor, &r.core, clips, edit);
        assert_eq!(
            reply.is_error,
            Some(false),
            "CAPACITY_ACCEPTED {clips}/{edit}: {reply:?}"
        );
        if (edit + 1).is_multiple_of(25) {
            println!(
                "CAPACITY_EDIT_PROGRESS clips={clips} accepted={} seconds={:.3}",
                edit + 1,
                edits_started.elapsed().as_secs_f64()
            );
        }
    }
    assert_eq!(r.core.export_history().unwrap().history().undo.len(), edits);
    if clips == 1200 {
        let before = r.core.export_history().unwrap();
        let refused = capacity_edit(&r.editor, &r.core, clips, edits);
        assert_eq!(refused.is_error, Some(true));
        assert!(
            c5_fix10_text(&refused).contains(
                "Task history limit reached. Finish or freeze this task before making more edits."
            ),
            "{refused:?}"
        );
        assert_eq!(
            r.core.export_history().unwrap(),
            before,
            "CAPACITY_REFUSAL_UNCHANGED"
        );
    }
    let undone = r.editor.wire(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(
        undone.is_error,
        Some(false),
        "CAPACITY_KEEP_REDO {undone:?}"
    );
    assert_eq!(r.core.export_history().unwrap().history().redo.len(), 1);
    capacity_finish_merge_stop(r, Some(std::path::Path::new(&handoff)));
}

fn capacity_edit(
    editor: &C4Mcp,
    core: &Core,
    clips: usize,
    edit: usize,
) -> rmcp::model::CallToolResult {
    let revision = grant_test_revision(core);
    let operations = if clips == 400 && edit == 100 {
        (1..=20)
            .map(|track| serde_json::json!({"op":"ripple_delete_clip", "clip":track * 100 + 10}))
            .collect::<Vec<_>>()
    } else {
        vec![
            serde_json::json!({"op":"set_track_sync_lock", "track":1,"locked":edit.is_multiple_of(2)}),
        ]
    };
    let prepared = editor.raw(
        "prepare_edit_plan",
        serde_json::json!({"expected_revision":revision,"operations":operations}),
    );
    if prepared.is_error == Some(true) {
        return prepared;
    }
    editor.raw("commit_edit_plan", serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}))
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn history_capacity_full_lifecycle_crash_reopen() {
    capacity_lifecycle_crash_reopen_row(400);
}

#[test]
#[ignore = "local measurement: 1200 clips/68 edits through real tools, crash and exact reopen; timed separately from the required 400/200 CI witness"]
fn history_capacity_1200_lifecycle_crash_reopen() {
    capacity_lifecycle_crash_reopen_row(1200);
}

fn capacity_lifecycle_crash_reopen_row(clips: usize) {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "capacity-crash-parent",
    );
    let handoff = temp.path(&format!("handoff-{clips}.json"));
    let log = temp.path(&format!("child-{clips}.log"));
    let started = Instant::now();
    let executable = std::env::current_exe().unwrap();
    let directory = std::env::current_dir().unwrap();
    let mut child = std::process::Command::new(&executable)
        .current_dir(&directory)
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::history_capacity_crash_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("KINEWRIGHT_CAPACITY_HANDOFF", &handoff)
        .env("KINEWRIGHT_CAPACITY_CLIPS", clips.to_string())
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::options().append(true).open(&log).unwrap())
        .spawn()
        .unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(500) {
            let proc = std::path::PathBuf::from(format!("/proc/{}", child.id()));
            assert_eq!(std::fs::read_link(proc.join("cwd")).unwrap(), directory);
            let argv = std::fs::read(proc.join("cmdline")).unwrap();
            assert_eq!(
                argv.split(|byte| *byte == 0).next().unwrap(),
                executable.as_os_str().as_encoded_bytes()
            );
            assert!(
                argv.windows(b"history_capacity_crash_child".len())
                    .any(|part| part == b"history_capacity_crash_child")
            );
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "CAPACITY_CHILD_TIMEOUT clips={clips}; owned child terminated and reaped: {}",
                std::fs::read_to_string(&log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        status.code(),
        Some(86),
        "CAPACITY_REAL_CRASH clips={clips}: {}",
        std::fs::read_to_string(&log).unwrap()
    );
    println!(
        "CAPACITY_CHILD_LOG clips={clips} {}",
        std::fs::read_to_string(log).unwrap()
    );
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
    let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
    assert!(root.starts_with(kinewright_media::test_support::workspace_target_dir()));
    let samples: usize =
        std::env::var("KINEWRIGHT_CAPACITY_OPEN_SAMPLES").map_or(1, |value| value.parse().unwrap());
    assert!((1..=6).contains(&samples));
    for _ in 0..samples {
        capacity_check_reopen(&data);
    }
    std::fs::remove_dir_all(root).unwrap();
    println!(
        "CAPACITY_LIFECYCLE clips={clips} edits={} seconds={:.3}",
        if clips == 400 {
            200
        } else if clips == 10 {
            2
        } else {
            68
        },
        started.elapsed().as_secs_f64()
    );
}

fn capacity_next_editor(r: &C5Fix10Run) -> (TaskId, C4Mcp, Core) {
    let mut brief = r.f.brief.clone();
    brief.role = Role::Editor;
    r.director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: r.run.clone(),
            parent: r.parent.task.clone(),
            brief,
        })),
    );
    let child = r.f.projection(&r.run).tasks.last().unwrap().id.clone();
    let (execution, actor, endpoint, core) =
        c4_reserve_task_with_ttl(&r.f, &r.run, &child, Duration::from_secs(3600));
    let state = r.f.projection(&r.run);
    let head = state
        .tasks
        .iter()
        .find(|task| task.id == child)
        .unwrap()
        .brief
        .scope
        .base
        .head
        .clone();
    let turn = TaskTurn {
        task: child.clone(),
        attempt: execution.attempt.clone(),
        session: execution.nonce.clone(),
        ordinal: 1,
        owner_epoch: endpoint.authority().owner_epoch,
        expected_head: head,
        purpose: TurnPurpose::Work,
    };
    let request = task_request(
        &r.f,
        &actor,
        &endpoint,
        OrchestrationCommand::BeginTurn(turn),
    );
    ok(
        actor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "CAPACITY_NEXT_TURN",
    );
    (child.clone(), C4Mcp::new(&r.f, &r.run, &child), core)
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "all four fixture sizes through accepted and refused candidate merges"
)]
fn history_capacity_candidate_refusal_does_not_latch() {
    use kinewright_core::{Command, Event, Marker, MarkerId, Operation, TimeCode, TrackId};
    // Keep the original short edit stream and pressure the new byte room with
    // two explicit payloads. The cleanup test separately fills all task steps.
    // One seven-digit-ID marker has 79 literal JSON header bytes plus its label.
    const LABEL_BYTES: usize = 190_000;
    const PAYLOAD_BYTES: u64 = 380_158;
    let mut observed = Vec::new();
    let maximum_merges =
        kinewright_core::undo::RoleBounds::CANDIDATE.operation_bytes / PAYLOAD_BYTES;
    assert_eq!(
        maximum_merges, 2,
        "CAPACITY_LITERAL_PAYLOAD_ROOM_WITH_UNCHANGED_PROFILE"
    );
    for clips in [40, 120, 400, 1200] {
        let document = capacity_document(clips);
        let r = capacity_run_document_with_objects(
            &document,
            Role::Editor,
            (1_000_001..=1_000_006)
                .map(|id| format!("marker:{id}"))
                .collect(),
        );
        let edits: usize = if clips == 1200 { 68 } else { 200 };
        let mut accepted = 0;
        loop {
            let (child, editor, core) = if accepted == 0 {
                (
                    r.child.clone(),
                    C4Mcp::new(&r.f, &r.run, &r.child),
                    r.core.clone(),
                )
            } else {
                capacity_next_editor(&r)
            };
            // Build the task with accepted Core commands, then check native
            // admission before publication. The lifecycle test covers every
            // prepare/commit consent call.
            for edit in 0..edits {
                let operations = if clips == 400 && accepted == 0 && edit == 100 {
                    (1..=20)
                        .map(|track| Operation::RippleDeleteClip {
                            clip: kinewright_core::ClipId(track * 100 + 10),
                        })
                        .collect()
                } else {
                    vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: edit.is_multiple_of(2),
                    }]
                };
                let event = core.request(Command::DoBatch(operations)).unwrap();
                assert!(
                    matches!(event, Event::DocumentChanged { .. }),
                    "CAPACITY_CANDIDATE_EDIT {event:?}"
                );
            }
            let payload = (1..=2)
                .map(|offset| Operation::AddMarker {
                    marker: Marker {
                        id: MarkerId(1_000_000 + accepted as u64 * 2 + offset),
                        position: TimeCode(0),
                        label: "x".repeat(LABEL_BYTES),
                        color_token: 0,
                    },
                })
                .collect::<Vec<_>>();
            assert_eq!(
                serde_json::to_vec(&payload).unwrap().len(),
                380_161,
                "CAPACITY_PAYLOAD_HAS_LITERAL_JSON_BYTES"
            );
            {
                let before = core.export_history().unwrap();
                crate::orchestration::policy::TaskPolicy {
                    role: Role::Editor,
                    scope: r.f.brief.scope.clone(),
                }
                .validate_batch(&before.history().document, &payload)
                .expect("CAPACITY_PAYLOAD_HAS_THE_ACTUAL_EDITOR_GRANTS");
            }
            assert!(matches!(
                core.request(Command::DoBatch(payload)).unwrap(),
                Event::DocumentChanged { .. }
            ));
            capacity_check_task(&r.f, &core);
            let finish_control = te13_control(&editor, &child, &core);
            let finished = editor.wire("finish_task", finish_control);
            assert_eq!(
                finished.is_error,
                Some(false),
                "CAPACITY_CANDIDATE_FINISH {finished:?}"
            );
            let published = r.f.projection(&r.run);
            let TaskState::Terminal(result_ref) = &published
                .tasks
                .iter()
                .find(|task| task.id == child)
                .unwrap()
                .state
            else {
                panic!("CAPACITY_FINISH_PUBLISHED {finished:?}");
            };
            let result_ref = result_ref.clone();
            let frozen = c5_fix10_stored_result(&r.f, &result_ref);
            let mut before_state = r.f.projection(&r.run);
            let (before, before_actor) = {
                let owner = r.f.host.owner.upgrade().unwrap();
                let host = owner.lock().unwrap();
                let owner = host.owner().unwrap();
                (
                    owner
                        .candidate_binding(&r.f.brief.scope.base.branch)
                        .unwrap(),
                    capacity_candidate_actor(owner, &r.f.brief.scope.base.branch),
                )
            };
            assert_eq!(
                before_actor.0.history().policy,
                kinewright_core::undo::RetentionPolicy::NeverFold,
                "CAPACITY_CANDIDATE_NEVER_FOLDS"
            );
            assert_eq!(
                before_actor.0.history().undo.len(),
                accepted,
                "CAPACITY_EVERY_ACCEPTED_MERGE_STAYS_RETAINED"
            );
            let merged = r.director.wire(
                "merge_task",
                r.director
                    .control(OrchestrationCommand::MergeTask(MergeTask {
                        task: child.clone(),
                        target: frozen.base.clone().unwrap(),
                        result: result_ref.clone(),
                    })),
            );
            if merged.is_error == Some(true) {
                assert!(
                    accepted > 0,
                    "CAPACITY_FRESH_CANDIDATE_ACCEPTS_ITS_ADMITTED_SOURCE"
                );
                assert_eq!(
                    u64::try_from(accepted).unwrap(),
                    maximum_merges,
                    "CAPACITY_LITERAL_MERGE_COUNT {clips}"
                );
                observed.push((clips, accepted));
                assert!(c5_fix10_text(&merged).contains("Candidate history limit reached. Keep this result frozen and use publish_result to report the limit."), "{merged:?}");
                let state = r.f.projection(&r.run);
                // The existing tool-accounting record includes refused calls.
                // Every control/task/result transition must remain unchanged.
                before_state.usage.clone_from(&state.usage);
                assert_eq!(state, before_state, "CAPACITY_NO_RUN_TRANSITION");
                let owner = r.f.host.owner.upgrade().unwrap();
                let host = owner.lock().unwrap();
                assert_eq!(
                    host.owner()
                        .unwrap()
                        .candidate_binding(&r.f.brief.scope.base.branch)
                        .unwrap(),
                    before,
                    "CAPACITY_CANDIDATE_UNCHANGED"
                );
                assert_eq!(
                    capacity_candidate_actor(host.owner().unwrap(), &r.f.brief.scope.base.branch),
                    before_actor,
                    "CAPACITY_CANDIDATE_CORE_UNCHANGED"
                );
                assert!(
                    !host.owner().unwrap().store().unwrap().run_limit_reached(),
                    "CAPACITY_NO_LATCH"
                );
                assert!(
                    !host.owner().unwrap().pending_agent_command(),
                    "CAPACITY_NO_PENDING"
                );
                assert_eq!(state.outcome, RunOutcome::Pending);
                assert!(state.results.contains(&result_ref));
                let saved = host.saved_task_history(&state, &child).unwrap().unwrap();
                capacity_candidate_metrics(
                    host.owner().unwrap(),
                    &before,
                    &frozen,
                    clips,
                    accepted,
                );
                assert!(saved.frozen);
                assert!(saved.merge_envelope.is_none(), "CAPACITY_NO_STAGED_MERGE");
                assert!(
                    saved.merge_authority.is_none(),
                    "CAPACITY_NO_RETAINED_CALLER"
                );
                drop(host);
                capacity_retire_finished(&r, &child);
                let retained = cleanup_history(&r.f, &r.run, &child);
                assert_eq!(
                    retained.history().undo.len(),
                    edits + 1,
                    "CAPACITY_REFUSED_SOURCE_REOPENS_EVERY_ACCEPTED_COMMAND"
                );
                for offset in 1..=2 {
                    let id = MarkerId(1_000_000 + accepted as u64 * 2 + offset);
                    let marker = retained
                        .history()
                        .document
                        .markers
                        .iter()
                        .find(|marker| marker.id == id)
                        .expect("CAPACITY_REFUSED_SOURCE_REOPENS_THE_ACTUAL_PAYLOAD");
                    assert_eq!(marker.label.len(), LABEL_BYTES);
                    assert!(
                        marker.label.bytes().all(|byte| byte == b'x'),
                        "CAPACITY_REFUSED_SOURCE_REOPENS_EVERY_PAYLOAD_BYTE"
                    );
                }
                drop(retained);
                // Finish includes Freeze and retires that endpoint. Use
                // fresh siblings to prove each available operation after the
                // refusal, without sending new commands to a terminal task.
                let (probe, probe_editor, probe_core) = capacity_next_editor(&r);
                capacity_probe_edit(&probe_core);
                let finished = probe_editor.wire(
                    "finish_task",
                    te13_control(&probe_editor, &probe, &probe_core),
                );
                assert_eq!(
                    finished.is_error,
                    Some(false),
                    "CAPACITY_REFUSAL_FINISH_STILL_AVAILABLE {finished:?}"
                );
                assert!(
                    matches!(
                        r.f.projection(&r.run)
                            .tasks
                            .iter()
                            .find(|task| task.id == probe)
                            .unwrap()
                            .state,
                        TaskState::Terminal(_)
                    ),
                    "CAPACITY_PROBE_FINISH_COMPLETED {finished:?}"
                );
                capacity_retire_finished(&r, &probe);
                let (probe, probe_editor, probe_core) = capacity_next_editor(&r);
                capacity_probe_edit(&probe_core);
                let frozen_probe = c5_fix10_freeze(&probe_editor, &probe);
                assert!(
                    frozen_probe.head.is_some(),
                    "CAPACITY_REFUSAL_FREEZE_STILL_AVAILABLE"
                );
                println!(
                    "CAPACITY_CANDIDATE clips={clips} edits_per_task={edits} accepted_merges={accepted} refusal_non_latching=true"
                );
                break;
            }
            assert_eq!(merged.is_error, Some(false), "CAPACITY_MERGE {merged:?}");
            accepted += 1;
            assert!(
                u64::try_from(accepted).unwrap() <= maximum_merges,
                "CAPACITY_REFUSES_BY_LITERAL_ENCODED_BUDGET"
            );
            capacity_retire_finished(&r, &child);
        }
        capacity_stop(&r.f, &r.run);
    }
    assert_eq!(
        observed.iter().map(|(clips, _)| *clips).collect::<Vec<_>>(),
        [40, 120, 400, 1200],
        "CAPACITY_ALL_FOUR_DOCUMENT_FIXTURES_EXERCISED"
    );
}

fn capacity_candidate_metrics(
    owner: &Ja1EditingSession,
    before: &BranchBinding,
    result: &TaskResult,
    clips: usize,
    accepted: usize,
) {
    let store = owner.store().unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(&before.checkpoint)
        .unwrap();
    let retained = checkpoint
        .open(
            store,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .into_reader()
        .unwrap()
        .into_capture()
        .unwrap();
    let delta: Vec<kinewright_core::Operation> = serde_json::from_slice(
        &store
            .read_evidence(result.delta_ref.as_ref().unwrap())
            .unwrap(),
    )
    .unwrap();
    let charge = kinewright_core::undo::batch_charge(&delta).unwrap();
    let steps: Vec<_> = retained
        .history()
        .undo
        .iter()
        .chain(retained.history().redo.iter())
        .map(kinewright_core::undo::Step::charge)
        .collect();
    println!(
        "CAPACITY_CANDIDATE_METRICS={}",
        serde_json::json!({
            "clips":clips, "accepted_merges":accepted, "operations_in_refused_merge":delta.len(),
            "retained_steps":steps.len(), "history_encoded": steps.iter().map(|charge| charge.encoded).sum::<u64>(),
            "history_decoded":steps.iter().map(|charge| charge.decoded).sum::<u64>(),
            "refused_batch_encoded":charge.encoded, "refused_batch_decoded":charge.decoded,
            "retained_envelope":retained.creation_sizing().limits().unwrap().envelope.retained,
            "document_live":kinewright_core::undo::document_capacity_bytes(&retained.history().document).unwrap(),
        })
    );
}

fn capacity_candidate_actor(
    owner: &Ja1EditingSession,
    branch: &kinewright_project::bundle_records::BranchId,
) -> (
    kinewright_project::operation_history::OwnerSnapshot,
    kinewright_core::TimelineRevision,
) {
    let candidate = owner
        .durable_sessions()
        .into_iter()
        .find(|(_, guard)| guard.branch_id() == branch)
        .unwrap()
        .1;
    (
        candidate.core().export_history().unwrap(),
        grant_test_revision(candidate.core()),
    )
}

fn capacity_probe_edit(core: &Core) {
    let locked = !core.export_history().unwrap().history().document.tracks[0].sync_lock;
    let changed = core
        .request(kinewright_core::Command::DoBatch(vec![
            kinewright_core::Operation::SetTrackSyncLock {
                track: kinewright_core::TrackId(1),
                locked,
            },
        ]))
        .unwrap();
    assert!(matches!(
        changed,
        kinewright_core::Event::DocumentChanged { .. }
    ));
}

fn capacity_retire_finished(r: &C5Fix10Run, task: &TaskId) {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};
    let state = r.f.projection(&r.run);
    assert!(matches!(
        state
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .unwrap()
            .state,
        TaskState::Terminal(_)
    ));
    let attempt = state
        .attempts
        .iter()
        .find(|attempt| {
            &attempt.identity.task == task && !matches!(attempt.state, AttemptState::Retired(_))
        })
        .unwrap();
    let native = crate::native_launcher::native_identity(&attempt.identity);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &r.f.temp.path(&format!(
            "capacity-retire-{}",
            attempt.identity.attempt.0.as_str()
        )),
        attempt.identity.project.as_str(),
        r.run.0.as_str(),
    )
    .unwrap();
    claim.reserve(&native).unwrap();
    let proof = RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap();
    let producer = r.f.producer(&r.run);
    r.f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .reconcile_native_effect(
            &producer,
            &attempt.identity,
            &AttemptRetirementProof::Unbound(&proof),
        )
        .unwrap();
    claim.retire_unbound(&proof).unwrap();
    claim.finish().unwrap();
}

fn capacity_check_task(f: &Fixture, core: &Core) {
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    let owner = host.owner().unwrap();
    let guard = owner
        .durable_sessions()
        .into_iter()
        .find(|(_, guard)| guard.core().actor_identity() == core.actor_identity())
        .unwrap()
        .1;
    assert_eq!(guard.core().actor_identity(), core.actor_identity());
    let snapshot = core.export_history().unwrap();
    assert_eq!(
        snapshot.creation_sizing().kind.role(),
        kinewright_project::operation_history::Role::Task
    );
    snapshot
        .validate()
        .expect("ACCEPTED_TASK_STATE_FITS_ITS_IMMUTABLE_CREATION_BOUNDS");
}
