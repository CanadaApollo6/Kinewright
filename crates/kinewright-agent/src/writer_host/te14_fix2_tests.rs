fn te14_fix2_raw_grant(f: &mut Fixture, bytes: &[u8]) -> ObjectRef {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(bytes)
        .unwrap()
}
fn te14_fix2_publish(mode: &str) -> (C5Fix10Run, ObjectRef, TaskResult) {
    let mut r = c5_fix10_run();
    if matches!(mode, "bulk" | "oversized") {
        for n in 0..1 {
            let revision = grant_test_revision(&r.core);
            let operation = kinewright_core::Operation::SetMarkerParam {
                marker: kinewright_core::MarkerId(1),
                name: "label".into(),
                value: kinewright_core::ParamValue::Text(format!(
                    "{n}{}",
                    "x".repeat(if mode == "bulk" { 15_000 } else { 5_000 })
                )),
            };
            let prepared=te14_outcome(r.editor.wire("prepare_edit_plan",serde_json::json!({"expected_revision":revision,"operations":[operation.clone(),operation.clone(),operation]})));
            te14_outcome(r.editor.wire(
                "commit_edit_plan",
                serde_json::json!({"expected_revision":revision,"plan_id":prepared["plan_id"]}),
            ));
        }
    }
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    if mode == "bulk" {
        assert!(
            result.delta_ref.as_ref().unwrap().byte_length > 40_000,
            "TE14_FIX2_REAL_FROZEN_DELTA"
        );
        result.explanation = te14_fix2_raw_grant(&mut r.f, &vec![b'x'; 150 * 1024]);
    }
    if mode == "oversized" {
        result
            .artifacts
            .push(te14_fix2_raw_grant(&mut r.f, &vec![b'x'; 210_000]));
    }
    if mode == "many" {
        for n in 0..16 {
            result.artifacts.push(te14_grant(
                &mut r.f,
                &serde_json::json!({"n":n,"body":"x".repeat(12_000)}),
            ));
        }
    }
    result.artifacts.push(te14_grant(&mut r.f,&serde_json::json!({"kind":"task_proof_v1","revision":result.head,"proof_status":"failed","reason":"small rejection"})));
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    (r, reference, result)
}
#[test]
fn te14_fix2_rejection_precedes_bulk_records() {
    let (r, reference, result) = te14_fix2_publish("bulk");
    let page = te14_outcome(
        r.director.wire(
            "read_child_result",
            serde_json::to_value(ChildRead {
                run: r.run.clone(),
                child: r.child.clone(),
                reference,
                source: result.head,
                cursor: None,
            })
            .unwrap(),
        ),
    );
    assert!(
        serde_json::to_string(&page["items"])
            .unwrap()
            .contains("small rejection"),
        "TE14_FIX2_PROOF_FIRST"
    );
    assert_eq!(page["partial_coverage"], true, "TE14_FIX2_PARTIAL_BULK");
}
#[test]
fn te14_fix2_oversized_record_does_not_poison_child() {
    let (r, reference, result) = te14_fix2_publish("oversized");
    let reply = r.director.wire(
        "read_child_result",
        serde_json::to_value(ChildRead {
            run: r.run.clone(),
            child: r.child.clone(),
            reference: reference.clone(),
            source: result.head.clone(),
            cursor: None,
        })
        .unwrap(),
    );
    assert_eq!(reply.is_error, Some(false), "TE14_FIX2_OVERSIZE_CONTINUES");
    let page = te14_outcome(reply);
    let text = serde_json::to_string(&page).unwrap();
    assert!(
        text.contains("small rejection"),
        "TE14_FIX2_OVERSIZE_CONTINUES"
    );
    let omission = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["omitted"] == true)
        .expect("TE14_FIX2_OMISSION_ROW");
    assert_eq!(
        omission["reference"],
        serde_json::to_value(&result.artifacts[0]).unwrap()
    );
    assert_eq!(omission["byte_length"], 210_000);
    assert_eq!(
        omission["acceptance"], "cannot_accept_from_this_read",
        "TE14_FIX2_OMITTED_PROOF_UNKNOWN"
    );
    assert_eq!(page["omitted"], true, "TE15_OMISSION_FIXTURE_SPILLS");
    let mut last = page.clone();
    while let Some(cursor) = last["next_cursor"].as_str() {
        last = te14_outcome(
            r.director.wire(
                "read_child_result",
                serde_json::to_value(ChildRead {
                    run: r.run.clone(),
                    child: r.child.clone(),
                    reference: reference.clone(),
                    source: result.head.clone(),
                    cursor: Some(cursor.into()),
                })
                .unwrap(),
            ),
        );
    }
    assert_eq!(last["omitted"], false, "TE15_OMISSION_LAST_PAGE");
    assert_eq!(last["partial_coverage"], true, "TE14_FIX2_PARTIAL_OMISSION");
    let legacy = r.director.wire(
        "read_task_result",
        serde_json::json!({"reference":result.artifacts[0]}),
    );
    assert_eq!(legacy.is_error, Some(true));
    let direct = te14_read(
        &r.director,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference {
            reference: result.artifacts[0].clone(),
        },
        None,
    );
    assert_eq!(direct.is_error, Some(true));
    assert!(
        !serde_json::to_string(&direct)
            .unwrap()
            .contains("capacity exhausted"),
        "TE14_FIX2_RECORD_LIMIT_WORDS"
    );
}
#[test]
fn te14_fix2_many_artifacts_remain_unknown_at_budget() {
    let (r, reference, result) = te14_fix2_publish("many");
    let mut request = ChildRead {
        run: r.run.clone(),
        child: r.child.clone(),
        reference,
        source: result.head,
        cursor: None,
    };
    for _ in 0..8 {
        let page = te14_outcome(
            r.director
                .wire("read_child_result", serde_json::to_value(&request).unwrap()),
        );
        assert_eq!(
            page["partial_coverage"], true,
            "TE14_FIX2_UNREAD_ARTIFACTS_UNKNOWN"
        );
        assert!(
            !serde_json::to_string(&page["items"])
                .unwrap()
                .contains("small rejection"),
            "TE14_FIX2_UNREAD_ARTIFACTS_UNKNOWN"
        );
        request.cursor = Some(page["next_cursor"].as_str().unwrap().into());
    }
    let reply = r
        .director
        .wire("read_child_result", serde_json::to_value(request).unwrap());
    assert_eq!(reply.is_error, Some(true), "TE14_FIX2_SHARED_BUDGET");
}
#[test]
fn te14_fix2_all_reference_routes_preserve_bytes_or_refuse() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let raw = format!(
        "{{\"kind\":\"task_proof_v1\",\"revision\":{},\"proof_status\":\"failed\",\"proof_status\":\"passed\"}}",
        serde_json::to_string(&result.head).unwrap()
    );
    let artifact = te14_fix2_raw_grant(&mut r.f, raw.as_bytes());
    result.artifacts.push(artifact.clone());
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    for kind in [
        EvidenceKind::Reference {
            reference: artifact.clone(),
        },
        EvidenceKind::Proof {
            reference: artifact.clone(),
        },
        EvidenceKind::Preview {
            reference: artifact.clone(),
        },
        EvidenceKind::Transcript {
            reference: artifact.clone(),
            start: 0,
            end: 1,
        },
    ] {
        let classified = !matches!(kind, EvidenceKind::Reference { .. });
        let reply = te14_read(
            &r.director,
            kinewright_core::TimelineRevision(0),
            kind,
            None,
        );
        if classified {
            assert_eq!(
                reply.is_error,
                Some(true),
                "TE14_FIX2_AMBIGUOUS_CLASSIFICATION {reply:?}"
            );
        } else {
            let page = te14_outcome(reply);
            let bytes = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    v["chunk"]["text"]
                        .as_str()
                        .expect("TE14_FIX2_ORIGINAL_REFERENCE_BYTES")
                })
                .collect::<String>();
            assert_eq!(bytes, raw, "TE14_FIX2_ORIGINAL_REFERENCE_BYTES");
        }
    }
    let rows = te14_fix1_rows(
        &r.director,
        ChildRead {
            run: r.run.clone(),
            child: r.child.clone(),
            reference: reference.clone(),
            source: result.head,
            cursor: None,
        },
    );
    assert_eq!(
        rows.iter()
            .filter(|v| v["kind"] == "artifact")
            .map(|v| v["chunk"]["text"].as_str().unwrap())
            .collect::<String>(),
        raw
    );
    assert_eq!(
        r.director
            .wire(
                "read_task_result",
                serde_json::json!({"reference":reference})
            )
            .is_error,
        Some(false),
        "TE14_FIX2_LEGACY_STILL_WORKS"
    );
}
#[test]
fn te14_fix2_classification_is_host_owned() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let reference = te14_grant(
        &mut r.f,
        &serde_json::json!({"kind":"ordinary","revision":result.head}),
    );
    result.artifacts.push(reference.clone());
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    for kind in [
        EvidenceKind::Proof {
            reference: reference.clone(),
        },
        EvidenceKind::Preview {
            reference: reference.clone(),
        },
    ] {
        let reply = te14_read(
            &r.director,
            kinewright_core::TimelineRevision(0),
            kind,
            None,
        );
        assert_eq!(
            reply.is_error,
            Some(true),
            "TE14_CLASSIFICATION_IS_HOST_OWNED {reply:?}"
        );
        assert!(
            serde_json::to_string(&reply)
                .unwrap()
                .contains("not a classified"),
            "TE14_CLASSIFICATION_IS_HOST_OWNED"
        );
    }
}
fn te14_fix2_media_wait(cold: bool, action: u8) {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let unexpected_cold = Arc::new(AtomicBool::new(false));
    let endpoint = {
        let mut host = owner.lock().unwrap();
        host.hooks.context_media_wait = Some(if action == 0 {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(30)
        });
        host.hooks.context_media_prepare_delay = Some(Duration::from_millis(80));
        let endpoint = host.hooks.runs[&run].tasks[&child].endpoint();
        host.hooks.context_media_gate = Some(gate.clone());
        if cold {
            let store = host.owner().unwrap().store().unwrap();
            store.clear_media_memo_for_fixture().unwrap();
            let unexpected_cold = unexpected_cold.clone();
            store
                .hold_next_cold_media_for_fixture(Arc::new(move || {
                    unexpected_cold.store(true, Ordering::Release);
                }))
                .unwrap();
        }
        endpoint
    };
    let request = serde_json::to_value(EvidenceRead {
        revision: grant_test_revision(&core),
        evidence: EvidenceKind::Frame {
            clip: kinewright_core::ClipId(3),
            source_frame: 0,
        },
        cursor: None,
    })
    .unwrap();
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    let (timely, first, busy, during) = std::thread::scope(|scope| {
        let read = scope.spawn(|| mcp.wire("read_task_evidence", request.clone()));
        gate.0.wait();
        let control = scope.spawn(|| {
            if action == 0 {
                return None;
            }
            Fixture::issue(&f.editor);
            let request = f.control(
                &f.editor,
                &run,
                if action == 1 {
                    RunAction::Stop
                } else {
                    RunAction::Pause
                },
            );
            Some(call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                },
                0,
            ))
        });
        let until = Instant::now() + Duration::from_secs(2);
        while (!read.is_finished() || !control.is_finished()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let timely = read.is_finished() && control.is_finished();
        let busy = scope.spawn(|| {
            if action == 0 {
                Some(mcp.wire("read_task_evidence", request.clone()))
            } else {
                None
            }
        });
        let until = Instant::now() + Duration::from_secs(2);
        while !busy.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let timely = timely && busy.is_finished();
        gate.1.wait();
        let first = read.join().unwrap();
        let busy = busy.join().unwrap();
        let control = control.join().unwrap();
        if let Some(control) = control {
            assert!(control.error.is_none());
        }
        let during = f.projection(&run).tasks[1].progress.decisions.len();
        (timely, first, busy, during)
    });
    let until = Instant::now() + Duration::from_secs(5);
    while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(timely, "TE14_FIX2_MEDIA_WAIT_ENDS");
    assert_eq!(during, before, "TE14_FIX2_MEDIA_REFUSAL_FREE");
    assert_eq!(first.is_error, Some(true));
    let words = serde_json::to_string(&first).unwrap();
    if action == 0 {
        assert!(
            words.contains("Media source verification or an earlier read is still running. Retry shortly; no page is charged until a sample is returned."),
            "TE14_FIX2_MEDIA_WORDS {words}"
        );
        assert!(
            serde_json::to_string(&busy.unwrap())
                .unwrap()
                .contains("Media source verification or an earlier read is still running. Retry shortly; no page is charged until a sample is returned."),
            "TE14_FIX2_BUSY_RETRY"
        );
        {
            let sample = te14_outcome(mcp.wire("read_task_evidence", request));
            assert!(
                !sample["items"].as_array().unwrap().is_empty(),
                "TE14_FIX2_EVENTUAL_SAMPLE"
            );
            assert_eq!(
                f.projection(&run).tasks[1].progress.decisions.len(),
                before + 1,
                "TE14_FIX2_ONE_MEDIA_PAGE"
            );
        }
    } else {
        assert!(words.contains("no longer live"), "TE14_FIX2_REVOKED_WORDS");
    }
    assert!(!unexpected_cold.load(Ordering::Acquire));
}
#[test]
fn te14_fix2_verification_wait_retry_then_sample() {
    te14_fix2_media_wait(true, 0);
}
#[test]
fn te14_fix2_decode_overrun_reuses_pending_read() {
    te14_fix2_media_wait(false, 0);
}
#[test]
fn te14_fix2_stop_ends_wait() {
    te14_fix2_media_wait(true, 1);
}
#[test]
fn te14_fix2_pause_ends_wait() {
    te14_fix2_media_wait(true, 2);
}

