fn te15_b2b0_notes(texts: &[String]) -> (Fixture, RunId) {
    te15_b2b0_notes_for(texts, "scripted")
}
fn te15_b2b0_notes_for(texts: &[String], provider: &str) -> (Fixture, RunId) {
    use kinewright_project::studio_records::{CommentId, FeedbackRevision};
    let mut f = if provider == "codex" {
        te15_fix_setup("codex").0
    } else {
        Fixture::new()
    };
    if provider == "codex" {
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .fixture_director = false;
    }
    configure_director(&f);
    let batch = manual_batch(&f);
    let (template, head) = {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let project = host.owner().unwrap();
        let submitted = project.read_submitted_batch(&batch).unwrap();
        let revision = project
            .store()
            .unwrap()
            .read_record::<FeedbackRevision>(&submitted.batch.revision_refs[0])
            .unwrap();
        (
            (*revision).clone(),
            project.manual_checkpoint().unwrap().1.head_id,
        )
    };
    let mut refs = Vec::new();
    for text in texts {
        let mut revision = template.clone();
        revision.comment_id = CommentId::fresh().unwrap();
        revision.original_text_ref = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(text.as_bytes())
            .unwrap();
        let saved = ok(
            f.editor
                .request(metadata_request(
                    &f,
                    &f.editor,
                    MetadataChange::SaveFeedback(revision),
                ))
                .unwrap(),
            "TE14_FIX1_NOTE_SAVED",
        );
        let ReceiptResult::Metadata {
            result: MetadataResult::Feedback(reference),
        } = saved.metadata.unwrap().result
        else {
            panic!("note result")
        };
        refs.push(reference);
    }
    let batch = FeedbackBatchId::fresh().unwrap();
    let issued = Fixture::issue(&f.editor);
    ok(
        f.editor
            .request(HostCommand::Metadata {
                envelope: issued.envelope.clone(),
                update: MetadataUpdate {
                    expected_root_generation: issued.root_generation,
                    change: MetadataChange::SubmitBatch {
                        batch: FeedbackBatch {
                            batch_id: batch.clone(),
                            revision_refs: refs,
                        },
                        request: ReviseRequest {
                            command_id: issued.envelope.id,
                            batch_id: batch.clone(),
                            expected_candidate_head: head,
                        },
                    },
                },
            })
            .unwrap(),
        "TE14_FIX1_NOTES_SUBMITTED",
    );
    let request = submitted_request(&f, &batch);
    let outcome = revise_result(revise(&f, &f.editor, &request), "TE14_FIX1_NOTES_REVISE");
    let kinewright_project::studio_records::ReviseResult::Accepted { run_id, .. } = outcome else {
        panic!("B2B0_NOTES_ADMITTED {outcome:?}")
    };
    let run = run_of(&run_id);
    if provider == "scripted-claude" {
        // The submitted fixture Director picker deliberately offers only its
        // Codex dialect. Preserve the real saved/submitted notes, stop that
        // unsent fixture run, and start the Claude protocol fixture against
        // the same immutable brief and batch.
        let mut brief = f.projection(&run).tasks[0].brief.clone();
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "B2B0_FIXTURE_STOP",
        );
        brief.provider = super::super::studio_runtime::fixture::claude_fixture_selection();
        f.brief = brief;
        let started = f.start();
        let run = started.envelope.identity("run").unwrap();
        return (f, run);
    }
    (f, run)
}
fn te15_b2b0_reserved_notes(texts: &[String]) -> (Fixture, RunId, ExecutionIdentity) {
    let (f, run) = te15_b2b0_notes(texts);
    let receipt = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_cut(&run)
        .unwrap()
        .unwrap()
        .receipt;
    let (_, identity, _) = reserve_from_started(&f, receipt);
    (f, run, identity)
}

