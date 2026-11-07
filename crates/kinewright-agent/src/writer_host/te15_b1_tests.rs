struct Te15Observe(std::sync::Mutex<Option<kinewright_core::SessionConfig>>);
impl crate::orchestration::adapters::ConnectorRuntimeChecks for Te15Observe {
    fn preflight(
        &self,
        _: &crate::orchestration::adapters::ConnectorSpecification,
    ) -> Result<(), crate::orchestration::adapters::CertificationRefusal> {
        Ok(())
    }
    fn probe(
        &self,
        spec: &crate::orchestration::adapters::ConnectorSpecification,
        binding: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<
        crate::orchestration::adapters::RuntimeQualification,
        crate::orchestration::adapters::CertificationRefusal,
    > {
        *self.0.lock().unwrap() = Some(spec.session_config(binding));
        Err(crate::orchestration::adapters::CertificationRefusal::SandboxUnavailable)
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end assignment keeps its model through replay and reconciliation"
)]
fn te15_b1_assignment_snapshots_person_choice_and_replays_original_after_change() {
    use super::super::{SpecialistChoice, specialist_choices, write_specialist_setting};
    let f = Fixture::new();
    let (run, parent, director) = c4_director(&f);
    let folder = f.temp.path("scheduler");
    let choices = specialist_choices();
    let first_choice = choices
        .iter()
        .find(|s| s.connector == "claude")
        .unwrap()
        .clone();
    write_specialist_setting(
        Some(&folder),
        &SpecialistChoice::Specific(first_choice.clone()),
    )
    .unwrap();
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    brief.provider.model = "model-request-must-not-override-person".into();
    let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: parent.task.clone(),
        brief: brief.clone(),
    }));
    let control = director.control(command.clone());
    let first = director.wire("assign_task", control.clone());
    assert_eq!(first.is_error, Some(false), "TE15_ASSIGN_FIRST {first:?}");
    let state = f.projection(&run);
    let child = state.tasks[1].id.clone();
    assert_eq!(
        state.tasks[1].brief.provider, first_choice,
        "TE15_PERSON_CHOICE"
    );
    let (attempt, _client, _endpoint, _core) = c4_reserve_task(&f, &run, &child);
    let admitted = native_admitted(&f, &attempt);
    let next_choice = choices
        .iter()
        .find(|s| s.connector == "codex")
        .unwrap()
        .clone();
    write_specialist_setting(
        Some(&folder),
        &SpecialistChoice::Specific(next_choice.clone()),
    )
    .unwrap();
    let observed = std::sync::Arc::new(Te15Observe(std::sync::Mutex::new(None)));
    assert!(
        f.host
            .runtime
            .bind_profile_for_test(&admitted, director.server.endpoint(), observed.clone())
            .is_err()
    );
    let observed = observed.0.lock().unwrap().clone().unwrap();
    assert_eq!(
        observed.model.as_deref(),
        Some(first_choice.model.as_str()),
        "TE15_FACTORY_MODEL"
    );
    assert_eq!(
        observed.tool_names.unwrap(),
        task_mcp::manifest::EDITOR,
        "TE15_FACTORY_ROLE"
    );
    let replay = director.wire("assign_task", control.clone());
    assert_eq!(
        replay.is_error,
        Some(false),
        "TE15_ASSIGN_REPLAY {replay:?}"
    );
    assert_eq!(
        first.structured_content.as_ref().unwrap()["outcome"],
        replay.structured_content.as_ref().unwrap()["outcome"],
        "TE15_ASSIGN_IMMUTABLE"
    );
    assert_eq!(
        f.projection(&run).tasks[1].brief.provider,
        first_choice,
        "TE15_RUNNING_SELECTION"
    );
    let query = super::super::receipt_reconciliation::ReceiptQuery {
        project: parent.project.clone(),
        run: run.clone(),
        task: parent.task.clone(),
        attempt: parent.attempt.clone(),
        id: serde_json::from_value(control["id"].clone()).unwrap(),
        digest: None,
        command: Some(serde_json::from_value(control["command"].clone()).unwrap()),
    };
    let recovered = f
        .editor
        .request(HostCommand::ReconcileTaskReceipt { query })
        .unwrap();
    assert!(
        recovered.error.is_none(),
        "TE15_ASSIGN_RECONCILE {recovered:?}"
    );
    let Some(super::super::receipt_reconciliation::ReceiptReply::Stored { outcome, .. }) =
        recovered.task_receipt
    else {
        panic!("TE15_ASSIGN_RECEIPT");
    };
    assert_eq!(
        outcome,
        first.structured_content.as_ref().unwrap()["outcome"],
        "TE15_ASSIGN_RECONCILE_OUTCOME"
    );
    let OrchestrationCommand::AssignTask(mut next) = command else {
        unreachable!();
    };
    next.brief.role = Role::Reviewer;
    director.accepted("assign_task", OrchestrationCommand::AssignTask(next));
    assert_eq!(
        f.projection(&run).tasks[2].brief.provider,
        next_choice,
        "TE15_NEW_SELECTION"
    );
    assert_eq!(
        f.projection(&run).tasks[0].brief.provider,
        f.brief.provider,
        "TE15_DIRECTOR_UNCHANGED"
    );
}

