#![cfg(target_os = "linux")]
//! C1 attachment checks at the actual transport and owner boundaries.
use super::*;
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    bundle_records::RecordKind,
    receipts::{CommandId, ReceiptResult},
    studio_records::*,
};

fn fixture() -> (
    kinewright_media::test_support::TempDirectory,
    WriterHost,
    Vec<ObjectRef>,
) {
    let parent = kinewright_media::test_support::workspace_target_dir();
    std::fs::create_dir_all(&parent).unwrap();
    let temp = kinewright_media::test_support::TempDirectory::new_in(&parent, "s3-capabilities");
    let path = temp.path("a.kinewright");
    let core = Core::spawn(Document::default()).unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    let refs = [
        b"original feedback".as_slice(),
        b"[]",
        b"displayed frame proof",
    ]
    .map(|bytes| {
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let sealed = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &bytes[..], &reference, &[])
            .unwrap();
        bundle.retain_evidence(&reference).unwrap();
        drop(sealed);
        reference
    });
    bundle.save(&core, &inputs).unwrap();
    drop(bundle);
    let host = WriterHost::start(HostConfig {
        project: "s3-capabilities".into(),
        path: Some(path),
        roots: SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
        origin: HostOrigin::Headless,
    })
    .unwrap();
    (temp, host, refs.into())
}

#[test]
fn s3_gui_source_admission_stays_private_and_command_bound() {
    let (_temp, host, _refs) = fixture();
    let private = host.client();
    let roots = private.bootstrap().roots;
    let client = "s".repeat(64);
    let editor = host
        .attach(
            "narrow-editor".into(),
            AttachmentRole::Editor,
            roots.clone(),
        )
        .unwrap()
        .with_manual_editor(client.clone());
    let private = private.with_manual_editor(client.clone());
    accepted(
        private
            .request(HostCommand::AcquireManual {
                client: client.clone(),
            })
            .unwrap(),
    );
    let outside = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c1-explicit-source",
    );
    let path = outside.path("chosen.wav");
    std::fs::write(
        &path,
        kinewright_media::test_support::wav_f32(&vec![0.25; 4800].into_boxed_slice(), 48_000, 1),
    )
    .unwrap();
    let owner = host.owner.upgrade().unwrap();
    let snapshot = {
        let mut owner = owner.lock().unwrap();
        let snapshot = HostSnapshot::capture(owner.core()).unwrap();
        owner.config.origin = HostOrigin::Gui {
            retention: HostSourceBinding {
                source: None,
                inputs: None,
                callback: private.bootstrap(),
            },
            initial_snapshot: snapshot.clone(),
            expected_root_sha256: None,
            expected_project_id: None,
        };
        snapshot
    };
    let command = HostCommand::PrepareOwnedMedia {
        client,
        path,
        asset_id: kinewright_core::AssetId(1),
        snapshot,
    };
    let mut forwarded = roots.clone();
    forwarded.read.push(outside.root().into());
    let before = state(&host);
    let denied = editor
        .with_roots(forwarded.clone())
        .request(command.clone())
        .unwrap();
    assert_eq!(
        denied.error.as_ref().map(|e| e.code.as_str()),
        Some("root_refused"),
        "C1_GUI_SOURCE_SCOPE"
    );
    assert_eq!(state(&host), before, "C1_GUI_SOURCE_SCOPE");
    let accepted = accepted(private.with_roots(forwarded).request(command).unwrap());
    assert!(
        accepted.owned_media.is_some(),
        "private picker source control"
    );
    let unrelated = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c1-unrelated-root",
    );
    let mut widened = roots;
    widened.read.push(unrelated.root().into());
    let before = state(&host);
    let denied = private
        .with_roots(widened)
        .request(HostCommand::Status)
        .unwrap();
    assert!(denied.error.is_some(), "C1_GUI_SOURCE_COMMAND_BOUND");
    assert_eq!(state(&host), before, "C1_GUI_SOURCE_COMMAND_BOUND");
}

