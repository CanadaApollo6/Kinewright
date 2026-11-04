fn te12_fixed_context() -> serde_json::Value {
    serde_json::json!({"version":1,"state":{"project":"11111111111111111111111111111111","run":"11111111111111111111111111111111","phase":"running","outcome":{"kind":"pending"},"sequence":42,"task":"33333333333333333333333333333333","task_state":{"kind":"running","detail":"44444444444444444444444444444444"}},"controls":{"id":{"issuer":"55555555555555555555555555555555","sequence":7},"attempt":"44444444444444444444444444444444","expected_generation":99,"owner_epoch":1,"expected_head":"66666666666666666666666666666666"}})
}

#[test]
fn te12_fresh_controls_and_stale_epoch_generation_head() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let command = || {
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief,
        }))
    };
    for (field, value, marker) in [
        (
            "attempt",
            serde_json::json!("0".repeat(32)),
            "TE12_STALE_ATTEMPT",
        ),
        ("owner_epoch", serde_json::json!(0), "TE12_STALE_EPOCH"),
        (
            "expected_generation",
            serde_json::json!(0),
            "TE12_STALE_GENERATION",
        ),
        (
            "expected_head",
            serde_json::json!("0".repeat(32)),
            "TE12_STALE_HEAD",
        ),
    ] {
        let mut controls = director.control(command());
        controls[field] = value;
        let refused = director.wire("assign_task", controls);
        assert_eq!(refused.is_error, Some(true), "{marker} {refused:?}");
        let refreshed = refused.structured_content.unwrap();
        assert!(
            refreshed["controls"].is_object(),
            "TE12_ERROR_REFRESH {refreshed}"
        );
        assert_eq!(f.projection(&run).tasks.len(), 1, "TE12_NO_STALE_EFFECT");
    }
    let mut controls = director
        .wire("task_status", serde_json::json!({}))
        .structured_content
        .unwrap()["controls"]
        .clone();
    controls["command"] = serde_json::to_value(command()).unwrap();
    let applied = director.wire("assign_task", controls.clone());
    assert_eq!(
        applied.is_error,
        Some(false),
        "TE12_FRESH_CONTROL {applied:?}"
    );
    let applied = applied.structured_content.unwrap();
    let repeated = director
        .wire("assign_task", controls)
        .structured_content
        .unwrap();
    assert_eq!(
        applied["outcome"], repeated["outcome"],
        "TE12_IMMUTABLE_OUTCOME"
    );
    assert_eq!(
        applied["receipt"], repeated["receipt"],
        "TE12_IMMUTABLE_RECEIPT"
    );
}

#[test]
fn te12_role_manifest_and_dispatch_are_durable() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    c4_reserve_task(&f, &run, &child);
    let editor = C4Mcp::new(&f, &run, &child);
    let listed = |mcp: &C4Mcp| {
        mcp.runtime
            .block_on(mcp.client.list_all_tools())
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>()
    };
    assert!(
        listed(&director).iter().any(|name| name == "assign_task"),
        "TE12_LIST_ALLOWED"
    );
    assert!(
        !listed(&editor).iter().any(|name| name == "assign_task"),
        "TE12_LIST_HIDDEN"
    );
    assert_eq!(
        listed(&editor),
        task_mcp::manifest::EDITOR,
        "TE12_LIST_FORGED_ROLE"
    );
    let status = editor.wire("task_status", serde_json::json!({}));
    assert_eq!(status.is_error, Some(false), "TE12_DISPATCH_ALLOWED");
    let hidden = director.wire(
        "prepare_edit_plan",
        serde_json::json!({"expected_revision":0,"operations":[]}),
    );
    assert_eq!(hidden.is_error, Some(true), "TE12_DISPATCH_HIDDEN");
    let forged = editor.wire("task_status", serde_json::json!({"role":"director"}));
    assert_eq!(forged.is_error, Some(true), "TE12_DISPATCH_FORGED_ROLE");
    let old = editor.wire(
        "invoke_capability",
        serde_json::json!({"name":"task_status","arguments":{}}),
    );
    assert_eq!(old.is_error, Some(true), "TE12_OLD_DISPATCHER");
}