#[test]
fn te15_b1_assignment_default_ignores_model_choice_and_unavailable_refuses() {
    let f = Fixture::new();
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    brief.provider.model = "not-person-selected".into();
    let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: parent.task.clone(),
        brief,
    }));
    director.accepted("assign_task", command.clone());
    assert_eq!(
        f.projection(&run).tasks[1].brief.provider,
        f.brief.provider,
        "TE15_DEFAULT_PARENT"
    );
    let folder = f.temp.path("scheduler");
    std::fs::create_dir_all(&folder).unwrap();
    let bytes = br#"{"version":1,"specialists":{"kind":"specific","selection":{"connector":"retired","model":"retired","options":[]}}}"#;
    std::fs::write(folder.join("specialists.json"), bytes).unwrap();
    let refused = director.wire("assign_task", director.control(command));
    assert_eq!(
        refused.is_error,
        Some(true),
        "TE15_ASSIGN_UNAVAILABLE {refused:?}"
    );
    assert_eq!(f.projection(&run).tasks.len(), 2);
    assert_eq!(
        std::fs::read(folder.join("specialists.json")).unwrap(),
        bytes
    );
}

#[test]
fn te15_b1_legacy_assignment_receipt_digest_is_exact_and_read_only() {
    use super::super::receipt_reconciliation::{ReceiptQuery, ReceiptReply};
    use kinewright_project::receipts::CommandDigest;
    let f = Fixture::new();
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: parent.task.clone(),
        brief,
    }));
    let control = director.control(command.clone());
    let first = director.wire("assign_task", control.clone());
    assert_eq!(first.is_error, Some(false));
    // The pre-B1 algorithm: the complete canonical typed assignment, with provider.
    let mut original_bytes = b"kinewright.task-command.v1\0".to_vec();
    original_bytes.extend(serde_json::to_vec(&serde_json::to_value(&command).unwrap()).unwrap());
    let legacy: CommandDigest = serde_json::from_value(serde_json::json!(
        kinewright_media::sha256_bytes(&original_bytes)
    ))
    .unwrap();
    assert_ne!(
        legacy,
        kinewright_project::orchestration_records::task_command_digest(&command).unwrap()
    );
    let mut query = ReceiptQuery {
        project: parent.project.clone(),
        run: run.clone(),
        task: parent.task.clone(),
        attempt: parent.attempt.clone(),
        id: serde_json::from_value(control["id"].clone()).unwrap(),
        digest: Some(legacy),
        command: None,
    };
    let before = f.projection(&run);
    for typed in [None, Some(command.clone())] {
        query.command = typed;
        let recovered = f
            .editor
            .request(HostCommand::ReconcileTaskReceipt {
                query: query.clone(),
            })
            .unwrap();
        assert!(
            recovered.error.is_none(),
            "TE15_LEGACY_RECEIPT {recovered:?}"
        );
        let Some(ReceiptReply::Stored { outcome, .. }) = recovered.task_receipt else {
            panic!("TE15_LEGACY_STORED");
        };
        assert_eq!(
            outcome,
            first.structured_content.as_ref().unwrap()["outcome"]
        );
    }
    let OrchestrationCommand::AssignTask(mut changed) = command else {
        unreachable!();
    };
    changed.brief.role = Role::Reviewer;
    query.command = Some(OrchestrationCommand::AssignTask(changed));
    let refused = f
        .editor
        .request(HostCommand::ReconcileTaskReceipt { query })
        .unwrap();
    assert!(
        refused.error.is_some(),
        "TE15_LEGACY_CHANGED_COMMAND {refused:?}"
    );
    assert_eq!(f.projection(&run), before, "TE15_LEGACY_READ_ONLY");
}
