mod recovery;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, AgentSession, AssetId, AuthenticationStatus, ClipId, Command, Core,
    Document, Event, ExportCancellation, ExportProgress, ExportSettings, FrameRounding,
    HarnessInfo, MediaAsset, MediaEngine, MediaError, MediaEvent, Operation, PlaybackState,
    Rational, SessionConfig, TimeCode, Track, TrackId, TrackKind,
    map_frames_with_rounding, map_source_range_to_project,
};
use openreel_media::{FfmpegMediaEngine, GpuContext, timeline_source_at};

const DEFAULT_TRACK_ID: TrackId = TrackId(1);
const TIMELINE_HEIGHT: f32 = 112.0;
const CLIP_HEIGHT: f32 = 56.0;
const EDGE_HANDLE_WIDTH: f32 = 7.0;

enum ChatEntry {
    User(String),
    Text(String),
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, result: String },
    Cost {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    },
}

struct ExportDialog {
    open: bool,
    output: String,
    width: u32,
    height: u32,
    fps_numerator: u32,
    fps_denominator: u32,
}

struct ExportJob {
    cancellation: ExportCancellation,
    progress_rx: crossbeam_channel::Receiver<ExportProgress>,
    result_rx: mpsc::Receiver<(PathBuf, Result<(), MediaError>)>,
    progress: ExportProgress,
}

struct OpenReelApp {
    recovery: recovery::Recovery,
    core: Core,
    core_events: crossbeam_channel::Receiver<Event>,
    media: Arc<FfmpegMediaEngine>,
    frames: crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    media_events: crossbeam_channel::Receiver<MediaEvent>,
    mcp_server: Option<McpServer>,
    agent_info: Option<HarnessInfo>,
    agent_session: Option<Box<dyn AgentSession>>,
    agent_events: Option<crossbeam_channel::Receiver<AgentEvent>>,
    agent_running: bool,
    agent_input: String,
    agent_turn_cap: u32,
    chat: Vec<ChatEntry>,
    probe_tx: mpsc::Sender<(PathBuf, Result<MediaAsset, MediaError>)>,
    probe_rx: mpsc::Receiver<(PathBuf, Result<MediaAsset, MediaError>)>,
    document: Arc<Document>,
    texture: Option<egui::TextureHandle>,
    position: TimeCode,
    playing: bool,
    resume_after_scrub: bool,
    selected_clip: Option<ClipId>,
    pixels_per_frame: f32,
    project_path: Option<PathBuf>,
    status: String,
    export_dialog: ExportDialog,
    export_job: Option<ExportJob>,
}

impl OpenReelApp {
    fn new(media: Arc<FfmpegMediaEngine>) -> Self {
        let document = Document::default();
        let core = Core::spawn(document.clone()).expect("default document must be valid");
        let core_events = core.subscribe().expect("Core actor must accept subscribers");
        let frames = media.frames();
        let media_events = media.events();
        let (probe_tx, probe_rx) = mpsc::channel();
        let agent_media: Arc<dyn MediaEngine> = media.clone();
        let mut chat = Vec::new();
        let mcp_server = match McpServer::start(core.clone(), agent_media) {
            Ok(server) => Some(server),
            Err(error) => {
                chat.push(ChatEntry::Text(format!(
                    "Could not start the OpenReel agent server: {error}"
                )));
                None
            }
        };
        let agent_info = ClaudeCodeDriver.detect();
        let resolution = document.resolution;
        let fps = document.fps;
        let app = Self {
            recovery: recovery::Recovery::start(&core),
            core,
            core_events,
            media,
            frames,
            media_events,
            mcp_server,
            agent_info,
            agent_session: None,
            agent_events: None,
            agent_running: false,
            agent_input: String::new(),
            agent_turn_cap: 8,
            chat,
            probe_tx,
            probe_rx,
            document: Arc::new(document),
            texture: None,
            position: TimeCode::ZERO,
            playing: false,
            resume_after_scrub: false,
            selected_clip: None,
            pixels_per_frame: 6.0,
            project_path: None,
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
            return Self {
                status: "Core actor stopped while creating the default track".to_owned(),
                ..app
            };
        }
        app
    }

