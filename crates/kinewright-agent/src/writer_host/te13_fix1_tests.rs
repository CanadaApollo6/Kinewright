#[test]
fn te13_fix1_admitted_edit_cannot_race_an_accepted_finish() {
    let r = c5_fix10_run();
    let finish = te13_control(&r.editor, &r.child, &r.core);
    let revision = grant_test_revision(&r.core);
    let peer = C4Mcp::new(&r.f, &r.run, &r.child);
    let prepared = peer.raw("prepare_edit_plan", serde_json::json!({"expected_revision":revision,"operations":[{"op":"move_marker","marker":1,"to":2}]}));
    assert_eq!(
        prepared.is_error,
        Some(false),
        "TE13_FIX1_RACE_PREPARE {prepared:?}"
    );
    let endpoint = r.f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&r.run].tasks
        [&r.child]
        .endpoint();
    let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
    let released = std::sync::Arc::new(std::sync::Barrier::new(2));
    let (a, b) = (entered.clone(), released.clone());
    endpoint.hold_next_admission_for_fixture(std::sync::Arc::new(move || {
        a.wait();
        b.wait();
    }));
    let thread = std::thread::spawn(move || {
        peer.wire("commit_edit_plan", serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}))
    });
    entered.wait();
    let probe = endpoint.begin_finish();
    let busy = probe.is_err();
    drop(probe);
    if !busy {
        released.wait();
        thread.join().unwrap();
        assert!(busy, "TE13_FIX1_RACE_DRAIN_BEFORE_REVISION");
        return;
    }
    let refused = r.editor.wire("finish_task", finish.clone());
    let accepted = r.f.projection(&r.run).tasks[1]
        .progress
        .completion
        .is_some();
    released.wait();
    let edited = thread.join().unwrap();
    assert!(!accepted, "TE13_FIX1_RACE_NO_STALE_INTENT {refused:?}");
    assert_eq!(
        edited.is_error,
        Some(false),
        "TE13_FIX1_RACE_EDIT {edited:?}"
    );
    let stale = r.editor.wire("finish_task", finish);
    assert!(
        r.f.projection(&r.run).tasks[1]
            .progress
            .completion
            .is_none(),
        "TE13_FIX1_RACE_REVISION_REFUSED {stale:?}"
    );
    let fresh = te13_control(&r.editor, &r.child, &r.core);
    let completed = r.editor.wire("finish_task", fresh);
    assert_eq!(
        completed.is_error,
        Some(false),
        "TE13_FIX1_RACE_FINISH {completed:?}"
    );
}

