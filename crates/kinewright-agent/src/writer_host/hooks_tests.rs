#![cfg(target_os = "linux")]
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::orchestration::{
        orchestration_commit::{CommandDecision, OrchestrationReceipt, ValidatedRunUpdate},
        orchestration_records::*,
    };
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        receipts::{CommandId, ReceiptResult},
        studio_records::{
            FeedbackBatch, FeedbackBatchId, MetadataChange, MetadataResult, MetadataUpdate,
            PublishSnapshot, ReviseRequest, VersionId,
        },
    };
    use std::{collections::BTreeSet, sync::atomic::AtomicUsize, time::Instant};

    include!("hooks_attention_tests.rs");
    include!("hooks_store_tests.rs");
    include!("hooks_grant_tests.rs");
    struct Fixture {
        host: WriterHost,
        editor: HostClient,
        issuer: HostIssuer,
        issuer_auth: HostAuth,
        brief: TaskBrief,
        temp: kinewright_media::test_support::TempDirectory,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_media(false)
        }
        fn with_media(media: bool) -> Self {
            Self::with_document(media, Document::default())
        }
        fn with_document(media: bool, document: Document) -> Self {
            let temp = kinewright_media::test_support::TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "s3-hooks",
            );
            let path = temp.path("project.kinewright");
            let core = Core::spawn(document).unwrap();
            let mut bundle =
                ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
            let mut inputs = bundle.empty_inputs().unwrap().reference.clone();
            bundle.save(&core, &inputs).unwrap();
            drop(bundle);
            if media {
                let source = temp.path("retained.wav");
                std::fs::write(
                    &source,
                    kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
                )
                .unwrap();
                let mut owner = Ja1EditingSession::open(&path).unwrap();
                owner.import_owned_media(&source).unwrap();
                owner.save_active().unwrap();
                inputs = owner.current_inputs_ref().clone();
            }
            let roots = SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            };
            let issuer_auth = HostAuth::generate().unwrap();
            let host = WriterHost::start_with_issuer(
                HostConfig {
                    project: "hooks".into(),
                    path: Some(path),
                    roots: roots.clone(),
                    origin: HostOrigin::Headless,
                },
                HostAuth::generate().unwrap(),
                Some(issuer_auth.clone()),
            )
            .unwrap();
            let editor = host.client();
            let issuer =
                HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
            let brief = {
                let owner = host.owner.upgrade().unwrap();
                let mut host = owner.lock().unwrap();
                let owner = host.owner_mut().unwrap();
                let mut candidate = owner.fork_durable_branch("candidate").unwrap();
                owner.persist_durable_branch(&mut candidate).unwrap();
                let base = owner.candidate_binding(candidate.branch_id()).unwrap();
                let evidence = owner
                    .admit_host_grant(b"hook plan, script and assignment")
                    .unwrap();
                TaskBrief {
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
                }
            };
            Self {
                host,
                editor,
                issuer,
                issuer_auth,
                brief,
                temp,
            }
        }
        fn actor(&self, role: AttachmentRole, subject: &str) -> HostClient {
            self.host
                .attach(subject.into(), role, self.host.roots.clone())
                .unwrap()
        }
        fn issue(actor: &HostClient) -> MetadataIssuer {
            ok(
                actor
                    .request(HostCommand::IssueMetadataIssuer { issuer: None })
                    .unwrap(),
                "H3_ISSUER_CONTROL",
            )
            .metadata_issuer
            .unwrap()
        }
        fn request(
            &self,
            actor: &HostClient,
            run: Option<RunId>,
            command: OrchestrationCommand,
        ) -> ScopedOrchestrationRequest {
            assert_eq!(actor.endpoint().project, self.host.endpoint.project);
            let issued = Self::issue(actor);
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
                command,
            };
            if let OrchestrationCommand::ControlRun(intent) = &mut envelope.command {
                intent.command_id = envelope.id.clone();
            }
            envelope.digest = envelope.computed_digest().unwrap();
            ScopedOrchestrationRequest {
                run,
                envelope,
                consume_batch: None,
            }
        }
        fn start_request(&self, actor: &HostClient) -> ScopedOrchestrationRequest {
            self.request(
                actor,
                None,
                OrchestrationCommand::StartRun(Box::new(StartRun {
                    director: self.brief.clone(),
                })),
            )
        }
        fn start(&self) -> OrchestrationReceipt {
            let request = self.start_request(&self.editor);
            ok(
                self.editor
                    .request(HostCommand::DispatchOrchestration {
                        request: Box::new(request),
                    })
                    .unwrap(),
                "H1_DURABLE_CUT",
            )
            .orchestration
            .unwrap()
        }
        fn projection(&self, run: &RunId) -> RunProjection {
            self.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .orchestration_projection(run)
                .unwrap()
                .expect("H1_DURABLE_CUT")
        }
        fn control(
            &self,
            actor: &HostClient,
            run: &RunId,
            action: RunAction,
        ) -> ScopedOrchestrationRequest {
            let projection = self.projection(run);
            self.request(
                actor,
                Some(run.clone()),
                OrchestrationCommand::ControlRun(RunIntent {
                    command_id: CommandId {
                        issuer: serde_json::from_value(serde_json::json!("0".repeat(32))).unwrap(),
                        sequence: 1,
                    },
                    run_id: run.clone(),
                    expected_control_revision: projection.control_revision,
                    action,
                }),
            )
        }
        fn producer(&self, run: &RunId) -> HostAuth {
            self.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .run_producer(run)
                .unwrap()
        }
        fn producer_client(&self, run: &RunId) -> HostClient {
            HostClient::connect(
                self.host.endpoint.clone(),
                self.producer(run),
                self.host.roots.clone(),
            )
            .unwrap()
        }
    }
    fn ok(reply: HostReply, marker: &str) -> HostReply {
        assert!(reply.error.is_none(), "{marker}: {:?}", reply.error);
        reply
    }
    fn call(f: &Fixture, actor: &HostClient, command: HostCommand, route: u8) -> HostReply {
        match route {
            0 => {
                let request = HostRequest {
                    editor: None,
                    operation: None,
                    protocol: HOST_PROTOCOL,
                    project: actor.endpoint().project.clone(),
                    roots: actor.bootstrap.roots.clone(),
                    command,
                };
                f.host
                    .owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .execute_attached(&actor.bootstrap.auth, request)
            }
            1 => actor.request(command).unwrap(),
            _ => {
                let value = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host_command","arguments":{"command":command}}});
                let bytes = actor
                    .post("mcp", &serde_json::to_vec(&value).unwrap())
                    .unwrap();
                let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                serde_json::from_value(reply["result"]["structuredContent"].clone()).unwrap()
            }
        }
    }
    fn state(f: &Fixture) -> serde_json::Value {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let usage = host.owner().unwrap().store().unwrap().usage().unwrap();
        serde_json::json!({"root": host.owner().unwrap().root_sha256().unwrap(), "history": host.core().export_history().unwrap(), "manual": host.manual,
        "foreground": host.foreground_owner, "sessions": host.sessions.len(), "attachments": host.attachments.iter().map(|a| (a.id, &a.subject, a.role, a.read_pins.len())).collect::<Vec<_>>(),
        "runs": host.hooks.runs.keys().collect::<Vec<_>>(), "pending": host.hooks.pending,
        "ledger": [usage.physical_bytes, usage.owned_bytes, usage.metadata_bytes, usage.copy_bytes, usage.reserved_bytes, usage.reserved_copy_bytes, usage.decoded_bytes, usage.decoder_bytes, usage.live_history_bytes, usage.objects, usage.edges, usage.unclassified_bytes], "damaged": usage.damaged_accounting, "unsettled": usage.unsettled})
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Each ingress receives the same owned request"
    )]
    fn denied(f: &Fixture, actor: &HostClient, command: HostCommand, code: &str, marker: &str) {
        // Shutdown is intentionally absent from the MCP connector surface.
        let routes = if matches!(&command, HostCommand::Shutdown) {
            2
        } else {
            3
        };
        for route in 0..routes {
            let before = state(f);
            let (reply, trace) =
                kinewright_project::io_observer::observe(|| call(f, actor, command.clone(), route));
            assert_eq!(
                reply.error.as_ref().map(|e| e.code.as_str()),
                Some(code),
                "{marker}: route {route}"
            );
            assert_eq!(state(f), before, "{marker}: refused side effects");
            assert!(
                trace.events.iter().all(|event| matches!(
                    event.operation,
                    "publication_reopen_fit"
                        | "rescue_permission_read"
                        | "studio_dirty_history_hash"
                        | "studio_save_disclosure_select"
                )) && !trace.overflowed,
                "{marker}: refused IO: {:?}",
                trace.events
            );
        }
    }

    #[test]
    fn h1_registered_cut_ack_and_reopen() {
        let f = Fixture::new();
        let request = f.start_request(&f.editor);
        let run: RunId = request.envelope.identity("run").unwrap();
        let first = ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
                0,
            ),
            "H1_DURABLE_CUT",
        )
        .orchestration
        .unwrap();
        let projection = f.projection(&run);
        assert_eq!(projection.sequence, first.sequence, "H1_DURABLE_CUT");
        let root = state(&f)["root"].clone();
        for route in 0..3 {
            let repeated = ok(
                call(
                    &f,
                    &f.editor,
                    HostCommand::DispatchOrchestration {
                        request: Box::new(request.clone()),
                    },
                    route,
                ),
                "H1_ORIGINAL_REPLAY",
            )
            .orchestration
            .unwrap();
            assert_eq!(first, repeated, "H1_ORIGINAL_REPLAY");
            assert_eq!(state(&f)["root"], root, "H1_ORIGINAL_REPLAY");
        }
        let Fixture { temp, host, .. } = f;
        let path = temp.path("project.kinewright");
        drop(host);
        let owner = Ja1EditingSession::open(&path).unwrap();
        assert_eq!(
            owner.orchestration_projection(&run).unwrap(),
            Some(projection),
            "H1_REOPEN_ORIGINAL"
        );
        assert_eq!(
            owner.orchestration_replay(&request.envelope, None).unwrap(),
            Some(first),
            "H1_REOPEN_ORIGINAL"
        );
    }

    #[test]
    fn h3_viewer_start_refused() {
        let f = Fixture::new();
        let viewer = f.actor(AttachmentRole::Viewer, "viewer");
        let request = f.start_request(&viewer);
        denied(
            &f,
            &viewer,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "role_refused",
            "H3_VIEWER_START",
        );
        f.start();
    }
    #[test]
    fn h3_viewer_control_refused() {
        let f = Fixture::new();
        let started = f.start();
        let run = started.envelope.identity::<RunId>("run").unwrap();
        let viewer = f.actor(AttachmentRole::Viewer, "viewer");
        let request = f.control(&viewer, &run, RunAction::Pause);
        denied(
            &f,
            &viewer,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "role_refused",
            "H3_VIEWER_CONTROL",
        );
        let request = f.control(&f.editor, &run, RunAction::Pause);
        ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                },
                1,
            ),
            "H3_EDITOR_CONTROL",
        );
    }
    #[test]
    fn h3_metadata_subject_refused_before_publish_and_replay() {
        let f = Fixture::new();
        let first = f.actor(AttachmentRole::Viewer, "first");
        let second = f.actor(AttachmentRole::Viewer, "second");
        let issued = Fixture::issue(&first);
        denied(
            &f,
            &second,
            HostCommand::IssueMetadataIssuer {
                issuer: Some(issued.envelope.id.issuer),
            },
            "metadata_issuer",
            "H3_METADATA_SUBJECT",
        );
        let request = f.start_request(&f.editor);
        denied(
            &f,
            &second,
            HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            },
            "metadata_issuer",
            "H3_METADATA_REPLAY_SUBJECT",
        );
        ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
                0,
            ),
            "H3_EDITOR_START",
        );
        denied(
            &f,
            &second,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "metadata_issuer",
            "H3_METADATA_REPLAY_SUBJECT",
        );
    }
    fn identity<T: serde::de::DeserializeOwned>() -> T {
        serde_json::from_value(serde_json::json!(
            kinewright_project::bundle_records::SessionId::fresh()
                .unwrap()
                .as_str()
        ))
        .unwrap()
    }
    fn publish_projection(
        f: &Fixture,
        actor: &HostClient,
        run: &RunId,
        mut projection: RunProjection,
        reservation: Option<&ExecutionIdentity>,
    ) -> OrchestrationReceipt {
        let request = f.request(
            actor,
            Some(run.clone()),
            OrchestrationCommand::RecordContext {
                task: projection.tasks[0].id.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        );
        projection.sequence += 1;
        projection.control_revision = request.envelope.identity("control-revision").unwrap();
        let owner = f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let segment = EventSegment {
            run: run.clone(),
            first_sequence: projection.sequence,
            previous: Some(
                host.owner()
                    .unwrap()
                    .orchestration_cut(run)
                    .unwrap()
                    .unwrap()
                    .segment,
            ),
            events: vec![OrchestrationEvent {
                command: request.envelope.id.clone(),
                sequence: projection.sequence,
                projection: projection.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &request.envelope.project,
            request.envelope.expected_generation,
            &request.envelope,
            projection,
            segment,
            CommandDecision::Applied,
        )
        .unwrap();
        let (receipt, admitted) = host
            .publish_orchestration_transition(&actor.bootstrap.auth, &update, reservation)
            .unwrap();
        assert_eq!(
            admitted
                .as_ref()
                .map(crate::orchestration::AdmittedAttempt::identity),
            reservation,
            "H1_ACKNOWLEDGED_RESERVATION"
        );
        receipt
    }
    fn reserve(f: &Fixture) -> (RunId, ExecutionIdentity, HostClient) {
        let started = f.start();
        let run: RunId = started.envelope.identity("run").unwrap();
        let actor = f.producer_client(&run);
        let mut projection = f.projection(&run);
        let epoch = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .owner_epoch();
        let identity = ExecutionIdentity {
            version: 1,
            project: started.envelope.project,
            run: run.clone(),
            task: projection.tasks[0].id.clone(),
            attempt: identity(),
            nonce: identity(),
            owner_instance: identity(),
        };
        projection.phase = RunPhase::Running;
        projection.tasks[0].state = TaskState::Preparing;
        projection.tasks[0].progress.capacity =
            DependencyReservation::Backed(identity.attempt.clone());
        projection.attempts.push(AttemptReservation {
            identity: identity.clone(),
            owner_epoch: epoch,
            state: AttemptState::Reserved,
            recovery_ref: f.brief.assignment.clone(),
        });
        projection.cleanup = RunCleanup::Pending(vec![CleanupIdentity {
            attempt: identity.attempt.clone(),
            nonce: identity.nonce.clone(),
            owner_instance: identity.owner_instance.clone(),
            recovery_ref: f.brief.assignment.clone(),
        }]);
        publish_projection(f, &actor, &run, projection, Some(&identity));
        (run, identity, actor)
    }
    fn task_actor(
        f: &Fixture,
        run: &RunId,
        execution: &ExecutionIdentity,
        producer: &HostClient,
    ) -> (HostClient, crate::orchestration::tasks::TaskEndpoint) {
        let (auth, endpoint) = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .prepare_task_endpoint(
                &producer.bootstrap.auth,
                run,
                &execution.task,
                &execution.attempt,
                Duration::from_secs(300),
            )
            .unwrap();
        (
            HostClient::connect(f.host.endpoint.clone(), auth, f.host.roots.clone()).unwrap(),
            endpoint,
        )
    }
    fn task_request(
        f: &Fixture,
        actor: &HostClient,
        endpoint: &crate::orchestration::tasks::TaskEndpoint,
        command: OrchestrationCommand,
    ) -> ScopedOrchestrationRequest {
        let mut request = f.request(actor, Some(endpoint.authority().run.clone()), command);
        request.envelope.authority.attempt = Some(endpoint.authority().attempt.clone());
        request.envelope.authority.endpoint = Some(endpoint.authority().endpoint.clone());
        request.envelope.digest = request.envelope.computed_digest().unwrap();
        request
    }

    #[test]
    fn h3_producer_scope_refused_and_live_control() {
        let f = Fixture::new();
        let (run, execution, producer) = reserve(&f);
        let (actor, endpoint) = task_actor(&f, &run, &execution, &producer);
        let mut request = task_request(
            &f,
            &actor,
            &endpoint,
            OrchestrationCommand::RecordContext {
                task: execution.task.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        );
        request.envelope.authority.endpoint = Some(identity());
        request.envelope.digest = request.envelope.computed_digest().unwrap();
        denied(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "orchestration_refused",
            "H3_PRODUCER_SCOPE",
        );
        let request = task_request(
            &f,
            &actor,
            &endpoint,
            OrchestrationCommand::RecordContext {
                task: execution.task.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        );
        let original = ok(
            call(
                &f,
                &actor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
                2,
            ),
            "H3_PRODUCER_CONTROL",
        )
        .orchestration
        .unwrap();
        let replay = ok(
            call(
                &f,
                &actor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                },
                1,
            ),
            "H3_PRODUCER_REPLAY",
        )
        .orchestration
        .unwrap();
        assert_eq!(original, replay);
    }
    #[test]
    fn h3_producer_expiry_refused_before_replay() {
        let f = Fixture::new();
        let (run, execution, producer) = reserve(&f);
        let (actor, endpoint) = task_actor(&f, &run, &execution, &producer);
        let request = task_request(
            &f,
            &actor,
            &endpoint,
            OrchestrationCommand::RecordContext {
                task: execution.task,
                feedback: vec![],
                decisions: vec![],
            },
        );
        ok(
            call(
                &f,
                &actor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
                0,
            ),
            "H3_PRODUCER_LIVE",
        );
        {
            let owner = f.host.owner.upgrade().unwrap();
            let mut host = owner.lock().unwrap();
            let index = host
                .resolve_attachment(Some(&actor.bootstrap.auth.bearer()))
                .unwrap();
            host.attachments[index]
                .producer_scope
                .as_mut()
                .unwrap()
                .expires_at = Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        }
        denied(
            &f,
            &actor,
            HostCommand::Status,
            "producer_expired",
            "H3_PRODUCER_EXPIRY",
        );
        denied(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "producer_expired",
            "H3_PRODUCER_EXPIRY_REPLAY",
        );
    }
    #[test]
    fn h3_producer_cannot_start_or_mint_scope() {
        let f = Fixture::new();
        let (run, execution, producer) = reserve(&f);
        let (actor, endpoint) = task_actor(&f, &run, &execution, &producer);
        let request = f.start_request(&actor);
        denied(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "orchestration_refused",
            "H3_PRODUCER_START",
        );
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        brief.scope.operations.insert("split_clip".into());
        let request = task_request(
            &f,
            &actor,
            &endpoint,
            OrchestrationCommand::AssignTask(Box::new(AssignTask {
                run,
                parent: execution.task,
                brief,
            })),
        );
        denied(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            "role_refused",
            "H3_PRODUCER_NARROWING",
        );
    }
    #[test]
    fn h4_detach_preserves_run_producer_and_reattaches() {
        let f = Fixture::with_media(true);
        let (run, execution, producer) = reserve(&f);
        let (task, endpoint) = task_actor(&f, &run, &execution, &producer);
        denied(
            &f,
            &f.editor,
            HostCommand::Shutdown,
            "continuation_required",
            "H4_SHUTDOWN_LIVE",
        );
        let before = f.projection(&run);
        let root = state(&f)["root"].clone();
        let mut retained = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap();
        assert!(f.issuer.reattach_editor().is_err(), "H4_ONE_EDITOR");
        let detached = ok(
            call(&f, &f.editor, HostCommand::DetachForContinuation, 1),
            "H4_DETACH",
        )
        .detached
        .unwrap();
        assert_eq!(detached.retained_runs, vec![run.clone()]);
        assert!(
            f.editor.request(HostCommand::Status).is_err(),
            "H4_GUI_REVOKED"
        );
        ok(
            call(&f, &producer, HostCommand::Status, 0),
            "H4_PRODUCER_SURVIVES",
        );
        ok(call(&f, &task, HostCommand::Status, 0), "H4_TASK_SURVIVES");
        assert_eq!(f.projection(&run), before, "H4_SAME_ATTEMPT");
        assert_eq!(state(&f)["root"], root, "H4_HEADS_UNCHANGED");
        let mut after_usage = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .usage()
            .unwrap();
        // Readiness may increase probe counters and temporary scratch high-water marks.
        for usage in [&mut retained, &mut after_usage] {
            usage.verified_calls = 0;
            usage.verified_bytes = 0;
            usage.peak_physical_bytes = 0;
            usage.peak_reserved_bytes = 0;
            usage.peak_decoded_bytes = 0;
            usage.peak_decoder_bytes = 0;
            usage.peak_live_history_bytes = 0;
        }
        assert_eq!(after_usage, retained, "H4_PINS_CHARGES");
        let reattached = f.issuer.reattach_editor().unwrap();
        assert_eq!(f.projection(&run).attempts[0].identity, execution);
        assert!(endpoint.check_live().is_ok(), "H4_TASK_ENDPOINT_LIVE");
        let page = ok(
            reattached
                .request(HostCommand::ObserveRun {
                    run: run.clone(),
                    after_sequence: 0,
                })
                .unwrap(),
            "H4_REATTACH",
        )
        .run_view
        .unwrap();
        assert_eq!(page.projection, before);
        assert!(f.issuer.reattach_editor().is_err(), "H4_ONE_EDITOR");
        ok(
            reattached.request(HostCommand::Detach).unwrap(),
            "H4_DETACH_CONTROL",
        );
    }
    #[test]
    fn h4_private_issuer_refuses_viewer_and_role_spoofing() {
        let f = Fixture::new();
        let viewer = f.actor(AttachmentRole::Viewer, "viewer");
        ok(
            f.editor.request(HostCommand::Detach).unwrap(),
            "H4_SETUP_DETACH",
        );
        assert!(viewer.post("issuer", b"{}").is_err(), "H4_ISSUER_PRIVATE");
        let issuer_client = HostClient::connect(
            f.host.endpoint.clone(),
            f.issuer_auth.clone(),
            f.host.roots.clone(),
        )
        .unwrap();
        assert!(
            issuer_client
                .post("issuer", br#"{"role":"producer"}"#)
                .is_err(),
            "H4_ISSUER_FIXED_ROLE"
        );
        assert!(
            issuer_client.request(HostCommand::Status).is_err(),
            "H4_ISSUER_NOT_ATTACHMENT"
        );
        let editor = f.issuer.reattach_editor().unwrap();
        ok(
            editor.request(HostCommand::ReadHistory).unwrap(),
            "H4_TRUSTED_REATTACH",
        );
    }
    #[test]
    fn h4_non_editor_detach_refused() {
        let f = Fixture::new();
        f.start();
        for (role, subject) in [
            (AttachmentRole::Viewer, "viewer"),
            (AttachmentRole::Producer, "producer"),
        ] {
            let actor = f.actor(role, subject);
            denied(
                &f,
                &actor,
                HostCommand::DetachForContinuation,
                "continuation_refused",
                "H4_DEPARTING_EDITOR",
            );
        }
    }
    #[test]
    fn h4_expired_retained_producer_refuses_detach() {
        let f = Fixture::new();
        let started = f.start();
        let run: RunId = started.envelope.identity("run").unwrap();
        let auth = f.producer(&run);
        let owner = f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let index = host.resolve_attachment(Some(&auth.bearer())).unwrap();
        host.attachments[index]
            .producer_scope
            .as_mut()
            .unwrap()
            .expires_at = Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        drop(host);
        drop(owner);
        denied(
            &f,
            &f.editor,
            HostCommand::DetachForContinuation,
            "continuation_refused",
            "H4_PRODUCER_READINESS",
        );
    }
    #[test]
    fn h4_refuses_without_independently_retained_inputs() {
        let f = Fixture::new();
        let started = f.start();
        let run: RunId = started.envelope.identity("run").unwrap();
        let config = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .config
            .clone();
        let Fixture {
            temp, host, brief, ..
        } = f;
        drop(host);
        let issuer_auth = HostAuth::generate().unwrap();
        let host = WriterHost::start_with_issuer(
            config,
            HostAuth::generate().unwrap(),
            Some(issuer_auth.clone()),
        )
        .unwrap();
        let editor = host.client();
        let issuer = HostIssuer::new(
            host.endpoint.clone(),
            issuer_auth.clone(),
            host.roots.clone(),
        )
        .unwrap();
        let f = Fixture {
            host,
            editor,
            issuer,
            issuer_auth,
            brief,
            temp,
        };
        assert_eq!(f.projection(&run).outcome, RunOutcome::Pending);
        denied(
            &f,
            &f.editor,
            HostCommand::DetachForContinuation,
            "continuation_refused",
            "H4_READY_REFUSED",
        );
        ok(
            f.editor.request(HostCommand::Status).unwrap(),
            "H4_STILL_ATTACHED",
        );
    }

    #[test]
    fn h3_control_revision_refresh_and_original_replay() {
        let f = Fixture::new();
        let started = f.start();
        let run: RunId = started.envelope.identity("run").unwrap();
        let old = f.projection(&run).control_revision;
        let request = f.control(&f.editor, &run, RunAction::Pause);
        let committed = ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
                2,
            ),
            "H3_CONTROL_ACK",
        )
        .orchestration
        .unwrap();
        assert_eq!(f.projection(&run).phase, RunPhase::Paused);
        let replay = ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                },
                0,
            ),
            "H3_CONTROL_REPLAY",
        )
        .orchestration
        .unwrap();
        assert_eq!(committed, replay);
        let mut stale = f.control(&f.editor, &run, RunAction::Continue);
        let OrchestrationCommand::ControlRun(intent) = &mut stale.envelope.command else {
            unreachable!()
        };
        intent.expected_control_revision = old;
        stale.envelope.digest = stale.envelope.computed_digest().unwrap();
        let refused = ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(stale),
                },
                1,
            ),
            "H3_STALE_CONTROL",
        )
        .orchestration
        .unwrap();
        assert!(matches!(
            refused.decision,
            CommandDecision::Refused(
                crate::orchestration::orchestration_commit::RecordedRefusal::StaleControl
            )
        ));
        assert_eq!(f.projection(&run).phase, RunPhase::Paused);
        let viewer = f.actor(AttachmentRole::Viewer, "observer");
        for route in 0..3 {
            let page = ok(
                call(
                    &f,
                    &viewer,
                    HostCommand::ObserveRun {
                        run: run.clone(),
                        after_sequence: 0,
                    },
                    route,
                ),
                "H3_REFRESH",
            )
            .run_view
            .unwrap();
            assert!(page.full_refresh && page.allowed_actions.is_empty());
            let gap = ok(
                call(
                    &f,
                    &f.editor,
                    HostCommand::ObserveRun {
                        run: run.clone(),
                        after_sequence: 1,
                    },
                    route,
                ),
                "H3_SEQUENCE_GAP",
            )
            .run_view
            .unwrap();
            assert!(gap.full_refresh);
            assert!(gap.allowed_actions.contains(&RunActionKind::Continue));
        }
    }
}
