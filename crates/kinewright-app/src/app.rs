use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use eframe::egui;
use kinewright_agent::{ClaudeCodeDriver, CodexDriver, CursorAcpDriver};
use kinewright_core::{
    AgentDriver, Analysis, Command, Document, Event, Export, HarnessInfo, JournalCommand,
    MediaAsset, MediaError, MediaEvent, Operation, Playback, PlaybackState, TimeCode, Track,
    TrackId, TrackKind,
};
use kinewright_media::{FfmpegMediaEngine, GpuContext, compositor_required_limits};

use crate::{
    error_ui::ErrorLog,
    export_ui::{ExportDialog, ExportJob},
    icons::Icon,
    media_workflow::media_asset_requires_refresh,
    project::{ProjectSession, index_after_close, project_name, session_index_by_id},
    theme::{self, color, size, space},
    timeline_ui::is_internal_marker,
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
pub(crate) struct KinewrightApp {
    pub(crate) projects: Vec<ProjectSession>,
    pub(crate) focused_project: usize,
    next_project_id: u64,
    pub(crate) playback: Arc<dyn Playback>,
    pub(crate) analysis: Arc<dyn Analysis>,
    pub(crate) exporter: Arc<dyn Export>,
    pub(crate) frames: crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)>,
    pub(crate) media_events: crossbeam_channel::Receiver<MediaEvent>,
    pub(crate) visual_cache: crate::visual_cache::VisualCache,
    pub(crate) claude_info: Option<HarnessInfo>,
    pub(crate) codex_info: Option<HarnessInfo>,
    pub(crate) cursor_info: Option<HarnessInfo>,
    pub(crate) show_thread_rail: bool,
    pub(crate) settings_open: bool,
    /// Selectable models per harness; `None` chosen means the CLI's default.
    pub(crate) claude_models: Vec<kinewright_agent::ModelChoice>,
    pub(crate) codex_models: Vec<kinewright_agent::ModelChoice>,
    pub(crate) cursor_models: Vec<kinewright_agent::ModelChoice>,
    /// The model the Codex CLI's config actually runs as its default, so the
    /// picker's "Default" resolves to that model's real efforts and tiers.
    pub(crate) codex_default_model: Option<String>,
    pub(crate) claude_model: Option<String>,
    pub(crate) codex_model: Option<String>,
    pub(crate) cursor_model: Option<String>,
    pub(crate) claude_effort: Option<String>,
    pub(crate) codex_effort: Option<String>,
    pub(crate) cursor_effort: Option<String>,
    /// Service tier ids per harness; `None` means the provider's standard
    /// tier (only offered where the harness catalog advertises tiers).
    pub(crate) claude_tier: Option<String>,
    pub(crate) codex_tier: Option<String>,
    pub(crate) cursor_tier: Option<String>,
    pub(crate) probe_tx: mpsc::Sender<(u64, PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) probe_rx: mpsc::Receiver<(u64, PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) relink_probe_tx: mpsc::Sender<crate::media_workflow::RelinkProbeResponse>,
    pub(crate) relink_probe_rx: mpsc::Receiver<crate::media_workflow::RelinkProbeResponse>,
    pub(crate) relink_probe_pending: usize,
    pub(crate) media_status_tx: mpsc::Sender<crate::media_workflow::MediaStatusResponse>,
    pub(crate) media_status_rx: mpsc::Receiver<crate::media_workflow::MediaStatusResponse>,
    pub(crate) cache_clear_tx: mpsc::Sender<crate::media_workflow::CacheClearResponse>,
    pub(crate) cache_clear_rx: mpsc::Receiver<crate::media_workflow::CacheClearResponse>,
    pub(crate) media_statuses: crate::media_workflow::MediaStatusStore,
    /// A human Insert/Overwrite request that is waiting for its mandatory,
    /// current source availability verification. It is intentionally global
    /// to the app so a Source selection change cannot reveal pixels or enable
    /// another edit before the original request resolves fail-closed.
    pub(crate) pending_source_edit: Option<crate::media_workflow::PendingSourceEdit>,
    pub(crate) pending_legacy_relink: Option<crate::media_workflow::PendingLegacyRelink>,
    pub(crate) media_cache_dialog_open: bool,
    pub(crate) media_cache_inventory: Option<kinewright_core::MediaCacheInventory>,
    pub(crate) media_cache_clear_pending: Option<kinewright_core::MediaCacheFamily>,
    pub(crate) media_cache_clear_result: Option<kinewright_core::MediaCacheClearResult>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) color_scopes: crate::color_scopes_ui::ColorScopesState,
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
    /// Monotonic identity for one live control gesture (a slider drag).
    ///
    /// The counter is part of every coalesce key, so a second drag over the
    /// same control opens its own undo entry instead of merging into the
    /// previous gesture's entry.
    edit_gesture: u64,
}