fn request(client: &HostClient, command: HostCommand) -> HostRequest {
    HostRequest {
        protocol: HOST_PROTOCOL,
        project: client.endpoint().project.clone(),
        roots: client.bootstrap.roots.clone(),
        editor: client.editor.clone(),
        operation: None,
        command,
    }
}

fn direct(host: &WriterHost, client: &HostClient, command: HostCommand) -> HostReply {
    host.owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .execute_attached(&client.bootstrap.auth, request(client, command))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The fixture mirrors owned requests across the three ingress routes"
)]
fn mcp(client: &HostClient, command: HostCommand) -> Result<HostReply, HostError> {
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host_command","arguments":{"command":command}}});
    let response = client.post("mcp", &serde_json::to_vec(&body)?)?;
    let value: serde_json::Value = serde_json::from_slice(&response)?;
    Ok(serde_json::from_value(
        value["result"]["structuredContent"].clone(),
    )?)
}

fn state(host: &WriterHost) -> serde_json::Value {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    serde_json::json!({"root": owner.owner().unwrap().root_sha256().unwrap(), "history": owner.core().export_history().unwrap(), "lease": owner.manual, "next_manual": owner.next_manual_generation, "sessions": owner.sessions.len(), "next_session": owner.next_session, "dispatches": owner.dispatches, "status": owner.status(), "admitted": owner.admitted.len()})
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The fixture consumes one request and repeats it across the three ingress routes"
)]
fn refused(host: &WriterHost, client: &HostClient, command: HostCommand, marker: &str) {
    for route in 0..3 {
        let before = state(host);
        let result = match route {
            0 => {
                let (reply, trace) = kinewright_project::io_observer::observe(|| {
                    direct(host, client, command.clone())
                });
                assert!(
                    trace.events.is_empty() && !trace.overflowed,
                    "{marker}: refused IO side-effect count must be zero"
                );
                Ok(reply)
            }
            1 => client.request(command.clone()),
            _ => mcp(client, command.clone()),
        };
        assert!(
            result.is_err() || result.as_ref().unwrap().error.is_some(),
            "{marker}: route {route} accepted"
        );
        assert_eq!(
            state(host),
            before,
            "{marker}: refused side-effect count must be zero on route {route}"
        );
    }
}

fn metadata(host: &WriterHost, change: MetadataChange) -> HostCommand {
    let owner = host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let owner = host.owner_mut().unwrap();
    let issuer = owner.issue_metadata_issuer().unwrap();
    let envelope = owner
        .metadata_envelope(CommandId {
            issuer,
            sequence: 1,
        })
        .unwrap();
    let generation = owner.metadata_generation().unwrap();
    HostCommand::Metadata {
        envelope,
        update: MetadataUpdate {
            expected_root_generation: generation,
            change,
        },
    }
}

fn publish_command(host: &WriterHost) -> HostCommand {
    let (reference, checkpoint) = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .manual_checkpoint()
        .unwrap();
    metadata(
        host,
        MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: checkpoint.head_id,
            checkpoint_ref: reference,
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    )
}

fn accepted(reply: HostReply) -> HostReply {
    assert!(reply.error.is_none(), "{:?}", reply.error);
    reply
}

fn feedback_revision(key: SnapshotKey, refs: &[ObjectRef]) -> FeedbackRevision {
    FeedbackRevision {
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
        original_text_ref: refs[0].clone(),
        strokes_ref: refs[1].clone(),
        frame_proof_ref: refs[2].clone(),
        voice_ref: None,
        transcript_revision: None,
    }
}

