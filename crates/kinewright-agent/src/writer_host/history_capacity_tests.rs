fn capacity_document(clips: usize) -> Document {
    let mut document = kinewright_project::c0_fixtures::save_cost_document();
    let template = document.tracks.remove(0);
    document.tracks.clear();
    for index in 1..=clips / 10 {
        let mut track = template.clone();
        track.id = kinewright_core::TrackId(index as u64);
        for (offset, clip) in track.clips.iter_mut().enumerate() {
            clip.id = kinewright_core::ClipId(index as u64 * 100 + offset as u64 + 1);
        }
        document.tracks.push(track);
    }
    document.validate().unwrap();
    document
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one unpaid task lifecycle across Finish, merge, Stop and reopen"
)]
fn history_capacity_vertical_400_finish_merge_stop_reopen() {
    let r = c5_fix10_run_from(Fixture::with_document(false, capacity_document(400)));
    assert_eq!(r.core.export_history().unwrap().batches.len(), 1);
    capacity_finish_merge_stop(r, None);
}

#[allow(
    clippy::too_many_lines,
    reason = "one durable lifecycle with an optional real process exit before destructors"
)]
fn capacity_finish_merge_stop(r: C5Fix10Run, crash_handoff: Option<&std::path::Path>) {
    use kinewright_project::history_record::{HistoryRecord, HistoryRole};
    let expected = r.core.export_history().unwrap();
    let finish_control = te13_control(&r.editor, &r.child, &r.core);
    let finished = r.editor.wire("finish_task", finish_control);
    assert_eq!(
        finished.is_error,
        Some(false),
        "CAPACITY_FINISH {finished:?}"
    );
    let state = r.f.projection(&r.run);
    assert!(matches!(state.tasks[1].state, TaskState::Terminal(_)));
    let (history, batch_ids) = {
        let host = r.f.host.owner.upgrade().unwrap();
        let host = host.lock().unwrap();
        let history = host.saved_task_history(&state, &r.child).unwrap().unwrap();
        assert!(history.frozen, "CAPACITY_FINISH_FROZE");
        let store = host.owner().unwrap().store().unwrap();
        let checkpoint = store
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &history.source.checkpoint,
            )
            .unwrap();
        let record = store
            .read_record::<HistoryRecord>(&checkpoint.history_ref)
            .unwrap();
        assert_eq!(record.role, HistoryRole::Task);
        assert_eq!(record.baseline_ref, checkpoint.baseline_ref);
        assert_eq!(record.current_ref, checkpoint.document_ref);
        assert_eq!(
            *store.resolve_history(&checkpoint.history_ref).unwrap(),
            expected,
            "CAPACITY_REFERENCED_REPLAY"
        );
        let ids = checkpoint.batch_ids.clone();
        (history, ids)
    };
    let freeze: OrchestrationReceipt = serde_json::from_value(
        finished.structured_content.as_ref().unwrap()["completion_status"]["freeze_receipt"]
            .clone(),
    )
    .unwrap();
    assert_eq!(
        freeze.decision,
        CommandDecision::Applied,
        "CAPACITY_FINISH_FREEZE_RECEIPT"
    );
    assert_eq!(
        freeze.envelope.command,
        OrchestrationCommand::FreezeTask(r.child.clone())
    );
    let merged = r.director.wire(
        "merge_task",
        r.director
            .control(OrchestrationCommand::MergeTask(MergeTask {
                task: r.child.clone(),
                target: r.f.brief.scope.base.clone(),
                result: state.results.last().unwrap().clone(),
            })),
    );
    assert_eq!(merged.is_error, Some(false), "CAPACITY_MERGE {merged:?}");
    {
        let owner = r.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        println!(
            "CAPACITY_VERTICAL_STORE_USAGE={:?}",
            host.owner().unwrap().store().unwrap().usage().unwrap()
        );
        println!(
            "CAPACITY_VERTICAL_ACTORS tasks={} registered={}",
            host.hooks.runs[&r.run].tasks.len(),
            host.owner().unwrap().durable_sessions().len()
        );
    }
    capacity_stop(&r.f, &r.run);
    let data = serde_json::json!({
        "root":r.f.temp.root(), "run":r.run, "child":r.child,
        "candidate":r.f.brief.scope.base.branch, "checkpoint":history.source.checkpoint,
        "batch_ids":batch_ids, "expected":expected,
    });
    if let Some(path) = crash_handoff {
        std::fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
        // Writer, Core and fixture remain live: this bypasses every destructor.
        std::process::exit(86);
    }
    let C5Fix10Run {
        f,
        core,
        director,
        editor,
        ..
    } = r;
    drop(core);
    drop(director);
    drop(editor);
    let Fixture { host, temp, .. } = f;
    drop(host);
    capacity_check_reopen(&data);
    drop(temp);
}

