fn metadata_request(f: &Fixture, actor: &HostClient, change: MetadataChange) -> HostCommand {
    assert_eq!(actor.endpoint().project, f.host.endpoint.project);
    let issued = Fixture::issue(actor);
    HostCommand::Metadata {
        envelope: issued.envelope,
        update: MetadataUpdate {
            expected_root_generation: issued.root_generation,
            change,
        },
    }
}
fn submitted_batch(f: &Fixture) -> FeedbackBatchId {
    submitted_batch_at(f, f.brief.scope.base.head.clone())
}
fn submitted_batch_at(
    f: &Fixture,
    expected_candidate_head: kinewright_project::bundle_records::HeadId,
) -> FeedbackBatchId {
    use kinewright_project::studio_records::{
        CommentId, FeedbackAnchor, FeedbackRevision, FrameRate, Orientation,
    };
    let (checkpoint_ref, checkpoint) = f
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
    let publisher = f.actor(AttachmentRole::Producer, "snapshot-publisher");
    let reply = ok(
        publisher
            .request(metadata_request(
                f,
                &publisher,
                MetadataChange::PublishSnapshot(PublishSnapshot {
                    version: VersionId::fresh().unwrap(),
                    expected_head: checkpoint.head_id.clone(),
                    checkpoint_ref,
                    parent_version: None,
                    run_provenance: None,
                    revision_refs: vec![],
                }),
            ))
            .unwrap(),
        "H1_FEEDBACK_SNAPSHOT",
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = reply.metadata.unwrap().result
    else {
        panic!("H1_FEEDBACK_SNAPSHOT")
    };
    let evidence = f.brief.plan.clone();
    let strokes = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"[]")
        .unwrap();
    let revision = FeedbackRevision {
        comment_id: CommentId::fresh().unwrap(),
        revision: 1,
        predecessor: None,
        anchor: FeedbackAnchor {
            snapshot: key,
            frame: 0,
            fps: FrameRate {
                numerator: 24,
                denominator: 1,
            },
            raster_size: [1920, 1080],
            orientation: Orientation::Upright,
            target: None,
            source_ref: None,
            script_beat_id: None,
        },
        original_text_ref: evidence.clone(),
        strokes_ref: strokes,
        frame_proof_ref: evidence,
        voice_ref: None,
        transcript_revision: None,
    };
    let reply = ok(
        f.editor
            .request(metadata_request(
                f,
                &f.editor,
                MetadataChange::SaveFeedback(revision),
            ))
            .unwrap(),
        "H1_FEEDBACK_REVISION",
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(reference),
    } = reply.metadata.unwrap().result
    else {
        panic!("H1_FEEDBACK_REVISION")
    };
    let batch = FeedbackBatchId::fresh().unwrap();
    let issued = Fixture::issue(&f.editor);
    ok(
        f.editor
            .request(HostCommand::Metadata {
                update: MetadataUpdate {
                    expected_root_generation: issued.root_generation,
                    change: MetadataChange::SubmitBatch {
                        batch: FeedbackBatch {
                            batch_id: batch.clone(),
                            revision_refs: vec![reference],
                        },
                        request: ReviseRequest {
                            command_id: issued.envelope.id.clone(),
                            batch_id: batch.clone(),
                            expected_candidate_head,
                        },
                    },
                },
                envelope: issued.envelope,
            })
            .unwrap(),
        "H1_SUBMITTED_BATCH",
    );
    batch
}
#[test]
fn h1_run_and_feedback_publish_atomically_and_reopen() {
    let f = Fixture::new();
    let batch = submitted_batch(&f);
    let mut request = f.start_request(&f.editor);
    request.consume_batch = Some(batch.clone());
    let receipt = ok(
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            },
            0,
        ),
        "H1_ATOMIC_ACK",
    )
    .orchestration
    .unwrap();
    let run: RunId = request.envelope.identity("run").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    let submitted = host.owner().unwrap().read_submitted_batch(&batch).unwrap();
    assert_eq!(
        submitted
            .consumed_by
            .as_ref()
            .map(kinewright_project::studio_records::RunId::as_str),
        Some(run.0.as_str()),
        "H1_ATOMIC_CONSUMPTION"
    );
    assert_eq!(
        host.owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap()
            .sequence,
        receipt.sequence,
        "H1_ATOMIC_CONSUMPTION"
    );
    drop(submitted);
    drop(host);
    drop(owner);
    let Fixture { temp, host, .. } = f;
    drop(host);
    let mut owner = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    assert_eq!(
        owner
            .orchestration_replay(&request.envelope, Some(&batch))
            .unwrap(),
        Some(receipt),
        "H1_ATOMIC_REOPEN"
    );
    assert_eq!(
        owner
            .read_submitted_batch(&batch)
            .unwrap()
            .consumed_by
            .as_ref()
            .map(kinewright_project::studio_records::RunId::as_str),
        Some(run.0.as_str()),
        "H1_ATOMIC_REOPEN"
    );
    owner.save_active().unwrap();
    let copied = temp.path("s4-copy.kinewright");
    owner
        .save_as(
            &copied,
            kinewright_project::storage_budget::StorageLimits::default(),
        )
        .unwrap();
    drop(owner);
    let owner = Ja1EditingSession::open(&copied).unwrap();
    assert!(
        owner.orchestration_projection(&run).unwrap().is_none(),
        "H1_COPY_HAS_NO_INHERITED_RUN"
    );
    assert!(
        owner
            .read_submitted_batch(&batch)
            .unwrap()
            .consumed_by
            .is_none(),
        "H1_COPY_HAS_NO_RUN_CONSUMPTION"
    );
    drop(owner);
    let original = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    assert!(
        original.orchestration_projection(&run).unwrap().is_some(),
        "H1_ORIGINAL_KEEPS_RUN"
    );
    assert_eq!(
        original
            .read_submitted_batch(&batch)
            .unwrap()
            .consumed_by
            .as_ref()
            .map(kinewright_project::studio_records::RunId::as_str),
        Some(run.0.as_str()),
        "H1_ORIGINAL_KEEPS_CONSUMPTION"
    );
}
#[test]
fn h1_unknown_publication_retries_only_original_envelope() {
    let f = Fixture::new();
    let request = f.start_request(&f.editor);
    let uncertain = kinewright_project::io_observer::fail_once("root_parent_sync", || {
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            },
            0,
        )
    });
    assert!(uncertain.error.is_some(), "H1_UNKNOWN_SETUP");
    let mut changed = request.clone();
    changed.envelope.id.sequence += 1;
    changed.envelope.expected_generation += 1;
    changed.envelope.digest = changed.envelope.computed_digest().unwrap();
    let before = state(&f);
    let refused_result = call(
        &f,
        &f.editor,
        HostCommand::DispatchOrchestration {
            request: Box::new(changed),
        },
        0,
    );
    assert_eq!(
        refused_result
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("orchestration_refused"),
        "H1_UNKNOWN_ORIGINAL_ONLY"
    );
    assert_eq!(state(&f), before, "H1_UNKNOWN_ORIGINAL_ONLY");
    let receipt = ok(
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            },
            0,
        ),
        "H1_UNKNOWN_RECONCILED",
    )
    .orchestration
    .unwrap();
    let replay = ok(
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            2,
        ),
        "H1_UNKNOWN_REPLAY",
    )
    .orchestration
    .unwrap();
    assert_eq!(receipt, replay, "H1_UNKNOWN_ORIGINAL_RECEIPT");
}
#[test]
fn h3_metadata_subject_ordinary_command_refused_before_replay() {
    let f = Fixture::new();
    let first = f.actor(AttachmentRole::Viewer, "first");
    let second = f.actor(AttachmentRole::Viewer, "second");
    let batch = submitted_batch(&f);
    let source = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .read_submitted_batch(&batch)
        .unwrap()
        .revisions[0]
        .clone();
    let mut revision = source;
    revision.comment_id = kinewright_project::studio_records::CommentId::fresh().unwrap();
    let command = metadata_request(&f, &first, MetadataChange::SaveFeedback(revision));
    denied(
        &f,
        &second,
        command.clone(),
        "metadata_issuer",
        "H3_METADATA_ORDINARY_SUBJECT",
    );
    ok(
        call(&f, &first, command.clone(), 1),
        "H3_METADATA_ORDINARY_CONTROL",
    );
    denied(
        &f,
        &second,
        command,
        "metadata_issuer",
        "H3_METADATA_ORDINARY_REPLAY",
    );
}
#[test]
fn h3_recovery_freeze_blocks_metadata_and_hook_writes() {
    let f = Fixture::new();
    let batch = submitted_batch(&f);
    let mut revision = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .read_submitted_batch(&batch)
        .unwrap()
        .revisions[0]
        .clone();
    revision.comment_id = kinewright_project::studio_records::CommentId::fresh().unwrap();
    let metadata = metadata_request(&f, &f.editor, MetadataChange::SaveFeedback(revision));
    let start = f.start_request(&f.editor);
    f.host.owner.upgrade().unwrap().lock().unwrap().recovering = true;
    denied(
        &f,
        &f.editor,
        metadata,
        "recovery_frozen",
        "H3_RECOVERY_METADATA",
    );
    denied(
        &f,
        &f.editor,
        HostCommand::IssueMetadataIssuer { issuer: None },
        "recovery_frozen",
        "H3_RECOVERY_HOOK",
    );
    denied(
        &f,
        &f.editor,
        HostCommand::DispatchOrchestration {
            request: Box::new(start),
        },
        "recovery_frozen",
        "H3_RECOVERY_HOOK",
    );
}
#[test]
fn h3_controls_pause_and_revoke_task_endpoints() {
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let (_, endpoint) = task_actor(&f, &run, &execution, &producer);
    let (_, repeated) = task_actor(&f, &run, &execution, &producer);
    assert_eq!(
        endpoint.authority().endpoint,
        repeated.authority().endpoint,
        "H3_TASK_ISSUANCE_IDEMPOTENT"
    );
    let pause = f.control(&f.editor, &run, RunAction::Pause);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(pause),
            },
            0,
        ),
        "H3_PAUSE_ACK",
    );
    assert!(endpoint.check_live().is_err(), "H3_PAUSE_TASK_ENDPOINT");
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            },
            1,
        ),
        "H3_STOP_ACK",
    );
    endpoint.set_paused(false);
    assert!(endpoint.check_live().is_err(), "H3_STOP_TASK_ENDPOINT");
}
#[test]
fn h4_damaged_input_refuses_before_source_release() {
    let f = Fixture::with_media(true);
    let started = f.start();
    let run: RunId = started.envelope.identity("run").unwrap();
    let object = {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let manifest = host
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_input_manifest(&f.brief.inputs)
            .unwrap();
        assert!(!manifest.entries.is_empty(), "H4_NONEMPTY_CERTIFIED_INPUT");
        manifest.entries[0].object.clone()
    };
    let path = f.temp.path(&format!(
        "project.kinewright-assets/studio/objects/{}",
        object.sha256
    ));
    let before = std::fs::read(f.temp.path("project.kinewright")).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(1)
        .unwrap();
    for route in 0..3 {
        let reply = call(&f, &f.editor, HostCommand::DetachForContinuation, route);
        assert_eq!(
            reply.error.as_ref().map(|error| error.code.as_str()),
            Some("continuation_inputs"),
            "H4_CERTIFIED_INPUT_READY"
        );
        assert!(
            !reply
                .error
                .as_ref()
                .unwrap()
                .message
                .to_ascii_lowercase()
                .contains("capacity"),
            "H4_INPUT_DAMAGE_NOT_CAPACITY"
        );
        assert_eq!(
            std::fs::read(f.temp.path("project.kinewright")).unwrap(),
            before,
            "H4_REFUSED_HEADS_RETAINED"
        );
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        assert!(
            host.resolve_attachment(Some(&f.editor.bootstrap.auth.bearer()))
                .is_ok(),
            "H4_REFUSED_SOURCE_ATTACHED"
        );
        let runtime = host.hooks.runs.get(&run).unwrap();
        assert!(
            host.resolve_attachment(Some(&runtime.producer.bearer()))
                .is_ok(),
            "H4_REFUSED_PRODUCER_RETAINED"
        );
        assert!(
            host.owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .owned_bytes
                >= object.byte_length,
            "H4_REFUSED_CHARGE_RETAINED"
        );
    }
}