fn feedback_controls(
    host: &WriterHost,
    viewer: &HostClient,
    producer: &HostClient,
    refs: &[ObjectRef],
) {
    let reply = accepted(producer.request(publish_command(host)).unwrap());
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = reply.metadata.unwrap().result
    else {
        panic!("snapshot result");
    };
    for route in 0..3 {
        let revision = feedback_revision(key.clone(), refs);
        let command = metadata(host, MetadataChange::SaveFeedback(revision.clone()));
        let reply = accepted(match route {
            0 => direct(host, viewer, command),
            1 => viewer.request(command).unwrap(),
            _ => mcp(viewer, command).unwrap(),
        });
        let ReceiptResult::Metadata {
            result: MetadataResult::Feedback(reference),
        } = reply.metadata.unwrap().result
        else {
            panic!("feedback result");
        };
        let owner = host.owner.upgrade().unwrap();
        let before_pins = owner
            .lock()
            .unwrap()
            .attachments
            .iter()
            .find(|grant| grant.auth.accepts(Some(&viewer.bootstrap.auth.bearer())))
            .unwrap()
            .read_pins
            .len();
        let read = accepted(
            viewer
                .request(HostCommand::ReadFeedback { reference })
                .unwrap(),
        );
        assert_eq!(
            read.feedback,
            Some(revision),
            "C1_ADMITTED_FEEDBACK_CONTROL"
        );
        let after_pins = owner
            .lock()
            .unwrap()
            .attachments
            .iter()
            .find(|grant| grant.auth.accepts(Some(&viewer.bootstrap.auth.bearer())))
            .unwrap()
            .read_pins
            .len();
        assert_eq!(after_pins, before_pins + 1, "C1_READ_PIN_RETENTION");
    }
}

fn viewer_attempts(
    host: &WriterHost,
    viewer: &HostClient,
    producer: &HostClient,
) -> Vec<(HostCommand, &'static str)> {
    let snapshot =
        HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
    let client = "e".repeat(64);
    let (envelope, identity) = {
        let owner = host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let lease = owner
            .issue_manual_lease(client.clone(), "t".repeat(64))
            .unwrap();
        let identity = owner.current_switch_identity().unwrap();
        let target = kinewright_project::bundle_switch::SwitchTarget::WorkingBranch {
            branch: lease.branch.clone(),
        };
        (owner.switch_envelope(&lease, target).unwrap(), identity)
    };
    let target = envelope.switch.as_ref().unwrap().target.clone();
    let commands = vec![
        (
            HostCommand::AcquireManual {
                client: client.clone(),
            },
            "C1_MANUAL_ROLE",
        ),
        (
            HostCommand::CommitHistory {
                client: client.clone(),
                messages: vec![],
            },
            "C1_MANUAL_STREAM_ROLE",
        ),
        (
            HostCommand::PrepareSwitch { target },
            "C1_PREPARE_SWITCH_ROLE",
        ),
        (
            HostCommand::Switch {
                envelope: Box::new(envelope.clone()),
            },
            "C1_SWITCH_ROLE",
        ),
        (
            HostCommand::SwitchReceipt {
                envelope: Box::new(envelope),
            },
            "C1_SWITCH_REPLAY_ROLE",
        ),
        (
            HostCommand::ConfirmSwitch {
                client,
                installed: Box::new(identity),
            },
            "C1_CONFIRM_SWITCH_ROLE",
        ),
        (HostCommand::Save { snapshot }, "C1_SAVE_ROLE"),
        (
            HostCommand::StartScripted {
                turns: vec![],
                prompts: vec![],
            },
            "C1_SESSION_ROLE",
        ),
        (
            HostCommand::StartSession {
                harness: "missing".into(),
                config: HostSessionConfig::default(),
            },
            "C1_SESSION_START_ROLE",
        ),
        (
            HostCommand::SendMessage {
                session: 1,
                message: "spoof".into(),
            },
            "C1_SESSION_CONTROL_ROLE",
        ),
        (HostCommand::Shutdown, "C1_SHUTDOWN_ROLE"),
    ];
    for (command, marker) in &commands {
        refused(host, viewer, command.clone(), marker);
    }
    let publish = publish_command(host);
    refused(host, viewer, publish.clone(), "C1_SNAPSHOT_ROLE");
    let original = accepted(producer.request(publish.clone()).unwrap());
    let replay = accepted(mcp(producer, publish.clone()).unwrap());
    assert_eq!(
        original.metadata, replay.metadata,
        "producer replay returns original receipt"
    );
    refused(host, viewer, publish, "C1_SNAPSHOT_REPLAY_ROLE");
    commands
}

