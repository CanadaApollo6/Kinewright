use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use eframe::egui;
use kinewright_agent::{ClaudeCodeDriver, CodexDriver, CursorAcpDriver};
use kinewright_core::{
    AgentDriver, Analysis, AudioChain, Command, Document, Effect, EffectId, Event, Export,
    HarnessInfo, JournalCommand, LiveAudioChange, MediaAsset, MediaError, MediaEvent,
    MixNoiseProfileRequest, MixSpectrumPoint, NOISE_PROFILE_BAND_COUNT,
    NOISE_PROFILE_PARAMETER_NAMES, Operation, ParamValue, Playback, PlaybackState, Rational,
    SilenceStatus, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::{FfmpegMediaEngine, GpuContext, compositor_required_limits};

use crate::{
    error_ui::ErrorLog,
    export_ui::{ExportDialog, ExportJob},
    icons::Icon,
    media_workflow::media_asset_requires_refresh,
    mixer_pane_ui::{LEARN_NO_SILENCE, NoiseLearnRange},
    project::{
        ProjectSaveError, ProjectSaveReport, ProjectSession, derive_lut_store,
        focus_publishes_lut_library, index_after_close, project_name, session_index_by_id,
        write_project_document,
    },
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
    /// AU1 §5.1: the manual mixer — per-track faders, meters, and M/S.
    Mixer,
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
    /// The engine as itself, so the verified LUT library can be published
    /// (CC4 §2.4).
    ///
    /// `set_lut_library` is an inherent method on the concrete engine rather
    /// than a `Playback` trait method, because `LutLibrary` holds verified
    /// media-crate sample data that Core — which is I/O-free — cannot name.
    /// Publishing once covers preview, proofs, and export: the engine both
    /// sends the library to the playback worker and stores it where the proof
    /// and export entry points read it.
    pub(crate) lut_publisher: Arc<FfmpegMediaEngine>,
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
    pub(crate) lut_import_tx: mpsc::Sender<crate::media_workflow::LutImportResponse>,
    pub(crate) lut_import_rx: mpsc::Receiver<crate::media_workflow::LutImportResponse>,
    pub(crate) lut_restore_tx: mpsc::Sender<crate::media_workflow::LutRestoreResponse>,
    pub(crate) lut_restore_rx: mpsc::Receiver<crate::media_workflow::LutRestoreResponse>,
    /// Outstanding look import/restore workers, so the frame loop keeps
    /// repainting until every typed result has landed.
    pub(crate) lut_worker_pending: usize,
    /// The LUT asset id the last submitted import batch allocated, held until
    /// the document records it so a second import finishing in the same frame
    /// cannot be built against the same pre-import document (CC4 §7).
    pub(crate) lut_import_reservation: Option<crate::media_workflow::LutImportReservation>,
    pub(crate) look_browser: crate::look_browser_ui::LookBrowserState,
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
    /// CC6 §8.1 Colour QC: the read-only measurement window and its single
    /// worker. Nothing in it can reach the document.
    pub(crate) color_qc: crate::color_qc_ui::ColorQcState,
    /// AU5 §6.3 rule 127: the single-flight worker behind `Learn profile`.
    pub(crate) noise_learn: NoiseLearnState,
    /// AU5 §6.4 rule 129: finished room-tone captures on their way to one
    /// `DoBatch`. Decode never runs on the UI thread (DESIGN.md).
    pub(crate) room_tone_tx: mpsc::Sender<crate::timeline_ui::RoomToneCaptureResponse>,
    pub(crate) room_tone_rx: mpsc::Receiver<crate::timeline_ui::RoomToneCaptureResponse>,
    /// How many captures are still decoding, which is what keeps the frame
    /// clock running while one is.
    pub(crate) room_tone_pending: usize,
    /// CC6 §8.2 QC clipping mask: the program viewer's whole-picture
    /// replacement view and the working-proof worker behind it.
    pub(crate) qc_mask: crate::preview_ui::QcMaskState,
    /// The one full-resolution working proof CC6 §8.1 and §8.2 share.
    ///
    /// Both surfaces measure the same `working_linear_post_composite` raster at
    /// the playhead, and each worker owns its own renderer; keyed by
    /// `(session, revision, frame)`, one render answers both.
    pub(crate) working_proof_cache: std::sync::Arc<crate::color_qc_ui::WorkingProofCache>,
    /// CC5 §6 matte overlay: the expanded section's identity, the selected
    /// window, the live drag, and the matte-view coverage worker.
    pub(crate) matte_overlay: crate::matte_overlay_ui::MatteOverlayState,
    pub(crate) playing: bool,
    pub(crate) meter_levels: [f32; 2],
    /// AU1 §5.1: the mixer's displayed meter levels, one entry per mix point,
    /// decaying on the transport meter's schedule between peaks.
    pub(crate) mixer_levels: crate::mixer_ui::MixerMeterLevels,
    /// AU2 §6.6: the chain the mixer's pane is editing, if any. UI state, not
    /// document state: it is cleared when the bus it names leaves the focused
    /// document and never travels with a project.
    pub(crate) mixer_selection: Option<crate::mixer_ui::MixerSelection>,
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
    /// The live press-and-hold A/B comparison, mirrored out of the look card
    /// so a card that stops rendering mid-hold cannot leave `bypass = 1` in
    /// the document (CC4 §7).
    pub(crate) look_ab_hold: Option<crate::inspector_ui::MirroredAbHold>,
    /// Whether the held card reported itself during the previous frame.
    pub(crate) look_ab_hold_seen: bool,
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
        let (room_tone_tx, room_tone_rx) = mpsc::channel();
        let (relink_probe_tx, relink_probe_rx) = mpsc::channel();
        let (lut_import_tx, lut_import_rx) = mpsc::channel();
        let (lut_restore_tx, lut_restore_rx) = mpsc::channel();
        let (media_status_tx, media_status_rx) = mpsc::channel();
        let (cache_clear_tx, cache_clear_rx) = mpsc::channel();
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media.clone();
        let lut_publisher = media;
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
            lut_publisher,
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
            lut_import_tx,
            lut_import_rx,
            lut_restore_tx,
            lut_restore_rx,
            lut_worker_pending: 0,
            lut_import_reservation: None,
            look_browser: crate::look_browser_ui::LookBrowserState::default(),
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
            color_qc: crate::color_qc_ui::ColorQcState::default(),
            noise_learn: NoiseLearnState::default(),
            room_tone_tx,
            room_tone_rx,
            room_tone_pending: 0,
            qc_mask: crate::preview_ui::QcMaskState::default(),
            working_proof_cache: std::sync::Arc::default(),
            matte_overlay: crate::matte_overlay_ui::MatteOverlayState::default(),
            playing: false,
            meter_levels: [0.0; 2],
            mixer_levels: crate::mixer_ui::MixerMeterLevels::default(),
            mixer_selection: screenshot_mixer_selection(
                std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").ok().as_deref(),
                &document,
            ),
            resume_after_scrub: false,
            transcript_scope: TranscriptScope::default(),
            material_tab: match std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref() {
                Ok("transcript") => MaterialTab::Transcript,
                Ok("mixer" | "mixer-chain") => MaterialTab::Mixer,
                _ => MaterialTab::default(),
            },
            show_material_strip: matches!(
                std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref(),
                Ok("timeline" | "transcript" | "mixer" | "mixer-chain")
            ),
            show_media_rail: false,
            pending_project_action: None,
            exit_discarded_projects: Vec::new(),
            allow_close: false,
            last_window_title: String::new(),
            status: "Ready".to_owned(),
            export_dialog: ExportDialog {
                open: screenshot_export_dialog_open(
                    std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").ok().as_deref(),
                ),
                output: "export.mp4".to_owned(),
                width: resolution.0,
                height: resolution.1,
                fps_numerator: fps.numerator(),
                fps_denominator: fps.denominator(),
                delivery_aspect: None,
                focus_x_percent: 50,
                focus_y_percent: 50,
                conformance_cache: None,
                delivery_bit_depth: kinewright_core::DeliveryEncodeDepth::default(),
                normalize_loudness: false,
                verification: None,
                audio_verification: None,
                audio_report: None,
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
            look_ab_hold: None,
            look_ab_hold_seen: false,
        };
        app.publish_focused_lut_library();
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

    /// Retire the held import id for one session after a rejection.
    fn release_lut_import_reservation(&mut self, project_index: usize) {
        let session_id = self.projects[project_index].id;
        if self
            .lut_import_reservation
            .is_some_and(|reservation| reservation.session_id == session_id)
        {
            self.lut_import_reservation = None;
        }
    }

    /// Hand the focused project's verified library to the engine (CC4 §2.4).
    ///
    /// Every path that makes a different project — or a different set of
    /// bytes — the one the compositor renders goes through here, so there is
    /// one publication rule rather than one per call site.
    pub(crate) fn publish_focused_lut_library(&self) {
        self.lut_publisher
            .set_lut_library(Arc::clone(&self.focused().lut_library));
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
        if !focus_publishes_lut_library(
            index,
            self.projects.len(),
            self.focused_project,
            force_rebind,
        ) {
            return;
        }
        self.playback.pause();
        self.playing = false;
        self.resume_after_scrub = false;
        self.meter_levels = [0.0; 2];
        self.mixer_levels = crate::mixer_ui::MixerMeterLevels::default();
        self.mixer_selection = None;
        self.texture = None;
        self.focused_project = index;
        let document = Arc::clone(&self.focused().document);
        let position = self.focused().position;
        self.publish_focused_lut_library();
        self.playback.set_document(document);
        self.playback.seek(position);
        self.playback.request_frame(position);
    }

    /// Write the focused project to an explicit path with no dialog (CC4 §2.2,
    /// §10.3.11).
    ///
    /// Serializes, writes, derives the new store root, copies every referenced
    /// store file across when the root moved, collects one
    /// `lut_store_copy_failed` entry per asset it could not place, and only
    /// then adopts the new identity and checkpoints recovery. Rebuilding and
    /// republishing the library is part of the write because a moved store
    /// root changes where every imported look resolves from.
    ///
    /// # Errors
    ///
    /// Returns the typed serialize or write failure. A store-root refusal is
    /// not fatal: the project is saved and its imported looks report `missing`
    /// until the root is usable.
    pub(crate) fn write_project(
        &mut self,
        path: &Path,
    ) -> Result<ProjectSaveReport, ProjectSaveError> {
        let document = Arc::clone(&self.focused().document);
        let previous_store = self.focused().lut_store.clone();
        let report = write_project_document(&document, path, previous_store.as_ref())?;
        let name = project_name(Some(path), &self.focused().name);
        let (store, store_error) = match derive_lut_store(Some(path)) {
            Ok(store) => (store, None),
            Err(reason) => (None, Some(reason)),
        };
        let session = self.focused_mut();
        session.name = name;
        session.project_path = Some(path.to_path_buf());
        session.set_lut_store(store, store_error);
        session.saved_document = Some(Arc::clone(&session.document));
        let library = session.rebuild_lut_library();
        session.publish_project_path_to_agents();
        let core = session.core.clone();
        session
            .recovery
            .checkpoint(&core, session.project_path.as_deref());
        self.lut_publisher.set_lut_library(library);
        if let Some(reason) = &report.lut_store_error {
            self.record_error(
                "Look",
                format!(
                    "Saved {}, but its LUT store root is unusable: {reason}",
                    path.display()
                ),
            );
        }
        Ok(report)
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
        match self.write_project(&path) {
            Ok(report) => {
                self.status = format!("Saved {}", report.path.display());
                if let Some(summary) = report.copy_failure_summary() {
                    self.record_error(
                        "Look",
                        format!(
                            "Saved {} but could not copy every look into the new store — {summary}",
                            report.path.display()
                        ),
                    );
                }
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
        let store_refusal = session.lut_store_error.clone();
        let unavailable =
            crate::project::unavailable_lut_assets(&session.document, &session.lut_availability);
        let lut_titles: Vec<String> = unavailable
            .iter()
            .filter_map(|id| session.document.lut_asset(*id))
            .map(|asset| format!("{} ({})", asset.title, asset.id))
            .collect();
        self.projects.push(session);
        self.focus_project(self.projects.len() - 1);
        self.publish_focused_lut_library();
        if let Some(reason) = store_refusal {
            self.record_error(
                "Look",
                format!(
                    "The LUT store beside {} is unusable: {reason}",
                    path.display()
                ),
            );
        }
        if !lut_titles.is_empty() {
            self.error_log.push(
                "Look",
                format!(
                    "LUT assets need restore or replacement after open: {}",
                    lut_titles.join(", ")
                ),
            );
        }
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
        let session = self.focused();
        let (session_id, revision, position) = (session.id, session.revision.0, session.position);
        self.color_qc
            .observe_context(session_id, revision, position);
        self.working_proof_cache
            .retain_context(crate::color_qc_ui::WorkingProofKey {
                session_id,
                revision,
                frame: position,
            });
        self.color_qc.poll();
        if self.color_qc.is_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.poll_noise_learn(ctx);
        self.poll_room_tone(ctx);
        self.poll_recording(ctx);
        self.poll_media_workflow(ctx);
        self.recover_stranded_ab_hold(ctx);
        if self.poll_lut_workers() {
            ctx.request_repaint();
        }
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
            || self.lut_worker_pending > 0
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
                    let live_audio =
                        live_audio_change(journal_command.as_ref(), &previous_document, &doc);
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
                    if previous_document.lut_assets != doc.lut_assets {
                        let library = self.projects[project_index].rebuild_lut_library();
                        self.lut_publisher.set_lut_library(library);
                    }
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
                        if !media_changed_assets.is_empty() {
                            self.texture = None;
                        }
                        let position = self.projects[project_index].position;
                        if !apply_live_audio_change(
                            self.playback.as_ref(),
                            live_audio,
                            &doc,
                            position,
                        ) {
                            self.playing = false;
                        }
                    }
                    if let Some(Operation::AddAsset { asset }) = &last_op {
                        self.projects[project_index].cue_source_asset(asset.id);
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
                    self.release_lut_import_reservation(project_index);
                }
                Event::BatchRejected { error, .. } => {
                    let name = &self.projects[project_index].name;
                    self.record_error(
                        "Operations",
                        format!("Edit plan rejected in {name}: {error}"),
                    );
                    self.release_lut_import_reservation(project_index);
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

    /// The `View` menu: the read-only inspection surfaces.
    ///
    /// CC6 §8.1 adds the Colour QC window here rather than to `File`: nothing
    /// in it writes anything, so it belongs with the things you look through,
    /// not with the things you save.
    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("View", |ui| {
            let binding = crate::keys::KEYMAP
                .iter()
                .find(|binding| binding.action == crate::keys::KeyAction::ColorQc);
            if ui
                .button("Colour QC…")
                .on_hover_text(format!(
                    "{}{}",
                    crate::color_qc_ui::COLOR_QC_BANNER,
                    binding.map_or_else(String::new, |binding| format!(" ({})", binding.shortcut))
                ))
                .clicked()
            {
                ui.close();
                self.color_qc.open = true;
            }
        });
    }

    /// The CC4 §7 `Look` menu: import a `.cube` and browse the catalogue.
    ///
    /// Both entries need a saved project, because an imported look is bytes
    /// the project owns and an unsaved project has nowhere to own them
    /// (CC4 §2.2). A look applies to a clip, so both entries also need a
    /// selected media clip; without one, `Import LUT…` still registers the
    /// asset so the operator can bind it afterwards.
    fn look_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Look", |ui| {
            let unavailable = self.focused().lut_store_unavailable_reason();
            let selected_clip = self.selected_media_clip();
            let import = ui
                .add_enabled(unavailable.is_none(), egui::Button::new("Import LUT…"))
                .on_hover_text(unavailable.clone().unwrap_or_else(|| {
                    "Import a .cube into this project's store and apply it to the selected clip"
                        .to_owned()
                }));
            if import.clicked() {
                ui.close();
                if let Some(path) = crate::media_workflow::choose_lut_file() {
                    self.start_lut_import(
                        path,
                        crate::media_workflow::LutImportIntent::Apply {
                            clip: selected_clip,
                            stage: kinewright_core::ColorStage::Look,
                        },
                    );
                }
                return;
            }
            let browse = ui
                .add_enabled(selected_clip.is_some(), egui::Button::new("Browse looks…"))
                .on_hover_text("Select a media clip to apply a look to it");
            if browse.clicked() {
                ui.close();
                if let Some(clip) = selected_clip {
                    self.look_browser.open_for(clip, None);
                }
            }
        });
    }

    /// The focused project's selected clip, when it is a media clip a look can
    /// be applied to.
    pub(crate) fn selected_media_clip(&self) -> Option<kinewright_core::ClipId> {
        let session = self.focused();
        session.selected_clip.filter(|clip| {
            session
                .document
                .clip(*clip)
                .is_some_and(|clip| matches!(clip.content, kinewright_core::ClipContent::Media))
        })
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
        self.show_color_qc_window(ui.ctx());
        self.look_browser(ui.ctx());
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
                egui::Frame::new()
                    .fill(color::SURFACE)
                    .inner_margin(egui::Margin::symmetric(
                        theme::margin(space::THREE),
                        theme::margin(space::ONE),
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(theme::wordmark("KINEWRIGHT", color::TEXT_SECONDARY));
                    ui.separator();
                    self.file_menu(ui);
                    self.view_menu(ui);
                    self.look_menu(ui);
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
        let mut strip_open = self.show_material_strip
            || self
                .focused()
                .threads
                .iter()
                .any(|thread| !thread.pending_confirmations.is_empty());
        let mut thread_rail_open = self.show_thread_rail;
        let mut media_rail_open = self.show_media_rail;
        let (dock_id, dock_default, dock_minimum) = if self.material_tab == MaterialTab::Mixer {
            ("mixer-dock", 320.0, 260.0)
        } else {
            ("timeline-dock", 240.0, 160.0)
        };
        egui::Panel::bottom(dock_id)
            .default_size(dock_default)
            .min_size(dock_minimum)
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
                    ui.selectable_value(&mut self.material_tab, MaterialTab::Mixer, "Mixer");
                });
                ui.separator();
                match self.material_tab {
                    MaterialTab::Timeline => self.timeline(ui),
                    MaterialTab::Transcript => self.transcript_panel(ui),
                    MaterialTab::Mixer => self.mixer_panel(ui),
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
        self.matte_overlay.expire_unreported();
    }
}

/// Two seconds of lead-in so a reviewed change plays with context.
pub(crate) fn review_preroll_frames(fps: kinewright_core::Rational) -> i64 {
    let nominal = i64::from(fps.numerator().saturating_add(fps.denominator() / 2))
        / i64::from(fps.denominator().max(1));
    nominal.max(1) * 2
}

/// Whether one operation only ever edits `audio_mix` (AU2 §6.8, AU4 §4.4).
///
/// Every variant here is an idempotent full set of one mix target, and
/// `audio_mix` is read by exactly the mix processor, the mixer panel, core
/// validation, and one line of the renderer — never by a video or clip path.
/// AU4 adds `SetTrackAutomation`, which parks its curve on the same
/// `TrackMix` entry the other track operation writes (AU4 §2.5 rule 32a).
const fn is_audio_mix_operation(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::SetTrackMix { .. }
            | Operation::SetTrackAutomation { .. }
            | Operation::UpsertAudioBus { .. }
            | Operation::RemoveAudioBus { .. }
            | Operation::SetAudioMaster { .. }
            | Operation::SetPanLaw { .. }
    )
}