#[test]
fn te13_fix1_narrow_reopen_never_unseals_accepted_or_frozen_ingress() {
    let r = c5_fix10_run();
    let endpoint = r.f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&r.run].tasks
        [&r.child]
        .endpoint();
    {
        let _drain = endpoint.begin_finish().unwrap();
    }
    assert!(endpoint.admit().is_ok(), "TE13_FIX1_UNACCEPTED_REOPENS");
    let mut drain = endpoint.begin_finish().unwrap();
    drain.publication_unknown();
    drop(drain);
    assert!(endpoint.admit().is_err(), "TE13_FIX1_UNKNOWN_STAYS_FENCED");
    endpoint.reconcile_unapplied_finish();
    assert!(endpoint.admit().is_ok());
    let mut drain = endpoint.begin_finish().unwrap();
    drain.accept();
    drop(drain);
    endpoint.reconcile_unapplied_finish();
    assert!(
        endpoint.admit().is_err(),
        "TE13_FIX1_ACCEPTED_NEVER_REOPENS"
    );
    let frozen = c5_fix10_freeze(&r.editor, &r.child);
    assert!(frozen.head.is_some(), "TE13_FIX1_REAL_SUCCESSFUL_FREEZE");
    {
        let mut drain = endpoint.begin_finish().unwrap();
        drain.publication_unknown();
    }
    endpoint.reconcile_unapplied_finish();
    assert!(endpoint.admit().is_err(), "TE13_FIX1_FROZEN_NEVER_REOPENS");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "classification, ready send and original receipt retry share fixed identities"
)]
fn te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache() {
    let r = c5_fix10_run();
    let control = te13_control(&r.editor, &r.child, &r.core);
    let host = r.f.host.owner.upgrade().unwrap();
    host.lock().unwrap().hooks.completion_before_freeze = true;
    let first = r.editor.wire("finish_task", control.clone());
    assert_eq!(first.is_error, Some(false));
    assert!(
        host.lock().unwrap().hooks.runs[&r.run]
            .recovery_blocks
            .is_empty(),
        "TE13_FIX1_RETRYABLE_NOT_BLOCKED"
    );
    // The Director has pending process-bound work and a free slot. Retire its
    // prior attempt so the recovery fence is the only reason admission refuses.
    c5_fix10_retire(&r.f, &r.run, &r.parent);
    // A changed actor revision is a permanent invariant failure after intent.
    // Direct Core access models an owner-side invariant violation, not an MCP edit.
    r.core.request(kinewright_core::Command::Undo).unwrap();
    r.editor.wire("finish_task", control.clone());
    assert!(
        !host.lock().unwrap().hooks.runs[&r.run]
            .recovery_blocks
            .is_empty(),
        "TE13_FIX1_PERMANENT_BLOCK"
    );
    // A later finish can clear the transient last-failure field. The installed
    // recovery cause must still report this task as Blocked until it resolves.
    let state = r.f.projection(&r.run);
    let mut owner = host.lock().unwrap();
    owner.hooks.completion_failure = None;
    let status = owner
        .completion_statuses(&state)
        .unwrap()
        .into_iter()
        .find(|status| status.task == r.child)
        .unwrap();
    assert_eq!(
        status.state,
        super::super::task_completion::CompletionState::Blocked,
        "TE13_FIX1_PERMANENT_STATUS_SURVIVES_LAST_FAILURE"
    );
    drop(owner);
    let producer = r.f.producer(&r.run);
    assert!(
        host.lock()
            .unwrap()
            .reserve_native_effect(&producer, &r.run)
            .unwrap()
            .is_none(),
        "TE13_FIX1_PERMANENT_NO_CHARGE"
    );
    let cached = std::mem::take(
        &mut host
            .lock()
            .unwrap()
            .hooks
            .runs
            .get_mut(&r.run)
            .unwrap()
            .recovery_blocks,
    );
    let ready = host
        .lock()
        .unwrap()
        .reserve_native_effect(&producer, &r.run)
        .unwrap();
    host.lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&r.run)
        .unwrap()
        .recovery_blocks = cached;
    assert!(ready.is_some(), "TE13_FIX1_SEND_READY_WITHOUT_BLOCK");
    let (admitted, effect, kind) = ready.unwrap();
    host.lock()
        .unwrap()
        .prepare_task_endpoint(
            &producer,
            &r.run,
            &admitted.identity().task,
            &admitted.identity().attempt,
            Duration::from_secs(300),
        )
        .unwrap();
    let (profile, _, scratch) = native_profile_at(r.f.temp.root(), &admitted);
    let now = native_now(&r.f, &r.run);
    let before = spending_record(&r.f, &r.run);
    let charged = host.lock().unwrap().charge_native_invocation(
        &producer,
        &admitted,
        profile.clone(),
        &effect,
        kind,
        now,
    );
    let blocked =
        matches!(&charged, Err(error) if error.to_string().contains("recovery_requires_person"));
    drop(charged);
    if !blocked {
        std::fs::remove_dir_all(&scratch).unwrap();
        assert!(blocked, "TE13_FIX1_PRECOMMIT_BLOCKS_RESERVED_SEND");
    }
    assert_eq!(
        spending_record(&r.f, &r.run),
        before,
        "TE13_FIX1_NO_RESERVED_SEND_DEBIT"
    );
    let cached = std::mem::take(
        &mut host
            .lock()
            .unwrap()
            .hooks
            .runs
            .get_mut(&r.run)
            .unwrap()
            .recovery_blocks,
    );
    let possible = host
        .lock()
        .unwrap()
        .charge_native_invocation(&producer, &admitted, profile, &effect, kind, now);
    host.lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&r.run)
        .unwrap()
        .recovery_blocks = cached;
    let detail = possible
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    let permitted = possible.is_ok();
    drop(possible);
    std::fs::remove_dir_all(scratch).unwrap();
    assert!(
        permitted,
        "TE13_FIX1_PRECOMMIT_READY_WITHOUT_BLOCK {detail}"
    );
    // A second fixture's same-process missing receipt clears its old cache when
    // the original finish is retried; no new identity or endpoint is minted.
    let q = c5_fix10_run();
    let control = te13_control(&q.editor, &q.child, &q.core);
    te13_cut(&q.f, CompletionStep::Freeze);
    q.editor.wire("finish_task", control.clone());
    let host = q.f.host.owner.upgrade().unwrap();
    host.lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&q.run)
        .unwrap()
        .recovery_blocks
        .push(super::super::task_completion::RecoveryBlock::Completion(
            q.child.clone(),
        ));
    q.editor.wire("finish_task", control);
    assert!(
        host.lock().unwrap().hooks.runs[&q.run]
            .recovery_blocks
            .is_empty(),
        "TE13_FIX4_SUCCESS_CACHE_CLEARED"
    );
    // Independently exercise the filtered view even though successful live
    // continuation now removes its own cache entry immediately.
    host.lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&q.run)
        .unwrap()
        .recovery_blocks
        .push(super::super::task_completion::RecoveryBlock::Completion(
            q.child.clone(),
        ));
    let state = q.f.projection(&q.run);
    assert!(
        host.lock()
            .unwrap()
            .run_recovery_blocks(&state)
            .unwrap()
            .is_empty(),
        "TE13_FIX1_CLEARED_CACHE"
    );
}

