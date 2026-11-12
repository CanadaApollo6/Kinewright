//! Gate-forced frame/terminal ownership controls. No provider process is launched.
use super::*;
use crate::studio_feedback::{BoundIdentity, Draft, DraftState, FrameProof, Origin, Shown};
use kinewright_agent::{
    McpServer,
    export_queue::{ExportJobState, ExportQueue, QueueExportRequest},
};
use kinewright_core::*;
use kinewright_core::{
    Analysis, Export, ExportCancellation, ExportSettings, MediaAsset, MediaError, ProgressSink,
    RefusedMedia, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::studio_records::{CommentId, FrameRate, Orientation};
use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct Gate {
    state: Mutex<(bool, bool, bool)>,
    changed: Condvar,
}
impl Gate {
    fn hold(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.changed.notify_all();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(180), |state| !state.1)
            .unwrap();
        assert!(state.1, "EXIT_FIX1_RELEASE_BOUNDED");
    }
    fn entered(&self) {
        let (state, _) = self
            .changed
            .wait_timeout_while(
                self.state.lock().unwrap(),
                Duration::from_secs(5),
                |state| !state.0,
            )
            .unwrap();
        assert!(state.0, "EXIT_FIX1_NATIVE_ENTERED");
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
    fn frame(&self, action: impl FnOnce()) -> bool {
        std::thread::scope(|scope| {
            let controller = scope.spawn(|| {
                let (mut state, _) = self
                    .changed
                    .wait_timeout_while(
                        self.state.lock().unwrap(),
                        Duration::from_secs(120),
                        |state| !state.2,
                    )
                    .unwrap();
                if !state.2 {
                    state.1 = true;
                    self.changed.notify_all();
                }
            });
            action();
            let mut state = self.state.lock().unwrap();
            let held = !state.1;
            state.2 = true;
            state.1 = true;
            self.changed.notify_all();
            drop(state);
            controller.join().unwrap();
            held
        })
    }
}

struct HeldExport {
    gate: Arc<Gate>,
    token: Arc<Mutex<Option<ExportCancellation>>>,
}
impl Export for HeldExport {
    fn export(
        &self,
        path: &Path,
        settings: ExportSettings,
        _: ProgressSink,
    ) -> Result<(), MediaError> {
        *self.token.lock().unwrap() = Some(settings.cancellation.clone());
        std::fs::write(path, b"partial").unwrap();
        self.gate.hold();
        if settings.cancellation.is_cancelled() {
            std::fs::remove_file(path).unwrap();
            Err(MediaError::Cancelled)
        } else {
            std::fs::write(path, b"complete").unwrap();
            Ok(())
        }
    }
}
struct ExportWitness {
    gate: Arc<Gate>,
    token: Arc<Mutex<Option<ExportCancellation>>>,
    queue: ExportQueue,
    job: kinewright_agent::export_queue::ExportJobId,
    output: PathBuf,
    directory: TempDirectory,
}
impl ExportWitness {
    fn install(app: &mut KinewrightApp) -> Self {
        let directory = TempDirectory::new_in(&workspace_target_dir(), "exit-fix1-export");
        let gate = Arc::new(Gate::default());
        let token = Arc::new(Mutex::new(None));
        let queue = ExportQueue::with_lut_project_path(
            Arc::new(HeldExport {
                gate: Arc::clone(&gate),
                token: Arc::clone(&token),
            }),
            Arc::new(RefusedMedia),
            Arc::clone(&app.projects[0].agent_project_path),
        )
        .unwrap();
        app.projects[0].exports = Some(queue.clone());
        app.exit_fix1_branch_action("refresh");
        let output = directory.path("held.mp4");
        let document = renderable_document(10);
        let job = queue
            .enqueue(
                &document,
                QueueExportRequest {
                    output_path: output.clone(),
                    profile: kinewright_core::DeliveryProfile::SourceMaster,
                    focus_x_percent: 50,
                    focus_y_percent: 50,
                    overwrite: false,
                    verify: false,
                    delivery_bit_depth: kinewright_core::DeliveryEncodeDepth::default(),
                    normalize_loudness: false,
                },
            )
            .unwrap()
            .id;
        gate.entered();
        Self {
            gate,
            token,
            queue,
            job,
            output,
            directory,
        }
    }
    fn cancelled(&self) -> bool {
        self.token.lock().unwrap().as_ref().unwrap().is_cancelled()
    }
    fn finish(&self) -> ExportJobState {
        self.gate.release();
        self.queue.close_admission();
        self.queue.join();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let record = self.queue.get(self.job).unwrap();
            if record.state.is_terminal() {
                return record.state;
            }
            assert!(Instant::now() < deadline, "EXIT_FIX1_EXPORT_FINISHED");
            std::thread::yield_now();
        }
    }
}
impl Drop for ExportWitness {
    fn drop(&mut self) {
        self.gate.release();
    }
}