/// Whether one operation only ever edits one clip's audio shaping (AU4 §4.4).
///
/// Both variants are idempotent full sets of one clip's audio, and neither can
/// move a clip boundary, change its asset, or declare a lookahead, so the
/// running mixer can rebuild its per-source shaping in place instead of
/// stopping (AU4 §3.8 rules 73–74).
const fn is_clip_audio_operation(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::SetClipAudio { .. } | Operation::SetClipGainEnvelope { .. }
    )
}

/// What a document change asks of a running engine (AU2 §6.8, AU4 §4.4).
///
/// The app is the only place that decides the kind; core owns the
/// [`LiveAudioChange`] type because the engine's `Control::UpdateAudio` carries
/// it across the crate boundary (AU4 §3.8 rule 75).
///
/// A history command qualifies only when *every* one of its operations falls
/// into one of the two live sets: [`is_audio_mix_operation`] or
/// [`is_clip_audio_operation`]. The returned kind then says which sets were
/// touched — `Mix`, `ClipShaping`, or `Both`. Undo, redo, the initial
/// snapshot (`None`), an empty batch, and any batch carrying one operation
/// outside both sets are `LiveAudioChange::None` and take the ordinary
/// stop-and-re-cue path, because only the all-live case is known to leave
/// every video and clip structure untouched.
///
/// A mixed batch is deliberately **not** demoted to the half it can serve: a
/// widened bool that took the live path and then applied only the mix half of
/// a Mixer-plus-inspector gesture would be silently worse than the re-cue it
/// replaced (AU4 §4.4 rule 89).
///
/// The lookahead comparison is the second half of the claim (AU2 §5.8,
/// OPEN-3). `L` is derived from the document and fixed for a processor's life,
/// so a chain edit that changes any declared lookahead — inserting a true-peak
/// limiter, removing one, retuning a compressor's lookahead — cannot be
/// retargeted into a running processor and takes the stop-and-re-cue path even
/// though every one of its operations is eligible.
pub(crate) fn live_audio_change(
    journal_command: Option<&JournalCommand>,
    old: &Document,
    new: &Document,
) -> LiveAudioChange {
    fn classify(operations: &[Operation]) -> LiveAudioChange {
        let mut mix = false;
        let mut clip_shaping = false;
        for operation in operations {
            if is_audio_mix_operation(operation) {
                mix = true;
            } else if is_clip_audio_operation(operation) {
                clip_shaping = true;
            } else {
                return LiveAudioChange::None;
            }
        }
        match (mix, clip_shaping) {
            (false, false) => LiveAudioChange::None,
            (true, false) => LiveAudioChange::Mix,
            (false, true) => LiveAudioChange::ClipShaping,
            (true, true) => LiveAudioChange::Both,
        }
    }

    let eligible = match journal_command {
        Some(JournalCommand::Do(operation)) => classify(std::slice::from_ref(operation)),
        Some(
            JournalCommand::DoBatch(operations)
            | JournalCommand::DoBatchCoalesced { operations, .. },
        ) => classify(operations),
        Some(JournalCommand::Undo | JournalCommand::Redo) | None => LiveAudioChange::None,
    };
    if old.audio_mix.lookahead_milliseconds() == new.audio_mix.lookahead_milliseconds() {
        eligible
    } else {
        LiveAudioChange::None
    }
}