fn te13_fix1_stop_host(host: &WriterHost, run: &RunId) {
    let client = host.client();
    let issued = Fixture::issue(&client);
    let state = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_projection(run)
        .unwrap()
        .unwrap();
    let mut envelope = OrchestrationEnvelope {
        id: issued.envelope.id.clone(),
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
        project: issued.envelope.project_id,
        expected_generation: issued.root_generation,
        authority: AuthorityBinding {
            grant_ref: issued.grant_ref,
            owner_epoch: issued.envelope.owner_epoch,
            endpoint: None,
            attempt: None,
        },
        command: OrchestrationCommand::ControlRun(RunIntent {
            run_id: run.clone(),
            expected_control_revision: state.control_revision,
            command_id: issued.envelope.id,
            action: RunAction::Stop,
        }),
    };
    envelope.digest = envelope.computed_digest().unwrap();
    let reply = client
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(ScopedOrchestrationRequest {
                run: Some(run.clone()),
                envelope,
                consume_batch: None,
            }),
        })
        .unwrap();
    assert!(reply.error.is_none(), "TE13_FIX1_STOP {reply:?}");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "real crash, Stop, owner selection and second reopen share one history"
)]
fn te13_fix1_crash_stop_recovery_preserves_both_histories_and_original_merge() {
    for phase in ["before-freeze", "freeze", "merge", "published-child"] {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "te13-fix1-recover",
        );
        let handoff = temp.path("handoff.json");
        let output = std::fs::File::create(temp.path("child.log")).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                if phase == "merge" {
                    "writer_host::hooks_tests::tests::s4b_c4_fix1_merge_crash_child"
                } else {
                    "writer_host::hooks_tests::tests::te13_finish_crash_child"
                },
                "--nocapture",
            ])
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        if phase == "merge" {
            command
                .env("KINEWRIGHT_C4_MERGE_CUT", "before_candidate_commit")
                .env("KINEWRIGHT_C4_MERGE_HANDOFF", &handoff);
        } else {
            command
                .env("KINEWRIGHT_TE13_HANDOFF", &handoff)
                .env("KINEWRIGHT_TE13_PHASE", phase);
        }
        let result = command.status().unwrap();
        assert_eq!(
            result.code(),
            Some(86),
            "TE13_FIX1_CRASH {}",
            std::fs::read_to_string(temp.path("child.log")).unwrap()
        );
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
        let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
        let run: RunId = serde_json::from_value(data["run"].clone()).unwrap();
        let host = crash_host(&root);
        let owner = host.owner.upgrade().unwrap();
        let (producer, state) = owner.lock().unwrap().restore_native_run(&run).unwrap();
        let work = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap()
            .into_iter()
            .find(|work| work.run == run)
            .expect("TE13_FIX1_RETAINED_WORK");
        let task = work.task.clone();
        let saved = owner
            .lock()
            .unwrap()
            .saved_task_history(&state, &task)
            .unwrap()
            .unwrap();
        let checkpoint = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &saved.source.checkpoint,
            )
            .unwrap();
        let expected = (*owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
            .unwrap())
        .clone();
        if phase != "merge" {
            assert_eq!(
                serde_json::to_value(&expected).unwrap(),
                data["history"],
                "TE13_FIX1_CRASH_LATEST_HISTORY"
            );
        }
        assert!(!expected.cursor.redo.is_empty(), "TE13_FIX1_REAL_REDO");
        // Public production endpoint creation, with no test-only new reservation.
        let director = state
            .tasks
            .iter()
            .find(|task| task.parent.is_none())
            .unwrap()
            .id
            .clone();
        assert!(
            owner
                .lock()
                .unwrap()
                .start_task_server(&producer, &run, &director)
                .is_err(),
            "TE13_FIX1_REOPENED_DIRECTOR_NO_AUTHORITY"
        );
        assert!(
            owner
                .lock()
                .unwrap()
                .owner_mut()
                .unwrap()
                .prepare_recovery(
                    kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                        saved.source.branch.clone()
                    )
                )
                .is_err(),
            "TE13_FIX1_LIVE_PERSON_SELECTION_REFUSED"
        );
        te13_fix1_stop_host(&host, &run);
        let stopped = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        assert!(
            owner
                .lock()
                .unwrap()
                .run_recovery_message(&stopped)
                .unwrap()
                .is_some_and(|text| text.contains("Save current edit, then open")),
            "TE13_FIX1_STOP_DISCLOSURE"
        );
        assert!(
            owner
                .lock()
                .unwrap()
                .merge_task_history(
                    &producer,
                    &run,
                    &task,
                    saved.merge_envelope.as_ref().map_or_else(
                        || CommandId {
                            issuer: identity(),
                            sequence: 1
                        },
                        |e| e.id.clone()
                    )
                )
                .is_err(),
            "TE13_FIX1_RETIRED_MERGE_REFUSED"
        );
        drop(owner);
        drop(host);
        let path = root.join("project.kinewright");
        let mut owner = Ja1EditingSession::open(&path).unwrap();
        let work = owner.retained_task_work().unwrap();
        assert!(
            work.iter()
                .any(|w| w.task == task && w.disclosure.contains("Save current edit, then open")),
            "TE13_FIX1_REOPEN_DISCLOSURE"
        );
        owner
            .core()
            .request(kinewright_core::Command::Do(history_marker(50)))
            .unwrap();
        owner
            .core()
            .request(kinewright_core::Command::Do(history_marker(51)))
            .unwrap();
        owner
            .core()
            .request(kinewright_core::Command::Undo)
            .unwrap();
        let manual = owner.core().export_history().unwrap();
        let manual_branch = owner.current_switch_identity().unwrap().current.branch;
        let target = kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
            saved.source.branch.clone(),
        );
        assert!(
            owner
                .retirement_disclosure(&saved.source.branch)
                .unwrap()
                .contains("retained task"),
            "TE13_FIX1_REMOVE_WARNING"
        );
        owner.prepare_recovery(target.clone()).unwrap();
        assert!(
            matches!(
                owner.select_local(),
                kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
            ),
            "TE13_FIX1_RECOVERY_ACTIVATED"
        );
        assert_eq!(
            owner.core().export_history().unwrap(),
            expected,
            "TE13_FIX1_RECOVER_EXACT_TASK"
        );
        assert_ne!(
            owner.current_switch_identity().unwrap().current.branch,
            saved.source.branch,
            "TE13_FIX1_RECOVERY_FORKS_SOURCE"
        );
        let first_root = owner.root_sha256().unwrap();
        let repeat = owner.prepare_recovery(target);
        assert!(repeat.is_ok(), "TE13_FIX1_RECOVERY_REPEAT {repeat:?}");
        assert!(matches!(
            owner.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ));
        assert_eq!(
            owner.root_sha256().unwrap(),
            first_root,
            "TE13_FIX1_RECOVERY_ONCE"
        );
        let state = owner.orchestration_projection(&run).unwrap().unwrap();
        // Task history and original merge envelope remain part of the stopped run.
        let mut found = None;
        for reference in &state
            .tasks
            .iter()
            .find(|t| t.id == task)
            .unwrap()
            .progress
            .decisions
        {
            if reference.record_kind == kinewright_project::bundle_records::RecordKind::RunIndex
                && let OrchestrationPayload::TaskHistory(history) = owner
                    .store()
                    .unwrap()
                    .read_run_record(reference)
                    .unwrap()
                    .payload
            {
                found = Some(*history);
            }
        }
        assert_eq!(
            found,
            Some(saved.clone()),
            "TE13_FIX1_ORIGINAL_HISTORY_ENVELOPE"
        );
        if let Some(envelope) = &saved.merge_envelope {
            assert_eq!(
                owner.manual_transition_receipt(envelope).unwrap(),
                kinewright_project::receipts::ReceiptLookup::Absent,
                "TE13_FIX1_NO_RETIRED_MERGE"
            );
        }
        drop(owner);
        if let Some(envelope) = &saved.merge_envelope {
            let replay_host = crash_host(&root);
            let retained = replay_host.owner.upgrade().unwrap();
            {
                let mut host = retained.lock().unwrap();
                let producer = host.restore_native_run(&run).unwrap().0;
                let retry = host.merge_task_history(&producer, &run, &task, envelope.id.clone());
                assert!(
                    retry.is_err(),
                    "TE13_FIX1_ORIGINAL_MERGE_AFTER_ACTION_REFUSED"
                );
                assert_eq!(
                    host.core().export_history().unwrap(),
                    expected,
                    "TE13_FIX1_RETIRED_RETRY_NO_SECOND_MERGE"
                );
                assert_eq!(
                    host.owner()
                        .unwrap()
                        .manual_transition_receipt(envelope)
                        .unwrap(),
                    kinewright_project::receipts::ReceiptLookup::Absent,
                    "TE13_FIX1_RETIRED_RETRY_NO_RECEIPT"
                );
            }
            drop(retained);
            drop(replay_host);
        }
        let owner = Ja1EditingSession::open(&path).unwrap();
        assert_eq!(
            owner.core().export_history().unwrap(),
            expected,
            "TE13_FIX1_SECOND_REOPEN_TASK"
        );
        drop(owner);
        let loaded = kinewright_project::bundle::LoadedBundle::read(&path)
            .unwrap()
            .into_writer()
            .unwrap();
        assert!(
            loaded.branches_probe().contains_key(&manual_branch),
            "TE13_FIX1_PERSON_BRANCH_RETAINED"
        );
        let manual_checkpoint = loaded.branches_probe().get(&manual_branch).unwrap();
        let retained = loaded
            .store()
            .read_record::<kinewright_core::HistorySnapshot>(&manual_checkpoint.history_ref)
            .unwrap();
        assert_eq!(*retained, manual, "TE13_FIX1_PERSON_HISTORY_SAVED");
        drop(retained);
        drop(loaded);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn te13_fix1_exhausted_run_refuses_finish_without_using_reserve() {
    let r = c5_fix10_run();
    let control = te13_control(&r.editor, &r.child, &r.core);
    let host = r.f.host.owner.upgrade().unwrap();
    {
        let mut host = host.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        owner.store().unwrap().exhaust_run_budget_probe();
    }
    let before = r.core.export_history().unwrap();
    let reply = r.editor.wire("finish_task", control);
    assert_eq!(
        reply.is_error,
        Some(true),
        "TE13_FIX1_RUN_LIMIT_FINISH {reply:?}"
    );
    assert!(
        serde_json::to_string(&reply).unwrap().contains("run_limit"),
        "TE13_FIX1_RUN_LIMIT_CODE {reply:?}"
    );
    assert!(
        r.f.projection(&r.run).tasks[1]
            .progress
            .completion
            .is_none(),
        "TE13_FIX1_NO_EXHAUSTED_INTENT"
    );
    assert_eq!(r.core.export_history().unwrap(), before);
}

