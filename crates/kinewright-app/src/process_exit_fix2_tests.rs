//! Close composition uses real source readers, sidecar and recorder IO, and `SessionMedia`.
use super::*;
use kinewright_agent::writer_host::{
    ProtocolFixture, ProtocolFixtureMode, RecoveryPhase, SessionRoots,
};
use kinewright_project::{
    bundle_records::ProjectId, editing_session::Ja1EditingSession, storage_budget::StorageLimits,
};

fn keep_note(project: &mut ProjectSession, directory: &Path, identity: ProjectId) -> PathBuf {
    let file = directory.join(format!("{}.json", identity.as_str()));
    project
        .studio
        .feedback
        .open(Ok(identity), Ok(directory.to_path_buf()))
        .unwrap();
    let branch = kinewright_project::bundle_records::BranchId::fresh().unwrap();
    let history = project.core.export_history().unwrap();
    let cut = crate::studio_feedback::AcceptedCut::of(&branch, &history).unwrap();
    project.studio.feedback.drafts.push(Draft {
        comment: CommentId::fresh().unwrap(),
        text: "Saved before any host or acknowledgement wait".into(),
        strokes: vec![],
        state: DraftState::Editing,
        live: true,
        origin: Origin::Captured {
            proof: FrameProof {
                version: 1,
                shown: Shown::Cut { cut },
                frame: 0,
                fps: FrameRate {
                    numerator: 30,
                    denominator: 1,
                },
                raster: [16, 9],
                orientation: Orientation::Upright,
                pixels: None,
                bound: BoundIdentity {
                    frame_id: 1,
                    epoch: 1,
                    seq: 1,
                    stale: false,
                },
            },
            readback: None,
            snapshot: None,
            layers: vec![],
            layer: None,
        },
    });
    project.studio.feedback.changed = true;
    file
}
fn kept_note(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            value["drafts"][0]["text"] == "Saved before any host or acknowledgement wait"
        })
}

fn incomplete_quit(studio: bool) {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "exit-fix2-pending");
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let owner = Ja1EditingSession::new(
        directory.root(),
        &app.projects[0].core,
        StorageLimits::default(),
    )
    .unwrap();
    let mut pending = crate::ja1_attachment::Ja1Attachment::new(owner).unwrap();
    let transfer = pending.owner.prepare_host_attachment().unwrap();
    let descriptor = transfer.source.unwrap();
    let service = transfer.source_service.unwrap();
    let reader = service
        .open_borrowed_source(&descriptor, &transfer.inputs)
        .unwrap();
    app.projects[0].ja1 = Some(pending);
    app.new_project();
    let healthy_id = app.projects[1].id;
    let named = if studio {
        let (temp, mut attachment) = crate::ja1_attachment::tests::named();
        let path = attachment.project_path().unwrap();
        let fixture = ProtocolFixture::new(temp.root(), ProtocolFixtureMode::Dormant);
        let writer = attachment
            .launch_protocol_fixture(
                SessionRoots {
                    read: vec![temp.root().into()],
                    write: vec![temp.root().into()],
                },
                temp.path("scheduler"),
                &fixture,
            )
            .unwrap();
        assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
        attachment.fixture_host = Some(writer);
        app.projects[1].ja1 = Some(attachment);
        app.projects[1].install_ja1_view();
        Some((temp, path, fixture))
    } else {
        app.focus_project(1);
        let path = directory.path("healthy.kinewright");
        app.write_project(&path).unwrap();
        None
    };
    let path = named.as_ref().map_or_else(
        || directory.path("healthy.kinewright"),
        |(_, path, _)| path.clone(),
    );
    let host = app.projects[1]
        .ja1
        .as_ref()
        .and_then(crate::ja1_attachment::Ja1Attachment::host_client);
    let expected = app.projects[1].core.export_history().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let identity = app.projects[1].ja1.as_ref().map_or_else(
        || ProjectId::fresh().unwrap(),
        |attachment| attachment.studio_project_id().unwrap(),
    );
    let note = keep_note(&mut app.projects[1], &directory.path("notes"), identity);
    app.exit_discarded_projects = app.projects.iter().map(|project| project.id).collect();
    app.request_exit(&egui::Context::default());
    assert!(!app.allow_close);
    assert!(
        !app.projects.iter().any(|project| project.id == healthy_id),
        "EXIT_FIX2_NO_DEAD_HEALTHY_TAB studio={studio}"
    );
    assert_eq!(app.projects.len(), 1);
    assert!(
        app.status.contains("readers") || app.status.contains("cleanup"),
        "{}",
        app.status
    );
    assert!(kept_note(&note));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    app.retirement.join();
    if studio {
        assert!(
            host.unwrap()
                .request(kinewright_agent::writer_host::HostCommand::Status)
                .is_err(),
            "EXIT_FIX2_COMPLETED_HOST_RETIRED"
        );
        let reopened = Ja1EditingSession::open(&path).unwrap();
        assert_eq!(
            reopened.core().export_history().unwrap(),
            expected,
            "EXIT_FIX2_REOPEN_EXACT_STUDIO"
        );
    } else {
        assert_eq!(
            kinewright_project::load_document(&path).unwrap().0,
            *expected.document,
            "EXIT_FIX2_REOPEN_EXACT_LEGACY"
        );
    }
    drop(reader);
    app.exit_discarded_projects = app.projects.iter().map(|project| project.id).collect();
    app.request_exit(&egui::Context::default());
    assert!(app.allow_close && app.projects.is_empty());
}
#[test]
fn exit_fix2_incomplete_quit_removes_healthy_legacy_project() {
    incomplete_quit(false);
}
#[test]
fn exit_fix2_incomplete_quit_removes_healthy_studio_project() {
    incomplete_quit(true);
}