#[test]
fn te15_b2b0_long_feedback_exact_stdin() {
    use std::io::Read;
    let mut sizes = Vec::new();
    for total in [1024, 20 * 1024, 200 * 1024] {
        let texts: Vec<_> = (0..total)
            .step_by(16 * 1024)
            .map(|start| "x".repeat((total - start).min(16 * 1024)))
            .collect();
        let (f, run, identity) = te15_b2b0_reserved_notes(&texts);
        let admitted = native_admitted(&f, &identity);
        let (profile, _, scratch) = native_profile(&f, &admitted);
        let invocation = native_charge(&f, &admitted, profile.clone());
        let bytes = invocation.invocation().bytes();
        let prompt: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let original: Vec<_> = prompt["data"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            original,
            texts.iter().map(String::as_str).collect::<Vec<_>>(),
            "B2B0_NOTES_UNCHANGED"
        );
        let program =
            crate::native_launcher::program(&profile, std::str::from_utf8(bytes).unwrap()).unwrap();
        let binding = program.input.as_ref().unwrap();
        let mut stdin = crate::job_guardian::input::seal(bytes, binding).unwrap();
        let mut received = Vec::new();
        stdin.read_to_end(&mut received).unwrap();
        assert_eq!(received, bytes, "B2B0_EXACT_INPUT");
        assert_eq!(kinewright_media::sha256_bytes(&received), binding.sha256);
        assert_eq!(stdin.metadata().unwrap().len(), bytes.len() as u64);
        sizes.push(serde_json::to_vec(&program).unwrap().len());
        let state = f.projection(&run);
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        assert_eq!(
            host.native_accounting(&state).unwrap().unwrap().tasks[0]
                .sends
                .len(),
            1
        );
        drop(host);
        std::fs::remove_dir_all(scratch).unwrap();
        eprintln!(
            "B2B0_INPUT feedback={total} bytes={} before={} after={}",
            bytes.len(),
            binding.sha256,
            kinewright_media::sha256_bytes(&received)
        );
    }
    // Only decimal byte_length width may grow. Full prompt length cannot be
    // allowed to consume the launch-metadata budget again.
    assert!(
        sizes.iter().all(|n| *n < 16384)
            && sizes.iter().max().unwrap() - sizes.iter().min().unwrap() < 16,
        "B2B0_METADATA_CONSTANT {sizes:?}"
    );
}

