use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use eframe::egui;
use openreel_agent::{
    ClaudeCodeDriver, CodexDriver, ConfirmationBroker, ConfirmationRequest, McpServer,
};
use openreel_core::{
    AgentDriver, AgentEvent, AgentSession, Analysis, AssetId, ClipId, Command, Core, Document,
    Event, Export, HarnessInfo, JournalCommand, MarkerId, MediaAsset, MediaError, MediaEvent,
    Operation, Playback, PlaybackState, TimeCode, Track, TrackId, TrackKind,
};
use openreel_media::{FfmpegMediaEngine, GpuContext};

use crate::{
    chat_ui::{AgentHarnessChoice, ChatEntry, CostAccumulator},
    error_ui::ErrorLog,
    export_ui::{ExportDialog, ExportJob},
    icons::Icon,
    theme::{self, color, size, space, type_size},
    transcript_ui::TranscriptScope,
};

const DEFAULT_TRACK_ID: TrackId = TrackId(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectAction {
    New,
    Open,
    Close,
}

// Independent transport, agent, dialog, and window flags model separate UI state machines.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct OpenReelApp {
    pub(crate) core: Core,
    pub(crate) core_events: crossbeam_channel::Receiver<Event>,
    pub(crate) playback: Arc<dyn Playback>,
    pub(crate) analysis: Arc<dyn Analysis>,
    pub(crate) exporter: Arc<dyn Export>,
    pub(crate) frames: crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    pub(crate) media_events: crossbeam_channel::Receiver<MediaEvent>,
    pub(crate) visual_cache: crate::visual_cache::VisualCache,
    pub(crate) mcp_server: Option<McpServer>,
    pub(crate) claude_info: Option<HarnessInfo>,
    pub(crate) codex_info: Option<HarnessInfo>,
    pub(crate) agent_harness: AgentHarnessChoice,
    pub(crate) agent_session: Option<Box<dyn AgentSession>>,
    pub(crate) agent_events: Option<crossbeam_channel::Receiver<AgentEvent>>,
    pub(crate) agent_running: bool,
    pub(crate) agent_input: String,
    pub(crate) agent_turn_cap: u32,
    pub(crate) agent_cost: CostAccumulator,
    pub(crate) chat: Vec<ChatEntry>,
    pub(crate) confirmations: Option<ConfirmationBroker>,
    pub(crate) pending_confirmations: Vec<ConfirmationRequest>,
    pub(crate) probe_tx: mpsc::Sender<(PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) probe_rx: mpsc::Receiver<(PathBuf, Result<MediaAsset, MediaError>)>,
    pub(crate) document: Arc<Document>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) position: TimeCode,
    pub(crate) playing: bool,
    pub(crate) resume_after_scrub: bool,
    pub(crate) selected_clip: Option<ClipId>,
    pub(crate) selected_marker: Option<MarkerId>,
    pub(crate) selected_asset: Option<AssetId>,
    pub(crate) title_text_draft: Option<(ClipId, String)>,
    pub(crate) marker_label_draft: Option<(MarkerId, String)>,
    pub(crate) title_text_focus: Option<ClipId>,
    pub(crate) transcript_scope: TranscriptScope,
    pub(crate) pixels_per_frame: f32,
    pub(crate) timeline_zoom_target: f32,
    pub(crate) timeline_scroll_target: f32,
    pub(crate) project_path: Option<PathBuf>,
    saved_document: Option<Arc<Document>>,
    pending_project_action: Option<ProjectAction>,
    allow_close: bool,
    last_window_title: String,
    pub(crate) status: String,
    pub(crate) export_dialog: ExportDialog,
    pub(crate) export_job: Option<ExportJob>,
    pub(crate) help_open: bool,
    pub(crate) ripple_mode: bool,
    pub(crate) error_log: ErrorLog,
    pub(crate) error_log_open: bool,
    recovery: crate::recovery::Recovery,
    screenshot: crate::screenshot::ScreenshotCapture,
}

