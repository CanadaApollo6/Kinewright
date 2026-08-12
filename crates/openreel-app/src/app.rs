use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use eframe::egui;
use openreel_agent::{ClaudeCodeDriver, CodexDriver};
use openreel_core::{
    AgentDriver, Analysis, Command, Document, Event, Export, HarnessInfo, JournalCommand,
    MediaAsset, MediaError, MediaEvent, Operation, Playback, PlaybackState, TimeCode, Track,
    TrackId, TrackKind,
};
use openreel_media::{FfmpegMediaEngine, GpuContext};

use crate::{
    chat_ui::ChatEntry,
    error_ui::ErrorLog,
    export_ui::{ExportDialog, ExportJob},
    icons::Icon,
    project::{ProjectSession, index_after_close, project_name, session_index_by_id},
    theme::{self, color, size, space},
    transcript_ui::TranscriptScope,
};

const DEFAULT_TRACK_ID: TrackId = TrackId(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectAction {
    CloseProject(u64),
    Exit(u64),
}

/// Which view the bottom material strip shows (M24 conversation-first layout).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterialTab {
    #[default]
    Timeline,
    Transcript,
}

// Independent transport, agent, dialog, and window flags model separate UI state machines.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct OpenReelApp {
    pub(crate) projects: Vec<ProjectSession>,
    pub(crate) focused_project: usize,
    next_project_id: u64,
    pub(crate) playback: Arc<dyn Playback>,
    pub(crate) analysis: Arc<dyn Analysis>,
    pub(crate) exporter: Arc<dyn Export>,
    pub(crate) frames: crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    pub(crate) media_events: crossbeam_channel::Receiver<MediaEvent>,
    pub(crate) visual_cache: crate::visual_cache::VisualCache,
    pub(crate) claude_info: Option<HarnessInfo>,
    pub(crate) codex_info: Option<HarnessInfo>,
    pub(crate) show_thread_rail: bool,
    pub(crate) settings_open: bool,
    /// Selectable models per harness; `None` chosen means the CLI's default.
    pub(crate) claude_models: Vec<openreel_agent::ModelChoice>,
    pub(crate) codex_models: Vec<openreel_agent::ModelChoice>,
    /// The model the Codex CLI's config actually runs as its default, so the
    /// picker's "Default" resolves to that model's real efforts and tiers.
    pub(crate) codex_default_model: Option<String>,
    pub(crate) claude_model: Option<String>,
    pub(crate) codex_model: Option<String>,
    pub(crate) claude_effort: Option<String>,
    pub(crate) codex_effort: Option<String>,
    /// Service tier ids per harness; `None` means the provider's standard
    /// tier (only offered where the harness catalog advertises tiers).
    pub(crate) claude_tier: Option<String>,
    pub(crate) codex_tier: Option<String>,
    pub(crate) probe_tx: mpsc::Sender<(u64, PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) probe_rx: mpsc::Receiver<(u64, PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) playing: bool,
    pub(crate) meter_levels: [f32; 2],
    pub(crate) resume_after_scrub: bool,
    pub(crate) transcript_scope: TranscriptScope,
    pub(crate) material_tab: MaterialTab,
    pub(crate) show_material_strip: bool,
    pub(crate) show_media_rail: bool,
    pending_project_action: Option<ProjectAction>,
    exit_discarded_projects: Vec<u64>,
    allow_close: bool,
    last_window_title: String,
    pub(crate) status: String,
    pub(crate) export_dialog: ExportDialog,
    pub(crate) export_job: Option<ExportJob>,
    pub(crate) help_open: bool,
    pub(crate) ripple_mode: bool,
    pub(crate) error_log: ErrorLog,
    pub(crate) error_log_open: bool,
    screenshot: crate::screenshot::ScreenshotCapture,
    pub(crate) recording: Option<crate::recording::ActiveRecording>,
    pub(crate) record_dialog: crate::recording::RecordDialog,
}