/// Hand one document change to the running engine (AU4 §4.4 rule 89).
///
/// Returns whether playback survived: `true` when the engine was retargeted in
/// place, `false` when the change re-cued and the caller must stop the
/// transport. Extracted from `poll_background` because it is the whole
/// observable half of the live path and `KinewrightApp::new` needs a live GPU
/// engine, so this is the only seam a counting `Playback` double can reach.
fn apply_live_audio_change(
    playback: &dyn Playback,
    change: LiveAudioChange,
    doc: &Arc<Document>,
    position: TimeCode,
) -> bool {
    match change {
        LiveAudioChange::Mix | LiveAudioChange::ClipShaping | LiveAudioChange::Both => {
            playback.update_audio(change, Arc::clone(doc));
        }
        LiveAudioChange::None => {
            playback.set_document(Arc::clone(doc));
            playback.seek(position);
            playback.request_frame(position);
            return false;
        }
    }
    true
}

/// AU4 §2.5: the status line spells `SetTrackAutomation.parameter` the way a
/// person reads it, not the way the wire carries it.
///
/// `operation_status` also previews an agent's *proposed* operations
/// (`chat_ui.rs`), which core has not validated yet, so an unknown key falls
/// back to itself rather than being asserted away.
fn track_automation_label(parameter: &str) -> &str {
    match parameter {
        "gain_tenth_db" => "gain",
        "pan_percent" => "pan",
        other => other,
    }
}

