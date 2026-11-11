fn c5e_depth(f: &Fixture, maximum: &mut u64) {
    let host = f.host.owner.upgrade().unwrap();
    let depth = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .maximum_committed_depth()
        .unwrap();
    *maximum = (*maximum).max(depth);
    assert!(*maximum <= 16, "C5E_CONSTANT_GRAPH_DEPTH {maximum}");
}
fn c5e_history(f: &Fixture, run: &RunId, task: &TaskId) -> kinewright_core::HistorySnapshot {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(run)
        .unwrap()
        .unwrap();
    let saved = owner.saved_task_history(&state, task).unwrap().unwrap();
    let store = owner.owner().unwrap().store().unwrap();
    let binding = owner
        .owner()
        .unwrap()
        .candidate_binding(&saved.source.branch)
        .unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(&binding.checkpoint)
        .unwrap();
    (*store
        .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
        .unwrap())
    .clone()
}
fn c5e_send(f: &Fixture, execution: &ExecutionIdentity, turn: u64) {
    use kinewright_project::orchestration_accounting::SendConsumption;
    let producer = f.producer(&execution.run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let mut state = owner.spending_scope(&producer, &execution.run).unwrap();
    let previous = owner.native_accounting(&state).unwrap().unwrap();
    let mut next = previous.successor().unwrap();
    if !next
        .task_mut(&execution.task.0)
        .unwrap()
        .providers
        .iter()
        .any(|p| p.attempt == execution.attempt.0)
    {
        use kinewright_project::orchestration_accounting::{ProviderMetadata, ProviderSelection};
        let selection = ProviderSelection {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        };
        next.task_mut(&execution.task.0)
            .unwrap()
            .select_provider(ProviderMetadata {
                attempt: execution.attempt.0.clone(),
                requested: selection.clone(),
                resolved: selection,
                cli_version: "fixture-1".into(),
            })
            .unwrap();
    }
    let index = owner.resolve_attachment(Some(&producer.bearer())).unwrap();
    let issued = owner.issue_metadata(index, None).unwrap();
    // Distinct, unrooted payloads exercise accounting ownership instead of
    // letting a shared assignment or a host-grant root hide a lost dependency.
    let payload_bytes = serde_json::to_vec(&("c5e-send-payload", execution, turn)).unwrap();
    let payload_seal = owner
        .owner_mut()
        .unwrap()
        .admit_feedback_evidence(
            kinewright_project::studio_records::FeedbackEvidenceKind::Text,
            &payload_bytes,
        )
        .unwrap();
    let payload = payload_seal.reference.clone();
    next.task_mut(&execution.task.0)
        .unwrap()
        .consume_send(SendConsumption {
            command: issued.envelope.id.clone(),
            attempt: execution.attempt.0.clone(),
            turn,
            payload: payload.clone(),
        })
        .unwrap();
    next.validate_successor(&previous).unwrap();
    let record = owner
        .owner_mut()
        .unwrap()
        .admit_scheduler_record(OrchestrationRecord {
            retention: kinewright_project::orchestration_retention::Retention::Complete,
            version: 1,
            payload: OrchestrationPayload::Accounting(Box::new(next)),
        })
        .unwrap();
    owner
        .replace_accounting_reference(&mut state, record.reference)
        .unwrap();
    let digest = kinewright_media::sha256_bytes(
        &serde_json::to_vec(&("c5e-send", &issued.envelope.id)).unwrap(),
    );
    let lane = state
        .tasks
        .iter()
        .find(|t| t.id == execution.task)
        .unwrap()
        .brief
        .scope
        .base
        .branch
        .clone();
    state.effects.push(EffectRecord {
        id: serde_json::from_value(serde_json::json!(&digest[..32])).unwrap(),
        task: execution.task.clone(),
        lane,
        order: state.effects.iter().map(|e| e.order).max().unwrap() + 1,
        safety: EffectSafety::ProcessBound,
        state: EffectState::Settled,
        payload,
    });
    let result = owner.publish_native_issued(&producer, state, execution, vec![], issued);
    assert!(result.is_ok(), "C5E_SEND_PUBLICATION {turn}: {result:?}");
}

#[derive(Clone, Copy)]
enum C5eSize {
    Maximal,
    Small,
    SendsOnly,
}
impl C5eSize {
    fn clocks(self) -> u64 {
        match self {
            Self::Maximal => 720,
            Self::Small | Self::SendsOnly => 3,
        }
    }
    fn tasks(self) -> usize {
        match self {
            Self::Maximal | Self::SendsOnly => 8,
            Self::Small => 2,
        }
    }
    fn rounds(self) -> usize {
        match self {
            Self::Maximal | Self::SendsOnly => 3,
            Self::Small => 2,
        }
    }
    fn approvals(self) -> usize {
        match self {
            Self::Maximal => 256,
            Self::SendsOnly => 0,
            Self::Small => 4,
        }
    }
    fn sends(self, task: usize) -> usize {
        match self {
            Self::Small => 4,
            Self::Maximal | Self::SendsOnly if task == 7 => 80,
            Self::Maximal | Self::SendsOnly if task == 6 => 6,
            Self::Maximal | Self::SendsOnly => 7,
        }
    }
}
#[derive(Default, Debug)]
struct C5eStorage {
    objects: u64,
    physical_bytes: u64,
    metadata_bytes: u64,
}
fn c5e_storage(f: &Fixture, maximum: &mut C5eStorage) {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let store = owner.owner().unwrap().store().unwrap();
    let usage = store.usage().unwrap();
    let limits = store.limits();
    maximum.objects = maximum.objects.max(usage.objects);
    maximum.physical_bytes = maximum.physical_bytes.max(usage.peak_physical_bytes);
    maximum.metadata_bytes = maximum.metadata_bytes.max(usage.metadata_bytes);
    assert!(
        maximum.objects <= limits.objects,
        "C5E_OBJECT_BOUND {maximum:?} limit={}",
        limits.objects
    );
    assert!(
        maximum.metadata_bytes <= limits.metadata_bytes,
        "C5E_METADATA_BOUND {maximum:?} limit={}",
        limits.metadata_bytes
    );
    assert!(
        maximum.physical_bytes
            <= limits.owned_bytes + limits.copy_overlap_bytes + 2 * limits.metadata_bytes,
        "C5E_PHYSICAL_BOUND {maximum:?}"
    );
    assert!(
        !usage.damaged_accounting,
        "C5E_STORAGE_ACCOUNTING {usage:?}"
    );
}
fn c5e_pending_history(f: &Fixture, core: &Core, cycles: usize, phase: &str) {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let profile = owner
        .owner()
        .unwrap()
        .pending_task_history_profile(core)
        .unwrap();
    eprintln!(
        "C5_PENDING_HISTORY cycles={cycles} phase={phase} {}",
        serde_json::to_string(&profile).unwrap()
    );
    eprintln!(
        "C5_HISTORY_REPLAY_SHADOW_CANDIDATE charge={} limit={}",
        profile.history_decoded,
        kinewright_project::run_admission::HISTORY_BYTES
    );
}
fn c5e_approval_edits(
    f: &Fixture,
    execution: &ExecutionIdentity,
    core: &Core,
    count: usize,
    depth: &mut u64,
    storage: &mut C5eStorage,
) {
    let mcp = C4Mcp::new(f, &execution.run, &execution.task);
    for clip in 1..=count {
        let before_document = core.export_history().unwrap().document;
        let revision = grant_test_revision(core);
        let prepared = mcp.raw(
            "prepare_edit_plan",
            serde_json::json!({
            "expected_revision":revision,"operations":[{"op":"delete_clip","clip":1}]}),
        );
        assert_eq!(
            prepared.is_error,
            Some(false),
            "C5E_APPROVAL_PREPARE {clip} {prepared:?}"
        );
        let before = {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            owner
                .owner()
                .unwrap()
                .orchestration_cut(&execution.run)
                .unwrap()
                .unwrap()
                .segment
        };
        let committed = mcp.raw("commit_edit_plan", serde_json::json!({
            "expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
        assert_eq!(
            committed.is_error,
            Some(false),
            "C5E_APPROVAL_COMMIT {clip} {committed:?}"
        );
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        let project = owner.owner().unwrap();
        let cut = project.orchestration_cut(&execution.run).unwrap().unwrap();
        let final_record = project
            .store()
            .unwrap()
            .read_record::<OrchestrationRecord>(&cut.segment)
            .unwrap();
        let OrchestrationPayload::EventSegment(applied) = &final_record.payload else {
            panic!("C5E_APPLICATION_H1")
        };
        let approval_ref = applied.binding.previous.as_ref().unwrap();
        let approval_record = project
            .store()
            .unwrap()
            .read_record::<OrchestrationRecord>(approval_ref)
            .unwrap();
        let OrchestrationPayload::EventSegment(approved) = &approval_record.payload else {
            panic!("C5E_APPROVAL_H1")
        };
        assert_eq!(
            approved.binding.previous.as_ref(),
            Some(&before),
            "C5E_TWO_H1_PUBLICATIONS {clip}"
        );
        let approved_snapshot = project
            .store()
            .unwrap()
            .read_record::<OrchestrationRecord>(&approved.snapshot)
            .unwrap();
        let applied_snapshot = project
            .store()
            .unwrap()
            .read_record::<OrchestrationRecord>(&applied.snapshot)
            .unwrap();
        let OrchestrationPayload::Snapshot(approved_state) = &approved_snapshot.payload else {
            panic!("C5E_APPROVAL_SNAPSHOT")
        };
        let OrchestrationPayload::Snapshot(applied_state) = &applied_snapshot.payload else {
            panic!("C5E_APPLICATION_SNAPSHOT")
        };
        assert_eq!(
            approved_state.edit_approvals.len(),
            clip,
            "C5E_APPROVAL_PUBLISHED {clip}"
        );
        assert_eq!(
            approved_state.edit_applications.len(),
            clip - 1,
            "C5E_APPROVAL_BEFORE_APPLY {clip}"
        );
        assert_eq!(applied_state.edit_approvals, approved_state.edit_approvals);
        assert_eq!(
            applied_state.edit_applications.len(),
            clip,
            "C5E_APPLICATION_PUBLISHED {clip}"
        );
        let marker: serde_json::Value = serde_json::from_slice(
            &project
                .store()
                .unwrap()
                .read_evidence(applied_state.edit_applications.last().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["kind"], "studio_edit_applied_v1");
        assert_eq!(
            marker["approval"],
            serde_json::to_value(approved_state.edit_approvals.last().unwrap()).unwrap()
        );
        drop(owner);
        assert_eq!(
            core.export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            3,
            "C5E_DESTRUCTIVE_EDIT_APPLIED {clip}"
        );
        // Restore the same owned clip between deletions. This reaches the full
        // approval cap without requiring more than the admitted object scope.
        let undone = mcp.invoke(
            "undo_task_edit",
            serde_json::json!({"expected_revision":grant_test_revision(core)}),
        );
        assert_eq!(
            undone.is_error,
            Some(false),
            "C5E_APPROVAL_UNDO {clip} {undone:?}"
        );
        assert_eq!(
            core.export_history().unwrap().document,
            before_document,
            "C5E_APPROVAL_UNDO_EXACT {clip}"
        );
        c5e_depth(f, depth);
        c5e_storage(f, storage);
        if matches!(clip, 8 | 16 | 32 | 64) {
            c5e_pending_history(f, core, clip, "undo");
        }
        if clip % 32 == 0 {
            eprintln!("C5E_APPROVALS {clip} {storage:?} max_depth={depth}");
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            let profile = owner
                .owner()
                .unwrap()
                .retained_run_profile(&execution.run)
                .unwrap();
            eprintln!(
                "C5_LINEAR_GROWTH approvals={clip} {}",
                serde_json::to_string(&profile).unwrap()
            );
        }
    }
}

#[test]
#[ignore = "on-demand: maximal six-hour durable run"]
fn s4b_c5e_maximal_run_keeps_depth_stop_and_recovery_bounded() {
    c5e_run(C5eSize::Maximal);
}
#[test]
#[ignore = "on-demand: retained zero-approval 128-send storage measurement"]
fn s4b_c5e_zero_approvals_128_distinct_sends() {
    c5e_run(C5eSize::SendsOnly);
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s4b_c5e_small_run_keeps_approvals_stop_and_recovery_bounded() {
    c5e_run(C5eSize::Small);
}
#[allow(
    clippy::too_many_lines,
    reason = "one scaled durable run retains the same bounds and histories through Stop and reopen"
)]
fn c5e_run(size: C5eSize) {
    use super::super::native_invocation::SpendingChange;
    use kinewright_project::{
        orchestration_accounting::{RUN_MILLIS, UsageMode, UsageSample, UsageValues},
        run_admission,
    };
    let mut document = Document::default();
    kinewright_core::apply_batch(
        &mut document,
        &[kinewright_core::Operation::AddTrack {
            track: kinewright_core::Track {
                id: kinewright_core::TrackId(81),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: false,
                clips: vec![],
            },
        }],
    )
    .unwrap();
    for clip in 0..4 {
        kinewright_core::apply_batch(
            &mut document,
            &[kinewright_core::Operation::AddSolidClip {
                track: kinewright_core::TrackId(81),
                timeline_start: kinewright_core::TimeCode(i64::from(clip) * 10),
                duration: kinewright_core::TimeCode(10),
                color: kinewright_core::SolidColor::default(),
            }],
        )
        .unwrap();
    }
    let mut f = Fixture::with_inputs(false, document, "project.kinewright", |bundle| {
        bundle.empty_inputs().unwrap().reference.clone()
    });
    f.host.runtime.stop();
    let scope_document = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap()
        .document;
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scope_document, f.brief.scope.base.clone())
            .unwrap();
    // Reserve the two marker identities used by the existing redo witness.
    f.brief
        .scope
        .objects
        .extend(["marker:1".into(), "marker:2".into()]);
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        for id in [50, 51] {
            owner
                .core()
                .request(kinewright_core::Command::Do(history_marker(id)))
                .unwrap();
        }
        owner
            .core()
            .request(kinewright_core::Command::Undo)
            .unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
        owner.core().export_history().unwrap()
    };
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"hook plan, script and assignment")
        .unwrap();
    let (run, parent, _) = reserve(&f);
    let run_auth = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .prepare_task_endpoint(
            &run_auth,
            &run,
            &parent.task,
            &parent.attempt,
            Duration::from_secs(3600),
        )
        .unwrap();
    let director = C4Mcp::new(&f, &run, &parent.task);
    let now = native_now(&f, &run);
    let mut maximum = 0;
    let mut storage = C5eStorage::default();
    let sequence = f.projection(&run).sequence;
    for tick in 1..size.clocks() {
        assert!(
            spending_change(
                &f,
                &parent,
                SpendingChange::Clock,
                now + tick * 30_000,
                30_000,
                true
            )
            .is_ok(),
            "C5_FIX1_CLOCK {tick}"
        );
        assert_eq!(
            f.projection(&run).sequence,
            sequence,
            "C5_FIX1_CLOCK_REPLACES"
        );
        if tick % 120 == 0 {
            eprintln!("C5_FIX1_CLOCK {tick}");
            c5e_depth(&f, &mut maximum);
            c5e_storage(&f, &mut storage);
        }
    }
    let claim_root = f.temp.path("c5-fix1-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut all_attempts = vec![];
    let mut histories = vec![];
    let mut last = parent.clone();
    // Seven serial specialists fit the three simultaneous dependency slots.
    // One task reaches its 80-send ceiling; the run reaches its shared 128-send ceiling.
    for task_index in 0..size.tasks() {
        let task = if task_index == size.tasks() - 1 {
            parent.task.clone()
        } else {
            let mut brief = f.brief.clone();
            brief.role = if task_index == 0 {
                Role::Editor
            } else {
                Role::Reviewer
            };
            if brief.role == Role::Reviewer {
                brief.scope.operations.clear();
            }
            director.accepted(
                "assign_task",
                OrchestrationCommand::AssignTask(Box::new(AssignTask {
                    run: run.clone(),
                    parent: parent.task.clone(),
                    brief,
                })),
            );
            f.projection(&run).tasks.last().unwrap().id.clone()
        };
        let sends = size.sends(task_index);
        let mut history = None;
        for round in 0..size.rounds() {
            let (execution, client, endpoint, core) = if task_index == size.tasks() - 1
                && round == 0
            {
                let (client, endpoint) = task_actor(&f, &run, &parent, &f.producer_client(&run));
                let core = f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&run].tasks
                    [&task]
                    .core()
                    .clone();
                (parent.clone(), client, endpoint, core)
            } else {
                // Match EffectPump::launch's native lease. C4's short default
                // is not long enough for the maximal approval workload.
                c4_reserve_task_with_ttl(&f, &run, &task, Duration::from_secs(3600))
            };
            // Reserve the real launcher's private transport fence, as in
            // s4b_c3_fix1_unbound_crash_child. Recovery must not claim a known
            // not-dispatched zero after this scripted attempt reports usage.
            claim.reserve(&unbound_identity(&execution)).unwrap();
            if round == 0 {
                history_turn(&f, &execution, &client, &endpoint, 1, true);
                history = Some(if task_index == 0 {
                    c5e_approval_edits(
                        &f,
                        &execution,
                        &core,
                        size.approvals(),
                        &mut maximum,
                        &mut storage,
                    );
                    c4_mcp_edit_redo(&C4Mcp::new(&f, &run, &task), &core)
                } else {
                    core.export_history().unwrap()
                });
                c5e_pending_history(
                    &f,
                    &core,
                    if task_index == 0 { size.approvals() } else { 0 },
                    "checkpoint",
                );
                f.host
                    .owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .checkpoint_task_history(&run_auth, &run, &task)
                    .unwrap();
                history_turn(&f, &execution, &client, &endpoint, 1, false);
                spending_change(
                    &f,
                    &execution,
                    SpendingChange::Ceiling(2_000_000),
                    now + (size.clocks() - 1) * 30_000,
                    0,
                    true,
                )
                .unwrap();
            } else {
                assert_eq!(
                    core.export_history().unwrap(),
                    *history.as_ref().unwrap(),
                    "C5_FIX1_ATTEMPT_HISTORY"
                );
            }
            let count = sends / size.rounds() + usize::from(round < sends % size.rounds());
            for turn in 1..=count as u64 {
                c5e_send(&f, &execution, turn);
                for (event, ordinal, input, output, complete) in [
                    ("partial", 1, 2, 1, false),
                    ("complete", 2, 5, 2, true),
                    ("complete", 2, 5, 2, true),
                ] {
                    let sample=crate::protocol::limits::usage::decode_usage(crate::protocol::limits::usage::UsageAdapter::Codex,
                        &serde_json::json!({"usage":{"input_tokens":input,"cached_input_tokens":1,"output_tokens":output,"reasoning_output_tokens":1}}),
                        UsageSample { snapshot: None,attempt:execution.attempt.0.clone(),turn,event:event.into(),ordinal,mode:UsageMode::Cumulative,values:UsageValues::default(),complete,input_includes_cache:true,output_includes_reasoning:true}).unwrap();
                    spending_change(
                        &f,
                        &execution,
                        SpendingChange::Usage(sample),
                        now + (size.clocks() - 1) * 30_000,
                        0,
                        true,
                    )
                    .unwrap();
                }
            }
            all_attempts.push(execution.clone());
            last = execution.clone();
            if task_index != size.tasks() - 1 || round != size.rounds() - 1 {
                crate::effect_pump::reconcile_attempt(
                    &f.host.owner.upgrade().unwrap(),
                    &mut claim,
                    &run_auth,
                    &execution,
                )
                .unwrap();
            }
            c5e_depth(&f, &mut maximum);
            c5e_storage(&f, &mut storage);
        }
        histories.push((task, history.unwrap(), sends));
        eprintln!("C5_FIX1_MAX_TASK {task_index} sends={sends}");
    }
    if matches!(size, C5eSize::Maximal | C5eSize::SendsOnly) {
        assert_eq!(histories.len(), run_admission::TASKS);
        assert_eq!(all_attempts.len(), run_admission::ATTEMPTS);
        assert_eq!(
            spending_record(&f, &run)
                .tasks
                .iter()
                .map(|t| t.sends.len())
                .sum::<usize>(),
            run_admission::SENDS
        );
    } else {
        assert_eq!(histories.len(), size.tasks());
        assert_eq!(all_attempts.len(), size.tasks() * size.rounds());
        assert_eq!(
            spending_record(&f, &run)
                .tasks
                .iter()
                .map(|t| t.sends.len())
                .sum::<usize>(),
            (0..size.tasks()).map(|i| size.sends(i)).sum::<usize>()
        );
    }
    {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        let profile = owner.owner().unwrap().retained_run_profile(&run).unwrap();
        eprintln!(
            "C5_LINEAR_CAPS approvals={} {}",
            size.approvals(),
            serde_json::to_string(&profile).unwrap()
        );
    }
    drop(director);
    assert!(
        spending_change(
            &f,
            &last,
            SpendingChange::Clock,
            now + RUN_MILLIS,
            30_000,
            true
        )
        .is_err()
    );
    assert_eq!(spending_record(&f, &run).elapsed_ms, RUN_MILLIS);
    let full = spending_record(&f, &run);
    if matches!(size, C5eSize::Maximal | C5eSize::SendsOnly) {
        assert!(
            matches!(
                f.host
                    .owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .owner()
                    .unwrap()
                    .check_run_send_budget(&full),
                Err(kinewright_project::bundle::BundleError::RunLimit)
            ),
            "C5_FIX1_MAX_SEND_ADMISSION_CLOSED"
        );
    } else {
        assert!(
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .check_run_send_budget(&full)
                .is_ok(),
            "C5E_SMALL_BELOW_SEND_CEILING"
        );
    }
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX1_MAX_STOP",
    );
    // Stop closes H1 authority. Complete this scripted transport's unbound
    // cleanup through the launcher's acknowledgment port, then replay recovery.
    // The recovery-only unbound path itself requires a still-Reserved attempt.
    {
        use crate::native_launcher::InvocationOwner;
        let proof = crate::guardian::RecoveryObserver::unbound_cleanup(
            &mut claim,
            &unbound_identity(&last),
        )
        .unwrap();
        let host = f.host.owner.upgrade().unwrap();
        let port =
            super::super::native_invocation::HostInvocationOwner::new(&host, run_auth.clone());
        port.unbound_completed(&last, &proof).unwrap();
        claim.retire_unbound(&proof).unwrap();
    }
    crate::effect_pump::reconcile_attempt(
        &f.host.owner.upgrade().unwrap(),
        &mut claim,
        &run_auth,
        &last,
    )
    .unwrap();
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut state = owner.spending_scope(&run_auth, &run).unwrap();
        owner.archive_retired_attempts(&mut state).unwrap();
        owner
            .publish_native(&run_auth, state, &last, vec![])
            .unwrap();
        let store = owner.owner().unwrap().store().unwrap();
        eprintln!("C5_FIX1_MAX_USAGE {:?}", store.usage().unwrap());
    }
    c5e_depth(&f, &mut maximum);
    c5e_storage(&f, &mut storage);
    claim.finish().unwrap();
    let record = spending_record(&f, &run);
    let state = f.projection(&run);
    assert_eq!(
        state.edit_approvals.len(),
        size.approvals(),
        "C5E_STOP_APPROVALS"
    );
    assert_eq!(
        state.edit_applications.len(),
        size.approvals(),
        "C5E_STOP_APPLICATIONS"
    );
    assert_eq!(
        state.edit_approvals.iter().collect::<BTreeSet<_>>().len(),
        size.approvals(),
        "C5E_DISTINCT_APPROVALS"
    );
    assert_eq!(
        state
            .edit_applications
            .iter()
            .collect::<BTreeSet<_>>()
            .len(),
        size.approvals(),
        "C5E_DISTINCT_APPLICATIONS"
    );
    assert_eq!(state.cleanup, RunCleanup::Settled);
    assert!(state.attempts.is_empty());
    for (task, history, sends) in &histories {
        let t = record.tasks.iter().find(|t| t.task == task.0).unwrap();
        assert_eq!(t.sends.len(), *sends);
        if matches!(size, C5eSize::Maximal | C5eSize::SendsOnly) {
            assert_eq!(t.providers.len(), 3);
        } else {
            assert_eq!(t.providers.len(), size.rounds());
        }
        assert_eq!(t.usage.len(), sends * 2);
        assert_eq!(t.totals.token_lower_bound, *sends as u64 * 7);
        assert_eq!(c5e_history(&f, &run, task), *history);
    }
    for execution in &all_attempts {
        assert!(matches!(
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .recovery_attempt(&state, &crate::native_launcher::native_identity(execution))
                .unwrap()
                .unwrap()
                .state,
            AttemptState::Retired(_)
        ));
    }
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let cuts = owner
            .owner_mut()
            .unwrap()
            .verify_retained_run_after_collection(&run)
            .unwrap();
        eprintln!("C5_LINEAR_GC_ALL_CUTS {cuts} receipts_and_complete_references=true");
    }
    let f = c4_restart(f, &claim_root);
    assert_eq!(
        spending_record(&f, &run),
        record,
        "C5_FIX1_MAX_REOPEN_ACCOUNTING"
    );
    assert_eq!(f.projection(&run), state, "C5_FIX1_MAX_REOPEN_PROJECTION");
    for (task, history, _) in &histories {
        assert_eq!(c5e_history(&f, &run, task), *history);
    }
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .core()
            .export_history()
            .unwrap(),
        manual,
        "C5_FIX1_MAX_REOPEN_MANUAL"
    );
    c5e_depth(&f, &mut maximum);
    c5e_storage(&f, &mut storage);
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let approvals = owner.edit_approval_details(&state);
    assert_eq!(
        approvals.len(),
        size.approvals(),
        "C5E_REOPEN_APPROVAL_COUNT"
    );
    assert!(
        approvals.iter().all(|row| row.applied
            && row.removed_clips == 1
            && row.removed_tracks == 0
            && row.issue.is_none()),
        "C5E_REOPEN_APPLICATION_MARKERS {approvals:?}"
    );
    drop(owner);
    eprintln!(
        "C5E_RETAINED_MAXIMA {storage:?} max_depth={maximum} approvals={} h1_edit_publications={}",
        size.approvals(),
        size.approvals() * 2
    );
    eprintln!(
        "C5_FIX1_MAXIMAL clocks={} tasks={} sends={} attempts={} max_depth={maximum} stop=true cleanup=true reopen_exact=true",
        size.clocks(),
        size.tasks(),
        (0..size.tasks()).map(|i| size.sends(i)).sum::<usize>(),
        all_attempts.len()
    );
    if matches!(size, C5eSize::Maximal | C5eSize::SendsOnly) {
        {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            eprintln!(
                "C5_LINEAR_FINAL {}",
                serde_json::to_string(&owner.owner().unwrap().retained_run_profile(&run).unwrap())
                    .unwrap()
            );
        }
        let Fixture { host, temp, .. } = f;
        drop(host);
        for sample in 0..7 {
            let started = Instant::now();
            let project = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
            let opened = started.elapsed();
            assert_eq!(
                project.orchestration_projection(&run).unwrap().unwrap(),
                state
            );
            eprintln!(
                "C5_OPEN approvals={} sample={sample} seconds={:.6}",
                size.approvals(),
                opened.as_secs_f64()
            );
            drop(project);
        }
    }
}

