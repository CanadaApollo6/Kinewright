#[derive(Clone, Copy, Debug)]
enum PersonLatch {
    Storage,
    Specialist,
    Sends,
    Attempts,
    Cuts,
}

#[allow(
    clippy::too_many_lines,
    reason = "one fixture constructs validated storage and send-count latches with dirty task work"
)]
fn person_latched_fixture(latch: PersonLatch) -> (Fixture, RunId, HostClient) {
    person_latched_fixture_inner(latch, true, false)
}

#[allow(
    clippy::too_many_lines,
    reason = "shared real run and dirty-task fixture with explicit latch controls"
)]
fn person_latched_fixture_inner(
    latch: PersonLatch,
    private_claim: bool,
    staged: bool,
) -> (Fixture, RunId, HostClient) {
    use kinewright_project::orchestration_accounting::{
        ProviderMetadata, ProviderSelection, SendConsumption,
    };
    let mut document = Document::default();
    kinewright_core::apply_batch(
        &mut document,
        &[
            kinewright_core::Operation::AddTrack {
                track: kinewright_core::Track {
                    id: kinewright_core::TrackId(81),
                    kind: kinewright_core::TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            },
            kinewright_core::Operation::AddSolidClip {
                track: kinewright_core::TrackId(81),
                timeline_start: kinewright_core::TimeCode(0),
                duration: kinewright_core::TimeCode(100),
                color: kinewright_core::SolidColor::default(),
            },
        ],
    )
    .unwrap();
    let f = Fixture::with_document(false, document);
    let mut f = if matches!(latch, PersonLatch::Storage) {
        c5_fix1_apply_limits(
            f,
            kinewright_project::storage_budget::StorageLimits {
                objects: 512,
                ..Default::default()
            },
        )
    } else if matches!(latch, PersonLatch::Specialist) {
        c5_fix1_apply_limits(
            f,
            kinewright_project::storage_budget::StorageLimits {
                versions: kinewright_project::run_admission::SPECIALIST_BASES + 4,
                ..Default::default()
            },
        )
    } else {
        f
    };
    f.host.runtime.stop();
    let document = {
        let host = f.host.owner.upgrade().unwrap();
        let mut host = host.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        if !owner
            .durable_sessions()
            .iter()
            .any(|(_, guard)| guard.branch_id() == &f.brief.scope.base.branch)
        {
            owner.save_active().unwrap();
            let mut candidate = owner
                .fork_durable_branch("person-pressure-candidate")
                .unwrap();
            owner.persist_durable_branch(&mut candidate).unwrap();
            f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
        }
        owner.core().export_history().unwrap().document
    };
    f.brief.scope =
        crate::orchestration::policy::director_scope(&document, f.brief.scope.base.clone())
            .unwrap();
    f.brief
        .scope
        .objects
        .extend(["marker:1".into(), "marker:2".into()]);
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (execution, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    c4_mcp_edit_redo(&C4Mcp::new(&f, &run, &child), &core);
    if private_claim {
        let claim_root = f.temp.path("person-machine");
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &claim_root,
            parent.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        claim.reserve(&unbound_identity(&parent)).unwrap();
        claim.reserve(&unbound_identity(&execution)).unwrap();
        drop(claim);
    }
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    if staged {
        let mut state = owner.spending_scope(&producer, &run).unwrap();
        owner
            .persist_runtime_task(&mut state, &execution, false)
            .unwrap();
        // Deliberately leave the paired H1 cut unpublished at peak overlap.
    }
    if matches!(latch, PersonLatch::Storage) {
        let mut refused = false;
        for n in 0_u64..256 {
            match owner
                .owner_mut()
                .unwrap()
                .admit_host_grant(&n.to_le_bytes())
            {
                Ok(_) => {}
                Err(kinewright_project::bundle::BundleError::RunLimit) => {
                    refused = true;
                    break;
                }
                Err(error) => panic!("PERSON_STORAGE_WRONG_REFUSAL {error}"),
            }
        }
        assert!(refused, "PERSON_STORAGE_LATCH");
    } else if matches!(latch, PersonLatch::Specialist) {
        let mut refused = false;
        match owner
            .owner_mut()
            .unwrap()
            .fork_durable_branch("specialist quota pressure")
        {
            Ok(mut branch) => {
                for _ in 0..16 {
                    let locked =
                        branch.core().export_history().unwrap().document.tracks[0].sync_lock;
                    assert!(matches!(
                        branch
                            .core()
                            .request(kinewright_core::Command::Do(
                                kinewright_core::Operation::SetTrackSyncLock {
                                    track: kinewright_core::TrackId(81),
                                    locked: !locked
                                }
                            ))
                            .unwrap(),
                        kinewright_core::Event::DocumentChanged { .. }
                    ));
                    match owner
                        .owner_mut()
                        .unwrap()
                        .persist_durable_branch(&mut branch)
                    {
                        Ok(_) => {}
                        Err(kinewright_project::bundle::BundleError::RunLimit) => {
                            refused = true;
                            break;
                        }
                        Err(error) => panic!("C3_SPECIALIST_WRONG_REFUSAL {error}"),
                    }
                }
            }
            Err(kinewright_project::bundle::BundleError::RunLimit) => refused = true,
            Err(error) => panic!("C3_SPECIALIST_WRONG_REFUSAL {error}"),
        }
        assert!(refused, "C3_SPECIALIST_SLOT_LATCH");
    } else if matches!(latch, PersonLatch::Sends) {
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        let previous = owner.native_accounting(&state).unwrap().unwrap();
        let mut next = previous.successor().unwrap();
        for (task, attempt, count) in [
            (&parent.task, &parent.attempt, 80_u64),
            (&child, &execution.attempt, 48),
        ] {
            if !next.tasks.iter().any(|entry| entry.task == task.0) {
                next.add_task(task.0.clone(), None).unwrap();
            }
            let entry = next.task_mut(&task.0).unwrap();
            let provider = ProviderSelection {
                connector: "scripted".into(),
                model: "fixture".into(),
                options: vec![],
            };
            entry
                .select_provider(ProviderMetadata {
                    attempt: attempt.0.clone(),
                    requested: provider.clone(),
                    resolved: provider,
                    cli_version: "fixture".into(),
                })
                .unwrap();
            for turn in 1..=count {
                entry
                    .consume_send(SendConsumption {
                        command: CommandId {
                            issuer: serde_json::from_value(serde_json::json!(
                                kinewright_project::bundle_records::ProjectId::fresh()
                                    .unwrap()
                                    .as_str()
                            ))
                            .unwrap(),
                            sequence: turn,
                        },
                        attempt: attempt.0.clone(),
                        turn,
                        payload: f.brief.assignment.clone(),
                    })
                    .unwrap();
            }
        }
        next.validate_successor(&previous).unwrap();
        owner
            .owner_mut()
            .unwrap()
            .publish_run_accounting(&run, &next)
            .unwrap();
        assert!(
            matches!(
                owner.owner().unwrap().check_run_send_budget(&next),
                Err(kinewright_project::bundle::BundleError::RunLimit)
            ),
            "PERSON_SEND_CAP_LATCH"
        );
    }
    if matches!(latch, PersonLatch::Attempts | PersonLatch::Cuts) {
        // Isolate the shared sticky latch; actual attempt/cut admission guards
        // retain their independent unit witnesses. No sends are seeded here.
        owner
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .exhaust_run_budget_probe();
    }
    assert!(owner.owner().unwrap().store().unwrap().run_limit_reached());
    drop(owner);
    drop(host);
    (f, run, client)
}

fn person_history_redo(core: &Core, marker: u64) -> kinewright_core::HistorySnapshot {
    for command in [
        kinewright_core::Command::Do(history_marker(marker)),
        kinewright_core::Command::Do(history_marker(marker + 1)),
        kinewright_core::Command::Undo,
    ] {
        assert!(
            matches!(
                core.request(command).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ),
            "PERSON_MANUAL_EDIT"
        );
    }
    core.export_history().unwrap()
}

fn person_voice_wav() -> Vec<u8> {
    // The production capture contract is mono 16 kHz PCM16, as in voice_tests::wav.
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(32_036_u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(16_000_u32.to_le_bytes());
    bytes.extend(32_000_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(32_000_u32.to_le_bytes());
    for _ in 0..16_000 {
        bytes.extend(1_000_i16.to_le_bytes());
    }
    bytes
}

fn person_save(f: &Fixture, marker: u64) -> kinewright_core::HistorySnapshot {
    let history = {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        person_history_redo(owner.core(), marker)
    };
    let saved = ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: HostSnapshot::from_history(history.clone()).unwrap(),
            })
            .unwrap(),
        "PERSON_SAVE_APPLIED",
    );
    assert!(saved.saved.is_some(), "PERSON_VERIFIED_SAVE");
    assert_eq!(
        saved.history.as_ref(),
        Some(&history),
        "PERSON_SAVE_EXACT_REDO"
    );
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    assert!(
        matches!(
            owner
                .owner_mut()
                .unwrap()
                .admit_host_grant(b"ordinary run still refused"),
            Err(kinewright_project::bundle::BundleError::RunLimit)
        ),
        "PERSON_SAVE_DID_NOT_RELEASE_RUN"
    );
    history
}

fn person_snapshot(f: &Fixture, actor: &HostClient) -> HostCommand {
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
    metadata_request(
        f,
        actor,
        MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: checkpoint.head_id,
            checkpoint_ref,
            parent_version: None,
            run_provenance: None,
            revision_refs: vec![],
        }),
    )
}