#[test]
fn te12_receipt_reconciliation_is_read_only_and_scope_bound() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let result = director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief,
        })),
    );
    let receipt: OrchestrationReceipt = serde_json::from_value(result["receipt"].clone()).unwrap();
    let query = ReceiptQuery {
        project: execution.project.clone(),
        run: run.clone(),
        task: execution.task.clone(),
        attempt: execution.attempt.clone(),
        id: receipt.envelope.id.clone(),
        digest: Some(task_command_digest(&receipt.envelope.command).unwrap()),
        command: None,
    };
    let reconcile = |query| {
        f.editor
            .request(HostCommand::ReconcileTaskReceipt { query })
            .unwrap()
    };
    let before = f.projection(&run);
    let stored = reconcile(query.clone());
    assert!(stored.error.is_none(), "TE12_RECEIPT_READ {stored:?}");
    let Some(ReceiptReply::Stored { outcome, .. }) = stored.task_receipt else {
        panic!("TE12_STORED_RECEIPT");
    };
    assert_eq!(outcome, result, "TE12_RECEIPT_ORIGINAL_OUTCOME");
    let mut wrong_task = query.clone();
    wrong_task.task = before.tasks[1].id.clone();
    assert!(
        reconcile(wrong_task).error.is_some(),
        "TE12_CROSS_TASK_RECEIPT"
    );
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_receipt(&identity(), &query.id)
            .is_err(),
        "TE12_STORE_CROSS_RUN"
    );
    let mut wrong_run = query.clone();
    wrong_run.run = identity();
    assert!(
        reconcile(wrong_run).error.is_some(),
        "TE12_CROSS_RUN_RECEIPT"
    );
    let mut changed = query.clone();
    changed.digest = serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap();
    assert!(
        reconcile(changed).error.is_some(),
        "TE12_CHANGED_RECEIPT_DIGEST"
    );
    let mut absent = query.clone();
    absent.id.sequence += 100;
    assert!(
        matches!(reconcile(absent).task_receipt, Some(ReceiptReply::Unknown)),
        "TE12_UNKNOWN_RECEIPT"
    );
    let owner = f.host.owner.upgrade().unwrap();
    owner.lock().unwrap().hooks.runs[&run].tasks[&execution.task]
        .endpoint()
        .revoke();
    assert!(
        reconcile(query).error.is_none(),
        "TE12_RECEIPT_AFTER_REVOCATION"
    );
    assert_eq!(
        f.projection(&run),
        before,
        "TE12_RECONCILIATION_HAS_NO_EFFECT"
    );
    let status = director.wire("task_status", serde_json::json!({}));
    assert_eq!(status.is_error, Some(true), "TE12_NO_ENDPOINT_REVIVAL");
    assert!(
        status.structured_content.unwrap().get("controls").is_none(),
        "TE12_NO_RECOVERY_CONTROLS"
    );
    drop(director);
    let claim = f.temp.path("te12-receipt-recovery");
    c5_fix13_stop_settle(&f, &run, &execution, &claim);
    c5_fix13_archive(&f, &run, &execution).unwrap();
    assert!(
        f.projection(&run).attempts.is_empty(),
        "TE12_ATTEMPTS_ARCHIVED"
    );
    let query = ReceiptQuery {
        project: execution.project,
        run: run.clone(),
        task: execution.task,
        attempt: execution.attempt,
        id: receipt.envelope.id,
        digest: Some(task_command_digest(&receipt.envelope.command).unwrap()),
        command: None,
    };
    assert!(
        reconcile(query).error.is_none(),
        "TE12_RECEIPT_AFTER_ARCHIVE"
    );
}

