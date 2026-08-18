use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
    future::Future,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use image::{ColorType, ImageEncoder as _, codecs::png::PngEncoder};
use kinewright_core::{
    Analysis, AnalysisKind, AssetId, AssetSilences, AssetTranscript, AudioBus, AudioBusId,
    AudioLoudness, AutomationCurve, BeatMontageCadenceContract, BeatMontageSelect, BeatStatus,
    CaptionCue, CaptionMotion, CaptionPreset, Clip, ClipContent, ClipId, Command, Core,
    DeliveryAspect, DeliveryProfile, DeliveryVariant, Document, Effect, EffectId, Event, Export,
    ExportCancellation, Keyframe, KeyframeInterpolation, MUSIC_STRUCTURE_DEFAULT_METER_BEATS,
    MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS, Marker, MarkerId, MediaAsset, MediaKind, Operation,
    ParamValue, Playback, Query, QueryResult, ReframeFocusBounds, SceneStatus, SilenceStatus,
    SpeakerAngleAssignment, SpeakerMulticamSettings, SubjectCenterBasisPointSample,
    SubjectFocusBasisPointConstraint, SubjectReframeSettings, SyncGroupId, ThreePointMode,
    TimeCode, TimelineBeat, TimelineBeatAnalysisState, TimelineRevision, TimelineSceneChange,
    TimelineSilenceSpan, TimelineTranscriptWord, TitlePosition, Track, TrackId, TrackKind,
    TranscriptStatus, animated_caption_operations_at, apply_batch, authored_caption_cues,
    beat_montage_plan, beat_montage_plan_near_anchors_with_report, beat_montage_plan_with_anchors,
    beat_pacing_plan, caption_cues, dedup_timeline_words, delivery_conformance,
    document_for_delivery_profile, document_for_delivery_variant, is_filler_word,
    map_source_range_to_project, music_fit_plan_with_end_anchor, music_structure_analysis,
    plan_speaker_multicam, plan_subject_reframe_basis_points_with_containment, qa_document,
    validate_beat_montage_plan_cadence,
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::oneshot;

use crate::{
    export_queue::{ExportJobId, ExportQueue, ExportQueueError, QueueExportRequest},
    pacing::{DialoguePacingGap, dialogue_pacing_gaps},
    render::{
        cuttable_timeline_silences, render_asset_scene_changes, render_asset_silences,
        render_asset_transcript, render_clip_info, render_timeline_scene_changes,
        render_timeline_silences, render_timeline_state, render_timeline_transcript,
    },
    runtime::{
        CapabilityDescriptor, CapabilityKind, PreparedEditPlan, PreparedPlanId, PreparedPlanStore,
        ToolSurfaceMetrics, capabilities, is_invocable_capability, search_capabilities,
    },
    schema::{SchemaError, decode_operation, operation_tool_name, operation_tools, schema_object},
};

const THUMBNAIL_MAX_WIDTH: u32 = 512;
const STORYBOARD_DEFAULT_FRAMES: u8 = 9;
const STORYBOARD_MAX_FRAMES: u8 = 16;
const STORYBOARD_DEFAULT_CELL_WIDTH: u32 = 320;
const SHOT_BOARD_DEFAULT_CANDIDATES: u8 = 6;
const SHOT_BOARD_MAX_CANDIDATES: u8 = 12;
const SHOT_BOARD_EVIDENCE_PER_CANDIDATE: u8 = 3;
const STORYBOARD_COLUMNS: u32 = 4;
const STORYBOARD_GUTTER: u32 = 4;
const DEFAULT_CONFIRMATION_TIMEOUT: Duration = Duration::from_mins(1);
const DEFAULT_MINIMUM_SILENCE_FRAMES: i64 = 6;
const DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;
const DEFAULT_BEAT_STRENGTH_BASIS_POINTS: u16 = 1_000;
const DEFAULT_TRACKING_STEP_FRAMES: i64 = 5;
const DEFAULT_TRACKING_SEARCH_RADIUS_PERCENT: u8 = 10;
const DEFAULT_TRACKING_WIDTH: u32 = 256;
const DEFAULT_REFRAME_DEAD_ZONE_PERCENT: u8 = 6;
const DEFAULT_REFRAME_MAXIMUM_STEP_PERCENT: u8 = 2;
const MAX_TRACKING_SAMPLES: usize = 120;
/// Compact marker-label sidecar for deterministic subject-tracking evidence.
///
/// Effects intentionally accept only registered render parameters, so this
/// marker is the smallest document-native place to retain non-rendering
/// tracker evidence through delivery materialization without exposing one
/// operation per observation to the editing model.
pub(crate) const REFRAME_SUBJECT_PROVENANCE_PREFIX: &str = "__kinewright_reframe_subject_v1:";
const REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES: usize = 18;
const REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES: usize = 16;
/// AAC and other lossy encoders can overshoot a decoded sample ceiling. Keep
/// deterministic pre-encode headroom while evaluating the public ceiling on
/// the actual decoded delivery artifact.
const LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS: i32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationRequest {
    pub id: u64,
    pub tool_name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfirmationDecision {
    Approved,
    Rejected(String),
}

#[derive(Clone)]
pub struct ConfirmationBroker {
    requests_tx: crossbeam_channel::Sender<ConfirmationRequest>,
    requests_rx: crossbeam_channel::Receiver<ConfirmationRequest>,
    pending: Arc<Mutex<HashMap<u64, crossbeam_channel::Sender<ConfirmationDecision>>>>,
    next_id: Arc<AtomicU64>,
    timeout: Duration,
}

impl Default for ConfirmationBroker {
    fn default() -> Self {
        Self::with_timeout(DEFAULT_CONFIRMATION_TIMEOUT)
    }
}

impl ConfirmationBroker {
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let (requests_tx, requests_rx) = crossbeam_channel::unbounded();
        Self {
            requests_tx,
            requests_rx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            timeout,
        }
    }

    #[must_use]
    pub fn pending_requests(&self) -> Vec<ConfirmationRequest> {
        self.requests_rx
            .try_iter()
            .filter(|request| self.is_pending(request.id))
            .collect()
    }

    #[must_use]
    pub fn is_pending(&self, id: u64) -> bool {
        self.pending
            .lock()
            .is_ok_and(|pending| pending.contains_key(&id))
    }

    #[must_use]
    pub fn approve(&self, id: u64) -> bool {
        self.resolve(id, ConfirmationDecision::Approved)
    }

    pub fn reject(&self, id: u64, reason: impl Into<String>) -> bool {
        self.resolve(id, ConfirmationDecision::Rejected(reason.into()))
    }

    pub fn reject_all(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let pending = self
            .pending
            .lock()
            .map(|mut pending| {
                pending
                    .drain()
                    .map(|(_, sender)| sender)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for sender in pending {
            let _ = sender.send(ConfirmationDecision::Rejected(reason.clone()));
        }
    }

    fn resolve(&self, id: u64, decision: ConfirmationDecision) -> bool {
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id));
        sender.is_some_and(|sender| sender.send(decision).is_ok())
    }

    fn confirm(&self, tool_name: &str, description: String) -> Result<(), String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (decision_tx, decision_rx) = crossbeam_channel::bounded(1);
        self.pending
            .lock()
            .map_err(|_| "confirmation broker stopped".to_owned())?
            .insert(id, decision_tx);
        if self
            .requests_tx
            .send(ConfirmationRequest {
                id,
                tool_name: tool_name.to_owned(),
                description,
            })
            .is_err()
        {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err("confirmation broker stopped".to_owned());
        }
        let decision = decision_rx.recv_timeout(self.timeout);
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
        match decision {
            Ok(ConfirmationDecision::Approved) => Ok(()),
            Ok(ConfirmationDecision::Rejected(reason)) => Err(reason),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Err("confirmation timed out".to_owned())
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err("confirmation was interrupted".to_owned())
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum McpServerError {
    #[error("could not bind the Kinewright MCP server: {0}")]
    Bind(#[source] std::io::Error),
    #[error("could not configure the Kinewright MCP listener: {0}")]
    Listener(#[source] std::io::Error),
    #[error("could not start the Kinewright MCP server thread: {0}")]
    Thread(#[source] std::io::Error),
    #[error("could not start the Kinewright export queue: {0}")]
    ExportQueue(#[from] ExportQueueError),
    #[error("could not build the Kinewright tool surface: {0}")]
    Schema(#[from] SchemaError),
}

pub struct McpServer {
    endpoint: String,
    confirmations: ConfirmationBroker,
    tool_surface_metrics: ToolSurfaceMetrics,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl McpServer {
    /// Start the loopback MCP server for the live core and media engine.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener or server thread cannot start.
    pub fn start(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
    ) -> Result<Self, McpServerError> {
        Self::start_with_broker(core, playback, analysis, ConfirmationBroker::default())
    }

    /// Start the live MCP server with agent-accessible delivery exports.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener, export worker, or server thread cannot start.
    pub fn start_with_exporter(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Arc<dyn Export>,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            Some(exporter),
            ConfirmationBroker::default(),
            true,
        )
    }

    /// Start a branch-scoped MCP server whose edits and proof renders never
    /// replace the live playback document.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener or server thread cannot start.
    pub fn start_isolated(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            None,
            ConfirmationBroker::default(),
            false,
        )
    }

    /// Start a branch-scoped MCP server with serial exports of immutable branch snapshots.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener, export worker, or server thread cannot start.
    pub fn start_isolated_with_exporter(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Arc<dyn Export>,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            Some(exporter),
            ConfirmationBroker::default(),
            false,
        )
    }

    fn start_with_broker(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(core, playback, analysis, None, confirmations, true)
    }

    fn start_configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Option<Arc<dyn Export>>,
        confirmations: ConfirmationBroker,
        publish_to_playback: bool,
    ) -> Result<Self, McpServerError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(McpServerError::Bind)?;
        let address = listener.local_addr().map_err(McpServerError::Listener)?;
        listener
            .set_nonblocking(true)
            .map_err(McpServerError::Listener)?;
        let endpoint = format!("http://{address}/mcp");
        let (shutdown, shutdown_rx) = oneshot::channel();
        let export_queue = exporter.map(ExportQueue::new).transpose()?;
        let tool_surface_metrics = ToolSurfaceMetrics::measure(&KinewrightMcp::served_tools()?);
        let handler = KinewrightMcp::configured(
            core,
            playback,
            analysis,
            export_queue,
            confirmations.clone(),
            publish_to_playback,
        );
        let server_thread = thread::Builder::new()
            .name("kinewright-mcp".to_owned())
            .spawn(move || run_server(listener, handler, shutdown_rx))
            .map_err(McpServerError::Thread)?;
        Ok(Self {
            endpoint,
            confirmations,
            tool_surface_metrics,
            shutdown: Some(shutdown),
            thread: Some(server_thread),
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub fn confirmations(&self) -> ConfirmationBroker {
        self.confirmations.clone()
    }

    #[must_use]
    pub const fn tool_surface_metrics(&self) -> ToolSurfaceMetrics {
        self.tool_surface_metrics
    }

    pub fn shutdown(mut self) {
        self.signal_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn signal_shutdown(&mut self) {
        self.confirmations
            .reject_all("the Kinewright agent session was interrupted");
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.signal_shutdown();
    }
}

fn run_server(listener: TcpListener, handler: KinewrightMcp, shutdown: oneshot::Receiver<()>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("kinewright-mcp-worker")
        .build()
        .expect("Kinewright MCP Tokio runtime must start");
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .expect("validated MCP listener must enter Tokio");
        let service: StreamableHttpService<KinewrightMcp, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(handler.clone()),
                Arc::default(),
                StreamableHttpServerConfig::default(),
            );
        let router = axum::Router::new().nest_service("/mcp", service);
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown.await;
            })
            .await;
    });
}

#[derive(Clone)]
struct KinewrightMcp {
    core: Core,
    playback: Arc<dyn Playback>,
    analysis: Arc<dyn Analysis>,
    export_queue: Option<ExportQueue>,
    confirmations: ConfirmationBroker,
    publish_to_playback: bool,
    prepared_plans: Arc<Mutex<PreparedPlanStore>>,
}

impl KinewrightMcp {
    #[cfg(test)]
    fn new(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Self {
        Self::configured(core, playback, analysis, None, confirmations, true)
    }

    fn configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        export_queue: Option<ExportQueue>,
        confirmations: ConfirmationBroker,
        publish_to_playback: bool,
    ) -> Self {
        Self {
            core,
            playback,
            analysis,
            export_queue,
            confirmations,
            publish_to_playback,
            prepared_plans: Arc::new(Mutex::new(PreparedPlanStore::default())),
        }
    }

    fn capability_tools() -> Result<Vec<Tool>, SchemaError> {
        let mut tools = operation_tools()?
            .into_iter()
            .map(|definition| definition.tool)
            .collect::<Vec<_>>();
        tools.extend(inspector_tools());
        Ok(tools)
    }

    fn served_tools() -> Result<Vec<Tool>, SchemaError> {
        Ok(Self::capability_tools()?
            .into_iter()
            .filter(|tool| crate::runtime::COMPACT_TOOL_NAMES.contains(&tool.name.as_ref()))
            .collect())
    }

    #[cfg(test)]
    fn tools() -> Result<Vec<Tool>, SchemaError> {
        Self::capability_tools()
    }

    fn call_exposed_blocking(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, McpError> {
        if !crate::runtime::COMPACT_TOOL_NAMES.contains(&request.name.as_ref()) {
            return Ok(error_text(format!(
                "{} is an internal capability, not an MCP tool; use search_capabilities, get_capability, and invoke_capability or prepare_edit_plan",
                request.name
            )));
        }
        self.call_blocking(request)
    }

    #[allow(clippy::too_many_lines)]
    fn call_blocking(&self, request: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "search_capabilities" => {
                let args: CapabilitySearchArgs = decode_args("search_capabilities", arguments)?;
                let tools = Self::capability_tools()
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                let found = search_capability_queries(&tools, &args);
                Ok(success_structured(
                    format!("found {} matching Kinewright capabilities", found.len()),
                    serde_json::json!({
                        "capabilities": found,
                        "next": "Call get_capability once with the exact names needed before invoking them or using edit operations in prepare_edit_plan."
                    }),
                ))
            }
            "get_capability" => {
                let args: CapabilityArgs = decode_args("get_capability", arguments)?;
                let tools = Self::capability_tools()
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                Ok(open_capabilities(&tools, args))
            }
            "invoke_capability" => {
                let args: InvokeCapabilityArgs = decode_args("invoke_capability", arguments)?;
                if !is_invocable_capability(&args.name) {
                    return Ok(error_text(format!(
                        "capability {} cannot be invoked through the compact dispatcher; edit operations must be prepared and committed atomically",
                        args.name
                    )));
                }
                let serde_json::Value::Object(arguments) = args.arguments else {
                    return Ok(error_text("capability arguments must be a JSON object"));
                };
                self.call_blocking(CallToolRequestParams::new(args.name).with_arguments(arguments))
            }
            "prepare_edit_plan" => {
                let args: PrepareEditPlanArgs = decode_args("prepare_edit_plan", arguments)?;
                let (actual_revision, document) = self.snapshot()?;
                let plan = self
                    .prepared_plans
                    .lock()
                    .map_err(|_| McpError::internal_error("prepared plan store stopped", None))?
                    .prepare(
                        args.expected_revision,
                        actual_revision,
                        &document,
                        args.operations,
                    );
                Ok(match plan {
                    Ok(plan) => success_structured(
                        format!(
                            "prepared edit plan {}; review its preview, then commit it at timeline revision {}",
                            plan.id, plan.expected_revision
                        ),
                        serde_json::json!({
                            "plan_id": plan.id,
                            "preview": plan.preview,
                        }),
                    ),
                    Err(error) => error_text(error.to_string()),
                })
            }
            "commit_edit_plan" => {
                let args: CommitEditPlanArgs = decode_args("commit_edit_plan", arguments)?;
                let plan = {
                    let mut plans = self.prepared_plans.lock().map_err(|_| {
                        McpError::internal_error("prepared plan store stopped", None)
                    })?;
                    let Some(plan) = plans.get(args.plan_id) else {
                        return Ok(error_text(format!(
                            "prepared edit plan {} is missing or expired; prepare it again against the current timeline revision",
                            args.plan_id
                        )));
                    };
                    if args.expected_revision != plan.expected_revision {
                        return Ok(error_text(format!(
                            "prepared edit plan {} belongs to timeline revision {}, not {}",
                            args.plan_id, plan.expected_revision, args.expected_revision
                        )));
                    }
                    plans
                        .take(args.plan_id)
                        .expect("prepared plan was just read")
                };
                self.apply_edit_plan(args.expected_revision, &plan.operations)
            }
            "discard_edit_plan" => {
                let args: DiscardEditPlanArgs = decode_args("discard_edit_plan", arguments)?;
                let discarded = self
                    .prepared_plans
                    .lock()
                    .map_err(|_| McpError::internal_error("prepared plan store stopped", None))?
                    .discard(args.plan_id);
                Ok(if discarded {
                    success_text(format!("discarded prepared edit plan {}", args.plan_id))
                } else {
                    error_text(format!(
                        "prepared edit plan {} is missing or expired",
                        args.plan_id
                    ))
                })
            }
            "get_timeline_state" => {
                let (revision, document) = self.snapshot()?;
                Ok(success_text(format!(
                    "timeline_revision={revision}\n{}",
                    render_timeline_state(&document)
                )))
            }
            "get_clip_info" => {
                let args: ClipInfoArgs = decode_args("get_clip_info", arguments)?;
                let document = self.document()?;
                Ok(match render_clip_info(&document, args.clip_id) {
                    Ok(rendered) => success_text(rendered),
                    Err(error) => error_text(error),
                })
            }
            "get_source_info" => {
                let args: SourceInfoArgs = decode_args("get_source_info", arguments)?;
                self.source_info(&args)
            }
            "get_source_storyboard" => {
                let args: SourceStoryboardArgs = decode_args("get_source_storyboard", arguments)?;
                self.source_storyboard(&args)
            }
            "get_source_shot_board" => {
                let args: SourceShotBoardArgs = decode_args("get_source_shot_board", arguments)?;
                self.source_shot_board(&args)
            }
            "search_media" => {
                let args: MediaSearchArgs = decode_args("search_media", arguments)?;
                self.search_media(&args)
            }
            "get_frame_at" => {
                let args: FrameAtArgs = decode_args("get_frame_at", arguments)?;
                self.frame_at(args.timecode)
            }
            "get_video_scopes" => {
                let args: VideoScopesArgs = decode_args("get_video_scopes", arguments)?;
                self.video_scopes(&args)
            }
            "track_mask_region" => {
                let args: TrackMaskArgs = decode_args("track_mask_region", arguments)?;
                self.track_mask_region(&args)
            }
            "track_reframe_subject" => {
                let args: TrackReframeArgs = decode_args("track_reframe_subject", arguments)?;
                self.track_reframe_subject(&args)
            }
            "get_timeline_storyboard" => {
                let args: StoryboardArgs = decode_args("get_timeline_storyboard", arguments)?;
                self.timeline_storyboard(args)
            }
            "get_transcript" => {
                let args: TranscriptArgs = decode_args("get_transcript", arguments)?;
                self.asset_transcript(args.asset_id)
            }
            "get_transcripts" => {
                let args: TranscriptsArgs = decode_args("get_transcripts", arguments)?;
                self.asset_transcripts(&args.asset_ids)
            }
            "get_timeline_transcript" => {
                let args: TimelineTranscriptArgs =
                    decode_args("get_timeline_transcript", arguments)?;
                self.timeline_transcript(args.range)
            }
            "get_dialogue_pacing" => {
                let args: DialoguePacingArgs = decode_args("get_dialogue_pacing", arguments)?;
                self.dialogue_pacing(&args)
            }
            "get_silences" => {
                let args: SilencesArgs = decode_args("get_silences", arguments)?;
                self.asset_silences(args.asset_id, args.min_duration_frames)
            }
            "get_timeline_silences" => {
                let args: TimelineSilencesArgs = decode_args("get_timeline_silences", arguments)?;
                self.timeline_silences(args.range, args.min_duration_frames)
            }
            "get_scene_changes" => {
                let args: SceneChangesArgs = decode_args("get_scene_changes", arguments)?;
                self.asset_scene_changes(args.asset_id, args.min_confidence)
            }
            "get_timeline_scene_changes" => {
                let args: TimelineDerivedArgs =
                    decode_args("get_timeline_scene_changes", arguments)?;
                self.timeline_scene_changes(args.range)
            }
            "get_beats" => {
                let args: BeatsArgs = decode_args("get_beats", arguments)?;
                self.asset_beats(args.asset_id, args.min_strength)
            }
            "get_timeline_beats" => {
                let args: TimelineBeatsArgs = decode_args("get_timeline_beats", arguments)?;
                self.timeline_beats(args.range, args.min_strength)
            }
            "get_music_structure" => {
                let args: MusicStructureArgs = decode_args("get_music_structure", arguments)?;
                self.music_structure(&args)
            }
            "plan_dialogue_assembly" => {
                let args: DialogueAssemblyPlanArgs =
                    decode_args("plan_dialogue_assembly", arguments)?;
                self.plan_dialogue_assembly(&args)
            }
            "plan_beat_pacing" => {
                let args: BeatPacingPlanArgs = decode_args("plan_beat_pacing", arguments)?;
                self.plan_beat_pacing(args)
            }
            "plan_beat_montage" => {
                let args: BeatMontagePlanArgs = decode_args("plan_beat_montage", arguments)?;
                self.plan_beat_montage(&args)
            }
            "plan_music_fit" => {
                let args: MusicFitPlanArgs = decode_args("plan_music_fit", arguments)?;
                self.plan_music_fit(&args)
            }
            "plan_speaker_multicam" => {
                let args: SpeakerMulticamPlanArgs =
                    decode_args("plan_speaker_multicam", arguments)?;
                self.plan_speaker_multicam(args)
            }
            "plan_audio_normalization" => {
                let args: AudioNormalizationPlanArgs =
                    decode_args("plan_audio_normalization", arguments)?;
                self.plan_audio_normalization(&args)
            }
            "get_analysis_status" => {
                let args: AnalysisStatusArgs = decode_args("get_analysis_status", arguments)?;
                self.analysis_status(args.asset_id)
            }
            "get_caption_presets" => Ok(Self::caption_presets()),
            "get_captions" => {
                let args: CaptionListArgs = decode_args("get_captions", arguments)?;
                self.captions(args)
            }
            "plan_caption_corrections" => {
                let args: CaptionCorrectionPlanArgs =
                    decode_args("plan_caption_corrections", arguments)?;
                self.plan_caption_corrections(args)
            }
            "add_styled_captions" => {
                let args: StyledCaptionsArgs = decode_args("add_styled_captions", arguments)?;
                self.add_styled_captions(&args)
            }
            "get_qa_report" => Ok(self.qa_report()?),
            "get_delivery_variants" => Ok(Self::delivery_variants()),
            "get_delivery_profiles" => Ok(self.delivery_profiles()?),
            "get_delivery_conformance" => {
                let args: DeliveryConformanceArgs =
                    decode_args("get_delivery_conformance", arguments)?;
                self.delivery_conformance(&args)
            }
            "queue_export" => {
                let args: QueueExportArgs = decode_args("queue_export", arguments)?;
                self.queue_export(args)
            }
            "get_export_jobs" => Ok(self.export_jobs()),
            "cancel_export" => {
                let args: ExportJobArgs = decode_args("cancel_export", arguments)?;
                Ok(self.cancel_export(args.job_id))
            }
            "get_delivery_variant_storyboard" => {
                let args: DeliveryStoryboardArgs =
                    decode_args("get_delivery_variant_storyboard", arguments)?;
                self.delivery_variant_storyboard(args)
            }
            "get_editorial_readiness" => {
                let args: EditorialReadinessArgs =
                    decode_args("get_editorial_readiness", arguments)?;
                self.editorial_readiness(&args)
            }
            "request_analysis" => {
                let args: RequestAnalysisArgs = decode_args("request_analysis", arguments)?;
                self.request_analysis(args.asset_id, &args.kinds)
            }
            "cancel_analysis" => {
                let args: CancelAnalysisArgs = decode_args("cancel_analysis", arguments)?;
                self.cancel_analysis(args.asset_id, args.kind)
            }
            "apply_edit_plan" => {
                let args: EditPlanArgs = decode_args("apply_edit_plan", arguments)?;
                let operations = args
                    .operations
                    .into_iter()
                    .map(|operation| operation.0)
                    .collect::<Vec<_>>();
                self.apply_edit_plan(args.expected_revision, &operations)
            }
            "import_media" => {
                let args: ImportMediaArgs = decode_args("import_media", arguments)?;
                Ok(self.import_media(args.expected_revision, &args.path))
            }
            tool_name => {
                let revisioned = decode_operation(tool_name, arguments)
                    .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
                let (actual_revision, document) = self.snapshot()?;
                if revisioned.expected_revision != actual_revision {
                    return Ok(revision_conflict_text(
                        revisioned.expected_revision,
                        actual_revision,
                    ));
                }
                if let Some(description) =
                    Self::confirmation_description(&document, &revisioned.operation)
                    && let Err(reason) = self.confirmations.confirm(tool_name, description)
                {
                    return Ok(error_text(format!(
                        "refused destructive tool {tool_name}: {reason}"
                    )));
                }
                Ok(self.apply_operation(
                    tool_name,
                    revisioned.expected_revision,
                    revisioned.operation,
                ))
            }
        }
    }

    fn confirmation_description(document: &Document, operation: &Operation) -> Option<String> {
        match operation {
            Operation::DeleteClip { clip } | Operation::RippleDeleteClip { clip } => Some(format!(
                "The agent wants to delete clip {clip}. This edit can be undone."
            )),
            Operation::RemoveTrack { track } => {
                let track = document
                    .tracks
                    .iter()
                    .find(|candidate| candidate.id == *track)?;
                if track.clips.is_empty() {
                    None
                } else {
                    Some(format!(
                        "The agent wants to remove track {} and its {} clip(s). This edit can be undone.",
                        track.id,
                        track.clips.len()
                    ))
                }
            }
            _ => None,
        }
    }

    fn document(&self) -> Result<Arc<Document>, McpError> {
        match self
            .core
            .request(Command::Query(Query::Document))
            .map_err(|error| McpError::internal_error(error.to_string(), None))?
        {
            Event::QueryResult(QueryResult::Document(document)) => Ok(document),
            _ => Err(McpError::internal_error(
                "Core returned the wrong query result",
                None,
            )),
        }
    }

    fn snapshot(&self) -> Result<(TimelineRevision, Arc<Document>), McpError> {
        match self
            .core
            .request(Command::Query(Query::Snapshot))
            .map_err(|error| McpError::internal_error(error.to_string(), None))?
        {
            Event::QueryResult(QueryResult::Snapshot { revision, document }) => {
                Ok((revision, document))
            }
            _ => Err(McpError::internal_error(
                "Core returned the wrong snapshot query result",
                None,
            )),
        }
    }

    fn apply_operation(
        &self,
        tool_name: &str,
        expected_revision: TimelineRevision,
        operation: Operation,
    ) -> CallToolResult {
        let imported_asset = match &operation {
            Operation::AddAsset { asset } => Some(asset.clone()),
            _ => None,
        };
        let before = self.snapshot().ok();
        match self.core.request(Command::DoIfRevision {
            expected: expected_revision,
            operation,
        }) {
            Ok(Event::DocumentChanged { doc, revision, .. }) => {
                if self.publish_to_playback {
                    self.playback.set_document(Arc::clone(&doc));
                }
                if let Some(asset) = imported_asset {
                    self.request_asset_analysis(asset);
                }
                success_text(state_delta(
                    tool_name,
                    before.as_ref().map(|(_, document)| document.as_ref()),
                    &doc,
                    revision,
                ))
            }
            Ok(Event::OpRejected { error, .. }) => error_text(error.to_string()),
            Ok(Event::BatchRejected { error, .. }) => error_text(error.to_string()),
            Ok(Event::RevisionConflict { expected, actual }) => {
                revision_conflict_text(expected, actual)
            }
            Ok(_) => error_text("Core returned the wrong operation result"),
            Err(error) => error_text(error.to_string()),
        }
    }

    fn import_media(&self, expected_revision: TimelineRevision, path: &Path) -> CallToolResult {
        let asset = match self.analysis.probe(path) {
            Ok(asset) => asset,
            Err(error) => return error_text(error.to_string()),
        };
        self.apply_operation(
            "import_media",
            expected_revision,
            Operation::AddAsset { asset },
        )
    }

    fn apply_edit_plan(
        &self,
        expected_revision: TimelineRevision,
        operations: &[Operation],
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, before) = self.snapshot()?;
        if expected_revision != actual_revision {
            return Ok(revision_conflict_text(expected_revision, actual_revision));
        }
        if let Some(description) = plan_confirmation_description(&before, operations)
            && let Err(reason) = self.confirmations.confirm("apply_edit_plan", description)
        {
            return Ok(error_text(format!(
                "refused destructive tool apply_edit_plan: {reason}"
            )));
        }
        let added_assets = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::AddAsset { asset } => Some(asset.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let event = self
            .core
            .request(Command::DoBatchIfRevision {
                expected: expected_revision,
                operations: operations.to_vec(),
            })
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(match event {
            Event::DocumentChanged { doc, revision, .. } => {
                if self.publish_to_playback {
                    self.playback.set_document(Arc::clone(&doc));
                }
                for asset in added_assets {
                    self.request_asset_analysis(asset);
                }
                let footer = self.remaining_silence_footer(&doc);
                success_text(format!(
                    "{}{footer}",
                    render_plan_outcomes(
                        operations,
                        None,
                        Some(state_delta(
                            "apply_edit_plan",
                            Some(&before),
                            &doc,
                            revision,
                        )),
                    )
                ))
            }
            Event::BatchRejected { error, .. } => {
                error_text(render_plan_outcomes(operations, Some(&error), None))
            }
            Event::RevisionConflict { expected, actual } => {
                revision_conflict_text(expected, actual)
            }
            _ => error_text("Core returned the wrong edit-plan result"),
        })
    }

    fn request_asset_analysis(&self, asset: kinewright_core::MediaAsset) {
        self.analysis.request_transcription(asset.clone());
        self.analysis.request_silence_detection(asset.clone());
        self.analysis.request_scene_detection(asset.clone());
        self.analysis.request_beat_detection(asset);
    }

    fn caption_presets() -> CallToolResult {
        let presets = CaptionPreset::ALL.map(|preset| {
            let title = preset.title("Example caption");
            serde_json::json!({
                "id": preset.as_str(),
                "font_size_token": title.font_size_token,
                "color_token": title.color_token,
                "position": title.position.as_str(),
                "background_scrim": title.background_scrim,
                "motions": CaptionMotion::ALL.map(CaptionMotion::as_str),
            })
        });
        success_text(
            serde_json::to_string_pretty(&presets)
                .unwrap_or_else(|error| format!("could not serialize presets: {error}")),
        )
    }

    fn captions(&self, args: CaptionListArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let range = args.range.map(|range| range.start..range.end);
        if let Some(range) = &range
            && (range.start < TimeCode::ZERO
                || range.end <= range.start
                || range.end > document.duration)
        {
            return Ok(error_text(format!(
                "caption range {}..{} is outside project range 0..{}",
                range.start.0, range.end.0, document.duration.0
            )));
        }
        let offset = args.offset.unwrap_or(0);
        let limit = args.limit.unwrap_or(50).clamp(1, 200);
        let mut captions = Vec::new();
        for track in &document.tracks {
            for clip in &track.clips {
                let ClipContent::Title(title) = &clip.content else {
                    continue;
                };
                let Some(preset) = title.caption_preset else {
                    continue;
                };
                let Ok(duration) = document.clip_duration(clip) else {
                    continue;
                };
                let Some(end) = clip.timeline_start.checked_add(duration) else {
                    continue;
                };
                if range.as_ref().is_some_and(|requested| {
                    end <= requested.start || clip.timeline_start >= requested.end
                }) {
                    continue;
                }
                captions.push((track.id, clip, title, preset, end));
            }
        }
        captions.sort_by_key(|(_, clip, _, _, _)| (clip.timeline_start, clip.id));
        let total = captions.len();
        let page = captions
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(track, clip, title, preset, end)| {
                serde_json::json!({
                    "clip_id": clip.id,
                    "track_id": track,
                    "start_frame": clip.timeline_start,
                    "end_frame": end,
                    "text": title.text,
                    "preset": preset,
                })
            })
            .collect::<Vec<_>>();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        let mut rendered = format!(
            "timeline_revision={revision} captions_total={total} offset={offset} returned={} next_offset={next_offset:?}",
            page.len()
        );
        for caption in &page {
            let _ = write!(
                rendered,
                "\nclip={} track={} range={}..{} preset={} text={:?}",
                caption["clip_id"],
                caption["track_id"],
                caption["start_frame"],
                caption["end_frame"],
                caption["preset"].as_str().unwrap_or("unknown"),
                caption["text"].as_str().unwrap_or_default(),
            );
        }
        Ok(success_structured(
            rendered,
            serde_json::json!({
                "timeline_revision": revision,
                "total": total,
                "offset": offset,
                "limit": limit,
                "next_offset": next_offset,
                "captions": page,
            }),
        ))
    }

    fn plan_caption_corrections(
        &self,
        args: CaptionCorrectionPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if args.expected_revision != revision {
            return Ok(revision_conflict_text(args.expected_revision, revision));
        }
        if args.corrections.is_empty() || args.corrections.len() > 100 {
            return Ok(error_text(
                "caption correction plan requires between 1 and 100 corrections",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut operations = Vec::with_capacity(args.corrections.len());
        for correction in args.corrections {
            if !seen.insert(correction.clip_id) {
                return Ok(error_text(format!(
                    "caption clip {} appears more than once",
                    correction.clip_id
                )));
            }
            if correction.text.trim().is_empty() {
                return Ok(error_text(format!(
                    "caption clip {} replacement text is empty",
                    correction.clip_id
                )));
            }
            let Some(clip) = document.clip(correction.clip_id) else {
                return Ok(error_text(format!(
                    "caption clip {} does not exist",
                    correction.clip_id
                )));
            };
            if !matches!(
                &clip.content,
                ClipContent::Title(title) if title.caption_preset.is_some()
            ) {
                return Ok(error_text(format!(
                    "clip {} is not a generated caption",
                    correction.clip_id
                )));
            }
            operations.push(Operation::SetTitleParam {
                clip: correction.clip_id,
                name: "text".to_owned(),
                value: ParamValue::Text(correction.text),
            });
        }
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "caption corrections are invalid: {error}"
                )));
            }
        };
        Ok(success_structured(
            format!(
                "prepared {} caption correction(s) as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.preview.operation_count, plan.id,
            ),
            serde_json::json!({
                "timeline_revision": revision,
                "prepared_edit_plan": {
                    "plan_id": plan.id,
                    "expected_revision": revision,
                    "preview": plan.preview,
                },
            }),
        ))
    }

    fn add_styled_captions(&self, args: &StyledCaptionsArgs) -> Result<CallToolResult, McpError> {
        let expected_revision = args.expected_revision;
        let (actual_revision, document) = self.snapshot()?;
        if expected_revision != actual_revision {
            return Ok(revision_conflict_text(expected_revision, actual_revision));
        }
        if args.intent == CaptionIntent::EditedReadable && args.script.is_none() {
            return Ok(error_text(
                "edited_readable captions require an explicit authored script",
            ));
        }
        let position = match caption_position(args.position, args.subject_y_percent) {
            Ok(position) => Some(position),
            Err(error) => return Ok(error_text(error)),
        };
        let words = self
            .analysis
            .timeline_transcript(&document, None)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let words = dedup_timeline_words(words);
        let mut cues = caption_cues(&words, document.fps);
        clamp_caption_cues_to_duration(&mut cues, document.duration);
        if let Some(script) = args.script.as_deref() {
            cues = match authored_caption_cues(&cues, script) {
                Ok(cues) => cues,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        }
        let operations = match animated_caption_operations_at(
            &document,
            &cues,
            args.preset,
            args.motion,
            position,
        ) {
            Ok(operations) => operations,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        self.apply_edit_plan(expected_revision, &operations)
    }

    fn qa_report(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let report = qa_document(&document);
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "timeline_revision": revision,
            "export_ready": report.export_ready(),
            "error_count": report.count(kinewright_core::QaSeverity::Error),
            "warning_count": report.count(kinewright_core::QaSeverity::Warning),
            "info_count": report.count(kinewright_core::QaSeverity::Info),
            "issues": report.issues,
        }))
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(success_text(json))
    }

    fn delivery_variants() -> CallToolResult {
        let variants = DeliveryAspect::ALL.map(|aspect| {
            let (width, height) = aspect.resolution();
            serde_json::json!({
                "aspect": aspect,
                "label": aspect.as_str(),
                "resolution": {"width": width, "height": height},
                "framing": "deterministic cover crop with explicit focal point",
            })
        });
        success_text(
            serde_json::to_string_pretty(&variants)
                .unwrap_or_else(|error| format!("could not serialize variants: {error}")),
        )
    }

    fn delivery_profiles(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let profiles = DeliveryProfile::ALL.map(|profile| {
            let settings = profile.export_settings(&document, ExportCancellation::default());
            serde_json::json!({
                "id": profile.as_str(),
                "container": profile.container_extension(),
                "aspect": profile.aspect(),
                "resolution": {
                    "width": settings.resolution.0,
                    "height": settings.resolution.1,
                },
                "video_codec": settings.video_codec,
                "audio_codec": settings.audio_codec,
                "video_bitrate": settings.video_bitrate,
                "audio_bitrate": settings.audio_bitrate,
                "fps": {
                    "numerator": settings.fps.numerator(),
                    "denominator": settings.fps.denominator(),
                },
            })
        });
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "profiles": profiles,
        });
        Ok(success_structured(
            serde_json::to_string_pretty(&structured)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
            structured,
        ))
    }

    fn delivery_conformance(
        &self,
        args: &DeliveryConformanceArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let report = match delivery_conformance(
            &document,
            args.profile,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(report) => report,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "export_ready": report.export_ready(),
            "report": report,
        });
        Ok(success_structured(
            serde_json::to_string_pretty(&structured)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?,
            structured,
        ))
    }

    fn editorial_readiness(
        &self,
        args: &EditorialReadinessArgs,
    ) -> Result<CallToolResult, McpError> {
        let minimum = args.min_silence_source_frames.unwrap_or(TimeCode(20));
        if args.check_silence && minimum <= TimeCode::ZERO {
            return Ok(error_text("min_silence_source_frames must be positive"));
        }
        if args.focus_x_percent > 100 || args.focus_y_percent > 100 {
            return Ok(error_text("delivery focus percentages must be in 0..=100"));
        }
        let (revision, document) = self.snapshot()?;
        let (cuttable, pending_silence_assets) = if args.check_silence {
            self.editorial_silence_evidence(&document, minimum)?
        } else {
            (Vec::new(), Vec::new())
        };
        let qa = qa_document(&document);
        let conformance = match delivery_conformance(
            &document,
            args.profile,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(report) => report,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut storyboard = self.editorial_readiness_storyboard(revision, &document, args)?;
        if storyboard.is_error == Some(true) {
            return Ok(storyboard);
        }
        let qa_errors = qa.count(kinewright_core::QaSeverity::Error);
        let conformance_errors = conformance
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Error)
            .count();
        let ready = pending_silence_assets.is_empty()
            && cuttable.is_empty()
            && qa_errors == 0
            && conformance_errors == 0;
        let cuttable_json = cuttable
            .iter()
            .map(|span| {
                serde_json::json!({
                    "asset_id": span.asset,
                    "track_id": span.track,
                    "clip_id": span.clip,
                    "source_start": span.source_start,
                    "source_end": span.source_end,
                    "project_start": span.project_start,
                    "project_end": span.project_end,
                })
            })
            .collect::<Vec<_>>();
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "ready": ready,
            "silence": {
                "checked": args.check_silence,
                "minimum_source_frames": minimum,
                "cuttable_count": cuttable.len(),
                "spans": cuttable_json,
                "pending_asset_ids": pending_silence_assets,
            },
            "qa": {
                "export_ready": qa.export_ready(),
                "error_count": qa_errors,
                "warning_count": qa.count(kinewright_core::QaSeverity::Warning),
                "blocking_issues": qa.issues.iter().filter(|issue| issue.severity == kinewright_core::QaSeverity::Error).collect::<Vec<_>>(),
            },
            "delivery": {
                "profile": args.profile,
                "export_ready": conformance.export_ready(),
                "resolution": conformance.resolution,
                "error_count": conformance_errors,
                "blocking_issues": conformance.issues.iter().filter(|issue| issue.severity == kinewright_core::QaSeverity::Error).collect::<Vec<_>>(),
            },
            "storyboard": storyboard.structured_content,
        });
        let summary = serde_json::to_string(&structured)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let mut result = CallToolResult::success(vec![ContentBlock::text(summary)]);
        result.content.append(&mut storyboard.content);
        result.structured_content = Some(structured);
        Ok(result)
    }

    fn editorial_silence_evidence(
        &self,
        document: &Document,
        minimum: TimeCode,
    ) -> Result<(Vec<TimelineSilenceSpan>, Vec<AssetId>), McpError> {
        let transcripts = document
            .media_pool
            .iter()
            .filter_map(|asset| match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some((asset.id, transcript)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .map(|asset| asset.id)
            .collect::<Vec<_>>();
        let spans = self
            .analysis
            .timeline_silences(document, None, minimum)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok((
            cuttable_timeline_silences(document, &spans, &transcripts, minimum),
            pending,
        ))
    }

    fn editorial_readiness_storyboard(
        &self,
        revision: TimelineRevision,
        document: &Document,
        args: &EditorialReadinessArgs,
    ) -> Result<CallToolResult, McpError> {
        let document = match document_for_delivery_profile(
            document,
            args.profile,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(document) => Arc::new(document),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        self.storyboard_for_document(
            revision,
            &document,
            StoryboardArgs {
                range: args
                    .storyboard
                    .range
                    .as_ref()
                    .map(|range| TranscriptRangeArgs {
                        start: range.start,
                        end: range.end,
                    }),
                frame_count: args.storyboard.frame_count,
                max_width: args.storyboard.max_width,
            },
            "editorial readiness storyboard",
            Some(serde_json::json!({
                "profile": args.profile,
                "focus_x_percent": args.focus_x_percent,
                "focus_y_percent": args.focus_y_percent,
                "resolution": {"width": document.resolution.0, "height": document.resolution.1},
            })),
        )
    }

    fn queue_export(&self, args: QueueExportArgs) -> Result<CallToolResult, McpError> {
        let Some(queue) = &self.export_queue else {
            return Ok(error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            ));
        };
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            return Ok(revision_conflict_text(
                args.expected_revision,
                actual_revision,
            ));
        }
        if document
            .media_pool
            .iter()
            .any(|asset| paths_resolve_equal(&args.output_path, &asset.path))
        {
            return Ok(error_text(
                "refusing to export over a source media asset used by this project",
            ));
        }
        if args.overwrite {
            let description = format!(
                "The agent wants permission to replace the regular file at {} if it exists when this queued export starts.",
                args.output_path.display()
            );
            if let Err(reason) = self.confirmations.confirm("queue_export", description) {
                return Ok(error_text(format!(
                    "refused destructive tool queue_export: {reason}"
                )));
            }
        }
        let record = match queue.enqueue(
            &document,
            QueueExportRequest {
                output_path: args.output_path,
                profile: args.profile,
                focus_x_percent: args.focus_x_percent,
                focus_y_percent: args.focus_y_percent,
                overwrite: args.overwrite,
            },
        ) {
            Ok(record) => record,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let structured = serde_json::json!({
            "timeline_revision": actual_revision.0,
            "job": record,
        });
        Ok(success_structured(
            format!(
                "queued export job {} from immutable timeline revision {} to {}",
                record.id.0,
                actual_revision.0,
                record.output_path.display(),
            ),
            structured,
        ))
    }

    fn export_jobs(&self) -> CallToolResult {
        let Some(queue) = &self.export_queue else {
            return error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            );
        };
        let jobs = queue.list();
        success_structured(
            format!("{} retained export job(s)", jobs.len()),
            serde_json::json!({"jobs": jobs}),
        )
    }

    fn cancel_export(&self, job_id: ExportJobId) -> CallToolResult {
        let Some(queue) = &self.export_queue else {
            return error_text(
                "agent exports are unavailable because this MCP server has no export backend",
            );
        };
        let Some(job) = queue.cancel(job_id) else {
            return error_text(format!("export job {} does not exist", job_id.0));
        };
        success_structured(
            format!("export job {} is now {:?}", job_id.0, job.state),
            serde_json::json!({"job": job}),
        )
    }

    /// Deterministic completion feedback: the plan result itself reports how
    /// much cuttable silence remains, so an agent asked to remove dead air
    /// cannot mistake a partial plan for a finished one.
    fn remaining_silence_footer(&self, document: &kinewright_core::Document) -> String {
        let Ok(spans) = self.analysis.timeline_silences(
            document,
            None,
            TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
        ) else {
            return String::new();
        };
        let mut cuttable = 0_usize;
        for span in &spans {
            let Some(asset) = document.asset(span.asset) else {
                continue;
            };
            let words = match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some(transcript),
                _ => None,
            };
            cuttable += crate::silence::shrink_silence_span_for_cutting_with_transcript(
                kinewright_core::SilenceSpan {
                    source_start: span.source_start,
                    source_end: span.source_end,
                },
                asset.fps,
                words.as_ref().map(|transcript| transcript.words.as_slice()),
            )
            .len();
        }
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .count();
        let mut footer = if cuttable == 0 {
            "\nno cuttable silence remains on the timeline".to_owned()
        } else {
            format!("\ncuttable silence spans remaining on the timeline: {cuttable}")
        };
        if pending > 0 {
            let _ = write!(footer, " (silence analysis pending for {pending} asset(s))");
        }
        footer
    }

    fn frame_at(&self, timecode: TimeCode) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if timecode < TimeCode::ZERO || timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                timecode.0, document.duration.0
            )));
        }
        let image =
            match self
                .analysis
                .thumbnail_for_document(document, timecode, THUMBNAIL_MAX_WIDTH)
            {
                Ok(image) => image,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let png = encode_png(&image)?;
        Ok(CallToolResult::success(vec![
            ContentBlock::text(format!(
                "project frame {} ({}x{})",
                timecode.0, image.width, image.height
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]))
    }

    fn video_scopes(&self, args: &VideoScopesArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if args.timecode < TimeCode::ZERO || args.timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                args.timecode.0, document.duration.0
            )));
        }
        let max_width = args.max_width.unwrap_or(512).clamp(32, 1_024);
        let bins = usize::from(args.bins.unwrap_or(64).clamp(16, 128));
        let image = match self
            .analysis
            .thumbnail_for_document(document, args.timecode, max_width)
        {
            Ok(image) => image,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let scopes = scope_data(&image, bins);
        Ok(success_structured(
            format!(
                "video scopes at project frame {} from {}x{} compositor output\n{}",
                args.timecode.0, image.width, image.height, scopes
            ),
            scopes,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn track_mask_region(&self, args: &TrackMaskArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("mask tracking requires a media clip"));
        }
        let Some(effect) = clip
            .effects
            .iter()
            .find(|effect| effect.id == args.effect_id)
        else {
            return Ok(error_text(format!(
                "effect {} does not exist on clip {}",
                args.effect_id, args.clip_id
            )));
        };
        if effect.name != "mask" {
            return Ok(error_text(format!(
                "effect {} is {}; mask tracking requires a mask effect",
                args.effect_id, effect.name
            )));
        }
        let duration = match document.clip_duration(clip) {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let start = args.start_local_frame.unwrap_or(TimeCode::ZERO);
        let end = args.end_local_frame.unwrap_or(duration);
        if start < TimeCode::ZERO || end > duration || end <= start {
            return Ok(error_text(format!(
                "tracking range {start}..{end} is outside clip-local range 0..{duration}"
            )));
        }
        let step = args.step_frames.unwrap_or(DEFAULT_TRACKING_STEP_FRAMES);
        if !(1..=120).contains(&step) {
            return Ok(error_text("step_frames must be in 1..=120"));
        }
        let sample_frames = tracking_sample_frames(start..end, step);
        if sample_frames.len() > MAX_TRACKING_SAMPLES {
            return Ok(error_text(format!(
                "tracking would render {} samples; increase step_frames to stay at or below {MAX_TRACKING_SAMPLES}",
                sample_frames.len()
            )));
        }
        let parameter =
            |name: &str, neutral: i64| effect.integer_parameter_at(name, start).unwrap_or(neutral);
        let center_percent = [
            u8::try_from(parameter("center_x_percent", 50).clamp(0, 100)).unwrap_or(50),
            u8::try_from(parameter("center_y_percent", 50).clamp(0, 100)).unwrap_or(50),
        ];
        let box_percent = [
            parameter("width_percent", 100),
            parameter("height_percent", 100),
        ];
        if box_percent.iter().any(|value| !(1..=75).contains(value)) {
            return Ok(error_text(
                "mask width_percent and height_percent must each be in 1..=75 for tracking; set a bounded subject region first",
            ));
        }
        let search_radius = args
            .search_radius_percent
            .unwrap_or(DEFAULT_TRACKING_SEARCH_RADIUS_PERCENT);
        if !(1..=25).contains(&search_radius) {
            return Ok(error_text("search_radius_percent must be in 1..=25"));
        }
        let max_width = args.max_width.unwrap_or(DEFAULT_TRACKING_WIDTH);
        if !(64..=512).contains(&max_width) {
            return Ok(error_text("max_width must be in 64..=512"));
        }

        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            excluded_effect_name: "mask",
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        let observations = tracked.observations;

        let curve_for = |axis: usize, extent: u32| AutomationCurve {
            keyframes: observations
                .iter()
                .map(|observation| Keyframe {
                    at: observation.local_frame,
                    value: i64::from(pixel_to_percent(observation.center[axis], extent)),
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        let x_curve = curve_for(0, tracked.width);
        let y_curve = curve_for(1, tracked.height);
        let operations = vec![
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: "center_x_percent".to_owned(),
                curve: x_curve.clone(),
            },
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: "center_y_percent".to_owned(),
                curve: y_curve.clone(),
            },
        ];
        let observations_json = observations
            .iter()
            .map(|observation| {
                serde_json::json!({
                    "local_frame": observation.local_frame.0,
                    "project_frame": observation.project_frame.0,
                    "center_x_percent": pixel_to_percent(observation.center[0], tracked.width),
                    "center_y_percent": pixel_to_percent(observation.center[1], tracked.height),
                    "confidence_basis_points": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked mask keyframes do not fit the current clip: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "observations": observations_json,
            "curves": {
                "center_x_percent": x_curve,
                "center_y_percent": y_curve,
            },
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "tracked mask effect {} on clip {} across {} samples as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                args.effect_id,
                args.clip_id,
                observations.len(),
                plan.id,
            ),
            structured,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn track_reframe_subject(&self, args: &TrackReframeArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("subject reframe tracking requires a media clip"));
        }
        let Some(effect) = clip
            .effects
            .iter()
            .find(|effect| effect.id == args.effect_id)
        else {
            return Ok(error_text(format!(
                "effect {} does not exist on clip {}",
                args.effect_id, args.clip_id
            )));
        };
        if effect.name != "reframe" {
            return Ok(error_text(format!(
                "effect {} is {}; subject tracking requires a reframe effect",
                args.effect_id, effect.name
            )));
        }
        let Some((source_width, source_height)) = document
            .asset(clip.asset)
            .and_then(|asset| asset.resolution)
        else {
            return Ok(error_text(format!(
                "clip {} source resolution is required to plan full tracked-subject containment",
                args.clip_id
            )));
        };
        if source_width == 0 || source_height == 0 {
            return Ok(error_text(format!(
                "clip {} has invalid source resolution {source_width}x{source_height}",
                args.clip_id
            )));
        }
        if !(1..=75).contains(&args.subject_width_percent)
            || !(1..=75).contains(&args.subject_height_percent)
        {
            return Ok(error_text(
                "subject_width_percent and subject_height_percent must each be in 1..=75",
            ));
        }
        let duration = match document.clip_duration(clip) {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let start = args.start_local_frame.unwrap_or(TimeCode::ZERO);
        let end = args.end_local_frame.unwrap_or(duration);
        if start < TimeCode::ZERO || end > duration || end <= start {
            return Ok(error_text(format!(
                "tracking range {start}..{end} is outside clip-local range 0..{duration}"
            )));
        }
        let step = args.step_frames.unwrap_or(DEFAULT_TRACKING_STEP_FRAMES);
        if !(1..=120).contains(&step) {
            return Ok(error_text("step_frames must be in 1..=120"));
        }
        let sample_frames = tracking_sample_frames(start..end, step);
        if sample_frames.len() > MAX_TRACKING_SAMPLES {
            return Ok(error_text(format!(
                "tracking would render {} samples; increase step_frames to stay at or below {MAX_TRACKING_SAMPLES}",
                sample_frames.len()
            )));
        }
        let parameter = |name: &str| {
            u8::try_from(
                effect
                    .integer_parameter_at(name, start)
                    .unwrap_or(50)
                    .clamp(0, 100),
            )
            .unwrap_or(50)
        };
        let initial_x = args
            .initial_subject_x_percent
            .unwrap_or_else(|| parameter("focus_x_percent"));
        let initial_y = args
            .initial_subject_y_percent
            .unwrap_or_else(|| parameter("focus_y_percent"));
        if initial_x > 100 || initial_y > 100 {
            return Ok(error_text(
                "initial_subject_x_percent and initial_subject_y_percent must be in 0..=100",
            ));
        }
        let focus_bounds = [
            args.minimum_focus_x_percent.unwrap_or(0),
            args.maximum_focus_x_percent.unwrap_or(100),
            args.minimum_focus_y_percent.unwrap_or(0),
            args.maximum_focus_y_percent.unwrap_or(100),
        ];
        if focus_bounds.iter().any(|value| *value > 100)
            || focus_bounds[0] > focus_bounds[1]
            || focus_bounds[2] > focus_bounds[3]
        {
            return Ok(error_text(
                "focus bounds must be ordered percentages in 0..=100",
            ));
        }
        let focus_dead_zone = args
            .focus_dead_zone_percent
            .unwrap_or(DEFAULT_REFRAME_DEAD_ZONE_PERCENT);
        if focus_dead_zone > 25 {
            return Ok(error_text("focus_dead_zone_percent must be in 0..=25"));
        }
        let maximum_focus_step = args
            .maximum_focus_step_percent
            .unwrap_or(DEFAULT_REFRAME_MAXIMUM_STEP_PERCENT);
        if !(1..=25).contains(&maximum_focus_step) {
            return Ok(error_text("maximum_focus_step_percent must be in 1..=25"));
        }
        let search_radius = args
            .search_radius_percent
            .unwrap_or(DEFAULT_TRACKING_SEARCH_RADIUS_PERCENT);
        if !(1..=25).contains(&search_radius) {
            return Ok(error_text("search_radius_percent must be in 1..=25"));
        }
        let max_width = args.max_width.unwrap_or(DEFAULT_TRACKING_WIDTH);
        if !(64..=512).contains(&max_width) {
            return Ok(error_text("max_width must be in 64..=512"));
        }
        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent: [initial_x, initial_y],
            box_percent: [
                i64::from(args.subject_width_percent),
                i64::from(args.subject_height_percent),
            ],
            search_radius_percent: search_radius,
            max_width,
            excluded_effect_name: "reframe",
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        let subject_box_percent = [
            i64::from(args.subject_width_percent),
            i64::from(args.subject_height_percent),
        ];
        let provenance_samples = tracked
            .observations
            .iter()
            .map(|observation| {
                tracked_subject_bounds(
                    observation,
                    tracked.width,
                    tracked.height,
                    subject_box_percent,
                )
            })
            .collect::<Vec<_>>();
        let containment = provenance_samples
            .iter()
            .map(|subject| {
                let target_aspect_basis_points = effect
                    .integer_parameter_at("target_aspect_basis_points", subject.at)
                    .ok_or_else(|| {
                        format!(
                            "reframe effect {} has no target_aspect_basis_points at frame {}",
                            args.effect_id, subject.at
                        )
                    })?;
                tracked_subject_focus_constraint(
                    *subject,
                    source_width,
                    source_height,
                    target_aspect_basis_points,
                )
            })
            .collect::<Result<Vec<_>, _>>();
        let containment = match containment {
            Ok(constraints) => constraints,
            Err(error) => return Ok(error_text(error)),
        };
        let samples = tracked
            .observations
            .iter()
            .map(|observation| SubjectCenterBasisPointSample {
                at: observation.local_frame,
                x_basis_points: i64::from(pixel_to_basis_points(
                    observation.center[0],
                    tracked.width,
                )),
                y_basis_points: i64::from(pixel_to_basis_points(
                    observation.center[1],
                    tracked.height,
                )),
                confidence_basis_points: observation.confidence_basis_points,
            })
            .collect::<Vec<_>>();
        let reframe = match plan_subject_reframe_basis_points_with_containment(
            &document,
            SubjectReframeSettings {
                clip: args.clip_id,
                effect: args.effect_id,
                bounds: ReframeFocusBounds {
                    min_x_percent: i64::from(focus_bounds[0]),
                    max_x_percent: i64::from(focus_bounds[1]),
                    min_y_percent: i64::from(focus_bounds[2]),
                    max_y_percent: i64::from(focus_bounds[3]),
                },
                minimum_confidence_basis_points: 0,
                focus_dead_zone_percent: focus_dead_zone,
                maximum_focus_step_percent: maximum_focus_step,
            },
            &samples,
            &containment,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "full tracked-subject containment could not be planned: {error}"
                )));
            }
        };
        let x_curve = &reframe.focus_x_curve;
        let y_curve = &reframe.focus_y_curve;
        let focus_keyframes = tracked
            .observations
            .iter()
            .zip(&x_curve.keyframes)
            .zip(&y_curve.keyframes)
            .map(|((observation, x), y)| {
                serde_json::json!({
                    "frame": observation.local_frame.0,
                    "x_basis_points": x.value,
                    "y_basis_points": y.value,
                    "confidence": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
        let minimum_confidence = tracked
            .observations
            .iter()
            .map(|observation| observation.confidence_basis_points)
            .min()
            .unwrap_or_default();
        let provenance = ReframeSubjectProvenance {
            clip: args.clip_id,
            effect: args.effect_id,
            samples: provenance_samples,
        };
        let provenance_label = encode_reframe_subject_provenance(&provenance);
        let existing_provenance_marker = document.markers.iter().find_map(|marker| {
            decode_reframe_subject_provenance(&marker.label)
                .ok()
                .flatten()
                .filter(|existing| {
                    existing.clip == args.clip_id && existing.effect == args.effect_id
                })
                .map(|_| marker.id)
        });
        let provenance_operation = if let Some(marker) = existing_provenance_marker {
            Operation::SetMarkerParam {
                marker,
                name: "label".to_owned(),
                value: ParamValue::Text(provenance_label),
            }
        } else {
            let next_marker_id = document
                .markers
                .iter()
                .map(|marker| marker.id.0)
                .max()
                .unwrap_or_default()
                .checked_add(1)
                .map(MarkerId)
                .ok_or_else(|| {
                    McpError::internal_error("marker id space is exhausted".to_owned(), None)
                })?;
            Operation::AddMarker {
                marker: Marker {
                    id: next_marker_id,
                    position: clip.timeline_start,
                    label: provenance_label,
                    color_token: 3,
                },
            }
        };
        let mut operations = reframe.operations;
        operations.push(provenance_operation);
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked reframe keyframes do not fit the current clip: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "subject_template": {
                "width_percent": args.subject_width_percent,
                "height_percent": args.subject_height_percent,
                "initial_center_percent": {"x": initial_x, "y": initial_y},
            },
            "focus_bounds_percent": {
                "minimum_x": focus_bounds[0],
                "maximum_x": focus_bounds[1],
                "minimum_y": focus_bounds[2],
                "maximum_y": focus_bounds[3],
            },
            "camera_stabilization": {
                "controller": "offline_lookahead_containment",
                "subject_dead_zone_percent": focus_dead_zone,
                "maximum_step_percent": maximum_focus_step,
                "observation_filter": "three_sample_median",
                "keyframe_interpolation": "linear",
            },
            "minimum_confidence_basis_points": minimum_confidence,
            "focus_keyframes": focus_keyframes,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
            "detection_boundary": "tracks the explicitly supplied subject region; no learned person or face detection",
        });
        Ok(success_structured(
            format!(
                "tracked and stabilized reframe effect {} on clip {} across {} samples (minimum confidence {minimum_confidence}/10000) as edit plan {}; review low-confidence spans and the preview, then commit it at timeline revision {revision}",
                args.effect_id,
                args.clip_id,
                tracked.observations.len(),
                plan.id,
            ),
            structured,
        ))
    }

    fn track_clip_region(
        &self,
        request: &RegionTrackingRequest<'_>,
    ) -> Result<TrackedRegion, String> {
        let mut isolated = request.document.clone();
        for track in &mut isolated.tracks {
            track
                .clips
                .retain(|candidate| candidate.id == request.clip_id);
            for candidate in &mut track.clips {
                candidate
                    .effects
                    .retain(|effect| effect.name != request.excluded_effect_name);
            }
        }
        isolated.tracks.retain(|track| !track.clips.is_empty());
        let isolated = Arc::new(isolated);
        let project_frame = |local: TimeCode| {
            request
                .clip_timeline_start
                .checked_add(local)
                .ok_or_else(|| "tracking frame overflowed".to_owned())
        };
        let Some(first_local) = request.sample_frames.first().copied() else {
            return Err("tracking requires at least one sample".to_owned());
        };
        let first_project = project_frame(first_local)?;
        let mut previous = self
            .analysis
            .thumbnail_for_document(Arc::clone(&isolated), first_project, request.max_width)
            .map_err(|error| error.to_string())?;
        let width = previous.width;
        let height = previous.height;
        let half_size = tracking_half_size(&previous, request.box_percent);
        let mut center = clamp_tracking_center(
            &previous,
            [
                percent_to_pixel(request.center_percent[0], width),
                percent_to_pixel(request.center_percent[1], height),
            ],
            half_size,
        );
        let mut observations = vec![TrackingObservation {
            local_frame: first_local,
            project_frame: first_project,
            center,
            confidence_basis_points: 10_000,
        }];

        for local_frame in request.sample_frames.iter().copied().skip(1) {
            let project_frame = project_frame(local_frame)?;
            let current = self
                .analysis
                .thumbnail_for_document(Arc::clone(&isolated), project_frame, request.max_width)
                .map_err(|error| error.to_string())?;
            if current.width != width || current.height != height {
                return Err("tracking compositor resolution changed between samples".to_owned());
            }
            let tracked = track_region(
                &previous,
                &current,
                center,
                half_size,
                request.search_radius_percent,
            );
            center = tracked.center;
            observations.push(TrackingObservation {
                local_frame,
                project_frame,
                center,
                confidence_basis_points: tracked.confidence_basis_points,
            });
            previous = current;
        }

        Ok(TrackedRegion {
            observations,
            width,
            height,
        })
    }

    fn timeline_storyboard(&self, args: StoryboardArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        self.storyboard_for_document(revision, &document, args, "timeline storyboard", None)
    }

    #[allow(clippy::too_many_lines)]
    fn source_storyboard(&self, args: &SourceStoryboardArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(args.asset_id).cloned() else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        if !asset.kind.supports(TrackKind::Video) {
            return Ok(error_text(format!(
                "asset {} is not a video asset",
                asset.id
            )));
        }

        let source_in = args
            .range
            .as_ref()
            .map_or(TimeCode::ZERO, |range| range.start);
        let source_out = args
            .range
            .as_ref()
            .map_or(asset.duration, |range| range.end);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source storyboard range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }

        let frame_count = args.frame_count.unwrap_or(STORYBOARD_DEFAULT_FRAMES);
        if !(1..=STORYBOARD_MAX_FRAMES).contains(&frame_count) {
            return Ok(error_text(format!(
                "frame_count must be in 1..={STORYBOARD_MAX_FRAMES}"
            )));
        }
        let max_width = args.max_width.unwrap_or(STORYBOARD_DEFAULT_CELL_WIDTH);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        let source_range = source_in..source_out;
        let duration = match map_source_range_to_project(source_range.clone(), asset.fps, asset.fps)
        {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let temporary = Arc::new(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range,
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset.clone()],
            fps: asset.fps,
            resolution: asset.resolution.unwrap_or((1_920, 1_080)),
            duration,
            ..Document::default()
        });

        let frames = storyboard_sample_frames(&(TimeCode::ZERO..duration), frame_count);
        let mut images = Vec::with_capacity(frames.len());
        for frame in &frames {
            match self
                .analysis
                .thumbnail_for_document(Arc::clone(&temporary), *frame, max_width)
            {
                Ok(image) => images.push(image),
                Err(error) => return Ok(error_text(error.to_string())),
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let source_range_value = serde_json::json!({
            "start": source_in.0,
            "end": source_out.0,
        });
        let cells = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let source_frame = source_in
                    .checked_add(*frame)
                    .expect("validated source storyboard frame cannot overflow");
                serde_json::json!({
                    "cell": index + 1,
                    "asset_id": asset.id.0,
                    "source_frame": source_frame.0,
                    "source_range": source_range_value.clone(),
                })
            })
            .collect::<Vec<_>>();
        let manifest = serde_json::json!({
            "asset_id": asset.id.0,
            "source_range": source_range_value,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "source storyboard asset={} range={}..{} cells={}\n{}",
                asset.id,
                source_in,
                source_out,
                frames.len(),
                manifest
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    /// Return source-monitor candidates derived from cached scene boundaries.
    ///
    /// This deliberately builds an isolated, throwaway document for thumbnail
    /// rendering. It is an inspector: no Core command, prepared plan, or
    /// playback document is changed.
    #[allow(clippy::too_many_lines)]
    fn source_shot_board(&self, args: &SourceShotBoardArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(args.asset_id).cloned() else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        if !asset.kind.supports(TrackKind::Video) {
            return Ok(error_text(format!(
                "asset {} is not a video asset",
                asset.id
            )));
        }

        let source_in = args
            .range
            .as_ref()
            .map_or(TimeCode::ZERO, |range| range.start);
        let source_out = args
            .range
            .as_ref()
            .map_or(asset.duration, |range| range.end);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source shot board range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }
        let candidate_selection = args.candidate_selection.unwrap_or_default();
        if candidate_selection == ShotBoardCandidateSelection::Coverage
            && args.candidate_offset.is_some()
        {
            return Ok(error_text(
                "candidate_offset is only supported when candidate_selection is `page`; omit it when using `coverage`",
            ));
        }
        let candidate_count = args
            .candidate_count
            .unwrap_or(SHOT_BOARD_DEFAULT_CANDIDATES);
        if !(1..=SHOT_BOARD_MAX_CANDIDATES).contains(&candidate_count) {
            return Ok(error_text(format!(
                "candidate_count must be in 1..={SHOT_BOARD_MAX_CANDIDATES}"
            )));
        }
        let max_width = args.max_width.unwrap_or(STORYBOARD_DEFAULT_CELL_WIDTH);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        if let Some(minimum_duration_frames) = args.minimum_duration_frames
            && minimum_duration_frames.0 < 1
        {
            return Ok(error_text(
                "minimum_duration_frames must be at least 1 when provided",
            ));
        }
        let minimum_confidence_basis_points = args
            .minimum_confidence_basis_points
            .unwrap_or(DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS);
        if minimum_confidence_basis_points > 10_000 {
            return Ok(error_text(
                "minimum_confidence_basis_points must be in 0..=10000",
            ));
        }

        let mut status = self.analysis.scene_status(&asset);
        if status == SceneStatus::NotRequested {
            self.analysis.request_scene_detection(asset.clone());
            status = self.analysis.scene_status(&asset);
        }
        let scenes = match status {
            SceneStatus::Ready(scenes) => scenes,
            SceneStatus::NotRequested
            | SceneStatus::Queued
            | SceneStatus::Hashing
            | SceneStatus::Analyzing => {
                let status = match status {
                    SceneStatus::NotRequested => "requested",
                    SceneStatus::Queued => "queued",
                    SceneStatus::Hashing => "hashing",
                    SceneStatus::Analyzing => "analyzing",
                    _ => unreachable!(),
                };
                let manifest = serde_json::json!({
                    "asset_id": asset.id.0,
                    "source_range": {"start": source_in.0, "end": source_out.0},
                    "status": "pending",
                    "analysis_status": status,
                    "scene_confidence_threshold_basis_points": minimum_confidence_basis_points,
                    "minimum_duration_frames": args.minimum_duration_frames.map(|duration| duration.0),
                    "candidate_selection": candidate_selection.as_str(),
                    "candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| args.candidate_offset.unwrap_or(0)),
                    "requested_candidate_count": candidate_count,
                    "message": "scene analysis is pending; call get_source_shot_board again when it is ready",
                });
                let mut result = success_text(manifest.to_string());
                result.structured_content = Some(manifest);
                return Ok(result);
            }
            SceneStatus::NoVideo => {
                return Ok(error_text(format!(
                    "asset {} has no decodable video stream",
                    asset.id
                )));
            }
            SceneStatus::Cancelled => {
                return Ok(error_text(format!(
                    "scene analysis for asset {} was cancelled; request it again",
                    asset.id
                )));
            }
            SceneStatus::Failed(message) => {
                return Ok(error_text(format!(
                    "scene analysis for asset {} failed: {message}",
                    asset.id
                )));
            }
        };

        let mut cuts = BTreeMap::<TimeCode, u16>::new();
        for change in &scenes.changes {
            if change.confidence_basis_points >= minimum_confidence_basis_points
                && change.source_frame > source_in
                && change.source_frame < source_out
            {
                cuts.entry(change.source_frame)
                    .and_modify(|confidence| {
                        *confidence = (*confidence).max(change.confidence_basis_points);
                    })
                    .or_insert(change.confidence_basis_points);
            }
        }
        let boundaries = std::iter::once(source_in)
            .chain(cuts.keys().copied())
            .chain(std::iter::once(source_out))
            .collect::<Vec<_>>();
        let candidates = boundaries
            .windows(2)
            .enumerate()
            .map(|(index, boundary)| {
                let start = boundary[0];
                let end = boundary[1];
                serde_json::json!({
                    "candidate_id": format!("asset-{}-scene-{}-{}", asset.id.0, start.0, end.0),
                    "candidate_index": index,
                    "asset_id": asset.id.0,
                    "source_range": {"start": start.0, "end": end.0},
                    "duration_frames": end.0 - start.0,
                    "boundary_provenance": {
                        "start": if let Some(confidence) = cuts.get(&start) {
                            serde_json::json!({"kind": "scene_cut", "source_frame": start.0, "confidence_basis_points": confidence})
                        } else {
                            serde_json::json!({"kind": "requested_range_start", "source_frame": start.0})
                        },
                        "end": if let Some(confidence) = cuts.get(&end) {
                            serde_json::json!({"kind": "scene_cut", "source_frame": end.0, "confidence_basis_points": confidence})
                        } else {
                            serde_json::json!({"kind": "requested_range_end", "source_frame": end.0})
                        },
                    },
                })
            })
            .collect::<Vec<_>>();
        let minimum_duration_frames = args.minimum_duration_frames.map(|duration| duration.0);
        let eligible_candidates = candidates
            .iter()
            .filter(|candidate| {
                minimum_duration_frames.is_none_or(|minimum| {
                    candidate["duration_frames"]
                        .as_i64()
                        .is_some_and(|duration| duration >= minimum)
                })
            })
            .collect::<Vec<_>>();
        let selected_positions = match candidate_selection {
            ShotBoardCandidateSelection::Page => {
                let offset = args.candidate_offset.unwrap_or(0);
                if offset >= eligible_candidates.len() {
                    return Ok(error_text(format!(
                        "candidate_offset {offset} is outside 0..{} for the {} eligible candidates in this source range",
                        eligible_candidates.len().saturating_sub(1),
                        eligible_candidates.len()
                    )));
                }
                (offset..(offset + usize::from(candidate_count)).min(eligible_candidates.len()))
                    .collect::<Vec<_>>()
            }
            ShotBoardCandidateSelection::Coverage => coverage_candidate_positions(
                eligible_candidates.len(),
                usize::from(candidate_count),
            ),
        };
        let selected = selected_positions
            .iter()
            .map(|&position| eligible_candidates[position].clone())
            .collect::<Vec<_>>();

        let source_range = source_in..source_out;
        let duration = match map_source_range_to_project(source_range.clone(), asset.fps, asset.fps)
        {
            Ok(duration) => duration,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let temporary = Arc::new(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range,
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset.clone()],
            fps: asset.fps,
            resolution: asset.resolution.unwrap_or((1_920, 1_080)),
            duration,
            ..Document::default()
        });
        let mut images = Vec::new();
        let mut cells = Vec::new();
        for candidate in &selected {
            let candidate_range = candidate["source_range"]
                .as_object()
                .expect("candidate source range");
            let candidate_start =
                TimeCode(candidate_range["start"].as_i64().expect("candidate start"));
            let candidate_end = TimeCode(candidate_range["end"].as_i64().expect("candidate end"));
            for (evidence_index, source_frame) in
                shot_board_evidence_frames(candidate_start..candidate_end)
                    .into_iter()
                    .enumerate()
            {
                let evidence = ["start", "middle", "end"][evidence_index];
                let local_frame = TimeCode(source_frame.0 - source_in.0);
                match self.analysis.thumbnail_for_document(
                    Arc::clone(&temporary),
                    local_frame,
                    max_width,
                ) {
                    Ok(image) => images.push(image),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
                cells.push(serde_json::json!({
                    "cell": cells.len() + 1,
                    "candidate_id": candidate["candidate_id"].clone(),
                    "candidate_index": candidate["candidate_index"].clone(),
                    "evidence": evidence,
                    "asset_id": asset.id.0,
                    "source_frame": source_frame.0,
                    "source_range": candidate["source_range"].clone(),
                }));
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let manifest = serde_json::json!({
            "asset_id": asset.id.0,
            "source_range": {"start": source_in.0, "end": source_out.0},
            "status": "ready",
            "scene_confidence_threshold_basis_points": minimum_confidence_basis_points,
            "minimum_duration_frames": minimum_duration_frames,
            "candidate_selection": candidate_selection.as_str(),
            "candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| args.candidate_offset.unwrap_or(0)),
            "candidate_count": selected.len(),
            "requested_candidate_count": candidate_count,
            "returned_candidates": selected.len(),
            "filtered_candidates": eligible_candidates.len(),
            "total_candidates": candidates.len(),
            "selected_eligible_candidate_positions": selected_positions,
            "selected_candidate_indexes": selected.iter().map(|candidate| candidate["candidate_index"].clone()).collect::<Vec<_>>(),
            "next_candidate_offset": (candidate_selection == ShotBoardCandidateSelection::Page).then(|| {
                let offset = args.candidate_offset.unwrap_or(0);
                (offset + selected.len() < eligible_candidates.len()).then_some(offset + selected.len())
            }).flatten(),
            "evidence_per_candidate": SHOT_BOARD_EVIDENCE_PER_CANDIDATE,
            "candidates": selected,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "source shot board asset={} ready: returned {} of {} eligible candidates (selection={}, {}), {} evidence cells, sheet={}x{}; candidate ranges are in structured content",
                asset.id,
                selected.len(),
                eligible_candidates.len(),
                candidate_selection.as_str(),
                if candidate_selection == ShotBoardCandidateSelection::Page {
                    format!("offset={}", args.candidate_offset.unwrap_or(0))
                } else {
                    "full-range coverage".to_owned()
                },
                cells.len(),
                sheet.width,
                sheet.height,
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    fn delivery_variant_storyboard(
        &self,
        args: DeliveryStoryboardArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let variant =
            match DeliveryVariant::new(args.aspect, args.focus_x_percent, args.focus_y_percent) {
                Ok(variant) => variant,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let document = match document_for_delivery_variant(&document, variant) {
            Ok(document) => Arc::new(document),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let metadata = serde_json::json!({
            "aspect": variant.aspect,
            "aspect_label": variant.aspect.as_str(),
            "focus_x_percent": variant.focus_x_percent,
            "focus_y_percent": variant.focus_y_percent,
            "resolution": {"width": document.resolution.0, "height": document.resolution.1},
        });
        self.storyboard_for_document(
            revision,
            &document,
            args.storyboard,
            "delivery variant storyboard",
            Some(metadata),
        )
    }

    fn storyboard_for_document(
        &self,
        revision: TimelineRevision,
        document: &Arc<Document>,
        args: StoryboardArgs,
        label: &str,
        variant: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let range = validated_timeline_range(document, args.range, label)?;
        let frame_count = args.frame_count.unwrap_or(STORYBOARD_DEFAULT_FRAMES);
        if !(1..=STORYBOARD_MAX_FRAMES).contains(&frame_count) {
            return Ok(error_text(format!(
                "frame_count must be in 1..={STORYBOARD_MAX_FRAMES}"
            )));
        }
        let max_width = args.max_width.unwrap_or(STORYBOARD_DEFAULT_CELL_WIDTH);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        let frames = storyboard_sample_frames(&range, frame_count);
        let mut images = Vec::with_capacity(frames.len());
        for frame in &frames {
            match self
                .analysis
                .thumbnail_for_document(Arc::clone(document), *frame, max_width)
            {
                Ok(image) => images.push(image),
                Err(error) => return Ok(error_text(error.to_string())),
            }
        }
        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let cells = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                serde_json::json!({
                    "cell": index + 1,
                    "project_frame": frame.0,
                })
            })
            .collect::<Vec<_>>();
        let mut manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "range": {"start": range.start.0, "end": range.end.0},
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        if let Some(variant) = variant {
            manifest["delivery_variant"] = variant;
        }
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!("{label} {manifest}")),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    fn source_info(&self, args: &SourceInfoArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        let source_in = args.source_in.unwrap_or(TimeCode::ZERO);
        let source_out = args.source_out.unwrap_or(asset.duration);
        if source_in < TimeCode::ZERO || source_out > asset.duration || source_out <= source_in {
            return Ok(error_text(format!(
                "source monitor range {source_in}..{source_out} is outside asset {} range 0..{}",
                asset.id, asset.duration
            )));
        }

        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => Some(transcript),
            _ => None,
        };
        let words = transcript
            .as_ref()
            .map(|transcript| {
                transcript
                    .words
                    .iter()
                    .filter(|word| word.source_end > source_in && word.source_start < source_out)
                    .map(|word| {
                        serde_json::json!({
                            "text": word.text,
                            "speaker": word.speaker,
                            "source_start": word.source_start.0,
                            "source_end": word.source_end.0,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let speakers = transcript
            .as_ref()
            .into_iter()
            .flat_map(|transcript| &transcript.words)
            .filter(|word| word.source_end > source_in && word.source_start < source_out)
            .filter_map(|word| word.speaker.as_deref())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let scenes = match self.analysis.scene_status(asset) {
            SceneStatus::Ready(scenes) => scenes
                .changes
                .iter()
                .filter(|change| {
                    change.source_frame >= source_in && change.source_frame < source_out
                })
                .map(|change| {
                    serde_json::json!({
                        "source_frame": change.source_frame.0,
                        "confidence_basis_points": change.confidence_basis_points,
                    })
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let beats = match self.analysis.beat_status(asset) {
            BeatStatus::Ready(beats) => beats
                .beats
                .iter()
                .filter(|beat| beat.source_frame >= source_in && beat.source_frame < source_out)
                .map(|beat| {
                    serde_json::json!({
                        "source_frame": beat.source_frame.0,
                        "strength_basis_points": beat.strength_basis_points,
                    })
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let value = serde_json::json!({
            "asset": {
                "id": asset.id.0,
                "name": asset.name,
                "path": asset.path,
                "kind": asset.kind,
                "duration": asset.duration.0,
                "fps": {
                    "numerator": asset.fps.numerator(),
                    "denominator": asset.fps.denominator(),
                },
                "resolution": asset.resolution,
            },
            "source_monitor": {
                "source_in": source_in.0,
                "source_out": source_out.0,
                "duration": source_out.0 - source_in.0,
                "in_marked": args.source_in.is_some(),
                "out_marked": args.source_out.is_some(),
            },
            "speakers": speakers,
            "words": words,
            "scene_changes": scenes,
            "beats": beats,
            "analysis_jobs": self.analysis.analysis_jobs(asset),
        });
        Ok(success_structured(
            format!(
                "source asset={} range={}..{} words={} scenes={} beats={}\n{}",
                asset.id,
                source_in,
                source_out,
                value["words"].as_array().map_or(0, Vec::len),
                value["scene_changes"].as_array().map_or(0, Vec::len),
                value["beats"].as_array().map_or(0, Vec::len),
                value
            ),
            value,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn search_media(&self, args: &MediaSearchArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let query = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let speaker = args
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|speaker| !speaker.is_empty())
            .map(str::to_lowercase);
        let limit = args.limit.unwrap_or(25).clamp(1, 100);
        let mut matches = Vec::new();

        for asset in &document.media_pool {
            if args.kind.is_some_and(|kind| kind != asset.kind)
                || args
                    .min_width
                    .is_some_and(|minimum| asset.resolution.is_none_or(|value| value.0 < minimum))
                || args
                    .min_height
                    .is_some_and(|minimum| asset.resolution.is_none_or(|value| value.1 < minimum))
                || args
                    .min_duration_frames
                    .is_some_and(|minimum| asset.duration < minimum)
            {
                continue;
            }

            let transcript = match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some(transcript),
                _ => None,
            };
            if args
                .has_transcript
                .is_some_and(|required| required != transcript.is_some())
            {
                continue;
            }
            let scene_count = match self.analysis.scene_status(asset) {
                SceneStatus::Ready(scenes) => scenes.changes.len(),
                _ => 0,
            };
            if args
                .min_scene_count
                .is_some_and(|minimum| scene_count < minimum)
            {
                continue;
            }
            let beat_count = match self.analysis.beat_status(asset) {
                BeatStatus::Ready(beats) => beats.beats.len(),
                _ => 0,
            };
            if args
                .min_beat_count
                .is_some_and(|minimum| beat_count < minimum)
            {
                continue;
            }

            let speaker_labels = transcript
                .as_ref()
                .into_iter()
                .flat_map(|transcript| &transcript.words)
                .filter_map(|word| word.speaker.as_deref())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if let Some(speaker) = speaker.as_deref()
                && !speaker_labels
                    .iter()
                    .any(|label| label.to_lowercase() == speaker)
            {
                continue;
            }

            let name_match = query.as_ref().is_some_and(|query| {
                asset.name.to_lowercase().contains(query)
                    || asset.path.to_string_lossy().to_lowercase().contains(query)
            });
            let matching_words = transcript
                .as_ref()
                .into_iter()
                .flat_map(|transcript| &transcript.words)
                .filter(|word| {
                    query.as_ref().is_some_and(|query| {
                        word.text.to_lowercase().contains(query)
                            || word
                                .speaker
                                .as_ref()
                                .is_some_and(|speaker| speaker.to_lowercase().contains(query))
                    })
                })
                .collect::<Vec<_>>();
            if query.is_some() && !name_match && matching_words.is_empty() {
                continue;
            }
            let score = usize::from(name_match) * 100 + matching_words.len().min(99);
            let word_matches = matching_words
                .into_iter()
                .take(12)
                .map(|word| {
                    serde_json::json!({
                        "text": word.text,
                        "speaker": word.speaker,
                        "source_start": word.source_start.0,
                        "source_end": word.source_end.0,
                    })
                })
                .collect::<Vec<_>>();
            matches.push((
                score,
                asset.id,
                serde_json::json!({
                    "asset_id": asset.id.0,
                    "name": asset.name,
                    "path": asset.path,
                    "kind": asset.kind,
                    "duration": asset.duration.0,
                    "fps": {
                        "numerator": asset.fps.numerator(),
                        "denominator": asset.fps.denominator(),
                    },
                    "resolution": asset.resolution,
                    "score": score,
                    "word_matches": word_matches,
                    "speakers": speaker_labels,
                    "scene_count": scene_count,
                    "beat_count": beat_count,
                    "analysis_jobs": self.analysis.analysis_jobs(asset),
                }),
            ));
        }
        matches.sort_by_key(|(score, asset, _)| (std::cmp::Reverse(*score), *asset));
        let total_matches = matches.len();
        let hits = matches
            .into_iter()
            .take(limit)
            .map(|(_, _, hit)| hit)
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "query": args.query,
            "speaker": args.speaker,
            "total_matches": total_matches,
            "returned": hits.len(),
            "hits": hits,
        });
        Ok(success_structured(
            format!(
                "media search matched {} asset(s), returned {}\n{}",
                total_matches, value["returned"], value
            ),
            value,
        ))
    }

    fn asset_transcript(&self, asset_id: AssetId) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let mut status = self.analysis.transcript_status(asset);
        if status == TranscriptStatus::NotRequested {
            self.analysis.request_transcription(asset.clone());
            status = self.analysis.transcript_status(asset);
        }
        Ok(success_text(render_asset_transcript(asset_id, &status)))
    }

    fn asset_transcripts(&self, asset_ids: &[AssetId]) -> Result<CallToolResult, McpError> {
        if asset_ids.is_empty() || asset_ids.len() > 32 {
            return Ok(error_text("get_transcripts requires 1..=32 asset_ids"));
        }
        let unique = asset_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != asset_ids.len() {
            return Ok(error_text("get_transcripts asset_ids must be unique"));
        }
        let document = self.document()?;
        let mut rendered = Vec::with_capacity(asset_ids.len());
        for asset_id in asset_ids {
            let Some(asset) = document.asset(*asset_id) else {
                return Ok(error_text(format!("asset {asset_id} does not exist")));
            };
            let mut status = self.analysis.transcript_status(asset);
            if status == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
                status = self.analysis.transcript_status(asset);
            }
            rendered.push(render_asset_transcript(*asset_id, &status));
        }
        Ok(success_text(rendered.join("\n")))
    }

    fn asset_silences(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<TimeCode>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = requested_minimum.unwrap_or(TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("min_duration_frames must be positive"));
        }
        let mut status = self.analysis.silence_status(asset);
        if status == SilenceStatus::NotRequested {
            self.analysis.request_silence_detection(asset.clone());
            status = self.analysis.silence_status(asset);
        }
        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => Some(transcript),
            _ => None,
        };
        Ok(success_text(render_asset_silences(
            asset_id,
            &status,
            minimum,
            transcript.as_deref(),
        )))
    }

    fn asset_scene_changes(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = match requested_minimum {
            Some(value) => match confidence_to_basis_points(value) {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
        };
        let mut status = self.analysis.scene_status(asset);
        if status == SceneStatus::NotRequested {
            self.analysis.request_scene_detection(asset.clone());
            status = self.analysis.scene_status(asset);
        }
        Ok(success_text(render_asset_scene_changes(
            asset_id, &status, minimum,
        )))
    }

    fn asset_beats(
        &self,
        asset_id: AssetId,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let minimum = match requested_minimum {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let mut status = self.analysis.beat_status(asset);
        if status == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(asset.clone());
            status = self.analysis.beat_status(asset);
        }
        Ok(render_asset_beats(asset_id, &status, minimum))
    }

    fn analysis_status(&self, asset_id: AssetId) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let jobs = self.analysis.analysis_jobs(asset);
        Ok(success_structured(
            format!(
                "asset {asset_id} analysis jobs {}",
                serde_json::to_string(&jobs).unwrap_or_else(|_| "[]".to_owned())
            ),
            serde_json::json!({"asset_id": asset_id.0, "jobs": jobs}),
        ))
    }

    fn request_analysis(
        &self,
        asset_id: AssetId,
        requested: &[AnalysisKind],
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id).cloned() else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let kinds = if requested.is_empty() {
            AnalysisKind::ALL.as_slice()
        } else {
            requested
        };
        for kind in kinds {
            match kind {
                AnalysisKind::Transcript => self.analysis.request_transcription(asset.clone()),
                AnalysisKind::Silence => self.analysis.request_silence_detection(asset.clone()),
                AnalysisKind::Scene => self.analysis.request_scene_detection(asset.clone()),
                AnalysisKind::Beat => self.analysis.request_beat_detection(asset.clone()),
            }
        }
        self.analysis_status(asset_id)
    }

    fn cancel_analysis(
        &self,
        asset_id: AssetId,
        kind: AnalysisKind,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let cancelled = self.analysis.cancel_analysis(asset, kind);
        let jobs = self.analysis.analysis_jobs(asset);
        Ok(success_structured(
            format!("asset {asset_id} analysis kind={kind:?} cancelled={cancelled}"),
            serde_json::json!({
                "asset_id": asset_id.0,
                "kind": kind,
                "cancelled": cancelled,
                "jobs": jobs,
            }),
        ))
    }

    fn timeline_transcript(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = requested.map_or(TimeCode::ZERO..document.duration, |range| {
            range.start..range.end
        });
        if range.start < TimeCode::ZERO || range.end <= range.start || range.end > document.duration
        {
            return Ok(error_text(format!(
                "timeline transcript range {}..{} is outside project range 0..{}",
                range.start.0, range.end.0, document.duration.0
            )));
        }
        for asset in &document.media_pool {
            if self.analysis.transcript_status(asset) == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
            }
        }
        let words: Vec<TimelineTranscriptWord> = match self
            .analysis
            .timeline_transcript(&document, Some(range.clone()))
        {
            Ok(words) => words,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_transcript(&document, range, &words);
        for asset in &document.media_pool {
            let status = self.analysis.transcript_status(asset);
            if !matches!(
                status,
                TranscriptStatus::Ready(_) | TranscriptStatus::NoSpeech
            ) {
                rendered.push('\n');
                rendered.push_str(&render_asset_transcript(asset.id, &status));
            }
        }
        Ok(success_text(rendered))
    }

    fn dialogue_pacing(&self, args: &DialoguePacingArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(
            &document,
            args.range.as_ref().map(|range| TranscriptRangeArgs {
                start: range.start,
                end: range.end,
            }),
            "dialogue pacing",
        )?;
        let minimum = args.minimum_pause_frames.unwrap_or(TimeCode(10));
        let maximum = args.maximum_pause_frames.unwrap_or(TimeCode(40));
        let capitalization_minimum = args
            .capitalization_boundary_minimum_frames
            .unwrap_or(TimeCode(4));
        if minimum < TimeCode::ZERO || maximum < minimum || capitalization_minimum < TimeCode::ZERO
        {
            return Ok(error_text(
                "dialogue pacing requires 0 <= minimum_pause_frames <= maximum_pause_frames and a non-negative capitalization boundary minimum",
            ));
        }
        let referenced_assets = document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| clip.content.is_media())
            .map(|clip| clip.asset)
            .collect::<BTreeSet<_>>();
        for asset in document
            .media_pool
            .iter()
            .filter(|asset| referenced_assets.contains(&asset.id))
        {
            if self.analysis.transcript_status(asset) == TranscriptStatus::NotRequested {
                self.analysis.request_transcription(asset.clone());
            }
            if self.analysis.silence_status(asset) == SilenceStatus::NotRequested {
                self.analysis.request_silence_detection(asset.clone());
            }
        }
        let words = match self
            .analysis
            .timeline_transcript(&document, Some(range.clone()))
        {
            Ok(words) => dedup_timeline_words(words),
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let silences =
            match self
                .analysis
                .timeline_silences(&document, Some(range.clone()), TimeCode(1))
            {
                Ok(silences) => silences,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let pending_acoustic_assets = document
            .media_pool
            .iter()
            .filter(|asset| referenced_assets.contains(&asset.id))
            .filter(|asset| {
                !matches!(
                    self.analysis.silence_status(asset),
                    SilenceStatus::Ready(_) | SilenceStatus::NoAudio
                )
            })
            .map(|asset| asset.id.0)
            .collect::<Vec<_>>();
        let pacing =
            dialogue_pacing_gaps(&words, &silences, minimum, maximum, capitalization_minimum);
        Ok(dialogue_pacing_result(
            range,
            minimum,
            maximum,
            capitalization_minimum,
            &pacing,
            &pending_acoustic_assets,
        ))
    }

    fn timeline_silences(
        &self,
        requested: Option<TranscriptRangeArgs>,
        requested_minimum: Option<TimeCode>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline silence")?;
        let minimum = requested_minimum.unwrap_or(TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("min_duration_frames must be positive"));
        }
        for asset in &document.media_pool {
            if self.analysis.silence_status(asset) == SilenceStatus::NotRequested {
                self.analysis.request_silence_detection(asset.clone());
            }
        }
        let transcripts = document
            .media_pool
            .iter()
            .filter_map(|asset| match self.analysis.transcript_status(asset) {
                TranscriptStatus::Ready(transcript) => Some((asset.id, transcript)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let spans: Vec<TimelineSilenceSpan> =
            match self
                .analysis
                .timeline_silences(&document, Some(range.clone()), minimum)
            {
                Ok(spans) => spans,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let mut rendered =
            render_timeline_silences(&document, range, &spans, &transcripts, minimum);
        for asset in &document.media_pool {
            let status = self.analysis.silence_status(asset);
            if !matches!(status, SilenceStatus::Ready(_) | SilenceStatus::NoAudio) {
                rendered.push('\n');
                rendered.push_str(&render_asset_silences(
                    asset.id,
                    &status,
                    minimum,
                    transcripts.get(&asset.id).map(Arc::as_ref),
                ));
            }
        }
        Ok(success_text(rendered))
    }

    fn timeline_scene_changes(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline scene")?;
        for asset in &document.media_pool {
            if self.analysis.scene_status(asset) == SceneStatus::NotRequested {
                self.analysis.request_scene_detection(asset.clone());
            }
        }
        let changes: Vec<TimelineSceneChange> = match self.analysis.timeline_scene_changes(
            &document,
            Some(range.clone()),
            DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
        ) {
            Ok(changes) => changes,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_scene_changes(&document, range, &changes);
        for asset in &document.media_pool {
            let status = self.analysis.scene_status(asset);
            if !matches!(status, SceneStatus::Ready(_) | SceneStatus::NoVideo) {
                rendered.push('\n');
                rendered.push_str(&render_asset_scene_changes(
                    asset.id,
                    &status,
                    DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
                ));
            }
        }
        Ok(success_text(rendered))
    }

    fn timeline_beats(
        &self,
        requested: Option<TranscriptRangeArgs>,
        requested_minimum: Option<f64>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline beat")?;
        let minimum = match requested_minimum {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        for asset in &document.media_pool {
            if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
                self.analysis.request_beat_detection(asset.clone());
            }
        }
        let beats: Vec<TimelineBeat> =
            match self
                .analysis
                .timeline_beats(&document, Some(range.clone()), minimum)
            {
                Ok(beats) => beats,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let pending = document
            .media_pool
            .iter()
            .filter(|asset| {
                !matches!(
                    self.analysis.beat_status(asset),
                    BeatStatus::Ready(_) | BeatStatus::NoAudio
                )
            })
            .map(|asset| asset.id.0)
            .collect::<Vec<_>>();
        Ok(success_structured(
            render_timeline_beats(&document, &range, &beats, &pending),
            serde_json::json!({
                "range": {"start": range.start.0, "end": range.end.0},
                "minimum_strength_basis_points": minimum,
                "beats": beats,
                "pending_asset_ids": pending,
            }),
        ))
    }

    /// Return a compact, read-only heuristic hypothesis about one music
    /// asset's beat/bar/phrase structure. The analysis is deliberately kept
    /// separate from edit planning: it produces no operations and never
    /// changes the document or prepared-plan store.
    #[allow(clippy::too_many_lines)]
    fn music_structure(&self, args: &MusicStructureArgs) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(music_asset) = document.asset(args.music_asset_id) else {
            return Ok(error_text(format!(
                "music asset {} does not exist",
                args.music_asset_id
            )));
        };
        if !music_asset.kind.supports(TrackKind::Audio) {
            return Ok(error_text(format!(
                "music asset {} does not contain audio",
                args.music_asset_id
            )));
        }
        let requested_range = args.range.as_ref().map(|range| TranscriptRangeArgs {
            start: range.start,
            end: range.end,
        });
        let range = validated_timeline_range(&document, requested_range, "music structure")?;
        if !timeline_contains_asset(&document, args.music_asset_id, &range) {
            return Ok(error_text(format!(
                "music asset {} is not present on an audio-capable timeline clip overlapping project range {}..{}",
                args.music_asset_id, range.start, range.end
            )));
        }
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let meter_beats = args
            .meter_beats
            .unwrap_or(MUSIC_STRUCTURE_DEFAULT_METER_BEATS);
        let phrase_bars = args
            .phrase_bars
            .unwrap_or(MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS);

        let mut status = self.analysis.beat_status(music_asset);
        if status == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(music_asset.clone());
            status = self.analysis.beat_status(music_asset);
        }
        let analysis_state = beat_montage_analysis_state(args.music_asset_id, &status);
        let beats =
            match self
                .analysis
                .timeline_beats(&document, Some(range.clone()), minimum_strength)
            {
                Ok(beats) => beats,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let analysis = match music_structure_analysis(
            &document,
            args.music_asset_id,
            range,
            &beats,
            &analysis_state,
            minimum_strength,
            meter_beats,
            phrase_bars,
        ) {
            Ok(analysis) => analysis,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let total_candidate_count = analysis.candidates.len();
        let omitted_ordinary_candidate_count = if args.structural_only {
            analysis
                .candidates
                .iter()
                .filter(|candidate| candidate.role == kinewright_core::MusicStructureRole::Beat)
                .count()
        } else {
            0
        };
        let candidates = if args.structural_only {
            analysis
                .candidates
                .iter()
                .copied()
                .filter(|candidate| candidate.role != kinewright_core::MusicStructureRole::Beat)
                .collect::<Vec<_>>()
        } else {
            analysis.candidates.clone()
        };
        let returned_candidate_count = candidates.len();
        let structured = serde_json::json!({
            "music_asset_id": analysis.music_asset.0,
            "range": {
                "start": analysis.timeline_range.start.0,
                "end": analysis.timeline_range.end.0,
            },
            "minimum_strength_basis_points": analysis.minimum_strength_basis_points,
            "analysis_status": "ready",
            "timeline_audio_asset_present": true,
            "heuristic": true,
            "structural_only": args.structural_only,
            "total_candidate_count": total_candidate_count,
            "returned_candidate_count": returned_candidate_count,
            "omitted_ordinary_candidate_count": omitted_ordinary_candidate_count,
            "disclaimer": "Heuristic candidates, not guaranteed music theory; validate the musical result by listening before using them to drive cuts.",
            "parameters": analysis.parameters,
            "candidates": candidates,
        });
        Ok(success_structured(
            format!(
                "heuristic music structure for asset {} in {}..{}: {} candidate onsets returned ({} total; {} ordinary omitted), inferred meter {} and phrase length {} bars; structural_only={}; candidates are not guaranteed music theory",
                analysis.music_asset,
                analysis.timeline_range.start,
                analysis.timeline_range.end,
                returned_candidate_count,
                total_candidate_count,
                omitted_ordinary_candidate_count,
                analysis.parameters.meter_beats,
                analysis.parameters.phrase_bars,
                args.structural_only,
            ),
            structured,
        ))
    }

    fn plan_dialogue_assembly(
        &self,
        args: &DialogueAssemblyPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Err(error) = validate_dialogue_assembly_assets(args) {
            return Ok(error_text(error));
        }
        let target_track = args.target_track_id;
        if document.tracks.iter().all(|track| track.id != target_track) {
            return Ok(error_text(format!(
                "target track {target_track} does not exist"
            )));
        }
        let minimum = args.minimum_silence_source_frames.unwrap_or(TimeCode(20));
        if minimum <= TimeCode::ZERO {
            return Ok(error_text("minimum_silence_source_frames must be positive"));
        }
        let remove_fillers = args.remove_fillers.unwrap_or(true);
        let pacing = match dialogue_pacing_settings(args) {
            Ok(settings) => settings,
            Err(error) => return Ok(error_text(error)),
        };
        let mut at = args.timeline_start.unwrap_or(TimeCode::ZERO);
        if at < TimeCode::ZERO {
            return Ok(error_text("timeline_start must be non-negative"));
        }
        let mut operations = Vec::new();
        let mut selections = Vec::new();
        for (index, asset_id) in args.asset_ids.iter().enumerate() {
            let Some(asset) = document.asset(*asset_id).cloned() else {
                return Ok(error_text(format!("asset {asset_id} does not exist")));
            };
            let source_range = match dialogue_source_range(args, index, &asset) {
                Ok(range) => range,
                Err(error) => return Ok(error_text(error)),
            };
            let (transcript, silences) = match self.ready_dialogue_analysis(&asset, minimum) {
                Ok(analysis) => analysis,
                Err(result) => return Ok(result),
            };
            let ranges = dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                minimum,
                remove_fillers,
                pacing,
                source_range,
            );
            if ranges.is_empty() {
                return Ok(error_text(format!(
                    "asset {asset_id} has no source frames left after dialogue cleanup"
                )));
            }
            for source in &ranges {
                operations.push(Operation::AddClip {
                    track: target_track,
                    asset: *asset_id,
                    at,
                    source: source.clone(),
                });
                let duration = map_source_range_to_project(source.clone(), asset.fps, document.fps)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                at = at.checked_add(duration).ok_or_else(|| {
                    McpError::internal_error("dialogue assembly overflowed", None)
                })?;
            }
            let selection = dialogue_selection(&ranges, &transcript, &silences, pacing, minimum);
            selections.push(selection);
        }

        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "dialogue assembly does not fit the current target track: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "retained_pause_source_frames": pacing.retained_pause,
            "filler_padding_source_frames": pacing.filler_padding,
            "maximum_filler_bridge_pause_source_frames": pacing.maximum_filler_bridge_pause,
            "selections": selections,
            "resulting_range": {
                "start": args.timeline_start.unwrap_or(TimeCode::ZERO),
                "end": at,
            },
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} gapless dialogue clip(s) from {} ordered asset(s) as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.preview.operation_count,
                args.asset_ids.len(),
                plan.id,
            ),
            structured,
        ))
    }

    fn prepare_operations(
        &self,
        revision: TimelineRevision,
        document: &Document,
        operations: Vec<Operation>,
    ) -> Result<PreparedEditPlan, String> {
        self.prepared_plans
            .lock()
            .map_err(|_| "prepared plan store stopped".to_owned())?
            .prepare_operations(revision, revision, document, operations)
            .map_err(|error| error.to_string())
    }

    fn ready_dialogue_analysis(
        &self,
        asset: &MediaAsset,
        minimum: TimeCode,
    ) -> Result<(Arc<AssetTranscript>, Arc<AssetSilences>), CallToolResult> {
        let transcript = match self.analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => transcript,
            status => {
                if status == TranscriptStatus::NotRequested {
                    self.analysis.request_transcription(asset.clone());
                }
                return Err(error_text(format!(
                    "asset {} transcript is not ready: {}",
                    asset.id,
                    render_asset_transcript(asset.id, &status)
                )));
            }
        };
        let silences = match self.analysis.silence_status(asset) {
            SilenceStatus::Ready(silences) => silences,
            status => {
                if status == SilenceStatus::NotRequested {
                    self.analysis.request_silence_detection(asset.clone());
                }
                return Err(error_text(format!(
                    "asset {} silence analysis is not ready: {}",
                    asset.id,
                    render_asset_silences(asset.id, &status, minimum, Some(transcript.as_ref()),)
                )));
            }
        };
        Ok((transcript, silences))
    }

    fn plan_beat_pacing(&self, args: BeatPacingPlanArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let range = args.range.map(|range| range.start..range.end);
        let referenced_assets = document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| clip.content.is_media())
            .map(|clip| clip.asset)
            .collect::<BTreeSet<_>>();
        let mut pending = Vec::new();
        let mut unavailable = Vec::new();
        for asset_id in &referenced_assets {
            let Some(asset) = document.asset(*asset_id) else {
                continue;
            };
            if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
                self.analysis.request_beat_detection(asset.clone());
            }
            match self.analysis.beat_status(asset) {
                BeatStatus::Ready(_) | BeatStatus::NoAudio => {}
                BeatStatus::Failed(reason) => {
                    unavailable.push((*asset_id, format!("failed: {reason}")));
                }
                BeatStatus::Cancelled => {
                    unavailable.push((*asset_id, "cancelled".to_owned()));
                }
                BeatStatus::NotRequested
                | BeatStatus::Queued
                | BeatStatus::Hashing
                | BeatStatus::Analyzing { .. } => pending.push(*asset_id),
            }
        }
        let analysis_state = if !unavailable.is_empty() {
            let reason = unavailable
                .iter()
                .map(|(asset, reason)| format!("asset {asset}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            TimelineBeatAnalysisState::Unavailable {
                asset_ids: unavailable.into_iter().map(|(asset, _)| asset).collect(),
                reason,
            }
        } else if pending.is_empty() {
            TimelineBeatAnalysisState::Ready
        } else {
            TimelineBeatAnalysisState::Pending { asset_ids: pending }
        };
        let beats = match self
            .analysis
            .timeline_beats(&document, range.clone(), minimum_strength)
        {
            Ok(beats) => beats,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let plan = match beat_pacing_plan(
            &document,
            args.clip_id,
            range,
            &beats,
            &analysis_state,
            minimum_strength,
            args.minimum_spacing_frames.unwrap_or(TimeCode(6)),
        ) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "beat pacing plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} beat-aligned split(s) for clip {} as edit plan {}; inspect the selected onsets and preview, then commit it at timeline revision {revision}",
                plan.operations.len(),
                plan.target_clip,
                prepared.id,
            ),
            structured,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn plan_beat_montage(&self, args: &BeatMontagePlanArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(music_asset) = document.asset(args.music_asset_id) else {
            return Ok(error_text(format!(
                "music asset {} does not exist",
                args.music_asset_id
            )));
        };
        if !music_asset.kind.supports(TrackKind::Audio) {
            return Ok(error_text(format!(
                "music asset {} does not contain audio",
                args.music_asset_id
            )));
        }
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        let range = args.timeline_range.start..args.timeline_range.end;
        let mut status = self.analysis.beat_status(music_asset);
        if status == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(music_asset.clone());
            status = self.analysis.beat_status(music_asset);
        }
        let analysis_state = beat_montage_analysis_state(args.music_asset_id, &status);
        let beats =
            match self
                .analysis
                .timeline_beats(&document, Some(range.clone()), minimum_strength)
            {
                Ok(beats) => beats,
                Err(error) => return Ok(error_text(error.to_string())),
            };
        let selects = args
            .selects
            .iter()
            .map(|select| BeatMontageSelect {
                asset: select.asset_id,
                source_range: select.source_range.start..select.source_range.end,
            })
            .collect::<Vec<_>>();
        let minimum_shot_frames = args.minimum_shot_frames.unwrap_or(TimeCode(20));
        let maximum_shot_frames = args.maximum_shot_frames.unwrap_or(TimeCode(120));
        let (plan, anchor_repair) = match (
            args.cut_anchor_frames.as_deref(),
            args.anchor_repair.as_ref(),
        ) {
            (Some(preferred_anchors), Some(settings)) => {
                if settings.maximum_movement_frames < TimeCode::ZERO {
                    return Ok(error_text(
                        "anchor_repair.maximum_movement_frames must be non-negative; repair is always bounded and never silently broadened",
                    ));
                }
                if settings
                    .locked_anchor_indices
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                {
                    return Ok(error_text(
                        "anchor_repair.locked_anchor_indices must be strictly increasing and unique",
                    ));
                }
                let (plan, report) = match beat_montage_plan_near_anchors_with_report(
                    &document,
                    args.target_track_id,
                    args.music_asset_id,
                    range,
                    &selects,
                    preferred_anchors,
                    &beats,
                    &analysis_state,
                    minimum_strength,
                    minimum_shot_frames,
                    maximum_shot_frames,
                    args.mode,
                    Some(settings.maximum_movement_frames),
                    &settings.locked_anchor_indices,
                    args.cadence,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        return Ok(error_text(format!(
                            "beat montage anchor repair could not satisfy preferred anchors within maximum_movement_frames={}: {error}; revise preferred anchors, increase the explicit bound, unlock an anchor, or adjust source envelopes and retry",
                            settings.maximum_movement_frames
                        )));
                    }
                };
                let repaired = report.signed_deltas.iter().any(|delta| *delta != 0);
                let evidence = serde_json::json!({
                    "repaired": repaired,
                    "preferred_anchor_frames": report.preferred_anchors,
                    "resolved_anchor_frames": report.resolved_anchors,
                    "signed_delta_frames": report.signed_deltas,
                    "absolute_delta_frames": report.absolute_deltas,
                    "maximum_absolute_delta_frames": report.maximum_absolute_delta,
                    "total_absolute_delta_frames": report.total_absolute_delta,
                    "maximum_movement_frames": settings.maximum_movement_frames,
                    "locked_anchor_indices": settings.locked_anchor_indices,
                });
                (plan, Some(evidence))
            }
            (Some(cut_anchor_frames), None) => {
                match beat_montage_plan_with_anchors(
                    &document,
                    args.target_track_id,
                    args.music_asset_id,
                    range,
                    &selects,
                    cut_anchor_frames,
                    &beats,
                    &analysis_state,
                    minimum_strength,
                    minimum_shot_frames,
                    maximum_shot_frames,
                    args.mode,
                ) {
                    Ok(plan) => (plan, None),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
            }
            (None, Some(_)) => {
                return Ok(error_text(
                    "anchor_repair requires explicit cut_anchor_frames; supply exactly one fewer preferred anchor than selects or omit anchor_repair",
                ));
            }
            (None, None) => match beat_montage_plan(
                &document,
                args.target_track_id,
                args.music_asset_id,
                range,
                &selects,
                &beats,
                &analysis_state,
                minimum_strength,
                minimum_shot_frames,
                maximum_shot_frames,
                args.mode,
            ) {
                Ok(plan) => (plan, None),
                Err(error) => return Ok(error_text(error.to_string())),
            },
        };
        let cadence_summary = match args.cadence {
            Some(contract) => match validate_beat_montage_plan_cadence(&plan, contract) {
                Ok(summary) => Some(summary),
                Err(error) => {
                    return Ok(error_text(format!(
                        "beat montage cadence contract rejected prepared plan: {error}; revise shot durations or cut anchors and retry"
                    )));
                }
            },
            None => None,
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "beat montage plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "cadence": cadence_summary,
            "anchor_repair": anchor_repair,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} model-ordered, source-feasible hard-cut montage shot(s) against music asset {} as edit plan {}; inspect the resolved beat anchors, optional anchor_repair evidence, and preview before committing it at timeline revision {revision}; no transition or retime was added",
                plan.shots.len(),
                plan.music_asset,
                prepared.id,
            ),
            structured,
        ))
    }

    fn plan_music_fit(&self, args: &MusicFitPlanArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(asset) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };
        let minimum_strength = match args.min_strength {
            Some(value) => match percentage_to_basis_points(value, "min_strength") {
                Ok(value) => value,
                Err(error) => return Ok(error_text(error)),
            },
            None => DEFAULT_BEAT_STRENGTH_BASIS_POINTS,
        };
        if self.analysis.beat_status(asset) == BeatStatus::NotRequested {
            self.analysis.request_beat_detection(asset.clone());
        }
        let status = self.analysis.beat_status(asset);
        let end_anchor = match (args.preferred_source_end, args.maximum_end_drift_frames) {
            (None, None) => None,
            (Some(preferred_source_end), Some(maximum_drift_frames)) => {
                Some(kinewright_core::MusicEndAnchor {
                    preferred_source_end,
                    maximum_drift_frames,
                })
            }
            (Some(_), None) => {
                return Ok(error_text(
                    "preferred_source_end requires maximum_end_drift_frames; end targeting is always explicitly bounded",
                ));
            }
            (None, Some(_)) => {
                return Ok(error_text(
                    "maximum_end_drift_frames requires preferred_source_end; end targeting is never inferred",
                ));
            }
        };
        let plan = match music_fit_plan_with_end_anchor(
            &document,
            args.track_id,
            args.asset_id,
            args.timeline_range.start..args.timeline_range.end,
            args.preferred_source_start,
            end_anchor,
            &status,
            minimum_strength,
            args.mode,
        ) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "music fit plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {:?} real-time music edit from source frames {}..{} into project frames {}..{} as edit plan {}; inspect the endpoint evidence and preview, then commit it at timeline revision {revision}; no looping or hidden time stretch was used",
                plan.strategy,
                plan.source_range.start.0,
                plan.source_range.end.0,
                plan.timeline_range.start.0,
                plan.timeline_range.end.0,
                prepared.id,
            ),
            structured,
        ))
    }

    fn plan_speaker_multicam(
        &self,
        args: SpeakerMulticamPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let Some(reference_asset) = document.asset(args.reference_asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.reference_asset_id
            )));
        };
        let mut transcript_status = self.analysis.transcript_status(reference_asset);
        if transcript_status == TranscriptStatus::NotRequested {
            self.analysis.request_transcription(reference_asset.clone());
            transcript_status = self.analysis.transcript_status(reference_asset);
        }
        let TranscriptStatus::Ready(transcript) = transcript_status else {
            return Ok(error_text(format!(
                "speaker-aware multicam requires a ready diarized transcript for asset {}; current analysis state: {}",
                args.reference_asset_id,
                render_asset_transcript(args.reference_asset_id, &transcript_status),
            )));
        };
        let settings = SpeakerMulticamSettings {
            sync_group: args.sync_group_id,
            target_track: args.target_track_id,
            group_start: args.group_range.start,
            group_end: args.group_range.end,
            record_start: args.record_start,
            maximum_word_gap_frames: args.maximum_word_gap_frames.unwrap_or(TimeCode(3)),
            minimum_shot_frames: args.minimum_shot_frames.unwrap_or(TimeCode(5)),
            assignments: args.assignments,
        };
        let plan = match plan_speaker_multicam(&document, &transcript, &settings) {
            Ok(plan) => plan,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let prepared = match self.prepare_operations(revision, &document, plan.operations.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "speaker multicam plan does not fit the current timeline: {error}"
                )));
            }
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "plan": plan,
            "prepared_edit_plan": {
                "plan_id": prepared.id,
                "expected_revision": revision,
                "preview": prepared.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared {} speaker-aware multicam shot(s) from transcript asset {} as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                plan.cuts.len(),
                plan.reference_asset,
                prepared.id,
            ),
            structured,
        ))
    }

    fn plan_audio_normalization(
        &self,
        args: &AudioNormalizationPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let context = match normalization_context(&document, args) {
            Ok(context) => context,
            Err(error) => return Ok(error_text(error)),
        };
        let current = match self.analysis.timeline_loudness(&document) {
            Ok(measurement) => measurement,
            Err(error) => {
                return Ok(error_text(format!(
                    "could not measure timeline audio: {error}"
                )));
            }
        };
        let (operation, predicted) = match verified_normalization_operation(
            self.analysis.as_ref(),
            &document,
            args,
            &context,
            current,
        ) {
            Ok(result) => result,
            Err(error) => return Ok(error_text(error)),
        };
        let prepared = match self.prepare_operations(revision, &document, vec![operation]) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Ok(error_text(format!(
                    "normalization plan does not fit the current timeline: {error}"
                )));
            }
        };
        let current_lufs = current.integrated_lufs_hundredths.unwrap_or_default();
        let predicted_lufs = predicted.integrated_lufs_hundredths.unwrap_or_default();
        Ok(success_structured(
            format!(
                "prepared measured audio normalization from {current_lufs} to {predicted_lufs} LUFS hundredths as edit plan {}; inspect the bus processing and preview, then commit it at timeline revision {revision}",
                prepared.id
            ),
            serde_json::json!({
                "timeline_revision": revision.0,
                "target_lufs_hundredths": args.target_lufs_hundredths,
                "maximum_sample_peak_dbfs_hundredths": args.maximum_sample_peak_dbfs_hundredths,
                "lossy_codec_peak_headroom_hundredths": LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS,
                "processing_ceiling_dbfs_hundredths": args.maximum_sample_peak_dbfs_hundredths
                    .saturating_sub(LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS),
                "tolerance_hundredths": args.tolerance_hundredths,
                "current": current,
                "predicted": predicted,
                "prepared_edit_plan": {
                    "plan_id": prepared.id,
                    "expected_revision": revision,
                    "preview": prepared.preview,
                },
            }),
        ))
    }
}

