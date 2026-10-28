#[cfg(target_os = "linux")]
#[test]
fn h4_app_close_waits_for_detach_and_keeps_host_run() {
    use kinewright_agent::{
        orchestration::orchestration_records::*,
        writer_host::{HostCommand, HostReply, ScopedOrchestrationRequest, SessionRoots},
    };
    use std::{
        collections::BTreeSet,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn hook(app: &mut Ja1Attachment, command: HostCommand) -> HostReply {
        app.request_orchestration(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.poll_host();
            if let Some(reply) = app.take_orchestration_reply() {
                assert!(reply.error.is_none(), "H4_APP_HOOK: {:?}", reply.error);
                return reply;
            }
            assert!(Instant::now() < deadline, "H4_APP_HOOK_TIMEOUT");
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }
    let temp = temp();
    let path = temp.path("h4-app.kinewright");
    let core = Core::spawn(doc()).unwrap();
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::new(
        temp.root(),
        &core,
        StorageLimits::default(),
    )
    .unwrap();
    owner.first_save(&path, StorageLimits::default()).unwrap();
    let audio = temp.path("retained.wav");
    std::fs::write(
        &audio,
        kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
    )
    .unwrap();
    owner.import_owned_media(&audio).unwrap();
    owner.save_active().unwrap();
    // Host handover requires settled ownership; reopen the durable media so the
    // fixture no longer holds the import transaction's temporary input lease.
    drop(owner);
    let mut owner = kinewright_project::editing_session::Ja1EditingSession::open(&path).unwrap();
    let mut candidate = owner.fork_durable_branch("run-candidate").unwrap();
    owner.persist_durable_branch(&mut candidate).unwrap();
    owner.close_durable_branch(&candidate).unwrap();
    let base = owner.candidate_binding(candidate.branch_id()).unwrap();
    let evidence = owner
        .admit_host_grant(b"App hook fixture plan and scope")
        .unwrap();
    let inputs = owner
        .manual_checkpoint()
        .unwrap()
        .1
        .input_manifest_ref
        .clone();
    let brief = TaskBrief {
        plan: evidence.clone(),
        script: evidence.clone(),
        assignment: evidence,
        inputs,
        role: Role::Director,
        provider: ProviderSelectionRecord {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        },
        scope: TaskScope {
            operations: BTreeSet::new(),
            objects: BTreeSet::new(),
            fields: BTreeSet::new(),
            tracks: BTreeSet::new(),
            bus_sidechain_closure: BTreeSet::new(),
            asset_roots: vec![],
            base,
            consent_refs: vec![],
        },
    };
    owner.save_active().unwrap();
    let mut app = Ja1Attachment::new(owner).unwrap();
    app.launch_host_at(
        &host::executable(),
        "h4-app".into(),
        SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
    )
    .unwrap();
    let mut child = ChildGuard(app.host.as_mut().unwrap().take_child_for_test());
    let issued = hook(&mut app, HostCommand::IssueMetadataIssuer { issuer: None })
        .metadata_issuer
        .unwrap();
    let mut envelope = OrchestrationEnvelope {
        id: issued.envelope.id,
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
        project: issued.envelope.project_id,
        expected_generation: issued.root_generation,
        authority: AuthorityBinding {
            grant_ref: issued.grant_ref,
            owner_epoch: issued.envelope.owner_epoch,
            endpoint: None,
            attempt: None,
        },
        command: OrchestrationCommand::StartRun(Box::new(StartRun { director: brief })),
    };
    envelope.digest = envelope.computed_digest().unwrap();
    let run: RunId = envelope.identity("run").unwrap();
    hook(
        &mut app,
        HostCommand::DispatchOrchestration {
            request: Box::new(ScopedOrchestrationRequest {
                run: None,
                envelope,
                consume_batch: None,
            }),
        },
    );
    let reissuer = app.continuation_issuer().unwrap();
    assert!(!app.close(), "H4_APP_WAIT_FOR_DETACH");
    assert!(
        app.originating_context.load(Ordering::Acquire),
        "H4_APP_RETAIN_SOURCE"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll_host();
        if let Some(reply) = app.take_orchestration_reply() {
            assert!(
                reply.detached.is_some() && reply.error.is_none(),
                "H4_APP_DETACH_ACK: {:?}",
                reply.error
            );
            break;
        }
        assert!(Instant::now() < deadline, "H4_APP_DETACH_TIMEOUT");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    assert!(app.close(), "H4_APP_CONFIRMED_CLOSE");
    assert!(!app.originating_context.load(Ordering::Acquire));
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "H4_APP_HOST_SURVIVES"
    );
    let editor = reissuer.reattach_editor().unwrap();
    let editor = editor
        .admit_destination(&temp.path("reattached-save.kinewright"))
        .expect("H4_APP_GUI_REATTACH");
    let observed = editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap();
    assert!(observed.error.is_none(), "H4_APP_REATTACH");
    let projection = observed.run_view.unwrap().projection;
    assert_eq!(projection.run, run, "H4_APP_SAME_RUN");
    let issued = editor
        .request(HostCommand::IssueMetadataIssuer { issuer: None })
        .unwrap()
        .metadata_issuer
        .unwrap();
    let mut envelope = OrchestrationEnvelope {
        command: OrchestrationCommand::ControlRun(RunIntent {
            command_id: issued.envelope.id.clone(),
            run_id: run.clone(),
            expected_control_revision: projection.control_revision,
            action: RunAction::Stop,
        }),
        id: issued.envelope.id,
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
        project: issued.envelope.project_id,
        expected_generation: issued.root_generation,
        authority: AuthorityBinding {
            grant_ref: issued.grant_ref,
            owner_epoch: issued.envelope.owner_epoch,
            endpoint: None,
            attempt: None,
        },
    };
    envelope.digest = envelope.computed_digest().unwrap();
    let reply = editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(ScopedOrchestrationRequest {
                run: Some(run),
                envelope,
                consume_batch: None,
            }),
        })
        .unwrap();
    assert!(reply.error.is_none(), "H4_APP_STOP: {:?}", reply.error);
    assert!(
        editor
            .request(HostCommand::Shutdown)
            .unwrap()
            .error
            .is_none(),
        "H4_APP_SHUTDOWN_AFTER_STOP"
    );
    child.0.wait().unwrap();
}
