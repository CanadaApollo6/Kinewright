use kinewright_core::ClipId;
#[test]
fn te15_a_certification_tail_is_retryable() {
    te15_a_poll_boundary(false);
}
#[test]
fn te15_a_finished_worker_wins_over_deadline() {
    te15_a_poll_boundary(true);
}
fn te15_a_poll_boundary(finished: bool) {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicU8, Ordering},
    };
    use std::time::Duration;
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let cold = Arc::new((Barrier::new(2), Barrier::new(2)));
    let decode = Arc::new((Barrier::new(2), Barrier::new(2)));
    let poll = Arc::new((Barrier::new(2), Barrier::new(2)));
    let outcome = Arc::new(AtomicU8::new(0));
    let endpoint = {
        let mut host = owner.lock().unwrap();
        host.hooks.context_media_wait = Some(Duration::from_millis(600));
        host.hooks.context_media_gate = Some(decode.clone());
        host.hooks.context_media_poll_gate = Some(poll.clone());
        host.hooks.context_media_outcome = Some(outcome.clone());
        let store = host.owner().unwrap().store().unwrap();
        store.clear_media_memo_for_fixture().unwrap();
        let gate = cold.clone();
        store
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                gate.0.wait();
                gate.1.wait();
            }))
            .unwrap();
        host.hooks.runs[&run].tasks[&child].endpoint()
    };
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    let reply = std::thread::scope(|scope| {
        let read = scope.spawn(|| {
            te14_read(
                &mcp,
                grant_test_revision(&core),
                EvidenceKind::Frame {
                    clip: ClipId(3),
                    source_frame: 0,
                },
                None,
            )
        });
        poll.0.wait();
        cold.0.wait();
        std::thread::sleep(Duration::from_millis(350));
        cold.1.wait();
        decode.0.wait();
        if finished {
            decode.1.wait();
            let until = Instant::now() + Duration::from_secs(2);
            while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(outcome.load(Ordering::Acquire), 1, "TE15_WORKER_FINISHED");
        }
        std::thread::sleep(Duration::from_millis(350));
        poll.1.wait();
        let reply = read.join().unwrap();
        if !finished {
            decode.1.wait();
        }
        reply
    });
    let until = Instant::now() + Duration::from_secs(2);
    while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    if finished {
        assert_eq!(
            reply.is_error,
            Some(false),
            "TE15_FINISHED_AT_DEADLINE {reply:?}"
        );
        let page = te14_outcome(reply);
        assert!(
            serde_json::to_string(&page)
                .unwrap()
                .contains("owned_frame_sample"),
            "TE15_FINISHED_AT_DEADLINE"
        );
        assert_eq!(
            f.projection(&run).tasks[1].progress.decisions.len(),
            before + 1
        );
    } else {
        assert_eq!(reply.is_error, Some(true));
        assert!(
            serde_json::to_string(&reply)
                .unwrap()
                .contains("Media evidence preparation exceeded this wait. Retry shortly"),
            "TE15_PHASE_TAIL_RETRY {reply:?}"
        );
        assert_eq!(f.projection(&run).tasks[1].progress.decisions.len(), before);
        let page = te14_outcome(te14_read(
            &mcp,
            grant_test_revision(&core),
            EvidenceKind::Frame {
                clip: ClipId(3),
                source_frame: 0,
            },
            None,
        ));
        assert!(
            !page["items"].as_array().unwrap().is_empty(),
            "TE15_WARM_RETRY"
        );
    }
}
#[test]
fn te15_a_begin_media_read_distinguishes_liveness_from_busy() {
    use crate::orchestration::{
        policy::TaskPolicy,
        tasks::{RuntimeTaskAuthority, TaskEndpoint},
    };
    use std::time::Duration;
    let (f, run, child, core, _, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let state = f.projection(&run);
    let task = state.tasks.iter().find(|t| t.id == child).unwrap();
    let original = owner.lock().unwrap().hooks.runs[&run].tasks[&child].endpoint();
    for mode in ["revoked", "paused", "expired", "freezing", "busy"] {
        let authority = RuntimeTaskAuthority {
            expires_at: if mode == "expired" {
                Instant::now().checked_sub(Duration::from_secs(1)).unwrap()
            } else {
                Instant::now() + Duration::from_secs(60)
            },
            ..original.authority().clone()
        };
        let endpoint = TaskEndpoint::for_fixture(
            TaskPolicy {
                role: task.brief.role,
                scope: task.brief.scope.clone(),
            },
            authority,
            &core,
        );
        let prepared = {
            let mut host = owner.lock().unwrap();
            let producer = host.run_producer(&run).unwrap();
            let index = host.resolve_attachment(Some(&producer.bearer())).unwrap();
            host.prepare_media_context(
                index,
                &state,
                task,
                &EvidenceRead {
                    revision: grant_test_revision(&core),
                    evidence: EvidenceKind::Frame {
                        clip: ClipId(3),
                        source_frame: 0,
                    },
                    cursor: None,
                },
            )
            .unwrap()
        };
        let mut permit = None;
        let mut drain = None;
        match mode {
            "revoked" => endpoint.revoke(),
            "paused" => endpoint.set_paused(true),
            "freezing" => drain = Some(endpoint.begin_finish().unwrap()),
            "busy" => permit = Some(endpoint.begin_media_read().unwrap()),
            _ => {}
        }
        let error = prepared
            .render(&endpoint, &owner)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains(if mode == "busy" {
                "Retry shortly"
            } else {
                "no longer live"
            }),
            "TE15_BEGIN_LIVENESS {mode}: {error}"
        );
        drop(permit);
        drop(drain);
    }
}
#[test]
fn te15_a_transcript_duplicate_words_are_refused_on_wire() {
    let mut f = Fixture::new();
    let reference = te14_fix2_raw_grant(&mut f, br#"{"words":["keep"],"words":["drop","not"]}"#);
    f.brief.plan = reference.clone();
    let (_, _, mcp) = c4_director(&f);
    let reply = te14_read(
        &mcp,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Transcript {
            reference,
            start: 0,
            end: 1,
        },
        None,
    );
    assert_eq!(reply.is_error, Some(true), "TE15_TRANSCRIPT_DUPLICATES");
    assert!(
        serde_json::to_string(&reply).unwrap().contains("ambiguous"),
        "TE15_TRANSCRIPT_DUPLICATES"
    );
}
#[test]
fn te15_a_nested_revision_duplicates_are_refused_on_wire() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let mut reads = Vec::new();
    for preview in [false, true] {
        for field in ["head", "branch", "checkpoint", "checkpoint.sha256"] {
            let mut revision = serde_json::to_value(&result.head).unwrap();
            let key = if field == "checkpoint.sha256" {
                "sha256"
            } else {
                field
            };
            let value = if field == "checkpoint.sha256" {
                revision["checkpoint"].get_mut(key)
            } else {
                revision.get_mut(key)
            };
            if let Some(value) = value {
                let good = serde_json::to_string(value).unwrap();
                let token = format!("\"{key}\":{good}");
                let raw = serde_json::to_string(&revision)
                    .unwrap()
                    .replace(&token, &format!("\"{key}\":null,{token}"));
                let raw = format!(
                    "{{\"kind\":\"{}\",\"revision\":{raw}}}",
                    if preview {
                        "task_preview_v1"
                    } else {
                        "task_proof_v1"
                    }
                );
                let reference = te14_fix2_raw_grant(&mut r.f, raw.as_bytes());
                result.artifacts.push(reference.clone());
                reads.push(if preview {
                    EvidenceKind::Preview { reference }
                } else {
                    EvidenceKind::Proof { reference }
                });
            }
        }
    }
    assert_eq!(reads.len(), 8, "TE15_NESTED_FIXTURE_FIELDS");
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result,
        })),
    );
    for read in reads {
        let reply = te14_read(
            &r.director,
            kinewright_core::TimelineRevision(0),
            read,
            None,
        );
        assert_eq!(
            reply.is_error,
            Some(true),
            "TE15_NESTED_REVISION_DUPLICATES"
        );
        assert!(
            serde_json::to_string(&reply).unwrap().contains("ambiguous"),
            "TE15_NESTED_REVISION_DUPLICATES"
        );
    }
}
#[test]
fn te15_a_empty_record_is_present_on_wire() {
    let mut r = c5_fix10_run();
    let mut result = c5_fix10_freeze(&r.director, &r.child);
    let reference = te14_fix2_raw_grant(&mut r.f, b"");
    result.artifacts.push(reference.clone());
    r.director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: r.child.clone(),
            result: result.clone(),
        })),
    );
    let published = r.f.projection(&r.run).results.last().unwrap().clone();
    let page = te14_outcome(
        r.director.wire(
            "read_child_result",
            serde_json::to_value(ChildRead {
                run: r.run.clone(),
                child: r.child.clone(),
                reference: published,
                source: result.head,
                cursor: None,
            })
            .unwrap(),
        ),
    );
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |row| row["reference"] == serde_json::to_value(&reference).unwrap()
                    && row["chunk"]["empty"] == true
                    && row["chunk"]["text"] == ""
            ),
        "TE15_EMPTY_CHILD_PRESENT"
    );
    let page = te14_outcome(te14_read(
        &r.director,
        kinewright_core::TimelineRevision(0),
        EvidenceKind::Reference { reference },
        None,
    ));
    assert_eq!(
        page["items"][0]["chunk"]["empty"], true,
        "TE15_EMPTY_REFERENCE_PRESENT"
    );
    assert_eq!(page["items"][0]["chunk"]["byte_length"], 0);
}

