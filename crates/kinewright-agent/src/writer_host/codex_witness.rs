//! On-demand product launch. No provider execution occurs while compiling or in CI.
#![allow(clippy::missing_errors_doc)]
use super::{
    Arc, Core, Document, Duration, HostAuth, HostClient, HostCommand, HostConfig, HostOrigin,
    HostReply, Ja1EditingSession, MetadataIssuer, Mutex, ScopedOrchestrationRequest, SessionRoots,
    WriterHost, studio_runtime,
};
use crate::orchestration::{
    orchestration_commit::CommandDecision,
    orchestration_records::{
        AuthorityBinding, OrchestrationCommand, OrchestrationEnvelope, Role, RunAction, RunCleanup,
        RunId, RunIntent, RunOutcome, RunPhase,
    },
};
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    receipts::ReceiptResult,
    studio_records::{
        CommentId, FeedbackAnchor, FeedbackBatch, FeedbackBatchId, FeedbackRevision, FrameRate,
        MetadataChange, MetadataResult, MetadataUpdate, Orientation, PublishSnapshot,
        ReviseRequest, ReviseResult, VersionId,
    },
};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::Instant,
};

fn reply(client: &HostClient, command: HostCommand) -> HostReply {
    let reply = client.request(command).expect("C5 host transport");
    assert!(reply.error.is_none(), "C5 host refusal");
    reply
}
fn issue(client: &HostClient) -> MetadataIssuer {
    reply(client, HostCommand::IssueMetadataIssuer { issuer: None })
        .metadata_issuer
        .unwrap()
}
fn metadata(
    client: &HostClient,
    change: MetadataChange,
) -> kinewright_project::receipts::CommandReceipt {
    let issued = issue(client);
    reply(
        client,
        HostCommand::Metadata {
            envelope: issued.envelope,
            update: MetadataUpdate {
                expected_root_generation: issued.root_generation,
                change,
            },
        },
    )
    .metadata
    .unwrap()
}
fn wait_recovery(host: &WriterHost) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = reply(&host.client(), HostCommand::Status).status;
        if status.native_runtime_status.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "C5 recovery deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ProbeOwner(PathBuf);
