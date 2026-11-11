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
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};
    use kinewright_project::history_record::{HistoryRecord, HistoryRole};
    let r = c5_fix10_run_from(Fixture::with_document(false, capacity_document(400)));
    let expected = r.core.export_history().unwrap();
    assert_eq!(expected.batches.len(), 1, "CAPACITY_ONE_ACCEPTED_EDIT");
    assert_eq!(
        expected
            .document
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        400
    );
    let finished = r
        .editor
        .wire("finish_task", te13_control(&r.editor, &r.child, &r.core));
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
    let before_stop = r.f.projection(&r.run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &r.f.temp.path("capacity-machine"),
        before_stop.attempts[0].identity.project.as_str(),
        r.run.0.as_str(),
    )
    .unwrap();
    let mut proofs = Vec::new();
    for attempt in &before_stop.attempts {
        let native = crate::native_launcher::native_identity(&attempt.identity);
        claim.reserve(&native).unwrap();
        proofs.push((
            attempt.identity.clone(),
            RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap(),
        ));
    }
    te13_fix1_stop_host(&r.f.host, &r.run);
    let producer = r.f.producer(&r.run);
    for (identity, proof) in &proofs {
        r.f.host
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
        r.f.projection(&r.run).cleanup,
        RunCleanup::Settled,
        "CAPACITY_STOP_SETTLED"
    );
    let candidate = r.f.brief.scope.base.branch.clone();
    let C5Fix10Run {
        f,
        run,
        child,
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
    let owner = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
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
            &history.source.checkpoint,
        )
        .unwrap();
    assert_eq!(
        checkpoint.batch_ids, batch_ids,
        "CAPACITY_BATCH_IDENTITIES_STABLE"
    );
    assert_eq!(
        *store.resolve_history(&checkpoint.history_ref).unwrap(),
        expected,
        "CAPACITY_REOPEN_EXACT"
    );
    let kept_candidate = owner.candidate_binding(&candidate).unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &kept_candidate.checkpoint,
        )
        .unwrap();
    let merged = store.resolve_history(&checkpoint.history_ref).unwrap();
    assert_eq!(merged.document, expected.document);
    assert_eq!(merged.batches.len(), 1, "CAPACITY_ONE_BOUNDARY_PER_MERGE");
    assert_eq!(checkpoint.history_role, HistoryRole::Candidate);
}