impl OpenReelApp {
    // Construction keeps all channel subscriptions and coupled UI state initialization together.
    #[allow(clippy::too_many_lines)]
    fn new(media: Arc<FfmpegMediaEngine>) -> Self {
        let document = Document::default();
        let frames = media.frames();
        let media_events = media.events();
        let visual_cache = crate::visual_cache::VisualCache::new(media.visual_asset_results());
        let (probe_tx, probe_rx) = mpsc::channel();
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media;
        let project =
            ProjectSession::create(1, "Project 1", document.clone(), None, &playback, &analysis)
                .expect("default project session must be valid");
        let error_log = ErrorLog::default();
        let claude_info = ClaudeCodeDriver.detect();
        let codex_info = CodexDriver.detect();
        let resolution = document.resolution;
        let fps = document.fps;
        let error_log_open = error_log.len() > 0;
        let mut app = Self {
            projects: vec![project],
            focused_project: 0,
            next_project_id: 2,
            playback,
            analysis,
            exporter,
            frames,
            media_events,
            visual_cache,
            claude_info,
            codex_info,
            show_thread_rail: true,
            settings_open: matches!(
                std::env::var("OPENREEL_SCREENSHOT_SHOW").as_deref(),
                Ok("settings")
            ),
            claude_models: openreel_agent::claude_models(),
            codex_models: openreel_agent::codex_models(),
            codex_default_model: openreel_agent::codex_default_model(),
            claude_model: None,
            codex_model: None,
            claude_effort: None,
            codex_effort: None,
            claude_tier: None,
            codex_tier: None,
            probe_tx,
            probe_rx,
            texture: None,
            playing: false,
            meter_levels: [0.0; 2],
            resume_after_scrub: false,
            transcript_scope: TranscriptScope::default(),
            // The screenshot harness can pre-raise a summoned surface that no
            // startup interaction could otherwise reach in a static capture.
            material_tab: match std::env::var("OPENREEL_SCREENSHOT_SHOW").as_deref() {
                Ok("transcript") => MaterialTab::Transcript,
                _ => MaterialTab::default(),
            },
            show_material_strip: matches!(
                std::env::var("OPENREEL_SCREENSHOT_SHOW").as_deref(),
                Ok("timeline" | "transcript")
            ),
            show_media_rail: false,
            pending_project_action: None,
            exit_discarded_projects: Vec::new(),
            allow_close: false,
            last_window_title: String::new(),
            status: "Creating default video track…".to_owned(),
            export_dialog: ExportDialog {
                open: false,
                output: "export.mp4".to_owned(),
                width: resolution.0,
                height: resolution.1,
                fps_numerator: fps.numerator(),
                fps_denominator: fps.denominator(),
            },
            export_job: None,
            help_open: false,
            ripple_mode: false,
            error_log,
            error_log_open,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
            recording: None,
            record_dialog: crate::recording::RecordDialog::default(),
        };
        if app
            .focused()
            .core
            .send(Command::Do(Operation::AddTrack {
                track: Track {
                    id: DEFAULT_TRACK_ID,
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            }))
            .is_err()
        {
            app.record_error(
                "Operations",
                "Core actor stopped while creating the default track",
            );
        }
        app.playback
            .set_document(Arc::clone(&app.focused().document));
        app.playback.request_frame(TimeCode::ZERO);
        app
    }

    pub(crate) fn focused(&self) -> &ProjectSession {
        &self.projects[self.focused_project]
    }

    pub(crate) fn focused_mut(&mut self) -> &mut ProjectSession {
        &mut self.projects[self.focused_project]
    }

    fn create_project_session(
        &mut self,
        name: String,
        document: Document,
        project_path: Option<PathBuf>,
    ) -> Result<ProjectSession, String> {
        let id = self.next_project_id;
        self.next_project_id = self
            .next_project_id
            .checked_add(1)
            .ok_or_else(|| "project session identity space is exhausted".to_owned())?;
        ProjectSession::create(
            id,
            name,
            document,
            project_path,
            &self.playback,
            &self.analysis,
        )
    }

    pub(crate) fn focus_project(&mut self, index: usize) {
        self.focus_project_with_rebind(index, false);
    }

    fn focus_project_with_rebind(&mut self, index: usize, force_rebind: bool) {
        if index >= self.projects.len() || (!force_rebind && index == self.focused_project) {
            return;
        }
        self.playback.pause();
        self.playing = false;
        self.resume_after_scrub = false;
        self.meter_levels = [0.0; 2];
        self.texture = None;
        self.focused_project = index;
        let document = Arc::clone(&self.focused().document);
        let position = self.focused().position;
        self.playback.set_document(document);
        self.playback.seek(position);
        self.playback.request_frame(position);
    }

    pub(crate) fn save_project(&mut self, save_as: bool) -> bool {
        let path = if save_as {
            None
        } else {
            self.focused().project_path.clone()
        };
        let path = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("OpenReel project", &["openreel"])
                .set_file_name("project.openreel")
                .save_file()
        });
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension("openreel");
        }
        let document = Arc::clone(&self.focused().document);
        let result = serde_json::to_string_pretty(&*document)
            .map_err(|error| error.to_string())
            .and_then(|json| fs::write(&path, json).map_err(|error| error.to_string()));
        match result {
            Ok(()) => {
                let name = project_name(Some(&path), &self.focused().name);
                let session = self.focused_mut();
                session.name = name;
                session.project_path = Some(path.clone());
                session.saved_document = Some(Arc::clone(&session.document));
                let core = session.core.clone();
                session
                    .recovery
                    .checkpoint(&core, session.project_path.as_deref());
                self.status = format!("Saved {}", path.display());
                true
            }
            Err(error) => {
                self.record_error(
                    "Project",
                    format!("Could not save {}: {error}", path.display()),
                );
                false
            }
        }
    }

    fn choose_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("OpenReel project", &["openreel", "json"])
            .pick_file()
        else {
            return;
        };
        self.open_project(&path);
    }

    pub(crate) fn new_project(&mut self) {
        let name = format!("Project {}", self.next_project_id);
        let session = match self.create_project_session(name, Document::default(), None) {
            Ok(session) => session,
            Err(error) => {
                self.record_error(
                    "Project",
                    format!("Could not create a new project: {error}"),
                );
                return;
            }
        };
        self.projects.push(session);
        self.focus_project(self.projects.len() - 1);
        "Creating default video track…".clone_into(&mut self.status);
        self.send_operation(Operation::AddTrack {
            track: Track {
                id: DEFAULT_TRACK_ID,
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        });
    }

    fn open_project(&mut self, path: &Path) {
        let loaded = fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|json| {
                serde_json::from_str::<Document>(&json).map_err(|error| error.to_string())
            })
            .and_then(|document| {
                document
                    .validate()
                    .map_err(|error| error.to_string())
                    .map(|()| document)
            });
        let document = match loaded {
            Ok(document) => document,
            Err(error) => {
                self.record_error(
                    "Project",
                    format!("Could not open {}: {error}", path.display()),
                );
                return;
            }
        };
        let missing: Vec<String> = document
            .media_pool
            .iter()
            .filter(|asset| !asset.path.is_file())
            .map(|asset| format!("{} ({})", asset.name, asset.path.display()))
            .collect();
        let fallback = format!("Project {}", self.next_project_id);
        let name = project_name(Some(path), &fallback);
        let mut session =
            match self.create_project_session(name, document, Some(path.to_path_buf())) {
                Ok(session) => session,
                Err(error) => {
                    self.record_error(
                        "Project",
                        format!("Could not open {}: {error}", path.display()),
                    );
                    return;
                }
            };
        session.saved_document = Some(Arc::clone(&session.document));
        let assets = session.document.media_pool.clone();
        self.projects.push(session);
        self.focus_project(self.projects.len() - 1);
        for asset in assets {
            self.request_asset_analysis(asset);
        }
        self.status = if missing.is_empty() {
            format!("Opened {}", path.display())
        } else {
            self.error_log.push(
                "Media",
                format!("Missing media after open: {}", missing.join(", ")),
            );
            format!(
                "Opened {} — missing media: {}",
                path.display(),
                missing.join(", ")
            )
        };
    }

    fn is_dirty(&self) -> bool {
        self.focused().is_dirty()
    }

    pub(crate) fn project_name(&self) -> String {
        project_name(self.focused().project_path.as_deref(), &self.focused().name)
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let dirty = if self.is_dirty() { " *" } else { "" };
        let title = format!("{}{} — OpenReel", self.project_name(), dirty);
        if title != self.last_window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = title;
        }
    }

    pub(crate) fn request_close_project(&mut self, index: usize) {
        if self.projects.len() <= 1 || index >= self.projects.len() {
            return;
        }
        let id = self.projects[index].id;
        if self.projects[index].is_dirty() {
            self.focus_project(index);
            self.pending_project_action = Some(ProjectAction::CloseProject(id));
        } else {
            self.close_project(id);
        }
    }

    fn close_project(&mut self, id: u64) {
        let Some(index) = session_index_by_id(id, &self.projects) else {
            return;
        };
        if self.projects.len() <= 1 {
            return;
        }
        let closing_focused = index == self.focused_project;
        let next_focus = index_after_close(self.focused_project, index, self.projects.len());
        self.projects[index].stop_threads("the project was closed");
        let name = self.projects[index].name.clone();
        if index == 0 {
            let (first, remaining) = self.projects.split_at_mut(1);
            first[0]
                .recovery
                .move_pending_to(&mut remaining[0].recovery);
        }
        self.projects.remove(index);
        self.focus_project_with_rebind(next_focus, closing_focused);
        self.status = format!("Closed {name}");
    }

    fn request_exit(&mut self, ctx: &egui::Context) {
        if let Some(index) = self.projects.iter().position(|project| {
            project.is_dirty() && !self.exit_discarded_projects.contains(&project.id)
        }) {
            self.focus_project(index);
            self.pending_project_action = Some(ProjectAction::Exit(self.projects[index].id));
            return;
        }
        for project in &mut self.projects {
            project.stop_threads("OpenReel is closing");
        }
        self.allow_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending_project_action.is_none() {
                self.request_exit(ctx);
            }
        }
    }

    fn show_unsaved_confirmation(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_project_action else {
            return;
        };
        let project_id = match action {
            ProjectAction::CloseProject(id) | ProjectAction::Exit(id) => id,
        };
        let Some(project_index) = session_index_by_id(project_id, &self.projects) else {
            self.pending_project_action = None;
            return;
        };
        let project_name = self.projects[project_index].name.clone();
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(theme::caps_label(
                    "PROJECT HAS UNSAVED CHANGES",
                    color::STATUS_WARNING,
                ));
                ui.label(format!("Save changes to {project_name} before continuing?"));
                ui.add_space(space::TWO);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("Save")
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                        )
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .add(egui::Button::new("Discard").fill(color::SURFACE_ACTIVE))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if save && self.focused_project != project_index {
            self.focus_project(project_index);
        }
        if (save && self.save_project(false)) || discard {
            self.pending_project_action = None;
            match action {
                ProjectAction::CloseProject(id) => self.close_project(id),
                ProjectAction::Exit(id) => {
                    if discard && !self.exit_discarded_projects.contains(&id) {
                        self.exit_discarded_projects.push(id);
                    }
                    self.request_exit(ctx);
                }
            }
        } else if cancel {
            self.pending_project_action = None;
            if matches!(action, ProjectAction::Exit(_)) {
                self.exit_discarded_projects.clear();
            }
        }
    }

    pub(crate) fn send_operation(&mut self, operation: Operation) {
        if self.focused().core.send(Command::Do(operation)).is_err() {
            self.record_error("Operations", "Core actor stopped while applying the edit");
        } else {
            "Applying edit…".clone_into(&mut self.status);
        }
    }

    pub(crate) fn send_operations(&mut self, operations: Vec<Operation>) {
        let count = operations.len();
        if count == 0 {
            return;
        }
        if self
            .focused()
            .core
            .send(Command::DoBatch(operations))
            .is_err()
        {
            self.record_error(
                "Operations",
                "Core actor stopped while applying the edit batch",
            );
        } else {
            self.status = format!("Applying {count} batched editsâ€¦");
        }
    }

    fn request_asset_analysis(&self, asset: MediaAsset) {
        self.analysis.request_transcription(asset.clone());
        self.analysis.request_silence_detection(asset.clone());
        self.analysis.request_scene_detection(asset);
    }

    pub(crate) fn undo(&mut self) {
        if self.focused().core.send(Command::Undo).is_err() {
            self.record_error("Operations", "Core actor stopped while undoing");
        } else {
            "Undo".clone_into(&mut self.status);
        }
    }

    pub(crate) fn redo(&mut self) {
        if self.focused().core.send(Command::Redo).is_err() {
            self.record_error("Operations", "Core actor stopped while redoing");
        } else {
            "Redo".clone_into(&mut self.status);
        }
    }

    // Polling coordinates six independent channels and preserves their visible event ordering.
    #[allow(clippy::too_many_lines)]
    fn poll_background(&mut self, ctx: &egui::Context) {
        self.poll_agent(ctx);
        self.poll_export(ctx);
        self.poll_recording(ctx);
        for (asset, error) in self.visual_cache.poll(ctx) {
            self.error_log.push(
                "Media",
                format!("Could not build timeline visuals for asset {asset}: {error}"),
            );
        }
        if self.visual_cache.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        while let Ok((session_id, path, result)) = self.probe_rx.try_recv() {
            let Some(project_index) = session_index_by_id(session_id, &self.projects) else {
                continue;
            };
            match result {
                Ok(asset) => {
                    self.status = format!("Importing {}…", path.display());
                    self.projects[project_index]
                        .pending_timeline_adds
                        .push(asset.id);
                    if self.projects[project_index]
                        .core
                        .send(Command::Do(Operation::AddAsset { asset }))
                        .is_err()
                    {
                        self.record_error("Operations", "Core actor stopped while importing media");
                    }
                }
                Err(error) => self.record_error(
                    "Media",
                    format!("Could not import {}: {error}", path.display()),
                ),
            }
        }

        let core_events = self
            .projects
            .iter()
            .enumerate()
            .flat_map(|(project_index, project)| {
                project
                    .core_events
                    .try_iter()
                    .map(move |event| (project_index, event))
            })
            .collect::<Vec<_>>();
        for (project_index, event) in core_events {
            match event {
                Event::DocumentChanged {
                    doc,
                    last_op,
                    journal_command,
                } => {
                    let new_assets = doc
                        .media_pool
                        .iter()
                        .filter(|asset| {
                            self.projects[project_index]
                                .document
                                .asset(asset.id)
                                .is_none()
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    // Timeline attribution is ambiguous with concurrent
                    // writers, so every running thread receives the same
                    // review card while the monitor cues only once.
                    let any_agent_running = self.projects[project_index]
                        .threads
                        .iter()
                        .any(|thread| thread.running);
                    if any_agent_running
                        && let Some(range) = crate::edit_diff::changed_project_range(
                            &self.projects[project_index].document,
                            &doc,
                        )
                    {
                        let cue = TimeCode(
                            range
                                .start
                                .0
                                .saturating_sub(review_preroll_frames(doc.fps))
                                .max(0),
                        );
                        let edit_card = ChatEntry::EditCard {
                            summary: last_op
                                .as_ref()
                                .map_or_else(|| "Edited the timeline".to_owned(), operation_status),
                            start: range.start,
                            end: range.end,
                            cue,
                        };
                        for thread in self.projects[project_index]
                            .threads
                            .iter_mut()
                            .filter(|thread| thread.running)
                        {
                            thread.chat.push(edit_card.clone());
                        }
                        if project_index == self.focused_project {
                            self.projects[project_index].position =
                                TimeCode(cue.0.min(doc.duration.0.saturating_sub(1).max(0)));
                        }
                    }
                    self.projects[project_index].document = Arc::clone(&doc);
                    self.projects[project_index].transcript_selection = None;
                    if self.projects[project_index]
                        .selected_clip
                        .is_some_and(|clip| doc.clip(clip).is_none())
                    {
                        self.projects[project_index].selected_clip = None;
                    }
                    if self.projects[project_index]
                        .selected_marker
                        .is_some_and(|marker| doc.marker(marker).is_none())
                    {
                        self.projects[project_index].selected_marker = None;
                    }
                    if self.projects[project_index]
                        .selected_asset
                        .is_some_and(|asset| doc.asset(asset).is_none())
                    {
                        self.projects[project_index].selected_asset = None;
                    }
                    if doc.duration <= TimeCode::ZERO {
                        self.projects[project_index].position = TimeCode::ZERO;
                    } else {
                        self.projects[project_index].position = TimeCode(
                            self.projects[project_index]
                                .position
                                .0
                                .clamp(0, doc.duration.0.saturating_sub(1)),
                        );
                    }
                    if project_index == self.focused_project {
                        self.playing = false;
                        let position = self.projects[project_index].position;
                        self.playback.set_document(Arc::clone(&doc));
                        self.playback.seek(position);
                        self.playback.request_frame(position);
                    }
                    if let Some(Operation::AddAsset { asset }) = &last_op {
                        self.projects[project_index].selected_asset = Some(asset.id);
                        // A user import is one gesture: the probed asset goes
                        // straight onto the timeline, and the playhead moves
                        // to the new footage so the monitor answers "it
                        // worked". The pending list keeps agent-driven asset
                        // operations out of this path.
                        if let Some(index) = self.projects[project_index]
                            .pending_timeline_adds
                            .iter()
                            .position(|pending| *pending == asset.id)
                        {
                            self.projects[project_index]
                                .pending_timeline_adds
                                .remove(index);
                            let asset_id = asset.id;
                            self.add_asset_to_timeline_for(project_index, asset_id);
                        }
                    }
                    for asset in new_assets {
                        self.request_asset_analysis(asset);
                    }
                    if project_index == self.focused_project {
                        if let Some(operation) = last_op {
                            self.status = operation_status(&operation);
                        } else if let Some(JournalCommand::DoBatch(operations)) = journal_command {
                            self.status = format!("Applied {} linked edits", operations.len());
                        }
                    }
                }
                Event::OpRejected { error, .. } => {
                    let name = &self.projects[project_index].name;
                    self.record_error("Operations", format!("Edit rejected in {name}: {error}"));
                }
                Event::BatchRejected { error, .. } => {
                    let name = &self.projects[project_index].name;
                    self.record_error(
                        "Operations",
                        format!("Edit plan rejected in {name}: {error}"),
                    );
                }
                Event::QueryResult(_) => {}
            }
        }

        if self.projects.iter().any(|project| {
            project.document.media_pool.iter().any(|asset| {
                self.analysis.silence_status(asset).is_running()
                    || self.analysis.scene_status(asset).is_running()
            })
        }) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        while let Ok(event) = self.media_events.try_recv() {
            match event {
                MediaEvent::Position(position) => {
                    if !self.resume_after_scrub {
                        self.focused_mut().position = position;
                    }
                }
                MediaEvent::PlaybackStateChanged(state) => {
                    self.playing = state == PlaybackState::Playing;
                }
                MediaEvent::Error(error) => {
                    self.playing = false;
                    self.record_error("Media", format!("Playback error: {error}"));
                }
            }
        }

        let mut newest_frame = None;
        while let Ok(frame) = self.frames.try_recv() {
            newest_frame = Some(frame);
        }
        if let Some((at, frame)) = newest_frame {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    usize::try_from(frame.width).unwrap_or_default(),
                    usize::try_from(frame.height).unwrap_or_default(),
                ],
                frame.rgba.as_slice(),
            );
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("openreel-preview", image, egui::TextureOptions::LINEAR));
            }
            if !self.resume_after_scrub {
                self.focused_mut().position = at;
            }
        }

        if self.playing {
            let position = self.playback.position();
            self.focused_mut().position = position;
            ctx.request_repaint_after(Duration::from_millis(10));
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("New").clicked() {
                ui.close();
                self.new_project();
            }
            if ui.button("Open…").clicked() {
                ui.close();
                self.choose_project();
            }
            if ui.button("Save").clicked() {
                ui.close();
                self.save_project(false);
            }
            if ui.button("Save As…").clicked() {
                ui.close();
                self.save_project(true);
            }
            if ui
                .add_enabled(self.projects.len() > 1, egui::Button::new("Close Project"))
                .clicked()
            {
                ui.close();
                self.request_close_project(self.focused_project);
            }
            ui.separator();
            if ui
                .add_enabled(self.export_job.is_none(), egui::Button::new("Export MP4…"))
                .clicked()
            {
                ui.close();
                self.open_export_dialog();
            }
        });
    }
}

