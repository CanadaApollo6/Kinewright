// S3 C7: the S4 `ReviseCut` port. Included by hooks_tests.rs (Linux).
use kinewright_project::{io_observer, studio_records::ReviseResult};
fn run_of(id: &kinewright_project::studio_records::RunId) -> RunId {
    serde_json::from_value(serde_json::json!(id.as_str())).unwrap()
}
fn configure_director(f: &Fixture) -> std::path::PathBuf {
    let folder = f.temp.path("scheduler");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("director.json"),
        serde_json::to_vec(&super::super::DirectorSetting {
            version: 1,
            director: f.brief.provider.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .config
        .scheduler_settings = Some(folder.clone());
    folder
}
/// A batch submitted at the manual head, as the App submits it.
fn manual_batch(f: &Fixture) -> FeedbackBatchId {
    let head = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .manual_checkpoint()
        .unwrap()
        .1
        .head_id;
    submitted_batch_at(f, head)
}
fn no_director(f: &Fixture) {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .config
        .scheduler_settings = Some(f.temp.path("no-scheduler-settings"));
}
fn submitted_request(f: &Fixture, batch: &FeedbackBatchId) -> ReviseRequest {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .read_submitted_batch(batch)
        .unwrap()
        .request
        .clone()
}
fn revise(f: &Fixture, actor: &HostClient, request: &ReviseRequest) -> HostReply {
    call(
        f,
        actor,
        HostCommand::ReviseCut {
            request: request.clone(),
        },
        0,
    )
}
fn revise_result(reply: HostReply, marker: &str) -> ReviseResult {
    ok(reply, marker).revise_result.expect(marker)
}
fn run_count(f: &Fixture) -> usize {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_runs()
        .unwrap()
        .len()
}
fn consumed(f: &Fixture, batch: &FeedbackBatchId) -> Option<String> {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .read_submitted_batch(batch)
        .unwrap()
        .consumed_by
        .map(|run| run.as_str().to_owned())
}

#[test]
fn c7_revise_cut_starts_one_director_run_and_replays_it() {
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let first = revise_result(revise(&f, &f.editor, &request), "C7_ACCEPTED");
    let ReviseResult::Accepted { run_id, batch_id } = &first else {
        panic!("C7_ACCEPTED: {first:?}")
    };
    assert_eq!(batch_id, &batch, "C7_ACCEPTED");
    assert_eq!(
        consumed(&f, &batch).as_deref(),
        Some(run_id.as_str()),
        "C7_LINK"
    );
    let run = run_of(run_id);
    let projection = f.projection(&run);
    let director = &projection.tasks[0].brief;
    assert_eq!(director.role, Role::Director, "C7_S4_ADMISSION");
    assert_eq!(director.provider, f.brief.provider, "C7_S4_ADMISSION");
    let (_, manual) = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .manual_checkpoint()
        .unwrap();
    assert_eq!(
        manual.head_id, request.expected_candidate_head,
        "C7_CANDIDATE_AT_HEAD"
    );
    assert_eq!(
        director.scope.base.document_hash, manual.document_hash,
        "C7_CANDIDATE_AT_HEAD"
    );
    assert_ne!(
        director.scope.base.branch,
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .manual_branch_id()
            .unwrap(),
        "C7_CANDIDATE_AT_HEAD"
    );
    // A replay is the same Accepted, with no second run or consumption.
    let again = revise_result(revise(&f, &f.editor, &request), "C7_REPLAY");
    assert_eq!(again, first, "C7_REPLAY");
    assert_eq!(run_count(&f), 1, "C7_REPLAY");
    // The same command id naming another batch is a payload conflict.
    let mut changed = request.clone();
    changed.batch_id = FeedbackBatchId::fresh().unwrap();
    let conflict = revise(&f, &f.editor, &changed);
    assert_eq!(
        conflict.error.map(|error| error.code).as_deref(),
        Some("id_payload_conflict"),
        "C7_ID_PAYLOAD_CONFLICT"
    );
    let other = manual_batch(&f);
    let mut crossed = submitted_request(&f, &other);
    crossed.command_id = request.command_id.clone();
    let conflict = revise(&f, &f.editor, &crossed);
    assert_eq!(
        conflict.error.map(|error| error.code).as_deref(),
        Some("id_payload_conflict"),
        "C7_ID_PAYLOAD_CONFLICT"
    );
    assert_eq!(run_count(&f), 1, "C7_ID_PAYLOAD_CONFLICT");
}

#[test]
fn c7_revise_cut_replays_after_host_restart() {
    let f = Fixture::new();
    let folder = configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let first = revise_result(revise(&f, &f.editor, &request), "C7_RESTART");
    let Fixture { temp, host, .. } = f;
    host.shutdown().unwrap();
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let host = WriterHost::start(HostConfig {
        scheduler_settings: Some(folder),
        project: "hooks".into(),
        path: Some(temp.path("project.kinewright")),
        roots,
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let editor = host.client();
    let again = editor
        .request(HostCommand::ReviseCut {
            request: request.clone(),
        })
        .unwrap();
    assert!(again.error.is_none(), "C7_RESTART: {:?}", again.error);
    assert_eq!(again.revise_result, Some(first), "C7_RESTART");
    let owner = host.owner.upgrade().unwrap();
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_runs()
            .unwrap()
            .len(),
        1,
        "C7_RESTART"
    );
    drop(owner);
    host.shutdown().unwrap();
}

#[test]
fn c7_fault_before_the_run_publication_leaves_the_batch_unconsumed() {
    let publications = {
        let f = Fixture::new();
        configure_director(&f);
        let batch = manual_batch(&f);
        let request = submitted_request(&f, &batch);
        let (reply, trace) = io_observer::observe(|| revise(&f, &f.editor, &request));
        revise_result(reply, "C7_ATOMIC_LINK");
        trace
            .events
            .iter()
            .filter(|event| event.operation == "root_rename")
            .count()
    };
    assert!(publications >= 2, "C7_ATOMIC_LINK: fork, then the run");
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let admitted = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = std::rc::Rc::clone(&admitted);
    // The last root publication of the port is the run's own publication.
    let faulted = io_observer::at_barrier(
        "metadata_admission",
        move || seen.set(true),
        || {
            io_observer::fail_nth("root_before_publish", publications, || {
                revise(&f, &f.editor, &request)
            })
        },
    );
    assert!(
        admitted.get(),
        "C7_ATOMIC_LINK: the run publication was reached"
    );
    assert!(
        faulted
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("injected")),
        "C7_ATOMIC_LINK: {:?}",
        faulted.error
    );
    assert_eq!(consumed(&f, &batch), None, "C7_ATOMIC_LINK");
    assert_eq!(run_count(&f), 0, "C7_ATOMIC_LINK");
    // Still submitted and retryable: the same request now starts its run.
    let retried = revise_result(revise(&f, &f.editor, &request), "C7_ATOMIC_LINK");
    let ReviseResult::Accepted { run_id, .. } = retried else {
        panic!("C7_ATOMIC_LINK: {retried:?}")
    };
    assert_eq!(
        consumed(&f, &batch).as_deref(),
        Some(run_id.as_str()),
        "C7_ATOMIC_LINK"
    );
    assert_eq!(run_count(&f), 1, "C7_ATOMIC_LINK");
}

/// While the run publication's outcome is unknown, a retry with the same batch
/// but a changed request is `id_payload_conflict`, and the exact request still
/// settles its one run.
#[test]
fn c7_fix1_pending_retry_with_a_changed_request_is_conflict() {
    let syncs = {
        let f = Fixture::new();
        configure_director(&f);
        let batch = manual_batch(&f);
        let request = submitted_request(&f, &batch);
        let (reply, trace) = io_observer::observe(|| revise(&f, &f.editor, &request));
        revise_result(reply, "C7_PENDING_PAYLOAD");
        trace
            .events
            .iter()
            .filter(|event| event.operation == "root_rename")
            .count()
    };
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    // The run's own publication renames, then its parent sync fails: unknown.
    let unknown = io_observer::fail_nth("root_parent_sync", syncs, || {
        revise(&f, &f.editor, &request)
    });
    assert_eq!(
        unknown.error.as_ref().map(|error| error.code.as_str()),
        Some("durability_unknown"),
        "C7_PENDING_PAYLOAD: {:?}",
        unknown.error
    );
    let mut changed = request.clone();
    changed.expected_candidate_head = kinewright_project::bundle_records::HeadId::fresh().unwrap();
    let refused = revise(&f, &f.editor, &changed);
    assert_eq!(
        refused.error.as_ref().map(|error| error.code.as_str()),
        Some("id_payload_conflict"),
        "C7_PENDING_PAYLOAD: {:?}",
        refused.error
    );
    let settled = revise_result(revise(&f, &f.editor, &request), "C7_PENDING_PAYLOAD");
    let ReviseResult::Accepted { run_id, .. } = settled else {
        panic!("C7_PENDING_PAYLOAD: {settled:?}")
    };
    assert_eq!(
        consumed(&f, &batch).as_deref(),
        Some(run_id.as_str()),
        "C7_PENDING_PAYLOAD"
    );
    assert_eq!(run_count(&f), 1, "C7_PENDING_PAYLOAD");
}

#[test]
fn c7_moved_head_is_conflict() {
    use kinewright_core::{Command, Operation, Track, TrackId, TrackKind};
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    {
        let owner = f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let session = host.owner_mut().unwrap();
        session
            .core()
            .request(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(41),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            }))
            .unwrap();
        session.save_active().unwrap();
    }
    let result = revise_result(revise(&f, &f.editor, &request), "C7_CONFLICT");
    assert_eq!(result, ReviseResult::Conflict, "C7_CONFLICT");
    assert_eq!(consumed(&f, &batch), None, "C7_CONFLICT");
    assert_eq!(run_count(&f), 0, "C7_CONFLICT");
}

