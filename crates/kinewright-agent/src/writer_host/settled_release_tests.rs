fn settled_sweep(f: &Fixture) {
    let host = f.host.owner.upgrade().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut owner = host.lock().unwrap();
        owner.sweep_settled_run_actors();
        if owner
            .hooks
            .runs
            .values()
            .all(|run| run.tasks.is_empty() && run.settled_candidates.is_empty())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SETTLED_RELEASE_HOLDERS {:?}",
            owner
                .hooks
                .runs
                .iter()
                .map(|(id, run)| (id, run.tasks.len(), &run.release_notice))
                .collect::<Vec<_>>()
        );
        drop(owner);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn settled_run(f: &mut Fixture, edits: usize) -> (RunId, TaskId, kinewright_core::HistorySnapshot) {
    let (run, task, history, core) = settled_pending_run(f, edits);
    drop(core);
    settled_sweep(f);
    (run, task, history)
}

fn settled_observation_notice(owner: &HostCommandOwner, index: usize, run: &RunId) -> String {
    match owner.observe_run(index, run, 0) {
        Ok(view) => view.runtime_status.expect("SETTLED_NOTICE_IN_RUN_VIEW"),
        Err(error) => {
            let refusal = error.refusal();
            assert_eq!(refusal.code, "damaged_bundle");
            refusal.message
        }
    }
}

#[test]
fn settled_release_candidate_without_a_task_actor_is_released() {
    let f = te13_fixture();
    let candidate = f.brief.scope.base.branch.clone();
    let (run, _execution, producer) = reserve(&f);
    drop(producer);
    capacity_stop(&f, &run);
    settled_sweep(&f);
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    assert!(owner.hooks.runs[&run].tasks.is_empty());
    assert!(
        !owner
            .owner()
            .unwrap()
            .is_registered_run_candidate(&candidate)
    );
}

fn settled_pending_run(
    f: &mut Fixture,
    edits: usize,
) -> (RunId, TaskId, kinewright_core::HistorySnapshot, Core) {
    let doc = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap()
        .document;
    f.brief = settled_candidate_brief(f, &doc);
    let (run, parent, director) = te13_director(f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task,
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (execution, client, endpoint, core) = c4_reserve_task(f, &run, &child);
    history_turn(f, &execution, &client, &endpoint, 1, true);
    let editor = C4Mcp::new(f, &run, &child);
    for edit in 0..edits {
        let operations = if edits == 200 && edit == 100 {
            (1..=20)
                .map(|track| kinewright_core::Operation::RippleDeleteClip {
                    clip: kinewright_core::ClipId(track * 100 + 10),
                })
                .collect()
        } else {
            vec![kinewright_core::Operation::SetTrackSyncLock {
                track: kinewright_core::TrackId(1),
                locked: edit % 2 == 0,
            }]
        };
        core.request(kinewright_core::Command::DoBatch(operations))
            .unwrap();
    }
    core.request(kinewright_core::Command::Undo).unwrap();
    let expected = core.export_history().unwrap();
    let finished = editor.wire("finish_task", te13_control(&editor, &child, &core));
    assert_eq!(
        finished.is_error,
        Some(false),
        "SETTLED_FINISH {finished:?}"
    );
    capacity_stop(f, &run);
    // These holders deliberately make the first sweep postpone release.
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        owner.sweep_settled_run_actors();
        assert!(owner.hooks.runs[&run].tasks.contains_key(&child));
    }
    drop(endpoint);
    drop(editor);
    drop(director);
    drop(client);
    (run, child, expected, core)
}