impl eframe::App for OpenReelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_window_title(ui.ctx());
        self.handle_close_request(ui.ctx());
        self.poll_background(ui.ctx());
        self.keyboard_shortcuts(ui.ctx());
        self.import_dropped_files(ui.ctx());
        let restore = self
            .projects
            .first_mut()
            .and_then(|project| project.recovery.show_dialog(ui.ctx()));
        if let Some(request) = restore {
            let journal_path = request.journal_path;
            let name = project_name(request.project_path.as_deref(), "Recovered project");
            let result = self
                .create_project_session(name, request.document, request.project_path)
                .map(|session| {
                    let assets = session.document.media_pool.clone();
                    self.projects.push(session);
                    self.focus_project(self.projects.len() - 1);
                    for asset in assets {
                        self.request_asset_analysis(asset);
                    }
                    self.projects[0].recovery.consume_pending(&journal_path);
                });
            self.status = crate::recovery::restore_status(result);
        }

        self.app_top_bar(ui);
        self.panel_layout(ui);
        self.show_export_dialog(ui.ctx());
        self.show_record_dialog(ui.ctx());
        self.show_settings_dialog(ui.ctx());
        self.show_help(ui.ctx());
        self.show_error_log(ui.ctx());
        self.show_unsaved_confirmation(ui.ctx());
        self.screenshot.update(ui.ctx());
    }
}