    fn choose_media(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi"])
            .pick_file()
        else {
            return;
        };
        self.status = format!("Probing {}…", path.display());
        let media = Arc::clone(&self.media);
        let result_tx = self.probe_tx.clone();
        thread::Builder::new()
            .name("openreel-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((path, result));
            })
            .expect("failed to spawn media probe worker");
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if !save_as {
            self.project_path.clone()
        } else {
            None
        };
        let path = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("OpenReel project", &["openreel"])
                .set_file_name("project.openreel")
                .save_file()
        });
        let Some(mut path) = path else {
            return;
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
                self.recovery.checkpoint(&self.core);
                self.status = format!("Saved {}", path.display());
            }
            Err(error) => self.status = format!("Could not save {}: {error}", path.display()),
        }
    }

    fn open_export_dialog(&mut self) {
        self.export_dialog.width = self.document.resolution.0;
        self.export_dialog.height = self.document.resolution.1;
        self.export_dialog.fps_numerator = self.document.fps.numerator();
        self.export_dialog.fps_denominator = self.document.fps.denominator();
        if let Some(project_path) = &self.project_path {
            self.export_dialog.output = project_path.with_extension("mp4").display().to_string();
        }
        self.export_dialog.open = true;
    }

    fn choose_export_output(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("MPEG-4 video", &["mp4"])
            .set_file_name("export.mp4")
            .save_file()
        else {
            return;
        };
        self.export_dialog.output = path.display().to_string();
    }

    fn start_export(&mut self) {
        if self.export_job.is_some() {
            return;
        }
        if self.document.duration <= TimeCode::ZERO {
            self.status = "Add a clip to the timeline before exporting".to_owned();
            return;
        }
        if self.export_dialog.width % 2 != 0 || self.export_dialog.height % 2 != 0 {
            self.status = "H.264 export width and height must be even".to_owned();
            return;
        }
        let fps = match Rational::new(
            self.export_dialog.fps_numerator,
            self.export_dialog.fps_denominator,
        ) {
            Ok(fps) => fps,
            Err(error) => {
                self.status = format!("Invalid export frame rate: {error}");
                return;
            }
        };
        let mut output = PathBuf::from(self.export_dialog.output.trim());
        if output.as_os_str().is_empty() {
            self.status = "Choose an export output path".to_owned();
            return;
        }
        if output.extension().is_none() {
            output.set_extension("mp4");
            self.export_dialog.output = output.display().to_string();
        }
        let cancellation = ExportCancellation::default();
        let settings = ExportSettings {
            fps,
            resolution: (self.export_dialog.width, self.export_dialog.height),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 8_000_000,
            audio_bitrate: 192_000,
            cancellation: cancellation.clone(),
        };
        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = mpsc::channel();
        let media = Arc::clone(&self.media);
        let worker_output = output.clone();
        let spawn = thread::Builder::new()
            .name("openreel-export".to_owned())
            .spawn(move || {
                let result = media.export(&worker_output, settings, progress_tx);
                let _ = result_tx.send((worker_output, result));
            });
        if let Err(error) = spawn {
            self.status = format!("Could not start export: {error}");
            return;
        }
        self.status = format!("Exporting {}…", output.display());
        self.export_job = Some(ExportJob {
            cancellation,
            progress_rx,
            result_rx,
            progress: ExportProgress {
                completed_frames: 0,
                total_frames: 0,
            },
        });
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        let mut completed = None;
        if let Some(job) = &mut self.export_job {
            while let Ok(progress) = job.progress_rx.try_recv() {
                job.progress = progress;
            }
            match job.result_rx.try_recv() {
                Ok(result) => completed = Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    completed = Some((
                        PathBuf::from(&self.export_dialog.output),
                        Err(MediaError::Backend("export worker stopped".to_owned())),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
            }
        }
        if let Some((path, result)) = completed {
            self.export_job = None;
            self.status = match result {
                Ok(()) => format!("Exported {}", path.display()),
                Err(MediaError::Cancelled) => "Export cancelled".to_owned(),
                Err(error) => format!("Export failed: {error}"),
            };
        }
    }

    fn show_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_dialog.open {
            return;
        }
        let mut open = self.export_dialog.open;
        let mut browse = false;
        let mut start = false;
        let mut cancel = false;
        egui::Window::new("Export MP4")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("export-settings").show(ui, |ui| {
                    ui.label("Output");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.export_dialog.output)
                                .desired_width(320.0),
                        );
                        if ui.button("Browse…").clicked() {
                            browse = true;
                        }
                    });
                    ui.end_row();
                    ui.label("Resolution");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.export_dialog.width).range(2..=16_384));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.export_dialog.height).range(2..=16_384));
                    });
                    ui.end_row();
                    ui.label("FPS");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.fps_numerator)
                                .range(1..=120_000),
                        );
                        ui.label("/");
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.fps_denominator)
                                .range(1..=10_000),
                        );
                    });
                    ui.end_row();
                });
                ui.separator();
                if let Some(job) = &self.export_job {
                    let fraction = if job.progress.total_frames == 0 {
                        0.0
                    } else {
                        job.progress.completed_frames as f32 / job.progress.total_frames as f32
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .show_percentage()
                            .text(format!(
                                "{} / {} frames",
                                job.progress.completed_frames, job.progress.total_frames
                            )),
                    );
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                } else if ui.button("Export").clicked() {
                    start = true;
                }
            });
        self.export_dialog.open = open || self.export_job.is_some();
        if browse {
            self.choose_export_output();
        }
        if start {
            self.start_export();
        }
        if cancel {
            if let Some(job) = &self.export_job {
                job.cancellation.cancel();
                self.status = "Cancelling export…".to_owned();
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
                self.status = format!("Could not open {}: {error}", path.display());
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
            self.status = format!("Could not open {}: {error}", path.display());
            return;
        }
        self.project_path = Some(path.to_path_buf());
        self.status = if missing.is_empty() {
            format!("Opened {}", path.display())
        } else {
            format!(
                "Opened {} — missing media: {}",
                path.display(),
                missing.join(", ")
            )
        };
    }

    fn replace_core(&mut self, document: Document) -> Result<(), String> {
        let core = Core::spawn(document.clone()).map_err(|error| error.to_string())?;
        let events = core.subscribe().map_err(|error| error.to_string())?;
        let agent_media: Arc<dyn MediaEngine> = self.media.clone();
        let mcp_server = McpServer::start(core.clone(), agent_media)
            .map_err(|error| format!("agent server: {error}"))?;
        if let Some(session) = &mut self.agent_session {
            session.interrupt();
        }
        self.media.pause();
        self.recovery.attach(&core);
        self.core = core;
        self.core_events = events;
        self.mcp_server = Some(mcp_server);
        self.agent_session = None;
        self.agent_events = None;
        self.agent_running = false;
        self.document = Arc::new(document);
        self.position = TimeCode::ZERO;
        self.playing = false;
        self.resume_after_scrub = false;
        self.selected_clip = None;
        self.texture = None;
        self.media.set_document(Arc::clone(&self.document));
        self.media.request_frame(TimeCode::ZERO);
        Ok(())
    }

    fn send_operation(&mut self, operation: Operation) {
        if self.core.send(Command::Do(operation)).is_err() {
            self.status = "Core actor stopped while applying the edit".to_owned();
        } else {
            self.status = "Applying edit…".to_owned();
        }
    }

    fn undo(&mut self) {
        if self.core.send(Command::Undo).is_err() {
            self.status = "Core actor stopped while undoing".to_owned();
        } else {
            self.status = "Undo".to_owned();
        }
    }

    fn redo(&mut self) {
        if self.core.send(Command::Redo).is_err() {
            self.status = "Core actor stopped while redoing".to_owned();
        } else {
            self.status = "Redo".to_owned();
        }
    }

    fn add_asset_to_timeline(&mut self, asset_id: AssetId) {
        let Some(track) = self
            .document
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Video)
        else {
            self.status = "No video track exists".to_owned();
            return;
        };
        let Some(asset) = self.document.asset(asset_id) else {
            self.status = format!("Asset {asset_id} no longer exists");
            return;
        };
        self.send_operation(Operation::AddClip {
            track: track.id,
            asset: asset.id,
            at: self.document.duration,
            source: TimeCode::ZERO..asset.duration,
        });
    }

    fn split_at_playhead(&mut self) {
        let clip = self.selected_clip.or_else(|| {
            timeline_source_at(&self.document, self.position)
                .ok()
                .flatten()
                .map(|source| source.clip)
        });
        let Some(clip) = clip else {
            self.status = "No clip is selected or active at the playhead".to_owned();
            return;
        };
        self.send_operation(Operation::SplitClip {
            clip,
            at: self.position,
        });
    }

    fn delete_selected(&mut self) {
        let Some(clip) = self.selected_clip else {
            self.status = "Select a clip to delete".to_owned();
            return;
        };
        self.send_operation(Operation::DeleteClip { clip });
    }

    fn start_agent_turn(&mut self) {
        let message = self.agent_input.trim().to_owned();
        if message.is_empty() || self.agent_running {
            return;
        }
        let Some(endpoint) = self
            .mcp_server
            .as_ref()
            .map(|server| server.endpoint().to_owned())
        else {
            self.status = "The OpenReel agent server is unavailable".to_owned();
            return;
        };
        if self.agent_info.is_none() {
            self.status = "Claude Code is not installed on PATH".to_owned();
            return;
        }

        if self.agent_session.is_none() {
            let working_directory = self
                .project_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok());
            let config = SessionConfig {
                working_directory,
                model: None,
                max_turns: Some(self.agent_turn_cap),
                mcp_url: Some(endpoint),
            };
            match ClaudeCodeDriver.start_session(config) {
                Ok(session) => {
                    self.agent_events = Some(session.events());
                    self.agent_session = Some(session);
                }
                Err(error) => {
                    self.status = format!("Could not start Claude Code: {error}");
                    return;
                }
            }
        }

        let result = self
            .agent_session
            .as_mut()
            .expect("agent session was initialized")
            .send_user_message(message.clone());
        match result {
            Ok(()) => {
                self.chat.push(ChatEntry::User(message));
                self.agent_input.clear();
                self.agent_running = true;
                self.status = "Claude Code is editing the timeline".to_owned();
            }
            Err(error) => self.status = format!("Could not send agent message: {error}"),
        }
    }

    fn stop_agent(&mut self) {
        if let Some(session) = &mut self.agent_session {
            session.interrupt();
        }
        self.agent_session = None;
        self.agent_events = None;
        self.agent_running = false;
        self.status = "Agent stopped".to_owned();
    }

    fn poll_agent(&mut self, ctx: &egui::Context) {
        let events = self
            .agent_events
            .as_ref()
            .map(|receiver| receiver.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                AgentEvent::Text(text) => self.chat.push(ChatEntry::Text(text)),
                AgentEvent::ToolCall { name, arguments } => {
                    self.chat.push(ChatEntry::ToolCall { name, arguments });
                }
                AgentEvent::ToolResult { name, result } => {
                    self.chat.push(ChatEntry::ToolResult { name, result });
                }
                AgentEvent::Cost {
                    input_tokens,
                    output_tokens,
                    cost_usd,
                } => self.chat.push(ChatEntry::Cost {
                    input_tokens,
                    output_tokens,
                    cost_usd,
                }),
                AgentEvent::Done => {
                    self.agent_running = false;
                    self.status = "Agent turn finished".to_owned();
                }
            }
        }
        if self.agent_running {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Agent");
        match &self.agent_info {
            Some(info) => {
                let authentication = match info.authentication {
                    AuthenticationStatus::Authenticated => "authenticated",
                    AuthenticationStatus::Unauthenticated => "not authenticated",
                    AuthenticationStatus::Unknown => "authentication unknown",
                };
                ui.label(format!(
                    "Claude Code {} ({authentication})",
                    info.version.as_deref().unwrap_or("version unknown")
                ));
            }
            None => {
                ui.colored_label(egui::Color32::LIGHT_RED, "Claude Code not found on PATH");
            }
        }
        ui.horizontal(|ui| {
            ui.label("Turn cap");
            ui.add_enabled(
                !self.agent_running && self.agent_session.is_none(),
                egui::DragValue::new(&mut self.agent_turn_cap).range(1..=20),
            );
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (index, entry) in self.chat.iter().enumerate() {
                    match entry {
                        ChatEntry::User(text) => {
                            ui.strong("You");
                            ui.label(text);
                        }
                        ChatEntry::Text(text) => {
                            ui.strong("Claude");
                            ui.label(text);
                        }
                        ChatEntry::ToolCall { name, arguments } => {
                            ui.label(format!("Tool: {name}"));
                            ui.small(summarize(arguments, 180));
                        }
                        ChatEntry::ToolResult { name, result } => {
                            egui::CollapsingHeader::new(format!("Result: {name}"))
                                .id_salt(("agent-result", index))
                                .show(ui, |ui| {
                                    ui.small(summarize(result, 500));
                                });
                        }
                        ChatEntry::Cost {
                            input_tokens,
                            output_tokens,
                            cost_usd,
                        } => {
                            let cost = cost_usd
                                .map(|cost| format!(", ${cost:.4}"))
                                .unwrap_or_default();
                            ui.small(format!(
                                "{input_tokens} input / {output_tokens} output tokens{cost}"
                            ));
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        ui.separator();
        ui.add_enabled(
            !self.agent_running,
            egui::TextEdit::multiline(&mut self.agent_input)
                .desired_rows(3)
                .hint_text("Describe an edit…"),
        );
        ui.horizontal(|ui| {
            let can_send = !self.agent_running
                && !self.agent_input.trim().is_empty()
                && self.agent_info.is_some()
                && self.mcp_server.is_some();
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked()
            {
                self.start_agent_turn();
            }
            if ui
                .add_enabled(self.agent_running, egui::Button::new("Stop"))
                .clicked()
            {
                self.stop_agent();
            }
        });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        self.poll_agent(ctx);
        self.poll_export(ctx);
        while let Ok((path, result)) = self.probe_rx.try_recv() {
            match result {
                Ok(asset) => {
                    self.status = format!("Importing {}…", path.display());
                    self.send_operation(Operation::AddAsset { asset });
                }
                Err(error) => self.status = format!("Could not import {}: {error}", path.display()),
            }
        }

        while let Ok(event) = self.core_events.try_recv() {
            match event {
                Event::DocumentChanged { doc, last_op, .. } => {
                    self.document = Arc::clone(&doc);
                    if self
                        .selected_clip
                        .is_some_and(|clip| doc.clip(clip).is_none())
                    {
                        self.selected_clip = None;
                    }
                    if doc.duration <= TimeCode::ZERO {
                        self.position = TimeCode::ZERO;
                    } else {
                        self.position = TimeCode(
                            self.position.0.clamp(0, doc.duration.0.saturating_sub(1)),
                        );
                    }
                    self.playing = false;
                    self.media.set_document(Arc::clone(&doc));
                    self.media.seek(self.position);
                    self.media.request_frame(self.position);
                    if let Some(operation) = last_op {
                        self.status = operation_status(&operation);
                    }
                }
                Event::OpRejected { error, .. } => {
                    self.status = format!("Edit rejected: {error}");
                }
                Event::QueryResult(_) => {}
            }
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
                    self.status = format!("Playback error: {error}");
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
                    "openreel-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            if !self.resume_after_scrub {
                self.position = at;
            }
        }

        if self.playing {
            self.position = self.media.position();
            ctx.request_repaint_after(Duration::from_millis(10));
        }
    }

    fn toggle_playback(&mut self) {
        if self.document.duration <= TimeCode::ZERO {
            self.status = "Add a clip to the timeline before playing".to_owned();
            return;
        }
        if self.playing {
            self.media.pause();
        } else {
            if self.position >= self.document.duration {
                self.position = TimeCode::ZERO;
            }
            self.media.play(self.position);
        }
    }

    fn seek_to(&mut self, position: TimeCode) {
        let maximum = self.document.duration.0.saturating_sub(1).max(0);
        self.position = TimeCode(position.0.clamp(0, maximum));
        self.media.seek(self.position);
        self.media.request_frame(self.position);
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
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

    fn media_bin(&mut self, ui: &mut egui::Ui) {
        ui.heading("Media bin");
        if ui.button("Import media…").clicked() {
            self.choose_media();
        }
        ui.separator();
        if self.document.media_pool.is_empty() {
            ui.label("No imported assets");
            return;
        }
        let assets = self.document.media_pool.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for asset in assets {
                ui.group(|ui| {
                    ui.strong(&asset.name);
                    ui.small(format!(
                        "{} frames · {}/{} fps",
                        asset.duration.0,
                        asset.fps.numerator(),
                        asset.fps.denominator()
                    ));
                    if ui.button("Add to timeline").clicked() {
                        self.add_asset_to_timeline(asset.id);
                    }
                });
            }
        });
    }

    fn preview(&self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let preview_height = (available.y - 220.0).max(120.0);
        if let Some(texture) = &self.texture {
            let source = texture.size_vec2();
            let scale = (available.x / source.x)
                .min(preview_height / source.y)
                .min(1.0);
            let size = source * scale;
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new((texture.id(), source))
                        .fit_to_exact_size(size)
                        .maintain_aspect_ratio(true),
                );
            });
        } else {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(available.x, preview_height),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(rect, 4.0, egui::Color32::BLACK);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No timeline frame",
                egui::FontId::proportional(18.0),
                egui::Color32::GRAY,
            );
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(if self.playing { "⏸ Pause" } else { "▶ Play" })
                .clicked()
            {
                self.toggle_playback();
            }
            let maximum = self.document.duration.0.saturating_sub(1).max(0);
            let mut slider_position = self.position.0.clamp(0, maximum);
            let response = ui.add_enabled(
                maximum > 0,
                egui::Slider::new(&mut slider_position, 0..=maximum)
                    .show_value(false)
                    .text("Position"),
            );
            if response.drag_started() {
                self.resume_after_scrub = self.playing;
                if self.playing {
                    self.media.pause();
                }
            }
            if response.changed() {
                self.position = TimeCode(slider_position);
                self.media.request_frame(self.position);
            }
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                self.media.seek(self.position);
                if self.resume_after_scrub {
                    self.media.play(self.position);
                }
                self.resume_after_scrub = false;
            }
            ui.monospace(format!("{} / {}", self.position.0, maximum));
        });
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Timeline");
            if ui.button("Split (S)").clicked() {
                self.split_at_playhead();
            }
            if ui.button("Delete").clicked() {
                self.delete_selected();
            }
            if ui.button("Undo").clicked() {
                self.undo();
            }
            if ui.button("Redo").clicked() {
                self.redo();
            }
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.pixels_per_frame, 1.0..=20.0));
        });

        let document = Arc::clone(&self.document);
        let Some(track) = document
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Video)
        else {
            ui.label("No video track");
            return;
        };
        let content_frames = document
            .duration
            .0
            .max(self.position.0.saturating_add(1))
            .max(60);
        let width = ((content_frames as f32) * self.pixels_per_frame + 180.0)
            .max(ui.available_width());
        let mut pending_operation = None;
        let mut seek = None;

        egui::ScrollArea::horizontal()
            .id_salt("timeline-scroll")
            .show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(width, TIMELINE_HEIGHT),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, egui::Color32::from_gray(26));
                let strip = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + 30.0),
                    egui::pos2(rect.right(), rect.top() + 30.0 + CLIP_HEIGHT),
                );
                painter.rect_filled(strip, 2.0, egui::Color32::from_gray(42));

                let tick = i64::from(
                    (document.fps.numerator() / document.fps.denominator()).max(1),
                );
                let mut frame = 0_i64;
                while frame <= content_frames {
                    let x = rect.left() + (frame as f32) * self.pixels_per_frame;
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.top() + 12.0)],
                        egui::Stroke::new(1.0, egui::Color32::DARK_GRAY),
                    );
                    painter.text(
                        egui::pos2(x + 3.0, rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        frame,
                        egui::FontId::monospace(10.0),
                        egui::Color32::LIGHT_GRAY,
                    );
                    frame = frame.saturating_add(tick);
                }

                for clip in &track.clips {
                    let Some(asset) = document.asset(clip.asset) else {
                        continue;
                    };
                    let Ok(duration) = map_source_range_to_project(
                        clip.source_range.clone(),
                        asset.fps,
                        document.fps,
                    ) else {
                        continue;
                    };
                    let x = rect.left()
                        + (clip.timeline_start.0 as f32) * self.pixels_per_frame;
                    let clip_width = ((duration.0 as f32) * self.pixels_per_frame).max(30.0);
                    let clip_rect = egui::Rect::from_min_size(
                        egui::pos2(x, strip.top()),
                        egui::vec2(clip_width, CLIP_HEIGHT),
                    );
                    let body_rect = egui::Rect::from_min_max(
                        egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
                        egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.bottom()),
                    );
                    let body = ui
                        .interact(
                            body_rect,
                            ui.make_persistent_id(("clip-body", clip.id.0)),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_text("Drag to move; click to select");
                    let left_rect = egui::Rect::from_min_max(
                        clip_rect.min,
                        egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.bottom()),
                    );
                    let right_rect = egui::Rect::from_min_max(
                        egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.top()),
                        clip_rect.max,
                    );
                    let left = ui
                        .interact(
                            left_rect,
                            ui.make_persistent_id(("clip-left", clip.id.0)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    let right = ui
                        .interact(
                            right_rect,
                            ui.make_persistent_id(("clip-right", clip.id.0)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);

                    if body.clicked() {
                        self.selected_clip = Some(clip.id);
                    }
                    if body.drag_stopped() {
                        let delta = (body.drag_delta().x / self.pixels_per_frame).round() as i64;
                        if delta != 0 {
                            pending_operation = Some(Operation::MoveClip {
                                clip: clip.id,
                                to_track: track.id,
                                to: TimeCode(clip.timeline_start.0.saturating_add(delta).max(0)),
                            });
                        }
                    }
                    if left.drag_stopped() {
                        let project_delta =
                            (left.drag_delta().x / self.pixels_per_frame).round() as i64;
                        let source_delta = project_delta_to_source(
                            project_delta,
                            document.fps,
                            asset.fps,
                        );
                        let new_start = TimeCode(
                            clip.source_range
                                .start
                                .0
                                .saturating_add(source_delta)
                                .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                        );
                        if new_start != clip.source_range.start {
                            pending_operation = Some(Operation::TrimClip {
                                clip: clip.id,
                                new_source: new_start..clip.source_range.end,
                            });
                        }
                    }
                    if right.drag_stopped() {
                        let project_delta =
                            (right.drag_delta().x / self.pixels_per_frame).round() as i64;
                        let source_delta = project_delta_to_source(
                            project_delta,
                            document.fps,
                            asset.fps,
                        );
                        let new_end = TimeCode(
                            clip.source_range
                                .end
                                .0
                                .saturating_add(source_delta)
                                .clamp(clip.source_range.start.0.saturating_add(1), asset.duration.0),
                        );
                        if new_end != clip.source_range.end {
                            pending_operation = Some(Operation::TrimClip {
                                clip: clip.id,
                                new_source: clip.source_range.start..new_end,
                            });
                        }
                    }

                    let drag = if body.dragged() {
                        body.drag_delta()
                    } else {
                        egui::Vec2::ZERO
                    };
                    let draw_rect = clip_rect.translate(drag);
                    let selected = self.selected_clip == Some(clip.id);
                    let color = clip_color(clip.asset, selected);
                    painter.rect_filled(draw_rect, 4.0, color);
                    painter.rect_stroke(
                        draw_rect,
                        4.0,
                        egui::Stroke::new(
                            if selected { 2.0 } else { 1.0 },
                            if selected {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(150)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            draw_rect.min,
                            egui::pos2(draw_rect.left() + EDGE_HANDLE_WIDTH, draw_rect.bottom()),
                        ),
                        2.0,
                        egui::Color32::from_white_alpha(70),
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(draw_rect.right() - EDGE_HANDLE_WIDTH, draw_rect.top()),
                            draw_rect.max,
                        ),
                        2.0,
                        egui::Color32::from_white_alpha(70),
                    );
                    painter.text(
                        draw_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &asset.name,
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                }

                let playhead_x =
                    rect.left() + (self.position.0 as f32) * self.pixels_per_frame;
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top()),
                        egui::pos2(playhead_x, rect.bottom()),
                    ],
                    egui::Stroke::new(2.0, egui::Color32::RED),
                );

                if response.clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let frame = ((pointer.x - rect.left()) / self.pixels_per_frame)
                            .round() as i64;
                        seek = Some(TimeCode(frame));
                    }
                }
            });

        if let Some(operation) = pending_operation {
            self.send_operation(operation);
        }
        if let Some(position) = seek {
            self.seek_to(position);
        }
    }

    fn keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, undo, redo, split, delete) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.modifiers.ctrl && input.key_pressed(egui::Key::Z),
                input.modifiers.ctrl && input.key_pressed(egui::Key::Y),
                !input.modifiers.ctrl && input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::Delete),
            )
        });
        if undo {
            self.undo();
        } else if redo {
            self.redo();
        } else if split {
            self.split_at_playhead();
        } else if delete {
            self.delete_selected();
        } else if space {
            self.toggle_playback();
        }
    }
}