#[test]
fn te15_a_fix_repeated_decode_ends_after_two_timeouts() {
    for audio in [false, true] {
        te15_a_fix_retry_sequence(audio, true);
    }
}
#[test]
fn te15_a_fix_cold_decode_succeeds_on_warm_retry() {
    for audio in [false, true] {
        te15_a_fix_retry_sequence(audio, false);
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "one repeated-read sequence covers cold work, busy wind-down, warm retry and page accounting"
)]
fn te15_a_fix_retry_sequence(audio: bool, terminal: bool) {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicU8, Ordering},
    };
    use std::time::Duration;
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let first_gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let warm_gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    let first_outcome = Arc::new(AtomicU8::new(0));
    let warm_outcome = Arc::new(AtomicU8::new(0));
    let unexpected_cold = Arc::new(AtomicBool::new(false));
    let endpoint = {
        let host = owner.lock().unwrap();
        host.owner()
            .unwrap()
            .store()
            .unwrap()
            .clear_media_memo_for_fixture()
            .unwrap();
        host.hooks.runs[&run].tasks[&child].endpoint()
    };
    let request = serde_json::to_value(EvidenceRead {
        revision: grant_test_revision(&core),
        evidence: if audio {
            EvidenceKind::Audio {
                clip: ClipId(2),
                source_frame: 0,
            }
        } else {
            EvidenceKind::Frame {
                clip: ClipId(3),
                source_frame: 0,
            }
        },
        cursor: None,
    })
    .unwrap();
    let prepare = |gate, outcome| {
        let mut host = owner.lock().unwrap();
        host.hooks.context_media_wait = Some(Duration::from_secs(1));
        host.hooks.context_media_prepare_delay = Some(Duration::from_millis(400));
        host.hooks.context_media_gate = gate;
        host.hooks.context_media_outcome = outcome;
    };
    let drain = || {
        let until = Instant::now() + Duration::from_secs(5);
        while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !endpoint.media_read_pending_for_fixture(),
            "TE15_FIX_WORKER_DRAINED"
        );
    };
    let before = f.projection(&run).tasks[1].progress.decisions.len();
    prepare(Some(first_gate.clone()), Some(first_outcome.clone()));
    let (first, busy, pending, after_first) = std::thread::scope(|scope| {
        let read = scope.spawn(|| mcp.wire("read_task_evidence", request.clone()));
        first_gate.0.wait();
        let first = read.join().unwrap();
        let pending = endpoint.media_read_pending_for_fixture();
        let busy = mcp.wire("read_task_evidence", request.clone());
        let after = f.projection(&run).tasks[1].progress.decisions.len();
        first_gate.1.wait();
        (first, busy, pending, after)
    });
    drain();
    let first_cancelled = first_outcome.load(Ordering::Acquire);
    {
        let host = owner.lock().unwrap();
        let unexpected = unexpected_cold.clone();
        host.owner()
            .unwrap()
            .store()
            .unwrap()
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                unexpected.store(true, Ordering::Release);
            }))
            .unwrap();
    }
    prepare(
        terminal.then(|| warm_gate.clone()),
        Some(warm_outcome.clone()),
    );
    let second = std::thread::scope(|scope| {
        let read = scope.spawn(|| mcp.wire("read_task_evidence", request.clone()));
        if terminal {
            warm_gate.0.wait();
        }
        let second = read.join().unwrap();
        if terminal {
            warm_gate.1.wait();
        }
        second
    });
    drain();
    assert_eq!(first.is_error, Some(true));
    assert!(
        serde_json::to_string(&first)
            .unwrap()
            .contains("Retry shortly"),
        "TE15_FIX_COLD_RETRY {first:?}"
    );
    assert!(pending, "TE15_FIX_WIND_DOWN_HELD");
    assert!(
        serde_json::to_string(&busy)
            .unwrap()
            .contains("earlier read is still running. Retry shortly"),
        "TE15_FIX_BUSY_RETRY {busy:?}"
    );
    assert_eq!(after_first, before, "TE15_FIX_TIMEOUTS_UNCHARGED");
    assert_eq!(first_cancelled, 2, "TE15_FIX_REAL_CANCELLATION");
    assert!(
        !unexpected_cold.load(Ordering::Acquire),
        "TE15_FIX_REAL_WARM_MEMO"
    );
    if terminal {
        assert_eq!(second.is_error, Some(true));
        let words = serde_json::to_string(&second).unwrap();
        assert!(
            words.contains("not retryable") && !words.contains("Retry shortly"),
            "TE15_FIX_WARM_TERMINAL {second:?}"
        );
        assert!(words.contains("Media decode or seek did not finish within the 30-second wait; not retryable. Replan without guessing."), "TE15_FIX_TERMINAL_WORDS {second:?}");
        assert_eq!(
            f.projection(&run).tasks[1].progress.decisions.len(),
            before,
            "TE15_FIX_TWO_TIMEOUTS_UNCHARGED"
        );
        assert_eq!(warm_outcome.load(Ordering::Acquire), 2);
    } else {
        let sample = te14_outcome(second);
        assert!(
            serde_json::to_string(&sample).unwrap().contains(if audio {
                "owned_audio_sample"
            } else {
                "owned_frame_sample"
            }),
            "TE15_FIX_RETRY_SAMPLE"
        );
        assert_eq!(
            f.projection(&run).tasks[1].progress.decisions.len(),
            before + 1,
            "TE15_FIX_ONE_SAMPLE_PAGE"
        );
        assert_eq!(warm_outcome.load(Ordering::Acquire), 1);
    }
}
