fn history_marker(id: u64) -> kinewright_core::Operation {
    kinewright_core::Operation::AddMarker {
        marker: kinewright_core::Marker {
            id: kinewright_core::MarkerId(id),
            position: kinewright_core::TimeCode(i64::try_from(id).unwrap()),
            label: format!("marker {id}"),
            color_token: 0,
        },
    }
}
fn history_task_endpoint(
    f: &Fixture,
    run: &RunId,
) -> (
    ExecutionIdentity,
    HostClient,
    crate::orchestration::tasks::TaskEndpoint,
    Core,
) {
    let producer = f.producer(run);
    let host = f.host.owner.upgrade().unwrap();
    let (execution, auth, endpoint, core) = {
        let mut owner = host.lock().unwrap();
        let (admitted, _, _) = owner
            .reserve_native_effect(&producer, run)
            .unwrap()
            .expect("C3_HISTORY_RESERVATION");
        let execution = admitted.identity().clone();
        let (auth, endpoint) = owner
            .prepare_task_endpoint(
                &producer,
                run,
                &execution.task,
                &execution.attempt,
                Duration::from_secs(300),
            )
            .unwrap_or_else(|e| panic!("C3_EXACT_TASK_CONTINUATION: {e}"));
        let core = owner.hooks.runs[run].tasks[&execution.task].core().clone();
        (execution, auth, endpoint, core)
    };
    let client = HostClient::connect(f.host.endpoint.clone(), auth, f.host.roots.clone()).unwrap();
    (execution, client, endpoint, core)
}
fn history_turn(
    f: &Fixture,
    execution: &ExecutionIdentity,
    actor: &HostClient,
    endpoint: &crate::orchestration::tasks::TaskEndpoint,
    n: u64,
    begin: bool,
) {
    let turn = TaskTurn {
        task: execution.task.clone(),
        attempt: execution.attempt.clone(),
        session: execution.nonce.clone(),
        ordinal: n,
        owner_epoch: endpoint.authority().owner_epoch,
        expected_head: f.brief.scope.base.head.clone(),
        purpose: TurnPurpose::Work,
    };
    let request = task_request(
        f,
        actor,
        endpoint,
        if begin {
            OrchestrationCommand::BeginTurn(turn)
        } else {
            OrchestrationCommand::EndTurn(turn)
        },
    );
    let reply = ok(
        actor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "C3_TURN_CUT",
    );
    assert_eq!(
        reply.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
}
fn restart_history_fixture(f: Fixture) -> Fixture {
    let Fixture {
        host,
        temp,
        brief,
        issuer_auth,
        ..
    } = f;
    drop(host);
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let host = WriterHost::start_with_issuer(
        HostConfig {
            scheduler_settings: None,
            project: "hooks".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
    )
    .unwrap();
    let editor = host.client();
    let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
    Fixture {
        host,
        editor,
        issuer,
        issuer_auth,
        brief,
        temp,
    }
}
#[test]
fn s4b_c3b_exact_task_history_continues_restarts_merges_and_stops() {
    use kinewright_core::Command;
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    if let kinewright_core::Operation::AddMarker { marker } = history_marker(1) {
        scoped.markers.push(marker);
    }
    if let kinewright_core::Operation::AddMarker { marker } = history_marker(2) {
        scoped.markers.push(marker);
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        owner
            .core()
            .request(Command::Do(history_marker(50)))
            .unwrap();
        owner
            .core()
            .request(Command::Do(history_marker(51)))
            .unwrap();
        owner.core().request(Command::Undo).unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
        owner.core().export_history().unwrap()
    };
    assert!(!manual.cursor.redo.is_empty());
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("history-machine"),
        start.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let (first, first_client, first_endpoint, core) = history_task_endpoint(&f, &run);
    history_turn(&f, &first, &first_client, &first_endpoint, 1, true);
    for operation in [history_marker(1), history_marker(2)] {
        let _gate = first_endpoint.admit().unwrap();
        first_endpoint
            .validate_batch(
                &core.export_history().unwrap().document,
                std::slice::from_ref(&operation),
            )
            .unwrap();
        core.request(Command::Do(operation)).unwrap();
    }
    core.request(Command::Undo).unwrap();
    let expected = core.export_history().unwrap();
    assert!(!expected.cursor.redo.is_empty());
    let saved = {
        let producer = f.producer(&run);
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .checkpoint_task_history(&producer, &run, &first.task)
            .unwrap()
    };
    history_turn(&f, &first, &first_client, &first_endpoint, 1, false);
    let (claim, _) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    assert!(
        first_endpoint.check_live().is_err(),
        "C3_RETIRED_TASK_AUTHORITY"
    );
    let (second, _, second_endpoint, second_core) = history_task_endpoint(&f, &run);
    assert_ne!(first.attempt, second.attempt);
    assert_eq!(
        second_core.export_history().unwrap(),
        expected,
        "C3_EXACT_TASK_CONTINUATION"
    );
    let second_state = f.projection(&run);
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .saved_task_history(&second_state, &second.task)
            .unwrap()
            .unwrap()
            .source,
        saved.source,
        "C3_EXACT_TASK_BRANCH_CHECKPOINT"
    );
    drop(core);
    drop(second_core);
    drop(claim);
    let f = restart_history_fixture(f);
    assert!(second_endpoint.check_live().is_err());
    let claim =
        crate::execution_claim::MachineClaim::recover_fixture(&f.temp.path("history-machine"))
            .unwrap();
    let (claim, producer) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    let (third, _, _, third_core) = history_task_endpoint(&f, &run);
    assert_ne!(second.attempt, third.attempt);
    assert_eq!(
        third_core.export_history().unwrap(),
        expected,
        "C3_RESTART_EXACT_REDO"
    );
    let (id, receipt, merged) = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let issuer = owner
            .owner_mut()
            .unwrap()
            .issue_manual_lease("history-merge".into(), "private".into())
            .unwrap()
            .issuer;
        let id = CommandId {
            issuer,
            sequence: 1,
        };
        let receipt = owner
            .merge_task_history(&producer, &run, &third.task, id.clone())
            .unwrap();
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        let merged = owner
            .saved_task_history(&state, &third.task)
            .unwrap()
            .unwrap();
        assert_eq!(
            owner
                .merge_task_history(&producer, &run, &third.task, id.clone())
                .unwrap(),
            receipt,
            "C3_MERGE_ORIGINAL_RECEIPT"
        );
        assert_eq!(
            owner.core().export_history().unwrap(),
            manual,
            "C3_MANUAL_REDO_UNCHANGED"
        );
        (id, receipt, merged)
    };
    assert!(matches!(
        receipt,
        kinewright_project::receipts::ReceiptLookup::Committed(_)
    ));
    assert!(merged.frozen && merged.merge_envelope.as_ref().is_some_and(|e| e.id == id));
    assert_eq!(third_core.export_history().unwrap(), expected);
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut runtime = owner
            .hooks
            .runs
            .get_mut(&run)
            .unwrap()
            .tasks
            .remove(&third.task)
            .unwrap();
        runtime.close(owner.owner_mut().unwrap()).unwrap();
        owner
            .hooks
            .runs
            .get_mut(&run)
            .unwrap()
            .tasks
            .insert(third.task.clone(), runtime);
    }

    let (claim, _) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    let (_, _, fourth_endpoint, fourth_core) = history_task_endpoint(&f, &run);
    assert!(
        fourth_endpoint.admit().is_err(),
        "C3_FROZEN_ATTEMPT_STAYS_CLOSED"
    );
    assert_eq!(fourth_core.export_history().unwrap(), expected);
    drop(third_core);
    drop(fourth_core);
    drop(claim);
    let f = restart_history_fixture(f);
    let claim =
        crate::execution_claim::MachineClaim::recover_fixture(&f.temp.path("history-machine"))
            .unwrap();
    let (claim, producer) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    let (fifth, _, fifth_endpoint, fifth_core) = history_task_endpoint(&f, &run);
    assert!(
        fifth_endpoint.admit().is_err(),
        "C3_RESTART_RETAINS_FROZEN_FENCE"
    );
    let state = f.projection(&run);
    let final_expected = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let restored = owner
            .saved_task_history(&state, &fifth.task)
            .unwrap()
            .unwrap();
        let mut expected_record = merged.clone();
        expected_record.attempt = fifth.attempt.clone();
        assert_eq!(
            restored, expected_record,
            "C3_RESTART_CHANGES_ONLY_ATTEMPT_AUTHORITY"
        );
        assert_eq!(
            owner
                .merge_task_history(&producer, &run, &fifth.task, id)
                .unwrap(),
            receipt,
            "C3_RESTART_MERGE_REPLAY_ONCE"
        );
        restored
    };
    stop_native_run(&f, &run);
    let (claim, _) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    claim.finish().unwrap();
    drop(fifth_core);
    let Fixture { host, temp, .. } = f;
    drop(host);
    let mut owner = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    assert_eq!(
        owner.core().export_history().unwrap(),
        manual,
        "C3_STOP_REOPEN_MANUAL_REDO"
    );
    let state = owner.orchestration_projection(&run).unwrap().unwrap();
    let final_record = state.tasks[0]
        .progress
        .decisions
        .iter()
        .find_map(|r| {
            if r.record_kind != kinewright_project::bundle_records::RecordKind::RunIndex {
                return None;
            }
            match &owner
                .store()
                .unwrap()
                .read_record::<OrchestrationRecord>(r)
                .unwrap()
                .payload
            {
                OrchestrationPayload::TaskHistory(h) => Some((**h).clone()),
                _ => None,
            }
        })
        .unwrap();
    assert_eq!(
        final_record, final_expected,
        "C3_FROZEN_AND_MERGE_RECEIPTS_REOPEN"
    );
    assert_eq!(
        owner
            .manual_transition_receipt(final_record.merge_envelope.as_ref().unwrap())
            .unwrap(),
        receipt
    );
    let task = owner
        .reopen_registered_candidate(final_record.source.branch, &final_record.base_ref)
        .unwrap();
    assert_eq!(
        task.core().export_history().unwrap(),
        expected,
        "C3_STOP_REOPEN_EXACT_TASK_REDO"
    );
}
#[test]
fn s4b_c3b_retired_archive_admits_more_than_three_attempts() {
    let f = Fixture::new();
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let producer = f.producer(&run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("archive-machine"),
        start.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut expected = Vec::new();
    for _ in 0..8 {
        let next = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .reserve_native_effect(&producer, &run);
        assert!(next.is_ok(), "C3_ARCHIVE_CAP_FOURTH_RESERVATION: {next:?}");
        let (admitted, _, _) = next.unwrap().expect("C3_ARCHIVE_CAP_FOURTH_RESERVATION");
        expected.push(admitted.identity().clone());
        (claim, _) =
            crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    }
    let state = f.projection(&run);
    assert_eq!(state.attempts.len(), 1);
    let record = state
        .usage
        .iter()
        .find_map(|r| {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            match &owner
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_record::<OrchestrationRecord>(r)
                .unwrap()
                .payload
            {
                OrchestrationPayload::AttemptArchive(h) => Some((**h).clone()),
                _ => None,
            }
        })
        .unwrap();
    let actual = record
        .attempts
        .iter()
        .chain(&state.attempts)
        .map(|a| a.identity.clone())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "C3_ARCHIVE_RETAINS_ORIGINAL_IDENTITIES");
    claim.finish().unwrap();
}
