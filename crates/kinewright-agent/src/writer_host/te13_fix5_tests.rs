const TE13_FIX5_WAITING: &str =
    "Task completion is waiting for its original finish retry. No automatic retry is confirmed.";

fn te13_fix5_waiting(p: &Te13Fix4Pending) {
    let host = p.run.f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    owner.owner_mut().unwrap().reconcile_metadata().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&p.run.run)
        .unwrap()
        .unwrap();
    assert!(
        owner.run_recovery_blocks(&state).unwrap().is_empty(),
        "TE13_FIX5_RETRYABLE_NO_BLOCK"
    );
    assert_eq!(
        owner.run_recovery_message(&state).unwrap().as_deref(),
        Some(TE13_FIX5_WAITING),
        "TE13_FIX5_TRUTHFUL_WAITING"
    );
    let index = owner
        .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
        .unwrap();
    let view = owner.observe_run(index, &p.run.run, 0).unwrap();
    assert!(
        !view
            .runtime_status
            .as_ref()
            .is_some_and(|s| s.contains("Stop this run")),
        "TE13_FIX5_NO_FALSE_STOP"
    );
    // This is authorization of Continue, independent of whether this phase needs it.
    let mut command = p.request.clone();
    command.envelope.authority.endpoint = None;
    command.envelope.authority.attempt = None;
    command.envelope.command = OrchestrationCommand::ControlRun(RunIntent {
        command_id: command.envelope.id.clone(),
        run_id: p.run.run.clone(),
        expected_control_revision: state.control_revision,
        action: RunAction::Continue,
    });
    command.envelope.digest = command.envelope.computed_digest().unwrap();
    let allowed = owner.authorize_orchestration(index, &command);
    assert!(
        !allowed
            .err()
            .is_some_and(|e| e.to_string().contains("recovery_requires_person")),
        "TE13_FIX5_CONTINUE_NOT_BLOCKED"
    );
}

fn te13_fix5_repeated_fault(step: CompletionStep) {
    let p = te13_fix4_pending(step, false, false);
    te13_fix5_waiting(&p);
    let second = kinewright_project::io_observer::fail_once("root_rename_io", || {
        te13_fix2_dispatch(&p.run, &p.client, &p.request)
    });
    assert_eq!(
        second.orchestration.as_ref(),
        Some(&p.receipt),
        "TE13_FIX5_SECOND_FAILURE_ORIGINAL_RECEIPT"
    );
    assert!(
        matches!(
            p.run
                .f
                .host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .hooks
                .pending,
            Some(super::super::orchestration::PendingPublication::Completion(
                _
            ))
        ),
        "TE13_FIX5_SECOND_REAL_FAULT"
    );
    te13_fix5_waiting(&p);
    te13_fix4_replay(&p);
    let state = p.run.f.projection(&p.run.run);
    assert_eq!(
        (state.results.len(), state.deliveries.len()),
        (1, 1),
        "TE13_FIX5_EXACTLY_ONCE"
    );
    assert!(
        p.run
            .f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .run_recovery_message(&state)
            .unwrap()
            .is_none()
    );
}
#[test]
fn te13_fix5_repeated_freeze_fault() {
    te13_fix5_repeated_fault(CompletionStep::Freeze);
}
#[test]
fn te13_fix5_repeated_publication_fault() {
    te13_fix5_repeated_fault(CompletionStep::Publication);
}