#[test]
fn te15_b2b0_over_budget_keeps_feedback_and_does_not_send() {
    let texts = vec!["q".repeat(16 * 1024); 17];
    let (f, run, identity) = te15_b2b0_reserved_notes(&texts);
    let admitted = native_admitted(&f, &identity);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let before = f.projection(&run);
    let producer = f.producer(&run);
    let owner = f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let accounting = host.native_accounting(&before).unwrap().unwrap();
    let now = accounting.last_unix_ms;
    let error = host
        .charge_native_invocation(
            &producer,
            &admitted,
            profile,
            &before.effects[0].id,
            crate::orchestration::adapters::SendKind::OpeningAssignment,
            now,
        )
        .err()
        .expect("B2B0_LIMIT_REFUSED");
    assert!(
        error
            .to_string()
            .contains(crate::job_guardian::protocol::INPUT_TOO_LONG),
        "B2B0_TRUE_WORDS {error}"
    );
    let prompt = host
        .codex_prompt(&before.tasks[0], b"inspect", &te12_fixed_context())
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&prompt).unwrap();
    assert_eq!(
        value["data"]["notes"].as_array().unwrap().len(),
        17,
        "B2B0_FEEDBACK_RETAINED"
    );
    assert!(
        value["data"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["text"].as_str().unwrap().len() == 16 * 1024)
    );
    assert_eq!(
        host.native_accounting(&before).unwrap().unwrap(),
        accounting,
        "B2B0_NO_CHARGE"
    );
    drop(host);
    assert_eq!(f.projection(&run), before, "B2B0_NO_RETRY");
    assert!(
        before
            .attempts
            .iter()
            .all(|a| a.state != AttemptState::Executing),
        "B2B0_ZERO_EXECUTIONS"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b2b0_guardian_long_input_and_stop() {
    te15_b2b0_guardian_bad_input();
    for provider in ["scripted", "scripted-claude"] {
        for total in [1024, 20 * 1024, 200 * 1024] {
            let texts: Vec<_> = (0..total)
                .step_by(16 * 1024)
                .map(|start| "f".repeat((total - start).min(16 * 1024)))
                .collect();
            let (f, run) = te15_b2b0_notes_for(&texts, provider);
            let receipt = f
                .host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .orchestration_cut(&run)
                .unwrap()
                .unwrap()
                .receipt;
            let (_, identity, _) = reserve_from_started(&f, receipt);
            let _unit = NativeUnitCleanup::new(&identity);
            let admitted = native_admitted(&f, &identity);
            let (profile, checks, scratch) = native_profile_for(f.temp.root(), &admitted, provider);
            let invocation = native_charge(&f, &admitted, profile);
            let expected = invocation.invocation().bytes().to_vec();
            let (session, claim) = native_start(&f, admitted, invocation, checks, false);
            let mut session = session.unwrap();
            wait_native(&scratch.join("stdin-input"));
            assert_eq!(
                std::fs::read(scratch.join("stdin-input")).unwrap(),
                expected,
                "B2B0_CHILD_EXACT_BYTES"
            );
            wait_native(&scratch.join("execution-ready"));
            assert_eq!(
                std::fs::read_to_string(scratch.join("executions"))
                    .unwrap()
                    .lines()
                    .count(),
                1,
                "B2B0_ONE_EXECUTION"
            );
            if total == 20 * 1024 {
                let mut accepted = false;
                for _ in 0..20 {
                    let request = f.control(&f.editor, &run, RunAction::Stop);
                    let reply = f
                        .editor
                        .request(HostCommand::DispatchOrchestration {
                            request: Box::new(request),
                        })
                        .unwrap();
                    if reply.error.is_none() {
                        accepted = true;
                        break;
                    }
                    assert_eq!(reply.error.unwrap().message, "StaleGeneration");
                }
                assert!(accepted, "B2B0_STOP_ACCEPTED");
            } else {
                std::fs::write(scratch.join("exit"), b"").unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while claim.lock().unwrap().retained_attempts() != 0 {
                assert!(Instant::now() < deadline, "B2B0_INPUT_CLEANUP");
                std::thread::park_timeout(Duration::from_millis(5));
            }
            session.interrupt();
            drop(session);
            finish_stopped_native(claim);
            assert!(
                !std::fs::read_dir(&scratch)
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|e| e.file_name() == "prompt"),
                "B2B0_NO_NAMED_PROMPT"
            );
            std::fs::remove_dir_all(scratch).unwrap();
            eprintln!(
                "B2B0_GUARDIAN provider={provider} feedback={total} bytes={} hash={} cleanup=true",
                expected.len(),
                kinewright_media::sha256_bytes(&expected)
            );
        }
    }
}

#[test]
fn te15_b2b0_every_role_has_exact_input_and_no_file_tool() {
    use std::io::Read;
    let (f, run) = te15_b2b0_notes(&["A quoted \"note\" with UTF-8 café and a newline\n".into()]);
    let state = f.projection(&run);
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    for role in [
        Role::Director,
        Role::Editor,
        Role::Reviewer,
        Role::Motion,
        Role::Audio,
        Role::Colour,
    ] {
        let mut task = state.tasks[0].clone();
        task.brief.role = role;
        let bytes = host
            .codex_prompt(&task, b"inspect exact data", &te12_fixed_context())
            .unwrap();
        let hash = kinewright_media::sha256_bytes(&bytes);
        let binding = crate::job_guardian::protocol::InputBinding::new(&bytes).unwrap();
        let mut stdin = crate::job_guardian::input::seal(&bytes, &binding).unwrap();
        let mut received = Vec::new();
        stdin.read_to_end(&mut received).unwrap();
        assert_eq!(received, bytes, "B2B0_EVERY_ROLE_INPUT {role:?}");
        assert_eq!(kinewright_media::sha256_bytes(&received), hash);
        let names = crate::orchestration::task_manifest::names(role);
        assert!(
            !names
                .iter()
                .any(|n| matches!(*n, "read_file" | "write_file" | "run_command")),
            "B2B0_NO_INPUT_PATH_TOOL"
        );
        eprintln!(
            "B2B0_ROLE role={role:?} before={hash} after={}",
            kinewright_media::sha256_bytes(&received)
        );
    }
    drop(host);
    let tool_fixture = Fixture::new();
    let (_, _, mcp) = c4_director(&tool_fixture);
    for name in ["read_file", "write_file"] {
        let result = mcp.runtime.block_on(
            mcp.client.call_tool(
                rmcp::model::CallToolRequestParams::new(name).with_arguments(
                    serde_json::json!({"path":"/proc/self/fd/0","contents":"replace input"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            ),
        );
        assert!(
            result.is_err() || result.unwrap().is_error == Some(true),
            "B2B0_MCP_NO_INPUT_FILE {name}"
        );
    }
}

#[test]
fn te15_b2b0_oversize_pump_never_launches_or_retries() {
    if !te15_fix_counting_child("te15_b2b0_oversize_pump_never_launches_or_retries") {
        return;
    }
    let (f, run) = te15_b2b0_notes_for(&vec!["q".repeat(16 * 1024); 17], "codex");
    let owner = f.host.owner.upgrade().unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let mut pump = te15_fix_pump(&f, &run, launches.clone());
    let first_result = pump.tick();
    let state = f.projection(&run);
    assert_eq!(
        launches.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "B2B0_PUMP_ZERO_LAUNCHES"
    );
    let cause = match &state.wait {
        RunWait::Provider(detail) | RunWait::ProviderSettings(detail) => String::from_utf8(
            owner
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(&detail.reason)
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
        _ => panic!(
            "B2B0_PUMP_REFUSAL tick={first_result:?} wait={:?}",
            state.wait
        ),
    };
    assert!(
        cause.contains(crate::job_guardian::protocol::INPUT_TOO_LONG),
        "B2B0_PUMP_TRUE_WORDS {cause}"
    );
    for _ in 0..3 {
        let _ = pump.tick();
    }
    assert_eq!(
        f.projection(&run).attempts.len(),
        state.attempts.len(),
        "B2B0_PUMP_NO_RETRY"
    );
    assert_eq!(
        launches.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "B2B0_PUMP_ZERO_LAUNCHES"
    );
    let accounting = owner
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap();
    assert!(
        accounting.tasks.iter().all(|t| t.sends.is_empty()),
        "B2B0_PUMP_NO_SPEND"
    );
}

fn te15_b2b0_guardian_bad_input() {
    use crate::{
        guardian::{CertifiedProfile, ContainedSession, DurableAttemptOwner},
        job_guardian::protocol::{AttemptIdentity, InputBinding, Program, ProviderSandbox},
    };
    struct Owner(usize);
    impl DurableAttemptOwner for Owner {
        fn reserve(&mut self, _: &AttemptIdentity) -> std::io::Result<()> {
            Ok(())
        }
        fn authorize_commit(
            &mut self,
            _: &crate::job_guardian::protocol::RecoveryIdentity,
        ) -> std::io::Result<()> {
            self.0 += 1;
            Ok(())
        }
    }
    for mode in ["digest", "truncated", "changed"] {
        let f = Fixture::new();
        let (_, execution, _) = reserve(&f);
        let _unit = NativeUnitCleanup::new(&execution);
        let mut binding = InputBinding::new(b"exact private input").unwrap();
        let file = crate::job_guardian::input::seal(b"exact private input", &binding).unwrap();
        let source = crate::job_guardian::input::source(&file);
        binding.source = Some(source.clone());
        match mode {
            "digest" => binding.sha256 = "0".repeat(64),
            "truncated" => binding.byte_length += 1,
            _ => {
                let path = f.temp.path("mutated-input");
                std::fs::write(&path, b"a changed input").unwrap();
                binding.source = Some(path);
            }
        }
        let program = Program {
            input: Some(binding),
            image: None,
            credential_root: None,
            executable: native_child(),
            arguments: vec![
                f.temp.root().to_string_lossy().into_owned(),
                "payload".into(),
            ],
            environment: std::collections::BTreeMap::default(),
            working_directory: f.temp.root().into(),
            sandbox: ProviderSandbox::ContainmentOnly,
            pipe_directory: None,
        };
        let identity = crate::native_launcher::native_identity(&execution);
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("bad-input-claim"),
            &identity.project,
            &identity.run,
        )
        .unwrap();
        let mut owner = Owner(0);
        let result = ContainedSession::prepare(
            &mut claim,
            &mut owner,
            identity,
            program,
            &kinewright_media::test_support::workspace_target_dir()
                .join("debug/kinewright-job-guardian"),
            &CertifiedProfile::fixture(),
        );
        assert!(result.is_err(), "B2B0_GUARDIAN_REFUSES_{mode}");
        assert_eq!(owner.0, 0, "B2B0_NO_RELEASE_{mode}");
        assert!(
            !f.temp.path("direct.json").exists(),
            "B2B0_ZERO_LAUNCHES_{mode}"
        );
        assert_eq!(claim.retained_attempts(), 0, "B2B0_REFUSAL_CLEANUP_{mode}");
        claim.finish().unwrap();
        drop(file);
        assert!(!source.exists(), "B2B0_REFUSED_INPUT_GONE");
    }
}