#[test]
fn te12_terminal_reply_keeps_receipt_and_state_without_controls() {
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let (client, endpoint) = task_actor(&f, &run, &execution, &producer);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let director = C4Mcp::new(&f, &run, &execution.task);
    let frozen = director.wire(
        "freeze_task",
        director.control(OrchestrationCommand::FreezeTask(execution.task.clone())),
    );
    assert_eq!(frozen.is_error, Some(false), "TE12_FREEZE_REPLY {frozen:?}");
    let frozen = frozen.structured_content.unwrap();
    let mut control = frozen["controls"].clone();
    control["command"] = serde_json::to_value(OrchestrationCommand::PublishResult(Box::new(
        PublishResult {
            task: execution.task,
            result: serde_json::from_value(frozen["outcome"]["result"].clone()).unwrap(),
        },
    )))
    .unwrap();
    let published = director.wire("publish_result", control);
    assert_eq!(
        published.is_error,
        Some(false),
        "TE12_TERMINAL_COMMIT {published:?}"
    );
    let published = published.structured_content.unwrap();
    assert!(published["receipt"].is_object(), "TE12_TERMINAL_RECEIPT");
    assert!(
        published["state"]["task_state"].is_object(),
        "TE12_TERMINAL_STATE"
    );
    assert!(
        published.get("controls").is_none(),
        "TE12_TERMINAL_NO_CONTROLS {published}"
    );
    assert!(matches!(
        f.projection(&run).tasks[0].state,
        TaskState::Terminal(_)
    ));
}

#[test]
fn te12_merge_receipt_retains_original_director_attempt() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    let r = c5_fix10_run();
    let frozen = c5_fix10_freeze(&r.director, &r.child);
    let published = r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: frozen,
        })),
    );
    let result =
        r.f.projection(&r.run)
            .tasks
            .iter()
            .find_map(|task| {
                if task.id == r.child
                    && let TaskState::Terminal(reference) = &task.state
                {
                    Some(reference.clone())
                } else {
                    None
                }
            })
            .unwrap();
    let merged = r.director.accepted(
        "merge_task",
        OrchestrationCommand::MergeTask(MergeTask {
            task: r.child.clone(),
            target: r.f.brief.scope.base.clone(),
            result,
        }),
    );
    let query: ReceiptQuery = serde_json::from_value(merged["reconciliation"].clone()).unwrap();
    assert_eq!(query.task, r.parent.task, "TE12_MERGE_CALLER");
    assert_eq!(query.attempt, r.parent.attempt, "TE12_MERGE_ATTEMPT");
    let read =
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt {
                query: query.clone(),
            })
            .unwrap();
    assert!(read.error.is_none(), "TE12_MERGE_RECEIPT_READ {read:?}");
    let Some(ReceiptReply::Stored { outcome, .. }) = read.task_receipt else {
        panic!("TE12_MERGE_STORED");
    };
    assert_eq!(outcome, merged, "TE12_MERGE_ORIGINAL");
    let mut changed = query;
    changed.attempt = r.attempt.attempt;
    assert!(
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt { query: changed })
            .unwrap()
            .error
            .is_some(),
        "TE12_MERGE_WRONG_ATTEMPT"
    );
    assert!(published["receipt"].is_object());
}

#[test]
fn te12_scoped_reads_and_initialization_controls() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let info = director.client.peer_info().unwrap();
    let text = info.instructions.as_ref().unwrap();
    let seed: serde_json::Value =
        serde_json::from_str(text.split("Current live reply metadata: ").nth(1).unwrap()).unwrap();
    assert!(seed["controls"].is_object(), "TE12_INITIAL_CONTROLS");
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let mut control = seed["controls"].clone();
    control["command"] =
        serde_json::to_value(OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task,
            brief,
        })))
        .unwrap();
    let first = director.wire("assign_task", control);
    assert_eq!(
        first.is_error,
        Some(false),
        "TE12_INITIAL_CONTROL_WITHOUT_PREFETCH {first:?}"
    );

    let read = director.wire(
        "read_task_evidence",
        serde_json::json!({"reference":f.brief.plan}),
    );
    assert_eq!(read.is_error, Some(false), "TE12_EVIDENCE_ALLOWED {read:?}");
    let inputs = director.wire(
        "read_task_evidence",
        serde_json::json!({"reference":f.brief.inputs}),
    );
    assert_eq!(
        inputs.is_error,
        Some(false),
        "TE12_INPUTS_ALLOWED {inputs:?}"
    );
    let other = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"unrelated private data")
        .unwrap();
    let refused = director.wire("read_task_evidence", serde_json::json!({"reference":other}));
    assert_eq!(refused.is_error, Some(true), "TE12_EVIDENCE_SCOPE");
    assert_eq!(f.projection(&run).tasks.len(), 2);
}