fn te13_fix5_issuance(step: CompletionStep, committed: bool) {
    let p = te13_fix4_pending(step, committed, false);
    let before = p.run.f.projection(&p.run.run);
    let reply = p
        .run
        .f
        .editor
        .request(HostCommand::IssueMetadataIssuer { issuer: None })
        .unwrap();
    assert!(
        reply.error.is_none(),
        "TE13_FIX5_PROOF_RELEASES_ENVELOPE {reply:?}"
    );
    let issued = reply.metadata_issuer.unwrap();
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let owner = host.lock().unwrap();
        assert!(
            owner.hooks.pending.is_none(),
            "TE13_FIX5_PROOF_RELEASES_ENVELOPE"
        );
        assert_eq!(
            owner
                .owner()
                .unwrap()
                .orchestration_projection(&p.run.run)
                .unwrap()
                .unwrap(),
            before,
            "TE13_FIX5_ISSUANCE_PRESERVES_RUN"
        );
        assert!(
            owner.hooks.runs[&p.run.run].recovery_blocks.is_empty(),
            "TE13_FIX5_ISSUANCE_NO_BLOCK"
        );
        assert_eq!(
            owner
                .owner()
                .unwrap()
                .metadata_issuer_sequence(&issued.envelope.id.issuer)
                .unwrap(),
            issued.envelope.id.sequence
        );
    }
    te13_fix4_replay(&p);
    if step != CompletionStep::Publication || !committed {
        te13_fix4_retired_receipt(&p);
    } else {
        // Proof can release an already completed step before the first receipt
        // read. This path is still an authenticated immutable read, not dispatch.
        let before = p.run.f.projection(&p.run.run);
        te13_fix4_replay(&p);
        assert_eq!(
            p.run.f.projection(&p.run.run),
            before,
            "TE13_FIX5_COMPLETED_RECEIPT_READ_ONLY"
        );
    }
    let state = p.run.f.projection(&p.run.run);
    assert_eq!(
        (state.results.len(), state.deliveries.len()),
        (1, 1),
        "TE13_FIX5_EXACTLY_ONCE"
    );
}
#[test]
fn te13_fix5_issue_freeze_committed() {
    te13_fix5_issuance(CompletionStep::Freeze, true);
}
#[test]
fn te13_fix5_issue_freeze_absent() {
    te13_fix5_issuance(CompletionStep::Freeze, false);
}
#[test]
fn te13_fix5_issue_publication_committed() {
    te13_fix5_issuance(CompletionStep::Publication, true);
}
#[test]
fn te13_fix5_issue_publication_absent() {
    te13_fix5_issuance(CompletionStep::Publication, false);
}

fn te13_fix5_publisher(step: CompletionStep, issue: bool, save: bool) {
    let p = te13_fix4_pending(step, false, false);
    if issue {
        Fixture::issue(&p.run.f.editor);
    }
    let host = p.run.f.host.owner.upgrade().unwrap();
    let (generation, history) = {
        let owner = host.lock().unwrap();
        owner
            .core()
            .request(kinewright_core::Command::Do(history_marker(99)))
            .unwrap();
        (
            owner.owner().unwrap().metadata_generation().unwrap(),
            owner.core().export_history().unwrap(),
        )
    };
    if save {
        let reply = p
            .run
            .f
            .editor
            .request(HostCommand::Save {
                snapshot: HostSnapshot::from_history(history.clone()).unwrap(),
            })
            .unwrap();
        assert!(reply.error.is_none(), "TE13_FIX5_MANUAL_SAVE {reply:?}");
    } else {
        host.lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .save_active()
            .unwrap();
        // A maintenance publisher also moves the root without consuming the task ID.
        host.lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .issue_metadata_issuer_for("fix5-maintenance")
            .unwrap();
    }
    assert!(
        host.lock()
            .unwrap()
            .owner()
            .unwrap()
            .metadata_generation()
            .unwrap()
            > generation,
        "TE13_FIX5_PUBLISHER_ADVANCED"
    );
    te13_fix4_replay(&p);
    let owner = host.lock().unwrap();
    assert_eq!(
        owner.core().export_history().unwrap(),
        history,
        "TE13_FIX5_SAVED_EDIT_INTACT"
    );
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&p.run.run)
        .unwrap()
        .unwrap();
    assert_eq!(
        (state.results.len(), state.deliveries.len()),
        (1, 1),
        "TE13_FIX5_CURRENT_GENERATION_RETRY"
    );
}
#[test]
fn te13_fix5_save_after_proof() {
    te13_fix5_publisher(CompletionStep::Publication, true, true);
}
#[test]
fn te13_fix5_save_without_issuance() {
    te13_fix5_publisher(CompletionStep::Publication, false, true);
}
#[test]
fn te13_fix5_maintenance_after_proof() {
    te13_fix5_publisher(CompletionStep::Freeze, true, false);
}
#[test]
fn te13_fix5_maintenance_without_issuance() {
    te13_fix5_publisher(CompletionStep::Freeze, false, false);
}