fn te14_fix2_notes_fixture() -> Fixture {
    use kinewright_project::studio_records::{CommentId, FeedbackRevision};
    let mut f = Fixture::new();
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
    for n in 0..180 {
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
    let owner = f.host.owner.upgrade().unwrap();
    {
        let mut host = owner.lock().unwrap();
        let batch_ref = host
            .owner()
            .unwrap()
            .read_submitted_batch(&batch)
            .unwrap()
            .roots[0]
            .clone();
        f.brief.plan = host
            .owner_mut()
            .unwrap()
            .admit_host_grant(
                &serde_json::to_vec(
                    &serde_json::json!({"kind":"revise_cut_plan","batch_ref":batch_ref}),
                )
                .unwrap(),
            )
            .unwrap();
    }
    f
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn te14_fix2_many_notes_reach_wake_trim() {
    let r = c5_fix10_run_from(te14_fix2_notes_fixture());
    let result = c5_fix10_freeze(&r.director, &r.child);
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    let mut refs = vec![r.f.projection(&r.run).results.last().unwrap().clone()];
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
    let result = c5_fix10_freeze(&r.director, &child);
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.clone(),
            result,
        })),
    );
    refs.push(r.f.projection(&r.run).results.last().unwrap().clone());
    let wake = te14_fix1_wake(&r, &refs.iter().collect::<Vec<_>>());
    assert_eq!(wake["mode"], "summary");
    assert_eq!(
        wake["results"].as_array().unwrap().len(),
        2,
        "TE14_FIX2_TRIM_RETAINS_CHILDREN"
    );
    for row in wake["results"].as_array().unwrap() {
        assert_eq!(row["expected_note_count"], 180, "TE14_FIX2_TRIM_NOTE_COUNT");
        assert!(row.get("expected_note_ids").is_none());
        assert!(row["detail"]["reference"].is_object());
    }
    assert!(
        serde_json::to_vec(&wake).unwrap().len() <= 12 * 1024,
        "TE14_FIX2_TRIM_BOUND"
    );
    assert!(wake.get("omitted_results").is_none());
}
#[test]
#[ignore = "on-demand: generated multi-gigabyte product-path timing"]
fn te14_fix2_real_scale_measurements() {
    use kinewright_core::{AssetId, Command, Operation, TimeCode, Track, TrackId, TrackKind};
    use std::time::Instant;
    let mut f = Fixture::new();
    let media = kinewright_media::test_support::GeneratedMedia::ffmpeg(
        "te14-fix2-scale",
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-t",
            "34000",
            "-c:a",
            "pcm_s16le",
            "-rf64",
            "always",
        ],
        "wav",
    );
    let source_path = media.path().to_owned();
    let bytes = std::fs::metadata(&source_path).unwrap().len();
    assert!(bytes > 4 << 30);
    let owner = f.host.owner.upgrade().unwrap();
    {
        let mut host = owner.lock().unwrap();
        let project = host.owner_mut().unwrap();
        project.import_owned_media(&source_path).unwrap();
        project
            .core()
            .request(Command::DoBatch(vec![
                Operation::AddTrack {
                    track: Track {
                        id: TrackId(1),
                        kind: TrackKind::Audio,
                        sync_lock: true,
                        clips: vec![],
                    },
                },
                Operation::AddClip {
                    track: TrackId(1),
                    asset: AssetId(1),
                    at: TimeCode(0),
                    source: TimeCode(0)..TimeCode(1),
                },
            ]))
            .unwrap();
        project.save_active().unwrap();
        let mut branch = project.fork_durable_branch("scale-read").unwrap();
        project.persist_durable_branch(&mut branch).unwrap();
        f.brief.scope.base = project.candidate_binding(branch.branch_id()).unwrap();
        f.brief.inputs = project.current_inputs_ref().clone();
        let document = project.core().export_history().unwrap().document;
        f.brief.scope =
            crate::orchestration::policy::director_scope(&document, f.brief.scope.base.clone())
                .unwrap();
        project
            .store()
            .unwrap()
            .clear_media_memo_for_fixture()
            .unwrap();
    }
    let now = Instant::now();
    let started = f.start();
    println!(
        "TE14_FIX2_SCALE phase=cold_start bytes={bytes} seconds={:.6}",
        now.elapsed().as_secs_f64()
    );
    let (run, execution, producer) = reserve_from_started(&f, started);
    task_actor(&f, &run, &execution, &producer);
    f.host.runtime.stop();
    let mcp = C4Mcp::new(&f, &run, &execution.task);
    let core = owner.lock().unwrap().hooks.runs[&run].tasks[&execution.task]
        .core()
        .clone();
    let args = serde_json::to_value(EvidenceRead {
        revision: grant_test_revision(&core),
        evidence: EvidenceKind::Audio {
            clip: kinewright_core::ClipId(1),
            source_frame: 0,
        },
        cursor: None,
    })
    .unwrap();
    let before = f.projection(&run).tasks[0].progress.decisions.len();
    let mut original = None;
    for trial in 0..5 {
        owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .clear_media_memo_for_fixture()
            .unwrap();
        for cold in [true, false] {
            let now = Instant::now();
            let sample = te14_outcome(mcp.wire("read_task_evidence", args.clone()));
            let elapsed = now.elapsed().as_secs_f64();
            assert!(!sample["items"].as_array().unwrap().is_empty());
            if let Some(original) = &original {
                assert_eq!(&sample, original, "TE14_FIX2_SCALE_REPLAY");
            } else {
                original = Some(sample);
            }
            println!(
                "TE14_FIX2_SCALE phase=media cold={cold} trial={trial} bytes={bytes} seconds={elapsed:.6} sample=true"
            );
        }
    }
    assert_eq!(
        f.projection(&run).tasks[0].progress.decisions.len(),
        before + 1,
        "TE14_FIX2_SCALE_ONE_PAGE"
    );
    drop(mcp);
    {
        let mut host = owner.lock().unwrap();
        host.hooks.runs.remove(&run);
        host.owner()
            .unwrap()
            .store()
            .unwrap()
            .clear_media_memo_for_fixture()
            .unwrap();
        drop(host);
        let now = Instant::now();
        super::super::run_inputs::restore(&owner, &run).unwrap();
        println!(
            "TE14_FIX2_SCALE phase=cold_restore bytes={bytes} seconds={:.6}",
            now.elapsed().as_secs_f64()
        );
    }
    drop(media);
    assert!(!source_path.exists());
    println!("TE14_FIX2_SCALE generated_source_deleted=true");
}
#[test]
#[ignore = "on-demand: maximum child replay owner-lock timing"]
fn te14_fix2_largest_child_replay_measurement() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let payload = te14_fix2_raw_grant(&mut r.f, &vec![b'x'; 192 * 1024]);
    result.explanation = payload.clone();
    result.artifacts = vec![payload.clone(); 256];
    result.evidence = vec![payload; 256];
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let reference = r.f.projection(&r.run).results.last().unwrap().clone();
    let read = ChildRead {
        run: r.run.clone(),
        child: r.child.clone(),
        reference,
        source: result.head,
        cursor: None,
    };
    let first = te14_outcome(
        r.director
            .wire("read_child_result", serde_json::to_value(&read).unwrap()),
    );
    let before = r.f.projection(&r.run).tasks[0].progress.decisions.len();
    let owner = r.f.host.owner.upgrade().unwrap();
    let state = r.f.projection(&r.run);
    let mut host = owner.lock().unwrap();
    let producer = host.run_producer(&r.run).unwrap();
    let index = host.resolve_attachment(Some(&producer.bearer())).unwrap();
    let now = std::time::Instant::now();
    let replay = host
        .read_child_context(index, &state, &state.tasks[0], &read)
        .unwrap();
    let elapsed = now.elapsed().as_secs_f64();
    assert_eq!(first, replay);
    drop(host);
    assert_eq!(
        r.f.projection(&r.run).tasks[0].progress.decisions.len(),
        before
    );
    println!(
        "TE14_FIX2_REPLAY artifacts=256 evidence=256 per_record_bytes=196608 logical_body_bytes={} seconds={elapsed:.6} owner_locked=true no_new_page=true",
        513usize * 192 * 1024
    );
}
#[test]
fn te14_fix2_retry_and_budget_are_named_in_provider_text() {
    let instructions = super::super::codex_instructions::CODEX_INSTRUCTIONS;
    assert!(
        instructions.contains("verification still running is retryable shortly"),
        "TE14_FIX2_INSTRUCTIONS_RETRY"
    );
    let f = Fixture::new();
    let (_, _, mcp) = c4_director(&f);
    let tools = mcp.runtime.block_on(mcp.client.list_all_tools()).unwrap();
    for name in [
        "read_task_evidence",
        "read_child_result",
        "read_task_result",
    ] {
        let description = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .description
            .as_ref()
            .unwrap();
        assert!(
            description.contains("shared"),
            "TE14_FIX2_SHARED_TOOL_BUDGET {name}"
        );
    }
    assert!(
        tools
            .iter()
            .find(|tool| tool.name == "read_task_evidence")
            .unwrap()
            .description
            .as_ref()
            .unwrap()
            .contains("retry shortly"),
        "TE14_FIX2_TOOL_RETRY"
    );
}
#[test]
fn te14_fix2_duplicate_classification_fields_are_ambiguous() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let revision = serde_json::to_string(&result.head).unwrap();
    let mut reads = Vec::new();
    for preview in [false, true] {
        let kind = if preview {
            "task_preview_v1"
        } else {
            "task_proof_v1"
        };
        for duplicate_kind in [false, true] {
            let raw = if duplicate_kind {
                format!("{{\"kind\":\"ordinary\",\"kind\":\"{kind}\",\"revision\":{revision}}}")
            } else {
                format!("{{\"kind\":\"{kind}\",\"revision\":null,\"revision\":{revision}}}")
            };
            let reference = te14_fix2_raw_grant(&mut r.f, raw.as_bytes());
            result.artifacts.push(reference.clone());
            reads.push(if preview {
                EvidenceKind::Preview { reference }
            } else {
                EvidenceKind::Proof { reference }
            });
        }
    }
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    for kind in reads {
        let reply = te14_read(
            &r.director,
            kinewright_core::TimelineRevision(0),
            kind,
            None,
        );
        assert_eq!(
            reply.is_error,
            Some(true),
            "TE14_FIX2_DUPLICATE_CLASSIFICATION"
        );
        assert!(
            serde_json::to_string(&reply).unwrap().contains("ambiguous"),
            "TE14_FIX2_DUPLICATE_CLASSIFICATION"
        );
    }
}
#[test]
fn te14_fix2_decode_failure_does_not_claim_task_was_cancelled() {
    let (f, run, _child, core, mcp, _) = te14_media_fixture();
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    let reply = te14_read(
        &mcp,
        grant_test_revision(&core),
        EvidenceKind::Audio {
            clip: kinewright_core::ClipId(3),
            source_frame: 0,
        },
        None,
    );
    assert_eq!(reply.is_error, Some(true));
    let words = serde_json::to_string(&reply).unwrap();
    assert!(
        words.contains("could not produce the requested page"),
        "TE14_FIX2_WORKER_FAILURE_WORDS {words}"
    );
    assert!(
        !words.contains("no longer live"),
        "TE14_FIX2_WORKER_FAILURE_WORDS"
    );
    assert_eq!(f.projection(&run).tasks[1].progress.decisions.len(), before);
}