fn te12_list(mcp: &C4Mcp) -> Vec<String> {
    mcp.runtime
        .block_on(mcp.client.list_all_tools())
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}
#[test]
fn te12_list_allowed() {
    let f = Fixture::new();
    let (_, _, director) = c4_director(&f);
    assert!(
        te12_list(&director)
            .iter()
            .any(|name| name == "assign_task"),
        "TE12_LIST_ALLOWED"
    );
}
#[test]
fn te12_list_hidden() {
    let f = Fixture::new();
    let (_, _, director) = c4_director(&f);
    assert!(
        !te12_list(&director)
            .iter()
            .any(|name| name == "prepare_edit_plan"),
        "TE12_LIST_HIDDEN"
    );
}
#[test]
fn te12_list_forged_role() {
    let f = Fixture::new();
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task,
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    c4_reserve_task(&f, &run, &child);
    let editor = C4Mcp::new(&f, &run, &child);
    assert_eq!(
        te12_list(&editor),
        task_mcp::manifest::EDITOR,
        "TE12_LIST_FORGED_ROLE"
    );
}
#[test]
fn te12_dispatch_allowed() {
    let r = c5_fix10_run();
    let reply = r.editor.wire("get_timeline_state", serde_json::json!({}));
    assert_eq!(
        reply.is_error,
        Some(false),
        "TE12_DISPATCH_ALLOWED {reply:?}"
    );
}
#[test]
fn te12_dispatch_hidden() {
    let r = c5_fix10_run();
    let reply = r.director.wire("prepare_edit_plan",serde_json::json!({"expected_revision":0,"operations":[{"op":"add_marker","marker":{"id":1,"position":1,"label":"marker 1","color_token":0}}]}));
    assert_eq!(reply.is_error, Some(true), "TE12_DISPATCH_HIDDEN {reply:?}");
}
#[test]
fn te12_dispatch_forged_role() {
    let r = c5_fix10_run();
    let reply = r
        .editor
        .wire("task_status", serde_json::json!({"role":"director"}));
    assert_eq!(
        reply.is_error,
        Some(true),
        "TE12_DISPATCH_FORGED_ROLE {reply:?}"
    );
}
#[test]
fn te12_old_dispatcher_is_refused() {
    let r = c5_fix10_run();
    let reply = r.editor.wire(
        "invoke_capability",
        serde_json::json!({"name":"task_status","arguments":{}}),
    );
    assert_eq!(reply.is_error, Some(true), "TE12_OLD_DISPATCHER {reply:?}");
}
#[test]
fn te12_freeze_reconciliation_is_exact_and_does_not_restore_actor() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    let r = c5_fix10_run();
    let frozen = r.editor.wire(
        "freeze_task",
        r.editor
            .control(OrchestrationCommand::FreezeTask(r.child.clone())),
    );
    assert_eq!(
        frozen.is_error,
        Some(false),
        "TE12_FREEZE_ORIGINAL {frozen:?}"
    );
    let frozen = frozen.structured_content.unwrap();
    let receipt: OrchestrationReceipt = serde_json::from_value(frozen["receipt"].clone()).unwrap();
    let query = ReceiptQuery {
        project: r.attempt.project.clone(),
        run: r.run.clone(),
        task: r.child.clone(),
        attempt: r.attempt.attempt.clone(),
        id: receipt.envelope.id,
        digest: Some(task_command_digest(&receipt.envelope.command).unwrap()),
        command: None,
    };
    c5_fix10_tick(&r.f, &r.run, &r.attempt);
    let host = r.f.host.owner.upgrade().unwrap();
    let (before, generation, objects) = {
        let mut owner = host.lock().unwrap();
        owner
            .hooks
            .runs
            .get_mut(&r.run)
            .unwrap()
            .tasks
            .remove(&r.child);
        (
            owner
                .owner()
                .unwrap()
                .orchestration_projection(&r.run)
                .unwrap(),
            owner.owner().unwrap().metadata_generation().unwrap(),
            owner
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .objects,
        )
    };
    let read =
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt { query })
            .unwrap();
    assert!(read.error.is_none(), "TE12_FREEZE_READ {read:?}");
    let Some(ReceiptReply::Stored { outcome, .. }) = read.task_receipt else {
        panic!("TE12_FREEZE_STORED");
    };
    assert_eq!(outcome, frozen["outcome"], "TE12_FREEZE_IMMUTABLE_OUTCOME");
    let owner = host.lock().unwrap();
    assert!(
        !owner.hooks.runs[&r.run].tasks.contains_key(&r.child),
        "TE12_RECONCILE_NO_ACTOR"
    );
    assert_eq!(
        owner.owner().unwrap().metadata_generation().unwrap(),
        generation,
        "TE12_RECONCILE_NO_PUBLICATION"
    );
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap()
            .objects,
        objects,
        "TE12_RECONCILE_NO_OBJECT_WRITES"
    );
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .orchestration_projection(&r.run)
            .unwrap(),
        before,
        "TE12_RECONCILE_NO_STATE_CHANGE"
    );
}