#[test]
fn te13_fix5_waiting_without_caller_has_truthful_words() {
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let owner = host.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&p.client.bootstrap.auth.bearer()))
            .unwrap();
        owner.attachments[index]
            .alive
            .store(false, std::sync::atomic::Ordering::Release);
    }
    te13_fix5_waiting(&p);
}

fn te13_fix5_frozen_empty(recovering: bool) {
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    Fixture::issue(&p.run.f.editor);
    let stop = p
        .run
        .f
        .control(&p.run.f.editor, &p.run.run, RunAction::Stop);
    let host = p.run.f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    assert!(
        owner.hooks.pending.is_none() && owner.hooks.runs[&p.run.run].recovery_blocks.is_empty()
    );
    owner.recovering = recovering;
    owner.switch_frozen = !recovering;
    let index = owner
        .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
        .unwrap();
    let issued = owner.execute_hook(
        index,
        HostCommand::IssueMetadataIssuer { issuer: None },
        &p.run.f.editor.bootstrap.roots,
    );
    assert!(
        issued.error.is_none(),
        "TE13_FIX5_DURABLE_FROZEN_ISSUANCE {issued:?}"
    );
    let reply = owner.execute_hook(
        index,
        HostCommand::DispatchOrchestration {
            request: Box::new(stop),
        },
        &p.run.f.editor.bootstrap.roots,
    );
    assert!(
        reply.error.is_none(),
        "TE13_FIX5_DURABLE_FROZEN_STOP {reply:?}"
    );
    assert_eq!(
        reply.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
}
#[test]
fn te13_fix5_recovering_stop_without_slot_or_block() {
    te13_fix5_frozen_empty(true);
}
#[test]
fn te13_fix5_switch_stop_without_slot_or_block() {
    te13_fix5_frozen_empty(false);
}

#[test]
fn te13_fix5_temporary_recovery_does_not_install_block() {
    let p = te13_fix4_pending(CompletionStep::Freeze, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&p.client.bootstrap.auth.bearer()))
            .unwrap();
        owner.recovering = true;
        let error = owner
            .reconcile_completion_substep(index, &p.request)
            .unwrap_err();
        assert_eq!(error.refusal().code, "recovery_frozen");
        assert!(owner.hooks.pending.is_none());
        assert!(
            owner.hooks.runs[&p.run.run].recovery_blocks.is_empty(),
            "TE13_FIX5_TEMPORARY_AUTH_NO_BLOCK"
        );
        owner.recovering = false;
    }
    te13_fix5_waiting(&p);
    te13_fix4_replay(&p);
}

#[test]
fn te13_fix5_real_switch_then_exact_finish() {
    use kinewright_project::bundle_switch::SwitchTarget;
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    let client = "e".repeat(64);
    let (envelope, expected) = {
        let mut owner = host.lock().unwrap();
        owner.acquire_manual(client.clone()).unwrap();
        let project = owner.owner_mut().unwrap();
        let mut branch = project.fork_durable_branch("fix5 switch target").unwrap();
        branch
            .core()
            .request(kinewright_core::Command::Do(history_marker(101)))
            .unwrap();
        project.persist_durable_branch(&mut branch).unwrap();
        let expected = branch.core().export_history().unwrap();
        let id = branch.branch_id().clone();
        project.close_durable_branch(&branch).unwrap();
        let mut reply = HostReply::default();
        owner
            .prepare_switch(SwitchTarget::WorkingBranch { branch: id }, &mut reply)
            .unwrap();
        (reply.switch_envelope.unwrap(), expected)
    };
    Fixture::issue(&p.run.f.editor);
    {
        let mut owner = host.lock().unwrap();
        let mut reply = HostReply::default();
        owner.publish_switch(&envelope, &mut reply).unwrap();
        let installed = reply.switch_attachment.unwrap().identity().unwrap();
        owner
            .confirm_switch(client.clone(), &installed, &mut HostReply::default())
            .unwrap();
        assert!(!owner.switch_frozen, "TE13_FIX5_SWITCH_CONFIRMED");
        assert_eq!(owner.core().export_history().unwrap(), expected);
        owner
            .confirm_switch(client, &installed, &mut HostReply::default())
            .unwrap();
    }
    te13_fix4_replay(&p);
    let state = p.run.f.projection(&p.run.run);
    assert_eq!(
        (state.results.len(), state.deliveries.len()),
        (1, 1),
        "TE13_FIX5_SWITCH_FINISH_EXACTLY_ONCE"
    );
    assert_eq!(
        host.lock().unwrap().core().export_history().unwrap(),
        expected,
        "TE13_FIX5_SWITCH_EDIT_INTACT"
    );
}

