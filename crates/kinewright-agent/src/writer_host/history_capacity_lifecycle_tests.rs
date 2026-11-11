fn capacity_run(clips: usize) -> C5Fix10Run {
    let document = capacity_document(clips);
    let mut f = Fixture::with_document(false, document.clone());
    let mut scope = crate::orchestration::policy::director_scope(
        &Document::default(),
        f.brief.scope.base.clone(),
    )
    .unwrap();
    for track in &document.tracks {
        let name = format!("track:{}", track.id);
        scope.objects.insert(name.clone());
        scope.tracks.insert(name.clone());
        scope.bus_sidechain_closure.insert(name);
    }
    for track in 1..=20.min(clips / 10) {
        scope.objects.insert(format!("clip:{}", track * 100 + 10));
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
    brief.role = Role::Editor;
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
    let r = capacity_run(clips);
    let edits = if clips == 1200 { 68 } else { 200 };
    for edit in 0..edits {
        let reply = capacity_edit(&r.editor, &r.core, clips, edit);
        assert_eq!(
            reply.is_error,
            Some(false),
            "CAPACITY_ACCEPTED {clips}/{edit}: {reply:?}"
        );
    }
    assert_eq!(r.core.export_history().unwrap().batches.len(), edits);
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
    assert_eq!(r.core.export_history().unwrap().cursor.redo.len(), 1);
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
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "capacity-crash-parent",
    );
    for clips in [400, 1200] {
        let handoff = temp.path(&format!("handoff-{clips}.json"));
        let log = temp.path(&format!("child-{clips}.log"));
        let started = Instant::now();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
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
            .status()
            .unwrap();
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
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
        let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
        assert!(root.starts_with(kinewright_media::test_support::workspace_target_dir()));
        let samples: usize = std::env::var("KINEWRIGHT_CAPACITY_OPEN_SAMPLES")
            .map_or(1, |value| value.parse().unwrap());
        assert!((1..=6).contains(&samples));
        for _ in 0..samples {
            capacity_check_reopen(&data);
        }
        std::fs::remove_dir_all(root).unwrap();
        println!(
            "CAPACITY_LIFECYCLE clips={clips} edits={} seconds={:.3}",
            if clips == 400 { 200 } else { 68 },
            started.elapsed().as_secs_f64()
        );
    }
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
    use kinewright_core::{Command, Event, Operation, TrackId};
    let mut observed = Vec::new();
    for (clips, edits) in [(40, 200), (120, 200), (400, 200), (1200, 68)] {
        let r = capacity_run(clips);
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
                        locked: edit % 2 == 0,
                    }]
                };
                let event = core.request(Command::DoBatch(operations)).unwrap();
                assert!(
                    matches!(event, Event::DocumentChanged { .. }),
                    "CAPACITY_CANDIDATE_EDIT {event:?}"
                );
            }
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
            assert!(accepted <= 4, "CAPACITY_EXPECT_REFUSAL");
            capacity_retire_finished(&r, &child);
        }
        capacity_stop(&r.f, &r.run);
    }
    assert_eq!(
        observed,
        [(40, 3), (120, 2), (400, 1), (1200, 1)],
        "CAPACITY_MERGE_COUNTS"
    );
}

fn capacity_candidate_metrics(
    owner: &Ja1EditingSession,
    before: &BranchBinding,
    result: &TaskResult,
    clips: usize,
    accepted: usize,
) {
    use kinewright_core::{Command, Event};
    use kinewright_project::{
        bundle_records::BranchCheckpoint,
        history_record::{HistoryRecord, HistoryRole},
        task_history_admission::{lineage_for_fixture, live_history_bytes_for_fixture},
    };
    let store = owner.store().unwrap();
    let checkpoint = store
        .read_record::<BranchCheckpoint>(&before.checkpoint)
        .unwrap();
    let retained = store.resolve_history(&checkpoint.history_ref).unwrap();
    let bytes = store
        .read_evidence(result.delta_ref.as_ref().unwrap())
        .unwrap();
    let delta: Vec<kinewright_core::Operation> = serde_json::from_slice(&bytes).unwrap();
    let operation_count = delta.len();
    let preview = Core::spawn(std::sync::Arc::clone(&retained.baseline)).unwrap();
    preview.restore_history((*retained).clone()).unwrap();
    assert!(matches!(
        preview.request(Command::DoBatch(delta)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let proposed = preview.export_history().unwrap();
    let metrics = |history: &kinewright_core::HistorySnapshot| {
        let lineage = lineage_for_fixture(history).unwrap();
        let (history_live, lineage_live) =
            live_history_bytes_for_fixture(history, &lineage).unwrap();
        let record = HistoryRecord::capture(history, HistoryRole::Candidate).unwrap();
        serde_json::json!({"history_live":history_live,"lineage_live":lineage_live,"role_live":history_live + lineage_live,
            "replay_peak":kinewright_core::history_size::replay_live_bound(&record.journal,&history.baseline,&history.document).unwrap(),
            "record_encoded":serde_json::to_vec(&record).unwrap().len()})
    };
    println!(
        "CAPACITY_CANDIDATE_METRICS={}",
        serde_json::json!({"clips":clips,"accepted_merges":accepted,
        "operations_in_refused_merge":operation_count,"accepted":metrics(&retained),"refused":metrics(&proposed)})
    );
}

fn capacity_candidate_actor(
    owner: &Ja1EditingSession,
    branch: &kinewright_project::bundle_records::BranchId,
) -> (
    kinewright_core::HistorySnapshot,
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
    let locked = !core.export_history().unwrap().document.tracks[0].sync_lock;
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
    owner
        .task_history_admission(&guard)
        .unwrap()
        .check(&core.export_history().unwrap())
        .unwrap();
}
