fn b20_fixture() -> Fixture {
    let mut document = Document::default();
    kinewright_core::apply_batch(
        &mut document,
        &[
            history_marker(1),
            kinewright_core::Operation::AddTrack {
                track: kinewright_core::Track {
                    id: kinewright_core::TrackId(81),
                    kind: kinewright_core::TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            },
            kinewright_core::Operation::AddSolidClip {
                track: kinewright_core::TrackId(81),
                timeline_start: kinewright_core::TimeCode(0),
                duration: kinewright_core::TimeCode(30),
                color: kinewright_core::SolidColor::default(),
            },
        ],
    )
    .unwrap();
    let mut f = Fixture::with_document(false, document.clone());
    f.brief.scope =
        crate::orchestration::policy::director_scope(&document, f.brief.scope.base.clone())
            .unwrap();
    f
}
fn b20_assign(
    f: &Fixture,
    run: &RunId,
    parent: &ExecutionIdentity,
    director: &C4Mcp,
    role: Role,
) -> TaskId {
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
    f.projection(run).tasks.last().unwrap().id.clone()
}
fn b20_start(f: &Fixture, run: &RunId, child: &TaskId) -> (ExecutionIdentity, Core, C4Mcp) {
    let (attempt, client, endpoint, core) = c4_reserve_task(f, run, child);
    let turn = TaskTurn {
        task: child.clone(),
        attempt: attempt.attempt.clone(),
        session: attempt.nonce.clone(),
        ordinal: 1,
        owner_epoch: endpoint.authority().owner_epoch,
        expected_head: endpoint.head().clone(),
        purpose: TurnPurpose::Work,
    };
    let request = task_request(f, &client, &endpoint, OrchestrationCommand::BeginTurn(turn));
    assert!(
        client
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request)
            })
            .unwrap()
            .error
            .is_none(),
        "B20_REAL_TURN"
    );
    (attempt, core, C4Mcp::new(f, run, child))
}
fn b20_edit(mcp: &C4Mcp, core: &Core, operations: &serde_json::Value) {
    let revision = grant_test_revision(core);
    let prepared = mcp.raw(
        "prepare_edit_plan",
        serde_json::json!({"expected_revision":revision,"operations":operations}),
    );
    assert_eq!(prepared.is_error, Some(false), "B20_ROLE_EDIT {prepared:?}");
    let committed=mcp.wire("commit_edit_plan",serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
    assert_eq!(committed.is_error, Some(false), "B20_COMMIT {committed:?}");
}
fn b20_publish(f: &Fixture, run: &RunId, child: &TaskId, mcp: &C4Mcp) -> (ObjectRef, TaskResult) {
    let result = c5_fix10_freeze(mcp, child);
    mcp.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.clone(),
            result: result.clone(),
        })),
    );
    (f.projection(run).results.last().unwrap().clone(), result)
}
fn b20_merge(
    director: &C4Mcp,
    child: &TaskId,
    reference: &ObjectRef,
    result: &TaskResult,
) -> rmcp::model::CallToolResult {
    director.wire(
        "merge_task",
        director.control(OrchestrationCommand::MergeTask(MergeTask {
            task: child.clone(),
            target: result.base.clone().unwrap(),
            result: reference.clone(),
        })),
    )
}
fn b20_retire(f: &Fixture, run: &RunId, execution: &ExecutionIdentity) {
    let mut state = f.projection(run);
    state
        .attempts
        .iter_mut()
        .find(|a| a.identity == *execution)
        .unwrap()
        .state = AttemptState::Retired(AttemptOutcome::Completed);
    let task = state
        .tasks
        .iter_mut()
        .find(|t| t.id == execution.task)
        .unwrap();
    task.progress.capacity = DependencyReservation::Free;
    task.progress.turn_ended = true;
    for effect in state
        .effects
        .iter_mut()
        .filter(|e| e.task == execution.task)
    {
        effect.state = EffectState::Settled;
    }
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &f.producer_client(run), run, state, None);
}
fn b20_sequence(role: Role, early: bool) {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let first = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (first_attempt, first_core, first_mcp) = b20_start(&f, &run, &first);
    b20_edit(
        &first_mcp,
        &first_core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":2}]),
    );
    let (first_ref, first_result) = b20_publish(&f, &run, &first, &first_mcp);
    b20_retire(&f, &run, &first_attempt);
    let assigned = early.then(|| b20_assign(&f, &run, &parent, &director, role));
    assert_eq!(
        b20_merge(&director, &first, &first_ref, &first_result).is_error,
        Some(false),
        "B20_FIRST_MERGE"
    );
    let advanced = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .candidate_binding(&f.brief.scope.base.branch)
        .unwrap();
    assert_ne!(advanced, f.brief.scope.base);
    let child = assigned.unwrap_or_else(|| b20_assign(&f, &run, &parent, &director, role));
    let (second_attempt, core, mcp) = b20_start(&f, &run, &child);
    assert_eq!(
        f.projection(&run).tasks.last().unwrap().brief.scope.base,
        advanced,
        "B20_PREPARED_CURRENT_BASE"
    );
    let operations = if role == Role::Motion {
        let clip = core
            .export_history()
            .unwrap()
            .document
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| matches!(c.content, kinewright_core::ClipContent::Solid(_)))
            .unwrap()
            .id;
        serde_json::json!([{"op":"set_solid_color","clip":clip,"color":{"r":20,"g":30,"b":40}}])
    } else {
        serde_json::json!([{"op":"move_marker","marker":1,"to":3}])
    };
    b20_edit(&mcp, &core, &operations);
    let (reference, result) = b20_publish(&f, &run, &child, &mcp);
    b20_retire(&f, &run, &second_attempt);
    assert_eq!(
        b20_merge(&director, &child, &reference, &result).is_error,
        Some(false),
        "B20_SECOND_MERGE"
    );
    let current = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .candidate_binding(&advanced.branch)
        .unwrap();
    assert_ne!(current, advanced, "B20_SECOND_ACCEPTED");
}
#[test]
fn te15_b20_editor_then_motion() {
    b20_sequence(Role::Motion, false);
}
#[test]
fn te15_b20_editor_then_editor() {
    b20_sequence(Role::Editor, false);
}
#[test]
fn te15_b20_assigned_before_merge_prepared_after() {
    b20_sequence(Role::Editor, true);
}
#[test]
fn te15_b20_two_prepared_children_keep_the_existing_conflict() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let a = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (a_attempt, a_core, a_mcp) = b20_start(&f, &run, &a);
    b20_edit(
        &a_mcp,
        &a_core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":2}]),
    );
    let (a_ref, a_result) = b20_publish(&f, &run, &a, &a_mcp);
    b20_retire(&f, &run, &a_attempt);
    let b = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (b_attempt, b_core, b_mcp) = b20_start(&f, &run, &b);
    b20_edit(
        &b_mcp,
        &b_core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":3}]),
    );
    let (b_ref, b_result) = b20_publish(&f, &run, &b, &b_mcp);
    b20_retire(&f, &run, &b_attempt);
    assert_eq!(a_result.base, b_result.base, "B20_COMMON_CHECKPOINT");
    assert_eq!(
        b20_merge(&director, &a, &a_ref, &a_result).is_error,
        Some(false)
    );
    let rejected = b20_merge(&director, &b, &b_ref, &b_result);
    assert_eq!(
        rejected.is_error,
        Some(true),
        "B20_SECOND_CONFLICT {rejected:?}"
    );
    let text = serde_json::to_string(&rejected).unwrap();
    assert!(
        text.contains("history_changed") || text.contains("history changed"),
        "B20_CONFLICT_PATH {text}"
    );
}
#[test]
fn te15_b20_foreign_and_widened_assignments_refuse() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let foreign = {
        let owner = f.host.owner.upgrade().unwrap();
        let mut owner = owner.lock().unwrap();
        let candidate = owner
            .owner_mut()
            .unwrap()
            .fork_durable_branch(
                "other candidate",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner
            .owner()
            .unwrap()
            .candidate_binding(candidate.branch_id())
            .unwrap()
    };
    for widened in [false, true] {
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        if widened {
            brief.scope.objects.insert("marker:999".into());
        } else {
            brief.scope.base = foreign.clone();
        }
        let reply = director.wire(
            "assign_task",
            director.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
                run: run.clone(),
                parent: parent.task.clone(),
                brief,
            }))),
        );
        assert_eq!(
            reply.is_error,
            Some(true),
            "B20_AUTHORITY_REFUSAL {reply:?}"
        );
        assert_eq!(f.projection(&run).tasks.len(), 1, "B20_NO_FOREIGN_TASK");
    }
}
struct B20NoProfiles;
impl crate::effect_pump::ProfileFactory for B20NoProfiles {
    fn bind(
        &self,
        _: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        panic!("B20_NO_PROVIDER_CONSTRUCTION")
    }
}
fn b20_host_failure(widened: bool) {
    use kinewright_core::AgentSession;
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    b20_retire(&f, &run, &parent);
    if widened {
        let mut state = f.projection(&run);
        state
            .tasks
            .iter_mut()
            .find(|t| t.id == child)
            .unwrap()
            .brief
            .scope
            .objects
            .insert("marker:999".into());
        publish_projection(&f, &f.producer_client(&run), &run, state, None);
    } else {
        // A same-branch checkpoint without this run's merge receipt is not an
        // accepted descendant for a new child.
        b20_advance_candidate(&f, &f.brief.scope.base);
    }
    let owner = f.host.owner.upgrade().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("b20-machine"),
            owner.lock().unwrap().spending_project().unwrap().as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let sent = Arc::new(AtomicUsize::new(0));
    let mut pump = crate::effect_pump::EffectPump::with_launcher(
        &owner,
        f.producer(&run),
        run.clone(),
        claim,
        Te15CountingLauncher(sent.clone()),
        Arc::new(B20NoProfiles),
        std::num::NonZeroU64::new(8000),
    );
    pump.send_user_message(String::new()).unwrap();
    let state = f.projection(&run);
    assert!(
        matches!(state.wait, RunWait::Replan(_)),
        "B20_HOST_REPLAN_WAIT {state:?}"
    );
    assert_eq!(
        state.tasks.iter().find(|t| t.id == child).unwrap().state,
        TaskState::Cancelled
    );
    assert!(
        !crate::orchestration::machine::eligible(&state, &RunAction::Continue),
        "B20_NO_CONTINUE_LOOP"
    );
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        !state
            .effects
            .iter()
            .any(|e| e.task == child && e.state == EffectState::Pending),
        "B20_NO_CHILD_RETRY"
    );
    let wake = state
        .effects
        .iter()
        .find(|e| e.task == parent.task && e.state == EffectState::Pending)
        .expect("B20_DIRECTOR_TOLD");
    let text = String::from_utf8(
        owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_evidence(&wake.payload)
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        text,
        "The task could not start because its scope is invalid. This start sent nothing. The Director must replan the task; Continue cannot repair it.",
        "B20_TRUE_HOST_WORDS"
    );
    let page = f
        .editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    assert_eq!(page.wait_reason.as_deref(), Some(text.as_str()));
    assert_eq!(
        page.continue_unavailable_reason.as_deref(),
        Some("Continue cannot retry this task. The Director must replan it.")
    );
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .native_accounting(&state)
            .unwrap()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        0,
        "B20_NO_CHARGE"
    );
    drop(pump);
    drop(director);
    let (_, _, director) = c4_reserve_root_for_b20(&f, &run);
    b20_assign(&f, &run, &parent, &director, Role::Editor);
    assert!(
        matches!(f.projection(&run).wait, RunWait::None),
        "B20_DIRECTOR_REPLANS"
    );
    assert!(
        !f.projection(&run)
            .effects
            .iter()
            .any(|e| e.id == wake.id && e.state == EffectState::Pending),
        "B201_ASSIGN_CONSUMES_NOTIFICATION"
    );
}
fn c4_reserve_root_for_b20(f: &Fixture, run: &RunId) -> (ExecutionIdentity, Core, C4Mcp) {
    let root = f.projection(run).tasks[0].id.clone();
    b20_start(f, run, &root)
}
#[test]
fn te15_b20_recorded_base_and_assignment_replay_survive_reopen() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let first = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, core, mcp) = b20_start(&f, &run, &first);
    b20_edit(
        &mcp,
        &core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":2}]),
    );
    let (reference, result) = b20_publish(&f, &run, &first, &mcp);
    b20_retire(&f, &run, &attempt);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: parent.task.clone(),
        brief,
    }));
    let control = director.control(command);
    let original = director.wire("assign_task", control.clone());
    assert_eq!(original.is_error, Some(false));
    let child = f.projection(&run).tasks.last().unwrap().id.clone();
    assert_eq!(
        b20_merge(&director, &first, &reference, &result).is_error,
        Some(false)
    );
    let (_, _, child_mcp) = b20_start(&f, &run, &child);
    let recorded = f
        .projection(&run)
        .tasks
        .last()
        .unwrap()
        .brief
        .scope
        .base
        .clone();
    assert_ne!(recorded, f.brief.scope.base, "B20_LATE_RECORDED");
    let count = f.projection(&run).tasks.len();
    let replay = director.wire("assign_task", control);
    assert_eq!(
        f.projection(&run).tasks.len(),
        count,
        "B20_REPLAY_TASK_COUNT"
    );
    assert_eq!(replay.is_error, Some(false));
    assert_eq!(
        original.structured_content.unwrap()["outcome"],
        replay.structured_content.unwrap()["outcome"],
        "B20_IMMUTABLE_ASSIGNMENT_REPLAY"
    );
    // A later cut must not retarget an already prepared task.
    b20_advance_candidate(&f, &recorded);
    let machine = f.temp.path("b20-reopen");
    drop(child_mcp);
    drop(mcp);
    drop(director);
    let f = c4_restart(f, &machine);
    assert_eq!(
        f.projection(&run).tasks.last().unwrap().brief.scope.base,
        recorded,
        "B20_REOPEN_BINDING"
    );
    let (_, _, _mcp) = b20_start(&f, &run, &child);
    assert_eq!(
        f.projection(&run).tasks.last().unwrap().brief.scope.base,
        recorded,
        "B20_NO_RETARGET_ON_RECOVERY"
    );
}
#[test]
fn te15_b20_history_refusal_has_host_words() {
    let error = super::super::task_preparation::preparation_refusal(HostError::from(
        kinewright_project::bundle::BundleError::HistoryChanged,
    ));
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<super::super::task_preparation::PreparationRefusal>()
            .unwrap()
            .kind
            .words(),
        "The task could not start because its candidate changed. This start sent nothing. The Director must replan the task; Continue cannot repair it.",
        "B20_HISTORY_WORDS"
    );
}

