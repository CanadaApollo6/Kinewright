struct Te13Fix4Pending {
    run: C5Fix10Run,
    client: HostClient,
    request: ScopedOrchestrationRequest,
    receipt: OrchestrationReceipt,
    unrelated: Option<RunId>,
}
fn te13_fix4_pending(step: CompletionStep, committed: bool, other_run: bool) -> Te13Fix4Pending {
    let mut f = Fixture::new();
    let unrelated = if other_run {
        // Independent runs own independent candidates, even when the earlier
        // run has already stopped. Preserve the Stop-isolation scenario.
        let original = f.brief.scope.base.clone();
        f.brief.scope.base = {
            let host = f.host.owner.upgrade().unwrap();
            let mut host = host.lock().unwrap();
            let owner = host.owner_mut().unwrap();
            let mut candidate = owner.fork_durable_branch("unrelated run").unwrap();
            owner.persist_durable_branch(&mut candidate).unwrap();
            owner.candidate_binding(candidate.branch_id()).unwrap()
        };
        let prior = RunId(f.start().envelope.identity("run").unwrap());
        f.brief.scope.base = original;
        let stop_request = f.control(&f.editor, &prior, RunAction::Stop);
        let reply = call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(stop_request),
            },
            0,
        );
        assert_eq!(
            reply.orchestration.unwrap().decision,
            CommandDecision::Applied
        );
        Some(prior)
    } else {
        None
    };
    let run = c5_fix10_run_from(f);
    te13_fix4_pending_run(run, step, committed, unrelated)
}
fn te13_fix4_pending_run(
    run: C5Fix10Run,
    step: CompletionStep,
    committed: bool,
    unrelated: Option<RunId>,
) -> Te13Fix4Pending {
    let (client, request) = te13_fix2_request(&run);
    let operation = if committed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    let nth = if step == CompletionStep::Freeze { 2 } else { 3 };
    let first = kinewright_project::io_observer::fail_nth(operation, nth, || {
        te13_fix2_dispatch(&run, &client, &request)
    });
    let receipt = first.orchestration.unwrap();
    assert_eq!(receipt.decision, CommandDecision::Applied);
    let host = run.f.host.owner.upgrade().unwrap();
    {
        let mut host = host.lock().unwrap();
        assert!(matches!(
            host.hooks.pending,
            Some(super::super::orchestration::PendingPublication::Completion(
                _
            ))
        ));
        host.owner_mut().unwrap().reconcile_metadata().unwrap();
        let intent = host
            .owner()
            .unwrap()
            .orchestration_projection(&run.run)
            .unwrap()
            .unwrap()
            .tasks[1]
            .progress
            .completion
            .clone()
            .unwrap();
        assert_eq!(
            host.owner()
                .unwrap()
                .orchestration_receipt(&run.run, &intent.step_id(step).unwrap())
                .unwrap()
                .is_some(),
            committed,
            "TE13_FIX4_REAL_FAULT_FATE"
        );
    }
    Te13Fix4Pending {
        run,
        client,
        request,
        receipt,
        unrelated,
    }
}
fn te13_fix4_control(p: &Te13Fix4Pending) -> serde_json::Value {
    serde_json::json!({"id":p.request.envelope.id,"attempt":p.run.attempt.attempt,
        "expected_generation":p.request.envelope.expected_generation,
        "owner_epoch":p.request.envelope.authority.owner_epoch,
        "expected_head":p.run.f.brief.scope.base.head,"command":p.request.envelope.command})
}
fn te13_fix4_replay(p: &Te13Fix4Pending) {
    let replay = p.run.editor.wire("finish_task", te13_fix4_control(p));
    assert_eq!(
        replay.is_error,
        Some(false),
        "TE13_FIX4_EXACT_FINISH_REPLAY {replay:?}"
    );
    assert_eq!(
        replay.structured_content.unwrap()["outcome"]["receipt"],
        serde_json::to_value(&p.receipt).unwrap(),
        "TE13_FIX4_ORIGINAL_RECEIPT"
    );
}
#[test]
fn te13_fix4_successful_continuation_closes_both_frozen_exceptions() {
    let p = te13_fix4_pending(CompletionStep::Freeze, true, false);
    p.run
        .f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .hooks
        .runs
        .get_mut(&p.run.run)
        .unwrap()
        .recovery_blocks
        .push(super::super::task_completion::RecoveryBlock::Completion(
            p.run.child.clone(),
        ));
    te13_fix4_replay(&p);
    let r = &p.run;
    let stop = r.f.control(&r.f.editor, &r.run, RunAction::Stop);
    let host = r.f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let index = owner
        .resolve_attachment(Some(&r.f.editor.bootstrap.auth.bearer()))
        .unwrap();
    let issuance = HostCommand::IssueMetadataIssuer { issuer: None };
    let stop = HostCommand::DispatchOrchestration {
        request: Box::new(stop),
    };
    assert!(
        owner.hooks.runs[&r.run].recovery_blocks.is_empty(),
        "TE13_FIX4_SUCCESS_NO_RAW_BLOCK"
    );
    // A stale cache entry must also fail the current durable predicate.
    owner
        .hooks
        .runs
        .get_mut(&r.run)
        .unwrap()
        .recovery_blocks
        .push(super::super::task_completion::RecoveryBlock::Completion(
            r.child.clone(),
        ));
    assert!(
        !owner.completion_owner_recovery(index, &issuance),
        "TE13_FIX4_CURRENT_ISSUANCE_GUARD"
    );
    assert!(
        !owner.completion_owner_recovery(index, &stop),
        "TE13_FIX4_CURRENT_STOP_GUARD"
    );
    for recovering in [true, false] {
        owner.recovering = recovering;
        owner.switch_frozen = !recovering;
        for command in [issuance.clone(), stop.clone()] {
            let reply = owner.execute_hook(index, command, &r.f.editor.bootstrap.roots);
            assert_eq!(
                reply.error.unwrap().code,
                "recovery_frozen",
                "TE13_FIX4_NO_HEALTHY_FROZEN_BYPASS"
            );
        }
    }
}
#[test]
fn te13_fix4_refused_continuation_keeps_current_block_and_receipt() {
    let p = te13_fix4_pending(CompletionStep::Freeze, true, false);
    let r = &p.run;
    let host = r.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&p.client.bootstrap.auth.bearer()))
            .unwrap();
        owner.attachments[index]
            .producer_scope
            .as_mut()
            .unwrap()
            .expires_at = Some(Instant::now());
        assert!(
            owner
                .reconcile_completion_substep(index, &p.request)
                .is_err()
        );
        assert!(owner.hooks.pending.is_none());
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&r.run)
            .unwrap()
            .unwrap();
        assert!(
            !owner.run_recovery_blocks(&state).unwrap().is_empty(),
            "TE13_FIX4_REFUSED_CONTINUATION_BLOCKS"
        );
        let index = owner
            .resolve_attachment(Some(&r.f.editor.bootstrap.auth.bearer()))
            .unwrap();
        assert!(
            owner.completion_owner_recovery(
                index,
                &HostCommand::IssueMetadataIssuer { issuer: None }
            ),
            "TE13_FIX4_LIVE_BLOCK_ALLOWS_ISSUANCE"
        );
    }
    te13_fix4_replay(&p);
    let state = r.f.projection(&r.run);
    assert!(
        state.results.is_empty() && state.deliveries.is_empty(),
        "TE13_FIX4_RECEIPT_READ_NEVER_CONTINUES"
    );
}
#[test]
fn te13_fix4_after_stop_reads_receipt_without_executing_absent_step() {
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let r = &p.run;
    let stop = r.f.control(&r.f.editor, &r.run, RunAction::Stop);
    let reply = te13_fix2_dispatch(r, &r.f.editor, &stop);
    assert_eq!(
        reply.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    te13_fix4_retired_receipt(&p);
    let state = r.f.projection(&r.run);
    assert!(
        state.results.is_empty() && state.deliveries.is_empty(),
        "TE13_FIX4_STOP_REPLAY_NO_PUBLICATION"
    );
}

fn te13_fix4_retired_receipt(p: &Te13Fix4Pending) {
    let mcp_result = p.run.editor.wire("finish_task", te13_fix4_control(p));
    assert_eq!(
        mcp_result.is_error,
        Some(true),
        "TE13_FIX4_RETIRED_MCP_CANNOT_MUTATE"
    );
    assert!(
        serde_json::to_string(&mcp_result.content)
            .unwrap()
            .contains("attachment_refused")
    );
    let reply = call(
        &p.run.f,
        &p.run.f.editor,
        HostCommand::ReconcileTaskReceipt {
            query: super::super::receipt_reconciliation::ReceiptQuery {
                project: p.request.envelope.project.clone(),
                run: p.run.run.clone(),
                task: p.run.child.clone(),
                attempt: p.run.attempt.attempt.clone(),
                id: p.request.envelope.id.clone(),
                digest: Some(task_command_digest(&p.request.envelope.command).unwrap()),
                command: None,
            },
        },
        0,
    );
    let Some(super::super::receipt_reconciliation::ReceiptReply::Stored { receipt, .. }) =
        reply.task_receipt
    else {
        panic!("TE13_FIX4_RETIRED_ORIGINAL_RECEIPT {reply:?}")
    };
    assert_eq!(
        receipt,
        serde_json::to_value(&p.receipt).unwrap(),
        "TE13_FIX4_RETIRED_ORIGINAL_RECEIPT"
    );
}

#[test]
fn te13_fix4_committed_note_replay_reads_original_receipt_during_completion() {
    use kinewright_project::studio_records::{
        CommentId, FeedbackAnchor, FeedbackRevision, FrameRate, Orientation,
    };
    let f = Fixture::new();
    let (checkpoint_ref, checkpoint) = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .manual_checkpoint()
        .unwrap();
    let publisher = f.actor(AttachmentRole::Producer, "fix4-snapshot-publisher");
    let snapshot = ok(
        publisher
            .request(metadata_request(
                &f,
                &publisher,
                MetadataChange::PublishSnapshot(PublishSnapshot {
                    version: VersionId::fresh().unwrap(),
                    expected_head: checkpoint.head_id,
                    checkpoint_ref,
                    parent_version: None,
                    run_provenance: None,
                    revision_refs: vec![],
                }),
            ))
            .unwrap(),
        "TE13_FIX4_NOTE_SNAPSHOT",
    )
    .metadata
    .unwrap();
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = snapshot.result
    else {
        panic!("TE13_FIX4_NOTE_SNAPSHOT")
    };
    let strokes = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"[]")
        .unwrap();
    let note = metadata_request(
        &f,
        &f.editor,
        MetadataChange::SaveFeedback(FeedbackRevision {
            comment_id: CommentId::fresh().unwrap(),
            revision: 1,
            predecessor: None,
            anchor: FeedbackAnchor {
                snapshot: key,
                frame: 0,
                fps: FrameRate {
                    numerator: 24,
                    denominator: 1,
                },
                raster_size: [1920, 1080],
                orientation: Orientation::Upright,
                target: None,
                source_ref: None,
                script_beat_id: None,
            },
            original_text_ref: f.brief.plan.clone(),
            strokes_ref: strokes,
            frame_proof_ref: f.brief.plan.clone(),
            voice_ref: None,
            transcript_revision: None,
        }),
    );
    let first = ok(
        f.editor.request(note.clone()).unwrap(),
        "TE13_FIX4_NOTE_FIRST",
    );
    let p = te13_fix4_pending_run(
        c5_fix10_run_from(f),
        CompletionStep::Publication,
        false,
        None,
    );
    let host = p.run.f.host.owner.upgrade().unwrap();
    let pending = serde_json::to_vec(&host.lock().unwrap().hooks.pending).unwrap();
    let root = std::fs::read(p.run.f.temp.path("project.kinewright")).unwrap();
    let replay = p.run.f.editor.request(note).unwrap();
    assert!(
        replay.error.is_none(),
        "TE13_FIX4_COMMITTED_NOTE_REPLAY {replay:?}"
    );
    assert_eq!(
        replay.metadata, first.metadata,
        "TE13_FIX4_COMMITTED_NOTE_RECEIPT"
    );
    assert_eq!(
        serde_json::to_vec(&host.lock().unwrap().hooks.pending).unwrap(),
        pending,
        "TE13_FIX4_NOTE_REPLAY_PRESERVES_PENDING"
    );
    assert_eq!(
        std::fs::read(p.run.f.temp.path("project.kinewright")).unwrap(),
        root,
        "TE13_FIX4_NOTE_REPLAY_READ_ONLY"
    );
    te13_fix4_replay(&p);
}