impl OpenReelApp {
    fn app_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("app-top-bar")
            .exact_size(size::TOP_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(
                // Separation by fill contrast, not outline (M25): the bar
                // sits one surface step above the panels beneath it.
                egui::Frame::new()
                    .fill(color::SURFACE)
                    .inner_margin(egui::Margin::symmetric(
                        theme::margin(space::THREE),
                        theme::margin(space::ONE),
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // The wordmark stays quiet: accent color is reserved for the
                    // playhead, selection, and live agent state.
                    ui.label(theme::wordmark("OPENREEL", color::TEXT_SECONDARY));
                    ui.separator();
                    self.file_menu(ui);
                    if ui
                        .add_enabled(
                            self.export_job.is_none(),
                            egui::Button::image_and_text(
                                Icon::Export.image(size::ICON_MD),
                                "Export",
                            ),
                        )
                        .clicked()
                    {
                        self.open_export_dialog();
                    }
                    self.record_control(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::image(Icon::Settings.image(size::ICON_SM))
                                    .image_tint_follows_text_color(true),
                            )
                            .on_hover_text("Settings")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                        // A zero-count alert chip is permanent noise; show it
                        // only when there is something to look at.
                        if self.error_log.len() > 0
                            && ui
                                .add(egui::Button::image_and_text(
                                    Icon::Alert.image(size::ICON_SM),
                                    format!("{}", self.error_log.len()),
                                ))
                                .on_hover_text("Open error log")
                                .clicked()
                        {
                            self.error_log_open = true;
                        }
                        // Summon toggles for the non-resident surfaces.
                        if ui
                            .selectable_label(self.show_material_strip, "Timeline")
                            .on_hover_text("Show the timeline and transcript strip")
                            .clicked()
                        {
                            self.show_material_strip = !self.show_material_strip;
                        }
                        if ui
                            .selectable_label(self.show_media_rail, "Media")
                            .on_hover_text("Show the media rail")
                            .clicked()
                        {
                            self.show_media_rail = !self.show_media_rail;
                        }
                        if ui
                            .selectable_label(self.show_thread_rail, "Threads")
                            .on_hover_text("Show the thread rail")
                            .clicked()
                        {
                            self.show_thread_rail = !self.show_thread_rail;
                        }
                        ui.separator();
                        ui.colored_label(color::TEXT_MUTED, &self.status);
                    });
                });
            });
    }

    fn panel_layout(&mut self, ui: &mut egui::Ui) {
        // Conversation-first geometry (M24, slimmed in M25): three columns by
        // default - thread rail, session, monitor. The media browser and the
        // material strip are summoned, never resident; the one contextual
        // self-raise left is a pending destructive confirmation raising the
        // timeline (span-level truth beats watching for approvals). Import
        // does not need a column: drop a file anywhere, use /import, or the
        // rail's import row - media lands on the timeline either way.
        let strip_visible =
            self.show_material_strip || !self.focused().pending_confirmations.is_empty();
        let rail_visible = self.show_media_rail;
        if strip_visible {
            egui::Panel::bottom("timeline-dock")
                .default_size(240.0)
                .min_size(160.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(color::PANEL)
                        .inner_margin(egui::Margin::same(theme::margin(space::TWO))),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.material_tab,
                            MaterialTab::Timeline,
                            "Timeline",
                        );
                        ui.selectable_value(
                            &mut self.material_tab,
                            MaterialTab::Transcript,
                            "Transcript",
                        );
                    });
                    ui.separator();
                    match self.material_tab {
                        MaterialTab::Timeline => self.timeline(ui),
                        MaterialTab::Transcript => self.transcript_panel(ui),
                    }
                });
        }
        if self.show_thread_rail {
            egui::Panel::left("thread-rail")
                .default_size(200.0)
                .min_size(160.0)
                .resizable(true)
                .frame(theme::panel_frame())
                .show(ui, |ui| self.thread_rail(ui));
        }
        if rail_visible {
            egui::Panel::left("media-rail")
                .default_size(220.0)
                .min_size(64.0)
                .resizable(true)
                .frame(theme::panel_frame())
                .show(ui, |ui| self.media_bin(ui));
        }
        egui::Panel::right("monitor-dock")
            .default_size(460.0)
            .min_size(340.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(color::PANEL)
                    .inner_margin(egui::Margin::same(theme::margin(space::TWO))),
            )
            .show(ui, |ui| {
                self.preview(ui);
                ui.separator();
                self.transport(ui);
                ui.add_space(space::ONE);
                ui.separator();
                self.inspector_dock(ui);
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(color::PANEL)
                    .inner_margin(egui::Margin::same(theme::margin(space::THREE))),
            )
            .show(ui, |ui| self.agent_panel(ui));
    }
}