#[test]
fn s4b_c5e_cumulative_reopen_prefix_and_lost_ack() {
    use super::super::native_invocation::SpendingChange;
    let f = Fixture::new();
    f.host.runtime.stop();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let raw = serde_json::json!({"type":"turn.completed","id":"completed","usage":{"input_tokens":8,"output_tokens":2,"cached_input_tokens":3,"reasoning_output_tokens":1}});
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &raw, 1, false)
            .unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &raw, 2, false)
            .unwrap();
    }
    let before = spending_record(&f, &run);
    spending_change(
        &f,
        &execution,
        SpendingChange::Clock,
        before.last_unix_ms + 1000,
        1000,
        true,
    )
    .unwrap();
    let record = spending_record(&f, &run);
    assert_eq!(record.tasks[0].usage.len(), 1);
    assert_eq!(record.tasks[0].totals.token_lower_bound, 10);
    let mut owner = host.lock().unwrap();
    let root = owner.owner().unwrap().root_sha256().unwrap();
    let mut forged = record.successor().unwrap();
    forged.tasks[0].usage.clear();
    forged.tasks[0].totals = kinewright_project::orchestration_accounting::UsageTotals::default();
    assert!(
        owner
            .publish_accounting(&producer, &execution, &forged)
            .is_err(),
        "C5E_CUMULATIVE_PREFIX_REQUIRED"
    );
    assert_eq!(owner.owner().unwrap().root_sha256().unwrap(), root);
    let mut next = record.successor().unwrap();
    next.observe_clock(record.last_unix_ms + 1000, 1000, true)
        .unwrap();
    let result = kinewright_project::io_observer::fail_nth("root_parent_sync", 1, || {
        owner.publish_accounting(&producer, &execution, &next)
    });
    assert!(result.is_err(), "C5E_LOST_ACK_CUT");
    let pending = owner.hooks.pending.clone();
    assert!(pending.is_some(), "C5E_RETAIN_UNCERTAIN_CUMULATIVE");
    assert!(
        owner
            .publish_accounting(&producer, &execution, &next.successor().unwrap())
            .is_err(),
        "C5E_NO_SUBSTITUTED_CUMULATIVE_RETRY"
    );
    assert!(
        owner.hooks.pending == pending,
        "C5E_PENDING_IDENTITY_RETAINED"
    );
    owner.reconcile_native_pending(&producer).unwrap();
    assert!(owner.hooks.pending.is_none());
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    assert_eq!(
        owner.native_accounting(&state).unwrap().unwrap(),
        next,
        "C5E_ORIGINAL_RECONCILED"
    );
    drop(owner);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
    let path = f.temp.path("project.kinewright");
    let Fixture { host, temp, .. } = f;
    drop(host);
    let reopened = Ja1EditingSession::open(&path).unwrap();
    let projection = reopened.orchestration_projection(&run).unwrap().unwrap();
    let saved = projection
        .usage
        .iter()
        .find_map(|reference| {
            let record = reopened
                .store()
                .unwrap()
                .read_record::<OrchestrationRecord>(reference)
                .unwrap();
            if let OrchestrationPayload::Accounting(record) = &record.payload {
                Some((**record).clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(saved, next, "C5E_REOPEN_CURRENT_CUMULATIVE_EXACT");
    drop(reopened);
    drop(temp);
}

#[test]
#[ignore = "on-demand: paired 81-approval open measurement"]
#[allow(
    clippy::too_many_lines,
    reason = "the matched before/after workload preserves the original maximal setup"
)]
fn s4b_c5e_open_81_approval_measure() {
    let approvals = std::env::var("KINEWRIGHT_C5_OPEN_APPROVALS")
        .unwrap_or_else(|_| "81".into())
        .parse::<usize>()
        .unwrap();
    c5e_prefix_measure(approvals, C5ePrefixEvidence::Open);
}

#[test]
#[ignore = "on-demand: pending checkpoint history at 8/16/32/64 approval cycles"]
fn s4b_c5e_pending_history_capacity_measure() {
    c5e_prefix_measure(64, C5ePrefixEvidence::History);
}

#[derive(Clone, Copy)]
enum C5ePrefixEvidence {
    Open,
    History,
    Boundary,
}

#[allow(
    clippy::too_many_lines,
    reason = "matched maximal setup for open and pending-history measurements"
)]
fn c5e_prefix_measure(approvals: usize, evidence: C5ePrefixEvidence) {
    use super::super::native_invocation::SpendingChange;
    let size = C5eSize::Small;
    let mut document = Document::default();
    kinewright_core::apply_batch(
        &mut document,
        &[kinewright_core::Operation::AddTrack {
            track: kinewright_core::Track {
                id: kinewright_core::TrackId(81),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: false,
                clips: vec![],
            },
        }],
    )
    .unwrap();
    for clip in 0..4 {
        kinewright_core::apply_batch(
            &mut document,
            &[kinewright_core::Operation::AddSolidClip {
                track: kinewright_core::TrackId(81),
                timeline_start: kinewright_core::TimeCode(i64::from(clip) * 10),
                duration: kinewright_core::TimeCode(10),
                color: kinewright_core::SolidColor::default(),
            }],
        )
        .unwrap();
    }
    let mut f = Fixture::with_inputs(false, document, "project.kinewright", |bundle| {
        bundle.empty_inputs().unwrap().reference.clone()
    });
    f.host.runtime.stop();
    let scope_document = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap()
        .document;
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scope_document, f.brief.scope.base.clone())
            .unwrap();
    // Reserve the two marker identities used by the existing redo witness.
    f.brief
        .scope
        .objects
        .extend(["marker:1".into(), "marker:2".into()]);
    let _manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        for id in [50, 51] {
            owner
                .core()
                .request(kinewright_core::Command::Do(history_marker(id)))
                .unwrap();
        }
        owner
            .core()
            .request(kinewright_core::Command::Undo)
            .unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
        owner.core().export_history().unwrap()
    };
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"hook plan, script and assignment")
        .unwrap();
    let (run, parent, _) = reserve(&f);
    let run_auth = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .prepare_task_endpoint(
            &run_auth,
            &run,
            &parent.task,
            &parent.attempt,
            Duration::from_secs(3600),
        )
        .unwrap();
    let director = C4Mcp::new(&f, &run, &parent.task);
    let now = native_now(&f, &run);
    let mut maximum = 0;
    let mut storage = C5eStorage::default();
    let sequence = f.projection(&run).sequence;
    for tick in 1..size.clocks() {
        assert!(
            spending_change(
                &f,
                &parent,
                SpendingChange::Clock,
                now + tick * 30_000,
                30_000,
                true
            )
            .is_ok(),
            "C5_FIX1_CLOCK {tick}"
        );
        assert_eq!(
            f.projection(&run).sequence,
            sequence,
            "C5_FIX1_CLOCK_REPLACES"
        );
        if tick % 120 == 0 {
            eprintln!("C5_FIX1_CLOCK {tick}");
            c5e_depth(&f, &mut maximum);
            c5e_storage(&f, &mut storage);
        }
    }
    let claim_root = f.temp.path("c5-fix1-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();

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
    let task = f.projection(&run).tasks.last().unwrap().id.clone();
    let (execution, client, endpoint, core) =
        c4_reserve_task_with_ttl(&f, &run, &task, Duration::from_secs(3600));
    claim.reserve(&unbound_identity(&execution)).unwrap();
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    c5e_approval_edits(&f, &execution, &core, approvals, &mut maximum, &mut storage);
    if matches!(evidence, C5ePrefixEvidence::Boundary) {
        c5e_seeded_history_boundary(&f, &execution, &endpoint, &core);
        return;
    }
    if matches!(evidence, C5ePrefixEvidence::History) {
        c4_mcp_edit_redo(&C4Mcp::new(&f, &run, &task), &core);
        c5e_pending_history(&f, &core, approvals, "final_redo");
        return;
    }
    let expected = f.projection(&run);
    drop(director);
    drop(client);
    drop(endpoint);
    drop(core);
    drop(claim);
    let Fixture { host, temp, .. } = f;
    drop(host);
    let path = temp.path("project.kinewright");
    for sample in 0..7 {
        let started = Instant::now();
        let project = Ja1EditingSession::open(&path).unwrap();
        let opened = started.elapsed();
        assert_eq!(
            project.orchestration_projection(&run).unwrap().unwrap(),
            expected
        );
        eprintln!(
            "C5_OPEN approvals={approvals} sample={sample} seconds={:.6}",
            opened.as_secs_f64()
        );
        drop(project);
    }
}