#[test]
fn te12_hidden_operation_is_refused_before_planning() {
    let r = c5_fix10_run();
    let revision = grant_test_revision(&r.core);
    let before = r.core.export_history().unwrap();
    let reply = r.editor.wire("prepare_edit_plan",serde_json::json!({"expected_revision":revision,
        "operations":[{"op":"convert_legacy_look","clip":0,"effect":0,"lut_asset":0,"mix_basis_points":10000}]}));
    assert_eq!(reply.is_error, Some(true), "TE12_OPERATION_HIDDEN");
    assert!(
        serde_json::to_string(&reply.content)
            .unwrap()
            .contains("root_refused"),
        "TE12_OPERATION_BEFORE_PLANNING {reply:?}"
    );
    assert_eq!(
        r.core.export_history().unwrap(),
        before,
        "TE12_HIDDEN_OPERATION_NO_EFFECT"
    );
}

#[test]
fn te12_evidence_cannot_supply_a_receipt() {
    let mut f = Fixture::new();
    let data =
        serde_json::json!({"receipt":{"decision":{"kind":"applied"}},"controls":{"owner_epoch":0}});
    let reference = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(&serde_json::to_vec(&data).unwrap())
        .unwrap();
    f.brief.plan = reference.clone();
    let (_, _, director) = c4_director(&f);
    let read = director.wire(
        "read_task_evidence",
        serde_json::json!({"reference":reference}),
    );
    assert_eq!(read.is_error, Some(false), "TE12_EVIDENCE_DATA {read:?}");
    let read = read.structured_content.unwrap();
    assert_eq!(read["outcome"], data, "TE12_EVIDENCE_UNMODIFIED");
    assert!(
        read["receipt"].is_null(),
        "TE12_EVIDENCE_NOT_RECEIPT {read}"
    );
    assert_ne!(
        read["controls"]["owner_epoch"], data["controls"]["owner_epoch"],
        "TE12_EVIDENCE_NOT_CONTROLS"
    );
}