#[test]
fn te13_fix1_merge_receipt_clears_live_recovery_cache() {
    let r = c5_fix10_run();
    let finish = te13_control(&r.editor, &r.child, &r.core);
    r.editor.wire("finish_task", finish);
    let host = r.f.host.owner.upgrade().unwrap();
    host.lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&r.run)
        .unwrap()
        .recovery_blocks
        .push(super::super::task_completion::RecoveryBlock::Merge(
            r.child.clone(),
        ));
    let control = r
        .director
        .control(OrchestrationCommand::MergeTask(MergeTask {
            task: r.child.clone(),
            target: r.f.brief.scope.base.clone(),
            result: r.f.projection(&r.run).results.last().unwrap().clone(),
        }));
    let first = r.director.wire("merge_task", control.clone());
    assert_eq!(
        first.is_error,
        Some(false),
        "TE13_FIX1_MERGE_COMPLETE {first:?}"
    );
    let state = r.f.projection(&r.run);
    assert!(
        host.lock()
            .unwrap()
            .run_recovery_blocks(&state)
            .unwrap()
            .is_empty(),
        "TE13_FIX1_MERGE_CACHE_CLEARED"
    );
    let retry = r.director.wire("merge_task", control);
    assert_eq!(
        retry.structured_content.unwrap()["outcome"],
        first.structured_content.unwrap()["outcome"],
        "TE13_FIX1_MERGE_ORIGINAL_OUTCOME"
    );
}