fn acknowledgement_order(recorder: bool) {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "exit-fix2-ack");
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    app.write_project(&directory.path("first.kinewright"))
        .unwrap();
    app.new_project();
    let (studio_directory, mut attachment) = crate::ja1_attachment::tests::named();
    let fixture = ProtocolFixture::new(studio_directory.root(), ProtocolFixtureMode::Dormant);
    let writer = attachment
        .launch_protocol_fixture(
            SessionRoots {
                read: vec![studio_directory.root().into()],
                write: vec![studio_directory.root().into()],
            },
            studio_directory.path("scheduler"),
            &fixture,
        )
        .unwrap();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    let identity = attachment.studio_project_id().unwrap();
    app.projects[1].ja1 = Some(attachment);
    app.projects[1].install_ja1_view();
    let note = keep_note(&mut app.projects[1], &directory.path("notes"), identity);
    let io = Arc::new(Gate::default());
    let closing = Arc::new(Gate::default());
    let append = Arc::clone(&io);
    if recorder {
        app.projects[0]
            .recovery
            .set_append_hook(Arc::new(move || append.hold()));
        app.projects[0]
            .core
            .request(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(333),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            }))
            .unwrap();
        io.entered();
    } else {
        let sidecar = Arc::clone(&io);
        app.projects[0]
            .sidecar_writer
            .set_rename_hook(Arc::new(move || sidecar.hold()));
        app.focus_project(0);
        app.projects[0]
            .incidents
            .write()
            .unwrap()
            .observe(sidecar_write_failed_observation(
                "fixture sidecar write",
                app.projects[0].revision,
            ));
    }
    let journal = app.projects[0].recovery.journal_path().to_path_buf();
    let close_gate = Arc::clone(&closing);
    app.exit_discarded_projects = app.projects.iter().map(|project| project.id).collect();
    let observed = std::thread::scope(|scope| {
        let check = scope.spawn(|| {
            if recorder {
                closing.entered();
            } else {
                io.entered();
            }
            let preserved = kept_note(&note) && writer.shutdown_requested();
            closing.release();
            io.release();
            preserved
        });
        if recorder {
            crate::recovery::at_close(
                journal,
                move || close_gate.hold(),
                || app.request_exit(&egui::Context::default()),
            );
        } else {
            app.request_exit(&egui::Context::default());
        }
        check.join().unwrap()
    });
    assert!(
        observed,
        "EXIT_FIX2_ALL_SAVES_AND_HOST_STOPS_PRECEDE_ACK recorder={recorder}"
    );
    assert!(app.allow_close && app.projects.is_empty());
    assert!(fixture.observations().is_empty());
}
#[test]
fn exit_fix2_sidecar_ack_follows_other_projects_saves_and_host_stop() {
    acknowledgement_order(false);
}
#[test]
fn exit_fix2_recorder_io_follows_other_projects_saves_and_host_stop() {
    acknowledgement_order(true);
}

