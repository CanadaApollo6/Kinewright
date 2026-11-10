fn cleanup_history(f: &Fixture, run: &RunId, child: &TaskId) -> kinewright_core::HistorySnapshot {
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
    let stored = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
        .unwrap();
    (*stored).clone()
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
            assert!(!expected.cursor.redo.is_empty(), "C3_DIRTY_REDO");
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
                            snapshot: HostSnapshot::from_history(saved.clone()).unwrap(),
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
            let replay = Core::spawn(std::sync::Arc::clone(&expected.baseline)).unwrap();
            replay.restore_history(expected.clone()).unwrap();
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
                        &owner.core().export_history().unwrap(),
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
            let (source, snapshot) = {
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                let state = owner
                    .owner()
                    .unwrap()
                    .orchestration_projection(&run)
                    .unwrap()
                    .unwrap();
                (
                    owner
                        .saved_task_history(&state, &child)
                        .unwrap()
                        .unwrap()
                        .source
                        .branch,
                    HostSnapshot::capture(owner.core()).unwrap(),
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
                assert_eq!(
                    owner.core().export_history().unwrap(),
                    expected,
                    "C3_REVIEW_EXACT_HISTORY_AND_REDO"
                );
                let owner = owner.owner_mut().unwrap();
                let mut candidate = owner
                    .fork_durable_branch("reviewed work candidate")
                    .unwrap();
                owner.persist_durable_branch(&mut candidate).unwrap();
                let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
                f.brief.scope =
                    crate::orchestration::policy::director_scope(&expected.document, binding)
                        .unwrap();
                f.brief.inputs = owner.current_inputs_ref().clone();
                println!(
                    "C3_CLEANUP_CASE latch={latch:?} staged_manual_overlap={overlap} terminal_cuts={} peak_history={} objects={}",
                    budget.terminal,
                    owner
                        .store()
                        .unwrap()
                        .usage()
                        .unwrap()
                        .peak_live_history_bytes,
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
#[allow(
    clippy::too_many_lines,
    reason = "one real-shaped task proves reversible/destructive admission, exact refusal and successful finish"
)]
fn run_limit_cleanup_history_bound_refuses_before_any_live_mutation() {
    let document = kinewright_project::c0_fixtures::save_cost_document();
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
    let before = core.export_history().unwrap();
    assert!(f.projection(&run).edit_approvals.is_empty());
    for operation in [
        serde_json::json!({"op":"set_track_sync_lock","track":1,"locked":false}),
        serde_json::json!({"op":"delete_clip","clip":101}),
    ] {
        let refused = mcp.raw(
            "prepare_edit_plan",
            serde_json::json!({
                "expected_revision":grant_test_revision(&core), "operations":[operation],
            }),
        );
        assert_eq!(refused.is_error, Some(true), "C3_BOUND_REFUSED {refused:?}");
        assert!(
            format!("{refused:?}")
                .contains(kinewright_project::task_history_admission::LIMIT_REASON),
            "C3_ACTIONABLE_REFUSAL {refused:?}"
        );
        assert_eq!(
            core.export_history().unwrap(),
            before,
            "C3_REFUSAL_PRECEDES_MUTATION"
        );
        assert!(
            f.projection(&run).edit_approvals.is_empty(),
            "C3_REFUSAL_PRECEDES_APPROVAL"
        );
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
    fn history_at(size: usize, redo: bool) -> Core {
        let core = Core::spawn(Document::default()).unwrap();
        let mut operation = history_marker(1);
        let kinewright_core::Operation::AddMarker { marker } = &mut operation else {
            unreachable!()
        };
        marker.label = "x".repeat(size);
        assert!(matches!(
            core.request(kinewright_core::Command::Do(operation))
                .unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        if redo {
            assert!(matches!(
                core.request(kinewright_core::Command::Undo).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ));
        }
        core
    }
    let cost = |core: &Core| {
        kinewright_project::task_history_admission::decoded_history_bytes_for_fixture(
            &core.export_history().unwrap(),
        )
        .unwrap()
    };
    for redo in [false, true] {
        let (mut low, mut high) = (0_usize, 600_000_usize);
        while low < high {
            let middle = (low + high).div_ceil(2);
            if cost(&history_at(middle, redo)) <= kinewright_project::run_admission::HISTORY_BYTES {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let core = history_at(low, redo);
        let before = core.export_history().unwrap();
        let binding = BranchBinding {
            branch: identity(),
            head: identity(),
            document_hash: "0".repeat(64),
            checkpoint: ObjectRef {
                sha256: "0".repeat(64),
                byte_length: 0,
                record_kind: kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
            },
        };
        let mut scope =
            crate::orchestration::policy::director_scope(&before.document, binding).unwrap();
        scope.objects.insert("marker:1".into());
        let endpoint = crate::orchestration::tasks::TaskEndpoint::for_fixture(
            crate::orchestration::policy::TaskPolicy {
                role: Role::Editor,
                scope,
            },
            crate::orchestration::tasks::RuntimeTaskAuthority {
                project: identity(),
                run: identity(),
                task: identity(),
                attempt: identity(),
                endpoint: identity(),
                owner_epoch: 1,
                expires_at: Instant::now() + Duration::from_secs(60),
            },
            &core,
        );
        let refused = endpoint
            .history_step(&core, grant_test_revision(&core), redo)
            .unwrap_err();
        assert!(
            refused.contains(kinewright_project::task_history_admission::LIMIT_REASON),
            "C3_HISTORY_STEP_REFUSAL {redo} {refused}"
        );
        assert_eq!(
            core.export_history().unwrap(),
            before,
            "C3_HISTORY_STEP_PREMUTATION {redo}"
        );
        println!(
            "C3_HISTORY_STEP_BOUND redo={redo} decoded={} label_bytes={low}",
            cost(&core)
        );
    }
}