#[test]
fn c5_linear_context_capacity_refuses_before_mutation_and_stop_still_works() {
    use kinewright_project::orchestration_records::{
        TASK_DECISION_CAPACITY_WORDS, TASK_DECISION_LIMIT, TASK_FEEDBACK_CAPACITY_WORDS,
        TASK_FEEDBACK_LIMIT,
    };
    for (feedback, limit, code, words) in [
        (
            true,
            TASK_FEEDBACK_LIMIT,
            "task_feedback_capacity",
            TASK_FEEDBACK_CAPACITY_WORDS,
        ),
        (
            false,
            TASK_DECISION_LIMIT,
            "task_decision_capacity",
            TASK_DECISION_CAPACITY_WORDS,
        ),
    ] {
        let f = Fixture::new();
        f.host.runtime.stop();
        let (run, execution, producer) = reserve(&f);
        let (actor, endpoint) = task_actor(&f, &run, &execution, &producer);
        let before = f.projection(&run);
        let progress = &before
            .tasks
            .iter()
            .find(|t| t.id == execution.task)
            .unwrap()
            .progress;
        let current = if feedback {
            progress.feedback.len()
        } else {
            progress.decisions.len()
        };
        let refs = vec![f.brief.plan.clone(); limit - current];
        let (feedback_refs, decision_refs) = if feedback {
            (refs, vec![])
        } else {
            (vec![], refs)
        };
        let command = OrchestrationCommand::RecordContext {
            task: execution.task.clone(),
            feedback: feedback_refs,
            decisions: decision_refs,
        };
        let request = task_request(&f, &actor, &endpoint, command);
        ok(
            call(
                &f,
                &actor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                },
                1,
            ),
            "C5_CONTEXT_AT_LIMIT",
        );
        let command = OrchestrationCommand::RecordContext {
            task: execution.task.clone(),
            feedback: if feedback {
                vec![f.brief.plan.clone()]
            } else {
                vec![]
            },
            decisions: if feedback {
                vec![]
            } else {
                vec![f.brief.plan.clone()]
            },
        };
        let request = task_request(&f, &actor, &endpoint, command);
        let original = state(&f);
        let projection = f.projection(&run);
        let reply = call(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            1,
        );
        let error = reply.error.expect("the specific context-capacity refusal");
        assert_eq!(error.code, code);
        assert_eq!(error.message, words);
        assert_eq!(state(&f), original, "C5_CONTEXT_REFUSAL_NO_MUTATION {code}");
        assert_eq!(f.projection(&run), projection);
        let request = f.control(&f.editor, &run, RunAction::Stop);
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                })
                .unwrap(),
            "C5_CONTEXT_FULL_STOP",
        );
    }
}

