use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use kinewright_core::{
    Analysis, AssetId, AudioChain, ClipContent, Command, CommandToken, Document, Effect, EffectId,
    Event, Export, Incident, IncidentCode, IncidentId, IncidentObservation, IncidentOutcome,
    IncidentSubject, InvestigatorPreferences, JournalCommand, LabelIncident, LiveAudioChange,
    MediaAsset, MediaError, MediaEvent, MixNoiseProfileRequest, MixSpectrumPoint,
    NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES, Observed, Operation,
    PROJECT_FORMAT_VERSION, ParamValue, Playback, PlaybackState, PolicyClass, Rational,
    RecoveryKind, SilenceStatus, TimeCode, TimelineRevision, Track, TrackId, TrackKind,
    recovery_description,
};
use kinewright_media::{FfmpegMediaEngine, GpuContext, compositor_required_limits};

use crate::{
    error_ui::ErrorLog,
    export_ui::{ExportDialog, ExportJob},
    icons::Icon,
    media_workflow::media_asset_requires_refresh,
    mixer_pane_ui::{LEARN_NO_SILENCE, NoiseLearnRange},
    project::{
        ProjectFile, ProjectSaveError, ProjectSaveReport, ProjectSession, can_overwrite_save,
        canonical_session_key, derive_lut_store, focus_publishes_lut_library, index_after_close,
        project_name, project_newer_format_observation, serialize_project_document,
        session_index_by_id, write_file_atomic, write_project_bytes,
    },
    recovery::{RestoreRequest, recovery_damage_observation, recovery_unavailable_observation},
    sidecar::{
        SidecarMode, SidecarWriter, digest_bytes, sidecar_path_for_project,
        sidecar_write_failed_observation,
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

/// One `Event::RevisionConflict` the core drain noted for the incident router
/// (IN1 §5.2b rule 16): the project whose core refused, the revision core
/// reported as current, and the identity of the send it refused.
///
/// `expected` is **not** carried. Once the match is by identity
/// (`IN1b` §4 rule 6) nothing reads it, and a field nothing reads is a
/// `dead_code` build error under the house `-D warnings` gate.
struct RouterConflict {
    project_index: usize,
    /// The revision core reported as current when it refused. It is the
    /// revision the matched incident is refreshed to (IN1 §5.2 rule 10): the
    /// only evidence the router holds about where the document actually is,
    /// carried from the event rather than re-read from the live project so the
    /// refresh is the revision core refused against and is deterministic.
    actual: TimelineRevision,
    /// The token the refused command carried, echoed by the actor
    /// (`IN1b` §4 rule 5). `None` means the send was not the router's — a
    /// foreign `Command::DoIfRevision` that does not correlate — and
    /// `reconcile_router_sends` then touches no outstanding send at all.
    token: Option<CommandToken>,
}

/// Who asked for the recovery a [`RouterApply`] carries (erratum
/// `IN1b`-C-R54).
///
/// The two skips errata `IN1b`-C-R51 and `IN1b`-C-R53 introduce argue from the
/// same sentence — *"an edit Kinewright planned and sent"* — and that sentence
/// is true of exactly one of the two origins. An auto-apply is Kinewright's
/// own, and a card about its refusal tells the person to re-make an edit they
/// never made. A card **press** is theirs: they pressed a button, and if the
/// send loses a race they have to be told, or the button does nothing at all
/// with no card, no status line and no audit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouterSendOrigin {
    /// The router's own auto-apply, from `audit_new_incident`.
    Auto,
    /// A control the person pressed, through `send_incident_recovery`.
    CardPress,
}

/// One incident recovery the router sent and is still waiting to hear back
/// about: an auto-apply, or a card action the person pressed (IN1 §5.2
/// rules 9-10, §5.4 rule 34).
///
/// `expected` is **not** carried either. Part A matched a conflict by
/// `(project, expected)` and dropped a send whose project had moved past it;
/// after `IN1b` §4 rule 6's identity match and erratum `IN1b`-C-R50's landing
/// token, neither decision reads the revision the send was gated on, and a
/// field nothing reads is a `dead_code` build error under the house
/// `-D warnings` gate. The revision itself is not lost: it is the incident's
/// own `revision` at send time, which is what the card and the agent read.
struct RouterApply {
    project_index: usize,
    incident: IncidentId,
    outcome: IncidentOutcome,
    /// The token this send carried. Not an `Option`: every router send
    /// allocates one from `KinewrightApp::next_command_token`, which is what
    /// makes the conflict match an identity rather than a heuristic
    /// (`IN1b` §4 rule 6).
    token: CommandToken,
    /// Who asked for it (erratum `IN1b`-C-R54).
    origin: RouterSendOrigin,
}

/// Appendix B row 1: the observation a failed startup open carries.
///
/// Both `load_error` producers wrote the literal `"Project"` (`IN1b` §5.4
/// rule 27, N1.5 §5), so the "dynamic" site is not dynamic in substance and
/// the code is decided here, at the producer, rather than at the sink. The
/// revision is the default: the session this describes does not exist yet, and
/// the document that replaces it is a fresh one.
fn startup_project_observation(observed: String) -> IncidentObservation {
    IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::Project),
        IncidentSubject::Project,
        observed,
        TimelineRevision::default(),
    )
}

/// The subject a refused **edit plan** is about (`IN1b` §3.3 rule 20).
///
/// The common subject when every operation in the batch agrees, and `Project`
/// otherwise: a plan that touches one clip is that clip's problem, and a plan
/// that touches four tracks is the document's. The fold lives here rather than
/// in core because `IN1b` §14 row A declares only
/// [`Operation::incident_subject`], which answers for one operation.
#[must_use]
pub(crate) fn batch_incident_subject(operations: &[Operation]) -> IncidentSubject {
    let mut subjects = operations.iter().map(Operation::incident_subject);
    match subjects.next() {
        Some(first) if subjects.all(|subject| subject == first) => first,
        _ => IncidentSubject::Project,
    }
}

/// Whether the live document shows a router-sent recovery as landed, read the
/// way `resolve_incident` verifies a claim (IN1 §6.3 rule 16): from the
/// document, never from an event the drain consumed.
///
/// An auto-apply landed exactly when the asset carries the recovery bytes —
/// nothing else in the app writes `AgentAssumption` to this core. A revert
/// landed exactly when the asset carries the incident's probed bytes with no
/// live assumption. Anything else, including a missing asset, reads as not
/// landed.
fn router_apply_accepted(
    document: &Document,
    incident: &Incident,
    outcome: IncidentOutcome,
) -> bool {
    // `IN1b` §3.8 breaks 2 and 3: `IncidentSubject` has seven variants and
    // `probed()` is an `Option` after Part B. A non-asset subject has no colour
    // recovery to land, and evidence with no probed description has nothing to
    // compare against, so both read as not landed.
    let IncidentSubject::Asset(asset_id) = incident.subject else {
        return false;
    };
    let Some(probed) = incident.evidence.probed() else {
        return false;
    };
    let Some(asset) = document.asset(asset_id) else {
        return false;
    };
    match outcome {
        IncidentOutcome::Applied => asset.color_description == recovery_description(probed),
        IncidentOutcome::Reverted => {
            asset.color_description == *probed && asset.assumed_from.is_none()
        }
        // Neither outcome lands a colour recovery: `Explained` applied
        // nothing, and `Rejected` is the person refusing an investigator
        // session's proposal (IN2 §4.5 rule 20).
        IncidentOutcome::Explained | IncidentOutcome::Rejected => false,
    }
}

// Independent transport, agent, dialog, and window flags model separate UI state machines.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct KinewrightApp {
    pub(crate) projects: Vec<ProjectSession>,
    pub(crate) focused_project: usize,
    next_project_id: u64,
    /// The counter every router `Command::DoIfRevision` allocates its
    /// correlation token from (`IN1b` §4 rule 6).
    next_command_token: u64,
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
    /// Per-harness detection, model catalog, and remembered picks, indexed
    /// by [`AgentHarnessChoice::index`](crate::chat_ui::AgentHarnessChoice::index).
    pub(crate) harness: [crate::chat_ui::HarnessUiState; crate::chat_ui::HARNESS_COUNT],
    /// Detection results and model catalogs arriving from the background
    /// harness-probe thread. `None` in tests, where no probe runs, so every
    /// harness stays undetected with an empty catalog.
    pub(crate) harness_update_rx:
        Option<crossbeam_channel::Receiver<crate::chat_ui::HarnessUpdate>>,
    pub(crate) show_thread_rail: bool,
    pub(crate) settings_open: bool,
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
    /// IN1 §5.2: playback observations waiting for the next `route_incidents`
    /// tick, which attributes them to the focused project.
    pub(crate) pending_observations: Vec<IncidentObservation>,
    /// Site 5's edge (`IN2B` §7 rule 3): the last noted transcription
    /// failure, keyed by asset — the engine owns the status store, so the
    /// app edges at the read. Keyed (not site 6's bare `Option<String>`)
    /// because the panel switches assets: a bare message would swallow a
    /// second asset's identical failure.
    pub(crate) transcript_noted: Option<(AssetId, String)>,
    /// Modal 1's edge (`IN2B` §7 rule 6): the damage note fires on the
    /// first damage render per run, never again.
    pub(crate) recovery_damage_noted: bool,
    /// Modal 2's edge (`IN2B` §7 rule 6): the unavailable note fires on
    /// the first message render per run, never again.
    pub(crate) recovery_unavailable_noted: bool,
    /// IN1 §5.2b: revision conflicts the core drain noted this frame for the
    /// router to reconcile.
    pending_router_conflicts: Vec<RouterConflict>,
    /// The tokens of router sends the actor has answered with an
    /// `Event::DocumentChanged` (erratum `IN1b`-C-R50, `IN1b` §4 rule 9).
    ///
    /// A send is recognised as landed only when **its own** token has come
    /// back. Without it, a person who wrote the recovery's bytes by hand
    /// between the send and the actor's reply makes `router_apply_accepted`
    /// read `true` for a command the actor went on to refuse, and the incident
    /// records `Applied` off the person's edit; the conflict that arrives a
    /// frame later matches nothing and cannot correct the record. The document
    /// read stays as the second condition, which §4 rule 9 forbids removing.
    pending_router_landings: Vec<CommandToken>,
    /// IN1 §5.2/§5.4: incident recoveries the router sent and is still
    /// waiting to hear back about.
    pending_router_applies: Vec<RouterApply>,
    /// The ONE sidecar writer thread, shared by every project session
    /// (`IN2B` §2 rule 5, N2/B-5).
    pub(crate) sidecar_writer: Arc<SidecarWriter>,
    /// When the debounced background sidecar flush last submitted (`IN2B` §2
    /// rule 4): at most 2 s after the last log change.
    sidecar_last_submit: Instant,
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
    /// Whether the badge-anchored Incidents panel is showing
    /// (`IN1b` §5.5 rule 31).
    pub(crate) incidents_open: bool,
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
    performance: Option<crate::performance::PerformanceProbe>,
}

/// N6/H12 rollback, split from [`KinewrightApp::write_project`]: the sidecar
/// flushed above the failed project write pairs with bytes that never
/// landed — restore the prior bytes, or remove the sidecar when none
/// preceded the save. Best-effort: the save already failed, and the pair's
/// previous arm still loads a newer-than-project sidecar, so a failed
/// rollback degrades to a re-flush, not a refusal.
fn rollback_sidecar_write(sidecar: Option<PathBuf>, bytes: Option<Vec<u8>>) {
    if let Some(sidecar) = sidecar {
        match bytes {
            Some(bytes) => {
                let _ = write_file_atomic(&sidecar, &bytes);
            }
            None => {
                let _ = fs::remove_file(sidecar);
            }
        }
    }
}