impl eframe::App for OpenReelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_background(ui.ctx());
        self.keyboard_shortcuts(ui.ctx());

        ui.horizontal(|ui| {
            self.file_menu(ui);
            ui.separator();
            ui.label(&self.status);
        });
        ui.separator();

        egui::Panel::left("media-bin-panel")
            .default_size(240.0)
            .resizable(true)
            .show(ui, |ui| self.media_bin(ui));
        egui::Panel::right("agent-panel")
            .default_size(340.0)
            .resizable(true)
            .show(ui, |ui| self.agent_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            self.preview(ui);
            ui.separator();
            self.transport(ui);
            ui.separator();
            self.timeline(ui);
        });
        self.show_export_dialog(ui.ctx());
        if let Some(document) = self.recovery.show_dialog(ui.ctx(), &self.core) {
            self.status = recovery::restore_status(self.replace_core(document));
        }
    }
}

fn summarize(value: &str, maximum_chars: usize) -> String {
    let mut summary = value.chars().take(maximum_chars).collect::<String>();
    if value.chars().count() > maximum_chars {
        summary.push('…');
    }
    summary
}

fn operation_status(operation: &Operation) -> String {
    match operation {
        Operation::AddAsset { asset } => format!("Imported {}", asset.name),
        Operation::AddTrack { track } => format!("Added {:?} track {}", track.kind, track.id),
        Operation::RemoveTrack { track } => format!("Removed track {track}"),
        Operation::AddClip { asset, .. } => format!("Added asset {asset} to timeline"),
        Operation::SplitClip { clip, at } => format!("Split clip {clip} at frame {at}"),
        Operation::TrimClip { clip, .. } => format!("Trimmed clip {clip}"),
        Operation::MoveClip { clip, to, .. } => format!("Moved clip {clip} to frame {to}"),
        Operation::DeleteClip { clip } => format!("Deleted clip {clip}"),
        Operation::AddEffect { clip, effect } => {
            format!("Added {} effect {} to clip {clip}", effect.name, effect.id)
        }
        Operation::RemoveEffect { clip, effect } => {
            format!("Removed effect {effect} from clip {clip}")
        }
        Operation::SetEffectParam {
            clip,
            effect,
            name,
            ..
        } => format!("Set {name} on effect {effect} for clip {clip}"),
        Operation::AddTransition { clip, transition } => {
            format!("Added {} transition to clip {clip}", transition.name)
        }
        Operation::RemoveTransition { clip } => {
            format!("Removed transition from clip {clip}")
        }
    }
}