struct HeldSessionMedia {
    media: Arc<dyn Export>,
    gate: Arc<Gate>,
    first: std::sync::atomic::AtomicBool,
}
impl Export for HeldSessionMedia {
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        if self.first.swap(false, std::sync::atomic::Ordering::AcqRel) {
            self.gate.hold();
        }
        self.media.export(out, settings, progress)
    }
}
#[test]
fn exit_fix2_studio_activation_preserves_running_and_queued_session_media_exports() {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "exit-fix2-real-export");
    let mut document = renderable_document(3);
    document.resolution = (64, 64);
    document.tracks[0].clips[0].content = ClipContent::Solid(kinewright_core::SolidColor {
        r: 90,
        g: 70,
        b: 40,
    });
    let (mut app, _) = in1_tests::in1_harness(document);
    let media = app.projects[0].media();
    let gate = Arc::new(Gate::default());
    let exporter = Arc::new(HeldSessionMedia {
        media: media.exporter,
        gate: Arc::clone(&gate),
        first: std::sync::atomic::AtomicBool::new(true),
    });
    let queue = ExportQueue::with_lut_project_path(
        exporter,
        media.analysis,
        Arc::clone(&app.projects[0].agent_project_path),
    )
    .unwrap();
    app.projects[0].exports = Some(queue.clone());
    let request = |name| QueueExportRequest {
        output_path: directory.path(name),
        profile: DeliveryProfile::SourceMaster,
        focus_x_percent: 50,
        focus_y_percent: 50,
        overwrite: false,
        verify: false,
        delivery_bit_depth: DeliveryEncodeDepth::default(),
        normalize_loudness: false,
    };
    let first = queue
        .enqueue(&app.projects[0].document, request("first.mp4"))
        .unwrap()
        .id;
    gate.entered();
    let second = queue
        .enqueue(&app.projects[0].document, request("second.mp4"))
        .unwrap()
        .id;
    assert_eq!(queue.get(first).unwrap().state, ExportJobState::Running);
    assert_eq!(queue.get(second).unwrap().state, ExportJobState::Queued);
    let journal = app.projects[0].recovery.journal_path().to_path_buf();
    app.enable_ja1();
    let refused =
        app.status == "Wait for the export to finish, or cancel it, before turning on Studio";
    let unchanged = app.projects[0].ja1.is_none()
        && journal.is_file()
        && app.projects[0].legacy_media().is_some();
    gate.release();
    queue.close_admission();
    queue.join();
    assert!(
        refused && unchanged,
        "EXIT_FIX2_ACTIVATION_REFUSES_BEFORE_REVOCATION"
    );
    for (job, name) in [(first, "first.mp4"), (second, "second.mp4")] {
        assert_eq!(
            queue.get(job).unwrap().state,
            ExportJobState::Completed,
            "EXIT_FIX2_BOTH_ACCEPTED_OUTPUTS"
        );
        assert!(std::fs::metadata(directory.path(name)).unwrap().len() > 0);
        assert!(
            app.projects[0]
                .media()
                .analysis
                .probe(&directory.path(name))
                .is_ok()
        );
    }
}