fn b20_advance_candidate(f: &Fixture, base: &BranchBinding) {
    let owner = f.host.owner.upgrade().unwrap();
    let mut owner = owner.lock().unwrap();
    let mut candidate = owner
        .owner()
        .unwrap()
        .durable_sessions()
        .into_iter()
        .find(|(_, g)| g.branch_id() == &base.branch)
        .unwrap()
        .1;
    candidate
        .core()
        .request(kinewright_core::Command::Do(
            kinewright_core::Operation::MoveMarker {
                marker: kinewright_core::MarkerId(1),
                to: kinewright_core::TimeCode(4),
            },
        ))
        .unwrap();
    owner
        .owner_mut()
        .unwrap()
        .persist_durable_branch(&mut candidate)
        .unwrap();
}
#[test]
fn te15_b20_host_failure_notifies_director_without_retrying_child() {
    b20_host_failure(true);
}
#[test]
fn te15_b20_unowned_checkpoint_refuses_before_charge() {
    b20_host_failure(false);
}

#[test]
fn te15_b20_host_refusal_preserves_person_pause() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, _, _mcp) = b20_start(&f, &run, &child);
    b20_retire(&f, &run, &attempt);
    b20_retire(&f, &run, &parent);
    let mut state = f.projection(&run);
    state.phase = RunPhase::Paused;
    publish_projection(&f, &f.producer_client(&run), &run, state, None);
    // Exercise the publication after a person Pause between preparation and
    // failure reconciliation. No launch or provider charge exists.
    let producer = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .preparation_failed(
            &producer,
            &attempt,
            super::super::task_preparation::PreparationKind::Scope,
        )
        .unwrap();
    let state = f.projection(&run);
    assert_eq!(state.phase, RunPhase::Paused, "B20_PERSON_PAUSE_RETAINED");
    assert!(
        crate::orchestration::machine::eligible(&state, &RunAction::Continue),
        "B20_RESUME_DIRECTOR_NOTIFICATION"
    );
    let wake = state
        .effects
        .iter()
        .find(|e| e.task == parent.task && e.state == EffectState::Pending)
        .unwrap()
        .clone();
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
        })
        .unwrap();
    assert!(reply.error.is_none(), "B20_REAL_RESUME {reply:?}");
    let resumed = f.projection(&run);
    assert!(
        resumed.effects.iter().any(|e| e == &wake),
        "B20_SAME_NOTIFICATION"
    );
    assert!(
        !resumed
            .effects
            .iter()
            .any(|e| e.task == child && e.state == EffectState::Pending),
        "B20_PAUSE_NO_CHILD_RETRY"
    );
}