fn project_delta_to_source(
    project_delta: i64,
    project_fps: openreel_core::Rational,
    source_fps: openreel_core::Rational,
) -> i64 {
    let sign = project_delta.signum();
    let magnitude = TimeCode(project_delta.saturating_abs());
    map_frames_with_rounding(magnitude, project_fps, source_fps, FrameRounding::Nearest)
        .map_or(0, |frames| frames.0.saturating_mul(sign))
}

fn clip_color(asset: AssetId, selected: bool) -> egui::Color32 {
    if selected {
        return egui::Color32::from_rgb(55, 125, 210);
    }
    let seed = u8::try_from(asset.0 % 80).unwrap_or_default();
    egui::Color32::from_rgb(
        45_u8.saturating_add(seed),
        80_u8.saturating_add(seed / 2),
        135_u8.saturating_add(seed / 3),
    )
}

fn main() -> eframe::Result {
    eframe::run_native(
        "OpenReel",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        },
        Box::new(move |creation_context| {
            let render_state = creation_context
                .wgpu_render_state
                .as_ref()
                .expect("the OpenReel app requires eframe's wgpu renderer");
            let gpu = GpuContext::new(render_state.device.clone(), render_state.queue.clone());
            let media = Arc::new(
                FfmpegMediaEngine::new_with_gpu(gpu)
                    .expect("FFmpeg media engine must initialize"),
            );
            Ok(Box::new(OpenReelApp::new(media)))
        }),
    )
}