#[test]
fn te13_fix5_durable_stop_read_failure_fails_closed() {
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    Fixture::issue(&p.run.f.editor);
    let stop = p
        .run
        .f
        .control(&p.run.f.editor, &p.run.run, RunAction::Stop);
    let host = p.run.f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let reference = owner
        .owner()
        .unwrap()
        .orchestration_cut(&p.run.run)
        .unwrap()
        .unwrap()
        .snapshot;
    let file = p
        .run
        .f
        .temp
        .path("project.kinewright-assets/studio/objects")
        .join(reference.sha256);
    std::fs::remove_file(&file).unwrap();
    let index = owner
        .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
        .unwrap();
    assert!(
        !owner.completion_owner_recovery(index, &HostCommand::IssueMetadataIssuer { issuer: None }),
        "TE13_FIX5_UNREADABLE_NO_ISSUANCE_BYPASS"
    );
    assert!(
        !owner.completion_owner_recovery(
            index,
            &HostCommand::DispatchOrchestration {
                request: Box::new(stop)
            }
        ),
        "TE13_FIX5_UNREADABLE_NO_STOP_BYPASS"
    );
}

#[test]
fn te13_fix5_different_run_stop_cannot_abandon_uncertain_finish() {
    let p = te13_fix4_pending(CompletionStep::Publication, false, true);
    let other = p.unrelated.clone().unwrap();
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
            .unwrap();
        let subject = owner.metadata_subject(index);
        let issuer = owner
            .owner_mut()
            .unwrap()
            .reuse_metadata_issuer_for(&subject)
            .unwrap();
        let id = kinewright_project::receipts::CommandId {
            sequence: owner
                .owner()
                .unwrap()
                .metadata_issuer_sequence(&issuer)
                .unwrap(),
            issuer,
        };
        let mut stop = p.request.clone();
        stop.run = Some(other.clone());
        stop.envelope.id = id.clone();
        stop.envelope.expected_generation = owner.owner().unwrap().metadata_generation().unwrap();
        stop.envelope.authority = AuthorityBinding {
            grant_ref: owner.expected_grant(index).unwrap(),
            owner_epoch: owner.owner().unwrap().store().unwrap().owner_epoch(),
            endpoint: None,
            attempt: None,
        };
        stop.envelope.command = OrchestrationCommand::ControlRun(RunIntent {
            command_id: id,
            run_id: other.clone(),
            expected_control_revision: owner
                .owner()
                .unwrap()
                .orchestration_projection(&other)
                .unwrap()
                .unwrap()
                .control_revision,
            action: RunAction::Stop,
        });
        stop.envelope.digest = stop.envelope.computed_digest().unwrap();
        let before = serde_json::to_vec(&owner.hooks.pending).unwrap();
        let _ = owner.dispatch_orchestration(index, &stop);
        assert_eq!(
            serde_json::to_vec(&owner.hooks.pending).unwrap(),
            before,
            "TE13_FIX5_ONLY_MATCHING_STOP_ABANDONS"
        );
    }
    Fixture::issue(&p.run.f.editor);
    te13_fix4_replay(&p);
}