#[test]
fn c7_no_director_or_viewer_is_waiting_and_retryable() {
    let f = Fixture::new();
    no_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let result = revise_result(revise(&f, &f.editor, &request), "C7_WAITING");
    let ReviseResult::Waiting { reason } = &result else {
        panic!("C7_WAITING: {result:?}")
    };
    assert!(
        reason.contains("No Director provider"),
        "C7_WAITING: {reason}"
    );
    assert_eq!(consumed(&f, &batch), None, "C7_WAITING");
    assert_eq!(run_count(&f), 0, "C7_WAITING");
    configure_director(&f);
    let viewer = f.actor(AttachmentRole::Viewer, "c7-viewer");
    let waiting = revise_result(revise(&f, &viewer, &request), "C7_VIEWER_WAITS");
    assert!(
        matches!(waiting, ReviseResult::Waiting { .. }),
        "C7_VIEWER_WAITS: {waiting:?}"
    );
    let producer = f.actor(AttachmentRole::Producer, "c7-producer");
    assert!(
        revise(&f, &producer, &request).error.is_some(),
        "C7_PRODUCER_REFUSED"
    );
    assert_eq!(run_count(&f), 0, "C7_VIEWER_WAITS");
    // The same batch is retryable once a Director is set up.
    let accepted = revise_result(revise(&f, &f.editor, &request), "C7_WAITING_RETRY");
    assert!(
        matches!(accepted, ReviseResult::Accepted { .. }),
        "C7_WAITING_RETRY: {accepted:?}"
    );
    assert_eq!(run_count(&f), 1, "C7_WAITING_RETRY");
}

#[test]
fn c7_director_scope_names_the_whole_document_exactly() {
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let ReviseResult::Accepted { run_id, .. } =
        revise_result(revise(&f, &f.editor, &request), "C7_SCOPE")
    else {
        panic!("C7_SCOPE")
    };
    let projection = f.projection(&run_of(&run_id));
    let scope = &projection.tasks[0].brief.scope;
    assert!(scope.objects.contains("document"), "C7_SCOPE");
    assert!(scope.operations.contains("split_clip"), "C7_SCOPE");
    assert!(
        scope.fields.contains("clip.audio_gain_tenth_db"),
        "C7_SCOPE"
    );
    assert!(
        scope.objects.iter().all(|object| !object.contains('*')),
        "C7_SCOPE"
    );
}