fn beat_montage_analysis_state(
    music_asset: AssetId,
    status: &BeatStatus,
) -> TimelineBeatAnalysisState {
    match status {
        BeatStatus::Ready(_) => TimelineBeatAnalysisState::Ready,
        BeatStatus::NoAudio => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: "music asset has no audio stream".to_owned(),
        },
        BeatStatus::Cancelled => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: "beat analysis was cancelled".to_owned(),
        },
        BeatStatus::Failed(reason) => TimelineBeatAnalysisState::Unavailable {
            asset_ids: vec![music_asset],
            reason: format!("beat analysis failed: {reason}"),
        },
        BeatStatus::NotRequested
        | BeatStatus::Queued
        | BeatStatus::Hashing
        | BeatStatus::Analyzing { .. } => TimelineBeatAnalysisState::Pending {
            asset_ids: vec![music_asset],
        },
    }
}

fn timeline_contains_asset(
    document: &Document,
    asset_id: AssetId,
    range: &std::ops::Range<TimeCode>,
) -> bool {
    document.tracks.iter().any(|track| {
        if track.kind != TrackKind::Audio {
            return false;
        }
        track.clips.iter().any(|clip| {
            if clip.asset != asset_id || !clip.content.is_media() {
                return false;
            }
            let Some(duration) = document.clip_duration(clip).ok() else {
                return false;
            };
            let Some(end) = clip.timeline_start.checked_add(duration) else {
                return false;
            };
            clip.timeline_start < range.end && end > range.start
        })
    })
}