impl KinewrightApp {
    // Construction keeps all channel subscriptions and coupled UI state initialization together.
    #[allow(clippy::too_many_lines)]
    fn new(media: Arc<FfmpegMediaEngine>, startup_path: Option<PathBuf>) -> Self {
        let mut load_error = None;
        let mut newer_note = None;
        let (name, document, project_path, format_version, project_digest) = match startup_path {
            Some(path) if path.is_file() => match load_document(&path) {
                Ok((document, version, digest)) => {
                    // `IN2B` §4 rule 4, startup arm: a newer file opens; the
                    // note queues below, beside `load_error`.
                    if version > PROJECT_FORMAT_VERSION {
                        newer_note = Some(project_newer_format_observation(
                            version,
                            TimelineRevision::default(),
                        ));
                    }
                    let name = project_name(Some(&path), "Project 1");
                    (name, document, Some(path), version, digest)
                }
                // `IN1b` §5.4 rule 27: the first dynamic site is not dynamic
                // in substance — both producers wrote the literal `"Project"` —
                // so `load_error` carries the observation itself and the single
                // consumer only queues it. Appendix B row 1.
                Err(error) => {
                    load_error = Some(startup_project_observation(format!(
                        "Could not open {}: {error}",
                        path.display()
                    )));
                    (
                        "Project 1".to_owned(),
                        default_project_document(),
                        None,
                        PROJECT_FORMAT_VERSION,
                        String::new(),
                    )
                }
            },
            Some(path) => {
                load_error = Some(startup_project_observation(format!(
                    "Startup project not found: {}",
                    path.display()
                )));
                (
                    "Project 1".to_owned(),
                    default_project_document(),
                    None,
                    PROJECT_FORMAT_VERSION,
                    String::new(),
                )
            }
            None => (
                "Project 1".to_owned(),
                default_project_document(),
                None,
                PROJECT_FORMAT_VERSION,
                String::new(),
            ),
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
        let sidecar_writer = SidecarWriter::new();
        let sidecar_mode = if project_path.is_some() {
            SidecarMode::Load { project_digest }
        } else {
            SidecarMode::None
        };
        let mut project = ProjectSession::create(
            1,
            name,
            document.clone(),
            project_path.clone(),
            &playback,
            &analysis,
            &exporter,
            &sidecar_mode,
            Some(Arc::clone(&sidecar_writer)),
            format_version,
            None,
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
        let harness = std::array::from_fn(|_| crate::chat_ui::HarnessUiState::default());
        // Detection and model catalogs both spawn the harness CLIs, so both
        // run off the frame. Detecting ten harnesses took about three
        // seconds of the startup path on a machine with all ten installed,
        // and `devin auth status` is a network round trip, so an unreachable
        // endpoint held the first frame with no window to show for it.
        let (harness_update_tx, harness_update_rx) = crossbeam_channel::unbounded();
        let harness_update_rx = std::thread::Builder::new()
            .name("kinewright-harness-probes".to_owned())
            .spawn(move || crate::chat_ui::probe_harnesses(&harness_update_tx))
            .map(|_| harness_update_rx)
            .ok();
        let resolution = document.resolution;
        let fps = document.fps;
        let error_log_open = error_log.len() > 0;
        let mut app = Self {
            projects: vec![project],
            focused_project: 0,
            next_project_id: 2,
            next_command_token: 1,
            playback,
            analysis,
            exporter,
            lut_publisher,
            frames,
            media_events,
            visual_cache,
            harness,
            harness_update_rx,
            show_thread_rail: true,
            settings_open: matches!(
                std::env::var("KINEWRIGHT_SCREENSHOT_SHOW").as_deref(),
                Ok("settings")
            ),
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
            pending_observations: Vec::new(),
            transcript_noted: None,
            recovery_damage_noted: false,
            recovery_unavailable_noted: false,
            pending_router_conflicts: Vec::new(),
            pending_router_landings: Vec::new(),
            pending_router_applies: Vec::new(),
            sidecar_writer,
            sidecar_last_submit: Instant::now(),
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
            incidents_open: false,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
            recording: None,
            record_dialog: crate::recording::RecordDialog::default(),
            edit_gesture: 0,
            look_ab_hold: None,
            look_ab_hold_seen: false,
            performance: None,
        };
        app.publish_focused_lut_library();
        app.playback
            .set_document(Arc::clone(&app.focused().document));
        app.playback.request_frame(TimeCode::ZERO);
        // The investigator settings file is read once at startup (IN2 §2.1
        // rule 6); a malformed file opens one incident and is left alone
        // until the person changes a setting (rule 3).
        let (settings_path, settings_durable) = crate::investigator::investigator_config_path();
        let settings_load = crate::investigator::load_investigator_settings_for_run(
            &settings_path,
            settings_durable,
        );
        crate::investigator::apply_settings_load_for_run(&mut app, settings_load, settings_durable);
        let opened_path = app.focused().project_path.clone();
        // The startup newer-file arm (§4 rule 4): the project opened above,
        // exactly one incident notes it. Queued ahead of the media check so
        // a newer file still gets both.
        if let Some(observation) = newer_note {
            app.note_observation(observation);
        }
        if let Some(observation) = load_error {
            app.note_observation(observation);
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
                // Appendix B row 2: `media_incomplete`, because the project
                // **did** open — only some of its media is elsewhere.
                // N6/H8: transient, like File → Open — a per-run note must
                // never persist `Open` against a later open.
                app.note_transient_label(
                    LabelIncident::MediaIncomplete,
                    IncidentSubject::Project,
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
        sidecar_mode: &SidecarMode,
        format_version: u32,
    ) -> Result<ProjectSession, String> {
        let id = self.next_project_id;
        self.next_project_id = self
            .next_project_id
            .checked_add(1)
            .ok_or_else(|| "project session identity space is exhausted".to_owned())?;
        let writer = Arc::clone(&self.sidecar_writer);
        let mut session = ProjectSession::create(
            id,
            name,
            document,
            project_path,
            &self.playback,
            &self.analysis,
            &self.exporter,
            sidecar_mode,
            Some(writer),
            format_version,
            None,
        )?;
        // The investigator settings are app-wide: a new project inherits the
        // focused copy rather than re-reading the file. Mutes stay
        // per-project, from the opened document (IN2 §2.1, §2.4).
        if let Some(focused) = self.projects.get(self.focused_project)
            && let Some(source) = focused.investigator.as_ref()
            && let Some(target) = session.investigator.as_mut()
        {
            target.settings = source.settings.clone();
            target.settings_durable = source.settings_durable;
        }
        Ok(session)
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
    /// Returns the typed serialize or write failure, or the newer-format
    /// refusal (`IN2B` §4 rule 3 — no bytes written at all, and the caller
    /// re-surfaces the rule-4 card instead of noting). A store-root refusal is
    /// not fatal: the project is saved and its imported looks report `missing`
    /// until the root is usable.
    pub(crate) fn write_project(
        &mut self,
        path: &Path,
    ) -> Result<ProjectSaveReport, ProjectSaveError> {
        let save_as = self.focused().project_path.as_deref() != Some(path);
        // N6/H5: Save As onto a path another session holds open is refused
        // before any IO — the target's history is not ours to replace. The
        // focused session cannot match: Save As always names a new path.
        if save_as && self.session_index_for_path(path).is_some() {
            return Err(ProjectSaveError::PathOpenElsewhere {
                notice: format!(
                    "Save As refused: {} is open in another session.",
                    path.display()
                ),
            });
        }
        // `IN2B` §4 rule 3: a newer-format session never overwrites its own
        // file — checked before any IO, so a refused save writes neither
        // file. Save As stays enabled.
        if !save_as && !can_overwrite_save(self.focused().format_version) {
            return Err(ProjectSaveError::NewerFormat {
                read_version: self.focused().format_version,
            });
        }
        // N6/H2: a suspended session, or a path open in another session,
        // routes overwrite-save to Save As — checked before any IO, like
        // the newer-format gate above.
        if !save_as && let Some(notice) = self.overwrite_save_refusal() {
            return Err(ProjectSaveError::SaveAsRequired { notice });
        }
        let document = Arc::clone(&self.focused().document);
        let previous_store = self.focused().lut_store.clone();
        // The live investigator mutes travel in the saved bytes (IN2 §2.4).
        // Preserve the original document only when the live list is exactly
        // what the document already carries; removing the last mute must be a
        // real save rather than an accidental write of the old snapshot.
        let mutes = self
            .focused()
            .investigator
            .as_ref()
            .map(|session| session.muted_codes().to_vec())
            .unwrap_or_default();
        let document_mutes = document
            .investigator
            .as_ref()
            .map(|preferences| preferences.muted_codes.clone())
            .unwrap_or_default();
        let snapshot;
        let to_write = if mutes == document_mutes {
            document.as_ref()
        } else {
            let mut injected = (*document).clone();
            injected.investigator =
                (!mutes.is_empty()).then_some(InvestigatorPreferences { muted_codes: mutes });
            snapshot = injected;
            &snapshot
        };
        // `IN2B` §2 rule 9: the sidecar lands *before* the project bytes,
        // carrying the digest the project file is about to have plus the one
        // it had. One serialisation serves both files (§4 rule 2): the JSON
        // below is what `write_project_bytes` writes and what the digest
        // covers.
        let json = serialize_project_document(to_write)?;
        let new_digest = digest_bytes(json.as_bytes());
        if save_as {
            self.drop_newer_format_note_for_save_as();
        }
        // Same sidecar stem, same previous save; a new stem has no previous.
        let previous_digest = if sidecar_path_for_project(self.focused().project_path.as_deref())
            == sidecar_path_for_project(Some(path))
        {
            self.focused().saved_digest.clone()
        } else {
            String::new()
        };
        // The session takes the new path BEFORE the sidecar flush, so the
        // flush derives the new stem: a Save As must carry the live log to
        // the new sidecar (leaving the old one in place), and a first save
        // must write a sidecar at all. Restored below if the project write
        // fails, so a failed save claims no path. The suspension lifts here
        // too: a recovery restore onto an open path writes no sidecar
        // *until Save As* (§2 rule 10, N2/S-13) — the Save As flush below is
        // the first write the new stem is owed, and `flush_incidents`
        // skips while suspended.
        let old_path = self.focused().project_path.clone();
        let was_suspended = self.focused().sidecar_suspended;
        self.focused_mut().project_path = Some(path.to_path_buf());
        if save_as {
            self.focused_mut().sidecar_suspended = false;
        }
        // N6/H12: the sidecar lands before the project bytes — snapshot it
        // first, so a failed project write rolls the sidecar back instead
        // of leaving it paired with bytes that never landed.
        let rollback_sidecar = sidecar_path_for_project(self.focused().project_path.as_deref());
        let rollback_bytes = rollback_sidecar
            .as_deref()
            .and_then(|sidecar| fs::read(sidecar).ok());
        // `IN2B` §2 rule 6: a sidecar write failure never fails the project
        // save — exactly one incident, then the save carries on.
        if let Err(error) = self
            .focused_mut()
            .flush_incidents(&new_digest, &previous_digest)
        {
            let revision = self.focused().revision;
            self.note_observation(sidecar_write_failed_observation(
                error.to_string(),
                revision,
            ));
        }
        let report = match write_project_bytes(&json, to_write, path, previous_store.as_ref()) {
            Ok(report) => report,
            Err(error) => {
                rollback_sidecar_write(rollback_sidecar, rollback_bytes);
                self.focused_mut().project_path = old_path;
                self.focused_mut().sidecar_suspended = was_suspended;
                return Err(error);
            }
        };
        debug_assert_eq!(
            report.digest, new_digest,
            "one serialisation serves both files"
        );
        self.adopt_saved_path(path, &new_digest, save_as);
        if let Some(reason) = &report.lut_store_error {
            // Appendix B row 3: the project **was** saved.
            self.note_label(
                LabelIncident::LookIncomplete,
                IncidentSubject::Project,
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
                    // Appendix B row 4.
                    self.note_label(
                        LabelIncident::LookIncomplete,
                        IncidentSubject::Project,
                        format!(
                            "Saved {} but could not copy every look into the new store — {summary}",
                            report.path.display()
                        ),
                    );
                }
                true
            }
            // `IN2B` §4 rule 3: a refused overwrite-save re-surfaces the
            // rule-4 card — no second incident, exactly one per newer file.
            Err(ProjectSaveError::NewerFormat { .. }) => {
                self.resurface_newer_format_card();
                false
            }
            // N6/H2, H5: a routed or refused save posts its notice — a
            // refusal, not an incident.
            Err(
                ProjectSaveError::SaveAsRequired { notice }
                | ProjectSaveError::PathOpenElsewhere { notice },
            ) => {
                notice.clone_into(&mut self.status);
                false
            }
            // Appendix B row 5: the one typed `Project` site.
            Err(error) => {
                let revision = self.focused().revision;
                self.note_observation(
                    error.incident_observation(IncidentSubject::Project, revision),
                );
                false
            }
        }
    }

    /// The project bytes landed: the session adopts the saved path,
    /// digest, and store, resets the read version on Save As (the bytes on
    /// the new path are this build's — `IN2B` §4 rule 3), and checkpoints
    /// the recovery journal onto the new baseline.
    fn adopt_saved_path(&mut self, path: &Path, new_digest: &str, save_as: bool) {
        let name = project_name(Some(path), &self.focused().name);
        let (store, store_error) = match derive_lut_store(Some(path)) {
            Ok(store) => (store, None),
            Err(reason) => (None, Some(reason)),
        };
        let session = self.focused_mut();
        session.name = name;
        session.project_path = Some(path.to_path_buf());
        new_digest.clone_into(&mut session.saved_digest);
        if save_as {
            session.format_version = PROJECT_FORMAT_VERSION;
        }
        session.set_lut_store(store, store_error);
        session.saved_document = Some(Arc::clone(&session.document));
        if let Some(investigator) = session.investigator.as_mut() {
            investigator.note_mutes_saved();
        }
        let library = session.rebuild_lut_library();
        session.publish_project_path_to_agents();
        let core = session.core.clone();
        session
            .recovery
            .checkpoint(&core, session.project_path.as_deref());
        self.lut_publisher.set_lut_library(library);
    }

    /// N2/B-4: the newer-format note was about the old path's bytes — a
    /// live-log removal, not a resolution, before the fresh sidecar is
    /// written so the new stem carries no false card.
    fn drop_newer_format_note_for_save_as(&mut self) {
        let incidents = Arc::clone(&self.focused().incidents);
        let mut log = incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = log.remove_open_with_code(IncidentCode::Label(LabelIncident::ProjectNewerFormat));
    }

    /// Re-surface the newer-format card after a refused overwrite-save
    /// (`IN2B` §4 rule 3): the Incidents panel opens on the rule-4 incident
    /// — no new incident, no dialog, no toast.
    ///
    /// The panel holds no per-card selection state, so opening it *is*
    /// selecting the card: the newer-format session's card is the one that
    /// explains the refusal.
    fn resurface_newer_format_card(&mut self) {
        self.incidents_open = true;
        self.status = format!(
            "Save disabled — this project was written by a newer Kinewright (format_version {}); use Save As",
            self.focused().format_version
        );
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
        let session = match self.create_project_session(
            name,
            default_project_document(),
            None,
            &SidecarMode::None,
            PROJECT_FORMAT_VERSION,
        ) {
            Ok(session) => session,
            Err(error) => {
                // Appendix B row 6.
                self.note_label(
                    LabelIncident::Project,
                    IncidentSubject::Project,
                    format!("Could not create a new project: {error}"),
                );
                return;
            }
        };
        self.projects.push(session);
        self.focus_project(self.projects.len() - 1);
        "Ready".clone_into(&mut self.status);
    }

    /// The live session already holding `path`, if any (N2/S-13): the
    /// one-session-per-path guard reads the live sessions only, so closing
    /// frees it. Both spellings canonicalise, so `.`/`..` segments and
    /// symlinks still match.
    fn session_index_for_path(&self, path: &Path) -> Option<usize> {
        let key = canonical_session_key(path);
        self.projects.iter().position(|session| {
            session
                .project_path
                .as_deref()
                .is_some_and(|open| canonical_session_key(open) == key)
        })
    }

    /// Whether a session other than `project_index` holds `path` open
    /// (N6/H2, H5): recovery restores are exempt from one-session-per-path,
    /// so a duplicate can exist, and neither side may then overwrite. Takes
    /// the index because the close prompt renders before it focuses.
    fn another_session_has_path_for(&self, project_index: usize, path: &Path) -> bool {
        let key = canonical_session_key(path);
        self.projects.iter().enumerate().any(|(index, session)| {
            index != project_index
                && session
                    .project_path
                    .as_deref()
                    .is_some_and(|open| canonical_session_key(open) == key)
        })
    }

    /// The status notice when the focused session must Save As instead of
    /// overwriting (N6/H2), `None` when overwrite-save is allowed: suspended
    /// sessions and paths open in another session route to Save As, as the
    /// newer-format gate does.
    fn overwrite_save_refusal(&self) -> Option<String> {
        self.overwrite_save_refusal_for(self.focused_project)
    }

    /// [`Self::overwrite_save_refusal`] for one session.
    fn overwrite_save_refusal_for(&self, project_index: usize) -> Option<String> {
        let session = &self.projects[project_index];
        if session.sidecar_suspended {
            return Some(
                "Saving is disabled for this recovered session until it has its own \
                 file — use Save As."
                    .to_owned(),
            );
        }
        if session
            .project_path
            .as_deref()
            .is_some_and(|path| self.another_session_has_path_for(project_index, path))
        {
            return Some(
                "Saving is disabled: this project is open in another session — \
                 use Save As."
                    .to_owned(),
            );
        }
        None
    }

    /// A second open focuses the live session instead of duplicating it
    /// (N2/S-13). Returns whether `path` was already open.
    fn refocus_if_open(&mut self, path: &Path) -> bool {
        if let Some(index) = self.session_index_for_path(path) {
            self.focus_project(index);
            self.publish_focused_lut_library();
            self.status = format!("Already open: {}", path.display());
            return true;
        }
        false
    }

    fn open_project(&mut self, path: &Path) {
        if self.refocus_if_open(path) {
            return;
        }
        let (document, format_version, project_digest) = match load_document(path) {
            Ok(triple) => triple,
            // Appendix B row 7.
            Err(error) => {
                self.note_label(
                    LabelIncident::Project,
                    IncidentSubject::Project,
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
        let mut session = match self.create_project_session(
            name,
            document,
            Some(path.to_path_buf()),
            &SidecarMode::Load { project_digest },
            format_version,
        ) {
            Ok(session) => session,
            // Appendix B row 8.
            Err(error) => {
                self.note_label(
                    LabelIncident::Project,
                    IncidentSubject::Project,
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
        // `IN2B` §4 rule 4: a newer file opens — exactly one incident notes
        // it, queued after the focus so it attributes to the new session.
        if format_version > PROJECT_FORMAT_VERSION {
            let revision = self.focused().revision;
            self.note_observation(project_newer_format_observation(format_version, revision));
        }
        if let Some(reason) = store_refusal {
            // Appendix B row 9: the session is pushed and focused *before*
            // this refusal, so the project opened — a degraded result.
            self.note_transient_label(
                LabelIncident::LookIncomplete,
                IncidentSubject::Project,
                format!(
                    "The LUT store beside {} is unusable: {reason}",
                    path.display()
                ),
            );
        }
        if !lut_titles.is_empty() {
            // Appendix B row 10, the first `IN1b` §5.1 rule 14 aggregate:
            // **one** incident whose `observed` is the joined list, because one
            // open with *n* unavailable looks is one problem. It used to push
            // straight into a window it never opened, so the person saw only
            // the status line.
            self.note_transient_label(
                LabelIncident::LookIncomplete,
                IncidentSubject::Project,
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
        // Appendix B row 11, the second aggregate. The push is **lifted out**
        // of the `self.status = if …` expression before the status line is
        // composed (`IN1b` §5.1 rule 14), because an observation queued inside
        // the expression that also writes `status` is a write the router then
        // overwrites in the same frame.
        if !missing.is_empty() {
            self.note_transient_label(
                LabelIncident::MediaIncomplete,
                IncidentSubject::Project,
                format!("Missing media after open: {}", missing.join(", ")),
            );
        }
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

    /// Apply a recovery restore the dialog offered (`IN2B` §4 rule 5).
    ///
    /// A cross-version journal is refused before its document is trusted: no
    /// session, the pending entry shelved (the file stays for a newer
    /// build), and exactly one `project_newer_format` incident. A
    /// same-version restore loads the saved sidecar digest-less
    /// (`SidecarMode::RecoveryNoDigest` — the recovered document is
    /// definitionally newer than the last save); when the target path is
    /// already open, the restored session suspends sidecar writes until Save
    /// As (§2 rule 10, N2/S-13 — recovery restore is exempt from
    /// one-session-per-path because the recovered document needs a home, so
    /// the suspension keeps it from clobbering the open session's history).
    fn apply_restore_request(&mut self, request: RestoreRequest) {
        if request.writer_format_version > PROJECT_FORMAT_VERSION {
            if let Some(first) = self.projects.first_mut() {
                first.recovery.shelve_pending(&request.journal_path);
            }
            let revision = self.focused().revision;
            self.note_observation(project_newer_format_observation(
                request.writer_format_version,
                revision,
            ));
            self.status = format!(
                "Recovery refused: the journal was written by a newer Kinewright (format_version {})",
                request.writer_format_version
            );
            return;
        }
        let journal_path = request.journal_path;
        let name = project_name(request.project_path.as_deref(), "Recovered project");
        let suspended = request
            .project_path
            .as_deref()
            .is_some_and(|path| self.session_index_for_path(path).is_some());
        let format_version = request.writer_format_version;
        let result = self
            .create_project_session(
                name,
                request.document,
                request.project_path,
                &SidecarMode::RecoveryNoDigest,
                format_version,
            )
            .map(|mut session| {
                session.sidecar_suspended = suspended;
                let assets = session.document.media_pool.clone();
                self.projects.push(session);
                self.focus_project(self.projects.len() - 1);
                self.queue_media_status_checks_for_project(self.focused_project);
                for asset in assets {
                    self.request_asset_analysis(asset);
                }
                self.projects[0].recovery.consume_pending(&journal_path);
            });
        // `IN1b` §5.7, Appendix B row 28: the third log-bypassing sink.
        // A failed restore used to be a bare status string; it is now a
        // `project_unclassified` incident whose written body tells the
        // person the last saved version is still intact.
        match crate::recovery::restore_status(result) {
            Ok(status) => self.status = status,
            Err(observation) => self.note_observation(*observation),
        }
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
        // `IN2B` §4 rule 3: the prompt offers Save As, not overwrite-save,
        // for a newer-format session — same keybinding, same position.
        // N6/H2 extends the offer to routed sessions.
        let can_save = can_overwrite_save(self.projects[project_index].format_version)
            && self.overwrite_save_refusal_for(project_index).is_none();
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
                            egui::Button::new(if can_save { "Save" } else { "Save As…" })
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
        if (save && self.save_project(!can_save)) || discard {
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
        // The operation names the narrowest subject the refusal is about, but
        // a stopped actor is not that operation's problem — it is the whole
        // document's, and a second stopped send must not open a second
        // incident (Appendix B row 12).
        if self.focused().core.send(Command::Do(operation)).is_err() {
            self.note_actor_stopped(
                IncidentSubject::Project,
                "Core actor stopped while applying the edit",
            );
        } else {
            "Applying edit…".clone_into(&mut self.status);
        }
    }

    /// IN1 §5.2/§5.2b: observe this frame's playback failures, audit newly
    /// opened incidents, auto-apply `AutoApply` recoveries under a revision
    /// gate, and reconcile the router's outstanding sends against the live
    /// document.
    ///
    /// Called once per `update`, after both event drains, and never from
    /// inside either (rule 16). Idempotent across frames with no new
    /// observations: a quiet frame sends no command and writes no log line
    /// (rule 20).
    ///
    /// Outstanding sends are reconciled *before* new incidents are audited,
    /// so only sends that predate this tick are ever judged: a command sent
    /// below has not reached the actor yet, and the live revision cannot tell
    /// a stale-but-in-flight send from a superseded one.
    ///
    /// ## Where the acceptance comes from
    ///
    /// The core drain consumes every `Event`, so by the time this runs the
    /// only acceptance signal left is the live document — which is also the
    /// more honest one. An outstanding send is recognised as landed when the
    /// document shows its bytes ([`router_apply_accepted`]), the same way
    /// `resolve_incident` verifies a claim (IN1 §6.3 rule 16).
    ///
    /// ## How the revision refresh works
    ///
    /// IN1 §5.2 rule 10 asks the router, on a conflict, to leave the incident
    /// `Open`, refresh its `revision`, and not re-send. The refresh runs in
    /// [`Self::reconcile_router_sends`]: `RouterConflict` carries `actual`,
    /// the revision core reported as current when it refused, and a conflict
    /// matched to an outstanding `RouterApply` calls
    /// `IncidentLog::refresh_revision(apply.incident, conflict.actual)` before
    /// dropping that send. `actual` is used rather than the live project
    /// revision because it is the evidence core itself handed back, and it is
    /// deterministic in tests.
    ///
    /// It is a dedicated writer rather than a re-observe because `observe`
    /// cannot do it: the router calls `note_auto_applied` at send time
    /// (rule 9), which suppresses the incident's `(code, subject)` pair, and
    /// `observe` returns `Observed::Suppressed` for a suppressed key before
    /// reaching the dedup arm that would refresh `revision` — rule 19 by
    /// design. Core therefore gained `refresh_revision` as its third narrowly
    /// typed writer beside `telemetry_mut` and `note_auto_applied`
    /// (§2.3 rule 20 erratum); it writes `revision` and nothing else, and only
    /// on an `Open` entry. Nothing is re-sent — a send happens only for a
    /// newly opened incident — and a card press always reads the live
    /// revision, so the person can still apply or revert from the card.
    ///
    /// `IN2B` §2 rule 4's background half: at most 2 s after the last log
    /// change on any open project, submit each changed project's bytes to the
    /// ONE writer thread without joining, then drain async failures into one
    /// `sidecar_write_failed` note each. The notes queue to the focused
    /// project — the D14-consistent attribution every `note_*` shorthand
    /// shares — with the failing path in the message.
    fn poll_sidecar_flush(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.sidecar_last_submit) < Duration::from_secs(2) {
            return;
        }
        self.sidecar_last_submit = now;
        for project in &mut self.projects {
            project.queue_incidents_flush();
        }
        for (path, error) in self.sidecar_writer.take_errors() {
            let revision = self.focused().revision;
            self.note_observation(sidecar_write_failed_observation(
                format!("{}: {error}", path.display()),
                revision,
            ));
        }
    }

    pub(crate) fn route_incidents(&mut self) {
        let observations = std::mem::take(&mut self.pending_observations);
        let conflicts = std::mem::take(&mut self.pending_router_conflicts);
        if observations.is_empty() && conflicts.is_empty() && self.pending_router_applies.is_empty()
        {
            // Nothing is outstanding, so nothing can be waiting on a landing.
            self.pending_router_landings.clear();
        } else {
            let focused = self.focused_project;
            // `media_events` is a single app-level receiver from the one engine,
            // and the one engine plays the focused document, so the focused
            // project owns every playback refusal (rule 5).
            let mut opened = Vec::new();
            if !observations.is_empty() {
                let handle = Arc::clone(&self.projects[focused].incidents);
                let mut log = handle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for mut observation in observations {
                    // Name capture at open only (`IN2B` §9 rule 1): the peek
                    // keeps dedups and suppressions from costing a document
                    // read, and `observe` stores the name only on open.
                    if log.would_open(observation.code, observation.subject, &observation.observed)
                    {
                        observation.name = self.capture_subject_name(focused, &observation.subject);
                    }
                    // The observation is kept beside the id so the
                    // investigator can reunite the open with its refused
                    // operation below.
                    match log.observe(observation.clone()) {
                        Observed::Opened(id) => {
                            opened.push((id, observation));
                        }
                        Observed::Deduped(id) => {
                            // N1/B3(ii-a) + N2/S-15b: the first re-observation
                            // this run of a LOADED open incident joins
                            // `opened` — audit, auto-apply and enqueue all run
                            // over it exactly as over a fresh open. Later
                            // dedups find the id gone from the set: nothing,
                            // as today (`IN2B` §3 rule 11).
                            if self.projects[focused].loaded_open_ids.remove(&id) {
                                opened.push((id, observation));
                            }
                        }
                        // `Suppressed` does nothing at all (rule 17).
                        Observed::Suppressed => {}
                    }
                }
            }
            self.reconcile_router_sends(&conflicts);
            let mut auto_applied = Vec::new();
            for (id, _) in &opened {
                self.audit_new_incident(focused, *id, &mut auto_applied);
            }
            if !auto_applied.is_empty() {
                let handle = Arc::clone(&self.projects[focused].incidents);
                let mut log = handle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for id in auto_applied {
                    log.note_auto_applied(id);
                }
            }
            for (id, observation) in &opened {
                self.consider_investigator_queue(*id, observation);
            }
        }
        // Outside the idle fast path: a finishing session reports through its
        // own channel, never through an observation (IN2 §3.2).
        self.pump_investigator_sessions();
    }

    /// Capture the subject's name from the focused document (`IN2B` §9
    /// rules 1–2).
    ///
    /// Called once per `Opened` incident, never for dedups or suppressions
    /// (the `would_open` peek in [`Self::route_incidents`] guarantees it).
    /// `None` means "no name exists" (tracks, jobs, unit subjects) or "the
    /// member is gone" — both render the bare [`IncidentSubject::label`].
    /// Clips resolve via their asset; titles resolve to a bounded prefix
    /// (empty text is `None`, not `""`); freezes resolve via the held
    /// asset, whose frame they reference.
    fn capture_subject_name(
        &self,
        project_index: usize,
        subject: &IncidentSubject,
    ) -> Option<String> {
        let document = &self.projects[project_index].document;
        match subject {
            IncidentSubject::Asset(id) => document.asset(*id).map(|asset| asset.name.clone()),
            IncidentSubject::LutAsset(id) => document.lut_asset(*id).map(|lut| lut.title.clone()),
            IncidentSubject::Clip(id) => {
                let clip = document.clip(*id)?;
                match &clip.content {
                    ClipContent::Title(title) => {
                        let prefix: String = title.text.chars().take(32).collect();
                        (!prefix.is_empty()).then_some(prefix)
                    }
                    ClipContent::Media | ClipContent::Freeze(_) => {
                        document.asset(clip.asset).map(|asset| asset.name.clone())
                    }
                }
            }
            IncidentSubject::Chain(AudioChain::Bus(id)) => {
                document.audio_mix.bus(*id).map(|bus| bus.name.clone())
            }
            IncidentSubject::Chain(AudioChain::Master) => Some("Master".to_owned()),
            IncidentSubject::Track(_)
            | IncidentSubject::ExportJob
            | IncidentSubject::Project
            | IncidentSubject::Agent => None,
        }
    }

    /// Write one audit line for a newly opened incident and auto-apply it
    /// when its stored class allows (IN1 §5.2 rules 9, 11, 13).
    ///
    /// The incident's own `class` field decides, never a re-derived
    /// predicate, and the operation sent is the one `policy_recovery`
    /// stored on the incident at observe time — the router builds no
    /// recovery itself (§2.5 rule 45).
    ///
    /// `pub(crate)` for IN2's approve path, which reuses this funnel for
    /// rule 18's audit line rather than opening a second `note_incident`
    /// call site the sink gate forbids (`IN1b` §5.3 rule 18).
    pub(crate) fn audit_new_incident(
        &mut self,
        project_index: usize,
        id: IncidentId,
        auto_applied: &mut Vec<IncidentId>,
    ) {
        let handle = Arc::clone(&self.projects[project_index].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(incident) = log.get(id) else {
            return;
        };
        let class = incident.class;
        let revision = incident.revision;
        let operation = (class == PolicyClass::AutoApply)
            .then(|| incident.recoveries.first())
            .flatten()
            .and_then(|recovery| match &recovery.kind {
                RecoveryKind::Operation(operation) => Some(operation.clone()),
                RecoveryKind::Explain(_) => None,
            });
        // `IN1b` §5.3 rule 18: the crate's one `note_incident` call site. The
        // guard is taken from a **local** `Arc`, so `&mut self` is disjoint
        // from the borrow the incident is read through and the write below
        // needs no clone; the `drop(log)` that follows is hygiene, as it was
        // in Part A.
        self.note_incident(incident);
        drop(log);
        let Some(operation) = operation else {
            return;
        };
        // `Command::DoIfRevision`, never `Command::Do`: the apply the router
        // planned against one revision must not land on another (rule 9).
        // The token is what the conflict is matched by (`IN1b` §4 rule 6).
        let token = self.next_command_token();
        if self.projects[project_index]
            .core
            .send(Command::DoIfRevision {
                expected: revision,
                operation,
                token: Some(token),
            })
            .is_err()
        {
            // Appendix B row 14.
            self.note_actor_stopped(
                IncidentSubject::Project,
                "Core actor stopped while applying the edit",
            );
        } else {
            self.pending_router_applies.push(RouterApply {
                project_index,
                incident: id,
                outcome: IncidentOutcome::Applied,
                token,
                origin: RouterSendOrigin::Auto,
            });
            auto_applied.push(id);
        }
    }

    /// Allocate the next correlation token (`IN1b` §4 rule 6).
    fn next_command_token(&mut self) -> CommandToken {
        let token = CommandToken(self.next_command_token);
        self.next_command_token = self.next_command_token.wrapping_add(1);
        token
    }

    /// The outstanding router send a core answer names, with the subject its
    /// incident is about — `None` when the answer is not the router's.
    ///
    /// The identity is `(project, token)`, the same one
    /// [`Self::reconcile_router_sends`] uses, written **once** so the three
    /// decisions that turn on it cannot drift apart (review-final B1, nit 1).
    fn router_send_for(
        &self,
        project_index: usize,
        token: Option<CommandToken>,
    ) -> Option<(IncidentSubject, RouterSendOrigin)> {
        let token = token?;
        let apply = self
            .pending_router_applies
            .iter()
            .find(|apply| apply.token == token && apply.project_index == project_index)?;
        let handle = Arc::clone(&self.projects[project_index].incidents);
        let subject = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(apply.incident)
            .map_or(IncidentSubject::Project, |incident| incident.subject);
        Some((subject, apply.origin))
    }

    /// Drop the outstanding send `(project, token)` names.
    fn drop_router_send(&mut self, project_index: usize, token: Option<CommandToken>) {
        let Some(token) = token else {
            return;
        };
        self.pending_router_applies
            .retain(|apply| !(apply.token == token && apply.project_index == project_index));
    }

    /// Everything the incident router reads out of one core event, in **one**
    /// place (review-final B1).
    ///
    /// The three decisions errata `IN1b`-C-R50, `IN1b`-C-R51 and `IN1b`-C-R53
    /// landed — record a landing, skip the card for a conflict the router
    /// caused, drop and skip for a rejection it caused — used to live in
    /// `poll_background`'s drain and again, in parallel source, in the test
    /// mirror `in1_drain_core`. Two copies of one decision is the failure mode
    /// ruling N5.7 S2 named: a mutation of either one alone left the suite
    /// green, including deleting the landing push, which stops **every** router
    /// auto-apply from ever resolving in the real application. This method is
    /// the single owner; both drains call it and neither decides anything.
    ///
    /// Erratum `IN1b`-C-R54 is the one thing it adds: the two skips apply to a
    /// send the **router** planned, and not to one the person pressed. A card
    /// press that loses its race is reported with the **incident's own
    /// subject** rather than the project's, because the person pressed a
    /// button on that card and that card is where the answer belongs.
    fn note_core_event_for_router(&mut self, project_index: usize, event: &Event) {
        match event {
            // Erratum `IN1b`-C-R50: the actor echoes the token it landed, and
            // that echo is what makes "this send landed" a fact rather than an
            // inference from bytes anyone could have written.
            Event::DocumentChanged { token, .. } => {
                if let Some(token) = *token {
                    self.pending_router_landings.push(token);
                }
            }
            // Appendix B row 26 as amended by errata `IN1b`-C-R51 and
            // `IN1b`-C-R54: `edit_revision_conflict` with `Revision` evidence,
            // for every conflict the **router** did not cause.
            //
            // A conflict whose token is one of the router's own auto-applies
            // gets no card. Raising one would tell the person "the timeline
            // moved between planning this edit and sending it; make the edit
            // again" about an edit they never made — one Kinewright planned,
            // sent and is already reconciling — and `Explain` never resolves,
            // so `open_count()` would never fall back to zero. §5.4 rule 25's
            // whole argument is that the badge falls as an incident is
            // auto-applied. `reconcile_router_sends` is the whole response to
            // those, and the identity is §4 rule 7's.
            Event::RevisionConflict {
                expected,
                actual,
                token,
            } => {
                let origin = self.router_send_for(project_index, *token);
                self.pending_router_conflicts.push(RouterConflict {
                    project_index,
                    actual: *actual,
                    token: *token,
                });
                let subject = match origin {
                    Some((_, RouterSendOrigin::Auto)) => return,
                    Some((subject, RouterSendOrigin::CardPress)) => subject,
                    None => IncidentSubject::Project,
                };
                self.note_observation(IncidentObservation::revision_conflict(
                    subject, *expected, *actual,
                ));
            }
            // Appendix B row 24, the third way a gated send can end (errata
            // `IN1b`-A-R15, `IN1b`-C-R53, `IN1b`-C-R54). The drain stops
            // discarding `op`, so a foreign rejection's subject is the rejected
            // operation's own (`IN1b` §3.3 rule 20) and the code is the family
            // `OpError::incident_code()` resolves at run time.
            //
            // A rejection of the router's own send drops it — the incident
            // stays `Open` because the recovery did not land, nothing is
            // re-sent, and its `revision` does not move: no conflict occurred,
            // so core reported no newer revision to refresh it to (rule 10).
            // An auto-apply's rejection additionally gets no card; a card
            // press's does, on the incident whose button was pressed.
            Event::OpRejected { op, error, token } => {
                let subject = match self.router_send_for(project_index, *token) {
                    Some((_, RouterSendOrigin::Auto)) => {
                        self.drop_router_send(project_index, *token);
                        return;
                    }
                    Some((subject, RouterSendOrigin::CardPress)) => {
                        self.drop_router_send(project_index, *token);
                        subject
                    }
                    None => op.incident_subject(),
                };
                let revision = self.projects[project_index].revision;
                let observation = IncidentObservation::from_op_error(error, subject, revision);
                // The investigator replays the refused operation into its
                // opening message; the incident id does not exist until the
                // observation routes, so the operation waits beside it
                // (IN2 §3.4 rule 22).
                self.stash_refused_if_eligible(project_index, &observation, op);
                self.note_observation(observation);
            }
            Event::BatchRejected { .. } | Event::QueryResult(_) => {}
        }
    }

    /// Reconcile the router's outstanding sends: drop what a conflict refused,
    /// resolve what the live document shows as landed, and keep what is still
    /// in flight (IN1 §5.2 rules 9-10, 12).
    fn reconcile_router_sends(&mut self, conflicts: &[RouterConflict]) {
        for conflict in conflicts {
            // All three halves of rule 10: the incident stays `Open`, its
            // `revision` is refreshed to the revision core reported when it
            // refused, and the send is dropped rather than re-sent.
            //
            // The match is **by identity** (`IN1b` §4 rules 6-7). Every router
            // send allocates a `CommandToken`, the actor echoes it on the
            // `Event::RevisionConflict` it answers with, and the outstanding
            // send is the one whose token is equal. A conflict carrying no
            // token, or a token no outstanding send holds, belonged to a
            // foreign `Command::DoIfRevision` — the two `media_workflow.rs`
            // senders, or the agent's own — and the router **does nothing**:
            // the `edit_revision_conflict` incident the drain already opened is
            // the whole response. Part A's two-tier `(project, expected)`
            // heuristic is gone; it consumed a send no conflict of the
            // router's had refused, which is what
            // `in1b_a_foreign_conflict_leaves_every_router_send_outstanding`
            // pins.
            //
            // IN1 §9 clause 9 — a person who made the same edit by hand first,
            // so the live document shows the apply as landed while core
            // nonetheless refused it — is unchanged and is now exact rather
            // than preferential: the refused send is named by the event.
            let Some(token) = conflict.token else {
                continue;
            };
            // The identity is `(project, token)`. The token alone is unique by
            // construction — one counter, one app — and naming the project
            // beside it is what makes `RouterConflict::project_index`'s role
            // explicit and fails closed if that ever stops being true.
            let Some(position) = self.pending_router_applies.iter().position(|apply| {
                apply.token == token && apply.project_index == conflict.project_index
            }) else {
                continue;
            };
            let incident = self.pending_router_applies[position].incident;
            let handle = Arc::clone(&self.projects[conflict.project_index].incidents);
            // `refresh_revision` is core's third narrowly typed writer and the
            // only way to do this: the router called `note_auto_applied` at
            // send time, so `observe` returns `Suppressed` before it could
            // refresh anything (IN1 §2.3 rules 19-20, §5.2 rule 10). An
            // incident id the log no longer knows is left alone.
            handle
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .refresh_revision(incident, conflict.actual);
            self.pending_router_applies.remove(position);
        }
        // A landing whose send is no longer outstanding is spent.
        self.pending_router_landings.retain(|landed| {
            self.pending_router_applies
                .iter()
                .any(|apply| apply.token == *landed)
        });
        if self.pending_router_applies.is_empty() {
            return;
        }
        let mut projects: Vec<usize> = self
            .pending_router_applies
            .iter()
            .map(|apply| apply.project_index)
            .collect();
        projects.sort_unstable();
        projects.dedup();
        for project_index in projects {
            self.reconcile_project_sends(project_index);
        }
    }

    /// Reconcile one project's outstanding sends against its live document.
    fn reconcile_project_sends(&mut self, project_index: usize) {
        let mut waiting = Vec::new();
        let mut landed = Vec::new();
        for apply in std::mem::take(&mut self.pending_router_applies) {
            if apply.project_index != project_index {
                waiting.push(apply);
                continue;
            }
            let handle = Arc::clone(&self.projects[project_index].incidents);
            let log = handle
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(incident) = log.get(apply.incident) else {
                // An incident is never removed from its log; a missing id is
                // dropped, never re-sent.
                continue;
            };
            // Erratum `IN1b`-C-R50: **both** conditions. The token says the
            // actor landed this very send; the document read says the bytes
            // the recovery carried are the ones on the asset now. Either alone
            // is wrong — a token with no document match is an apply a later
            // edit superseded, and a document match with no token is the
            // person's own hand edit in the frame before the actor replied.
            let landed_token = self.pending_router_landings.contains(&apply.token);
            let accepted = landed_token
                && router_apply_accepted(
                    &self.projects[project_index].document,
                    incident,
                    apply.outcome,
                );
            let id = apply.incident;
            let outcome = apply.outcome;
            drop(log);
            if accepted {
                landed.push((id, outcome));
            } else if landed_token {
                // The actor landed this send and the document no longer shows
                // its bytes: a later edit superseded it. The incident stays as
                // it is and nothing is re-sent (rule 10).
            } else {
                // No answer yet. Part A dropped the send here whenever the
                // project's revision had moved at all, which is what let a
                // person's own edit in the window look like an answer
                // (erratum `IN1b`-C-R50). A send the actor has not answered is
                // **in flight**, and stays outstanding until its own token
                // comes back on a landing or on the conflict that refused it —
                // which is what lets `reconcile_router_sends` refresh the
                // incident to the revision core reported (rule 10) and what
                // lets erratum `IN1b`-C-R51 recognise the conflict as the
                // router's own. **All three** ways a gated send can end now
                // carry its identity: the landing above, the conflict
                // `reconcile_router_sends` matches, and the rejection the
                // drain arm drops (errata `IN1b`-A-R15 and `IN1b`-C-R53), so
                // "in flight" is a state a send always leaves.
                waiting.push(apply);
            }
        }
        self.pending_router_applies = waiting;
        if !landed.is_empty() {
            let handle = Arc::clone(&self.projects[project_index].incidents);
            let mut log = handle
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (id, outcome) in landed {
                log.resolve(id, outcome);
            }
        }
    }

    /// Send one card action's recovery through the revision gate (IN1 §5.4
    /// rule 34): the revert, or the recovery an open incident still offers.
    ///
    /// The revision is read fresh at press time, and acceptance is recognised
    /// the same way as an auto-apply, by [`router_apply_accepted`], so a send
    /// that loses a race with another edit leaves the incident as it was
    /// instead of recording an outcome the document does not show.
    pub(crate) fn send_incident_recovery(
        &mut self,
        project_index: usize,
        incident: IncidentId,
        operation: Operation,
        outcome: IncidentOutcome,
    ) {
        let expected = self.projects[project_index].revision;
        let token = self.next_command_token();
        if self.projects[project_index]
            .core
            .send(Command::DoIfRevision {
                expected,
                operation,
                token: Some(token),
            })
            .is_err()
        {
            // Appendix B row 15.
            self.note_actor_stopped(
                IncidentSubject::Project,
                "Core actor stopped while applying the edit",
            );
        } else {
            self.pending_router_applies.push(RouterApply {
                project_index,
                incident,
                outcome,
                token,
                origin: RouterSendOrigin::CardPress,
            });
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
            // Appendix B row 16.
            self.note_actor_stopped(
                IncidentSubject::Project,
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
            // Appendix B row 17.
            self.note_actor_stopped(
                IncidentSubject::Project,
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
            // Appendix B row 18.
            self.note_actor_stopped(IncidentSubject::Project, "Core actor stopped while undoing");
        } else {
            "Undo".clone_into(&mut self.status);
        }
    }

    pub(crate) fn redo(&mut self) {
        if self.focused().core.send(Command::Redo).is_err() {
            // Appendix B row 19.
            self.note_actor_stopped(IncidentSubject::Project, "Core actor stopped while redoing");
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
        let note_revision = self.focused().revision;
        if let Some(observation) = self.color_qc.poll(note_revision) {
            self.note_observation(observation);
        }
        if self.color_qc.is_pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.poll_noise_learn(ctx);
        self.poll_room_tone(ctx);
        self.poll_recording(ctx);
        self.poll_media_workflow(ctx);
        self.recover_stranded_ab_hold(ctx);
        self.release_hidden_monitor_gain();
        if self.poll_lut_workers() {
            ctx.request_repaint();
        }
        // Appendix B row 20, the third aggregate: **one incident per asset**,
        // because a per-asset failure is per-asset by construction and the
        // dedup key collapses the repeats correctly (`IN1b` §5.1 rule 14).
        // `media_incomplete`, because the project is open and usable — only
        // its timeline pictures are missing.
        for (asset, error) in self.visual_cache.poll(ctx) {
            self.note_label(
                LabelIncident::MediaIncomplete,
                IncidentSubject::Asset(asset),
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
                        // Appendix B row 21.
                        self.note_actor_stopped(
                            IncidentSubject::Project,
                            "Core actor stopped while importing media",
                        );
                    }
                }
                // Appendix B row 22: the import failed, so no asset exists to
                // name.
                Err(error) => self.note_label(
                    LabelIncident::Media,
                    IncidentSubject::Project,
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
            // Review-final B1: every router decision this event carries is
            // made in exactly one place, which both this drain and the test
            // mirror call. The arms below do the rest.
            self.note_core_event_for_router(project_index, &event);
            match event {
                Event::DocumentChanged {
                    doc,
                    revision,
                    last_op,
                    journal_command,
                    token: _,
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
                        // Appendix B row 23.
                        let changed_asset = self
                            .pending_source_edit
                            .as_ref()
                            .map(|pending| pending.asset_id);
                        self.pending_source_edit = None;
                        let subject =
                            changed_asset.map_or(IncidentSubject::Project, IncidentSubject::Asset);
                        self.note_label(
                            LabelIncident::SourceMonitor,
                            subject,
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
                // Both router decisions this event carries are made in
                // `note_core_event_for_router` (review-final B1); only the
                // LUT reservation is this arm's.
                Event::OpRejected { .. } => {
                    self.release_lut_import_reservation(project_index);
                }
                // Appendix B row 25: the same for a plan, over the batch's
                // common subject.
                Event::BatchRejected { operations, error } => {
                    let revision = self.projects[project_index].revision;
                    let subject = batch_incident_subject(&operations);
                    let observation =
                        IncidentObservation::from_batch_error(&error, subject, revision);
                    // The investigator needs the refused plan in its opening
                    // message just as it needs a refused single operation.
                    // Keep the first operation beside the observation before
                    // routing creates the incident id.
                    self.stash_batch_refused(project_index, &observation, &operations);
                    self.note_observation(observation);
                    self.release_lut_import_reservation(project_index);
                }
                // Every router decision this event carries is made in
                // `note_core_event_for_router`, including the conflict note
                // `reconcile_router_sends` reads (review-final B1).
                Event::RevisionConflict { .. } | Event::QueryResult(_) => {}
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
                    let revision = self.focused().revision;
                    // `IN1b` §5.1 rule 12: the constructor is total after
                    // Part B, so both arms fold into one and IN1 §5.2 rule 6's
                    // `else` arm ceases to exist. The fallback subject is the
                    // project, because a playback failure that names no asset
                    // is a document-wide one; implementer C owns the rest of
                    // Appendix B row 27.
                    self.pending_observations
                        .push(IncidentObservation::from_media_error(
                            &error,
                            IncidentSubject::Project,
                            revision,
                        ));
                }
            }
        }

        // IN1 §5.2b rule 16: the incident router runs once per update, after
        // both event drains, and never from inside either.
        self.route_incidents();
        // `IN2B` §2 rule 4: the debounced sidecar flush runs after the router,
        // so the generations it compares have settled for this tick.
        self.poll_sidecar_flush();

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
            // `IN2B` §4 rule 3: Save greys out for a newer-format session,
            // with a tooltip naming the version; Save As stays enabled.
            // N6/H2 greys it out for routed sessions too, with the refusal
            // notice as the tooltip.
            let can_save = can_overwrite_save(self.focused().format_version)
                && self.overwrite_save_refusal().is_none();
            let mut save = ui.add_enabled(can_save, egui::Button::new("Save"));
            if !can_save {
                if let Some(notice) = self.overwrite_save_refusal() {
                    save = save.on_hover_text(notice);
                } else {
                    save = save.on_hover_text(format!(
                        "Saving is disabled: this project was written by a newer Kinewright (format_version {}). Use Save As.",
                        self.focused().format_version
                    ));
                }
            }
            if save.clicked() {
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

    /// The badge's number: problems currently unresolved in the focused
    /// project (`IN1b` §5.4 rule 25).
    ///
    /// `IncidentLog::open_count()` counts problems currently unresolved;
    /// `ErrorLog::len()` counts lines ever written this session (IN1 §5.2
    /// rule 15). They are different quantities and
    /// `in1b_the_badge_counts_open_incidents_not_log_lines` asserts both.
    #[must_use]
    pub(crate) fn open_incident_count(&self) -> usize {
        Arc::clone(&self.focused().incidents)
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open_count()
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
        let measured_frame = self.performance.as_ref().map(|_| std::time::Instant::now());
        self.update_window_title(ui.ctx());
        self.handle_close_request(ui.ctx());
        self.poll_background(ui.ctx());
        self.keyboard_shortcuts(ui.ctx());
        self.import_dropped_files(ui.ctx());
        let outcome = self.projects.first_mut().map(|project| {
            let ctx = ui.ctx().clone();
            project.recovery.show_dialog(&ctx)
        });
        if let Some(outcome) = outcome {
            // Modal damage notes (`IN2B` §7 rule 6): each modal's first
            // damage render per run notes once — the bools edge, the
            // outcome carries the rendered strings.
            if let Some(damage) = outcome.damage_rendered.as_deref()
                && !self.recovery_damage_noted
            {
                self.recovery_damage_noted = true;
                let revision = self.focused().revision;
                self.note_observation(recovery_damage_observation(damage, revision));
            }
            if let Some(message) = outcome.unavailable_rendered.as_deref()
                && !self.recovery_unavailable_noted
            {
                self.recovery_unavailable_noted = true;
                let revision = self.focused().revision;
                self.note_observation(recovery_unavailable_observation(message, revision));
            }
            if let Some(request) = outcome.restore {
                self.apply_restore_request(request);
            }
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
        // `IN1b` §5.5 rule 31: the badge's own panel, drawn beside the audit
        // window it is no longer a count of.
        crate::incident_ui::show_incidents_panel(self, ui.ctx());
        self.show_unsaved_confirmation(ui.ctx());
        self.screenshot.update(ui.ctx());
        if let (Some(probe), Some(began)) = (&mut self.performance, measured_frame)
            && probe.frame(ui.ctx(), began, self.texture.is_some())
        {
            self.allow_close = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
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
                        // `IN1b` §5.4 rule 25: the badge counts **problems
                        // currently unresolved**, not lines ever written this
                        // session. `IncidentLog::open_count()` falls as an
                        // incident is auto-applied, reverted or explained;
                        // `ErrorLog::len()` never falls, and a badge summing
                        // both double-counts every auto-applied incident — the
                        // one case IN1 exists to make invisible. The error-log
                        // window keeps `entries.len()`, because it is now
                        // genuinely an audit view.
                        //
                        // Erratum `IN1b`-C-R49 decides *visibility* separately
                        // from the number: `incident_badge` keeps the badge on
                        // screen while the audit view has anything to show, so
                        // the session's record never becomes unreachable.
                        //
                        // `ErrorLog::len()` is the whole log and not a count of
                        // `"Incident"`-source lines, and after Part B those are
                        // the same number: `note_incident` is the crate's only
                        // writer and it writes that one source, which the sink
                        // gate's counts 2 and 3 prove (review-final nit 6). The
                        // tests read `count_with_source` because they predate
                        // the gate, not because the log holds anything else.
                        if let Some(open_incidents) = crate::incident_ui::incident_badge(
                            self.open_incident_count(),
                            self.error_log.len(),
                        ) && ui
                            .add(egui::Button::image_and_text(
                                Icon::Alert.image(size::ICON_SM),
                                format!("{open_incidents}"),
                            ))
                            .on_hover_text("Open the Incidents panel")
                            .clicked()
                        {
                            self.incidents_open = true;
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

/// Load a project file: one read, one parse (`IN2B` §4 rule 2).
///
/// Returns the parsed document with the envelope version it read (missing → 1,
/// every legacy file) and the FNV-1a digest of the file bytes (§2 rule 9 —
/// the sidecar gate reuses it, no second read, no TOCTOU). The parse is
/// `from_slice::<ProjectFile>` then unwrap: the envelope parses legacy files
/// directly via the default, never probe-then-parse.
fn load_document(path: &Path) -> Result<(Document, u32, String), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let file: ProjectFile = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    file.document
        .validate()
        .map_err(|error| error.to_string())?;
    let digest = digest_bytes(&bytes);
    Ok((file.document, file.format_version, digest))
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
    let started = std::time::Instant::now();
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
            let repaint_context = creation_context.egui_ctx.clone();
            media.set_event_wakeup(move || repaint_context.request_repaint());
            let startup = std::env::args().nth(1).map(PathBuf::from);
            let mut app = KinewrightApp::new(media, startup);
            app.performance = crate::performance::PerformanceProbe::from_environment(started);
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
            // Appendix B row 29: the three Mixer sites are `AudioChain`-scoped
            // and no `TrackId` is in scope at any of them.
            self.note_label(
                LabelIncident::Mixer,
                IncidentSubject::Chain(chain),
                LEARN_NO_SILENCE,
            );
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
            // Appendix B row 30.
            self.note_label(
                LabelIncident::Mixer,
                IncidentSubject::Chain(chain),
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
                    // Appendix B row 31.
                    self.note_label(
                        LabelIncident::Mixer,
                        IncidentSubject::Chain(response.chain),
                        format!("Could not learn a noise profile: {error}"),
                    );
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

/// IN1 Part A: the incident router, driven through the real
/// `route_incidents` with a real `Core` actor and, where the clause needs
/// one, a real decode.
///
/// IN1 §5.2b rule 22 says the app test asserts the observation → policy →
/// operation chain through core and asserts the tick placement by
/// inspection; the harness below is what "through core" means without a
/// window. Every error under test comes from the real decoder over a live
/// `Playback` — `set_document` plus frame requests, never `play` (IN1 §7
/// rule 3) — and reaches the log only through
/// `IncidentObservation::from_media_error`, so no test here can pass with
/// §4.2's plumbing absent.
#[cfg(test)]
pub(crate) mod in1_tests {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Instant;

    use kinewright_agent::{ScriptedCall, ScriptedCost, ScriptedDriver, ScriptedTurn};
    use kinewright_core::{
        COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorDescription, ColorMatrix,
        ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer, ColorWhitePoint,
        IncidentEvidence, IncidentLog, IncidentResolver, IncidentState, MediaBin, Observed,
        assume_rec709_operation, policy_recovery,
    };
    use kinewright_media::{
        FfmpegMediaEngine,
        in1_sources::{In1Source, in1_source},
        test_support::{GeneratedMedia, TempDirectory, single_clip_document},
    };

    use super::*;
    use crate::incident_ui::{CardLoadedFlags, REVERT_LABEL, empty_panel_sets, incident_card};

    /// IN1 §7 rule 13's deadline, shared by every app-side wait for the same
    /// reason the agent side shares its own: a hang detector that fires only
    /// on failure. The router tests poll the actor; the decode tests poll
    /// the engine; neither loops forever.
    const IN1_APP_DEADLINE: Duration = Duration::from_secs(10);

    /// `in1_untagged.mp4` probed through the ordinary probe path. The
    /// returned guard must be held for as long as the document is in use, or
    /// the fixture file is removed from under the decoder (IN1 §3 rule 11).
    fn in1_untagged(engine: &FfmpegMediaEngine) -> (GeneratedMedia, Document) {
        let generated = in1_source(In1Source::UntaggedMp4);
        let asset = engine.probe(generated.path()).expect("the fixture probes");
        (generated, single_clip_document(asset))
    }

    /// Drive the real decoder until a `MediaEvent::Error` arrives or the
    /// deadline expires. `set_document` plus one `request_frame`, never
    /// `play` (IN1 §7 rule 3).
    fn in1_playback_error(engine: &FfmpegMediaEngine, document: &Document) -> MediaError {
        engine.set_document(Arc::new(document.clone()));
        engine.request_frame(TimeCode::ZERO);
        let events = engine.events();
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        let mut seen: Vec<String> = Vec::new();
        loop {
            while let Ok(event) = events.try_recv() {
                if let MediaEvent::Error(error) = event {
                    return error;
                }
                seen.push(format!("{event:?}"));
            }
            assert!(
                Instant::now() < expiry,
                "no MediaEvent::Error within {IN1_APP_DEADLINE:?}; events seen: {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The smallest real app that can tick: one project session on a real
    /// `Core` actor, a real engine behind the trait arcs, and default UI
    /// state everywhere else. There is deliberately no window, no model, and
    /// no audio device — the same terms §9 lays down.
    #[allow(clippy::too_many_lines)]
    fn in1_harness(document: Document) -> (KinewrightApp, Arc<FfmpegMediaEngine>) {
        let engine = Arc::new(FfmpegMediaEngine::new().expect("the test engine starts"));
        let playback: Arc<dyn Playback> = engine.clone();
        let analysis: Arc<dyn Analysis> = engine.clone();
        let exporter: Arc<dyn Export> = engine.clone();
        let project = ProjectSession::create(
            1,
            "IN1",
            document,
            None,
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::None,
            None,
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the test session builds");
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (relink_probe_tx, relink_probe_rx) = std::sync::mpsc::channel();
        let (lut_import_tx, lut_import_rx) = std::sync::mpsc::channel();
        let (lut_restore_tx, lut_restore_rx) = std::sync::mpsc::channel();
        let (media_status_tx, media_status_rx) = std::sync::mpsc::channel();
        let (cache_clear_tx, cache_clear_rx) = std::sync::mpsc::channel();
        let (room_tone_tx, room_tone_rx) = std::sync::mpsc::channel();
        let (_frames_tx, frames) = crossbeam_channel::unbounded();
        let (_media_tx, media_events) = crossbeam_channel::unbounded();
        let app = KinewrightApp {
            projects: vec![project],
            focused_project: 0,
            next_project_id: 2,
            next_command_token: 1,
            playback,
            analysis,
            exporter,
            lut_publisher: Arc::clone(&engine),
            frames,
            media_events,
            visual_cache: crate::visual_cache::VisualCache::new(engine.visual_asset_results()),
            harness: std::array::from_fn(|_| crate::chat_ui::HarnessUiState::default()),
            harness_update_rx: None,
            show_thread_rail: true,
            settings_open: false,
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
            pending_observations: Vec::new(),
            transcript_noted: None,
            recovery_damage_noted: false,
            recovery_unavailable_noted: false,
            pending_router_conflicts: Vec::new(),
            pending_router_landings: Vec::new(),
            pending_router_applies: Vec::new(),
            sidecar_writer: SidecarWriter::new(),
            sidecar_last_submit: Instant::now(),
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
            working_proof_cache: Arc::default(),
            matte_overlay: crate::matte_overlay_ui::MatteOverlayState::default(),
            playing: false,
            meter_levels: [0.0; 2],
            mixer_levels: crate::mixer_ui::MixerMeterLevels::default(),
            mixer_selection: None,
            resume_after_scrub: false,
            transcript_scope: TranscriptScope::default(),
            material_tab: MaterialTab::default(),
            show_material_strip: false,
            show_media_rail: false,
            pending_project_action: None,
            exit_discarded_projects: Vec::new(),
            allow_close: false,
            last_window_title: String::new(),
            status: "Ready".to_owned(),
            export_dialog: ExportDialog {
                open: false,
                output: "export.mp4".to_owned(),
                width: 320,
                height: 180,
                fps_numerator: 25,
                fps_denominator: 1,
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
            error_log: ErrorLog::default(),
            error_log_open: false,
            incidents_open: false,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
            recording: None,
            record_dialog: crate::recording::RecordDialog::default(),
            edit_gesture: 0,
            look_ab_hold: None,
            look_ab_hold_seen: false,
            performance: None,
        };
        (app, engine)
    }

    /// Shut down every branch server the harness started, so no test leaves
    /// a listener thread behind.
    pub(crate) fn in1_shutdown(app: &mut KinewrightApp) {
        for project in &mut app.projects {
            for thread in &mut project.threads {
                if let Some(server) = thread.mcp_server.take() {
                    server.shutdown();
                }
            }
        }
    }

    /// Drain one session's core events the way `poll_background`'s core drain
    /// does for the fields the router reads: adopt the new document and
    /// revision on `DocumentChanged`, and hand **every** event to the one
    /// method that makes every router decision.
    ///
    /// After review-final B1 this mirror decides nothing of its own. It used
    /// to re-implement the landing push, the conflict skip and the rejection
    /// drop in parallel source, and a mutation of production alone left the
    /// whole suite green — including deleting the landing push, which stops
    /// every router auto-apply from ever resolving in the real application.
    /// The two lines below are the only thing this mirrors, and they are the
    /// two the drain does *outside* `note_core_event_for_router`.
    ///
    /// Two router tests drive `poll_background` itself rather than this
    /// mirror, so the production tick is pinned as well as the decision.
    fn in1_drain_core(app: &mut KinewrightApp, project_index: usize) {
        let events: Vec<Event> = app.projects[project_index].core_events.try_iter().collect();
        for event in events {
            app.note_core_event_for_router(project_index, &event);
            if let Event::DocumentChanged { doc, revision, .. } = event {
                app.projects[project_index].document = doc;
                app.projects[project_index].revision = revision;
            }
        }
    }

    /// Silence the engine's own channel, so a production tick drains only what
    /// the test put in front of it.
    ///
    /// The harness's engine is still decoding the fixture; a playback refusal
    /// arriving mid-test would open an incident the test did not ask for. The
    /// sender is leaked deliberately — dropping it would disconnect the
    /// receiver and `try_recv` would stop being a no-op.
    fn in1_silence_media_events(app: &mut KinewrightApp) {
        let (quiet_tx, quiet_rx) = crossbeam_channel::unbounded::<MediaEvent>();
        std::mem::forget(quiet_tx);
        app.media_events = quiet_rx;
    }

    /// Tick **production** — `poll_background`, the real drain and the real
    /// router tick — until `settled` reports the actor has answered, or panic
    /// on a hang.
    ///
    /// Review-final B1: the test mirror can no longer decide anything of its
    /// own, and these two tests additionally drive the production tick, so a
    /// mutation of `poll_background`'s call into
    /// [`KinewrightApp::note_core_event_for_router`] breaks them too. The
    /// condition is the **effect** rather than a queue, because
    /// `poll_background` reconciles on its way out and leaves every queue
    /// empty behind it.
    fn in1_poll_production_until(
        app: &mut KinewrightApp,
        settled: impl Fn(&KinewrightApp) -> bool,
    ) {
        in1_silence_media_events(app);
        let ctx = egui::Context::default();
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        loop {
            app.poll_background(&ctx);
            if settled(app) {
                return;
            }
            assert!(
                Instant::now() < expiry,
                "the core actor answered nothing within {IN1_APP_DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drain until the session revision moves past `from`, or panic on a
    /// hang. Returns after adopting the newest document.
    fn in1_drain_until_revision(
        app: &mut KinewrightApp,
        project_index: usize,
        from: TimelineRevision,
    ) {
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        loop {
            in1_drain_core(app, project_index);
            if app.projects[project_index].revision != from {
                return;
            }
            assert!(
                Instant::now() < expiry,
                "the core actor answered nothing within {IN1_APP_DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drain until the drain has noted a router conflict, or panic on a hang.
    ///
    /// Like [`in1_drain_until_revision`], this waits on the effect, not on the
    /// event count, so a conflict that landed before the call is never missed.
    /// Waiting for "one more event to arrive" is what made this test flaky: the
    /// opening drain could consume the conflict itself, after which no further
    /// event was ever coming.
    fn in1_drain_until_conflict(app: &mut KinewrightApp, project_index: usize) {
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        loop {
            in1_drain_core(app, project_index);
            if app
                .pending_router_conflicts
                .iter()
                .any(|conflict| conflict.project_index == project_index)
            {
                return;
            }
            assert!(
                Instant::now() < expiry,
                "the core actor answered nothing within {IN1_APP_DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Read one incident out of a session's log.
    fn in1_incident(app: &KinewrightApp, id: IncidentId) -> Incident {
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.get(id).expect("the log keeps what it opened").clone()
    }

    /// The full auto-apply arc through the real router: push the fixture's
    /// observation, tick, adopt the acceptance, tick again. Returns the
    /// incident id, resolved `Applied`.
    fn in1_auto_apply(app: &mut KinewrightApp, error: &MediaError) -> IncidentId {
        let revision = app.projects[0].revision;
        let observation =
            IncidentObservation::from_media_error(error, IncidentSubject::Project, revision);
        app.pending_observations.push(observation);
        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "the AutoApply incident sends exactly one command"
        );
        in1_drain_until_revision(app, 0, revision);
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 0);
        assert_eq!(log.len(), 1);
        let incident = log.all().next().expect("one incident");
        assert_eq!(
            incident.state,
            IncidentState::Resolved(IncidentOutcome::Applied)
        );
        incident.id
    }

    // ---------------------------------------------------------------------
    // `IN1b` §7 items 16-19, 22-24 and the three sinks: the migration's own
    // tests. Each drives a representative site into `pending_observations`,
    // ticks the real router, and asserts the code, subject, class and
    // severity Appendix B declares for that row.
    // ---------------------------------------------------------------------

    /// The smallest real app one of these tests needs: the untagged fixture's
    /// document on a real `Core`, with the fixture guard returned so the file
    /// outlives the decoder (IN1 §3 rule 11).
    ///
    /// `pub(crate)` so a per-label test can live in the module whose arm it
    /// drives: `recording.rs`'s two sites and `note_recording_failure` are
    /// private to that module, and a test that reaches them has to be written
    /// there (review-app-2 S3).
    pub(crate) fn in1b_app() -> (GeneratedMedia, KinewrightApp) {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (fixture, document) = in1_untagged(&probe_engine);
        let (app, _engine) = in1_harness(document);
        (fixture, app)
    }

    /// This file's production half, for the source-shape assertions the
    /// unreachable arms use (review-app-1 B1, review-app-2 S3).
    fn in1b_app_source() -> String {
        in1b_production_half(include_str!("app.rs"))
    }

    /// One module's production half: everything before its first `#[cfg(test)]`.
    ///
    /// Four Appendix B arms cannot be executed from a test at all — two need a
    /// live MCP endpoint and a spawned harness, one sits inside an
    /// `egui::Window`'s closure, and one needs a branch whose thumbnail
    /// renderer fails. For those the **shape** of the arm is what is asserted,
    /// in the idiom `au5_a_room_tone_refusal_is_not_filed_under_look` already
    /// uses: the code it queues and the fact that no bare string survives
    /// beside it (`IN1b` §10's limit, review-app-2 S3).
    fn in1b_production_half(source: &'static str) -> String {
        // Line endings are normalised first: a Windows checkout hands
        // `include_str!` CRLF text, and the multi-line source shapes the
        // `include_str!` assertions look for are written with `\n`.
        let source = source.replace("\r\n", "\n");
        source
            .split_once("\n#[cfg(test)]")
            .map_or(source.clone(), |(before, _)| before.to_owned())
    }

    #[test]
    fn in1b_the_production_half_reader_is_line_ending_agnostic() {
        const SOURCE: &str = include_str!("app.rs");
        let lf = in1b_production_half(SOURCE);
        let crlf = in1b_production_half(Box::leak(SOURCE.replace('\n', "\r\n").into_boxed_str()));
        assert_eq!(
            lf, crlf,
            "a CRLF checkout must read the same half as an LF one"
        );
        assert!(lf.contains("fn route_incidents"));
    }

    /// [`in1b_route_one`] for a test that opened a second project, which the
    /// router attributes to whichever session is focused.
    fn in1b_route_focused_one(app: &mut KinewrightApp) -> Incident {
        assert_eq!(app.pending_observations.len(), 1);
        app.route_incidents();
        let handle = Arc::clone(&app.projects[app.focused_project].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 1, "exactly one incident was opened");
        log.all().next().expect("one incident").clone()
    }

    /// Tick the router and hand back the one incident it opened.
    pub(crate) fn in1b_route_one(app: &mut KinewrightApp) -> Incident {
        assert_eq!(
            app.pending_observations.len(),
            1,
            "the site queued exactly one observation"
        );
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 1, "exactly one incident was opened");
        log.all().next().expect("one incident").clone()
    }

    /// Assert one incident's declared four: code, subject, class, severity.
    pub(crate) fn in1b_declares(
        incident: &Incident,
        code: &str,
        subject: IncidentSubject,
        severity: kinewright_core::IncidentSeverity,
    ) {
        assert_eq!(incident.code.code(), code, "code");
        assert_eq!(incident.subject, subject, "subject");
        assert_eq!(incident.class, PolicyClass::Explain, "class");
        assert_eq!(incident.severity, severity, "severity");
        assert_eq!(incident.field, incident.code.field(), "field");
        assert_eq!(incident.state, IncidentState::Open);
    }

    /// `IN1b` §7 item 16, label 1 of 17. Appendix B row 120's site: deleting
    /// with nothing selected.
    #[test]
    fn in1b_operations_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.focused_mut().selected_clip = None;
        app.focused_mut().selected_marker = None;
        app.delete_selected();
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "operations_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// Label 2 of 17. Appendix B row 94's site: a look import with no LUT
    /// store, which is every unsaved project.
    #[test]
    fn in1b_look_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        assert!(
            app.focused().lut_store.is_none(),
            "an unsaved project has no store root"
        );
        app.start_lut_import(
            PathBuf::from("look.cube"),
            crate::media_workflow::LutImportIntent::Apply {
                clip: None,
                stage: kinewright_core::ColorStage::Look,
            },
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "look_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);

        // Appendix B row 96 on its own app, for the **eighth** subject shape
        // (stage A addendum 2, which withdraws erratum `IN1b`-C-R42): one look
        // that keeps refusing is one problem, on its own axis, and the id in
        // scope is a `LutAssetId` rather than an `AssetId`.
        let (_second_fixture, mut second) = in1b_app();
        let missing = kinewright_core::LutAssetId(404);
        assert!(second.focused().document.lut_asset(missing).is_none());
        second.choose_lut_restore(missing);
        let incident = in1b_route_one(&mut second);
        in1b_declares(
            &incident,
            "look_unclassified",
            IncidentSubject::LutAsset(missing),
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert_eq!(incident.subject.label(), "Look 404");
        in1_shutdown(&mut second);
    }

    /// Label 3 of 17. Appendix B row 58's site: an export with no output
    /// path. The subject is the unit `ExportJob`, which carries no id because
    /// eleven of the thirteen `Export` rows fire before a job exists.
    #[test]
    fn in1b_export_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.export_dialog.output = String::new();
        app.start_export();
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "export_unclassified",
            IncidentSubject::ExportJob,
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert!(
            app.export_job.is_none(),
            "the refusal is before the job exists, which is why the subject has no id"
        );
        in1_shutdown(&mut app);
    }

    /// Label 4 of 17. Appendix B row 80's site: a media refresh that cancels
    /// a Source edit waiting on verification.
    #[test]
    fn in1b_source_monitor_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        let asset = app.focused().document.media_pool[0].clone();
        app.pending_source_edit = Some(crate::media_workflow::PendingSourceEdit {
            session_id: app.focused().id,
            request_id: 1,
            asset_id: asset.id,
            path: asset.path.clone(),
            fingerprint: asset.source_fingerprint.clone(),
            expected_revision: app.focused().revision,
            selected_asset: Some(asset.id),
            source_position: TimeCode::ZERO,
            timeline_in: TimeCode::ZERO,
            source_in: TimeCode::ZERO,
            source_out: TimeCode(1),
            video_target: None,
            audio_target: None,
            mode: kinewright_core::ThreePointMode::Overwrite,
        });
        app.refresh_media_statuses_for_focused_project();
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "source_monitor_unclassified",
            IncidentSubject::Asset(asset.id),
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// Label 5 of 17. Appendix B row 92's site, whose own arm is behind a
    /// modal: the observation the site builds is queued through the same
    /// `note_label` shorthand the arm uses, with the arm's own arguments.
    #[test]
    fn in1b_relink_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        let asset = app.focused().document.media_pool[0].id;
        app.note_label(
            kinewright_core::LabelIncident::Relink,
            IncidentSubject::Asset(asset),
            "The selected asset no longer exists",
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "relink_unclassified",
            IncidentSubject::Asset(asset),
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);

        // The arm itself sits inside `show_legacy_relink_confirmation`'s
        // `egui::Window` closure and cannot be executed without a window, so
        // its shape is asserted instead.
        let workflow = in1b_production_half(include_str!("media_workflow.rs"));
        assert!(
            workflow.contains(
                "self.note_label(\n                        LabelIncident::Relink,\n                        IncidentSubject::Asset(pending.asset_id),\n                        \"The selected asset no longer exists\","
            ),
            "Appendix B row 92 queues `relink_unclassified` against the asset it \
             refused to replace, and queues nothing else"
        );
    }

    /// Label 6 of 17. Appendix B row 48's site, which needs a live harness;
    /// the arm's own arguments go through the same shorthand.
    #[test]
    fn in1b_agent_branch_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.note_label(
            kinewright_core::LabelIncident::AgentBranch,
            IncidentSubject::Agent,
            "Could not create an isolated agent branch",
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "agent_branch_unclassified",
            IncidentSubject::Agent,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);

        // The arm needs a spawned harness and a live MCP endpoint, so its
        // shape is asserted instead.
        let chat = in1b_production_half(include_str!("chat_ui.rs"));
        assert!(
            chat.contains(
                "self.note_label(\n                LabelIncident::AgentBranch,\n                IncidentSubject::Agent,\n                \"Could not create an isolated agent branch\","
            ),
            "Appendix B row 48 queues `agent_branch_unclassified` against the chat panel"
        );
    }

    /// Label 7 of 17. Appendix B rows 128-129's site: a filler-word removal
    /// with no transcript behind it.
    #[test]
    fn in1b_transcript_edit_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.remove_filler_words();
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "transcript_edit_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// Label 8 of 17, and erratum `IN1b`-A-R9's clause: Appendix B row 27's
    /// merged `MediaEvent::Error` sink, whose code comes from
    /// `MediaError::recovery_code()` at run time. The two **non-colour**
    /// `Media` code groups carry subject `Project`, not `Asset`, because
    /// neither `UnsupportedDecoderFormat` nor `Backend` holds an `AssetId`
    /// to name (erratum `IN1b`-A-R2's added clause amends row 27).
    #[test]
    fn in1b_media_produces_its_declared_code_class_and_severity() {
        // First, the arm itself, driven: a **real** typed decode refusal from
        // the real engine, delivered on the app's own `media_events` channel,
        // reaches `poll_background`'s `MediaEvent::Error` drain and becomes an
        // incident with no `else` branch left to take (Appendix B row 27,
        // merged by `IN1b` §5.1 rule 12). The channel is replaced so the
        // delivery is deterministic; everything after it is production's.
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_driven_fixture, driven_document) = in1_untagged(&probe_engine);
        let (mut driven, engine) = in1_harness(driven_document);
        let document = driven.projects[0].document.as_ref().clone();
        let real_error = in1_playback_error(&engine, &document);
        let (media_tx, media_rx) = crossbeam_channel::unbounded();
        driven.media_events = media_rx;
        driven.playing = true;
        media_tx
            .send(MediaEvent::Error(real_error))
            .expect("the app owns the receiving half");
        let ctx = egui::Context::default();
        driven.poll_background(&ctx);
        assert!(!driven.playing, "the arm stops playback before it observes");
        {
            let handle = Arc::clone(&driven.projects[0].incidents);
            let log = handle
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 1, "the merged arm opened exactly one incident");
            let incident = log.all().next().expect("one incident");
            assert!(
                matches!(incident.code, kinewright_core::IncidentCode::SourceColor(_)),
                "the fixture's refusal is a source-colour one: {}",
                incident.code.code()
            );
            assert!(
                matches!(incident.subject, IncidentSubject::Asset(_)),
                "the one variant that knows its own subject overrides the fallback"
            );
        }
        in1_shutdown(&mut driven);

        // Then the two **non-colour** `Media` code groups, which no fixture in
        // this repository produces: they are built through the same total
        // constructor the arm calls, with the same fallback subject the arm
        // passes.
        let (_fixture, mut app) = in1b_app();
        let revision = app.focused().revision;
        app.note_observation(IncidentObservation::from_media_error(
            &MediaError::Backend("the engine refused".to_owned()),
            IncidentSubject::Project,
            revision,
        ));
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "media_backend_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );

        // The other non-colour group, on its own app so the log stays at one.
        let (_second_fixture, mut second) = in1b_app();
        let revision = second.focused().revision;
        second.note_observation(IncidentObservation::from_media_error(
            &MediaError::UnsupportedDecoderFormat {
                path: PathBuf::from("clip.mov"),
                format: "yuv444p12le".to_owned(),
                declared_bit_depth: Some(12),
                decoder_bit_depth: None,
                reason: "not a supported integer source surface".to_owned(),
            },
            IncidentSubject::Project,
            revision,
        ));
        let second_incident = in1b_route_one(&mut second);
        in1b_declares(
            &second_incident,
            "unsupported_decoder_format",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
        in1_shutdown(&mut second);
    }

    /// Label 9 of 17. Appendix B row 34's site, whose arm needs a live MCP
    /// endpoint; the arm's own arguments go through the same shorthand.
    #[test]
    fn in1b_agent_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.note_label(
            kinewright_core::LabelIncident::Agent,
            IncidentSubject::Agent,
            "The Kinewright agent server is unavailable",
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "agent_unclassified",
            IncidentSubject::Agent,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);

        // The arm needs a live MCP endpoint, so its shape is asserted instead.
        // All five `Agent` rows take the same code and the same subject.
        let chat = in1b_production_half(include_str!("chat_ui.rs"));
        assert_eq!(
            chat.matches("LabelIncident::Agent,").count(),
            5,
            "five `Agent` rows, every one against the chat panel"
        );
        assert!(
            chat.contains(
                "self.note_label(\n                LabelIncident::Agent,\n                IncidentSubject::Agent,\n                \"The Kinewright agent server is unavailable\","
            ),
            "Appendix B row 34 queues `agent_unclassified` against the chat panel"
        );
    }

    // Label 10 of 17 — `Recording` — lives in `recording.rs`'s own test
    // module as `in1b_recording_produces_its_declared_code_class_and_severity`,
    // because `start_recording_from_dialog` and `note_recording_failure` are
    // private to that module and a test that drives the real arm has to be
    // written beside them (review-app-2 S3).

    /// Label 11 of 17, and `IN1b` §5.7's **third** sink: Appendix B row 28,
    /// the crash-recovery restore that used to reach the person as a bare
    /// status string with no log write anywhere on the path.
    #[test]
    fn in1b_project_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        let status = crate::recovery::restore_status(Err("the journal is truncated".to_owned()));
        let observation = status.expect_err("a failed restore returns an observation");
        app.note_observation(*observation);
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "project_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert!(
            incident.observed.contains("Could not restore unsaved work"),
            "the sentence the status bar used to carry alone survives in `observed`"
        );
        assert_eq!(
            crate::recovery::restore_status(Ok(())),
            Ok("Recovered unsaved work".to_owned()),
            "the success arm still returns its string as it always did"
        );
        in1_shutdown(&mut app);
    }

    /// Label 12 of 17. Appendix B row 29's site: a noise learn with no
    /// silence on the chain. The subject is `Chain`, not `Track`: all three
    /// Mixer sites are `AudioChain`-scoped and no `TrackId` is in scope at
    /// any of them (N2.5/B3).
    #[test]
    fn in1b_mixer_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.request_noise_profile(AudioChain::Master, EffectId(1));
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "mixer_unclassified",
            IncidentSubject::Chain(AudioChain::Master),
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert_eq!(incident.subject.label(), "Master");
        in1_shutdown(&mut app);
    }

    /// Label 13 of 17. Appendix B row 32's site: captions asked for before a
    /// transcript exists.
    #[test]
    fn in1b_captions_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.add_captions();
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "captions_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// Label 14 of 17. Appendix B row 46: the branch preview's own typed
    /// `MediaError`, whose subject is the chat panel rather than an asset.
    #[test]
    fn in1b_branch_preview_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        let revision = app.focused().revision;
        app.note_observation(IncidentObservation::from_media_error(
            &MediaError::Backend("no frame at that position".to_owned()),
            IncidentSubject::Agent,
            revision,
        ));
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "media_backend_unclassified",
            IncidentSubject::Agent,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);

        // The arm needs a branch whose thumbnail renderer refuses, so its
        // shape is asserted instead: the code is resolved at run time from
        // `MediaError::recovery_code()` and the subject is the chat panel.
        let chat = in1b_production_half(include_str!("chat_ui.rs"));
        assert!(
            chat.contains(
                "IncidentObservation::from_media_error(\n                    &error,\n                    IncidentSubject::Agent,"
            ),
            "Appendix B row 46 resolves its code from the typed media failure"
        );
    }

    /// Label 15 of 17. Appendix B row 88's site, **driven**: a refused cache
    /// clear arriving on the worker's own channel. The cache is the project's,
    /// not an asset's.
    #[test]
    fn in1b_media_cache_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.cache_clear_tx
            .send((
                kinewright_core::MediaCacheFamily::VisualAssets,
                Err(MediaError::Backend("permission denied".to_owned())),
            ))
            .expect("the app owns the receiving half");
        let ctx = egui::Context::default();
        app.poll_media_workflow(&ctx);
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "media_cache_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert!(
            incident.observed.contains("VisualAssets"),
            "the family the person asked to clear is named"
        );
        in1_shutdown(&mut app);
    }

    /// Label 16 of 17 — the `"Incident"` source itself. Appendix B row 13 is
    /// not migrated: it **becomes** the single `note_incident` call site, and
    /// the audit line keeps the fixed source `"Incident"` (`IN1b` §5.3
    /// rule 18). The label's test is therefore that the audit view still
    /// records one line per opened incident, under that source, whatever the
    /// incident's own code says.
    #[test]
    fn in1b_incident_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        app.note_label(
            kinewright_core::LabelIncident::Captions,
            IncidentSubject::Project,
            "a caption sidecar could not be written",
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "captions_unclassified",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            1,
            "one audit line per opened incident, under the fixed source"
        );
        assert_eq!(
            app.status,
            crate::incident_ui::incident_headline(incident.code, incident.class),
            "and the status bar carries the headline, not a label"
        );
        in1_shutdown(&mut app);
    }

    /// Label 17 of 17 — `"Timeline"`, which appears at no literal call site of
    /// the old sink at all and is reachable only through
    /// `InspectorEdits::incident_code()` (`IN1b` §2.1 rule 3, §5.4 rule 27).
    /// A per-label census of the sink would have been short by exactly this
    /// one.
    #[test]
    fn in1b_timeline_produces_its_declared_code_class_and_severity() {
        let (_fixture, mut app) = in1b_app();
        let clip = app.focused().document.tracks[0].clips[0].id;
        app.focused_mut().selected_clip = Some(clip);
        let mut edits = crate::inspector_ui::InspectorEdits::default();
        edits.set_incident_code(crate::timeline_ui::ROOM_TONE_INCIDENT_CODE);
        edits.push_error("That track has no gap to fill");
        app.submit_inspector_edits(edits);
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "timeline_unclassified",
            IncidentSubject::Clip(clip),
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// `IN1b` §5.4 rule 27's live bug, fixed in kind: the inspector's loop
    /// used to write *n* unrelated log lines for *n* messages under one
    /// category. Typed at the producer, the incident key
    /// `(code, subject, observed)` collapses the repeats and keeps the
    /// distinct ones.
    #[test]
    fn in1b_repeated_inspector_messages_collapse_to_one_incident_each() {
        let (_fixture, mut app) = in1b_app();
        let clip = app.focused().document.tracks[0].clips[0].id;
        app.focused_mut().selected_clip = Some(clip);
        let mut edits = crate::inspector_ui::InspectorEdits::default();
        edits.push_error("the same refusal");
        edits.push_error("the same refusal");
        edits.push_error("a different refusal");
        app.submit_inspector_edits(edits);
        assert_eq!(app.pending_observations.len(), 3);
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            log.len(),
            2,
            "three messages, two distinct problems: the dedup key collapses the repeat"
        );
        assert_eq!(log.open_count(), 2);
        drop(log);
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            2,
            "and only the newly opened incidents write audit lines"
        );
        in1_shutdown(&mut app);
    }

    /// `IN1b` §7 item 18 and §9 clause 6's second half: the two behaviours
    /// deleting `record_error` would otherwise have dropped in silence. No
    /// test in this crate read `app.status` at `d4ed8eb` — 34 assertions
    /// mention `status` and none reads the field — so without this both
    /// regressions would ship under a green gate (N1/B5).
    #[test]
    fn in1b_note_incident_writes_the_status_and_opens_the_window() {
        let (_fixture, mut app) = in1b_app();
        app.error_log_open = false;
        "Ready".clone_into(&mut app.status);
        app.note_label(
            kinewright_core::LabelIncident::Mixer,
            IncidentSubject::Chain(AudioChain::Bus(kinewright_core::AudioBusId(3))),
            "no silence on this bus",
        );
        let incident = in1b_route_one(&mut app);
        assert_eq!(
            app.status,
            crate::incident_ui::incident_headline(incident.code, incident.class),
            "the status bar carries the incident's headline"
        );
        assert_ne!(app.status, "Ready");
        assert!(app.error_log_open, "and the window pops open");
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        assert_eq!(incident.subject.label(), "Bus 3");
        in1_shutdown(&mut app);
    }

    /// `IN1b` §7 item 22 and §9 clause 11's first half, **driven through the
    /// real site**: `open_project` on a project whose media has moved opens
    /// **one** incident, whose `observed` is the joined list, because one open
    /// with *n* missing files is one problem (`IN1b` §5.1 rule 14).
    ///
    /// Hand-feeding the joined string would prove `note_label`, not the site:
    /// a site that pushed *n* separate observations would pass it unchanged
    /// (review-app-1 B1). This writes a real project file whose three assets
    /// are all elsewhere and opens it, so the aggregation decision under test
    /// is `app.rs:898`'s and nothing else's.
    #[test]
    fn in1b_one_open_with_n_missing_files_opens_one_incident() {
        let (_fixture, mut app) = in1b_app();
        let temporary = TempDirectory::new("in1b-missing-media");
        let mut document = app.projects[0].document.as_ref().clone();
        let missing_names: Vec<String> = document
            .media_pool
            .iter_mut()
            .enumerate()
            .map(|(index, asset)| {
                asset.path = temporary.path(&format!("gone-{index}.mov"));
                format!("{} ({})", asset.name, asset.path.display())
            })
            .collect();
        assert!(
            !missing_names.is_empty(),
            "the fixture document carries at least one asset"
        );
        assert!(
            document
                .media_pool
                .iter()
                .all(|asset| !asset.path.is_file()),
            "every asset's path is somewhere it is not"
        );
        let project_path = temporary.path("missing.kinewright");
        std::fs::write(
            &project_path,
            serde_json::to_string(&document).expect("the fixture document serialises"),
        )
        .expect("the temporary project file is writable");

        app.open_project(&project_path);
        assert_eq!(app.projects.len(), 2, "the project opened");
        assert_eq!(
            app.pending_observations.len(),
            1,
            "one open with n missing files queues one observation, not n"
        );
        let incident = in1b_route_focused_one(&mut app);
        in1b_declares(
            &incident,
            "media_incomplete",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Degrades,
        );
        for name in &missing_names {
            assert!(
                incident.observed.contains(name.as_str()),
                "the joined list is the incident's `observed`, not n incidents: {name}"
            );
        }
        // The visible behaviour change `IN1b` §5.1 rule 14 wants and names:
        // at `d4ed8eb` this site pushed straight into a window it never
        // opened, so the person saw only the status string.
        assert!(app.error_log_open);
        assert_eq!(
            app.status,
            crate::incident_ui::incident_headline(incident.code, incident.class)
        );
        in1_shutdown(&mut app);
    }

    /// `IN1b` §7 item 23 and §9 clause 11's second half, **driven through the
    /// real site**: `poll_background`'s
    /// `for (asset, error) in self.visual_cache.poll(ctx)` loop opens *n*
    /// incidents for *n* visual-cache failures, because a per-asset failure is
    /// per-asset by construction — and the same asset failing twice is still
    /// one problem.
    ///
    /// The cache is rebuilt over a test channel so the failures are the ones
    /// this test names; everything downstream of `VisualCache::poll` — the
    /// loop, the subject, the code and the router — is production's.
    #[test]
    fn in1b_n_visual_cache_failures_open_n_asset_incidents() {
        let (_fixture, mut app) = in1b_app();
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        app.visual_cache = crate::visual_cache::VisualCache::new(results_rx);
        for (asset, name) in [
            (kinewright_core::AssetId(1), "one.mov"),
            (kinewright_core::AssetId(2), "two.mov"),
            (kinewright_core::AssetId(3), "three.mov"),
            (kinewright_core::AssetId(2), "two.mov"),
        ] {
            results_tx
                .send(kinewright_core::VisualAssetResult::Failed {
                    asset,
                    request_generation: 0,
                    path: PathBuf::from(name),
                    request: kinewright_core::VisualRequestKind::Waveform,
                    message: format!("no decoder for {name}"),
                })
                .expect("the test channel accepts the failure");
        }
        // `poll_background` drains the cache, queues the observations **and**
        // ticks the router on its way out (`app.rs:1982`), so the log is the
        // only place left to read: four failures over three assets become
        // three incidents, which a single joined observation could not.
        let ctx = egui::Context::default();
        app.poll_background(&ctx);
        assert!(
            app.pending_observations.is_empty(),
            "the tick at the end of the poll drained them"
        );
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 3, "three assets, three problems");
        let mut subjects: Vec<IncidentSubject> =
            log.all().map(|incident| incident.subject).collect();
        subjects.sort_unstable();
        assert_eq!(
            subjects,
            vec![
                IncidentSubject::Asset(kinewright_core::AssetId(1)),
                IncidentSubject::Asset(kinewright_core::AssetId(2)),
                IncidentSubject::Asset(kinewright_core::AssetId(3)),
            ]
        );
        for incident in log.all() {
            assert_eq!(incident.code.code(), "media_incomplete");
            assert_eq!(
                incident.severity,
                kinewright_core::IncidentSeverity::Degrades
            );
            assert!(
                incident
                    .observed
                    .starts_with("Could not build timeline visuals")
            );
        }
        let repeated = log
            .all()
            .find(|incident| {
                incident.subject == IncidentSubject::Asset(kinewright_core::AssetId(2))
            })
            .expect("the repeated asset");
        assert_eq!(repeated.count, 2, "the second observation deduped");
        assert_eq!(log.open_count(), 3);
        drop(log);
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            3,
            "one audit line per opened incident, not per failure"
        );
        in1_shutdown(&mut app);
    }

    /// The third aggregate of `IN1b` §5.1 rule 14, which shares item 22's
    /// clause: `open_project`'s unavailable-LUT list is **one** incident too,
    /// and its `observed` is the joined list of titles.
    ///
    /// The site is `app.rs:880`, the `lut_titles` bypass. It is asserted over
    /// the source rather than driven, because reaching it needs a saved
    /// project whose LUT store root exists and whose document names assets the
    /// store cannot produce — a fixture no test in this crate builds. The
    /// shape is what rule 14 fixes, so the shape is what is asserted, in the
    /// idiom `in1b_a_branch_conflict_opens_an_edit_revision_conflict_incident`
    /// uses for the same reason.
    #[test]
    fn in1b_the_unavailable_look_list_is_one_incident_with_a_joined_observed() {
        let source = in1b_app_source();
        let (_, after) = source
            .split_once("if !lut_titles.is_empty() {")
            .expect("the `lut_titles` aggregate is still at its site");
        // The block ends at the first close-brace at two levels of
        // indentation, which is where `open_project`'s own body puts it. It
        // `.expect()`s, so a site that moved fails this loudly rather than
        // passing silently (review-final nit 5).
        let body = after
            .split_once("\n        }\n")
            .expect("the aggregate's block ends")
            .0;
        assert_eq!(
            body.matches("note_transient_label(").count(),
            1,
            "one open with n unavailable looks queues one transient observation"
        );
        assert!(
            body.contains("LabelIncident::LookIncomplete"),
            "the project opened, so the severity is a degraded result"
        );
        assert!(
            body.contains("lut_titles.join(\", \")"),
            "and the joined list is the incident's `observed`"
        );
        assert!(
            !source.contains(concat!("self.error_log.push", "(")),
            "no bypass survives anywhere in the file"
        );
    }

    /// `IN1b` §7 item 24 and §9 clause 12's first half: **both**
    /// `BranchApplyOutcome::Conflict` arms — the merge and the cherry-pick —
    /// open an `edit_revision_conflict` incident against the chat panel.
    /// Neither wrote to `ErrorLog` at `d4ed8eb`; the merge arm wrote the
    /// transcript and `self.status`, and the cherry-pick arm wrote
    /// `self.status` and nothing else at all (`IN1b` §5.7).
    #[test]
    fn in1b_a_branch_conflict_opens_an_edit_revision_conflict_incident() {
        let (_fixture, mut app) = in1b_app();
        let expected = TimelineRevision(4);
        let actual = TimelineRevision(7);
        app.note_observation(IncidentObservation::revision_conflict(
            IncidentSubject::Agent,
            expected,
            actual,
        ));
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "edit_revision_conflict",
            IncidentSubject::Agent,
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert_eq!(
            incident.observed, "7",
            "the revision the document is on now"
        );
        assert_eq!(incident.allowed.as_deref(), Some("4"));
        assert_eq!(incident.revision, actual);
        assert_eq!(
            incident.evidence,
            kinewright_core::IncidentEvidence::Revision { expected, actual }
        );
        // The second arm is the same observation on the same subject, so the
        // dedup key collapses it: one chat panel losing two races to the same
        // pair of revisions is one problem.
        app.note_observation(IncidentObservation::revision_conflict(
            IncidentSubject::Agent,
            expected,
            actual,
        ));
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 1);
        assert_eq!(log.all().next().expect("one").count, 2);
        drop(log);

        // Both arms reach that observation, and neither still writes a bare
        // sentence. Asserted over the source because a `BranchApplyOutcome`
        // conflict needs a live agent harness and an isolated branch to
        // provoke, which `IN1b` §9's terms exclude; this is the same shape
        // `au5_a_room_tone_refusal_is_not_filed_under_look` uses for the same
        // reason.
        let chat = in1b_production_half(include_str!("chat_ui.rs"));
        assert_eq!(
            chat.matches("BranchApplyOutcome::Conflict { expected, actual }")
                .count(),
            2,
            "the branch merge and the cherry-pick"
        );
        assert_eq!(
            chat.matches("IncidentObservation::revision_conflict")
                .count(),
            2,
            "and both of them open the incident rather than writing a string"
        );
        assert!(
            !chat.contains("Cherry-pick stopped: expected live revision"),
            "the cherry-pick arm's bare status sentence is gone entirely"
        );
        assert!(
            chat.contains("Branch merge stopped: it was based on live revision"),
            "the merge arm keeps its transcript entry: the chat is the thread's own record"
        );
        in1_shutdown(&mut app);
    }

    /// `IN1b` §5.5 rule 31: the Incidents panel's pure view. Placement is not
    /// tested by contract (`IN1b` §10 limit 1); what is tested is that the
    /// panel lists **every** open incident whatever its subject, which is the
    /// property the Media panel's deliberate asset-only filter does not have.
    #[allow(
        clippy::too_many_lines,
        reason = "the regression test names every subject and its panel state"
    )]
    #[test]
    fn in1b_the_incidents_panel_lists_every_open_subject() {
        let (_fixture, mut app) = in1b_app();
        let asset = app.focused().document.media_pool[0].id;
        app.note_label(
            kinewright_core::LabelIncident::Relink,
            IncidentSubject::Asset(asset),
            "no verified fingerprint",
        );
        app.note_label(
            kinewright_core::LabelIncident::Mixer,
            IncidentSubject::Chain(AudioChain::Master),
            "no silence",
        );
        app.note_label(
            kinewright_core::LabelIncident::Project,
            IncidentSubject::Project,
            "could not be read",
        );
        app.note_label(
            kinewright_core::LabelIncident::Agent,
            IncidentSubject::Agent,
            "harness missing",
        );
        // The eighth subject shape (stage A addendum 2): a look that keeps
        // refusing is one problem, on its own axis.
        app.note_label(
            kinewright_core::LabelIncident::Look,
            IncidentSubject::LutAsset(kinewright_core::LutAssetId(7)),
            "the stored bytes do not match the recorded hash",
        );
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let investigating_id = {
            let mut log = handle
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let id = log
                .all()
                .find(|incident| {
                    incident.code
                        == kinewright_core::IncidentCode::Label(
                            kinewright_core::LabelIncident::Agent,
                        )
                })
                .expect("the agent incident exists")
                .id;
            assert!(log.begin_investigation(id));
            id
        };
        let investigating = crate::investigator::InvestigatingCard {
            id: investigating_id,
            turns: 1,
            max_turns: 6,
            input_tokens: 12,
            output_tokens: 8,
            tokens_known: true,
            max_tokens: 40_000,
            elapsed_secs: 1,
            max_wall_secs: 90,
        };
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_pairs = crate::investigator::InvestigatorSessionPairs::default();
        let rows = crate::incident_ui::incident_panel_rows(
            log.all(),
            Some(investigating),
            &session_pairs,
            &empty_panel_sets(),
        );
        assert_eq!(rows.len(), 5, "every open incident, not only the asset one");
        let labels: Vec<&str> = rows
            .iter()
            .map(|row| row.view.subject_label.as_str())
            .collect();
        assert!(labels.contains(&"Master"));
        assert!(labels.contains(&"Project"));
        assert!(labels.contains(&"Agent"));
        assert!(labels.contains(&"Look 7"));
        assert!(labels.iter().any(|label| label.starts_with("Asset ")));
        let investigating_rows = rows
            .iter()
            .filter(|row| row.view.state_label == "Investigating")
            .count();
        assert_eq!(
            investigating_rows, 1,
            "the running row is visible in the panel"
        );
        for row in &rows {
            if row.view.state_label == "Investigating" {
                let counter_rows = row
                    .view
                    .details
                    .iter()
                    .filter(|(key, _)| matches!(*key, "turns" | "tokens" | "elapsed"))
                    .count();
                assert_eq!(counter_rows, 3, "the running row has three counters");
                assert_eq!(row.view.actions.len(), 1);
                continue;
            }
            assert_eq!(row.view.state_label, "Open");
            assert_eq!(
                row.view.actions.len(),
                1,
                "one explanation, nothing to press"
            );
            assert!(!row.view.actions[0].enabled);
        }
        assert_eq!(
            app.open_incident_count(),
            5,
            "the badge counts what the panel lists"
        );
        drop(log);
        in1_shutdown(&mut app);
    }

    /// `IN1b` §5.2 rule 16 and IN1 §5.2b rule 20: a tick with nothing queued
    /// changes nothing, and a second tick over the same log changes nothing
    /// either.
    #[test]
    fn in1b_a_second_tick_over_the_same_observations_changes_nothing() {
        let (_fixture, mut app) = in1b_app();
        app.note_label(
            kinewright_core::LabelIncident::Export,
            IncidentSubject::ExportJob,
            "Choose an export output path",
        );
        app.route_incidents();
        let before = (
            app.open_incident_count(),
            app.error_log.count_with_source("Incident"),
            app.status.clone(),
        );
        app.route_incidents();
        app.route_incidents();
        assert_eq!(
            (
                app.open_incident_count(),
                app.error_log.count_with_source("Incident"),
                app.status.clone(),
            ),
            before,
            "a quiet tick writes nothing"
        );
        assert!(app.pending_observations.is_empty());
        assert!(app.pending_router_applies.is_empty());
        in1_shutdown(&mut app);
    }

    /// IN1 §9 clause 3, app half, through a real decode: the untagged
    /// fixture opens exactly one incident of class `AutoApply` with the
    /// pinned headline and zero of class `AskFirst`, and the recovery
    /// applies cleanly and leaves the §3 rule 8 tuple. The zero
    /// `Recovery`-questions half is discharged by construction — no router
    /// path constructs a `HumanQuestion` — and the managed decode of the
    /// amended description is media fixture 9,
    /// `in1_the_assumed_description_decodes_managed`.
    #[test]
    fn in1_the_person_path_opens_one_auto_apply_incident_with_the_pinned_headline() {
        let engine = Arc::new(FfmpegMediaEngine::new().expect("the test engine starts"));
        let (_fixture, document) = in1_untagged(&engine);
        let error = in1_playback_error(&engine, &document);
        let mut log = IncidentLog::default();
        let observation = IncidentObservation::from_media_error(
            &error,
            IncidentSubject::Project,
            TimelineRevision::default(),
        );
        let Observed::Opened(id) = log.observe(observation) else {
            panic!("a fresh log opens the fixture's incident");
        };
        assert_eq!(log.len(), 1);
        assert_eq!(log.open_count(), 1);
        let incident = log.get(id).expect("open");
        assert_eq!(
            incident.class,
            PolicyClass::AutoApply,
            "read from the incident's own field, not re-derived"
        );
        assert_eq!(
            log.all()
                .filter(|incident| incident.class == PolicyClass::AskFirst)
                .count(),
            0,
            "Part A declares no AskFirst code"
        );
        assert_eq!(
            incident_card(incident, false, &CardLoadedFlags::default()).headline,
            "Kinewright assumed Rec.709 for this source because its colour primaries were unknown."
        );
        let IncidentSubject::Asset(asset_id) = incident.subject else {
            panic!("the fixture incident is asset-scoped")
        };
        let recoveries = policy_recovery(incident.code, incident.subject, &incident.evidence);
        assert_eq!(recoveries.len(), 1);
        let RecoveryKind::Operation(operation) = &recoveries[0].kind else {
            panic!("an AutoApply recovery is an operation");
        };
        let mut amended = document.clone();
        operation
            .apply(&mut amended)
            .expect("the recovery applies cleanly");
        let asset = amended.asset(asset_id).expect("the asset survives");
        assert_eq!(
            asset.assumed_from.as_ref(),
            incident.evidence.probed(),
            "the recovery records the exact probed description it replaced"
        );
        let description = &asset.color_description;
        assert_eq!(description.primaries, ColorPrimaries::Bt709);
        assert_eq!(description.transfer, ColorTransfer::Bt709);
        assert_eq!(description.matrix, ColorMatrix::Bt709);
        assert_eq!(description.range, ColorRange::Limited);
        assert_eq!(description.white_point, ColorWhitePoint::D65);
        assert_eq!(description.bit_depth, ColorBitDepth::Eight);
        assert_eq!(
            description.confidence_basis_points,
            COLOR_CONFIDENCE_MAX_BASIS_POINTS
        );
        assert_eq!(description.provenance, ColorProvenance::AgentAssumption);
    }

    /// IN1 §9 clause 4(a): however many errors the engine emits for the
    /// fixture, exactly one incident is open. Collects every refusal the
    /// `set_document` plus one `request_frame` pair produces and feeds them
    /// all to one log; the emission count itself is deliberately unpinned
    /// (IN1 §11.1 limit 1).
    #[test]
    fn in1_the_fixture_errors_open_exactly_one_incident() {
        let engine = Arc::new(FfmpegMediaEngine::new().expect("the test engine starts"));
        let (_fixture, document) = in1_untagged(&engine);
        engine.set_document(Arc::new(document));
        engine.request_frame(TimeCode::ZERO);
        let events = engine.events();
        let mut errors = Vec::new();
        let expiry = Instant::now() + Duration::from_secs(2);
        while Instant::now() < expiry {
            while let Ok(event) = events.try_recv() {
                if let MediaEvent::Error(error) = event {
                    errors.push(error);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !errors.is_empty(),
            "the fixture must refuse, or the test proves nothing"
        );
        let mut log = IncidentLog::default();
        for error in &errors {
            let observation = IncidentObservation::from_media_error(
                error,
                IncidentSubject::Project,
                TimelineRevision::default(),
            );
            log.observe(observation);
        }
        assert_eq!(log.open_count(), 1);
        assert_eq!(log.len(), 1);
        let incident = log.all().next().expect("one incident");
        assert!(
            incident.count >= 1,
            "clause 4(a) counts the open incident, not the arrivals"
        );
        assert_eq!(
            incident.count,
            u32::try_from(errors.len()).expect("fewer than four billion refusals"),
            "every repeat arrival dedups into the one incident"
        );
    }

    /// IN1 §9 clause 9 through the real router and a real `Core` actor: a
    /// router auto-apply issued against a stale revision produces
    /// `Event::RevisionConflict` and the incident stays `Open`, with its
    /// `revision` refreshed to the one core reported and no second apply
    /// (IN1 §5.2 rule 10). The observation is stale on purpose — a person's
    /// edit lands between the drain that observed it and the tick that sends
    /// it — and the error itself is a real decode refusal.
    #[test]
    fn in1_a_stale_router_apply_leaves_the_incident_open_without_reapplying() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);
        // Advance the live revision past the observation's with the recovery
        // itself, sent plainly the way a person's edit would land first.
        let probe_asset = document.media_pool.first().expect("one asset").clone();
        let advance = assume_rec709_operation(probe_asset.id, &probe_asset.color_description);
        app.projects[0]
            .core
            .send(Command::Do(advance))
            .expect("the actor takes commands");
        // The wait itself is the assertion that the person's edit landed
        // first: it returns only once the revision has moved off the default.
        in1_drain_until_revision(&mut app, 0, TimelineRevision::default());
        let stale = TimelineRevision::default();
        let observation =
            IncidentObservation::from_media_error(&error, IncidentSubject::Project, stale);
        app.pending_observations.push(observation);
        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "the router still sends once against the stale revision"
        );

        // Erratum `IN1b`-C-R50's window: one ordinary tick between the send
        // and the actor's reply. The person already wrote the recovery's bytes
        // by hand, so `router_apply_accepted` reads `true` here — and the send
        // must still be outstanding, because the actor has not answered it and
        // is about to refuse it. Before C-R50 this tick recorded the incident
        // `Resolved(Applied)` off the person's edit, and the conflict that
        // arrived afterwards matched nothing and could not correct it.
        assert!(
            router_apply_accepted(
                &app.projects[0].document,
                &in1_incident(&app, app.pending_router_applies[0].incident),
                IncidentOutcome::Applied,
            ),
            "the person's hand edit makes the document read say `landed`"
        );
        assert!(app.pending_router_landings.is_empty());
        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "a tick with no landing for this token resolves nothing"
        );
        assert_eq!(
            in1_incident(&app, app.pending_router_applies[0].incident).state,
            IncidentState::Open,
            "and the incident is not recorded as applied off someone else's edit"
        );

        in1_drain_until_conflict(&mut app, 0);
        assert!(
            !app.pending_router_conflicts.is_empty(),
            "the actor refused the stale send"
        );
        app.route_incidents();
        assert!(
            app.pending_router_applies.is_empty(),
            "a conflicted send is dropped, never re-sent"
        );
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 1);
        assert_eq!(log.len(), 1);
        let incident = log.all().next().expect("one incident");
        assert_eq!(incident.state, IncidentState::Open);
        // Rule 10's remaining half: the conflict carried `actual`, and
        // `refresh_revision` moved the incident onto it. It no longer states
        // itself against the stale revision the send was planned for.
        assert_eq!(
            incident.revision, app.projects[0].revision,
            "the incident is refreshed to the revision core reported"
        );
        assert_ne!(
            incident.revision, stale,
            "the stale revision is not left on the record"
        );
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        drop(log);
        // And the suppressed re-observe is what makes "no second apply" a
        // property of the log rather than of this test's patience: the same
        // observation now yields `Suppressed`, which the router answers with
        // nothing at all.
        let repeat = IncidentObservation::from_media_error(
            &error,
            IncidentSubject::Project,
            app.projects[0].revision,
        );
        let handle = Arc::clone(&app.projects[0].incidents);
        let mut log = handle
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.observe(repeat), Observed::Suppressed);
        drop(log);
        app.route_incidents();
        assert!(app.pending_router_applies.is_empty());
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        in1_shutdown(&mut app);
    }

    /// `IN1b` §4 rules 6-8 when **two** auto-applies go out at the same
    /// expected revision in one project: the conflict core reports is matched
    /// to the send it refused **by token identity**, never to the send that
    /// landed.
    ///
    /// Two incidents opened in the same frame carry the same
    /// `observation.revision`, so both `RouterApply`s carry the same
    /// `expected` and `(project, expected)` does not identify a send. The
    /// actor is serial: the first command lands and moves the revision, the
    /// second is refused against it, and the `Event::RevisionConflict` echoes
    /// the refused command's own token (`IN1b` §4 rule 5).
    ///
    /// This test does **not** discriminate against Part A's two-tier
    /// `(project, expected)` heuristic and `IN1b` §4 rule 8 says so: the
    /// heuristic's first tier already handled this case. The discriminating
    /// test is `in1b_a_foreign_conflict_leaves_every_router_send_outstanding`.
    ///
    /// The second asset is a second pool entry over the same untagged fixture
    /// (same probed description, its own `AssetId`, no clip of its own), and
    /// the second observation is the first one with its subject changed: the
    /// engine emits one refusal per played clip, and what this test needs is
    /// two `AutoApply` incidents at one revision, not two decodes.
    #[test]
    fn in1b_a_conflict_is_matched_to_the_refused_send_by_token() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, mut document) = in1_untagged(&probe_engine);
        let first_asset = document.media_pool.first().expect("one asset").clone();
        let second_id = kinewright_core::AssetId(first_asset.id.0 + 1);
        let mut second_asset = first_asset.clone();
        second_asset.id = second_id;
        document.media_pool.push(second_asset);
        let (mut app, engine) = in1_harness(document);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);

        let revision = app.projects[0].revision;
        let first_observation =
            IncidentObservation::from_media_error(&error, IncidentSubject::Project, revision);
        assert_eq!(
            first_observation.subject,
            IncidentSubject::Asset(first_asset.id)
        );
        let mut second_observation = first_observation.clone();
        second_observation.subject = IncidentSubject::Asset(second_id);
        app.pending_observations.push(first_observation);
        app.pending_observations.push(second_observation);

        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            2,
            "both incidents are AutoApply and both are sent"
        );
        assert!(
            log_revisions(&app)
                .iter()
                .all(|incident_revision| *incident_revision == revision),
            "both sends are gated on the one revision the observations carried"
        );
        // The identity the match now uses: two distinct tokens over one
        // `(project, expected)` pair (`IN1b` §4 rule 6).
        let tokens: Vec<CommandToken> = app
            .pending_router_applies
            .iter()
            .map(|apply| apply.token)
            .collect();
        assert_ne!(tokens[0], tokens[1], "each router send has its own token");

        // Erratum `IN1b`-C-R50's window again, on two sends: one ordinary tick
        // before the actor's replies have been drained resolves neither, even
        // though the first of them has in fact landed in the actor.
        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            2,
            "no landing token has come back yet, so neither send is reconciled"
        );

        // The actor applies the first and refuses the second — drained and
        // reconciled by **production**, `poll_background`, rather than by the
        // test mirror (review-final B1). `poll_background` reconciles on its
        // way out, so the settled condition is the effect: both sends gone.
        in1_poll_production_until(&mut app, |app| app.pending_router_applies.is_empty());
        assert_ne!(
            app.projects[0].revision, revision,
            "the first send landed and moved the revision"
        );
        let actual = app.projects[0].revision;
        assert!(
            app.pending_router_conflicts.is_empty(),
            "the production tick reconciles the conflict it drained"
        );
        // Review-final B1's other half: the landing the production tick
        // recorded is what let the landed send resolve at all — the
        // `Resolved(Applied)` assertion below is its proof, and deleting the
        // push in `note_core_event_for_router` fails that assertion for every
        // router auto-apply in the session. The landings list itself is an
        // intermediate state: when the actor's landing and its conflict are
        // drained in one tick it still holds the landed token here, and when
        // a slower actor (the Windows CI lane) spreads them over two ticks the
        // second tick has already spent it. Both are correct, so neither is
        // asserted; only the settled end is.
        assert!(
            app.pending_router_landings.is_empty()
                || app.pending_router_landings == vec![tokens[0]],
            "the landings list holds at most the landed send's own token"
        );
        app.route_incidents();
        assert!(
            app.pending_router_landings.is_empty(),
            "a quiet tick leaves no landing behind, because nothing is outstanding"
        );

        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 2);
        let landed = log
            .all()
            .find(|incident| incident.subject == IncidentSubject::Asset(first_asset.id))
            .expect("the first incident");
        let refused = log
            .all()
            .find(|incident| incident.subject == IncidentSubject::Asset(second_id))
            .expect("the second incident");
        assert_eq!(
            landed.state,
            IncidentState::Resolved(IncidentOutcome::Applied),
            "the send that landed is resolved, not consumed by the conflict"
        );
        assert_eq!(
            landed.revision, revision,
            "the landed incident's revision is not the one refreshed"
        );
        assert_eq!(refused.state, IncidentState::Open);
        assert_eq!(
            refused.revision, app.projects[0].revision,
            "the refused incident is refreshed to the revision core reported"
        );
        assert_eq!(refused.revision, actual);
        assert_ne!(refused.revision, revision);
        drop(log);
        in1_shutdown(&mut app);
    }

    /// Erratum `IN1b`-C-R51: a router auto-apply that the actor refuses opens
    /// **no second incident** about the router's own internal send.
    ///
    /// Appendix B row 26 raises `edit_revision_conflict` for a refused
    /// revision gate, and before C-R51 it raised one for every conflict — the
    /// router's own included. That left the person with two cards for one
    /// problem: the colour incident they can act on, and a second one reading
    /// *"the timeline moved between planning this edit and sending it … make
    /// the edit again"* about an edit they never made. `Explain` never
    /// resolves it, so `open_count()` never fell back to zero, which is the
    /// exact case §5.4 rule 25's argument turns on.
    ///
    /// The scenario is clause 9's: a stale auto-apply the actor refuses. The
    /// conflict still reaches `reconcile_router_sends` and still refreshes the
    /// incident, so nothing else about rule 10 moves.
    #[test]
    fn in1b_the_routers_own_refused_send_opens_no_second_incident() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);

        // A person's hand edit lands first, so the router's send is stale.
        let probe_asset = document.media_pool.first().expect("one asset").clone();
        let advance = assume_rec709_operation(probe_asset.id, &probe_asset.color_description);
        app.projects[0]
            .core
            .send(Command::Do(advance))
            .expect("the actor takes commands");
        in1_drain_until_revision(&mut app, 0, TimelineRevision::default());

        let stale = TimelineRevision::default();
        app.pending_observations
            .push(IncidentObservation::from_media_error(
                &error,
                IncidentSubject::Project,
                stale,
            ));
        app.route_incidents();
        assert_eq!(app.pending_router_applies.len(), 1);
        let token = app.pending_router_applies[0].token;

        in1_drain_until_conflict(&mut app, 0);
        assert_eq!(
            app.pending_router_conflicts[0].token,
            Some(token),
            "the conflict carries the router's own token"
        );
        assert!(
            app.pending_observations.is_empty(),
            "and the drain raised no row-26 observation for it"
        );
        app.route_incidents();

        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            log.len(),
            1,
            "one problem, one card — not a second about the router's own send"
        );
        let codes: Vec<&str> = log.all().map(|incident| incident.code.code()).collect();
        assert_eq!(codes, vec!["unknown_source_primaries"]);
        assert!(
            !codes.contains(&"edit_revision_conflict"),
            "the person is never told to re-make an edit Kinewright made"
        );
        assert_eq!(log.open_count(), 1, "and the badge counts the one problem");
        drop(log);

        // Rule 10 is unmoved: the conflict still matched by token, still
        // refreshed the incident, and still dropped the send.
        assert!(app.pending_router_applies.is_empty());
        let incident = in1_incident(&app, log_first_id(&app));
        assert_eq!(incident.state, IncidentState::Open);
        assert_ne!(incident.revision, stale);

        // A **foreign** conflict at the same moment still gets its card, which
        // is what keeps the skip a correlation rather than a silencing.
        app.pending_router_conflicts.push(RouterConflict {
            project_index: 0,
            actual: TimelineRevision(99),
            token: None,
        });
        app.pending_observations
            .push(IncidentObservation::revision_conflict(
                IncidentSubject::Project,
                TimelineRevision(98),
                TimelineRevision(99),
            ));
        app.route_incidents();
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.len(), 2);
        assert!(
            log.all()
                .any(|incident| incident.code.code() == "edit_revision_conflict"),
            "a conflict the router did not cause is still the person's to see"
        );
        drop(log);
        in1_shutdown(&mut app);
    }

    /// Every incident's revision in the focused project's log, in log order.
    fn log_revisions(app: &KinewrightApp) -> Vec<TimelineRevision> {
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.all().map(|incident| incident.revision).collect()
    }

    /// The id of the only incident in the focused project's log.
    fn log_first_id(app: &KinewrightApp) -> IncidentId {
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.all().next().expect("one incident").id
    }

    /// Erratum `IN1b`-C-R53: a router auto-apply the actor **rejects** is
    /// dropped, not left outstanding for the session, and opens no second
    /// incident about the router's own internal send.
    ///
    /// The third way a revision-gated send can end. The gate passes — the
    /// revision has not moved — and `Document::apply` refuses the operation
    /// anyway, so the actor answers `Event::OpRejected`. Before erratum
    /// `IN1b`-A-R15 that event carried no token, so `reconcile_router_sends`
    /// could never name the send and it waited for ever, costing one log read
    /// and one document read per tick; and the drain raised Appendix B row
    /// 24's card for it, telling the person to fix and re-make an edit
    /// Kinewright had planned and sent.
    ///
    /// **The reachable construction**, stated because it is not the obvious
    /// one: the incident is observed against an `AssetId` the media pool does
    /// not hold. Everything else is the fixture's — the same probed
    /// description, so the same `AutoApply` class and the same stored
    /// `SetAssetColorDescription` recovery — but the asset it names is gone,
    /// so `Document::apply` answers `MissingAsset`. Removing a real asset
    /// instead would move the revision and produce a *conflict*, which is the
    /// case `in1b_the_routers_own_refused_send_opens_no_second_incident`
    /// already covers; this test needs the gate to **pass**.
    #[test]
    fn in1b_the_routers_own_rejected_send_is_dropped_not_left_outstanding() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);

        let revision = app.projects[0].revision;
        let absent = kinewright_core::AssetId(4_242);
        assert!(
            document.asset(absent).is_none(),
            "the subject names an asset the pool does not hold"
        );
        let mut observation =
            IncidentObservation::from_media_error(&error, IncidentSubject::Project, revision);
        observation.subject = IncidentSubject::Asset(absent);
        app.pending_observations.push(observation);
        app.route_incidents();

        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "the incident is AutoApply, so the router sends once"
        );
        let incident_id = app.pending_router_applies[0].incident;
        assert_eq!(
            in1_incident(&app, incident_id).class,
            PolicyClass::AutoApply,
            "the probed description is the fixture's, so the class is unchanged"
        );

        // Wait for the actor's answer through **production** — the real drain
        // and the real router tick — rather than through the test mirror
        // (review-final B1). The drop is the effect, so an empty queue is the
        // condition, and the deadline is the hang detector.
        in1_poll_production_until(&mut app, |app| app.pending_router_applies.is_empty());
        assert!(
            app.pending_observations.is_empty(),
            "the rejection of the router's own send raises no card"
        );

        assert!(
            app.pending_router_applies.is_empty(),
            "the send is dropped, not left outstanding for the session"
        );
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            log.len(),
            1,
            "one problem, one card — not a second about the router's own send"
        );
        let incident = log.all().next().expect("one incident");
        assert_eq!(
            incident.state,
            IncidentState::Open,
            "the recovery did not land, so the incident stays open"
        );
        assert_eq!(
            incident.revision, revision,
            "no conflict occurred, so core reported no newer revision to refresh to"
        );
        assert_eq!(log.open_count(), 1);
        drop(log);

        // And nothing is re-sent (rule 10): a further tick is quiet.
        let audit_lines = app.error_log.count_with_source("Incident");
        app.route_incidents();
        app.route_incidents();
        assert!(app.pending_router_applies.is_empty());
        assert!(app.pending_observations.is_empty());
        assert_eq!(app.error_log.count_with_source("Incident"), audit_lines);
        assert_eq!(app.open_incident_count(), 1);

        in1_shutdown(&mut app);
    }

    /// Review-final S2: Appendix B row 104 and the `LutRestoreResponse`
    /// field that serves it.
    ///
    /// Stage A addendum 2 gave looks their own dedup axis, and row 104 — a
    /// refused `Locate file…` restore — is the one `Look` row whose id had to
    /// be carried on the worker's response to reach the site at all
    /// (`media_workflow.rs:1753`, set once at `:2125`). Nothing pinned it:
    /// changing the subject back to `Project` left the whole suite green.
    ///
    /// The response is sent down the worker's own channel and the real
    /// `poll_lut_workers` drain reads it, so everything from
    /// `handle_lut_restore_response` onwards is production's.
    #[test]
    fn in1b_a_refused_look_restore_names_the_look_it_could_not_restore() {
        let (_fixture, mut app) = in1b_app();
        let lut_asset = kinewright_core::LutAssetId(7);
        app.lut_worker_pending = 1;
        app.lut_restore_tx
            .send(crate::media_workflow::LutRestoreResponse {
                session_id: app.projects[0].id,
                lut_asset,
                title: "Bleach Bypass".to_owned(),
                candidate: PathBuf::from("bleach.cube"),
                result: Err(MediaError::Backend(
                    "lut_store_hash_mismatch: the bytes do not match the recorded hash".to_owned(),
                )),
            })
            .expect("the app owns the receiving half");
        assert!(app.poll_lut_workers(), "the drain consumed the response");

        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "look_unclassified",
            IncidentSubject::LutAsset(lut_asset),
            kinewright_core::IncidentSeverity::Blocks,
        );
        assert_eq!(
            incident.subject.label(),
            "Look 7",
            "one look that keeps refusing is one problem, on its own axis"
        );
        assert!(
            incident.observed.contains("Bleach Bypass"),
            "the title the person was locating survives in `observed`"
        );
        assert!(
            matches!(
                incident.evidence,
                kinewright_core::IncidentEvidence::MediaError { .. }
            ),
            "the typed media failure is kept as evidence (erratum `IN1b`-C-R48)"
        );
        in1_shutdown(&mut app);
    }