/// Two seconds of lead-in so a reviewed change plays with context.
fn review_preroll_frames(fps: openreel_core::Rational) -> i64 {
    let nominal = i64::from(fps.numerator().saturating_add(fps.denominator() / 2))
        / i64::from(fps.denominator().max(1));
    nominal.max(1) * 2
}

fn operation_status(operation: &Operation) -> String {
    match operation {
        Operation::AddAsset { asset } => format!("Imported {}", asset.name),
        Operation::AddTrack { track } => format!("Added {:?} track {}", track.kind, track.id),
        Operation::RemoveTrack { track } => format!("Removed track {track}"),
        Operation::SetTrackSyncLock { track, locked } => format!(
            "Turned sync lock {} for track {track}",
            if *locked { "on" } else { "off" }
        ),
        Operation::AddClip { asset, .. } => format!("Added asset {asset} to timeline"),
        Operation::AddTitle { title, .. } => format!("Added title {:?}", title.text),
        Operation::AddFreezeFrame {
            asset,
            source_frame,
            ..
        } => format!("Added freeze frame {source_frame} from asset {asset}"),
        Operation::SplitClip { clip, at } => format!("Split clip {clip} at frame {at}"),
        Operation::TrimClip { clip, .. } => format!("Trimmed clip {clip}"),
        Operation::MoveClip { clip, to, .. } => format!("Moved clip {clip} to frame {to}"),
        Operation::DeleteClip { clip } => format!("Deleted clip {clip}"),
        Operation::RippleDeleteClip { clip } => format!("Ripple deleted clip {clip}"),
        Operation::RippleInsertGap {
            track,
            at,
            duration,
        } => format!("Inserted a {duration}-frame gap on track {track} at frame {at}"),
        Operation::LinkClips { clips } => format!("Linked {} clips", clips.len()),
        Operation::UnlinkClips { clips } => format!("Unlinked {} clips", clips.len()),
        Operation::AddMarker { marker } => format!("Added marker {}", marker.id),
        Operation::RemoveMarker { marker } => format!("Removed marker {marker}"),
        Operation::MoveMarker { marker, to } => format!("Moved marker {marker} to frame {to}"),
        Operation::AddEffect { clip, effect } => {
            format!("Added {} effect {} to clip {clip}", effect.name, effect.id)
        }
        Operation::RemoveEffect { clip, effect } => {
            format!("Removed effect {effect} from clip {clip}")
        }
        Operation::SetEffectParam {
            clip, effect, name, ..
        } => format!("Set {name} on effect {effect} for clip {clip}"),
        Operation::SetTitleParam { clip, name, .. } => {
            format!("Set {name} on title clip {clip}")
        }
        Operation::SetClipAudio { clip, .. } => format!("Set audio on clip {clip}"),
        Operation::AddTransition { clip, transition } => {
            format!("Added {} transition to clip {clip}", transition.name)
        }
        Operation::RemoveTransition { clip } => {
            format!("Removed transition from clip {clip}")
        }
        Operation::SetMarkerParam { marker, name, .. } => {
            format!("Set {name} on marker {marker}")
        }
        Operation::SetClipSpeed {
            clip,
            speed_percent,
        } => format!("Set clip {clip} speed to {speed_percent}%"),
    }
}

fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/openreel-icon.png")).ok()?;
    let image = image.thumbnail(256, 256).to_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

pub(crate) fn run() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([size::WINDOW_WIDTH, size::WINDOW_HEIGHT])
        .with_min_inner_size([size::WINDOW_MIN_WIDTH, size::WINDOW_MIN_HEIGHT]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "OpenReel",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport,
            ..Default::default()
        },
        Box::new(move |creation_context| {
            crate::theme::install(&creation_context.egui_ctx);
            egui_extras::install_image_loaders(&creation_context.egui_ctx);
            let render_state = creation_context
                .wgpu_render_state
                .as_ref()
                .expect("the OpenReel app requires eframe's wgpu renderer");
            let gpu = GpuContext::new(render_state.device.clone(), render_state.queue.clone());
            let media = Arc::new(
                FfmpegMediaEngine::new_with_gpu(gpu).expect("FFmpeg media engine must initialize"),
            );
            let mut app = OpenReelApp::new(media);
            // `OpenReel.exe project.openreel` opens the project directly; this
            // is also the hook Windows file association needs.
            if let Some(argument) = std::env::args().nth(1) {
                let path = PathBuf::from(&argument);
                if path.is_file() {
                    app.open_project(&path);
                } else {
                    app.record_error("Project", format!("Startup project not found: {argument}"));
                }
            }
            Ok(Box::new(app))
        }),
    )
}
