// These witnesses drive authenticated MCP/Editor requests and durable records.
fn te14_fix1_rows(mcp: &C4Mcp, mut read: ChildRead) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    loop {
        let page =
            te14_outcome(mcp.wire("read_child_result", serde_json::to_value(&read).unwrap()));
        rows.extend(page["items"].as_array().unwrap().clone());
        read.cursor = page["next_cursor"].as_str().map(str::to_owned);
        if read.cursor.is_none() {
            break;
        }
    }
    rows
}
fn te14_fix1_wake(r: &C5Fix10Run, refs: &[&ObjectRef]) -> serde_json::Value {
    let state = r.f.projection(&r.run);
    serde_json::from_slice(
        &r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .context_result_wake(&state, &r.parent.task, refs)
            .expect("TE14_FIX1_WAKE_DELIVERED"),
    )
    .unwrap()
}
#[test]
fn te14_fix1_artifact_failure_matches_full_and_expanded_wire() {
    for padding in [0, 24_000] {
        let mut r = c5_fix10_run();
        let mut result = c5_fix10_freeze(&r.director, &r.child);
        let artifact = serde_json::json!({"kind":"task_proof_v1","revision":result.head,"proof_status":"failed","reason":"audio drift"});
        result.artifacts.push(te14_grant(&mut r.f, &artifact));
        result.evidence.push(te14_grant(
            &mut r.f,
            &serde_json::json!({"source":"evidence only"}),
        ));
        result.explanation = te14_grant(
            &mut r.f,
            &serde_json::json!({"message":"finished","padding":"x".repeat(padding)}),
        );
        r.director.accepted(
            "publish_result",
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: r.child.clone(),
                result: result.clone(),
            })),
        );
        let reference = r.f.projection(&r.run).results.last().unwrap().clone();
        let wake = te14_fix1_wake(&r, &[&reference]);
        if padding == 0 {
            assert_eq!(wake["mode"], "full", "TE14_FIX1_FULL_RECORDS");
            assert!(
                wake.get("disclosure").is_none()
                    && wake.get("results").is_none()
                    && wake.get("omitted_changes").is_none(),
                "TE14_FIX1_FULL_NO_DUPLICATE"
            );
            assert_eq!(
                wake["result_refs"][0],
                serde_json::json!({"child":r.child,"reference":reference}),
                "TE14_FIX1_FULL_ROUTE"
            );
            assert!(
                serde_json::to_string(&wake)
                    .unwrap()
                    .contains("audio drift"),
                "TE14_FIX1_FULL_ARTIFACT_REJECT"
            );
        } else {
            assert_eq!(wake["mode"], "summary");
            assert_eq!(wake["proof_status"], "unknown", "TE14_FIX1_PROOF_UNKNOWN");
            let rows = te14_fix1_rows(
                &r.director,
                ChildRead {
                    run: r.run.clone(),
                    child: r.child.clone(),
                    reference,
                    source: result.head,
                    cursor: None,
                },
            );
            let contents = rows
                .iter()
                .filter(|row| row["kind"] == "artifact")
                .map(|row| row["chunk"]["text"].as_str().unwrap())
                .collect::<String>();
            let proof: serde_json::Value = serde_json::from_str(&contents).unwrap();
            assert_eq!(
                proof["proof_status"], "failed",
                "TE14_FIX1_ARTIFACT_REJECTION"
            );
            assert_eq!(proof, artifact, "TE14_FIX1_ARTIFACT_EXACT");
            let evidence = rows
                .iter()
                .filter(|row| row["kind"] == "evidence")
                .map(|row| row["chunk"]["text"].as_str().unwrap())
                .collect::<String>();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&evidence).unwrap()["source"],
                "evidence only",
                "TE14_FIX1_EVIDENCE_CONTENTS"
            );
        }
    }
}
#[test]
fn te14_fix1_shaped_explanation_cannot_certify_a_pass() {
    let (r, reference, _) = te14_published_report(false, false, 24_000);
    let wake = te14_fix1_wake(&r, &[&reference]);
    assert_eq!(
        wake["proof_status"], "unknown",
        "TE14_FIX1_UNTRUSTED_PASS_UNKNOWN"
    );
    assert_eq!(wake["note_fate"], "unknown", "TE14_FIX1_NOTES_UNKNOWN");
    assert!(
        wake["results"][0].get("disclosure").is_none(),
        "TE14_FIX1_NO_CHILD_FACTS"
    );
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn te14_fix1_many_results_always_deliver_a_bounded_wake() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    // Real, admitted references can repeat across simultaneous results. Their
    // number must not turn a result event into an undeliverable wake.
    let artifact = te14_grant(
        &mut r.f,
        &serde_json::json!({"proof_status":"failed","padding":"x".repeat(24_000)}),
    );
    result.artifacts = vec![artifact];
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    let mut references = vec![r.f.projection(&r.run).results.last().unwrap().clone()];
    let owner = r.f.host.owner.upgrade().unwrap();
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &r.f.temp.path("te14-many-cleanup"),
        r.parent.project.as_str(),
        r.run.0.as_str(),
    )
    .unwrap();
    let producer = r.f.producer(&r.run);
    crate::effect_pump::reconcile_attempt(&owner, &mut claim, &producer, &r.attempt).unwrap();
    for _ in 1..7 {
        let mut brief = r.f.brief.clone();
        brief.role = Role::Editor;
        r.director.accepted(
            "assign_task",
            OrchestrationCommand::AssignTask(Box::new(AssignTask {
                run: r.run.clone(),
                parent: r.parent.task.clone(),
                brief,
            })),
        );
        let child = r.f.projection(&r.run).tasks.last().unwrap().id.clone();
        let (execution, client, endpoint, _) = c4_reserve_task(&r.f, &r.run, &child);
        history_turn(&r.f, &execution, &client, &endpoint, 1, true);
        let mut result = c5_fix10_freeze(&r.director, &child);
        result.artifacts = vec![r.f.brief.plan.clone()];
        r.director.accepted(
            "publish_result",
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: child,
                result,
            })),
        );
        references.push(r.f.projection(&r.run).results.last().unwrap().clone());
        crate::effect_pump::reconcile_attempt(&owner, &mut claim, &producer, &execution).unwrap();
    }
    let wake = te14_fix1_wake(&r, &references.iter().collect::<Vec<_>>());
    assert!(
        serde_json::to_vec(&wake).unwrap().len() <= 12 * 1024,
        "TE14_FIX1_WAKE_ALWAYS_FITS"
    );
    assert_eq!(wake["result_count"], 7, "TE14_FIX1_WAKE_COUNT");
    assert_eq!(wake["decision"], "expand_before_accepting");
}
#[test]
fn te14_fix1_child_proof_uses_durable_source_not_parent_clock() {
    for mode in ["current", "stale", "missing"] {
        let mut r = c5_fix10_run();
        assert!(grant_test_revision(&r.core).0 > 0);
        let mut result = c5_fix10_freeze(&r.director, &r.child);
        let mut proof = serde_json::json!({"kind":"task_proof_v1","revision":if mode=="stale"{result.base.clone()}else{result.head.clone()},"proof_status":"failed"});
        if mode == "missing" {
            proof.as_object_mut().unwrap().remove("revision");
        }
        let reference = te14_grant(&mut r.f, &proof);
        result.artifacts.push(reference.clone());
        r.director.accepted(
            "publish_result",
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: r.child.clone(),
                result,
            })),
        );
        let reply = te14_read(
            &r.director,
            kinewright_core::TimelineRevision(0),
            EvidenceKind::Proof { reference },
            None,
        );
        assert_eq!(
            reply.is_error,
            Some(mode != "current"),
            "TE14_FIX1_CHILD_PROOF_SOURCE {mode} {reply:?}"
        );
    }
}
#[test]
fn te14_fix1_lost_page_reply_retries_without_a_write() {
    let mut f = Fixture::new();
    let reference = te14_grant(&mut f, &serde_json::json!({"text":"x".repeat(60_000)}));
    f.brief.script = reference.clone();
    let (run, parent, mcp) = c4_director(&f);
    let gate = std::sync::Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .hooks
        .context_reply_gate = Some(super::super::orchestration::ContextReplyGate {
        gate: gate.clone(),
        outcome: observed.clone(),
    });
    let arguments = serde_json::to_value(EvidenceRead {
        revision: kinewright_core::TimelineRevision(0),
        evidence: EvidenceKind::Reference {
            reference: reference.clone(),
        },
        cursor: None,
    })
    .unwrap();
    let lost = std::thread::scope(|scope| {
        let first=scope.spawn(||mcp.runtime.block_on(async {
            tokio::select! {
                reply=mcp.client.call_tool(rmcp::model::CallToolRequestParams::new("read_task_evidence").with_arguments(arguments.as_object().unwrap().clone()))=>Some(reply),
                ()=async {while !cancelled.load(std::sync::atomic::Ordering::Acquire) {tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}=>None,
            }
        }));
        gate.0.wait(); // actual durable page landed, reply still withheld
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !first.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        gate.1.wait();
        first.join().unwrap()
    });
    assert!(lost.is_none(), "TE14_FIX1_REPLY_ACTUALLY_LOST");
    let first = observed.lock().unwrap().take().unwrap();
    let before = f.projection(&run);
    let reconnect = C4Mcp::new(&f, &run, &parent.task);
    let again = te14_outcome(te14_read(
        &reconnect,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: reference.clone(),
        },
        None,
    ));
    assert_eq!(first, again, "TE14_FIX1_LOST_REPLY_IDENTICAL");
    let after = f.projection(&run);
    assert_eq!(before, after, "TE14_FIX1_RETRY_NO_WRITE");
    let next = te14_outcome(te14_read(
        &reconnect,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference { reference },
        first["next_cursor"].as_str().map(str::to_owned),
    ));
    assert_ne!(
        next["page_id"], first["page_id"],
        "TE14_FIX1_CURSOR_IDENTITY"
    );
    let state = f.projection(&run);
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    assert_eq!(
        host.expansion_count(&state.tasks[0]).unwrap(),
        2,
        "TE14_FIX1_DISTINCT_PAGES"
    );
    let digests = state.tasks[0]
        .progress
        .decisions
        .iter()
        .filter_map(|reference| {
            host.owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(reference)
                .ok()
        })
        .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|value| value["kind"] == "task_evidence_expansion_v1")
        .map(|value| value["request_sha256"].clone())
        .collect::<Vec<_>>();
    assert_eq!(digests.len(), 2);
    assert_ne!(digests[0], digests[1], "TE14_FIX1_AUDIT_BINDS_CURSOR");
}

