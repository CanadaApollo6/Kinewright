#[test]
fn te1_safety_source_changed_between_certification_and_binding_starts_nothing() {
    use std::io::{Seek as _, Write as _};
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    let request = f.start_request(&f.editor);
    let host = f.host.owner.upgrade().unwrap();
    let prepared = host
        .lock()
        .unwrap()
        .prepare_run_inputs(&f.brief.scope.base.checkpoint)
        .unwrap()
        .open()
        .unwrap();
    let entry = &prepared.context.certified_input_leases[0].identity().entry;
    let root = f.temp.root();
    let source = te1_safety_find_object(root, &entry.object.sha256).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(source)
        .unwrap();
    file.seek(std::io::SeekFrom::End(-1)).unwrap();
    file.write_all(&[123]).unwrap();
    file.sync_all().unwrap();
    let mut owner = host.lock().unwrap();
    owner.hooks.prepared_run_inputs = Some(prepared);
    let index = owner
        .resolve_attachment(Some(&f.editor.bootstrap.auth.bearer()))
        .unwrap();
    let result = owner.dispatch_orchestration(index, &request);
    assert!(
        result.is_err(),
        "TE1_SAFETY_RECHECK: changed input started a run"
    );
    assert!(owner.hooks.runs.is_empty());
    let run: RunId = request.envelope.identity("run").unwrap();
    assert!(
        owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .is_none()
    );
}
fn te1_safety_find_object(root: &std::path::Path, digest: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(root).ok()? {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(found) = te1_safety_find_object(&path, digest) {
                return Some(found);
            }
        } else if path.file_name().unwrap().to_string_lossy().contains(digest) {
            return Some(path);
        }
    }
    None
}
#[test]
fn te1_safety_stop_releases_initial_owner_wait() {
    te1_safety_stop_owner_wait(false);
}
#[test]
fn te1_safety_stop_releases_final_owner_wait() {
    te1_safety_stop_owner_wait(true);
}
fn te1_safety_stop_owner_wait(final_lock: bool) {
    use std::sync::{Arc, Barrier};
    let (f, run, child, core, mcp, _) = te14_media_fixture();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    let endpoint = owner.lock().unwrap().hooks.runs[&run].tasks[&child].endpoint();
    let waiting = Arc::new(Barrier::new(2));
    if !final_lock {
        endpoint.observe_next_owner_wait_for_fixture(waiting.clone());
    }
    let gate = Arc::new((Barrier::new(2), Barrier::new(2)));
    if final_lock {
        owner.lock().unwrap().hooks.context_media_gate = Some(gate.clone());
    }
    let args = serde_json::to_value(EvidenceRead {
        revision: grant_test_revision(&core),
        evidence: EvidenceKind::Frame {
            clip: kinewright_core::ClipId(3),
            source_frame: 0,
        },
        cursor: None,
    })
    .unwrap();
    std::thread::scope(|scope| {
        let mut guard = if final_lock {
            None
        } else {
            Some(owner.lock().unwrap())
        };
        let read = scope.spawn(|| mcp.wire("read_task_evidence", args));
        if final_lock {
            gate.0.wait();
            guard = Some(owner.lock().unwrap());
            endpoint.observe_next_owner_wait_for_fixture(waiting.clone());
            gate.1.wait();
        }
        if final_lock {
            let until = Instant::now() + Duration::from_secs(2);
            while endpoint.media_read_pending_for_fixture() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                !endpoint.media_read_pending_for_fixture(),
                "media worker must finish before the final-lock Stop"
            );
        }
        waiting.wait();
        assert!(!read.is_finished());
        let host = guard.as_mut().unwrap();
        let before = host
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap()
            .tasks[1]
            .progress
            .decisions
            .len();
        let index = host
            .resolve_attachment(Some(&f.editor.bootstrap.auth.bearer()))
            .unwrap();
        assert_eq!(
            host.dispatch_orchestration(index, &stop).unwrap().decision,
            CommandDecision::Applied
        );
        let until = Instant::now() + Duration::from_secs(2);
        while !read.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let timely = read.is_finished();
        let after = host
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap()
            .tasks[1]
            .progress
            .decisions
            .len();
        drop(guard);
        let reply = read.join().unwrap();
        assert!(timely, "TE1_SAFETY_STOP_WAITER final={final_lock}");
        assert_eq!(reply.is_error, Some(true), "{reply:?}");
        let text = serde_json::to_string(&reply).unwrap();
        assert!(text.contains("task_authority_expired"), "{text}");
        assert_eq!(before, after, "TE1_SAFETY_WAITER_WORK_RAN");
    });
}
#[test]
#[ignore = "on-demand: generated 6.5 GB owner-lock and media stage timing"]
fn te1_safety_scale_measurement() {
    use kinewright_core::{AssetId, Command, Operation, TimeCode, Track, TrackId, TrackKind};
    let path =
        kinewright_media::test_support::workspace_target_dir().join("te1-safety/source-6.5gb.wav");
    if !path.exists() {
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
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
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert!(bytes > 6_500_000_000);
    let mut f = Fixture::new();
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    {
        let mut host = owner.lock().unwrap();
        let project = host.owner_mut().unwrap();
        project.import_owned_media(&path).unwrap();
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
        let mut branch = project.fork_durable_branch("safety-scale").unwrap();
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
    let (started, hold) = te1_safety_sample_hold(&owner, || f.start());
    println!(
        "TE1_SAFETY_TIMING phase=StartRun bytes={bytes} max_observed_owner_hold={:.6}",
        hold.as_secs_f64()
    );
    let (run, execution, producer) = reserve_from_started(&f, started);
    task_actor(&f, &run, &execution, &producer);
    let mcp = C4Mcp::new(&f, &run, &execution.task);
    let core = owner.lock().unwrap().hooks.runs[&run].tasks[&execution.task]
        .core()
        .clone();
    owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .clear_media_memo_for_fixture()
        .unwrap();
    let now = Instant::now();
    let reply = te14_read(
        &mcp,
        grant_test_revision(&core),
        EvidenceKind::Audio {
            clip: kinewright_core::ClipId(1),
            source_frame: 0,
        },
        None,
    );
    println!(
        "TE1_SAFETY_TIMING phase=full_media_reply seconds={:.6} error={:?}",
        now.elapsed().as_secs_f64(),
        reply.is_error
    );
    assert_ne!(reply.is_error, Some(true), "{reply:?}");
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
    }
    let (_, hold) = te1_safety_sample_hold(&owner, || {
        super::super::run_inputs::restore(&owner, &run).unwrap()
    });
    println!(
        "TE1_SAFETY_TIMING phase=restore_native_run bytes={bytes} max_observed_owner_hold={:.6}",
        hold.as_secs_f64()
    );
}
fn te1_safety_sample_hold<T>(
    owner: &Arc<std::sync::Mutex<HostCommandOwner>>,
    work: impl FnOnce() -> T,
) -> (T, Duration) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let done = AtomicBool::new(false);
    let ready = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let sampler = scope.spawn(|| {
            let mut longest = Duration::ZERO;
            let mut blocked = None;
            ready.wait();
            while !done.load(Ordering::Acquire) {
                if owner.try_lock().is_ok() {
                    if let Some(since) = blocked.take() {
                        longest = longest.max(Instant::now().duration_since(since));
                    }
                } else {
                    blocked.get_or_insert_with(Instant::now);
                }
                std::thread::sleep(Duration::from_micros(100));
            }
            if let Some(since) = blocked {
                longest = longest.max(since.elapsed());
            }
            longest
        });
        ready.wait();
        let result = work();
        done.store(true, Ordering::Release);
        (result, sampler.join().unwrap())
    })
}