fn person_metadata(f: &Fixture) -> kinewright_project::studio_records::SnapshotKey {
    use kinewright_project::studio_records::{FeedbackEvidenceKind, MetadataResult};
    let mut refs = Vec::new();
    for (kind, bytes) in [
        (FeedbackEvidenceKind::Text, b"person feedback".as_slice()),
        (FeedbackEvidenceKind::Strokes, b"[[[0.1,0.2],[0.3,0.4]]]"),
        (FeedbackEvidenceKind::FrameProof, b"{}"),
    ] {
        let reply = ok(
            f.editor
                .request(HostCommand::AdmitFeedbackEvidence {
                    kind,
                    bytes: bytes.to_vec(),
                })
                .unwrap(),
            "PERSON_FEEDBACK_EVIDENCE",
        );
        refs.push(reply.feedback_evidence.unwrap());
    }
    let publisher = f.actor(AttachmentRole::Producer, "person-version");
    let snapshot = ok(
        publisher.request(person_snapshot(f, &publisher)).unwrap(),
        "PERSON_VERSION",
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = snapshot.metadata.unwrap().result
    else {
        panic!("PERSON_VERSION_RESULT")
    };
    let mut revision = super::super::ja_s3_host::feedback_revision(key, &refs);
    let audio = person_voice_wav();
    {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        let document = owner.core().export_history().unwrap().document;
        revision.anchor.fps.numerator = document.fps.numerator();
        revision.anchor.fps.denominator = document.fps.denominator();
    }
    let mut log = kinewright_media::voice_note::PlayheadLog::new(revision.anchor.clone());
    log.version = 2;
    log.capture_status = Some(kinewright_media::voice_note::CaptureStatus {
        complete: true,
        capture_error: None,
    });
    let log = serde_json::to_vec(&log).unwrap();
    let reference = |bytes: &[u8]| kinewright_project::bundle_records::ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
    };
    let issuer = Fixture::issue(&f.editor);
    let upload = kinewright_project::voice_ingress::VoiceUpload {
        envelope: issuer.envelope,
        root_generation: issuer.root_generation,
        note: kinewright_project::studio_records::NoteId::fresh().unwrap(),
        duration_micros: 1_000_000,
        audio: reference(&audio),
        log: reference(&log),
    };
    let voice = ok(
        f.editor
            .acknowledge_voice(upload, std::io::Cursor::new([audio, log].concat()))
            .unwrap(),
        "PERSON_SPOKEN_FEEDBACK",
    );
    revision.voice_ref = Some(voice.voice.unwrap().voice);
    let note = ok(
        f.editor
            .request(metadata_request(
                f,
                &f.editor,
                MetadataChange::SaveFeedback(revision.clone()),
            ))
            .unwrap(),
        "PERSON_FEEDBACK",
    );
    let ReceiptResult::Metadata {
        result: MetadataResult::Feedback(reference),
    } = note.metadata.unwrap().result
    else {
        panic!("PERSON_FEEDBACK_RESULT")
    };
    let reread = ok(
        f.editor
            .request(HostCommand::ReadFeedback { reference })
            .unwrap(),
        "PERSON_FEEDBACK_READ",
    );
    assert_eq!(reread.feedback.as_ref(), Some(&revision));
    revision.anchor.snapshot
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn run_limit_person_save_feedback_version_and_dirty_copy_survive_latches() {
    for latch in [PersonLatch::Storage, PersonLatch::Sends] {
        for phase in 0..3 {
            let (mut f, run, model) = person_latched_fixture(latch);
            if phase > 0 {
                let stop = f.control(&f.editor, &run, RunAction::Stop);
                ok(
                    f.editor
                        .request(HostCommand::DispatchOrchestration {
                            request: Box::new(stop),
                        })
                        .unwrap(),
                    "PERSON_STOP_APPLIED",
                );
            }
            drop(model);
            if phase == 2 {
                let claim_root = f.temp.path("person-machine");
                f = c4_restart(f, &claim_root);
                f.host.runtime.stop();
            }
            person_save(&f, 100);
            let snapshot = person_metadata(&f);
            let dirty = {
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                person_history_redo(owner.core(), 200)
            };
            let destination = f.temp.path("person-copy.kinewright");
            let copied = ok(
                f.editor
                    .request(HostCommand::SaveAs {
                        destination: destination.clone(),
                        snapshot: HostSnapshot::from_history(dirty.clone()).unwrap(),
                    })
                    .unwrap(),
                "PERSON_DIRTY_SAVE_AS",
            );
            assert!(copied.saved.is_some());
            assert_eq!(
                copied.history.as_ref(),
                Some(&dirty),
                "PERSON_COPY_EXACT_REDO"
            );
            let final_history = {
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                assert!(
                    matches!(
                        owner
                            .core()
                            .request(kinewright_core::Command::Do(history_marker(300)))
                            .unwrap(),
                        kinewright_core::Event::DocumentChanged { .. }
                    ),
                    "PERSON_LATER_MANUAL_EDIT"
                );
                owner.core().export_history().unwrap()
            };
            let saved = ok(
                f.editor
                    .request(HostCommand::Save {
                        snapshot: HostSnapshot::from_history(final_history.clone()).unwrap(),
                    })
                    .unwrap(),
                "PERSON_COPIED_PROJECT_SAVE",
            );
            assert_eq!(saved.history.as_ref(), Some(&final_history));
            let original = f.temp.path("project.kinewright");
            let Fixture { host, temp, .. } = f;
            drop(host);
            for (path, expected) in [(&original, &dirty), (&destination, &final_history)] {
                let reopened = Ja1EditingSession::open(path).unwrap();
                assert_eq!(
                    &reopened.core().export_history().unwrap(),
                    expected,
                    "PERSON_CLOSE_REOPEN {latch:?} phase={phase}"
                );
                assert_eq!(
                    reopened.studio_versions().unwrap().as_slice(),
                    std::slice::from_ref(&snapshot),
                    "PERSON_VERSION_REOPEN"
                );
                reopened.read_snapshot(&snapshot).unwrap();
                let feedback = reopened.feedback_index().unwrap();
                assert_eq!(feedback.revisions.len(), 1, "PERSON_FEEDBACK_REOPEN");
                assert_eq!(feedback.notes.len(), 1, "PERSON_VOICE_REOPEN");
                assert_eq!(
                    feedback.notes.values().next().unwrap().duration_micros,
                    1_000_000
                );
            }
            drop(temp);
        }
    }
}

#[test]
fn run_limit_person_admission_is_unavailable_to_task_producers() {
    let (f, run, model) = person_latched_fixture(PersonLatch::Sends);
    let history = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap();
    let before = f.projection(&run);
    let denied = model
        .request(HostCommand::Save {
            snapshot: HostSnapshot::from_history(history).unwrap(),
        })
        .unwrap();
    assert_eq!(denied.error.unwrap().code, "role_refused");
    let denied = model
        .request(HostCommand::AcquireManual {
            client: "m".repeat(64),
        })
        .unwrap();
    assert_eq!(denied.error.unwrap().code, "role_refused");
    let snapshot = person_snapshot(&f, &model);
    let denied = model.request(snapshot).unwrap();
    assert_eq!(denied.error.unwrap().code, "producer_scope");
    let run_producer = f.producer_client(&run);
    let snapshot = person_snapshot(&f, &run_producer);
    let denied = run_producer.request(snapshot).unwrap();
    assert_eq!(denied.error.unwrap().code, "producer_scope");
    person_save(&f, 500);
    assert_eq!(f.projection(&run), before, "PERSON_SAVE_PRESERVES_RUN");
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let execution = before.attempts[0].identity.clone();
    let error = owner
        .publish_native(&producer, before, &execution, vec![])
        .unwrap_err();
    assert_eq!(
        error.refusal().code,
        "run_limit",
        "PERSON_TOKEN_NOT_AVAILABLE_TO_RUN_EFFECT"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one real staged checkpoint crosses a failed publication, manual Save and idempotent run retry"
)]
fn run_limit_person_save_preserves_staged_checkpoint_for_run_retry() {
    let mut f = Fixture::new();
    f.host.runtime.stop();
    f.brief.scope = crate::orchestration::policy::director_scope(
        &Document::default(),
        f.brief.scope.base.clone(),
    )
    .unwrap();
    f.brief
        .scope
        .objects
        .extend(["marker:1".into(), "marker:2".into()]);
    let (run, parent, director) = c4_director(&f);
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
    let task = f.projection(&run).tasks[1].id.clone();
    let (execution, client, endpoint, core) = c4_reserve_task(&f, &run, &task);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let history = c4_mcp_edit_redo(&C4Mcp::new(&f, &run, &task), &core);
    let producer = f.producer(&run);
    let before = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        let before = owner.saved_task_history(&state, &task).unwrap().unwrap();
        let failed = kinewright_project::io_observer::fail_once("root_before_publish", || {
            owner.checkpoint_task_history(&producer, &run, &task)
        })
        .unwrap_err();
        assert!(
            failed.to_string().contains("injected root_before_publish"),
            "PERSON_STAGED_FAULT {failed}"
        );
        before
    };
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        person_history_redo(owner.core(), 50)
    };
    let saved = ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: HostSnapshot::from_history(manual.clone()).unwrap(),
            })
            .unwrap(),
        "PERSON_SAVE_WITH_STAGED_CHECKPOINT",
    );
    assert_eq!(saved.history.as_ref(), Some(&manual));
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        assert_eq!(
            owner.saved_task_history(&state, &task).unwrap().unwrap(),
            before,
            "PERSON_SAVE_DOES_NOT_PUBLISH_TASK_CHECKPOINT"
        );
        owner
            .checkpoint_task_history(&producer, &run, &task)
            .unwrap();
        owner
            .checkpoint_task_history(&producer, &run, &task)
            .unwrap();
    }
    assert_eq!(
        c5e_history(&f, &run, &task),
        history,
        "PERSON_STAGED_RETRY_EXACT_REDO"
    );
}