// Never assert while a gate is held: mutants must release and join their
// workers before failing their named behavioural assertion.
fn te14_fix1_media_control(cold: bool, switch: bool, timeout: bool) {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicU8, Ordering},
    };
    use std::time::{Duration, Instant};
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let entered = Arc::new(AtomicBool::new(false));
    let unexpected_cold = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(AtomicU8::new(0));
    let (probe, endpoint, target) = {
        let mut host = owner.lock().unwrap();
        let endpoint = host.hooks.runs[&run].tasks[&child].endpoint();
        if switch {
            host.acquire_manual("e".repeat(64)).unwrap();
        }
        let project = host.owner_mut().unwrap();
        let mut branch = project
            .fork_durable_branch(
                "TE14 real switch",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        project.persist_durable_branch(&mut branch).unwrap();
        let target = branch.branch_id().clone();
        project.close_durable_branch(&branch).unwrap();
        let store = project.store().unwrap();
        let probe = store.pin(&[]).unwrap();
        if cold {
            store.clear_media_memo_for_fixture().unwrap();
            let unexpected_cold = unexpected_cold.clone();
            store
                .hold_next_cold_media_for_fixture(Arc::new(move || {
                    unexpected_cold.store(true, Ordering::Release);
                }))
                .unwrap();
        }
        host.hooks.context_media_gate = Some(gate.clone());
        entered.store(true, Ordering::Release);
        if timeout {
            host.hooks.context_media_wait = Some(Duration::from_millis(300));
        }
        host.hooks.context_media_outcome = Some(outcome.clone());
        (probe, endpoint, target)
    };
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    let started = Instant::now();
    let (control_done, read_done, reply, control, held_pins) = std::thread::scope(|scope| {
        let read = scope.spawn(|| {
            mcp.runtime.block_on(
                mcp.client.call_tool(
                    rmcp::model::CallToolRequestParams::new("read_task_evidence").with_arguments(
                        serde_json::to_value(EvidenceRead {
                            revision: grant_test_revision(&core),
                            evidence: EvidenceKind::Frame {
                                clip: kinewright_core::ClipId(3),
                                source_frame: 0,
                            },
                            cursor: None,
                        })
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                ),
            )
        });
        if cold {
            let limit = Instant::now() + Duration::from_secs(10);
            while !entered.load(Ordering::Acquire) && !read.is_finished() && Instant::now() < limit
            {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        gate.0.wait();
        assert!(
            !unexpected_cold.load(Ordering::Acquire),
            "TE14_FIX1_READ_REUSES_CERTIFICATE"
        );
        let control = scope.spawn(|| {
            if timeout {
                return None;
            }
            if switch {
                Fixture::issue(&f.editor);
                let editor = f.editor.with_manual_editor("e".repeat(64));
                let prepared = editor
                    .request(HostCommand::PrepareSwitch {
                        target: kinewright_project::bundle_switch::SwitchTarget::WorkingBranch {
                            branch: target,
                        },
                    })
                    .unwrap();
                if prepared.error.is_some() {
                    return Some(prepared);
                }
                let installed = editor
                    .request(HostCommand::Switch {
                        envelope: Box::new(prepared.switch_envelope.unwrap()),
                    })
                    .unwrap();
                if installed.error.is_some() {
                    return Some(installed);
                }
                let confirmed = editor
                    .request(HostCommand::ConfirmSwitch {
                        client: "e".repeat(64),
                        installed: Box::new(
                            installed
                                .switch_attachment
                                .as_ref()
                                .unwrap()
                                .identity()
                                .unwrap(),
                        ),
                    })
                    .unwrap();
                if confirmed.error.is_some() {
                    return Some(confirmed);
                }
                assert!(
                    confirmed.manual_lease.is_some(),
                    "TE14_FIX1_SWITCH_CONFIRMED"
                );
                Some(installed)
            } else {
                Fixture::issue(&f.editor);
                let request = f.control(&f.editor, &run, RunAction::Stop);
                Some(call(
                    &f,
                    &f.editor,
                    HostCommand::DispatchOrchestration {
                        request: Box::new(request),
                    },
                    0,
                ))
            }
        });
        let limit = Instant::now() + Duration::from_secs(if timeout { 7 } else { 2 });
        while (!read.is_finished() || !control.is_finished()) && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(5));
        }
        let control_done = control.is_finished();
        let read_done = read.is_finished();
        let held = probe.retention_probe_for_fixture().unwrap().1;
        gate.1.wait();
        (
            control_done,
            read_done,
            read.join().unwrap(),
            control.join().unwrap(),
            held,
        )
    });
    let wait = Instant::now() + Duration::from_secs(10);
    while endpoint.media_read_pending_for_fixture() && Instant::now() < wait {
        std::thread::sleep(Duration::from_millis(5));
    }
    let after_pins = probe.retention_probe_for_fixture().unwrap().1;
    assert!(
        control_done,
        "TE14_FIX1_MEDIA_CONTROL_NOT_BLOCKED cold={cold} switch={switch}"
    );
    assert!(
        read_done,
        "TE14_FIX1_MEDIA_CANCEL_OR_DEADLINE cold={cold} switch={switch} control={control:?}"
    );
    assert!(
        reply.is_err() || reply.as_ref().unwrap().is_error == Some(true),
        "TE14_FIX1_CANCELLED_NO_DATA {reply:?}"
    );
    if let Some(control) = control {
        assert!(
            control.error.is_none(),
            "TE14_FIX1_REAL_CONTROL {control:?}"
        );
        if switch {
            assert!(
                control.switch_attachment.is_some(),
                "TE14_FIX1_REAL_SWITCH_INSTALLED"
            );
        }
    }
    assert_eq!(
        outcome.load(Ordering::Acquire),
        if timeout { 1 } else { 2 },
        "TE14_FIX1_DECODER_CONTINUES_OR_CANCELS"
    );
    assert!(
        !endpoint.media_read_pending_for_fixture(),
        "TE14_FIX1_WORKER_RELEASED"
    );
    assert!(
        held_pins
            .iter()
            .all(|(key, held)| *held >= after_pins.get(key).copied().unwrap_or(0)),
        "TE14_FIX1_RETAINED_SOURCE_PINS_DO_NOT_GROW"
    );
    // Switching can certify its newly installed inputs after the read was cancelled.
    if !switch {
        assert!(!unexpected_cold.load(Ordering::Acquire));
    }
    assert_eq!(
        f.projection(&run).tasks[1].progress.decisions.len(),
        before,
        "TE14_FIX1_REFUSED_NO_AUDIT"
    );
    println!(
        "TE14_FIX1_MEDIA_CONTROL cold={cold} switch={switch} timeout={timeout} elapsed={:.3}s",
        started.elapsed().as_secs_f64()
    );
}
#[test]
fn te14_fix1_retained_certificate_stop_after_memo_eviction() {
    te14_fix1_media_control(true, false, false);
}
#[test]
fn te14_fix1_retained_certificate_switch_after_memo_eviction() {
    te14_fix1_media_control(true, true, false);
}
#[test]
fn te14_fix1_decode_stop() {
    te14_fix1_media_control(false, false, false);
}
#[test]
fn te14_fix1_decode_switch() {
    te14_fix1_media_control(false, true, false);
}
#[test]
fn te14_fix1_media_wall_clock_limit() {
    te14_fix1_media_control(true, false, true);
}
fn te14_fix1_notes_prompt() -> Result<Vec<u8>, HostError> {
    use kinewright_project::studio_records::{CommentId, FeedbackRevision};
    let f = Fixture::new();
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
    for n in 0..6 {
        let mut revision = template.clone();
        revision.comment_id = CommentId::fresh().unwrap();
        let text = format!("person note {n}: keep the original wording");
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
    let kinewright_project::studio_records::ReviseResult::Accepted { run_id, .. } =
        revise_result(revise(&f, &f.editor, &request), "TE14_FIX1_NOTES_REVISE")
    else {
        panic!("accepted")
    };
    let state = f.projection(&run_of(&run_id));
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    host.codex_prompt(&state.tasks[0], b"inspect", &te12_fixed_context())
}
#[test]
fn te14_fix1_note_capacity_is_unchanged() {
    let mut f = Fixture::new();
    let reference = te14_grant(
        &mut f,
        &serde_json::json!("x".repeat(crate::protocol::limits::FRAME_BYTES / 2 + 1)),
    );
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    assert!(
        host.prompt_note_text(&reference).is_err(),
        "TE14_FIX1_NOTE_CAPACITY"
    );
}
#[test]
fn te14_fix1_six_original_notes_are_inline_once() {
    let prompt = te14_fix1_notes_prompt().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&prompt).unwrap();
    assert_eq!(
        value["data"]["notes"].as_array().unwrap().len(),
        6,
        "TE14_FIX1_SIX_INLINE_NOTES"
    );
    for n in 0..6 {
        let text = format!("person note {n}: keep the original wording");
        assert_eq!(
            String::from_utf8_lossy(&prompt).matches(&text).count(),
            1,
            "TE14_FIX1_NOTE_TEXT_ONCE"
        );
    }
    assert!(
        value["data"]["overview"].get("note_anchors").is_none(),
        "TE14_FIX1_NO_NOTE_DUPLICATION"
    );
}
#[test]
fn te14_fix1_usage_reads_hide_host_observers() {
    use kinewright_project::{
        orchestration_accounting::AccountingRecord, usage_measurement::Measurements,
    };
    let r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let accounting = {
        let owner = r.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let mut record = AccountingRecord::new(
            host.owner().unwrap().project_id().unwrap(),
            serde_json::from_value(serde_json::to_value(&r.run).unwrap()).unwrap(),
            1000,
        )
        .unwrap();
        record
            .add_task(
                serde_json::from_value(serde_json::to_value(&r.child).unwrap()).unwrap(),
                None,
            )
            .unwrap();
        record.tasks[0].measurements = Some(Measurements::default());
        host.owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(record)),
            })
            .unwrap()
            .reference
    };
    result.artifacts.push(accounting.clone());
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let value = te14_outcome(te14_read(
        &r.director,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: accounting,
        },
        None,
    ));
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("measurements"),
        "TE14_FIX1_REFERENCE_USAGE_PROJECTED"
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    let rows = te14_fix1_rows(
        &r.director,
        ChildRead {
            run: r.run.clone(),
            child: r.child.clone(),
            reference,
            source: result.head,
            cursor: None,
        },
    );
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("measurements"),
        "TE14_FIX1_CHILD_USAGE_PROJECTED"
    );
}
#[test]
fn te14_fix1_retry_after_host_restart_is_free() {
    let mut f = Fixture::new();
    let reference = te14_grant(&mut f, &serde_json::json!({"text":"x".repeat(50_000)}));
    f.brief.script = reference.clone();
    let (run, parent, mcp) = c4_director(&f);
    let first = te14_outcome(te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: reference.clone(),
        },
        None,
    ));
    drop(mcp);
    f.host.shutdown().unwrap();
    let host = crash_host(f.temp.root());
    let mcp = crash_director(&host, &run, &parent.task);
    let before = {
        let owner = host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        host.owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap()
    };
    let revision = {
        let owner = host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        grant_test_revision(host.hooks.runs[&run].tasks[&parent.task].core())
    };
    let retry = te14_outcome(te14_read(
        &mcp,
        revision,
        EvidenceKind::Reference { reference },
        None,
    ));
    assert_eq!(retry, first, "TE14_FIX1_RESTART_REPLY_IDENTICAL");
    let owner = host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    let after = host
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    assert_eq!(before, after, "TE14_FIX1_RESTART_NO_WRITE");
    assert_eq!(
        host.expansion_count(&after.tasks[0]).unwrap(),
        1,
        "TE14_FIX1_RESTART_COUNT"
    );
}
#[test]
fn te14_fix1_measure_persisted_cold_certification() {
    use kinewright_project::owned_inputs::OwnedInputRef;
    let f = Fixture::new();
    let media = kinewright_media::test_support::GeneratedMedia::ffmpeg(
        "te14-cold-budget",
        &[
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=d=60:r=48000",
            "-ac",
            "2",
            "-c:a",
            "pcm_f32le",
        ],
        "wav",
    );
    let bytes = std::fs::metadata(media.path()).unwrap().len();
    let owner = f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let project = host.owner_mut().unwrap();
    project.import_owned_media(media.path()).unwrap();
    project.save_active().unwrap();
    let manifest_ref = project.current_inputs_ref().clone();
    let store = project.store().unwrap();
    let manifest = store.read_input_manifest(&manifest_ref).unwrap();
    let identity = OwnedInputRef {
        manifest_ref,
        entry: manifest.entries.last().unwrap().clone(),
    };
    for _ in 0..3 {
        store.clear_media_memo_for_fixture().unwrap();
        let request = store.input_lease_request(&identity).unwrap();
        let started = std::time::Instant::now();
        let lease = request.open().unwrap();
        let source = lease.certified_source().unwrap();
        let mut decoder = source
            .open_audio(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )))
            .unwrap();
        decoder.seek(0).unwrap();
        assert!(decoder.next_frame().unwrap().is_some());
        println!(
            "TE14_FIX1_COLD_BUDGET bytes={bytes} seconds={:.6}",
            started.elapsed().as_secs_f64()
        );
    }
}
#[test]
fn te14_fix1_legacy_limit_names_the_paged_route() {
    let r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    result.artifacts = vec![r.f.brief.plan.clone(); 256];
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    let reply = r.director.wire(
        "read_task_result",
        serde_json::json!({"reference":reference}),
    );
    let text = serde_json::to_string(&reply).unwrap();
    assert!(
        reply.is_error == Some(true)
            && text.contains("one-shot 20 KiB limit")
            && text.contains("read_child_result")
            && !text.contains("exhausted"),
        "TE14_FIX1_LEGACY_LIMIT_TRUE {text}"
    );
}