#[test]
fn s3_viewer_capabilities() {
    let (temp, host, refs) = fixture();
    let roots = host.roots.clone();
    let viewer = host
        .attach(
            "person-viewer".into(),
            AttachmentRole::Viewer,
            roots.clone(),
        )
        .unwrap()
        .with_manual_editor("e".repeat(64));
    let producer = host
        .attach(
            "candidate-producer".into(),
            AttachmentRole::Producer,
            roots.clone(),
        )
        .unwrap();
    accepted(viewer.request(HostCommand::ReadHistory).unwrap());
    accepted(mcp(&viewer, HostCommand::Status).unwrap());
    viewer_attempts(&host, &viewer, &producer);
    feedback_controls(&host, &viewer, &producer, &refs);
    let editor = host.client().with_manual_editor("e".repeat(64));
    let grant = accepted(
        editor
            .request(HostCommand::AcquireManual {
                client: "e".repeat(64),
            })
            .unwrap(),
    );
    assert!(grant.manual_lease.is_some());
    let released_commands = viewer_attempts(&host, &viewer, &producer);
    feedback_controls(&host, &viewer, &producer, &refs);
    refused(
        &host,
        &producer,
        HostCommand::AcquireManual {
            client: "p".repeat(64),
        },
        "C1_PRODUCER_MANUAL_ROLE",
    );

    let published = accepted(producer.request(publish_command(&host)).unwrap());
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = published.metadata.as_ref().unwrap().result.clone()
    else {
        panic!("snapshot receipt");
    };
    let descriptor = accepted(
        viewer
            .request(HostCommand::ReadSnapshot { key: key.clone() })
            .unwrap(),
    );
    assert_eq!(descriptor.snapshot.as_ref().unwrap().key, key);
    assert!(
        !host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .attachments
            .iter()
            .find(|grant| grant.subject == "person-viewer")
            .unwrap()
            .read_pins
            .is_empty()
    );
    let revision = feedback_revision(key.clone(), &refs);
    let saved = accepted(
        mcp(
            &viewer,
            metadata(&host, MetadataChange::SaveFeedback(revision.clone())),
        )
        .unwrap(),
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(reference),
    } = saved.metadata.unwrap().result
    else {
        panic!("feedback receipt");
    };
    let read = accepted(
        viewer
            .request(HostCommand::ReadFeedback {
                reference: reference.clone(),
            })
            .unwrap(),
    );
    assert_eq!(read.feedback, Some(revision));
    let batch_id = FeedbackBatchId::fresh().unwrap();
    let mut submit = metadata(
        &host,
        MetadataChange::SubmitBatch {
            batch: FeedbackBatch {
                batch_id: batch_id.clone(),
                revision_refs: vec![reference],
            },
            request: ReviseRequest {
                command_id: published.metadata.as_ref().unwrap().id.clone(),
                batch_id: batch_id.clone(),
                expected_candidate_head: key.head.clone(),
            },
        },
    );
    if let HostCommand::Metadata {
        envelope,
        update:
            MetadataUpdate {
                change: MetadataChange::SubmitBatch { request, .. },
                ..
            },
    } = &mut submit
    {
        request.command_id = envelope.id.clone();
    }
    let submitted = accepted(viewer.request(submit).unwrap());
    assert!(
        matches!(submitted.revise_result, Some(ReviseResult::Waiting { .. })),
        "viewer revision submission cannot start a run"
    );
    let read = accepted(
        mcp(
            &producer,
            HostCommand::ReadSubmittedBatch { batch: batch_id },
        )
        .unwrap(),
    );
    assert_eq!(read.submitted_revisions.len(), 1);
    refused(
        &host,
        &producer,
        HostCommand::ReadFeedback {
            reference: refs[0].clone(),
        },
        "C1_PRODUCER_DRAFT_ROLE",
    );

    // A real C10 close releases the previous lease, with no lifecycle change.
    let begin = accepted(
        editor
            .request(HostCommand::BeginRecovery {
                client: "e".repeat(64),
                frozen: None,
                pending: vec![],
                discard_batches: None,
            })
            .unwrap(),
    );
    let barrier = begin.recovery.unwrap();
    assert!(
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .manual
            .is_none()
    );
    for (command, marker) in released_commands {
        refused(&host, &viewer, command, marker);
    }
    feedback_controls(&host, &viewer, &producer, &refs);
    let replay = published.metadata.as_ref().unwrap();
    assert!(matches!(replay.result, ReceiptResult::Metadata { .. }));
    accepted(
        editor
            .request(HostCommand::PublishBarrier {
                barrier: Box::new(barrier.clone()),
            })
            .unwrap(),
    );
    accepted(
        editor
            .request(HostCommand::ConfirmBarrier {
                barrier: Box::new(barrier.clone()),
                installed: barrier.after,
            })
            .unwrap(),
    );
    // C10 returns a new lease. Detach revokes the credential independently of it.
    accepted(viewer.request(HostCommand::Detach).unwrap());
    assert!(
        viewer.request(HostCommand::Status).is_err(),
        "C1_DETACH_REVOKES"
    );
    let fresh = host
        .attach(
            "person-viewer".into(),
            AttachmentRole::Viewer,
            roots.clone(),
        )
        .unwrap();
    refused(
        &host,
        &fresh,
        HostCommand::AcquireManual {
            client: "f".repeat(64),
        },
        "C1_REATTACH_ROLE",
    );
    let old_auth = fresh.bootstrap.auth.clone();
    drop(host);
    let replacement = WriterHost::start(HostConfig {
        project: "s3-capabilities".into(),
        path: Some(temp.path("a.kinewright")),
        roots: roots.clone(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let expired =
        HostClient::connect(replacement.endpoint.clone(), old_auth, roots.clone()).unwrap();
    assert!(
        expired.request(HostCommand::ReadHistory).is_err(),
        "C1_RESTART_REVOKES"
    );
    let fresh = replacement
        .attach(
            "person-viewer".into(),
            AttachmentRole::Viewer,
            roots.clone(),
        )
        .unwrap();
    let producer = replacement
        .attach("candidate-producer".into(), AttachmentRole::Producer, roots)
        .unwrap();
    viewer_attempts(&replacement, &fresh, &producer);
    feedback_controls(&replacement, &fresh, &producer, &refs);
}

#[test]
fn s3_attachment_scope_cap_and_wire_spoofing() {
    let (temp, host, _refs) = fixture();
    let unknown = HostClient::connect(
        host.endpoint.clone(),
        HostAuth::generate().unwrap(),
        host.roots.clone(),
    )
    .unwrap();
    refused(
        &host,
        &unknown,
        HostCommand::Status,
        "C1_UNKNOWN_CREDENTIAL",
    );
    let viewer = host
        .attach(
            "project-viewer".into(),
            AttachmentRole::Viewer,
            host.roots.clone(),
        )
        .unwrap();
    let mut wrong_project = request(&viewer, HostCommand::Status);
    wrong_project.project = "another-project".into();
    let before = state(&host);
    let reply = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .execute_attached(&viewer.bootstrap.auth, wrong_project);
    assert!(reply.error.is_some(), "C1_PROJECT_BINDING");
    assert_eq!(state(&host), before, "C1_PROJECT_BINDING");
    accepted(viewer.request(HostCommand::Detach).unwrap());
    let child = temp.path("narrow");
    std::fs::create_dir(&child).unwrap();
    let narrow = host
        .attach(
            "narrow".into(),
            AttachmentRole::Viewer,
            SessionRoots {
                read: vec![child.clone()],
                write: vec![],
            },
        )
        .unwrap();
    let expanded = narrow.with_roots(host.roots.clone());
    refused(
        &host,
        &expanded,
        HostCommand::ReadHistory,
        "C1_ROOT_NARROWING",
    );
    assert!(
        host.attach(
            "wide".into(),
            AttachmentRole::Editor,
            SessionRoots {
                read: vec![temp.root().parent().unwrap().into()],
                write: vec![]
            }
        )
        .is_err(),
        "C1_ISSUANCE_ROOTS"
    );
    let mut spoof = serde_json::to_value(request(&narrow, HostCommand::Status)).unwrap();
    spoof["role"] = "editor".into();
    assert!(
        narrow
            .post("host", &serde_json::to_vec(&spoof).unwrap())
            .is_err(),
        "C1_WIRE_ROLE_SPOOF"
    );
    for n in 2..64 {
        host.attach(
            format!("viewer-{n}"),
            AttachmentRole::Viewer,
            SessionRoots::default(),
        )
        .unwrap();
    }
    assert_eq!(
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .attachments
            .len(),
        64
    );
    assert!(
        host.attach(
            "overflow".into(),
            AttachmentRole::Producer,
            SessionRoots::default()
        )
        .is_err(),
        "C1_ATTACHMENT_CAP"
    );
    accepted(narrow.request(HostCommand::Detach).unwrap());
    assert!(
        host.attach(
            "replacement".into(),
            AttachmentRole::Viewer,
            SessionRoots::default()
        )
        .is_ok()
    );
}

#[test]
fn s3_viewer_snapshot_replay() {
    let (_temp, host, _refs) = fixture();
    let viewer = host
        .attach("viewer".into(), AttachmentRole::Viewer, host.roots.clone())
        .unwrap();
    let producer = host
        .attach(
            "producer".into(),
            AttachmentRole::Producer,
            host.roots.clone(),
        )
        .unwrap();
    let command = publish_command(&host);
    let original = accepted(producer.request(command.clone()).unwrap());
    let repeated = accepted(mcp(&producer, command.clone()).unwrap());
    assert_eq!(original.metadata, repeated.metadata);
    refused(&host, &viewer, command, "C1_SNAPSHOT_REPLAY_ROLE");
}

#[test]
fn s3_foreground_replay_is_attachment_subject_bound() {
    let (_temp, host, _refs) = fixture();
    let first = host
        .attach(
            "first-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("e".repeat(64));
    let second = host
        .attach(
            "second-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("e".repeat(64));
    let snapshot =
        HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
    let command = HostCommand::Save { snapshot };
    accepted(first.request_operation(command.clone(), Some(1)).unwrap());
    let before = state(&host);
    assert!(
        second.request_operation(command.clone(), Some(1)).is_err(),
        "C1_FOREGROUND_SUBJECT_REPLAY"
    );
    assert_eq!(state(&host), before, "C1_FOREGROUND_SUBJECT_REPLAY");
    accepted(first.request_operation(command, Some(1)).unwrap());
}

#[test]
fn s3_attachment_role_binding() {
    let (_temp, host, _refs) = fixture();
    let viewer = host
        .attach("viewer".into(), AttachmentRole::Viewer, host.roots.clone())
        .unwrap()
        .with_manual_editor("e".repeat(64));
    let editor = host.client().with_manual_editor("e".repeat(64));
    let snapshot =
        HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
    accepted(editor.request(HostCommand::Save { snapshot }).unwrap());
    refused(
        &host,
        &viewer,
        HostCommand::AcquireManual {
            client: "e".repeat(64),
        },
        "C1_ATTACH_ROLE_BINDING",
    );
}