/// The `s` a keyframe count needs, so a one-key curve reads "1 keyframe".
///
/// AU4 §2.5 only: the effect count next door is long-standing wording and is
/// pinned as it stands.
fn plural_s(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
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
        Operation::SetAudioMaster { master } => format!(
            "Set master mix (gain {:+.1} dB, {} effects)",
            f64::from(master.gain_tenth_db) / 10.0,
            master.effects.len()
        ),
        Operation::SetPanLaw { law } => format!(
            "Set pan law to {}",
            match law {
                kinewright_core::PanLaw::Balance => "balance",
                kinewright_core::PanLaw::ConstantPower => "constant power",
            }
        ),
        Operation::AddTrack { track } => format!("Added {:?} track {}", track.kind, track.id),
        Operation::RemoveTrack { track } => format!("Removed track {track}"),
        Operation::SetTrackSyncLock { track, locked } => format!(
            "Turned sync lock {} for track {track}",
            if *locked { "on" } else { "off" }
        ),
        Operation::SetTrackMix {
            track,
            gain_tenth_db,
            pan_percent,
            mute,
            solo,
        } => format!(
            "Set mix on track {track} (gain {:+.1} dB, pan {pan_percent}, mute {mute}, solo {solo})",
            f64::from(*gain_tenth_db) / 10.0
        ),
        Operation::SetTrackAutomation {
            track,
            parameter,
            curve,
        } => {
            let label = track_automation_label(parameter);
            curve.as_ref().map_or_else(
                || format!("Cleared {label} automation on track {track}"),
                |curve| {
                    let keys = curve.keyframes.len();
                    format!(
                        "Set {keys} {label} automation keyframe{} on track {track}",
                        plural_s(keys)
                    )
                },
            )
        }
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
        Operation::InsertEffect {
            clip,
            index,
            effect,
        } => format!(
            "Inserted {} effect {} at position {index} on clip {clip}",
            effect.name, effect.id
        ),
        Operation::ConvertLegacyLook {
            clip,
            effect,
            lut_asset,
            mix_basis_points,
        } => format!(
            "Converted legacy look {effect} on clip {clip} to a managed look on LUT asset \
             {lut_asset} at {}% mix",
            mix_basis_points / 100
        ),
        Operation::AddLutAsset { asset } => {
            format!("Registered LUT asset {} ({})", asset.id, asset.title)
        }
        Operation::RemoveLutAsset { lut_asset } => {
            format!("Removed LUT asset {lut_asset}")
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
        Operation::SetClipGainEnvelope { clip, curve } => curve.as_ref().map_or_else(
            || format!("Cleared the gain envelope on clip {clip}"),
            |curve| {
                let keys = curve.keyframes.len();
                format!(
                    "Set {keys} gain envelope keyframe{} on clip {clip}",
                    plural_s(keys)
                )
            },
        ),
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

/// The chain the screenshot harness pre-selects, if any (AU2 §6.6).
///
/// `KINEWRIGHT_SCREENSHOT_SHOW=mixer-chain` raises the Mixer with its chain
/// pane already open on the first bus, because a static capture has no way to
/// press an `Edit` toggle. `mixer` keeps AU1's behaviour: the tab, the strips,
/// and no pane. A document with no bus falls back to the master chain, which
/// every document has.
/// Whether the screenshot harness asked for the export dialog (AU3 §6.7).
///
/// The dialog is a summoned surface: no startup interaction reaches it in a
/// static capture, so the harness pre-raises it exactly as it pre-raises the
/// settings window. It opens on the controls only — the harness can seed no
/// finished export, so this lane never captures the verification block.
fn screenshot_export_dialog_open(show: Option<&str>) -> bool {
    matches!(show, Some("export"))
}

fn screenshot_mixer_selection(
    show: Option<&str>,
    document: &Document,
) -> Option<crate::mixer_ui::MixerSelection> {
    match show {
        Some("mixer-chain") => Some(
            document
                .audio_mix
                .buses
                .first()
                .map_or(crate::mixer_ui::MixerSelection::Master, |bus| {
                    crate::mixer_ui::MixerSelection::Bus(bus.id)
                }),
        ),
        _ => None,
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

/// AU5 §3.7 rule 63: the shortest range a noise profile can be learned from,
/// in **audio sample frames** at the render rate.
///
/// `NOISE_PROFILE_SEGMENT_FRAMES + 9 × NOISE_PROFILE_HOP_FRAMES` — ten
/// windows, the fewest a 20th percentile means anything over. The media crate
/// keeps its own copy `pub(crate)` and re-checks the rendered count itself, so
/// this one is the app's *gate*, never the authority: `mix_noise_profile`
/// refuses a short range by name whatever the app believed.
pub(crate) const NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES: u64 = 22_528;
/// The rate `mix_noise_profile` renders at (`export.rs`'s `AUDIO_RATE`).
pub(crate) const NOISE_PROFILE_SAMPLE_RATE: u64 = 48_000;
/// What a `Learn` says when the machine could not give it a thread.
pub(crate) const LEARN_WORKER_UNAVAILABLE: &str = "Could not start the noise profile worker";

/// `ceil(22 528 / 48 000 × fps)`, computed rather than written down (AU5 §3.7
/// rule 63).
///
/// The three frame domains are audio sample, source and project. This turns
/// the sample-frame minimum into either of the other two, given that domain's
/// rate: a project minimum at `document.fps`, a source minimum at
/// `asset.fps` — which for an audio-only asset is `Rational::default()`, 30/1.
pub(crate) fn noise_profile_minimum_frames(fps: Rational) -> u64 {
    let numerator = NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES * u64::from(fps.numerator());
    let denominator = NOISE_PROFILE_SAMPLE_RATE * u64::from(fps.denominator());
    numerator.div_ceil(denominator.max(1))
}

/// The tracks whose signal reaches one chain (AU5 §6.3 rule 127).
///
/// A bus is fed by exactly the tracks it lists; the master is fed by every
/// track in the project, because every track reaches it through a bus or
/// directly. `validate_audio_mix` already forbids a track in two buses, so
/// the bus answer needs no de-duplication.
pub(crate) fn tracks_feeding_chain(document: &Document, chain: AudioChain) -> Vec<TrackId> {
    match chain {
        AudioChain::Bus(id) => document
            .audio_mix
            .bus(id)
            .map(|bus| bus.tracks.clone())
            .unwrap_or_default(),
        AudioChain::Master => document.tracks.iter().map(|track| track.id).collect(),
    }
}

/// One learn measurement, as the worker delivers it.
struct NoiseProfileResponse {
    generation: u64,
    session: u64,
    chain: AudioChain,
    effect: EffectId,
    result: Result<[i32; NOISE_PROFILE_BAND_COUNT], String>,
}

/// A learn the app accepted while a worker was still measuring.
struct NoiseProfileJob {
    generation: u64,
    session: u64,
    chain: AudioChain,
    effect: EffectId,
    analysis: Arc<dyn Analysis>,
    document: Arc<Document>,
    request: MixNoiseProfileRequest,
}

/// The running worker and the flag that retires it.
struct LearnWorker {
    cancelled: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl LearnWorker {
    fn retire(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// AU5 §6.3 rule 127: the single-flight machine behind `Learn profile`.
///
/// The colour-QC pattern in miniature, and for the same reason:
/// `mix_noise_profile` decodes and renders a mix range, which must not happen
/// on a frame. Latest request wins — a second click parks in `queued` rather
/// than starting a second render — and a response is accepted only if its
/// generation is still the live one, so a superseded measurement can never
/// land in the document.
pub(crate) struct NoiseLearnState {
    active: Option<LearnWorker>,
    queued: Option<NoiseProfileJob>,
    /// The generation the app is waiting for, if any.
    pending: Option<u64>,
    generation: u64,
    response_tx: mpsc::Sender<NoiseProfileResponse>,
    response_rx: mpsc::Receiver<NoiseProfileResponse>,
    #[cfg(test)]
    spawned_workers: u64,
    /// Refuse the next thread spawn, so the arm that reports a worker that
    /// could not start has a test.
    #[cfg(test)]
    refuse_next_spawn: bool,
}

impl std::fmt::Debug for NoiseLearnState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NoiseLearnState")
            .field("pending", &self.pending)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Default for NoiseLearnState {
    fn default() -> Self {
        let (response_tx, response_rx) = mpsc::channel();
        Self {
            active: None,
            queued: None,
            pending: None,
            generation: 0,
            response_tx,
            response_rx,
            #[cfg(test)]
            spawned_workers: 0,
            #[cfg(test)]
            refuse_next_spawn: false,
        }
    }
}

impl NoiseLearnState {
    #[must_use]
    pub(crate) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Accept one learn request. Latest wins.
    fn request(&mut self, job: NoiseProfileJob) {
        self.generation = self.generation.wrapping_add(1);
        let job = NoiseProfileJob {
            generation: self.generation,
            ..job
        };
        self.pending = Some(self.generation);
        if self
            .active
            .as_ref()
            .is_some_and(|worker| !LearnWorker::is_finished(worker))
        {
            if let Some(worker) = self.active.as_ref() {
                worker.retire();
            }
            self.queued = Some(job);
            return;
        }
        self.reap_finished_worker();
        self.spawn(job);
    }

    fn spawn(&mut self, job: NoiseProfileJob) {
        let NoiseProfileJob {
            generation,
            session,
            chain,
            effect,
            analysis,
            document,
            request,
        } = job;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let response_tx = self.response_tx.clone();
        let spawn_result = if self.spawn_is_refused() {
            Err(std::io::Error::other(LEARN_WORKER_UNAVAILABLE))
        } else {
            thread::Builder::new()
                .name("kinewright-noise-profile".to_owned())
                .spawn(move || {
                    let result = analysis
                        .mix_noise_profile(&document, &request)
                        .map(|report| report.bands)
                        .map_err(|error| error.to_string());
                    if worker_cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    let _ = response_tx.send(NoiseProfileResponse {
                        generation,
                        session,
                        chain,
                        effect,
                        result,
                    });
                })
        };
        let Ok(handle) = spawn_result else {
            self.active = None;
            let _ = self.response_tx.send(NoiseProfileResponse {
                generation,
                session,
                chain,
                effect,
                result: Err(LEARN_WORKER_UNAVAILABLE.to_owned()),
            });
            return;
        };
        #[cfg(test)]
        {
            self.spawned_workers += 1;
        }
        self.active = Some(LearnWorker { cancelled, handle });
    }

    /// Whether this spawn is refused before it is attempted.
    ///
    /// Always `false` outside tests. A refused thread spawn is the one arm of
    /// the worker no fixture can otherwise reach — the OS has to run out of
    /// threads — and it is exactly the arm that swallowed its own error until
    /// the pass-1 review found it, so it gets a seam rather than no cover.
    #[cfg(test)]
    fn spawn_is_refused(&mut self) -> bool {
        std::mem::take(&mut self.refuse_next_spawn)
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    const fn spawn_is_refused(&mut self) -> bool {
        false
    }

    fn reap_finished_worker(&mut self) {
        if self.active.as_ref().is_some_and(LearnWorker::is_finished)
            && let Some(worker) = self.active.take()
        {
            let _ = worker.handle.join();
        }
    }

    /// Drain worker responses, accepting only the live generation, and start a
    /// parked request when the running worker is done.
    fn poll(&mut self) -> Vec<NoiseProfileResponse> {
        let mut accepted = Vec::new();
        while let Ok(response) = self.response_rx.try_recv() {
            if self.pending != Some(response.generation) {
                continue;
            }
            self.pending = None;
            accepted.push(response);
        }
        self.reap_finished_worker();
        if self.active.is_none()
            && let Some(job) = self.queued.take()
        {
            self.spawn(job);
        }
        accepted
    }

    #[cfg(test)]
    const fn spawned_workers(&self) -> u64 {
        self.spawned_workers
    }

    /// Refuse the next thread spawn, so R136's arm has a test.
    #[cfg(test)]
    const fn refusing_next_spawn(mut self) -> Self {
        self.refuse_next_spawn = true;
        self
    }
}

impl KinewrightApp {
    /// AU5 §6.3 rule 127: the range a `Learn profile` on the selected chain
    /// would read, or why there is not one.
    ///
    /// The routine takes `(document, range, minimum_source_frames)` and has no
    /// track argument, so the **caller** filters the answer on
    /// `TimelineSilenceSpan.track`. The minimum handed down is the smallest
    /// any relevant asset could need, so nothing long enough is filtered out
    /// on the way; the project-frame gate below is the one that decides.
    pub(crate) fn noise_learn_range(&self, chain: AudioChain) -> NoiseLearnRange {
        let session = self.focused();
        let document = &session.document;
        let tracks = tracks_feeding_chain(document, chain);
        if tracks.is_empty() {
            return NoiseLearnRange::NoSilence;
        }
        let assets = assets_on_tracks(document, &tracks);
        if assets.is_empty() {
            return NoiseLearnRange::NoSilence;
        }
        let mut minimum_source_frames = u64::MAX;
        for asset in &assets {
            let status = self.analysis.silence_status(asset);
            match status {
                SilenceStatus::Ready(_) => {}
                SilenceStatus::NoAudio | SilenceStatus::Failed(_) | SilenceStatus::Cancelled => {
                    continue;
                }
                SilenceStatus::NotRequested
                | SilenceStatus::Queued
                | SilenceStatus::Hashing
                | SilenceStatus::Analyzing => {
                    if matches!(status, SilenceStatus::NotRequested) {
                        self.analysis.request_silence_detection(asset.clone());
                    }
                    return NoiseLearnRange::Analysing;
                }
            }
            minimum_source_frames =
                minimum_source_frames.min(noise_profile_minimum_frames(asset.fps));
        }
        if minimum_source_frames == u64::MAX {
            return NoiseLearnRange::NoSilence;
        }
        let Ok(spans) = self.analysis.timeline_silences(
            document,
            None,
            TimeCode(i64::try_from(minimum_source_frames).unwrap_or(i64::MAX)),
        ) else {
            return NoiseLearnRange::NoSilence;
        };
        let minimum_project_frames =
            i64::try_from(noise_profile_minimum_frames(document.fps)).unwrap_or(i64::MAX);
        spans
            .into_iter()
            .filter(|span| tracks.contains(&span.track))
            .filter(|span| span.project_end.0 - span.project_start.0 >= minimum_project_frames)
            .max_by_key(|span| span.project_end.0 - span.project_start.0)
            .map_or(NoiseLearnRange::NoSilence, |span| {
                NoiseLearnRange::Span(span.project_start, span.project_end)
            })
    }

    /// AU5 §6.3: start one learn measurement for the node the card named.
    pub(crate) fn request_noise_profile(&mut self, chain: AudioChain, effect: EffectId) {
        let NoiseLearnRange::Span(start, end) = self.noise_learn_range(chain) else {
            self.record_error("Mixer", LEARN_NO_SILENCE);
            return;
        };
        let session = self.focused();
        let job = NoiseProfileJob {
            generation: 0,
            session: session.id,
            chain,
            effect,
            analysis: Arc::clone(&self.analysis),
            document: Arc::clone(&session.document),
            request: MixNoiseProfileRequest {
                range: Some(start..end),
                point: match chain {
                    AudioChain::Bus(id) => MixSpectrumPoint::Bus(id),
                    AudioChain::Master => MixSpectrumPoint::Master,
                },
            },
        };
        self.noise_learn.request(job);
        "Learning the noise floor\u{2026}".clone_into(&mut self.status);
    }

    /// AU5 §6.3 rule 127: land one accepted measurement as **one** chain set.
    fn apply_noise_profile(
        &mut self,
        session: u64,
        chain: AudioChain,
        effect: EffectId,
        bands: [i32; NOISE_PROFILE_BAND_COUNT],
    ) {
        if self.focused().id != session {
            return;
        }
        let document = Arc::clone(&self.focused().document);
        let Some(operation) = noise_profile_operation(&document, chain, effect, &bands) else {
            self.record_error(
                "Mixer",
                "The denoise node the profile was learned for is no longer on this chain",
            );
            return;
        };
        let gesture = self.begin_edit_gesture();
        let key = match chain {
            AudioChain::Bus(id) => crate::mixer_ui::MixerSelection::Bus(id).coalesce_key(),
            AudioChain::Master => crate::mixer_ui::MixerSelection::Master.coalesce_key(),
        };
        self.send_operations_coalesced(vec![operation], format!("{key}#{gesture}"));
        "Learned the noise floor".clone_into(&mut self.status);
    }

    /// Drain the learn worker; called from `poll_background`.
    fn poll_noise_learn(&mut self, ctx: &egui::Context) {
        for response in self.noise_learn.poll() {
            match response.result {
                Ok(bands) => self.apply_noise_profile(
                    response.session,
                    response.chain,
                    response.effect,
                    bands,
                ),
                Err(error) => {
                    self.record_error("Mixer", format!("Could not learn a noise profile: {error}"));
                }
            }
        }
        if self.noise_learn.is_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

/// Every asset a clip on one of these tracks plays.
fn assets_on_tracks(document: &Document, tracks: &[TrackId]) -> Vec<MediaAsset> {
    let mut assets: Vec<MediaAsset> = Vec::new();
    for track in document.tracks.iter().filter(|t| tracks.contains(&t.id)) {
        for clip in &track.clips {
            if !clip.content.is_media() {
                continue;
            }
            if assets.iter().any(|held| held.id == clip.asset) {
                continue;
            }
            if let Some(found) = document
                .media_pool
                .iter()
                .find(|held| held.id == clip.asset)
            {
                assets.push(found.clone());
            }
        }
    }
    assets
}

/// The one operation a completed learn writes (AU5 §6.3 rule 127).
///
/// Written as a whole-chain set, because `UpsertAudioBus` and `SetAudioMaster`
/// are the only shapes a chain edit has: there is no per-parameter operation,
/// which is exactly why one `Learn` is one undo entry.
pub(crate) fn noise_profile_operation(
    document: &Document,
    chain: AudioChain,
    effect: EffectId,
    bands: &[i32; NOISE_PROFILE_BAND_COUNT],
) -> Option<Operation> {
    match chain {
        AudioChain::Bus(id) => {
            let mut bus = document.audio_mix.bus(id)?.clone();
            write_noise_profile(&mut bus.effects, effect, bands)?;
            Some(Operation::UpsertAudioBus { bus })
        }
        AudioChain::Master => {
            let mut master = document.audio_mix.master.clone();
            write_noise_profile(&mut master.effects, effect, bands)?;
            Some(Operation::SetAudioMaster { master })
        }
    }
}

/// Write all 31 bands into one denoise node, or refuse.
///
/// All 31 or none (AU5 §2.1): a partially written profile is not a profile,
/// and the runtime's "all 31 at the neutral means unity gain" rule reads the
/// whole block.
fn write_noise_profile(
    effects: &mut [Effect],
    effect: EffectId,
    bands: &[i32; NOISE_PROFILE_BAND_COUNT],
) -> Option<()> {
    let node = effects
        .iter_mut()
        .find(|candidate| candidate.id == effect && candidate.name == "audio_denoise")?;
    for (name, band) in NOISE_PROFILE_PARAMETER_NAMES.iter().zip(bands) {
        node.parameters
            .insert((*name).to_owned(), ParamValue::Integer(i64::from(*band)));
    }
    Some(())
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

    fn set_track_mix(track: u64) -> super::Operation {
        super::Operation::SetTrackMix {
            track: super::TrackId(track),
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
        }
    }

    fn set_track_automation(track: u64) -> super::Operation {
        super::Operation::SetTrackAutomation {
            track: super::TrackId(track),
            parameter: "gain_tenth_db".to_owned(),
            curve: Some(curve()),
        }
    }

    fn set_clip_audio(clip: u64) -> super::Operation {
        super::Operation::SetClipAudio {
            clip: kinewright_core::ClipId(clip),
            gain_tenth_db: -60,
            fade_in_frames: kinewright_core::TimeCode::ZERO,
            fade_out_frames: kinewright_core::TimeCode::ZERO,
        }
    }

    fn set_clip_gain_envelope(clip: u64) -> super::Operation {
        super::Operation::SetClipGainEnvelope {
            clip: kinewright_core::ClipId(clip),
            curve: Some(curve()),
        }
    }

    fn curve() -> kinewright_core::AutomationCurve {
        kinewright_core::AutomationCurve {
            keyframes: vec![kinewright_core::Keyframe {
                at: kinewright_core::TimeCode::ZERO,
                value: -60,
                interpolation: kinewright_core::KeyframeInterpolation::Linear,
            }],
        }
    }

    fn two_key_curve() -> kinewright_core::AutomationCurve {
        kinewright_core::AutomationCurve {
            keyframes: vec![
                kinewright_core::Keyframe {
                    at: kinewright_core::TimeCode::ZERO,
                    value: -60,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                },
                kinewright_core::Keyframe {
                    at: kinewright_core::TimeCode(12),
                    value: 0,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                },
            ],
        }
    }

    /// AU2 §6.8 and AU4 §7 A15: the `live_audio_change` truth table.
    ///
    /// A history command keeps the transport running only when every one of
    /// its operations is in one of the two live sets, and the kind it returns
    /// says which sets were touched. Everything else re-cues, because only
    /// that case is known to leave the video and clip structure — and the
    /// running processor's fixed latency — untouched.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn live_audio_change_names_which_half_of_the_engine_a_command_retargets() {
        use kinewright_core::LiveAudioChange;

        use super::{JournalCommand, live_audio_change};

        let plain = super::Document::default();
        let live = |command: Option<&JournalCommand>| live_audio_change(command, &plain, &plain);

        let mix_operations = [
            set_track_mix(1),
            set_track_automation(1),
            super::Operation::UpsertAudioBus {
                bus: kinewright_core::AudioBus {
                    id: kinewright_core::AudioBusId(1),
                    name: "Dialogue".to_owned(),
                    tracks: vec![super::TrackId(1)],
                    gain_tenth_db: -30,
                    effects: Vec::new(),
                    ducking_sidechain_tracks: Vec::new(),
                    gain_curve: None,
                },
            },
            super::Operation::RemoveAudioBus {
                bus: kinewright_core::AudioBusId(1),
            },
            super::Operation::SetAudioMaster {
                master: kinewright_core::AudioMaster {
                    gain_tenth_db: -20,
                    effects: Vec::new(),
                    gain_curve: None,
                },
            },
            super::Operation::SetPanLaw {
                law: kinewright_core::PanLaw::ConstantPower,
            },
        ];
        for operation in &mix_operations {
            assert_eq!(
                live(Some(&JournalCommand::Do(operation.clone()))),
                LiveAudioChange::Mix,
                "{operation:?} is a mix operation"
            );
            assert_eq!(
                live(Some(&JournalCommand::DoBatch(vec![
                    set_track_mix(1),
                    operation.clone()
                ]))),
                LiveAudioChange::Mix
            );
            assert_eq!(
                live(Some(&JournalCommand::DoBatchCoalesced {
                    operations: vec![operation.clone()],
                    coalesce_key: "track_mix:1#3".to_owned(),
                })),
                LiveAudioChange::Mix
            );
        }

        // The two clip-audio operations are `ClipShaping`, alone and together.
        let clip_operations = [set_clip_audio(1), set_clip_gain_envelope(1)];
        for operation in &clip_operations {
            assert_eq!(
                live(Some(&JournalCommand::Do(operation.clone()))),
                LiveAudioChange::ClipShaping,
                "{operation:?} shapes one clip's audio"
            );
            assert_eq!(
                live(Some(&JournalCommand::DoBatchCoalesced {
                    operations: vec![operation.clone()],
                    coalesce_key: "envelope:1#3".to_owned(),
                })),
                LiveAudioChange::ClipShaping,
                "AU4 §3.8 rule 77: a coalesced envelope drag retargets, it does not re-cue"
            );
        }
        assert_eq!(
            live(Some(&JournalCommand::DoBatch(vec![
                set_clip_audio(1),
                set_clip_gain_envelope(1)
            ]))),
            LiveAudioChange::ClipShaping
        );

        for mix in &mix_operations {
            for clip in &clip_operations {
                assert_eq!(
                    live(Some(&JournalCommand::DoBatch(vec![
                        mix.clone(),
                        clip.clone()
                    ]))),
                    LiveAudioChange::Both,
                    "{mix:?} beside {clip:?} touches both halves"
                );
                assert_eq!(
                    live(Some(&JournalCommand::DoBatchCoalesced {
                        operations: vec![clip.clone(), mix.clone()],
                        coalesce_key: "envelope:1#3".to_owned(),
                    })),
                    LiveAudioChange::Both,
                    "order inside the batch does not change which halves were touched"
                );
            }
        }

        // Everything else re-cues.
        let other = super::Operation::RemoveTrack {
            track: super::TrackId(2),
        };
        assert_eq!(
            live(Some(&JournalCommand::Do(other.clone()))),
            LiveAudioChange::None,
            "an ordinary edit still stops and re-cues"
        );
        for batch in [
            vec![set_track_mix(1), other.clone()],
            vec![set_clip_gain_envelope(1), other.clone()],
            vec![set_track_mix(1), set_clip_audio(1), other.clone()],
        ] {
            assert_eq!(
                live(Some(&JournalCommand::DoBatch(batch.clone()))),
                LiveAudioChange::None,
                "a mixed batch may change anything, so it takes the ordinary path"
            );
            assert_eq!(
                live(Some(&JournalCommand::DoBatchCoalesced {
                    operations: batch,
                    coalesce_key: "track_mix:1#3".to_owned(),
                })),
                LiveAudioChange::None,
                "a mixed coalesced batch is no different"
            );
        }
        assert_eq!(
            live(Some(&JournalCommand::Undo)),
            LiveAudioChange::None,
            "AU1 §5.4: undo during playback stops and re-cues"
        );
        assert_eq!(live(Some(&JournalCommand::Redo)), LiveAudioChange::None);
        assert_eq!(
            live(None),
            LiveAudioChange::None,
            "the initial snapshot is not a live edit"
        );
        assert_eq!(
            live(Some(&JournalCommand::DoBatch(Vec::new()))),
            LiveAudioChange::None,
            "an empty batch claims nothing about the mix"
        );
        assert_eq!(
            live(Some(&JournalCommand::DoBatchCoalesced {
                operations: Vec::new(),
                coalesce_key: "track_mix:1#3".to_owned(),
            })),
            LiveAudioChange::None,
            "and neither does an empty coalesced batch"
        );

        let mut with_lookahead = super::Document::default();
        with_lookahead.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: 0,
            effects: vec![kinewright_core::Effect {
                id: kinewright_core::EffectId(1),
                name: "audio_true_peak_limiter".to_owned(),
                parameters: std::collections::BTreeMap::from([(
                    "lookahead_milliseconds".to_owned(),
                    kinewright_core::ParamValue::Integer(5),
                )]),
                keyframes: std::collections::BTreeMap::new(),
            }],
            gain_curve: None,
        };
        assert_ne!(
            plain.audio_mix.lookahead_milliseconds(),
            with_lookahead.audio_mix.lookahead_milliseconds(),
            "the fixture must actually change the declared latency"
        );
        for eligible in [
            super::Operation::SetAudioMaster {
                master: with_lookahead.audio_mix.master.clone(),
            },
            set_clip_gain_envelope(1),
        ] {
            assert_eq!(
                live_audio_change(
                    Some(&JournalCommand::Do(eligible.clone())),
                    &plain,
                    &with_lookahead,
                ),
                LiveAudioChange::None,
                "a change to the declared latency re-cues however eligible {eligible:?} is"
            );
        }
        assert_eq!(
            live_audio_change(
                Some(&JournalCommand::DoBatch(vec![
                    set_track_mix(1),
                    set_clip_gain_envelope(1)
                ])),
                &with_lookahead,
                &with_lookahead,
            ),
            LiveAudioChange::Both,
            "the same latency on both sides is the live case, whatever the figure"
        );
    }

    /// A `Playback` that counts what the live path asked of it (AU4 §7 A15).
    ///
    /// Only the calls the live path can make are counted; everything else is
    /// the inert stub `KinewrightApp` would otherwise need a GPU for.
    #[derive(Default)]
    struct CountingPlayback {
        set_documents: std::sync::atomic::AtomicUsize,
        seeks: std::sync::atomic::AtomicUsize,
        requested_frames: std::sync::atomic::AtomicUsize,
        mixes: std::sync::atomic::AtomicUsize,
        shapings: std::sync::atomic::AtomicUsize,
        /// Every kind handed to `update_audio`, so "one control per change"
        /// (AU4 §3.8 rule 75) is a count, not a guess.
        audio_changes: std::sync::Mutex<Vec<super::LiveAudioChange>>,
        /// The order the counted calls arrived in, so the re-cue path can be
        /// asserted to be document, seek, frame.
        order: std::sync::Mutex<Vec<&'static str>>,
    }

    impl CountingPlayback {
        fn count(counter: &std::sync::atomic::AtomicUsize) -> usize {
            counter.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn bump(&self, counter: &std::sync::atomic::AtomicUsize, name: &'static str) {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.order.lock().expect("no test panics here").push(name);
        }
    }

    impl super::Playback for CountingPlayback {
        fn set_document(&self, _document: std::sync::Arc<super::Document>) {
            self.bump(&self.set_documents, "set_document");
        }
        fn request_frame(&self, _at: super::TimeCode) {
            self.bump(&self.requested_frames, "request_frame");
        }
        fn frames(
            &self,
        ) -> crossbeam_channel::Receiver<(super::TimeCode, kinewright_core::FrameTexture)> {
            crossbeam_channel::bounded(0).1
        }
        fn events(&self) -> crossbeam_channel::Receiver<super::MediaEvent> {
            crossbeam_channel::bounded(0).1
        }
        fn play(&self, _from: super::TimeCode) {}
        fn pause(&self) {}
        fn seek(&self, _to: super::TimeCode) {
            self.bump(&self.seeks, "seek");
        }
        fn position(&self) -> super::TimeCode {
            super::TimeCode::ZERO
        }
        fn output_peaks(&self) -> [f32; 2] {
            [0.0, 0.0]
        }
        fn update_audio_mix(&self, _doc: std::sync::Arc<super::Document>) {
            self.bump(&self.mixes, "update_audio_mix");
        }
        fn update_clip_shaping(&self, _doc: std::sync::Arc<super::Document>) {
            self.bump(&self.shapings, "update_clip_shaping");
        }
        fn update_audio(
            &self,
            change: super::LiveAudioChange,
            _doc: std::sync::Arc<super::Document>,
        ) {
            self.audio_changes
                .lock()
                .expect("no test panics here")
                .push(change);
            self.order
                .lock()
                .expect("no test panics here")
                .push("update_audio");
        }
    }

    /// AU4 §7 A15 and §3.8 rule 77: an envelope edit retargets the running
    /// engine instead of stopping it, and every live kind — `Both` included —
    /// crosses the seam as exactly one `update_audio` call carrying one
    /// document, which is what stops the two halves interleaving with each
    /// other or with a re-cue (§4.4 rule 89).
    ///
    /// That one call is where the app's job ends: dispatching `Both` to the
    /// two halves in order is the trait default's job, and
    /// `the_default_update_audio_dispatches_every_live_audio_change`
    /// (`kinewright-media/src/engine.rs`) is what tests it, including on a
    /// double that overrides neither half.
    #[test]
    fn a_clip_shaping_change_retargets_the_engine_instead_of_re_cueing() {
        use kinewright_core::LiveAudioChange;

        use super::apply_live_audio_change;

        /// Neither half is ever called directly: the app hands the kind to
        /// `update_audio` and lets the implementation split it.
        fn assert_no_half_called(playback: &CountingPlayback) {
            assert_eq!(CountingPlayback::count(&playback.mixes), 0);
            assert_eq!(CountingPlayback::count(&playback.shapings), 0);
            assert_eq!(
                CountingPlayback::count(&playback.set_documents),
                0,
                "a retarget never re-cues"
            );
        }

        let doc = std::sync::Arc::new(super::Document::default());
        let at = super::TimeCode(12);
        let changes = |playback: &CountingPlayback| {
            playback
                .audio_changes
                .lock()
                .expect("no test panics here")
                .clone()
        };

        let shaping = CountingPlayback::default();
        assert!(
            apply_live_audio_change(&shaping, LiveAudioChange::ClipShaping, &doc, at),
            "a clip shaping change keeps the transport running"
        );
        assert_eq!(changes(&shaping), [LiveAudioChange::ClipShaping]);
        assert_no_half_called(&shaping);
        assert_eq!(CountingPlayback::count(&shaping.seeks), 0);
        assert_eq!(CountingPlayback::count(&shaping.requested_frames), 0);

        let mix = CountingPlayback::default();
        assert!(apply_live_audio_change(
            &mix,
            LiveAudioChange::Mix,
            &doc,
            at
        ));
        assert_eq!(changes(&mix), [LiveAudioChange::Mix]);
        assert_no_half_called(&mix);

        let both = CountingPlayback::default();
        assert!(apply_live_audio_change(
            &both,
            LiveAudioChange::Both,
            &doc,
            at
        ));
        assert_eq!(
            changes(&both),
            [LiveAudioChange::Both],
            "AU4 §4.4 rule 89: one document, one control, no re-cue between"
        );
        assert_eq!(
            *both.order.lock().expect("no test panics here"),
            ["update_audio"],
            "and nothing else is asked of the engine"
        );
        assert_no_half_called(&both);

        let recue = CountingPlayback::default();
        assert!(
            !apply_live_audio_change(&recue, LiveAudioChange::None, &doc, at),
            "the ordinary path stops the transport"
        );
        assert_eq!(
            *recue.order.lock().expect("no test panics here"),
            ["set_document", "seek", "request_frame"],
            "and re-cues at the stored position"
        );
        assert!(
            changes(&recue).is_empty(),
            "a re-cue is not a live audio change"
        );
        assert_eq!(CountingPlayback::count(&recue.mixes), 0);
        assert_eq!(CountingPlayback::count(&recue.shapings), 0);
    }

    /// AU4 §3.8 rule 75: `Playback::update_clip_shaping` is defaulted, so a
    /// double that never heard of AU4 still applies the document.
    #[test]
    fn update_clip_shaping_defaults_to_set_document_on_an_older_double() {
        struct OlderDouble(std::sync::atomic::AtomicUsize);

        impl super::Playback for OlderDouble {
            fn set_document(&self, _document: std::sync::Arc<super::Document>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            fn request_frame(&self, _at: super::TimeCode) {}
            fn frames(
                &self,
            ) -> crossbeam_channel::Receiver<(super::TimeCode, kinewright_core::FrameTexture)>
            {
                crossbeam_channel::bounded(0).1
            }
            fn events(&self) -> crossbeam_channel::Receiver<super::MediaEvent> {
                crossbeam_channel::bounded(0).1
            }
            fn play(&self, _from: super::TimeCode) {}
            fn pause(&self) {}
            fn seek(&self, _to: super::TimeCode) {}
            fn position(&self) -> super::TimeCode {
                super::TimeCode::ZERO
            }
            fn output_peaks(&self) -> [f32; 2] {
                [0.0, 0.0]
            }
        }

        let double = OlderDouble(std::sync::atomic::AtomicUsize::new(0));
        super::Playback::update_clip_shaping(
            &double,
            std::sync::Arc::new(super::Document::default()),
        );
        super::Playback::update_audio_mix(&double, std::sync::Arc::new(super::Document::default()));
        assert_eq!(
            double.0.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both live methods default to `set_document`"
        );
    }

    /// AU3 §6.7 and §7 B14: the screenshot harness can raise the export
    /// dialog, and no other value opens it.
    ///
    /// The recognised set is `settings`, `timeline`, `transcript`, `mixer`,
    /// `mixer-chain`, and now `export`; an unknown value raises nothing, which
    /// is what keeps a typo from silently capturing the default window.
    #[test]
    fn the_screenshot_harness_can_raise_the_export_dialog() {
        use super::screenshot_export_dialog_open;

        assert!(screenshot_export_dialog_open(Some("export")));
        for other in [
            None,
            Some("settings"),
            Some("timeline"),
            Some("transcript"),
            Some("mixer"),
            Some("mixer-chain"),
            Some("Export"),
            Some(""),
        ] {
            assert!(
                !screenshot_export_dialog_open(other),
                "{other:?} does not open the export dialog"
            );
        }
    }

    /// AU2 §6.6: the screenshot harness can raise the Mixer with the chain
    /// pane already open, and `mixer` still means strips only.
    #[test]
    fn the_screenshot_harness_can_pre_select_a_chain() {
        use super::screenshot_mixer_selection;
        use crate::mixer_ui::MixerSelection;

        let mut document = super::Document::default();
        assert_eq!(screenshot_mixer_selection(None, &document), None);
        assert_eq!(
            screenshot_mixer_selection(Some("mixer"), &document),
            None,
            "the AU1 value still shows the strips and no pane"
        );
        assert_eq!(
            screenshot_mixer_selection(Some("timeline"), &document),
            None
        );
        assert_eq!(
            screenshot_mixer_selection(Some("mixer-chain"), &document),
            Some(MixerSelection::Master),
            "a document with no bus falls back to the chain every document has"
        );
        document.audio_mix.buses.push(kinewright_core::AudioBus {
            id: kinewright_core::AudioBusId(4),
            name: "Dialogue".to_owned(),
            tracks: Vec::new(),
            gain_tenth_db: 0,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        });
        assert_eq!(
            screenshot_mixer_selection(Some("mixer-chain"), &document),
            Some(MixerSelection::Bus(kinewright_core::AudioBusId(4))),
            "the first bus in document order is the one a capture shows"
        );
    }

    /// AU2 §6.8 and §7 B20: the two new mix operations read back in the same
    /// voice the rest of the status map uses.
    #[test]
    fn the_master_and_pan_law_edits_report_what_they_set() {
        assert_eq!(
            super::operation_status(&super::Operation::SetAudioMaster {
                master: kinewright_core::AudioMaster {
                    gain_tenth_db: -35,
                    effects: vec![kinewright_core::Effect {
                        id: kinewright_core::EffectId(1),
                        name: "audio_gain".to_owned(),
                        parameters: std::collections::BTreeMap::new(),
                        keyframes: std::collections::BTreeMap::new(),
                    }],
                    gain_curve: None,
                }
            }),
            "Set master mix (gain -3.5 dB, 1 effects)"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetAudioMaster {
                master: kinewright_core::AudioMaster::default()
            }),
            "Set master mix (gain +0.0 dB, 0 effects)"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetPanLaw {
                law: kinewright_core::PanLaw::Balance
            }),
            "Set pan law to balance"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetPanLaw {
                law: kinewright_core::PanLaw::ConstantPower
            }),
            "Set pan law to constant power"
        );
    }

    /// AU4 §2.5 and E12: the two automation edits read back in the same voice,
    /// with the label E12 chose rather than the wire spelling.
    ///
    /// Set and cleared for each of the two variants, both counts of the
    /// singular/plural pair, and one parameter core has not validated —
    /// `chat_ui` previews an agent's *proposed* operations, so an unknown key
    /// has to fall through to itself rather than be asserted away.
    #[test]
    fn the_two_automation_edits_report_what_they_set() {
        assert_eq!(
            super::operation_status(&set_track_automation(4)),
            "Set 1 gain automation keyframe on track 4"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetTrackAutomation {
                track: super::TrackId(4),
                parameter: "pan_percent".to_owned(),
                curve: Some(two_key_curve()),
            }),
            "Set 2 pan automation keyframes on track 4"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetTrackAutomation {
                track: super::TrackId(4),
                parameter: "gain_tenth_db".to_owned(),
                curve: None,
            }),
            "Cleared gain automation on track 4"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetTrackAutomation {
                track: super::TrackId(4),
                parameter: "tilt_percent".to_owned(),
                curve: Some(curve()),
            }),
            "Set 1 tilt_percent automation keyframe on track 4"
        );
        assert_eq!(
            super::operation_status(&set_clip_gain_envelope(7)),
            "Set 1 gain envelope keyframe on clip 7"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetClipGainEnvelope {
                clip: kinewright_core::ClipId(7),
                curve: Some(two_key_curve()),
            }),
            "Set 2 gain envelope keyframes on clip 7"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetClipGainEnvelope {
                clip: kinewright_core::ClipId(7),
                curve: None,
            }),
            "Cleared the gain envelope on clip 7"
        );
    }

    /// The status line reads back every value the operation carries, in the
    /// voice the rest of the map uses.
    #[test]
    fn a_track_mix_edit_reports_all_four_values() {
        assert_eq!(
            super::operation_status(&set_track_mix(3)),
            "Set mix on track 3 (gain -6.0 dB, pan 25, mute false, solo true)"
        );
        assert_eq!(
            super::operation_status(&super::Operation::SetTrackMix {
                track: super::TrackId(1),
                gain_tenth_db: 0,
                pan_percent: 0,
                mute: false,
                solo: false,
            }),
            "Set mix on track 1 (gain +0.0 dB, pan 0, mute false, solo false)"
        );
    }

    /// AU5 §3.7 rule 63: the three frame domains, derived and never written
    /// down.
    #[test]
    fn au5_the_learn_minimum_is_derived_per_rate() {
        use kinewright_core::Rational;
        let thirty = Rational::new(30, 1).unwrap();
        assert_eq!(super::noise_profile_minimum_frames(thirty), 15);
        assert_eq!(
            super::noise_profile_minimum_frames(Rational::new(24, 1).unwrap()),
            12
        );
        assert_eq!(
            super::noise_profile_minimum_frames(Rational::new(25, 1).unwrap()),
            12
        );
        assert_eq!(
            super::noise_profile_minimum_frames(Rational::new(60, 1).unwrap()),
            29
        );
        assert_eq!(
            super::noise_profile_minimum_frames(Rational::new(48_000, 1).unwrap()),
            super::NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES
        );
        // A drop-frame rate is exact rather than rounded to its nominal.
        assert_eq!(
            super::noise_profile_minimum_frames(Rational::new(30_000, 1_001).unwrap()),
            15
        );
    }

    /// AU5 §6.3 rule 127: the tracks feeding one chain.
    #[test]
    fn au5_the_learn_range_reads_the_tracks_feeding_the_chain() {
        use kinewright_core::{AudioBus, AudioBusId, AudioChain, Document, Track, TrackKind};
        let mut document = Document {
            tracks: vec![
                Track {
                    id: super::TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: super::TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            ..Document::default()
        };
        document.audio_mix.buses.push(AudioBus {
            id: AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![super::TrackId(2)],
            gain_tenth_db: 0,
            gain_curve: None,
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        });
        assert_eq!(
            super::tracks_feeding_chain(&document, AudioChain::Bus(AudioBusId(1))),
            vec![super::TrackId(2)],
            "a bus is fed by exactly the tracks it lists"
        );
        assert_eq!(
            super::tracks_feeding_chain(&document, AudioChain::Master),
            vec![super::TrackId(1), super::TrackId(2)],
            "the master is fed by every track"
        );
        assert!(
            super::tracks_feeding_chain(&document, AudioChain::Bus(AudioBusId(9))).is_empty(),
            "an unknown bus feeds nothing rather than panicking"
        );
    }

    /// AU5 §7 B15: one completed `Learn` is **one** operation under the
    /// chain's own coalesce key, and one `Undo` puts the document back.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn au5_one_learn_completion_is_one_operation_and_one_undo() {
        use kinewright_core::{
            AudioBus, AudioBusId, AudioChain, Command, Core, Document, Effect, EffectId, Event,
            NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES, ParamValue,
        };
        let mut denoise = Effect {
            id: EffectId(1),
            name: "audio_denoise".to_owned(),
            parameters: std::collections::BTreeMap::new(),
            keyframes: std::collections::BTreeMap::new(),
        };
        for (name, value) in [
            ("bypass", 0),
            ("reduction_tenth_db", 0),
            ("floor_offset_tenth_db", 0),
            ("smoothing_milliseconds", 50),
            ("lookahead_milliseconds", 12),
        ] {
            denoise
                .parameters
                .insert(name.to_owned(), ParamValue::Integer(value));
        }
        let mut document = Document {
            tracks: vec![kinewright_core::Track {
                id: super::TrackId(1),
                kind: kinewright_core::TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            ..Document::default()
        };
        document.audio_mix.buses.push(AudioBus {
            id: AudioBusId(3),
            name: "Dialogue".to_owned(),
            tracks: vec![super::TrackId(1)],
            gain_tenth_db: 0,
            gain_curve: None,
            effects: vec![denoise],
            ducking_sidechain_tracks: Vec::new(),
        });
        let original = document.clone();

        let mut bands = [-1_200; NOISE_PROFILE_BAND_COUNT];
        for (index, band) in bands.iter_mut().enumerate() {
            *band = -900 + i32::try_from(index).unwrap() * 10;
        }
        let operation = super::noise_profile_operation(
            &document,
            AudioChain::Bus(AudioBusId(3)),
            EffectId(1),
            &bands,
        )
        .expect("the node is on the chain");
        let super::Operation::UpsertAudioBus { bus } = &operation else {
            panic!("a learn writes one whole-chain set; it wrote {operation:?}");
        };
        let node = &bus.effects[0];
        for (index, name) in NOISE_PROFILE_PARAMETER_NAMES.iter().enumerate() {
            assert_eq!(
                node.parameters.get(*name),
                Some(&ParamValue::Integer(i64::from(bands[index]))),
                "all 31 or none: {name} is missing"
            );
        }
        assert_eq!(
            node.parameters.len(),
            5 + NOISE_PROFILE_BAND_COUNT,
            "the five controls survive the learn untouched"
        );
        assert_eq!(
            node.parameters.get("smoothing_milliseconds"),
            Some(&ParamValue::Integer(50))
        );

        assert_eq!(
            crate::mixer_ui::MixerSelection::Bus(AudioBusId(3)).coalesce_key(),
            "audio_bus:3"
        );
        assert_eq!(
            crate::mixer_ui::MixerSelection::Master.coalesce_key(),
            "audio_master"
        );

        let core = Core::spawn(original.clone()).unwrap();
        assert!(matches!(
            core.request(Command::DoBatchCoalesced {
                operations: vec![operation],
                coalesce_key: "audio_bus:3#7".to_owned(),
            })
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("undo returns the restored document");
        };
        assert_eq!(
            *doc, original,
            "one `Undo` restores the pre-gesture document"
        );

        document.audio_mix.buses[0].effects.clear();
        assert!(
            super::noise_profile_operation(
                &document,
                AudioChain::Bus(AudioBusId(3)),
                EffectId(1),
                &bands
            )
            .is_none()
        );
    }

    /// An `Analysis` whose `mix_noise_profile` blocks until its gate opens.
    ///
    /// The only way to have two learn requests in flight at once without a
    /// real engine, and the only thing the worker tests need from `Analysis`
    /// at all — every other method is the `NotImplemented` shape the app's own
    /// stubs use.
    struct GatedAnalysis {
        gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl std::fmt::Debug for GatedAnalysis {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GatedAnalysis")
        }
    }
    impl kinewright_core::Analysis for GatedAnalysis {
        fn probe(
            &self,
            _path: &std::path::Path,
        ) -> Result<kinewright_core::MediaAsset, super::MediaError> {
            Err(super::MediaError::NotImplemented)
        }
        fn thumbnail_at(
            &self,
            _at: super::TimeCode,
            _max_width: u32,
        ) -> Result<kinewright_core::RgbaImage, super::MediaError> {
            Err(super::MediaError::NotImplemented)
        }
        fn timeline_transcript(
            &self,
            _document: &kinewright_core::Document,
            _range: Option<std::ops::Range<super::TimeCode>>,
        ) -> Result<Vec<kinewright_core::TimelineTranscriptWord>, super::MediaError> {
            Ok(Vec::new())
        }
        fn request_waveform(&self, _asset: kinewright_core::MediaAsset, _generation: u64) -> bool {
            false
        }
        fn request_thumbnail(
            &self,
            _asset: kinewright_core::MediaAsset,
            _source_at: super::TimeCode,
            _max_width: u32,
            _generation: u64,
        ) -> bool {
            false
        }
        fn visual_asset_results(
            &self,
        ) -> crossbeam_channel::Receiver<kinewright_core::VisualAssetResult> {
            crossbeam_channel::bounded(0).1
        }
        fn request_transcription(&self, _asset: kinewright_core::MediaAsset) {}
        fn transcript_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::TranscriptStatus {
            kinewright_core::TranscriptStatus::NotRequested
        }
        fn request_silence_detection(&self, _asset: kinewright_core::MediaAsset) {}
        fn silence_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SilenceStatus {
            kinewright_core::SilenceStatus::NotRequested
        }
        fn timeline_silences(
            &self,
            _document: &kinewright_core::Document,
            _range: Option<std::ops::Range<super::TimeCode>>,
            _minimum_source_frames: super::TimeCode,
        ) -> Result<Vec<kinewright_core::TimelineSilenceSpan>, super::MediaError> {
            Ok(Vec::new())
        }
        fn request_scene_detection(&self, _asset: kinewright_core::MediaAsset) {}
        fn scene_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SceneStatus {
            kinewright_core::SceneStatus::NotRequested
        }
        fn timeline_scene_changes(
            &self,
            _document: &kinewright_core::Document,
            _range: Option<std::ops::Range<super::TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<kinewright_core::TimelineSceneChange>, super::MediaError> {
            Ok(Vec::new())
        }
        fn mix_noise_profile(
            &self,
            _document: &kinewright_core::Document,
            _request: &super::MixNoiseProfileRequest,
        ) -> Result<kinewright_core::NoiseProfileReport, super::MediaError> {
            while !self.gate.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok(kinewright_core::NoiseProfileReport {
                range: super::TimeCode::ZERO..super::TimeCode(30),
                point: super::MixSpectrumPoint::Master,
                sample_rate: 48_000,
                sample_frames: 22_528,
                windows: 10,
                bands: [-700; kinewright_core::NOISE_PROFILE_BAND_COUNT],
            })
        }
    }

    /// A gate that is already open, for a test whose worker never runs.
    fn refusing_analysis() -> std::sync::Arc<dyn kinewright_core::Analysis> {
        std::sync::Arc::new(GatedAnalysis {
            gate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })
    }

    /// AU5 §6.3 rule 127: latest request wins, and only the live generation
    /// lands.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn au5_the_learn_worker_is_single_flight() {
        use kinewright_core::{AudioBusId, AudioChain, Document, EffectId};
        use std::sync::Arc;

        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let analysis: Arc<dyn kinewright_core::Analysis> = Arc::new(GatedAnalysis {
            gate: Arc::clone(&gate),
        });
        let document = Arc::new(Document::default());
        let mut state = super::NoiseLearnState::default();
        let job = |chain| super::NoiseProfileJob {
            generation: 0,
            session: 1,
            chain,
            effect: EffectId(1),
            analysis: Arc::clone(&analysis),
            document: Arc::clone(&document),
            request: super::MixNoiseProfileRequest {
                range: Some(super::TimeCode::ZERO..super::TimeCode(30)),
                point: super::MixSpectrumPoint::Master,
            },
        };
        state.request(job(AudioChain::Master));
        assert!(state.is_pending());
        state.request(job(AudioChain::Bus(AudioBusId(2))));
        state.request(job(AudioChain::Bus(AudioBusId(5))));
        assert_eq!(
            state.spawned_workers(),
            1,
            "three requests, one worker at a time"
        );
        gate.store(true, std::sync::atomic::Ordering::Release);

        let mut landed = Vec::new();
        for _ in 0..2_000 {
            landed.extend(state.poll());
            if !landed.is_empty() && !state.is_pending() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(landed.len(), 1, "exactly one measurement lands");
        assert_eq!(landed[0].chain, AudioChain::Bus(AudioBusId(5)));
        assert_eq!(state.spawned_workers(), 2, "the parked request ran second");
        assert!(!state.is_pending());
    }

    /// AU5 §0 R136: a worker that could not start says so.
    ///
    /// `spawn` retired the pending generation before it queued its own error
    /// response, and `poll` drops any response whose generation is not the
    /// pending one — so the one arm that reports a machine out of threads
    /// could never reach the error log, and the click looked like it did
    /// nothing.
    #[test]
    fn au5_a_learn_worker_that_cannot_start_reports_itself() {
        use kinewright_core::{AudioBusId, AudioChain, Document, EffectId};
        use std::sync::Arc;

        let analysis = refusing_analysis();
        let document = Arc::new(Document::default());
        let mut state = super::NoiseLearnState::default().refusing_next_spawn();
        state.request(super::NoiseProfileJob {
            generation: 0,
            session: 1,
            chain: AudioChain::Bus(AudioBusId(4)),
            effect: EffectId(2),
            analysis,
            document,
            request: super::MixNoiseProfileRequest {
                range: Some(super::TimeCode::ZERO..super::TimeCode(30)),
                point: super::MixSpectrumPoint::Master,
            },
        });
        assert!(
            state.is_pending(),
            "the request is still the live one until its answer is read"
        );
        assert_eq!(state.spawned_workers(), 0, "no thread ever started");

        let landed = state.poll();
        assert_eq!(landed.len(), 1, "the failure is delivered, not swallowed");
        assert_eq!(landed[0].chain, AudioChain::Bus(AudioBusId(4)));
        assert_eq!(landed[0].effect, EffectId(2));
        assert_eq!(
            landed[0].result.as_ref().err().map(String::as_str),
            Some(super::LEARN_WORKER_UNAVAILABLE),
            "and it names the failure the editor is shown"
        );
        assert!(
            !state.is_pending(),
            "and the generation retires once its answer has been read"
        );
    }
}