impl KinewrightApp {
    // Construction keeps all channel subscriptions and coupled UI state initialization together.
    #[allow(clippy::too_many_lines)]
    fn new(media: Arc<FfmpegMediaEngine>, startup_path: Option<PathBuf>) -> Self {
        let mut load_error = None;
        let (name, document, project_path) = match startup_path {
            Some(path) if path.is_file() => match load_document(&path) {
                Ok(document) => {
                    let name = project_name(Some(&path), "Project 1");
                    (name, document, Some(path))
                }
                Err(error) => {
                    load_error = Some((
                        "Project",
                        format!("Could not open {}: {error}", path.display()),
                    ));
                    ("Project 1".to_owned(), default_project_document(), None)
                }
            },
            Some(path) => {
                load_error = Some((
                    "Project",
                    format!("Startup project not found: {}", path.display()),
                ));
                ("Project 1".to_owned(), default_project_document(), None)
            }
            None => ("Project 1".to_owned(), default_project_document(), None),
        };
        let frames = media.frames();
        let media_events = media.events();
        let visual_cache = crate::visual_cache::VisualCache::new(media.visual_asset_results());
        let (probe_tx, probe_rx) = mpsc::channel();
        let (relink_probe_tx, relink_probe_rx) = mpsc::channel();
        let (media_status_tx, media_status_rx) = mpsc::channel();
        let (cache_clear_tx, cache_clear_rx) = mpsc::channel();
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media;
        let mut project = ProjectSession::create(
            1,
            name,
            document.clone(),
            project_path.clone(),
            &playback,
            &analysis,
            &exporter,
        )
        .expect("startup project session must be valid");
        if project_path.is_some() {
            project.saved_document = Some(Arc::clone(&project.document));
        }
        if std::env::var_os("KINEWRIGHT_SCREENSHOT_TO").is_some()
            && let Some(clip) = project
                .document
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.content.title().is_some())
        {
            project.selected_clip = Some(clip.id);
        }
        let screenshotting = std::env::var_os("KINEWRIGHT_SCREENSHOT_TO").is_some();
        let assets = project.document.media_pool.clone();
        let error_log = ErrorLog::default();
        let claude_info = ClaudeCodeDriver.detect();
        let codex_info = CodexDriver.detect();
        let cursor_info = CursorAcpDriver.detect();
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
            cursor_info,
            show_thread_rail: true,
            settings_open: matches!(
                std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref(),
                Ok("settings")
            ),
            claude_models: kinewright_agent::claude_models(),
            codex_models: kinewright_agent::codex_models(),
            cursor_models: kinewright_agent::cursor_models(),
            codex_default_model: kinewright_agent::codex_default_model(),
            claude_model: None,
            codex_model: None,
            cursor_model: None,
            claude_effort: None,
            codex_effort: None,
            cursor_effort: None,
            claude_tier: None,
            codex_tier: None,
            cursor_tier: None,
            probe_tx,
            probe_rx,
            relink_probe_tx,
            relink_probe_rx,
            relink_probe_pending: 0,
            media_status_tx,
            media_status_rx,
            cache_clear_tx,
            cache_clear_rx,
            media_statuses: crate::media_workflow::MediaStatusStore::default(),
            pending_source_edit: None,
            pending_legacy_relink: None,
            media_cache_dialog_open: false,
            media_cache_inventory: None,
            media_cache_clear_pending: None,
            media_cache_clear_result: None,
            texture: None,
            color_scopes: crate::color_scopes_ui::ColorScopesState::default(),
            playing: false,
            meter_levels: [0.0; 2],
            resume_after_scrub: false,
            transcript_scope: TranscriptScope::default(),
            // The screenshot harness can pre-raise a summoned surface that no
            // startup interaction could otherwise reach in a static capture.
            material_tab: match std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref() {
                Ok("transcript") => MaterialTab::Transcript,
                _ => MaterialTab::default(),
            },
            show_material_strip: matches!(
                std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref(),
                Ok("timeline" | "transcript")
            ),
            show_media_rail: false,
            pending_project_action: None,
            exit_discarded_projects: Vec::new(),
            allow_close: false,
            last_window_title: String::new(),
            status: "Ready".to_owned(),
            export_dialog: ExportDialog {
                open: false,
                output: "export.mp4".to_owned(),
                width: resolution.0,
                height: resolution.1,
                fps_numerator: fps.numerator(),
                fps_denominator: fps.denominator(),
                delivery_aspect: None,
                focus_x_percent: 50,
                focus_y_percent: 50,
                conformance_cache: None,
            },
            export_job: None,
            help_open: false,
            ripple_mode: false,
            error_log,
            error_log_open,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
            recording: None,
            record_dialog: crate::recording::RecordDialog::default(),
            edit_gesture: 0,
        };
        app.playback
            .set_document(Arc::clone(&app.focused().document));
        app.playback.request_frame(TimeCode::ZERO);
        let opened_path = app.focused().project_path.clone();
        if let Some((source, message)) = load_error {
            app.record_error(source, message);
        } else if let Some(path) = opened_path {
            let missing: Vec<String> = app
                .focused()
                .document
                .media_pool
                .iter()
                .filter(|asset| !asset.path.is_file())
                .map(|asset| format!("{} ({})", asset.name, asset.path.display()))
                .collect();
            if missing.is_empty() {
                app.status = if screenshotting {
                    "Ready".to_owned()
                } else {
                    format!("Opened {}", path.display())
                };
            } else {
                app.record_error(
                    "Media",
                    format!("Missing media after open: {}", missing.join(", ")),
                );
                app.status = format!(
                    "Opened {} — missing media: {}",
                    path.display(),
                    missing.join(", ")
                );
            }
            if !screenshotting {
                for asset in assets {
                    app.request_asset_analysis(asset);
                }
            }
        }
        app.queue_media_status_checks_for_project(0);
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
            &self.exporter,
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
                .add_filter("Kinewright project", &["kinewright"])
                .set_file_name("project.kinewright")
                .save_file()
        });
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension("kinewright");
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
            .add_filter("Kinewright project", &["kinewright", "json"])
            .pick_file()
        else {
            return;
        };
        self.open_project(&path);
    }

    pub(crate) fn new_project(&mut self) {
        let name = format!("Project {}", self.next_project_id);
        let session = match self.create_project_session(name, default_project_document(), None) {
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
        "Ready".clone_into(&mut self.status);
    }

    fn open_project(&mut self, path: &Path) {
        let document = match load_document(path) {
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
        self.queue_media_status_checks_for_project(self.focused_project);
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
        let title = format!("{}{} — Kinewright", self.project_name(), dirty);
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
        self.media_statuses.remove_session(id);
        if self
            .pending_source_edit
            .as_ref()
            .is_some_and(|pending| pending.session_id == id)
        {
            self.pending_source_edit = None;
        }
        if self
            .pending_legacy_relink
            .as_ref()
            .is_some_and(|pending| pending.session_id == id)
        {
            self.pending_legacy_relink = None;
        }
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
            project.stop_threads("Kinewright is closing");
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

    /// Open a new live-gesture identity and return it.
    pub(crate) fn begin_edit_gesture(&mut self) -> u64 {
        self.edit_gesture = self.edit_gesture.wrapping_add(1);
        self.edit_gesture
    }

    #[must_use]
    pub(crate) const fn edit_gesture(&self) -> u64 {
        self.edit_gesture
    }

    /// Send one batch that belongs to a live gesture such as a dragged slider.
    ///
    /// Consecutive batches that share `coalesce_key` collapse into a single
    /// undo entry whose undo target is the document from before the gesture,
    /// while every batch still advances the revision so the preview updates.
    pub(crate) fn send_operations_coalesced(
        &mut self,
        operations: Vec<Operation>,
        coalesce_key: String,
    ) {
        let count = operations.len();
        if count == 0 {
            return;
        }
        if self
            .focused()
            .core
            .send(Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            })
            .is_err()
        {
            self.record_error(
                "Operations",
                "Core actor stopped while applying the live edit",
            );
        } else {
            self.status = format!("Applying {count} live edits\u{2026}");
        }
    }

    fn request_asset_analysis(&self, asset: MediaAsset) {
        self.analysis.request_transcription(asset.clone());
        self.analysis.request_silence_detection(asset.clone());
        self.analysis.request_scene_detection(asset.clone());
        self.analysis.request_beat_detection(asset);
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
        self.poll_media_workflow(ctx);
        for (asset, error) in self.visual_cache.poll(ctx) {
            self.error_log.push(
                "Media",
                format!("Could not build timeline visuals for asset {asset}: {error}"),
            );
        }
        if self.visual_cache.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.media_statuses.has_pending()
            || self.relink_probe_pending > 0
            || self.media_cache_clear_pending.is_some()
        {
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
                    revision,
                    last_op,
                    journal_command,
                } => {
                    let previous_document = Arc::clone(&self.projects[project_index].document);
                    let media_changed_assets = doc
                        .media_pool
                        .iter()
                        .filter(|asset| {
                            media_asset_requires_refresh(
                                previous_document.asset(asset.id),
                                asset,
                                last_op.as_ref(),
                            )
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let previous_paths = media_changed_assets
                        .iter()
                        .filter_map(|asset| {
                            previous_document
                                .asset(asset.id)
                                .map(|previous| previous.path.clone())
                        })
                        .collect::<Vec<_>>();
                    let session_id = self.projects[project_index].id;
                    let invalidated_pending_source_edit =
                        self.pending_source_edit.as_ref().is_some_and(|pending| {
                            pending.session_id == session_id
                                && media_changed_assets
                                    .iter()
                                    .any(|asset| asset.id == pending.asset_id)
                        });
                    for asset in &media_changed_assets {
                        self.media_statuses.invalidate(session_id, asset.id);
                    }
                    if invalidated_pending_source_edit {
                        self.pending_source_edit = None;
                        self.record_error(
                            "Source monitor",
                            "Source file changed while Source was being verified; no edit was applied",
                        );
                    }
                    for path in previous_paths {
                        self.visual_cache.invalidate_path(&path);
                    }
                    for asset in &media_changed_assets {
                        if self
                            .media_statuses
                            .path_has_changed_observation(&asset.path)
                        {
                            self.visual_cache.invalidate_path(&asset.path);
                        } else {
                            self.visual_cache.invalidate_and_unblock_path(&asset.path);
                        }
                    }
                    self.projects[project_index].document = Arc::clone(&doc);
                    self.projects[project_index].revision = revision;
                    self.projects[project_index].transcript_selection = None;
                    if self.projects[project_index]
                        .selected_clip
                        .is_some_and(|clip| doc.clip(clip).is_none())
                    {
                        self.projects[project_index].selected_clip = None;
                    }
                    if self.projects[project_index]
                        .selected_marker
                        .is_some_and(|marker| doc.marker(marker).is_none_or(is_internal_marker))
                    {
                        self.projects[project_index].selected_marker = None;
                    }
                    if self.projects[project_index]
                        .selected_asset
                        .is_some_and(|asset| doc.asset(asset).is_none())
                    {
                        self.projects[project_index].selected_asset = None;
                    }
                    self.projects[project_index].reconcile_source_state();
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
                        if !media_changed_assets.is_empty() {
                            self.texture = None;
                        }
                        let position = self.projects[project_index].position;
                        self.playback.set_document(Arc::clone(&doc));
                        self.playback.seek(position);
                        self.playback.request_frame(position);
                    }
                    if let Some(Operation::AddAsset { asset }) = &last_op {
                        self.projects[project_index].cue_source_asset(asset.id);
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
                    for asset in media_changed_assets {
                        self.request_asset_analysis(asset);
                    }
                    self.queue_media_status_checks_for_project(project_index);
                    if project_index == self.focused_project {
                        if let Some(operation) = last_op {
                            self.status = operation_status(&operation);
                        } else {
                            match journal_command {
                                Some(JournalCommand::DoBatch(operations)) => {
                                    self.status =
                                        format!("Applied {} linked edits", operations.len());
                                }
                                Some(JournalCommand::DoBatchCoalesced { operations, .. }) => {
                                    // Live slider drags coalesce into one undo entry; the
                                    // status reflects the gesture rather than the frame count.
                                    self.status = format!(
                                        "Adjusting {} linked edit(s) as one undo step",
                                        operations.len()
                                    );
                                }
                                _ => {}
                            }
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
                Event::RevisionConflict { expected, actual } => {
                    let name = &self.projects[project_index].name;
                    self.record_error(
                        "Operations",
                        format!(
                            "Stale edit rejected in {name}: expected timeline revision {expected}, current revision is {actual}"
                        ),
                    );
                }
                Event::QueryResult(_) => {}
            }
        }

        if self.projects.iter().any(|project| {
            project.document.media_pool.iter().any(|asset| {
                self.analysis.silence_status(asset).is_running()
                    || self.analysis.scene_status(asset).is_running()
                    || self.analysis.beat_status(asset).is_running()
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
                self.texture = Some(ctx.load_texture(
                    "kinewright-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
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

impl eframe::App for KinewrightApp {
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
                    self.queue_media_status_checks_for_project(self.focused_project);
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
        self.show_media_cache_dialog(ui.ctx());
        self.show_legacy_relink_confirmation(ui.ctx());
        self.show_help(ui.ctx());
        self.show_error_log(ui.ctx());
        self.show_unsaved_confirmation(ui.ctx());
        self.screenshot.update(ui.ctx());
    }
}

impl KinewrightApp {
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
                    ui.label(theme::wordmark("KINEWRIGHT", color::TEXT_SECONDARY));
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
        // Summoned surfaces slide rather than pop (M28 motion): the animated
        // panel variants ease presence over the house animation_time.
        let mut strip_open = self.show_material_strip
            || self
                .focused()
                .threads
                .iter()
                .any(|thread| !thread.pending_confirmations.is_empty());
        let mut thread_rail_open = self.show_thread_rail;
        let mut media_rail_open = self.show_media_rail;
        egui::Panel::bottom("timeline-dock")
            .default_size(240.0)
            .min_size(160.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(color::PANEL)
                    .inner_margin(egui::Margin::same(theme::margin(space::TWO))),
            )
            .show_collapsible(ui, &mut strip_open, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.material_tab, MaterialTab::Timeline, "Timeline");
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
        egui::Panel::left("thread-rail")
            .default_size(200.0)
            .min_size(160.0)
            .resizable(true)
            .frame(theme::panel_frame())
            .show_collapsible(ui, &mut thread_rail_open, |ui| self.thread_rail(ui));
        egui::Panel::left("media-rail")
            .default_size(220.0)
            .min_size(64.0)
            .resizable(true)
            .frame(theme::panel_frame())
            .show_collapsible(ui, &mut media_rail_open, |ui| self.media_bin(ui));
        // show_collapsible flips its flag when the user drags a panel shut;
        // write the results back so the top-bar toggles stay truthful. A
        // confirmation-forced strip reopens next frame by design.
        if self.show_material_strip && !strip_open {
            self.show_material_strip = false;
        }
        self.show_thread_rail = thread_rail_open;
        self.show_media_rail = media_rail_open;
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
                self.color_scopes_panel(ui);
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
pub(crate) fn review_preroll_frames(fps: kinewright_core::Rational) -> i64 {
    let nominal = i64::from(fps.numerator().saturating_add(fps.denominator() / 2))
        / i64::from(fps.denominator().max(1));
    nominal.max(1) * 2
}

#[allow(clippy::too_many_lines)]
pub(crate) fn operation_status(operation: &Operation) -> String {
    match operation {
        Operation::AddAsset { asset } => format!("Imported {}", asset.name),
        Operation::RelinkAsset {
            asset, candidate, ..
        } => {
            format!("Relinked asset {asset} to {}", candidate.path.display())
        }
        Operation::SetAssetColorDescription { asset, .. } => {
            format!("Updated source color metadata for asset {asset}")
        }
        Operation::SetColorContext { .. } => "Updated project color pipeline context".to_owned(),
        Operation::UpsertBin { bin } => format!("Saved bin {}", bin.name),
        Operation::RemoveBin { bin } => format!("Removed bin {bin}"),
        Operation::SetAssetBin { asset, bin } => bin.map_or_else(
            || format!("Moved asset {asset} to the media root"),
            |bin| format!("Moved asset {asset} to bin {bin}"),
        ),
        Operation::UpsertStringOut { string_out } => {
            format!("Saved string-out {}", string_out.name)
        }
        Operation::RemoveStringOut { string_out } => {
            format!("Removed string-out {string_out}")
        }
        Operation::UpsertSyncGroup { sync_group } => {
            format!("Saved sync group {}", sync_group.name)
        }
        Operation::RemoveSyncGroup { sync_group } => {
            format!("Removed sync group {sync_group}")
        }
        Operation::UpsertAudioBus { bus } => {
            format!("Updated audio bus {} ({})", bus.id, bus.name)
        }
        Operation::RemoveAudioBus { bus } => format!("Removed audio bus {bus}"),
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
        Operation::ThreePointEdit { mode, asset, .. } => {
            format!("Applied {mode:?} three-point edit from asset {asset}")
        }
        Operation::PatchedThreePointEdit {
            mode,
            asset,
            video_track,
            audio_track,
            ..
        } => format!(
            "Applied {mode:?} source patch from asset {asset} (video {}, audio {})",
            video_track.map_or_else(|| "off".to_owned(), |track| track.to_string()),
            audio_track.map_or_else(|| "off".to_owned(), |track| track.to_string()),
        ),
        Operation::SlipClip { clip, .. } => format!("Slipped clip {clip}"),
        Operation::RollEdit {
            left_clip,
            right_clip,
            to,
        } => format!("Rolled clips {left_clip}/{right_clip} to frame {to}"),
        Operation::SlideClip { clip, to } => format!("Slid clip {clip} to frame {to}"),
        Operation::ReplaceClip { clip, asset, .. } => {
            format!("Replaced clip {clip} with asset {asset}")
        }
        Operation::FitToFill { clip, asset, .. } => {
            format!("Fit asset {asset} into clip {clip}")
        }
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
        Operation::SetEffectKeyframes {
            clip,
            effect,
            name,
            curve,
        } => format!(
            "Set {} keyframes for {name} on effect {effect} for clip {clip}",
            curve.keyframes.len()
        ),
        Operation::ClearEffectKeyframes {
            clip, effect, name, ..
        } => format!("Cleared {name} keyframes on effect {effect} for clip {clip}"),
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

fn default_project_document() -> Document {
    Document {
        tracks: vec![Track {
            id: DEFAULT_TRACK_ID,
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        color_context: kinewright_core::ColorContext::default(),
        ..Document::default()
    }
}

fn load_document(path: &Path) -> Result<Document, String> {
    let json = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let document: Document = serde_json::from_str(&json).map_err(|error| error.to_string())?;
    document.validate().map_err(|error| error.to_string())?;
    Ok(document)
}

fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/kinewright-icon.png")).ok()?;
    let image = image.thumbnail(256, 256).to_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn native_wgpu_configuration() -> eframe::WgpuConfiguration {
    let mut configuration = eframe::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut configuration.wgpu_setup {
        // Kinewright's native render contract is Vulkan on Linux, DX12 on
        // Windows, and Metal on macOS. eframe otherwise also considers GL and
        // requests WebGL2-compatible device limits there, which expose no
        // fragment-stage storage buffers and cannot run the ordered primary
        // correction shader.
        setup.instance_descriptor.backends = eframe::wgpu::Backends::PRIMARY;
        setup.device_descriptor = Arc::new(|_| eframe::wgpu::DeviceDescriptor {
            label: Some("Kinewright shared native device"),
            required_limits: compositor_required_limits(eframe::wgpu::Limits::default()),
            ..Default::default()
        });
    }
    configuration
}

pub(crate) fn run() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([size::WINDOW_WIDTH, size::WINDOW_HEIGHT])
        .with_min_inner_size([size::WINDOW_MIN_WIDTH, size::WINDOW_MIN_HEIGHT]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "Kinewright",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport,
            wgpu_options: native_wgpu_configuration(),
            ..Default::default()
        },
        Box::new(move |creation_context| {
            crate::theme::install(&creation_context.egui_ctx);
            egui_extras::install_image_loaders(&creation_context.egui_ctx);
            let render_state = creation_context
                .wgpu_render_state
                .as_ref()
                .expect("the Kinewright app requires eframe's wgpu renderer");
            let gpu = GpuContext::new_with_adapter_info(
                render_state.device.clone(),
                render_state.queue.clone(),
                render_state.adapter.get_info(),
            );
            let media = Arc::new(
                FfmpegMediaEngine::new_with_gpu(gpu).expect("FFmpeg media engine must initialize"),
            );
            let startup = std::env::args().nth(1).map(PathBuf::from);
            let app = KinewrightApp::new(media, startup);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_renderer_excludes_the_unsupported_gl_backend() {
        let configuration = super::native_wgpu_configuration();
        let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = configuration.wgpu_setup else {
            panic!("native app must create its configured wgpu instance");
        };
        assert_eq!(
            setup.instance_descriptor.backends,
            eframe::wgpu::Backends::PRIMARY
        );
        assert!(
            !setup
                .instance_descriptor
                .backends
                .contains(eframe::wgpu::Backends::GL)
        );
    }
}