#[test]
fn exit_fix1_accepted_export_survives_refresh_apply_and_discard() {
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let export = ExportWitness::install(&mut app);
    for action in ["refresh", "apply", "discard"] {
        if action == "apply" {
            app.projects[0].threads[0]
                .branch
                .core()
                .request(Command::Do(Operation::AddTrack {
                    track: Track {
                        id: TrackId(99),
                        kind: TrackKind::Video,
                        sync_lock: true,
                        clips: vec![],
                    },
                }))
                .unwrap();
        }
        app.exit_fix1_branch_action(action);
        app.retirement.prune();
        assert!(
            !export.cancelled(),
            "EXIT_FIX1_ACCEPTED_EXPORT_SURVIVES {action}"
        );
        let queue = app.projects[0].threads[0]
            .mcp_server
            .as_ref()
            .unwrap()
            .export_queue()
            .unwrap();
        assert_eq!(queue.get(export.job).unwrap().id, export.job);
        assert_eq!(
            queue.get(export.job).unwrap().state,
            ExportJobState::Running
        );
    }
    assert_eq!(export.finish(), ExportJobState::Completed);
    assert_eq!(std::fs::read(&export.output).unwrap(), b"complete");
}

struct HeldBackend(Arc<Gate>);
impl Analysis for HeldBackend {
    fn probe(&self, _: &Path) -> Result<MediaAsset, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn thumbnail_at(&self, _: TimeCode, _: u32) -> Result<RgbaImage, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn thumbnail_for_document(
        &self,
        _: Arc<Document>,
        _: TimeCode,
        _: u32,
    ) -> Result<RgbaImage, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn working_proof_for_document(
        &self,
        _: Arc<Document>,
        _: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_transcription(&self, _: MediaAsset) {}
    fn transcript_status(&self, _: &MediaAsset) -> TranscriptStatus {
        TranscriptStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_transcript(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_silence_detection(&self, _: MediaAsset) {}
    fn silence_status(&self, _: &MediaAsset) -> SilenceStatus {
        SilenceStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_silences(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
        _: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_scene_detection(&self, _: MediaAsset) {}
    fn scene_status(&self, _: &MediaAsset) -> SceneStatus {
        SceneStatus::Failed(MediaError::MediaAdmissionRequired.to_string())
    }
    fn timeline_scene_changes(
        &self,
        _: &Document,
        _: Option<std::ops::Range<TimeCode>>,
        _: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        Err(MediaError::MediaAdmissionRequired)
    }
    fn request_waveform(&self, _: MediaAsset, _: u64) -> bool {
        false
    }
    fn request_thumbnail(&self, _: MediaAsset, _: TimeCode, _: u32, _: u64) -> bool {
        false
    }
    fn visual_asset_results(&self) -> crossbeam_channel::Receiver<VisualAssetResult> {
        crossbeam_channel::never()
    }
}

impl Drop for HeldBackend {
    fn drop(&mut self) {
        self.0.hold();
    }
}

#[test]
fn exit_fix1_branch_and_thread_close_retire_without_joining_a_live_server() {
    for action in ["refresh", "apply", "discard", "close"] {
        let (mut app, _) = in1_tests::in1_harness(Document::default());
        let gate = Arc::new(Gate::default());
        let core = app.projects[0].threads[0].branch.core();
        let mut server = McpServer::start_isolated(
            core,
            Arc::new(RefusedMedia),
            Arc::new(HeldBackend(Arc::clone(&gate))),
        )
        .unwrap();
        server.request_shutdown();
        gate.entered();
        app.projects[0].threads[0].mcp_server = Some(server);
        if action == "apply" {
            app.projects[0].threads[0]
                .branch
                .core()
                .request(Command::Do(Operation::AddTrack {
                    track: Track {
                        id: TrackId(99),
                        kind: TrackKind::Video,
                        sync_lock: true,
                        clips: vec![],
                    },
                }))
                .unwrap();
        }
        let returned = gate.frame(|| app.exit_fix1_branch_action(action));
        assert!(returned, "EXIT_FIX1_BRANCH_FRAME_RETURNS {action}");
        app.retirement.join();
    }
}

#[test]
fn exit_fix1_project_close_retires_its_live_investigator_reaper() {
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    app.new_project();
    let gate = Arc::new(Gate::default());
    let worker_gate = Arc::clone(&gate);
    let worker = std::thread::spawn(move || worker_gate.hold());
    app.projects[0]
        .investigator
        .as_mut()
        .unwrap()
        .hold_retired_for_test(worker);
    gate.entered();
    app.projects[0].saved_document = Some(Arc::clone(&app.projects[0].document));
    let id = app.projects[0].id;
    let returned = gate.frame(|| app.request_close_project(0));
    assert!(returned, "EXIT_FIX1_PROJECT_FRAME_RETURNS");
    assert!(!app.projects.iter().any(|project| project.id == id));
    app.retirement.join();
}

#[test]
fn exit_fix1_close_and_quit_ask_before_explicit_export_cancel() {
    for quit in [false, true] {
        let (mut app, _) = in1_tests::in1_harness(Document::default());
        let export = ExportWitness::install(&mut app);
        app.new_project();
        for project in &mut app.projects {
            project.saved_document = Some(Arc::clone(&project.document));
        }
        let context = egui::Context::default();
        if quit {
            app.request_exit(&context);
        } else {
            app.request_close_project(0);
        }
        assert!(
            app.pending_export_close.is_some(),
            "EXIT_FIX1_EXPORT_CLOSE_CHOICE"
        );
        assert_eq!(app.projects.len(), 2);
        assert!(!app.allow_close);
        assert!(!export.cancelled());
        app.export_close_choice(&context, false);
        assert!(!export.cancelled(), "EXIT_FIX1_KEEP_WORKING_IS_UNCHANGED");
        assert_eq!(app.projects.len(), 2);
        if quit {
            app.request_exit(&context);
        } else {
            app.request_close_project(0);
        }
        app.export_close_choice(&context, true);
        assert!(export.cancelled(), "EXIT_FIX1_CLOSE_CANCEL_IS_EXPLICIT");
        assert_eq!(export.finish(), ExportJobState::Cancelled);
        assert!(!export.output.exists());
        if quit {
            assert!(app.allow_close);
        } else {
            assert_eq!(app.projects.len(), 1);
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "ordered two-project shutdown witness keeps the held native work and durable observations together"
)]
fn exit_fix1_quit_persists_and_shuts_down_all_projects_before_export_join() {
    use kinewright_agent::writer_host::{
        ProtocolFixture, ProtocolFixtureMode, RecoveryPhase, SessionRoots,
    };
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let export = ExportWitness::install(&mut app);
    app.new_project();
    let (directory, mut attachment) = crate::ja1_attachment::tests::named();
    let history = attachment.owner.core().export_history().unwrap();
    let branch = attachment.owner.manual_checkpoint().unwrap().1.branch_id;
    let cut = crate::studio_feedback::AcceptedCut::of(&branch, &history).unwrap();
    let fixture = ProtocolFixture::new(directory.root(), ProtocolFixtureMode::Dormant);
    let writer = attachment
        .launch_protocol_fixture(
            SessionRoots {
                read: vec![directory.root().into()],
                write: vec![directory.root().into()],
            },
            directory.path("scheduler"),
            &fixture,
        )
        .unwrap();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    let project_id = attachment.studio_project_id().unwrap();
    let feedback_folder = directory.path("feedback");
    app.projects[1]
        .studio
        .feedback
        .open(Ok(project_id.clone()), Ok(feedback_folder.clone()))
        .unwrap();
    app.projects[1].studio.feedback.drafts.push(Draft {
        comment: CommentId::fresh().unwrap(),
        text: "Kept before quitting every project".into(),
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
    app.projects[1].studio.feedback.changed = true;
    let feedback_file = feedback_folder.join(format!("{}.json", project_id.as_str()));
    if feedback_file.exists() {
        std::fs::remove_file(&feedback_file).unwrap();
    }
    let journal = app.projects[1].recovery.journal_path().to_path_buf();
    app.projects[1].ja1 = Some(attachment);
    app.projects[1].install_ja1_view();
    app.exit_discarded_projects = app.projects.iter().map(|project| project.id).collect();
    let context = egui::Context::default();
    app.request_exit(&context);
    assert!(app.pending_export_close.is_some());
    let mut phase1 = false;
    let returned = export.gate.frame(|| {
        app.export_close_choice(&context, true);
        eprintln!(
            "EXIT_FIX1_PHASE1 allow={} shutdown={} feedback={} journal={} status={} pending={:?}",
            app.allow_close,
            writer.shutdown_requested(),
            feedback_file.is_file(),
            journal.exists(),
            app.status,
            app.pending_project_action
        );
        phase1 = app.allow_close
            && writer.shutdown_requested()
            && feedback_file.is_file()
            && !journal.exists();
    });
    assert!(
        returned && phase1,
        "EXIT_FIX1_ALL_PROJECTS_PERSIST_AND_SIGNAL_BEFORE_JOIN"
    );
    assert!(export.cancelled());
    let kept: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&feedback_file).unwrap()).unwrap();
    assert_eq!(
        kept["drafts"][0]["text"],
        "Kept before quitting every project"
    );
    assert!(
        app.projects.is_empty(),
        "EXIT_FIX2_COMPLETED_PROJECTS_REMOVED"
    );
    assert_eq!(export.finish(), ExportJobState::Cancelled);
    assert!(
        fixture.observations().is_empty(),
        "EXIT_FIX1_NO_PROVIDER_LAUNCH"
    );
    app.join_native_owners();
    drop(writer);
}

#[test]
fn exit_fix1_gui_export_close_choice_uses_the_same_project_guard() {
    for quit in [false, true] {
        let (mut app, _) = in1_tests::in1_harness(Document::default());
        let gate = Arc::new(Gate::default());
        let worker_gate = Arc::clone(&gate);
        let worker = std::thread::spawn(move || worker_gate.hold());
        let cancellation = ExportCancellation::default();
        app.export_job = Some(ExportJob::held_for_test(
            app.projects[0].id,
            cancellation.clone(),
            worker,
        ));
        gate.entered();
        app.new_project();
        for project in &mut app.projects {
            project.saved_document = Some(Arc::clone(&project.document));
        }
        let context = egui::Context::default();
        if quit {
            app.request_exit(&context);
        } else {
            app.request_close_project(0);
        }
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            app.show_export_close_confirmation(ui.ctx());
        });
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            app.show_export_close_confirmation(ui.ctx());
        });
        let text = crate::ja1_attachment::tests::painted_text(&output);
        assert!(
            text.contains("Keep working"),
            "EXIT_FIX1_EXPORT_PROMPT_RENDERED {text}"
        );
        assert!(app.pending_export_close.is_some());
        app.export_close_choice(&context, false);
        assert!(!cancellation.is_cancelled());
        assert_eq!(app.projects.len(), 2);
        if quit {
            app.request_exit(&context);
        } else {
            app.request_close_project(0);
        }
        app.export_close_choice(&context, true);
        assert!(cancellation.is_cancelled());
        gate.release();
        if quit {
            assert!(app.allow_close);
        } else {
            assert_eq!(app.projects.len(), 1);
        }
    }
}

fn renderable_document(duration: i64) -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: kinewright_core::ClipId(1),
                asset: AssetId::default(),
                source_range: TimeCode::ZERO..TimeCode(duration),
                content: ClipContent::Title(Title {
                    text: "Export queue fixture".to_owned(),
                    ..Title::default()
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: kinewright_core::BlendMode::Normal,
            }],
        }],
        duration: TimeCode(duration),
        ..Document::default()
    }
}