// B1's separate-lane fixtures predate host-selected fresh-child preparation.
// Seed their already-prepared authority directly, then let ordinary recovery
// reuse it. Fresh foreign assignments still refuse through the real MCP.
fn b20_prepare_legacy_lane(f: &Fixture, run: &RunId, task: &TaskId) {
    use crate::orchestration::tasks::{EditorTask, RuntimeTaskAuthority};
    let producer = f.producer_client(run);
    let owner = f.host.owner.upgrade().unwrap();
    let mut state = f.projection(run);
    let (project, epoch) = {
        let host = owner.lock().unwrap();
        (
            host.owner().unwrap().project_id().unwrap(),
            host.owner().unwrap().store().unwrap().owner_epoch(),
        )
    };
    let execution = ExecutionIdentity {
        version: 1,
        project: project.clone(),
        run: run.clone(),
        task: task.clone(),
        attempt: identity(),
        nonce: identity(),
        owner_instance: identity(),
    };
    state.attempts.push(AttemptReservation {
        identity: execution.clone(),
        owner_epoch: epoch,
        state: AttemptState::Reserved,
        recovery_ref: fixture_launch_intent(f, &execution, false, false),
    });
    let child = state.tasks.iter_mut().find(|t| &t.id == task).unwrap();
    child.state = TaskState::Preparing;
    child.progress.capacity = DependencyReservation::Backed(execution.attempt.clone());
    let brief = child.brief.clone();
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &producer, run, state, Some(&execution));
    let mut state = f.projection(run);
    {
        let mut host = owner.lock().unwrap();
        let actor = EditorTask::prepare(
            host.owner_mut().unwrap(),
            brief,
            RuntimeTaskAuthority {
                project,
                run: run.clone(),
                task: task.clone(),
                attempt: execution.attempt.clone(),
                endpoint: EndpointId(identity()),
                owner_epoch: epoch,
                expires_at: Instant::now() + Duration::from_secs(300),
            },
        )
        .unwrap();
        host.hooks
            .runs
            .get_mut(run)
            .unwrap()
            .tasks
            .insert(task.clone(), actor);
        host.persist_runtime_task(&mut state, &execution, false)
            .unwrap();
        host.publish_native(&producer.bootstrap.auth, state, &execution, vec![])
            .unwrap();
    }
    let mut state = f.projection(run);
    state
        .attempts
        .iter_mut()
        .find(|a| a.identity == execution)
        .unwrap()
        .state = AttemptState::Retired(AttemptOutcome::Interrupted);
    let child = state.tasks.iter_mut().find(|t| &t.id == task).unwrap();
    child.state = TaskState::Queued;
    child.progress.capacity = DependencyReservation::Free;
    child.progress.turn = None;
    child.progress.turn_ended = true;
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &producer, run, state, None);
}