#[test]
fn te12_pending_receipt_is_bound_and_read_only() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let mut control = director.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: execution.task.clone(),
        brief,
    })));
    let owner = f.host.owner.upgrade().unwrap();
    let (query, before) = {
        let mut host = owner.lock().unwrap();
        let index = host
            .attachments
            .iter()
            .position(|attachment| {
                attachment
                    .producer_scope
                    .as_ref()
                    .is_some_and(|scope| scope.task.as_ref() == Some(&execution.task))
            })
            .unwrap();
        let endpoint = host.hooks.runs[&run].tasks[&execution.task].endpoint();
        let mut envelope = OrchestrationEnvelope {
            id: serde_json::from_value(control["id"].take()).unwrap(),
            digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
            project: execution.project.clone(),
            expected_generation: serde_json::from_value(control["expected_generation"].take())
                .unwrap(),
            authority: AuthorityBinding {
                grant_ref: host.expected_grant(index).unwrap(),
                owner_epoch: endpoint.authority().owner_epoch,
                endpoint: Some(endpoint.authority().endpoint.clone()),
                attempt: Some(execution.attempt.clone()),
            },
            command: serde_json::from_value(control["command"].take()).unwrap(),
        };
        envelope.digest = envelope.computed_digest().unwrap();
        let query = ReceiptQuery {
            project: execution.project,
            run: run.clone(),
            task: execution.task,
            attempt: execution.attempt,
            id: envelope.id.clone(),
            digest: Some(task_command_digest(&envelope.command).unwrap()),
            command: None,
        };
        host.hooks.pending = Some(super::super::orchestration::PendingPublication::Wire(
            Box::new(ScopedOrchestrationRequest {
                run: Some(run.clone()),
                envelope,
                consume_batch: None,
            }),
            host.attachments[index].id,
        ));
        (query, host.owner().unwrap().metadata_generation().unwrap())
    };
    let read = f
        .editor
        .request(HostCommand::ReconcileTaskReceipt {
            query: query.clone(),
        })
        .unwrap();
    let mut changed = query;
    changed.task = identity();
    let refused = f
        .editor
        .request(HostCommand::ReconcileTaskReceipt { query: changed })
        .unwrap();
    let after = {
        let mut host = owner.lock().unwrap();
        host.hooks.pending = None;
        host.owner().unwrap().metadata_generation().unwrap()
    };
    assert!(read.error.is_none(), "TE12_PENDING_READ {read:?}");
    assert!(
        matches!(read.task_receipt, Some(ReceiptReply::Pending)),
        "TE12_PENDING_STATUS"
    );
    assert!(refused.error.is_some(), "TE12_PENDING_SCOPE");
    assert_eq!(before, after, "TE12_PENDING_READ_NO_WRITE");
}

#[test]
fn te12_lost_reply_uses_only_the_original_sent_command() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    let r = c5_fix10_run();
    let frozen = c5_fix10_freeze(&r.editor, &r.child);
    c5_fix10_tick(&r.f, &r.run, &r.attempt);
    let command = OrchestrationCommand::PublishResult(Box::new(PublishResult {
        task: r.child.clone(),
        result: frozen,
    }));
    let control = r.editor.control(command.clone());
    // Construct recovery entirely BEFORE sending: no reply digest, grant ref,
    // endpoint identity or receipt is available to the caller.
    let query = ReceiptQuery {
        project: r.attempt.project.clone(),
        run: r.run.clone(),
        task: r.child.clone(),
        attempt: r.attempt.attempt.clone(),
        id: serde_json::from_value(control["id"].clone()).unwrap(),
        digest: None,
        command: Some(command.clone()),
    };
    let published = r.editor.wire("publish_result", control);
    assert_eq!(
        published.is_error,
        Some(false),
        "TE12_LOST_REPLY_COMMIT {published:?}"
    );
    let expected = published.structured_content.unwrap()["outcome"].clone();
    let read =
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt {
                query: query.clone(),
            })
            .unwrap();
    assert!(
        read.error.is_none(),
        "TE12_LOST_REPLY_FROM_SENT_COMMAND {read:?}"
    );
    let Some(ReceiptReply::Stored { outcome, .. }) = read.task_receipt else {
        panic!("TE12_LOST_REPLY_STORED");
    };
    assert_eq!(outcome, expected, "TE12_LOST_REPLY_EXACT");
    let mut changed = query;
    changed.command = Some(OrchestrationCommand::FreezeTask(r.child.clone()));
    assert!(
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt { query: changed })
            .unwrap()
            .error
            .is_some(),
        "TE12_LOST_REPLY_CHANGED_COMMAND"
    );
}