fn te13_fix5_publisher_sweep(step: CompletionStep, committed: bool) {
    use kinewright_project::{
        bundle::BundleError, storage_budget::StorageLimits, studio_records::MetadataScope,
    };
    let r = c5_fix10_run();
    let host = r.f.host.owner.upgrade().unwrap();
    let mut branch = {
        let mut owner = host.lock().unwrap();
        let mut branch = owner
            .owner_mut()
            .unwrap()
            .fork_durable_branch("fault sweep task")
            .unwrap();
        owner
            .owner_mut()
            .unwrap()
            .persist_durable_branch(&mut branch)
            .unwrap();
        branch
            .core()
            .request(kinewright_core::Command::Do(history_marker(88)))
            .unwrap();
        branch
    };
    let issued = Fixture::issue(&r.f.editor);
    let (client, request) = te13_fix2_request(&r);
    let operation = if committed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    let nth = if step == CompletionStep::Freeze { 2 } else { 3 };
    let result = kinewright_project::io_observer::fail_nth(operation, nth, || {
        te13_fix2_dispatch(&r, &client, &request)
    });
    assert_eq!(
        result.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    let root = std::fs::read(r.f.temp.path("project.kinewright")).unwrap();
    {
        let mut owner = host.lock().unwrap();
        let project = owner.owner_mut().unwrap();
        assert!(
            project.has_unresolved_publication(),
            "TE13_FIX5_UNSETTLED_SWEEP_TRIGGER"
        );
        assert!(
            matches!(project.save_active(), Err(BundleError::Unsettled)),
            "TE13_FIX5_UNSETTLED_MANUAL_AUTOSAVE"
        );
        assert!(
            matches!(
                project.issue_metadata_issuer_for("unsettled-maintenance"),
                Err(BundleError::Unsettled)
            ),
            "TE13_FIX5_UNSETTLED_MAINTENANCE"
        );
        let checkpoint = project.persist_durable_branch(&mut branch);
        assert!(
            matches!(
                checkpoint,
                Err(BundleError::Unsettled | BundleError::HistoryBarrierInvariant)
            ),
            "TE13_FIX5_UNSETTLED_TASK_CHECKPOINT {checkpoint:?}"
        );
        let (checkpoint_ref, checkpoint) = project.manual_checkpoint().unwrap();
        let update = MetadataUpdate {
            expected_root_generation: project.metadata_generation().unwrap(),
            change: MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: checkpoint.head_id,
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        };
        assert!(
            matches!(
                project.metadata_command(&issued.envelope, MetadataScope::Snapshot, &update),
                Err(BundleError::Unsettled)
            ),
            "TE13_FIX5_UNSETTLED_METADATA_PUBLISHER"
        );
        assert!(
            project
                .save_as(
                    &r.f.temp.path("uncertain-copy.kinewright"),
                    StorageLimits::default()
                )
                .is_err(),
            "TE13_FIX5_UNSETTLED_COPY"
        );
        assert!(
            project
                .prepare_recovery(
                    kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                        branch.branch_id().clone()
                    )
                )
                .is_err(),
            "TE13_FIX5_UNSETTLED_SWITCH"
        );
    }
    assert_eq!(
        std::fs::read(r.f.temp.path("project.kinewright")).unwrap(),
        root,
        "TE13_FIX5_UNSETTLED_NO_ROOT_REPLACEMENT"
    );
    let replay = r.editor.wire("finish_task",serde_json::json!({"id":request.envelope.id,"attempt":r.attempt.attempt,
        "expected_generation":request.envelope.expected_generation,"owner_epoch":request.envelope.authority.owner_epoch,
        "expected_head":r.f.brief.scope.base.head,"command":request.envelope.command}));
    assert_eq!(
        replay.is_error,
        Some(false),
        "TE13_FIX5_SWEEP_RECOVERS_ORIGINAL {replay:?}"
    );
    assert_eq!(
        (
            r.f.projection(&r.run).results.len(),
            r.f.projection(&r.run).deliveries.len()
        ),
        (1, 1)
    );
}
#[test]
fn te13_fix5_sweep_freeze_committed() {
    te13_fix5_publisher_sweep(CompletionStep::Freeze, true);
}
#[test]
fn te13_fix5_sweep_freeze_absent() {
    te13_fix5_publisher_sweep(CompletionStep::Freeze, false);
}
#[test]
fn te13_fix5_sweep_publication_committed() {
    te13_fix5_publisher_sweep(CompletionStep::Publication, true);
}
#[test]
fn te13_fix5_sweep_publication_absent() {
    te13_fix5_publisher_sweep(CompletionStep::Publication, false);
}