#[test]
#[ignore = "on-demand: C3 prospective history boundary after the C5 merge"]
fn s4b_c5e_merged_history_refusal_measure() {
    c5e_prefix_measure(1, C5ePrefixEvidence::Boundary);
}

// Only history transitions are seeded; the first destructive edit and final
// refusal use MCP. This is a counter probe, not the full H1 publication proof.
#[allow(
    clippy::too_many_lines,
    reason = "one bounded probe measures the prospective charge and verifies the real refusal before mutation"
)]
fn c5e_seeded_history_boundary(
    f: &Fixture,
    execution: &ExecutionIdentity,
    endpoint: &crate::orchestration::tasks::TaskEndpoint,
    core: &Core,
) {
    use kinewright_core::{Command, Event, Operation};
    use kinewright_project::task_history_admission::{
        LIMIT_REASON, decoded_history_bytes_for_fixture,
    };
    let mcp = C4Mcp::new(f, &execution.run, &execution.task);
    for cycle in 2..=256 {
        for undo in [false, true] {
            let before = core.export_history().unwrap();
            // Both actors read the immutable replay baseline; the owned history
            // moves into the private actor and the live actor remains untouched.
            let prospective = Core::spawn(std::sync::Arc::clone(&before.baseline)).unwrap();
            prospective.restore_history(before).unwrap();
            let command = || {
                if undo {
                    Command::Undo
                } else {
                    Command::DoBatch(vec![Operation::DeleteClip {
                        clip: kinewright_core::ClipId(1),
                    }])
                }
            };
            assert!(matches!(
                prospective.request(command()).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let history = prospective.export_history().unwrap();
            let encoded = serde_json::to_vec(&history).unwrap().len();
            let charge = decoded_history_bytes_for_fixture(&history).unwrap();
            let limit = kinewright_project::run_admission::HISTORY_BYTES;
            if charge > limit || u64::try_from(encoded).unwrap() > limit {
                assert!(
                    endpoint
                        .check_history(&history)
                        .unwrap_err()
                        .contains(LIMIT_REASON)
                );
                let before = core.export_history().unwrap();
                let projection = f.projection(&execution.run);
                let host = f.host.owner.upgrade().unwrap();
                let root = host.lock().unwrap().owner().unwrap().root_sha256().unwrap();
                let refused = if undo {
                    mcp.invoke(
                        "undo_task_edit",
                        serde_json::json!({"expected_revision":grant_test_revision(core)}),
                    )
                } else {
                    mcp.raw(
                        "prepare_edit_plan",
                        serde_json::json!({
                            "expected_revision":grant_test_revision(core),
                            "operations":[{"op":"delete_clip","clip":1}]
                        }),
                    )
                };
                assert_eq!(
                    refused.is_error,
                    Some(true),
                    "C5_C3_PRE_MUTATION_REFUSAL {refused:?}"
                );
                assert!(
                    format!("{refused:?}").contains(LIMIT_REASON),
                    "C5_C3_SPECIFIC_REFUSAL {refused:?}"
                );
                assert_eq!(
                    core.export_history().unwrap(),
                    before,
                    "C5_C3_NO_LIVE_MUTATION"
                );
                assert_eq!(
                    f.projection(&execution.run),
                    projection,
                    "C5_C3_NO_H1_PUBLICATION"
                );
                assert_eq!(
                    host.lock().unwrap().owner().unwrap().root_sha256().unwrap(),
                    root
                );
                eprintln!(
                    "C5_C3_BOUNDARY cycle={cycle} phase={} history_encoded={encoded} history_decoded={charge} limit={limit} refusal={refused:?}",
                    if undo { "undo" } else { "delete" }
                );
                return;
            }
            endpoint.check_history(&history).unwrap();
            assert!(matches!(
                core.request(command()).unwrap(),
                Event::DocumentChanged { .. }
            ));
            assert_eq!(
                core.export_history().unwrap(),
                history,
                "C5_C3_PROSPECTIVE_EQUALS_APPLIED"
            );
            if undo && matches!(cycle, 8 | 16 | 32 | 64) {
                eprintln!(
                    "C5_C3_SEEDED_HISTORY cycles={cycle} history_encoded={encoded} history_decoded={charge}"
                );
                if cycle == 64 {
                    assert_eq!(
                        (encoded, charge),
                        (22_770, 367_337),
                        "C5_C3_CALIBRATES_TO_REAL_64_CYCLE_MEASUREMENT"
                    );
                }
            }
        }
    }
    eprintln!("C5_C3_BOUNDARY all_256_fit=true");
}