#[test]
fn exit_fix1_aborting_export_quit_forgets_discard_and_keeps_admission_open() {
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let export = ExportWitness::install(&mut app);
    app.new_project();
    app.projects[1].saved_document = Some(Arc::clone(&app.projects[1].document));
    let id = app.projects[0].id;
    assert!(app.projects[0].is_dirty());
    let context = egui::Context::default();
    app.request_exit(&context);
    assert!(matches!(app.pending_project_action, Some(ProjectAction::Exit(p)) if p == id));
    assert!(app.pending_export_close.is_none());
    // The existing Discard handler records this choice then requests exit again.
    app.pending_project_action = None;
    app.exit_discarded_projects.push(id);
    app.request_exit(&context);
    assert!(app.pending_export_close.is_some());
    app.export_close_choice(&context, false);
    assert!(app.exit_discarded_projects.is_empty());
    assert!(!export.cancelled());
    let request = |path| QueueExportRequest {
        output_path: path,
        profile: DeliveryProfile::SourceMaster,
        focus_x_percent: 50,
        focus_y_percent: 50,
        overwrite: false,
        verify: false,
        delivery_bit_depth: DeliveryEncodeDepth::default(),
        normalize_loudness: false,
    };
    export
        .queue
        .enqueue(
            &renderable_document(10),
            request(export.directory.path("second.mp4")),
        )
        .unwrap();
    app.request_exit(&context);
    assert!(matches!(app.pending_project_action, Some(ProjectAction::Exit(p)) if p == id));
    app.pending_project_action = None;
    app.exit_discarded_projects.push(id);
    app.request_exit(&context);
    app.export_close_choice(&context, true);
    assert!(app.allow_close);
    assert!(app.pending_project_action.is_none());
    assert!(matches!(
        export.queue.enqueue(
            &renderable_document(10),
            request(export.directory.path("late.mp4"))
        ),
        Err(kinewright_agent::export_queue::ExportQueueError::WorkerUnavailable)
    ));
    assert_eq!(export.finish(), ExportJobState::Cancelled);
    assert!(
        export
            .queue
            .list()
            .iter()
            .all(|job| job.state == ExportJobState::Cancelled)
    );
}

#[path = "process_exit_fix2_tests.rs"]
mod fix2;
