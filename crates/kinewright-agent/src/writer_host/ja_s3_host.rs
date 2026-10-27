#![cfg(target_os = "linux")]
//! C1 attachment checks at the actual transport and owner boundaries.
use super::*;
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    bundle_records::RecordKind,
    receipts::{CommandId, ReceiptResult},
    studio_records::*,
};

#[cfg(test)]
fn test_only_attach_editor_bypassing_issuance(
    host: &WriterHost,
    subject: String,
    roots: SessionRoots,
) -> Result<HostClient, HostError> {
    // Recreate multiple Editors solely to exercise the retained defence-in-depth rules.
    let owner = host.owner.upgrade().unwrap();
    let mut owner = owner.lock().unwrap();
    let auth = owner.issue_attachment(subject, AttachmentRole::Viewer, roots.clone())?;
    let index = owner.resolve_attachment(Some(&auth.bearer()))?;
    owner.attachments[index].role = AttachmentRole::Editor;
    drop(owner);
    HostClient::connect(host.endpoint.clone(), auth, roots)
}

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
    let editor =
        test_only_attach_editor_bypassing_issuance(&host, "narrow-editor".into(), roots.clone())
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
            AttachmentRole::Viewer,
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
    let first = test_only_attach_editor_bypassing_issuance(
        &host,
        "shared-editor".into(),
        host.roots.clone(),
    )
    .unwrap()
    .with_manual_editor("e".repeat(64));
    let second = test_only_attach_editor_bypassing_issuance(
        &host,
        "shared-editor".into(),
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

fn foreground_state(host: &WriterHost) -> Vec<u8> {
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    serde_json::to_vec(&(
        &owner.foreground_owner,
        &owner.foreground_reply,
        &owner.foreground_history,
    ))
    .unwrap()
}

fn foreground_request(
    host: &WriterHost,
    client: &HostClient,
    command: HostCommand,
    operation: Option<u64>,
    over_http: bool,
) -> HostReply {
    let mut request = request(client, command);
    request.operation = operation;
    if over_http {
        serde_json::from_slice(
            &client
                .post("host", &serde_json::to_vec(&request).unwrap())
                .unwrap(),
        )
        .unwrap()
    } else {
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .execute_attached(&client.bootstrap.auth, request)
    }
}

fn acquire_and_save_foreground(host: &WriterHost, editor: &HostClient, over_http: bool) {
    let grant = accepted(foreground_request(
        host,
        editor,
        HostCommand::AcquireManual {
            client: editor.editor.clone().unwrap(),
        },
        Some(1),
        over_http,
    ));
    assert!(
        grant.manual_lease.is_some(),
        "C1_FOREGROUND_EDITOR_ACQUIRES"
    );
    let snapshot =
        HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
    let saved = accepted(foreground_request(
        host,
        editor,
        HostCommand::Save { snapshot },
        Some(2),
        over_http,
    ));
    assert!(saved.saved.is_some(), "C1_FOREGROUND_EDITOR_SAVES");
}

fn non_editor_cannot_poison_foreground(over_http: bool) {
    let (_temp, host, _refs) = fixture();
    let editor = host.client().with_manual_editor("e".repeat(64));
    {
        let owner = host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        assert!(owner.manual.is_none(), "C1_FOREGROUND_BEFORE_ANY_LEASE");
        assert!(
            owner.foreground_owner.is_none() && owner.foreground_reply.is_none(),
            "C1_FOREGROUND_BEFORE_FIRST_OPERATION"
        );
    }
    let empty = foreground_state(&host);
    for (subject, role) in [
        ("forged-viewer", AttachmentRole::Viewer),
        ("forged-producer", AttachmentRole::Producer),
    ] {
        let client = host
            .attach(subject.into(), role, host.roots.clone())
            .unwrap();
        accepted(foreground_request(
            &host,
            &client,
            HostCommand::Status,
            None,
            over_http,
        ));
        for length in [32, 64, 128] {
            let forged = client.with_manual_editor("x".repeat(length));
            let before = state(&host);
            let denied =
                foreground_request(&host, &forged, HostCommand::Status, Some(1), over_http);
            assert_eq!(
                denied.error.as_ref().map(|error| error.code.as_str()),
                Some("role_refused"),
                "C1_FOREGROUND_ROLE_REFUSED: {subject}, editor length {length}"
            );
            assert_eq!(
                foreground_state(&host),
                empty,
                "C1_FOREGROUND_REFUSAL_PRESERVES_BYTES: {subject}, editor length {length}"
            );
            assert_eq!(
                state(&host),
                before,
                "C1_FOREGROUND_REFUSAL_HAS_NO_SIDE_EFFECTS: {subject}, editor length {length}"
            );
        }
    }
    acquire_and_save_foreground(&host, &editor, over_http);
}

#[test]
fn s3_foreground_replay_rejects_non_editor_http_before_lease() {
    non_editor_cannot_poison_foreground(true);
}

#[test]
fn s3_foreground_replay_rejects_non_editor_direct_before_lease() {
    non_editor_cannot_poison_foreground(false);
}

#[test]
fn s3_foreground_replay_detach_clears_only_owned_slot() {
    for (over_http, detach_operation) in [(false, None), (true, Some(2))] {
        let (_temp, host, _refs) = fixture();
        let first = test_only_attach_editor_bypassing_issuance(
            &host,
            "departing-editor".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("e".repeat(64));
        let empty = foreground_state(&host);
        accepted(foreground_request(
            &host,
            &first,
            HostCommand::Status,
            Some(1),
            over_http,
        ));
        let retained = foreground_state(&host);
        assert_ne!(retained, empty, "C1_FOREGROUND_DETACH_POPULATED_CONTROL");
        for (subject, role) in [
            ("unrelated-viewer", AttachmentRole::Viewer),
            ("unrelated-producer", AttachmentRole::Producer),
        ] {
            let unrelated = host
                .attach(subject.into(), role, host.roots.clone())
                .unwrap();
            accepted(foreground_request(
                &host,
                &unrelated,
                HostCommand::Detach,
                None,
                over_http,
            ));
            assert_eq!(
                foreground_state(&host),
                retained,
                "C1_FOREGROUND_UNRELATED_DETACH_PRESERVES_BYTES"
            );
        }
        accepted(foreground_request(
            &host,
            &first,
            HostCommand::Detach,
            detach_operation,
            over_http,
        ));
        assert_eq!(
            foreground_state(&host),
            empty,
            "C1_FOREGROUND_DETACH_CLEARS_OWNED_SLOT"
        );
        let revoked = direct(&host, &first, HostCommand::Status);
        assert_eq!(
            revoked.error.as_ref().map(|error| error.code.as_str()),
            Some("attachment_refused")
        );
        let next = test_only_attach_editor_bypassing_issuance(
            &host,
            "next-editor".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("n".repeat(64));
        acquire_and_save_foreground(&host, &next, over_http);
    }
}

#[test]
fn s3_foreground_replay_detach_clears_stale_non_editor_slot() {
    for (subject, role, over_http) in [
        ("stale-viewer", AttachmentRole::Viewer, false),
        ("stale-producer", AttachmentRole::Producer, true),
    ] {
        let (_temp, host, _refs) = fixture();
        let stale = host
            .attach(subject.into(), role, host.roots.clone())
            .unwrap()
            .with_manual_editor("x".repeat(64));
        let empty = foreground_state(&host);
        {
            let owner = host.owner.upgrade().unwrap();
            let mut owner = owner.lock().unwrap();
            // Recreate the cached state admitted before the attachment-role guard existed.
            accepted(owner.execute_operation(
                HostCommand::Status,
                host.roots.clone(),
                stale.editor.as_deref(),
                Some(1),
            ));
            let index = owner
                .resolve_attachment(Some(&stale.bootstrap.auth.bearer()))
                .unwrap();
            owner.foreground_owner = Some(super::manual::ForegroundOwner {
                attachment: Some(owner.attachments[index].id),
                client: stale.editor.clone().unwrap(),
            });
        }
        assert_ne!(
            foreground_state(&host),
            empty,
            "C1_FOREGROUND_STALE_SLOT_CONTROL"
        );
        accepted(foreground_request(
            &host,
            &stale,
            HostCommand::Detach,
            None,
            over_http,
        ));
        assert_eq!(
            foreground_state(&host),
            empty,
            "C1_FOREGROUND_DETACH_CLEARS_STALE_NON_EDITOR_SLOT: {subject}"
        );
        let editor = host.client().with_manual_editor("e".repeat(64));
        acquire_and_save_foreground(&host, &editor, over_http);
    }
}

fn discard_foreground_reply(
    host: &WriterHost,
    client: &HostClient,
    command: HostCommand,
    operation: u64,
    over_http: bool,
) -> HostReply {
    let mut request = request(client, command);
    request.operation = Some(operation);
    if over_http {
        let response = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap()
            .post(format!("http://{}/host", client.endpoint().address))
            .header("authorization", client.bootstrap.auth.bearer())
            .header("x-kinewright-project", &client.endpoint().project)
            .header("x-kinewright-protocol", HOST_PROTOCOL.to_string())
            .header(
                "x-kinewright-roots",
                client.bootstrap.roots.wire_header().unwrap(),
            )
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&request).unwrap())
            .send()
            .unwrap();
        assert!(response.status().is_success());
        drop(response);
    } else {
        drop(
            host.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .execute_attached(&client.bootstrap.auth, request),
        );
    }
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    accepted(
        owner
            .foreground_reply
            .as_ref()
            .expect("C2_LOST_REPLY_RETAINED")
            .3
            .clone(),
    )
}

fn assert_original_foreground(mut repeated: HostReply, original: &HostReply, marker: &str) {
    repeated.status = original.status.clone();
    assert_eq!(
        serde_json::to_value(repeated).unwrap(),
        serde_json::to_value(original).unwrap(),
        "{marker}"
    );
}

#[test]
fn s3_fix2_same_subject_detach_preserves_lost_grant() {
    for over_http in [false, true] {
        let (_temp, host, _refs) = fixture();
        let editor = test_only_attach_editor_bypassing_issuance(
            &host,
            "shared-subject".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("a".repeat(64));
        let command = HostCommand::AcquireManual {
            client: editor.editor.clone().unwrap(),
        };
        let original = discard_foreground_reply(&host, &editor, command.clone(), 1, over_http);
        assert!(original.manual_lease.is_some());
        for role in [AttachmentRole::Viewer, AttachmentRole::Producer] {
            for route in 0..3 {
                let other = host
                    .attach("shared-subject".into(), role, host.roots.clone())
                    .unwrap();
                let before = foreground_state(&host);
                accepted(match route {
                    0 => direct(&host, &other, HostCommand::Detach),
                    1 => other.request(HostCommand::Detach).unwrap(),
                    _ => mcp(&other, HostCommand::Detach).unwrap(),
                });
                assert_eq!(
                    foreground_state(&host),
                    before,
                    "C2_SAME_SUBJECT_DETACH_UNCHANGED: {role:?}, route {route}"
                );
                let repeated = accepted(foreground_request(
                    &host,
                    &editor,
                    command.clone(),
                    Some(1),
                    over_http,
                ));
                assert_eq!(
                    repeated.manual_lease, original.manual_lease,
                    "C2_ORIGINAL_GRANT_REPLAY"
                );
                assert_original_foreground(repeated, &original, "C2_ORIGINAL_GRANT_PAYLOAD");
            }
        }
        let second = test_only_attach_editor_bypassing_issuance(
            &host,
            "shared-subject".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor(editor.editor.clone().unwrap());
        let before = foreground_state(&host);
        let denied = foreground_request(&host, &second, command.clone(), Some(1), over_http);
        assert_eq!(
            denied.error.as_ref().map(|error| error.code.as_str()),
            Some("operation_changed"),
            "C2_SAME_CLIENT_OTHER_INSTANCE_REFUSED"
        );
        assert_eq!(foreground_state(&host), before);
        let snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        assert!(
            accepted(foreground_request(
                &host,
                &editor,
                HostCommand::Save { snapshot },
                Some(2),
                over_http
            ))
            .saved
            .is_some()
        );
    }
}

#[test]
fn s3_fix2_unnumbered_lease_guards_empty_slot() {
    for over_http in [false, true] {
        let (temp, host, _refs) = fixture();
        let editor = host.client().with_manual_editor("a".repeat(64));
        let other = test_only_attach_editor_bypassing_issuance(
            &host,
            "other-editor".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("b".repeat(64));
        let original = accepted(foreground_request(
            &host,
            &editor,
            HostCommand::AcquireManual {
                client: editor.editor.clone().unwrap(),
            },
            None,
            over_http,
        ));
        assert!(
            host.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .foreground_reply
                .is_none()
        );
        let before = foreground_state(&host);
        let forged = editor.with_manual_editor(other.editor.clone().unwrap());
        let denied = foreground_request(&host, &forged, HostCommand::Status, Some(1), over_http);
        assert_eq!(
            denied.error.as_ref().map(|e| e.code.as_str()),
            Some("manual_busy"),
            "C2_LEASE_CLIENT_GUARD"
        );
        assert_eq!(
            foreground_state(&host),
            before,
            "C2_LEASE_REFUSAL_SLOT_BYTES"
        );
        let snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        for command in [
            HostCommand::Status,
            HostCommand::Save {
                snapshot: snapshot.clone(),
            },
        ] {
            let denied = foreground_request(&host, &other, command, Some(1), over_http);
            assert_eq!(
                denied.error.as_ref().map(|e| e.code.as_str()),
                Some("manual_busy"),
                "C2_NONOWNER_LEASE_REFUSED"
            );
            assert_eq!(
                foreground_state(&host),
                before,
                "C2_NONOWNER_SLOT_UNCHANGED"
            );
        }
        let path = temp.path("chosen.wav");
        std::fs::write(
            &path,
            kinewright_media::test_support::wav_f32(
                &vec![0.25; 4800].into_boxed_slice(),
                48_000,
                1,
            ),
        )
        .unwrap();
        let same_client = other.with_manual_editor(editor.editor.clone().unwrap());
        let denied = foreground_request(
            &host,
            &same_client,
            HostCommand::PrepareOwnedMedia {
                client: editor.editor.clone().unwrap(),
                path,
                asset_id: kinewright_core::AssetId(1),
                snapshot: snapshot.clone(),
            },
            None,
            over_http,
        );
        assert_eq!(
            denied.error.as_ref().map(|error| error.code.as_str()),
            Some("operation_changed"),
            "C2_LEASE_RENEWAL_INSTANCE"
        );
        assert_eq!(foreground_state(&host), before, "C2_RENEWAL_SLOT_UNCHANGED");
        assert!(
            accepted(foreground_request(
                &host,
                &editor,
                HostCommand::Save { snapshot },
                Some(1),
                over_http
            ))
            .saved
            .is_some()
        );
        assert_eq!(
            host.owner.upgrade().unwrap().lock().unwrap().manual,
            original.manual_lease
        );
    }
}

#[test]
fn s3_fix2_detach_reattach_replays_lost_save() {
    for over_http in [false, true] {
        let (_temp, host, _refs) = fixture();
        let editor = test_only_attach_editor_bypassing_issuance(
            &host,
            "shared-subject".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("a".repeat(64));
        let lease = accepted(foreground_request(
            &host,
            &editor,
            HostCommand::AcquireManual {
                client: editor.editor.clone().unwrap(),
            },
            Some(1),
            over_http,
        ))
        .manual_lease
        .unwrap();
        let snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        let command = HostCommand::Save { snapshot };
        let original = discard_foreground_reply(&host, &editor, command.clone(), 2, over_http);
        assert!(original.saved.is_some());
        let root = host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .root_sha256()
            .unwrap();
        accepted(if over_http {
            mcp(&editor, HostCommand::Detach).unwrap()
        } else {
            direct(&host, &editor, HostCommand::Detach)
        });
        {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            assert!(
                owner
                    .foreground_owner
                    .as_ref()
                    .unwrap()
                    .attachment
                    .is_none()
            );
            assert_eq!(owner.manual.as_ref(), Some(&lease));
            assert_eq!(
                serde_json::to_value(
                    &owner
                        .foreground_reply
                        .as_ref()
                        .expect("C2_DETACH_RETAINS_SAVE")
                        .3
                )
                .unwrap(),
                serde_json::to_value(&original).unwrap(),
                "C2_DETACH_RETAINS_SAVE"
            );
        }
        let other = test_only_attach_editor_bypassing_issuance(
            &host,
            "shared-subject".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("b".repeat(64));
        let before = foreground_state(&host);
        for command in [
            HostCommand::Status,
            HostCommand::AcquireManual {
                client: other.editor.clone().unwrap(),
            },
        ] {
            let denied = foreground_request(&host, &other, command, Some(3), over_http);
            assert_eq!(
                denied.error.as_ref().map(|e| e.code.as_str()),
                Some("manual_busy"),
                "C2_DETACHED_LEASE_PROTECTED"
            );
            assert_eq!(foreground_state(&host), before);
        }
        let resumed = test_only_attach_editor_bypassing_issuance(
            &host,
            "shared-subject".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor(editor.editor.clone().unwrap());
        accepted(foreground_request(
            &host,
            &resumed,
            HostCommand::Status,
            None,
            over_http,
        ));
        {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            let index = owner
                .resolve_attachment(Some(&resumed.bootstrap.auth.bearer()))
                .unwrap();
            assert_eq!(
                owner.foreground_owner.as_ref().unwrap().attachment,
                Some(owner.attachments[index].id),
                "C2_UNNUMBERED_REATTACH_BINDS"
            );
        }
        let repeated = accepted(foreground_request(
            &host,
            &resumed,
            command,
            Some(2),
            over_http,
        ));
        assert_original_foreground(repeated, &original, "C2_REATTACH_ORIGINAL_SAVE");
        assert_eq!(
            host.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .root_sha256()
                .unwrap(),
            root,
            "C2_REATTACH_DOES_NOT_SAVE_TWICE"
        );
        let before = foreground_state(&host);
        let same_client = other.with_manual_editor(resumed.editor.clone().unwrap());
        let denied =
            foreground_request(&host, &same_client, HostCommand::Status, Some(3), over_http);
        assert_eq!(
            denied.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_changed"),
            "C2_REATTACHED_INSTANCE_EXCLUSIVE"
        );
        assert_eq!(foreground_state(&host), before);
        assert_eq!(
            host.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .manual
                .as_ref(),
            Some(&lease)
        );
    }
}

#[test]
fn s3_fix2_no_lease_acquisition_preempts_non_authority_slot() {
    for over_http in [false, true] {
        let (_temp, host, _refs) = fixture();
        let first = host.client().with_manual_editor("a".repeat(64));
        accepted(foreground_request(
            &host,
            &first,
            HostCommand::Status,
            Some(99),
            over_http,
        ));
        assert!(
            host.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .manual
                .is_none()
        );
        let second = test_only_attach_editor_bypassing_issuance(
            &host,
            "second-editor".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("b".repeat(64));
        let grant = foreground_request(
            &host,
            &second,
            HostCommand::AcquireManual {
                client: second.editor.clone().unwrap(),
            },
            Some(1),
            over_http,
        );
        assert!(
            grant.error.is_none()
                && grant
                    .manual_lease
                    .as_ref()
                    .is_some_and(|lease| Some(&lease.client) == second.editor.as_ref()),
            "C2_NO_LEASE_ACQUIRE_PREEMPTS: {:?}",
            grant.error
        );
        let snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        assert!(
            accepted(foreground_request(
                &host,
                &second,
                HostCommand::Save { snapshot },
                Some(2),
                over_http
            ))
            .saved
            .is_some()
        );
    }
}

#[test]
fn s3_fix2_no_lease_reads_and_refusals_cannot_replace_slot() {
    for over_http in [false, true] {
        let (_temp, host, _refs) = fixture();
        let first = host.client().with_manual_editor("a".repeat(64));
        let original = accepted(foreground_request(
            &host,
            &first,
            HostCommand::Status,
            Some(1),
            over_http,
        ));
        let other = test_only_attach_editor_bypassing_issuance(
            &host,
            "other-editor".into(),
            host.roots.clone(),
        )
        .unwrap()
        .with_manual_editor("b".repeat(64));
        let before = foreground_state(&host);
        let denied = foreground_request(&host, &other, HostCommand::Status, Some(1), over_http);
        assert_eq!(
            denied.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_changed"),
            "C2_NO_LEASE_READ_CANNOT_PREEMPT"
        );
        assert_eq!(foreground_state(&host), before);
        let denied = foreground_request(
            &host,
            &other,
            HostCommand::AcquireManual {
                client: first.editor.clone().unwrap(),
            },
            Some(2),
            over_http,
        );
        assert_eq!(
            denied.error.as_ref().map(|e| e.code.as_str()),
            Some("manual_client"),
            "C2_OPERATION_COMMAND_CLIENT_BOUND"
        );
        assert_eq!(foreground_state(&host), before);
        let mut snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        snapshot.history_sha256 = "0".repeat(64);
        let denied = foreground_request(
            &host,
            &first,
            HostCommand::Save { snapshot },
            Some(2),
            over_http,
        );
        assert_eq!(
            denied.error.as_ref().map(|e| e.code.as_str()),
            Some("history_changed")
        );
        assert_eq!(foreground_state(&host), before, "C2_REFUSAL_NOT_CACHED");
        let repeated = accepted(foreground_request(
            &host,
            &first,
            HostCommand::Status,
            Some(1),
            over_http,
        ));
        assert_original_foreground(repeated, &original, "C2_NO_LEASE_ORIGINAL_REPLY");
    }
}

fn issuance_state(host: &WriterHost) -> Vec<u8> {
    let observable = state(host);
    let foreground = foreground_state(host);
    let owner = host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let attachments: Vec<_> = owner
        .attachments
        .iter()
        .map(|grant| {
            (
                grant.id,
                grant.auth.bearer(),
                &grant.incarnation,
                &grant.project,
                &grant.subject,
                grant.role,
                &grant.roots,
                grant.bootstrap,
                grant.read_pins.len(),
            )
        })
        .collect();
    serde_json::to_vec(&(
        observable,
        foreground,
        owner.next_attachment_id,
        attachments,
    ))
    .unwrap()
}

#[test]
fn s3_fix3_second_editor_refused() {
    let (_temp, host, _refs) = fixture();
    let before = issuance_state(&host);
    let error = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .issue_attachment(
            "issued-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .map(|_| ())
        .expect_err("C3_SECOND_EDITOR_REFUSED")
        .refusal();
    assert_eq!(error.code, "attachment_role");
    assert_eq!(error.message, "one live Editor attachment");
    assert_eq!(issuance_state(&host), before, "C3_ISSUANCE_UNCHANGED");
    let error = host
        .attach(
            "attached-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .map(|_| ())
        .expect_err("C3_ATTACH_SECOND_EDITOR_REFUSED")
        .refusal();
    assert_eq!(error.code, "attachment_role");
    assert_eq!(error.message, "one live Editor attachment");
    assert_eq!(issuance_state(&host), before, "C3_ATTACH_UNCHANGED");
    for role in [AttachmentRole::Viewer, AttachmentRole::Producer] {
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .issue_attachment("same-subject".into(), role, host.roots.clone())
            .unwrap();
        host.attach("same-subject".into(), role, host.roots.clone())
            .unwrap();
    }
    accepted(host.client().request(HostCommand::Detach).unwrap());
    let auth = host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .issue_attachment(
            "reattached-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone(),
        )
        .expect("C3_ISSUE_AFTER_DETACH");
    let editor = HostClient::connect(host.endpoint.clone(), auth, host.roots.clone()).unwrap();
    let before = issuance_state(&host);
    assert_eq!(
        host.attach(
            "another-editor".into(),
            AttachmentRole::Editor,
            host.roots.clone()
        )
        .map(|_| ())
        .expect_err("C3_ISSUED_EDITOR_COUNTS")
        .refusal()
        .code,
        "attachment_role"
    );
    assert_eq!(issuance_state(&host), before);
    accepted(mcp(&editor, HostCommand::Detach).unwrap());
    host.attach(
        "next-editor".into(),
        AttachmentRole::Editor,
        host.roots.clone(),
    )
    .expect("C3_ATTACH_AFTER_DETACH");
    {
        let owner = host.owner.upgrade().unwrap();
        let mut owner = owner.lock().unwrap();
        while owner.attachments.len() < super::auth::ATTACHMENT_CAPACITY {
            let role = if owner.attachments.len().is_multiple_of(2) {
                AttachmentRole::Viewer
            } else {
                AttachmentRole::Producer
            };
            owner
                .issue_attachment("within-cap".into(), role, host.roots.clone())
                .unwrap();
        }
    }
    let before = issuance_state(&host);
    for (role, expected) in [
        (AttachmentRole::Editor, "attachment_role"),
        (AttachmentRole::Viewer, "attachment_capacity"),
        (AttachmentRole::Producer, "attachment_capacity"),
    ] {
        assert_eq!(
            host.attach("over-cap".into(), role, host.roots.clone())
                .map(|_| ())
                .expect_err("C3_SHARED_ATTACHMENT_CAP")
                .refusal()
                .code,
            expected
        );
        assert_eq!(issuance_state(&host), before);
    }
}

#[test]
fn s3_fix3_lost_save_as_and_switch_retry() {
    for over_http in [false, true] {
        for switching in [false, true] {
            let (temp, host, _refs) = fixture();
            let editor = host.client().with_manual_editor("a".repeat(64));
            accepted(foreground_request(
                &host,
                &editor,
                HostCommand::AcquireManual {
                    client: editor.editor.clone().unwrap(),
                },
                Some(1),
                over_http,
            ));
            let command = if switching {
                let target = {
                    let owner = host.owner.upgrade().unwrap();
                    let owner = owner.lock().unwrap();
                    let kinewright_project::bundle_recovery::RecoverySelection::Checkpoint {
                        root,
                        branch,
                    } = owner
                        .owner()
                        .unwrap()
                        .list_recovery_points()
                        .unwrap()
                        .remove(0)
                    else {
                        panic!("checkpoint required")
                    };
                    kinewright_project::bundle_switch::SwitchTarget::Checkpoint { root, branch }
                };
                let envelope = accepted(foreground_request(
                    &host,
                    &editor,
                    HostCommand::PrepareSwitch { target },
                    None,
                    over_http,
                ))
                .switch_envelope
                .unwrap();
                HostCommand::Switch {
                    envelope: Box::new(envelope),
                }
            } else {
                let snapshot =
                    HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core())
                        .unwrap();
                HostCommand::SaveAs {
                    destination: temp.path("saved-as.kinewright"),
                    snapshot,
                }
            };
            let original = discard_foreground_reply(&host, &editor, command.clone(), 2, over_http);
            let (root, identity) = {
                let owner = host.owner.upgrade().unwrap();
                let owner = owner.lock().unwrap();
                assert!(owner.manual.is_none(), "C3_HANDOFF_RELEASES_LEASE");
                assert_eq!(owner.switch_frozen, switching);
                (
                    owner.owner().unwrap().root_sha256().unwrap(),
                    owner.owner().unwrap().current_switch_identity().unwrap(),
                )
            };
            assert_eq!(original.switch_result.is_some(), switching);
            if !switching {
                assert_eq!(
                    original.saved.as_ref().unwrap().path,
                    temp.path("saved-as.kinewright")
                );
            }
            let retained = foreground_state(&host);
            for role in [AttachmentRole::Viewer, AttachmentRole::Producer] {
                let other = host
                    .attach("private-bootstrap".into(), role, host.roots.clone())
                    .unwrap()
                    .with_manual_editor("b".repeat(64));
                accepted(direct(&host, &other, HostCommand::Status));
                accepted(other.request(HostCommand::CurrentAttachment).unwrap());
                accepted(mcp(&other, HostCommand::CurrentAttachment).unwrap());
                refused(
                    &host,
                    &other,
                    HostCommand::AcquireManual {
                        client: other.editor.clone().unwrap(),
                    },
                    "C3_LOWER_ROLE_CANNOT_ACQUIRE",
                );
                refused(
                    &host,
                    &other,
                    HostCommand::ConfirmSwitch {
                        client: other.editor.clone().unwrap(),
                        installed: Box::new(identity.clone()),
                    },
                    "C3_LOWER_ROLE_CANNOT_CONFIRM",
                );
                let denied =
                    foreground_request(&host, &other, HostCommand::Status, Some(3), over_http);
                assert_eq!(
                    denied.error.as_ref().map(|error| error.code.as_str()),
                    Some("role_refused")
                );
                accepted(mcp(&other, HostCommand::Detach).unwrap());
                assert_eq!(
                    foreground_state(&host),
                    retained,
                    "C3_LOWER_ROLE_SLOT_UNCHANGED"
                );
            }
            accepted(foreground_request(
                &host,
                &editor,
                HostCommand::Detach,
                None,
                over_http,
            ));
            assert!(
                host.owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .foreground_reply
                    .is_some(),
                "C3_DETACH_RETAINS_HANDOFF"
            );
            let resumed = host
                .attach(
                    "reattached-editor".into(),
                    AttachmentRole::Editor,
                    host.roots.clone(),
                )
                .unwrap()
                .with_manual_editor(editor.editor.clone().unwrap());
            let repeated = foreground_request(&host, &resumed, command, Some(2), over_http);
            assert!(
                repeated.error.is_none(),
                "C3_REATTACH_REPLAYS_HANDOFF: {:?}",
                repeated.error
            );
            assert_original_foreground(repeated, &original, "C3_ORIGINAL_HANDOFF_RESULT");
            {
                let owner = host.owner.upgrade().unwrap();
                let owner = owner.lock().unwrap();
                assert_eq!(
                    owner.owner().unwrap().root_sha256().unwrap(),
                    root,
                    "C3_NO_SECOND_PUBLICATION"
                );
                assert_eq!(
                    owner.owner().unwrap().current_switch_identity().unwrap(),
                    identity
                );
                assert!(owner.manual.is_none());
            }
            let grant = if switching {
                HostCommand::ConfirmSwitch {
                    client: resumed.editor.clone().unwrap(),
                    installed: Box::new(original.switch_attachment.unwrap().identity().unwrap()),
                }
            } else {
                HostCommand::AcquireManual {
                    client: resumed.editor.clone().unwrap(),
                }
            };
            assert!(
                accepted(foreground_request(
                    &host,
                    &resumed,
                    grant,
                    Some(3),
                    over_http
                ))
                .manual_lease
                .is_some()
            );
            let snapshot =
                HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core())
                    .unwrap();
            assert!(
                accepted(foreground_request(
                    &host,
                    &resumed,
                    HostCommand::Save { snapshot },
                    Some(4),
                    over_http
                ))
                .saved
                .is_some()
            );
        }
    }
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

struct LostDetachedHandoff {
    host: WriterHost,
    _temp: kinewright_media::test_support::TempDirectory,
    client: String,
    command: HostCommand,
    original: HostReply,
}

fn lost_detached_handoff(over_http: bool, switching: bool) -> LostDetachedHandoff {
    let (temp, host, _refs) = fixture();
    let client = "a".repeat(64);
    let editor = host.client().with_manual_editor(client.clone());
    accepted(foreground_request(
        &host,
        &editor,
        HostCommand::AcquireManual {
            client: client.clone(),
        },
        Some(1),
        over_http,
    ));
    let command = if switching {
        let target = {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            let kinewright_project::bundle_recovery::RecoverySelection::Checkpoint { root, branch } =
                owner
                    .owner()
                    .unwrap()
                    .list_recovery_points()
                    .unwrap()
                    .remove(0)
            else {
                panic!("checkpoint")
            };
            kinewright_project::bundle_switch::SwitchTarget::Checkpoint { root, branch }
        };
        let envelope = accepted(foreground_request(
            &host,
            &editor,
            HostCommand::PrepareSwitch { target },
            None,
            over_http,
        ))
        .switch_envelope
        .unwrap();
        HostCommand::Switch {
            envelope: Box::new(envelope),
        }
    } else {
        let snapshot =
            HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
        HostCommand::SaveAs {
            destination: temp.path("handoff.kinewright"),
            snapshot,
        }
    };
    let original = discard_foreground_reply(&host, &editor, command.clone(), 2, over_http);
    assert!(
        host.owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .manual
            .is_none()
    );
    accepted(foreground_request(
        &host,
        &editor,
        HostCommand::Detach,
        None,
        over_http,
    ));
    LostDetachedHandoff {
        _temp: temp,
        host,
        client,
        command,
        original,
    }
}

#[test]
fn s3_followup_detached_handoff_requires_explicit_discard() {
    for over_http in [false, true] {
        for switching in [false, true] {
            let fixture = lost_detached_handoff(over_http, switching);
            let host = &fixture.host;
            let other_client = "b".repeat(64);
            let other = host
                .attach(
                    "replacement".into(),
                    AttachmentRole::Editor,
                    host.roots.clone(),
                )
                .unwrap()
                .with_manual_editor(other_client.clone());
            let installed = accepted(foreground_request(
                host,
                &other,
                HostCommand::CurrentAttachment,
                None,
                over_http,
            ))
            .switch_attachment
            .unwrap()
            .identity()
            .unwrap();
            let before = foreground_state(host);
            let root = state(host)["root"].clone();
            for operation in [None, Some(3)] {
                let snapshot =
                    HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core())
                        .unwrap();
                for command in [
                    HostCommand::AcquireManual {
                        client: other_client.clone(),
                    },
                    HostCommand::ConfirmSwitch {
                        client: other_client.clone(),
                        installed: Box::new(installed.clone()),
                    },
                    HostCommand::Save { snapshot },
                ] {
                    let denied = foreground_request(host, &other, command, operation, over_http);
                    assert_eq!(
                        denied.error.as_ref().map(|error| error.code.as_str()),
                        Some("handoff_pending"),
                        "C1_SEQUENTIAL_HANDOFF_REFUSED: switch={switching}, operation={operation:?}"
                    );
                    assert_eq!(
                        foreground_state(host),
                        before,
                        "C1_SEQUENTIAL_HANDOFF_BYTES"
                    );
                    assert_eq!(state(host)["root"], root);
                    assert!(
                        host.owner
                            .upgrade()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .manual
                            .is_none()
                    );
                }
            }
            assert_eq!(
                host.discard_detached_handoff("wrong-client")
                    .expect_err("C1_HANDOFF_DISCARD_EXPECTED_CLIENT")
                    .refusal()
                    .code,
                "handoff_changed"
            );
            assert_eq!(foreground_state(host), before);
            accepted(foreground_request(
                host,
                &other,
                HostCommand::Detach,
                None,
                over_http,
            ));
            let resumed = host
                .attach(
                    "original-client".into(),
                    AttachmentRole::Editor,
                    host.roots.clone(),
                )
                .unwrap()
                .with_manual_editor(fixture.client.clone());
            let repeated = accepted(foreground_request(
                host,
                &resumed,
                fixture.command.clone(),
                Some(2),
                over_http,
            ));
            assert_original_foreground(
                repeated,
                &fixture.original,
                "C1_SEQUENTIAL_ORIGINAL_HANDOFF",
            );
            assert_eq!(
                state(host)["root"],
                root,
                "C1_SEQUENTIAL_NO_SECOND_PUBLICATION"
            );
            accepted(foreground_request(
                host,
                &resumed,
                HostCommand::Detach,
                None,
                over_http,
            ));
            let replacement = host
                .attach(
                    "explicit-discard".into(),
                    AttachmentRole::Editor,
                    host.roots.clone(),
                )
                .unwrap()
                .with_manual_editor(other_client.clone());
            host.discard_detached_handoff(&fixture.client)
                .expect("C1_TRUSTED_HANDOFF_DISCARD");
            {
                let owner = host.owner.upgrade().unwrap();
                let owner = owner.lock().unwrap();
                assert!(owner.foreground_reply.is_none() && owner.foreground_owner.is_none());
            }
            assert_eq!(state(host)["root"], root);
            let grant = if switching {
                HostCommand::ConfirmSwitch {
                    client: other_client,
                    installed: Box::new(installed),
                }
            } else {
                HostCommand::AcquireManual {
                    client: other_client,
                }
            };
            assert!(
                accepted(foreground_request(
                    host,
                    &replacement,
                    grant,
                    Some(3),
                    over_http
                ))
                .manual_lease
                .is_some()
            );
            let snapshot =
                HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core())
                    .unwrap();
            assert!(
                accepted(foreground_request(
                    host,
                    &replacement,
                    HostCommand::Save { snapshot },
                    Some(4),
                    over_http
                ))
                .saved
                .is_some()
            );
        }
    }
}

fn metadata_feedback_reference(
    host: &WriterHost,
    viewer: &HostClient,
    revision: FeedbackRevision,
) -> ObjectRef {
    let reply = accepted(direct(
        host,
        viewer,
        metadata(host, MetadataChange::SaveFeedback(revision)),
    ));
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(reference),
    } = reply.metadata.unwrap().result
    else {
        panic!("feedback")
    };
    reference
}

#[test]
fn s3_followup_repeated_reads_dedupe_reference_pins_at_capacity() {
    let (temp, host, refs) = fixture();
    let viewer = host
        .attach("reader".into(), AttachmentRole::Viewer, host.roots.clone())
        .unwrap();
    let producer = host
        .attach(
            "snapshot-publisher".into(),
            AttachmentRole::Producer,
            host.roots.clone(),
        )
        .unwrap();
    let published = accepted(direct(&host, &producer, publish_command(&host)));
    let command_id = published.metadata.as_ref().unwrap().id.clone();
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = published.metadata.unwrap().result
    else {
        panic!("snapshot")
    };
    let revision = feedback_revision(key.clone(), &refs);
    let reference = metadata_feedback_reference(&host, &viewer, revision.clone());
    let batch = FeedbackBatchId::fresh().unwrap();
    let mut submit = metadata(
        &host,
        MetadataChange::SubmitBatch {
            batch: FeedbackBatch {
                batch_id: batch.clone(),
                revision_refs: vec![reference.clone()],
            },
            request: ReviseRequest {
                command_id,
                batch_id: batch.clone(),
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
    accepted(direct(&host, &viewer, submit));
    let commands = [
        HostCommand::ReadSnapshot { key: key.clone() },
        HostCommand::ReadFeedback {
            reference: reference.clone(),
        },
        HostCommand::ReadSubmittedBatch { batch },
    ];
    let pin_count = || {
        let owner = host.owner.upgrade().unwrap();
        let owner = owner.lock().unwrap();
        let index = owner
            .resolve_attachment(Some(&viewer.bootstrap.auth.bearer()))
            .unwrap();
        owner.attachments[index].read_pins.len()
    };
    for (index, command) in commands.iter().enumerate() {
        for route in 0..70 {
            let read = accepted(match route % 3 {
                0 => direct(&host, &viewer, command.clone()),
                1 => viewer.request(command.clone()).unwrap(),
                _ => mcp(&viewer, command.clone()).unwrap(),
            });
            match index {
                0 => assert_eq!(read.snapshot.unwrap().key, key),
                1 => assert_eq!(read.feedback.unwrap(), revision),
                _ => assert_eq!(read.submitted_revisions, vec![revision.clone()]),
            }
            assert_eq!(pin_count(), index + 1, "C1_REPEATED_READ_PIN_DEDUPE");
        }
    }
    let editor = host.client().with_manual_editor("p".repeat(64));
    accepted(foreground_request(
        &host,
        &editor,
        HostCommand::AcquireManual {
            client: editor.editor.clone().unwrap(),
        },
        Some(1),
        false,
    ));
    let snapshot =
        HostSnapshot::capture(host.owner.upgrade().unwrap().lock().unwrap().core()).unwrap();
    accepted(foreground_request(
        &host,
        &editor,
        HostCommand::SaveAs {
            destination: temp.path("reader-copy.kinewright"),
            snapshot,
        },
        Some(2),
        false,
    ));
    for (index, command) in commands.iter().enumerate() {
        accepted(direct(&host, &viewer, command.clone()));
        assert_eq!(pin_count(), 4 + index, "C1_READ_PIN_OWNER_IDENTITY");
        accepted(direct(&host, &viewer, command.clone()));
        assert_eq!(pin_count(), 4 + index);
    }
    for _ in 6..64 {
        let reference =
            metadata_feedback_reference(&host, &viewer, feedback_revision(key.clone(), &refs));
        accepted(direct(
            &host,
            &viewer,
            HostCommand::ReadFeedback { reference },
        ));
    }
    assert_eq!(pin_count(), 64);
    for command in &commands {
        accepted(direct(&host, &viewer, command.clone()));
    }
    assert_eq!(pin_count(), 64, "C1_PIN_CAPACITY_REPEAT_CONTROL");
    let extra = metadata_feedback_reference(&host, &viewer, feedback_revision(key, &refs));
    let root = state(&host)["root"].clone();
    let denied = direct(
        &host,
        &viewer,
        HostCommand::ReadFeedback {
            reference: extra.clone(),
        },
    );
    assert_eq!(
        denied.error.unwrap().code,
        "attachment_capacity",
        "C1_DISTINCT_PIN_CAPACITY"
    );
    assert_eq!(pin_count(), 64);
    assert_eq!(state(&host)["root"], root);
    accepted(mcp(&viewer, HostCommand::Detach).unwrap());
    let replacement = host
        .attach(
            "fresh-reader".into(),
            AttachmentRole::Viewer,
            host.roots.clone(),
        )
        .unwrap();
    accepted(direct(
        &host,
        &replacement,
        HostCommand::ReadFeedback { reference: extra },
    ));
}