#[test]
fn run_limit_person_gui_history_publication_survives_the_run_latch() {
    for latch in [PersonLatch::Storage, PersonLatch::Sends] {
        let (f, _run, model) = person_latched_fixture(latch);
        drop(model);
        let before = person_save(&f, 700);
        let client = "p".repeat(64);
        let editor = f.editor.with_manual_editor(client.clone());
        let lease = ok(
            editor
                .request(HostCommand::AcquireManual {
                    client: client.clone(),
                })
                .unwrap(),
            "PERSON_GUI_ACQUIRE",
        );
        let gui = Core::spawn(Document::default()).unwrap();
        gui.restore_history(before).unwrap();
        let replica = Ja1EditingSession::new(
            f.temp.root(),
            &gui,
            kinewright_project::storage_budget::StorageLimits::default(),
        )
        .unwrap();
        let permit = replica
            .bind_manual_producer(lease.manual_lease.unwrap(), lease.manual_inputs.unwrap())
            .unwrap();
        for command in [
            kinewright_core::Command::Do(history_marker(702)),
            kinewright_core::Command::Do(history_marker(703)),
            kinewright_core::Command::Undo,
        ] {
            assert!(matches!(
                permit.request(command).unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ));
        }
        let expected = gui.export_history().unwrap();
        let messages = permit.pending().unwrap();
        let committed = ok(
            editor
                .request(HostCommand::CommitHistory { client, messages })
                .unwrap(),
            "PERSON_GUI_COMMIT",
        );
        assert_eq!(committed.manual_ack.len(), 3);
        assert!(committed.saved.is_some(), "PERSON_GUI_DURABLE_REPORT");
        replica
            .acknowledge_manual(&permit, committed.manual_ack)
            .unwrap();
        assert!(
            permit.pending().unwrap().is_empty(),
            "PERSON_GUI_ACK_SETTLED"
        );
        {
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            assert_eq!(
                owner.core().export_history().unwrap(),
                expected,
                "PERSON_GUI_HOST_HISTORY"
            );
            assert!(matches!(
                owner
                    .owner_mut()
                    .unwrap()
                    .admit_host_grant(b"still a run write"),
                Err(kinewright_project::bundle::BundleError::RunLimit)
            ));
        }
        drop((permit, replica, gui, editor));
        let path = f.temp.path("project.kinewright");
        let Fixture { host, temp, .. } = f;
        drop(host);
        let reopened = Ja1EditingSession::open(&path).unwrap();
        assert_eq!(reopened.core().export_history().unwrap(), expected);
        drop(reopened);
        drop(temp);
    }
}