#[test]
fn settled_release_verification_failure_keeps_actor_and_discloses_the_task() {
    let mut f = Fixture::with_document(false, capacity_document(40));
    let (run, child, expected, core) = settled_pending_run(&mut f, 4);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    let saved = owner.saved_task_history(&state, &child).unwrap().unwrap();
    let checkpoint = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &saved.source.checkpoint,
        )
        .unwrap();
    let path = f.temp.path(&format!(
        "project.kinewright-assets/studio/objects/{}",
        checkpoint.history_ref.sha256
    ));
    let original = std::fs::read(&path).unwrap();
    let mut damaged = original.clone();
    damaged[0] = b'!';
    std::fs::write(&path, &damaged).unwrap();
    // Hold the last outside Core until the fault exists and the owner mutex
    // excludes the runtime's automatic sweep.
    drop(core);
    let deadline = Instant::now() + Duration::from_secs(10);
    while owner.hooks.runs[&run].release_notice.is_none() {
        owner.sweep_settled_run_actors();
        assert!(Instant::now() < deadline, "SETTLED_MISMATCH_NEVER_VERIFIED");
        drop(owner);
        std::thread::sleep(Duration::from_millis(10));
        owner = host.lock().unwrap();
    }
    assert_eq!(
        owner.hooks.runs[&run].tasks[&child]
            .core()
            .export_history()
            .unwrap(),
        expected,
        "SETTLED_MISMATCH_KEEPS_LIVE_WORK"
    );
    for auth in [&f.editor.bootstrap.auth, &owner.hooks.runs[&run].producer] {
        let index = owner.resolve_attachment(Some(&auth.bearer())).unwrap();
        let words = settled_observation_notice(&owner, index, &run);
        assert!(
            words.contains(child.0.as_str())
                && words.contains("saved result could not be reopened"),
            "{words}"
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
    assert_eq!(
        owner.hooks.runs[&run].tasks[&child]
            .core()
            .export_history()
            .unwrap(),
        expected,
        "FAILED_VERIFICATION_LEAVES_THE_RESIDENT_RESULT_USABLE"
    );
}

fn settled_candidate_brief(f: &Fixture, document: &Document) -> TaskBrief {
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let project = owner.owner_mut().unwrap();
    let mut candidate = project
        .fork_durable_branch(
            "settled-run candidate",
            kinewright_project::editing_session::DurableBranchRole::RunCandidate,
        )
        .unwrap();
    project.persist_durable_branch(&mut candidate).unwrap();
    let base = project.candidate_binding(candidate.branch_id()).unwrap();
    let mut brief = f.brief.clone();
    // Match the existing 400/200 lifecycle's scoped tracks and tail deletes;
    // the entire 400-clip document is still retained by every history boundary.
    brief.scope = crate::orchestration::policy::director_scope(&Document::default(), base).unwrap();
    for track in &document.tracks {
        let name = format!("track:{}", track.id);
        brief.scope.objects.insert(name.clone());
        brief.scope.tracks.insert(name.clone());
        brief.scope.bus_sidechain_closure.insert(name);
    }
    for track in 1..=20.min(document.tracks.len()) {
        brief
            .scope
            .objects
            .insert(format!("clip:{}", track * 100 + 10));
    }
    brief
}

#[test]
fn settled_release_dirty_checkpoint_rehydrates_and_damaged_retry_refuses_truthfully() {
    let mut f = Fixture::with_document(false, capacity_document(40));
    let (run, child, expected) = settled_run(&mut f, 4);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    let saved = owner.saved_task_history(&state, &child).unwrap().unwrap();
    assert!(owner.hooks.runs[&run].tasks.is_empty());
    let before = owner
        .owner()
        .unwrap()
        .candidate_binding(&saved.source.branch)
        .unwrap();
    owner.restore_frozen_task(&state, &child).unwrap();
    assert_eq!(
        owner.hooks.runs[&run].tasks[&child]
            .core()
            .export_history()
            .unwrap(),
        expected,
        "SETTLED_REHYDRATE_EXACT"
    );
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .candidate_binding(&saved.source.branch)
            .unwrap(),
        before
    );
    owner.sweep_settled_run_actors();
    assert!(owner.hooks.runs[&run].tasks.is_empty());
    let checkpoint = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &saved.source.checkpoint,
        )
        .unwrap();
    let path = f.temp.path(&format!(
        "project.kinewright-assets/studio/objects/{}",
        checkpoint.history_ref.sha256
    ));
    let root = std::fs::read(f.temp.path("project.kinewright")).unwrap();
    let mut damaged = std::fs::read(&path).unwrap();
    damaged[0] = b'!';
    std::fs::write(&path, &damaged).unwrap();
    let registrations = owner.owner().unwrap().durable_sessions().len();
    let error = owner.restore_frozen_task(&state, &child).unwrap_err();
    let refusal = error.refusal();
    assert_eq!(refusal.code, "damaged_bundle");
    assert!(
        refusal.message.contains(child.0.as_str())
            && refusal
                .message
                .contains("saved result could not be reopened")
            && refusal.message.contains("other work is unaffected"),
        "{refusal:?}"
    );
    assert!(owner.hooks.runs[&run].tasks.is_empty());
    assert_eq!(
        owner.owner().unwrap().durable_sessions().len(),
        registrations,
        "FAILED_REHYDRATION_REGISTERS_NOTHING"
    );
    for auth in [&f.editor.bootstrap.auth, &owner.hooks.runs[&run].producer] {
        let index = owner.resolve_attachment(Some(&auth.bearer())).unwrap();
        assert!(
            settled_observation_notice(&owner, index, &run)
                .contains("saved result could not be reopened")
        );
    }
    assert_eq!(std::fs::read(path).unwrap(), damaged);
    assert_eq!(
        std::fs::read(f.temp.path("project.kinewright")).unwrap(),
        root
    );
}