#[test]
fn te13_fix4_missing_row_fallback_names_no_saved_branch() {
    let p = te13_fix4_pending(CompletionStep::Freeze, true, false);
    let r = &p.run;
    let host = r.f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let index = owner
        .resolve_attachment(Some(&p.client.bootstrap.auth.bearer()))
        .unwrap();
    owner.attachments[index]
        .producer_scope
        .as_mut()
        .unwrap()
        .expires_at = Some(Instant::now());
    assert!(
        owner
            .reconcile_completion_substep(index, &p.request)
            .is_err()
    );
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&r.run)
        .unwrap()
        .unwrap();
    let work = owner
        .owner()
        .unwrap()
        .retained_task_work()
        .unwrap()
        .into_iter()
        .find(|w| w.task == r.child)
        .unwrap();
    let project = owner.owner_mut().unwrap();
    let root = project.root_sha256().unwrap();
    assert!(
        project
            .retirement_disclosure(&work.source.branch)
            .unwrap()
            .contains("retained task"),
        "TE13_FIX4_REMOVAL_WARNING"
    );
    let removed = project.retire_working_branch(&work.source.branch, &root, true);
    assert!(
        matches!(removed, Err(BundleError::Identity(ref why)) if why.contains("This branch is in use by Studio task")),
        "TE13_FIX4_LIVE_BRANCH_REMOVAL_REFUSED {removed:?}"
    );
    assert!(
        owner
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap()
            .iter()
            .any(|w| w.task == r.child)
    );
    assert!(owner.run_recovery_message(&state).unwrap().is_some());
    // The defensive fallback cannot promise a branch when no row was found.
    let text = super::super::task_completion::RecoveryBlock::Completion(r.child.clone())
        .fallback_disclosure();
    assert_eq!(
        text,
        format!(
            "completion_recovery_requires_person: task {} needs recovery. Stop this run and wait for cleanup, then check Studio recovery and rescue for available work.",
            r.child.0.as_str()
        ),
        "TE13_FIX4_TRUTHFUL_MISSING_ROW"
    );
}
#[test]
fn te13_fix4_stop_between_real_switch_prepare_and_confirm() {
    use kinewright_project::bundle_switch::SwitchTarget;
    let p = te13_fix4_pending(CompletionStep::Publication, false, false);
    let r = &p.run;
    let host = r.f.host.owner.upgrade().unwrap();
    let client = "d".repeat(64);
    let (envelope, expected) = {
        let mut owner = host.lock().unwrap();
        owner.acquire_manual(client.clone()).unwrap();
        let project = owner.owner_mut().unwrap();
        let mut branch = project.fork_durable_branch("switch target").unwrap();
        branch
            .core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddMarker {
                    marker: kinewright_core::Marker {
                        id: kinewright_core::MarkerId(77),
                        position: kinewright_core::TimeCode(7),
                        label: "selected work".into(),
                        color_token: 0,
                    },
                },
            ))
            .unwrap();
        project.persist_durable_branch(&mut branch).unwrap();
        let expected = branch.core().export_history().unwrap();
        let branch_id = branch.branch_id().clone();
        project.close_durable_branch(&branch).unwrap();
        let mut reply = HostReply::default();
        owner
            .prepare_switch(
                SwitchTarget::WorkingBranch { branch: branch_id },
                &mut reply,
            )
            .unwrap();
        assert!(owner.switch_frozen);
        (reply.switch_envelope.unwrap(), expected)
    };
    // Studio's actual issuance-then-dispatch shape runs inside the prepared switch.
    let issuance = call(
        &r.f,
        &r.f.editor,
        HostCommand::IssueMetadataIssuer { issuer: None },
        0,
    );
    assert!(
        issuance.error.is_none(),
        "TE13_FIX4_REAL_SWITCH_STOP_ENVELOPE {issuance:?}"
    );

    let stop = r.f.control(&r.f.editor, &r.run, RunAction::Stop);
    let reply = te13_fix2_dispatch(r, &r.f.editor, &stop);
    assert!(
        reply.error.is_none(),
        "TE13_FIX4_REAL_SWITCH_STOP {reply:?}"
    );
    assert_eq!(
        reply.orchestration.unwrap().decision,
        CommandDecision::Applied,
        "TE13_FIX4_REAL_SWITCH_STOP"
    );
    let mut owner = host.lock().unwrap();
    let mut reply = HostReply::default();
    owner.publish_switch(&envelope, &mut reply).unwrap();
    let installed = reply.switch_attachment.unwrap().identity().unwrap();
    let mut confirmed = HostReply::default();
    owner
        .confirm_switch(client.clone(), &installed, &mut confirmed)
        .unwrap();
    assert!(
        confirmed.manual_lease.is_some(),
        "TE13_FIX4_SWITCH_CONFIRMED_AFTER_STOP"
    );
    assert!(!owner.switch_frozen);
    assert_eq!(
        owner.core().export_history().unwrap(),
        expected,
        "TE13_FIX4_SWITCH_EXACT_HISTORY"
    );
    let mut repeated = HostReply::default();
    owner
        .confirm_switch(client, &installed, &mut repeated)
        .unwrap();
    assert_eq!(repeated.manual_lease, confirmed.manual_lease);
}
