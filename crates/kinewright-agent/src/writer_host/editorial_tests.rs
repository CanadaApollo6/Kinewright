fn s5_version(f: &Fixture) -> kinewright_project::studio_records::SnapshotKey {
    let host = f.host.owner.upgrade().unwrap();
    let mut host = host.lock().unwrap();
    let owner = host.owner_mut().unwrap();
    let issuer = owner
        .reuse_metadata_issuer_for("fixture-s5-versions")
        .unwrap();
    let envelope = owner
        .metadata_envelope(CommandId {
            sequence: owner.metadata_issuer_sequence(&issuer).unwrap(),
            issuer,
        })
        .unwrap();
    let update = MetadataUpdate {
        expected_root_generation: owner.metadata_generation().unwrap(),
        change: MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: f.brief.scope.base.head.clone(),
            checkpoint_ref: f.brief.scope.base.checkpoint.clone(),
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    };
    let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
        .metadata_command(
            &envelope,
            kinewright_project::studio_records::MetadataScope::Snapshot,
            &update,
        )
        .unwrap()
    else {
        panic!("S5_VERSION_COMMITTED");
    };
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = receipt.result
    else {
        panic!("S5_VERSION_RECEIPT");
    };
    key
}

fn s5_child(
    f: &Fixture,
    director: &C4Mcp,
    parent: &ExecutionIdentity,
    role: Role,
) -> (ExecutionIdentity, HostClient, C4Mcp) {
    let mut brief = f.brief.clone();
    brief.role = role;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: parent.run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let task = f.projection(&parent.run).tasks.last().unwrap().id.clone();
    let (attempt, client, endpoint, _) = c4_reserve_task(f, &parent.run, &task);
    history_turn(f, &attempt, &client, &endpoint, 1, true);
    (attempt, client, C4Mcp::new(f, &parent.run, &task))
}

fn s5_audit_args(key: &kinewright_project::studio_records::SnapshotKey) -> serde_json::Value {
    serde_json::json!({
        "version": key.version, "snapshot": key, "rulebook_ref": null,
        "entries": [
            {"line_or_beat": "Promise", "output": {"start_micros": 0, "end_micros": 1000},
                "text": "The required promise is missing.", "kind": "retention", "finding": "missing_required_beat"},
            {"line_or_beat": "Opening", "output": {"start_micros": 10, "end_micros": 1000},
                "text": "The opening works.", "kind": "hook", "finding": "resolved"},
            {"line_or_beat": "Close", "output": {"start_micros": 1000, "end_micros": 2000},
                "text": "Try a longer pause.", "kind": "story", "finding": "taste"}
        ]
    })
}

fn s5_accepted(mcp: &C4Mcp, name: &str, arguments: serde_json::Value) {
    let reply = mcp.wire(name, arguments);
    assert_eq!(reply.is_error, Some(false), "S5_ACCEPTED {name}: {reply:?}");
}

fn s5_refusal(
    mcp: &C4Mcp,
    name: &str,
    arguments: serde_json::Value,
    expected: &str,
) -> Option<String> {
    let reply = mcp.wire(name, arguments);
    (reply.is_error != Some(true) || !c5_fix10_text(&reply).contains(expected))
        .then(|| format!("{expected}: {reply:?}"))
}

fn s5_editorial(
    f: &Fixture,
    version: Option<kinewright_project::studio_records::SnapshotKey>,
) -> HostReply {
    ok(
        f.editor
            .request(HostCommand::ReadEditorial { version })
            .unwrap(),
        "S5_READ_EDITORIAL",
    )
}

fn s5_propose(f: &Fixture, director: &C4Mcp) -> kinewright_project::editorial::RulebookDecision {
    s5_accepted(
        director,
        "propose_rulebook_revision",
        serde_json::json!({"rule":"Keep the spoken promise."}),
    );
    let (proposal, _) = s5_editorial(f, None).rulebook_proposal.unwrap();
    kinewright_project::editorial::RulebookDecision {
        proposal,
        approve: true,
    }
}