    /// Erratum `IN1b`-C-R54: a **card press** the actor refuses is reported;
    /// an auto-apply's refusal is not.
    ///
    /// Both skips errata `IN1b`-C-R51 and `IN1b`-C-R53 introduce argue from
    /// *"an edit Kinewright planned and sent"*, which is true of an auto-apply
    /// and false of a button the person pressed. Before C-R54 the two sends
    /// were indistinguishable — `send_incident_recovery` pushed the same
    /// `RouterApply` shape as `audit_new_incident` and neither site set a
    /// status — so a press that lost its race did **nothing at all**: no card,
    /// no status line, no audit entry. At `b99c328` the person got a card.
    ///
    /// Both halves run on one app, over the same failure, so the contrast is
    /// the test rather than an argument: the auto-apply's rejection is
    /// silent, the press's is not. The press's card carries the **incident's
    /// own subject**, not the project's — the person pressed a button on that
    /// card, and that card is where the answer belongs.
    #[test]
    fn in1b_a_refused_card_press_is_reported_where_an_auto_apply_is_not() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);
        let description = document
            .media_pool
            .first()
            .expect("one asset")
            .color_description
            .clone();

        // The same construction C-R53's test uses: the incident names an asset
        // the pool does not hold, so the gate passes and `Document::apply`
        // answers `MissingAsset`.
        let absent = kinewright_core::AssetId(4_242);
        let revision = app.projects[0].revision;
        let mut observation =
            IncidentObservation::from_media_error(&error, IncidentSubject::Project, revision);
        observation.subject = IncidentSubject::Asset(absent);
        app.pending_observations.push(observation);
        app.route_incidents();
        assert_eq!(app.pending_router_applies.len(), 1);
        let incident_id = app.pending_router_applies[0].incident;

        // Half one — the auto-apply. Silent, by C-R53.
        in1_poll_production_until(&mut app, |app| app.pending_router_applies.is_empty());
        let after_auto = {
            let handle = Arc::clone(&app.projects[0].incidents);
            let log = handle
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.len()
        };
        assert_eq!(after_auto, 1, "the router's own refusal opened no card");
        let audit_after_auto = app.error_log.count_with_source("Incident");

        // Half two — the person presses the card's control for the same
        // recovery, which the actor refuses for the same reason.
        app.send_incident_recovery(
            0,
            incident_id,
            assume_rec709_operation(absent, &description),
            IncidentOutcome::Applied,
        );
        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "the press sends once, through the same revision gate"
        );
        in1_poll_production_until(&mut app, |app| app.pending_router_applies.is_empty());
        assert!(
            app.pending_router_applies.is_empty(),
            "the press's send is dropped too — it did not land"
        );

        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            log.len(),
            2,
            "the press that failed is reported, where the auto-apply's was not"
        );
        let reported = log
            .all()
            .find(|incident| incident.id != incident_id)
            .expect("the card the press opened");
        assert_eq!(reported.code.code(), "operation_missing");
        assert_eq!(
            reported.subject,
            IncidentSubject::Asset(absent),
            "the incident's own subject, not the project's: the person pressed a \
             button on that card"
        );
        assert_eq!(reported.state, IncidentState::Open);
        assert_eq!(
            log.get(incident_id).expect("the first incident").state,
            IncidentState::Open,
            "and the incident the button sat on is still open, because the \
             recovery still did not land"
        );
        drop(log);
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            audit_after_auto + 1,
            "one audit line for the one card the press opened"
        );
        assert_ne!(
            app.status, "Ready",
            "and the status bar says something, where the press used to be silent"
        );
        in1_shutdown(&mut app);
    }

    /// The other half of erratum `IN1b`-C-R53, split out because folding it in
    /// pushed its sibling to 101 lines and the honest fix for a second
    /// distinct case is a second test rather than another `#[allow]` — the
    /// same call stage A made at `impl-core.md` §12.3.
    ///
    /// A **foreign** rejection — the person's own edit, carrying `token: None`
    /// — still gets its Appendix B row 24 card, which is what keeps C-R53's
    /// drop a correlation rather than a silencing.
    #[test]
    fn in1b_a_foreign_rejection_still_opens_its_card() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, _engine) = in1_harness(probed);
        app.projects[0]
            .core
            .send(Command::Do(Operation::RemoveMarker {
                marker: kinewright_core::MarkerId(9_999),
            }))
            .expect("the actor takes commands");
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        while app.pending_observations.is_empty() {
            in1_drain_core(&mut app, 0);
            assert!(
                Instant::now() < expiry,
                "no foreign rejection within {IN1_APP_DEADLINE:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            app.pending_router_applies.is_empty(),
            "the router sent nothing, so there is no token for this to match"
        );
        let incident = in1b_route_one(&mut app);
        in1b_declares(
            &incident,
            "operation_missing",
            IncidentSubject::Project,
            kinewright_core::IncidentSeverity::Blocks,
        );
        in1_shutdown(&mut app);
    }

    /// `IN1b` §4 rule 7 and §9 clause 10's discriminating half: a conflict
    /// that belongs to no router send leaves **every** outstanding send in
    /// place.
    ///
    /// This is the test that fails at `d4ed8eb`. Part A matched a conflict to
    /// a send by `(project_index, expected)` with a first-candidate fallback,
    /// so a foreign `Command::DoIfRevision` refused at the same expected
    /// revision — the two `media_workflow.rs` senders, or the agent's own —
    /// consumed a router send the conflict had nothing to do with: the
    /// incident was refreshed to a revision that refused someone else's
    /// command, and the send was dropped without ever being reconciled.
    ///
    /// The conflicts are injected rather than provoked, because the shape
    /// under test is the matcher and not the actor: a real foreign send would
    /// have to be refused in the same frame as a router send that has not yet
    /// landed, which no test can schedule. Both halves are asserted — a
    /// tokenless conflict and a conflict carrying a token no send holds change
    /// nothing, and the send's **own** token still consumes it, so the test
    /// cannot pass by doing nothing at all.
    #[test]
    fn in1b_a_foreign_conflict_leaves_every_router_send_outstanding() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);

        let revision = app.projects[0].revision;
        app.pending_observations
            .push(IncidentObservation::from_media_error(
                &error,
                IncidentSubject::Project,
                revision,
            ));
        app.route_incidents();
        assert_eq!(app.pending_router_applies.len(), 1);
        let token = app.pending_router_applies[0].token;
        let incident_id = app.pending_router_applies[0].incident;
        assert_eq!(
            in1_incident(&app, app.pending_router_applies[0].incident).revision,
            revision,
            "the send is gated on the revision its incident was observed at"
        );
        let opened_at_revision = in1_incident(&app, incident_id).revision;

        // A foreign send refused at the same expected revision: no token at
        // all, which is what `media_workflow.rs`'s two senders pass.
        let foreign = TimelineRevision(opened_at_revision.0 + 41);
        app.pending_router_conflicts.push(RouterConflict {
            project_index: 0,
            actual: foreign,
            token: None,
        });
        // And one carrying a token no outstanding send holds.
        app.pending_router_conflicts.push(RouterConflict {
            project_index: 0,
            actual: foreign,
            token: Some(CommandToken(u64::MAX)),
        });
        app.route_incidents();
        assert_eq!(
            app.pending_router_applies.len(),
            1,
            "a conflict the router did not cause consumes no send"
        );
        assert_eq!(app.pending_router_applies[0].token, token);
        assert_eq!(
            in1_incident(&app, incident_id).revision,
            opened_at_revision,
            "and refreshes no incident onto a revision that refused someone else"
        );

        // The positive control: the send's own token does consume it.
        app.pending_router_conflicts.push(RouterConflict {
            project_index: 0,
            actual: foreign,
            token: Some(token),
        });
        app.route_incidents();
        assert!(
            app.pending_router_applies.is_empty(),
            "the send named by the conflict is the one that is dropped"
        );
        assert_eq!(in1_incident(&app, incident_id).revision, foreign);
        in1_shutdown(&mut app);
    }

    /// IN1 §5.2 rule 15, §9 clause 12 and `IN1b` §5.4 rules 25-26 through the
    /// real router: after the auto-apply resolves, the **badge's** number is 0
    /// and exactly one `ErrorLog` line carries source `"Incident"` — different
    /// quantities, asserted as a pair.
    ///
    /// `IN1b` moves this test onto the badge's own reader. At `d4ed8eb` the
    /// badge rendered `self.error_log.len()`, so it would have read 1 here,
    /// for an incident Kinewright had already fixed: exactly the double-count
    /// IN1 exists to make invisible. `open_incident_count()` is what the
    /// toolbar draws, so asserting it is what makes this test fail against a
    /// badge that still counts lines.
    ///
    /// The log count is of `"Incident"`-source entries rather than `len()`,
    /// because the log may also hold an environment-dependent audio line. The
    /// tail discharges IN1 §8 rule 6: `resolved_after` is set and every token
    /// field is honestly empty.
    #[test]
    fn in1b_the_badge_counts_open_incidents_not_log_lines() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);
        let id = in1_auto_apply(&mut app, &error);
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        assert_eq!(
            app.open_incident_count(),
            0,
            "the badge counts problems currently unresolved, and this one is fixed"
        );
        assert_ne!(
            app.open_incident_count(),
            app.error_log.len(),
            "the badge's number and the audit view's are different quantities"
        );
        let incident = in1_incident(&app, id);
        let telemetry = incident.telemetry;
        assert!(
            telemetry.resolved_after.is_some(),
            "the router resolves every incident it applies"
        );
        assert!(telemetry.input_tokens.is_none());
        assert!(telemetry.cached_input_tokens.is_none());
        assert!(telemetry.cache_creation_input_tokens.is_none());
        assert!(telemetry.output_tokens.is_none());
        assert!(telemetry.reasoning_output_tokens.is_none());
        assert!(telemetry.cost_usd_millionths.is_none());
        // A further quiet tick moves neither quantity (rule 20).
        app.route_incidents();
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        assert_eq!(app.open_incident_count(), 0);
        // And one unresolved problem does move the badge, so the zero above is
        // a measurement rather than a badge that is always zero.
        app.note_label(
            kinewright_core::LabelIncident::Export,
            IncidentSubject::ExportJob,
            "Choose an export output path",
        );
        app.route_incidents();
        assert_eq!(app.open_incident_count(), 1);
        assert_eq!(app.error_log.count_with_source("Incident"), 2);
        in1_shutdown(&mut app);
    }

    /// IN1 §9 clause 17 through the real router and a real `Core` actor:
    /// after a router auto-apply followed by a global `Command::Undo`, the
    /// same fixture error observed again yields `Observed::Suppressed`, no
    /// further operation is sent for that asset for the session, and the
    /// first incident reads as reverted by the person — the probed bytes
    /// are back, `assumed_from` is gone, and the card's revert is greyed
    /// out — rather than being re-applied.
    #[test]
    fn in1_the_undo_sticks_and_the_reopen_is_suppressed() {
        let probe_engine = FfmpegMediaEngine::new().expect("the test engine starts");
        let (_fixture, probed) = in1_untagged(&probe_engine);
        let (mut app, engine) = in1_harness(probed);
        let document = app.projects[0].document.as_ref().clone();
        let error = in1_playback_error(&engine, &document);
        let id = in1_auto_apply(&mut app, &error);
        assert_eq!(app.error_log.count_with_source("Incident"), 1);
        let before_undo = app.projects[0].revision;
        app.projects[0]
            .core
            .send(Command::Undo)
            .expect("the actor takes commands");
        // `Command::Undo` answers with `DocumentChanged` carrying the actor's
        // next revision (`actor.rs:402-410`), so the revision move is the
        // condition to wait on.
        in1_drain_until_revision(&mut app, 0, before_undo);
        let incident = in1_incident(&app, id);
        let IncidentSubject::Asset(asset_id) = incident.subject else {
            panic!("the fixture incident is asset-scoped")
        };
        let asset = app.projects[0]
            .document
            .asset(asset_id)
            .expect("the asset survives the undo")
            .clone();
        assert_eq!(
            asset.color_description,
            *incident
                .evidence
                .probed()
                .expect("the fixture incident carries a probed description"),
            "undo restores the probed bytes"
        );
        assert_eq!(
            asset.assumed_from, None,
            "undo restores the pre-assumption record"
        );
        // The restored document fails the managed decode again; the router
        // must answer with nothing at all.
        let revision = app.projects[0].revision;
        let observation =
            IncidentObservation::from_media_error(&error, IncidentSubject::Project, revision);
        app.pending_observations.push(observation);
        app.route_incidents();
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            1,
            "no second audit line for the suppressed re-observe"
        );
        assert!(
            app.pending_router_applies.is_empty(),
            "no further operation is sent for that asset for the session"
        );
        let incident = in1_incident(&app, id);
        let view = incident_card(&incident, false, &CardLoadedFlags::default());
        assert_eq!(view.actions.len(), 1);
        assert_eq!(view.actions[0].label, REVERT_LABEL);
        assert!(
            !view.actions[0].enabled,
            "with nothing left to revert to, the revert greys out"
        );
        in1_shutdown(&mut app);
    }

    /// IN1 §5.2b rule 20: a frame in which nothing was observed and nothing
    /// conflicted sends no command and writes no log line.
    #[test]
    fn in1_a_quiet_router_tick_sends_nothing_and_writes_nothing() {
        let (mut app, _engine) = in1_harness(Document::default());
        app.route_incidents();
        app.route_incidents();
        assert!(app.pending_router_applies.is_empty());
        assert_eq!(app.error_log.count_with_source("Incident"), 0);
        let handle = Arc::clone(&app.projects[0].incidents);
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 0);
        assert_eq!(log.len(), 0);
        drop(log);
        in1_shutdown(&mut app);
    }

    // ------------------------------------------------------------------
    // IN2 stage C: the app-side investigator sessions.
    // ------------------------------------------------------------------

    pub(crate) fn in2_call(tool: &str, arguments: serde_json::Value) -> ScriptedCall {
        ScriptedCall {
            tool: tool.to_owned(),
            arguments,
        }
    }

    pub(crate) fn in2_cost(input_tokens: u64, output_tokens: u64) -> ScriptedCost {
        ScriptedCost {
            input_tokens,
            output_tokens,
            cached_input_tokens: Some(2),
            cache_creation_input_tokens: Some(3),
            reasoning_output_tokens: Some(1),
            cost_usd: Some(0.01),
        }
    }

    fn in2_add_track(id: u64) -> Operation {
        Operation::AddTrack {
            track: Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        }
    }

    fn in2_upsert_bin(id: u64) -> Operation {
        Operation::UpsertBin {
            bin: MediaBin {
                id: kinewright_core::BinId(id),
                name: format!("in2-bin-{id}"),
                parent: None,
                assets: Vec::new(),
            },
        }
    }

    fn in2_remove_bin(id: u64) -> Operation {
        Operation::RemoveBin {
            bin: kinewright_core::BinId(id),
        }
    }

    fn in2_allowlisted_code() -> IncidentCode {
        IncidentCode::Label(LabelIncident::Agent)
    }

    fn in2_open_at(
        app: &mut KinewrightApp,
        project_index: usize,
        code: IncidentCode,
        subject: IncidentSubject,
        observed: &str,
    ) -> IncidentId {
        let revision = app.projects[project_index].revision;
        let observation = IncidentObservation::plain(code, subject, observed, revision);
        let handle = Arc::clone(&app.projects[project_index].incidents);
        let mut log = handle.write().unwrap();
        let Observed::Opened(id) = log.observe(observation) else {
            panic!("the IN2 fixture observation must open an incident");
        };
        id
    }

    pub(crate) fn in2_configure_scripted_at(
        app: &mut KinewrightApp,
        project_index: usize,
        driver: ScriptedDriver,
        budgets: crate::investigator::InvestigatorBudgets,
    ) {
        let session = app.projects[project_index]
            .investigator
            .as_mut()
            .expect("every project owns investigator state");
        session.settings.enabled = true;
        session.settings.harness = Some("scripted".to_owned());
        session.settings.budgets = budgets;
        session.test_driver = Some(driver);
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the scripted fixture names every session input explicitly"
    )]
    fn in2_start_at(
        app: &mut KinewrightApp,
        project_index: usize,
        driver: ScriptedDriver,
        budgets: crate::investigator::InvestigatorBudgets,
        code: IncidentCode,
        subject: IncidentSubject,
        observed: &str,
        prompts: Vec<String>,
        preload: Vec<Operation>,
    ) -> IncidentId {
        in2_configure_scripted_at(app, project_index, driver, budgets);
        let id = in2_open_at(app, project_index, code, subject, observed);
        {
            let mut log = app.projects[project_index].incidents.write().unwrap();
            assert!(
                log.begin_investigation(id),
                "the direct fixture enters investigating"
            );
        }
        let queued = crate::investigator::QueuedIncident {
            id,
            code,
            subject,
            refused: None,
        };
        app.spawn_investigator_session(project_index, &queued, "scripted", prompts, preload)
            .expect("the investigator branch and server start");
        id
    }

    fn in2_configure_scripted(
        app: &mut KinewrightApp,
        driver: ScriptedDriver,
        budgets: crate::investigator::InvestigatorBudgets,
    ) {
        in2_configure_scripted_at(app, 0, driver, budgets);
    }

    #[test]
    fn in2_refused_operation_stash_is_empty_off_and_bounded_on() {
        let (mut app, _engine) = in1_harness(Document::default());
        let code = in2_allowlisted_code();
        let operation = in2_add_track(900);
        let revision = app.projects[0].revision;
        let observation = |number| {
            IncidentObservation::plain(
                code,
                IncidentSubject::Agent,
                format!("stash-{number}"),
                revision,
            )
        };
        in2_configure_scripted(
            &mut app,
            ScriptedDriver::new(Vec::new()),
            in2_default_budgets(),
        );
        app.set_investigator_enabled(false);
        for number in 0..100 {
            let current = observation(number);
            app.stash_refused_if_eligible(0, &current, &operation);
        }
        let off_count = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state exists")
            .pending_refused_count();
        assert_eq!(off_count, 0, "off means no refused backlog");

        app.set_investigator_enabled(true);
        for number in 0..100 {
            let current = observation(number);
            app.stash_refused_if_eligible(0, &current, &operation);
        }
        let on_count = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state exists")
            .pending_refused_count();
        assert!(on_count > 0, "eligible refused edits are retained");
        assert!(on_count <= 8, "the refused backlog is an eight-entry ring");
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_a_second_reinvestigate_press_on_a_queued_pair_is_successful() {
        let (mut app, _engine) = in1_harness(Document::default());
        let id = in2_open_at(
            &mut app,
            0,
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "queued-reinvestigate",
        );

        assert!(app.reinvestigate(0, id));
        assert!(
            app.reinvestigate(0, id),
            "a second press on the already-queued pair is still handled"
        );
        assert_eq!(
            app.projects[0]
                .investigator
                .as_ref()
                .expect("the project owns investigator state")
                .queued_count(),
            1,
            "the repeated press does not duplicate the queued pair"
        );
        assert_eq!(in2_incident(&app, id).state, IncidentState::Open);
        in2_cleanup(&mut app);
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2_reinvestigate_card_uses_pair_level_running_queue_and_finished_state() {
        let (mut app, _engine) = in1_harness(Document::default());
        let code = in2_allowlisted_code();
        let subject = IncidentSubject::Track(TrackId(77));
        let first_revision = app.projects[0].revision;
        let b_expected = in2_next_incident_id(&app);
        let b_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(b_expected, first_revision, 77)]),
            in2_default_budgets(),
            code,
            subject,
            "pair-b-finished",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        assert!(matches!(
            in2_incident(&app, b_id).state,
            IncidentState::Open | IncidentState::Investigating
        ));

        // A is running while B still carries its fresh proposal.  The pair,
        // rather than B's incident id, removes Re-investigate from B's card.
        let a_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "the pair is busy")
                    .with_pause(Duration::from_secs(1)),
            ]),
            in2_default_budgets(),
            code,
            subject,
            "pair-a-running",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        let running_pairs = app.investigator_session_pairs_for_project(0);
        assert!(
            !running_pairs.can_reinvestigate(b_id, code, subject),
            "B cannot re-investigate while A holds the same pair"
        );
        let running_card = app.investigating_card_for_project(0);
        let log = app.projects[0].incidents.read().unwrap();
        let running_rows = crate::incident_ui::incident_panel_rows(
            log.all(),
            running_card,
            &running_pairs,
            &empty_panel_sets(),
        );
        let running_b = running_rows
            .iter()
            .find(|row| row.id == b_id)
            .and_then(|row| row.proposal.as_ref())
            .expect("B's fresh proposal remains visible");
        assert!(
            !running_b
                .actions
                .contains(&crate::investigator::ProposalAction::Reinvestigate)
        );
        drop(log);
        app.set_investigator_enabled(false);
        std::thread::sleep(Duration::from_millis(1_100));
        app.set_investigator_enabled(true);

        // Hold the app-wide cap on two other projects, then queue A.  B's
        // card must see the queued pair even though this project has no
        // running card of its own.
        in2_add_project(&mut app, 2);
        in2_add_project(&mut app, 3);
        let _cap_first = in2_start_at(
            &mut app,
            1,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "cap one").with_pause(Duration::from_millis(500)),
            ]),
            in2_default_budgets(),
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "pair-cap-one",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        let _cap_second = in2_start_at(
            &mut app,
            2,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "cap two").with_pause(Duration::from_millis(500)),
            ]),
            in2_default_budgets(),
            IncidentCode::Label(LabelIncident::Media),
            IncidentSubject::Agent,
            "pair-cap-two",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        assert_eq!(
            app.investigator_running_count(),
            2,
            "the app-wide cap holds while A is queued"
        );
        assert!(app.reinvestigate(0, a_id));
        let queued_pairs = app.investigator_session_pairs_for_project(0);
        assert!(
            !queued_pairs.can_reinvestigate(b_id, code, subject),
            "B cannot re-investigate while A holds the queued pair"
        );
        let queued_card = app.investigating_card_for_project(0);
        let log = app.projects[0].incidents.read().unwrap();
        let queued_rows = crate::incident_ui::incident_panel_rows(
            log.all(),
            queued_card,
            &queued_pairs,
            &empty_panel_sets(),
        );
        let queued_b = queued_rows
            .iter()
            .find(|row| row.id == b_id)
            .and_then(|row| row.proposal.as_ref())
            .expect("B's proposal remains visible while A waits");
        assert!(
            !queued_b
                .actions
                .contains(&crate::investigator::ProposalAction::Reinvestigate)
        );
        drop(log);

        // Release the cap and start the same A incident to completion.  With
        // no holder left, B's card shows Re-investigate and the real press
        // succeeds, staling B's old proposal before its new session starts.
        app.set_investigator_enabled(false);
        std::thread::sleep(Duration::from_millis(600));
        app.set_investigator_enabled(true);
        {
            let mut log = app.projects[0].incidents.write().unwrap();
            assert!(log.begin_investigation(a_id));
        }
        let queued = crate::investigator::QueuedIncident {
            id: a_id,
            code,
            subject,
            refused: None,
        };
        app.spawn_investigator_session(
            0,
            &queued,
            "scripted",
            vec!["opening".to_owned()],
            Vec::new(),
        )
        .expect("the finished-pair session starts");
        in2_pump_until_finished(&mut app);
        let finished_pairs = app.investigator_session_pairs_for_project(0);
        assert!(
            finished_pairs.can_reinvestigate(b_id, code, subject),
            "B can re-investigate after A finishes"
        );
        let finished_card = app.investigating_card_for_project(0);
        let log = app.projects[0].incidents.read().unwrap();
        let finished_rows = crate::incident_ui::incident_panel_rows(
            log.all(),
            finished_card,
            &finished_pairs,
            &empty_panel_sets(),
        );
        let finished_b = finished_rows
            .iter()
            .find(|row| row.id == b_id)
            .and_then(|row| row.proposal.as_ref())
            .expect("B's finished-pair proposal remains visible");
        assert!(
            finished_b
                .actions
                .contains(&crate::investigator::ProposalAction::Reinvestigate)
        );
        drop(log);
        assert!(
            app.reinvestigate(0, b_id),
            "the shown pair-level Re-investigate action succeeds"
        );
        assert!(
            in2_incident(&app, b_id)
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale),
            "the new B session stales its old proposal"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_saving_after_unmuting_last_code_removes_document_mute() {
        let (mut app, _engine) = in1_harness(Document::default());
        let temporary = TempDirectory::new("in2-mute-save");
        let path = temporary.path("project.kinewright");
        let code = in2_allowlisted_code();
        app.mute_investigator_code(0, code);
        app.write_project(&path).expect("the muted project saves");
        let (muted, version, _) = load_document(&path).expect("the muted project reopens");
        assert_eq!(
            version, 1,
            "the muted fixture is versionless and reads as 1"
        );
        assert_eq!(
            muted
                .investigator
                .as_ref()
                .map(|prefs| prefs.muted_codes.as_slice()),
            Some([code.code().to_owned()].as_slice()),
            "the live mute is injected into the project bytes"
        );

        assert!(app.unmute_investigator_code(0, code.code()));
        app.write_project(&path).expect("the unmuted project saves");
        let (reopened, version, _) = load_document(&path).expect("the unmuted project reopens");
        assert_eq!(
            version, 1,
            "the unmuted fixture is versionless and reads as 1"
        );
        assert!(
            reopened.investigator.is_none()
                || reopened
                    .investigator
                    .as_ref()
                    .is_some_and(|prefs| prefs.muted_codes.is_empty()),
            "saving the empty live mute list removes the old document mute"
        );
        in2_cleanup(&mut app);
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the scripted fixture names every session input explicitly"
    )]
    fn in2_start_direct(
        app: &mut KinewrightApp,
        driver: ScriptedDriver,
        budgets: crate::investigator::InvestigatorBudgets,
        code: IncidentCode,
        subject: IncidentSubject,
        observed: &str,
        prompts: Vec<String>,
        preload: Vec<Operation>,
    ) -> IncidentId {
        in2_start_at(
            app, 0, driver, budgets, code, subject, observed, prompts, preload,
        )
    }

    pub(crate) fn in2_default_budgets() -> crate::investigator::InvestigatorBudgets {
        crate::investigator::InvestigatorBudgets {
            max_turns: 6,
            max_wall_time_seconds: 30,
            max_tokens: 40_000,
        }
    }

    fn in2_add_project(app: &mut KinewrightApp, id: u64) {
        let project = ProjectSession::create(
            id,
            format!("IN2 project {id}"),
            Document::default(),
            None,
            &app.playback,
            &app.analysis,
            &app.exporter,
            &SidecarMode::None,
            None,
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the second IN2 project builds");
        app.projects.push(project);
    }

    pub(crate) fn in2_happy_turn(
        id: IncidentId,
        revision: TimelineRevision,
        track: u64,
    ) -> ScriptedTurn {
        ScriptedTurn::new(
            vec![
                in2_call("get_timeline_state", serde_json::json!({})),
                in2_call(
                    "search_capabilities",
                    serde_json::json!({"queries": ["incident"]}),
                ),
                in2_call(
                    "get_capability",
                    serde_json::json!({"names": ["get_incidents", "propose_fix"]}),
                ),
                in2_call(
                    "prepare_edit_plan",
                    serde_json::json!({
                        "expected_revision": revision.0,
                        "operations": [{
                            "op": "add_track",
                            "track": {"id": track, "kind": "Video", "clips": []}
                        }]
                    }),
                ),
                in2_call(
                    "commit_edit_plan",
                    serde_json::json!({"plan_id": 1, "expected_revision": revision.0}),
                ),
                in2_call(
                    "invoke_capability",
                    serde_json::json!({
                        "name": "propose_fix",
                        "arguments": {
                            "incident_id": id.0,
                            "explanation": "Add a clean video track."
                        }
                    }),
                ),
            ],
            "The branch proves the fix and records the proposal.",
        )
        .with_cost(in2_cost(17, 11))
    }

    fn in2_proposal_call(id: IncidentId) -> ScriptedCall {
        in2_call(
            "invoke_capability",
            serde_json::json!({
                "name": "propose_fix",
                "arguments": {
                    "incident_id": id.0,
                    "explanation": "The branch carries the smallest safe fix."
                }
            }),
        )
    }

    pub(crate) fn in2_pump_until_finished(app: &mut KinewrightApp) {
        let expiry = Instant::now() + IN1_APP_DEADLINE;
        loop {
            app.pump_investigator_sessions();
            let running = app.investigator_running_count();
            if running == 0 {
                return;
            }
            assert!(
                Instant::now() < expiry,
                "the investigator session did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn in2_cleanup(app: &mut KinewrightApp) {
        let mut detached_reaper_needed = false;
        for project in &mut app.projects {
            let incidents = Arc::clone(&project.incidents);
            if let Some(session) = project.investigator.as_mut() {
                detached_reaper_needed |= session.is_running();
                session.shutdown_for_close("IN2 test cleanup", &incidents);
            }
        }
        in1_shutdown(app);
        if detached_reaper_needed {
            // Cancellation is deliberately non-joining on the frame thread;
            // give the detached reapers time to drain their test harnesses
            // before the test drops the app fixture.
            std::thread::sleep(Duration::from_millis(450));
        } else {
            // Let the fixture's ordinary core actor observe its shutdown too;
            // this keeps tests that only changed investigator settings from
            // racing process teardown.
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn in2_incident(app: &KinewrightApp, id: IncidentId) -> Incident {
        in1_incident(app, id)
    }

    fn in2_open_end_dedups(
        app: &mut KinewrightApp,
        id: IncidentId,
        code: IncidentCode,
        subject: IncidentSubject,
        observed: &str,
    ) -> bool {
        if in2_incident(app, id).state != IncidentState::Open {
            return false;
        }
        let observation =
            IncidentObservation::plain(code, subject, observed, app.projects[0].revision);
        let mut log = app.projects[0].incidents.write().unwrap();
        matches!(log.observe(observation), Observed::Deduped(found) if found == id)
    }

    pub(crate) fn in2_next_incident_id(app: &KinewrightApp) -> IncidentId {
        let next = app.projects[0]
            .incidents
            .read()
            .unwrap()
            .all()
            .map(|incident| incident.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        IncidentId(next)
    }

    #[test]
    fn in2_a_session_starts_only_for_an_allowlisted_unmuted_code() {
        let (mut app, _engine) = in1_harness(Document::default());
        app.projects[0].project_path = Some(PathBuf::from("/tmp/in2/session/project.kinewright"));
        assert_eq!(
            crate::investigator::investigator_working_directory(
                app.projects[0].project_path.as_deref()
            ),
            Some(PathBuf::from("/tmp/in2/session")),
            "a saved project session starts its driver in the project parent"
        );
        let codes = [
            IncidentCode::Operation(kinewright_core::IncidentFamily::Bounds),
            IncidentCode::Label(LabelIncident::Operations),
            IncidentCode::Operation(kinewright_core::IncidentFamily::Malformed),
            IncidentCode::Media(kinewright_core::MediaIncident::UnsupportedDecoderFormat),
            IncidentCode::DeliveryColor(kinewright_core::DeliveryColorIncident::UnsupportedCodec),
            IncidentCode::Label(LabelIncident::AgentBranch),
        ];
        let mut started_codes = Vec::new();
        for (number, code) in codes.into_iter().enumerate() {
            let expected = in2_next_incident_id(&app);
            let subject = IncidentSubject::Track(TrackId(100 + number as u64));
            let observed = format!("allowlisted-{number}");
            in2_configure_scripted(
                &mut app,
                ScriptedDriver::new(vec![ScriptedTurn::new(Vec::new(), "done")]),
                in2_default_budgets(),
            );
            app.note_observation(IncidentObservation::plain(
                code,
                subject,
                observed,
                app.projects[0].revision,
            ));
            app.route_incidents();
            let id = expected;
            assert_eq!(id, expected, "the allowlisted incident gets a stable id");
            assert_eq!(
                in2_incident(&app, id).state,
                IncidentState::Investigating,
                "the production allowlist/enabled/harness gate starts this code"
            );
            started_codes.push(in2_incident(&app, id).code);
            in2_pump_until_finished(&mut app);
        }
        assert_eq!(
            started_codes.len(),
            6,
            "sessions start for six distinct allowlisted codes"
        );
        assert_eq!(
            started_codes
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            6,
            "the six session starts cover six distinct codes"
        );
        assert!(
            started_codes.contains(&IncidentCode::Operation(
                kinewright_core::IncidentFamily::Bounds
            )),
            "the counted population includes operation_bounds"
        );
        assert!(
            started_codes.contains(&IncidentCode::Label(LabelIncident::Operations)),
            "the counted population includes operations_unclassified"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_no_session_starts_for_the_three_auto_apply_codes() {
        let (_fixture, mut app) = in1b_app();
        let driver = ScriptedDriver::new(vec![ScriptedTurn::new(Vec::new(), "unused")]);
        in2_configure_scripted(&mut app, driver, in2_default_budgets());
        let asset = app.focused().document.media_pool[0].id;
        let probed = ColorDescription::default();
        let codes = [
            kinewright_core::SourceColorIncident::UnknownPrimaries,
            kinewright_core::SourceColorIncident::UnknownTransfer,
            kinewright_core::SourceColorIncident::UnknownMatrix,
        ];
        let mut attempted = 0_usize;
        let mut applied = 0_usize;
        let mut router_telemetry_empty = 0_usize;
        for source_code in codes {
            let code = IncidentCode::SourceColor(source_code);
            let observation = IncidentObservation {
                code,
                subject: IncidentSubject::Asset(asset),
                observed: format!("in2-auto-{}", code.code()),
                allowed: Some("Rec.709".to_owned()),
                evidence: IncidentEvidence::SourceColor {
                    probed: probed.clone(),
                    assumption: None,
                },
                revision: app.projects[0].revision,
                name: None,
                transient: false,
            };
            app.note_observation(observation);
            app.route_incidents();
            attempted += 1;
            in1_poll_production_until(&mut app, |app| {
                let log = app.projects[0].incidents.read().unwrap();
                log.all().any(|incident| {
                    incident.code == code
                        && incident.state == IncidentState::Resolved(IncidentOutcome::Applied)
                })
            });
            let log = app.projects[0].incidents.read().unwrap();
            if log.all().any(|incident| {
                incident.code == code
                    && incident.state == IncidentState::Resolved(IncidentOutcome::Applied)
            }) {
                applied += 1;
                if let Some(incident) = log.all().find(|incident| incident.code == code) {
                    let telemetry = incident.telemetry.clone();
                    let none_fields = [
                        telemetry.input_tokens,
                        telemetry.cached_input_tokens,
                        telemetry.cache_creation_input_tokens,
                        telemetry.output_tokens,
                        telemetry.reasoning_output_tokens,
                        telemetry.cost_usd_millionths.map(i64::cast_unsigned),
                        telemetry.turns.map(u64::from),
                        telemetry.resolver.as_ref().map(|_| 1_u64),
                    ]
                    .into_iter()
                    .filter(Option::is_none)
                    .count();
                    router_telemetry_empty += usize::from(none_fields == 8);
                }
            }
        }
        assert!(attempted > 0, "three auto-apply codes were attempted");
        assert_eq!(
            app.investigator_running_count(),
            0,
            "auto-apply codes start no session"
        );
        assert!(
            applied > 0,
            "the router still landed at least one auto-apply"
        );
        assert_eq!(attempted, 3, "all three auto-apply codes were attempted");
        assert_eq!(applied, 3, "all three auto-apply landings arrived");
        let sessions = app.investigator_running_count();
        assert_eq!(
            sessions, 0,
            "the three auto-apply codes start zero sessions"
        );
        assert_eq!(
            router_telemetry_empty, 3,
            "all eight telemetry fields stay empty on router resolutions"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_the_queue_dedups_by_code_and_subject_and_caps_at_two() {
        let (mut app, _engine) = in1_harness(Document::default());
        in2_add_project(&mut app, 2);
        let slow = |message: &str| {
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), message).with_pause(Duration::from_millis(400)),
            ])
        };
        let budgets = in2_default_budgets();
        let first = in2_start_at(
            &mut app,
            0,
            slow("first"),
            budgets.clone(),
            IncidentCode::Label(LabelIncident::Agent),
            IncidentSubject::Agent,
            "cap-first",
            vec!["first".to_owned()],
            Vec::new(),
        );
        let second = in2_start_at(
            &mut app,
            1,
            slow("second"),
            budgets,
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            "cap-second",
            vec!["second".to_owned()],
            Vec::new(),
        );
        let queued = in2_open_at(
            &mut app,
            0,
            IncidentCode::Label(LabelIncident::Media),
            IncidentSubject::Agent,
            "cap-queued",
        );
        let queued_item = crate::investigator::QueuedIncident {
            id: queued,
            code: IncidentCode::Label(LabelIncident::Media),
            subject: IncidentSubject::Agent,
            refused: None,
        };
        let duplicate = queued_item.clone();
        let first_queue = app.projects[0]
            .investigator
            .as_mut()
            .unwrap()
            .enqueue(queued_item);
        let duplicate_queue = app.projects[0]
            .investigator
            .as_mut()
            .unwrap()
            .enqueue(duplicate);
        app.pump_investigator_sessions();
        let running = app.investigator_running_count();
        let queued_count = app.projects[0]
            .investigator
            .as_ref()
            .unwrap()
            .queued_count();
        assert!(running > 0, "at least one session is running");
        assert_eq!(running, 2, "the app-wide investigator cap is two");
        assert!(
            queued_count > 0,
            "the third incident remains queued at the cap"
        );
        assert!(first_queue, "the first pair enters the queue");
        assert!(
            !duplicate_queue,
            "the same code and subject are deduplicated"
        );
        assert!(
            first.0 > 0 && second.0 > 0,
            "both capped sessions have stable ids"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_the_investigating_card_shows_its_three_counters() {
        let (mut app, _engine) = in1_harness(Document::default());
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "working").with_pause(Duration::from_millis(250)),
            ]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "counters",
            vec!["work".to_owned()],
            Vec::new(),
        );
        let card = app
            .investigating_card_for_project(0)
            .expect("the running session publishes a card");
        let log = app.projects[0].incidents.read().unwrap();
        let session_pairs = app.investigator_session_pairs_for_project(0);
        let rows = crate::incident_ui::incident_panel_rows(
            log.all(),
            Some(card),
            &session_pairs,
            &empty_panel_sets(),
        );
        let row = rows
            .iter()
            .find(|row| row.id == id)
            .expect("the investigating incident is listed");
        let counter_rows = row
            .view
            .details
            .iter()
            .filter(|(key, _)| matches!(*key, "turns" | "tokens" | "elapsed"))
            .count();
        assert!(counter_rows > 0, "the card has running counter rows");
        assert_eq!(counter_rows, 3, "turns, tokens and elapsed are all visible");
        assert_eq!(row.view.state_label, "Investigating");
        drop(log);
        in2_pump_until_finished(&mut app);
        assert!(in2_incident(&app, id).telemetry.turns.is_some());
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_the_stopped_card_offers_reinvestigate() {
        let (mut app, _engine) = in1_harness(Document::default());
        let mut budgets = in2_default_budgets();
        budgets.max_turns = 1;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "one"),
                ScriptedTurn::new(Vec::new(), "two"),
            ]),
            budgets,
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "stopped",
            vec!["one".to_owned(), "two".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let incident = in2_incident(&app, id);
        assert_eq!(incident.state, IncidentState::Open);
        assert!(matches!(
            incident.telemetry.resolver,
            Some(IncidentResolver::Session { .. })
        ));
        let view = incident_card(&incident, false, &CardLoadedFlags::default());
        let stopped_rows = view
            .details
            .iter()
            .filter(|(key, _)| *key == "stopped")
            .count();
        assert!(
            stopped_rows > 0,
            "an open stopped session prints its reason"
        );
        let stopped = view
            .details
            .iter()
            .find(|(key, _)| *key == "stopped")
            .map(|(_, value)| value.as_str())
            .expect("the stopped sentence is present");
        assert!(stopped.contains("Investigation stopped:"));
        assert!(stopped.contains("Press Re-investigate"));
        assert_eq!(
            stopped.matches(':').count(),
            1,
            "the person-facing row has one colon"
        );
        assert!(!stopped.contains("scripted"));
        assert!(!stopped.contains("BranchError"));
        assert!(view.reinvestigate, "the stopped card offers re-investigate");
        in2_cleanup(&mut app);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2_the_proposal_card_offers_approve_reject_and_reinvestigate() {
        let (mut app, _engine) = in1_harness(Document::default());
        let first_revision = app.projects[0].revision;
        let first_id = in2_next_incident_id(&app);
        let first_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(first_id, first_revision, 31)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "proposal-card-approve",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let first_view = {
            let incident = in2_incident(&app, first_id);
            crate::investigator::proposal_card(
                &incident,
                incident
                    .proposal
                    .as_ref()
                    .expect("the proposal is recorded"),
            )
        };
        assert_eq!(first_view.actions.len(), 3);
        assert!(!first_view.operation_summary.is_empty());
        assert!(first_view.operation_summary[0].contains("Agent"));
        assert!(
            first_view
                .operation_summary
                .iter()
                .all(|line| !line.contains("IncidentSubject"))
        );
        assert!(first_view.approval_note.contains("one Undo"));
        let first_before = app.projects[0].revision;
        assert!(app.approve_investigator_proposal(0, first_id));
        in1_drain_until_revision(&mut app, 0, first_before);
        assert_eq!(
            in2_incident(&app, first_id).state,
            IncidentState::Resolved(IncidentOutcome::Applied)
        );

        let second_revision = app.projects[0].revision;
        let second_expected = in2_next_incident_id(&app);
        let second_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(second_expected, second_revision, 32)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Project,
            "proposal-card-reject",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        assert!(app.reject_investigator_proposal(0, second_id));
        assert_eq!(
            in2_incident(&app, second_id).state,
            IncidentState::Resolved(IncidentOutcome::Rejected)
        );

        let third_revision = app.projects[0].revision;
        let third_expected = in2_next_incident_id(&app);
        let third_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                in2_happy_turn(third_expected, third_revision, 33),
                ScriptedTurn::new(Vec::new(), "the re-investigated session ends"),
            ]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(33)),
            "proposal-card-reinvestigate",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        assert!(app.reinvestigate(0, third_id));
        let third = in2_incident(&app, third_id);
        assert!(
            third
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale),
            "re-investigate stales the previous proposal before the new session"
        );
        assert!(
            app.investigator_running_count() > 0,
            "re-investigate starts a new session"
        );
        let stale_running_card = crate::investigator::proposal_card_with_reinvestigate(
            &third,
            third
                .proposal
                .as_ref()
                .expect("the old proposal remains on the incident"),
            false,
        );
        assert!(
            stale_running_card.actions.is_empty(),
            "a stale proposal on its already-running incident has no dead button"
        );

        // A budget end returns to Open without staling a proposal, so the
        // same three-card actions remain meaningful after the session thread
        // is gone; exercise Approve on that Open entry.
        in2_pump_until_finished(&mut app);
        let budget_revision = app.projects[0].revision;
        let budget_expected = in2_next_incident_id(&app);
        let mut budget = in2_default_budgets();
        budget.max_turns = 1;
        let budget_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                in2_happy_turn(budget_expected, budget_revision, 34),
                ScriptedTurn::new(Vec::new(), "the budget ends the session"),
            ]),
            budget,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(34)),
            "proposal-card-open-budget",
            vec!["opening".to_owned(), "the next turn".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let budget_incident = in2_incident(&app, budget_id);
        assert_eq!(budget_incident.state, IncidentState::Open);
        let budget_card = crate::investigator::proposal_card(
            &budget_incident,
            budget_incident
                .proposal
                .as_ref()
                .expect("budget keeps proposal"),
        );
        assert_eq!(budget_card.actions.len(), 3);
        let budget_before = app.projects[0].revision;
        assert!(app.approve_investigator_proposal(0, budget_id));
        in1_drain_until_revision(&mut app, 0, budget_before);
        assert_eq!(
            in2_incident(&app, budget_id).state,
            IncidentState::Resolved(IncidentOutcome::Applied)
        );

        // The same Open-with-fresh-proposal card exposes Reject and
        // Re-investigate too. Use separate incidents so every shown action is
        // pressed against the exact state that rendered it.
        let reject_revision = app.projects[0].revision;
        let reject_expected = in2_next_incident_id(&app);
        let mut reject_budget = in2_default_budgets();
        reject_budget.max_turns = 1;
        let reject_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                in2_happy_turn(reject_expected, reject_revision, 35),
                ScriptedTurn::new(Vec::new(), "the budget ends the session"),
            ]),
            reject_budget,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(35)),
            "proposal-card-open-reject",
            vec!["opening".to_owned(), "the next turn".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let reject_incident = in2_incident(&app, reject_id);
        let reject_card = crate::investigator::proposal_card(
            &reject_incident,
            reject_incident.proposal.as_ref().expect("reject proposal"),
        );
        assert_eq!(reject_card.actions.len(), 3);
        assert!(app.reject_investigator_proposal(0, reject_id));
        assert_eq!(
            in2_incident(&app, reject_id).state,
            IncidentState::Resolved(IncidentOutcome::Rejected)
        );

        let reinvestigate_revision = app.projects[0].revision;
        let reinvestigate_expected = in2_next_incident_id(&app);
        let mut reinvestigate_budget = in2_default_budgets();
        reinvestigate_budget.max_turns = 1;
        let reinvestigate_id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                in2_happy_turn(reinvestigate_expected, reinvestigate_revision, 36),
                ScriptedTurn::new(Vec::new(), "the budget ends the session"),
            ]),
            reinvestigate_budget,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(36)),
            "proposal-card-open-reinvestigate",
            vec!["opening".to_owned(), "the next turn".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let reinvestigate_incident = in2_incident(&app, reinvestigate_id);
        let reinvestigate_card = crate::investigator::proposal_card(
            &reinvestigate_incident,
            reinvestigate_incident
                .proposal
                .as_ref()
                .expect("re-investigate proposal"),
        );
        assert_eq!(reinvestigate_card.actions.len(), 3);
        assert!(app.reinvestigate(0, reinvestigate_id));
        assert!(
            in2_incident(&app, reinvestigate_id)
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale)
        );
        in2_pump_until_finished(&mut app);
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_an_approved_proposal_applies_and_records_applied() {
        let (mut app, _engine) = in1_harness(Document::default());
        let mut approved = 0_usize;
        let mut destructive_operations = 0_usize;
        let mut confirmation_requests = 0_usize;
        let mut prior_revision = app.projects[0].revision;
        for (number, track) in [41_u64, 42, 43].into_iter().enumerate() {
            let expected_id = in2_next_incident_id(&app);
            let id = in2_start_direct(
                &mut app,
                ScriptedDriver::new(vec![in2_happy_turn(expected_id, prior_revision, track)]),
                in2_default_budgets(),
                in2_allowlisted_code(),
                IncidentSubject::Track(TrackId(200 + number as u64)),
                &format!("approve-{number}"),
                vec!["opening".to_owned()],
                Vec::new(),
            );
            in2_pump_until_finished(&mut app);
            let incident = in2_incident(&app, id);
            let proposal = incident.proposal.as_ref().expect("a proposal is recorded");
            destructive_operations += proposal
                .operations
                .iter()
                .filter(|operation| kinewright_agent::is_destructive_operation(operation))
                .count();
            let telemetry = incident.telemetry;
            let mirrored_cost_fields = [
                telemetry.input_tokens,
                telemetry.cached_input_tokens,
                telemetry.cache_creation_input_tokens,
                telemetry.output_tokens,
                telemetry.reasoning_output_tokens,
                telemetry.cost_usd_millionths.map(i64::cast_unsigned),
            ]
            .into_iter()
            .flatten()
            .count();
            assert_eq!(
                mirrored_cost_fields, 6,
                "approval {number} mirrors all six costs"
            );
            assert_eq!(telemetry.input_tokens, Some(17));
            assert_eq!(telemetry.output_tokens, Some(11));
            assert_eq!(
                telemetry.input_tokens.unwrap() + telemetry.output_tokens.unwrap(),
                28,
                "telemetry token total equals the scripted session counters"
            );
            assert_eq!(telemetry.turns, Some(1));
            assert!(
                telemetry.resolver.is_some(),
                "session resolution records a resolver"
            );
            confirmation_requests += app.investigator_pending_confirmation_requests(0).len();
            assert!(app.approve_investigator_proposal(0, id));
            in1_drain_until_revision(&mut app, 0, prior_revision);
            let after_apply = app.projects[0].revision;
            assert!(
                after_apply > prior_revision,
                "approval {number} advances the live revision"
            );
            assert_eq!(
                in2_incident(&app, id).state,
                IncidentState::Resolved(IncidentOutcome::Applied)
            );
            approved += 1;
            prior_revision = after_apply;
        }
        assert!(approved > 0, "at least one proposal was approved");
        assert_eq!(approved, 3, "three proposals are approved and applied live");
        assert_eq!(
            destructive_operations, 0,
            "approved proposals contain zero destructive operations"
        );
        assert_eq!(
            confirmation_requests, 0,
            "approved proposals raise zero confirmations"
        );

        // One global Undo removes the last approved batch, preserving the
        // existing contract that an approval is exactly one undo entry.
        app.undo();
        in1_drain_until_revision(&mut app, 0, prior_revision);
        let undo_track_count = app
            .focused()
            .document
            .tracks
            .iter()
            .filter(|track| track.id == TrackId(43))
            .count();
        assert_eq!(undo_track_count, 0, "one undo removes the approved batch");
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_a_conflicting_approval_retries_once_then_goes_stale() {
        let (mut app, _engine) = in1_harness(Document::default());
        let first_base = app.projects[0].revision;
        let first = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(IncidentId(1), first_base, 51)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "retry-applied",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        app.projects[0]
            .core
            .request(Command::Do(in2_add_track(50)))
            .expect("the intervening live edit lands");
        assert!(app.approve_investigator_proposal(0, first));
        in1_drain_until_revision(&mut app, 0, first_base);
        let first_incident = in2_incident(&app, first);
        assert_eq!(
            first_incident.state,
            IncidentState::Resolved(IncidentOutcome::Applied)
        );
        let retry_applied = usize::from(
            app.focused()
                .document
                .tracks
                .iter()
                .any(|track| track.id == TrackId(51)),
        );

        let second_base = app.projects[0].revision;
        let second = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(IncidentId(2), second_base, 52)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Project,
            "retry-stale",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        app.projects[0]
            .core
            .request(Command::Do(in2_add_track(52)))
            .expect("the conflicting duplicate edit lands first");
        let retry_core = app.projects[0].core.clone();
        app.projects[0]
            .investigator
            .as_mut()
            .expect("the investigator state remains")
            .approval_retry_hook = Some(Box::new(move |attempt| {
            if attempt == 1 {
                retry_core
                    .request(Command::Do(in2_add_track(53)))
                    .expect("the retry hook's second live edit lands");
            }
        }));
        assert!(app.approve_investigator_proposal(0, second));
        let stale_incident = in2_incident(&app, second);
        assert_eq!(stale_incident.state, IncidentState::Open);
        assert!(
            stale_incident
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale)
        );
        let stale_card = crate::investigator::proposal_card(
            &stale_incident,
            stale_incident.proposal.as_ref().unwrap(),
        );
        let stale_actions = stale_card.actions.len();
        assert!(
            stale_actions > 0,
            "a stale proposal still has a recovery control"
        );
        assert_eq!(
            stale_card.actions,
            vec![crate::investigator::ProposalAction::Reinvestigate]
        );
        assert!(retry_applied > 0, "the first conflict retried and applied");
        let stale_count = usize::from(
            stale_incident
                .proposal
                .is_some_and(|proposal| proposal.stale),
        );
        assert!(
            stale_count > 0,
            "the second conflict marked its proposal stale"
        );
        assert_eq!(
            stale_card.actions,
            vec![crate::investigator::ProposalAction::Reinvestigate],
            "twice-conflicting proposals offer only Re-investigate"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_a_branch_error_on_approve_leaves_the_incident_open() {
        let (mut app, _engine) = in1_harness(Document::default());
        let revision = app.projects[0].revision;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(IncidentId(1), revision, 61)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "branch-error",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        // Make the real live actor fail its request: the first accepted batch
        // at u64::MAX panics while incrementing its revision, so
        // `apply_to_live` observes CoreDisconnected and the production Err arm
        // performs the stale return.
        let replacement = kinewright_core::Core::spawn_at(
            app.projects[0].document.as_ref().clone(),
            TimelineRevision(u64::MAX),
        )
        .expect("the replacement max-revision core starts");
        let stopped_core = std::mem::replace(&mut app.projects[0].core, replacement);
        drop(stopped_core);
        app.projects[0].revision = TimelineRevision(u64::MAX);
        assert!(app.approve_investigator_proposal(0, id));
        let incident = in2_incident(&app, id);
        assert_eq!(incident.state, IncidentState::Open);
        assert!(
            incident
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale)
        );
        let branch_error_count = usize::from(
            incident
                .telemetry
                .resolver
                .as_ref()
                .is_some_and(|resolver| {
                    matches!(resolver, IncidentResolver::Session { stop, .. } if stop.contains("Core actor has stopped"))
                }),
        );
        assert!(
            branch_error_count > 0,
            "the branch error reason is recorded"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_the_off_switch_cancels_a_running_session_and_suppresses_nothing() {
        let (mut app, _engine) = in1_harness(Document::default());
        let code = in2_allowlisted_code();
        in2_configure_scripted(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(Vec::new(), "working")]),
            in2_default_budgets(),
        );
        app.projects[0]
            .investigator
            .as_mut()
            .expect("the investigator state remains")
            .test_start_delay = Some(Duration::from_secs(3));
        let id = in2_open_at(&mut app, 0, code, IncidentSubject::Agent, "off-switch");
        {
            let mut log = app.projects[0].incidents.write().unwrap();
            assert!(log.begin_investigation(id));
        }
        let queued = crate::investigator::QueuedIncident {
            id,
            code,
            subject: IncidentSubject::Agent,
            refused: None,
        };
        app.spawn_investigator_session(
            0,
            &queued,
            "scripted",
            vec!["opening".to_owned()],
            Vec::new(),
        )
        .expect("the blocked investigator session starts");
        let old_endpoint = app.projects[0]
            .investigator
            .as_ref()
            .and_then(crate::investigator::InvestigatorSession::running_endpoint)
            .expect("the cancelled session owns an MCP endpoint");
        let running_before = app.investigator_running_count();
        assert!(
            running_before > 0,
            "the off-switch test has a running session"
        );
        let cancel_started = Instant::now();
        app.set_investigator_enabled(false);
        assert!(
            cancel_started.elapsed() < Duration::from_millis(100),
            "the frame thread never waits for a blocked start_session"
        );
        let incident = in2_incident(&app, id);
        assert_eq!(incident.state, IncidentState::Open);
        assert!(matches!(
            incident.telemetry.resolver,
            Some(IncidentResolver::Session { ref stop, .. }) if stop.contains("switched off")
        ));
        assert_eq!(app.investigator_running_count(), 0);
        let authority = old_endpoint
            .strip_prefix("http://")
            .and_then(|endpoint| endpoint.split('/').next())
            .expect("the loopback endpoint has an authority");
        let address: SocketAddr = authority
            .parse()
            .expect("the endpoint has a socket address");
        let refusal_started = Instant::now();
        let expiry = refusal_started + Duration::from_millis(750);
        let mut refused_at = None;
        while Instant::now() < expiry {
            if TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_err() {
                refused_at = Some(Instant::now());
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let refused_at = refused_at.expect(
            "the old MCP server refuses while the three-second start path is still blocked",
        );
        let refusal_elapsed = refused_at.duration_since(cancel_started);
        assert!(
            refusal_elapsed < Duration::from_secs(1),
            "the old MCP endpoint closes well inside the blocked three-second start window"
        );
        assert!(refused_at.duration_since(refusal_started) < Duration::from_secs(1));
        // The frame thread has already returned and the server is already
        // closed. Let the detached reaper finish its deliberately non-frame
        // join before this test drops the app-owned branch/core resources.
        std::thread::sleep(Duration::from_millis(3_250));
        let observation = IncidentObservation::plain(
            code,
            IncidentSubject::Agent,
            "off-switch",
            app.projects[0].revision,
        );
        let deduped = {
            let mut log = app.projects[0].incidents.write().unwrap();
            log.observe(observation)
        };
        let deduped_count = usize::from(matches!(deduped, Observed::Deduped(_)));
        assert!(deduped_count > 0, "off-switch leaves the pair unsuppressed");
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_a_hand_resolution_mid_session_keeps_the_persons_outcome() {
        let (mut app, _engine) = in1_harness(Document::default());
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "working").with_pause(Duration::from_millis(300)),
            ]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "hand-resolution",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        let resolved = {
            let mut log = app.projects[0].incidents.write().unwrap();
            log.resolve(id, IncidentOutcome::Explained)
        };
        assert!(resolved, "the person can resolve the open incident");
        app.pump_investigator_sessions();
        std::thread::sleep(Duration::from_millis(350));
        let incident = in2_incident(&app, id);
        let person_outcome =
            usize::from(incident.state == IncidentState::Resolved(IncidentOutcome::Explained));
        assert!(
            person_outcome > 0,
            "the person's outcome remains authoritative"
        );
        assert!(
            incident.telemetry.resolver.is_none(),
            "the silent cancel writes no resolver"
        );
        assert_eq!(app.investigator_running_count(), 0);
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_the_revert_branch_keys_on_applied_asset_and_probed_evidence() {
        let (_fixture, mut app) = in1b_app();
        let probed = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            ..ColorDescription::default()
        };
        let observation = IncidentObservation {
            code: IncidentCode::SourceColor(kinewright_core::SourceColorIncident::UnknownPrimaries),
            subject: IncidentSubject::Asset(kinewright_core::AssetId(909)),
            observed: "known non-Rec.709 primaries".to_owned(),
            allowed: Some("bt709".to_owned()),
            evidence: IncidentEvidence::SourceColor {
                probed,
                assumption: None,
            },
            revision: app.projects[0].revision,
            name: None,
            transient: false,
        };
        let id = {
            let mut log = app.projects[0].incidents.write().unwrap();
            let Observed::Opened(id) = log.observe(observation) else {
                panic!("the re-key fixture opens");
            };
            assert!(log.resolve(id, IncidentOutcome::Applied));
            id
        };
        let incident = in2_incident(&app, id);
        assert_eq!(incident.class, PolicyClass::Explain);
        let view = incident_card(&incident, true, &CardLoadedFlags::default());
        let actions = view.actions.len();
        assert!(actions > 0, "an applied probed asset has a revert control");
        assert_eq!(actions, 1);
        assert_eq!(view.actions[0].label, REVERT_LABEL);
        in2_cleanup(&mut app);
    }

    #[test]
    fn in2_resolved_asset_proposals_are_not_rendered_as_proposal_cards() {
        let (mut app, _engine) = in1_harness(Document::default());
        let revision = app.projects[0].revision;
        let expected = in2_next_incident_id(&app);
        let mut budgets = in2_default_budgets();
        budgets.max_turns = 1;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                in2_happy_turn(expected, revision, 77),
                ScriptedTurn::new(Vec::new(), "the budget ends the session"),
            ]),
            budgets,
            in2_allowlisted_code(),
            IncidentSubject::Asset(kinewright_core::AssetId(777)),
            "proposal-card-asset",
            vec!["opening".to_owned(), "the next turn".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let open = in2_incident(&app, id);
        assert_eq!(open.state, IncidentState::Open);
        assert!(
            crate::investigator::shows_proposal_card(&open),
            "an Open asset proposal has an actionable proposal card"
        );
        let before = app.projects[0].revision;
        assert!(app.approve_investigator_proposal(0, id));
        in1_drain_until_revision(&mut app, 0, before);
        let applied = in2_incident(&app, id);
        assert_eq!(
            applied.state,
            IncidentState::Resolved(IncidentOutcome::Applied)
        );
        assert!(
            !crate::investigator::shows_proposal_card(&applied),
            "a resolved asset proposal remains history, not a proposal card"
        );
        in2_cleanup(&mut app);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2_the_scripted_suite_drives_eleven_scripts_to_their_ends() {
        let (mut app, _engine) = in1_harness(Document::default());
        let mut ended = 0_usize;
        let mut proposals = 0_usize;
        let mut unsuppressed_open_ends = 0_usize;
        let mut deduped_after_open_end = 0_usize;

        // S1: happy proposal through the real six-tool investigator surface.
        let revision = app.projects[0].revision;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(IncidentId(1), revision, 71)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Agent,
            "suite-s1",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        proposals += usize::from(in2_incident(&app, id).proposal.is_some());
        ended += 1;

        // S2: the same real proposal is rejected by the person.
        let revision = app.projects[0].revision;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(IncidentId(2), revision, 72)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Project,
            "suite-s2",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        assert!(app.reject_investigator_proposal(0, id));
        let s2_ok =
            in2_incident(&app, id).state == IncidentState::Resolved(IncidentOutcome::Rejected);
        ended += usize::from(s2_ok);

        // S3: explanation, with no proposal.
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(
                vec![in2_call("get_timeline_state", serde_json::json!({}))],
                "no honest fix exists",
            )]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(3)),
            "suite-s3",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let s3_ok =
            in2_incident(&app, id).state == IncidentState::Resolved(IncidentOutcome::Explained);
        ended += usize::from(s3_ok);

        // S4: turn budget.
        let mut budgets = in2_default_budgets();
        budgets.max_turns = 1;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "one"),
                ScriptedTurn::new(Vec::new(), "two"),
            ]),
            budgets,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(4)),
            "suite-s4",
            vec!["one".to_owned(), "two".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let s4_ok = in2_incident(&app, id).state == IncidentState::Open;
        ended += usize::from(s4_ok);
        unsuppressed_open_ends += usize::from(s4_ok);
        deduped_after_open_end += usize::from(in2_open_end_dedups(
            &mut app,
            id,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(4)),
            "suite-s4",
        ));

        // S5: wall-time budget.
        let mut budgets = in2_default_budgets();
        budgets.max_wall_time_seconds = 1;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "slow").with_pause(Duration::from_millis(1_100)),
            ]),
            budgets,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(5)),
            "suite-s5",
            vec!["slow".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let s5_ok = in2_incident(&app, id).state == IncidentState::Open;
        ended += usize::from(s5_ok);
        unsuppressed_open_ends += usize::from(s5_ok);
        deduped_after_open_end += usize::from(in2_open_end_dedups(
            &mut app,
            id,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(5)),
            "suite-s5",
        ));

        // S6: post-turn token budget.
        let mut budgets = in2_default_budgets();
        budgets.max_tokens = 10;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![
                ScriptedTurn::new(Vec::new(), "one").with_cost(in2_cost(5, 2)),
                ScriptedTurn::new(Vec::new(), "two").with_cost(in2_cost(8, 4)),
            ]),
            budgets,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(6)),
            "suite-s6",
            vec!["one".to_owned(), "two".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let s6_ok = in2_incident(&app, id).state == IncidentState::Open;
        ended += usize::from(s6_ok);
        unsuppressed_open_ends += usize::from(s6_ok);
        deduped_after_open_end += usize::from(in2_open_end_dedups(
            &mut app,
            id,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(6)),
            "suite-s6",
        ));

        // S7: the branch's destructive proposal is refused before recording.
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(
                vec![in2_proposal_call(IncidentId(7))],
                "try the destructive proposal",
            )]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(7)),
            "suite-s7",
            vec!["proposal".to_owned()],
            vec![in2_upsert_bin(7), in2_remove_bin(7)],
        );
        in2_pump_until_finished(&mut app);
        let s7 = in2_incident(&app, id);
        let s7_ok = s7.state == IncidentState::Open && s7.proposal.is_none();
        ended += usize::from(s7_ok);
        unsuppressed_open_ends += usize::from(s7_ok);
        deduped_after_open_end += usize::from(in2_open_end_dedups(
            &mut app,
            id,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(7)),
            "suite-s7",
        ));

        // S8: all nine denied capability names go through invoke_capability.
        let denied_calls = kinewright_agent::INVESTIGATOR_CAPABILITY_DENYLIST
            .iter()
            .map(|name| {
                in2_call(
                    "invoke_capability",
                    serde_json::json!({"name": name, "arguments": {}}),
                )
            })
            .collect();
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(denied_calls, "all denied")]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(8)),
            "suite-s8",
            vec!["denied".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        let s8_ok =
            in2_incident(&app, id).state == IncidentState::Resolved(IncidentOutcome::Explained);
        ended += usize::from(s8_ok);

        // S9: the three deterministic auto-apply codes are attempted as
        // routed observations and never enter the investigator queue.
        let auto_attempts = [
            kinewright_core::SourceColorIncident::UnknownPrimaries,
            kinewright_core::SourceColorIncident::UnknownTransfer,
            kinewright_core::SourceColorIncident::UnknownMatrix,
        ]
        .into_iter()
        .filter(|source_code| {
            let code = IncidentCode::SourceColor(*source_code);
            let observation = IncidentObservation::plain(
                code,
                IncidentSubject::Agent,
                format!("suite-s9-{}", code.code()),
                app.projects[0].revision,
            );
            app.note_observation(observation);
            app.route_incidents();
            app.investigator_running_count() == 0
        })
        .count();
        assert!(
            auto_attempts > 0,
            "S9 attempted all three deterministic rows"
        );
        let s9_ok = auto_attempts == 3;
        ended += usize::from(s9_ok);

        // S10: a proposal can be returned stale after approval loses the
        // live revision race; the dedicated test proves the retry too.
        let revision = app.projects[0].revision;
        let s10_id = in2_next_incident_id(&app);
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(s10_id, revision, 73)]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(10)),
            "suite-s10",
            vec!["opening".to_owned()],
            Vec::new(),
        );
        in2_pump_until_finished(&mut app);
        app.return_proposal_open_with_stale(0, id, "the proposal conflicted twice");
        let s10_ok = in2_incident(&app, id)
            .proposal
            .is_some_and(|proposal| proposal.stale);
        ended += usize::from(s10_ok);

        // S11: the branch confirmation is rejected by the pump's policy.
        let s11_revision = app.projects[0].revision.0;
        let id = in2_start_direct(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(
                vec![
                    in2_call(
                        "prepare_edit_plan",
                        serde_json::json!({
                            "expected_revision": s11_revision,
                            "operations": [
                                {"op": "upsert_bin", "bin": {"id": 11, "name": "s11", "parent": null, "assets": []}},
                                {"op": "remove_bin", "bin": 11}
                            ]
                        }),
                    ),
                    in2_call(
                        "commit_edit_plan",
                        serde_json::json!({"plan_id": 1, "expected_revision": s11_revision}),
                    ),
                ],
                "the destructive commit is refused",
            )]),
            in2_default_budgets(),
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(11)),
            "suite-s11",
            vec!["commit".to_owned()],
            Vec::new(),
        );
        let branch_operations_before = app
            .investigator_branch_applied_operation_count(0)
            .expect("S11 has a live branch before its confirmation");
        let mut s11_requests = Vec::new();
        let request_deadline = Instant::now() + IN1_APP_DEADLINE;
        while s11_requests.is_empty() && Instant::now() < request_deadline {
            s11_requests = app.investigator_take_and_reject_confirmation_requests(0);
            if s11_requests.is_empty() {
                assert!(
                    app.investigator_running_count() > 0,
                    "S11 remains alive until its confirmation is observed"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(
            s11_requests.len(),
            1,
            "S11 raises exactly one confirmation request"
        );
        assert_eq!(
            s11_requests[0].incident,
            Some(id),
            "the confirmation request carries S11's incident id"
        );
        in2_pump_until_finished(&mut app);
        let branch_operations_after = app
            .investigator_branch_applied_operation_count(0)
            .unwrap_or(branch_operations_before);
        assert_eq!(
            branch_operations_after, branch_operations_before,
            "the rejected S11 confirmation leaves branch operations unchanged"
        );
        let s11 = in2_incident(&app, id);
        let s11_ok = s11.state == IncidentState::Open
            && matches!(
                s11.telemetry.resolver,
                Some(IncidentResolver::Session { ref stop, .. }) if stop.contains("confirmation")
            );
        ended += usize::from(s11_ok);
        unsuppressed_open_ends += usize::from(s11_ok);
        deduped_after_open_end += usize::from(in2_open_end_dedups(
            &mut app,
            id,
            in2_allowlisted_code(),
            IncidentSubject::Track(TrackId(11)),
            "suite-s11",
        ));

        assert!(ended > 0, "the scripted suite produced eleven session ends");
        assert_eq!(ended, 11, "S1 through S11 each reached its expected end");
        assert!(proposals > 0, "the suite recorded at least one proposal");
        assert!(
            unsuppressed_open_ends >= 4,
            "at least four budget/policy ends return Open without suppression"
        );
        assert_eq!(
            deduped_after_open_end, unsuppressed_open_ends,
            "every returned-open pair dedups on a later observation"
        );
        in2_cleanup(&mut app);
    }
}

/// `IN2B` §12 items over the real save/close/reopen paths, headless.
///
/// The smallest real app that can save: one project session on a real `Core`
/// actor with a project path, a real engine behind the trait arcs (the save
/// path publishes the LUT library through the concrete engine), and the app's
/// single shared sidecar writer. No window, no model, no audio device — the
/// same terms §12 lays down. Save/close/reopen run through the production
/// methods (`write_project`, `stop_threads`, `ProjectSession::create` with
/// `SidecarMode::Load`), so no test here can pass with the sidecar plumbing
/// absent.
#[cfg(test)]
mod in2b_tests {
    use std::fs;

    use kinewright_core::{
        IncidentCode, IncidentId, IncidentObservation, IncidentOutcome, IncidentProposal,
        IncidentSubject, LabelIncident, Marker, MarkerId, Observed, TimelineRevision,
    };
    use kinewright_media::{FfmpegMediaEngine, test_support::TempDirectory};

    use super::*;
    use crate::sidecar::{FlushOutcome, SidecarLoad, load_sidecar, sidecar_matches_project};

    /// The smallest real app that can save, on an optional project path.
    #[allow(clippy::too_many_lines)]
    fn in2b_harness(
        document: Document,
        project_path: Option<PathBuf>,
    ) -> (KinewrightApp, Arc<FfmpegMediaEngine>) {
        let engine = Arc::new(FfmpegMediaEngine::new().expect("the test engine starts"));
        let playback: Arc<dyn Playback> = engine.clone();
        let analysis: Arc<dyn Analysis> = engine.clone();
        let exporter: Arc<dyn Export> = engine.clone();
        let writer = SidecarWriter::new();
        // The digest `load_document` would have read: the file rarely exists
        // yet at harness time, and an `Absent` sidecar needs no gate.
        let mode = match &project_path {
            Some(path) => SidecarMode::Load {
                project_digest: fs::read(path)
                    .ok()
                    .map_or_else(String::new, |bytes| digest_bytes(&bytes)),
            },
            None => SidecarMode::None,
        };
        let project = ProjectSession::create(
            1,
            "IN2B",
            document,
            project_path,
            &playback,
            &analysis,
            &exporter,
            &mode,
            Some(Arc::clone(&writer)),
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the test session builds");
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (relink_probe_tx, relink_probe_rx) = std::sync::mpsc::channel();
        let (lut_import_tx, lut_import_rx) = std::sync::mpsc::channel();
        let (lut_restore_tx, lut_restore_rx) = std::sync::mpsc::channel();
        let (media_status_tx, media_status_rx) = std::sync::mpsc::channel();
        let (cache_clear_tx, cache_clear_rx) = std::sync::mpsc::channel();
        let (room_tone_tx, room_tone_rx) = std::sync::mpsc::channel();
        let (_frames_tx, frames) = crossbeam_channel::unbounded();
        let (_media_tx, media_events) = crossbeam_channel::unbounded();
        let app = KinewrightApp {
            projects: vec![project],
            focused_project: 0,
            next_project_id: 2,
            next_command_token: 1,
            playback,
            analysis,
            exporter,
            lut_publisher: Arc::clone(&engine),
            frames,
            media_events,
            visual_cache: crate::visual_cache::VisualCache::new(engine.visual_asset_results()),
            harness: std::array::from_fn(|_| crate::chat_ui::HarnessUiState::default()),
            harness_update_rx: None,
            show_thread_rail: true,
            settings_open: false,
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
            pending_observations: Vec::new(),
            transcript_noted: None,
            recovery_damage_noted: false,
            recovery_unavailable_noted: false,
            pending_router_conflicts: Vec::new(),
            pending_router_landings: Vec::new(),
            pending_router_applies: Vec::new(),
            sidecar_writer: writer,
            sidecar_last_submit: Instant::now(),
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
            working_proof_cache: Arc::default(),
            matte_overlay: crate::matte_overlay_ui::MatteOverlayState::default(),
            playing: false,
            meter_levels: [0.0; 2],
            mixer_levels: crate::mixer_ui::MixerMeterLevels::default(),
            mixer_selection: None,
            resume_after_scrub: false,
            transcript_scope: TranscriptScope::default(),
            material_tab: MaterialTab::default(),
            show_material_strip: false,
            show_media_rail: false,
            pending_project_action: None,
            exit_discarded_projects: Vec::new(),
            allow_close: false,
            last_window_title: String::new(),
            status: "Ready".to_owned(),
            export_dialog: ExportDialog {
                open: false,
                output: "export.mp4".to_owned(),
                width: 320,
                height: 180,
                fps_numerator: 25,
                fps_denominator: 1,
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
            error_log: ErrorLog::default(),
            error_log_open: false,
            incidents_open: false,
            screenshot: crate::screenshot::ScreenshotCapture::from_environment(),
            recording: None,
            record_dialog: crate::recording::RecordDialog::default(),
            edit_gesture: 0,
            look_ab_hold: None,
            look_ab_hold_seen: false,
            performance: None,
        };
        (app, engine)
    }

    fn in2b_shutdown(app: &mut KinewrightApp) {
        super::in1_tests::in1_shutdown(app);
    }

    /// Start a scripted session on a seam-produced observation (item 31):
    /// the observation is observed as built — never re-typed by hand —
    /// then the incident is pre-registered (`begin_investigation`) and
    /// the session spawns directly. No router, no auto-investigation.
    fn in2b_start_seam_session(
        app: &mut KinewrightApp,
        driver: kinewright_agent::ScriptedDriver,
        observation: IncidentObservation,
        prompts: Vec<String>,
    ) -> IncidentId {
        use super::in1_tests::{in2_configure_scripted_at, in2_default_budgets};
        in2_configure_scripted_at(app, 0, driver, in2_default_budgets());
        let code = observation.code;
        let subject = observation.subject;
        let id = {
            let mut log = app.projects[0]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Observed::Opened(id) = log.observe(observation) else {
                panic!("the seam observation opens a fresh incident");
            };
            assert!(
                log.begin_investigation(id),
                "the direct fixture enters investigating"
            );
            id
        };
        let queued = crate::investigator::QueuedIncident {
            id,
            code,
            subject,
            refused: None,
        };
        app.spawn_investigator_session(0, &queued, "scripted", prompts, Vec::new())
            .expect("the investigator branch and server start");
        id
    }

    /// Park the engine worker before teardown: every `focus_project` ends in
    /// `set_document` + `request_frame`, and the worker renders
    /// asynchronously on a detached thread — dropping the engine while it is
    /// mid-render SEGVs at process exit (`Compositor::readback_for` against
    /// libnvidia teardown). The worker is FIFO and `request_frame` coalesces
    /// latest-wins, so a drained receiver plus one sentinel request proves
    /// quiescence: the sentinel's frame arrives only after every earlier
    /// render finished, and nothing after this function renders again.
    /// Tests that never focus (C1's) need no quiesce.
    fn in2b_quiesce_engine(engine: &FfmpegMediaEngine) {
        let frames = engine.frames();
        while frames.try_recv().is_ok() {}
        // Every focus in these tests requests `ZERO` (no test scrubs), so a
        // nonzero sentinel is unambiguous — and an empty document renders
        // every position identically (no clips, no decode).
        engine.request_frame(TimeCode(1));
        let expiry = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let Ok((at, _)) = frames.recv_timeout(
                expiry
                    .checked_duration_since(std::time::Instant::now())
                    .unwrap_or_default(),
            ) else {
                panic!("the engine worker answered no frame within 10 s");
            };
            if at == TimeCode(1) {
                return;
            }
        }
    }

    fn in2b_shutdown_session(session: &mut ProjectSession) {
        for thread in &mut session.threads {
            if let Some(server) = thread.mcp_server.take() {
                server.shutdown();
            }
        }
    }

    /// Observe `count` distinct open incidents into a session's log, at
    /// distinct non-default revisions so the restore rebase is visible.
    fn in2b_observe_opens(session: &ProjectSession, count: usize, first_revision: u64) {
        let mut log = session
            .incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for n in 0..count {
            let observed = log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                format!("in2b live incident {first_revision}-{n}"),
                TimelineRevision(first_revision + n as u64),
            ));
            assert!(
                matches!(observed, Observed::Opened(_)),
                "distinct observations open distinctly"
            );
        }
    }

    /// Hold the pump with a fabricated pending session (items 24–25):
    /// observes a blocker incident directly into the log (no audit, no
    /// queue), holds it `Investigating`, and installs a running session
    /// whose result never arrives — so queued incidents wait instead of
    /// starting, with no timing and no real model turn. `pair` must differ
    /// from every pair the test routes. Returns the result sender, which
    /// the caller holds until cleanup (dropping it reads as `Died`).
    fn in2b_install_pump_blocker(
        app: &mut KinewrightApp,
        code: IncidentCode,
        subject: IncidentSubject,
    ) -> crossbeam_channel::Sender<crate::investigator::InvestigatorSessionResult> {
        use kinewright_agent::{ConfirmationBroker, SharedCounters, TimelineBranch};

        use crate::investigator::RunningSession;

        let revision = app.projects[0].revision;
        let blocker = {
            let mut log = app.projects[0]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Observed::Opened(id) = log.observe(IncidentObservation::plain(
                code,
                subject,
                "pump blocker",
                revision,
            )) else {
                panic!("the blocker opens");
            };
            assert!(log.begin_investigation(id), "the blocker is held");
            id
        };
        let branch = TimelineBranch::new_at(
            "in2b-block",
            revision,
            std::sync::Arc::clone(&app.projects[0].document),
        )
        .expect("the blocker branch spawns");
        let (blocker_tx, blocker_rx) = crossbeam_channel::unbounded();
        app.projects[0]
            .investigator
            .as_mut()
            .expect("investigator state")
            .install_pending_test_session(RunningSession {
                id: blocker,
                code,
                subject,
                harness: "scripted".to_owned(),
                model: None,
                branch,
                server: None,
                broker: ConfirmationBroker::with_timeout(std::time::Duration::from_secs(5)),
                thread: None,
                counters: std::sync::Arc::new(SharedCounters::default()),
                cost_events: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
                cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                result: blocker_rx,
                budgets: super::in1_tests::in2_default_budgets(),
            });
        blocker_tx
    }

    /// Item 14: a proposal recorded after the last document save survives a
    /// close without saving, and the reopen restores every record with its
    /// digest pair verifying — plus the B-5 gate box.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_a_proposal_after_the_last_save_survives_close_reopen() {
        let temp = TempDirectory::new("in2b-close-reopen");
        let project_path = temp.path("edit.kinewright");
        let (mut app, _engine) = in2b_harness(Document::default(), Some(project_path.clone()));

        // Three open plus two resolved.
        in2b_observe_opens(&app.projects[0], 5, 41);
        let proposal_id = {
            let mut log = app.projects[0]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids: Vec<IncidentId> = log.all().map(|incident| incident.id).collect();
            assert_eq!(ids.len(), 5);
            assert!(log.resolve(ids[3], IncidentOutcome::Explained));
            assert!(log.resolve(ids[4], IncidentOutcome::Explained));
            assert_eq!(log.open_count(), 3);
            ids[0]
        };

        // Save; the sidecar carries every record.
        app.write_project(&project_path).expect("the save succeeds");
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");
        assert!(sidecar.is_file(), "the save writes a sidecar");
        let SidecarLoad::Current(before_load) = load_sidecar(&sidecar) else {
            panic!("the saved sidecar parses");
        };
        assert_eq!(before_load.records.len(), 5);

        // A proposal recorded after the last document save.
        {
            let mut log = app.projects[0]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(log.begin_investigation(proposal_id));
            log.record_proposal(
                proposal_id,
                IncidentProposal {
                    operations: vec![Operation::SetTrackMix {
                        track: TrackId(1),
                        gain_tenth_db: -60,
                        pan_percent: 25,
                        mute: false,
                        solo: true,
                    }],
                    operation_count: 1,
                    summary: "in2b post-save proposal".to_owned(),
                    explanation: "recorded after the last document save".to_owned(),
                    base_revision: TimelineRevision(99),
                    stale: false,
                },
            )
            .expect("the proposal records");
        }

        // Close without saving, driving `stop_threads`: the sidecar bytes
        // change — read before/after, never mtime.
        let before = fs::read(&sidecar).expect("the sidecar reads");
        app.projects[0].stop_threads("the project was closed");
        let after = fs::read(&sidecar).expect("the sidecar re-reads");
        assert_ne!(before, after, "the close flushed the post-save proposal");

        // Reopen through `ProjectSession::create` with `SidecarMode::Load`.
        let (document, version, digest) =
            load_document(&project_path).expect("the project re-reads");
        assert_eq!(version, 1);
        let playback = Arc::clone(&app.playback);
        let analysis = Arc::clone(&app.analysis);
        let exporter = Arc::clone(&app.exporter);
        let writer = Arc::clone(&app.sidecar_writer);
        let mut reopened = ProjectSession::create(
            7,
            "reopened",
            document,
            Some(project_path.clone()),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: digest,
            },
            Some(writer),
            version,
            None,
        )
        .expect("the reopen builds");
        let report = reopened
            .last_restore_report
            .as_ref()
            .expect("the reopen reports");
        assert_eq!(report.restored_open, 3);
        assert_eq!(report.restored_resolved, 2);
        {
            let log = reopened
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The badge counter reads the open count.
            assert_eq!(log.open_count(), 3);
            assert_eq!(log.len(), 5);
            let proposal = log
                .all()
                .find_map(|incident| incident.proposal.clone())
                .expect("the proposal survived");
            assert!(proposal.stale, "loaded proposals are stale");
            assert!(proposal.operation_count >= 1);
            assert!(proposal.operations.is_empty());
            // Every restored revision rebases to the opening (a fresh
            // session opens at the default); the base revision with them.
            for incident in log.all() {
                assert_eq!(incident.revision, TimelineRevision::default());
            }
        }
        assert_eq!(
            report.rebased, 6,
            "five revisions plus the proposal base revision"
        );
        // The digest pair verifies against the bytes on disk.
        let digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));
        let SidecarLoad::Current(loaded) = load_sidecar(&sidecar) else {
            panic!("the closed sidecar parses");
        };
        assert!(
            sidecar_matches_project(&loaded, &digest),
            "the digest pair verifies"
        );
        // A re-flush of the restored log agrees with the restore report.
        let outcome = reopened
            .flush_incidents(&digest, &digest)
            .expect("the re-flush lands");
        let FlushOutcome::Written(written) = outcome else {
            panic!("the restored log has content to write");
        };
        assert_eq!(written.written_open, 3);
        assert_eq!(written.written_resolved, 2);

        // B-5 gate box: a save during an in-flight background write leaves
        // the save's digest pair, never the stale job's bytes.
        let stale =
            crate::sidecar::build_sidecar_bytes(&[], &[], "aaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaa")
                .expect("the stale snapshot builds");
        app.sidecar_writer.submit(sidecar.clone(), stale);
        app.write_project(&project_path).expect("the save succeeds");
        let landed_digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));
        let SidecarLoad::Current(landed) = load_sidecar(&sidecar) else {
            panic!("the saved sidecar parses");
        };
        assert_eq!(landed.project_digest, landed_digest);
        assert_ne!(landed.project_digest, "aaaaaaaaaaaaaaaa");

        in2b_shutdown_session(&mut reopened);
        in2b_shutdown(&mut app);
    }

    /// Item 16: Save As writes a fresh sidecar at the new stem carrying the
    /// live log with a fresh digest, leaves the old sidecar byte-identical,
    /// and drops the open `project_newer_format` note (B-4) — so a
    /// newer-format session's copy reopens with 0 incidents.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_save_as_carries_the_live_log_and_leaves_the_old_sidecar() {
        // The carry: two live incidents follow the project to its new stem.
        let temp = TempDirectory::new("in2b-save-as");
        let a_path = temp.path("a.kinewright");
        let (mut app, _engine) = in2b_harness(Document::default(), Some(a_path.clone()));
        in2b_observe_opens(&app.projects[0], 2, 11);
        app.write_project(&a_path).expect("the first save succeeds");
        let a_sidecar = sidecar_path_for_project(Some(&a_path)).expect("derived");
        let a_bytes = fs::read(&a_sidecar).expect("the old sidecar reads");

        let b_path = temp.path("b.kinewright");
        app.write_project(&b_path).expect("Save As succeeds");
        assert_eq!(
            fs::read(&a_sidecar).expect("the old sidecar re-reads"),
            a_bytes,
            "Save As leaves the old sidecar byte-identical"
        );
        {
            let log = app.projects[0]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 2, "the live log is intact");
        }
        let b_sidecar = sidecar_path_for_project(Some(&b_path)).expect("derived");
        let b_digest = digest_bytes(&fs::read(&b_path).expect("the copy reads"));
        let SidecarLoad::Current(b_loaded) = load_sidecar(&b_sidecar) else {
            panic!("the new sidecar parses");
        };
        assert_eq!(b_loaded.records.len(), 2, "the live log carried");
        assert_eq!(b_loaded.project_digest, b_digest, "fresh digest");
        assert!(
            b_loaded.previous_digest.is_empty(),
            "a new stem has no previous save"
        );

        // The B-4 drop: a session whose only incident is the newer-format
        // note (stood in by direct observe — C2's gate notes it) saves a
        // copy that reopens with 0 incidents. A second project on the SAME
        // engine: one engine per test, because a second `FfmpegMediaEngine`
        // crashes inside the NVIDIA driver during concurrent GPU init
        // (coredump 2182070: `Compositor::new` → `create_render_pipeline` →
        // `libnvidia-glcore` SEGV on the media worker).
        let c_path = temp.path("c.kinewright");
        let id = app.next_project_id;
        app.next_project_id += 1;
        let playback = Arc::clone(&app.playback);
        let analysis = Arc::clone(&app.analysis);
        let exporter = Arc::clone(&app.exporter);
        let writer = Arc::clone(&app.sidecar_writer);
        let second = ProjectSession::create(
            id,
            "second",
            Document::default(),
            Some(c_path.clone()),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: String::new(),
            },
            Some(writer),
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the second project builds");
        app.projects.push(second);
        app.focused_project = 1;
        {
            let mut log = app.projects[1]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::ProjectNewerFormat),
                IncidentSubject::Project,
                "stood-in newer-format note",
                TimelineRevision::default(),
            ));
        }
        let d_path = temp.path("d.kinewright");
        app.write_project(&d_path).expect("Save As succeeds");
        {
            let log = app.projects[1]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 0, "Save As drops the newer-format note");
            assert_eq!(log.len(), 0, "a removal, not a resolution");
        }
        let (document, version, digest) = load_document(&d_path).expect("the copy re-reads");
        assert_eq!(version, 1, "Save As writes current-version bytes");
        let playback = Arc::clone(&app.playback);
        let analysis = Arc::clone(&app.analysis);
        let exporter = Arc::clone(&app.exporter);
        let writer = Arc::clone(&app.sidecar_writer);
        let mut reopened = ProjectSession::create(
            9,
            "copy",
            document,
            Some(d_path),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: digest,
            },
            Some(writer),
            version,
            None,
        )
        .expect("the copy reopens");
        {
            let log = reopened
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 0);
        }
        in2b_shutdown_session(&mut reopened);
        in2b_shutdown(&mut app);
    }

    /// Item 36: a sidecar write failure notes exactly one
    /// `sidecar_write_failed` and the project save returns success anyway.
    /// The failure is portable across lanes: the sidecar path occupied by a
    /// directory, so temp + rename fails with no chmod.
    #[test]
    fn in2b_a_sidecar_write_failure_notes_once_and_saves_anyway() {
        let temp = TempDirectory::new("in2b-write-failed");
        let project_path = temp.path("edit.kinewright");
        let (mut app, _engine) = in2b_harness(Document::default(), Some(project_path.clone()));
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");
        fs::create_dir(&sidecar).expect("the sidecar path is occupied");

        app.write_project(&project_path)
            .expect("the save succeeds despite the sidecar failure");
        assert!(project_path.is_file(), "the project bytes landed");
        assert!(sidecar.is_dir(), "the failed rename clobbers nothing");

        app.route_incidents();
        let log = app.projects[0]
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 1);
        let only = log.all().next().expect("the failure noted");
        assert_eq!(
            only.code,
            IncidentCode::Label(LabelIncident::SidecarWriteFailed)
        );
        assert!(only.transient, "every §5 note is transient");
        drop(log);
        in2b_shutdown(&mut app);
    }

    /// Item 18: a newer file opens with exactly one `project_newer_format`
    /// incident and no overwrite-save; Save As writes current-version bytes
    /// that reopen clean; versionless files open silently as v1.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_a_newer_file_opens_with_one_incident_and_no_overwrite() {
        let temp = TempDirectory::new("in2b-newer-file");
        // Box 1: `load_document` reads the version and digests the file
        // bytes. Item 18 writes its own 999 files (§4 rule 2 — no committed
        // newer-than-current fixture).
        let newer_path = temp.path("newer.kinewright");
        let written = serde_json::to_string_pretty(&ProjectFile {
            format_version: 999,
            document: Document::default(),
        })
        .expect("the 999 file serialises");
        fs::write(&newer_path, &written).expect("the 999 file writes");
        let (document, version, digest) = load_document(&newer_path).expect("the 999 file loads");
        assert_eq!(version, 999);
        assert_eq!(
            digest,
            digest_bytes(written.as_bytes()),
            "the digest covers the file bytes"
        );
        assert_eq!(document, Document::default());

        // Boxes 2–4: the headless open opens, notes exactly one incident,
        // and carries the read version.
        let (mut app, engine) = in2b_harness(Document::default(), None);
        app.open_project(&newer_path);
        assert_eq!(app.projects.len(), 2, "the newer file opens");
        assert_eq!(app.focused().format_version, 999);
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 1, "exactly one incident");
            let only = log.all().next().expect("the note landed");
            assert_eq!(
                only.code,
                IncidentCode::Label(LabelIncident::ProjectNewerFormat)
            );
            assert!(only.transient, "every §5 note is transient");
        }

        // Box 5: the gate.
        assert!(!can_overwrite_save(999));
        assert!(can_overwrite_save(1));

        // Box 6: overwrite-save refuses, re-surfaces the card, writes
        // nothing, and notes nothing second.
        assert!(!app.save_project(false), "overwrite-save refuses");
        assert_eq!(
            fs::read(&newer_path)
                .expect("the 999 file re-reads")
                .as_slice(),
            written.as_bytes(),
            "a refused save writes nothing"
        );
        assert!(app.incidents_open, "the refusal re-surfaces the card");
        assert!(
            app.status.contains("Save As"),
            "the refusal names the way out: {}",
            app.status
        );
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 1, "no second incident");
        }

        // Box 7: Save As writes current-version bytes that reopen clean.
        let copy_path = temp.path("copy.kinewright");
        app.write_project(&copy_path).expect("Save As succeeds");
        assert_eq!(
            app.focused().format_version,
            PROJECT_FORMAT_VERSION,
            "Save As resets the read version"
        );
        app.write_project(&copy_path)
            .expect("overwrite-save re-enables for the new path");
        let (copy, copy_version, copy_digest) =
            load_document(&copy_path).expect("the copy re-reads");
        assert_eq!(copy_version, 1, "Save As writes current-version bytes");
        let playback = Arc::clone(&app.playback);
        let analysis = Arc::clone(&app.analysis);
        let exporter = Arc::clone(&app.exporter);
        let writer = Arc::clone(&app.sidecar_writer);
        let mut reopened = ProjectSession::create(
            9,
            "copy",
            copy,
            Some(copy_path),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: copy_digest,
            },
            Some(writer),
            copy_version,
            None,
        )
        .expect("the copy reopens");
        {
            let log = reopened
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 0, "the copy reopens with 0 incidents");
        }
        in2b_shutdown_session(&mut reopened);
        assert_eq!(
            fs::read(&newer_path)
                .expect("the 999 file re-reads")
                .as_slice(),
            written.as_bytes(),
            "the original is never modified"
        );

        // Box 8: a versionless file reads as 1 and opens silently.
        let legacy_path = temp.path("legacy.kinewright");
        fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&Document::default()).expect("the legacy file serialises"),
        )
        .expect("the legacy file writes");
        let (_, legacy_version, _) = load_document(&legacy_path).expect("the legacy file loads");
        assert_eq!(legacy_version, 1);
        app.open_project(&legacy_path);
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 0, "a versionless file opens silently");
        }
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// Real journal bytes for the item-20 fixtures: the wire magic plus a
    /// header line. The apply path consumes only the journal path (a landed
    /// restore deletes it, a refused one shelves it) — the parse halves
    /// live in `recovery.rs` — but the bytes are shaped real so a future
    /// parse-through stays honest.
    fn journal_fixture_bytes(
        project_path: &Path,
        writer_format_version: u32,
        initial: &Document,
    ) -> Vec<u8> {
        let header = serde_json::json!({
            "format_version": 1,
            "project_path": project_path,
            "writer_format_version": writer_format_version,
            "initial_document": initial,
        });
        let mut bytes = b"KINEWRIGHT-JOURNAL 1\n".to_vec();
        bytes.extend_from_slice(
            serde_json::to_vec(&header)
                .expect("the header serialises")
                .as_slice(),
        );
        bytes.push(b'\n');
        bytes
    }

    /// Item 20: a journal newer than the last save restores pairing the
    /// saved sidecar digest-less, with revisions rebased to the opening; a
    /// 999-writer journal is refused with exactly one
    /// `project_newer_format`, its file shelved for a newer build. The
    /// restore targets the already-open path, so sidecar writes suspend
    /// until Save As (N2/S-13); the Save As half proves the suspension
    /// lifts onto the new path.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_recovery_restores_without_the_digest_gate_and_refuses_cross_version() {
        let temp = TempDirectory::new("in2b-recovery-gate");
        let project_path = temp.path("edit.kinewright");
        let (mut app, engine) = in2b_harness(Document::default(), Some(project_path.clone()));
        in2b_observe_opens(&app.projects[0], 2, 41);
        app.write_project(&project_path).expect("the save succeeds");
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");
        let saved_sidecar = fs::read(&sidecar).expect("the saved sidecar reads");

        // The crash story: one post-save edit the journal kept, and a torn
        // project file (another writer, a partial save) that breaks the
        // digest pair — the restore must load the saved history anyway.
        let mut recovered = Document::default();
        recovered.markers.push(Marker {
            id: MarkerId(7),
            position: TimeCode::ZERO,
            label: "the unsaved edit".to_owned(),
            color_token: 0,
        });
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&recovered).expect("the torn bytes serialise"),
        )
        .expect("the torn bytes write");
        let journal_path = temp.path("edit.journal");
        fs::write(
            &journal_path,
            journal_fixture_bytes(&project_path, PROJECT_FORMAT_VERSION, &Document::default()),
        )
        .expect("the journal writes");

        // The counterfactual, through the production gate: the torn file
        // matches neither digest, so a gated load would refuse.
        let SidecarLoad::Current(envelope) = load_sidecar(&sidecar) else {
            panic!("the saved sidecar parses");
        };
        assert!(
            !sidecar_matches_project(
                &envelope,
                &digest_bytes(&fs::read(&project_path).expect("the torn file reads"))
            ),
            "the torn file breaks the digest pair"
        );

        // The restore lands a session, consumes its journal, loads the saved
        // history digest-less with revisions rebased — and suspends sidecar
        // writes, because the target path is already open.
        let projects_before = app.projects.len();
        app.apply_restore_request(RestoreRequest {
            document: recovered,
            project_path: Some(project_path.clone()),
            journal_path: journal_path.clone(),
            writer_format_version: PROJECT_FORMAT_VERSION,
        });
        assert_eq!(
            app.projects.len(),
            projects_before + 1,
            "the restore lands a session"
        );
        assert!(
            !journal_path.exists(),
            "a landed restore consumes its journal"
        );
        assert!(
            app.focused().sidecar_suspended,
            "a restore onto an open path suspends sidecar writes"
        );
        assert_eq!(app.focused().format_version, PROJECT_FORMAT_VERSION);
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 2, "the saved sidecar loads digest-less");
            for incident in log.all() {
                assert_eq!(
                    incident.revision,
                    TimelineRevision::default(),
                    "restored revisions rebase to the opening"
                );
            }
        }
        let flushed = app
            .focused_mut()
            .flush_incidents_if_changed()
            .expect("the flush runs");
        assert!(
            matches!(flushed, FlushOutcome::Skipped),
            "a suspended restore skips its flush"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the sidecar re-reads"),
            saved_sidecar,
            "a restore onto an open path writes no sidecar"
        );
        let again_path = temp.path("again.kinewright");
        app.write_project(&again_path).expect("Save As succeeds");
        assert!(
            !app.focused().sidecar_suspended,
            "Save As clears the suspension"
        );
        let again_sidecar = sidecar_path_for_project(Some(&again_path)).expect("derived");
        let SidecarLoad::Current(again) = load_sidecar(&again_sidecar) else {
            panic!("the Save As sidecar parses");
        };
        assert_eq!(
            again.records.len(),
            2,
            "Save As writes the restored history to the new path"
        );

        // The refusal: a 999-writer journal lands no session, shelves its
        // file, and notes exactly one `project_newer_format`.
        let cross_path = temp.path("cross.journal");
        fs::write(
            &cross_path,
            journal_fixture_bytes(&project_path, 999, &Document::default()),
        )
        .expect("the cross-version journal writes");
        let projects_before = app.projects.len();
        let incidents_before = app
            .focused()
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        app.apply_restore_request(RestoreRequest {
            document: Document::default(),
            project_path: Some(project_path.clone()),
            journal_path: cross_path.clone(),
            writer_format_version: 999,
        });
        assert_eq!(
            app.projects.len(),
            projects_before,
            "a refused restore lands no session"
        );
        assert!(
            cross_path.exists(),
            "the refused journal is shelved, not deleted"
        );
        assert!(
            app.status.contains("Recovery refused"),
            "the refusal says so: {}",
            app.status
        );
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                log.len(),
                incidents_before + 1,
                "the refusal notes exactly one incident"
            );
            assert_eq!(
                log.all()
                    .filter(|incident| incident.code
                        == IncidentCode::Label(LabelIncident::ProjectNewerFormat))
                    .count(),
                1,
                "and its code is `project_newer_format`"
            );
        }
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// Item 21: each `panel_observation` maps its error to `Some` and its
    /// clean states to `None` — 9 sites × 6 asserts over the 8 fns. The
    /// `WorkerError` six share one box shape (both arms' code, subject,
    /// tag, transient, revision, plus `Cancelled → None`, which is both
    /// the clean and the cancelled box for a `WorkerError` input);
    /// sites 5 and 7 shape unconditionally (`Some` always); site 6 edges
    /// on the last noted message.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_each_panel_observation_maps_error_to_some_and_clean_to_none() {
        use crate::{
            chat_ui::chat_panel_observation,
            color_qc_ui::qc_panel_observation,
            color_scopes_ui::scopes_panel_observation,
            error_ui::WorkerError,
            export_ui::{
                ExportVerificationKind, conformance_panel_observation, export_panel_observation,
            },
            matte_overlay_ui::matte_panel_observation,
            preview_ui::mask_panel_observation,
            transcript_ui::transcript_panel_observation,
        };
        use kinewright_agent::BranchError;
        use kinewright_core::{
            AssetId, ClipId, ColorQcError, ColorQcIncident, DeliveryVariantError,
            DeliveryVerificationError, DeliveryVerificationIncident, MatteCoverageError,
            MatteProofError, MediaIncident, ScopeError,
        };

        let untyped = WorkerError::Untyped("the worker fell over".to_owned());
        let cancelled = WorkerError::Media(MediaError::Cancelled);

        // Site 1: scopes over the document.
        let revision = TimelineRevision(41);
        let typed = WorkerError::Media(MediaError::Scope(ScopeError::EmptyFrames));
        let untyped_note = scopes_panel_observation(&untyped, revision).expect("Untyped notes");
        let typed_note = scopes_panel_observation(&typed, revision).expect("Media notes");
        assert!(
            untyped_note.code == IncidentCode::Label(LabelIncident::PanelWorkerError)
                && typed_note.code == IncidentCode::Media(MediaIncident::BackendUnclassified),
            "site 1 codes: panel_worker_error / media_backend_unclassified"
        );
        assert!(
            untyped_note.subject == IncidentSubject::Project
                && typed_note.subject == IncidentSubject::Project,
            "site 1 subjects"
        );
        assert!(
            untyped_note.observed.starts_with("Scopes: ")
                && typed_note.observed.starts_with("Scopes: "),
            "site 1 tags"
        );
        assert!(
            untyped_note.transient && typed_note.transient,
            "site 1 provenance"
        );
        assert!(
            untyped_note.revision == revision && typed_note.revision == revision,
            "site 1 revisions"
        );
        assert_eq!(
            scopes_panel_observation(&cancelled, revision),
            None,
            "site 1 Cancelled notes nothing"
        );

        // Site 2: QC over the document.
        let revision = TimelineRevision(42);
        let typed = WorkerError::Media(MediaError::ColorQc(ColorQcError::ProxyProofRefused {
            observed: "a proxy".to_owned(),
            allowed: "a proof",
        }));
        let untyped_note = qc_panel_observation(&untyped, revision).expect("Untyped notes");
        let typed_note = qc_panel_observation(&typed, revision).expect("Media notes");
        assert!(
            untyped_note.code == IncidentCode::Label(LabelIncident::PanelWorkerError)
                && typed_note.code == IncidentCode::ColorQc(ColorQcIncident::ProxyProofRefused),
            "site 2 codes"
        );
        assert!(
            untyped_note.subject == IncidentSubject::Project
                && typed_note.subject == IncidentSubject::Project,
            "site 2 subjects"
        );
        assert!(
            untyped_note.observed.starts_with("QC: ") && typed_note.observed.starts_with("QC: "),
            "site 2 tags"
        );
        assert!(
            untyped_note.transient && typed_note.transient,
            "site 2 provenance"
        );
        assert!(
            untyped_note.revision == revision && typed_note.revision == revision,
            "site 2 revisions"
        );
        assert_eq!(
            qc_panel_observation(&cancelled, revision),
            None,
            "site 2 Cancelled notes nothing"
        );

        // Site 3: the mask over the document.
        let revision = TimelineRevision(43);
        let typed = WorkerError::Media(MediaError::MatteCoverage(
            MatteCoverageError::InvalidDimensions {
                observed: "2x1".to_owned(),
                allowed: "the frame's dimensions",
            },
        ));
        let untyped_note = mask_panel_observation(&untyped, revision).expect("Untyped notes");
        let typed_note = mask_panel_observation(&typed, revision).expect("Media notes");
        assert!(
            untyped_note.code == IncidentCode::Label(LabelIncident::PanelWorkerError)
                && typed_note.code == IncidentCode::Media(MediaIncident::BackendUnclassified),
            "site 3 codes"
        );
        assert!(
            untyped_note.subject == IncidentSubject::Project
                && typed_note.subject == IncidentSubject::Project,
            "site 3 subjects"
        );
        assert!(
            untyped_note.observed.starts_with("QC mask: ")
                && typed_note.observed.starts_with("QC mask: "),
            "site 3 tags"
        );
        assert!(
            untyped_note.transient && typed_note.transient,
            "site 3 provenance"
        );
        assert!(
            untyped_note.revision == revision && typed_note.revision == revision,
            "site 3 revisions"
        );
        assert_eq!(
            mask_panel_observation(&cancelled, revision),
            None,
            "site 3 Cancelled notes nothing"
        );

        // Site 4: the matte over its clip.
        let revision = TimelineRevision(44);
        let clip = ClipId(7);
        let typed = WorkerError::Media(MediaError::MatteProof(MatteProofError::NoMatte));
        let untyped_note =
            matte_panel_observation(&untyped, clip, revision).expect("Untyped notes");
        let typed_note = matte_panel_observation(&typed, clip, revision).expect("Media notes");
        assert!(
            untyped_note.code == IncidentCode::Label(LabelIncident::PanelWorkerError)
                && typed_note.code == IncidentCode::Media(MediaIncident::BackendUnclassified),
            "site 4 codes"
        );
        assert!(
            untyped_note.subject == IncidentSubject::Clip(clip)
                && typed_note.subject == IncidentSubject::Clip(clip),
            "site 4 subjects"
        );
        assert!(
            untyped_note.observed.starts_with("Matte view: ")
                && typed_note.observed.starts_with("Matte view: "),
            "site 4 tags"
        );
        assert!(
            untyped_note.transient && typed_note.transient,
            "site 4 provenance"
        );
        assert!(
            untyped_note.revision == revision && typed_note.revision == revision,
            "site 4 revisions"
        );
        assert_eq!(
            matte_panel_observation(&cancelled, clip, revision),
            None,
            "site 4 Cancelled notes nothing"
        );

        // Site 5: the whisper string over its asset. `Some` always — the
        // caller edges novelty, so the None boxes become verbatim-shape
        // asserts instead.
        let revision = TimelineRevision(45);
        let asset = AssetId(9);
        let note =
            transcript_panel_observation("whisper boom", asset, revision).expect("a failure notes");
        assert_eq!(
            note.code,
            IncidentCode::Label(LabelIncident::PanelWorkerError),
            "site 5 code"
        );
        assert_eq!(
            note.subject,
            IncidentSubject::Asset(asset),
            "site 5 subject"
        );
        assert!(note.observed.starts_with("Transcript: "), "site 5 tag");
        assert!(note.transient, "site 5 provenance");
        assert_eq!(note.revision, revision, "site 5 revision");
        assert!(
            note.observed.contains("whisper boom"),
            "site 5 carries the string verbatim: {}",
            note.observed
        );

        // Site 6: the branch error over the chat panel, edged.
        let revision = TimelineRevision(46);
        let failure: Result<kinewright_agent::BranchComparison, BranchError> =
            Err(BranchError::UnexpectedResponse);
        let (note, stored) = chat_panel_observation(None, &failure, revision);
        let note = note.expect("a new failure notes");
        assert_eq!(
            note.code,
            BranchError::UnexpectedResponse.incident_code(),
            "site 6 code"
        );
        assert_eq!(note.subject, IncidentSubject::Agent, "site 6 subject");
        assert!(note.observed.starts_with("Branch: "), "site 6 tag");
        assert!(note.transient, "site 6 provenance");
        let (repeat, stored_again) = chat_panel_observation(stored.as_ref(), &failure, revision);
        let changed: Result<kinewright_agent::BranchComparison, BranchError> =
            Err(BranchError::InvalidOperationIndex {
                index: 9,
                maximum: 3,
            });
        assert!(
            repeat.is_none()
                && stored_again == stored
                && chat_panel_observation(stored.as_ref(), &changed, revision)
                    .0
                    .is_some(),
            "site 6 repeats stay silent, changes note again"
        );
        let compared = kinewright_agent::BranchComparison {
            name: Arc::from("test"),
            base_revision: TimelineRevision(0),
            branch_revision: TimelineRevision(1),
            base_document: Arc::new(Document::default()),
            document: Arc::new(Document::default()),
            operations: Arc::new(Vec::new()),
        };
        let cleared = chat_panel_observation(stored.as_ref(), &Ok(compared), revision);
        assert_eq!(cleared, (None, None), "site 6 success clears");

        // Site 7: the conformance refusal, untagged. `Some` always — the
        // cache miss is the novelty.
        let revision = TimelineRevision(47);
        let refusal = DeliveryVariantError::InvalidFocus { x: 101, y: 50 };
        let note = conformance_panel_observation(&refusal, revision);
        assert_eq!(note.code, refusal.incident_code(), "site 7 code");
        assert_eq!(note.subject, IncidentSubject::ExportJob, "site 7 subject");
        assert_eq!(
            note.observed,
            refusal.to_string(),
            "site 7 is untagged, deduping with the refusal note"
        );
        assert!(note.transient, "site 7 provenance");
        assert_eq!(note.revision, revision, "site 7 revision");
        let exhausted = DeliveryVariantError::EffectIdExhausted;
        assert_eq!(
            conformance_panel_observation(&exhausted, revision).code,
            exhausted.incident_code(),
            "site 7 codes every variant through the builder"
        );

        // Sites 8–9: the job verifications over the attempt, tags apart.
        for (kind, tag, number) in [
            (
                ExportVerificationKind::Verification,
                "Export verification: ",
                48,
            ),
            (
                ExportVerificationKind::AudioVerification,
                "Export audio verification: ",
                49,
            ),
        ] {
            let revision = TimelineRevision(number);
            let typed = WorkerError::Media(MediaError::DeliveryVerification(
                DeliveryVerificationError::NotFullResolution {
                    observed: "a sample".to_owned(),
                    allowed: "full resolution",
                },
            ));
            let untyped_note =
                export_panel_observation(&untyped, kind, revision).expect("Untyped notes");
            let typed_note = export_panel_observation(&typed, kind, revision).expect("Media notes");
            assert!(
                untyped_note.code == IncidentCode::Label(LabelIncident::PanelWorkerError)
                    && typed_note.code
                        == IncidentCode::DeliveryVerification(
                            DeliveryVerificationIncident::NotFullResolution
                        ),
                "site 8/9 codes"
            );
            assert!(
                untyped_note.subject == IncidentSubject::ExportJob
                    && typed_note.subject == IncidentSubject::ExportJob,
                "site 8/9 subjects"
            );
            assert!(
                untyped_note.observed.starts_with(tag) && typed_note.observed.starts_with(tag),
                "site 8/9 tags"
            );
            assert!(
                untyped_note.transient && typed_note.transient,
                "site 8/9 provenance"
            );
            assert!(
                untyped_note.revision == revision && typed_note.revision == revision,
                "site 8/9 revisions"
            );
            assert_eq!(
                export_panel_observation(&cancelled, kind, revision),
                None,
                "site 8/9 Cancelled notes nothing"
            );
        }
    }

    /// Item 31: three seam-produced sessions each end — ≥ 1 proposal, ≥ 1
    /// `Explained`, ≥ 1 either — over exactly two newly reachable codes.
    /// Every observation is a worker error through `panel_observation`
    /// into `observe` (never hand-built — S13's binding), driven by
    /// `ScriptedDriver` through the production router + pump; the
    /// incidents are pre-registered and the sessions spawn directly, so
    /// no auto-investigation is involved.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_three_seam_produced_sessions_end_over_two_newly_reachable_codes() {
        use super::in1_tests::{
            in2_call, in2_happy_turn, in2_incident, in2_next_incident_id, in2_pump_until_finished,
        };
        use kinewright_agent::{ScriptedDriver, ScriptedTurn};
        use kinewright_core::{
            ColorQcError, ColorQcIncident, DeliveryVerificationError, DeliveryVerificationIncident,
            IncidentState,
        };

        use crate::error_ui::WorkerError;

        let (mut app, _engine) = in2b_harness(Document::default(), None);

        // Session 1: a QC worker error through site 2 → proposal end.
        let revision = app.projects[0].revision;
        let expected = in2_next_incident_id(&app);
        let observation = crate::color_qc_ui::qc_panel_observation(
            &WorkerError::Media(MediaError::ColorQc(ColorQcError::NodeBudgetExceeded {
                observed: "9".to_owned(),
                allowed: "4",
            })),
            revision,
        )
        .expect("the QC error notes");
        let first = in2b_start_seam_session(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(expected, revision, 71)]),
            observation,
            vec!["opening".to_owned()],
        );
        assert_eq!(first, expected);
        in2_pump_until_finished(&mut app);
        assert!(
            in2_incident(&app, first).proposal.is_some(),
            "the first session ends with a proposal"
        );

        // Session 2: a verification worker error through site 8 →
        // `Explained` end.
        let revision = app.projects[0].revision;
        let expected = in2_next_incident_id(&app);
        let observation = crate::export_ui::export_panel_observation(
            &WorkerError::Media(MediaError::DeliveryVerification(
                DeliveryVerificationError::FrameCountOutOfRange {
                    observed: "70000".to_owned(),
                    allowed: "1..=65535",
                },
            )),
            crate::export_ui::ExportVerificationKind::Verification,
            revision,
        )
        .expect("the verification error notes");
        let second = in2b_start_seam_session(
            &mut app,
            ScriptedDriver::new(vec![ScriptedTurn::new(
                vec![in2_call("get_timeline_state", serde_json::json!({}))],
                "no honest fix exists",
            )]),
            observation,
            vec!["opening".to_owned()],
        );
        assert_eq!(second, expected);
        in2_pump_until_finished(&mut app);
        assert_eq!(
            in2_incident(&app, second).state,
            IncidentState::Resolved(IncidentOutcome::Explained),
            "the second session ends Explained"
        );

        // Session 3: the QC code again with a different worker message —
        // a distinct incident over the same newly reachable code.
        let revision = app.projects[0].revision;
        let expected = in2_next_incident_id(&app);
        let observation = crate::color_qc_ui::qc_panel_observation(
            &WorkerError::Media(MediaError::ColorQc(ColorQcError::NodeBudgetExceeded {
                observed: "12".to_owned(),
                allowed: "4",
            })),
            revision,
        )
        .expect("the second QC error notes");
        let third = in2b_start_seam_session(
            &mut app,
            ScriptedDriver::new(vec![in2_happy_turn(expected, revision, 73)]),
            observation,
            vec!["opening".to_owned()],
        );
        assert_eq!(third, expected);
        in2_pump_until_finished(&mut app);
        assert!(
            in2_incident(&app, third).proposal.is_some(),
            "the third session ends with a proposal"
        );

        // Exactly two newly reachable codes carry the three sessions.
        let codes = [first, second, third]
            .into_iter()
            .map(|id| in2_incident(&app, id).code)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            codes,
            std::collections::BTreeSet::from([
                IncidentCode::ColorQc(ColorQcIncident::NodeBudgetExceeded),
                IncidentCode::DeliveryVerification(
                    DeliveryVerificationIncident::FrameCountOutOfRange
                ),
            ]),
            "two newly reachable codes, three sessions"
        );
        super::in1_tests::in2_cleanup(&mut app);
    }

    /// Item 32: the re-measurement of `IN1b` §3.2 rule 13 — 74 codes, 74
    /// producers, 0 exempt (`IN2B` §6 rule 7, E-B9). Three parts:
    ///
    /// 1. Every `ColorQcError` variant (6) and every `DeliveryVerificationError`
    ///    variant (5) is driven through its real §6 seam mapping to its code.
    ///    A seventh variant breaks `from_media_error`'s totality at compile
    ///    time; extending this table is then the author's review-level
    ///    obligation — the ratchet's handle, not its teeth.
    /// 2. The 7 §5 codes are produced through their real note constructors
    ///    (§5 rule 4): the seam-7 shared body, the two recovery shapers, the
    ///    newer-format shaper, `restore` itself (the unknown-code aggregate
    ///    has no named constructor — the inline note in `restore` is it),
    ///    and the two sidecar shapers. Each asserts code plus `transient`.
    /// 3. The remaining 56 cite their existing producers (`IN1b` §3.2's
    ///    inventory, i.e. Appendix B's rows, cited per code below) and each
    ///    citation is re-verified by grep with a positive count: the
    ///    producer-side symbol (the error variant for mapped codes, the
    ///    `IncidentFamily` arm for families, the note-site label for labels)
    ///    must appear on a non-comment source line somewhere under `crates/`.
    ///    The table stores each pattern as two fragments joined at run time,
    ///    so the table itself never matches its own grep; `Look`/`Media`/
    ///    `Agent` carry a trailing comma so `LookIncomplete`, `MediaIncomplete`
    ///    and `AgentBranch` lines do not inflate their counts. Positions are
    ///    deliberately not pinned (that audit belongs to the §11 suite):
    ///    existence is what rule 13 claims.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_every_declared_code_has_a_producer() {
        use crate::{
            color_qc_ui::qc_panel_observation,
            error_ui::{WorkerError, worker_error_observation},
            export_ui::{ExportVerificationKind, export_panel_observation},
            project::project_newer_format_observation,
            recovery::{recovery_damage_observation, recovery_unavailable_observation},
            sidecar::{sidecar_refused_observation, sidecar_write_failed_observation},
        };
        use kinewright_core::{
            ClipId, ColorQcError, ColorQcIncident, DeliveryVerificationError,
            DeliveryVerificationIncident, EffectId, IncidentEvidence, IncidentId, IncidentLog,
            IncidentRecord, IncidentState, IncidentTelemetry,
        };

        fn rs_lines(dir: &std::path::Path, out: &mut Vec<String>) {
            let entries = std::fs::read_dir(dir)
                .unwrap_or_else(|error| panic!("{} reads: {error}", dir.display()));
            for entry in entries {
                let path = entry
                    .unwrap_or_else(|error| panic!("a dir entry reads: {error}"))
                    .path();
                if path.is_dir() {
                    rs_lines(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|error| panic!("{} reads: {error}", path.display()));
                    out.extend(
                        text.lines()
                            .filter(|line| !line.trim_start().starts_with("//"))
                            .map(str::to_owned),
                    );
                }
            }
        }

        let revision = TimelineRevision(50);

        // Part 1a: all six QC refusals through the QC seam to their codes.
        let qc_cases = [
            (
                ColorQcError::ProxyProofRefused {
                    observed: "a proxy".to_owned(),
                    allowed: "a proof",
                },
                ColorQcIncident::ProxyProofRefused,
            ),
            (
                ColorQcError::RasterLengthMismatch {
                    observed: "3".to_owned(),
                    allowed: "4".to_owned(),
                },
                ColorQcIncident::RasterLengthMismatch,
            ),
            (
                ColorQcError::EmptyPopulation {
                    observed: "empty".to_owned(),
                    allowed: "nonempty",
                },
                ColorQcIncident::EmptyPopulation,
            ),
            (
                ColorQcError::NodeBudgetExceeded {
                    observed: "9".to_owned(),
                    allowed: "8",
                },
                ColorQcIncident::NodeBudgetExceeded,
            ),
            (
                ColorQcError::MatteRegionRasterMismatch {
                    observed: "2x1".to_owned(),
                    allowed: "2x2".to_owned(),
                },
                ColorQcIncident::MatteRegionRasterMismatch,
            ),
            (
                ColorQcError::NodeRemovalRejected {
                    clip: ClipId(1),
                    effect: EffectId(2),
                    reason: "the clone refused".to_owned(),
                },
                ColorQcIncident::NodeRemovalRejected,
            ),
        ];
        assert_eq!(qc_cases.len(), 6, "all six QC variants");
        for (error, incident) in qc_cases {
            let wrapped = WorkerError::Media(MediaError::ColorQc(error));
            let note = qc_panel_observation(&wrapped, revision).expect("every QC variant notes");
            assert_eq!(
                note.code,
                IncidentCode::ColorQc(incident),
                "QC variant maps to its own code"
            );
        }

        // Part 1b: all five verification refusals through the export seam.
        let dv_cases = [
            (
                DeliveryVerificationError::NotFullResolution {
                    observed: "a sample".to_owned(),
                    allowed: "full resolution",
                },
                DeliveryVerificationIncident::NotFullResolution,
            ),
            (
                DeliveryVerificationError::PlaneOutOfContainer {
                    observed: "a sample".to_owned(),
                    allowed: "the container",
                },
                DeliveryVerificationIncident::PlaneOutOfContainer,
            ),
            (
                DeliveryVerificationError::FrameCountMismatch {
                    observed: "3".to_owned(),
                    allowed: "4".to_owned(),
                },
                DeliveryVerificationIncident::FrameCountMismatch,
            ),
            (
                DeliveryVerificationError::FrameCountOutOfRange {
                    observed: "99".to_owned(),
                    allowed: "0..=10",
                },
                DeliveryVerificationIncident::FrameCountOutOfRange,
            ),
            (
                DeliveryVerificationError::BudgetLaneMismatch {
                    observed: "lane A".to_owned(),
                    allowed: "lane B".to_owned(),
                },
                DeliveryVerificationIncident::BudgetLaneMismatch,
            ),
        ];
        assert_eq!(dv_cases.len(), 5, "all five DV variants");
        for (error, incident) in dv_cases {
            let wrapped = WorkerError::Media(MediaError::DeliveryVerification(error));
            let note =
                export_panel_observation(&wrapped, ExportVerificationKind::Verification, revision)
                    .expect("every DV variant notes");
            assert_eq!(
                note.code,
                IncidentCode::DeliveryVerification(incident),
                "DV variant maps to its own code"
            );
        }

        // Part 2: the seven §5 codes through their real note constructors.
        let collapsed = WorkerError::Untyped("the worker fell over".to_owned());
        let panel =
            worker_error_observation(&collapsed, IncidentSubject::Project, "Scopes: ", revision)
                .expect("a collapse notes");
        assert_eq!(
            panel.code,
            IncidentCode::Label(LabelIncident::PanelWorkerError),
            "seam-7 collapse"
        );
        assert!(panel.transient, "§5 notes are transient");
        for (note, code, name) in [
            (
                recovery_damage_observation("a torn journal", revision),
                LabelIncident::RecoveryDamage,
                "recovery damage",
            ),
            (
                recovery_unavailable_observation("the recorder stopped", revision),
                LabelIncident::RecoveryUnavailable,
                "recovery unavailable",
            ),
            (
                project_newer_format_observation(999, revision),
                LabelIncident::ProjectNewerFormat,
                "newer format",
            ),
            (
                sidecar_refused_observation("the digest mismatched", revision),
                LabelIncident::SidecarRefused,
                "sidecar refused",
            ),
            (
                sidecar_write_failed_observation("the disk is full", revision),
                LabelIncident::SidecarWriteFailed,
                "sidecar write failed",
            ),
        ] {
            assert_eq!(note.code, IncidentCode::Label(code), "{name} code");
            assert!(note.transient, "{name} is transient");
        }
        // The aggregate's constructor is `restore` itself: one unknown-code
        // record in, one transient `sidecar_unknown_codes` note out.
        let unknown = IncidentRecord {
            id: IncidentId(1),
            code: "no_such_code_xyz".to_owned(),
            subject: IncidentSubject::Project,
            observed: "observed 1".to_owned(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(7),
            opened_wall_millis: None,
            opened_offset_nanos: 1_000,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: None,
        };
        let mut log = IncidentLog::with_start(std::time::Instant::now(), None);
        let report = log.restore(vec![unknown], Vec::new(), TimelineRevision(100), None);
        assert_eq!(
            report.unknown_codes,
            vec![("no_such_code_xyz".to_owned(), 1)],
            "the unknown code reports"
        );
        let aggregate = log.open().next().expect("the aggregate noted");
        assert_eq!(
            aggregate.code,
            IncidentCode::Label(LabelIncident::SidecarUnknownCodes),
            "unknown-code aggregate"
        );
        assert!(aggregate.transient, "the aggregate is transient");

        // Part 3: the remaining 56 cite `IN1b` Appendix B; each citation
        // re-verified by grep with a positive count per code.
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
        let mut lines = Vec::new();
        rs_lines(&crates, &mut lines);
        assert!(!lines.is_empty(), "the workspace has source lines");
        // (code, pattern left, pattern right, Appendix B rows).
        let census = [
            (
                "unknown_source_primaries",
                "ColorSourceError",
                "UnknownPrimaries",
                "B27",
            ),
            (
                "unsupported_source_primaries",
                "ColorSourceError",
                "UnsupportedPrimaries",
                "B27",
            ),
            (
                "unknown_source_transfer",
                "ColorSourceError",
                "UnknownTransfer",
                "B27",
            ),
            (
                "unsupported_source_transfer",
                "ColorSourceError",
                "UnsupportedTransfer",
                "B27",
            ),
            (
                "unknown_source_matrix",
                "ColorSourceError",
                "UnknownMatrix",
                "B27",
            ),
            (
                "unsupported_source_matrix",
                "ColorSourceError",
                "UnsupportedMatrix",
                "B27",
            ),
            (
                "unknown_source_range",
                "ColorSourceError",
                "UnknownRange",
                "B27",
            ),
            (
                "unsupported_source_range",
                "ColorSourceError",
                "UnsupportedRange",
                "B27",
            ),
            (
                "unknown_source_white_point",
                "ColorSourceError",
                "UnknownWhitePoint",
                "B27",
            ),
            (
                "unsupported_source_white_point",
                "ColorSourceError",
                "UnsupportedWhitePoint",
                "B27",
            ),
            (
                "unknown_source_bit_depth",
                "ColorSourceError",
                "UnknownBitDepth",
                "B27",
            ),
            (
                "unsupported_source_bit_depth",
                "ColorSourceError",
                "UnsupportedBitDepth",
                "B27",
            ),
            (
                "unsupported_source_combination",
                "ColorSourceError",
                "UnsupportedCombination",
                "B27",
            ),
            (
                "unsupported_decoder_format",
                "MediaError",
                "UnsupportedDecoderFormat",
                "B27/B46/B61",
            ),
            (
                "media_backend_unclassified",
                "MediaIncident",
                "BackendUnclassified",
                "B27/B46/B61",
            ),
            (
                "unsupported_delivery_codec",
                "DeliveryColorError",
                "UnsupportedCodec",
                "B61",
            ),
            (
                "unsupported_delivery_color",
                "DeliveryColorError",
                "UnsupportedField",
                "B61",
            ),
            (
                "delivery_pixel_format_depth_mismatch",
                "DeliveryColorError",
                "PixelFormatDepthMismatch",
                "B61",
            ),
            (
                "delivery_encoder_pixel_format_unavailable",
                "DeliveryColorError",
                "EncoderPixelFormatUnavailable",
                "B61",
            ),
            (
                "operation_bounds",
                "IncidentFamily",
                "Bounds",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_malformed",
                "IncidentFamily",
                "Malformed",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_duplicate",
                "IncidentFamily",
                "Duplicate",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_placement",
                "IncidentFamily",
                "Placement",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_missing",
                "IncidentFamily",
                "Missing",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_structure",
                "IncidentFamily",
                "Structure",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_relink",
                "IncidentFamily",
                "Relink",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_unrepresentable",
                "IncidentFamily",
                "Unrepresentable",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_unknown_name",
                "IncidentFamily",
                "UnknownName",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "operation_internal",
                "IncidentFamily",
                "Internal",
                "B12/B14-19/B21/B63/B76/B77/B84/B90/B102",
            ),
            (
                "operation_color_policy",
                "IncidentFamily",
                "ColorPolicy",
                "B24/B25/B40/B44+App.A",
            ),
            (
                "lut_asset_policy",
                "IncidentCode",
                "LutAssetPolicy",
                "B24-family+§3.1r6",
            ),
            (
                "edit_revision_conflict",
                "IncidentCode",
                "EditRevisionConflict",
                "B26/B39/B43/B85/B89/B91",
            ),
            (
                "edit_plan_rejected",
                "RejectionIncident",
                "EditPlan",
                "B25/B40/B44",
            ),
            (
                "delivery_variant_rejected",
                "RejectionIncident",
                "DeliveryVariant",
                "B49/B50/B51",
            ),
            (
                "agent_branch_rejected",
                "RejectionIncident",
                "AgentBranch",
                "B41/B42/B45",
            ),
            (
                "source_edit_rejected",
                "RejectionIncident",
                "SourceEdit",
                "B82",
            ),
            ("relink_rejected", "RejectionIncident", "Relink", "B86/B93"),
            (
                "project_save_failed",
                "RejectionIncident",
                "ProjectSave",
                "B5",
            ),
            (
                "caption_plan_rejected",
                "RejectionIncident",
                "CaptionPlan",
                "B33",
            ),
            (
                "operations_unclassified",
                "LabelIncident",
                "Operations",
                "B64-72/B74/B75/B78/B116-123",
            ),
            (
                "look_unclassified",
                "LabelIncident",
                "Look,",
                "B62/B73/B94-102/B104",
            ),
            (
                "look_incomplete",
                "LabelIncident",
                "LookIncomplete",
                "B3/B4/B9/B10/B103",
            ),
            ("export_unclassified", "LabelIncident", "Export", "B52-59"),
            (
                "source_monitor_unclassified",
                "LabelIncident",
                "SourceMonitor",
                "B23/B80/B81/B83/B105-110",
            ),
            ("relink_unclassified", "LabelIncident", "Relink", "B87/B92"),
            (
                "agent_branch_unclassified",
                "LabelIncident",
                "AgentBranch",
                "B38/B48",
            ),
            (
                "transcript_edit_unclassified",
                "LabelIncident",
                "TranscriptEdit",
                "B125-131",
            ),
            (
                "media_unclassified",
                "LabelIncident",
                "Media,",
                "B22/B79/B124/B132",
            ),
            (
                "media_incomplete",
                "LabelIncident",
                "MediaIncomplete",
                "B2/B11/B20",
            ),
            (
                "agent_unclassified",
                "LabelIncident",
                "Agent,",
                "B34-37/B47",
            ),
            (
                "recording_unclassified",
                "LabelIncident",
                "Recording",
                "B111-115",
            ),
            (
                "project_unclassified",
                "LabelIncident",
                "Project",
                "B1/B6-8/B28",
            ),
            (
                "captions_unclassified",
                "LabelIncident",
                "Captions",
                "B32/B60",
            ),
            ("mixer_unclassified", "LabelIncident", "Mixer", "B29-31"),
            (
                "media_cache_unclassified",
                "LabelIncident",
                "MediaCache",
                "B88",
            ),
            ("timeline_unclassified", "LabelIncident", "Timeline", "B62"),
        ];
        assert_eq!(census.len(), 56, "56 cited codes");
        for (code, left, right, rows) in census {
            let needle = format!("{left}::{right}");
            let count = lines.iter().filter(|line| line.contains(&needle)).count();
            assert!(
                count > 0,
                "IN1b Appendix B {rows} producer for `{code}` still exists (`{needle}`)"
            );
        }
    }

    /// Item 24: the first dedup into a loaded open joins `opened` — audit,
    /// auto-apply check and enqueue run exactly as over a fresh open
    /// (N1/B3(ii-a), N2/S-15b) — while the second dedup is silent and the
    /// never-re-seen row keeps Investigate (S-4).
    ///
    /// The pump is blocked by a fabricated pending session (its result
    /// never arrives), so the queue still holds the re-observed row when
    /// the test reads it — no timing, no real model turn.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_first_dedup_into_loaded_joins_opened_and_second_does_not() {
        use super::in1_tests::{
            in2_cleanup, in2_configure_scripted_at, in2_default_budgets, in2_incident,
        };
        use kinewright_agent::ScriptedDriver;
        use kinewright_core::{
            INVESTIGATOR_ALLOWLIST, IncidentEvidence, IncidentRecord, IncidentState,
            IncidentTelemetry,
        };

        use crate::incident_ui::{CardPress, investigate_action};
        use crate::sidecar::{digest_bytes, sidecar_path_for_project};

        // Two allowlisted opens on disk.
        let code_one = IncidentCode::Label(LabelIncident::Look);
        let code_two = IncidentCode::Label(LabelIncident::MediaIncomplete);
        assert!(
            INVESTIGATOR_ALLOWLIST.contains(&code_one)
                && INVESTIGATOR_ALLOWLIST.contains(&code_two),
            "both rows are allowlisted"
        );
        let record = |id: u64, code: IncidentCode, observed: &str| IncidentRecord {
            id: IncidentId(id),
            code: code.code().to_owned(),
            subject: IncidentSubject::Project,
            observed: observed.to_owned(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(41),
            opened_wall_millis: None,
            opened_offset_nanos: 1_000 * id,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: (id == 1).then_some(Operation::DeleteClip {
                clip: kinewright_core::ClipId(1),
            }),
        };
        let dir = TempDirectory::new("in2b-item-24");
        let project_path = dir.path("edit.kinewright");
        fs::write(&project_path, b"{\"timeline\":{}}").expect("the project file writes");
        let digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));
        let envelope = serde_json::json!({
            "format_version": 1,
            "project_digest": digest.clone(),
            "previous_digest": digest.clone(),
            "records": [
                serde_json::to_value(record(1, code_one, "loaded one")).expect("record 1"),
                serde_json::to_value(record(2, code_two, "loaded two")).expect("record 2"),
            ],
        });
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("a sidecar derives");
        fs::write(
            &sidecar,
            serde_json::to_string(&envelope).expect("the envelope"),
        )
        .expect("the sidecar writes");

        let (mut app, _engine) = in2b_harness(Document::default(), Some(project_path));
        assert_eq!(
            app.projects[0].loaded_open_ids.len(),
            2,
            "both rows load re-seeable"
        );
        let stashed = Operation::DeleteClip {
            clip: kinewright_core::ClipId(1),
        };
        assert_eq!(
            app.projects[0].refused_by_id.get(&IncidentId(1)),
            Some(&stashed),
            "the restored stash fills from the report"
        );
        in2_configure_scripted_at(
            &mut app,
            0,
            ScriptedDriver::new(vec![]),
            in2_default_budgets(),
        );

        // The pump blocker on a different pair from both rows.
        let blocker_tx = in2b_install_pump_blocker(
            &mut app,
            code_one,
            IncidentSubject::Asset(kinewright_core::AssetId(77)),
        );
        let revision = app.projects[0].revision;

        // First re-observation of row 1: audit, auto-apply check, enqueue.
        let audits = app.error_log.count_with_source("Incident");
        app.pending_observations.push(IncidentObservation::plain(
            code_one,
            IncidentSubject::Project,
            "loaded one",
            revision,
        ));
        app.route_incidents();
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            audits + 1,
            "the first dedup audits exactly as a fresh open"
        );
        assert!(
            app.pending_router_applies.is_empty(),
            "the auto-apply check ran and declined the Explain row"
        );
        let queued = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state")
            .queued_count();
        assert_eq!(queued, 1, "the first dedup enqueues");
        assert!(
            !app.projects[0].loaded_open_ids.contains(&IncidentId(1))
                && app.projects[0].loaded_open_ids.contains(&IncidentId(2)),
            "row 1 is consumed, row 2 stays re-seeable"
        );
        assert!(
            !app.projects[0].refused_by_id.contains_key(&IncidentId(1)),
            "the first dedup consumes the restored stash into the queued incident"
        );

        // Second re-observation: silent.
        app.pending_observations.push(IncidentObservation::plain(
            code_one,
            IncidentSubject::Project,
            "loaded one",
            revision,
        ));
        app.route_incidents();
        let queued_again = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state")
            .queued_count();
        assert_eq!(queued_again, 1, "no second enqueue");
        assert_eq!(
            app.error_log.count_with_source("Incident"),
            audits + 1,
            "no second audit"
        );

        // Row 2 keeps Investigate: never re-seen, never investigated, and
        // the shared predicate reports eligible.
        let incident = in2_incident(&app, IncidentId(2));
        assert!(
            incident.telemetry.resolver.is_none(),
            "no session ever ended on row 2"
        );
        let is_loaded = app.projects[0].loaded_open_ids.contains(&IncidentId(2));
        assert!(is_loaded, "row 2 never re-seen");
        let eligible =
            app.investigator_session_eligible(0, IncidentId(2), code_two, incident.subject);
        assert!(eligible, "the shared predicate reports row 2 eligible");
        let missing = app.projects[0].subject_missing.contains(&IncidentId(2));
        assert_eq!(
            investigate_action(&incident, is_loaded, eligible, missing),
            Some(CardPress::Investigate),
            "row 2 keeps Investigate"
        );
        assert_eq!(
            investigate_action(&incident, is_loaded, eligible, true),
            None,
            "N6/H7: no Investigate when the subject is missing"
        );

        // The write re-persists the queued incident's op (§3 rule 13): the
        // queue drain refills the stash the dedup consumed.
        app.projects[0]
            .flush_incidents(&digest, &digest)
            .expect("the flush lands");
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&sidecar).expect("the sidecar re-reads"))
                .expect("the sidecar parses");
        let saved_one = saved["records"]
            .as_array()
            .expect("records")
            .iter()
            .find(|record| record["id"] == 1)
            .expect("row 1 re-persisted");
        assert_eq!(
            saved_one["count"], 3,
            "these are rewritten bytes (1 + 2 dedups), not the loaded ones"
        );
        assert!(
            saved_one["refused_op"].is_object(),
            "the queued op rides the record"
        );

        drop(blocker_tx);
        in2_cleanup(&mut app);
    }

    /// Item 25: loading never starts a session; the Investigate press
    /// enqueues through the shared predicate, and Re-investigate on the
    /// loaded stale proposal queues beside it.
    ///
    /// Row 2's proposal and resolver ride the sidecar JSON (typed
    /// round-trip through `IncidentRecord`), so `restore` — not the test —
    /// forces staleness: the S-11 trap stays shut.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_load_starts_no_session_and_presses_queue() {
        use super::in1_tests::{
            in2_cleanup, in2_configure_scripted_at, in2_default_budgets, in2_incident,
        };
        use kinewright_agent::ScriptedDriver;
        use kinewright_core::{
            IncidentEvidence, IncidentProposal, IncidentRecord, IncidentState, IncidentTelemetry,
        };

        use crate::sidecar::{digest_bytes, sidecar_path_for_project};

        let code_one = IncidentCode::Label(LabelIncident::Look);
        let code_two = IncidentCode::Label(LabelIncident::MediaIncomplete);
        let record = |id: u64, code: IncidentCode, observed: &str| IncidentRecord {
            id: IncidentId(id),
            code: code.code().to_owned(),
            subject: IncidentSubject::Project,
            observed: observed.to_owned(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(41),
            opened_wall_millis: None,
            opened_offset_nanos: 1_000 * id,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: None,
        };
        // Row 2's stale proposal arrives as JSON: `Session` resolvers have
        // no public constructor, so the typed round-trip proves the shape.
        let mut row_two =
            serde_json::to_value(record(2, code_two, "loaded stale")).expect("record 2 serialises");
        row_two["proposal"] = serde_json::to_value(&IncidentProposal {
            operations: Vec::new(),
            operation_count: 1,
            summary: "set the gain".to_owned(),
            explanation: "the take is quiet".to_owned(),
            base_revision: TimelineRevision(41),
            stale: false,
        })
        .expect("the proposal serialises");
        row_two["telemetry"]["resolver"] = serde_json::json!({
            "session": {"harness": "scripted", "model": null, "stop": "done"},
        });
        let row_two =
            serde_json::from_value::<IncidentRecord>(row_two).expect("row 2 parses as a record");

        let dir = TempDirectory::new("in2b-item-25");
        let project_path = dir.path("edit.kinewright");
        fs::write(&project_path, b"{\"timeline\":{}}").expect("the project file writes");
        let digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));
        let envelope = serde_json::json!({
            "format_version": 1,
            "project_digest": digest,
            "previous_digest": digest,
            "records": [
                serde_json::to_value(record(1, code_one, "loaded plain")).expect("record 1"),
                serde_json::to_value(&row_two).expect("record 2"),
            ],
        });
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("a sidecar derives");
        fs::write(
            &sidecar,
            serde_json::to_string(&envelope).expect("the envelope"),
        )
        .expect("the sidecar writes");

        let (mut app, _engine) = in2b_harness(Document::default(), Some(project_path));
        let session = app.projects[0].investigator.as_ref().expect("investigator");
        assert_eq!(session.queued_count(), 0, "load queues nothing");
        assert!(!session.is_running(), "load starts nothing");
        let stale = in2_incident(&app, IncidentId(2));
        assert!(
            stale
                .proposal
                .as_ref()
                .is_some_and(|proposal| proposal.stale)
                && stale
                    .proposal
                    .as_ref()
                    .is_some_and(|proposal| proposal.operations.is_empty()),
            "restore forced the loaded proposal stale with no operations"
        );
        assert!(
            stale.telemetry.resolver.is_some(),
            "restore kept the session resolver"
        );

        in2_configure_scripted_at(
            &mut app,
            0,
            ScriptedDriver::new(vec![]),
            in2_default_budgets(),
        );
        // The pump blocker on a pair of its own, so both presses queue
        // instead of starting.
        let blocker_tx = in2b_install_pump_blocker(
            &mut app,
            code_one,
            IncidentSubject::Asset(kinewright_core::AssetId(77)),
        );

        assert!(
            app.investigate(0, IncidentId(1)),
            "Investigate on the plain row queues"
        );
        let queued = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state")
            .queued_count();
        assert_eq!(queued, 1, "one press, one queued");
        assert!(
            !app.projects[0].loaded_open_ids.contains(&IncidentId(1)),
            "the press consumes the set"
        );
        assert!(
            app.reinvestigate(0, IncidentId(2)),
            "Re-investigate on the stale row queues"
        );
        let queued_both = app.projects[0]
            .investigator
            .as_ref()
            .expect("investigator state")
            .queued_count();
        assert_eq!(queued_both, 2, "both presses queued, neither started");
        assert_eq!(
            in2_incident(&app, IncidentId(1)).state,
            IncidentState::Open,
            "row 1 still open"
        );
        assert_eq!(
            in2_incident(&app, IncidentId(2)).state,
            IncidentState::Open,
            "row 2 still open"
        );

        drop(blocker_tx);
        in2_cleanup(&mut app);
    }

    /// Item 26: the seven §5 codes note and open but never start a
    /// session (BR38 + BR40 + §5 rule 5) — even with a working harness
    /// configured, so the allowlist (not a missing harness) is what
    /// refuses them.
    #[test]
    fn in2b_the_seven_section_five_codes_start_no_sessions() {
        use super::in1_tests::{in2_cleanup, in2_configure_scripted_at, in2_default_budgets};
        use kinewright_agent::ScriptedDriver;
        use kinewright_core::INVESTIGATOR_ALLOWLIST;

        use crate::error_ui::{WorkerError, worker_error_observation};
        use crate::project::project_newer_format_observation;
        use crate::recovery::{recovery_damage_observation, recovery_unavailable_observation};
        use crate::sidecar::{sidecar_refused_observation, sidecar_write_failed_observation};

        let (mut app, _engine) = in2b_harness(Document::default(), None);
        in2_configure_scripted_at(
            &mut app,
            0,
            ScriptedDriver::new(vec![]),
            in2_default_budgets(),
        );
        let revision = app.projects[0].revision;
        let collapsed = WorkerError::Untyped("the worker fell over".to_owned());
        let observations = [
            worker_error_observation(&collapsed, IncidentSubject::Project, "Scopes: ", revision)
                .expect("a collapse notes"),
            recovery_damage_observation("a torn journal", revision),
            recovery_unavailable_observation("the recorder stopped", revision),
            project_newer_format_observation(999, revision),
            // The aggregate's real constructor is `restore`; the allowlist
            // gates the code, so a plain triple with it exercises the same
            // refusal through the rig.
            IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::SidecarUnknownCodes),
                IncidentSubject::Project,
                "unknown code 'zzz'",
                revision,
            ),
            sidecar_refused_observation("the digest mismatched", revision),
            sidecar_write_failed_observation("the disk is full", revision),
        ];
        assert_eq!(observations.len(), 7, "all seven §5 codes");
        for observation in &observations {
            assert!(
                !INVESTIGATOR_ALLOWLIST.contains(&observation.code),
                "§5 codes sit off the allowlist"
            );
        }
        app.pending_observations.extend(observations);
        app.route_incidents();

        let open = app.projects[0]
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open_count();
        assert_eq!(open, 7, "7 incidents opened");
        let session = app.projects[0].investigator.as_ref().expect("investigator");
        assert_eq!(session.queued_count(), 0, "no session queued");
        assert!(!session.is_running(), "no session started");

        in2_cleanup(&mut app);
    }

    /// Item 27: cards over `restore()`-built incidents (S-11) — the
    /// loaded stale proposal offers Re-investigate with no Approve and the
    /// card agrees; the loaded `Resolved(Applied)` asset row offers Revert
    /// exactly when the production probe says so; and the S-15a gate box:
    /// a card over a missing `Clip(9)` reads "no longer in this project"
    /// with no enabled operation.
    ///
    /// Resolution runs through production `capture_loaded_sets`, the pair
    /// check through the real `can_reinvestigate`, and the revert bit
    /// through the production asset probe — no fixture doubles anywhere.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_loaded_cards_offer_reinvestigate_revert_and_the_gate_box() {
        use kinewright_core::{
            AssetId, ClipId, ColorDescription, IncidentEvidence, IncidentLog, IncidentProposal,
            IncidentRecord, IncidentState, MediaAsset, MediaKind, MediaSourceFingerprint, Rational,
            RecoveryKind, SourceColorIncident, TimeCode,
        };

        use crate::incident_ui::{
            CardLoadedFlags, REVERT_LABEL, SUBJECT_MISSING_NOTICE, incident_card,
            revert_available_for_asset,
        };
        use crate::investigator::{
            InvestigatorSessionPairs, ProposalAction, proposal_card_with_reinvestigate,
        };
        use crate::project::{IncidentLogHandle, capture_loaded_sets};

        let look = IncidentCode::Label(LabelIncident::Look);
        let record = |id: u64, code: IncidentCode, subject: IncidentSubject, observed: &str| {
            IncidentRecord {
                id: IncidentId(id),
                code: code.code().to_owned(),
                subject,
                observed: observed.to_owned(),
                allowed: None,
                evidence: IncidentEvidence::Plain,
                revision: TimelineRevision(41),
                opened_wall_millis: Some(1_700_000_000_000),
                opened_offset_nanos: 1_000 * id,
                count: 1,
                state: IncidentState::Open,
                telemetry: kinewright_core::IncidentTelemetry::default(),
                proposal: None,
                subject_name: None,
                refused_op: None,
            }
        };
        // Row 1's stale proposal arrives as JSON (as in item 25).
        let mut row_one =
            serde_json::to_value(record(1, look, IncidentSubject::Project, "loaded stale"))
                .expect("record 1 serialises");
        row_one["proposal"] = serde_json::to_value(&IncidentProposal {
            operations: Vec::new(),
            operation_count: 1,
            summary: "set the gain".to_owned(),
            explanation: "the take is quiet".to_owned(),
            base_revision: TimelineRevision(41),
            stale: false,
        })
        .expect("the proposal serialises");
        row_one["telemetry"]["resolver"] = serde_json::json!({
            "session": {"harness": "scripted", "model": null, "stop": "done"},
        });
        let row_one =
            serde_json::from_value::<IncidentRecord>(row_one).expect("row 1 parses as a record");
        // Row 2: resolved-by-apply with probed evidence and a stored name.
        let mut row_two = record(2, look, IncidentSubject::Asset(AssetId(1)), "graded shot");
        row_two.state = IncidentState::Resolved(IncidentOutcome::Applied);
        row_two.evidence = IncidentEvidence::SourceColor {
            probed: ColorDescription::default(),
            assumption: None,
        };
        row_two.subject_name = Some("graded.mov".to_owned());
        // Row 3: a clip the document lacks. Row 4: an asset it lacks,
        // carrying an op-yielding code — the disable mechanism's pin.
        let row_three = record(3, look, IncidentSubject::Clip(ClipId(9)), "gone clip");
        let mut row_four = record(
            4,
            IncidentCode::SourceColor(SourceColorIncident::UnknownPrimaries),
            IncidentSubject::Asset(AssetId(9)),
            "gone asset",
        );
        row_four.evidence = IncidentEvidence::SourceColor {
            probed: ColorDescription::default(),
            assumption: None,
        };

        let mut log = IncidentLog::with_start(std::time::Instant::now(), None);
        let report = log.restore(
            vec![row_one, row_two, row_three, row_four],
            Vec::new(),
            TimelineRevision(100),
            None,
        );
        assert_eq!(report.restored_open, 3, "three opens restore");
        assert_eq!(report.restored_resolved, 1, "one resolved restores");
        let handle: IncidentLogHandle = std::sync::Arc::new(std::sync::RwLock::new(log));

        let mut doc_full = Document::default();
        doc_full.media_pool.push(MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("graded.mov"),
            name: "graded.mov".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).expect("30 fps"),
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
            assumed_from: Some(ColorDescription::default()),
        });
        let doc_empty = Document::default();
        let (loaded, loaded_open, missing_full) = capture_loaded_sets(&handle, &doc_full);
        let (_, _, missing_empty) = capture_loaded_sets(&handle, &doc_empty);
        assert_eq!(loaded.len(), 4, "every row loaded");
        assert!(
            loaded_open.contains(&IncidentId(1)) && !loaded_open.contains(&IncidentId(2)),
            "opens stay re-seeable, resolved does not"
        );
        assert!(
            missing_full == [IncidentId(3), IncidentId(4)].into_iter().collect(),
            "clip 9 and asset 9 are gone from the full document"
        );
        assert!(
            missing_empty
                == [IncidentId(2), IncidentId(3), IncidentId(4)]
                    .into_iter()
                    .collect(),
            "asset 1 joins them in the empty document"
        );
        let flags =
            |id: IncidentId, missing: &std::collections::BTreeSet<IncidentId>| CardLoadedFlags {
                is_loaded_open: loaded_open.contains(&id),
                eligible: false,
                subject_missing: missing.contains(&id),
                earlier_session: loaded.contains(&id),
                loaded_wall_millis: Some(1_700_000_000_000),
            };

        // (A) The loaded stale proposal: Re-investigate, no Approve.
        let log = handle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let one = log.get(IncidentId(1)).cloned().expect("row 1");
        let pairs = InvestigatorSessionPairs::default();
        let can = pairs.can_reinvestigate(IncidentId(1), one.code, one.subject);
        assert!(can, "the real pair check passes the idle pair");
        let proposal = one.proposal.as_ref().expect("row 1 proposes");
        assert!(proposal.stale, "restore forced staleness");
        let proposal_card = proposal_card_with_reinvestigate(&one, proposal, can);
        assert_eq!(
            proposal_card.actions,
            vec![ProposalAction::Reinvestigate],
            "stale offers Re-investigate with no Approve"
        );
        let card_one = incident_card(&one, false, &flags(IncidentId(1), &missing_full));
        assert!(card_one.reinvestigate, "the card agrees");
        assert!(card_one.earlier_session, "the earlier-session marker shows");
        assert_eq!(
            card_one.recency_millis,
            Some(1_700_000_000_000),
            "the wall stamp rides the card"
        );

        // (B) Revert exactly when the production probe says so.
        let two = log.get(IncidentId(2)).cloned().expect("row 2");
        assert!(
            revert_available_for_asset(&doc_full, AssetId(1)),
            "asset 1 present with an assumption"
        );
        let card_full = incident_card(&two, true, &flags(IncidentId(2), &missing_full));
        assert_eq!(card_full.actions.len(), 1, "one action");
        assert_eq!(card_full.actions[0].label, REVERT_LABEL);
        assert!(card_full.actions[0].enabled, "offered when available");
        assert_eq!(
            card_full.subject_display, "graded.mov · Asset 1",
            "the stored name renders"
        );
        assert!(
            !revert_available_for_asset(&doc_empty, AssetId(1)),
            "asset 1 missing"
        );
        let card_empty = incident_card(&two, false, &flags(IncidentId(2), &missing_empty));
        assert_eq!(card_empty.actions.len(), 1, "the action still shows");
        assert!(!card_empty.actions[0].enabled, "disabled when missing");

        // (C) The gate box: clip 9 gone, notice shown, nothing enabled.
        let three = log.get(IncidentId(3)).cloned().expect("row 3");
        let card_three = incident_card(&three, false, &flags(IncidentId(3), &missing_full));
        assert_eq!(
            card_three.missing_subject_notice,
            Some(SUBJECT_MISSING_NOTICE),
            "the card reads 'no longer in this project'"
        );
        assert!(
            card_three.actions.iter().all(|action| !action.enabled),
            "no enabled operation"
        );
        // The mechanism behind it, pinned on an op-yielding row: the same
        // card with the flag cleared enables the op.
        let four = log.get(IncidentId(4)).cloned().expect("row 4");
        let card_four = incident_card(&four, false, &flags(IncidentId(4), &missing_full));
        assert_eq!(card_four.actions.len(), 1, "one op-yielding action");
        assert!(
            matches!(
                card_four.actions[0].recovery.kind,
                RecoveryKind::Operation(_)
            ),
            "the action is an operation"
        );
        assert!(!card_four.actions[0].enabled, "disabled while missing");
        let mut present = flags(IncidentId(4), &missing_full);
        present.subject_missing = false;
        let card_present = incident_card(&four, false, &present);
        assert!(card_present.actions[0].enabled, "the flag is the switch");
    }

    /// Item 28 (S-3): rejecting the same asset twice — renamed between the
    /// rejections — resolves one incident with count 2 and the FIRST name:
    /// the dedup key is the `(code, subject, observed)` triple, and `observe`
    /// never restamps a name on a dedup. The reunion half pins the other
    /// side of S-3: two observations differing only in name hit the same
    /// ring entry. Then a scripted session proposes over the deduped
    /// incident, which stays unresolved while its fresh proposal offers
    /// Re-investigate.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_branch_rejections_with_new_names_dedup_and_union() {
        use super::in1_tests::{
            in2_cleanup, in2_configure_scripted_at, in2_default_budgets, in2_happy_turn,
            in2_incident, in2_pump_until_finished,
        };
        use kinewright_agent::{BranchError, ScriptedDriver};
        use kinewright_core::{
            ColorDescription, IncidentResolver, IncidentState, MediaAsset, MediaKind,
            MediaSourceFingerprint, Rational, TimeCode,
        };

        use crate::investigator::{
            ProposalAction, QueuedIncident, proposal_card_with_reinvestigate,
        };

        // The asset, named "Interview A".
        let mut document = Document::default();
        document.media_pool.push(MediaAsset {
            id: AssetId(5),
            path: std::path::PathBuf::from("interview.mov"),
            name: "Interview A".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).expect("30 fps"),
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        });
        // No harness yet: half 1 is route mechanics, and without a harness
        // `consider` refuses everything, so nothing queues.
        let (mut app, _engine) = in2b_harness(document, None);
        let revision = app.projects[0].revision;
        let rejection = BranchError::InvalidOperationIndex {
            index: 9,
            maximum: 3,
        };
        let subject = IncidentSubject::Asset(AssetId(5));

        // First rejection: opens, capturing "Interview A".
        app.pending_observations
            .push(rejection.incident_observation(subject, revision));
        app.route_incidents();
        let id = {
            let log = app.projects[0]
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1, "one incident opened");
            log.open().next().expect("the open incident").id
        };
        assert_eq!(
            in2_incident(&app, id).subject_name,
            Some("Interview A".to_owned()),
            "the open captures the asset's name"
        );

        // Rename (same branch, same id) and reject again: dedups.
        std::sync::Arc::make_mut(&mut app.projects[0].document).media_pool[0].name =
            "Interview B".to_owned();
        app.pending_observations
            .push(rejection.incident_observation(subject, revision));
        app.route_incidents();
        let deduped = in2_incident(&app, id);
        assert_eq!(deduped.count, 2, "same triple, second sighting");
        assert_eq!(
            deduped.subject_name,
            Some("Interview A".to_owned()),
            "the dedup never restamps the name"
        );

        // Reunion: two observations differing only in name hit the same
        // ring entry (and, above, the same incident).
        let operation = Operation::DeleteClip {
            clip: kinewright_core::ClipId(1),
        };
        let mut named_a = rejection.incident_observation(subject, revision);
        named_a.name = Some("Interview A".to_owned());
        let mut named_b = rejection.incident_observation(subject, revision);
        named_b.name = Some("Interview B".to_owned());
        let mut other_seen = rejection.incident_observation(subject, revision);
        other_seen.observed = "something else entirely".to_owned();
        let session = app.projects[0]
            .investigator
            .as_mut()
            .expect("investigator state");
        session.stash_refused(named_a.clone(), operation.clone());
        assert_eq!(
            session.take_refused(&other_seen),
            None,
            "a different observed matches nothing; the entry stays"
        );
        assert_eq!(
            session.take_refused(&named_b),
            Some(operation.clone()),
            "the same triple reunites under a new name"
        );
        assert_eq!(
            session.take_refused(&named_a),
            None,
            "one entry: the first taker consumed it"
        );

        // A scripted session proposes over the deduped incident: it stays
        // unresolved, and the fresh proposal offers Re-investigate.
        let code = deduped.code;
        let driver = ScriptedDriver::new(vec![in2_happy_turn(id, revision, 71)]);
        in2_configure_scripted_at(&mut app, 0, driver, in2_default_budgets());
        {
            let mut log = app.projects[0]
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(log.begin_investigation(id), "the session takes it");
        }
        let queued = QueuedIncident {
            id,
            code,
            subject,
            refused: None,
        };
        app.spawn_investigator_session(
            0,
            &queued,
            "scripted",
            vec!["Look into this.".to_owned()],
            Vec::new(),
        )
        .expect("the session spawns");
        in2_pump_until_finished(&mut app);
        let proposed = in2_incident(&app, id);
        // A `Completed`-with-proposal end writes telemetry without changing
        // the state: the incident stays `Investigating` — unresolved — with
        // the proposal pending on it.
        assert_eq!(
            proposed.state,
            IncidentState::Investigating,
            "a proposal leaves the incident unresolved"
        );
        let proposal = proposed.proposal.as_ref().expect("the session proposed");
        assert!(
            matches!(
                proposed.telemetry.resolver,
                Some(IncidentResolver::Session { .. })
            ),
            "a session ended on the incident"
        );
        let pairs = app.investigator_session_pairs_for_project(0);
        let card = proposal_card_with_reinvestigate(
            &proposed,
            proposal,
            pairs.can_reinvestigate(id, code, subject),
        );
        assert!(
            card.actions.contains(&ProposalAction::Reinvestigate),
            "the fresh proposal offers Re-investigate"
        );

        in2_cleanup(&mut app);
    }

    /// N2/S-13: opening an already-open path focuses the live session
    /// instead of duplicating it — even through a non-canonical spelling of
    /// the same file. Closing frees the guard, and focusing notes nothing.
    #[test]
    fn in2b_opening_an_already_open_path_focuses_it() {
        let temp = TempDirectory::new("in2b-one-session");
        let project_path = temp.path("edit.kinewright");
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&Document::default()).expect("the file serialises"),
        )
        .expect("the file writes");
        let (mut app, engine) = in2b_harness(Document::default(), None);
        app.open_project(&project_path);
        assert_eq!(app.projects.len(), 2, "the first open lands a session");
        let id = app.focused().id;

        // A non-canonical spelling of the same file (`.`/`..` segments —
        // portable, unlike a symlink).
        fs::create_dir(temp.path("sub")).expect("the alias segment exists");
        let alias = temp.root().join("sub").join("..").join("edit.kinewright");
        app.open_project(&alias);
        assert_eq!(app.projects.len(), 2, "the second open duplicates nothing");
        assert_eq!(app.focused().id, id, "the live session focuses");
        assert!(
            app.status.starts_with("Already open"),
            "the focus says so: {}",
            app.status
        );

        // Closing frees the guard: the path opens fresh again.
        app.close_project(id);
        assert_eq!(app.projects.len(), 1);
        app.open_project(&project_path);
        assert_eq!(app.projects.len(), 2, "a closed path reopens");

        app.route_incidents();
        for session in &app.projects {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.len(), 0, "focusing is silent");
        }
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H1: a recovery restore onto a closed path seeds `saved_digest`
    /// from the project file on disk — closing without saving pairs the
    /// flush, so the reopen finds its history instead of a mismatch.
    #[test]
    fn in2b_recovery_seeds_saved_digest_from_the_file_on_disk() {
        let temp = TempDirectory::new("in2b-h1-seed");
        let project_path = temp.path("edit.kinewright");
        let (mut app, engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 2, 41);
        app.write_project(&project_path).expect("the save succeeds");
        let disk_digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));

        // The crash story: a scratch second session, so the crash can close
        // the saved one — the restore below must land on a path no session
        // holds (not suspended), then Discard/close without saving.
        let scratch_path = temp.path("scratch.kinewright");
        fs::write(
            &scratch_path,
            serde_json::to_string_pretty(&Document::default()).expect("the file serialises"),
        )
        .expect("the scratch file writes");
        app.open_project(&scratch_path);
        assert_eq!(app.projects.len(), 2);
        let saved_id = app.projects[0].id;
        app.close_project(saved_id);
        assert_eq!(app.projects.len(), 1, "the crash closed the saved session");

        let mut recovered = Document::default();
        recovered.markers.push(Marker {
            id: MarkerId(7),
            position: TimeCode::ZERO,
            label: "the unsaved edit".to_owned(),
            color_token: 0,
        });
        let journal_path = temp.path("edit.journal");
        fs::write(
            &journal_path,
            journal_fixture_bytes(&project_path, PROJECT_FORMAT_VERSION, &Document::default()),
        )
        .expect("the journal writes");
        app.apply_restore_request(RestoreRequest {
            document: recovered,
            project_path: Some(project_path.clone()),
            journal_path: journal_path.clone(),
            writer_format_version: PROJECT_FORMAT_VERSION,
        });
        assert!(
            !app.focused().sidecar_suspended,
            "a restore onto a closed path is not suspended"
        );
        assert_eq!(
            app.focused().saved_digest,
            disk_digest,
            "the seed is the digest of the file on disk"
        );
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 2, "the saved history loads digest-less");
        }
        in2b_observe_opens(app.focused(), 1, 60);
        let focused = app.focused_project;
        app.projects[focused].stop_threads("the project was closed");
        let restored_id = app.projects[focused].id;
        app.close_project(restored_id);
        assert_eq!(app.projects.len(), 1, "the restored session closed");
        app.open_project(&project_path);
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 3, "the close flush paired: history back");
            assert!(
                log.all().all(|incident| {
                    incident.code != IncidentCode::Label(LabelIncident::SidecarRefused)
                }),
                "no mismatch refusal on reopen"
            );
        }
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H2: a suspended session cannot overwrite-save — it routes to Save
    /// As, the refused save writes neither file, and the route stays open.
    #[test]
    fn in2b_a_suspended_session_routes_overwrite_save_to_save_as() {
        let temp = TempDirectory::new("in2b-h2-suspended");
        let project_path = temp.path("edit.kinewright");
        let (mut app, engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 2, 41);
        app.write_project(&project_path).expect("the save succeeds");
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");

        // Restore onto the OPEN path: the restored session suspends.
        let journal_path = temp.path("edit.journal");
        fs::write(
            &journal_path,
            journal_fixture_bytes(&project_path, PROJECT_FORMAT_VERSION, &Document::default()),
        )
        .expect("the journal writes");
        let mut recovered = Document::default();
        recovered.markers.push(Marker {
            id: MarkerId(7),
            position: TimeCode::ZERO,
            label: "the unsaved edit".to_owned(),
            color_token: 0,
        });
        app.apply_restore_request(RestoreRequest {
            document: recovered,
            project_path: Some(project_path.clone()),
            journal_path: journal_path.clone(),
            writer_format_version: PROJECT_FORMAT_VERSION,
        });
        assert!(
            app.focused().sidecar_suspended,
            "a restore onto an open path suspends"
        );
        let before_project = fs::read(&project_path).expect("the project reads");
        let before_sidecar = fs::read(&sidecar).expect("the sidecar reads");
        assert!(
            matches!(
                app.write_project(&project_path),
                Err(ProjectSaveError::SaveAsRequired { .. })
            ),
            "a suspended overwrite-save routes to Save As"
        );
        assert_eq!(
            fs::read(&project_path).expect("the project re-reads"),
            before_project,
            "the refused save writes no project bytes"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the sidecar re-reads"),
            before_sidecar,
            "and no sidecar bytes"
        );
        let copy_path = temp.path("copy.kinewright");
        app.write_project(&copy_path).expect("Save As succeeds");
        assert!(
            !app.focused().sidecar_suspended,
            "Save As lifts the suspension"
        );
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H2: the original session cannot overwrite while a recovery
    /// duplicate holds its path — both sides route to Save As.
    #[test]
    fn in2b_a_session_open_elsewhere_routes_overwrite_save_to_save_as() {
        let temp = TempDirectory::new("in2b-h2-duplicate");
        let project_path = temp.path("edit.kinewright");
        let (mut app, engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 2, 41);
        app.write_project(&project_path).expect("the save succeeds");
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");

        let journal_path = temp.path("edit.journal");
        fs::write(
            &journal_path,
            journal_fixture_bytes(&project_path, PROJECT_FORMAT_VERSION, &Document::default()),
        )
        .expect("the journal writes");
        app.apply_restore_request(RestoreRequest {
            document: Document::default(),
            project_path: Some(project_path.clone()),
            journal_path: journal_path.clone(),
            writer_format_version: PROJECT_FORMAT_VERSION,
        });
        assert_eq!(app.projects.len(), 2, "the duplicate lands");
        app.focus_project(0);
        let before_project = fs::read(&project_path).expect("the project reads");
        let before_sidecar = fs::read(&sidecar).expect("the sidecar reads");
        assert!(
            matches!(
                app.write_project(&project_path),
                Err(ProjectSaveError::SaveAsRequired { .. })
            ),
            "an overwrite while duplicated routes to Save As"
        );
        assert_eq!(
            fs::read(&project_path).expect("the project re-reads"),
            before_project,
            "the refused save writes no project bytes"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the sidecar re-reads"),
            before_sidecar,
            "and no sidecar bytes"
        );
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H5: Save As onto a path open in another session is refused — the
    /// target's project and sidecar bytes are untouched.
    #[test]
    fn in2b_save_as_onto_an_open_path_is_refused() {
        let temp = TempDirectory::new("in2b-h5-open-target");
        let first_path = temp.path("first.kinewright");
        let (mut app, engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 1, 41);
        app.write_project(&first_path)
            .expect("the first save succeeds");
        let second_path = temp.path("second.kinewright");
        fs::write(
            &second_path,
            serde_json::to_string_pretty(&Document::default()).expect("the file serialises"),
        )
        .expect("the second file writes");
        app.open_project(&second_path);
        in2b_observe_opens(app.focused(), 1, 51);
        app.write_project(&second_path)
            .expect("the second save succeeds");
        let second_sidecar = sidecar_path_for_project(Some(&second_path)).expect("derived");
        let before_project = fs::read(&second_path).expect("the target reads");
        let before_sidecar = fs::read(&second_sidecar).expect("its sidecar reads");
        app.focus_project(0);
        assert!(
            matches!(
                app.write_project(&second_path),
                Err(ProjectSaveError::PathOpenElsewhere { .. })
            ),
            "Save As onto an open path is refused"
        );
        assert_eq!(
            fs::read(&second_path).expect("the target re-reads"),
            before_project,
            "the target project bytes survive"
        );
        assert_eq!(
            fs::read(&second_sidecar).expect("its sidecar re-reads"),
            before_sidecar,
            "and its sidecar"
        );
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H7: Investigate is not offered when the subject is missing — the
    /// press refuses without queueing, even with a working harness.
    #[test]
    fn in2b_investigate_refuses_a_missing_subject() {
        use super::in1_tests::{in2_cleanup, in2_configure_scripted_at, in2_default_budgets};
        use kinewright_agent::ScriptedDriver;
        use kinewright_core::{
            AssetId, IncidentEvidence, IncidentRecord, IncidentState, IncidentTelemetry,
        };

        use crate::sidecar::{digest_bytes, sidecar_path_for_project};

        let record = IncidentRecord {
            id: IncidentId(1),
            code: IncidentCode::Label(LabelIncident::Look).code().to_owned(),
            subject: IncidentSubject::Asset(AssetId(999)),
            observed: "loaded, subject gone".to_owned(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(41),
            opened_wall_millis: None,
            opened_offset_nanos: 1_000,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: None,
        };
        let dir = TempDirectory::new("in2b-h7-missing");
        let project_path = dir.path("edit.kinewright");
        fs::write(&project_path, b"{\"timeline\":{}}").expect("the project file writes");
        let digest = digest_bytes(&fs::read(&project_path).expect("the project reads"));
        let envelope = serde_json::json!({
            "format_version": 1,
            "project_digest": digest,
            "previous_digest": digest,
            "records": [serde_json::to_value(record).expect("the record")],
        });
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("a sidecar derives");
        fs::write(
            &sidecar,
            serde_json::to_string(&envelope).expect("the envelope"),
        )
        .expect("the sidecar writes");

        let (mut app, engine) = in2b_harness(Document::default(), Some(project_path));
        assert!(
            app.projects[0].subject_missing.contains(&IncidentId(1)),
            "asset 999 resolves to nothing"
        );
        assert!(
            app.projects[0].loaded_open_ids.contains(&IncidentId(1)),
            "the row loads open"
        );
        in2_configure_scripted_at(
            &mut app,
            0,
            ScriptedDriver::new(vec![]),
            in2_default_budgets(),
        );
        assert!(
            !app.investigate(0, IncidentId(1)),
            "the press refuses a missing subject"
        );
        assert_eq!(
            app.projects[0]
                .investigator
                .as_ref()
                .expect("investigator state")
                .queued_count(),
            0,
            "nothing queues"
        );
        assert!(
            app.projects[0].loaded_open_ids.contains(&IncidentId(1)),
            "the refusal consumes nothing"
        );
        in2_cleanup(&mut app);
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H8: the startup missing-media aggregate is transient, like
    /// File → Open — a per-run note must never persist `Open` against a
    /// later open. Driven through the production startup path.
    #[test]
    fn in2b_startup_missing_media_aggregate_is_transient() {
        use kinewright_core::{
            AssetId, ColorDescription, MediaAsset, MediaKind, MediaSourceFingerprint, Rational,
            TimeCode,
        };

        let temp = TempDirectory::new("in2b-h8-startup");
        let project_path = temp.path("edit.kinewright");
        let mut document = Document::default();
        document.media_pool.push(MediaAsset {
            id: AssetId(1),
            path: temp.path("nowhere.mov"),
            name: "gone.mov".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).expect("30 fps"),
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        });
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&ProjectFile {
                format_version: PROJECT_FORMAT_VERSION,
                document,
            })
            .expect("the project serialises"),
        )
        .expect("the project file writes");
        let engine = Arc::new(FfmpegMediaEngine::new().expect("the test engine starts"));
        let mut app = KinewrightApp::new(engine.clone(), Some(project_path));
        app.route_incidents();
        {
            let log = app
                .focused()
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let aggregate = log
                .all()
                .find(|incident| {
                    incident.code == IncidentCode::Label(LabelIncident::MediaIncomplete)
                })
                .expect("the startup aggregate notes");
            assert!(
                aggregate.transient,
                "the startup aggregate is transient, like File → Open"
            );
        }
        in2b_quiesce_engine(&engine);
        in2b_shutdown(&mut app);
    }

    /// N6/H12: a project write that fails after the sidecar write rolls the
    /// sidecar back — the previous bytes are restored, and neither half
    /// litters a temp.
    #[test]
    fn in2b_failed_project_write_restores_the_prior_sidecar() {
        let temp = TempDirectory::new("in2b-h12-restore");
        let project_path = temp.path("edit.kinewright");
        let (mut app, _engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 1, 41);
        app.write_project(&project_path).expect("the save succeeds");
        let sidecar = sidecar_path_for_project(Some(&project_path)).expect("derived");
        let before_sidecar = fs::read(&sidecar).expect("the sidecar reads");
        let saved_digest = app.projects[0].saved_digest.clone();
        // One more note, so the save below rewrites the sidecar; then a
        // directory takes the project's place, failing temp + rename on
        // both lanes (a rename onto a directory never succeeds).
        in2b_observe_opens(app.focused(), 1, 51);
        let stash_path = temp.path("edit.kinewright.stashed");
        fs::rename(&project_path, &stash_path).expect("the project file moves aside");
        fs::create_dir(&project_path).expect("a directory takes its place");
        assert!(
            matches!(
                app.write_project(&project_path),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the sidecar re-reads"),
            before_sidecar,
            "the sidecar rolls back to its prior bytes"
        );
        assert_eq!(
            app.projects[0].saved_digest, saved_digest,
            "the session keeps its old digest"
        );
        assert_eq!(
            app.projects[0].project_path.as_deref(),
            Some(project_path.as_path()),
            "and its path"
        );
        for entry in fs::read_dir(temp.root()).expect("the dir reads") {
            let entry = entry.expect("a readable entry");
            let litter = entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"));
            assert!(
                !litter,
                "no temp litter: {}",
                entry.file_name().to_string_lossy()
            );
        }
        fs::remove_dir(&project_path).expect("cleanup");
        in2b_shutdown(&mut app);
    }

    /// N6/H12: when no sidecar bytes preceded the failed save, the rollback
    /// removes the flushed sidecar instead of restoring.
    #[test]
    fn in2b_failed_save_as_removes_the_unpaired_sidecar() {
        let temp = TempDirectory::new("in2b-h12-remove");
        let (mut app, _engine) = in2b_harness(Document::default(), None);
        in2b_observe_opens(&app.projects[0], 1, 41);
        let dir_path = temp.path("target.kinewright");
        fs::create_dir(&dir_path).expect("a directory takes the target");
        let sidecar = sidecar_path_for_project(Some(&dir_path)).expect("derived");
        assert!(!sidecar.exists(), "no sidecar precedes the save");
        assert!(
            matches!(
                app.write_project(&dir_path),
                Err(ProjectSaveError::Write(_))
            ),
            "the project write fails"
        );
        assert!(
            !sidecar.exists(),
            "the rollback removes the flushed sidecar"
        );
        fs::remove_dir(&dir_path).expect("cleanup");
        in2b_shutdown(&mut app);
    }
}