#[test]
fn settled_release_heap_child() {
    if std::env::var_os("KINEWRIGHT_SETTLED_HEAP").is_none() {
        return;
    }
    let mut f = Fixture::with_document(false, capacity_document(400));
    let registrations = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .durable_sessions()
        .len();
    let mut samples = Vec::new();
    let mut history_charge = 0;
    for run in 1..=6 {
        let (_, _, expected) = settled_run(&mut f, 200);
        assert_eq!(expected.batches.len(), 201);
        history_charge =
            kinewright_core::history_size::restored_history_live_bytes(&expected).unwrap();
        drop(expected);
        // Core drops its state on its receiver thread. Sample the quiescent
        // minimum over a bounded interval rather than timing that thread's exit.
        let until = Instant::now() + Duration::from_millis(500);
        let mut bytes = usize::MAX;
        while Instant::now() < until {
            let stats = crate::TEST_HEAP.stats();
            if let Some(current) = stats.bytes_allocated.checked_sub(stats.bytes_deallocated) {
                bytes = bytes.min(current);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let owner = f.host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        assert_ne!(bytes, usize::MAX, "SETTLED_HEAP_REQUIRES_A_COMPLETE_SAMPLE");
        assert!(owner.hooks.runs.values().all(|run| run.tasks.is_empty()));
        assert_eq!(
            owner.owner().unwrap().durable_sessions().len(),
            registrations
        );
        samples.push(bytes);
        println!(
            "SETTLED_HEAP runs={run} bytes={bytes} tasks=0 registrations={registrations} history_charge={history_charge}"
        );
    }
    assert!(
        u64::try_from(samples.iter().max().unwrap() - samples.iter().min().unwrap()).unwrap()
            < history_charge / 2,
        "SETTLED_HEAP_MUST_NOT_RETAIN_ANOTHER_HISTORY samples={samples:?} history={history_charge}"
    );
    drop(f);
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn settled_release_400_clip_resident_heap_after_six_runs() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "settled-heap-parent",
    );
    let log = temp.path("child.log");
    let executable = std::env::current_exe().unwrap();
    let directory = std::env::current_dir().unwrap();
    let output = std::fs::File::create(&log).unwrap();
    let errors = output.try_clone().unwrap();
    let mut child = std::process::Command::new(&executable)
        .current_dir(&directory)
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::settled_release_heap_child",
            "--show-output",
            "--test-threads=1",
        ])
        .env("KINEWRIGHT_SETTLED_HEAP", "1")
        .stdout(output)
        .stderr(errors)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(500) {
            let proc = std::path::PathBuf::from(format!("/proc/{}", child.id()));
            assert_eq!(std::fs::read_link(proc.join("cwd")).unwrap(), directory);
            let argv = std::fs::read(proc.join("cmdline")).unwrap();
            assert_eq!(
                argv.split(|byte| *byte == 0).next().unwrap(),
                executable.as_os_str().as_encoded_bytes()
            );
            assert!(
                argv.windows(b"settled_release_heap_child".len())
                    .any(|part| part == b"settled_release_heap_child")
            );
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "SETTLED_HEAP_CHILD_TIMEOUT {}",
                std::fs::read_to_string(&log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let log = std::fs::read_to_string(log).unwrap();
    assert!(status.success(), "SETTLED_HEAP_CHILD {log}");
    let mut samples = 0;
    for line in log.lines().filter(|line| line.starts_with("SETTLED_HEAP ")) {
        println!("{line}");
        samples += 1;
    }
    assert_eq!(samples, 6, "SETTLED_HEAP_SIX_COMPLETE_RUNS {log}");
}