impl OpenReelApp {
    // Construction keeps all channel subscriptions and coupled UI state initialization together.
    #[allow(clippy::too_many_lines)]
    fn new(media: Arc<FfmpegMediaEngine>) -> Self {
        let document = Document::default();
        let core = Core::spawn(document.clone()).expect("default document must be valid");
        let core_events = core
            .subscribe()
            .expect("Core actor must accept subscribers");
        let frames = media.frames();
        let media_events = media.events();
        let visual_cache = crate::visual_cache::VisualCache::new(media.visual_asset_results());
        let (probe_tx, probe_rx) = mpsc::channel();
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media;
        let mut chat = Vec::new();
        let mut error_log = ErrorLog::default();
        let mcp_server =
            match McpServer::start(core.clone(), Arc::clone(&playback), Arc::clone(&analysis)) {
                Ok(server) => Some(server),
                Err(error) => {
                    let message = format!("Could not start the OpenReel agent server: {error}");
                    error_log.push("Agent", message.clone());
                    chat.push(ChatEntry::Text(message));
                    None
                }
            };
        let confirmations = mcp_server.as_ref().map(McpServer::confirmations);
        let claude_info = ClaudeCodeDriver.detect();
        let codex_info = CodexDriver.detect();
        let agent_harness = if claude_info.is_some() {
            AgentHarnessChoice::ClaudeCode
        } else {
            AgentHarnessChoice::Codex
        };
        let resolution = document.resolution;
        let fps = document.fps;
        let error_log_open = error_log.len() > 0;
        let recovery = crate::recovery::Recovery::start(&core);
        let mut app = Self {
            core,
            core_events,
            playback,
            analysis,
            exporter,
            frames,
            media_events,
            visual_cache,
            mcp_server,
            claude_info,
            codex_info,
            agent_harness,
            agent_session: None,
            agent_events: None,
            agent_running: false,
            agent_input: String::new(),
            agent_turn_cap: 8,
            agent_cost: CostAccumulator::default(),
            chat,
            confirmations,
            pending_confirmations: Vec::new(),
            probe_tx,
            probe_rx,
            document: Arc::new(document),
            texture: None,
            position: TimeCode::ZERO,
            playing: false,
            resume_after_scrub: false,
            selected_clip: None,
            selected_marker: None,
            selected_asset: None,
            title_text_draft: None,
            marker_label_draft: None,
            title_text_focus: None,
            transcript_scope: TranscriptScope::default(),
            pixels_per_frame: 6.0,
            timeline_zoom_target: 6.0,
            timeline_scroll_target: 0.0,
            project_path: None,
            saved_document: None,
            pending_project_action: None,
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
            recovery,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
        };
        if app
            .core
            .send(Command::Do(Operation::AddTrack {
                track: Track {
                    id: DEFAULT_TRACK_ID,
                    kind: TrackKind::Video,
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
        app
    }

    pub(crate) fn save_project(&mut self, save_as: bool) -> bool {
        let path = if save_as {
            None
        } else {
            self.project_path.clone()
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
        let result = serde_json::to_string_pretty(&*self.document)
            .map_err(|error| error.to_string())
            .and_then(|json| fs::write(&path, json).map_err(|error| error.to_string()));
        match result {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.saved_document = Some(Arc::clone(&self.document));
                self.recovery.checkpoint(&self.core);
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

    fn new_project(&mut self) {
        if let Err(error) = self.replace_core(Document::default()) {
            self.record_error(
                "Project",
                format!("Could not create a new project: {error}"),
            );
            return;
        }
        self.project_path = None;
        self.saved_document = None;
        "Creating default video track…".clone_into(&mut self.status);
        self.send_operation(Operation::AddTrack {
            track: Track {
                id: DEFAULT_TRACK_ID,
                kind: TrackKind::Video,
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
        if let Err(error) = self.replace_core(document) {
            self.record_error(
                "Project",
                format!("Could not open {}: {error}", path.display()),
            );
            return;
        }
        self.project_path = Some(path.to_path_buf());
        self.saved_document = Some(Arc::clone(&self.document));
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
        self.saved_document
            .as_deref()
            .is_none_or(|saved| saved != self.document.as_ref())
    }

    fn project_name(&self) -> String {
        self.project_path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled")
            .to_owned()
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let dirty = if self.is_dirty() { " *" } else { "" };
        let title = format!("{}{} — OpenReel", self.project_name(), dirty);
        if title != self.last_window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = title;
        }
    }

    fn request_project_action(&mut self, action: ProjectAction, ctx: &egui::Context) {
        if self.is_dirty() {
            self.pending_project_action = Some(action);
        } else {
            self.perform_project_action(action, ctx);
        }
    }

    fn perform_project_action(&mut self, action: ProjectAction, ctx: &egui::Context) {
        match action {
            ProjectAction::New => self.new_project(),
            ProjectAction::Open => self.choose_project(),
            ProjectAction::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.allow_close && self.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_project_action = Some(ProjectAction::Close);
        }
    }

    fn show_unsaved_confirmation(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_project_action else {
            return;
        };
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.colored_label(
                    color::STATUS_WARNING,
                    egui::RichText::new("PROJECT HAS UNSAVED CHANGES")
                        .strong()
                        .size(type_size::MICRO),
                );
                ui.label(format!(
                    "Save changes to {} before continuing?",
                    self.project_name()
                ));
                ui.add_space(space::TWO);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("Save")
                                .fill(color::ACCENT_28)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_72)),
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
        if (save && self.save_project(false)) || discard {
            self.pending_project_action = None;
            self.perform_project_action(action, ctx);
        } else if cancel {
            self.pending_project_action = None;
        }
    }

    fn replace_core(&mut self, document: Document) -> Result<(), String> {
        let core = Core::spawn(document.clone()).map_err(|error| error.to_string())?;
        let events = core.subscribe().map_err(|error| error.to_string())?;
        let mcp_server = McpServer::start(
            core.clone(),
            Arc::clone(&self.playback),
            Arc::clone(&self.analysis),
        )
        .map_err(|error| format!("agent server: {error}"))?;
        let confirmations = mcp_server.confirmations();
        if let Some(session) = &mut self.agent_session {
            session.interrupt();
        }
        if let Some(confirmations) = &self.confirmations {
            confirmations.reject_all("the project changed during confirmation");
        }
        self.playback.pause();
        self.recovery.attach(&core);
        self.core = core;
        self.core_events = events;
        self.mcp_server = Some(mcp_server);
        self.confirmations = Some(confirmations);
        self.pending_confirmations.clear();
        self.agent_session = None;
        self.agent_events = None;
        self.agent_running = false;
        self.document = Arc::new(document);
        self.position = TimeCode::ZERO;
        self.timeline_scroll_target = 0.0;
        self.playing = false;
        self.resume_after_scrub = false;
        self.selected_clip = None;
        self.selected_marker = None;
        self.selected_asset = None;
        self.texture = None;
        self.visual_cache.clear();
        self.playback.set_document(Arc::clone(&self.document));
        for asset in &self.document.media_pool {
            self.request_asset_analysis(asset.clone());
        }
        self.playback.request_frame(TimeCode::ZERO);
        Ok(())
    }

    pub(crate) fn send_operation(&mut self, operation: Operation) {
        if self.core.send(Command::Do(operation)).is_err() {
            self.record_error("Operations", "Core actor stopped while applying the edit");
        } else {
            "Applying edit…".clone_into(&mut self.status);
        }
    }

    pub(crate) fn send_operations(&mut self, operations: Vec<Operation>) {
        match operations.len() {
            0 => {}
            1 => self.send_operation(
                operations
                    .into_iter()
                    .next()
                    .expect("a one-operation vector has one item"),
            ),
            count => {
                if self.core.send(Command::DoBatch(operations)).is_err() {
                    self.record_error(
                        "Operations",
                        "Core actor stopped while applying the linked edit",
                    );
                } else {
                    self.status = format!("Applying {count} linked editsâ€¦");
                }
            }
        }
    }

    fn request_asset_analysis(&self, asset: MediaAsset) {
        self.analysis.request_transcription(asset.clone());
        self.analysis.request_silence_detection(asset.clone());
        self.analysis.request_scene_detection(asset);
    }

    pub(crate) fn undo(&mut self) {
        if self.core.send(Command::Undo).is_err() {
            self.record_error("Operations", "Core actor stopped while undoing");
        } else {
            "Undo".clone_into(&mut self.status);
        }
    }

    pub(crate) fn redo(&mut self) {
        if self.core.send(Command::Redo).is_err() {
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
        for (asset, error) in self.visual_cache.poll(ctx) {
            self.error_log.push(
                "Media",
                format!("Could not build timeline visuals for asset {asset}: {error}"),
            );
        }
        if self.visual_cache.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        while let Ok((path, result)) = self.probe_rx.try_recv() {
            match result {
                Ok(asset) => {
                    self.status = format!("Importing {}…", path.display());
                    self.send_operation(Operation::AddAsset { asset });
                }
                Err(error) => self.record_error(
                    "Media",
                    format!("Could not import {}: {error}", path.display()),
                ),
            }
        }

        while let Ok(event) = self.core_events.try_recv() {
            match event {
                Event::DocumentChanged {
                    doc,
                    last_op,
                    journal_command,
                } => {
                    let new_assets = doc
                        .media_pool
                        .iter()
                        .filter(|asset| self.document.asset(asset.id).is_none())
                        .cloned()
                        .collect::<Vec<_>>();
                    self.document = Arc::clone(&doc);
                    if self
                        .selected_clip
                        .is_some_and(|clip| doc.clip(clip).is_none())
                    {
                        self.selected_clip = None;
                    }
                    if self
                        .selected_marker
                        .is_some_and(|marker| doc.marker(marker).is_none())
                    {
                        self.selected_marker = None;
                    }
                    if self
                        .selected_asset
                        .is_some_and(|asset| doc.asset(asset).is_none())
                    {
                        self.selected_asset = None;
                    }
                    if doc.duration <= TimeCode::ZERO {
                        self.position = TimeCode::ZERO;
                    } else {
                        self.position =
                            TimeCode(self.position.0.clamp(0, doc.duration.0.saturating_sub(1)));
                    }
                    self.playing = false;
                    self.playback.set_document(Arc::clone(&doc));
                    self.playback.seek(self.position);
                    self.playback.request_frame(self.position);
                    if let Some(Operation::AddAsset { asset }) = &last_op {
                        self.selected_asset = Some(asset.id);
                    }
                    for asset in new_assets {
                        self.request_asset_analysis(asset);
                    }
                    if let Some(operation) = last_op {
                        self.status = operation_status(&operation);
                    } else if let Some(JournalCommand::DoBatch(operations)) = journal_command {
                        self.status = format!("Applied {} linked edits", operations.len());
                    }
                }
                Event::OpRejected { error, .. } => {
                    self.record_error("Operations", format!("Edit rejected: {error}"));
                }
                Event::BatchRejected { error, .. } => {
                    self.record_error("Operations", format!("Edit plan rejected: {error}"));
                }
                Event::QueryResult(_) => {}
            }
        }

        if self.document.media_pool.iter().any(|asset| {
            self.analysis.silence_status(asset.id).is_running()
                || self.analysis.scene_status(asset.id).is_running()
        }) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        while let Ok(event) = self.media_events.try_recv() {
            match event {
                MediaEvent::Position(position) => {
                    if !self.resume_after_scrub {
                        self.position = position;
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
                self.position = at;
            }
        }

        if self.playing {
            self.position = self.playback.position();
            ctx.request_repaint_after(Duration::from_millis(10));
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("New").clicked() {
                ui.close();
                self.request_project_action(ProjectAction::New, ui.ctx());
            }
            if ui.button("Open…").clicked() {
                ui.close();
                self.request_project_action(ProjectAction::Open, ui.ctx());
            }
            if ui.button("Save").clicked() {
                ui.close();
                self.save_project(false);
            }
            if ui.button("Save As…").clicked() {
                ui.close();
                self.save_project(true);
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
        if let Some(document) = self.recovery.show_dialog(ui.ctx(), &self.core) {
            self.status = crate::recovery::restore_status(self.replace_core(document));
        }

        egui::Panel::top("app-top-bar")
            .exact_size(size::TOP_BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(color::SURFACE)
                    .stroke(egui::Stroke::new(1.0, color::BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(
                        theme::margin(space::THREE),
                        theme::margin(space::ONE),
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // The wordmark stays quiet: accent color is reserved for the
                    // playhead, selection, and live agent state.
                    ui.colored_label(
                        color::TEXT_SECONDARY,
                        egui::RichText::new("OPENREEL")
                            .strong()
                            .font(theme::title_font()),
                    );
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
                        ui.colored_label(color::TEXT_MUTED, &self.status);
                    });
                });
            });

        egui::Panel::left("media-bin-panel")
            .default_size(240.0)
            .min_size(208.0)
            .resizable(true)
            .frame(theme::panel_frame())
            .show(ui, |ui| self.media_bin(ui));
        egui::Panel::right("agent-panel")
            .default_size(340.0)
            .min_size(280.0)
            .resizable(true)
            .frame(theme::panel_frame())
            .show(ui, |ui| self.right_dock(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(color::CANVAS)
                    .inner_margin(egui::Margin::same(theme::margin(space::TWO))),
            )
            .show(ui, |ui| {
                self.preview(ui);
                ui.separator();
                self.transport(ui);
                ui.separator();
                self.timeline(ui);
                ui.separator();
                self.transcript_panel(ui);
            });
        self.show_export_dialog(ui.ctx());
        self.show_help(ui.ctx());
        self.show_error_log(ui.ctx());
        self.show_unsaved_confirmation(ui.ctx());
        self.screenshot.update(ui.ctx());
    }
}

fn operation_status(operation: &Operation) -> String {
    match operation {
        Operation::AddAsset { asset } => format!("Imported {}", asset.name),
        Operation::AddTrack { track } => format!("Added {:?} track {}", track.kind, track.id),
        Operation::RemoveTrack { track } => format!("Removed track {track}"),
        Operation::AddClip { asset, .. } => format!("Added asset {asset} to timeline"),
        Operation::AddTitle { title, .. } => format!("Added title {:?}", title.text),
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
        Operation::AddTransition { clip, transition } => {
            format!("Added {} transition to clip {clip}", transition.name)
        }
        Operation::RemoveTransition { clip } => {
            format!("Removed transition from clip {clip}")
        }
        Operation::SetMarkerParam { marker, name, .. } => {
            format!("Set {name} on marker {marker}")
        }
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