#[test]
fn te13_fix1_refused_freeze_receipt_does_not_advance_phase() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let control = te13_control(&mcp, &execution.task, &te13_core(&f, &run, &execution.task));
    let host = f.host.owner.upgrade().unwrap();
    host.lock().unwrap().hooks.completion_before_freeze = true;
    mcp.wire("finish_task", control);
    {
        let mut host = host.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let mut state = owner.orchestration_projection(&run).unwrap().unwrap();
        let outer = owner
            .orchestration_receipt(
                &run,
                &state.tasks[0].progress.completion.as_ref().unwrap().id,
            )
            .unwrap()
            .unwrap();
        let mut envelope = outer.envelope;
        envelope.id = state.tasks[0]
            .progress
            .completion
            .as_ref()
            .unwrap()
            .step_id(CompletionStep::Freeze)
            .unwrap();
        envelope.command = OrchestrationCommand::FreezeTask(execution.task.clone());
        envelope.expected_generation = owner.metadata_generation().unwrap();
        envelope.digest = envelope.computed_digest().unwrap();
        state.sequence += 1;
        let segment = EventSegment {
            run: run.clone(),
            first_sequence: state.sequence,
            previous: Some(owner.orchestration_cut(&run).unwrap().unwrap().segment),
            events: vec![OrchestrationEvent {
                command: envelope.id.clone(),
                sequence: state.sequence,
                projection: state.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &envelope.project,
            envelope.expected_generation,
            &envelope,
            state,
            segment,
            CommandDecision::Refused(
                kinewright_project::orchestration_commit::RecordedRefusal::InvalidState,
            ),
        )
        .unwrap();
        kinewright_project::orchestration_commit::commit_orchestration(owner, update, None)
            .unwrap();
    }
    let status = te13_status(&f, &run, &execution.task);
    assert_eq!(
        status.phase,
        super::super::CompletionPhase::Freeze,
        "TE13_FIX1_REFUSED_FREEZE_PHASE"
    );
    assert!(
        status.result.is_none(),
        "TE13_FIX1_REFUSED_FREEZE_NO_RESULT"
    );
}