fn capacity_check_reopen(data: &serde_json::Value) {
    use kinewright_project::history_record::HistoryRole;
    let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
    let run: RunId = serde_json::from_value(data["run"].clone()).unwrap();
    let child: TaskId = serde_json::from_value(data["child"].clone()).unwrap();
    let expected: kinewright_core::HistorySnapshot =
        serde_json::from_value(data["expected"].clone()).unwrap();
    let opened = Instant::now();
    let owner = Ja1EditingSession::open(&root.join("project.kinewright")).unwrap();
    println!(
        "CAPACITY_NATIVE_OPEN seed_clips={} batches={} open_ms={:.3}",
        expected
            .baseline
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        expected.batches.len(),
        opened.elapsed().as_secs_f64() * 1000.0
    );

    let projection = owner.orchestration_projection(&run).unwrap().unwrap();
    assert_eq!(projection.cleanup, RunCleanup::Settled);
    assert!(matches!(
        projection
            .tasks
            .iter()
            .find(|task| task.id == child)
            .unwrap()
            .state,
        TaskState::Terminal(_)
    ));
    let store = owner.store().unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &serde_json::from_value(data["checkpoint"].clone()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&checkpoint.batch_ids).unwrap(),
        data["batch_ids"],
        "CAPACITY_BATCH_IDENTITIES_STABLE"
    );
    assert_eq!(
        *store.resolve_history(&checkpoint.history_ref).unwrap(),
        expected,
        "CAPACITY_REOPEN_EXACT"
    );
    let kept_candidate = owner
        .candidate_binding(&serde_json::from_value(data["candidate"].clone()).unwrap())
        .unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &kept_candidate.checkpoint,
        )
        .unwrap();
    let merged = store.resolve_history(&checkpoint.history_ref).unwrap();
    assert_eq!(merged.document, expected.document);
    assert_eq!(merged.batches.len(), 1, "CAPACITY_ONE_BOUNDARY_PER_MERGE");
    if let Some(redo) = expected.cursor.redo.last() {
        let core = Core::spawn(Document::default()).unwrap();
        core.restore_history(expected.clone()).unwrap();
        core.request(kinewright_core::Command::Redo).unwrap();
        assert_eq!(
            core.export_history().unwrap().document,
            redo.document,
            "CAPACITY_REOPEN_REDO_EXACT"
        );
    }
    assert_eq!(checkpoint.history_role, HistoryRole::Candidate);
}

fn capacity_stop(f: &Fixture, run: &RunId) {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};
    let before_stop = f.projection(run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("capacity-machine"),
        before_stop.attempts[0].identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut proofs = Vec::new();
    for attempt in before_stop
        .attempts
        .iter()
        .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
    {
        let native = crate::native_launcher::native_identity(&attempt.identity);
        claim.reserve(&native).unwrap();
        proofs.push((
            attempt.identity.clone(),
            RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap(),
        ));
    }
    te13_fix1_stop_host(&f.host, run);
    let producer = f.producer(run);
    for (identity, proof) in &proofs {
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .reconcile_native_effect(&producer, identity, &AttemptRetirementProof::Unbound(proof))
            .unwrap();
        claim.retire_unbound(proof).unwrap();
    }
    claim.finish().unwrap();
    assert_eq!(
        f.projection(run).cleanup,
        RunCleanup::Settled,
        "CAPACITY_STOP_SETTLED"
    );
}