impl crate::guardian::DurableAttemptOwner for ProbeOwner {
    fn reserve(
        &mut self,
        identity: &crate::job_guardian::protocol::AttemptIdentity,
    ) -> io::Result<()> {
        fs::write(
            self.0.join("reservation.json"),
            serde_json::to_vec(identity)?,
        )?;
        fs::File::open(self.0.join("reservation.json"))?.sync_all()
    }
    fn authorize_commit(
        &mut self,
        identity: &crate::job_guardian::protocol::RecoveryIdentity,
    ) -> io::Result<()> {
        fs::write(self.0.join("commit.json"), serde_json::to_vec(identity)?)?;
        fs::File::open(self.0.join("commit.json"))?.sync_all()
    }
}
fn guardian() -> PathBuf {
    let executable = std::env::current_exe().unwrap();
    let directory = executable.parent().unwrap();
    let directory = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent().unwrap()
    } else {
        directory
    };
    directory.join("kinewright-job-guardian")
}
static PROBE_SEQUENCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn local_probe(arguments: Vec<String>) -> (String, String) {
    use crate::job_guardian::protocol::{
        AttemptIdentity, CredentialMount, Program, ProviderSandbox,
    };
    let executable =
        crate::codex_qualification::executable().expect("C5 installed native executable");
    let source = crate::codex_credentials::source_path().expect("C5 credential location");
    let identity_file =
        crate::codex_credentials::inspect(&source, None).expect("C5 subscription sign-in");
    let root = std::env::temp_dir().join(format!(
        "p5-{}-{}",
        std::process::id(),
        PROBE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let scratch = root.join("s");
    fs::create_dir(&scratch).unwrap();
    fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
    let home = crate::codex_credentials::prepare_home(&scratch).unwrap();
    let pipes = crate::provider_pipes::ProviderPipeListeners::bind(&scratch.join("io")).unwrap();
    let runtime = ["/usr", "/lib", "/lib64", "/dev/null"]
        .map(Into::into)
        .into_iter()
        .chain(crate::native_runtime::runtime_tree(&executable).unwrap())
        .collect();
    let program = Program {
        executable: executable.path.clone(),
        arguments,
        environment: std::collections::BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), scratch.to_string_lossy().into_owned()),
            ("TMPDIR".into(), scratch.to_string_lossy().into_owned()),
            ("CODEX_HOME".into(), home.to_string_lossy().into_owned()),
        ]),
        working_directory: scratch.clone(),
        pipe_directory: Some(pipes.directory().into()),
        sandbox: ProviderSandbox::LinuxAllowlist {
            credential_mount: Some(Box::new(CredentialMount {
                source: source.clone(),
                target: home.join("auth.json"),
                identity: identity_file,
            })),
            connect_tcp: vec![443],
            runtime,
            scratch: scratch.clone(),
            credential_store: home,
            executable_identity: executable.file_identity,
        },
    };
    let fresh = || {
        kinewright_project::bundle_records::SessionId::fresh()
            .unwrap()
            .as_str()
            .to_owned()
    };
    let identity = AttemptIdentity {
        project: fresh(),
        run: fresh(),
        task: fresh(),
        attempt: fresh(),
        nonce: fresh(),
        owner_instance: fresh(),
    };
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &root.join("claim"),
        &identity.project,
        &identity.run,
    )
    .unwrap();
    let mut owner = ProbeOwner(root.clone());
    let mut session = crate::guardian::ContainedSession::prepare(
        &mut claim,
        &mut owner,
        identity,
        program,
        &guardian(),
        &crate::guardian::CertifiedProfile::linux_allowlist(),
    )
    .unwrap();
    let mut pipes = pipes.accept().unwrap();
    pipes.credentials(b"").unwrap();
    session.commit(&mut owner).unwrap();
    let diagnostics =
        crate::protocol::limits::stderr_capture(pipes.diagnostics, "c5-local-diagnostics").unwrap();
    let mut output = String::new();
    pipes
        .output
        .take(65537)
        .read_to_string(&mut output)
        .unwrap();
    session.close_lifetime();
    let proof = session.cleanup().unwrap();
    claim.retire(&proof).unwrap();
    claim.finish().unwrap();
    crate::codex_credentials::inspect(&source, Some(identity_file)).unwrap();
    fs::remove_dir_all(root).unwrap();
    (output, diagnostics.join().unwrap())
}

fn local_version_probe() {
    let (output, _) = local_probe(vec!["--version".into()]);
    assert_eq!(
        output.trim(),
        "codex-cli 0.166.0",
        "C5 installed version through guardian"
    );
    eprintln!(
        "C5_LOCAL_VERSION version=0.166.0 guardian=true subscription=true credential_identity=true paid=false"
    );
}