fn te13_fix5_generic_collision(restart: bool) {
    use kinewright_project::studio_records::MetadataScope;
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    let step: ScopedOrchestrationRequest;
    {
        let mut owner = host.lock().unwrap();
        let Some(super::super::orchestration::PendingPublication::Completion(pending)) =
            owner.hooks.pending.clone()
        else {
            panic!("pending");
        };
        step = serde_json::from_value(serde_json::to_value(&pending).unwrap()["step"].clone())
            .unwrap();
        let project = owner.owner_mut().unwrap();
        let envelope = project.metadata_envelope(step.envelope.id.clone()).unwrap();
        let (checkpoint_ref, checkpoint) = project.manual_checkpoint().unwrap();
        let update = MetadataUpdate {
            expected_root_generation: project.metadata_generation().unwrap(),
            change: MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: checkpoint.head_id,
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        };
        project
            .metadata_command(&envelope, MetadataScope::Snapshot, &update)
            .unwrap();
        assert!(
            project
                .orchestration_receipt(&p.run.run, &step.envelope.id)
                .unwrap()
                .is_none()
        );
        if !restart {
            assert!(
                owner.proved_completion_fate(&step).is_err(),
                "TE13_FIX5_GENERIC_RECEIPT_IS_NOT_ABSENCE"
            );
            let index = owner
                .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
                .unwrap();
            assert!(
                owner.issue_metadata(index, None).is_err(),
                "TE13_FIX5_CONFLICTING_PROOF_REFUSES_ISSUANCE"
            );
        }
    }
    if restart {
        drop(host);
        let Te13Fix4Pending { run: r, client, .. } = p;
        drop(client);
        let C5Fix10Run {
            f,
            editor,
            director,
            core,
            ..
        } = r;
        drop(editor);
        drop(director);
        drop(core);
        let claim = f.temp.path("fix5-generic-restart");
        let f = c4_restart(f, &claim);
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        assert!(
            owner.proved_completion_fate(&step).is_err(),
            "TE13_FIX5_REOPENED_GENERIC_ID_NOT_ABSENT"
        );
        assert!(
            owner
                .owner()
                .unwrap()
                .metadata_issuer_sequence(&step.envelope.id.issuer)
                .map_or(true, |next| next > step.envelope.id.sequence)
        );
    }
}
#[test]
fn te13_fix5_generic_receipt_conflict_refuses() {
    te13_fix5_generic_collision(false);
}
#[test]
fn te13_fix5_generic_receipt_conflict_survives_restart() {
    te13_fix5_generic_collision(true);
}

#[test]
fn te13_fix5_stored_refusal_is_permanent() {
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        let Some(super::super::orchestration::PendingPublication::Completion(pending)) =
            &owner.hooks.pending
        else {
            panic!("pending");
        };
        let step: ScopedOrchestrationRequest =
            serde_json::from_value(serde_json::to_value(pending).unwrap()["step"].clone()).unwrap();
        let mut state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&p.run.run)
            .unwrap()
            .unwrap();
        state.sequence += 1;
        state.control_revision = step.envelope.identity("control-revision").unwrap();
        let segment = EventSegment {
            run: p.run.run.clone(),
            first_sequence: state.sequence,
            previous: Some(
                owner
                    .owner()
                    .unwrap()
                    .orchestration_cut(&p.run.run)
                    .unwrap()
                    .unwrap()
                    .segment,
            ),
            events: vec![OrchestrationEvent {
                command: step.envelope.id.clone(),
                sequence: state.sequence,
                projection: state.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &step.envelope.project,
            step.envelope.expected_generation,
            &step.envelope,
            state,
            segment,
            CommandDecision::Refused(
                kinewright_project::orchestration_commit::RecordedRefusal::InvalidState,
            ),
        )
        .unwrap();
        kinewright_project::orchestration_commit::commit_orchestration(
            owner.owner_mut().unwrap(),
            update,
            None,
        )
        .unwrap();
        let index = owner
            .resolve_attachment(Some(&p.client.bootstrap.auth.bearer()))
            .unwrap();
        assert!(
            owner
                .reconcile_completion_substep(index, &p.request)
                .is_err()
        );
        assert!(
            !owner.hooks.runs[&p.run.run].recovery_blocks.is_empty(),
            "TE13_FIX5_STORED_REFUSAL_BLOCKS"
        );
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&p.run.run)
            .unwrap()
            .unwrap();
        let status = owner
            .completion_status(
                &state,
                state.tasks[1].progress.completion.as_deref().unwrap(),
            )
            .unwrap();
        assert_eq!(
            status.state,
            super::super::CompletionState::Blocked,
            "TE13_FIX5_STORED_REFUSAL_NOT_PENDING"
        );
        assert!(
            owner
                .run_recovery_message(&state)
                .unwrap()
                .unwrap()
                .contains("Stop this run")
        );
    }
    te13_fix4_replay(&p);
    assert!(p.run.f.projection(&p.run.run).results.is_empty());
    assert!(p.run.f.projection(&p.run.run).deliveries.is_empty());
}

