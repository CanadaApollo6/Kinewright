fn cleanup_history(
    f: &Fixture,
    run: &RunId,
    child: &TaskId,
) -> kinewright_project::operation_history::OwnerSnapshot {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(run)
        .unwrap()
        .unwrap();
    let history = owner.saved_task_history(&state, child).unwrap().unwrap();
    let checkpoint = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &history.source.checkpoint,
        )
        .unwrap();
    checkpoint
        .open(
            owner.owner().unwrap().store().unwrap(),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .into_reader()
        .unwrap()
        .into_capture()
        .unwrap()
}

fn cleanup_accounting(
    f: &Fixture,
    state: &crate::orchestration::orchestration_records::RunProjection,
) -> (
    kinewright_project::bundle_records::ObjectRef,
    kinewright_project::orchestration_accounting::AccountingRecord,
) {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let store = owner.owner().unwrap().store().unwrap();
    let mut selected = None;
    for reference in &state.usage {
        let record = store.read_run_record(reference).unwrap();
        if let OrchestrationPayload::Accounting(accounting) = &record.payload {
            assert!(
                selected.is_none(),
                "C3_HAS_ONE_CUMULATIVE_ACCOUNTING_RECORD"
            );
            // Keep an independent oracle across cumulative publication and GC.
            selected = Some((reference.clone(), (**accounting).clone()));
        }
    }
    selected.expect("C3_HAS_NATIVE_ACCOUNTING")
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "dirty cleanup, private claim, two reopens and new-run oracle for each latch and overlap"
)]
fn run_limit_cleanup_dirty_stop_reopens_and_releases_every_latch() {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};
    for latch in [
        PersonLatch::Storage,
        PersonLatch::Specialist,
        PersonLatch::Sends,
        PersonLatch::Attempts,
        PersonLatch::Cuts,
    ] {
        for overlap in [false, true] {
            let (mut f, run, model) = person_latched_fixture_inner(latch, false, overlap);
            let before = f.projection(&run);
            let child = before.tasks[1].id.clone();
            let host = f.host.owner.upgrade().unwrap();
            let expected = host.lock().unwrap().hooks.runs[&run].tasks[&child]
                .core()
                .export_history()
                .unwrap();
            assert!(!expected.history().redo.is_empty(), "C3_DIRTY_REDO");
            assert_ne!(
                cleanup_history(&f, &run, &child),
                expected,
                "C3_NOT_ALREADY_CHECKPOINTED"
            );
            let machine = f.temp.path("cleanup-machine");
            let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
                &machine,
                before.attempts[0].identity.project.as_str(),
                run.0.as_str(),
            )
            .unwrap();
            let mut proofs = Vec::new();
            for attempt in &before.attempts {
                let native = crate::native_launcher::native_identity(&attempt.identity);
                claim.reserve(&native).unwrap();
                // Independent native absence observation plus a closed launch
                // transport, obtained before Stop changes Reserved to Stopping.
                proofs.push((
                    attempt.identity.clone(),
                    RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap(),
                ));
            }
            let manual = if overlap {
                let saved = person_history_redo(host.lock().unwrap().core(), 400);
                ok(
                    f.editor
                        .request(HostCommand::Save {
                            snapshot: locked_host_snapshot_for_test(&host),
                        })
                        .unwrap(),
                    "C3_PERSON_SAVE_OVERLAP",
                );
                Some(saved)
            } else {
                None
            };
            let stop = f.control(&f.editor, &run, RunAction::Stop);
            let stopped = ok(
                f.editor
                    .request(HostCommand::DispatchOrchestration {
                        request: Box::new(stop),
                    })
                    .unwrap(),
                "C3_STOP",
            );
            assert_eq!(
                stopped.orchestration.unwrap().decision,
                CommandDecision::Applied
            );
            let producer = f.producer(&run);
            for (identity, proof) in &proofs {
                host.lock()
                    .unwrap()
                    .reconcile_native_effect(
                        &producer,
                        identity,
                        &AttemptRetirementProof::Unbound(proof),
                    )
                    .unwrap();
                claim.retire_unbound(proof).unwrap();
                // Duplicate proof delivery must not stage or publish again.
                let sequence = f.projection(&run).sequence;
                host.lock()
                    .unwrap()
                    .reconcile_native_effect(
                        &producer,
                        identity,
                        &AttemptRetirementProof::Unbound(proof),
                    )
                    .unwrap();
                assert_eq!(f.projection(&run).sequence, sequence, "C3_RETIRE_ONCE");
            }
            assert!(
                claim.recovery_identities().is_empty(),
                "C3_PRIVATE_CLAIM_RELEASED"
            );
            claim.finish().unwrap();
            let state = f.projection(&run);
            assert_eq!(state.cleanup, RunCleanup::Settled, "C3_CLEANUP_SETTLED");
            assert!(
                state
                    .attempts
                    .iter()
                    .all(|attempt| matches!(attempt.state, AttemptState::Retired(_)))
            );
            assert_eq!(
                cleanup_history(&f, &run, &child),
                expected,
                "C3_LIVE_DIRTY_HISTORY_PERSISTED"
            );
            let budget = host
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .run_budget_probe(&run)
                .unwrap();
            assert!(
                !budget.active && budget.terminal > 0 && budget.terminal <= 32,
                "C3_TERMINAL_CUT_BOUND {budget:?}"
            );
            let replay = Core::open(
                expected.validate().unwrap(),
                identity(),
                kinewright_core::TimelineRevision(0),
            )
            .unwrap();
            let expected = replay.export_history().unwrap();
            assert_eq!(
                replay.export_history().unwrap(),
                expected,
                "C3_REPLAY_EXACT"
            );
            assert!(matches!(
                replay.request(kinewright_core::Command::Redo).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ));
            assert!(
                replay
                    .export_history()
                    .unwrap()
                    .history()
                    .document
                    .markers
                    .iter()
                    .any(|marker| marker.id == kinewright_core::MarkerId(2)),
                "C3_REDO_WORKS"
            );
            drop(proofs);
            drop(model);
            drop(host);
            for reopen in 0..2 {
                f = c4_restart(f, &machine);
                f.host.runtime.stop();
                assert_eq!(
                    cleanup_history(&f, &run, &child),
                    expected,
                    "C3_REOPEN_EXACT {reopen}"
                );
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                let kept = owner.owner().unwrap().retained_task_work().unwrap();
                assert!(
                    kept.iter().any(|work| work.run == run
                        && work.task == child
                        && work.available
                        && work.cleanup_settled),
                    "C3_RETAINED_REVIEW_AVAILABLE"
                );
                if let Some(manual) = &manual {
                    assert_eq!(
                        &owner.core().portable_history().unwrap(),
                        manual,
                        "C3_MANUAL_SAVE_EXACT"
                    );
                }
            }
            if matches!(latch, PersonLatch::Storage | PersonLatch::Specialist) {
                // Restore the fixture's deliberately tiny project capacity;
                // cleanup releases authority, not arbitrary raw disk space.
                f = c5_fix1_apply_limits(
                    f,
                    kinewright_project::storage_budget::StorageLimits::default(),
                );
                f.host.runtime.stop();
            }
            // Exercise the person's actual retained-work switch, not only
            // the discovery flag, then start from that reviewed work.
            let review_client = "c".repeat(64);
            ok(
                f.editor
                    .request(HostCommand::AcquireManual {
                        client: review_client.clone(),
                    })
                    .unwrap(),
                "C3_REVIEW_LEASE",
            );
            let review = f.editor.with_manual_editor(review_client.clone());
            let (source, source_checkpoint, snapshot) = {
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                let state = owner
                    .owner()
                    .unwrap()
                    .orchestration_projection(&run)
                    .unwrap()
                    .unwrap();
                let binding = owner
                    .saved_task_history(&state, &child)
                    .unwrap()
                    .unwrap()
                    .source;
                (
                    binding.branch,
                    binding.checkpoint,
                    received_snapshot_for_test(owner.core()).unwrap(),
                )
            };
            let prepared = ok(
                review
                    .request(HostCommand::PreviewRecovery {
                        target:
                            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                                source,
                            )
                            .into(),
                        snapshot,
                    })
                    .unwrap(),
                "C3_REVIEW_PREPARED",
            );
            let selected = ok(
                review
                    .request(HostCommand::Switch {
                        envelope: Box::new(prepared.switch_envelope.unwrap()),
                    })
                    .unwrap(),
                "C3_REVIEW_OPENED",
            );
            let installed = selected.switch_attachment.unwrap().identity().unwrap();
            ok(
                review
                    .request(HostCommand::ConfirmSwitch {
                        client: review_client,
                        installed: Box::new(installed),
                    })
                    .unwrap(),
                "C3_REVIEW_INSTALLED",
            );
            {
                let host = f.host.owner.upgrade().unwrap();
                let mut owner = host.lock().unwrap();
                assert_recovered_task_owner(
                    owner.core(),
                    serde_json::to_value(&expected).unwrap(),
                    "C3_REVIEW_EXACT_HISTORY_AND_REDO",
                );
                // Selecting work as the person's live project changes only its
                // future policy. The original authenticated task checkpoint is
                // still durable, exact and NeverFold after that selection.
                let store = owner.owner().unwrap().store().unwrap();
                let saved = store
                    .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                        &source_checkpoint,
                    )
                    .unwrap();
                assert_eq!(
                    saved.role,
                    kinewright_project::operation_history::Role::Task,
                    "C3_REVIEW_SOURCE_RETAINS_TASK_ROLE"
                );
                let retained = saved
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
                assert_eq!(retained, expected, "C3_REVIEW_SOURCE_TASK_REOPENS_EXACTLY");
                assert_eq!(
                    retained.history().policy,
                    kinewright_core::undo::RetentionPolicy::NeverFold,
                    "C3_REVIEW_SOURCE_NEVER_FOLDS"
                );
                let owner = owner.owner_mut().unwrap();
                let mut candidate = owner
                    .fork_durable_branch(
                        "reviewed work candidate",
                        kinewright_project::editing_session::DurableBranchRole::RunCandidate,
                    )
                    .unwrap();
                owner.persist_durable_branch(&mut candidate).unwrap();
                let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
                f.brief.scope = crate::orchestration::policy::director_scope(
                    &expected.history().document,
                    binding,
                )
                .unwrap();
                f.brief.inputs = owner.current_inputs_ref().clone();
                println!(
                    "C3_CLEANUP_CASE latch={latch:?} staged_manual_overlap={overlap} terminal_cuts={} peak_decoded={} objects={}",
                    budget.terminal,
                    owner.store().unwrap().usage().unwrap().peak_decoded_bytes,
                    owner.store().unwrap().usage().unwrap().objects
                );
            }
            let restarted = f.start();
            assert_eq!(restarted.decision, CommandDecision::Applied, "C3_NEW_RUN");
            let new_run: RunId = restarted.envelope.identity("run").unwrap();
            assert_ne!(new_run, run);
            let new_state = f.projection(&new_run);
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            let accounting = owner.native_accounting(&new_state).unwrap().unwrap();
            assert!(
                accounting.tasks.iter().all(|task| task.sends.is_empty()),
                "C3_NO_SEND_REPLAY"
            );
        }
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one real-shaped task proves reversible/destructive admission, exact refusal and successful finish"
)]
fn run_limit_cleanup_history_bound_refuses_before_any_live_mutation() {
    let document = capacity_document(120);
    let mut f = Fixture::with_document(false, document.clone());
    f.host.runtime.stop();
    f.brief.scope =
        crate::orchestration::policy::director_scope(&document, f.brief.scope.base.clone())
            .unwrap();
    let (run, parent, director) = c4_director(&f);
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
    let (execution, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let mcp = C4Mcp::new(&f, &run, &child);
    // Seed through the actual NeverFold executor until its fixed room is full.
    let mut admitted = 0;
    loop {
        let command = kinewright_core::Command::Do(kinewright_core::Operation::SetTrackSyncLock {
            track: kinewright_core::TrackId(1),
            locked: admitted % 2 == 0,
        });
        match core.request(command) {
            Ok(kinewright_core::Event::DocumentChanged { .. }) => admitted += 1,
            Err(kinewright_project::operation_actor::Refusal {
                error:
                    kinewright_project::operation_actor::Error::History(
                        kinewright_project::operation_history::Error::History(
                            kinewright_core::undo::Error::HistoryFull,
                        ),
                    ),
                ..
            }) => break,
            other => panic!("C3_ACTUAL_TASK_BOUNDARY {admitted}: {other:?}"),
        }
        assert!(admitted <= kinewright_core::undo::RoleBounds::TASK.step_cap);
    }
    assert!(admitted > 0, "C3_ROLE_BOUND_FOUND");
    let before = core.export_history().unwrap();
    let mut projection = f.projection(&run);
    let (mut accounting_ref, mut accounting) = cleanup_accounting(&f, &projection);
    for (call, operation) in [
        serde_json::json!({"op":"set_track_sync_lock","track":1,"locked":false}),
        serde_json::json!({"op":"delete_clip","clip":101}),
    ]
    .into_iter()
    .enumerate()
    {
        // Capture the independent MCP observer before the reply is measured.
        // Accounting records the previous completed replies on this call.
        let mut observed = {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            let measurements = owner.hooks.runs[&run].measurements[&child].lock().unwrap();
            measurements.clone()
        };
        let refused = mcp.raw(
            "prepare_edit_plan",
            serde_json::json!({
                "expected_revision":grant_test_revision(&core), "operations":[operation],
            }),
        );
        assert_eq!(refused.is_error, Some(true), "C3_BOUND_REFUSED {refused:?}");
        assert!(
            format!("{refused:?}")
                .contains(kinewright_project::operation_history::TASK_LIMIT_REASON),
            "C3_ACTIONABLE_REFUSAL {refused:?}"
        );
        assert_eq!(
            core.export_history().unwrap(),
            before,
            "C3_REFUSAL_PRECEDES_MUTATION"
        );
        let after = f.projection(&run);
        let (current_ref, current) = cleanup_accounting(&f, &after);
        current
            .validate_successor(&accounting)
            .expect("C3_REFUSAL_PRESERVES_PRIOR_ACCOUNTING");
        let mut expected = accounting.successor().unwrap();
        if !expected.tasks.iter().any(|task| task.task == child.0) {
            expected.add_task(child.0.clone(), None).unwrap();
        }
        let task = expected.task_mut(&child.0).unwrap();
        if let Some(previous) = &task.measurements {
            observed.sends.clone_from(&previous.sends);
        }
        task.measurements = Some(observed);
        task.refuse_tool(
            "prepare_edit_plan",
            kinewright_project::orchestration_accounting::ToolRefusalCode::Other,
        )
        .unwrap();
        // The accounting publication also observes the owner's clock and the
        // independently captured completed replies; no other spending may move.
        expected.last_unix_ms = current.last_unix_ms;
        expected.elapsed_ms = current.elapsed_ms;
        expected.clock_uncertain = current.clock_uncertain;
        assert_eq!(
            current, expected,
            "C3_REFUSAL_RECORDS_ONLY_THE_ACTUAL_REFUSED_TOOL_AND_CLOCK: call {call}"
        );
        let mut replaced = 0;
        for reference in &mut projection.usage {
            if *reference == accounting_ref {
                reference.clone_from(&current_ref);
                replaced += 1;
            }
        }
        assert_eq!(
            replaced, 1,
            "C3_REFUSAL_REPLACES_ONE_CUMULATIVE_ACCOUNTING_RECORD"
        );
        accounting = current;
        accounting_ref = current_ref;
        // Every non-accounting reference, approval, task, result and control
        // field must still be exactly the original projection.
        assert_eq!(after, projection, "C3_REFUSAL_PRECEDES_APPROVAL");
    }
    let finished = mcp.invoke(
        "finish_task",
        mcp.control(OrchestrationCommand::FinishTask(FinishTask {
            task: child.clone(),
            expected_revision: grant_test_revision(&core),
        })),
    );
    assert_eq!(
        finished.is_error,
        Some(false),
        "C3_REFUSED_GROWTH_CAN_FINISH {finished:?}"
    );
    assert_eq!(cleanup_history(&f, &run, &child), before);
    assert!(
        !f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .run_limit_reached()
    );
}

#[test]
fn run_limit_cleanup_undo_and_redo_are_bounded_before_mutation() {
    let pool = kinewright_project::operation_history::HistoryPool::new(
        kinewright_project::operation_history::PoolLimits::default(),
    );
    let core = Core::spawn(
        &pool,
        kinewright_project::operation_history::Role::Task,
        kinewright_project::operation_history::CreationOrigin {
            branch: identity(),
            batch: identity(),
        },
        kinewright_core::Document {
            tracks: vec![kinewright_core::Track {
                id: kinewright_core::TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let mut count = 0;
    loop {
        match core.request(kinewright_core::Command::Do(
            kinewright_core::Operation::SetTrackSyncLock {
                track: kinewright_core::TrackId(1),
                locked: count % 2 == 0,
            },
        )) {
            Ok(kinewright_core::Event::DocumentChanged { .. }) => count += 1,
            Err(kinewright_project::operation_actor::Refusal {
                error:
                    kinewright_project::operation_actor::Error::History(
                        kinewright_project::operation_history::Error::History(
                            kinewright_core::undo::Error::HistoryFull,
                        ),
                    ),
                ..
            }) => break,
            other => panic!("C3_FIXED_TASK_BOUND {count}: {other:?}"),
        }
    }
    let before = core.export_history().unwrap();
    for _ in 0..count {
        assert!(
            matches!(
                core.request(kinewright_core::Command::Undo).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ),
            "C3_UNDO_NEVER_REFUSES_HISTORY_CAPACITY"
        );
    }
    for _ in 0..count {
        assert!(
            matches!(
                core.request(kinewright_core::Command::Redo).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ),
            "C3_REDO_NEVER_REFUSES_HISTORY_CAPACITY"
        );
    }
    let after = core.export_history().unwrap();
    assert_eq!(after.history().undo, before.history().undo);
    assert_eq!(after.history().redo, before.history().redo);
    assert_eq!(after.history().document, before.history().document);
    assert_eq!(after.resources(), before.resources());
}