/// Run only version, login status and local feature enumeration under the guardian.
///
/// # Panics
/// Fails if the installed release, sign-in or disabled features differ.
#[doc(hidden)]
pub fn run_installed_codex_local_policy_witness() {
    local_version_probe();
    let (login_output, login_diagnostics) = local_probe(vec!["login".into(), "status".into()]);
    let signed_in = login_output
        .lines()
        .chain(login_diagnostics.lines())
        .any(|line| line.starts_with("Logged in using ChatGPT"));
    let arguments = vec![
        "features".into(),
        "list".into(),
        "-c".into(),
        crate::native_launcher::codex_feature_policy(),
    ];
    let (output, diagnostics) = local_probe(arguments);
    let features = [
        "shell_tool",
        "unified_exec",
        "multi_agent",
        "multi_agent_v2",
        "multi_agent_v2_dynamic_tools",
        "plugins",
        "hooks",
        "memories",
        "code_mode",
        "token_budget",
        "sleep_tool",
        "current_time_reminder",
        "send_message_to_user_async",
    ];
    let states: std::collections::BTreeMap<_, _> = features
        .into_iter()
        .map(|feature| {
            let state = output
                .lines()
                .find(|line| line.split_whitespace().next() == Some(feature))
                .and_then(|line| match line.split_whitespace().last() {
                    Some("true") => Some(true),
                    Some("false") => Some(false),
                    _ => None,
                });
            (feature, state)
        })
        .collect();
    // In 0.166.0 unified_exec is forced on by managed requirements. The
    // preceding ShellTool gate and disabled model shell suppress both executors.
    let disabled = states
        .iter()
        .all(|(feature, state)| *feature == "unified_exec" || *state == Some(false));
    let has = |needle: &[u8]| {
        diagnostics
            .as_bytes()
            .windows(needle.len())
            .any(|part| part.eq_ignore_ascii_case(needle))
    };
    let error_category = if has(b"auth") {
        "authentication"
    } else if has(b"network") || has(b"connect") {
        "network"
    } else if has(b"unknown feature") || has(b"invalid") {
        "configuration"
    } else {
        "none_or_unclassified"
    };
    let evidence =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/s4b-c5/local-policy.json");
    fs::write(evidence,serde_json::to_vec_pretty(&serde_json::json!({"subscription_status":signed_in,"feature_states":states,"error_category":error_category,"paid":false})).unwrap()).unwrap();
    assert!(signed_in, "C5 effective local subscription status refused");
    assert!(
        disabled,
        "C5 local feature policy mismatch; see sanitized record"
    );
    eprintln!("C5_LOCAL_POLICY subscription=true required_features_disabled=true paid=false");
}