#[test]
fn te13_fix5_metadata_publishes_with_pending_finish() {
    let f = Fixture::new();
    let publisher = f.actor(AttachmentRole::Producer, "fix5-metadata-publisher");
    let issued = Fixture::issue(&publisher);
    let r = c5_fix10_run_from(f);
    let p = te13_fix4_pending_run(r, CompletionStep::Publication, false, None);
    let host = p.run.f.host.owner.upgrade().unwrap();
    let update = {
        let owner = host.lock().unwrap();
        let (checkpoint_ref, checkpoint) = owner.owner().unwrap().manual_checkpoint().unwrap();
        MetadataUpdate {
            expected_root_generation: owner.owner().unwrap().metadata_generation().unwrap(),
            change: MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: checkpoint.head_id,
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        }
    };
    let reply = publisher
        .request(HostCommand::Metadata {
            envelope: issued.envelope,
            update,
        })
        .unwrap();
    assert!(
        reply.error.is_none(),
        "TE13_FIX5_METADATA_WITH_PENDING_SUCCESS {reply:?}"
    );
    assert!(host.lock().unwrap().hooks.pending.is_some());
    te13_fix4_replay(&p);
    let state = p.run.f.projection(&p.run.run);
    assert_eq!((state.results.len(), state.deliveries.len()), (1, 1));
}

#[test]
fn te13_fix5_refused_stop_does_not_abandon_finish() {
    let p = te13_fix4_pending(CompletionStep::Freeze, false, false);
    let host = p.run.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&p.run.f.editor.bootstrap.auth.bearer()))
            .unwrap();
        let subject = owner.metadata_subject(index);
        let issuer = owner
            .owner_mut()
            .unwrap()
            .reuse_metadata_issuer_for(&subject)
            .unwrap();
        let id = kinewright_project::receipts::CommandId {
            sequence: owner
                .owner()
                .unwrap()
                .metadata_issuer_sequence(&issuer)
                .unwrap(),
            issuer,
        };
        let mut stop = p.request.clone();
        stop.envelope.id = id.clone();
        stop.envelope.expected_generation = owner.owner().unwrap().metadata_generation().unwrap();
        stop.envelope.authority = AuthorityBinding {
            grant_ref: owner.expected_grant(index).unwrap(),
            owner_epoch: owner.owner().unwrap().store().unwrap().owner_epoch(),
            endpoint: None,
            attempt: None,
        };
        stop.envelope.command = OrchestrationCommand::ControlRun(RunIntent {
            command_id: id,
            run_id: p.run.run.clone(),
            expected_control_revision: serde_json::from_value(serde_json::json!("0".repeat(32)))
                .unwrap(),
            action: RunAction::Stop,
        });
        stop.envelope.digest = stop.envelope.computed_digest().unwrap();
        let refused = owner.dispatch_orchestration(index, &stop).unwrap();
        assert!(
            matches!(refused.decision, CommandDecision::Refused(_)),
            "TE13_FIX5_REAL_REFUSED_STOP"
        );
        assert!(
            owner.hooks.runs[&p.run.run].recovery_blocks.is_empty(),
            "TE13_FIX5_REFUSED_STOP_NO_FALSE_BLOCK"
        );
    }
    te13_fix5_waiting(&p);
    te13_fix4_replay(&p);
    assert_eq!(
        (
            p.run.f.projection(&p.run.run).results.len(),
            p.run.f.projection(&p.run.run).deliveries.len()
        ),
        (1, 1)
    );
}