fn s5_brief_rules(f: &Fixture, execution: &ExecutionIdentity) -> serde_json::Value {
    let state = f.projection(&execution.run);
    let task = state
        .tasks
        .iter()
        .find(|task| task.id == execution.task)
        .unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let prompt = host
        .lock()
        .unwrap()
        .codex_prompt(task, b"scripted", &te12_fixed_context())
        .unwrap();
    let prompt: serde_json::Value = serde_json::from_slice(&prompt).unwrap();
    prompt["data"]["rulebook"].clone()
}

#[test]
fn s5_rulebook_changes_only_briefs_prepared_after_person_approval() {
    let mut f = Fixture::new();
    let (_run, parent, director) = c4_director(&f);
    let old_rules = s5_brief_rules(&f, &parent);
    assert_eq!(old_rules, serde_json::json!({"revision":null,"rules":[]}));
    let decision = s5_propose(&f, &director);
    let (before, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    assert_eq!(
        s5_brief_rules(&f, &before),
        old_rules,
        "S5_PENDING_RULE_DOES_NOT_APPLY"
    );
    drop(reviewer);
    let command = HostCommand::DecideRulebook { decision };
    holder(&mut f);
    ok(f.editor.request(command).unwrap(), "S5_PERSON_APPROVED");
    assert!(s5_editorial(&f, None).rulebook_proposal.is_none());
    let (after, _, _) = s5_child(&f, &director, &parent, Role::Reviewer);
    let new_rules = s5_brief_rules(&f, &after);
    assert!(new_rules["revision"].is_object());
    assert_eq!(
        new_rules["rules"],
        serde_json::json!(["Keep the spoken promise."])
    );
    assert_eq!(
        s5_brief_rules(&f, &before),
        old_rules,
        "S5_START_FREEZES_RULES"
    );
    assert_eq!(
        s5_brief_rules(&f, &parent),
        old_rules,
        "S5_DIRECTOR_CANNOT_CHANGE_ITS_RULES"
    );
}

#[test]
fn s5_agent_cannot_approve_its_proposal() {
    let f = Fixture::new();
    let (_run, parent, director) = c4_director(&f);
    let decision = s5_propose(&f, &director);
    let (_, requester, _) = s5_child(&f, &director, &parent, Role::Reviewer);
    let command = HostCommand::DecideRulebook { decision };
    assert_eq!(
        s5_refusal(
            &director,
            "approve_rulebook_revision",
            serde_json::json!({}),
            "tool hidden for durable task role"
        ),
        None
    );
    denied(
        &f,
        &requester,
        command.clone(),
        "editorial_scope",
        "S5_TASK_CANNOT_APPROVE",
    );
    denied(
        &f,
        &f.editor,
        command,
        "editorial_scope",
        "S5_EDITOR_ROLE_ALONE_CANNOT_APPROVE",
    );
}

#[test]
fn s5_scripted_reviewer_audit_survives_reopen_and_collection() {
    use kinewright_project::editorial::AuditDisposition;
    let mut f = Fixture::new();
    let key = s5_version(&f);
    let (_run, parent, director) = c4_director(&f);
    let decision = s5_propose(&f, &director);
    holder(&mut f);
    ok(
        f.editor
            .request(HostCommand::DecideRulebook { decision })
            .unwrap(),
        "S5_PERSON_APPROVED",
    );
    let (editor_attempt, _, editor) = s5_child(&f, &director, &parent, Role::Editor);
    let (review_attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    let revision = s5_brief_rules(&f, &review_attempt)["revision"].clone();
    let mut args = s5_audit_args(&key);
    args["rulebook_ref"] = revision.clone();
    let mut rationale = args.clone();
    rationale["entries"] = serde_json::json!([
        {"line_or_beat":"Opening","output":{"start_micros":0,"end_micros":1000},
         "text":"Move the opening up.","kind":"cut_rationale","finding":"taste"}
    ]);
    let original = ok(
        f.editor
            .request(HostCommand::ReadSnapshot { key: key.clone() })
            .unwrap(),
        "S5_ORIGINAL_VERSION",
    )
    .snapshot
    .unwrap();
    s5_accepted(&editor, "record_editorial_audit", rationale);
    assert!(
        s5_editorial(&f, Some(key.clone()))
            .editorial_audit
            .is_none(),
        "S5_RATIONALE_IS_NOT_COMPLETE"
    );
    s5_accepted(&reviewer, "record_editorial_audit", args);
    let saved = s5_editorial(&f, Some(key.clone())).editorial_audit.unwrap();
    assert_eq!(saved.review.attempt, review_attempt);
    assert_eq!(saved.rationale.as_ref().unwrap().attempt, editor_attempt);
    assert_eq!(saved.snapshot(), &key);
    assert_eq!(
        serde_json::to_value(saved.rulebook_ref()).unwrap(),
        revision
    );
    assert_eq!(
        saved.review.entries[0].disposition,
        AuditDisposition::Blocking
    );
    let path = f.temp.path("project.kinewright");
    let Fixture {
        host, temp: _temp, ..
    } = f;
    drop((reviewer, editor, director, host));
    let pool = kinewright_project::operation_history::HistoryPool::new(
        kinewright_project::operation_history::PoolLimits::default(),
    );
    let open = || {
        ProjectBundle::activate(&path, BundleWriteAdmission::read(&path).unwrap(), &pool).unwrap()
    };
    let mut reopened = open();
    assert_eq!(
        *reopened.editorial_audit(&key).unwrap().unwrap(),
        saved,
        "S5_AUDIT_REOPENS"
    );
    reopened.collect_after_ack().unwrap();
    let cut = reopened.read_snapshot(&key).unwrap().descriptor;
    assert_eq!(cut.identity, original.identity);
    assert_eq!(cut.document_ref, original.document_ref);
    assert_eq!(cut.rulebook_ref.as_ref(), saved.rulebook_ref());
    assert_eq!(*reopened.editorial_audit(&key).unwrap().unwrap(), saved);
}

#[test]
fn s5_reviewer_refuses_invalid_audit_submissions() {
    let mut f = Fixture::new();
    let key = s5_version(&f);
    let other = s5_version(&f);
    let (_run, parent, director) = c4_director(&f);
    let (_, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    let mut changes = Vec::new();
    let mut check = |arguments, expected| {
        if let Some(change) = s5_refusal(&reviewer, "record_editorial_audit", arguments, expected) {
            changes.push(change);
        }
    };
    let args = s5_audit_args(&key);
    let mut wrong_version = args.clone();
    wrong_version["snapshot"] = serde_json::to_value(other).unwrap();
    check(wrong_version, "editorial audit snapshot");
    let decision = s5_propose(&f, &director);
    holder(&mut f);
    ok(
        f.editor
            .request(HostCommand::DecideRulebook { decision })
            .unwrap(),
        "S5_PERSON_APPROVED",
    );
    let host = f.host.owner.upgrade().unwrap();
    let approved = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .frozen_rulebook()
        .unwrap()
        .revision;
    let mut wrong_rulebook = args.clone();
    wrong_rulebook["rulebook_ref"] = serde_json::to_value(approved).unwrap();
    check(wrong_rulebook, "editorial audit frozen rulebook");
    let mut skipped = args.clone();
    skipped["entries"].as_array_mut().unwrap().pop();
    check(skipped, "unanswered editorial checklist item");
    let mut wrong_role = args.clone();
    wrong_role["entries"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "line_or_beat":"Opening","output":{"start_micros":0,"end_micros":1000},
            "text":"Move the opening up.","kind":"cut_rationale","finding":"taste"
        }));
    check(wrong_role, "editorial audit entry role");
    let mut too_many = args.clone();
    too_many["entries"] = serde_json::json!(
        args["entries"]
            .as_array()
            .unwrap()
            .iter()
            .cycle()
            .take(33)
            .cloned()
            .collect::<Vec<_>>()
    );
    check(too_many, "editorial audit entries");
    assert!(changes.is_empty(), "S5_AUDIT_REFUSALS_CHANGED {changes:#?}");
    assert!(s5_editorial(&f, Some(key)).editorial_audit.is_none());
}

#[test]
fn s5_director_refuses_overlong_rule_text() {
    let f = Fixture::new();
    let (_, _, director) = c4_director(&f);
    assert_eq!(
        s5_refusal(
            &director,
            "propose_rulebook_revision",
            serde_json::json!({
                "rule":"x".repeat(kinewright_project::editorial::MAX_RULE_TEXT + 1)
            }),
            "rulebook rule text"
        ),
        None,
        "S5_RULE_TEXT_LIMIT"
    );
    assert!(s5_editorial(&f, None).rulebook_proposal.is_none());
}