struct NormalizationContext {
    tracks: Vec<TrackId>,
    bus_id: AudioBusId,
    first_effect_id: u64,
}

fn normalization_context(
    document: &Document,
    args: &AudioNormalizationPlanArgs,
) -> Result<NormalizationContext, String> {
    if args.track_ids.is_empty() {
        return Err("track_ids must contain at least one audio source track".to_owned());
    }
    let tracks = args.track_ids.iter().copied().collect::<BTreeSet<_>>();
    if tracks.len() != args.track_ids.len() {
        return Err("track_ids must not contain duplicates".to_owned());
    }
    for track in &tracks {
        let candidate = document
            .tracks
            .iter()
            .find(|candidate| candidate.id == *track)
            .ok_or_else(|| format!("track {track} does not exist"))?;
        if candidate.clips.is_empty() {
            return Err(format!("track {track} contains no audio source clips"));
        }
    }
    if let Some(bus) = document
        .audio_mix
        .buses
        .iter()
        .find(|bus| bus.tracks.iter().any(|track| tracks.contains(track)))
    {
        return Err(format!(
            "track selection already intersects audio bus {} ({}); remove or deliberately revise that mix before normalizing",
            bus.id, bus.name
        ));
    }
    if !(-2_400..=-900).contains(&args.target_lufs_hundredths) {
        return Err("target_lufs_hundredths must be in -2400..=-900".to_owned());
    }
    if !(-300..=0).contains(&args.maximum_sample_peak_dbfs_hundredths) {
        return Err("maximum_sample_peak_dbfs_hundredths must be in -300..=0".to_owned());
    }
    if !(25..=300).contains(&args.tolerance_hundredths) {
        return Err("tolerance_hundredths must be in 25..=300".to_owned());
    }
    let bus_id = AudioBusId(
        document
            .audio_mix
            .buses
            .iter()
            .map(|bus| bus.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    let first_effect_id = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .chain(document.audio_mix.buses.iter().flat_map(|bus| &bus.effects))
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Ok(NormalizationContext {
        tracks: tracks.into_iter().collect(),
        bus_id,
        first_effect_id,
    })
}

fn verified_normalization_operation(
    analysis: &dyn Analysis,
    document: &Document,
    args: &AudioNormalizationPlanArgs,
    context: &NormalizationContext,
    current: AudioLoudness,
) -> Result<(Operation, AudioLoudness), String> {
    let current_lufs = current.integrated_lufs_hundredths.ok_or_else(|| {
        "timeline audio is silent; normalization cannot infer a programme level".to_owned()
    })?;
    let current_peak = current
        .sample_peak_dbfs_hundredths
        .ok_or_else(|| "timeline audio has no measurable sample peak".to_owned())?;
    let mut requested_gain = args.target_lufs_hundredths.saturating_sub(current_lufs);
    let mut final_operation = None;
    let mut predicted = current;
    for _ in 0..4 {
        let processing_ceiling = args
            .maximum_sample_peak_dbfs_hundredths
            .saturating_sub(LOSSY_CODEC_PEAK_HEADROOM_HUNDREDTHS);
        let bus = normalization_bus(
            context.bus_id,
            context.first_effect_id,
            context.tracks.clone(),
            requested_gain,
            current_peak,
            processing_ceiling,
        )?;
        let operation = Operation::UpsertAudioBus { bus };
        let mut candidate = document.clone();
        apply_batch(&mut candidate, std::slice::from_ref(&operation))
            .map_err(|error| format!("normalization processing is not applicable: {error}"))?;
        predicted = analysis
            .timeline_loudness(&candidate)
            .map_err(|error| format!("could not verify normalized timeline audio: {error}"))?;
        let predicted_lufs = predicted
            .integrated_lufs_hundredths
            .ok_or_else(|| "normalization unexpectedly produced silent output".to_owned())?;
        final_operation = Some(operation);
        let correction = args.target_lufs_hundredths.saturating_sub(predicted_lufs);
        if correction.unsigned_abs() <= u32::from(args.tolerance_hundredths) {
            break;
        }
        requested_gain = requested_gain.saturating_add(correction);
    }
    let predicted_lufs = predicted
        .integrated_lufs_hundredths
        .ok_or_else(|| "normalized loudness measurement disappeared".to_owned())?;
    let predicted_peak = predicted
        .sample_peak_dbfs_hundredths
        .ok_or_else(|| "normalized peak measurement disappeared".to_owned())?;
    if predicted_lufs.abs_diff(args.target_lufs_hundredths) > u32::from(args.tolerance_hundredths)
        || predicted_peak > args.maximum_sample_peak_dbfs_hundredths
    {
        return Err(format!(
            "normalization could not satisfy the delivery contract: predicted_lufs_hundredths={predicted_lufs}, predicted_peak_dbfs_hundredths={predicted_peak}"
        ));
    }
    Ok((
        final_operation.expect("normalization produced an operation"),
        predicted,
    ))
}

fn round_hundredths_to_tenths(value: i32) -> i64 {
    i64::from(if value >= 0 {
        value.saturating_add(5) / 10
    } else {
        value.saturating_sub(5) / 10
    })
}

fn static_audio_effect(id: EffectId, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id,
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

fn normalization_bus(
    bus_id: AudioBusId,
    first_effect_id: u64,
    tracks: Vec<TrackId>,
    gain_hundredths: i32,
    measured_peak_hundredths: i32,
    ceiling_hundredths: i32,
) -> Result<AudioBus, String> {
    if !(-6_000..=3_600).contains(&gain_hundredths) {
        return Err(format!(
            "required normalization gain {gain_hundredths} hundredths dB exceeds the supported -6000..=3600 range"
        ));
    }
    let mut effects = Vec::new();
    let mut next_effect_id = first_effect_id;
    if gain_hundredths >= 0 {
        let makeup_hundredths = gain_hundredths.min(2_400);
        let post_gain_hundredths = gain_hundredths.saturating_sub(makeup_hundredths);
        let compression_required =
            measured_peak_hundredths.saturating_add(gain_hundredths) > ceiling_hundredths;
        let (threshold_tenth_db, ratio_hundredths) = if compression_required {
            let numerator = i64::from(ceiling_hundredths)
                .saturating_sub(i64::from(gain_hundredths))
                .saturating_sub(i64::from(measured_peak_hundredths).div_euclid(4));
            let threshold_hundredths = numerator.saturating_mul(4).div_euclid(3).clamp(-6_000, 0);
            (threshold_hundredths.div_euclid(10), 400)
        } else {
            (0, 100)
        };
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_compressor",
            &[
                ("threshold_tenth_db", threshold_tenth_db),
                ("ratio_hundredths", ratio_hundredths),
                ("attack_milliseconds", 5),
                ("release_milliseconds", 200),
                (
                    "makeup_gain_tenth_db",
                    round_hundredths_to_tenths(makeup_hundredths),
                ),
            ],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
        if post_gain_hundredths > 0 {
            effects.push(static_audio_effect(
                EffectId(next_effect_id),
                "audio_gain",
                &[(
                    "gain_tenth_db",
                    round_hundredths_to_tenths(post_gain_hundredths),
                )],
            ));
            next_effect_id = next_effect_id.saturating_add(1);
        }
    } else {
        effects.push(static_audio_effect(
            EffectId(next_effect_id),
            "audio_gain",
            &[("gain_tenth_db", round_hundredths_to_tenths(gain_hundredths))],
        ));
        next_effect_id = next_effect_id.saturating_add(1);
    }
    effects.push(static_audio_effect(
        EffectId(next_effect_id),
        "audio_limiter",
        &[(
            "ceiling_tenth_db",
            i64::from(ceiling_hundredths).div_euclid(10),
        )],
    ));
    Ok(AudioBus {
        id: bus_id,
        name: "Delivery normalization".to_owned(),
        tracks,
        effects,
        ducking_sidechain_tracks: Vec::new(),
    })
}

impl ServerHandler for KinewrightMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kinewright", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Inspect with get_timeline_state. Open names already in the user request with one batched get_capability call; search only unnamed needs or after a miss. Load only needed schemas. Invoke capabilities through invoke_capability. When a planner returns prepared_edit_plan, inspect its preview and commit that plan id directly. Use prepare_edit_plan only for model-authored operations. Reinspect after revision conflicts. Frames are exact project integers.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let result = Self::served_tools()
            .map(ListToolsResult::with_all_items)
            .map_err(|error| McpError::internal_error(error.to_string(), None));
        std::future::ready(result)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::served_tools()
            .ok()?
            .into_iter()
            .find(|tool| tool.name == name)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
        let service = self.clone();
        async move {
            tokio::task::spawn_blocking(move || service.call_exposed_blocking(request))
                .await
                .map_err(|error| McpError::internal_error(error.to_string(), None))?
                .map(Into::into)
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CapabilitySearchArgs {
    /// One optional case-insensitive query matched against names and one-line summaries.
    #[serde(default)]
    query: Option<String>,
    /// Additional independent queries to run in the same call. Results are de-duplicated.
    #[serde(default)]
    queries: Vec<String>,
    /// Optional capability kinds. Omit or send an empty list to search every kind.
    #[serde(default)]
    kinds: Vec<CapabilityKind>,
    /// Maximum combined results. Defaults to 12 and is clamped to 1..=100.
    #[serde(default)]
    limit: Option<u8>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CapabilityArgs {
    /// Exact name from the user request or search results; no search required.
    #[serde(default)]
    name: Option<String>,
    /// Additional exact capability names to open in this same call.
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct InvokeCapabilityArgs {
    /// Exact non-edit capability name opened with `get_capability`.
    name: String,
    /// Arguments matching the schema returned by `get_capability`.
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PrepareEditPlanArgs {
    /// Exact revision returned by `get_timeline_state` before planning.
    expected_revision: TimelineRevision,
    /// Ordered compact operations such as `{"op":"split_clip","clip":1,"at":30}`.
    operations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CommitEditPlanArgs {
    /// Opaque server-local id returned by `prepare_edit_plan`.
    plan_id: PreparedPlanId,
    /// The same exact revision used to prepare the plan.
    expected_revision: TimelineRevision,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiscardEditPlanArgs {
    /// Opaque server-local id returned by `prepare_edit_plan`.
    plan_id: PreparedPlanId,
}

fn search_capability_queries(
    tools: &[Tool],
    args: &CapabilitySearchArgs,
) -> Vec<CapabilityDescriptor> {
    let limit = usize::from(args.limit.unwrap_or(12)).clamp(1, 100);
    let queries = args
        .query
        .iter()
        .chain(&args.queries)
        .map(String::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return search_capabilities(tools, None, &args.kinds, limit);
    }
    let mut found = Vec::new();
    let mut names = BTreeSet::new();
    for query in queries {
        for capability in search_capabilities(tools, Some(query), &args.kinds, 100) {
            if names.insert(capability.name.clone()) {
                found.push(capability);
                if found.len() == limit {
                    return found;
                }
            }
        }
    }
    found
}

fn open_capabilities(tools: &[Tool], args: CapabilityArgs) -> CallToolResult {
    let mut requested = Vec::new();
    if let Some(name) = args.name {
        requested.push(name);
    }
    requested.extend(args.names);
    let mut seen = BTreeSet::new();
    requested = requested
        .into_iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty() && seen.insert(name.clone()))
        .collect();
    if requested.is_empty() {
        return error_text("get_capability requires name or names");
    }
    if requested.len() > 16 {
        return error_text("get_capability accepts at most 16 names per call");
    }

    let descriptors = capabilities(tools)
        .into_iter()
        .map(|descriptor| (descriptor.name.clone(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let unknown = requested
        .iter()
        .filter(|name| !descriptors.contains_key(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return error_text(format!(
            "unknown Kinewright capabilities: {}",
            unknown.join(", ")
        ));
    }

    let opened = requested
        .iter()
        .map(|name| {
            let descriptor = &descriptors[name];
            let tool = tools
                .iter()
                .find(|tool| tool.name == name.as_str())
                .expect("a descriptor is built from a matching tool");
            serde_json::json!({
                "capability": descriptor,
                "input_schema": tool.input_schema,
                "invocation": if is_invocable_capability(name) {
                    "invoke_capability"
                } else {
                    "prepare_edit_plan"
                }
            })
        })
        .collect::<Vec<_>>();
    if opened.len() == 1 {
        return success_structured(
            format!("opened capability {}", requested[0]),
            opened
                .into_iter()
                .next()
                .expect("one capability was opened"),
        );
    }
    success_structured(
        format!("opened {} capabilities in one batch", opened.len()),
        serde_json::json!({"capabilities": opened}),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClipInfoArgs {
    /// Stable clip id shown by `get_timeline_state`.
    clip_id: ClipId,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FrameAtArgs {
    /// Exact project frame to render.
    timecode: TimeCode,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VideoScopesArgs {
    /// Exact project frame to measure after all compositing and effects.
    timecode: TimeCode,
    /// Histogram bin count. Defaults to 64 and is clamped to 16..=128.
    #[serde(default)]
    bins: Option<u8>,
    /// Maximum compositor width. Defaults to 512 and is clamped to 32..=1024.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TrackMaskArgs {
    /// Stable media clip id containing the mask effect.
    clip_id: ClipId,
    /// Stable mask effect id on the clip.
    effect_id: EffectId,
    /// First clip-local frame to track. Defaults to zero.
    #[serde(default)]
    start_local_frame: Option<TimeCode>,
    /// Exclusive clip-local end frame. Defaults to the clip duration.
    #[serde(default)]
    end_local_frame: Option<TimeCode>,
    /// Distance between tracked keyframes. Defaults to 5; valid range 1..=120.
    #[serde(default)]
    step_frames: Option<i64>,
    /// Search radius around the previous center as a frame percentage. Defaults to 10.
    #[serde(default)]
    search_radius_percent: Option<u8>,
    /// Analysis render width. Defaults to 256; valid range 64..=512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StoryboardArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Number of uniformly sampled cells. Defaults to 9 and is capped at 16.
    #[serde(default)]
    frame_count: Option<u8>,
    /// Maximum width of each rendered cell. Defaults to 320 and is capped at 512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeliveryStoryboardArgs {
    aspect: DeliveryAspect,
    #[serde(default = "default_delivery_focus")]
    focus_x_percent: u8,
    #[serde(default = "default_delivery_focus")]
    focus_y_percent: u8,
    #[serde(flatten)]
    storyboard: StoryboardArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditorialReadinessArgs {
    /// Stable delivery contract to verify and preview.
    profile: DeliveryProfile,
    /// Require transcript-safe silence clearance. Disable only when preserving
    /// continuous program audio or when dead-air editing is outside the brief.
    #[serde(default = "default_true")]
    check_silence: bool,
    /// Minimum transcript-safe cuttable silence in source frames. Defaults to 20.
    #[serde(default)]
    min_silence_source_frames: Option<TimeCode>,
    #[serde(default = "default_delivery_focus")]
    focus_x_percent: u8,
    #[serde(default = "default_delivery_focus")]
    focus_y_percent: u8,
    #[serde(flatten)]
    storyboard: StoryboardArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TrackReframeArgs {
    /// Stable media clip id containing the reframe effect.
    clip_id: ClipId,
    /// Stable reframe effect id on the clip.
    effect_id: EffectId,
    /// Width of the initial subject template as a frame percentage, in 1..=75.
    subject_width_percent: u8,
    /// Height of the initial subject template as a frame percentage, in 1..=75.
    subject_height_percent: u8,
    /// Explicit horizontal center of the subject template. Defaults to the effect focus at start.
    #[serde(default)]
    initial_subject_x_percent: Option<u8>,
    /// Explicit vertical center of the subject template. Defaults to the effect focus at start.
    #[serde(default)]
    initial_subject_y_percent: Option<u8>,
    /// Smallest editable horizontal focus emitted by the tracker. Defaults to zero.
    #[serde(default)]
    minimum_focus_x_percent: Option<u8>,
    /// Largest editable horizontal focus emitted by the tracker. Defaults to 100.
    #[serde(default)]
    maximum_focus_x_percent: Option<u8>,
    /// Smallest editable vertical focus emitted by the tracker. Defaults to zero.
    #[serde(default)]
    minimum_focus_y_percent: Option<u8>,
    /// Largest editable vertical focus emitted by the tracker. Defaults to 100.
    #[serde(default)]
    maximum_focus_y_percent: Option<u8>,
    /// Subject motion tolerated before the virtual camera follows. Defaults to 6%; valid range 0..=25.
    #[serde(default)]
    focus_dead_zone_percent: Option<u8>,
    /// Largest virtual-camera move between samples. Defaults to 2%; valid range 1..=25.
    #[serde(default)]
    maximum_focus_step_percent: Option<u8>,
    /// First clip-local frame to track. Defaults to zero.
    #[serde(default)]
    start_local_frame: Option<TimeCode>,
    /// Exclusive clip-local end frame. Defaults to the clip duration.
    #[serde(default)]
    end_local_frame: Option<TimeCode>,
    /// Distance between editable focus keyframes. Defaults to 5; valid range 1..=120.
    #[serde(default)]
    step_frames: Option<i64>,
    /// Search radius around the prior subject center as a frame percentage. Defaults to 10.
    #[serde(default)]
    search_radius_percent: Option<u8>,
    /// Analysis render width. Defaults to 256; valid range 64..=512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AudioNormalizationPlanArgs {
    /// Source tracks to route through one deterministic delivery bus.
    track_ids: Vec<TrackId>,
    /// Target integrated loudness in hundredths of LUFS. Defaults to -1600.
    #[serde(default = "default_target_lufs_hundredths")]
    target_lufs_hundredths: i32,
    /// Maximum decoded sample peak in hundredths of dBFS. Defaults to -100.
    #[serde(default = "default_maximum_sample_peak_dbfs_hundredths")]
    maximum_sample_peak_dbfs_hundredths: i32,
    /// Accepted measured loudness error in hundredths of LU. Defaults to 100.
    #[serde(default = "default_loudness_tolerance_hundredths")]
    tolerance_hundredths: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeliveryConformanceArgs {
    /// Stable delivery contract returned by `get_delivery_profiles`.
    profile: DeliveryProfile,
    /// Explicit horizontal focal point used when the profile changes aspect ratio.
    #[serde(default = "default_delivery_focus")]
    focus_x_percent: u8,
    /// Explicit vertical focal point used when the profile changes aspect ratio.
    #[serde(default = "default_delivery_focus")]
    focus_y_percent: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueueExportArgs {
    /// Exact branch revision whose immutable snapshot should be rendered.
    expected_revision: TimelineRevision,
    /// Destination media file. Parent directories must already exist.
    output_path: PathBuf,
    /// Stable delivery contract returned by `get_delivery_profiles`.
    profile: DeliveryProfile,
    #[serde(default = "default_delivery_focus")]
    focus_x_percent: u8,
    #[serde(default = "default_delivery_focus")]
    focus_y_percent: u8,
    /// Explicit permission to replace a regular destination file. Always requires human confirmation.
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportJobArgs {
    job_id: ExportJobId,
}

const fn default_delivery_focus() -> u8 {
    50
}

const fn default_true() -> bool {
    true
}

const fn default_target_lufs_hundredths() -> i32 {
    -1_600
}

const fn default_maximum_sample_peak_dbfs_hundredths() -> i32 {
    -100
}

const fn default_loudness_tolerance_hundredths() -> u16 {
    100
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ImportMediaArgs {
    /// Exact revision returned by `get_timeline_state` before planning this import.
    expected_revision: TimelineRevision,
    /// Absolute or working-directory-relative path on the user's machine.
    path: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SourceInfoArgs {
    /// Stable asset id shown by `get_timeline_state` or `search_media`.
    asset_id: AssetId,
    /// Optional source-monitor in mark in exact asset frames.
    #[serde(default)]
    source_in: Option<TimeCode>,
    /// Optional source-monitor out mark in exact asset frames.
    #[serde(default)]
    source_out: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SourceStoryboardArgs {
    /// Stable video asset id shown by `get_timeline_state`, `get_source_info`, or `search_media`.
    asset_id: AssetId,
    /// Optional half-open source-frame range. Omit for the full source asset.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Number of uniformly sampled cells. Defaults to 9 and is capped at 16.
    #[serde(default)]
    frame_count: Option<u8>,
    /// Maximum width of each rendered cell. Defaults to 320 and is capped at 512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ShotBoardCandidateSelection {
    /// Return a consecutive page of eligible candidates. This is the
    /// backward-compatible default and is the only mode that accepts an offset.
    #[default]
    Page,
    /// Sample eligible candidates across the full inspected range. For two or
    /// more returned candidates this always includes the first and last.
    Coverage,
}

impl ShotBoardCandidateSelection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Coverage => "coverage",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SourceShotBoardArgs {
    /// Stable video asset id shown by `get_timeline_state`, `get_source_info`, or `search_media`.
    asset_id: AssetId,
    /// Optional half-open source-frame range. Omit for the full source asset.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Candidate selection strategy. `page` (the default) returns a
    /// consecutive offset page. `coverage` deterministically spreads up to
    /// `candidate_count` eligible candidates across the complete source range;
    /// it cannot be combined with `candidate_offset`.
    #[serde(default)]
    candidate_selection: Option<ShotBoardCandidateSelection>,
    /// First eligible scene-derived candidate to return. Defaults to zero;
    /// this offset is applied after `minimum_duration_frames` filtering. Only
    /// valid with `candidate_selection: "page"`.
    #[serde(default)]
    candidate_offset: Option<usize>,
    /// Optional inclusive minimum scene duration in source frames. Candidates
    /// shorter than this are filtered before pagination; their original
    /// candidate index and id remain stable in the returned manifest.
    #[serde(default)]
    minimum_duration_frames: Option<TimeCode>,
    /// Minimum scene-boundary confidence in basis points (0..=10000).
    /// Defaults to 1000 (10%). Raise this for motion-heavy footage when weak
    /// frame differences over-segment a continuous shot.
    #[serde(default)]
    minimum_confidence_basis_points: Option<u16>,
    /// Number of scene-derived candidates to return. Defaults to 6 and is capped at 12.
    #[serde(default)]
    candidate_count: Option<u8>,
    /// Maximum width of each rendered evidence cell. Defaults to 320 and is capped at 512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MediaSearchArgs {
    /// Case-insensitive text matched against asset name, path, cached words,
    /// and cached speaker labels.
    #[serde(default)]
    query: Option<String>,
    /// Case-insensitive exact diarization label from cached transcript words.
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    kind: Option<MediaKind>,
    #[serde(default)]
    min_width: Option<u32>,
    #[serde(default)]
    min_height: Option<u32>,
    #[serde(default)]
    min_duration_frames: Option<TimeCode>,
    /// Require at least this many cached scene boundaries.
    #[serde(default)]
    min_scene_count: Option<usize>,
    /// Require at least this many cached beat onsets.
    #[serde(default)]
    min_beat_count: Option<usize>,
    /// Require a ready cached transcript when true, or no ready transcript when false.
    #[serde(default)]
    has_transcript: Option<bool>,
    /// Maximum hits to return. Defaults to 25 and is capped at 100.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranscriptArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SilencesArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
    /// Optional minimum silence duration in exact source frames. Defaults to 6.
    #[serde(default)]
    min_duration_frames: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SceneChangesArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
    /// Optional confidence threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_confidence: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BeatsArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TimelineBeatsArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MusicStructureArgs {
    /// Audio-capable media asset whose mapped timeline beats should be analyzed.
    music_asset_id: AssetId,
    /// Optional half-open project-frame range. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
    /// Optional meter hypothesis in beats per bar. Defaults to the core heuristic default.
    #[serde(default)]
    meter_beats: Option<u8>,
    /// Optional phrase hypothesis in bars. Defaults to the core heuristic default.
    #[serde(default)]
    phrase_bars: Option<u8>,
    /// Return only inferred bar and phrase candidates, omitting ordinary beat candidates.
    /// The structured response reports total, returned, and omitted candidate counts.
    #[serde(default)]
    structural_only: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BeatPacingPlanArgs {
    /// Existing media clip to split at the selected musical onsets.
    clip_id: ClipId,
    /// Optional half-open subrange in exact project frames. Defaults to the clip range.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
    /// Minimum distance between selected onsets in project frames. Defaults to 6.
    #[serde(default)]
    minimum_spacing_frames: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BeatMontageSelectArgs {
    /// Video-capable source asset selected by the editing model.
    asset_id: AssetId,
    /// Exact half-open source-frame envelope allowed for this shot.
    source_range: TranscriptRangeArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BeatMontageAnchorRepairArgs {
    /// Inclusive maximum project-frame movement allowed for every preferred anchor.
    /// Must be non-negative; the planner never broadens this bound.
    maximum_movement_frames: TimeCode,
    /// Optional strictly increasing zero-based preferred-anchor indices that must remain exact.
    #[serde(default)]
    locked_anchor_indices: Vec<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BeatMontagePlanArgs {
    /// Existing video track that receives the ordered hard-cut montage.
    target_track_id: TrackId,
    /// Audio-capable timeline asset whose mapped beats provide cut anchors.
    music_asset_id: AssetId,
    /// Exact half-open project range the montage must fill.
    timeline_range: TranscriptRangeArgs,
    /// Model-selected shots in final story order. The planner does not reorder or replace them.
    selects: Vec<BeatMontageSelectArgs>,
    /// Optional exact project-frame cut anchors chosen from music analysis.
    /// When present, there must be exactly one fewer anchor than `selects`.
    /// They remain strict unless `anchor_repair` explicitly opts into bounded repair.
    #[serde(default)]
    cut_anchor_frames: Option<Vec<TimeCode>>,
    /// Optional bounded repair for explicit preferred anchors. Valid only with
    /// `cut_anchor_frames`; omit it to preserve strict exact-anchor behavior.
    #[serde(default)]
    anchor_repair: Option<BeatMontageAnchorRepairArgs>,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
    /// Shortest permitted shot in exact project frames. Defaults to 20.
    #[serde(default)]
    minimum_shot_frames: Option<TimeCode>,
    /// Longest permitted shot in exact project frames. Defaults to 120.
    #[serde(default)]
    maximum_shot_frames: Option<TimeCode>,
    /// Optional observable cadence contract for the resolved project-frame shot durations.
    /// When present, the planner rejects the prepared plan unless it satisfies the
    /// requested number of duration buckets and similar-run limit.
    #[serde(default)]
    cadence: Option<BeatMontageCadenceContract>,
    /// Three-point collision policy. Overwrite is the predictable default.
    #[serde(default = "default_three_point_overwrite")]
    mode: ThreePointMode,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DialogueAssemblyPlanArgs {
    /// Existing media track that receives the gapless assembly.
    target_track_id: TrackId,
    /// Ordered audio/video asset ids whose spoken content should be preserved.
    asset_ids: Vec<AssetId>,
    /// Optional source-frame envelope for each ordered asset. When present, its length must match `asset_ids`. Cleanup never includes media outside an envelope.
    #[serde(default)]
    source_ranges: Option<Vec<TranscriptRangeArgs>>,
    /// Project-frame insertion point. Defaults to zero.
    #[serde(default)]
    timeline_start: Option<TimeCode>,
    /// Raw detector spans at least this long are safely removed. Defaults to 20 source frames.
    #[serde(default)]
    minimum_silence_source_frames: Option<TimeCode>,
    /// Remove conservative recognized hesitation words such as um and uh. Defaults to true.
    #[serde(default)]
    remove_fillers: Option<bool>,
    /// Total detector silence retained across each internal cut for natural pacing. Defaults to zero.
    #[serde(default)]
    retained_pause_source_frames: Option<TimeCode>,
    /// Extra source frames removed on each side of a recognized filler boundary. Defaults to zero.
    #[serde(default)]
    filler_padding_source_frames: Option<TimeCode>,
    /// Maximum acoustic pause retained between non-filler words bracketing a removed filler run. Longer pauses are trimmed to this cap; shorter natural pauses are preserved.
    #[serde(default)]
    maximum_filler_bridge_pause_source_frames: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MusicFitPlanArgs {
    /// Existing audio-capable target track.
    track_id: TrackId,
    /// Audio-capable media asset with beat analysis.
    asset_id: AssetId,
    /// Exact half-open project range the straight music edit must fill.
    timeline_range: TranscriptRangeArgs,
    /// Optional preferred source position. The nearest eligible beat with enough remaining source wins.
    #[serde(default)]
    preferred_source_start: Option<TimeCode>,
    /// Optional preferred half-open source end. Supply this together with
    /// `maximum_end_drift_frames` to choose a beat-anchored start whose exact
    /// real-time out point is closest to this endpoint.
    #[serde(default)]
    preferred_source_end: Option<TimeCode>,
    /// Inclusive non-negative source-frame tolerance for
    /// `preferred_source_end`. Required whenever that endpoint is supplied;
    /// the planner fails rather than silently broadening it.
    #[serde(default)]
    maximum_end_drift_frames: Option<TimeCode>,
    /// Optional onset-strength threshold from 0 through 100 percent. Defaults to 10.
    #[serde(default)]
    min_strength: Option<f64>,
    /// Three-point collision policy. Overwrite is the predictable default for fitting a music bed.
    #[serde(default = "default_three_point_overwrite")]
    mode: ThreePointMode,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SpeakerMulticamPlanArgs {
    /// Existing sync group containing the named camera angles.
    sync_group_id: SyncGroupId,
    /// Existing video track that will receive overwrite edits.
    target_track_id: TrackId,
    /// Sync-group member whose ready diarized transcript supplies speaker timing.
    reference_asset_id: AssetId,
    /// Half-open interval in sync-group project frames.
    group_range: TranscriptRangeArgs,
    /// Project-frame position corresponding to `group_range.start`.
    record_start: TimeCode,
    /// Merge same-angle words across gaps no larger than this. Defaults to 3 frames.
    #[serde(default)]
    maximum_word_gap_frames: Option<TimeCode>,
    /// Suppress rapid shots shorter than this. Defaults to 5 frames.
    #[serde(default)]
    minimum_shot_frames: Option<TimeCode>,
    /// Explicit diarization-label to sync-angle assignments.
    assignments: Vec<SpeakerAngleAssignment>,
}

const fn default_three_point_overwrite() -> ThreePointMode {
    ThreePointMode::Overwrite
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AnalysisStatusArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RequestAnalysisArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
    /// Analysis families to queue. Omit or pass an empty array to request all four.
    #[serde(default)]
    kinds: Vec<AnalysisKind>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CancelAnalysisArgs {
    /// Stable asset id shown by `get_timeline_state`.
    asset_id: AssetId,
    /// Analysis family whose queued or running job should be cancelled.
    kind: AnalysisKind,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StyledCaptionsArgs {
    /// Exact revision returned by `get_timeline_state` before planning captions.
    expected_revision: TimelineRevision,
    /// Stable declarative style shared by preview and export.
    preset: CaptionPreset,
    /// Renderer-native motion composition. Defaults to none.
    #[serde(default)]
    motion: CaptionMotion,
    /// Text contract for the delivered captions. Verbatim is the default and
    /// means every audible word must be represented. Edited-readable permits
    /// intentional omissions or rewrites and requires `script`.
    #[serde(default)]
    intent: CaptionIntent,
    /// Optional authored wording. In verbatim mode this is a corrected exact
    /// transcript; in edited-readable mode it is the explicit delivery copy.
    /// Punctuation becomes a hard cue-grouping boundary while generated
    /// transcript timing remains unchanged.
    #[serde(default)]
    script: Option<String>,
    /// Explicit caption placement. Omit for automatic subject-safe placement.
    #[serde(default)]
    position: Option<TitlePosition>,
    /// Optional vertical subject center from 0 (top) to 100 (bottom). Automatic
    /// placement uses the opposite safe region and defaults to lower third when
    /// no subject position is supplied.
    #[serde(default)]
    subject_y_percent: Option<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CaptionIntent {
    #[default]
    Verbatim,
    EditedReadable,
}

fn caption_position(
    explicit: Option<TitlePosition>,
    subject_y_percent: Option<u8>,
) -> Result<TitlePosition, &'static str> {
    if subject_y_percent.is_some_and(|value| value > 100) {
        return Err("caption subject_y_percent must be between 0 and 100");
    }
    Ok(explicit.unwrap_or_else(|| {
        if subject_y_percent.is_some_and(|subject_y| subject_y >= 60) {
            TitlePosition::Top
        } else {
            TitlePosition::LowerThird
        }
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CaptionListArgs {
    /// Optional half-open project-frame range. Omit for every generated caption.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Zero-based caption offset. Defaults to zero.
    #[serde(default)]
    offset: Option<usize>,
    /// Page size. Defaults to 50 and is capped at 200.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CaptionCorrection {
    /// Generated caption clip id returned by `get_captions`.
    clip_id: ClipId,
    /// Complete replacement caption text.
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CaptionCorrectionPlanArgs {
    /// Exact revision returned by `get_captions`.
    expected_revision: TimelineRevision,
    /// Atomic caption text replacements. Clip ids must be unique.
    corrections: Vec<CaptionCorrection>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TimelineDerivedArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TimelineSilencesArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Minimum transcript-safe cuttable span in source frames. Defaults to 6.
    #[serde(default)]
    min_duration_frames: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditPlanArgs {
    /// Exact revision returned by `get_timeline_state` before planning this batch.
    expected_revision: TimelineRevision,
    /// Ordered operations. Each item uses the generated Operation schema and sees prior effects.
    operations: Vec<PlanOperation>,
}

/// The generated schema remains the authoritative `Operation` schema, while
/// decoding also accepts the compact `{"op":"split_clip", ...}` shape that
/// coding agents naturally emit.
#[derive(Debug, Clone, JsonSchema)]
struct PlanOperation(Operation);

impl<'de> Deserialize<'de> for PlanOperation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        decode_plan_operation_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

fn decode_plan_operation_value(value: serde_json::Value) -> Result<Operation, String> {
    if let Ok(operation) = serde_json::from_value::<Operation>(value.clone()) {
        return Ok(operation);
    }
    let serde_json::Value::Object(mut object) = value else {
        return Err("operation must be an object".to_owned());
    };
    if let Some(op) = object.remove("op") {
        let op = op
            .as_str()
            .ok_or_else(|| "operation op must be a snake_case string".to_owned())?;
        let variant = snake_to_pascal(op);
        let tagged = serde_json::Value::Object(serde_json::Map::from_iter([(
            variant,
            serde_json::Value::Object(object),
        )]));
        return serde_json::from_value(tagged).map_err(|error| error.to_string());
    }
    if object.len() == 1 {
        let (name, payload) = object.into_iter().next().expect("length checked");
        let variant = snake_to_pascal(&name);
        let tagged = serde_json::Value::Object(serde_json::Map::from_iter([(variant, payload)]));
        return serde_json::from_value(tagged).map_err(|error| error.to_string());
    }
    Err("operation must use the generated enum envelope or include an op field".to_owned())
}

fn snake_to_pascal(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TimelineTranscriptArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DialoguePacingArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
    /// Shortest acceptable acoustic pause at a detected sentence boundary. Defaults to 10 project frames.
    #[serde(default)]
    minimum_pause_frames: Option<TimeCode>,
    /// Longest acceptable acoustic pause at a detected sentence boundary. Defaults to 40 project frames.
    #[serde(default)]
    maximum_pause_frames: Option<TimeCode>,
    /// Minimum word gap for an uppercase next word to count as a sentence boundary. Defaults to 4 project frames.
    #[serde(default)]
    capitalization_boundary_minimum_frames: Option<TimeCode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranscriptRangeArgs {
    start: TimeCode,
    end: TimeCode,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranscriptsArgs {
    /// Stable asset ids to inspect together, in response order. Maximum 32.
    asset_ids: Vec<AssetId>,
}

#[allow(clippy::too_many_lines)]
fn inspector_tools() -> Vec<Tool> {
    let read_only = || {
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false)
    };
    vec![
        Tool::new(
            "get_timeline_state",
            "Return the compact live project state and its exact timeline_revision. Every mutation must send that revision as expected_revision; inspect again after a conflict.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "search_capabilities",
            "Search only unnamed editing, perception, proof, or delivery needs. Skip exact names in the user request; batch independent terms.",
            schema_object::<CapabilitySearchArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_capability",
            "Open schemas for exact names from the user request or search results. Batch all workflow names; no prior search is required.",
            schema_object::<CapabilityArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "invoke_capability",
            "Invoke one discovered non-edit capability with arguments matching the schema returned by get_capability. Timeline edit operations must use prepare_edit_plan and commit_edit_plan.",
            schema_object::<InvokeCapabilityArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "prepare_edit_plan",
            "Decode and atomically validate ordered compact edit operations against one exact timeline revision. Returns an opaque plan id and deterministic before/after preview without changing the timeline.",
            schema_object::<PrepareEditPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "commit_edit_plan",
            "Commit one previously prepared plan as a single revision-gated undo entry. Stale, missing, invalid, or unconfirmed destructive plans are rejected.",
            schema_object::<CommitEditPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "discard_edit_plan",
            "Discard one opaque prepared plan without changing the timeline.",
            schema_object::<DiscardEditPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            "get_silences",
            "Return cached windowed-RMS silence spans for one asset in exact source frames and seconds, or background analysis status. For safe cutting, reported spans are clamped against cached transcribed words plus a 100 ms fps-aware margin; when no transcript is cached, the existing fixed 100 ms margin is used. Cached detector spans remain unchanged.",
            schema_object::<SilencesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_silences",
            "Return cached silence spans mapped through clips to exact project frames and seconds, filtered by a caller-selected final cuttable duration. Transcript protection and the 100 ms fps-aware margin are applied before the duration gate.",
            schema_object::<TimelineSilencesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_scene_changes",
            "Return cached proxy-resolution scene boundaries and confidence scores for one asset.",
            schema_object::<SceneChangesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_scene_changes",
            "Return cached scene boundaries mapped through clips to exact project frames and seconds.",
            schema_object::<TimelineDerivedArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_beats",
            "Return cached rhythmic onsets, strength scores, and estimated tempo for one asset in exact source frames, or queue deterministic background analysis.",
            schema_object::<BeatsArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_beats",
            "Return rhythmic onsets mapped through clips to exact project frames, with structured data suitable for beat-aware cutting.",
            schema_object::<TimelineBeatsArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_music_structure",
            "Infer a compact beat/bar/phrase hypothesis from one audio asset's mapped timeline beats. Results are heuristic candidates, not guaranteed music theory; this read-only capability produces no edit operations and never changes the timeline.",
            schema_object::<MusicStructureArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_dialogue_assembly",
            "Build and validate an exact, gapless AddClip plan from ordered dialogue assets using ready transcripts and raw silence analysis. It removes qualifying detector spans and optional conservative filler words without model-side frame arithmetic, then returns an opaque plan id ready for commit_edit_plan.",
            schema_object::<DialogueAssemblyPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "plan_beat_pacing",
            "Build and validate a deterministic, revision-gated SplitClip plan from fully analyzed timeline beats. Selected beats are inspectable in ascending order and operations are safely ordered newest-first; inspect the returned prepared_edit_plan preview, then commit its opaque plan id.",
            schema_object::<BeatPacingPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_beat_montage",
            "Build a source-feasible hard-cut montage timed to one analyzed music asset. The model owns every shot choice and the final order; the planner only selects beat boundaries that satisfy the supplied source envelopes and duration constraints, with no hidden transition, retime, or semantic replacement. Explicit anchors remain exact unless anchor_repair opts into a non-negative maximum movement bound and optional locked indices; requested, resolved, and per-anchor movement evidence is returned for review. An optional cadence contract validates rounded shot-duration buckets and similar runs before the prepared plan is stored. Returns an inspectable plan and opaque prepared_edit_plan preview without mutating the timeline.",
            schema_object::<BeatMontagePlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_music_fit",
            "Build one exact-duration real-time ThreePointEdit for an audio asset and project range. By default it starts on the eligible beat nearest preferred_source_start. Supplying both preferred_source_end and a non-negative maximum_end_drift_frames instead selects among eligible beat starts by endpoint distance first, then start preference and strength; it fails closed if no bounded endpoint fit exists. The plan returns exact endpoint evidence and never loops or time-stretches music.",
            schema_object::<MusicFitPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_speaker_multicam",
            "Build and validate a revision-gated multicam overwrite plan from real diarization labels, explicit speaker-to-angle assignments, and an existing sync group. Missing or ambiguous speaker data is returned as an error; inspect the returned prepared_edit_plan preview, then commit its opaque plan id.",
            schema_object::<SpeakerMulticamPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_audio_normalization",
            "Measure the rendered timeline mix, build compressor/gain/limiter processing with lossy-codec peak headroom for selected source tracks, render and remeasure the candidate in memory, and return a revision-gated plan only when it meets the requested LUFS target and sample-peak ceiling.",
            schema_object::<AudioNormalizationPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_analysis_status",
            "Return the uniform transcript, silence, scene, and beat job lifecycle for one asset without starting work.",
            schema_object::<AnalysisStatusArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_caption_presets",
            "List the stable clean, social, and minimal caption compositions as resolved title fields.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_captions",
            "Return generated caption text, clip ids, presets, and exact project ranges in a bounded page so transcription can be reviewed without expanding routine timeline state.",
            schema_object::<CaptionListArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_caption_corrections",
            "Build and validate one revision-bound SetTitleParam plan for up to 100 generated-caption text corrections. Returns an opaque plan id ready for commit_edit_plan; the timeline remains unchanged until commit.",
            schema_object::<CaptionCorrectionPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "add_styled_captions",
            "Build transcript-timed burned-in captions with an explicit verbatim or edited-readable text contract, semantic phrase grouping, optional corrected script, and automatic subject-safe top/lower-third placement. Applies as one revision-gated undo entry.",
            schema_object::<StyledCaptionsArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "get_qa_report",
            "Run deterministic export-health checks for missing media, gaps, abrupt cuts, retimed audio, caption readability, and animated title safe-area containment.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_delivery_variants",
            "List the built-in 16:9, 9:16, and 1:1 delivery graphs and their exact output resolutions.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_delivery_profiles",
            "List the stable source-master, YouTube 1080p, vertical-short, and square-social contracts using the current project frame rate, including exact raster, codecs, and bitrates.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_delivery_conformance",
            "Materialize one delivery profile from the current branch snapshot and run structural QA against the exact document and export settings that would render.",
            schema_object::<DeliveryConformanceArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "queue_export",
            "Queue a serial export of an immutable revision-gated branch snapshot using one stable delivery profile. New files require no confirmation; overwrite=true always enters the human confirmation broker and source media can never be targeted.",
            schema_object::<QueueExportArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        ),
        Tool::new(
            "get_export_jobs",
            "Return every retained export job in enqueue order with immutable request, conformance, progress, terminal state, and error data.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "cancel_export",
            "Idempotently cancel one queued or running export job. The backend observes the shared cancellation token cooperatively.",
            schema_object::<ExportJobArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(true),
        ),
        Tool::new(
            "get_delivery_variant_storyboard",
            "Render a real-compositor storyboard for a non-destructive delivery aspect using an explicit 0..=100 focal point. This is deterministic cover framing, not learned subject tracking.",
            schema_object::<DeliveryStoryboardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_editorial_readiness",
            "Run the common final editorial proof in one compact call: optional transcript-safe silence clearance, technical QA, delivery conformance, and a real delivery-profile storyboard. Set check_silence=false only when continuous program audio must be preserved or dead-air editing is outside the brief. Returns blocking details without repeating non-blocking issues.",
            schema_object::<EditorialReadinessArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "request_analysis",
            "Queue selected content-addressed analysis jobs for one asset, or all four families when kinds is omitted.",
            schema_object::<RequestAnalysisArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            "cancel_analysis",
            "Cooperatively cancel one queued or running asset-analysis job and return the resulting lifecycle state.",
            schema_object::<CancelAnalysisArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            "apply_edit_plan",
            "Atomically validate and apply ordered Kinewright Operations as one undo entry, only when expected_revision matches. Accepts the generated enum envelope and compact objects such as {\"op\":\"split_clip\",\"clip\":1,\"at\":30}.",
            schema_object::<EditPlanArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "get_clip_info",
            "Return detailed live information for one clip id.",
            schema_object::<ClipInfoArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_source_info",
            "Inspect one asset as source media with optional exact source-frame in/out marks. Returns technical metadata plus cached transcript words, speaker labels, scene boundaries, beats, and analysis lifecycle for that range.",
            schema_object::<SourceInfoArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_source_storyboard",
            "Render a bounded PNG contact sheet directly from one video asset's source frames. The manifest maps every cell to its exact source frame, asset id, and requested half-open source range without changing the timeline.",
            schema_object::<SourceStoryboardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_source_shot_board",
            "Return scene-derived, paged source-shot candidates for one video asset and render start/middle/end evidence cells for each. An optional inclusive minimum_duration_frames filters short candidates before pagination while preserving original candidate ids and indexes. minimum_confidence_basis_points controls scene-boundary sensitivity (0..=10000, default 1000); raise it for motion-heavy footage when weak differences over-segment a continuous shot. The manifest reports requested, filtered, returned, and total counts. Candidate ids, exact half-open source ranges, and scene-boundary confidence provenance are stable; this inspector never changes the timeline.",
            schema_object::<SourceShotBoardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "search_media",
            "Search the media graph by name/path text, cached transcript words, speaker label, media kind, resolution, duration, scene density, beat density, and transcript availability. Returns stable asset ids and exact matching source ranges for source-monitor and three-point edits.",
            schema_object::<MediaSearchArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_frame_at",
            "Render an actual PNG image at an exact project frame, downscaled to at most 512 pixels wide.",
            schema_object::<FrameAtArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_video_scopes",
            "Measure RGB and luma histograms, clipping, channel means, and a 64-column luma waveform from the real post-effect compositor output at an exact project frame. Use this before and after color or exposure edits instead of guessing from effect values.",
            schema_object::<VideoScopesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "track_mask_region",
            "Track an existing bounded mask region through one media clip using deterministic sequential template matching on isolated compositor frames. Returns confidence observations plus revision-gated SetEffectKeyframes operations for the mask center; it never silently mutates the timeline. Set mask width and height to 75 percent or less before tracking.",
            schema_object::<TrackMaskArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "track_reframe_subject",
            "Follow an explicitly seeded subject region through a clip using deterministic sequential template matching, then build an offline-lookahead camera path that contains every tracked box within explicit face-safe focus and maximum-step bounds. Returns an explicit error when full containment is infeasible plus confidence observations and a revision-gated plan; this is not a learned person detector and never silently mutates the timeline.",
            schema_object::<TrackReframeArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_storyboard",
            "Render a bounded PNG contact sheet from the real timeline compositor with a cell-to-frame manifest and timeline revision. Use it to survey footage and as visual proof after editing.",
            schema_object::<StoryboardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_transcript",
            "Return one asset's word-timestamped transcript in exact source frames and seconds, or its background transcription status.",
            schema_object::<TranscriptArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_transcripts",
            "Return transcripts for up to 32 assets in one ordered response, avoiding repeated model round trips while preserving exact source-frame word timestamps.",
            schema_object::<TranscriptsArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_transcript",
            "Return audible words mapped through clips to exact project frames and seconds. Use these boundaries for precise TrimClip, SplitClip, and DeleteClip edits.",
            schema_object::<TimelineTranscriptArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_dialogue_pacing",
            "Measure sentence-boundary pauses from mapped acoustic silence and classify each as short, target, or long, with transcript timing as an explicit fallback while silence analysis is unavailable. Boundaries use punctuation, asset or speaker changes, and pause-backed capitalization so agents can verify the rhythm viewers hear instead of guessing from clip edges.",
            schema_object::<DialoguePacingArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "import_media",
            "Probe a media path, then add the resulting asset metadata through Operation::AddAsset when expected_revision still matches.",
            schema_object::<ImportMediaArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
        ),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyArgs {}

fn decode_args<T: for<'de> Deserialize<'de>>(
    tool_name: &str,
    arguments: JsonObject,
) -> Result<T, McpError> {
    serde_json::from_value(serde_json::Value::Object(arguments))
        .map_err(|error| McpError::invalid_params(format!("{tool_name}: {error}"), None))
}

fn clamp_caption_cues_to_duration(cues: &mut Vec<CaptionCue>, duration: TimeCode) {
    for cue in &mut *cues {
        cue.end = cue.end.min(duration);
    }
    cues.retain(|cue| cue.start < duration && cue.end > cue.start);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DialoguePacingSettings {
    retained_pause: TimeCode,
    filler_padding: TimeCode,
    maximum_filler_bridge_pause: Option<TimeCode>,
}

fn validate_dialogue_assembly_assets(args: &DialogueAssemblyPlanArgs) -> Result<(), &'static str> {
    if args.asset_ids.is_empty() {
        return Err("dialogue assembly requires at least one ordered asset id");
    }
    if args
        .source_ranges
        .as_ref()
        .is_some_and(|ranges| ranges.len() != args.asset_ids.len())
    {
        return Err("dialogue assembly source_ranges must match asset_ids length");
    }
    Ok(())
}

fn dialogue_source_range(
    args: &DialogueAssemblyPlanArgs,
    index: usize,
    asset: &MediaAsset,
) -> Result<std::ops::Range<TimeCode>, String> {
    let source_range = args.source_ranges.as_ref().map_or_else(
        || TimeCode::ZERO..asset.duration,
        |ranges| ranges[index].start..ranges[index].end,
    );
    if source_range.start < TimeCode::ZERO
        || source_range.end > asset.duration
        || source_range.start >= source_range.end
    {
        return Err(format!(
            "asset {} source range {}..{} must be non-empty and within 0..{}",
            asset.id, source_range.start.0, source_range.end.0, asset.duration.0
        ));
    }
    Ok(source_range)
}

fn dialogue_pacing_settings(
    args: &DialogueAssemblyPlanArgs,
) -> Result<DialoguePacingSettings, &'static str> {
    let retained = args.retained_pause_source_frames.unwrap_or(TimeCode::ZERO);
    let padding = args.filler_padding_source_frames.unwrap_or(TimeCode::ZERO);
    let maximum_filler_bridge_pause = args.maximum_filler_bridge_pause_source_frames;
    if retained < TimeCode::ZERO
        || padding < TimeCode::ZERO
        || maximum_filler_bridge_pause.is_some_and(|pause| pause < TimeCode::ZERO)
    {
        return Err(
            "retained_pause_source_frames, filler_padding_source_frames, and maximum_filler_bridge_pause_source_frames must be non-negative",
        );
    }
    Ok(DialoguePacingSettings {
        retained_pause: retained,
        filler_padding: padding,
        maximum_filler_bridge_pause,
    })
}

fn dialogue_selection(
    ranges: &[std::ops::Range<TimeCode>],
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    pacing: DialoguePacingSettings,
    minimum_silence_source_frames: TimeCode,
) -> serde_json::Value {
    serde_json::json!({
        "asset_id": transcript.asset,
        "kept_source_ranges": ranges,
        "filler_bridges": dialogue_filler_bridges(
            transcript,
            silences,
            pacing.maximum_filler_bridge_pause,
            minimum_silence_source_frames,
        ),
    })
}

fn dialogue_pacing_result(
    range: std::ops::Range<TimeCode>,
    minimum: TimeCode,
    maximum: TimeCode,
    capitalization_minimum: TimeCode,
    pacing: &[DialoguePacingGap],
    pending_acoustic_assets: &[u64],
) -> CallToolResult {
    let short = pacing.iter().filter(|gap| gap.status == "short").count();
    let long = pacing.iter().filter(|gap| gap.status == "long").count();
    let target = pacing.len().saturating_sub(short).saturating_sub(long);
    let acoustic = pacing
        .iter()
        .filter(|gap| gap.measurement == "acoustic_silence")
        .count();
    let ready = pending_acoustic_assets.is_empty() && short == 0 && long == 0;
    let mut rendered = format!(
        "dialogue pacing range={}..{} boundaries={} acoustic={} target={} short={} long={} pending_acoustic_assets={:?} ready={ready}",
        range.start.0,
        range.end.0,
        pacing.len(),
        acoustic,
        target,
        short,
        long,
        pending_acoustic_assets,
    );
    for gap in pacing {
        let _ = write!(
            rendered,
            "\n{}..{} gap={} transcript_gap={} measurement={} status={} {:?} -> {:?} reason={}",
            gap.previous_end.0,
            gap.next_start.0,
            gap.pause_frames.0,
            gap.transcript_pause_frames.0,
            gap.measurement,
            gap.status,
            gap.previous_word,
            gap.next_word,
            gap.reason,
        );
    }
    success_structured(
        rendered,
        serde_json::json!({
            "range": {"start": range.start.0, "end": range.end.0},
            "target_pause_frames": {"minimum": minimum.0, "maximum": maximum.0},
            "capitalization_boundary_minimum_frames": capitalization_minimum.0,
            "summary": {
                "boundaries": pacing.len(),
                "target": target,
                "short": short,
                "long": long,
                "acoustic": acoustic,
                "pending_acoustic_asset_ids": pending_acoustic_assets,
                "ready": ready,
            },
            "gaps": pacing,
        }),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct DialogueFillerBridge {
    previous_word: String,
    next_word: String,
    source_start: TimeCode,
    source_end: TimeCode,
    cut_start: TimeCode,
    cut_end: TimeCode,
    available_pause_source_frames: TimeCode,
    maximum_pause_source_frames: TimeCode,
    maximum_contiguous_pause_source_frames: TimeCode,
    retained_pause_source_frames: TimeCode,
    measurement: &'static str,
}

fn dialogue_filler_bridges(
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    maximum_pause: Option<TimeCode>,
    minimum_silence_source_frames: TimeCode,
) -> Vec<DialogueFillerBridge> {
    let Some(maximum_pause) = maximum_pause else {
        return Vec::new();
    };
    let mut bridges = Vec::new();
    let mut index = 0;
    while index < transcript.words.len() {
        if !is_filler_word(&transcript.words[index].text) {
            index += 1;
            continue;
        }
        let first_filler = index;
        while index < transcript.words.len() && is_filler_word(&transcript.words[index].text) {
            index += 1;
        }
        let next_non_filler = index;
        let Some(previous_non_filler) = first_filler.checked_sub(1) else {
            continue;
        };
        let Some(next) = transcript.words.get(next_non_filler) else {
            continue;
        };
        let previous = &transcript.words[previous_non_filler];
        let first = &transcript.words[first_filler];
        let last = &transcript.words[next_non_filler - 1];
        if previous.source_end > first.source_start
            || first.source_start > last.source_end
            || last.source_end > next.source_start
        {
            continue;
        }
        let left_silence = silences
            .spans
            .iter()
            .filter(|span| {
                span.source_start < first.source_start && span.source_end >= first.source_start
            })
            .min_by_key(|span| span.source_start);
        let right_silence = silences
            .spans
            .iter()
            .filter(|span| {
                span.source_start <= last.source_end && span.source_end > last.source_end
            })
            .max_by_key(|span| span.source_end);
        let (bridge_start, bridge_end, left_available, right_available, measurement) =
            if let (Some(left_silence), Some(right_silence)) = (left_silence, right_silence) {
                (
                    left_silence.source_start,
                    right_silence.source_end,
                    first
                        .source_start
                        .0
                        .saturating_sub(left_silence.source_start.0),
                    right_silence.source_end.0.saturating_sub(last.source_end.0),
                    "acoustic_silence",
                )
            } else {
                (
                    previous.source_end,
                    next.source_start,
                    first.source_start.0.saturating_sub(previous.source_end.0),
                    next.source_start.0.saturating_sub(last.source_end.0),
                    "transcript_bounds",
                )
            };
        let available = left_available.saturating_add(right_available);
        let maximum_contiguous = minimum_silence_source_frames.0.saturating_sub(1);
        let left_capacity = left_available.min(maximum_contiguous);
        let right_capacity = right_available.min(maximum_contiguous);
        let requested = maximum_pause.0;
        let mut left = (requested / 2).min(left_capacity);
        let mut right = requested.saturating_sub(left).min(right_capacity);
        let mut remaining = requested.saturating_sub(left).saturating_sub(right);
        let left_extra = left_capacity.saturating_sub(left).min(remaining);
        left = left.saturating_add(left_extra);
        remaining = remaining.saturating_sub(left_extra);
        let right_extra = right_capacity.saturating_sub(right).min(remaining);
        right = right.saturating_add(right_extra);
        let cut_start = TimeCode(bridge_start.0.saturating_add(left));
        let cut_end = TimeCode(bridge_end.0.saturating_sub(right));
        if cut_end <= cut_start {
            continue;
        }
        bridges.push(DialogueFillerBridge {
            previous_word: previous.text.clone(),
            next_word: next.text.clone(),
            source_start: bridge_start,
            source_end: bridge_end,
            cut_start,
            cut_end,
            available_pause_source_frames: TimeCode(available),
            maximum_pause_source_frames: maximum_pause,
            maximum_contiguous_pause_source_frames: TimeCode(maximum_contiguous),
            retained_pause_source_frames: TimeCode(left.saturating_add(right)),
            measurement,
        });
    }
    bridges
}

fn dialogue_keep_ranges(
    asset: &MediaAsset,
    transcript: &AssetTranscript,
    silences: &AssetSilences,
    minimum_silence_source_frames: TimeCode,
    remove_fillers: bool,
    pacing: DialoguePacingSettings,
    source_range: std::ops::Range<TimeCode>,
) -> Vec<std::ops::Range<TimeCode>> {
    let bridges = if remove_fillers {
        dialogue_filler_bridges(
            transcript,
            silences,
            pacing.maximum_filler_bridge_pause,
            minimum_silence_source_frames,
        )
    } else {
        Vec::new()
    };
    let mut cuts = silences
        .spans
        .iter()
        .filter(|span| {
            span.source_end.0.saturating_sub(span.source_start.0) >= minimum_silence_source_frames.0
        })
        .flat_map(|span| {
            crate::silence::shrink_silence_span_for_cutting_with_transcript(
                *span,
                asset.fps,
                Some(&transcript.words),
            )
        })
        .flat_map(|span| subtract_dialogue_bridges(span.source_start..span.source_end, &bridges))
        .filter_map(|span| {
            let before = pacing.retained_pause.0 / 2;
            let after = pacing.retained_pause.0.saturating_sub(before);
            let start = TimeCode(span.start.0.saturating_add(before));
            let end = TimeCode(span.end.0.saturating_sub(after));
            (end > start).then_some(start..end)
        })
        .collect::<Vec<_>>();
    if remove_fillers {
        cuts.extend(
            transcript
                .words
                .iter()
                .filter(|word| is_filler_word(&word.text))
                .filter(|word| {
                    !bridges.iter().any(|bridge| {
                        word.source_start >= bridge.source_start
                            && word.source_end <= bridge.source_end
                    })
                })
                .map(|word| {
                    TimeCode(word.source_start.0.saturating_sub(pacing.filler_padding.0))
                        ..TimeCode(word.source_end.0.saturating_add(pacing.filler_padding.0))
                }),
        );
    }
    for cut in &mut cuts {
        cut.start = cut.start.clamp(source_range.start, source_range.end);
        cut.end = cut.end.clamp(source_range.start, source_range.end);
    }
    cuts.retain(|cut| cut.end > cut.start);
    let mut merged = merge_dialogue_cuts(cuts, pacing.retained_pause);
    merged.extend(
        bridges
            .iter()
            .map(|bridge| bridge.cut_start..bridge.cut_end),
    );
    let exact = merge_dialogue_cuts(merged, TimeCode::ZERO);
    let mut kept = Vec::new();
    let mut cursor = source_range.start;
    for cut in exact {
        if cut.start > cursor {
            kept.push(cursor..cut.start);
        }
        cursor = cursor.max(cut.end);
    }
    if cursor < source_range.end {
        kept.push(cursor..source_range.end);
    }
    kept
}

fn merge_dialogue_cuts(
    mut cuts: Vec<std::ops::Range<TimeCode>>,
    join_gap: TimeCode,
) -> Vec<std::ops::Range<TimeCode>> {
    cuts.sort_by_key(|cut| (cut.start, cut.end));
    let mut merged = Vec::<std::ops::Range<TimeCode>>::new();
    for cut in cuts {
        if let Some(previous) = merged.last_mut()
            && cut.start.0 <= previous.end.0.saturating_add(join_gap.0)
        {
            previous.end = previous.end.max(cut.end);
        } else {
            merged.push(cut);
        }
    }
    merged
}

fn subtract_dialogue_bridges(
    range: std::ops::Range<TimeCode>,
    bridges: &[DialogueFillerBridge],
) -> Vec<std::ops::Range<TimeCode>> {
    let mut remaining = vec![range];
    for bridge in bridges {
        let excluded = bridge.source_start..bridge.source_end;
        let mut next = Vec::new();
        for candidate in remaining {
            if excluded.end <= candidate.start || excluded.start >= candidate.end {
                next.push(candidate);
                continue;
            }
            if candidate.start < excluded.start {
                next.push(candidate.start..excluded.start.min(candidate.end));
            }
            if candidate.end > excluded.end {
                next.push(excluded.end.max(candidate.start)..candidate.end);
            }
        }
        remaining = next;
    }
    remaining
        .into_iter()
        .filter(|range| range.end > range.start)
        .collect()
}

fn success_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn success_structured(text: impl Into<String>, value: serde_json::Value) -> CallToolResult {
    let mut result = success_text(text);
    result.structured_content = Some(value);
    result
}

fn error_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn paths_resolve_equal(left: &Path, right: &Path) -> bool {
    let resolved = |path: &Path| {
        if let Ok(canonical) = path.canonicalize() {
            return Some(canonical);
        }
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        let parent = absolute.parent()?.canonicalize().ok()?;
        Some(parent.join(absolute.file_name()?))
    };
    let (Some(left), Some(right)) = (resolved(left), resolved(right)) else {
        return false;
    };
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn encode_png(image: &kinewright_core::RgbaImage) -> Result<Vec<u8>, McpError> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            &image.pixels,
            image.width,
            image.height,
            ColorType::Rgba8.into(),
        )
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(png)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackingObservation {
    local_frame: TimeCode,
    project_frame: TimeCode,
    center: [u32; 2],
    confidence_basis_points: u16,
}

/// Conservative source-normalized bounds for one tracked subject sample.
///
/// Coordinates use basis points so the evaluator can distinguish an actual
/// camera follow from an integer-percent approximation without making the
/// reframe effect itself carry non-rendering parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackedSubjectBounds {
    pub at: TimeCode,
    pub left_basis_points: u16,
    pub right_basis_points: u16,
    pub top_basis_points: u16,
    pub bottom_basis_points: u16,
}

/// Compact, document-persisted tracking evidence associated with one reframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReframeSubjectProvenance {
    pub clip: ClipId,
    pub effect: EffectId,
    pub samples: Vec<TrackedSubjectBounds>,
}

struct RegionTrackingRequest<'a> {
    document: &'a Document,
    clip_id: ClipId,
    clip_timeline_start: TimeCode,
    sample_frames: &'a [TimeCode],
    center_percent: [u8; 2],
    box_percent: [i64; 2],
    search_radius_percent: u8,
    max_width: u32,
    excluded_effect_name: &'a str,
}

struct TrackedRegion {
    observations: Vec<TrackingObservation>,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackingMatch {
    center: [u32; 2],
    confidence_basis_points: u16,
}

pub(crate) fn encode_reframe_subject_provenance(provenance: &ReframeSubjectProvenance) -> String {
    let sample_count = u16::try_from(provenance.samples.len()).unwrap_or(u16::MAX);
    let mut bytes = Vec::with_capacity(
        REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
            .saturating_add(usize::from(sample_count) * REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES),
    );
    bytes.extend_from_slice(&provenance.clip.0.to_le_bytes());
    bytes.extend_from_slice(&provenance.effect.0.to_le_bytes());
    bytes.extend_from_slice(&sample_count.to_le_bytes());
    for sample in provenance.samples.iter().take(usize::from(sample_count)) {
        bytes.extend_from_slice(&sample.at.0.to_le_bytes());
        bytes.extend_from_slice(&sample.left_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.right_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.top_basis_points.to_le_bytes());
        bytes.extend_from_slice(&sample.bottom_basis_points.to_le_bytes());
    }
    format!(
        "{REFRAME_SUBJECT_PROVENANCE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// Decode one opaque marker-label tracking sidecar.
///
/// Non-provenance labels return `Ok(None)` so ordinary user markers stay
/// entirely outside this contract. A matching prefix with malformed data is
/// intentionally an error: silently ignoring corrupted tracking evidence
/// would let a static or wrong-direction reframe pass evaluation.
pub(crate) fn decode_reframe_subject_provenance(
    label: &str,
) -> Result<Option<ReframeSubjectProvenance>, String> {
    let Some(encoded) = label.strip_prefix(REFRAME_SUBJECT_PROVENANCE_PREFIX) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("invalid base64: {error}"))?;
    if bytes.len() < REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES {
        return Err("missing provenance header".to_owned());
    }
    let decode_id = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| "truncated provenance header".to_owned())?;
        let array: [u8; 8] = slice
            .try_into()
            .map_err(|_| "invalid provenance header width".to_owned())?;
        Ok::<u64, String>(u64::from_le_bytes(array))
    };
    let read_u16 = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(2))
            .ok_or_else(|| "truncated provenance sample".to_owned())?;
        let array: [u8; 2] = slice
            .try_into()
            .map_err(|_| "invalid provenance sample width".to_owned())?;
        Ok::<u16, String>(u16::from_le_bytes(array))
    };
    let decode_frame = |offset: usize| {
        let slice = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| "truncated provenance sample".to_owned())?;
        let array: [u8; 8] = slice
            .try_into()
            .map_err(|_| "invalid provenance sample width".to_owned())?;
        Ok::<i64, String>(i64::from_le_bytes(array))
    };
    let clip = ClipId(decode_id(0)?);
    let effect = EffectId(decode_id(8)?);
    let sample_count = usize::from(read_u16(16)?);
    if sample_count > MAX_TRACKING_SAMPLES {
        return Err(format!(
            "contains {sample_count} samples, above the {MAX_TRACKING_SAMPLES} sample limit"
        ));
    }
    let expected_length = REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
        .saturating_add(sample_count.saturating_mul(REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES));
    if bytes.len() != expected_length {
        return Err(format!(
            "expected {expected_length} bytes for {sample_count} samples, found {}",
            bytes.len()
        ));
    }
    if sample_count == 0 {
        return Err("contains no tracked subject samples".to_owned());
    }
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let offset = REFRAME_SUBJECT_PROVENANCE_HEADER_BYTES
            .saturating_add(index.saturating_mul(REFRAME_SUBJECT_PROVENANCE_SAMPLE_BYTES));
        let at = TimeCode(decode_frame(offset)?);
        let left_basis_points = read_u16(offset + 8)?;
        let right_basis_points = read_u16(offset + 10)?;
        let top_basis_points = read_u16(offset + 12)?;
        let bottom_basis_points = read_u16(offset + 14)?;
        if at < TimeCode::ZERO
            || left_basis_points > right_basis_points
            || top_basis_points > bottom_basis_points
            || right_basis_points > 10_000
            || bottom_basis_points > 10_000
        {
            return Err(format!("sample {index} has invalid bounds"));
        }
        if samples
            .last()
            .is_some_and(|previous: &TrackedSubjectBounds| at <= previous.at)
        {
            return Err(format!("sample {index} is not strictly ordered"));
        }
        samples.push(TrackedSubjectBounds {
            at,
            left_basis_points,
            right_basis_points,
            top_basis_points,
            bottom_basis_points,
        });
    }
    Ok(Some(ReframeSubjectProvenance {
        clip,
        effect,
        samples,
    }))
}

fn tracked_subject_bounds(
    observation: &TrackingObservation,
    width: u32,
    height: u32,
    box_percent: [i64; 2],
) -> TrackedSubjectBounds {
    let half_size = [
        tracking_half_extent(width, box_percent[0]),
        tracking_half_extent(height, box_percent[1]),
    ];
    let left = observation.center[0].saturating_sub(half_size[0]);
    let right = observation.center[0]
        .saturating_add(half_size[0])
        .min(width.saturating_sub(1));
    let top = observation.center[1].saturating_sub(half_size[1]);
    let bottom = observation.center[1]
        .saturating_add(half_size[1])
        .min(height.saturating_sub(1));
    TrackedSubjectBounds {
        at: observation.local_frame,
        left_basis_points: pixel_to_basis_points_floor(left, width),
        right_basis_points: pixel_to_basis_points_ceil(right, width),
        top_basis_points: pixel_to_basis_points_floor(top, height),
        bottom_basis_points: pixel_to_basis_points_ceil(bottom, height),
    }
}

fn tracking_sample_frames(range: std::ops::Range<TimeCode>, step: i64) -> Vec<TimeCode> {
    let Some(last) = range.end.0.checked_sub(1) else {
        return Vec::new();
    };
    if last < range.start.0 {
        return Vec::new();
    }
    if last == range.start.0 {
        return vec![range.start];
    }

    // Treat `step` as the requested maximum spacing, then distribute the
    // samples across the whole visible span. Appending `last` after stepping
    // leaves a one-frame tail whenever the span is not divisible by `step`.
    // Evenly distributing ceil(span / step) intervals keeps every gap within
    // one frame of its neighbours and makes the final interval ordinary.
    let span = i128::from(last) - i128::from(range.start.0);
    let requested_step = i128::from(step.max(1));
    let interval_count = usize::try_from((span + requested_step - 1) / requested_step)
        .unwrap_or(usize::MAX)
        .max(1);
    let interval_count_i128 = i128::try_from(interval_count).unwrap_or(i128::MAX);
    let mut frames = Vec::with_capacity(interval_count.saturating_add(1));
    for index in 0..=interval_count {
        let index_i128 = i128::try_from(index).unwrap_or(i128::MAX);
        let offset = span.saturating_mul(index_i128) / interval_count_i128;
        let frame = if index == interval_count {
            last
        } else {
            i64::try_from(i128::from(range.start.0).saturating_add(offset)).unwrap_or(last)
        };
        frames.push(TimeCode(frame));
    }
    frames
}

fn tracking_half_size(image: &kinewright_core::RgbaImage, box_percent: [i64; 2]) -> [u32; 2] {
    [
        tracking_half_extent(image.width, box_percent[0]),
        tracking_half_extent(image.height, box_percent[1]),
    ]
}

fn tracking_half_extent(extent: u32, percent: i64) -> u32 {
    let percent = u32::try_from(percent).unwrap_or_default();
    extent
        .saturating_mul(percent)
        .div_ceil(200)
        .max(1)
        .min(extent.saturating_sub(1) / 2)
}

fn pixel_to_basis_points_floor(pixel: u32, extent: u32) -> u16 {
    let denominator = u64::from(extent.saturating_sub(1).max(1));
    let value = u64::from(pixel)
        .saturating_mul(10_000)
        .checked_div(denominator)
        .unwrap_or_default()
        .min(10_000);
    u16::try_from(value).unwrap_or(10_000)
}

fn pixel_to_basis_points_ceil(pixel: u32, extent: u32) -> u16 {
    let denominator = u64::from(extent.saturating_sub(1).max(1));
    let numerator = u64::from(pixel).saturating_mul(10_000);
    let value = numerator
        .saturating_add(denominator.saturating_sub(1))
        .checked_div(denominator)
        .unwrap_or_default()
        .min(10_000);
    u16::try_from(value).unwrap_or(10_000)
}

fn percent_to_pixel(percent: u8, extent: u32) -> u32 {
    u32::from(percent)
        .saturating_mul(extent.saturating_sub(1))
        .saturating_add(50)
        / 100
}

fn pixel_to_percent(pixel: u32, extent: u32) -> u8 {
    let denominator = extent.saturating_sub(1).max(1);
    let rounded = pixel.saturating_mul(100).saturating_add(denominator / 2) / denominator;
    u8::try_from(rounded.min(100)).unwrap_or(100)
}

fn pixel_to_basis_points(pixel: u32, extent: u32) -> u16 {
    let denominator = u64::from(extent.saturating_sub(1).max(1));
    let rounded = u64::from(pixel)
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    u16::try_from(rounded.min(10_000)).unwrap_or(10_000)
}

fn tracked_subject_focus_constraint(
    subject: TrackedSubjectBounds,
    source_width: u32,
    source_height: u32,
    target_aspect_basis_points: i64,
) -> Result<SubjectFocusBasisPointConstraint, String> {
    if source_width == 0 || source_height == 0 {
        return Err(format!(
            "source resolution must be positive, found {source_width}x{source_height}"
        ));
    }
    if target_aspect_basis_points <= 0 {
        return Err(format!(
            "target_aspect_basis_points must be positive, found {target_aspect_basis_points}"
        ));
    }

    let source_width = i128::from(source_width);
    let source_height = i128::from(source_height);
    let target_aspect = i128::from(target_aspect_basis_points);
    let source_is_wider =
        source_width.saturating_mul(10_000) > source_height.saturating_mul(target_aspect);
    let source_is_taller =
        source_width.saturating_mul(10_000) < source_height.saturating_mul(target_aspect);
    let (visible_width, visible_height) = if source_is_wider {
        (
            i64::try_from(ceil_positive_ratio(
                target_aspect.saturating_mul(source_height),
                source_width,
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
            10_000,
        )
    } else if source_is_taller {
        (
            10_000,
            i64::try_from(ceil_positive_ratio(
                source_width.saturating_mul(100_000_000),
                source_height.saturating_mul(target_aspect),
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
        )
    } else {
        (10_000, 10_000)
    };
    let (minimum_x, maximum_x) = focus_interval_for_subject_axis(
        i64::from(subject.left_basis_points),
        i64::from(subject.right_basis_points),
        visible_width,
    )
    .ok_or_else(|| {
        format!(
            "tracked subject at frame {} is wider than the delivery crop",
            subject.at
        )
    })?;
    let (minimum_y, maximum_y) = focus_interval_for_subject_axis(
        i64::from(subject.top_basis_points),
        i64::from(subject.bottom_basis_points),
        visible_height,
    )
    .ok_or_else(|| {
        format!(
            "tracked subject at frame {} is taller than the delivery crop",
            subject.at
        )
    })?;

    Ok(SubjectFocusBasisPointConstraint {
        at: subject.at,
        min_x_basis_points: minimum_x,
        max_x_basis_points: maximum_x,
        min_y_basis_points: minimum_y,
        max_y_basis_points: maximum_y,
    })
}

fn ceil_positive_ratio(numerator: i128, denominator: i128) -> i128 {
    numerator
        .saturating_add(denominator.saturating_sub(1))
        .checked_div(denominator.max(1))
        .unwrap_or_default()
}

/// Invert the compositor's clamped crop-axis transform.
///
/// At either frame edge, many focus values produce the same clamped crop. The
/// returned interval retains those plateaus instead of forcing the virtual
/// camera toward an arbitrary centre value.
fn focus_interval_for_subject_axis(
    subject_minimum: i64,
    subject_maximum: i64,
    visible_basis_points: i64,
) -> Option<(i64, i64)> {
    let visible = visible_basis_points.clamp(1, 10_000);
    if subject_minimum < 0
        || subject_maximum > 10_000
        || subject_minimum > subject_maximum
        || subject_maximum.saturating_sub(subject_minimum) > visible
    {
        return None;
    }
    let maximum_crop_start = 10_000_i64.saturating_sub(visible);
    let minimum_crop_start = subject_maximum.saturating_sub(visible).max(0);
    let maximum_allowed_crop_start = subject_minimum.min(maximum_crop_start);
    if minimum_crop_start > maximum_allowed_crop_start {
        return None;
    }
    let half_visible = visible / 2;
    let minimum_focus = if minimum_crop_start == 0 {
        0
    } else {
        minimum_crop_start.saturating_add(half_visible)
    };
    let maximum_focus = if maximum_allowed_crop_start == maximum_crop_start {
        10_000
    } else {
        maximum_allowed_crop_start.saturating_add(half_visible)
    };
    Some((minimum_focus, maximum_focus))
}

fn clamp_tracking_center(
    image: &kinewright_core::RgbaImage,
    center: [u32; 2],
    half_size: [u32; 2],
) -> [u32; 2] {
    let clamp = |value: u32, extent: u32, half: u32| {
        value.clamp(half, extent.saturating_sub(half).saturating_sub(1))
    };
    [
        clamp(center[0], image.width, half_size[0]),
        clamp(center[1], image.height, half_size[1]),
    ]
}

fn track_region(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    half_size: [u32; 2],
    search_radius_percent: u8,
) -> TrackingMatch {
    let radius = [
        previous
            .width
            .saturating_mul(u32::from(search_radius_percent))
            .div_ceil(100)
            .max(1),
        previous
            .height
            .saturating_mul(u32::from(search_radius_percent))
            .div_ceil(100)
            .max(1),
    ];
    let minimum = [
        previous_center[0]
            .saturating_sub(radius[0])
            .max(half_size[0]),
        previous_center[1]
            .saturating_sub(radius[1])
            .max(half_size[1]),
    ];
    let maximum = [
        previous_center[0]
            .saturating_add(radius[0])
            .min(current.width.saturating_sub(half_size[0]).saturating_sub(1)),
        previous_center[1].saturating_add(radius[1]).min(
            current
                .height
                .saturating_sub(half_size[1])
                .saturating_sub(1),
        ),
    ];
    let coarse_step = radius[0].max(radius[1]).div_ceil(8).max(1);
    let sample_step = half_size[0]
        .saturating_mul(2)
        .saturating_add(1)
        .max(half_size[1].saturating_mul(2).saturating_add(1))
        .div_ceil(24)
        .max(1);
    let mut best = (
        u64::MAX,
        u32::MAX,
        previous_center[1],
        previous_center[0],
        1_u64,
    );
    for y in candidate_axis(minimum[1], maximum[1], coarse_step) {
        for x in candidate_axis(minimum[0], maximum[0], coarse_step) {
            best = best.min(tracking_candidate(
                previous,
                current,
                previous_center,
                [x, y],
                half_size,
                sample_step,
            ));
        }
    }
    let coarse_center = [best.3, best.2];
    let refine_minimum = [
        coarse_center[0].saturating_sub(coarse_step).max(minimum[0]),
        coarse_center[1].saturating_sub(coarse_step).max(minimum[1]),
    ];
    let refine_maximum = [
        coarse_center[0].saturating_add(coarse_step).min(maximum[0]),
        coarse_center[1].saturating_add(coarse_step).min(maximum[1]),
    ];
    for y in refine_minimum[1]..=refine_maximum[1] {
        for x in refine_minimum[0]..=refine_maximum[0] {
            best = best.min(tracking_candidate(
                previous,
                current,
                previous_center,
                [x, y],
                half_size,
                sample_step,
            ));
        }
    }
    let maximum_sad = best.4.saturating_mul(3 * u64::from(u8::MAX)).max(1);
    let error_basis_points = best.0.saturating_mul(10_000) / maximum_sad;
    TrackingMatch {
        center: [best.3, best.2],
        confidence_basis_points: u16::try_from(
            10_000_u64.saturating_sub(error_basis_points.min(10_000)),
        )
        .unwrap_or_default(),
    }
}

fn tracking_candidate(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    candidate_center: [u32; 2],
    half_size: [u32; 2],
    sample_step: u32,
) -> (u64, u32, u32, u32, u64) {
    let (score, samples) = region_sad(
        previous,
        current,
        previous_center,
        candidate_center,
        half_size,
        sample_step,
    );
    let distance = candidate_center[0]
        .abs_diff(previous_center[0])
        .saturating_add(candidate_center[1].abs_diff(previous_center[1]));
    (
        score,
        distance,
        candidate_center[1],
        candidate_center[0],
        samples,
    )
}

fn candidate_axis(minimum: u32, maximum: u32, step: u32) -> Vec<u32> {
    let mut values = (minimum..=maximum)
        .step_by(usize::try_from(step).unwrap_or(1).max(1))
        .collect::<Vec<_>>();
    if values.last() != Some(&maximum) {
        values.push(maximum);
    }
    values
}

fn region_sad(
    previous: &kinewright_core::RgbaImage,
    current: &kinewright_core::RgbaImage,
    previous_center: [u32; 2],
    candidate_center: [u32; 2],
    half_size: [u32; 2],
    sample_step: u32,
) -> (u64, u64) {
    let step = usize::try_from(sample_step).unwrap_or(1).max(1);
    let mut sad = 0_u64;
    let mut samples = 0_u64;
    for offset_y in (0..=half_size[1].saturating_mul(2)).step_by(step) {
        for offset_x in (0..=half_size[0].saturating_mul(2)).step_by(step) {
            let previous_x = previous_center[0]
                .saturating_sub(half_size[0])
                .saturating_add(offset_x);
            let previous_y = previous_center[1]
                .saturating_sub(half_size[1])
                .saturating_add(offset_y);
            let current_x = candidate_center[0]
                .saturating_sub(half_size[0])
                .saturating_add(offset_x);
            let current_y = candidate_center[1]
                .saturating_sub(half_size[1])
                .saturating_add(offset_y);
            let previous_index = usize::try_from(
                previous_y
                    .saturating_mul(previous.width)
                    .saturating_add(previous_x)
                    .saturating_mul(4),
            )
            .unwrap_or_default();
            let current_index = usize::try_from(
                current_y
                    .saturating_mul(current.width)
                    .saturating_add(current_x)
                    .saturating_mul(4),
            )
            .unwrap_or_default();
            for channel in 0..3 {
                sad = sad.saturating_add(u64::from(
                    previous.pixels[previous_index + channel]
                        .abs_diff(current.pixels[current_index + channel]),
                ));
            }
            samples = samples.saturating_add(1);
        }
    }
    (sad, samples)
}

fn scope_data(image: &kinewright_core::RgbaImage, bins: usize) -> serde_json::Value {
    const WAVEFORM_COLUMNS: usize = 64;
    let bins = bins.clamp(1, 256);
    let mut red = vec![0_u64; bins];
    let mut green = vec![0_u64; bins];
    let mut blue = vec![0_u64; bins];
    let mut luma = vec![0_u64; bins];
    let mut channel_sums = [0_u64; 4];
    let mut clipped_black = 0_u64;
    let mut clipped_white = 0_u64;
    let mut waveform_min = [u8::MAX; WAVEFORM_COLUMNS];
    let mut waveform_max = [0_u8; WAVEFORM_COLUMNS];
    let mut waveform_sum = [0_u64; WAVEFORM_COLUMNS];
    let mut waveform_count = [0_u64; WAVEFORM_COLUMNS];
    let width = usize::try_from(image.width).unwrap_or(1).max(1);
    let mut pixel_count = 0_u64;

    for (pixel_index, pixel) in image.pixels.chunks_exact(4).enumerate() {
        let [red_value, green_value, blue_value, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        if alpha == 0 {
            continue;
        }
        let luma_value = u8::try_from(
            (54_u32 * u32::from(red_value)
                + 183_u32 * u32::from(green_value)
                + 19_u32 * u32::from(blue_value))
                / 256,
        )
        .unwrap_or(u8::MAX);
        let bucket = |value: u8| usize::from(value) * bins / 256;
        red[bucket(red_value)] += 1;
        green[bucket(green_value)] += 1;
        blue[bucket(blue_value)] += 1;
        luma[bucket(luma_value)] += 1;
        channel_sums[0] += u64::from(red_value);
        channel_sums[1] += u64::from(green_value);
        channel_sums[2] += u64::from(blue_value);
        channel_sums[3] += u64::from(luma_value);
        clipped_black += u64::from(luma_value <= 1);
        clipped_white += u64::from(luma_value >= 254);
        pixel_count += 1;

        let pixel_x = pixel_index % width;
        let column = (pixel_x * WAVEFORM_COLUMNS / width).min(WAVEFORM_COLUMNS - 1);
        waveform_min[column] = waveform_min[column].min(luma_value);
        waveform_max[column] = waveform_max[column].max(luma_value);
        waveform_sum[column] += u64::from(luma_value);
        waveform_count[column] += 1;
    }

    let mean_milli = channel_sums.map(|sum| {
        sum.saturating_mul(1_000)
            .checked_div(pixel_count)
            .unwrap_or(0)
    });
    let basis_points = |count: u64| {
        count
            .saturating_mul(10_000)
            .checked_div(pixel_count)
            .unwrap_or(0)
    };
    let waveform = (0..WAVEFORM_COLUMNS)
        .map(|column| {
            let count = waveform_count[column];
            serde_json::json!({
                "column": column,
                "minimum": if count == 0 { 0 } else { waveform_min[column] },
                "maximum": if count == 0 { 0 } else { waveform_max[column] },
                "mean_milli": waveform_sum[column].saturating_mul(1_000).checked_div(count).unwrap_or(0),
                "samples": count,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "resolution": [image.width, image.height],
        "visible_pixel_count": pixel_count,
        "histogram_bins": bins,
        "histograms": {
            "red": red,
            "green": green,
            "blue": blue,
            "luma": luma,
        },
        "mean_milli": {
            "red": mean_milli[0],
            "green": mean_milli[1],
            "blue": mean_milli[2],
            "luma": mean_milli[3],
        },
        "clipping_basis_points": {
            "black": basis_points(clipped_black),
            "white": basis_points(clipped_white),
        },
        "waveform_luma": waveform,
    })
}

#[cfg(test)]
mod tracking_tests {
    use super::*;

    fn box_frame(center: [u32; 2]) -> kinewright_core::RgbaImage {
        let width = 32;
        let height = 20;
        let mut pixels = vec![0_u8; usize::try_from(width * height * 4).unwrap()];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for y in center[1] - 2..=center[1] + 2 {
            for x in center[0] - 2..=center[0] + 2 {
                let index = usize::try_from((y * width + x) * 4).unwrap();
                pixels[index..index + 4].copy_from_slice(&[220, 40, 10, 255]);
            }
        }
        kinewright_core::RgbaImage {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn deterministic_tracker_follows_a_translated_region_exactly() {
        let previous = box_frame([8, 8]);
        let current = box_frame([13, 11]);
        let tracked = track_region(&previous, &current, [8, 8], [2, 2], 25);

        assert_eq!(tracked.center, [13, 11]);
        assert_eq!(tracked.confidence_basis_points, 10_000);
    }

    #[test]
    fn tracking_samples_include_the_exact_last_visible_frame() {
        assert_eq!(
            tracking_sample_frames(TimeCode(3)..TimeCode(15), 5),
            vec![TimeCode(3), TimeCode(6), TimeCode(10), TimeCode(14)]
        );
    }

    #[test]
    fn tracking_samples_distribute_non_divisible_spans_without_a_short_tail() {
        let frames = tracking_sample_frames(TimeCode(0)..TimeCode(12), 5);

        assert_eq!(
            frames,
            vec![TimeCode(0), TimeCode(3), TimeCode(7), TimeCode(11)]
        );
        let gaps = frames
            .windows(2)
            .map(|pair| pair[1].0 - pair[0].0)
            .collect::<Vec<_>>();
        assert_eq!(gaps, vec![3, 4, 4]);
        assert!(gaps.iter().all(|gap| *gap <= 5));
        assert!(gaps.windows(2).all(|pair| pair[0].abs_diff(pair[1]) <= 1));
    }

    #[test]
    fn tracking_samples_handle_short_ranges_and_exact_division() {
        assert_eq!(
            tracking_sample_frames(TimeCode(7)..TimeCode(10), 10),
            vec![TimeCode(7), TimeCode(9)]
        );
        assert_eq!(
            tracking_sample_frames(TimeCode(4)..TimeCode(15), 5),
            vec![TimeCode(4), TimeCode(9), TimeCode(14)]
        );
        assert_eq!(
            tracking_sample_frames(TimeCode(6)..TimeCode(7), 5),
            vec![TimeCode(6)]
        );
    }

    #[test]
    fn tracking_samples_are_unique_and_in_visible_range() {
        let frames = tracking_sample_frames(TimeCode(10)..TimeCode(31), 6);

        assert_eq!(frames.first(), Some(&TimeCode(10)));
        assert_eq!(frames.last(), Some(&TimeCode(30)));
        assert!(frames.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(frames.iter().all(|frame| (10..31).contains(&frame.0)));
    }

    #[test]
    fn tracked_subject_constraints_match_the_failed_vertical_crop_edges() {
        let constraint = tracked_subject_focus_constraint(
            TrackedSubjectBounds {
                at: TimeCode(69),
                left_basis_points: 2_392,
                right_basis_points: 4_902,
                top_basis_points: 1_442,
                bottom_basis_points: 4_520,
            },
            352,
            288,
            5_625,
        )
        .unwrap();

        assert_eq!(constraint.min_x_basis_points, 2_600);
        assert_eq!(constraint.max_x_basis_points, 4_693);
        assert_eq!(constraint.min_y_basis_points, 0);
        assert_eq!(constraint.max_y_basis_points, 10_000);
    }

    #[test]
    fn tracked_subject_constraints_preserve_the_right_edge_focus_plateau() {
        let constraint = tracked_subject_focus_constraint(
            TrackedSubjectBounds {
                at: TimeCode(235),
                left_basis_points: 1_921,
                right_basis_points: 4_432,
                top_basis_points: 3_557,
                bottom_basis_points: 6_635,
            },
            352,
            288,
            5_625,
        )
        .unwrap();

        assert_eq!(constraint.min_x_basis_points, 0);
        assert_eq!(constraint.max_x_basis_points, 4_222);
        assert_eq!(constraint.min_y_basis_points, 0);
        assert_eq!(constraint.max_y_basis_points, 10_000);
    }
}

#[cfg(test)]
mod reframe_geometry_tests {
    use super::*;

    fn crop_axis(focus_basis_points: i64, visible_basis_points: i64) -> (i64, i64) {
        let visible = visible_basis_points.clamp(1, 10_000);
        let maximum_left = 10_000 - visible;
        let left = focus_basis_points
            .saturating_sub(visible / 2)
            .clamp(0, maximum_left);
        (left, left + visible)
    }

    fn contains(
        focus_basis_points: i64,
        visible_basis_points: i64,
        subject_minimum: i64,
        subject_maximum: i64,
    ) -> bool {
        let (left, right) = crop_axis(focus_basis_points, visible_basis_points);
        left <= subject_minimum && right >= subject_maximum
    }

    #[test]
    fn tracked_subject_constraint_inverts_clamped_cover_crop_at_both_edges() {
        let subject = TrackedSubjectBounds {
            at: TimeCode(69),
            left_basis_points: 500,
            right_basis_points: 700,
            top_basis_points: 1_000,
            bottom_basis_points: 2_000,
        };
        let constraint = tracked_subject_focus_constraint(subject, 1_920, 1_080, 5_625)
            .expect("subject fits the vertical short crop");

        // 1920x1080 into a 9:16 delivery leaves 3165 basis points of source
        // width visible. The left crop edge is clamped for focus 0..=1582,
        // so the valid focus interval includes that entire plateau.
        assert_eq!(
            (constraint.min_x_basis_points, constraint.max_x_basis_points),
            (0, 2_082)
        );
        assert_eq!(
            (constraint.min_y_basis_points, constraint.max_y_basis_points),
            (0, 10_000)
        );
        for focus in constraint.min_x_basis_points..=constraint.max_x_basis_points {
            assert!(contains(
                focus,
                3_165,
                i64::from(subject.left_basis_points),
                i64::from(subject.right_basis_points)
            ));
        }
        assert!(!contains(2_083, 3_165, 500, 700));

        let right_edge_subject = TrackedSubjectBounds {
            left_basis_points: 9_000,
            right_basis_points: 9_500,
            ..subject
        };
        let right_edge = tracked_subject_focus_constraint(right_edge_subject, 1_920, 1_080, 5_625)
            .expect("right-edge subject fits the crop");
        assert_eq!(
            (right_edge.min_x_basis_points, right_edge.max_x_basis_points),
            (7_917, 10_000)
        );
        for focus in right_edge.min_x_basis_points..=right_edge.max_x_basis_points {
            assert!(contains(focus, 3_165, 9_000, 9_500));
        }
        assert!(!contains(7_916, 3_165, 9_000, 9_500));
    }

    #[test]
    fn tracked_subject_constraint_uses_the_same_aspect_rounding_as_evaluator() {
        let subject = TrackedSubjectBounds {
            at: TimeCode(235),
            left_basis_points: 1_938,
            right_basis_points: 6_541,
            top_basis_points: 3_000,
            bottom_basis_points: 6_000,
        };
        let constraint = tracked_subject_focus_constraint(subject, 1_080, 1_920, 16_000)
            .expect("subject fits the tall crop");

        // ceil(1080 * 100000000 / (1920 * 16000)) = 3516. The helper must
        // preserve that conservative evaluator rounding when inverting the
        // vertical crop axis.
        assert_eq!(
            (constraint.min_y_basis_points, constraint.max_y_basis_points),
            (4_242, 4_758)
        );
        for focus in constraint.min_y_basis_points..=constraint.max_y_basis_points {
            assert!(contains(focus, 3_516, 3_000, 6_000));
        }
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn scopes_are_exact_and_ignore_fully_transparent_pixels() {
        let scopes = scope_data(
            &kinewright_core::RgbaImage {
                width: 3,
                height: 1,
                pixels: vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0],
            },
            16,
        );
        assert_eq!(scopes["visible_pixel_count"], 2);
        assert_eq!(scopes["clipping_basis_points"]["black"], 5_000);
        assert_eq!(scopes["clipping_basis_points"]["white"], 5_000);
        assert_eq!(scopes["mean_milli"]["luma"], 127_500);
        assert_eq!(scopes["histograms"]["luma"][0], 1);
        assert_eq!(scopes["histograms"]["luma"][15], 1);
    }
}

fn storyboard_sample_frames(range: &std::ops::Range<TimeCode>, frame_count: u8) -> Vec<TimeCode> {
    let count = usize::from(frame_count.max(1));
    let inclusive_span = range.end.0.saturating_sub(range.start.0).saturating_sub(1);
    if count == 1 {
        return vec![TimeCode(range.start.0.saturating_add(inclusive_span / 2))];
    }
    let divisor = i128::try_from(count.saturating_sub(1)).unwrap_or(i128::MAX);
    (0..count)
        .map(|index| {
            let numerator = i128::from(inclusive_span)
                .saturating_mul(i128::try_from(index).unwrap_or(i128::MAX));
            let offset = i64::try_from(numerator / divisor).unwrap_or(inclusive_span);
            TimeCode(range.start.0.saturating_add(offset))
        })
        .collect()
}

/// Three source-monitor views per candidate. The last sample is always the
/// last visible frame, which makes an embedded fade or abrupt pre-cut visible
/// whenever the candidate is long enough to contain one. This is evidence,
/// not a claim that the server detected a fade.
fn shot_board_evidence_frames(range: std::ops::Range<TimeCode>) -> Vec<TimeCode> {
    storyboard_sample_frames(&range, SHOT_BOARD_EVIDENCE_PER_CANDIDATE)
}

/// Return stable positions from an eligible candidate list for a bounded,
/// whole-range inspection. The integer interpolation deliberately avoids
/// floating point rounding so every caller gets identical candidate ids.
fn coverage_candidate_positions(eligible_count: usize, requested_count: usize) -> Vec<usize> {
    let count = eligible_count.min(requested_count);
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![0];
    }

    let last = eligible_count.saturating_sub(1);
    let divisor = count.saturating_sub(1);
    (0..count)
        .map(|index| index.saturating_mul(last) / divisor)
        .collect()
}

fn compose_contact_sheet(
    images: &[kinewright_core::RgbaImage],
) -> Result<kinewright_core::RgbaImage, McpError> {
    let cell_width = images.iter().map(|image| image.width).max().unwrap_or(1);
    let cell_height = images.iter().map(|image| image.height).max().unwrap_or(1);
    let count = u32::try_from(images.len()).unwrap_or(u32::MAX).max(1);
    let columns = count.min(STORYBOARD_COLUMNS);
    let rows = count.div_ceil(columns);
    let width = cell_width
        .checked_mul(columns)
        .and_then(|value| {
            value.checked_add(STORYBOARD_GUTTER.saturating_mul(columns.saturating_sub(1)))
        })
        .ok_or_else(|| McpError::internal_error("storyboard width overflowed", None))?;
    let height = cell_height
        .checked_mul(rows)
        .and_then(|value| {
            value.checked_add(STORYBOARD_GUTTER.saturating_mul(rows.saturating_sub(1)))
        })
        .ok_or_else(|| McpError::internal_error("storyboard height overflowed", None))?;
    let byte_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| McpError::internal_error("storyboard allocation overflowed", None))?;
    let mut pixels = vec![16_u8; byte_count];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = u8::MAX;
    }
    for (index, image) in images.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let column = index % columns;
        let row = index / columns;
        let x = column.saturating_mul(cell_width.saturating_add(STORYBOARD_GUTTER));
        let y = row.saturating_mul(cell_height.saturating_add(STORYBOARD_GUTTER));
        for source_y in 0..image.height {
            let source_start = usize::try_from(source_y)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(image.width).unwrap_or(usize::MAX))
                .saturating_mul(4);
            let source_len = usize::try_from(image.width)
                .unwrap_or(usize::MAX)
                .saturating_mul(4);
            let destination_start = usize::try_from(y.saturating_add(source_y))
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(width).unwrap_or(usize::MAX))
                .saturating_add(usize::try_from(x).unwrap_or(usize::MAX))
                .saturating_mul(4);
            let Some(source) = image
                .pixels
                .get(source_start..source_start.saturating_add(source_len))
            else {
                return Err(McpError::internal_error(
                    "storyboard source image is truncated",
                    None,
                ));
            };
            let Some(destination) =
                pixels.get_mut(destination_start..destination_start.saturating_add(source_len))
            else {
                return Err(McpError::internal_error(
                    "storyboard destination image overflowed",
                    None,
                ));
            };
            destination.copy_from_slice(source);
        }
    }
    Ok(kinewright_core::RgbaImage {
        width,
        height,
        pixels,
    })
}

fn state_delta(
    tool_name: &str,
    before: Option<&Document>,
    after: &Document,
    revision: TimelineRevision,
) -> String {
    let counts = |document: &Document| {
        (
            document.tracks.len(),
            document
                .tracks
                .iter()
                .map(|track| track.clips.len())
                .sum::<usize>(),
            document.media_pool.len(),
        )
    };
    let (after_tracks, after_clips, after_assets) = counts(after);
    if let Some(before) = before {
        let (before_tracks, before_clips, before_assets) = counts(before);
        format!(
            "applied {tool_name}; timeline_revision={revision}; tracks {before_tracks}->{after_tracks}, clips {before_clips}->{after_clips}, assets {before_assets}->{after_assets}, duration {}f->{}f",
            before.duration.0, after.duration.0
        )
    } else {
        format!(
            "applied {tool_name}; timeline_revision={revision}; tracks={after_tracks}, clips={after_clips}, assets={after_assets}, duration={}f",
            after.duration.0
        )
    }
}

fn revision_conflict_text(expected: TimelineRevision, actual: TimelineRevision) -> CallToolResult {
    error_text(format!(
        "timeline revision conflict: expected {expected}, actual {actual}; call get_timeline_state and re-plan against the current revision"
    ))
}

fn validated_timeline_range(
    document: &Document,
    requested: Option<TranscriptRangeArgs>,
    label: &str,
) -> Result<std::ops::Range<TimeCode>, McpError> {
    let range = requested.map_or(TimeCode::ZERO..document.duration, |range| {
        range.start..range.end
    });
    if range.start < TimeCode::ZERO || range.end <= range.start || range.end > document.duration {
        return Err(McpError::invalid_params(
            format!(
                "{label} range {}..{} is outside project range 0..{}",
                range.start.0, range.end.0, document.duration.0
            ),
            None,
        ));
    }
    Ok(range)
}

// The validated 0..=100 percentage is intentionally rounded to integer basis points.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn confidence_to_basis_points(confidence: f64) -> Result<u16, String> {
    percentage_to_basis_points(confidence, "min_confidence")
}

// The validated 0..=100 percentage is intentionally rounded to integer basis points.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percentage_to_basis_points(value: f64, field: &str) -> Result<u16, String> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(format!("{field} must be between 0 and 100 percent"));
    }
    Ok((value * 100.0).round() as u16)
}

fn render_asset_beats(
    asset: AssetId,
    status: &BeatStatus,
    minimum_strength_basis_points: u16,
) -> CallToolResult {
    match status {
        BeatStatus::NotRequested => {
            success_text(format!("asset {asset} beats status=not-requested"))
        }
        BeatStatus::Queued => success_text(format!("asset {asset} beats status=queued")),
        BeatStatus::Hashing => success_text(format!("asset {asset} beats status=hashing")),
        BeatStatus::Analyzing { progress_percent } => success_text(progress_percent.map_or_else(
            || format!("asset {asset} beats status=analyzing"),
            |progress| format!("asset {asset} beats status=analyzing progress={progress}%"),
        )),
        BeatStatus::NoAudio => success_text(format!("asset {asset} beats: no audio stream")),
        BeatStatus::Cancelled => success_text(format!("asset {asset} beats status=cancelled")),
        BeatStatus::Failed(error) => {
            error_text(format!("asset {asset} beats status=failed error={error:?}"))
        }
        BeatStatus::Ready(beats) => {
            let selected = beats
                .beats
                .iter()
                .copied()
                .filter(|beat| beat.strength_basis_points >= minimum_strength_basis_points)
                .collect::<Vec<_>>();
            let mut output = format!(
                "asset {asset} beats fps={}/{} bpm={:.3} min_strength={:.2}% onsets={}\n",
                beats.source_fps.numerator(),
                beats.source_fps.denominator(),
                f64::from(beats.estimated_bpm_milli) / 1_000.0,
                f64::from(minimum_strength_basis_points) / 100.0,
                selected.len()
            );
            for beat in &selected {
                let _ = writeln!(
                    output,
                    "{}f strength={:.2}%",
                    beat.source_frame.0,
                    f64::from(beat.strength_basis_points) / 100.0
                );
            }
            output.pop();
            success_structured(
                output,
                serde_json::json!({
                    "asset_id": asset.0,
                    "source_fps": beats.source_fps,
                    "estimated_bpm_milli": beats.estimated_bpm_milli,
                    "minimum_strength_basis_points": minimum_strength_basis_points,
                    "beats": selected,
                }),
            )
        }
    }
}

fn render_timeline_beats(
    document: &Document,
    range: &std::ops::Range<TimeCode>,
    beats: &[TimelineBeat],
    pending: &[u64],
) -> String {
    let mut output = format!(
        "timeline beats range={}..{} fps={}/{} onsets={} pending_assets={pending:?}\n",
        range.start.0,
        range.end.0,
        document.fps.numerator(),
        document.fps.denominator(),
        beats.len()
    );
    for beat in beats {
        let _ = writeln!(
            output,
            "clip={} asset={} project={}f source={}f strength={:.2}% bpm={:.3}",
            beat.clip,
            beat.asset,
            beat.project_frame.0,
            beat.source_frame.0,
            f64::from(beat.strength_basis_points) / 100.0,
            f64::from(beat.estimated_bpm_milli) / 1_000.0,
        );
    }
    output.pop();
    output
}

fn plan_confirmation_description(document: &Document, operations: &[Operation]) -> Option<String> {
    let mut candidate = document.clone();
    let mut removed_clips = 0_usize;
    let mut removed_tracks = 0_usize;
    for operation in operations {
        match operation {
            Operation::DeleteClip { clip } | Operation::RippleDeleteClip { clip }
                if candidate.clip(*clip).is_some() =>
            {
                removed_clips = removed_clips.saturating_add(1);
            }
            Operation::RemoveTrack { track } => {
                let existing = candidate
                    .tracks
                    .iter()
                    .find(|candidate| candidate.id == *track)?;
                if !existing.clips.is_empty() {
                    removed_tracks = removed_tracks.saturating_add(1);
                    removed_clips = removed_clips.saturating_add(existing.clips.len());
                }
            }
            _ => {}
        }
        if operation.apply(&mut candidate).is_err() {
            return None;
        }
    }
    if removed_clips == 0 && removed_tracks == 0 {
        return None;
    }
    Some(format!(
        "Plan removes {removed_clips} {} and {removed_tracks} {} - approve?",
        if removed_clips == 1 { "clip" } else { "clips" },
        if removed_tracks == 1 {
            "track"
        } else {
            "tracks"
        },
    ))
}

fn render_plan_outcomes(
    operations: &[Operation],
    error: Option<&kinewright_core::BatchError>,
    summary: Option<String>,
) -> String {
    let failed = match error {
        Some(kinewright_core::BatchError::OperationFailed { op_number, .. }) => Some(*op_number),
        _ => None,
    };
    let mut output = String::new();
    if error.is_none() && operations.len() > 8 {
        let mut counts = BTreeMap::new();
        for operation in operations {
            *counts
                .entry(operation_tool_name(operation))
                .or_insert(0_usize) += 1;
        }
        let breakdown = counts
            .into_iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            output,
            "applied {} operations atomically ({breakdown})",
            operations.len()
        );
    } else {
        for (index, operation) in operations.iter().enumerate() {
            let number = index + 1;
            let outcome = match (error, failed) {
                (None, _) => "applied".to_owned(),
                (Some(kinewright_core::BatchError::Empty), _) => "not run: empty plan".to_owned(),
                (
                    Some(kinewright_core::BatchError::OperationFailed { error, .. }),
                    Some(failed),
                ) if number == failed => {
                    format!("rejected: {error}")
                }
                (Some(_), Some(failed)) if number < failed => "rolled back".to_owned(),
                _ => "not run".to_owned(),
            };
            let _ = writeln!(
                output,
                "op {number} {}: {outcome}",
                operation_tool_name(operation)
            );
        }
    }
    if let Some(error) = error {
        let _ = writeln!(output, "plan rejected atomically: {error}");
    }
    if let Some(summary) = summary {
        let _ = writeln!(output, "{summary}");
    }
    if output.ends_with('\n') {
        output.pop();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{
        AssetBeats, AssetId, AssetSceneChanges, AssetTranscript, BeatMarker, Clip, FrameTexture,
        Marker, MarkerId, MediaAsset, MediaError, MediaEvent, MediaKind, ParamValue, Rational,
        RgbaImage, SceneChange, SceneStatus, SilenceSpan, SilenceStatus, TimelineSceneChange,
        TimelineSilenceSpan, Title, Track, TrackId, TrackKind, TranscriptWord, VisualAssetResult,
    };
    use serde_json::json;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
        time::Instant,
    };

    #[derive(Default)]
    struct NoopMedia {
        transcript: Option<Arc<AssetTranscript>>,
        beat_statuses: BTreeMap<AssetId, BeatStatus>,
        scene_statuses: BTreeMap<AssetId, SceneStatus>,
        timeline_beats: Vec<TimelineBeat>,
        timeline_beat_error: Option<String>,
        beat_requests: Mutex<Vec<AssetId>>,
        scene_requests: Mutex<Vec<AssetId>>,
    }

    impl Playback for NoopMedia {
        fn set_document(&self, _doc: Arc<Document>) {}

        fn request_frame(&self, _t: TimeCode) {}

        fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, FrameTexture)> {
            crossbeam_channel::never()
        }

        fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
            crossbeam_channel::never()
        }

        fn play(&self, _from: TimeCode) {}

        fn pause(&self) {}

        fn seek(&self, _to: TimeCode) {}

        fn position(&self) -> TimeCode {
            TimeCode::ZERO
        }

        fn output_peaks(&self) -> [f32; 2] {
            [0.0; 2]
        }
    }

    impl Analysis for NoopMedia {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn request_transcription(&self, _asset: MediaAsset) {}

        fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus {
            self.transcript
                .as_ref()
                .map_or(TranscriptStatus::NotRequested, |transcript| {
                    if transcript.asset == asset.id {
                        TranscriptStatus::Ready(Arc::clone(transcript))
                    } else {
                        TranscriptStatus::NotRequested
                    }
                })
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: MediaAsset) {}

        fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
            SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, asset: MediaAsset) {
            self.scene_requests.lock().unwrap().push(asset.id);
        }

        fn scene_status(&self, asset: &MediaAsset) -> SceneStatus {
            self.scene_statuses
                .get(&asset.id)
                .cloned()
                .unwrap_or(SceneStatus::NotRequested)
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
        }

        fn request_beat_detection(&self, asset: MediaAsset) {
            self.beat_requests.lock().unwrap().push(asset.id);
        }

        fn beat_status(&self, asset: &MediaAsset) -> BeatStatus {
            self.beat_statuses
                .get(&asset.id)
                .cloned()
                .unwrap_or(BeatStatus::NotRequested)
        }

        fn timeline_beats(
            &self,
            _document: &Document,
            range: Option<std::ops::Range<TimeCode>>,
            minimum_strength_basis_points: u16,
        ) -> Result<Vec<TimelineBeat>, MediaError> {
            if let Some(error) = &self.timeline_beat_error {
                return Err(MediaError::Backend(error.clone()));
            }
            Ok(self
                .timeline_beats
                .iter()
                .copied()
                .filter(|beat| beat.strength_basis_points >= minimum_strength_basis_points)
                .filter(|beat| {
                    range.as_ref().is_none_or(|range| {
                        beat.project_frame >= range.start && beat.project_frame < range.end
                    })
                })
                .collect())
        }

        fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn thumbnail_for_document(
            &self,
            _document: Arc<Document>,
            _t: TimeCode,
            _max_w: u32,
        ) -> Result<RgbaImage, MediaError> {
            Ok(RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![0; 16],
            })
        }

        fn request_waveform(&self, _asset: MediaAsset) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
        ) -> bool {
            false
        }

        fn visual_asset_results(&self) -> crossbeam_channel::Receiver<VisualAssetResult> {
            crossbeam_channel::never()
        }
    }

    fn fixture() -> (Core, Arc<dyn Playback>, Arc<dyn Analysis>) {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
        };
        let document = Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode::ZERO..TimeCode(60),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(60),
        };
        let media = Arc::new(NoopMedia::default());
        (Core::spawn(document).unwrap(), media.clone(), media)
    }

    fn montage_analysis(status: BeatStatus) -> Arc<NoopMedia> {
        Arc::new(NoopMedia {
            beat_statuses: BTreeMap::from([(AssetId(9), status)]),
            timeline_beats: vec![TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 9_000,
                estimated_bpm_milli: 120_000,
            }],
            ..NoopMedia::default()
        })
    }

    fn montage_fixture(status: BeatStatus) -> (Core, Arc<NoopMedia>) {
        let fps = Rational::new(30, 1).unwrap();
        let video_asset = |id| MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("montage-{id}.mp4")),
            name: format!("montage-{id}"),
            duration: TimeCode(180),
            fps,
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
        };
        let music = MediaAsset {
            id: AssetId(9),
            path: PathBuf::from("montage-music.mp4"),
            name: "montage music".to_owned(),
            duration: TimeCode(180),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
        };
        let document = Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![Clip {
                        id: ClipId(90),
                        asset: music.id,
                        source_range: TimeCode::ZERO..TimeCode(120),
                        content: ClipContent::Media,
                        timeline_start: TimeCode::ZERO,
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                    }],
                },
            ],
            media_pool: vec![video_asset(1), video_asset(2), music],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(120),
            ..Document::default()
        };
        let analysis = montage_analysis(status);
        (Core::spawn(document).unwrap(), analysis)
    }

    fn music_structure_fixture(
        status: BeatStatus,
        timeline_beats: Vec<TimelineBeat>,
    ) -> (Core, Arc<NoopMedia>) {
        let fps = Rational::new(30, 1).unwrap();
        let video = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("music-structure-video.mp4"),
            name: "music structure video".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
        };
        let music = MediaAsset {
            id: AssetId(9),
            path: PathBuf::from("music-structure-audio.wav"),
            name: "music structure audio".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::Audio,
            resolution: None,
        };
        let media_clip = |id, asset, track_start| Clip {
            id: ClipId(id),
            asset: AssetId(asset),
            source_range: TimeCode::ZERO..TimeCode(120),
            content: ClipContent::Media,
            timeline_start: TimeCode(track_start),
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        let document = Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(1, 1, 0)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(90, 9, 0)],
                },
            ],
            media_pool: vec![video, music],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(120),
            ..Document::default()
        };
        let analysis = Arc::new(NoopMedia {
            beat_statuses: BTreeMap::from([(AssetId(9), status)]),
            timeline_beats,
            ..NoopMedia::default()
        });
        (Core::spawn(document).unwrap(), analysis)
    }

    fn end_anchored_music_fit_fixture() -> (Core, Arc<NoopMedia>) {
        let source_fps = Rational::new(30, 1).unwrap();
        let music = MediaAsset {
            id: AssetId(9),
            path: PathBuf::from("end-anchored-music.wav"),
            name: "end anchored music".to_owned(),
            duration: TimeCode(6_170),
            fps: source_fps,
            kind: MediaKind::Audio,
            resolution: None,
        };
        let document = Document {
            tracks: vec![Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![music],
            fps: Rational::new(25, 1).unwrap(),
            resolution: (1_920, 1_080),
            duration: TimeCode::ZERO,
            ..Document::default()
        };
        let analysis = Arc::new(NoopMedia {
            beat_statuses: BTreeMap::from([(
                AssetId(9),
                BeatStatus::Ready(Arc::new(AssetBeats {
                    asset: AssetId(9),
                    content_sha256: "end-anchored-music-test".to_owned(),
                    source_fps,
                    source_frames: TimeCode(6_170),
                    estimated_bpm_milli: 120_000,
                    beats: vec![
                        BeatMarker {
                            source_frame: TimeCode(5_160),
                            strength_basis_points: 5_638,
                        },
                        BeatMarker {
                            source_frame: TimeCode(5_161),
                            strength_basis_points: 10_000,
                        },
                    ],
                })),
            )]),
            ..NoopMedia::default()
        });
        (Core::spawn(document).unwrap(), analysis)
    }

    fn ready_music_structure_status() -> BeatStatus {
        BeatStatus::Ready(Arc::new(AssetBeats {
            asset: AssetId(9),
            content_sha256: "music-structure-test".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(120),
            estimated_bpm_milli: 120_000,
            beats: vec![
                BeatMarker {
                    source_frame: TimeCode::ZERO,
                    strength_basis_points: 9_000,
                },
                BeatMarker {
                    source_frame: TimeCode(30),
                    strength_basis_points: 5_000,
                },
                BeatMarker {
                    source_frame: TimeCode(60),
                    strength_basis_points: 8_000,
                },
            ],
        }))
    }

    fn ready_montage_status() -> BeatStatus {
        BeatStatus::Ready(Arc::new(AssetBeats {
            asset: AssetId(9),
            content_sha256: "montage-test".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(180),
            estimated_bpm_milli: 120_000,
            beats: vec![BeatMarker {
                source_frame: TimeCode(30),
                strength_basis_points: 9_000,
            }],
        }))
    }

    fn montage_plan_args() -> BeatMontagePlanArgs {
        BeatMontagePlanArgs {
            target_track_id: TrackId(1),
            music_asset_id: AssetId(9),
            timeline_range: TranscriptRangeArgs {
                start: TimeCode::ZERO,
                end: TimeCode(60),
            },
            selects: vec![
                BeatMontageSelectArgs {
                    asset_id: AssetId(1),
                    source_range: TranscriptRangeArgs {
                        start: TimeCode(10),
                        end: TimeCode(100),
                    },
                },
                BeatMontageSelectArgs {
                    asset_id: AssetId(2),
                    source_range: TranscriptRangeArgs {
                        start: TimeCode(20),
                        end: TimeCode(110),
                    },
                },
            ],
            cut_anchor_frames: None,
            anchor_repair: None,
            min_strength: None,
            minimum_shot_frames: None,
            maximum_shot_frames: None,
            cadence: None,
            mode: ThreePointMode::Overwrite,
        }
    }

    #[test]
    fn music_fit_schema_exposes_bounded_end_anchor_pair() {
        let tool = KinewrightMcp::capability_tools()
            .unwrap()
            .into_iter()
            .find(|tool| tool.name == "plan_music_fit")
            .expect("music fit capability is registered");
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("music fit schema properties");

        for property in ["preferred_source_end", "maximum_end_drift_frames"] {
            assert!(properties.contains_key(property), "missing {property}");
        }
        assert!(
            tool.description
                .as_deref()
                .unwrap_or_default()
                .contains("fails closed")
        );
    }

    #[test]
    fn music_fit_end_anchor_returns_resolved_endpoint_evidence() {
        let (core, analysis) = end_anchored_music_fit_fixture();
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );

        let result = service
            .plan_music_fit(&MusicFitPlanArgs {
                track_id: TrackId(2),
                asset_id: AssetId(9),
                timeline_range: TranscriptRangeArgs {
                    start: TimeCode::ZERO,
                    end: TimeCode(700),
                },
                preferred_source_start: Some(TimeCode(5_161)),
                preferred_source_end: Some(TimeCode(6_000)),
                maximum_end_drift_frames: Some(TimeCode::ZERO),
                min_strength: Some(0.0),
                mode: ThreePointMode::Overwrite,
            })
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["plan"]["strategy"], "end_anchored_straight_cut");
        assert_eq!(structured["plan"]["source_range"]["start"], 5_160);
        assert_eq!(structured["plan"]["source_range"]["end"], 6_000);
        assert_eq!(
            structured["plan"]["end_anchor"],
            json!({
                "target_source_end": 6_000,
                "resolved_source_end": 6_000,
                "signed_offset_frames": 0,
                "maximum_drift_frames": 0,
            })
        );
    }

    #[test]
    fn music_fit_requires_complete_and_nonnegative_end_anchor_arguments() {
        let (core, analysis) = end_anchored_music_fit_fixture();
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let base = || MusicFitPlanArgs {
            track_id: TrackId(2),
            asset_id: AssetId(9),
            timeline_range: TranscriptRangeArgs {
                start: TimeCode::ZERO,
                end: TimeCode(700),
            },
            preferred_source_start: Some(TimeCode(5_160)),
            preferred_source_end: Some(TimeCode(6_000)),
            maximum_end_drift_frames: Some(TimeCode::ZERO),
            min_strength: Some(0.0),
            mode: ThreePointMode::Overwrite,
        };

        let missing_drift = service
            .plan_music_fit(&MusicFitPlanArgs {
                maximum_end_drift_frames: None,
                ..base()
            })
            .unwrap();
        assert_eq!(missing_drift.is_error, Some(true));
        assert!(
            missing_drift.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("requires maximum_end_drift_frames")
        );

        let negative_drift = service
            .plan_music_fit(&MusicFitPlanArgs {
                maximum_end_drift_frames: Some(TimeCode(-1)),
                ..base()
            })
            .unwrap();
        assert_eq!(negative_drift.is_error, Some(true));
        assert!(
            negative_drift.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("maximum drift cannot be negative")
        );
    }

    struct CountingPlayback(AtomicUsize);

    impl Playback for CountingPlayback {
        fn set_document(&self, _doc: Arc<Document>) {
            self.0.fetch_add(1, AtomicOrdering::Relaxed);
        }

        fn request_frame(&self, _t: TimeCode) {}

        fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, FrameTexture)> {
            crossbeam_channel::never()
        }

        fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
            crossbeam_channel::never()
        }

        fn play(&self, _from: TimeCode) {}

        fn pause(&self) {}

        fn seek(&self, _to: TimeCode) {}

        fn position(&self) -> TimeCode {
            TimeCode::ZERO
        }

        fn output_peaks(&self) -> [f32; 2] {
            [0.0; 2]
        }
    }

    #[test]
    fn isolated_handler_edits_and_renders_without_publishing_to_live_playback() {
        let (core, _, analysis) = fixture();
        let playback = Arc::new(CountingPlayback(AtomicUsize::new(0)));
        let service = KinewrightMcp::configured(
            core,
            playback.clone(),
            analysis,
            None,
            ConfirmationBroker::default(),
            false,
        );
        let proof = service.frame_at(TimeCode(1)).unwrap();
        assert_eq!(proof.is_error, Some(false));
        let edit = service.apply_operation(
            "add_marker",
            TimelineRevision::default(),
            Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(1),
                    position: TimeCode(1),
                    label: "Branch".to_owned(),
                    color_token: 0,
                },
            },
        );
        assert_eq!(edit.is_error, Some(false));
        assert_eq!(playback.0.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn m31_agent_tools_expose_captions_qa_and_delivery_proofs() {
        let names = KinewrightMcp::tools()
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();
        for name in [
            "get_caption_presets",
            "get_captions",
            "get_transcripts",
            "plan_caption_corrections",
            "add_styled_captions",
            "get_qa_report",
            "get_delivery_variants",
            "get_delivery_variant_storyboard",
            "get_editorial_readiness",
        ] {
            assert!(names.iter().any(|candidate| candidate == name));
        }

        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let result = service
            .delivery_variant_storyboard(DeliveryStoryboardArgs {
                aspect: DeliveryAspect::Vertical,
                focus_x_percent: 25,
                focus_y_percent: 50,
                storyboard: StoryboardArgs {
                    range: None,
                    frame_count: Some(2),
                    max_width: Some(64),
                },
            })
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result.structured_content.unwrap()["delivery_variant"]["aspect"],
            "vertical"
        );

        let readiness = service
            .editorial_readiness(&EditorialReadinessArgs {
                profile: DeliveryProfile::VerticalShort,
                check_silence: true,
                min_silence_source_frames: Some(TimeCode(20)),
                focus_x_percent: 50,
                focus_y_percent: 50,
                storyboard: StoryboardArgs {
                    range: None,
                    frame_count: Some(2),
                    max_width: Some(64),
                },
            })
            .unwrap();
        assert_eq!(readiness.is_error, Some(false));
        assert_eq!(readiness.structured_content.unwrap()["ready"], false);

        let readiness = service
            .editorial_readiness(&EditorialReadinessArgs {
                profile: DeliveryProfile::VerticalShort,
                check_silence: false,
                min_silence_source_frames: None,
                focus_x_percent: 50,
                focus_y_percent: 50,
                storyboard: StoryboardArgs {
                    range: None,
                    frame_count: Some(2),
                    max_width: Some(64),
                },
            })
            .unwrap();
        assert_eq!(readiness.is_error, Some(false));
        let readiness = readiness.structured_content.unwrap();
        assert_eq!(readiness["silence"]["checked"], false);
        assert_eq!(readiness["silence"]["pending_asset_ids"], json!([]));
    }

    #[test]
    fn caption_inspection_and_correction_planning_are_compact_and_revision_bound() {
        let (core, playback, analysis) = fixture();
        let operations = vec![
            Operation::AddTrack {
                track: Track {
                    id: TrackId(2),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            },
            Operation::AddTitle {
                track: TrackId(2),
                at: TimeCode::ZERO,
                duration: TimeCode(30),
                title: CaptionPreset::Social.title("Map Steady the Exped"),
            },
        ];
        let event = core
            .request(Command::DoBatchIfRevision {
                expected: TimelineRevision::default(),
                operations,
            })
            .unwrap();
        let Event::DocumentChanged { revision, .. } = event else {
            panic!("caption fixture should apply");
        };
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

        let page = service
            .captions(CaptionListArgs {
                range: None,
                offset: None,
                limit: Some(1),
            })
            .unwrap();
        assert_eq!(page.is_error, Some(false));
        let page = page.structured_content.unwrap();
        assert_eq!(page["total"], 1);
        assert_eq!(page["captions"][0]["clip_id"], 2);
        assert_eq!(page["captions"][0]["text"], "Map Steady the Exped");

        let plan = service
            .plan_caption_corrections(CaptionCorrectionPlanArgs {
                expected_revision: revision,
                corrections: vec![CaptionCorrection {
                    clip_id: ClipId(2),
                    text: "River map steadies the expedition".to_owned(),
                }],
            })
            .unwrap();
        assert_eq!(plan.is_error, Some(false));
        let plan = plan.structured_content.unwrap();
        assert_eq!(plan["timeline_revision"], revision.0);
        assert_eq!(plan["prepared_edit_plan"]["plan_id"], 1);
        assert_eq!(plan["prepared_edit_plan"]["preview"]["operation_count"], 1);

        let unchanged = service
            .captions(CaptionListArgs {
                range: None,
                offset: None,
                limit: None,
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(unchanged["captions"][0]["text"], "Map Steady the Exped");

        let stale = service
            .plan_caption_corrections(CaptionCorrectionPlanArgs {
                expected_revision: TimelineRevision::default(),
                corrections: vec![CaptionCorrection {
                    clip_id: ClipId(2),
                    text: "River map steadies the expedition".to_owned(),
                }],
            })
            .unwrap();
        assert_eq!(stale.is_error, Some(true));

        let media_clip = service
            .plan_caption_corrections(CaptionCorrectionPlanArgs {
                expected_revision: revision,
                corrections: vec![CaptionCorrection {
                    clip_id: ClipId(1),
                    text: "Not a caption".to_owned(),
                }],
            })
            .unwrap();
        assert_eq!(media_clip.is_error, Some(true));

        let committed = commit_prepared_plan(&service, &plan, revision);
        assert_eq!(committed.is_error, Some(false));
        let corrected = service
            .captions(CaptionListArgs {
                range: None,
                offset: None,
                limit: None,
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            corrected["captions"][0]["text"],
            "River map steadies the expedition"
        );
    }

    fn commit_prepared_plan(
        service: &KinewrightMcp,
        plan: &serde_json::Value,
        revision: TimelineRevision,
    ) -> CallToolResult {
        service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    serde_json::json!({
                        "plan_id": plan["prepared_edit_plan"]["plan_id"],
                        "expected_revision": revision,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap()
    }

    #[test]
    fn beat_montage_returns_an_inspectable_ready_plan_without_mutating() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core.clone(),
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );

        let result = service.plan_beat_montage(&montage_plan_args()).unwrap();
        assert_eq!(result.is_error, Some(false));
        let result = result.structured_content.unwrap();
        assert_eq!(result["timeline_revision"], 0);
        assert_eq!(result["plan"]["shots"].as_array().unwrap().len(), 2);
        assert_eq!(
            result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
            30
        );
        assert_eq!(
            result["plan"]["shots"][0]["source_range"],
            json!({"start": 10, "end": 40})
        );
        assert_eq!(
            result["prepared_edit_plan"]["preview"]["operation_count"],
            2
        );

        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document query");
        };
        assert!(document.tracks[0].clips.is_empty());
    }

    #[test]
    fn beat_montage_validates_optional_cadence_before_preparing() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.cadence = Some(BeatMontageCadenceContract {
            minimum_duration_buckets: 1,
            duration_bucket_frames: TimeCode(20),
            maximum_similar_run: 2,
            similar_tolerance_frames: TimeCode(8),
        });
        let result = service
            .plan_beat_montage(&args)
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(result["cadence"]["distinct_buckets"], json!([2]));
        assert_eq!(result["cadence"]["longest_similar_run"], 2);

        args.cadence = Some(BeatMontageCadenceContract {
            minimum_duration_buckets: 3,
            duration_bucket_frames: TimeCode(20),
            maximum_similar_run: 2,
            similar_tolerance_frames: TimeCode(8),
        });
        let rejected = service.plan_beat_montage(&args).unwrap();
        assert_eq!(rejected.is_error, Some(true));
        let message = rejected.content[0].as_text().unwrap().text.as_str();
        assert!(message.contains("beat montage cadence contract rejected prepared plan"));
        assert!(message.contains("requires at least 3 distinct buckets"));
    }

    #[test]
    fn beat_montage_schema_exposes_optional_cadence_and_anchor_repair_contracts() {
        let tool = KinewrightMcp::capability_tools()
            .unwrap()
            .into_iter()
            .find(|tool| tool.name == "plan_beat_montage")
            .expect("beat montage capability is registered");
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("beat montage schema properties");
        assert!(properties.contains_key("cadence"));
        assert!(properties.contains_key("anchor_repair"));
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| description.contains("cadence contract")
                    && description.contains("remain exact unless anchor_repair"))
        );
        for field in [
            "minimum_duration_buckets",
            "duration_bucket_frames",
            "maximum_similar_run",
            "similar_tolerance_frames",
        ] {
            assert!(schema.contains(field), "cadence schema omitted {field}");
        }
        for field in ["maximum_movement_frames", "locked_anchor_indices"] {
            assert!(
                schema.contains(field),
                "anchor repair schema omitted {field}"
            );
        }
        let repair_schema = tool.input_schema["$defs"]
            .as_object()
            .and_then(|definitions| {
                definitions.values().find(|schema| {
                    schema["properties"].as_object().is_some_and(|properties| {
                        properties.contains_key("maximum_movement_frames")
                    })
                })
            })
            .expect("anchor repair definition");
        let repair_required = repair_schema["required"]
            .as_array()
            .expect("anchor repair required fields");
        assert!(
            repair_required
                .iter()
                .any(|field| field == "maximum_movement_frames")
        );
        assert!(
            repair_required
                .iter()
                .all(|field| field != "locked_anchor_indices")
        );
        assert!(
            !tool
                .input_schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|required| required.iter().any(|field| field == "cadence"))
        );
        assert!(
            !tool
                .input_schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|required| required.iter().any(|field| field == "anchor_repair"))
        );
    }

    #[test]
    fn beat_montage_preserves_explicit_model_selected_anchor() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.cut_anchor_frames = Some(vec![TimeCode(30)]);
        let result = service
            .plan_beat_montage(&args)
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
            30
        );
        assert!(result["anchor_repair"].is_null());

        args.cut_anchor_frames = Some(vec![TimeCode(31)]);
        let rejected = service.plan_beat_montage(&args).unwrap();
        assert_eq!(rejected.is_error, Some(true));
        assert!(
            rejected.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("not an eligible beat for music asset")
        );
    }

    #[test]
    fn beat_montage_repairs_preferred_anchor_with_bounded_inspectable_evidence() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core.clone(),
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.cut_anchor_frames = Some(vec![TimeCode(31)]);
        args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
            maximum_movement_frames: TimeCode(2),
            locked_anchor_indices: Vec::new(),
        });

        let result = service
            .plan_beat_montage(&args)
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
            30
        );
        assert_eq!(result["plan"]["shots"][0]["asset"], 1);
        assert_eq!(result["plan"]["shots"][1]["asset"], 2);
        assert_eq!(
            result["plan"]["shots"][0]["source_envelope"],
            json!({"start": 10, "end": 100})
        );
        assert_eq!(
            result["plan"]["shots"][1]["source_envelope"],
            json!({"start": 20, "end": 110})
        );
        assert_eq!(result["anchor_repair"]["repaired"], true);
        assert_eq!(
            result["anchor_repair"]["preferred_anchor_frames"],
            json!([31])
        );
        assert_eq!(
            result["anchor_repair"]["resolved_anchor_frames"],
            json!([30])
        );
        assert_eq!(result["anchor_repair"]["signed_delta_frames"], json!([-1]));
        assert_eq!(result["anchor_repair"]["absolute_delta_frames"], json!([1]));
        assert_eq!(result["anchor_repair"]["maximum_absolute_delta_frames"], 1);
        assert_eq!(result["anchor_repair"]["total_absolute_delta_frames"], 1);
        assert_eq!(result["anchor_repair"]["maximum_movement_frames"], 2);
        assert_eq!(
            result["prepared_edit_plan"]["preview"]["operation_count"],
            2
        );

        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document query");
        };
        assert!(document.tracks[0].clips.is_empty());
    }

    #[test]
    fn beat_montage_anchor_repair_enforces_opt_in_bounds_and_locks() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
            maximum_movement_frames: TimeCode(2),
            locked_anchor_indices: Vec::new(),
        });
        let missing_anchors = service.plan_beat_montage(&args).unwrap();
        assert_eq!(missing_anchors.is_error, Some(true));
        assert!(
            missing_anchors.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("anchor_repair requires explicit cut_anchor_frames")
        );

        args.cut_anchor_frames = Some(vec![TimeCode(31)]);
        args.anchor_repair.as_mut().unwrap().maximum_movement_frames = TimeCode(-1);
        let negative = service.plan_beat_montage(&args).unwrap();
        assert_eq!(negative.is_error, Some(true));
        assert!(
            negative.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("must be non-negative")
        );

        args.anchor_repair.as_mut().unwrap().maximum_movement_frames = TimeCode::ZERO;
        let bounded = service.plan_beat_montage(&args).unwrap();
        assert_eq!(bounded.is_error, Some(true));
        assert!(
            bounded.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("within maximum_movement_frames=0")
        );

        let settings = args.anchor_repair.as_mut().unwrap();
        settings.maximum_movement_frames = TimeCode(2);
        settings.locked_anchor_indices = vec![0];
        let locked = service.plan_beat_montage(&args).unwrap();
        assert_eq!(locked.is_error, Some(true));
        assert!(
            locked.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("not an eligible beat for music asset")
        );

        args.anchor_repair.as_mut().unwrap().locked_anchor_indices = vec![0, 0];
        let duplicates = service.plan_beat_montage(&args).unwrap();
        assert_eq!(duplicates.is_error, Some(true));
        assert!(
            duplicates.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("must be strictly increasing and unique")
        );

        args.anchor_repair.as_mut().unwrap().locked_anchor_indices = vec![1];
        let out_of_range_lock = service.plan_beat_montage(&args).unwrap();
        assert_eq!(out_of_range_lock.is_error, Some(true));
        assert!(
            out_of_range_lock.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("invalid beat montage anchor-repair settings")
        );
    }

    #[test]
    fn beat_montage_surfaces_source_capacity_and_repair_hint() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.selects[0].source_range = TranscriptRangeArgs {
            start: TimeCode::ZERO,
            end: TimeCode(20),
        };

        let rejected = service.plan_beat_montage(&args).unwrap();
        assert_eq!(rejected.is_error, Some(true));
        let message = rejected.content[0].as_text().unwrap().text.as_str();
        assert!(message.contains("can supply at most 20 project frames"));
        assert!(
            message.contains(
                "reassign this select to a shorter slot or select a larger source envelope"
            )
        );
    }

    #[test]
    fn beat_montage_reports_music_analysis_pending_and_failure_explicitly() {
        let (pending_core, pending_analysis) = montage_fixture(BeatStatus::NotRequested);
        let pending_service = KinewrightMcp::new(
            pending_core,
            pending_analysis.clone(),
            pending_analysis.clone(),
            ConfirmationBroker::default(),
        );
        let pending = pending_service
            .plan_beat_montage(&montage_plan_args())
            .unwrap();
        assert_eq!(pending.is_error, Some(true));
        assert!(
            pending.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("still pending for assets [AssetId(9)]")
        );
        assert_eq!(
            *pending_analysis.beat_requests.lock().unwrap(),
            vec![AssetId(9)]
        );

        let (failed_core, failed_analysis) =
            montage_fixture(BeatStatus::Failed("decoder stopped".to_owned()));
        let failed_service = KinewrightMcp::new(
            failed_core,
            failed_analysis.clone(),
            failed_analysis.clone(),
            ConfirmationBroker::default(),
        );
        let failed = failed_service
            .plan_beat_montage(&montage_plan_args())
            .unwrap();
        assert_eq!(failed.is_error, Some(true));
        assert!(
            failed.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("beat analysis failed: decoder stopped")
        );
        assert!(failed_analysis.beat_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn beat_montage_is_internal_and_invocable_through_the_compact_dispatcher() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        assert!(registry.iter().any(|tool| tool.name == "plan_beat_montage"));
        assert!(
            KinewrightMcp::served_tools()
                .unwrap()
                .iter()
                .all(|tool| tool.name != "plan_beat_montage")
        );

        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let invoked = service
            .call_exposed_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    json!({
                        "name": "plan_beat_montage",
                        "arguments": {
                            "target_track_id": 1,
                            "music_asset_id": 9,
                            "timeline_range": {"start": 0, "end": 60},
                            "selects": [
                                {"asset_id": 1, "source_range": {"start": 10, "end": 100}},
                                {"asset_id": 2, "source_range": {"start": 20, "end": 110}}
                            ],
                            "cut_anchor_frames": [31],
                            "anchor_repair": {
                                "maximum_movement_frames": 2,
                                "locked_anchor_indices": []
                            },
                            "cadence": {
                                "minimum_duration_buckets": 1,
                                "duration_bucket_frames": 20,
                                "maximum_similar_run": 2,
                                "similar_tolerance_frames": 8
                            }
                        }
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(invoked.is_error, Some(false));
        let invoked = invoked.structured_content.unwrap();
        assert_eq!(invoked["plan"]["shots"].as_array().unwrap().len(), 2);
        assert_eq!(invoked["anchor_repair"]["repaired"], true);
        assert_eq!(
            invoked["anchor_repair"]["preferred_anchor_frames"],
            json!([31])
        );
        assert_eq!(
            invoked["anchor_repair"]["resolved_anchor_frames"],
            json!([30])
        );
    }

    #[test]
    fn beat_montage_prepared_plan_commits_gaplessly() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core.clone(),
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let planned = service
            .plan_beat_montage(&montage_plan_args())
            .unwrap()
            .structured_content
            .unwrap();

        let committed = commit_prepared_plan(&service, &planned, TimelineRevision::default());
        assert_eq!(committed.is_error, Some(false));
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document query");
        };
        let clips = &document.tracks[0].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].timeline_start, TimeCode::ZERO);
        assert_eq!(clips[1].timeline_start, TimeCode(30));
        assert_eq!(clips[0].source_range, TimeCode(10)..TimeCode(40));
        assert_eq!(clips[1].source_range, TimeCode(20)..TimeCode(50));
    }

    #[test]
    fn music_structure_is_ready_filtered_and_does_not_mutate() {
        let beats = vec![
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode::ZERO,
                project_frame: TimeCode::ZERO,
                strength_basis_points: 9_000,
                estimated_bpm_milli: 120_000,
            },
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 4_000,
                estimated_bpm_milli: 120_000,
            },
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(60),
                project_frame: TimeCode(60),
                strength_basis_points: 8_000,
                estimated_bpm_milli: 120_000,
            },
            TimelineBeat {
                asset: AssetId(1),
                track: TrackId(1),
                clip: ClipId(1),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 10_000,
                estimated_bpm_milli: 120_000,
            },
        ];
        let (core, analysis) = music_structure_fixture(ready_music_structure_status(), beats);
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let before = service.document().unwrap();
        let result = service
            .music_structure(&MusicStructureArgs {
                music_asset_id: AssetId(9),
                range: Some(TranscriptRangeArgs {
                    start: TimeCode::ZERO,
                    end: TimeCode(100),
                }),
                min_strength: Some(50.0),
                meter_beats: Some(4),
                phrase_bars: Some(2),
                structural_only: false,
            })
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["analysis_status"], "ready");
        assert_eq!(structured["heuristic"], true);
        assert!(
            structured["disclaimer"]
                .as_str()
                .unwrap()
                .contains("not guaranteed music theory")
        );
        assert_eq!(structured["parameters"]["meter_beats"], 4);
        assert_eq!(structured["parameters"]["phrase_bars"], 2);
        assert_eq!(
            structured["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|candidate| candidate["project_frame"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 60]
        );
        assert!(
            structured["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate["asset"] == 9)
        );
        assert_eq!(&*service.document().unwrap(), &*before);
        assert!(
            service
                .prepared_plans
                .lock()
                .unwrap()
                .get(PreparedPlanId(1))
                .is_none()
        );
    }

    #[test]
    fn music_structure_structural_only_compacts_ordinary_candidates_and_reports_counts() {
        let beats = vec![
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode::ZERO,
                project_frame: TimeCode::ZERO,
                strength_basis_points: 9_000,
                estimated_bpm_milli: 120_000,
            },
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 4_000,
                estimated_bpm_milli: 120_000,
            },
            TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(60),
                project_frame: TimeCode(60),
                strength_basis_points: 8_000,
                estimated_bpm_milli: 120_000,
            },
        ];
        let (core, analysis) = music_structure_fixture(ready_music_structure_status(), beats);
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let result = service
            .music_structure(&MusicStructureArgs {
                music_asset_id: AssetId(9),
                range: Some(TranscriptRangeArgs {
                    start: TimeCode::ZERO,
                    end: TimeCode(100),
                }),
                min_strength: Some(0.0),
                meter_beats: Some(4),
                phrase_bars: Some(2),
                structural_only: true,
            })
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["structural_only"], true);
        assert_eq!(structured["total_candidate_count"], 3);
        assert_eq!(structured["returned_candidate_count"], 1);
        assert_eq!(structured["omitted_ordinary_candidate_count"], 2);
        assert!(
            structured["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate["role"] != "beat")
        );
    }

    #[test]
    fn music_structure_reports_pending_and_failed_analysis_lifecycle() {
        let (pending_core, pending_analysis) = music_structure_fixture(
            BeatStatus::NotRequested,
            vec![TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 9_000,
                estimated_bpm_milli: 120_000,
            }],
        );
        let pending_service = KinewrightMcp::new(
            pending_core,
            pending_analysis.clone(),
            pending_analysis.clone(),
            ConfirmationBroker::default(),
        );
        let pending = pending_service
            .music_structure(&MusicStructureArgs {
                music_asset_id: AssetId(9),
                range: None,
                min_strength: None,
                meter_beats: None,
                phrase_bars: None,
                structural_only: false,
            })
            .unwrap();
        assert_eq!(pending.is_error, Some(true));
        assert!(
            pending.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("still pending for assets [AssetId(9)]")
        );
        assert_eq!(
            *pending_analysis.beat_requests.lock().unwrap(),
            vec![AssetId(9)]
        );

        let (failed_core, failed_analysis) =
            music_structure_fixture(BeatStatus::Failed("decoder stopped".to_owned()), Vec::new());
        let failed_service = KinewrightMcp::new(
            failed_core,
            failed_analysis.clone(),
            failed_analysis.clone(),
            ConfirmationBroker::default(),
        );
        let failed = failed_service
            .music_structure(&MusicStructureArgs {
                music_asset_id: AssetId(9),
                range: None,
                min_strength: None,
                meter_beats: None,
                phrase_bars: None,
                structural_only: false,
            })
            .unwrap();
        assert_eq!(failed.is_error, Some(true));
        assert!(
            failed.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("beat analysis failed: decoder stopped")
        );
        assert!(failed_analysis.beat_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn music_structure_is_internal_and_invocable_through_compact_dispatcher() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        let tool = registry
            .iter()
            .find(|tool| tool.name == "get_music_structure")
            .expect("music structure is registered");
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("music structure schema properties");
        assert!(properties.contains_key("structural_only"));
        assert!(
            KinewrightMcp::served_tools()
                .unwrap()
                .iter()
                .all(|tool| tool.name != "get_music_structure")
        );

        let (core, analysis) = music_structure_fixture(
            ready_music_structure_status(),
            vec![TimelineBeat {
                asset: AssetId(9),
                track: TrackId(2),
                clip: ClipId(90),
                source_frame: TimeCode(30),
                project_frame: TimeCode(30),
                strength_basis_points: 9_000,
                estimated_bpm_milli: 120_000,
            }],
        );
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let invoked = service
            .call_exposed_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    json!({
                        "name": "get_music_structure",
                        "arguments": {
                            "music_asset_id": 9,
                            "range": {"start": 0, "end": 90},
                            "min_strength": 0,
                            "meter_beats": 4,
                            "phrase_bars": 4,
                            "structural_only": true
                        }
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(invoked.is_error, Some(false));
        let structured = invoked.structured_content.unwrap();
        assert_eq!(structured["structural_only"], true);
        assert_eq!(structured["total_candidate_count"], 1);
        assert_eq!(structured["returned_candidate_count"], 1);
        assert_eq!(structured["omitted_ordinary_candidate_count"], 0);
        assert_eq!(structured["candidates"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn m34_agent_tools_expose_creator_plans_tracking_and_delivery_jobs() {
        let tools = KinewrightMcp::tools().unwrap();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<BTreeSet<_>>();
        for name in [
            "plan_beat_pacing",
            "plan_music_fit",
            "plan_speaker_multicam",
            "track_reframe_subject",
            "get_delivery_profiles",
            "get_delivery_conformance",
            "queue_export",
            "get_export_jobs",
            "cancel_export",
        ] {
            assert!(names.contains(name), "missing M34 tool {name}");
        }

        let mut registered = crate::schema::capability_tool_names().unwrap();
        registered.sort_unstable();
        let mut served = tools
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();
        served.sort_unstable();
        assert_eq!(registered, served);
    }

    #[test]
    fn source_path_guard_resolves_a_nonexistent_destination_through_dot_dot() {
        let directory =
            std::env::temp_dir().join(format!("kinewright-source-guard-{}", std::process::id()));
        let nested = directory.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let source = directory.join("source.mp4");
        std::fs::write(&source, b"source").unwrap();
        let aliased = nested.join("..").join("source.mp4");

        assert!(paths_resolve_equal(&aliased, &source));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn m32_tools_expose_professional_edits_source_monitor_and_faceted_search() {
        let names = KinewrightMcp::tools()
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();
        for name in [
            "three_point_edit",
            "slip_clip",
            "roll_edit",
            "slide_clip",
            "replace_clip",
            "fit_to_fill",
            "get_source_info",
            "search_media",
        ] {
            assert!(names.iter().any(|candidate| candidate == name));
        }

        let (core, playback, _) = fixture();
        let transcript = Arc::new(AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            words: vec![TranscriptWord {
                text: "wedding vows".to_owned(),
                source_start: TimeCode(12),
                source_end: TimeCode(24),
                speaker: Some("Partner".to_owned()),
            }],
        });
        let analysis: Arc<dyn Analysis> = Arc::new(NoopMedia {
            transcript: Some(transcript),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

        let source = service
            .source_info(&SourceInfoArgs {
                asset_id: AssetId(1),
                source_in: Some(TimeCode(10)),
                source_out: Some(TimeCode(30)),
            })
            .unwrap();
        assert_eq!(source.is_error, Some(false));
        let source = source.structured_content.unwrap();
        assert_eq!(source["source_monitor"]["duration"], 20);
        assert_eq!(source["words"][0]["speaker"], "Partner");

        let search = service
            .search_media(&MediaSearchArgs {
                query: Some("vows".to_owned()),
                speaker: Some("partner".to_owned()),
                kind: Some(MediaKind::Video),
                min_width: Some(320),
                min_height: Some(180),
                min_duration_frames: Some(TimeCode(60)),
                min_scene_count: None,
                min_beat_count: None,
                has_transcript: Some(true),
                limit: Some(10),
            })
            .unwrap();
        assert_eq!(search.is_error, Some(false));
        let search = search.structured_content.unwrap();
        assert_eq!(search["total_matches"], 1);
        assert_eq!(search["hits"][0]["word_matches"][0]["source_start"], 12);
    }

    fn delete_request() -> CallToolRequestParams {
        CallToolRequestParams::new("delete_clip").with_arguments(
            json!({"expected_revision": 0, "clip": 1})
                .as_object()
                .unwrap()
                .clone(),
        )
    }

    fn plan_request(operations: serde_json::Value) -> CallToolRequestParams {
        CallToolRequestParams::new("apply_edit_plan").with_arguments(serde_json::Map::from_iter([
            ("expected_revision".to_owned(), json!(0)),
            ("operations".to_owned(), operations),
        ]))
    }

    fn wait_for_request(broker: &ConfirmationBroker) -> ConfirmationRequest {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(request) = broker.pending_requests().into_iter().next() {
                return request;
            }
            assert!(
                Instant::now() < deadline,
                "confirmation request was not published"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn invoke_in_background(
        service: KinewrightMcp,
        request: CallToolRequestParams,
    ) -> crossbeam_channel::Receiver<Result<CallToolResult, McpError>> {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        thread::spawn(move || {
            let _ = sender.send(service.call_blocking(request));
        });
        receiver
    }

    #[test]
    fn approved_confirmation_applies_the_operation() {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let result = invoke_in_background(
            KinewrightMcp::new(core.clone(), playback, analysis, broker.clone()),
            delete_request(),
        );
        let request = wait_for_request(&broker);
        assert!(broker.approve(request.id));
        let result = result
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document query result");
        };
        assert!(document.clip(ClipId(1)).is_none());
    }

    #[test]
    fn rejected_confirmation_returns_a_refusal_tool_result() {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let result = invoke_in_background(
            KinewrightMcp::new(core, playback, analysis, broker.clone()),
            delete_request(),
        );
        let request = wait_for_request(&broker);
        assert!(broker.reject(request.id, "rejected by user"));
        let result = result
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("rejected by user")
        );
    }

    #[test]
    fn confirmation_timeout_rejects_the_operation() {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_millis(10));
        let service = KinewrightMcp::new(core, playback, analysis, broker);
        let result = service.call_blocking(delete_request()).unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("timed out")
        );
    }

    #[test]
    fn interrupting_a_pending_confirmation_does_not_deadlock() {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(30));
        let result = invoke_in_background(
            KinewrightMcp::new(core, playback, analysis, broker.clone()),
            delete_request(),
        );
        let _request = wait_for_request(&broker);
        broker.reject_all("session interrupted");
        let result = result
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("session interrupted")
        );
    }

    #[test]
    fn removing_a_nonempty_track_requires_confirmation() {
        let (core, playback, analysis) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let request = CallToolRequestParams::new("remove_track").with_arguments(
            json!({"expected_revision": 0, "track": 1})
                .as_object()
                .unwrap()
                .clone(),
        );
        let result = invoke_in_background(
            KinewrightMcp::new(core, playback, analysis, broker.clone()),
            request,
        );
        let request = wait_for_request(&broker);
        assert!(request.description.contains("1 clip(s)"));
        assert!(broker.reject(request.id, "keep the track"));
        let result = result
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn ripple_delete_is_destructive_while_marker_and_title_edits_are_suggestions() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let document = service.document().unwrap();
        assert!(
            KinewrightMcp::confirmation_description(
                &document,
                &Operation::RippleDeleteClip { clip: ClipId(1) },
            )
            .is_some()
        );
        for operation in [
            Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(1),
                    position: TimeCode(5),
                    label: "Review".to_owned(),
                    color_token: 0,
                },
            },
            Operation::MoveMarker {
                marker: MarkerId(1),
                to: TimeCode(10),
            },
            Operation::RemoveMarker {
                marker: MarkerId(1),
            },
            Operation::AddTitle {
                track: TrackId(1),
                at: TimeCode(60),
                duration: TimeCode(30),
                title: Title::default(),
            },
            Operation::SetTitleParam {
                clip: ClipId(1),
                name: "text".to_owned(),
                value: ParamValue::Text("Title".to_owned()),
            },
        ] {
            assert!(KinewrightMcp::confirmation_description(&document, &operation).is_none());
        }
    }

    #[test]
    fn generated_plan_schema_composes_the_operation_schema() {
        let tool = KinewrightMcp::tools()
            .unwrap()
            .into_iter()
            .find(|tool| tool.name == "apply_edit_plan")
            .unwrap();
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema.contains("AddTrack"));
        assert!(schema.contains("DeleteClip"));
        assert!(schema.contains("operations"));
    }

    #[test]
    fn served_surface_is_small_and_keeps_the_internal_registry_discoverable() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        let served = KinewrightMcp::served_tools().unwrap();
        let names = served
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(names, crate::runtime::COMPACT_TOOL_NAMES);

        let registry_metrics = ToolSurfaceMetrics::measure(&registry);
        let served_metrics = ToolSurfaceMetrics::measure(&served);
        println!("registry={registry_metrics:?} served={served_metrics:?}");
        assert!(served_metrics.tool_count < registry_metrics.tool_count / 4);
        assert!(served_metrics.serialized_bytes < registry_metrics.serialized_bytes / 4);

        let catalog = capabilities(&registry);
        assert!(
            catalog
                .iter()
                .any(|capability| capability.name == "split_clip")
        );
        assert!(
            catalog
                .iter()
                .any(|capability| capability.name == "get_timeline_storyboard")
        );
    }

    #[test]
    fn compact_prepare_and_commit_is_revision_gated_and_atomic() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::configured(
            core.clone(),
            playback,
            analysis,
            None,
            ConfirmationBroker::default(),
            true,
        );
        let prepared = service
            .call_blocking(
                CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                    json!({
                        "expected_revision": 0,
                        "operations": [{"op": "split_clip", "clip": 1, "at": 30}]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(prepared.is_error, Some(false));
        let prepared = prepared.structured_content.unwrap();
        assert_eq!(prepared["preview"]["operation_count"], 1);
        assert_eq!(prepared["preview"]["before_clips"], 1);
        assert_eq!(prepared["preview"]["after_clips"], 2);

        let Event::QueryResult(QueryResult::Document(before_commit)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document query result");
        };
        assert_eq!(before_commit.tracks[0].clips.len(), 1);

        let committed = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": prepared["plan_id"],
                        "expected_revision": 0
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(committed.is_error, Some(false));
        let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
            core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("expected snapshot query result");
        };
        assert_eq!(revision, TimelineRevision(1));
        assert_eq!(document.tracks[0].clips.len(), 2);

        let duplicate = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": prepared["plan_id"],
                        "expected_revision": 0
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(duplicate.is_error, Some(true));
    }

    #[test]
    fn compact_capability_dispatcher_opens_and_invokes_existing_inspectors() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::configured(
            core,
            playback,
            analysis,
            None,
            ConfirmationBroker::default(),
            true,
        );
        let opened = service
            .call_blocking(
                CallToolRequestParams::new("get_capability").with_arguments(
                    json!({"name": "get_clip_info"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap();
        assert_eq!(opened.is_error, Some(false));
        assert_eq!(
            opened.structured_content.unwrap()["invocation"],
            "invoke_capability"
        );

        let invoked = service
            .call_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    json!({
                        "name": "get_clip_info",
                        "arguments": {"clip_id": 1}
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(invoked.is_error, Some(false));
        assert!(
            invoked.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("clip 1")
        );
    }

    #[test]
    fn compact_agent_plan_operations_decode_without_a_rust_enum_envelope() {
        let decoded = decode_plan_operation_value(json!({
            "op": "split_clip",
            "clip": 1,
            "at": 30
        }))
        .unwrap();
        assert_eq!(
            decoded,
            Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(30),
            }
        );
        let snake_envelope = decode_plan_operation_value(json!({"add_marker": {"marker": {
            "id": 1, "position": 30, "label": "proof", "color_token": 0
        }}}))
        .unwrap();
        assert!(matches!(snake_envelope, Operation::AddMarker { .. }));
    }

    #[test]
    fn storyboard_sampling_is_bounded_uniform_and_includes_visible_edges() {
        assert_eq!(
            storyboard_sample_frames(&(TimeCode(0)..TimeCode(10)), 4),
            [TimeCode(0), TimeCode(3), TimeCode(6), TimeCode(9)]
        );
        assert_eq!(
            storyboard_sample_frames(&(TimeCode(0)..TimeCode(10)), 1),
            [TimeCode(4)]
        );
    }

    #[test]
    fn contact_sheet_preserves_cells_and_uses_a_dark_opaque_gutter() {
        let red = kinewright_core::RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 255, 0, 0, 255],
        };
        let blue = kinewright_core::RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 255, 255, 0, 0, 255, 255],
        };
        let sheet = compose_contact_sheet(&[red, blue]).unwrap();
        assert_eq!(sheet.width, 2 * 2 + STORYBOARD_GUTTER);
        assert_eq!(sheet.height, 1);
        assert_eq!(&sheet.pixels[..4], &[255, 0, 0, 255]);
        let gutter = 2_usize * 4;
        assert_eq!(&sheet.pixels[gutter..gutter + 4], &[16, 16, 16, 255]);
        let blue_start = usize::try_from(2 + STORYBOARD_GUTTER).unwrap() * 4;
        assert_eq!(&sheet.pixels[blue_start..blue_start + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn source_storyboard_maps_cells_to_exact_source_frames_without_mutating_timeline() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let before = service.document().unwrap();
        let result = service
            .call_blocking(
                CallToolRequestParams::new("get_source_storyboard").with_arguments(
                    json!({
                        "asset_id": 1,
                        "range": {"start": 10, "end": 50},
                        "frame_count": 4,
                        "max_width": 64
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert!(
            result
                .content
                .iter()
                .any(|block| block.as_image().is_some())
        );
        let manifest = result.structured_content.unwrap();
        assert_eq!(manifest["asset_id"], 1);
        assert_eq!(manifest["source_range"], json!({"start": 10, "end": 50}));
        assert_eq!(manifest["sheet"], json!({"width": 20, "height": 2}));
        let cells = manifest["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 4);
        assert_eq!(
            cells
                .iter()
                .map(|cell| cell["source_frame"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [10, 23, 36, 49]
        );
        for cell in cells {
            assert_eq!(cell["asset_id"], 1);
            assert_eq!(cell["source_range"], json!({"start": 10, "end": 50}));
        }
        assert_eq!(&*service.document().unwrap(), &*before);
    }

    #[test]
    fn source_storyboard_rejects_missing_nonvideo_and_invalid_requests() {
        let (core, playback, analysis) = fixture();
        core.request(Command::Do(Operation::AddAsset {
            asset: MediaAsset {
                id: AssetId(2),
                path: PathBuf::from("fixture.wav"),
                name: "fixture audio".to_owned(),
                duration: TimeCode(60),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Audio,
                resolution: None,
            },
        }))
        .unwrap();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        for args in [
            SourceStoryboardArgs {
                asset_id: AssetId(999),
                range: None,
                frame_count: None,
                max_width: None,
            },
            SourceStoryboardArgs {
                asset_id: AssetId(2),
                range: None,
                frame_count: None,
                max_width: None,
            },
            SourceStoryboardArgs {
                asset_id: AssetId(1),
                range: Some(TranscriptRangeArgs {
                    start: TimeCode(40),
                    end: TimeCode(40),
                }),
                frame_count: None,
                max_width: None,
            },
            SourceStoryboardArgs {
                asset_id: AssetId(1),
                range: Some(TranscriptRangeArgs {
                    start: TimeCode(0),
                    end: TimeCode(61),
                }),
                frame_count: None,
                max_width: None,
            },
            SourceStoryboardArgs {
                asset_id: AssetId(1),
                range: None,
                frame_count: Some(STORYBOARD_MAX_FRAMES + 1),
                max_width: None,
            },
            SourceStoryboardArgs {
                asset_id: AssetId(1),
                range: None,
                frame_count: None,
                max_width: Some(THUMBNAIL_MAX_WIDTH + 1),
            },
        ] {
            let result = service.source_storyboard(&args).unwrap();
            assert_eq!(result.is_error, Some(true));
        }
    }

    #[test]
    fn source_storyboard_is_internal_registry_capability_not_compact_tool() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        assert!(
            registry
                .iter()
                .any(|tool| tool.name == "get_source_storyboard")
        );
        let served = KinewrightMcp::served_tools().unwrap();
        assert!(
            served
                .iter()
                .all(|tool| tool.name != "get_source_storyboard")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn source_shot_board_segments_exact_scenes_pages_evidence_and_does_not_mutate() {
        let (core, playback, _) = fixture();
        let analysis = Arc::new(NoopMedia {
            scene_statuses: BTreeMap::from([(
                AssetId(1),
                SceneStatus::Ready(Arc::new(AssetSceneChanges {
                    asset: AssetId(1),
                    content_sha256: "fixture".to_owned(),
                    source_fps: Rational::new(30, 1).unwrap(),
                    source_frames: TimeCode(60),
                    proxy_width: 160,
                    changes: vec![
                        SceneChange {
                            source_frame: TimeCode(10),
                            confidence_basis_points: 9_100,
                        },
                        SceneChange {
                            source_frame: TimeCode(20),
                            confidence_basis_points: DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS - 1,
                        },
                        SceneChange {
                            source_frame: TimeCode(30),
                            confidence_basis_points: 8_200,
                        },
                        SceneChange {
                            source_frame: TimeCode(45),
                            confidence_basis_points: 7_300,
                        },
                    ],
                })),
            )]),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let before = service.document().unwrap();
        let result = service
            .call_blocking(
                CallToolRequestParams::new("get_source_shot_board").with_arguments(
                    json!({
                        "asset_id": 1,
                        "range": {"start": 5, "end": 50},
                        "candidate_offset": 1,
                        "candidate_count": 2,
                        "max_width": 64,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert!(
            result
                .content
                .iter()
                .any(|block| block.as_image().is_some())
        );
        let manifest = result.structured_content.unwrap();
        assert_eq!(manifest["status"], "ready");
        assert_eq!(
            manifest["scene_confidence_threshold_basis_points"],
            DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS
        );
        assert_eq!(manifest["total_candidates"], 4);
        assert_eq!(manifest["next_candidate_offset"], 3);
        assert_eq!(manifest["sheet"], json!({"width": 20, "height": 8}));
        assert_eq!(
            manifest["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|candidate| candidate["source_range"].clone())
                .collect::<Vec<_>>(),
            vec![
                json!({"start": 10, "end": 30}),
                json!({"start": 30, "end": 45})
            ]
        );
        assert_eq!(
            manifest["candidates"][0]["boundary_provenance"]["start"]["confidence_basis_points"],
            9_100
        );
        assert_eq!(
            manifest["candidates"][1]["boundary_provenance"]["end"]["confidence_basis_points"],
            7_300
        );
        let cells = manifest["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 6);
        assert_eq!(
            cells
                .iter()
                .map(|cell| cell["source_frame"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![10, 19, 29, 30, 37, 44]
        );
        assert_eq!(
            cells[0]["candidate_id"],
            manifest["candidates"][0]["candidate_id"]
        );
        assert_eq!(
            cells[3]["candidate_id"],
            manifest["candidates"][1]["candidate_id"]
        );
        assert_eq!(&*service.document().unwrap(), &*before);

        let filtered = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: Some(1),
                minimum_duration_frames: Some(TimeCode(15)),
                minimum_confidence_basis_points: None,
                candidate_count: Some(1),
                max_width: Some(64),
            })
            .unwrap();
        assert_eq!(filtered.is_error, Some(false));
        let filtered_manifest = filtered.structured_content.unwrap();
        assert_eq!(filtered_manifest["minimum_duration_frames"], 15);
        assert_eq!(filtered_manifest["total_candidates"], 4);
        assert_eq!(filtered_manifest["filtered_candidates"], 3);
        assert_eq!(filtered_manifest["returned_candidates"], 1);
        assert_eq!(filtered_manifest["next_candidate_offset"], 2);
        assert_eq!(filtered_manifest["candidates"][0]["candidate_index"], 2);
        assert_eq!(
            filtered_manifest["candidates"][0]["candidate_id"],
            "asset-1-scene-30-45"
        );
        let strong_only = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: Some(8_000),
                candidate_count: Some(12),
                max_width: Some(64),
            })
            .unwrap();
        let strong_manifest = strong_only.structured_content.unwrap();
        assert_eq!(
            strong_manifest["scene_confidence_threshold_basis_points"],
            8_000
        );
        assert_eq!(strong_manifest["total_candidates"], 3);

        let coverage = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: Some(3),
                max_width: Some(64),
            })
            .unwrap();
        let coverage_manifest = coverage.structured_content.unwrap();
        assert_eq!(coverage_manifest["candidate_selection"], "coverage");
        assert_eq!(
            coverage_manifest["candidate_offset"],
            serde_json::Value::Null
        );
        assert_eq!(
            coverage_manifest["next_candidate_offset"],
            serde_json::Value::Null
        );
        assert_eq!(
            coverage_manifest["selected_eligible_candidate_positions"],
            json!([0, 1, 3])
        );
        assert_eq!(
            coverage_manifest["selected_candidate_indexes"],
            json!([0, 1, 3])
        );
        let coverage_ids = coverage_manifest["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| candidate["candidate_id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            coverage_ids,
            vec![
                json!("asset-1-scene-0-10"),
                json!("asset-1-scene-10-30"),
                json!("asset-1-scene-45-60"),
            ]
        );
        let coverage_again = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: Some(3),
                max_width: Some(64),
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            coverage_again["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|candidate| candidate["candidate_id"].clone())
                .collect::<Vec<_>>(),
            coverage_ids
        );

        let single_coverage = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: Some(1),
                max_width: Some(64),
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            single_coverage["selected_eligible_candidate_positions"],
            json!([0])
        );
        assert_eq!(single_coverage["candidates"][0]["candidate_index"], 0);
    }

    #[test]
    fn coverage_candidate_positions_span_full_range_without_duplicates() {
        assert_eq!(coverage_candidate_positions(10, 4), vec![0, 3, 6, 9]);
        assert_eq!(coverage_candidate_positions(3, 12), vec![0, 1, 2]);
        assert_eq!(coverage_candidate_positions(10, 1), vec![0]);
        assert!(coverage_candidate_positions(0, 6).is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn source_shot_board_requests_pending_scene_analysis_and_reports_invalid_states() {
        let (core, playback, _) = fixture();
        let analysis = Arc::new(NoopMedia::default());
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            analysis.clone(),
            ConfirmationBroker::default(),
        );
        let pending = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: None,
                max_width: None,
            })
            .unwrap();
        assert_eq!(pending.is_error, Some(false));
        assert_eq!(pending.structured_content.unwrap()["status"], "pending");
        assert_eq!(&*analysis.scene_requests.lock().unwrap(), &[AssetId(1)]);

        let incompatible_coverage = service
            .source_shot_board(&SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
                candidate_offset: Some(0),
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: None,
                max_width: None,
            })
            .unwrap();
        assert_eq!(incompatible_coverage.is_error, Some(true));
        assert!(
            incompatible_coverage.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("candidate_offset is only supported")
        );

        core.request(Command::Do(Operation::AddAsset {
            asset: MediaAsset {
                id: AssetId(2),
                path: PathBuf::from("fixture.wav"),
                name: "fixture audio".to_owned(),
                duration: TimeCode(60),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Audio,
                resolution: None,
            },
        }))
        .unwrap();
        for args in [
            SourceShotBoardArgs {
                asset_id: AssetId(2),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: None,
                max_width: None,
            },
            SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: Some(TranscriptRangeArgs {
                    start: TimeCode(10),
                    end: TimeCode(10),
                }),
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: None,
                max_width: None,
            },
            SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: None,
                candidate_count: Some(SHOT_BOARD_MAX_CANDIDATES + 1),
                max_width: None,
            },
            SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: Some(TimeCode(0)),
                minimum_confidence_basis_points: None,
                candidate_count: None,
                max_width: None,
            },
            SourceShotBoardArgs {
                asset_id: AssetId(1),
                range: None,
                candidate_selection: None,
                candidate_offset: None,
                minimum_duration_frames: None,
                minimum_confidence_basis_points: Some(10_001),
                candidate_count: None,
                max_width: None,
            },
        ] {
            assert_eq!(
                service.source_shot_board(&args).unwrap().is_error,
                Some(true)
            );
        }

        let failed_analysis = Arc::new(NoopMedia {
            scene_statuses: BTreeMap::from([(
                AssetId(1),
                SceneStatus::Failed("decoder error".to_owned()),
            )]),
            ..NoopMedia::default()
        });
        let failed = KinewrightMcp::new(
            core,
            Arc::new(NoopMedia::default()),
            failed_analysis,
            ConfirmationBroker::default(),
        )
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        })
        .unwrap();
        assert_eq!(failed.is_error, Some(true));
    }

    #[test]
    fn source_shot_board_is_internal_registry_capability_not_compact_tool() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        let tool = registry
            .iter()
            .find(|tool| tool.name == "get_source_shot_board")
            .expect("source shot board is registered");
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("source shot board schema properties");
        assert!(properties.contains_key("candidate_selection"));
        assert!(properties.contains_key("minimum_duration_frames"));
        assert!(properties.contains_key("minimum_confidence_basis_points"));
        let served = KinewrightMcp::served_tools().unwrap();
        assert!(
            served
                .iter()
                .all(|tool| tool.name != "get_source_shot_board")
        );
    }

    #[test]
    fn edit_plan_applies_atomically_and_undoes_once() {
        let (core, playback, analysis) = fixture();
        let Event::QueryResult(QueryResult::Document(original)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document");
        };
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            analysis,
            ConfirmationBroker::default(),
        );
        let result = service
            .call_blocking(plan_request(json!([
                {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
                {"MoveClip": {"clip": 1, "to_track": 2, "to": 0}}
            ])))
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let text = &result.content[0].as_text().unwrap().text;
        assert!(text.contains("op 1 add_track: applied"));
        assert!(text.contains("op 2 move_clip: applied"));

        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("expected undo result");
        };
        assert_eq!(&*doc, &*original);
    }

    #[test]
    fn successful_bulk_plan_outcomes_are_counted_instead_of_repeated() {
        let operations = (1..=48)
            .map(|id| Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(id),
                    position: TimeCode(i64::try_from(id).unwrap()),
                    label: String::new(),
                    color_token: 0,
                },
            })
            .collect::<Vec<_>>();

        let rendered = render_plan_outcomes(&operations, None, None);
        assert_eq!(rendered, "applied 48 operations atomically (add_marker=48)");
    }

    #[test]
    fn capability_discovery_batches_queries_and_schema_opens() {
        let tools = KinewrightMcp::tools().unwrap();
        let found = search_capability_queries(
            &tools,
            &CapabilitySearchArgs {
                query: None,
                queries: vec!["dialogue assembly".to_owned(), "styled captions".to_owned()],
                kinds: Vec::new(),
                limit: None,
            },
        );
        assert!(
            found
                .iter()
                .any(|capability| capability.name == "plan_dialogue_assembly")
        );
        assert!(
            found
                .iter()
                .any(|capability| capability.name == "add_styled_captions")
        );

        let opened = open_capabilities(
            &tools,
            CapabilityArgs {
                name: Some("plan_dialogue_assembly".to_owned()),
                names: vec![
                    "add_styled_captions".to_owned(),
                    "plan_dialogue_assembly".to_owned(),
                ],
            },
        );
        assert_eq!(opened.is_error, Some(false));
        let structured = opened.structured_content.unwrap();
        assert_eq!(structured["capabilities"].as_array().unwrap().len(), 2);
        let serialized = serde_json::to_string(&structured).unwrap();
        assert!(serialized.contains("script"));
        assert!(serialized.contains("Punctuation becomes a hard cue-grouping"));
    }

    #[test]
    fn caption_position_avoids_the_subject_and_honors_explicit_direction() {
        assert_eq!(
            caption_position(None, Some(50)),
            Ok(TitlePosition::LowerThird)
        );
        assert_eq!(caption_position(None, Some(75)), Ok(TitlePosition::Top));
        assert_eq!(
            caption_position(Some(TitlePosition::Top), Some(20)),
            Ok(TitlePosition::Top)
        );
        assert!(caption_position(None, Some(101)).is_err());
    }

    #[test]
    fn authored_caption_path_reduces_the_dialogue_capability_surface() {
        let tools = KinewrightMcp::tools().unwrap();
        let open = |names: &[&str]| {
            open_capabilities(
                &tools,
                CapabilityArgs {
                    name: None,
                    names: names.iter().map(ToString::to_string).collect(),
                },
            )
        };
        let legacy = serde_json::to_vec(&open(&[
            "get_transcripts",
            "plan_dialogue_assembly",
            "add_styled_captions",
            "get_captions",
            "plan_caption_corrections",
            "get_dialogue_pacing",
            "get_editorial_readiness",
        ]))
        .unwrap()
        .len();
        let authored = serde_json::to_vec(&open(&[
            "get_transcripts",
            "plan_dialogue_assembly",
            "add_styled_captions",
            "get_dialogue_pacing",
            "get_editorial_readiness",
        ]))
        .unwrap()
        .len();

        println!("dialogue capability payload: legacy={legacy} B authored={authored} B");
        assert!(authored < legacy);
    }

    #[test]
    fn dialogue_pacing_adds_a_bounded_capability_payload() {
        let tools = KinewrightMcp::tools().unwrap();
        let shared = [
            "get_transcripts",
            "plan_dialogue_assembly",
            "add_styled_captions",
            "get_captions",
            "plan_caption_corrections",
            "get_editorial_readiness",
        ];
        let open = |names: &[&str]| {
            open_capabilities(
                &tools,
                CapabilityArgs {
                    name: None,
                    names: names.iter().map(ToString::to_string).collect(),
                },
            )
        };
        let v3_bytes = serde_json::to_vec(&open(&shared)).unwrap().len();
        let mut v4 = shared.to_vec();
        v4.push("get_dialogue_pacing");
        let v4_bytes = serde_json::to_vec(&open(&v4)).unwrap().len();

        println!("dialogue capability payload: v3={v3_bytes} B v4={v4_bytes} B");
        assert!(v4_bytes > v3_bytes);
        assert!(v4_bytes - v3_bytes < 2_500);
        assert!(v4_bytes < 20_000);
    }

    #[test]
    fn dialogue_keep_ranges_remove_qualified_silence_and_fillers() {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: "dialogue.mp4".into(),
            name: "dialogue".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((320, 180)),
        };
        let transcript = AssetTranscript {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                TranscriptWord {
                    text: "Keep".to_owned(),
                    source_start: TimeCode(4),
                    source_end: TimeCode(15),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Um,".to_owned(),
                    source_start: TimeCode(75),
                    source_end: TimeCode(82),
                    speaker: None,
                },
                TranscriptWord {
                    text: "going".to_owned(),
                    source_start: TimeCode(90),
                    source_end: TimeCode(105),
                    speaker: None,
                },
            ],
        };
        let silences = AssetSilences {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: asset.duration,
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![SilenceSpan {
                source_start: TimeCode(20),
                source_end: TimeCode(70),
            }],
        };

        assert_eq!(
            dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                TimeCode(20),
                true,
                DialoguePacingSettings {
                    retained_pause: TimeCode::ZERO,
                    filler_padding: TimeCode::ZERO,
                    maximum_filler_bridge_pause: None,
                },
                TimeCode::ZERO..asset.duration,
            ),
            vec![
                TimeCode(0)..TimeCode(20),
                TimeCode(70)..TimeCode(75),
                TimeCode(82)..TimeCode(120),
            ]
        );
    }

    #[test]
    fn dialogue_keep_ranges_retain_pause_and_pad_fillers() {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: "dialogue.mp4".into(),
            name: "dialogue".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((320, 180)),
        };
        let transcript = AssetTranscript {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![TranscriptWord {
                text: "Um".to_owned(),
                source_start: TimeCode(75),
                source_end: TimeCode(82),
                speaker: None,
            }],
        };
        let silences = AssetSilences {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: asset.duration,
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![SilenceSpan {
                source_start: TimeCode(20),
                source_end: TimeCode(70),
            }],
        };

        assert_eq!(
            dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                TimeCode(20),
                true,
                DialoguePacingSettings {
                    retained_pause: TimeCode(6),
                    filler_padding: TimeCode(3),
                    maximum_filler_bridge_pause: None,
                },
                TimeCode::ZERO..asset.duration,
            ),
            vec![TimeCode(0)..TimeCode(23), TimeCode(85)..TimeCode(120),]
        );
    }

    #[test]
    fn dialogue_keep_ranges_never_escape_the_requested_source_envelope() {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: "dialogue.mp4".into(),
            name: "dialogue".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((320, 180)),
        };
        let transcript = AssetTranscript {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![TranscriptWord {
                text: "Um".to_owned(),
                source_start: TimeCode(75),
                source_end: TimeCode(82),
                speaker: None,
            }],
        };
        let silences = AssetSilences {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: asset.duration,
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![SilenceSpan {
                source_start: TimeCode(20),
                source_end: TimeCode(70),
            }],
        };

        assert_eq!(
            dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                TimeCode(20),
                true,
                DialoguePacingSettings {
                    retained_pause: TimeCode::ZERO,
                    filler_padding: TimeCode::ZERO,
                    maximum_filler_bridge_pause: None,
                },
                TimeCode(10)..TimeCode(100),
            ),
            vec![
                TimeCode(10)..TimeCode(20),
                TimeCode(70)..TimeCode(75),
                TimeCode(82)..TimeCode(100),
            ]
        );
    }

    #[test]
    fn dialogue_filler_bridge_caps_long_pauses_and_preserves_shorter_ones() {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: "dialogue.mp4".into(),
            name: "dialogue".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((320, 180)),
        };
        let transcript = AssetTranscript {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                TranscriptWord {
                    text: "First.".to_owned(),
                    source_start: TimeCode(5),
                    source_end: TimeCode(15),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Um".to_owned(),
                    source_start: TimeCode(25),
                    source_end: TimeCode(30),
                    speaker: None,
                },
                TranscriptWord {
                    text: "uh,".to_owned(),
                    source_start: TimeCode(30),
                    source_end: TimeCode(35),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Then".to_owned(),
                    source_start: TimeCode(50),
                    source_end: TimeCode(60),
                    speaker: None,
                },
            ],
        };
        let silences = AssetSilences {
            asset: asset.id,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: asset.duration,
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(15),
                    source_end: TimeCode(25),
                },
                SilenceSpan {
                    source_start: TimeCode(35),
                    source_end: TimeCode(50),
                },
            ],
        };

        let bridges =
            dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(12)), TimeCode(20));
        assert_eq!(bridges.len(), 1);
        assert_eq!(bridges[0].cut_start, TimeCode(21));
        assert_eq!(bridges[0].cut_end, TimeCode(44));
        assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(12));
        assert_eq!(bridges[0].measurement, "acoustic_silence");
        let preserved =
            dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(30)), TimeCode(20));
        assert_eq!(preserved[0].available_pause_source_frames, TimeCode(25));
        assert_eq!(preserved[0].retained_pause_source_frames, TimeCode(25));
        assert_eq!(preserved[0].cut_start, TimeCode(25));
        assert_eq!(preserved[0].cut_end, TimeCode(35));
        assert_eq!(
            dialogue_keep_ranges(
                &asset,
                &transcript,
                &silences,
                TimeCode(5),
                true,
                DialoguePacingSettings {
                    retained_pause: TimeCode(6),
                    filler_padding: TimeCode(3),
                    maximum_filler_bridge_pause: Some(TimeCode(12)),
                },
                TimeCode::ZERO..asset.duration,
            ),
            vec![TimeCode(0)..TimeCode(19), TimeCode(46)..TimeCode(120)]
        );
    }

    #[test]
    fn dialogue_filler_bridge_uses_acoustic_edges_when_asr_endpoints_are_late() {
        let fps = Rational::new(30, 1).unwrap();
        let transcript = AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                TranscriptWord {
                    text: "rain".to_owned(),
                    source_start: TimeCode(128),
                    source_end: TimeCode(141),
                    speaker: None,
                },
                TranscriptWord {
                    text: "um".to_owned(),
                    source_start: TimeCode(162),
                    source_end: TimeCode(184),
                    speaker: None,
                },
                TranscriptWord {
                    text: "um".to_owned(),
                    source_start: TimeCode(197),
                    source_end: TimeCode(219),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Neighbors".to_owned(),
                    source_start: TimeCode(233),
                    source_end: TimeCode(245),
                    speaker: None,
                },
            ],
        };
        let silences = AssetSilences {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: TimeCode(331),
            threshold_dbfs_hundredths: -3_500,
            window_milliseconds: 10,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(108),
                    source_end: TimeCode(162),
                },
                SilenceSpan {
                    source_start: TimeCode(205),
                    source_end: TimeCode(234),
                },
            ],
        };

        let bridges =
            dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(12)), TimeCode(20));

        assert_eq!(bridges.len(), 1);
        assert_eq!(bridges[0].source_start, TimeCode(108));
        assert_eq!(bridges[0].source_end, TimeCode(234));
        assert_eq!(bridges[0].cut_start, TimeCode(114));
        assert_eq!(bridges[0].cut_end, TimeCode(228));
        assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(12));
        assert_eq!(bridges[0].measurement, "acoustic_silence");
    }

    #[test]
    fn dialogue_filler_bridge_never_leaves_one_cuttable_acoustic_flank() {
        let fps = Rational::new(30, 1).unwrap();
        let transcript = AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                TranscriptWord {
                    text: "built".to_owned(),
                    source_start: TimeCode(96),
                    source_end: TimeCode(104),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Um".to_owned(),
                    source_start: TimeCode(162),
                    source_end: TimeCode(193),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Um".to_owned(),
                    source_start: TimeCode(193),
                    source_end: TimeCode(229),
                    speaker: None,
                },
                TranscriptWord {
                    text: "Then".to_owned(),
                    source_start: TimeCode(233),
                    source_end: TimeCode(237),
                    speaker: None,
                },
            ],
        };
        let silences = AssetSilences {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: TimeCode(323),
            threshold_dbfs_hundredths: -3_500,
            window_milliseconds: 10,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(107),
                    source_end: TimeCode(162),
                },
                SilenceSpan {
                    source_start: TimeCode(205),
                    source_end: TimeCode(234),
                },
            ],
        };

        let bridges =
            dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(31)), TimeCode(20));

        assert_eq!(bridges.len(), 1);
        assert_eq!(
            bridges[0].maximum_contiguous_pause_source_frames,
            TimeCode(19)
        );
        assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(24));
        assert_eq!(bridges[0].cut_start, TimeCode(126));
        assert_eq!(bridges[0].cut_end, TimeCode(229));
    }

    #[test]
    fn dialogue_pacing_classifies_sentence_gaps_without_marking_word_gaps() {
        let word = |text: &str, asset: u64, start: i64, end: i64| TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(asset),
            track: TrackId(1),
            clip: ClipId(asset),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        };
        let words = vec![
            word("rain", 1, 80, 100),
            word("Neighbors", 1, 112, 130),
            word("instead", 1, 180, 200),
            word("Over", 2, 212, 230),
            word("beds", 2, 280, 300),
            word("Then", 2, 307, 325),
            word("peppers.", 2, 380, 400),
            word("Now", 3, 420, 438),
            word("continues", 3, 440, 458),
        ];

        let gaps = dialogue_pacing_gaps(&words, &[], TimeCode(9), TimeCode(15), TimeCode(4));
        assert_eq!(gaps.len(), 4);
        assert_eq!(gaps[0].status, "target");
        assert_eq!(gaps[0].reason, "pause_backed_capitalization");
        assert_eq!(gaps[1].status, "target");
        assert!(gaps[1].reason.contains("asset_change"));
        assert_eq!(gaps[2].status, "short");
        assert_eq!(gaps[2].pause_frames, TimeCode(7));
        assert_eq!(gaps[3].status, "long");
        assert!(gaps[3].reason.contains("terminal_punctuation"));
    }

    #[test]
    fn caption_hold_is_clamped_to_the_media_timeline() {
        let mut cues = vec![CaptionCue {
            start: TimeCode(90),
            end: TimeCode(115),
            text: "last line".to_owned(),
        }];
        clamp_caption_cues_to_duration(&mut cues, TimeCode(100));
        assert_eq!(cues[0].end, TimeCode(100));
    }

    #[test]
    fn mixed_validity_edit_plan_rejects_without_partial_state() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            analysis,
            ConfirmationBroker::default(),
        );
        let result = service
            .call_blocking(plan_request(json!([
                {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
                {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}}
            ])))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = &result.content[0].as_text().unwrap().text;
        assert!(text.contains("op 1 add_track: rolled back"));
        assert!(text.contains("op 2 add_track: rejected"));
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document");
        };
        assert!(document.tracks.iter().all(|track| track.id != TrackId(2)));
    }

    #[test]
    fn destructive_edit_plan_uses_one_summary_confirmation_for_approve_and_reject() {
        for approve in [true, false] {
            let (core, playback, analysis) = fixture();
            let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
            let result = invoke_in_background(
                KinewrightMcp::new(core.clone(), playback, analysis, broker.clone()),
                plan_request(json!([
                    {"RemoveTrack": {"track": 1}}
                ])),
            );
            let request = wait_for_request(&broker);
            assert_eq!(request.tool_name, "apply_edit_plan");
            assert_eq!(
                request.description,
                "Plan removes 1 clip and 1 track - approve?"
            );
            if approve {
                assert!(broker.approve(request.id));
            } else {
                assert!(broker.reject(request.id, "keep the plan unchanged"));
            }
            let result = result
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(result.is_error, Some(!approve));
            let Event::QueryResult(QueryResult::Document(document)) =
                core.request(Command::Query(Query::Document)).unwrap()
            else {
                panic!("expected document");
            };
            assert_eq!(document.tracks.is_empty(), approve);
        }
    }
}