/// The only public qualification entry point runs the complete fixed witness.
/// It cannot supply profiles, auth evidence, or arbitrary provider commands.
///
/// # Panics
/// Fails the qualification test on a refusal, broken witness, or repeated run id.
#[doc(hidden)]
#[allow(
    clippy::too_many_lines,
    reason = "one live product witness retains ownership through Stop and cleanup"
)]
pub fn run_installed_codex_director_witness() {
    let number = std::env::var("KINEWRIGHT_S4B_C5_RUN")
        .unwrap_or_else(|_| "1".into())
        .parse::<u8>()
        .unwrap();
    assert!((1..=3).contains(&number), "C5 three-run cap");
    let evidence = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/s4b-c5");
    fs::create_dir_all(&evidence).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(evidence.join(format!("live-{number}.started")))
        .expect("C5 live run id already used");
    let report = Arc::new(Mutex::new(
        serde_json::json!({"run":number,"full_run_started":false,"usage":null,"refresh_live":"NOT RUN","version":"0.166.0","model":"gpt-6.1-sol","reasoning":"high","token_ceiling":crate::codex_qualification::TOKEN_CEILING}),
    ));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| live_body(&report)));
    report.lock().unwrap()["passed"] = serde_json::json!(result.is_ok());
    fs::write(
        evidence.join(format!("live-{number}.json")),
        serde_json::to_vec_pretty(&*report.lock().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(
        result.is_ok(),
        "C5 installed witness failed; usage retained in live report"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the live gate keeps ReviseCut, history and cleanup assertions together"
)]
fn live_body(report: &Arc<Mutex<serde_json::Value>>) {
    local_version_probe();
    let credential_source = crate::codex_credentials::source_path().unwrap();
    let credential_identity = crate::codex_credentials::inspect(&credential_source, None).unwrap();
    let credential_before = fs::metadata(&credential_source)
        .unwrap()
        .modified()
        .unwrap();
    let observed = report.clone();
    let witness = Arc::new(studio_runtime::RuntimeWitness {
        on_native_events: Arc::new(|_| {}),
        on_event: Arc::new(move |event| match event {
            kinewright_core::AgentEvent::ToolCall { name, .. } => {
                let known = crate::runtime::COMPACT_TOOL_NAMES.contains(&name.as_str())
                    || super::task_mcp::CAPABILITIES
                        .iter()
                        .any(|(candidate, _)| *candidate == name);
                let mut report = observed.lock().unwrap();
                if known {
                    if report["tools_observed"].is_null() {
                        report["tools_observed"] = serde_json::json!([]);
                    }
                    report["tools_observed"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!(name));
                } else {
                    report["unexpected_tool"] = serde_json::json!(true);
                }
            }
            kinewright_core::AgentEvent::Error(message) => {
                let has = |needle: &[u8]| {
                    message
                        .as_bytes()
                        .windows(needle.len())
                        .any(|part| part.eq_ignore_ascii_case(needle))
                };
                let kind = if has(b"mcp") {
                    "product_mcp"
                } else if has(b"auth")
                    || has(b"sign in")
                    || has(b"login")
                    || has(b"401")
                    || has(b"403")
                {
                    "authentication"
                } else if has(b"connect")
                    || has(b"network")
                    || has(b"timeout")
                    || has(b"timed out")
                    || has(b"dns")
                    || has(b"tls")
                {
                    "network"
                } else if has(b"model") && (has(b"not found") || has(b"unavailable")) {
                    "model_unavailable"
                } else if has(b"config") || has(b"permission denied") {
                    "product_configuration"
                } else {
                    "product_or_unknown"
                };
                let mut report = observed.lock().unwrap();
                if kind != "product_or_unknown" || report["error_kind"].is_null() {
                    report["error_kind"] = serde_json::json!(kind);
                }
            }
            _ => {}
        }),
    });
    let temp = kinewright_media::test_support::TempDirectory::new_in(&std::env::temp_dir(), "l5");
    let path = temp.path("project.kinewright");
    let mut document = Document::default();
    document.markers.push(kinewright_core::Marker {
        id: kinewright_core::MarkerId(1),
        position: kinewright_core::TimeCode(1),
        label: "before".into(),
        color_token: 0,
    });
    let core = Core::spawn(document).unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    drop(bundle);
    let source = temp.path("owned.wav");
    fs::write(
        &source,
        kinewright_media::test_support::wav_f32(&[0.0; 480], 48000, 1),
    )
    .unwrap();
    let mut owner = Ja1EditingSession::open(&path).unwrap();
    owner.import_owned_media(&source).unwrap();
    owner
        .core()
        .request(kinewright_core::Command::Do(
            kinewright_core::Operation::MoveMarker {
                marker: kinewright_core::MarkerId(1),
                to: kinewright_core::TimeCode(3),
            },
        ))
        .unwrap();
    owner
        .core()
        .request(kinewright_core::Command::Undo)
        .unwrap();
    owner.save_active().unwrap();
    let manual = owner.published_manual_history().unwrap();
    assert!(!manual.cursor.redo.is_empty());
    drop(owner);
    let claim = temp.path("machine");
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "C5 disposable live".into(),
            path: Some(path.clone()),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        None,
        studio_runtime::RuntimePorts {
            witness: Some(witness),
            qualification: true,
            guardian: guardian(),
            fixture_claim: Some(claim.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    wait_recovery(&host);
    let client = host.client();
    let owner_arc = host.owner.upgrade().unwrap();
    let (checkpoint_ref, checkpoint) = owner_arc
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .manual_checkpoint()
        .unwrap();
    let publisher_auth = HostAuth::generate().unwrap();
    owner_arc
        .lock()
        .unwrap()
        .register_attachment(
            publisher_auth.clone(),
            "C5 snapshot publisher".into(),
            super::AttachmentRole::Producer,
            host.roots.clone(),
            false,
        )
        .unwrap();
    let publisher =
        HostClient::connect(host.endpoint.clone(), publisher_auth, host.roots.clone()).unwrap();
    let snapshot = metadata(
        &publisher,
        MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: checkpoint.head_id.clone(),
            checkpoint_ref,
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(snapshot),
    } = snapshot.result
    else {
        panic!("C5 snapshot");
    };
    let mut locked = owner_arc.lock().unwrap();
    let text=locked.owner_mut().unwrap().admit_host_grant(b"Move existing marker 1 from frame 1 to frame 2. Use one Editor specialist and its MCP edit. Merge that child's result once into the candidate, then freeze/publish the Director. Preserve the person's manual history. Do not use any other provider.").unwrap();
    let empty = locked.owner_mut().unwrap().admit_host_grant(b"[]").unwrap();
    drop(locked);
    let saved = metadata(
        &client,
        MetadataChange::SaveFeedback(FeedbackRevision {
            comment_id: CommentId::fresh().unwrap(),
            revision: 1,
            predecessor: None,
            anchor: FeedbackAnchor {
                snapshot,
                frame: 1,
                fps: FrameRate {
                    numerator: 24,
                    denominator: 1,
                },
                raster_size: [16, 16],
                orientation: Orientation::Upright,
                target: None,
                source_ref: None,
                script_beat_id: None,
            },
            original_text_ref: text.clone(),
            strokes_ref: empty,
            frame_proof_ref: text,
            voice_ref: None,
            transcript_revision: None,
        }),
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(revision),
    } = saved.result
    else {
        panic!("C5 note");
    };
    let issued = issue(&client);
    let batch = FeedbackBatchId::fresh().unwrap();
    let request = ReviseRequest {
        command_id: issued.envelope.id.clone(),
        batch_id: batch.clone(),
        expected_candidate_head: checkpoint.head_id,
    };
    reply(
        &client,
        HostCommand::Metadata {
            envelope: issued.envelope,
            update: MetadataUpdate {
                expected_root_generation: issued.root_generation,
                change: MetadataChange::SubmitBatch {
                    batch: FeedbackBatch {
                        batch_id: batch,
                        revision_refs: vec![revision],
                    },
                    request: request.clone(),
                },
            },
        },
    );
    let revised = reply(&client, HostCommand::ReviseCut { request })
        .revise_result
        .unwrap();
    let ReviseResult::Accepted { run_id, .. } = revised else {
        panic!("C5 ReviseCut");
    };
    let run: RunId = serde_json::from_value(serde_json::to_value(run_id).unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut failure = false;
    let final_state = loop {
        let view = reply(
            &client,
            HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            },
        )
        .run_view
        .unwrap();
        let state = view.projection;
        let mut budget_exceeded = state.tasks.len() > 2;
        if let Some(accounting) = view.accounting {
            budget_exceeded |= accounting
                .tasks
                .iter()
                .map(|task| task.sends)
                .sum::<usize>()
                > 4;
            if accounting.tasks.iter().any(|task| task.sends != 0) {
                report.lock().unwrap()["full_run_started"] = serde_json::json!(true);
            }
            report.lock().unwrap()["usage"] = serde_json::to_value(accounting).unwrap();
        }
        if budget_exceeded {
            report.lock().unwrap()["witness_budget_exceeded"] = serde_json::json!(true);
            failure = true;
            break state;
        }
        if state.outcome != RunOutcome::Pending && state.cleanup == RunCleanup::Settled {
            break state;
        }
        if state.phase == RunPhase::Interrupted
            || Instant::now() >= deadline
            || view.runtime_status.is_some()
            || report.lock().unwrap()["error_kind"]
                .as_str()
                .is_some_and(|kind| {
                    matches!(
                        kind,
                        "authentication"
                            | "network"
                            | "model_unavailable"
                            | "product_mcp"
                            | "product_configuration"
                    )
                })
        {
            failure = true;
            break state;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // Always Stop through the same editor control path, including a failed run.
    let issued = issue(&client);
    let mut envelope = OrchestrationEnvelope {
        id: issued.envelope.id.clone(),
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
        project: issued.envelope.project_id,
        expected_generation: issued.root_generation,
        authority: AuthorityBinding {
            grant_ref: issued.grant_ref,
            owner_epoch: issued.envelope.owner_epoch,
            endpoint: None,
            attempt: None,
        },
        command: OrchestrationCommand::ControlRun(RunIntent {
            command_id: issued.envelope.id,
            run_id: run.clone(),
            expected_control_revision: final_state.control_revision.clone(),
            action: RunAction::Stop,
        }),
    };
    envelope.digest = envelope.computed_digest().unwrap();
    let stopped = reply(
        &client,
        HostCommand::DispatchOrchestration {
            request: Box::new(ScopedOrchestrationRequest {
                run: Some(run.clone()),
                envelope,
                consume_batch: None,
            }),
        },
    );
    assert_eq!(
        stopped.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    let cleanup = Instant::now() + Duration::from_secs(20);
    while claim.join("run.json").exists() {
        reply(&client, HostCommand::Status);
        assert!(Instant::now() < cleanup, "C5 cleanup");
        std::thread::sleep(Duration::from_millis(10));
    }
    let owner = owner_arc.lock().unwrap();
    let projection = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    let accounting = owner.native_accounting(&projection).unwrap().unwrap();
    report.lock().unwrap()["usage"] = serde_json::to_value(&accounting).unwrap();
    assert_eq!(
        owner.owner().unwrap().published_manual_history().unwrap(),
        manual,
        "C5 exact manual history"
    );
    report.lock().unwrap()["cleanup"] = serde_json::json!(true);
    report.lock().unwrap()["manual_exact"] = serde_json::json!(true);
    assert!(!failure, "C5 provider/runtime failure; no automatic retry");
    assert!(
        report.lock().unwrap()["unexpected_tool"] != true,
        "C5 unexpected provider tool"
    );
    let tools = report.lock().unwrap()["tools_observed"].clone();
    assert!(
        tools.as_array().is_some_and(|tools| tools
            .contains(&serde_json::json!("prepare_edit_plan"))
            && tools.contains(&serde_json::json!("commit_edit_plan"))),
        "C5 actual specialist MCP calls"
    );
    crate::codex_credentials::inspect(&credential_source, Some(credential_identity)).unwrap();
    report.lock().unwrap()["credential_write_observed"] = serde_json::json!(
        fs::metadata(&credential_source)
            .unwrap()
            .modified()
            .unwrap()
            != credential_before
    );
    report.lock().unwrap()["credential_identity_exit"] = serde_json::json!(true);
    assert!(
        accounting
            .tasks
            .iter()
            .filter(|task| !task.sends.is_empty())
            .all(|task| task.totals.known.input.is_some() && task.totals.known.output.is_some()),
        "C5 recorded usage"
    );
    assert_eq!(final_state.outcome, RunOutcome::Ready, "C5 live completion");
    assert_eq!(final_state.tasks.len(), 2, "C5 one specialist");
    let child = final_state
        .tasks
        .iter()
        .find(|task| task.parent.is_some())
        .unwrap();
    assert_eq!(
        owner.hooks.runs[&run].tasks[&child.id]
            .core()
            .export_history()
            .unwrap()
            .document
            .markers[0]
            .position,
        kinewright_core::TimeCode(2),
        "C5 editor owns the actual edit"
    );
    assert!(
        final_state
            .tasks
            .iter()
            .any(|task| task.parent.is_some() && task.brief.role == Role::Editor),
        "C5 specialist"
    );
    assert_eq!(
        final_state
            .tasks
            .iter()
            .filter(|task| owner
                .saved_task_history(&projection, &task.id)
                .unwrap()
                .is_some_and(|history| history.merge_envelope.is_some()))
            .count(),
        1,
        "C5 one candidate merge"
    );
    let candidate = &final_state.tasks[0].brief.scope.base;
    let guard = owner
        .owner()
        .unwrap()
        .durable_sessions()
        .into_iter()
        .find(|(_, guard)| guard.branch_id() == &candidate.branch)
        .unwrap()
        .1;
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .durable_branch_history(&guard)
            .unwrap()
            .document
            .markers[0]
            .position,
        kinewright_core::TimeCode(2),
        "C5 specialist edit"
    );
    report.lock().unwrap()["specialist_edit"] = serde_json::json!(true);
    report.lock().unwrap()["merge_once"] = serde_json::json!(true);
    drop(guard);
    drop(owner);
    drop(owner_arc);
    drop(host);
    let reopened = Ja1EditingSession::open(&path).unwrap();
    assert_eq!(
        reopened.published_manual_history().unwrap(),
        manual,
        "C5 reopened manual history"
    );
    eprintln!(
        "C5_INSTALLED_DIRECTOR ReviseCut=true specialist_mcp_edit=true merge_once=true manual_exact=true stop=true cleanup=true"
    );
}