#[test]
fn te14_fix1_child_expansion_preserves_original_owned_bytes() {
    use base64::Engine as _;
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let raw_json = b"{\n \"proof_status\": \"failed\", \"proof_status\": \"passed\" \n}\n";
    let binary = &[0u8, 255, 13, 10, 0];
    let references = {
        let owner = r.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let project = host.owner_mut().unwrap();
        vec![
            project.admit_host_grant(raw_json).unwrap(),
            project.admit_host_grant(binary).unwrap(),
        ]
    };
    result.artifacts = references.clone();
    result.explanation = te14_grant(&mut r.f, &serde_json::json!({"padding":"x".repeat(24_000)}));
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    let full: serde_json::Value = {
        let owner = r.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        serde_json::from_slice(&host.result_wake_payload(&[&reference]).unwrap()).unwrap()
    };
    assert_eq!(
        full[0]["artifacts"][0].as_str().unwrap().as_bytes(),
        raw_json,
        "TE14_FIX1_FULL_RAW_RECORD"
    );
    assert_eq!(
        full[0]["artifacts"][1]["contents"],
        serde_json::json!(binary),
        "TE14_FIX1_FULL_BINARY_RECORD"
    );
    assert_eq!(te14_fix1_wake(&r, &[&reference])["mode"], "summary");
    let rows = te14_fix1_rows(
        &r.director,
        ChildRead {
            run: r.run.clone(),
            child: r.child.clone(),
            reference,
            source: result.head,
            cursor: None,
        },
    );
    for (reference, expected) in references
        .iter()
        .zip([raw_json.as_slice(), binary.as_slice()])
    {
        let mut actual = Vec::new();
        for row in rows.iter().filter(|row| {
            row["kind"] == "artifact"
                && row["reference"] == serde_json::to_value(reference).unwrap()
        }) {
            let chunk = &row["chunk"];
            if chunk["encoding"] == "utf8" {
                actual.extend_from_slice(chunk["text"].as_str().unwrap().as_bytes());
            } else {
                actual.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk["bytes"].as_str().unwrap())
                        .unwrap(),
                );
            }
        }
        assert_eq!(actual, expected, "TE14_FIX1_RAW_RECORD_EXACT");
    }
}