#[test]
fn te1_safety_provider_choice_rechecks_after_input_certification() {
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    let request = f.start_request(&f.editor);
    let run: RunId = request.envelope.identity("run").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    {
        let host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        store.clear_media_memo_for_fixture().unwrap();
        let folder = f.temp.path("scheduler");
        let choice = super::super::specialist_choices()
            .into_iter()
            .find(|choice| choice.connector == "claude")
            .unwrap();
        store
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                super::super::write_specialist_setting(
                    Some(&folder),
                    &super::super::SpecialistChoice::Specific(choice.clone()),
                )
                .unwrap();
            }))
            .unwrap();
    }
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert_eq!(reply.error.unwrap().code, "provider_wait");
    let host = owner.lock().unwrap();
    assert!(host.hooks.runs.is_empty());
    assert!(
        host.owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .is_none()
    );
}

#[test]
fn te1_safety_non_media_inputs_still_start() {
    use kinewright_project::{
        bundle_records::{ObjectRef, RecordKind},
        owned_inputs::OwnedInputEntry,
    };
    let f = Fixture::with_inputs(false, Document::default(), "project.kinewright", |bundle| {
        let empty = bundle.empty_inputs().unwrap().reference.clone();
        let mut manifest = (*bundle.store().read_input_manifest(&empty).unwrap()).clone();
        let bytes = b"non-media run input";
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let _pin = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &bytes[..], &object, &[])
            .unwrap();
        manifest.entries.push(OwnedInputEntry {
            entry_id: "raw-input".into(),
            object,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        });
        bundle.admit_inputs(&manifest).unwrap().reference.clone()
    });
    f.host.runtime.stop();
    assert_eq!(f.start().decision, CommandDecision::Applied);
}