#[test]
fn te12_changed_publication_metadata_is_not_an_exact_replay() {
    let r = c5_fix10_run();
    let frozen = c5_fix10_freeze(&r.director, &r.child);
    let control = c5_fix10_publish(&r.director, &r.child, frozen);
    let first = r.director.wire("publish_result", control.clone());
    assert_eq!(first.is_error, Some(false), "TE12_REPLAY_FIRST {first:?}");
    let mut changed = control;
    changed["command"]["value"]["result"]["outcome"] = serde_json::json!("failed");
    let refused = r.director.wire("publish_result", changed);
    assert_eq!(
        refused.is_error,
        Some(true),
        "TE12_CHANGED_PUBLICATION_METADATA {refused:?}"
    );
}

#[test]
fn te12_recovery_mode_does_not_issue_controls() {
    let f = Fixture::new();
    let (_, _, director) = c4_director(&f);
    let owner = f.host.owner.upgrade().unwrap();
    owner.lock().unwrap().native_recovery_pending = true;
    let reply = director.wire("task_status", serde_json::json!({}));
    owner.lock().unwrap().native_recovery_pending = false;
    assert!(
        reply
            .structured_content
            .as_ref()
            .unwrap()
            .get("controls")
            .is_none(),
        "TE12_RECOVERY_NO_CONTROLS {reply:?}"
    );
    assert_eq!(reply.is_error, Some(true), "TE12_RECOVERY_REFUSED");
}

fn te12_legacy_history(r: &C5Fix10Run) -> TaskHistoryRecord {
    let owner = r.f.host.owner.upgrade().unwrap();
    let mut owner = owner.lock().unwrap();
    let mut state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&r.run)
        .unwrap()
        .unwrap();
    let mut history = owner.saved_task_history(&state, &r.child).unwrap().unwrap();
    history.frozen_result = None;
    history.merge_authority = None;
    let reference = owner
        .owner_mut()
        .unwrap()
        .admit_scheduler_record(&OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::TaskHistory(Box::new(history.clone())),
        })
        .unwrap()
        .reference;
    state
        .tasks
        .iter_mut()
        .find(|task| task.id == r.child)
        .unwrap()
        .progress
        .decisions
        .push(reference);
    let producer = owner.run_producer(&r.run).unwrap();
    owner
        .publish_native(&producer, state, &r.attempt, vec![])
        .unwrap();
    history
}
#[test]
fn te12_legacy_frozen_history_is_not_upgraded() {
    let r = c5_fix10_run();
    c5_fix10_freeze(&r.director, &r.child);
    let legacy = te12_legacy_history(&r);
    let owner = r.f.host.owner.upgrade().unwrap();
    let mut owner = owner.lock().unwrap();
    let producer = owner.run_producer(&r.run).unwrap();
    let saved = owner
        .checkpoint_task_history(&producer, &r.run, &r.child)
        .unwrap();
    assert_eq!(saved, legacy, "TE12_LEGACY_FROZEN_NOT_UPGRADED");
}
#[test]
fn te12_legacy_merge_cannot_gain_current_provenance() {
    let r = c5_fix10_run();
    let frozen = c5_fix10_freeze(&r.director, &r.child);
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: frozen,
        })),
    );
    let result = r.f.projection(&r.run).results.last().unwrap().clone();
    let control = r
        .director
        .control(OrchestrationCommand::MergeTask(MergeTask {
            task: r.child.clone(),
            target: r.f.brief.scope.base.clone(),
            result,
        }));
    let merged = r.director.wire("merge_task", control.clone());
    assert_eq!(
        merged.is_error,
        Some(false),
        "TE12_LEGACY_MERGE_SETUP {merged:?}"
    );
    te12_legacy_history(&r);
    let refused = r.director.wire("merge_task", control);
    assert_eq!(
        refused.is_error,
        Some(true),
        "TE12_LEGACY_ORIGIN_NOT_MINTED {refused:?}"
    );
    let owner = r.f.host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&r.run)
        .unwrap()
        .unwrap();
    assert!(
        owner
            .saved_task_history(&state, &r.child)
            .unwrap()
            .unwrap()
            .merge_authority
            .is_none(),
        "TE12_LEGACY_ORIGIN_UNCHANGED"
    );
}