#[test]
#[cfg(feature = "test-util")]
fn te1_safety_completion_fixture_retains_media_inputs() {
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    f.host
        .fixture_pending_completion(CompletionStep::Freeze)
        .unwrap();
}

#[test]
fn te1_safety_shared_direct_ingress_certifies_without_owner_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    let request = f.start_request(&f.editor);
    let owner = f.host.owner.upgrade().unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    {
        let mut host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        store.clear_media_memo_for_fixture().unwrap();
        let weak = Arc::downgrade(&owner);
        let count = observed.clone();
        store
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                assert!(
                    weak.upgrade().unwrap().try_lock().is_ok(),
                    "cold certification held the owner mutex"
                );
                count.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        let refused = host.execute_attached(
            &f.editor.bootstrap.auth,
            HostRequest {
                editor: None,
                operation: None,
                protocol: HOST_PROTOCOL,
                project: f.editor.endpoint().project.clone(),
                roots: f.editor.bootstrap.roots.clone(),
                command: HostCommand::DispatchOrchestration {
                    request: Box::new(request.clone()),
                },
            },
        );
        assert_eq!(refused.error.unwrap().code, "shared_owner_required");
        assert_eq!(observed.load(Ordering::SeqCst), 0);
    }
    let reply = call(
        &f,
        &f.editor,
        HostCommand::DispatchOrchestration {
            request: Box::new(request),
        },
        0,
    );
    assert_eq!(
        ok(reply, "TE1_SHARED_DIRECT_START")
            .orchestration
            .unwrap()
            .decision,
        CommandDecision::Applied
    );
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[test]
fn te1_safety_revise_replay_does_not_certify_inputs_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = Fixture::with_media(true);
    f.host.runtime.stop();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let first = revise_result(revise(&f, &f.editor, &request), "TE1_REVISE_INITIAL");
    assert!(matches!(first, ReviseResult::Accepted { .. }));
    let observed = Arc::new(AtomicUsize::new(0));
    {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        store.clear_media_memo_for_fixture().unwrap();
        let count = observed.clone();
        store
            .hold_next_cold_media_for_fixture(Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
    }
    let again = revise_result(revise(&f, &f.editor, &request), "TE1_REVISE_REPLAY");
    assert_eq!(again, first);
    assert_eq!(
        observed.load(Ordering::SeqCst),
        0,
        "replay must not recertify footage"
    );
}
