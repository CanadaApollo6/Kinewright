use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
    future::Future,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
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
    CaptionCue, CaptionMotion, CaptionPreset, Clip, ClipContent, ClipId, ColorNodeKind,
    ColorSourceError, Command, Core, DeliveryAspect, DeliveryEncodeDepth, DeliveryProfile,
    DeliveryVariant, Document, Effect, EffectId, Event, Export, ExportCancellation, Keyframe,
    KeyframeInterpolation, LutAsset, MUSIC_STRUCTURE_DEFAULT_METER_BEATS,
    MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS, Marker, MarkerId, MediaAsset, MediaAvailabilityKind,
    MediaCacheFamily, MediaCacheInventory, MediaKind, Operation, ParamValue, Playback, Query,
    QueryResult, ReframeFocusBounds, RelinkCandidate, SceneStatus, SilenceStatus,
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
    color_qc_tool::{COLOR_QC_DESCRIPTION, ColorQcArgs, get_color_qc},
    color_scopes::{
        AnalyzeColorShotArgs, PlanShotMatchArgs, ScopeError, VideoScopesV2Args, analyze_color_shot,
        plan_shot_match, video_scopes_v2,
    },
    color_status::{
        CC1_STAGE_NAMES, ColorContextArgs, ColorCurvesPlanArgs, ColorNodePlan, ColorNodePlanError,
        ColorProofError, ColorWheelsPlanArgs, LegacyLookConversion, LookAssetContext,
        LookComparison, LutNodePlanArgs, MatteComparison, PrimaryCorrectionPlanArgs,
        PrimaryPlanError, RenderColorProofArgs, SecondaryCorrectionPlanArgs,
        active_layer_source_classification, color_context_value_with_assumptions,
        color_context_value_with_options, color_curves_request_summary, color_node_manifest,
        color_wheels_parameter_summary, effect_chain_manifest, legacy_look_conversion,
        legacy_stage_warnings, look_assets_value, lut_node_parameter_summary,
        matte_legend_reference, plan_color_curves, plan_color_wheels, plan_creative_look,
        plan_primary_correction, plan_secondary_correction, plan_technical_lut,
        primary_parameter_summary, raw_only_conflict,
    },
    export_queue::{ExportJobId, ExportQueue, ExportQueueError, QueueExportRequest},
    pacing::{DialoguePacingGap, dialogue_pacing_gaps},
    render::{
        cuttable_timeline_silences, render_asset_scene_changes, render_asset_silences,
        render_asset_transcript, render_clip_info, render_timeline_scene_changes,
        render_timeline_silences, render_timeline_state, render_timeline_transcript,
    },
    runtime::{
        CapabilityDescriptor, CapabilityKind, PreparedEditPlan, PreparedPlanId, PreparedPlanStore,
        ToolSurfaceMetrics, capabilities, decode_plan_operations, is_invocable_capability,
        search_capabilities,
    },
    schema::{SchemaError, decode_operation, operation_tool_name, operation_tools, schema_object},
};

const THUMBNAIL_MAX_WIDTH: u32 = 512;
const MEDIA_PREVIEW_MAX_WIDTH: u32 = 1_280;
const STORYBOARD_DEFAULT_FRAMES: u8 = 9;
const STORYBOARD_MAX_FRAMES: u8 = 16;
const STORYBOARD_DEFAULT_CELL_WIDTH: u32 = 320;
const SHOT_BOARD_DEFAULT_CANDIDATES: u8 = 6;
const SHOT_BOARD_MAX_CANDIDATES: u8 = 12;
const SHOT_BOARD_EVIDENCE_PER_CANDIDATE: u8 = 3;
const DEFAULT_MAXIMUM_CUT_SECONDARY_CHANGE_BASIS_POINTS: u16 = 1_200;
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
/// The CC5 §2.2 matte-invert control, named once so the `outside_only` proof
/// variant and its test cannot drift apart.
const MATTE_INVERT_PARAMETER: &str = "matte_invert";
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
    /// The saved project file path this session owns, or `None` for a project
    /// that has never been saved (CC4 §2.2).
    ///
    /// The LUT store root is `<dir>/<stem>.kinewright-assets` and is
    /// **derived from this path at runtime and never stored**, so publishing
    /// the project path is what lets `import_lut_asset`, `list_look_assets`,
    /// the `color_nodes` manifests, and the export preflight resolve a look's
    /// bytes. Shared with the export queue so both see the same project.
    project_path: Arc<RwLock<Option<PathBuf>>>,
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
            Arc::new(RwLock::new(None)),
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
        Self::start_isolated_with_project_path(
            core,
            playback,
            analysis,
            Arc::new(RwLock::new(None)),
        )
    }

    /// Start a branch-scoped MCP server that shares the project session's
    /// saved-project-path handle (CC4 §2.2, §8).
    ///
    /// A branch server derives its own LUT store root from this handle, so a
    /// branch created with a fresh `None` handle is store-blind: every CC4
    /// availability surface reports `unknown_no_store` for imported assets and
    /// `import_lut_asset` reports `project_not_saved`, even on a saved
    /// project. Callers that own a project session should pass
    /// [`McpServer::project_path_handle`] from the live server (or the same
    /// `Arc` the session publishes into with
    /// [`McpServer::set_project_path`]), so a later Save As reaches every
    /// branch without republishing.
    ///
    /// In the application this is `chat_ui.rs`'s `AgentThread::new` and
    /// `AgentThread::replace_branch`, which start every branch server with the
    /// session's own `agent_project_path` handle.
    /// `ProjectSession::publish_project_path_to_agents` then writes that one
    /// shared handle, so a Save As reaches every branch at once and no branch
    /// is ever store-blind, not even for the frame it was created in.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener or server thread cannot start.
    pub fn start_isolated_with_project_path(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            None,
            ConfirmationBroker::default(),
            false,
            project_path,
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
        Self::start_isolated_with_exporter_and_project_path(
            core,
            playback,
            analysis,
            exporter,
            Arc::new(RwLock::new(None)),
        )
    }

    /// Start a branch-scoped MCP server with branch-snapshot exports that
    /// shares the project session's saved-project-path handle.
    ///
    /// See [`McpServer::start_isolated_with_project_path`] for why a branch
    /// server needs the handle at all.
    ///
    /// # Errors
    ///
    /// Returns an MCP server error when the listener, export worker, or server thread cannot start.
    pub fn start_isolated_with_exporter_and_project_path(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Arc<dyn Export>,
        project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            Some(exporter),
            ConfirmationBroker::default(),
            false,
            project_path,
        )
    }

    fn start_with_broker(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Result<Self, McpServerError> {
        Self::start_configured(
            core,
            playback,
            analysis,
            None,
            confirmations,
            true,
            Arc::new(RwLock::new(None)),
        )
    }

    fn start_configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Option<Arc<dyn Export>>,
        confirmations: ConfirmationBroker,
        publish_to_playback: bool,
        project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Result<Self, McpServerError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(McpServerError::Bind)?;
        let address = listener.local_addr().map_err(McpServerError::Listener)?;
        listener
            .set_nonblocking(true)
            .map_err(McpServerError::Listener)?;
        let endpoint = format!("http://{address}/mcp");
        let (shutdown, shutdown_rx) = oneshot::channel();
        let export_queue = exporter
            .map(|exporter| {
                ExportQueue::with_lut_project_path(
                    exporter,
                    Arc::clone(&analysis),
                    Arc::clone(&project_path),
                )
            })
            .transpose()?;
        let tool_surface_metrics = ToolSurfaceMetrics::measure(&KinewrightMcp::served_tools()?);
        let handler = KinewrightMcp::configured(
            core,
            playback,
            analysis,
            export_queue,
            confirmations.clone(),
            publish_to_playback,
            Arc::clone(&project_path),
        );
        let server_thread = thread::Builder::new()
            .name("kinewright-mcp".to_owned())
            .spawn(move || run_server(listener, handler, shutdown_rx))
            .map_err(McpServerError::Thread)?;
        Ok(Self {
            endpoint,
            confirmations,
            project_path,
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

    /// The shared saved-project-path handle backing the CC4 LUT store.
    ///
    /// The owner of the project session publishes the saved project file path
    /// here; the store root is derived from it as
    /// `<dir>/<stem>.kinewright-assets` on every use and never persisted
    /// (CC4 §2.2). Until a path is published, `import_lut_asset` reports
    /// `project_not_saved` and every availability surface reports
    /// `unknown_no_store` rather than inventing a status.
    #[must_use]
    pub fn project_path_handle(&self) -> Arc<RwLock<Option<PathBuf>>> {
        Arc::clone(&self.project_path)
    }

    /// Publish (or clear) the saved project file path for this session.
    pub fn set_project_path(&self, path: Option<PathBuf>) {
        if let Ok(mut slot) = self.project_path.write() {
            *slot = path;
        }
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
    /// See [`McpServer::project_path_handle`].
    project_path: Arc<RwLock<Option<PathBuf>>>,
}

impl KinewrightMcp {
    #[cfg(test)]
    fn new(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Self {
        Self::configured(
            core,
            playback,
            analysis,
            None,
            confirmations,
            true,
            Arc::new(RwLock::new(None)),
        )
    }

    fn configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        export_queue: Option<ExportQueue>,
        confirmations: ConfirmationBroker,
        publish_to_playback: bool,
        project_path: Arc<RwLock<Option<PathBuf>>>,
    ) -> Self {
        Self {
            core,
            playback,
            analysis,
            export_queue,
            confirmations,
            publish_to_playback,
            prepared_plans: Arc::new(Mutex::new(PreparedPlanStore::default())),
            project_path,
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
                let plan: Result<PreparedEditPlan, String> = if args.expected_revision
                    == actual_revision
                {
                    match decode_plan_operations(args.operations) {
                        Ok(operations) => {
                            self.prepare_operations(args.expected_revision, &document, operations)
                        }
                        Err(error) => Err(error.to_string()),
                    }
                } else {
                    Err(format!(
                        "timeline revision conflict: expected {}, actual {}",
                        args.expected_revision, actual_revision
                    ))
                };
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
                    Err(error) => error_text(error),
                })
            }
            "commit_edit_plan" => {
                let args: CommitEditPlanArgs = decode_args("commit_edit_plan", arguments)?;
                let plan = {
                    let plans = self.prepared_plans.lock().map_err(|_| {
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
                    plan
                };
                let (commit_revision, commit_document) = self.snapshot()?;
                if args.expected_revision != commit_revision {
                    return Ok(revision_conflict_text(
                        args.expected_revision,
                        commit_revision,
                    ));
                }
                if let Err(error) = self
                    .ensure_verified_source_assets(&commit_document, &plan.referenced_source_assets)
                {
                    return Ok(error_text(error));
                }
                let result = self.apply_edit_plan(args.expected_revision, &plan.operations)?;
                if result.is_error != Some(true) {
                    self.prepared_plans
                        .lock()
                        .map_err(|_| McpError::internal_error("prepared plan store stopped", None))?
                        .take(args.plan_id);
                }
                Ok(result)
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
            "get_color_context" => {
                let args: ColorContextArgs = decode_args("get_color_context", arguments)?;
                self.color_context(&args)
            }
            "plan_primary_correction" => {
                let args: PrimaryCorrectionPlanArgs =
                    decode_args("plan_primary_correction", arguments)?;
                self.primary_correction_plan(&args)
            }
            "plan_color_wheels" => {
                let args: ColorWheelsPlanArgs = decode_args("plan_color_wheels", arguments)?;
                self.color_node_plan("plan_color_wheels", |document, revision| {
                    plan_color_wheels(document, revision, &args)
                })
            }
            "plan_color_curves" => {
                let args: ColorCurvesPlanArgs = decode_args("plan_color_curves", arguments)?;
                self.color_node_plan("plan_color_curves", |document, revision| {
                    plan_color_curves(document, revision, &args)
                })
            }
            "plan_technical_lut" => {
                let args: LutNodePlanArgs = decode_args("plan_technical_lut", arguments)?;
                let (_, document) = self.snapshot()?;
                let looks = self.look_context(&document);
                self.color_node_plan("plan_technical_lut", |document, revision| {
                    plan_technical_lut(document, revision, &args, &looks)
                })
            }
            "plan_creative_look" => {
                let args: LutNodePlanArgs = decode_args("plan_creative_look", arguments)?;
                let (_, document) = self.snapshot()?;
                let looks = self.look_context(&document);
                self.color_node_plan("plan_creative_look", |document, revision| {
                    plan_creative_look(document, revision, &args, &looks)
                })
            }
            "plan_secondary_correction" => {
                let args: SecondaryCorrectionPlanArgs =
                    decode_args("plan_secondary_correction", arguments)?;
                self.color_node_plan("plan_secondary_correction", |document, revision| {
                    plan_secondary_correction(document, revision, self.analysis.as_ref(), &args)
                })
            }
            "inspect_grade_matte" => {
                let args: InspectGradeMatteArgs = decode_args("inspect_grade_matte", arguments)?;
                self.inspect_grade_matte(&args)
            }
            "track_matte_window" => {
                let args: TrackMatteWindowArgs = decode_args("track_matte_window", arguments)?;
                self.track_matte_window(&args)
            }
            "list_look_assets" => self.list_look_assets(),
            "import_lut_asset" => {
                let args: ImportLutAssetArgs = decode_args("import_lut_asset", arguments)?;
                self.import_lut_asset(&args)
            }
            "convert_legacy_look" => {
                let args: ConvertLegacyLookArgs = decode_args("convert_legacy_look", arguments)?;
                self.convert_legacy_look(&args)
            }
            "render_color_proof" => {
                let args: RenderColorProofArgs = decode_args("render_color_proof", arguments)?;
                self.render_color_proof(&args)
            }
            "get_media_status" => self.media_status(),
            "get_cache_status" => self.cache_status(),
            "clear_media_cache" => {
                let args: ClearMediaCacheArgs = decode_args("clear_media_cache", arguments)?;
                self.clear_media_cache(args.family)
            }
            "get_clip_info" => {
                let args: ClipInfoArgs = decode_args("get_clip_info", arguments)?;
                let document = self.document()?;
                Ok(match render_clip_info(&document, args.clip_id) {
                    Ok(rendered) => success_text(rendered),
                    Err(error) => error_text(error),
                })
            }
            "plan_source_program_edit" => {
                let args: SourceProgramEditArgs =
                    decode_args("plan_source_program_edit", arguments)?;
                self.source_program_edit_plan(&args)
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
            "get_cut_neighborhoods" => {
                let args: CutNeighborhoodsArgs = decode_args("get_cut_neighborhoods", arguments)?;
                self.cut_neighborhoods(&args)
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
            "get_video_scopes_v2" => {
                let args: VideoScopesV2Args = decode_args("get_video_scopes_v2", arguments)?;
                self.video_scopes_v2(&args)
            }
            "analyze_color_shot" => {
                let args: AnalyzeColorShotArgs = decode_args("analyze_color_shot", arguments)?;
                self.analyze_color_shot(&args)
            }
            "get_color_qc" => {
                let args: ColorQcArgs = decode_args("get_color_qc", arguments)?;
                self.color_qc(&args)
            }
            "plan_shot_match" => {
                let args: PlanShotMatchArgs = decode_args("plan_shot_match", arguments)?;
                self.plan_shot_match(&args)
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
            "relink_media" => {
                let args: RelinkMediaArgs = decode_args("relink_media", arguments)?;
                self.relink_media(&args)
            }
            "relink_asset" => Ok(error_text(
                "relink_asset is not exposed as a generated operation; use relink_media so the replacement is probed and hashed first",
            )),
            // CC4 §8: only `import_lut_asset` can create a `LutAsset`, because
            // only it can write the hashed bytes into the project store.
            "add_lut_asset" => Ok(error_text(
                "add_lut_asset is not exposed as a generated operation; use import_lut_asset so the .cube bytes are parsed, hashed, and stored first",
            )),
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

    fn media_status(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let assets = document
            .media_pool
            .iter()
            .map(|asset| {
                serde_json::json!({
                    "asset_id": asset.id.0,
                    "path": asset.path,
                    "persisted_fingerprint": asset.source_fingerprint,
                    "availability": self.analysis.media_availability(asset),
                    "analysis_jobs": self.analysis.analysis_jobs(asset),
                })
            })
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "timeline_revision": revision.0,
            "preview": {
                "mode": "in_memory",
                "max_width": MEDIA_PREVIEW_MAX_WIDTH,
                "persistent": false,
                "generated_proxy_supported": false,
            },
            "assets": assets,
        });
        Ok(success_structured(
            format!(
                "media status at timeline revision {}: {} asset(s), preview mode=in_memory max_width={} persistent=false generated_proxy_supported=false",
                revision,
                value["assets"].as_array().map_or(0, Vec::len),
                MEDIA_PREVIEW_MAX_WIDTH,
            ),
            value,
        ))
    }

    fn cache_status(&self) -> Result<CallToolResult, McpError> {
        let inventory: MediaCacheInventory = self.analysis.cache_inventory();
        let value = serde_json::to_value(&inventory)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(success_structured(
            format!("media cache status: {} family(s)", inventory.families.len()),
            value,
        ))
    }

    fn clear_media_cache(&self, family: MediaCacheFamily) -> Result<CallToolResult, McpError> {
        match self.analysis.clear_cache(family) {
            Ok(result) if !result.supported => {
                let generated_proxy = family == MediaCacheFamily::GeneratedProxy;
                let code = if generated_proxy {
                    "unsupported_generated_proxy"
                } else {
                    "unsupported_cache_family"
                };
                Ok(error_structured(
                    format!("cannot clear {family:?} media cache: {code}"),
                    serde_json::json!({
                        "family": family,
                        "supported": false,
                        "code": code,
                        "message": result.note,
                    }),
                ))
            }
            Ok(result) => {
                let value = serde_json::to_value(&result)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                Ok(success_structured(
                    format!(
                        "cleared {:?} media cache: {} file(s), {} byte(s)",
                        family, result.removed_file_count, result.removed_bytes
                    ),
                    value,
                ))
            }
            Err(error) if error == kinewright_core::MediaError::NotImplemented => {
                let generated_proxy = family == MediaCacheFamily::GeneratedProxy;
                let code = if generated_proxy {
                    "unsupported_generated_proxy"
                } else {
                    "unsupported_cache_family"
                };
                Ok(error_structured(
                    format!("cannot clear {family:?} media cache: {code}"),
                    serde_json::json!({
                        "family": family,
                        "supported": false,
                        "code": code,
                        "message": error.to_string(),
                    }),
                ))
            }
            Err(error) => Ok(error_structured(
                format!("could not clear {family:?} media cache: {error}"),
                serde_json::json!({
                    "family": family,
                    "supported": true,
                    "code": "cache_clear_failed",
                    "message": error.to_string(),
                }),
            )),
        }
    }

    fn relink_media(&self, args: &RelinkMediaArgs) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            return Ok(revision_conflict_text(
                args.expected_revision,
                actual_revision,
            ));
        }
        let Some(current) = document.asset(args.asset_id) else {
            return Ok(error_text(format!(
                "asset {} does not exist",
                args.asset_id
            )));
        };

        // Probe and hash the replacement before constructing the Core
        // operation. Core remains filesystem-free and receives only this
        // typed candidate; all mismatches therefore remain atomic Core
        // rejections after this read-only preflight.
        let probed = match self.analysis.probe(&args.path) {
            Ok(asset) => asset,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let candidate = RelinkCandidate {
            path: args.path.clone(),
            fingerprint: probed.source_fingerprint,
            kind: probed.kind,
            fps: probed.fps,
            duration: probed.duration,
            resolution: probed.resolution,
        };
        let operation = Operation::RelinkAsset {
            asset: args.asset_id,
            candidate,
            allow_unverified_source: args.allow_unverified_source,
        };
        let result = self.apply_operation("relink_media", args.expected_revision, operation);
        if result.is_error != Some(true) {
            // Refresh content-addressed analysis for the replacement path.
            // The operation itself remains the one Core history entry.
            if let Ok((_, updated)) = self.snapshot()
                && let Some(updated_asset) = updated.asset(current.id)
            {
                self.request_asset_analysis(updated_asset.clone());
            }
        }
        Ok(result)
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
        if let Err(error) = self.ensure_verified_patched_sources(&before, operations) {
            return Ok(error_text(error));
        }
        if operations
            .iter()
            .any(|operation| matches!(operation, Operation::RelinkAsset { .. }))
        {
            return Ok(error_text(
                "RelinkAsset cannot be submitted through apply_edit_plan; use relink_media so the replacement is probed and hashed first",
            ));
        }
        // CC4 §8: the plan path has no way to write the project LUT store, so
        // a plan-supplied record could reference bytes that do not exist.
        if operations
            .iter()
            .any(|operation| matches!(operation, Operation::AddLutAsset { .. }))
        {
            return Ok(error_text(
                "AddLutAsset cannot be submitted through apply_edit_plan; use import_lut_asset so the .cube bytes are parsed, hashed, and stored first",
            ));
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

    fn color_context(&self, args: &ColorContextArgs) -> Result<CallToolResult, McpError> {
        if let Some(conflict) = raw_only_conflict(args) {
            return Ok(error_structured(
                conflict["message"].as_str().unwrap_or_default().to_owned(),
                conflict,
            ));
        }
        let (revision, document) = self.snapshot()?;
        let looks = self.look_context(&document);
        let value = if args.raw_only {
            // `raw_only_conflict` above already refused `raw_only` combined with
            // an explicit assumption, so there is no assumption left to forward.
            color_context_value_with_options(
                revision,
                &document,
                None,
                &args.asset_ids,
                true,
                &looks,
            )
        } else {
            color_context_value_with_assumptions(
                revision,
                &document,
                args.profile_assumption,
                &args.asset_ids,
                &looks,
            )
        };
        Ok(success_structured(
            format!(
                "timeline_revision={} assets={} working={} monitoring={} delivery={}\n{}",
                revision,
                value["assets"].as_array().map_or(0, Vec::len),
                value["color_context"]["working"],
                value["color_context"]["monitoring"],
                value["color_context"]["delivery"],
                value,
            ),
            value,
        ))
    }

    fn primary_correction_plan(
        &self,
        args: &PrimaryCorrectionPlanArgs,
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        let plan = match plan_primary_correction(&document, actual_revision, args) {
            Ok(plan) => plan,
            Err(PrimaryPlanError::RevisionConflict { expected, actual }) => {
                return Ok(revision_conflict_text(expected, actual));
            }
            Err(error) => {
                return Ok(error_structured(
                    format!("primary correction plan rejected: {error}"),
                    serde_json::json!({
                        "code": error.code(),
                        "message": error.to_string(),
                        "details": error.details(),
                        "evidence_only": true,
                        "applied": false,
                    }),
                ));
            }
        };
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let value = serde_json::json!({
            "timeline_revision": plan.expected_revision.0,
            "clip_id": plan.clip_id.0,
            "effect_id": plan.effect_id.0,
            // Null when the proposal changes nothing and would have had to
            // create the node: no operation allocates that id, so publishing it
            // would name a node that does not exist and may later be reused.
            "target_effect_id": plan.target_effect_id().map(|effect| effect.0),
            "created_new_node": plan.created_new_node,
            "existing_primary_node_count": plan.existing_primary_node_count,
            "no_change": plan.no_change,
            "warnings": plan.warnings,
            "source_profile": plan.source_profile.id(),
            "profile_assumption": plan.profile_assumption,
            "evidence_only": true,
            "applied": false,
            "before": {
                "primary_node_count": plan.existing_primary_node_count,
            },
            "after": {
                "primary_node_count": plan.existing_primary_node_count
                    + usize::from(plan.created_new_node),
            },
            "requested_parameters": plan.requested_parameters,
            "resolved_parameters": plan.resolved_parameters,
            "operations": operations,
            "next": "Review these exact operations; submit them through prepare_edit_plan at the same revision if the edit is requested.",
        });
        Ok(success_structured(
            format!(
                "prepared evidence-only primary correction for clip {} at revision {}; no operation was applied",
                plan.clip_id, plan.expected_revision
            ),
            value,
        ))
    }

    /// The saved project file path published to this session, if any.
    fn project_path(&self) -> Option<PathBuf> {
        self.project_path.read().ok().and_then(|slot| slot.clone())
    }

    /// Derive the CC4 LUT store from the published project path.
    ///
    /// `None` means the project has never been saved, which is a distinct
    /// state from a store that exists but cannot be used: the outer `Option`
    /// answers "is there a project path", the inner `Result` answers "is its
    /// derived root usable" (CC4 §2.2).
    fn lut_store(&self) -> Option<Result<kinewright_media::LutStore, kinewright_core::MediaError>> {
        self.project_path()
            .map(|path| kinewright_media::LutStore::for_project(&path))
    }

    /// Snapshot the document's LUT assets together with their live
    /// availability, resolved through the store when one is known.
    fn look_context(&self, document: &Document) -> LookAssetContext {
        match self.lut_store() {
            Some(Ok(store)) => {
                let resolver = store.availability_resolver();
                LookAssetContext::new(
                    document,
                    Some(store.root().to_path_buf()),
                    Some(&resolver as &dyn Fn(&_) -> _),
                )
            }
            // No store root, or a root this process refuses to read: every
            // availability surface reports `unknown_no_store` rather than
            // inventing a status.
            Some(Err(_)) | None => LookAssetContext::document_only(document),
        }
    }

    /// CC4 §8 `list_look_assets`: the built-in catalogue plus every project
    /// asset with its identity, provenance, availability, and references.
    fn list_look_assets(&self) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        let looks = self.look_context(&document);
        let value = look_assets_value(revision, &document, &looks);
        Ok(success_structured(
            format!(
                "timeline_revision={} builtin_looks={} project_lut_assets={} store_root={}",
                revision,
                kinewright_media::BuiltinLook::ALL.len(),
                document.lut_assets.len(),
                looks.store_root().map_or_else(
                    || "none (project not saved)".to_owned(),
                    |root| root.display().to_string()
                ),
            ),
            value,
        ))
    }

    /// CC4 §8 `import_lut_asset`: the only path that can create a `LutAsset`.
    ///
    /// The confirmation is requested **before the first byte is read**, so a
    /// refused import leaves no store file and no document change (CC4 §13).
    #[allow(clippy::too_many_lines)]
    fn import_lut_asset(&self, args: &ImportLutAssetArgs) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            // CC4 §8: every rejection this tool can return is structured, so a
            // conflict is a machine-readable `revision_conflict`, not prose the
            // caller has to pattern-match on.
            return Ok(lut_revision_conflict(
                "import_lut_asset",
                args.expected_revision,
                actual_revision,
            ));
        }
        let Some(store) = self.lut_store() else {
            return Ok(lut_import_error(
                "project_not_saved",
                "the project has never been saved, so it has no LUT store root",
                &serde_json::json!({
                    "field": "project_path",
                    "observed": serde_json::Value::Null,
                    "allowed": "a saved project file path such as <dir>/<stem>.kinewright",
                    "recovery_action": "Save the project first; the store root is <dir>/<stem>.kinewright-assets and is derived from the project path at runtime.",
                }),
            ));
        };
        let store = match store {
            Ok(store) => store,
            Err(error) => {
                return Ok(lut_store_error_result("import_lut_asset", &error));
            }
        };
        // Ask before touching the filesystem. `symlink_metadata` on the source
        // is cheap and is the honest size to quote; a source we cannot even
        // stat is refused before a confirmation is spent on it.
        let observed_bytes = std::fs::symlink_metadata(&args.path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len());
        let description = format!(
            "The agent wants to import the LUT file {} ({}) into this project's LUT store at {}. The bytes are copied under the project directory and registered as an undoable AddLutAsset operation.",
            args.path.display(),
            observed_bytes.map_or_else(
                || "size unknown".to_owned(),
                |bytes| format!("{bytes} byte(s)")
            ),
            store.luts_dir().display(),
        );
        if let Err(reason) = self.confirmations.confirm("import_lut_asset", description) {
            return Ok(lut_import_error(
                "import_refused",
                &format!("refused destructive tool import_lut_asset: {reason}"),
                &serde_json::json!({
                    "field": "confirmation",
                    "observed": reason,
                    "allowed": "an approved confirmation",
                    "recovery_action": "Ask the operator to approve the import, then resend at the current timeline_revision.",
                    "reason": reason,
                    "store_file_written": false,
                    "document_changed": false,
                }),
            ));
        }
        let import = match store.import_lut_asset(&args.path) {
            Ok(import) => import,
            Err(error) => return Ok(lut_store_error_result("import_lut_asset", &error)),
        };
        // CC4 §2.1/§2.3: assets are content-addressed, so a second import of
        // the same bytes is the *same* asset. Allocating a second record would
        // give one look two ids, make `referenced_by` lie, and leave
        // `RemoveLutAsset` unable to clean either one up. The store write above
        // is idempotent by the same hash, so re-importing still repairs a
        // missing store file before this returns.
        if let Some(existing) = document
            .lut_assets
            .iter()
            .find(|asset| asset.sha256 == import.sha256)
        {
            let looks = self.look_context(&document);
            return Ok(success_structured(
                format!(
                    "LUT asset {} \"{}\" already records sha256={}; reused the existing record instead of registering a second one",
                    existing.id, existing.title, existing.sha256
                ),
                serde_json::json!({
                    "timeline_revision": actual_revision.0,
                    "lut_asset": looks.asset_summary(existing),
                    "reused_existing_asset": true,
                    "applied": false,
                    "next": "Bind the asset with plan_technical_lut or plan_creative_look, then submit the returned operations through prepare_edit_plan.",
                }),
            ));
        }
        let lut_asset_id = match document.next_lut_asset_id() {
            Ok(id) => id,
            Err(error) => {
                return Ok(lut_import_error(
                    "lut_asset_id_exhausted",
                    &error.to_string(),
                    &serde_json::json!({
                        "field": "lut_asset_id",
                        "observed": "exhausted",
                        "allowed": format!("1..={}", kinewright_core::LUT_ASSET_ID_MAX),
                        "recovery_action": "Remove unused LUT asset records before importing another look.",
                    }),
                ));
            }
        };
        let store_path = store.path_for(&import.sha256).ok();
        let mut asset = import.into_lut_asset(lut_asset_id);
        if let Some(title) = args.title.as_ref().map(|title| title.trim())
            && !title.is_empty()
        {
            title.clone_into(&mut asset.title);
        }
        let summary = serde_json::json!({
            "lut_asset_id": asset.id.0,
            "title": asset.title,
            "sha256": asset.sha256,
            "kind": asset.kind.as_str(),
            "size": asset.size,
            "byte_len": asset.byte_len,
            "domain_min_millionths": asset.domain_min_millionths,
            "domain_max_millionths": asset.domain_max_millionths,
            "store_path": store_path,
            "store_root": store.root(),
        });
        let result = self.apply_operation(
            "import_lut_asset",
            args.expected_revision,
            Operation::AddLutAsset { asset },
        );
        if result.is_error == Some(true) {
            return Ok(result);
        }
        Ok(success_structured(
            format!(
                "imported LUT asset {} \"{}\" sha256={} into {}",
                summary["lut_asset_id"],
                summary["title"].as_str().unwrap_or_default(),
                summary["sha256"].as_str().unwrap_or_default(),
                store.luts_dir().display(),
            ),
            serde_json::json!({
                "timeline_revision": args.expected_revision.0,
                "lut_asset": summary,
                "reused_existing_asset": false,
                "applied": true,
                "next": "Bind the asset with plan_technical_lut or plan_creative_look, then submit the returned operations through prepare_edit_plan.",
            }),
        ))
    }

    /// Apply one batch under a revision gate, for the CC4 tools whose batch
    /// contains an `AddLutAsset` the plan path refuses by design.
    ///
    /// This is `apply_edit_plan` without the `AddLutAsset` guard: the guard
    /// exists because a *plan-supplied* record could name bytes that do not
    /// exist, and the record here was built by the store from bytes it just
    /// hashed, or from this binary's own bake.
    fn apply_lut_batch(
        &self,
        tool: &str,
        expected_revision: TimelineRevision,
        operations: &[Operation],
    ) -> Result<Result<TimelineRevision, CallToolResult>, McpError> {
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
                    self.playback.set_document(doc);
                }
                Ok(revision)
            }
            Event::BatchRejected { error, .. } => Err(lut_tool_error(
                tool,
                "core_rejected",
                &error.to_string(),
                &serde_json::json!({
                    "field": "operations",
                    "observed": error.to_string(),
                    "allowed": "a batch Core validates against the current document",
                    "recovery_action": "Call get_color_context for the current node stack and asset table, then resend at the current timeline_revision.",
                }),
            )),
            Event::RevisionConflict { expected, actual } => {
                Err(lut_revision_conflict(tool, expected, actual))
            }
            _ => Err(lut_tool_error(
                tool,
                "core_rejected",
                "Core returned the wrong batch result",
                &serde_json::json!({
                    "field": "operations",
                    "observed": "an unexpected Core event",
                    "allowed": "a document change, a rejection, or a revision conflict",
                    "recovery_action": "Call get_timeline_state and retry.",
                }),
            )),
        })
    }

    /// CC4 §9 `convert_legacy_look`: the only agent path from a legacy
    /// compatibility stage to a managed `creative_look`.
    ///
    /// `get_color_context.legacy_look_conversions` publishes the exact batch
    /// each legacy node needs, but for a `look_lut` whose built-in is not
    /// registered yet that batch opens with `AddLutAsset`, which is refused on
    /// every plan path by design (CC4 §8) — so the evidence was unsubmittable.
    /// This tool performs the batch server-side under the same revision gate:
    ///
    /// - `look_lut`: resolve `preset_token` to a built-in, reuse an already
    ///   registered record with the same content hash or register the bake,
    ///   then convert. No filesystem access at all.
    /// - `cube_lut`: import the node's external `path` into the project store
    ///   through the same confirmation path as `import_lut_asset` — the
    ///   operator is asked **before the first byte is read** — then convert.
    ///
    /// The conversion is deliberately not bit-identical to the legacy stage
    /// (CC4 §9.3), which is why it is an explicit, confirmed, undoable action
    /// and never happens on load.
    #[allow(clippy::too_many_lines)]
    fn convert_legacy_look(
        &self,
        args: &ConvertLegacyLookArgs,
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        if args.expected_revision != actual_revision {
            return Ok(lut_revision_conflict(
                "convert_legacy_look",
                args.expected_revision,
                actual_revision,
            ));
        }
        let conversion = match legacy_look_conversion(&document, args.clip_id, args.effect_id) {
            Ok(conversion) => conversion,
            Err(error) => {
                return Ok(lut_tool_error(
                    "convert_legacy_look",
                    error.code(),
                    &error.to_string(),
                    &serde_json::json!({
                        "field": error.field(),
                        "observed": error.observed(),
                        "allowed": error.allowed(),
                        "recovery_action": error.recovery_action(),
                        "clip_id": args.clip_id.0,
                        "effect_id": args.effect_id.0,
                    }),
                ));
            }
        };
        let (operations, summary) = match conversion {
            LegacyLookConversion::Builtin {
                operations,
                builtin_name,
                lut_asset,
                mix_basis_points,
                reused_existing_asset,
            } => {
                let summary = serde_json::json!({
                    "source": "builtin",
                    "builtin_name": builtin_name,
                    "lut_asset_id": lut_asset.0,
                    "mix_basis_points": mix_basis_points,
                    "reused_existing_asset": reused_existing_asset,
                    "store_file_written": false,
                });
                (operations, summary)
            }
            LegacyLookConversion::NeedsImport {
                path,
                mix_basis_points,
            } => match self.import_legacy_look_path(&document, &path) {
                Err(refusal) => return Ok(refusal),
                Ok((asset, register, reused_existing_asset, store_root)) => {
                    let lut_asset = asset.id;
                    let mut operations = Vec::new();
                    if let Some(asset) = register {
                        operations.push(Operation::AddLutAsset { asset });
                    }
                    operations.push(Operation::ConvertLegacyLook {
                        clip: args.clip_id,
                        effect: args.effect_id,
                        lut_asset,
                        mix_basis_points,
                    });
                    let summary = serde_json::json!({
                        "source": "imported",
                        "source_path": path,
                        "lut_asset_id": lut_asset.0,
                        "title": asset.title,
                        "sha256": asset.sha256,
                        "kind": asset.kind.as_str(),
                        "size": asset.size,
                        "byte_len": asset.byte_len,
                        "mix_basis_points": mix_basis_points,
                        "reused_existing_asset": reused_existing_asset,
                        "store_file_written": true,
                        "store_root": store_root,
                    });
                    (operations, summary)
                }
            },
        };
        let applied_operations = serde_json::to_value(&operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let revision = match self.apply_lut_batch(
            "convert_legacy_look",
            args.expected_revision,
            &operations,
        )? {
            Ok(revision) => revision,
            Err(rejection) => return Ok(rejection),
        };
        let (_, converted) = self.snapshot()?;
        let looks = self.look_context(&converted);
        let lut_asset_id = summary["lut_asset_id"]
            .as_u64()
            .map(kinewright_core::LutAssetId);
        Ok(success_structured(
            format!(
                "converted legacy {} on clip {} effect {} into a managed creative_look at revision {revision}",
                summary["source"].as_str().unwrap_or_default(),
                args.clip_id,
                args.effect_id,
            ),
            serde_json::json!({
                "timeline_revision": revision.0,
                "clip_id": args.clip_id.0,
                "effect_id": args.effect_id.0,
                "conversion": summary,
                "lut_asset": lut_asset_id
                    .and_then(|id| looks.asset(id).map(|asset| looks.asset_summary(asset))),
                "operations": applied_operations,
                "applied": true,
                "bit_identical_to_legacy": false,
                "next": "Render render_color_proof with this clip's new creative_look effect_id to see the deliberate difference from the legacy stage (CC4 §9.3); undo restores the legacy node.",
            }),
        ))
    }

    /// Import one legacy `cube_lut`'s external path into the project store,
    /// behind the same confirmation `import_lut_asset` uses.
    ///
    /// Returns the record to reference, the record to register (`None` when an
    /// existing asset already carries these bytes), whether it was reused, and
    /// the store root. The outer `Err` is a ready-to-return refusal.
    #[allow(clippy::type_complexity)]
    fn import_legacy_look_path(
        &self,
        document: &Document,
        path: &str,
    ) -> Result<(LutAsset, Option<LutAsset>, bool, PathBuf), CallToolResult> {
        let source = PathBuf::from(path);
        let Some(store) = self.lut_store() else {
            return Err(lut_tool_error(
                "convert_legacy_look",
                "project_not_saved",
                "the project has never been saved, so it has no LUT store root to import this legacy .cube into",
                &serde_json::json!({
                    "field": "project_path",
                    "observed": serde_json::Value::Null,
                    "allowed": "a saved project file path such as <dir>/<stem>.kinewright",
                    "recovery_action": "Save the project first; the store root is <dir>/<stem>.kinewright-assets and is derived from the project path at runtime.",
                    "source_path": path,
                }),
            ));
        };
        let store = match store {
            Ok(store) => store,
            Err(error) => {
                return Err(lut_store_error_result("convert_legacy_look", &error));
            }
        };
        // Ask before touching the filesystem, exactly as `import_lut_asset`
        // does: a refused conversion must leave no store file behind.
        let observed_bytes = std::fs::symlink_metadata(&source)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len());
        let description = format!(
            "The agent wants to convert a legacy cube_lut node into a managed creative_look. That imports the LUT file {} ({}) into this project's LUT store at {}. The bytes are copied under the project directory and registered as an undoable AddLutAsset operation.",
            source.display(),
            observed_bytes.map_or_else(
                || "size unknown".to_owned(),
                |bytes| format!("{bytes} byte(s)")
            ),
            store.luts_dir().display(),
        );
        if let Err(reason) = self
            .confirmations
            .confirm("convert_legacy_look", description)
        {
            return Err(lut_tool_error(
                "convert_legacy_look",
                "import_refused",
                &format!("refused destructive tool convert_legacy_look: {reason}"),
                &serde_json::json!({
                    "field": "confirmation",
                    "observed": reason,
                    "allowed": "an approved confirmation",
                    "recovery_action": "Ask the operator to approve the import, then resend at the current timeline_revision.",
                    "reason": reason,
                    "store_file_written": false,
                    "document_changed": false,
                    "source_path": path,
                }),
            ));
        }
        let import = match store.import_lut_asset(&source) {
            Ok(import) => import,
            Err(error) => return Err(lut_store_error_result("convert_legacy_look", &error)),
        };
        // Content addressing again: the same bytes are the same asset.
        if let Some(existing) = document
            .lut_assets
            .iter()
            .find(|asset| asset.sha256 == import.sha256)
        {
            return Ok((existing.clone(), None, true, store.root().to_path_buf()));
        }
        let lut_asset_id = match document.next_lut_asset_id() {
            Ok(id) => id,
            Err(error) => {
                return Err(lut_tool_error(
                    "convert_legacy_look",
                    "lut_asset_id_exhausted",
                    &error.to_string(),
                    &serde_json::json!({
                        "field": "lut_asset_id",
                        "observed": "exhausted",
                        "allowed": format!("1..={}", kinewright_core::LUT_ASSET_ID_MAX),
                        "recovery_action": "Remove unused LUT asset records before converting another look.",
                    }),
                ));
            }
        };
        let asset = import.into_lut_asset(lut_asset_id);
        Ok((
            asset.clone(),
            Some(asset),
            false,
            store.root().to_path_buf(),
        ))
    }

    /// Render one evidence-only managed colour-node proposal (CC3 §8, CC4 §8).
    ///
    /// Every node planner shares this response shape so an agent that learned
    /// `plan_color_wheels` can read a `plan_creative_look` result unchanged.
    fn color_node_plan<Plan>(&self, tool: &str, plan: Plan) -> Result<CallToolResult, McpError>
    where
        Plan: FnOnce(&Document, TimelineRevision) -> Result<ColorNodePlan, ColorNodePlanError>,
    {
        let (actual_revision, document) = self.snapshot()?;
        let plan = match plan(&document, actual_revision) {
            Ok(plan) => plan,
            Err(ColorNodePlanError::RevisionConflict { expected, actual }) => {
                return Ok(revision_conflict_text(expected, actual));
            }
            Err(error) => {
                return Ok(error_structured(
                    format!("{tool} rejected: {error}"),
                    serde_json::json!({
                        "code": error.code(),
                        "message": error.to_string(),
                        "details": error.details(),
                        "evidence_only": true,
                        "applied": false,
                    }),
                ));
            }
        };
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let value = serde_json::json!({
            "timeline_revision": plan.expected_revision.0,
            "expected_revision": plan.expected_revision.0,
            "clip_id": plan.clip_id.0,
            "kind": plan.kind.effect_name(),
            "effect_id": plan.effect_id.0,
            // Null when the proposal changes nothing and would have had to
            // create the node: no operation allocates that id, so publishing it
            // would name a node that does not exist and may later be reused.
            "target_effect_id": plan.target_effect_id().map(|effect| effect.0),
            "created_new_node": plan.created_new_node,
            "targets_existing_node": plan.targets_existing_node,
            "existing_color_node_count": plan.existing_color_node_count,
            "existing_nodes_of_kind": plan.existing_nodes_of_kind,
            "color_node_limit_per_layer": kinewright_core::COLOR_NODE_LIMIT_PER_LAYER,
            "no_change": plan.no_change,
            "warnings": plan.warnings,
            "assumptions": plan.assumptions,
            "source_profile": plan.source_profile.id(),
            "profile_assumption": plan.profile_assumption,
            "evidence_only": true,
            "applied": false,
            "before": {
                "color_node_count": plan.existing_color_node_count,
                "nodes_of_kind": plan.existing_nodes_of_kind,
            },
            "after": {
                "color_node_count": plan.existing_color_node_count
                    + usize::from(plan.created_new_node),
                "nodes_of_kind": plan.existing_nodes_of_kind
                    + usize::from(plan.created_new_node),
            },
            "requested_parameters": plan.requested_parameters,
            "resolved_parameters": plan.resolved_parameters,
            "requested_curves": plan.requested_curves,
            "resolved_curves": plan.resolved_curves,
            // CC4 §8: the LUT planners publish the exact index their
            // InsertEffect uses, so an ordering rejection is unreachable
            // through the ordinary path, plus the bound asset's identity.
            "insert_index": plan.insert_index,
            "lut_asset": plan.lut_asset,
            "lut_node_limit_per_layer": kinewright_core::LUT_NODE_LIMIT_PER_LAYER,
            "role": plan.kind.role(),
            "color_stage": plan.kind.stage().as_str(),
            "operations": operations,
            "next": "Review these exact operations; submit them through prepare_edit_plan at the same revision if the edit is requested.",
        });
        // CC5 §7: inserted rather than written into the literal above, so a
        // CC3/CC4 plan response is byte-unchanged — the keys are absent, not
        // null, when the planner does not touch a matte.
        let mut value = value;
        if let Some(object) = value.as_object_mut() {
            for (key, field) in [
                ("matte", plan.matte),
                ("predicted_coverage", plan.predicted_coverage),
                ("sample_roi_evidence", plan.sample_evidence),
            ] {
                if let Some(field) = field {
                    object.insert(key.to_owned(), field);
                }
            }
        }
        Ok(success_structured(
            format!(
                "prepared evidence-only {} proposal for clip {} at revision {}; no operation was applied",
                plan.kind.effect_name(),
                plan.clip_id,
                plan.expected_revision
            ),
            value,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn render_color_proof(&self, args: &RenderColorProofArgs) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        let looks = self.look_context(&document);
        // CC4 §8: `effect_id` proofs the *stored* node, so a proposed-primary
        // parameter set alongside it would describe a different edit.
        if let Some(effect) = args.effect_id
            && !args.parameters.is_empty()
        {
            return Ok(color_proof_error_result(
                ColorProofError::LookProofParametersConflict { effect },
            ));
        }
        if args.look_comparison.is_some() && args.effect_id.is_none() {
            return Ok(color_proof_error_result(
                ColorProofError::LookComparisonRequiresEffectId,
            ));
        }
        // CC5 §7: `matte_comparison` is valid only alongside `effect_id`, and
        // both comparisons select what the AFTER cell renders, so exactly one
        // may be sent.
        if args.matte_comparison.is_some() && args.effect_id.is_none() {
            return Ok(color_proof_error_result(
                ColorProofError::MatteComparisonRequiresEffectId,
            ));
        }
        if args.matte_comparison.is_some() && args.look_comparison.is_some() {
            return Ok(color_proof_error_result(
                ColorProofError::MatteComparisonConflictsWithLookComparison,
            ));
        }
        // Resolve the stored node before any render work: an unrenderable
        // request must cost nothing.
        let stored_node = match args.effect_id {
            None => None,
            Some(effect_id) => {
                let Some(stored) = document
                    .clip(args.clip_id)
                    .and_then(|clip| clip.effects.iter().find(|effect| effect.id == effect_id))
                else {
                    return Ok(color_proof_error_result(
                        ColorProofError::ProofEffectNotFound {
                            clip: args.clip_id,
                            effect: effect_id,
                        },
                    ));
                };
                let Some(kind) = kinewright_core::classify_color_node(stored) else {
                    return Ok(color_proof_error_result(
                        ColorProofError::ProofEffectNotAColorNode {
                            effect: effect_id,
                            name: stored.name.clone(),
                        },
                    ));
                };
                // LUT nodes render through the `LutLibrary` the application
                // publishes on the media engine (`FfmpegMediaEngine::
                // set_lut_library`), which the proof renderer reads. An active
                // LUT node whose asset is not published fails the render with a
                // typed `missing_lut_asset:` error rather than a look-free frame.
                // CC3 §5: a CC1 primary carries no bypass control, so the
                // bypass variant is not a state this node can be put into.
                if matches!(args.look_comparison, Some(LookComparison::Bypass))
                    && kind == ColorNodeKind::Primary
                {
                    return Ok(color_proof_error_result(
                        ColorProofError::LookBypassUnsupported {
                            effect: effect_id,
                            kind: kind.effect_name(),
                        },
                    ));
                }
                // CC5 §7: a matte comparison needs a node that both may carry a
                // matte and actually does. Both are checked before any render
                // work, so an unrenderable request costs nothing.
                if args.matte_comparison.is_some() {
                    if !kind.supports_matte() {
                        return Ok(color_proof_error_result(
                            ColorProofError::MatteComparisonUnsupportedKind {
                                effect: effect_id,
                                kind: kind.effect_name(),
                            },
                        ));
                    }
                    let clip_local = args
                        .timecode
                        .0
                        .checked_sub(
                            document
                                .clip(args.clip_id)
                                .map_or(0, |clip| clip.timeline_start.0),
                        )
                        .map_or(TimeCode::ZERO, TimeCode);
                    if !kinewright_core::MatteParams::from_effect(&stored.evaluated_at(clip_local))
                        .has_matte()
                    {
                        return Ok(color_proof_error_result(
                            ColorProofError::MatteComparisonNoMatte { effect: effect_id },
                        ));
                    }
                }
                Some((effect_id, kind))
            }
        };
        let plan_args = PrimaryCorrectionPlanArgs::from(args);
        let plan = match plan_primary_correction(&document, actual_revision, &plan_args) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::from(error)));
            }
        };
        let look_comparison = args.look_comparison.unwrap_or(LookComparison::After);
        if !document.color_context.is_managed_sdr_compatible() {
            return Ok(color_proof_error_result(
                ColorProofError::PipelineIncompatible {
                    reason: format!(
                        "pipeline_state={:?}, working={:?}, monitoring={:?}",
                        document.color_context.pipeline_state,
                        document.color_context.working,
                        document.color_context.monitoring,
                    ),
                },
            ));
        }
        if args.timecode < TimeCode::ZERO || args.timecode >= document.duration {
            return Ok(color_proof_error_result(
                ColorProofError::ProjectFrameOutOfRange {
                    frame: args.timecode,
                    duration: document.duration,
                },
            ));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(color_proof_error_result(ColorProofError::Primary(
                PrimaryPlanError::MissingClip(args.clip_id),
            )));
        };
        let clip_duration = match document.clip_duration(clip) {
            Ok(duration) => duration,
            Err(error) => {
                return Ok(color_proof_error_result(
                    ColorProofError::ClipTimingInvalid {
                        clip: args.clip_id,
                        reason: error.to_string(),
                    },
                ));
            }
        };
        let Some(clip_end) = clip.timeline_start.checked_add(clip_duration) else {
            return Ok(color_proof_error_result(
                ColorProofError::ClipTimingInvalid {
                    clip: args.clip_id,
                    reason: "clip end overflowed".to_owned(),
                },
            ));
        };
        if args.timecode < clip.timeline_start || args.timecode >= clip_end {
            return Ok(color_proof_error_result(
                ColorProofError::ClipFrameOutOfRange {
                    clip: args.clip_id,
                    frame: args.timecode,
                    start: clip.timeline_start,
                    end: clip_end,
                },
            ));
        }
        let Some(asset) = document.asset(clip.asset) else {
            return Ok(color_proof_error_result(ColorProofError::Primary(
                PrimaryPlanError::MissingAsset {
                    clip: args.clip_id,
                    asset: clip.asset,
                },
            )));
        };
        // A proof renders one exact project frame.  Availability therefore
        // follows the compositor's active visual layers at that frame, not
        // every clip in the document (and never audio-only tracks).  This is
        // important for offline bins and for a later shot that is not part of
        // the requested BEFORE/AFTER image.
        let active_visual_layers =
            match kinewright_media::visual_layers_at(&document, args.timecode) {
                Ok(layers) => layers,
                Err(error) => {
                    return Ok(color_proof_error_result(ColorProofError::from_media_error(
                        "visual_layer_resolution",
                        error,
                    )));
                }
            };
        let mut active_rendered_layers = Vec::new();
        let mut active_rendered_sources = Vec::new();
        let mut unsupported_layer_warnings = Vec::new();
        let mut blocking_layer_source: Option<(ClipId, AssetId, ColorSourceError)> = None;
        let mut selected_clip_is_rendered = false;
        for layer in active_visual_layers {
            let (track_id, clip_id, asset_id) = match &layer {
                kinewright_media::TimelineVisualLayer::Video(layer) => (
                    layer.source.track,
                    layer.source.clip,
                    Some(layer.source.asset),
                ),
                kinewright_media::TimelineVisualLayer::Title(layer) => {
                    (layer.track, layer.clip, None)
                }
            };
            let Some(timeline_clip) = Self::document_clip_on_track(&document, track_id, clip_id)
            else {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver returned track {track_id} clip {clip_id}, but that clip is not present in the document"
                    ),
                }));
            };
            if timeline_clip.id == args.clip_id {
                selected_clip_is_rendered = true;
            }
            let Some(asset_id) = asset_id else {
                // Titles are compositor-native overlays and do not require a
                // source file. Record the production title layer explicitly,
                // without inventing asset identity or availability fields.
                let kinewright_media::TimelineVisualLayer::Title(title_layer) = &layer else {
                    unreachable!("only title layers omit an asset id")
                };
                let ClipContent::Title(document_title) = &timeline_clip.content else {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned a title layer for track {track_id} clip {clip_id}, but the document clip is not a title"
                        ),
                    }));
                };
                if document_title != &title_layer.title {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned title parameters that differ from track {track_id} clip {clip_id}"
                        ),
                    }));
                }
                active_rendered_layers.push(serde_json::json!({
                    "track_id": track_id.0,
                    "clip_id": clip_id.0,
                    "content": "title",
                    "title": title_layer.title,
                    "effects": proof_effect_manifest(&title_layer.effects),
                    "color_nodes": proof_color_node_manifest(&title_layer.effects, &looks),
                    "transition": {
                        "alpha": title_layer.transition.alpha,
                        "fade_mix": title_layer.transition.fade_mix,
                        "fade_white": title_layer.transition.fade_white,
                    },
                    "legacy_stage_warnings": legacy_stage_warnings(timeline_clip),
                }));
                unsupported_layer_warnings.extend(Self::layer_compatibility_warnings(
                    track_id,
                    timeline_clip,
                    None,
                ));
                continue;
            };
            if timeline_clip.asset != asset_id {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver mapped track {track_id} clip {clip_id} to asset {asset_id}, but the document clip references asset {}",
                        timeline_clip.asset
                    ),
                }));
            }
            let Some(timeline_asset) = document.asset(asset_id) else {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "visual_layer_resolution",
                    message: format!(
                        "production visual resolver returned missing asset {asset_id} for track {track_id} clip {clip_id}"
                    ),
                }));
            };
            let availability = self.analysis.media_availability(timeline_asset);
            if !matches!(
                availability.kind,
                kinewright_core::MediaAvailabilityKind::OnlineVerified
                    | kinewright_core::MediaAvailabilityKind::OnlineUnverified
            ) {
                return Ok(color_proof_error_result(
                    ColorProofError::MediaUnavailable {
                        clip: timeline_clip.id,
                        asset: timeline_asset.id,
                        status: availability,
                    },
                ));
            }
            let kinewright_media::TimelineVisualLayer::Video(video_layer) = &layer else {
                unreachable!("only video layers include an asset id")
            };
            let content = match &timeline_clip.content {
                ClipContent::Media => "media",
                ClipContent::Freeze(_) => "freeze",
                ClipContent::Title(_) => {
                    return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                        stage: "visual_layer_resolution",
                        message: format!(
                            "production visual resolver returned a source-backed layer for title clip {clip_id} on track {track_id}"
                        ),
                    }));
                }
            };
            // Every active layer is composited into the same BEFORE/AFTER
            // raster, so a non-selected layer's source profile is part of the
            // proof's claim and is classified with the same normative
            // assumption rather than left unreported.
            let (layer_source_status, layer_source_error) =
                active_layer_source_classification(&timeline_asset.color_description);
            if let Some(error) = layer_source_error
                && blocking_layer_source.is_none()
            {
                // The full warning list is only known once every layer has been
                // classified, so the refusal is assembled after the loop.
                blocking_layer_source = Some((timeline_clip.id, timeline_asset.id, error));
            }
            unsupported_layer_warnings.extend(Self::layer_compatibility_warnings(
                track_id,
                timeline_clip,
                Some(timeline_asset.id),
            ));
            active_rendered_layers.push(serde_json::json!({
                "track_id": track_id.0,
                "clip_id": clip_id.0,
                "content": content,
                "asset_id": timeline_asset.id.0,
                "source_frame": video_layer.source.source_at.0,
                "source_end": video_layer.source.source_end.0,
                "timeline_end": video_layer.source.timeline_end.0,
                "source": {
                    "raw_description": timeline_asset.color_description,
                    "provenance": timeline_asset.color_description.provenance,
                    "confidence_basis_points": timeline_asset.color_description.confidence_basis_points,
                    "status": layer_source_status,
                },
                "source_fingerprint": timeline_asset.source_fingerprint,
                "availability": availability,
                // `visual_layers_at` has already evaluated clip-local
                // automation at this exact project frame. Preserve the
                // serialized vector order and resolved primary values so the
                // production layer can be reproduced from the manifest.
                "effects": proof_effect_manifest(&video_layer.effects),
                "color_nodes": proof_color_node_manifest(&video_layer.effects, &looks),
                "transition": {
                    "alpha": video_layer.transition.alpha,
                    "fade_mix": video_layer.transition.fade_mix,
                    "fade_white": video_layer.transition.fade_white,
                },
                "legacy_stage_warnings": legacy_stage_warnings(timeline_clip),
            }));
            // Retain one manifest entry per rendered clip so per-clip legacy
            // warnings remain observable when an asset is overlaid more than
            // once.
            active_rendered_sources.push((track_id, timeline_clip, timeline_asset, availability));
        }
        // A proof whose composite includes an unsupported source cannot honestly
        // claim managed CC1 conformance, so it fails with the exact
        // asset/field/observed/allowed evidence instead of rendering. The
        // non-blocking layer warnings ride along: this error path is the only
        // place they can still be reported for this composite.
        if let Some((clip, asset, error)) = blocking_layer_source {
            return Ok(color_proof_error_result(
                ColorProofError::UnsupportedActiveLayerSource {
                    clip,
                    asset,
                    error,
                    layer_warnings: unsupported_layer_warnings,
                },
            ));
        }
        if !selected_clip_is_rendered {
            return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                stage: "selected_visual_layer",
                message: format!(
                    "selected clip {} is not an active rendered visual layer at project frame {}; an overlapping or higher-priority clip may obscure it",
                    args.clip_id, args.timecode
                ),
            }));
        }

        // CC4 §8: the BEFORE cell of a stored-node proof is the same composite
        // with the node removed, so `bypass` can be asserted byte-identical to
        // `before` rather than merely assumed (CC4 §3.6).
        let scratch_document = |operations: &[Operation]| -> Result<Arc<Document>, String> {
            if operations.is_empty() {
                return Ok(Arc::clone(&document));
            }
            let mut candidate = (*document).clone();
            apply_batch(&mut candidate, operations).map_err(|error| error.to_string())?;
            Ok(Arc::new(candidate))
        };
        // CC5 §7: the scratch automation this proof had to remove to render the
        // variant it names, published in the manifest so the removal is a
        // stated fact rather than an invisible difference from the document.
        let mut cleared_keyframes: Vec<&'static str> = Vec::new();
        let clip_local = args
            .timecode
            .checked_sub(clip.timeline_start)
            .unwrap_or(TimeCode::ZERO);
        let (before_operations, after_operations) = match stored_node {
            None => (Vec::new(), plan.operations.clone()),
            Some((effect_id, _)) => {
                let remove = vec![Operation::RemoveEffect {
                    clip: args.clip_id,
                    effect: effect_id,
                }];
                // CC5 §7: `inside_only` is the document exactly as stored, and
                // `outside_only` is a scratch copy with `matte_invert` toggled,
                // so the two variants partition the raster.
                let after = match (args.matte_comparison, look_comparison) {
                    (Some(MatteComparison::OutsideOnly), _) => {
                        let stored_effect = document.clip(args.clip_id).and_then(|clip| {
                            clip.effects.iter().find(|effect| effect.id == effect_id)
                        });
                        // `matte_invert` is Hold-only but it *is* keyframable,
                        // and automation beats the stored static value at every
                        // frame from its first keyframe onward. So the value to
                        // complement is the one this frame actually renders,
                        // and the static write only lands once the curve is out
                        // of the way — otherwise the "outside" cell would
                        // silently render the inside and the manifest would say
                        // otherwise. The clear is emitted on the scratch copy
                        // only, and only when a curve exists, so a node without
                        // automation produces the byte-identical single
                        // operation it always did.
                        let rendered_invert = stored_effect
                            .and_then(|effect| {
                                effect.integer_parameter_at(MATTE_INVERT_PARAMETER, clip_local)
                            })
                            .map_or_else(
                                || {
                                    stored_effect
                                        .map(kinewright_core::MatteParams::from_effect)
                                        .is_some_and(|matte| matte.is_inverted())
                                },
                                |value| value != 0,
                            );
                        let keyframed = stored_effect.is_some_and(|effect| {
                            effect
                                .keyframes
                                .get(MATTE_INVERT_PARAMETER)
                                .is_some_and(|curve| !curve.keyframes.is_empty())
                        });
                        if keyframed {
                            cleared_keyframes.push(MATTE_INVERT_PARAMETER);
                        }
                        let mut operations = Vec::new();
                        if keyframed {
                            operations.push(Operation::ClearEffectKeyframes {
                                clip: args.clip_id,
                                effect: effect_id,
                                name: MATTE_INVERT_PARAMETER.to_owned(),
                            });
                        }
                        operations.push(Operation::SetEffectParam {
                            clip: args.clip_id,
                            effect: effect_id,
                            name: MATTE_INVERT_PARAMETER.to_owned(),
                            value: ParamValue::Integer(i64::from(!rendered_invert)),
                        });
                        operations
                    }
                    (Some(MatteComparison::Coverage | MatteComparison::InsideOnly), _) => {
                        Vec::new()
                    }
                    (None, LookComparison::Before) => remove.clone(),
                    (None, LookComparison::After) => Vec::new(),
                    (None, LookComparison::Bypass) => vec![Operation::SetEffectParam {
                        clip: args.clip_id,
                        effect: effect_id,
                        name: kinewright_core::COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                        value: ParamValue::Integer(1),
                    }],
                };
                (remove, after)
            }
        };
        // A request that changes nothing produces no operations at all. Core
        // rejects an empty batch, and an identical BEFORE/AFTER is the honest
        // proof of a no-op request.
        let before_document = match scratch_document(&before_operations) {
            Ok(document) => document,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_document",
                    message,
                }));
            }
        };
        let after_document = match scratch_document(&after_operations) {
            Ok(document) => document,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "candidate_document",
                    message,
                }));
            }
        };
        let before = match self
            .analysis
            .monitor_proof_for_document(Arc::clone(&before_document), args.timecode)
        {
            Ok(proof) => proof,
            Err(error) => {
                // CC4 §2.3: an unpublished LUT asset is a typed refusal
                // naming the asset, not a prose render failure.
                return Ok(color_proof_error_result(
                    ColorProofError::from_proof_render_error(
                        "before",
                        error,
                        &looks,
                        stored_node.map(|(effect, _)| effect),
                    ),
                ));
            }
        };
        let after = match self
            .analysis
            .monitor_proof_for_document(Arc::clone(&after_document), args.timecode)
        {
            Ok(proof) => proof,
            Err(error) => {
                return Ok(color_proof_error_result(
                    ColorProofError::from_proof_render_error(
                        "after",
                        error,
                        &looks,
                        stored_node.map(|(effect, _)| effect),
                    ),
                ));
            }
        };
        if !before.metadata.full_resolution || !after.metadata.full_resolution {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: "managed monitor proof did not report full_resolution=true".to_owned(),
            }));
        }
        if before.metadata != after.metadata {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: format!(
                    "before/after renderer provenance differs: {:?} vs {:?}",
                    before.metadata, after.metadata
                ),
            }));
        }
        if before.image.width != document.resolution.0
            || before.image.height != document.resolution.1
            || after.image.width != document.resolution.0
            || after.image.height != document.resolution.1
        {
            return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                stage: "before_after",
                message: format!(
                    "full-resolution proof raster must match document resolution {}x{}; before={}x{}, after={}x{}",
                    document.resolution.0,
                    document.resolution.1,
                    before.image.width,
                    before.image.height,
                    after.image.width,
                    after.image.height,
                ),
            }));
        }
        // CC5 §7: `coverage` replaces the AFTER cell with the §4.1 proof
        // image itself. It is rendered here, after the BEFORE/AFTER rasters
        // have been proved to match the document raster, so the coverage is
        // asserted to be the same size as the picture it describes.
        let mut after = after;
        let mut matte_coverage = None;
        if matches!(args.matte_comparison, Some(MatteComparison::Coverage))
            && let Some((effect_id, _)) = stored_node
        {
            let proof = match self.analysis.matte_proof_for_document(
                Arc::clone(&after_document),
                args.timecode,
                args.clip_id,
                effect_id,
            ) {
                Ok(proof) => proof,
                Err(error) => {
                    return Ok(color_proof_error_result(
                        ColorProofError::MatteProofUnavailable {
                            effect: effect_id,
                            message: error.to_string(),
                        },
                    ));
                }
            };
            if proof.coverage.width != before.image.width
                || proof.coverage.height != before.image.height
            {
                return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                    stage: "matte_coverage",
                    message: format!(
                        "coverage raster {}x{} does not match the proof raster {}x{}",
                        proof.coverage.width,
                        proof.coverage.height,
                        before.image.width,
                        before.image.height,
                    ),
                }));
            }
            matte_coverage = kinewright_core::matte_coverage_statistics(&proof.coverage)
                .ok()
                .map(|statistics| {
                    serde_json::json!({
                        "statistics": statistics,
                        "covered_pixel_count": statistics.covered_pixel_count,
                        "matte_threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
                        "coverage_encoding": proof.metadata.coverage_encoding,
                        "coverage_scale": proof.metadata.coverage_scale,
                        "raster_aspect_millionths": proof.metadata.raster_aspect_millionths,
                    })
                });
            after.image = proof.coverage;
        }
        // CC4 §8: the manifest *asserts* that the bypass variant is the
        // byte-identical twin of the node-removed variant. A difference means
        // a bypassed node still contributed something, so the proof is refused
        // with both hashes and both rasters rather than published with a
        // `bypass_matches_absent: false` footnote nobody has to read.
        let bypass_matches_absent = match (look_comparison, stored_node) {
            (LookComparison::Bypass, Some((effect_id, _))) => {
                let absent = kinewright_media::sha256_bytes(&before.image.pixels);
                let bypassed = kinewright_media::sha256_bytes(&after.image.pixels);
                if absent != bypassed
                    || before.image.width != after.image.width
                    || before.image.height != after.image.height
                {
                    return Ok(color_proof_error_result(
                        ColorProofError::BypassNotLossless {
                            effect: effect_id,
                            absent_rgba8_pixels_sha256: absent,
                            bypass_rgba8_pixels_sha256: bypassed,
                            absent_raster: (before.image.width, before.image.height),
                            bypass_raster: (after.image.width, after.image.height),
                        },
                    ));
                }
                Some(true)
            }
            _ => None,
        };
        let objective = match color_proof_objective(&before.image, &after.image) {
            Ok(objective) => objective,
            Err(message) => {
                return Ok(color_proof_error_result(ColorProofError::InvalidImage {
                    stage: "before_after",
                    message,
                }));
            }
        };
        let sheet = match compose_contact_sheet(&[before.image.clone(), after.image.clone()]) {
            Ok(sheet) => sheet,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_after_composition",
                    message: error.to_string(),
                }));
            }
        };
        let png = match encode_png(&sheet) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let before_png = match encode_png(&before.image) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "before_png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let after_png = match encode_png(&after.image) {
            Ok(png) => png,
            Err(error) => {
                return Ok(color_proof_error_result(ColorProofError::RenderFailed {
                    stage: "after_png_encoding",
                    message: error.to_string(),
                }));
            }
        };
        let hashes = serde_json::json!({
            "before_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&before.image.pixels),
            "after_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&after.image.pixels),
            "before_png_bytes_sha256": kinewright_media::sha256_bytes(&before_png),
            "after_png_bytes_sha256": kinewright_media::sha256_bytes(&after_png),
            "contact_sheet_rgba8_pixels_sha256": kinewright_media::sha256_bytes(&sheet.pixels),
            "contact_sheet_png_bytes_sha256": kinewright_media::sha256_bytes(&png),
        });
        let operations = serde_json::to_value(&plan.operations)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let profile_assumption = plan.profile_assumption.map(|_| {
            serde_json::json!({
                "selected": "d65",
                "source": if args.profile_assumption.is_some() {
                    "explicit"
                } else {
                    "application_profile_assumption"
                },
            })
        });
        let clip_local_frame = args
            .timecode
            .checked_sub(clip.timeline_start)
            .unwrap_or(TimeCode::ZERO);
        // CC5 §7, hoisted out of the manifest literal so the `json!` macro
        // stays inside its recursion budget.
        let matte_comparison_manifest = args.matte_comparison.map(|variant| {
            serde_json::json!({
                "variant": variant.as_str(),
                "effect_id": stored_node.map(|(effect_id, _)| effect_id.0),
                "kind": stored_node.map(|(_, kind)| kind.effect_name()),
                "after_cell": match variant {
                    MatteComparison::Coverage => "the CC5 §4.1 coverage image, R = G = B = round(255 * m), alpha 255",
                    MatteComparison::InsideOnly => "the document as stored: the correction applies inside the matte and nowhere else",
                    MatteComparison::OutsideOnly => "a scratch copy with matte_invert toggled: the correction applies outside the matte and nowhere else",
                },
                "after_operations": after_operations,
                // CC5 §7: `matte_invert` is Hold-only but keyframable, and a
                // static write under an existing curve is dead. `outside_only`
                // therefore clears that curve on the scratch copy, and names it
                // here; empty for every other variant and for a node with no
                // `matte_invert` automation.
                "cleared_keyframes": cleared_keyframes,
                "coverage": matte_coverage,
            })
        });
        let manifest = serde_json::json!({
            "timeline_revision": actual_revision.0,
            "clip_id": args.clip_id.0,
            "asset_id": asset.id.0,
            "active_rendered_layers": active_rendered_layers,
            "unsupported_layer_warnings": unsupported_layer_warnings,
            "active_rendered_sources": active_rendered_sources.iter().map(|(track_id, active_clip, active_asset, availability)| {
                serde_json::json!({
                    "track_id": track_id.0,
                    "clip_id": active_clip.id.0,
                    "content": match &active_clip.content {
                        ClipContent::Media => "media",
                        ClipContent::Freeze(_) => "freeze",
                        ClipContent::Title(_) => "title",
                    },
                    "asset_id": active_asset.id.0,
                    "source": {
                        "raw_description": active_asset.color_description,
                        "provenance": active_asset.color_description.provenance,
                        "confidence_basis_points": active_asset.color_description.confidence_basis_points,
                    },
                    "source_fingerprint": active_asset.source_fingerprint,
                    "availability": availability,
                    "legacy_stage_warnings": legacy_stage_warnings(active_clip),
                })
            }).collect::<Vec<_>>(),
            "project_frame": args.timecode.0,
            "clip_local_frame": clip_local_frame.0,
            "source_profile": plan.source_profile.id(),
            "source": {
                "raw_description": asset.color_description,
                "provenance": asset.color_description.provenance,
                "confidence_basis_points": asset.color_description.confidence_basis_points,
                "profile_assumption": profile_assumption,
            },
            "profile_assumption": profile_assumption,
            "render_kind": before.metadata.render_kind,
            "renderer": "analysis.monitor_proof_for_document",
            "backend": before.metadata.backend,
            "adapter": before.metadata.adapter,
            "backend_provenance": {
                "backend": before.metadata.backend,
                "adapter": before.metadata.adapter,
                "software_fallback": before.metadata.software_fallback,
            },
            "software_fallback": before.metadata.software_fallback,
            "gpu_claim": before.metadata.gpu_claim,
            "full_resolution": before.metadata.full_resolution,
            "cpu_reference": false,
            "decoded_delivery": false,
            "ordered_stage_names": CC1_STAGE_NAMES,
            "legacy_stage_warnings": legacy_stage_warnings(clip),
            "color_context": {
                "pipeline_state": document.color_context.pipeline_state,
                "working": document.color_context.working,
                "monitoring": document.color_context.monitoring,
                "delivery": document.color_context.delivery,
            },
            "formats": {
                "input": {
                    "bit_depth": asset.color_description.bit_depth,
                    "range": asset.color_description.range,
                    "raster": asset.resolution,
                },
                "working": {
                    "bit_depth": document.color_context.working.bit_depth,
                    "range": document.color_context.working.range,
                },
                "monitoring": {
                    "bit_depth": document.color_context.monitoring.bit_depth,
                    "range": document.color_context.monitoring.range,
                },
                "delivery": {
                    "bit_depth": document.color_context.delivery.bit_depth,
                    "range": document.color_context.delivery.range,
                },
                "output": {
                    "bit_depth": "rgba8",
                    "range": document.color_context.monitoring.range,
                    "raster": [before.image.width, before.image.height],
                },
            },
            "sampling_region": {
                "project_frame": args.timecode.0,
                "clip_id": args.clip_id.0,
                "clip_local_frame": clip_local_frame.0,
            },
            "primary_correction": {
                "requested_parameters": plan.requested_parameters,
                "resolved_parameters": plan.resolved_parameters,
            },
            // CC4 §8: which variant the AFTER cell actually rendered, and the
            // exact scratch operations each cell was rendered from.
            "look_comparison": stored_node.map(|(effect_id, kind)| serde_json::json!({
                "effect_id": effect_id.0,
                "kind": kind.effect_name(),
                "role": kind.role(),
                "color_stage": kind.stage().as_str(),
                "variant": look_comparison.as_str(),
                "before_variant": "absent",
                "bypass_matches_absent": bypass_matches_absent,
                "before_operations": before_operations,
                "after_operations": after_operations,
            })),
            "operations": operations,
            "evidence_only": true,
            "applied": false,
            "cells": [
                {
                    "cell": "before",
                    "label": "BEFORE",
                    "index": 0,
                    "x": 0,
                    "y": 0,
                    "width": before.image.width,
                    "height": before.image.height,
                },
                {
                    "cell": "after",
                    "label": "AFTER",
                    "index": 1,
                    "x": before.image.width.saturating_add(STORYBOARD_GUTTER),
                    "y": 0,
                    "width": after.image.width,
                    "height": after.image.height,
                },
            ],
            "sheet": {"width": sheet.width, "height": sheet.height},
            "hashes": hashes,
            "objective": objective,
            "next": "Review the mapped BEFORE/AFTER cells and exact unapplied operations; submit through prepare_edit_plan at the same revision only if the edit is requested.",
        });
        // CC5 §7: inserted rather than written into the literal above, which is
        // already at the `json!` macro's recursion budget. Absent entirely when
        // no matte variant was requested, so a CC4 manifest is byte-unchanged.
        let mut manifest = manifest;
        if let Some(matte_comparison) = matte_comparison_manifest
            && let Some(object) = manifest.as_object_mut()
        {
            object.insert("matte_comparison".to_owned(), matte_comparison);
        }
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!(
                "CC1 colour proof clip={} asset={} project_frame={} revision={} BEFORE|AFTER",
                args.clip_id, asset.id, args.timecode, actual_revision
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
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
            let settings = profile.export_settings(
                &document,
                DeliveryEncodeDepth::Eight,
                ExportCancellation::default(),
            );
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
                "delivery_color": settings.delivery_color,
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
            args.delivery_bit_depth,
            args.focus_x_percent,
            args.focus_y_percent,
        ) {
            Ok(report) => report,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let structured = serde_json::json!({
            "timeline_revision": revision,
            "export_ready": report.export_ready(),
            "delivery_color": report.delivery_color,
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
            DeliveryEncodeDepth::Eight,
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
        let qa_warnings = qa
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Warning)
            .collect::<Vec<_>>();
        let conformance_warnings = conformance
            .issues
            .iter()
            .filter(|issue| issue.severity == kinewright_core::QaSeverity::Warning)
            .collect::<Vec<_>>();
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
                "warning_count": qa_warnings.len(),
                "warning_issues": qa_warnings,
                "blocking_issues": qa.issues.iter().filter(|issue| issue.severity == kinewright_core::QaSeverity::Error).collect::<Vec<_>>(),
            },
            "delivery": {
                "profile": args.profile,
                "delivery_color": conformance.delivery_color,
                "export_ready": conformance.export_ready(),
                "resolution": conformance.resolution,
                "error_count": conformance_errors,
                "warning_count": conformance_warnings.len(),
                "warning_issues": conformance_warnings,
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
                verify: args.verify,
                delivery_bit_depth: args.delivery_bit_depth,
            },
        ) {
            Ok(record) => record,
            Err(error) => return Ok(export_queue_error_result(error)),
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
        if let Some(error) = self.document_availability_error(&document, "frame proof") {
            return Ok(error);
        }
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
        if let Some(error) = self.document_availability_error(&document, "video scopes") {
            return Ok(error);
        }
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

    fn video_scopes_v2(&self, args: &VideoScopesV2Args) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match video_scopes_v2(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "CC2 scopes at timeline revision {revision}: {} sample(s), stage={}",
                    value["temporal"]["sample_count"], value["stage"]
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("get_video_scopes_v2", &error)),
        }
    }

    fn analyze_color_shot(&self, args: &AnalyzeColorShotArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match analyze_color_shot(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "evidence-only CC2 color analysis for clip {} at timeline revision {}; no operation was applied",
                    args.clip_id, revision
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("analyze_color_shot", &error)),
        }
    }

    /// CC6 §7: measure the working stage and publish evidence, nothing else.
    ///
    /// The revision is read once and republished; nothing on this path can
    /// advance it, and the response says so at the top level.
    fn color_qc(&self, args: &ColorQcArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match get_color_qc(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    // Read as the typed values they are: `Value`'s own Display
                    // would quote the stage and is only correct by accident.
                    "evidence-only CC6 colour QC at timeline revision {revision}, stage={}, project frame {}; no operation was applied",
                    value["stage"]
                        .as_str()
                        .unwrap_or(kinewright_core::WORKING_PROOF_STAGE),
                    value["report"]["project_frame"]
                        .as_i64()
                        .unwrap_or_default()
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("get_color_qc", &error)),
        }
    }

    fn plan_shot_match(&self, args: &PlanShotMatchArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        match plan_shot_match(&document, revision, self.analysis.as_ref(), args) {
            Ok(value) => Ok(success_structured(
                format!(
                    "evidence-only CC2 shot match at timeline revision {revision}; no operation was applied",
                ),
                value,
            )),
            Err(error) => Ok(color_scope_error_result("plan_shot_match", &error)),
        }
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
        // CC5 §5.2: `mask_center_x/y_percent` are evaluated at the fragment's
        // *layer* uv (`compositor.wgsl` reads `value / 100` of `input.uv`),
        // which the vertex stage's `scale`/`offset` placement has not yet
        // touched, while the tracker measures the *composited* thumbnail. Seed
        // the search with the stored centre pushed forward through the layer
        // transform resolved at the first sampled frame, and rescale the
        // template by that same scale — exactly as `track_matte_window` does.
        let seed_transform = resolve_layer_transform_at(effect_chain(clip), start);
        let stored_center_percent = [
            parameter("center_x_percent", 50).clamp(0, 100),
            parameter("center_y_percent", 50).clamp(0, 100),
        ];
        #[allow(clippy::cast_precision_loss)]
        let seed_layer = [
            stored_center_percent[0] as f64 / 100.0,
            stored_center_percent[1] as f64 / 100.0,
        ];
        let center_percent = match composite_seed_percent(seed_transform, seed_layer) {
            Ok(percent) => percent,
            Err(seed) => {
                return Ok(tracking_seed_outside_composite_result(
                    ["center_x_percent", "center_y_percent"],
                    args.clip_id,
                    start,
                    seed_transform,
                    &seed,
                    &[],
                ));
            }
        };
        let stored_box_percent = [
            parameter("width_percent", 100),
            parameter("height_percent", 100),
        ];
        let box_percent = [
            tracked_box_percent(stored_box_percent[0], seed_transform.scale),
            tracked_box_percent(stored_box_percent[1], seed_transform.scale),
        ];
        // CC5 §5.2: the template is sized once, at the seed frame's scale, but
        // it must be a legal template at *every* sampled frame. `tracked_box_percent`
        // is monotone in the scale, so testing the smallest and the largest
        // resolved scale tests the whole range — and the refusal names the
        // frame and the scale that failed, not the seed's.
        let scale_extremes = layer_scale_extremes(effect_chain(clip), &sample_frames);
        if let Some((offending, [template_width, template_height])) = scale_extremes
            .and_then(|extremes| offending_template_scale(stored_box_percent, extremes))
        {
            let mut message = "mask width_percent and height_percent must each be in 1..=75 for tracking; set a bounded subject region first".to_owned();
            if (offending.scale - 1.0).abs() > f64::EPSILON {
                use std::fmt::Write as _;
                let _ = write!(
                    message,
                    " (the stored {}x{} percent region maps to a {}x{} percent template on the composite at layer scale {} at clip-local frame {})",
                    stored_box_percent[0],
                    stored_box_percent[1],
                    template_width,
                    template_height,
                    offending.scale,
                    offending.frame,
                );
            }
            return Ok(error_text(message));
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
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        let observations = tracked.observations;
        // CC5 §5.2: the transform is resolved at *each* observation's own
        // frame, so a keyframed scale or offset is converted sample by sample
        // rather than refused. Every written value is the composite centre
        // measured as a fraction of the extent and pulled back into layer uv.
        let converted = observations
            .iter()
            .map(|observation| {
                let transform =
                    resolve_layer_transform_at(effect_chain(clip), observation.local_frame);
                let layer = tracked_centre_layer_unit(
                    observation.center,
                    tracked.width,
                    tracked.height,
                    transform,
                );
                (
                    transform,
                    [
                        layer_unit_to_percent(layer[0]),
                        layer_unit_to_percent(layer[1]),
                    ],
                )
            })
            .collect::<Vec<_>>();

        let curve_for = |axis: usize| AutomationCurve {
            keyframes: observations
                .iter()
                .zip(&converted)
                .map(|(observation, (_, layer))| Keyframe {
                    at: observation.local_frame,
                    value: layer[axis],
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        let x_curve = curve_for(0);
        let y_curve = curve_for(1);
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
            .zip(&converted)
            .map(|(observation, (_, layer))| {
                serde_json::json!({
                    "local_frame": observation.local_frame.0,
                    "project_frame": observation.project_frame.0,
                    // The values the plan writes, in the layer uv the mask is
                    // evaluated in.
                    "center_x_percent": layer[0],
                    "center_y_percent": layer[1],
                    "layer_center_x_percent": layer[0],
                    "layer_center_y_percent": layer[1],
                    // Provenance: what the tracker actually measured, on the
                    // composited thumbnail, in its own raster — read with the
                    // *same* fraction-of-the-extent convention this response's
                    // `coordinate_space.pixel_to_unit` publishes and the layer
                    // values above are converted from, so applying the published
                    // `composite_to_layer` map to these numbers reproduces
                    // `layer_center_*_percent`. The `extent − 1` lattice would
                    // silently disagree with the stated map.
                    "composite_center_pixel": observation.center,
                    "composite_center_x_percent": layer_unit_to_percent(
                        tracker_pixel_to_composite_unit(observation.center[0], tracked.width),
                    ),
                    "composite_center_y_percent": layer_unit_to_percent(
                        tracker_pixel_to_composite_unit(observation.center[1], tracked.height),
                    ),
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
            // CC5 §5.2: the two spaces and the exact maps between them, stated
            // rather than inferred, mirroring `track_matte_window`.
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the mask is evaluated",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "pixel_to_unit": "u_composite = (pixel + 0.5) / extent",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "unit_to_percent": "center_percent = round(u_layer * 100), clamped to 0..=100",
                "seed_center_percent": center_percent,
                "box_percent": box_percent,
                "box_percent_rule": "the stored region rescaled by the layer scale: box_percent = round([width_percent, height_percent] * scale) (CC5 §5.2)",
                "per_frame_transform": true,
                "keyframed_transform": keyframed_transform_note(seed_transform.scale, scale_extremes),
                "samples": observations
                    .iter()
                    .zip(&converted)
                    .map(|(observation, (transform, _))| serde_json::json!({
                        "local_frame": observation.local_frame.0,
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    }))
                    .collect::<Vec<_>>(),
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

    /// Measure one colour node's matte coverage at one exact project frame
    /// (CC5 §4.2).
    ///
    /// Read-only: it renders a scratch proof through the analysis backend and
    /// mutates nothing at all.
    #[allow(clippy::too_many_lines)]
    fn inspect_grade_matte(
        &self,
        args: &InspectGradeMatteArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(expected) = args.expected_revision
            && expected != revision
        {
            return Ok(revision_conflict_text(expected, revision));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(matte_error_result(
                "matte_clip_not_found",
                &format!("clip {} does not exist", args.clip_id),
                &serde_json::json!({
                    "field": "clip_id",
                    "observed": args.clip_id.0,
                    "allowed": "an existing clip id",
                    "recovery_action": "Call get_timeline_state or get_color_context for the current clip ids.",
                }),
            ));
        };
        let Some(effect) = clip
            .effects
            .iter()
            .find(|effect| effect.id == args.effect_id)
        else {
            return Ok(matte_error_result(
                "matte_effect_not_found",
                &format!(
                    "effect {} does not exist on clip {}",
                    args.effect_id, args.clip_id
                ),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": args.effect_id.0,
                    "allowed": "an effect id on the requested clip",
                    "recovery_action": "Call get_color_context for the clip's colour_nodes.",
                    "clip_id": args.clip_id.0,
                }),
            ));
        };
        let Some(kind) = kinewright_core::classify_color_node(effect) else {
            return Ok(matte_error_result(
                "matte_effect_not_a_color_node",
                &format!("effect {} is {}", args.effect_id, effect.name),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": {"effect_id": args.effect_id.0, "name": effect.name},
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    // CC5 §1: the layer `mask` effect is a compositing alpha
                    // operation, not a colour node, and is never a secondary.
                    "recovery_action": "A matte belongs to a managed correction node. The layer `mask` effect is a compositing alpha operation, not a matte; inspect it with get_clip_info.",
                }),
            ));
        };
        if !kind.supports_matte() {
            return Ok(matte_error_result(
                "matte_unsupported_node_kind",
                &format!("{} cannot carry a matte", kind.effect_name()),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": kind.effect_name(),
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    "recovery_action": "A technical input transform normalizes the whole source, so a partially applied one is not a meaningful state (CC5 §2.1).",
                }),
            ));
        }

        // CC5 §2.6: every inactivity question is answered on the *evaluated*
        // stored integers, never on floats and never on the authored values.
        let clip_local = args
            .timecode
            .0
            .checked_sub(clip.timeline_start.0)
            .map_or(TimeCode::ZERO, TimeCode);
        let evaluated = effect.evaluated_at(clip_local);
        let matte = kinewright_core::MatteParams::from_effect(&evaluated);
        let inactive_reason = kinewright_core::color_node_inactive_reason(&evaluated);
        let resolved = matte_parameter_object(&matte);

        let image = match self.analysis.matte_proof_for_document(
            Arc::clone(&document),
            args.timecode,
            args.clip_id,
            args.effect_id,
        ) {
            Ok(proof) => proof,
            Err(error) => {
                // The engine may not implement matte proofs yet, and a node
                // that is inactive or matte-free fails typed rather than
                // returning a blank frame (CC5 §4.1). Both surface here as one
                // stable code with the backend's own message attached, so a
                // caller never mistakes "could not measure" for "empty".
                return Ok(matte_error_result(
                    crate::color_status::MATTE_PROOF_UNAVAILABLE,
                    &format!(
                        "could not render a matte proof for effect {} on clip {} at project frame {}: {error}",
                        args.effect_id, args.clip_id, args.timecode
                    ),
                    &serde_json::json!({
                        "field": "effect_id",
                        "observed": {
                            "effect_id": args.effect_id.0,
                            "clip_id": args.clip_id.0,
                            "project_frame": args.timecode.0,
                            "message": error.to_string(),
                            "node_kind": kind.effect_name(),
                            "active": inactive_reason.is_none(),
                            "inactive_reason": inactive_reason.map(kinewright_core::ColorNodeInactiveReason::as_str),
                            "has_matte": matte.has_matte(),
                        },
                        "allowed": "an active matte-carrying colour node rendered by a backend that implements matte proofs",
                        "recovery_action": "Enable the node's matte with plan_secondary_correction, clear its bypass, or retry once this build's renderer supports matte proofs; no coverage is invented here.",
                        "resolved_matte": resolved,
                    }),
                ));
            }
        };

        let statistics = match kinewright_core::matte_coverage_statistics(&image.coverage) {
            Ok(statistics) => statistics,
            Err(error) => {
                return Ok(matte_error_result(
                    error.code(),
                    &error.to_string(),
                    &serde_json::json!({
                        "field": "coverage",
                        "observed": error.to_string(),
                        "allowed": "a coverage raster with R = G = B and an opaque alpha (CC5 §4.1)",
                        "recovery_action": "The renderer returned a raster that is not a coverage proof; report this build's provenance.",
                    }),
                ));
            }
        };

        let include_image = args.include_image.unwrap_or(true);
        let png = if include_image {
            Some(encode_png(&image.coverage)?)
        } else {
            None
        };
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "project_frame": args.timecode.0,
            "clip_local_frame": clip_local.0,
            "kind": kind.effect_name(),
            "role": kind.role(),
            "color_stage": kind.stage().as_str(),
            // CC5 §1: the two coverage concepts are named apart on every
            // surface, so a reader cannot mistake one for the other.
            "surface": "Matte (this correction)",
            "distinct_from": "Mask (layer alpha), which is a compositing operation and is never a CC1 secondary",
            "active": inactive_reason.is_none(),
            "inactive_reason": inactive_reason.map_or(serde_json::Value::Null, |reason| serde_json::json!(reason.as_str())),
            "matte": crate::color_status::matte_manifest_value(&evaluated),
            "resolved_matte_parameters": resolved,
            "statistics": statistics,
            // CC5 §4.3's threshold, restated at the level a caller reads it.
            "matte_threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
            "covered_pixel_count": statistics.covered_pixel_count,
            "raster": {
                "width": image.coverage.width,
                "height": image.coverage.height,
            },
            "raster_aspect_millionths": image.metadata.raster_aspect_millionths,
            "coverage_encoding": image.metadata.coverage_encoding,
            "coverage_scale": image.metadata.coverage_scale,
            "coverage_histogram_buckets": kinewright_core::MATTE_COVERAGE_HISTOGRAM_BUCKETS,
            "provenance": {
                "render": image.metadata.render,
                "clip_id": image.metadata.clip.0,
                "effect_id": image.metadata.effect.0,
                "node_kind": image.metadata.node_kind,
                "matte_enabled": image.metadata.matte_enabled,
                "window_count": image.metadata.window_count,
                "qualifier_enabled": image.metadata.qualifier_enabled,
            },
            "image_included": include_image,
            "evidence_only": true,
            "applied": false,
        });
        let mut content = vec![ContentBlock::text(format!(
            "matte coverage clip={} effect={} kind={} project_frame={} covered={}/{} pixels ({} bp)",
            args.clip_id,
            args.effect_id,
            kind.effect_name(),
            args.timecode,
            statistics.covered_pixel_count,
            statistics.total_pixel_count,
            statistics.covered_basis_points,
        ))];
        if let Some(png) = png {
            content.push(ContentBlock::image(BASE64.encode(png), "image/png"));
        }
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(structured);
        Ok(result)
    }
    /// Track one matte window through a clip and return an unapplied keyframe
    /// plan (CC5 §5.2).
    ///
    /// Commits nothing: the two `SetEffectKeyframes` operations are returned
    /// as a prepared edit plan, exactly like `track_mask_region`.
    #[allow(clippy::too_many_lines)]
    fn track_matte_window(&self, args: &TrackMatteWindowArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(expected) = args.expected_revision
            && expected != revision
        {
            return Ok(revision_conflict_text(expected, revision));
        }
        let Some(clip) = document.clip(args.clip_id) else {
            return Ok(error_text(format!("clip {} does not exist", args.clip_id)));
        };
        if !matches!(clip.content, ClipContent::Media) {
            return Ok(error_text("matte window tracking requires a media clip"));
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
        let Some(kind) = kinewright_core::classify_color_node(effect) else {
            return Ok(error_text(format!(
                "effect {} is {}; matte window tracking requires a matte-capable colour node",
                args.effect_id, effect.name
            )));
        };
        if !kind.supports_matte() {
            return Ok(matte_error_result(
                "matte_unsupported_node_kind",
                &format!("{} cannot carry a matte", kind.effect_name()),
                &serde_json::json!({
                    "field": "effect_id",
                    "observed": kind.effect_name(),
                    "allowed": crate::color_status::MATTE_CAPABLE_NODE_NAMES,
                    "recovery_action": "Track a window on a primary_correction, color_wheels, color_curves, or creative_look node (CC5 §2.1).",
                }),
            ));
        }
        let window_index = usize::from(args.window_index);
        if window_index >= kinewright_core::MATTE_WINDOW_LIMIT {
            return Ok(matte_error_result(
                "matte_window_index_out_of_range",
                &format!("window_index {} is outside 0..=3", args.window_index),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": args.window_index,
                    "allowed": {"min": 0, "max": kinewright_core::MATTE_WINDOW_LIMIT - 1},
                    "recovery_action": "A matte carries at most four windows (CC5 §2.2).",
                }),
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
        let minimum_confidence = args
            .minimum_confidence_basis_points
            .unwrap_or(DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS);
        if !(0..=10_000).contains(&minimum_confidence) {
            return Ok(error_text(
                "minimum_confidence_basis_points must be in 0..=10000",
            ));
        }

        let Some(first_local) = sample_frames.first().copied() else {
            return Ok(error_text("tracking requires at least one sample"));
        };
        // CC5 §5.2: the window is stored in *layer* uv while the tracker
        // measures the *composite*, so the layer transform must be resolvable
        // and static across the tracked range. A keyframed scale or offset
        // would make one composite pixel mean a different layer position at
        // every sample, which no single conversion can express.
        let transform = match resolve_static_layer_transform(effect_chain(clip), &sample_frames) {
            Ok(transform) => transform,
            Err(unsupported) => {
                return Ok(matte_error_result(
                    "matte_track_layer_transform_unsupported",
                    &format!(
                        "clip {} keyframes its layer {} over the tracked range",
                        args.clip_id, unsupported.field
                    ),
                    &serde_json::json!({
                        "field": unsupported.field,
                        "observed": unsupported.observed,
                        "allowed": "a layer scale and offset that resolve to one value across the whole tracked range",
                        "recovery_action": "Clear the transform automation over the tracked range, or track a range across which the layer transform is static; the matte window is matched with one template of one fixed size, and CC5 §5.2 requires a static layer transform over the tracked range so that template — and the window it produces — is reproducible.",
                        "clip_id": args.clip_id.0,
                        "range": {"start": start.0, "end": end.0},
                    }),
                ));
            }
        };

        let evaluated = effect.evaluated_at(first_local);
        let matte = kinewright_core::MatteParams::from_effect(&evaluated);
        if window_index >= matte.window_count {
            return Ok(matte_error_result(
                "matte_window_not_active",
                &format!(
                    "effect {} resolves matte_window_count {} at clip-local frame {first_local}, so window {} renders nothing",
                    args.effect_id, matte.window_count, args.window_index
                ),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": args.window_index,
                    "allowed": {"max_active_index": matte.window_count.saturating_sub(1), "window_count": matte.window_count},
                    // CC5 §2.2: a window at index >= window_count is preserved
                    // but never rendered, so tracking it would animate geometry
                    // that affects no pixel.
                    "recovery_action": "Raise matte_window_count with plan_secondary_correction so the window renders, then track it.",
                }),
            ));
        }
        let Some(window) = matte.window(window_index).copied() else {
            return Ok(error_text("matte window index is outside 0..=3"));
        };

        // CC5 §5.2: the tracking box is the window's axis-aligned bounding box
        // mapped into the composited thumbnail, so it is rescaled by the layer
        // scale. `box_percent` is a full width/height, hence the factor two.
        let box_percent = [
            matte_track_box_percent(window.half_width_bp, transform.scale),
            matte_track_box_percent(window.half_height_bp, transform.scale),
        ];
        if box_percent.iter().any(|value| !(1..=75).contains(value)) {
            return Ok(matte_error_result(
                "matte_track_window_size_unsupported",
                &format!(
                    "window {} maps to a {}x{} percent template on the composite",
                    args.window_index, box_percent[0], box_percent[1]
                ),
                &serde_json::json!({
                    "field": "window_index",
                    "observed": {
                        "box_percent": box_percent,
                        "half_width_basis_points": window.half_width_bp,
                        "half_height_basis_points": window.half_height_bp,
                        "layer_scale": transform.scale,
                    },
                    "allowed": {"min_percent": 1, "max_percent": 75},
                    "recovery_action": "Shrink the window's half extents to bound the subject before tracking; a template covering most of the frame has no distinguishing content to match.",
                }),
            ));
        }
        let center_percent = match composite_seed_percent(
            transform,
            [
                basis_points_to_unit(window.center_x_bp),
                basis_points_to_unit(window.center_y_bp),
            ],
        ) {
            Ok(percent) => percent,
            Err(seed) => {
                // The repairable input is the window's own stored centre, not
                // the index that selected it, so the refusal names the offending
                // parameter and keeps the index as context.
                let index = args.window_index;
                return Ok(tracking_seed_outside_composite_result(
                    [
                        &format!("matte_window{index}_center_x_basis_points"),
                        &format!("matte_window{index}_center_y_basis_points"),
                    ],
                    args.clip_id,
                    first_local,
                    transform,
                    &seed,
                    &[("window_index", serde_json::json!(index))],
                ));
            }
        };

        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            // CC5 §5.2: excluding *this exact node* by id removes the feedback
            // a matte-scoped correction would otherwise create — as the window
            // moves the graded picture changes inside it and a SAD template
            // would chase its own output — while leaving every other grade and
            // every other effect, including a second node of the same kind,
            // intact.
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };

        let mut observations = Vec::new();
        let mut low_confidence_samples = Vec::new();
        for observation in &tracked.observations {
            let composite = [
                matte_track_centre_basis_points(observation.center[0], tracked.width),
                matte_track_centre_basis_points(observation.center[1], tracked.height),
            ];
            let layer = transform.composite_to_layer_basis_points(composite);
            let record = serde_json::json!({
                "local_frame": observation.local_frame.0,
                "project_frame": observation.project_frame.0,
                "center_x_basis_points": layer[0],
                "center_y_basis_points": layer[1],
                "composite_center_x_basis_points": composite[0],
                "composite_center_y_basis_points": composite[1],
                "center_pixel": observation.center,
                "confidence_basis_points": observation.confidence_basis_points,
            });
            if i64::from(observation.confidence_basis_points) < minimum_confidence {
                low_confidence_samples.push(record);
                continue;
            }
            observations.push((observation.local_frame, layer, record));
        }

        // CC5 §5.2: two surviving samples is the minimum a Linear curve can be
        // built from, and the roadmap's manual fallback is the recovery.
        if observations.len() < MATTE_TRACK_MINIMUM_SAMPLES {
            return Ok(matte_error_result(
                "tracking_confidence_too_low",
                &format!(
                    "only {} of {} samples reached {minimum_confidence} basis points of confidence",
                    observations.len(),
                    tracked.observations.len()
                ),
                &serde_json::json!({
                    "field": "minimum_confidence_basis_points",
                    "observed": {
                        "surviving_samples": observations.len(),
                        "total_samples": tracked.observations.len(),
                        "minimum_confidence_basis_points": minimum_confidence,
                        "low_confidence_samples": low_confidence_samples,
                    },
                    "allowed": {"minimum_surviving_samples": MATTE_TRACK_MINIMUM_SAMPLES},
                    "recovery_action": "Lower minimum_confidence_basis_points, shorten the tracked range, raise max_width, or set the window keyframes by hand; the tracker has no occlusion handling and will not invent a position it did not measure.",
                }),
            ));
        }

        // CC5 §5.2 / M40: raw tracker centres stutter, and tracker noise must
        // not become visible matte motion. The dead zone is deliberately zero
        // - a dead zone lags, which is right for a virtual camera and wrong for
        // a matte, which must stay on the subject.
        let smoothed = [0_usize, 1].map(|axis| {
            kinewright_core::stabilize_tracked_centres_basis_points(
                &observations
                    .iter()
                    .map(|(_, layer, _)| layer[axis])
                    .collect::<Vec<_>>(),
                kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
                MATTE_TRACK_MAX_STEP_BASIS_POINTS,
            )
        });

        let Some(names) = kinewright_core::matte_window_parameter_names(window_index) else {
            return Ok(error_text("matte window index is outside 0..=3"));
        };
        let parameter = |suffix: &str| {
            names
                .iter()
                .find(|name| name.ends_with(suffix))
                .copied()
                .unwrap_or_default()
                .to_owned()
        };
        let curve_for = |axis: usize| AutomationCurve {
            keyframes: observations
                .iter()
                .enumerate()
                .map(|(index, (local_frame, _, _))| Keyframe {
                    at: *local_frame,
                    value: smoothed[axis].get(index).copied().unwrap_or_default(),
                    // CC5 §5.2: sustained movement gets continuous velocity;
                    // M40 rejected eased per-segment curves.
                    interpolation: KeyframeInterpolation::Linear,
                })
                .collect(),
        };
        let x_name = parameter("_center_x_basis_points");
        let y_name = parameter("_center_y_basis_points");
        let x_curve = curve_for(0);
        let y_curve = curve_for(1);
        let operations = vec![
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: x_name.clone(),
                curve: x_curve.clone(),
            },
            Operation::SetEffectKeyframes {
                clip: args.clip_id,
                effect: args.effect_id,
                name: y_name.clone(),
                curve: y_curve.clone(),
            },
        ];
        let plan = match self.prepare_operations(revision, &document, operations) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_text(format!(
                    "tracked matte window keyframes do not fit the current clip: {error}"
                )));
            }
        };

        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "clip_id": args.clip_id.0,
            "effect_id": args.effect_id.0,
            "kind": kind.effect_name(),
            "window_index": args.window_index,
            "range": {"start": start.0, "end": end.0, "step_frames": step},
            "observations": observations
                .iter()
                .map(|(_, _, record)| record.clone())
                .collect::<Vec<_>>(),
            "low_confidence_samples": low_confidence_samples,
            "minimum_confidence_basis_points": minimum_confidence,
            "curves": {
                x_name.clone(): x_curve,
                y_name.clone(): y_curve,
            },
            "parameters": [x_name, y_name],
            // CC5 §5.2: the pinned M40 smoothing policy, published so a reader
            // can reproduce the smoothed curve from the raw observations.
            "window_stabilization": {
                "median_filter": true,
                "dead_zone_basis_points": MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
                "maximum_step_basis_points": MATTE_TRACK_MAX_STEP_BASIS_POINTS,
                "minimum_basis_points": kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                "maximum_basis_points": kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                "interpolation": "Linear",
                "known_systematic_lag": "the three-sample median filter replaces the final sample with median(o[n-3], o[n-2], o[n-1]), so the last smoothed value lags a moving subject by one inter-sample displacement (CC5 §5.2)",
            },
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the matte is evaluated",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "layer_scale": transform.scale,
                "layer_offset": [transform.offset_x, transform.offset_y],
                "pixel_to_basis_points": "centre_bp = round((pixel + 0.5) * 10000 / extent)",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "box_percent": box_percent,
                "box_percent_rule": "the window bounding box rescaled by the layer scale: box_percent = [2 * hw * scale * 100, 2 * hh * scale * 100] (CC5 §5.2)",
            },
            // CC5 §5.2's provenance marker, mirroring M40's.
            "tracking_boundary": MATTE_TRACKING_BOUNDARY,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
            "applied": false,
        });
        Ok(success_structured(
            format!(
                "tracked matte window {} on effect {} of clip {} across {} samples as edit plan {}; inspect the preview, then commit it at timeline revision {revision}",
                args.window_index,
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
        // The stored focus, in the compositor's own precedence: an explicitly
        // stored `focus_*_basis_points` wins over `focus_*_percent`
        // (`compositor.rs`'s `ReframeFocusXBasisPoints` arm only overwrites the
        // percent-derived focus when the parameter is actually present), and a
        // reframe carrying neither is centred. This tool *writes* basis points,
        // so reading only the percent would seed a re-track of its own output
        // at 50 percent instead of where the camera actually is.
        let stored_focus = |basis_points: &str, percent: &str| -> u8 {
            if let Some(value) = effect.integer_parameter_at(basis_points, start) {
                let rounded = (value.clamp(0, 10_000) + 50) / 100;
                return u8::try_from(rounded).unwrap_or(50);
            }
            effect
                .integer_parameter_at(percent, start)
                .map_or(50, |value| u8::try_from(value.clamp(0, 100)).unwrap_or(50))
        };
        let initial_x = args
            .initial_subject_x_percent
            .unwrap_or_else(|| stored_focus("focus_x_basis_points", "focus_x_percent"));
        let initial_y = args
            .initial_subject_y_percent
            .unwrap_or_else(|| stored_focus("focus_y_basis_points", "focus_y_percent"));
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
        // CC5 §5.2: `focus_x/y_basis_points` name the centre of the visible
        // window *inside the layer texture* — `compositor.wgsl` builds
        // `sample_uv` from `reframe_focus_x/y` before the vertex stage places
        // the quad — while the tracker measures the composited thumbnail. Seed
        // the search with the initial focus pushed forward through the layer
        // transform resolved at the first sampled frame, and rescale the
        // subject template by that same scale, exactly as `track_matte_window`
        // does for a window.
        let seed_transform = resolve_layer_transform_at(effect_chain(clip), start);
        let seed_center_percent = match composite_seed_percent(
            seed_transform,
            [f64::from(initial_x) / 100.0, f64::from(initial_y) / 100.0],
        ) {
            Ok(percent) => percent,
            Err(seed) => {
                return Ok(tracking_seed_outside_composite_result(
                    ["initial_subject_x_percent", "initial_subject_y_percent"],
                    args.clip_id,
                    start,
                    seed_transform,
                    &seed,
                    &[],
                ));
            }
        };
        let subject_box_percent = [
            i64::from(args.subject_width_percent),
            i64::from(args.subject_height_percent),
        ];
        let box_percent = [
            tracked_box_percent(subject_box_percent[0], seed_transform.scale),
            tracked_box_percent(subject_box_percent[1], seed_transform.scale),
        ];
        // CC5 §5.2: the template is sized once, at the seed frame's scale, but
        // it must be a legal template at *every* sampled frame, so the gate is
        // applied at the smallest and the largest resolved scale and the
        // refusal names the frame and scale that failed rather than the seed's.
        let scale_extremes = layer_scale_extremes(effect_chain(clip), &sample_frames);
        if let Some((offending, [template_width, template_height])) = scale_extremes
            .and_then(|extremes| offending_template_scale(subject_box_percent, extremes))
        {
            return Ok(error_text(format!(
                "subject_width_percent and subject_height_percent must each be in 1..=75 for tracking; the {}x{} percent subject maps to a {}x{} percent template on the composite at layer scale {} at clip-local frame {}",
                subject_box_percent[0],
                subject_box_percent[1],
                template_width,
                template_height,
                offending.scale,
                offending.frame,
            )));
        }
        let tracked = match self.track_clip_region(&RegionTrackingRequest {
            document: &document,
            clip_id: args.clip_id,
            clip_timeline_start: clip.timeline_start,
            sample_frames: &sample_frames,
            center_percent: seed_center_percent,
            box_percent,
            search_radius_percent: search_radius,
            max_width,
            excluded_effect: args.effect_id,
        }) {
            Ok(tracked) => tracked,
            Err(error) => return Ok(error_text(error)),
        };
        // CC5 §5.2: one transform per observation, resolved at that
        // observation's own frame, so a keyframed scale or offset is converted
        // sample by sample rather than refused.
        let sample_transforms = tracked
            .observations
            .iter()
            .map(|observation| {
                resolve_layer_transform_at(effect_chain(clip), observation.local_frame)
            })
            .collect::<Vec<_>>();
        // The bounds the tracker measured, on the composite, from the same
        // rescaled template it matched with. Pure provenance: nothing plans
        // from these, because the template is sized once at the seed frame's
        // scale and converting it back through a *different* observation's
        // scale would inflate the box by `seed_scale / observation_scale`.
        let composite_samples = tracked
            .observations
            .iter()
            .map(|observation| {
                tracked_subject_bounds(observation, tracked.width, tracked.height, box_percent)
            })
            .collect::<Vec<_>>();
        // Every observation's centre, converted into layer uv with the
        // transform resolved at that observation's own frame.
        let layer_centres = tracked
            .observations
            .iter()
            .zip(&sample_transforms)
            .map(|(observation, transform)| {
                tracked_centre_layer_unit(
                    observation.center,
                    tracked.width,
                    tracked.height,
                    *transform,
                )
            })
            .collect::<Vec<_>>();
        // The reframe crop selects a sub-rectangle of the *layer* texture, so
        // the containment constraint — and the provenance marker that records
        // it — are stated in layer uv too. The box is the converted layer
        // centre bracketed by the *declared* layer subject size, rounded
        // outward and clamped to 0..=10000; it is never routed through the
        // composite template, whose size is pinned to the seed frame's scale.
        let provenance_samples = tracked
            .observations
            .iter()
            .zip(&layer_centres)
            .map(|(observation, centre)| {
                layer_subject_bounds(observation.local_frame, *centre, subject_box_percent)
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
        // CC5 §5.2: every observation is converted *before* the planner sees
        // it — composite pixel as a fraction of the extent, then pulled back
        // into layer uv — so the focus curve is planned, clamped, and written
        // entirely in the space the compositor reads it in.
        let samples = tracked
            .observations
            .iter()
            .zip(&layer_centres)
            .map(|(observation, layer)| SubjectCenterBasisPointSample {
                at: observation.local_frame,
                x_basis_points: layer_unit_to_basis_points(layer[0]),
                y_basis_points: layer_unit_to_basis_points(layer[1]),
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
            samples: provenance_samples.clone(),
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
        // CC5 §5.2 provenance: the raw composite measurement beside the layer
        // value that was actually planned from it, one row per sample.
        let subject_samples = tracked
            .observations
            .iter()
            .zip(&samples)
            .zip(&sample_transforms)
            .zip(&composite_samples)
            .zip(&provenance_samples)
            .map(|((((observation, sample), transform), composite), layer)| {
                serde_json::json!({
                    "local_frame": observation.local_frame.0,
                    "project_frame": observation.project_frame.0,
                    "layer_x_basis_points": sample.x_basis_points,
                    "layer_y_basis_points": sample.y_basis_points,
                    "composite_center_pixel": observation.center,
                    "composite_x_basis_points": matte_track_centre_basis_points(
                        observation.center[0],
                        tracked.width,
                    ),
                    "composite_y_basis_points": matte_track_centre_basis_points(
                        observation.center[1],
                        tracked.height,
                    ),
                    "composite_bounds_basis_points": {
                        "left": composite.left_basis_points,
                        "right": composite.right_basis_points,
                        "top": composite.top_basis_points,
                        "bottom": composite.bottom_basis_points,
                    },
                    // The box containment was planned from and the provenance
                    // marker records: the converted layer centre bracketed by
                    // the declared layer subject size, rounded outward.
                    "layer_bounds_basis_points": {
                        "left": layer.left_basis_points,
                        "right": layer.right_basis_points,
                        "top": layer.top_basis_points,
                        "bottom": layer.bottom_basis_points,
                    },
                    "layer_transform": {
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    },
                    "confidence_basis_points": observation.confidence_basis_points,
                })
            })
            .collect::<Vec<_>>();
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
                "composite_box_percent": box_percent,
                "composite_seed_center_percent": seed_center_percent,
            },
            // CC5 §5.2: the two spaces and the exact maps between them, stated
            // rather than inferred, mirroring `track_matte_window`.
            "coordinate_space": {
                "measured_on": "composited thumbnail, whose uv is the output frame",
                "written_in": "layer uv, which is where the reframe crop window is centred",
                "thumbnail": {"width": tracked.width, "height": tracked.height},
                "pixel_to_unit": "u_composite = (pixel + 0.5) / extent",
                "composite_to_layer": "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5",
                "unit_to_basis_points": "focus_basis_points = round(u_layer * 10000), clamped to 0..=10000",
                "seed_center_percent": seed_center_percent,
                "box_percent": box_percent,
                "box_percent_rule": "the subject template rescaled by the layer scale: box_percent = round([subject_width_percent, subject_height_percent] * scale) (CC5 §5.2)",
                "per_frame_transform": true,
                "keyframed_transform": keyframed_transform_note(seed_transform.scale, scale_extremes),
                "containment_space": "containment is planned in layer uv: each sample's box is the converted layer centre bracketed by the declared subject_width/height_percent (half extent = percent * 50 basis points), rounded outward — floor on left/top, ceil on right/bottom — and clamped to 0..=10000. The composite template bounds are provenance only and are never converted into the constraint, because that template is sized once at the seed frame's scale. The provenance marker records these layer-space bounds",
                "samples": tracked
                    .observations
                    .iter()
                    .zip(&sample_transforms)
                    .map(|(observation, transform)| serde_json::json!({
                        "local_frame": observation.local_frame.0,
                        "scale": transform.scale,
                        "offset_x": transform.offset_x,
                        "offset_y": transform.offset_y,
                    }))
                    .collect::<Vec<_>>(),
            },
            "subject_samples": subject_samples,
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
                    .retain(|effect| effect.id != request.excluded_effect);
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

    /// Render exact frames on both sides of contiguous media cuts.
    ///
    /// Uniform storyboards are intentionally poor at finding one-frame flashes
    /// and near-match jump cuts. This inspector keeps the cut-local evidence
    /// compact and maps every cell back to its exact project frame.
    #[allow(clippy::too_many_lines)]
    fn cut_neighborhoods(&self, args: &CutNeighborhoodsArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if let Some(error) = self.document_availability_error(&document, "cut proof") {
            return Ok(error);
        }
        let Some(track) = document
            .tracks
            .iter()
            .find(|track| track.id == args.track_id)
        else {
            return Ok(error_text(format!(
                "track {} does not exist",
                args.track_id
            )));
        };
        if track.kind != TrackKind::Video {
            return Ok(error_text(format!(
                "track {} is not a video track",
                args.track_id
            )));
        }

        let frames_before = args.frames_before.unwrap_or(1);
        let frames_after = args.frames_after.unwrap_or(3);
        if !(1..=6).contains(&frames_before) || !(1..=6).contains(&frames_after) {
            return Ok(error_text(
                "frames_before and frames_after must be in 1..=6",
            ));
        }
        let cut_count = args.cut_count.unwrap_or(12);
        if !(1..=12).contains(&cut_count) {
            return Ok(error_text("cut_count must be in 1..=12"));
        }
        let max_width = args.max_width.unwrap_or(160);
        if !(64..=THUMBNAIL_MAX_WIDTH).contains(&max_width) {
            return Ok(error_text(format!(
                "max_width must be in 64..={THUMBNAIL_MAX_WIDTH}"
            )));
        }
        let maximum_secondary_change_basis_points = args
            .maximum_secondary_change_basis_points
            .unwrap_or(DEFAULT_MAXIMUM_CUT_SECONDARY_CHANGE_BASIS_POINTS);
        if maximum_secondary_change_basis_points > 10_000 {
            return Ok(error_text(
                "maximum_secondary_change_basis_points must be in 0..=10000",
            ));
        }

        let mut clips = track
            .clips
            .iter()
            .filter(|clip| clip.content.is_media())
            .collect::<Vec<_>>();
        clips.sort_by_key(|clip| (clip.timeline_start, clip.id));
        let mut cuts = Vec::new();
        for pair in clips.windows(2) {
            let outgoing = pair[0];
            let incoming = pair[1];
            let Some(outgoing_end) = document
                .clip_duration(outgoing)
                .ok()
                .and_then(|duration| outgoing.timeline_start.checked_add(duration))
            else {
                return Ok(error_text(format!(
                    "could not map clip {} duration",
                    outgoing.id
                )));
            };
            if outgoing_end == incoming.timeline_start {
                cuts.push((incoming.timeline_start, outgoing.id, incoming.id));
            }
        }

        let cut_offset = args.cut_offset.unwrap_or_default();
        if cut_offset > cuts.len() {
            return Ok(error_text(format!(
                "cut_offset {cut_offset} exceeds {} contiguous media cuts",
                cuts.len()
            )));
        }
        let selected = cuts
            .iter()
            .enumerate()
            .skip(cut_offset)
            .take(usize::from(cut_count))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(success_structured(
                format!(
                    "track {} has no selected contiguous media cuts",
                    args.track_id
                ),
                serde_json::json!({
                    "timeline_revision": revision.0,
                    "track_id": args.track_id.0,
                    "total_cut_count": cuts.len(),
                    "cut_offset": cut_offset,
                    "returned_cut_count": 0,
                    "cuts": [],
                    "cells": [],
                }),
            ));
        }

        let returned_cut_count = selected.len();
        let mut images = Vec::with_capacity(
            selected.len() * (usize::from(frames_before) + usize::from(frames_after)),
        );
        let mut cells = Vec::with_capacity(images.capacity());
        let mut cut_manifest = Vec::with_capacity(selected.len());
        let mut issues = Vec::new();
        for &(cut_index, &(cut_frame, outgoing_clip, incoming_clip)) in &selected {
            let first_cell = cells.len() + 1;
            let first_image = images.len();
            let mut offsets =
                Vec::with_capacity(usize::from(frames_before) + usize::from(frames_after));
            for offset in -i64::from(frames_before)..i64::from(frames_after) {
                let project_frame = TimeCode(cut_frame.0.saturating_add(offset));
                if project_frame < TimeCode::ZERO || project_frame >= document.duration {
                    continue;
                }
                match self.analysis.thumbnail_for_document(
                    Arc::clone(&document),
                    project_frame,
                    max_width,
                ) {
                    Ok(image) => images.push(image),
                    Err(error) => return Ok(error_text(error.to_string())),
                }
                offsets.push(offset);
                cells.push(serde_json::json!({
                    "cell": cells.len() + 1,
                    "cut_index": cut_index,
                    "cut_frame": cut_frame.0,
                    "project_frame": project_frame.0,
                    "offset_from_cut": offset,
                    "side": if offset < 0 { "outgoing" } else { "incoming" },
                }));
            }
            let changes = images[first_image..]
                .windows(2)
                .zip(offsets.windows(2))
                .map(|(pair, offsets)| {
                    let change_basis_points =
                        rgba_mean_absolute_difference_basis_points(&pair[0], &pair[1])
                            .unwrap_or(10_000);
                    let secondary_change = offsets[0] >= 0
                        && change_basis_points > maximum_secondary_change_basis_points;
                    if secondary_change {
                        issues.push(serde_json::json!({
                            "cut_index": cut_index,
                            "cut_frame": cut_frame.0,
                            "kind": "suspected_internal_cut_after_in_point",
                            "from_offset": offsets[0],
                            "to_offset": offsets[1],
                            "change_basis_points": change_basis_points,
                            "maximum_basis_points": maximum_secondary_change_basis_points,
                        }));
                    }
                    serde_json::json!({
                        "from_offset": offsets[0],
                        "to_offset": offsets[1],
                        "change_basis_points": change_basis_points,
                        "secondary_change": secondary_change,
                    })
                })
                .collect::<Vec<_>>();
            cut_manifest.push(serde_json::json!({
                "cut_index": cut_index,
                "project_frame": cut_frame.0,
                    "outgoing_clip_id": outgoing_clip.0,
                    "incoming_clip_id": incoming_clip.0,
                "first_cell": first_cell,
                "last_cell": cells.len(),
                "adjacent_changes": changes,
            }));
        }

        let sheet = compose_contact_sheet(&images)?;
        let png = encode_png(&sheet)?;
        let next_cut_offset = (cut_offset + returned_cut_count < cuts.len())
            .then_some(cut_offset + returned_cut_count);
        let manifest = serde_json::json!({
            "timeline_revision": revision.0,
            "track_id": args.track_id.0,
            "total_cut_count": cuts.len(),
            "cut_offset": cut_offset,
            "returned_cut_count": returned_cut_count,
            "next_cut_offset": next_cut_offset,
            "frames_before": frames_before,
            "frames_after": frames_after,
            "maximum_secondary_change_basis_points": maximum_secondary_change_basis_points,
            "clean": issues.is_empty(),
            "issue_count": issues.len(),
            "issues": issues,
            "cuts": cut_manifest,
            "cells": cells,
            "sheet": {"width": sheet.width, "height": sheet.height},
        });
        let status = if manifest["clean"] == true {
            "CUT EDGE REVIEW PASSED"
        } else {
            "CUT EDGE REVIEW FAILED"
        };
        let mut result = CallToolResult::success(vec![
            ContentBlock::text(format!("{status}: cut neighborhoods {manifest}")),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]);
        result.structured_content = Some(manifest);
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    fn source_storyboard(&self, args: &SourceStoryboardArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
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
        if let Some(error) = self.source_availability_error(&asset, "source storyboard") {
            return Ok(error);
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
            "timeline_revision": revision.0,
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

    /// Prepare one explicit source/program patch against one observed
    /// timeline revision. Core owns the compound operation's exact
    /// three-point derivation, route validation, insert/overwrite semantics,
    /// and linked A/V construction; this boundary only adds typed agent
    /// routing, revision gating, and inspectable evidence around it.
    #[allow(clippy::too_many_lines)]
    fn source_program_edit_plan(
        &self,
        args: &SourceProgramEditArgs,
    ) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        if args.expected_revision != revision {
            return Ok(revision_conflict_text(args.expected_revision, revision));
        }

        let Some(asset) = document.asset(args.asset).cloned() else {
            return Ok(error_structured(
                format!("asset {} does not exist", args.asset),
                serde_json::json!({
                    "code": "missing_asset",
                    "asset_id": args.asset.0,
                    "timeline_revision": revision.0,
                }),
            ));
        };
        if let Some(error) = self.source_availability_error(&asset, "source program edit") {
            return Ok(error);
        }
        if args.video_track.is_none() && args.audio_track.is_none() {
            return Ok(error_structured(
                "source program edit requires at least one explicit destination",
                serde_json::json!({
                    "code": "empty_source_patch",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "video_track": serde_json::Value::Null,
                    "audio_track": serde_json::Value::Null,
                }),
            ));
        }
        if let (Some(video_track), Some(audio_track)) = (args.video_track, args.audio_track)
            && video_track == audio_track
        {
            let track = video_track;
            return Ok(error_structured(
                format!("source program edit targets track {track} more than once"),
                serde_json::json!({
                    "code": "duplicate_source_patch_track",
                    "track_id": track.0,
                    "timeline_revision": revision.0,
                }),
            ));
        }

        for (component, requested, expected_kind) in [
            ("video", args.video_track, TrackKind::Video),
            ("audio", args.audio_track, TrackKind::Audio),
        ] {
            let Some(track_id) = requested else {
                continue;
            };
            if !asset.kind.supports(expected_kind) {
                return Ok(error_structured(
                    format!(
                        "asset {} has no {component} component for destination track {track_id}",
                        asset.id
                    ),
                    serde_json::json!({
                        "code": "invalid_source_patch_route_kind",
                        "component": component,
                        "asset_kind": asset.kind,
                        "expected_track_kind": expected_kind,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            }
            let Some(track) = document.tracks.iter().find(|track| track.id == track_id) else {
                return Ok(error_structured(
                    format!("destination track {track_id} does not exist"),
                    serde_json::json!({
                        "code": "missing_source_patch_track",
                        "component": component,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            };
            if track.kind != expected_kind {
                return Ok(error_structured(
                    format!(
                        "{component} source route requires a {expected_kind:?} track, got {:?} track {track_id}",
                        track.kind
                    ),
                    serde_json::json!({
                        "code": "invalid_source_patch_route_kind",
                        "component": component,
                        "expected_track_kind": expected_kind,
                        "actual_track_kind": track.kind,
                        "track_id": track_id.0,
                        "timeline_revision": revision.0,
                    }),
                ));
            }
        }

        let operation = Operation::PatchedThreePointEdit {
            asset: asset.id,
            source_in: args.source_in,
            source_out: args.source_out,
            timeline_in: args.timeline_in,
            timeline_out: args.timeline_out,
            mode: args.mode,
            video_track: args.video_track,
            audio_track: args.audio_track,
        };
        // Resolve the derived range on an isolated, clip-free copy. This is
        // deliberately separate from the actual preview: overwrite may
        // remove the highest existing clip id and Core is allowed to reuse
        // that id for the replacement. Matching by source/timeline semantics
        // therefore remains correct where an id-only before/after diff would
        // lose the new clip.
        let mut range_document = document.as_ref().clone();
        for track in &mut range_document.tracks {
            track.clips.clear();
        }
        range_document.duration = TimeCode::ZERO;
        if let Err(error) = operation.apply(&mut range_document) {
            return Ok(error_structured(
                format!("source program edit is invalid: {error}"),
                serde_json::json!({
                    "code": "invalid_source_program_edit",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "mode": args.mode,
                    "video_track": args.video_track.map(|track| track.0),
                    "audio_track": args.audio_track.map(|track| track.0),
                    "error": error.to_string(),
                }),
            ));
        }
        let expected_clip = args
            .video_track
            .or(args.audio_track)
            .and_then(|track_id| {
                range_document
                    .tracks
                    .iter()
                    .find(|track| track.id == track_id)
                    .and_then(|track| track.clips.iter().find(|clip| clip.asset == asset.id))
            })
            .ok_or_else(|| {
                McpError::internal_error(
                    "patched source program range resolution produced no route clip",
                    None,
                )
            })?;
        let expected_source = expected_clip.source_range.clone();
        let expected_timeline_start = expected_clip.timeline_start;
        let mut projected = document.as_ref().clone();
        if let Err(error) = operation.apply(&mut projected) {
            return Ok(error_structured(
                format!("source program edit is invalid: {error}"),
                serde_json::json!({
                    "code": "invalid_source_program_edit",
                    "asset_id": asset.id.0,
                    "timeline_revision": revision.0,
                    "mode": args.mode,
                    "video_track": args.video_track.map(|track| track.0),
                    "audio_track": args.audio_track.map(|track| track.0),
                    "error": error.to_string(),
                }),
            ));
        }

        let mut routed_clips = BTreeMap::new();
        for (component, requested) in [("video", args.video_track), ("audio", args.audio_track)] {
            let Some(track_id) = requested else {
                continue;
            };
            let Some(clip) = projected
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .and_then(|track| {
                    track.clips.iter().find(|clip| {
                        clip.asset == asset.id
                            && clip.source_range == expected_source
                            && clip.timeline_start == expected_timeline_start
                    })
                })
            else {
                return Err(McpError::internal_error(
                    format!(
                        "patched source program operation did not produce its {component} route"
                    ),
                    None,
                ));
            };
            routed_clips.insert(component, clip.clone());
        }

        let first_clip = routed_clips
            .values()
            .next()
            .expect("at least one route was validated");
        let duration = projected
            .clip_duration(first_clip)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let timeline_out = first_clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| McpError::internal_error("timeline range overflowed", None))?;
        for clip in routed_clips.values() {
            if clip.source_range != first_clip.source_range
                || clip.timeline_start != first_clip.timeline_start
            {
                return Err(McpError::internal_error(
                    "patched source program routes are not aligned",
                    None,
                ));
            }
        }
        let linked = routed_clips.len() > 1
            && routed_clips
                .values()
                .map(|clip| clip.link)
                .collect::<BTreeSet<_>>()
                .len()
                == 1
            && routed_clips.values().all(|clip| clip.link.is_some());
        if routed_clips.len() > 1 && !linked {
            return Err(McpError::internal_error(
                "patched source program routes are not linked",
                None,
            ));
        }

        let plan = match self.prepare_operations(revision, &document, vec![operation]) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(error_structured(
                    format!("source program edit could not be prepared: {error}"),
                    serde_json::json!({
                        "code": "invalid_source_program_edit",
                        "asset_id": asset.id.0,
                        "timeline_revision": revision.0,
                        "error": error,
                    }),
                ));
            }
        };

        let routed = routed_clips
            .iter()
            .map(|(component, clip)| {
                (
                    (*component).to_owned(),
                    serde_json::json!({
                        "track_id": if *component == "video" { args.video_track } else { args.audio_track },
                        "clip_id": clip.id.0,
                        "link_id": clip.link.map(|link| link.0),
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let structured = serde_json::json!({
            "timeline_revision": revision.0,
            "asset_id": asset.id.0,
            "mode": args.mode,
            "source_range": {
                "start": first_clip.source_range.start.0,
                "end": first_clip.source_range.end.0,
            },
            "timeline_range": {
                "start": first_clip.timeline_start.0,
                "end": timeline_out.0,
            },
            "destinations": routed,
            "linked": linked,
            "prepared_edit_plan": {
                "plan_id": plan.id,
                "expected_revision": revision,
                "preview": plan.preview,
            },
        });
        Ok(success_structured(
            format!(
                "prepared source program {} edit for asset {} as plan {}; inspect the preview, then commit it at timeline revision {revision}",
                match args.mode {
                    ThreePointMode::Insert => "insert",
                    ThreePointMode::Overwrite => "overwrite",
                },
                asset.id,
                plan.id,
            ),
            structured,
        ))
    }

    /// Return source-monitor candidates derived from cached scene boundaries.
    ///
    /// This deliberately builds an isolated, throwaway document for thumbnail
    /// rendering. It is an inspector: no Core command, prepared plan, or
    /// playback document is changed.
    #[allow(clippy::too_many_lines)]
    fn source_shot_board(&self, args: &SourceShotBoardArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
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
        if let Some(error) = self.source_availability_error(&asset, "source shot board") {
            return Ok(error);
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
                    "timeline_revision": revision.0,
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
            "timeline_revision": revision.0,
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
        if let Some(error) = self.document_availability_error(document, label) {
            return Ok(error);
        }
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

    fn source_availability_error(
        &self,
        asset: &MediaAsset,
        consumer: &str,
    ) -> Option<CallToolResult> {
        let availability = self.analysis.media_availability(asset);
        if matches!(
            availability.kind,
            MediaAvailabilityKind::OnlineVerified | MediaAvailabilityKind::OnlineUnverified
        ) {
            return None;
        }
        Some(error_structured(
            format!(
                "{consumer} cannot read asset {} at {}: {:?}",
                asset.id,
                asset.path.display(),
                availability.kind
            ),
            serde_json::json!({
                "asset_id": asset.id.0,
                "path": asset.path,
                "availability": availability,
                "consumer": consumer,
            }),
        ))
    }

    fn ensure_verified_patched_sources(
        &self,
        document: &Document,
        operations: &[Operation],
    ) -> Result<(), String> {
        let asset_ids = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::PatchedThreePointEdit { asset, .. } => Some(*asset),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        self.ensure_verified_source_assets(document, &asset_ids)
    }

    fn ensure_verified_source_assets(
        &self,
        document: &Document,
        asset_ids: &BTreeSet<AssetId>,
    ) -> Result<(), String> {
        for asset_id in asset_ids {
            let Some(asset) = document.asset(*asset_id) else {
                return Err(format!(
                    "patched_three_point_edit references missing asset {asset_id}"
                ));
            };
            let availability = self.analysis.media_availability(asset);
            if availability.kind != MediaAvailabilityKind::OnlineVerified {
                return Err(format!(
                    "patched_three_point_edit requires asset {asset_id} to be online_verified at preparation and commit; current availability is {:?} ({})",
                    availability.kind,
                    availability
                        .reason
                        .as_deref()
                        .unwrap_or("no backend reason supplied")
                ));
            }
        }
        Ok(())
    }

    fn document_availability_error(
        &self,
        document: &Document,
        consumer: &str,
    ) -> Option<CallToolResult> {
        // An offline item sitting unused in the media pool must not block a
        // timeline proof. Only source-backed clips can contribute decoded
        // pixels; titles are project-native and need no source file.
        let mut inspected = BTreeSet::new();
        document.tracks.iter().find_map(|track| {
            track.clips.iter().find_map(|clip| {
                if matches!(clip.content, ClipContent::Title(_)) || !inspected.insert(clip.asset) {
                    return None;
                }
                document
                    .asset(clip.asset)
                    .and_then(|asset| self.source_availability_error(asset, consumer))
            })
        })
    }

    /// Resolve a production visual-layer identity back to its exact document
    /// clip. The media resolver owns interval, transition, freeze-frame, and
    /// overlap semantics; this lookup only joins its stable ids to metadata
    /// needed by the proof manifest.
    fn document_clip_on_track(
        document: &Document,
        track_id: TrackId,
        clip_id: ClipId,
    ) -> Option<&Clip> {
        document
            .tracks
            .iter()
            .find(|track| track.id == track_id)?
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
    }

    /// Every non-blocking reason one active proof layer falls outside the
    /// managed CC1 claim: each post-primary compatibility stage on the layer's
    /// effect chain.
    ///
    /// This covers non-selected layers too, so a proof can never present an
    /// unqualified managed claim for a composite that contains one.
    ///
    /// A blocking source profile is deliberately not reported here. It refuses
    /// the proof outright, so it is carried by the
    /// `active_layer_needs_color_override` error rather than by a warning that
    /// no successful response could ever contain.
    fn layer_compatibility_warnings(
        track_id: TrackId,
        clip: &Clip,
        asset: Option<AssetId>,
    ) -> Vec<serde_json::Value> {
        let mut warnings = Vec::new();
        warnings.extend(legacy_stage_warnings(clip).into_iter().map(|warning| {
            serde_json::json!({
                "track_id": track_id.0,
                "clip_id": clip.id.0,
                "asset_id": asset.map_or(serde_json::Value::Null, |asset| {
                    serde_json::json!(asset.0)
                }),
                "code": warning["code"].clone(),
                "blocking": false,
                "message": warning["message"].clone(),
                "effect_id": warning["effect_id"].clone(),
                "effect_index": warning["effect_index"].clone(),
                "name": warning["name"].clone(),
            })
        }));
        warnings
    }

    #[allow(clippy::too_many_lines)]
    fn source_info(&self, args: &SourceInfoArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
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
        let availability = self.analysis.media_availability(asset);
        let destinations = serde_json::json!({
            "video": document
                .tracks
                .iter()
                .filter(|track| {
                    track.kind == TrackKind::Video && asset.kind.supports(TrackKind::Video)
                })
                .map(|track| {
                    serde_json::json!({
                        "track_id": track.id.0,
                        "kind": track.kind,
                        "sync_lock": track.sync_lock,
                    })
                })
                .collect::<Vec<_>>(),
            "audio": document
                .tracks
                .iter()
                .filter(|track| {
                    track.kind == TrackKind::Audio && asset.kind.supports(TrackKind::Audio)
                })
                .map(|track| {
                    serde_json::json!({
                        "track_id": track.id.0,
                        "kind": track.kind,
                        "sync_lock": track.sync_lock,
                    })
                })
                .collect::<Vec<_>>(),
        });
        let value = serde_json::json!({
            "timeline_revision": revision.0,
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
                "color_description": asset.color_description,
                "persisted_fingerprint": asset.source_fingerprint,
                "availability": availability,
            },
            "source_monitor": {
                "source_in": source_in.0,
                "source_out": source_out.0,
                "duration": source_out.0 - source_in.0,
                "in_marked": args.source_in.is_some(),
                "out_marked": args.source_out.is_some(),
            },
            "destinations": destinations,
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
        self.ensure_verified_patched_sources(document, &operations)?;
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
                    range.clone(),
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
                        let failure = error.to_string();
                        let recovery = beat_montage_plan_near_anchors_with_report(
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
                            None,
                            &[],
                            args.cadence,
                        )
                        .ok()
                        .map(|(suggested_plan, suggested_report)| {
                            let shot_durations = suggested_plan
                                .shots
                                .iter()
                                .map(|shot| {
                                    shot.timeline_range
                                        .end
                                        .0
                                        .saturating_sub(shot.timeline_range.start.0)
                                })
                                .collect::<Vec<_>>();
                            serde_json::json!({
                                "cut_anchor_frames": suggested_report.resolved_anchors,
                                "shot_durations": shot_durations,
                                "signed_delta_frames": suggested_report.signed_deltas,
                                "maximum_absolute_delta_frames": suggested_report.maximum_absolute_delta,
                                "total_absolute_delta_frames": suggested_report.total_absolute_delta,
                                "exact_retry_patch": {
                                    "cut_anchor_frames": suggested_report.resolved_anchors,
                                    "anchor_repair": {
                                        "maximum_movement_frames": 0,
                                        "locked_anchor_indices": [],
                                    },
                                },
                            })
                        });
                        let message = format!(
                            "beat montage anchor repair could not satisfy preferred anchors within maximum_movement_frames={}: {failure}; revise preferred anchors, increase the explicit bound, unlock an anchor, or adjust source envelopes and retry{}",
                            settings.maximum_movement_frames,
                            if recovery.is_some() {
                                "; the structured error includes the nearest globally feasible source- and cadence-valid anchor schedule plus an exact_retry_patch, so reuse it instead of guessing"
                            } else {
                                ""
                            }
                        );
                        if let Some(recovery) = recovery {
                            return Ok(error_structured(
                                message,
                                serde_json::json!({
                                    "status": "bounded_anchor_repair_infeasible",
                                    "error": failure,
                                    "requested_maximum_movement_frames": settings.maximum_movement_frames,
                                    "requested_locked_anchor_indices": settings.locked_anchor_indices,
                                    "nearest_globally_feasible": recovery,
                                }),
                            ));
                        }
                        return Ok(error_text(message));
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
                "Inspect with get_timeline_state. Open names already in the user request with one batched get_capability call; search only unnamed needs or after a miss. Load only needed schemas. Invoke capabilities through invoke_capability. For source/program patching, use plan_source_program_edit with explicit video_track and audio_track destinations; never reconstruct a dual V/A patch as two three_point_edit operations because that can double-ripple the timeline. When a planner returns prepared_edit_plan, inspect its preview and commit that plan id directly. Use prepare_edit_plan only for model-authored operations. Reinspect after revision conflicts. Frames are exact project integers.",
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
    /// Search radius around the previous center as a *composited-frame*
    /// percentage, not a layer percentage. Defaults to 10.
    #[serde(default)]
    search_radius_percent: Option<u8>,
    /// Analysis render width. Defaults to 256; valid range 64..=512.
    #[serde(default)]
    max_width: Option<u32>,
}

/// Arguments for the read-only CC5 matte inspector (CC5 §4.2).
#[derive(Debug, Deserialize, JsonSchema)]
struct InspectGradeMatteArgs {
    /// Exact timeline revision returned by the preceding inspection. When
    /// supplied, a stale snapshot fails closed.
    #[serde(default)]
    expected_revision: Option<TimelineRevision>,
    /// Stable visual media clip id carrying the correction node.
    clip_id: ClipId,
    /// Stable matte-carrying colour node id on that clip.
    effect_id: EffectId,
    /// Exact project frame to measure.
    #[serde(alias = "frame")]
    timecode: TimeCode,
    /// Attach the coverage PNG. Defaults to true.
    #[serde(default)]
    include_image: Option<bool>,
}

/// Arguments for the CC5 matte window tracker (CC5 §5.2).
#[derive(Debug, Deserialize, JsonSchema)]
struct TrackMatteWindowArgs {
    /// Exact timeline revision returned by the preceding inspection.
    #[serde(default)]
    expected_revision: Option<TimelineRevision>,
    /// Stable visual media clip id carrying the correction node.
    clip_id: ClipId,
    /// Stable matte-carrying colour node id on that clip.
    effect_id: EffectId,
    /// Which of the node's up-to-four windows to track, `0..=3`.
    window_index: u8,
    /// First clip-local frame to track. Defaults to zero.
    #[serde(default)]
    start_local_frame: Option<TimeCode>,
    /// Exclusive clip-local end frame. Defaults to the clip duration.
    #[serde(default)]
    end_local_frame: Option<TimeCode>,
    /// Distance between tracked keyframes. Defaults to 5; valid range 1..=120.
    #[serde(default)]
    step_frames: Option<i64>,
    /// Search radius around the previous center as a *composited-frame*
    /// percentage, not a layer percentage. Defaults to 10; valid range 1..=25.
    #[serde(default)]
    search_radius_percent: Option<u8>,
    /// Analysis render width. Defaults to 256; valid range 64..=512.
    #[serde(default)]
    max_width: Option<u32>,
    /// Samples below this confidence are dropped and reported. Defaults to
    /// 5000; fewer than two survivors fails with `tracking_confidence_too_low`.
    #[serde(default)]
    minimum_confidence_basis_points: Option<i64>,
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
    #[serde(default = "crate::schema::default_true")]
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
    /// Width of the initial subject template as a *layer* percentage, in
    /// 1..=75. The composite template is size x layer scale, and that product
    /// must also be in 1..=75 at every sampled frame.
    subject_width_percent: u8,
    /// Height of the initial subject template as a *layer* percentage, in
    /// 1..=75. The composite template is size x layer scale, and that product
    /// must also be in 1..=75 at every sampled frame.
    subject_height_percent: u8,
    /// Horizontal center of the subject template as a *layer* percentage,
    /// forward-mapped through the clip's layer transform to seed the search on
    /// the composite. Defaults to the stored `focus_x_basis_points`, else
    /// `focus_x_percent`, else 50.
    #[serde(default)]
    initial_subject_x_percent: Option<u8>,
    /// Vertical center of the subject template as a *layer* percentage,
    /// forward-mapped through the clip's layer transform to seed the search on
    /// the composite. Defaults to the stored `focus_y_basis_points`, else
    /// `focus_y_percent`, else 50.
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
    /// Search radius around the prior subject center as a *composited-frame*
    /// percentage, not a layer percentage. Defaults to 10.
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
    /// CC6 §4.1: the delivery encode depth to report conformance for. `eight`
    /// (default) or `ten`; a report is bound to the lane it was produced for.
    #[serde(default)]
    delivery_bit_depth: DeliveryEncodeDepth,
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
    /// CC6 §6.5: decode the finished encode and compare it against a freshly
    /// rendered delivery reference, recording tags, luma and RGB differences,
    /// PSNR, and decoded legality on the job record. Defaults to true. A
    /// verification is a measurement: it never fails the job and never moves,
    /// renames, or deletes the output, so it needs no confirmation gate.
    #[serde(default = "crate::schema::default_true")]
    verify: bool,
    /// CC6 §4.1: `eight` (default) or `ten`. Selects the encoder pixel format
    /// and the declared delivery bit depth without editing the project's own
    /// colour contract.
    #[serde(default)]
    delivery_bit_depth: DeliveryEncodeDepth,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportJobArgs {
    job_id: ExportJobId,
}

const fn default_delivery_focus() -> u8 {
    50
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
struct RelinkMediaArgs {
    /// Exact revision returned by `get_media_status` or `get_timeline_state`.
    expected_revision: TimelineRevision,
    /// Stable asset id whose path should be replaced.
    asset_id: AssetId,
    /// Replacement path on the user's machine. The media layer probes and hashes it before Core sees it.
    path: PathBuf,
    /// Required for legacy assets whose persisted source fingerprint is unknown.
    #[serde(default)]
    allow_unverified_source: bool,
}

/// Arguments for CC4 §8 `import_lut_asset`, the only mutating media action
/// CC4 adds.
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportLutAssetArgs {
    /// Exact revision returned by `get_timeline_state` or `list_look_assets`.
    expected_revision: TimelineRevision,
    /// Path to a 3D `.cube` file on the user's machine. The media layer
    /// parses, hashes, and copies it into the project store before Core sees
    /// the record; the path itself is never opened by the renderer.
    path: PathBuf,
    /// Optional display title. Defaults to the file's `TITLE` keyword, or its
    /// file stem when the file declares none.
    #[serde(default)]
    title: Option<String>,
}

/// Arguments for CC4 §9 `convert_legacy_look`, the only agent path from a
/// legacy compatibility stage to a managed `creative_look`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ConvertLegacyLookArgs {
    /// Exact revision returned by `get_timeline_state` or `get_color_context`.
    expected_revision: TimelineRevision,
    /// Clip carrying the legacy `look_lut` or `cube_lut`.
    clip_id: ClipId,
    /// The legacy effect id to convert in place. `get_color_context`'s
    /// `legacy_look_conversions` lists every convertible node.
    effect_id: EffectId,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClearMediaCacheArgs {
    /// One owned cache family. Generated proxies are represented but unsupported in M41.
    family: MediaCacheFamily,
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
struct SourceProgramEditArgs {
    /// Exact revision returned by `get_timeline_state` before planning.
    expected_revision: TimelineRevision,
    /// Stable source asset id shown by `get_timeline_state`, `search_media`, or `get_source_info`.
    #[serde(alias = "asset_id")]
    asset: AssetId,
    /// Optional source-monitor In mark in exact asset frames.
    #[serde(default)]
    source_in: Option<TimeCode>,
    /// Optional source-monitor Out mark in exact asset frames.
    #[serde(default)]
    source_out: Option<TimeCode>,
    /// Optional record/timeline In mark in exact project frames.
    #[serde(default)]
    timeline_in: Option<TimeCode>,
    /// Optional record/timeline Out mark in exact project frames.
    #[serde(default)]
    timeline_out: Option<TimeCode>,
    /// Insert opens time at the record point; overwrite replaces only selected destinations.
    mode: ThreePointMode,
    /// Explicit destination for the source video's component. Omit to disable video patching.
    #[serde(default)]
    video_track: Option<TrackId>,
    /// Explicit destination for the source audio component. Omit to disable audio patching.
    #[serde(default)]
    audio_track: Option<TrackId>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct CutNeighborhoodsArgs {
    /// Stable video track id shown by `get_timeline_state`.
    track_id: TrackId,
    /// Number of exact outgoing frames before each cut. Defaults to 1; valid range 1..=6.
    #[serde(default)]
    frames_before: Option<u8>,
    /// Number of exact incoming frames starting at each cut. Defaults to 3; valid range 1..=6.
    #[serde(default)]
    frames_after: Option<u8>,
    /// First contiguous media cut to inspect. Defaults to zero.
    #[serde(default)]
    cut_offset: Option<usize>,
    /// Maximum contiguous media cuts to inspect. Defaults to 12; valid range 1..=12.
    #[serde(default)]
    cut_count: Option<u8>,
    /// Largest allowed mean pixel change between adjacent incoming frames,
    /// in basis points of full RGB range. Defaults to 1200. A larger change
    /// within the first incoming frames is reported as a likely dirty handle
    /// or baked source cut, while the intentional outgoing-to-incoming cut is
    /// measured but never rejected by this threshold.
    #[serde(default)]
    maximum_secondary_change_basis_points: Option<u16>,
    /// Maximum width of each rendered cell. Defaults to 160 and is capped at 512.
    #[serde(default)]
    max_width: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ShotBoardCandidateSelection {
    /// Return a consecutive page of eligible candidates. This is the
    /// only mode that accepts an offset.
    Page,
    /// Sample eligible candidates across the full inspected range. For two or
    /// more returned candidates this always includes the first and last.
    #[default]
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
    /// Candidate selection strategy. `coverage` (the default) deterministically
    /// spreads up to `candidate_count` eligible candidates across the complete
    /// source range. `page` returns a consecutive offset page and is the only
    /// strategy that accepts `candidate_offset`.
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
            "get_color_context",
            "Return the project working, monitoring, and delivery colour descriptions, source metadata, managed-profile status, ordered CC1 stages, and legacy-stage warnings at the exact timeline revision. Each clip carries its ordered managed colour-node stack across all five node kinds - technical_lut, primary_correction, color_wheels, color_curves, creative_look - with role, color_stage, stage_index, bypass, active, inactive_reason, and resolved values; LUT nodes add lut_asset_id, lut_title, lut_sha256, lut_size, lut_kind, lut_provenance, lut_availability, lut_store_path, mix_basis_points, input_encoding, and may_be_active. legacy_look_conversions lists every legacy look_lut/cube_lut with status ready, needs_import, or unconvertible, the exact operations, and a recovery_action naming convert_legacy_look. The default status matches the executed application D65 profile assumption; use raw_only for unassumed classifier evidence. Metadata remains unchanged. Use render_color_proof for an isolated mapped BEFORE/AFTER frame.",
            schema_object::<ColorContextArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_primary_correction",
            format!(
                "Validate an exact integer CC1 primary-correction request against the current revision, visual clip type, and Core descriptor, then return unapplied AddEffect/SetEffectParam operations. A clip that already carries a primary_correction node is corrected in place instead of stacking a second node. This is evidence-only and never mutates the document. Controls: {}.",
                primary_parameter_summary()
            ),
            schema_object::<PrimaryCorrectionPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_color_wheels",
            format!(
                "Validate an exact integer CC3 color_wheels (ASC CDL slope/offset/power) request against the current revision, visual clip type, and Core descriptor, then return unapplied AddEffect/SetEffectParam operations. A clip that already carries a color_wheels node is corrected in place unless append=true stacks another. This is evidence-only and never mutates the document. Controls: {}.",
                color_wheels_parameter_summary()
            ),
            schema_object::<ColorWheelsPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_secondary_correction",
            format!(
                "Validate a CC5 secondary request - up to four geometric windows, an HSL qualifier, combine, invert, and mix - against the current revision, visual clip type, and Core descriptor, then return the exact unapplied matte_* operations plus predicted_coverage measured on a scratch document. Target one stored node with target_effect_id, or a kind with node_kind; an existing node of that kind is matted in place unless append=true. technical_lut carries no matte (CC5 §2.1). Optional sample_roi returns measured hue/saturation/luma evidence, and derive_qualifier_from_sample proposes a qualifier from it by the pinned formula. This is evidence-only and never mutates the document. {}",
                matte_legend_reference()
            ),
            schema_object::<SecondaryCorrectionPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_color_curves",
            format!(
                "Validate a CC3 color_curves request against the current revision, visual clip type, and Core descriptor, then return unapplied AddEffect/SetEffectParam operations. A clip that already carries a color_curves node is edited in place unless append=true stacks another, and curves omitted from the request keep their current points. This is evidence-only and never mutates the document. {}",
                color_curves_request_summary()
            ),
            schema_object::<ColorCurvesPlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_technical_lut",
            format!(
                "Validate a CC4 technical_lut (input transform) request against the current revision, visual clip type, Core descriptor, and the project LUT asset table, then return one unapplied InsertEffect at a computed insert_index, or SetEffectParam operations when the clip already carries a technical_lut. The index is the first position satisfying the CC4 stage order (technical, then correction, then creative), so an ordering rejection is unreachable. Evidence-only: nothing is applied. Controls: {}.",
                lut_node_parameter_summary(ColorNodeKind::TechnicalLut)
            ),
            schema_object::<LutNodePlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_creative_look",
            format!(
                "Validate a CC4 creative_look request against the current revision, visual clip type, Core descriptor, and the project LUT asset table, then return one unapplied InsertEffect at a computed insert_index, or SetEffectParam operations when the clip already carries a creative_look (append=true stacks another). The index is the first position satisfying the CC4 stage order, so an ordering rejection is unreachable. Evidence-only: nothing is applied. Controls: {}.",
                lut_node_parameter_summary(ColorNodeKind::CreativeLook)
            ),
            schema_object::<LutNodePlanArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "list_look_assets",
            "Return the timeline revision, the built-in generated look catalogue (name, title, size, pinned sha256), and every project LUT asset with its id, title, sha256, kind, size, byte length, provenance, live availability, expected store path, and the clip/effect ids referencing it. Compact: no samples. Availability is runtime state and is never persisted; it reports unknown_no_store until the project is saved.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "import_lut_asset",
            "Parse, hash, and copy one 3D .cube file into this project's LUT store, then register it with one undoable AddLutAsset operation at the expected revision. This is the only path that can create a LUT asset record. It asks for confirmation before reading or writing any byte, so a refusal leaves no store file and no document change. Requires a saved project; otherwise it returns project_not_saved.",
            schema_object::<ImportLutAssetArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(true)
                .open_world(true),
        ),
        Tool::new(
            "convert_legacy_look",
            "Replace one legacy look_lut or cube_lut with an equivalent managed creative_look at its exact vector position, at the expected revision, as one journaled and undoable batch. A look_lut resolves its preset_token to the built-in generated asset (0 identity, 1 warm, 2 cool, 3 monochrome, 4 bleach_bypass) and registers it, reusing an already registered record with the same content hash; a cube_lut's external path is imported into the project LUT store first, which asks for confirmation before reading or writing any byte and needs a saved project. This is the only path that can submit the [AddLutAsset, ConvertLegacyLook] batch: AddLutAsset is refused on every plan path. The result is deliberately not bit-identical to the legacy stage, which clamped to [0, 1] in display space and mixed intensity in the encoded domain, so conversion is never automatic. Call get_color_context first: legacy_look_conversions lists every convertible node with its status and recovery_action.",
            schema_object::<ConvertLegacyLookArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        ),
        Tool::new(
            "render_color_proof",
            "Render an isolated managed-compositor CC1 BEFORE/AFTER proof at one exact project frame. The revision-bound integer primary correction is evidence-only: the live document is never mutated, and the response includes the mapped PNG cells, exact unapplied operations, resolved parameters, source/profile metadata, and objective deltas. Send effect_id (with no parameters) to proof a stored managed colour node instead: the BEFORE cell is the same composite with that node removed, and look_comparison selects before, after, or bypass for the AFTER cell.",
            schema_object::<RenderColorProofArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_media_status",
            "Return the exact timeline revision, every asset path and persisted fingerprint, current filesystem availability, derived-analysis lifecycle, and the honest in-memory preview contract. Availability is dynamic and is never persisted into the project.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_cache_status",
            "Return typed inventory for every owned media-cache family, including disk roots, file counts, bytes, repopulation notes, and the explicitly unsupported generated-proxy family.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "clear_media_cache",
            "Clear exactly one owned media-cache family as a non-Core side effect. This never deletes project state or source media; unsupported generated proxies return a typed unsupported result.",
            schema_object::<ClearMediaCacheArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            "relink_media",
            "Probe and hash one replacement path, require exact kind/frame-rate/duration/resolution compatibility, then apply one undoable RelinkAsset operation at the expected revision. Known fingerprints must match exactly; legacy unknown fingerprints require explicit allow_unverified_source.",
            schema_object::<RelinkMediaArgs>(),
        )
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
        ),
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
            "Build a source-feasible hard-cut montage timed to one analyzed music asset. The model owns every shot choice and the final order; the planner only selects beat boundaries that satisfy the supplied source envelopes and duration constraints, with no hidden transition, retime, or semantic replacement. Explicit anchors remain exact unless anchor_repair opts into a non-negative maximum movement bound and optional locked indices; requested, resolved, and per-anchor movement evidence is returned for review. When a bounded repair is infeasible but the same selects and cadence have a global solution, the structured error returns the nearest valid schedule and an exact retry patch instead of forcing repeated guesses. An optional cadence contract validates rounded shot-duration buckets and similar runs before the prepared plan is stored. Returns an inspectable plan and opaque prepared_edit_plan preview without mutating the timeline.",
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
            "Materialize one delivery profile from the current branch snapshot and run structural QA against the exact document and export settings that would render. delivery_bit_depth selects the eight- or ten-bit delivery lane; the report is bound to the lane it was produced for.",
            schema_object::<DeliveryConformanceArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "queue_export",
            "Queue a serial export of an immutable revision-gated branch snapshot using one stable delivery profile at the eight- or ten-bit delivery lane. New files require no confirmation; overwrite=true always enters the human confirmation broker and source media can never be targeted. verify (default true) decodes the finished encode and records its tags, luma and RGB differences, PSNR, and decoded legality on the job record; that measurement never fails the job and never moves, renames, or deletes the output.",
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
            "Return every retained export job in enqueue order with immutable request, delivery lane, conformance, progress, terminal state, error data, and - for a verified job - the decoded post-export verification or the reason one could not run.",
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
            "Inspect one asset as source media with optional exact source-frame in/out marks. Returns the evidence snapshot's timeline_revision, typed compatible video/audio destinations with track ids and sync-lock state, technical metadata, cached transcript words, speaker labels, scene boundaries, beats, and analysis lifecycle for that range.",
            schema_object::<SourceInfoArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_source_program_edit",
            "Prepare one revision-safe compound source/program edit from exactly three of source_in, source_out, timeline_in, and timeline_out. Destinations are explicit optional video_track and audio_track ids; Core derives the missing boundary once, inserts or overwrites atomically, and links a dual-route A/V pair. Returns resolved ranges, destinations, and an opaque prepared_edit_plan without changing the live timeline.",
            schema_object::<SourceProgramEditArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_source_storyboard",
            "Render a bounded PNG contact sheet directly from one video asset's source frames. The manifest includes the evidence snapshot's timeline_revision and maps every cell to its exact source frame, asset id, and requested half-open source range without changing the timeline.",
            schema_object::<SourceStoryboardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_source_shot_board",
            "Return scene-derived source-shot candidates for one video asset and render start/middle/end evidence cells for each. The manifest includes the evidence snapshot's timeline_revision. By default candidates are sampled across the complete requested source range so one bounded call exposes the whole asset; use candidate_selection=page only for consecutive pagination. An optional inclusive minimum_duration_frames filters short candidates while preserving original candidate ids and indexes. minimum_confidence_basis_points controls scene-boundary sensitivity (0..=10000, default 1000); raise it for motion-heavy footage when weak differences over-segment a continuous shot. The manifest reports selection strategy, selected positions, requested, filtered, returned, and total counts. Candidate ids, exact half-open source ranges, and scene-boundary confidence provenance are stable; this inspector never changes the timeline.",
            schema_object::<SourceShotBoardArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_cut_neighborhoods",
            "Render exact outgoing and incoming frames around every selected contiguous media cut on one video track. Use this after editing to catch one-frame flashes, dirty source handles, baked cuts immediately after an in-point, and near-match hard cuts that read as a stutter. It measures adjacent-frame RGB change, marks likely secondary cuts inside the incoming handle, and returns an explicit clean boolean plus issues; the intentional hard cut itself is measured but never rejected. The manifest maps every cell to its exact project frame and clip boundary; this inspector never changes the timeline.",
            schema_object::<CutNeighborhoodsArgs>(),
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
            "get_video_scopes_v2",
            "Measure bounded CC2 waveform, RGB parade, vectorscope, histogram, clipping, and gamut evidence at an explicit named monitoring stage. Full-resolution managed monitor proofs are the default; proxy sampling requires an explicit opt-in and is labeled in provenance. Requests support a half-open range with a positive step or explicit project frames plus a normalized geometric ROI, and fail closed on unsupported stages, unavailable media, invalid bounds, stale revisions, or excessive samples.",
            schema_object::<VideoScopesV2Args>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "analyze_color_shot",
            "Return evidence-only CC2 diagnosis for one explicit visual shot: bounded full-resolution-aware scopes, ROI/temporal provenance, signed measurements, confidence, and assumptions. This call never mutates the timeline.",
            schema_object::<AnalyzeColorShotArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_color_qc",
            COLOR_QC_DESCRIPTION,
            schema_object::<ColorQcArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "plan_shot_match",
            "Compare one explicit reference shot with one or more candidate shots and return signed deltas, retained reference evidence, confidence/assumptions, and exact revision-gated primary_correction operations for each candidate. The proposal is evidence-only; review the visible operations and submit the desired subset through prepare_edit_plan.",
            schema_object::<PlanShotMatchArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "track_mask_region",
            "Track an existing bounded mask region through one media clip using deterministic sequential template matching on isolated compositor frames. Returns confidence observations plus revision-gated SetEffectKeyframes operations for the mask center; it never silently mutates the timeline. The tracking template is the stored region rescaled onto the composite, so mask width_percent x layer scale and height_percent x layer scale must each be in 1..=75 at every sampled frame.",
            schema_object::<TrackMaskArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "inspect_grade_matte",
            "Measure one colour node's matte coverage at one exact project frame: covered/full/partial pixel counts, covered_basis_points, a 16-bucket coverage histogram, the tightest bounding box, the coverage-weighted centroid, the resolved 47 matte integers, active/inactive_reason, full renderer provenance, and a PNG of the coverage itself. This is the Matte (this correction), not the Mask (layer alpha) - the two never interact. Read-only; it mutates nothing. A node that is inactive, carries no matte, or that this build cannot proof fails typed rather than returning a blank frame.",
            schema_object::<InspectGradeMatteArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "track_matte_window",
            format!(
                "Track one matte window through a media clip by deterministic sequential template matching on composited thumbnails that exclude the tracked node itself, then return raw observations, the M40-smoothed curves, and a revision-gated prepared edit plan of two SetEffectKeyframes on the window centre. It commits nothing. Smoothing is pinned: three-sample median filter, dead zone {MATTE_TRACK_DEAD_ZONE_BASIS_POINTS}, maximum step {MATTE_TRACK_MAX_STEP_BASIS_POINTS} basis points, Linear interpolation. Boundary: {MATTE_TRACKING_BOUNDARY}",
            ),
            schema_object::<TrackMatteWindowArgs>(),
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

fn error_structured(text: impl Into<String>, value: serde_json::Value) -> CallToolResult {
    let mut result = error_text(text);
    result.structured_content = Some(value);
    result
}

/// One typed CC4 LUT rejection in the CC1/CC2 `field`/`observed`/`allowed`/
/// `recovery_action` shape.
fn lut_tool_error(
    tool: &str,
    code: &str,
    message: &str,
    details: &serde_json::Value,
) -> CallToolResult {
    error_structured(
        format!("{tool} rejected: {message}"),
        serde_json::json!({
            "code": code,
            "message": message,
            "details": details,
            "applied": false,
        }),
    )
}

/// [`lut_tool_error`] bound to `import_lut_asset`.
fn lut_import_error(code: &str, message: &str, details: &serde_json::Value) -> CallToolResult {
    lut_tool_error("import_lut_asset", code, message, details)
}

/// A revision conflict on a CC4 LUT tool, in the same structured shape as
/// every other rejection those tools return (CC4 §8).
fn lut_revision_conflict(
    tool: &str,
    expected: TimelineRevision,
    actual: TimelineRevision,
) -> CallToolResult {
    lut_tool_error(
        tool,
        "revision_conflict",
        &format!("timeline revision conflict: expected {expected}, actual {actual}"),
        &serde_json::json!({
            "field": "expected_revision",
            "observed": expected.0,
            "allowed": actual.0,
            "recovery_action": "Call get_timeline_state, then resend at the current timeline_revision.",
            "expected_revision": expected.0,
            "actual_revision": actual.0,
        }),
    )
}

/// The trailing keys the two media LUT formatters append, in emission order.
///
/// `LutStoreError` renders `"<code>: <detail>; observed=<v>; allowed=<v>"` and
/// `LutParseError` renders `"<code>: observed=<v>; allowed=<v>; line=<n>"` (the
/// parser also tolerates the older space-separated spelling), so
/// a field is recognised only at a field boundary and only when followed by
/// `=` or a space.
const LUT_ERROR_FIELD_KEYS: [&str; 3] = ["observed", "allowed", "line"];

/// The byte offset of `key` where it actually starts a field, or `None`.
///
/// A field starts at the beginning of the rendered remainder or immediately
/// after a `"; "` delimiter, and is always followed by `=` or a space. Bare
/// substring matching is wrong on both counts: `"line"` occurs inside a path
/// component such as `baseline`, and a value may itself contain `"; "`.
///
/// `allow_start_anchor` is what keeps a *value* from terminating itself. The
/// rendered remainder really can begin with a key — `LutParseError` leads with
/// `observed` — so offset 0 is a field boundary there. Inside an already
/// extracted value it is not: `observed=allowed=x` and
/// `observed line 1 2 3 4` both begin with another key's name, and anchoring
/// at 0 would cut them to the empty string.
fn lut_error_field_start(text: &str, key: &str, allow_start_anchor: bool) -> Option<usize> {
    let followed_by_value = |rest: &str| matches!(rest.as_bytes().first(), Some(b'=' | b' '));
    if allow_start_anchor
        && let Some(rest) = text.strip_prefix(key)
        && followed_by_value(rest)
    {
        return Some(0);
    }
    let mut search = 0;
    while let Some(offset) = text[search..].find("; ") {
        let start = search + offset + "; ".len();
        if let Some(rest) = text[start..].strip_prefix(key)
            && followed_by_value(rest)
        {
            return Some(start);
        }
        search = start;
    }
    None
}

/// The value of one anchored `; <key>=<value>` or `; <key> <value>` field
/// inside a rendered media LUT failure.
///
/// The value runs to the next *anchored* key, never to the first `"; "`, so a
/// filesystem path containing `"; "` survives intact.
fn lut_error_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let start = lut_error_field_start(text, key, true)?;
    let rest = &text[start + key.len()..];
    let value = rest.strip_prefix('=').or_else(|| rest.strip_prefix(' '))?;
    let end = LUT_ERROR_FIELD_KEYS
        .iter()
        .filter(|other| **other != key)
        // A value's own first byte is not a field boundary: only a `"; "`
        // delimiter inside it introduces the next field.
        .filter_map(|other| lut_error_field_start(value, other, false))
        // Back up over the `"; "` that introduced the next field.
        .map(|index| index.saturating_sub("; ".len()))
        .min()
        .unwrap_or(value.len());
    Some(&value[..end])
}

/// The leading detail sentence, cut at the first anchored trailing field.
fn lut_error_detail(remainder: &str) -> &str {
    let cut = LUT_ERROR_FIELD_KEYS
        .iter()
        .filter_map(|key| lut_error_field_start(remainder, key, true))
        .map(|index| index.saturating_sub("; ".len()))
        .min()
        .unwrap_or(remainder.len());
    // A parse failure leads with `observed`, so it has no detail sentence of
    // its own; quoting the whole remainder beats an empty message.
    if cut == 0 {
        return remainder;
    }
    remainder[..cut].trim_end_matches([';', ' '])
}

/// Turn one export-queue refusal into the structured result the agent reads.
///
/// Lifted out of `queue_export` so each CC4 rejection keeps its full typed
/// payload without the tool body growing past what one screen can hold.
fn export_queue_error_result(error: ExportQueueError) -> CallToolResult {
    match error {
        // CC4 §2.3: a blocked look is a typed, recoverable status naming
        // the asset, its recorded hash, the expected store path, and the
        // nodes that would have evaluated it — never a render-time failure.
        ExportQueueError::LutPreflight(report) => error_structured(
            report.summary(),
            serde_json::json!({
                "code": "lut_preflight_blocked",
                "message": report.summary(),
                "details": {
                    "field": "lut_assets",
                    "observed": report.issues,
                    "allowed": "every look a rendered frame could need hashes to its recorded sha256",
                    "recovery_action": "Call list_look_assets, then restore the store file or import a replacement and retarget the node before exporting.",
                    "checked_lut_assets": report.checked_lut_assets,
                },
                "applied": false,
            }),
        ),
        // CC4 §2.2: "there is no project path" and "the path is published but
        // its derived root is refused" are different failures with opposite
        // recoveries. Collapsing them would tell an operator who already saved
        // the project to save it again, which is a loop that cannot terminate.
        error @ ExportQueueError::LutStoreNotSaved => error_structured(
            format!("export blocked: {error}"),
            serde_json::json!({
                "code": "project_not_saved",
                "message": "this timeline carries a LUT node that could evaluate, but the project has no saved path, so its LUT store root cannot be derived (CC4 §2.2)",
                "details": {
                    "field": "project_path",
                    "observed": "project_not_saved",
                    "allowed": "a saved project file path such as <dir>/<stem>.kinewright",
                    "recovery_action": "Save the project first; the LUT store root is <dir>/<stem>.kinewright-assets and is derived from that path.",
                },
                "applied": false,
            }),
        ),
        ExportQueueError::LutStoreRootInvalid { reason } => error_structured(
            format!("export blocked: {reason}"),
            serde_json::json!({
                "code": "lut_store_root_invalid",
                "message": "this timeline carries a LUT node that could evaluate, and the project is saved, but the store root derived from its path is refused (CC4 §2.2)",
                "details": {
                    "field": "lut_store_root",
                    "observed": reason,
                    "allowed": "a writable <dir>/<stem>.kinewright-assets directory that is not a symbolic link",
                    "recovery_action": "Move the project to a directory where its <stem>.kinewright-assets store can be created, or remove the file or symlink occupying that path; the project is already saved, so saving it again cannot help.",
                },
                "applied": false,
            }),
        ),
        error => error_text(error.to_string()),
    }
}

/// Surface a media-layer LUT store or parser failure with its stable code.
///
/// `MediaError` has no LUT variant, so both the store and the `.cube` parser
/// encode their code as a `"<code>: "` prefix behind `MediaError::Backend`'s
/// own label, and the typed `LutStoreError`/`LutParseError` are not
/// recoverable from the `MediaError` the store's public API returns. The parts
/// are split back out here with anchored keys so an agent reads the same typed
/// `field`/`observed`/`allowed`/`recovery_action` shape every other CC1-CC4
/// rejection uses.
fn lut_store_error_result(tool: &str, error: &kinewright_core::MediaError) -> CallToolResult {
    let rendered = error.to_string();
    let payload = rendered
        .strip_prefix("media backend error: ")
        .unwrap_or(rendered.as_str());
    let (code, remainder) = payload
        .split_once(": ")
        .unwrap_or(("lut_import_failed", payload));
    lut_tool_error(
        tool,
        code,
        lut_error_detail(remainder),
        &serde_json::json!({
            "field": "path",
            "observed": lut_error_field(remainder, "observed"),
            "allowed": lut_error_field(remainder, "allowed"),
            "line": lut_error_field(remainder, "line"),
            "recovery_action": "Choose a 3D .cube file this build can parse, or repair the project LUT store root, then resend at the current timeline_revision.",
            "message": rendered,
        }),
    )
}

/// Ordered production effect manifest for one resolved visual layer.
///
/// `visual_layers_at` evaluates clip-local automation before handing effects
/// to the compositor, so the returned parameters are the values actually
/// rendered at that frame. The shared `color_status` helper owns the shape so
/// the proof manifest and `get_color_context` can never disagree.
fn proof_effect_manifest(effects: &[Effect]) -> Vec<serde_json::Value> {
    effect_chain_manifest(effects)
}

/// Keep the ordered CC3 colour-node stack aligned with `get_color_context`
/// while retaining the complete ordered effect chain above. This is
/// intentionally derived from the same frame-evaluated effects, never from the
/// raw clip vector, so bypass, activity, and resolved curve points describe the
/// frame that was actually rendered.
fn proof_color_node_manifest(
    effects: &[Effect],
    looks: &LookAssetContext,
) -> Vec<serde_json::Value> {
    color_node_manifest(effects, looks)
}

/// The complete internal capability registry, for crate-level contract tests.
#[cfg(test)]
pub(crate) fn capability_registry_tools() -> Vec<Tool> {
    KinewrightMcp::capability_tools().expect("the capability registry must build")
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

#[allow(clippy::needless_pass_by_value)]
fn color_proof_error_result(error: ColorProofError) -> CallToolResult {
    error_structured(
        format!("CC1 colour proof rejected: {error}"),
        serde_json::json!({
            "code": error.code(),
            "message": error.to_string(),
            "details": error.details(),
            "evidence_only": true,
            "applied": false,
        }),
    )
}

fn color_scope_error_result(tool: &str, error: &ScopeError) -> CallToolResult {
    error_structured(
        format!("{tool} rejected: {error}"),
        serde_json::json!({
            "code": error.code(),
            "message": error.to_string(),
            "details": error.details(),
            "evidence_only": true,
            "applied": false,
        }),
    )
}

fn color_proof_objective(
    before: &kinewright_core::RgbaImage,
    after: &kinewright_core::RgbaImage,
) -> Result<serde_json::Value, String> {
    let expected_len = usize::try_from(before.width)
        .ok()
        .and_then(|width| {
            usize::try_from(before.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "before raster dimensions overflowed".to_owned())?;
    if before.width != after.width || before.height != after.height {
        return Err(format!(
            "before raster is {}x{}, after raster is {}x{}",
            before.width, before.height, after.width, after.height
        ));
    }
    if before.pixels.len() != expected_len || after.pixels.len() != expected_len {
        return Err(format!(
            "RGBA8 raster length does not match {}x{} dimensions",
            before.width, before.height
        ));
    }
    let mut deltas = Vec::with_capacity(expected_len / 4 * 3);
    let mut before_clipped = 0_u128;
    let mut after_clipped = 0_u128;
    let mut channel_count = 0_u128;
    let mut delta_sum = 0_u128;
    for (before_pixel, after_pixel) in before
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.pixels.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            let before_channel = before_pixel[channel];
            let after_channel = after_pixel[channel];
            if before_channel == 0 || before_channel == u8::MAX {
                before_clipped = before_clipped.saturating_add(1);
            }
            if after_channel == 0 || after_channel == u8::MAX {
                after_clipped = after_clipped.saturating_add(1);
            }
            let delta = before_channel.abs_diff(after_channel);
            deltas.push(delta);
            delta_sum = delta_sum.saturating_add(u128::from(delta));
            channel_count = channel_count.saturating_add(1);
        }
    }
    if deltas.is_empty() {
        return Err("RGBA8 raster contains no RGB channels".to_owned());
    }
    deltas.sort_unstable();
    let p99_index = deltas
        .len()
        .saturating_mul(99)
        .div_ceil(100)
        .saturating_sub(1);
    let denominator = channel_count.saturating_mul(u128::from(u8::MAX));
    let mean_basis_points = delta_sum
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    let clipping_basis_points = |count: u128| {
        let rounded = count
            .saturating_mul(10_000)
            .saturating_add(channel_count / 2)
            / channel_count;
        u16::try_from(rounded).unwrap_or(u16::MAX).min(10_000)
    };
    let mean_milli_code_values = delta_sum
        .saturating_mul(1_000)
        .saturating_add(channel_count / 2)
        / channel_count;
    Ok(serde_json::json!({
        "max_channel_delta_code_values": deltas.last().copied().unwrap_or_default(),
        "p99_channel_delta_code_values": deltas[p99_index],
        "mean_channel_delta_milli_code_values": u32::try_from(mean_milli_code_values)
            .unwrap_or(u32::MAX),
        "mean_normalized_delta_basis_points": u16::try_from(mean_basis_points)
            .unwrap_or(u16::MAX)
            .min(10_000),
        "clipping_basis_points": {
            "before": clipping_basis_points(before_clipped),
            "after": clipping_basis_points(after_clipped),
            "definition": "RGB channels equal to final RGBA8 code 0 or 255",
        },
    }))
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
    /// CC5 §5.2: exactly one effect id, not every effect sharing a name.
    /// This narrows the exclusion from *every effect with that name* to the one
    /// node being tracked, so a clip carrying two masks keeps the second mask's
    /// alpha in the tracking thumbnails — which is the correct behaviour.
    excluded_effect: EffectId,
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

// ---------------------------------------------------------------------------
// CC5 §5.2 — matte window tracking
// ---------------------------------------------------------------------------

/// A dead zone deliberately lags. That is right for a virtual camera and wrong
/// for a matte, which must stay on the subject (CC5 §5.2).
pub(crate) const MATTE_TRACK_DEAD_ZONE_BASIS_POINTS: i64 = 0;

/// 8 % of the frame between samples; at the default 5-frame step, 1.6 % per
/// frame — well above ordinary subject motion, while still rejecting a tracker
/// jump to a false match across the frame (CC5 §5.2).
pub(crate) const MATTE_TRACK_MAX_STEP_BASIS_POINTS: i64 = 800;

/// Default confidence floor below which a tracked sample is dropped.
const DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS: i64 = 5_000;

/// Fewer surviving samples than this cannot describe motion, so the tool fails
/// typed rather than emitting a one-point curve (CC5 §5.2).
const MATTE_TRACK_MINIMUM_SAMPLES: usize = 2;

/// CC5 §5.2's provenance marker, stated so the tool's reach is not inferred.
const MATTE_TRACKING_BOUNDARY: &str = "tracks the explicitly supplied window rectangle by normalized SAD template match on composited thumbnails; no learned object, face, or skin detection, no scale or rotation estimation, and no occlusion handling. rotation_centidegrees, half_width_basis_points, and half_height_basis_points are never written.";

/// One layer's resolved geometric transform over a tracked range (CC5 §5.2).
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerTransform {
    /// Product of every `scale_percent / 100` on the layer.
    scale: f64,
    /// Sum of every `x_percent / 50`, in the compositor's own units.
    offset_x: f64,
    /// Sum of every `y_percent / 50`, in the compositor's own units.
    offset_y: f64,
}

impl LayerTransform {
    /// The identity layer: no scale, no offset.
    const IDENTITY: Self = Self {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };

    /// Map a layer uv to the composited frame's uv (CC5 §5.2).
    ///
    /// Derived from `compositor.wgsl`'s vertex stage, which places the layer
    /// quad at NDC `p = q·scale + (offset_x, −offset_y)` and hands the
    /// fragment stage `uv.y = (1 − ndc.y)/2`. The two sign flips on y — the
    /// shader's own negation and the flip built into the uv convention —
    /// cancel exactly, so **both** axes carry `+offset/2`:
    ///
    /// `u_composite = scale·(u_layer − 0.5) + (offset_x, offset_y)/2 + 0.5`
    ///
    /// `offset_x`/`offset_y` are the compositor's own accumulated units, i.e.
    /// `sum(percent) / 50`, exactly as `EffectUniform::OffsetX`/`OffsetY` are
    /// accumulated by the compositor.
    fn layer_to_composite(self, layer: [f64; 2]) -> [f64; 2] {
        [
            (layer[0] - 0.5).mul_add(self.scale, self.offset_x / 2.0) + 0.5,
            (layer[1] - 0.5).mul_add(self.scale, self.offset_y / 2.0) + 0.5,
        ]
    }

    /// CC5 §5.2's normative composite → layer conversion, in normalized uv.
    ///
    /// The exact inverse of [`Self::layer_to_composite`], **unclamped**:
    ///
    /// `u_layer = (u_composite − 0.5)/scale − (offset_x, offset_y)/(2·scale) + 0.5`
    ///
    /// No clamp, deliberately: a layer scaled below 1 occupies only part of the
    /// composite, so composite coordinates outside the layer's own quad map to
    /// layer coordinates outside `0..=1`, and every caller decides for itself
    /// what to do with them. A degenerate `scale <= 0` collapses the quad and
    /// has no inverse, so the composite coordinate is returned unchanged rather
    /// than divided by zero.
    fn composite_to_layer_unit(self, unit: [f64; 2]) -> [f64; 2] {
        let convert = |value: f64, offset: f64| {
            if self.scale <= 0.0 {
                return value;
            }
            (value - 0.5 - offset / 2.0) / self.scale + 0.5
        };
        [
            convert(unit[0], self.offset_x),
            convert(unit[1], self.offset_y),
        ]
    }

    /// [`Self::composite_to_layer_unit`] in basis points, clamped to CC5
    /// §2.2's matte window centre range.
    fn composite_to_layer_basis_points(self, composite: [i64; 2]) -> [i64; 2] {
        #[allow(clippy::cast_precision_loss)]
        let unit = self.composite_to_layer_unit([
            composite[0] as f64 / 10_000.0,
            composite[1] as f64 / 10_000.0,
        ]);
        unit.map(|layer| {
            #[allow(clippy::cast_possible_truncation)]
            let basis_points = (layer * 10_000.0).round() as i64;
            basis_points.clamp(
                kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            )
        })
    }
}

/// A layer-transform parameter that moves across the tracked range.
struct LayerTransformUnsupported {
    field: &'static str,
    observed: serde_json::Value,
}

/// Resolve one layer's scale and offset at exactly one frame.
///
/// The accumulation is the compositor's own, restated: `params_for` multiplies
/// every `EffectUniform::Scale` by `value / 100` and adds every
/// `EffectUniform::OffsetX` / `OffsetY` as `value / 50`, over the whole effect
/// chain in order, with a missing parameter taking the descriptor's neutral
/// value. Resolving per frame is what lets CC5 §5.2's composite → layer
/// conversion follow a *keyframed* transform: the map is affine at each
/// instant even when it moves between instants.
fn resolve_layer_transform_at(effects: &[Effect], frame: TimeCode) -> LayerTransform {
    let mut transform = LayerTransform::IDENTITY;
    for effect in effects {
        let Some(descriptor) = kinewright_core::effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            let value = effect
                .integer_parameter_at(parameter.name, frame)
                .unwrap_or(parameter.neutral);
            #[allow(clippy::cast_precision_loss)]
            let value = value as f64;
            match parameter.uniform {
                kinewright_core::EffectUniform::Scale => transform.scale *= value / 100.0,
                kinewright_core::EffectUniform::OffsetX => transform.offset_x += value / 50.0,
                kinewright_core::EffectUniform::OffsetY => transform.offset_y += value / 50.0,
                _ => {}
            }
        }
    }
    transform
}

/// Resolve one layer's static scale and offset across every sampled frame.
///
/// CC5 §5.2 requires the composite → layer conversion to be one affine map, so
/// a `scale` or `offset` whose resolved value differs between samples is a
/// typed refusal rather than a silently-wrong conversion. A keyframe curve that
/// happens to resolve to one constant value is accepted: the rule is about the
/// values the renderer uses, not about the presence of automation.
fn resolve_static_layer_transform(
    effects: &[Effect],
    sample_frames: &[TimeCode],
) -> Result<LayerTransform, LayerTransformUnsupported> {
    let mut resolved: Option<LayerTransform> = None;
    for frame in sample_frames {
        let transform = resolve_layer_transform_at(effects, *frame);
        match resolved {
            None => resolved = Some(transform),
            Some(first) => {
                for (field, first_value, value) in [
                    ("scale", first.scale, transform.scale),
                    ("offset_x", first.offset_x, transform.offset_x),
                    ("offset_y", first.offset_y, transform.offset_y),
                ] {
                    if (first_value - value).abs() > f64::EPSILON {
                        return Err(LayerTransformUnsupported {
                            field,
                            observed: serde_json::json!({
                                "parameter": field,
                                "at_first_sample": first_value,
                                "at_frame": frame.0,
                                "value_at_frame": value,
                            }),
                        });
                    }
                }
            }
        }
    }
    Ok(resolved.unwrap_or(LayerTransform::IDENTITY))
}

/// The clip's effect chain, borrowed for transform resolution.
fn effect_chain(clip: &Clip) -> &[Effect] {
    &clip.effects
}

/// One resolved layer scale and the sampled frame it was resolved at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerScaleAt {
    frame: TimeCode,
    scale: f64,
}

/// The smallest and largest layer scale resolved over `sample_frames`.
///
/// CC5 §5.2: the tracking template is sized *once*, at the seed frame's scale,
/// while the composite → layer conversion is redone per frame. A keyframed
/// scale therefore makes the *same* template legal at one end of the range and
/// illegal at the other, so the `1..=75` template gate is applied at both
/// extremes rather than at the seed alone. Returns `None` for an empty range.
fn layer_scale_extremes(
    effects: &[Effect],
    sample_frames: &[TimeCode],
) -> Option<(LayerScaleAt, LayerScaleAt)> {
    let mut minimum: Option<LayerScaleAt> = None;
    let mut maximum: Option<LayerScaleAt> = None;
    for frame in sample_frames {
        let resolved = LayerScaleAt {
            frame: *frame,
            scale: resolve_layer_transform_at(effects, *frame).scale,
        };
        if minimum.is_none_or(|current| resolved.scale < current.scale) {
            minimum = Some(resolved);
        }
        if maximum.is_none_or(|current| resolved.scale > current.scale) {
            maximum = Some(resolved);
        }
    }
    minimum.zip(maximum)
}

/// The first resolved scale at which a stored region is an illegal template.
///
/// The template gate is CC5 §5.2's `1..=75` percent of the composited frame.
/// [`tracked_box_percent`] is monotone in the scale, so a region that is legal
/// at both the smallest and the largest resolved scale is legal at every scale
/// between them. Returns the offending scale, the frame it was resolved at, and
/// the template that scale produces.
fn offending_template_scale(
    stored_percent: [i64; 2],
    extremes: (LayerScaleAt, LayerScaleAt),
) -> Option<(LayerScaleAt, [i64; 2])> {
    let (minimum, maximum) = extremes;
    [minimum, maximum].into_iter().find_map(|resolved| {
        let template = [
            tracked_box_percent(stored_percent[0], resolved.scale),
            tracked_box_percent(stored_percent[1], resolved.scale),
        ];
        template
            .iter()
            .any(|value| !(1..=75).contains(value))
            .then_some((resolved, template))
    })
}

/// CC5 §5.2's exact statement of what the per-frame transform does and does not
/// cover, for the `coordinate_space` block of both region trackers.
///
/// The *conversion* is redone at every sampled frame; the *template* is sized
/// once, at the seed frame's scale, and is gated against the whole resolved
/// range. Stating the seed scale and both extremes keeps the claim falsifiable.
fn keyframed_transform_note(
    seed_scale: f64,
    extremes: Option<(LayerScaleAt, LayerScaleAt)>,
) -> String {
    let range = extremes.map_or_else(
        || "no sampled frames".to_owned(),
        |(minimum, maximum)| {
            format!(
                "{} at clip-local frame {} to {} at clip-local frame {}",
                minimum.scale, minimum.frame, maximum.scale, maximum.frame
            )
        },
    );
    format!(
        "the composite-to-layer conversion is resolved at every sampled frame, so a keyframed scale or offset is converted sample by sample rather than refused; the tracking template itself is sized once, at the seed frame's scale {seed_scale}, and the 1..=75 percent template gate is applied across the resolved scale range {range}"
    )
}

/// A seed centre whose forward map lands outside the composited frame.
struct TrackingSeedOutsideComposite {
    layer: [f64; 2],
    composite: [f64; 2],
}

/// Push one layer-space seed centre forward onto the composite (CC5 §5.2).
///
/// The tracker searches the composited thumbnail, so a seed that maps outside
/// `0..=1` names no pixel at all. Clamping it to the raster edge would silently
/// track whatever happens to sit in the corner, so the caller refuses instead.
fn composite_seed_percent(
    transform: LayerTransform,
    layer: [f64; 2],
) -> Result<[u8; 2], TrackingSeedOutsideComposite> {
    let composite = transform.layer_to_composite(layer);
    if composite.iter().any(|unit| !(0.0..=1.0).contains(unit)) {
        return Err(TrackingSeedOutsideComposite { layer, composite });
    }
    Ok([unit_to_percent(composite[0]), unit_to_percent(composite[1])])
}

/// CC5 §5.2's typed refusal for a seed that leaves the composited frame.
///
/// `axis_fields` names the two *caller-editable parameters* the seed came from,
/// horizontal first. The published `field` is the one whose axis actually left
/// `0..=1`, or both of them when both did, so an agent can repair the exact
/// input rather than being handed a generic selector. `extra_observed` carries
/// any caller-specific context — `track_matte_window`'s `window_index`, say —
/// into `observed`, where it belongs now that it no longer names the field.
fn tracking_seed_outside_composite_result(
    axis_fields: [&str; 2],
    clip: ClipId,
    frame: TimeCode,
    transform: LayerTransform,
    seed: &TrackingSeedOutsideComposite,
    extra_observed: &[(&str, serde_json::Value)],
) -> CallToolResult {
    let outside = [
        !(0.0..=1.0).contains(&seed.composite[0]),
        !(0.0..=1.0).contains(&seed.composite[1]),
    ];
    let field = match outside {
        [true, true] => serde_json::json!([axis_fields[0], axis_fields[1]]),
        [false, true] => serde_json::json!(axis_fields[1]),
        // A refusal is only raised when at least one axis is outside, so the
        // remaining arms both name the horizontal parameter.
        _ => serde_json::json!(axis_fields[0]),
    };
    let mut observed = serde_json::json!({
        "layer_center_unit": seed.layer,
        "composite_center_unit": seed.composite,
        "scale": transform.scale,
        "offset_x": transform.offset_x,
        "offset_y": transform.offset_y,
        "clip_local_frame": frame.0,
    });
    if let Some(map) = observed.as_object_mut() {
        for (name, value) in extra_observed {
            map.insert((*name).to_owned(), value.clone());
        }
    }
    matte_error_result(
        "tracking_seed_outside_composite",
        &format!(
            "clip {clip}'s layer transform at clip-local frame {frame} places the tracking seed at composite ({:.4}, {:.4}), outside the composited frame",
            seed.composite[0], seed.composite[1],
        ),
        &serde_json::json!({
            "field": field,
            "observed": observed,
            "allowed": "a seed whose forward-mapped composite centre lies in 0..=1 on both axes",
            "recovery_action": "Move the layer back inside the frame over the tracked range, or seed the tracker on a point that is actually visible; the tracker matches composited pixels and a seed off the raster names none (CC5 §5.2).",
            "clip_id": clip.0,
        }),
    )
}

/// CC5 §5.2's tracker-pixel to matte-basis-point conversion.
///
/// The tracker's own `pixel_to_basis_points` divides by `extent − 1`, because
/// it names a *sample position* on a lattice. A matte centre names a *fraction
/// of the extent*, so it divides by `extent` and adds the half-pixel that puts
/// the sample at the pixel centre. The two are deliberately different functions
/// and must not be interchanged; §9.2.11 records the ≤ 17 bp divergence.
fn matte_track_centre_basis_points(pixel: u32, extent: u32) -> i64 {
    if extent == 0 {
        return 0;
    }
    // round((pixel + 0.5) * 10000 / extent), in exact integer arithmetic:
    // (2*pixel + 1) * 10000 / (2*extent), rounded half up by adding `extent`.
    let numerator = (u64::from(pixel).saturating_mul(2).saturating_add(1)).saturating_mul(10_000);
    let denominator = u64::from(extent).saturating_mul(2);
    i64::try_from(numerator.saturating_add(u64::from(extent)) / denominator).unwrap_or(10_000)
}

/// The tracking template width or height, as a whole frame percentage.
///
/// CC5 §5.2: the box is the window's bounding box *on the composite*, so a
/// half extent stored in layer basis points is doubled and rescaled by the
/// layer scale.
fn matte_track_box_percent(half_extent_basis_points: i64, scale: f64) -> i64 {
    #[allow(clippy::cast_precision_loss)]
    let half = half_extent_basis_points as f64 / 10_000.0;
    #[allow(clippy::cast_possible_truncation)]
    let percent = (2.0 * half * scale * 100.0).round() as i64;
    percent
}

/// CC5 §5.2's tracker pixel as a *fraction of the composited extent*.
///
/// The float twin of [`matte_track_centre_basis_points`]:
/// `u_composite = (pixel + 0.5) / extent`.
///
/// Deliberately **not** [`pixel_to_percent`]'s `extent − 1` lattice
/// denominator. `mask_center_x` and `focus_x_basis_points` are read by the
/// compositor as fractions of the extent (`value / 100`, `value / 10000`), not
/// as sample positions on a lattice, so the two conversions are different
/// functions and must not be interchanged.
fn tracker_pixel_to_composite_unit(pixel: u32, extent: u32) -> f64 {
    if extent == 0 {
        return 0.5;
    }
    (f64::from(pixel) + 0.5) / f64::from(extent)
}

/// One tracked composite pixel centre as a layer-space unit pair (CC5 §5.2).
fn tracked_centre_layer_unit(
    center: [u32; 2],
    width: u32,
    height: u32,
    transform: LayerTransform,
) -> [f64; 2] {
    transform.composite_to_layer_unit([
        tracker_pixel_to_composite_unit(center[0], width),
        tracker_pixel_to_composite_unit(center[1], height),
    ])
}

/// A unit-square coordinate as a whole-percent control value, clamped to
/// `0..=100`.
///
/// Used on layer units to produce the values the plan writes, and on composite
/// units to publish the tracker's raw reading as provenance in the same
/// convention, so the two are directly comparable.
fn layer_unit_to_percent(unit: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let percent = (unit * 100.0).round().clamp(0.0, 100.0) as i64;
    percent
}

/// A layer-space unit as a basis-point control value, clamped to `0..=10000`.
fn layer_unit_to_basis_points(unit: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let basis_points = (unit * 10_000.0).round().clamp(0.0, 10_000.0) as i64;
    basis_points
}

/// A tracked template extent, rescaled from layer percent onto the composite.
///
/// The mask region and the reframe subject both state a *full* width or height
/// percent in layer space, while the tracker matches on the composite, where
/// the layer scale has already been applied. Mirrors
/// [`matte_track_box_percent`], whose input is a half extent in basis points.
fn tracked_box_percent(full_percent: i64, scale: f64) -> i64 {
    #[allow(clippy::cast_precision_loss)]
    let percent = full_percent as f64 * scale;
    #[allow(clippy::cast_possible_truncation)]
    let rounded = percent.round() as i64;
    rounded
}

/// A `0..=10000` basis-point control as a `0.0..=1.0` fraction.
fn basis_points_to_unit(basis_points: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let value = basis_points as f64;
    value / 10_000.0
}

/// A normalized coordinate as the tracker's whole-percent seed, clamped.
fn unit_to_percent(unit: f64) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (unit * 100.0).round().clamp(0.0, 100.0) as u8;
    percent
}

/// The CC5 §4.2 matte parameters as one compact integer object.
fn matte_parameter_object(matte: &kinewright_core::MatteParams) -> serde_json::Value {
    serde_json::json!({
        "matte_enabled": matte.enabled,
        "matte_window_count": matte.window_count,
        "matte_combine_token": matte.combine_token,
        "matte_invert": matte.invert,
        "matte_mix_basis_points": matte.mix_bp,
        "matte_qualifier_enabled": matte.qualifier.enabled,
        "matte_hue_center_centidegrees": matte.qualifier.hue_center_cd,
        "matte_hue_width_centidegrees": matte.qualifier.hue_width_cd,
        "matte_hue_softness_centidegrees": matte.qualifier.hue_softness_cd,
        "matte_saturation_low_basis_points": matte.qualifier.sat_low_bp,
        "matte_saturation_high_basis_points": matte.qualifier.sat_high_bp,
        "matte_saturation_softness_basis_points": matte.qualifier.sat_softness_bp,
        "matte_luma_low_basis_points": matte.qualifier.luma_low_bp,
        "matte_luma_high_basis_points": matte.qualifier.luma_high_bp,
        "matte_luma_softness_basis_points": matte.qualifier.luma_softness_bp,
        // CC5 §2.2: stored windows past the count render nothing, so only the
        // active ones are published.
        "windows": matte
            .active_windows()
            .enumerate()
            .map(|(index, window)| serde_json::json!({
                "index": index,
                "shape_token": window.shape_token,
                "center_x_basis_points": window.center_x_bp,
                "center_y_basis_points": window.center_y_bp,
                "half_width_basis_points": window.half_width_bp,
                "half_height_basis_points": window.half_height_bp,
                "rotation_centidegrees": window.rotation_cd,
                "feather_basis_points": window.feather_bp,
                "invert": window.invert,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One typed CC5 refusal in the CC1/CC2 `field`/`observed`/`allowed` shape.
fn matte_error_result(code: &str, message: &str, details: &serde_json::Value) -> CallToolResult {
    error_structured(
        message.to_owned(),
        serde_json::json!({
            "code": code,
            "message": message,
            "details": details,
            "evidence_only": true,
            "applied": false,
        }),
    )
}

/// One tracked subject box, stated directly in layer uv (CC5 §5.2).
///
/// The reframe crop is a sub-rectangle of the *layer* texture, so the
/// containment constraint — and the provenance marker that records it — must be
/// stated in layer basis points. The box is built from the converted layer
/// centre and the **declared** layer subject size, never by converting the
/// composite template's own bounds: the template is sized once, at the seed
/// frame's scale, so converting it back through a *different* observation's
/// scale would inflate the box by `seed_scale / observation_scale`.
///
/// `subject_percent` is a full width/height in layer percent, so each half
/// extent is `percent · 50` basis points. Edges round **outward** — floor on
/// left/top, ceil on right/bottom — so the recorded box is never smaller than
/// the measured one and `eval.rs`'s zero-tolerance containment check stays
/// conservative. The result is clamped to `0..=10000` because the crop can only
/// sample layer uv `0..1`.
fn layer_subject_bounds(
    at: TimeCode,
    layer_centre: [f64; 2],
    subject_percent: [i64; 2],
) -> TrackedSubjectBounds {
    let edge = |centre: f64, percent: i64, upper: bool| -> u16 {
        #[allow(clippy::cast_precision_loss)]
        let half = percent as f64 * 50.0;
        // A basis point is the finest unit these parameters carry, so an edge
        // that is analytically integral is snapped onto the grid before the
        // outward rounding. Without it the last bits of the affine conversion
        // would inflate every exact box by a whole basis point on the ceil
        // side; 1e-6 bp is a thousand times the worst-case error at this
        // magnitude and a millionth of the finest real unit.
        let snap = |value: f64| {
            let nearest = value.round();
            if (value - nearest).abs() < 1e-6 {
                nearest
            } else {
                value
            }
        };
        let value = snap(centre * 10_000.0);
        let value = if upper {
            (value + half).ceil()
        } else {
            (value - half).floor()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamped = value.clamp(0.0, 10_000.0) as u16;
        clamped
    };
    TrackedSubjectBounds {
        at,
        left_basis_points: edge(layer_centre[0], subject_percent[0], false),
        right_basis_points: edge(layer_centre[0], subject_percent[0], true),
        top_basis_points: edge(layer_centre[1], subject_percent[1], false),
        bottom_basis_points: edge(layer_centre[1], subject_percent[1], true),
    }
}

/// The template's own bounds on the composited thumbnail, as **provenance**.
///
/// Nothing plans from these: the containment constraint is built by
/// [`layer_subject_bounds`] from the declared layer subject size. They are
/// published so a reader can see what the tracker actually matched.
///
/// The conversion is the same fraction-of-the-extent convention
/// [`tracker_pixel_to_composite_unit`] uses everywhere else in this path — the
/// matched pixel *covers* `[pixel, pixel + 1) / extent` — rounded outward, so
/// this tool never mixes the lattice (`extent − 1`) convention with the
/// fractional one.
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
        left_basis_points: composite_edge_basis_points(left, width, false),
        right_basis_points: composite_edge_basis_points(right, width, true),
        top_basis_points: composite_edge_basis_points(top, height, false),
        bottom_basis_points: composite_edge_basis_points(bottom, height, true),
    }
}

/// One thumbnail pixel edge as a fraction of the extent, rounded outward.
///
/// `upper` names the pixel's far edge, `(pixel + 1) / extent`, so a one-pixel
/// box is one pixel wide rather than zero.
fn composite_edge_basis_points(pixel: u32, extent: u32, upper: bool) -> u16 {
    let extent = u64::from(extent.max(1));
    let numerator = u64::from(pixel)
        .saturating_add(u64::from(upper))
        .saturating_mul(10_000);
    let value = if upper {
        numerator
            .saturating_add(extent.saturating_sub(1))
            .saturating_div(extent)
    } else {
        numerator.saturating_div(extent)
    };
    u16::try_from(value.min(10_000)).unwrap_or(10_000)
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

fn percent_to_pixel(percent: u8, extent: u32) -> u32 {
    u32::from(percent)
        .saturating_mul(extent.saturating_sub(1))
        .saturating_add(50)
        / 100
}

/// The *lattice* pixel-to-percent conversion, `pixel / (extent − 1)`.
///
/// No production path uses it any more: `track_mask_region` published its
/// composite provenance through it, which contradicted the
/// `u = (pixel + 0.5) / extent` map the same response declares, so it now goes
/// through [`tracker_pixel_to_composite_unit`] like every other CC5 §5.2 path.
/// Kept as the reference the §9.2.11 divergence test measures the two
/// denominators against, alongside [`pixel_to_basis_points`].
#[cfg(test)]
fn pixel_to_percent(pixel: u32, extent: u32) -> u8 {
    let denominator = extent.saturating_sub(1).max(1);
    let rounded = pixel.saturating_mul(100).saturating_add(denominator / 2) / denominator;
    u8::try_from(rounded.min(100)).unwrap_or(100)
}

/// The tracker's *lattice* pixel-to-basis-point conversion, `pixel / (extent −
/// 1)`.
///
/// No production path uses it any more: CC5 §5.2 requires every written control
/// to be a fraction of the extent, so `track_reframe_subject` now goes through
/// [`tracker_pixel_to_composite_unit`] like `track_matte_window` does. It is
/// kept as the reference the §9.2.11 divergence test measures the two
/// denominators against.
#[cfg(test)]
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

    for (pixel_index, pixel) in image.pixels.as_chunks::<4>().0.iter().enumerate() {
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
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
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

    /// CC5 §5.2's pixel → matte basis-point conversion, and the divergence
    /// from the tracker's own `extent − 1` denominator.
    ///
    /// The two are deliberately different functions: the tracker's names a
    /// *sample position* on a lattice, the matte's names a *fraction of the
    /// extent*. Asserting the divergence explicitly means a refactor cannot
    /// quietly swap them.
    #[test]
    fn matte_centre_conversion_uses_the_pixel_centre_over_the_full_extent() {
        // round((pixel + 0.5) * 10000 / extent), hand-computed.
        // extent 512: (0 + 0.5) * 10000 / 512 = 9.765625 -> 10
        assert_eq!(matte_track_centre_basis_points(0, 512), 10);
        // (255 + 0.5) * 10000 / 512 = 4990.234375 -> 4990
        assert_eq!(matte_track_centre_basis_points(255, 512), 4990);
        // (256 + 0.5) * 10000 / 512 = 5009.765625 -> 5010
        assert_eq!(matte_track_centre_basis_points(256, 512), 5010);
        // (511 + 0.5) * 10000 / 512 = 9990.234375 -> 9990
        assert_eq!(matte_track_centre_basis_points(511, 512), 9990);
        // extent 288: (144 + 0.5) * 10000 / 288 = 5017.361 -> 5017
        assert_eq!(matte_track_centre_basis_points(144, 288), 5017);

        // The tracker's own conversion divides by `extent - 1` and adds no
        // half pixel. The two agree in the middle and diverge most at the
        // edges, by 10 bp on a 512-wide thumbnail and 17 bp on a 288-tall one
        // — the divergence CC5 §9.2.11 records so a refactor cannot quietly
        // swap them.
        for (pixel, extent, expected_divergence) in [
            (0_u32, 512_u32, 10_i64),
            (511, 512, 10),
            (0, 288, 17),
            (287, 288, 17),
        ] {
            let matte = matte_track_centre_basis_points(pixel, extent);
            let lattice = i64::from(pixel_to_basis_points(pixel, extent));
            assert_eq!(
                (matte - lattice).abs(),
                expected_divergence,
                "pixel {pixel} of {extent}: matte {matte} vs lattice {lattice}"
            );
        }
        // In the middle of the raster the two coincide, which is why only the
        // edges bound the error.
        assert_eq!(
            matte_track_centre_basis_points(255, 512),
            i64::from(pixel_to_basis_points(255, 512))
        );
    }

    /// CC5 §5.2: `box_percent` is the window bounding box *on the composite*,
    /// so it is doubled and rescaled by the layer scale.
    #[test]
    fn matte_track_box_percent_rescales_the_window_by_the_layer_scale() {
        // hw = 2500 bp = 0.25 of the width; 2 * 0.25 * 1.0 * 100 = 50 percent.
        assert_eq!(matte_track_box_percent(2_500, 1.0), 50);
        // At scale 0.5 the same window covers half as much of the composite.
        assert_eq!(matte_track_box_percent(2_500, 0.5), 25);
        assert_eq!(matte_track_box_percent(1_300, 1.0), 26);
        assert_eq!(matte_track_box_percent(1_800, 0.5), 18);
    }

    /// CC5 §5.2's normative conversion, pinned against the *compositor*, not
    /// against its own inverse.
    ///
    /// `compositor.wgsl` places the layer quad at NDC
    /// `p = q·scale + (offset_x, −offset_y)` and the fragment stage reads
    /// `uv.y = (1 − ndc.y)/2`, so the shader's y negation and the uv flip
    /// cancel and *both* axes carry `+offset/2`:
    ///
    /// `u_composite = scale·(u_layer − 0.5) + offset/2 + 0.5`.
    ///
    /// A round-trip test cannot see a sign error that appears in both
    /// directions, so every case here is a hand-worked absolute value.
    #[test]
    fn layer_transform_offsets_move_the_window_the_way_the_compositor_does() {
        // At scale 1 an offset of 1.0 shifts the picture half a frame. The
        // composite point 10000 bp (the bottom edge) therefore came from the
        // layer's *centre*, 5000 bp — not from 15000 bp, which is what a
        // doubly-negated y produced.
        let vertical_shift = LayerTransform {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 1.0,
        };
        assert_eq!(
            vertical_shift.composite_to_layer_basis_points([5_000, 10_000]),
            [5_000, 5_000]
        );
        // Forward, the same fact: the layer centre lands on the bottom edge.
        let composite = vertical_shift.layer_to_composite([0.5, 0.5]);
        assert!((composite[0] - 0.5).abs() < 1e-12, "{composite:?}");
        assert!((composite[1] - 1.0).abs() < 1e-12, "{composite:?}");

        // The symmetric x case, which was already right and must stay right.
        let horizontal_shift = LayerTransform {
            scale: 1.0,
            offset_x: 1.0,
            offset_y: 0.0,
        };
        assert_eq!(
            horizontal_shift.composite_to_layer_basis_points([10_000, 5_000]),
            [5_000, 5_000]
        );
        let composite = horizontal_shift.layer_to_composite([0.5, 0.5]);
        assert!((composite[0] - 1.0).abs() < 1e-12, "{composite:?}");
        assert!((composite[1] - 0.5).abs() < 1e-12, "{composite:?}");

        // A negative y offset moves the picture the other way, by the same
        // half-frame: the layer centre lands on the top edge.
        let negative_y = LayerTransform {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: -1.0,
        };
        assert_eq!(
            negative_y.composite_to_layer_basis_points([5_000, 0]),
            [5_000, 5_000]
        );

        // Hand-worked from the forward formula at scale 0.5 with both offsets
        // non-zero: offsets (0.4, -0.2).
        //   u_c.x = 0.5·(0.25 − 0.5) + 0.4/2 + 0.5 = 0.575  -> 5750 bp
        //   u_c.y = 0.5·(0.75 − 0.5) − 0.2/2 + 0.5 = 0.525  -> 5250 bp
        let both = LayerTransform {
            scale: 0.5,
            offset_x: 0.4,
            offset_y: -0.2,
        };
        let composite = both.layer_to_composite([0.25, 0.75]);
        assert!((composite[0] - 0.575).abs() < 1e-12, "{composite:?}");
        assert!((composite[1] - 0.525).abs() < 1e-12, "{composite:?}");
        assert_eq!(
            both.composite_to_layer_basis_points([5_750, 5_250]),
            [2_500, 7_500]
        );

        // And the two directions still compose to the identity.
        for (scale, offset_x, offset_y) in [(1.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.5, 0.4, -0.2)] {
            let transform = LayerTransform {
                scale,
                offset_x,
                offset_y,
            };
            for layer in [[0.5, 0.5], [0.25, 0.75], [0.1, 0.9]] {
                let composite = transform.layer_to_composite(layer);
                #[allow(clippy::cast_possible_truncation)]
                let basis_points = [
                    (composite[0] * 10_000.0).round() as i64,
                    (composite[1] * 10_000.0).round() as i64,
                ];
                let back = transform.composite_to_layer_basis_points(basis_points);
                #[allow(clippy::cast_possible_truncation)]
                let expected = [
                    (layer[0] * 10_000.0).round() as i64,
                    (layer[1] * 10_000.0).round() as i64,
                ];
                // One basis point of rounding at each of the two conversions.
                assert!(
                    (back[0] - expected[0]).abs() <= 2 && (back[1] - expected[1]).abs() <= 2,
                    "scale {scale} offset ({offset_x}, {offset_y}): {layer:?} -> {composite:?} -> {back:?}, expected {expected:?}"
                );
            }
        }
    }

    /// CC5 §5.2, worked by hand at `scale = 0.5`.
    ///
    /// A layer centre of `(0.25, 0.75)` sits at composite
    /// `(0.25 − 0.5)·0.5 + 0.5 = 0.375` and `(0.75 − 0.5)·0.5 + 0.5 = 0.625`,
    /// i.e. 3750 and 6250 basis points. Converting back must recover 2500 and
    /// 7500 exactly.
    #[test]
    fn layer_transform_matches_the_hand_worked_half_scale_case() {
        let transform = LayerTransform {
            scale: 0.5,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        assert_eq!(
            transform.composite_to_layer_basis_points([3_750, 6_250]),
            [2_500, 7_500]
        );
        // An off-frame composite centre stays legal: CC5 §2.2's centre range
        // is deliberately wide so a tracked window may leave and re-enter.
        assert_eq!(
            transform.composite_to_layer_basis_points([0, 10_000]),
            [-5_000, 15_000]
        );
        // And the bounds clamp rather than wrapping.
        assert_eq!(
            transform.composite_to_layer_basis_points([-20_000, 30_000]),
            [
                kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS
            ]
        );
    }

    /// CC5 §5.2 / M40: the smoothing constants are pinned here, and the
    /// three-sample median filter's last-sample lag is reproduced by hand.
    #[test]
    fn matte_track_smoothing_uses_the_pinned_m40_constants_and_lags_the_last_sample() {
        // A dead zone deliberately lags, which is wrong for a matte.
        assert_eq!(MATTE_TRACK_DEAD_ZONE_BASIS_POINTS, 0);
        // 8 % of the frame between samples.
        assert_eq!(MATTE_TRACK_MAX_STEP_BASIS_POINTS, 800);

        // A steadily moving subject, 200 bp per sample.
        let observations = [1_000_i64, 1_200, 1_400, 1_600, 1_800];
        let smoothed = kinewright_core::stabilize_tracked_centres_basis_points(
            &observations,
            kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
            kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
            MATTE_TRACK_MAX_STEP_BASIS_POINTS,
        );
        assert_eq!(smoothed.len(), observations.len());
        // CC5 §5.2's stated systematic lag: the filter replaces the final
        // sample with median(o[n-3], o[n-2], o[n-1]) = median(1400, 1600,
        // 1800) = 1600, one inter-sample displacement behind the true 1800.
        assert!(
            smoothed[4] <= 1_600,
            "the last smoothed value must lag by one inter-sample displacement, was {}",
            smoothed[4]
        );
        assert_eq!(observations[4] - smoothed[4], 200);

        // One-sample noise is rejected, which was M40's first fix.
        let noisy = [5_000_i64, 5_000, 9_000, 5_000, 5_000];
        let filtered = kinewright_core::stabilize_tracked_centres_basis_points(
            &noisy,
            kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
            kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
            MATTE_TRACK_MAX_STEP_BASIS_POINTS,
        );
        assert!(
            filtered.iter().all(|value| *value == 5_000),
            "a single 9000 spike must not survive the median filter: {filtered:?}"
        );
    }

    /// CC5 §5.2: a layer whose scale or offset moves across the tracked range
    /// cannot be expressed as one affine map, so the tool refuses.
    #[test]
    fn static_layer_transform_refuses_a_keyframed_scale_or_offset() {
        let frames = [TimeCode(0), TimeCode(5), TimeCode(10)];

        // No transform effect at all is the identity.
        assert_eq!(
            resolve_static_layer_transform(&[], &frames).ok(),
            Some(LayerTransform::IDENTITY)
        );

        // A static transform resolves once and is accepted.
        let static_transform = [Effect {
            id: EffectId(2),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(50))]),
            keyframes: BTreeMap::new(),
        }];
        let resolved = resolve_static_layer_transform(&static_transform, &frames)
            .unwrap_or_else(|_| panic!("a static transform is one affine map"));
        assert!((resolved.scale - 0.5).abs() < f64::EPSILON);

        // A keyframe curve that resolves to one constant value is *also*
        // accepted: the rule is about the values the renderer uses, not about
        // the presence of automation.
        let mut constant = static_transform[0].clone();
        constant.keyframes.insert(
            "scale_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode(0),
                    value: 50,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );
        assert!(resolve_static_layer_transform(&[constant], &frames).is_ok());

        // A moving scale is refused, with the field and both observed values.
        let mut moving = static_transform[0].clone();
        moving.keyframes.insert(
            "scale_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 50,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 100,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let unsupported = resolve_static_layer_transform(&[moving], &frames)
            .expect_err("a moving scale cannot be one affine map");
        assert_eq!(unsupported.field, "scale");
        assert_eq!(unsupported.observed["parameter"], "scale");
        assert_eq!(unsupported.observed["at_first_sample"], 0.5);
        assert_eq!(unsupported.observed["at_frame"], 5);

        // A moving offset is refused the same way.
        let mut moving_offset = static_transform[0].clone();
        moving_offset.keyframes.insert(
            "x_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 0,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 20,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        assert_eq!(
            resolve_static_layer_transform(&[moving_offset], &frames)
                .err()
                .map(|unsupported| unsupported.field),
            Some("offset_x")
        );
    }

    /// CC5 §5.2, hand-worked: a tracked composite pixel becomes a *layer*
    /// control value through a fraction-of-extent read and the inverse of the
    /// compositor's placement, never through the tracker's `extent − 1`
    /// lattice.
    ///
    /// At `scale = 0.5` with `x_percent = y_percent = 20` the compositor
    /// accumulates `offset = 20 / 50 = 0.4` on both axes, so the forward map is
    /// `u_c = 0.5·(u_l − 0.5) + 0.2 + 0.5 = 0.5·u_l + 0.45` and its inverse is
    /// `u_l = 2·u_c − 0.9`.
    #[test]
    fn tracked_centre_converts_to_layer_space_as_a_fraction_of_the_extent() {
        let transform = LayerTransform {
            scale: 0.5,
            offset_x: 0.4,
            offset_y: 0.4,
        };

        // Pixel 160 of 320: u_c = 160.5 / 320 = 0.5015625,
        // u_l = 2 · 0.5015625 − 0.9 = 0.103125.
        let layer = tracked_centre_layer_unit([160, 125], 320, 180, transform);
        assert!(
            (layer[0] - 0.103_125).abs() < 1e-12,
            "x converted to {}",
            layer[0]
        );
        // Pixel 125 of 180: u_c = 125.5 / 180 = 0.697222…,
        // u_l = 2 · 0.697222… − 0.9 = 0.494444….
        assert!(
            (layer[1] - 0.494_444_444_444_444_4).abs() < 1e-12,
            "y converted to {}",
            layer[1]
        );
        // 10.3125 percent rounds to 10; 1031.25 bp rounds to 1031.
        assert_eq!(layer_unit_to_percent(layer[0]), 10);
        assert_eq!(layer_unit_to_basis_points(layer[0]), 1_031);
        // 49.4444 percent rounds to 49; 4944.44 bp rounds to 4944.
        assert_eq!(layer_unit_to_percent(layer[1]), 49);
        assert_eq!(layer_unit_to_basis_points(layer[1]), 4_944);

        // Pixel 224 of 320: u_c = 224.5 / 320 = 0.7015625, u_l = 0.503125.
        let layer = tracked_centre_layer_unit([224, 125], 320, 180, transform);
        assert_eq!(layer_unit_to_percent(layer[0]), 50);
        assert_eq!(layer_unit_to_basis_points(layer[0]), 5_031);

        // The composite value the *unconverted* code wrote is nowhere near it:
        // round(224 · 100 / 319) = 70 percent against the layer's 50.
        assert_eq!(pixel_to_percent(224, 320), 70);

        // At the identity the conversion is the fraction-of-extent read alone,
        // which is the deliberate ≤ 1 unit correction over the old `extent − 1`
        // lattice: pixel 0 of 320 is 0.15625 percent of the extent, not 0.
        let identity = tracked_centre_layer_unit([0, 0], 320, 180, LayerTransform::IDENTITY);
        assert!((identity[0] - 0.001_562_5).abs() < 1e-12);
        assert_eq!(layer_unit_to_percent(identity[0]), 0);
        assert_eq!(layer_unit_to_basis_points(identity[0]), 16);
        // The middle of the raster agrees with the lattice to the percent.
        let middle = tracked_centre_layer_unit([160, 90], 320, 180, LayerTransform::IDENTITY);
        assert_eq!(layer_unit_to_percent(middle[0]), 50);
        assert_eq!(layer_unit_to_percent(middle[1]), 50);
        assert_eq!(
            layer_unit_to_percent(middle[0]),
            i64::from(pixel_to_percent(160, 320))
        );

        // Both writers clamp: a layer coordinate outside the layer's own quad
        // is a real possibility at scale < 1, and neither control accepts it.
        assert_eq!(layer_unit_to_percent(-0.02), 0);
        assert_eq!(layer_unit_to_percent(1.4), 100);
        assert_eq!(layer_unit_to_basis_points(-0.02), 0);
        assert_eq!(layer_unit_to_basis_points(1.4), 10_000);
    }

    /// CC5 §5.2: the mask and the reframe subject state a *full* extent in
    /// layer percent, so the composite template is that extent times the scale.
    /// Cross-checked against [`matte_track_box_percent`], whose input is a half
    /// extent in basis points: a 50 percent region is `hw = 2500 bp`.
    #[test]
    fn tracked_box_percent_rescales_the_template_by_the_layer_scale() {
        assert_eq!(tracked_box_percent(40, 1.0), 40);
        assert_eq!(tracked_box_percent(40, 0.5), 20);
        assert_eq!(tracked_box_percent(70, 0.5), 35);
        // Out of the tracker's 1..=75 band in both directions.
        assert_eq!(tracked_box_percent(50, 2.0), 100);
        assert_eq!(tracked_box_percent(4, 0.1), 0);
        // The same rule the matte window already uses.
        assert_eq!(
            tracked_box_percent(50, 0.5),
            matte_track_box_percent(2_500, 0.5)
        );
    }

    /// CC5 §5.2: per-frame resolution is what lets a *keyframed* transform be
    /// converted instead of refused. The same effect that
    /// `resolve_static_layer_transform` rejects resolves cleanly at each frame.
    #[test]
    fn resolve_layer_transform_at_follows_a_keyframed_scale() {
        let mut moving = Effect {
            id: EffectId(2),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::new(),
        };
        moving.keyframes.insert(
            "scale_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 100,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(40),
                        value: 50,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let effects = [moving];
        // Linear from 100 to 50 over 40 frames: 100, 75, 50 at 0, 20, 40.
        for (frame, expected) in [(0_i64, 1.0_f64), (20, 0.75), (40, 0.5)] {
            let resolved = resolve_layer_transform_at(&effects, TimeCode(frame));
            assert!(
                (resolved.scale - expected).abs() < 1e-12,
                "frame {frame} resolved scale {}",
                resolved.scale
            );
            assert!(resolved.offset_x.abs() < f64::EPSILON);
            assert!(resolved.offset_y.abs() < f64::EPSILON);
        }
        // The static resolver still refuses it, which is why the two exist.
        assert!(
            resolve_static_layer_transform(&effects, &[TimeCode(0), TimeCode(40)]).is_err(),
            "a moving scale is not one affine map"
        );

        // A static chain resolves to the same values at every frame, and the
        // offsets are the compositor's own `percent / 50`.
        let static_chain = [Effect {
            id: EffectId(3),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([
                ("scale_percent".to_owned(), ParamValue::Integer(50)),
                ("x_percent".to_owned(), ParamValue::Integer(20)),
                ("y_percent".to_owned(), ParamValue::Integer(20)),
            ]),
            keyframes: BTreeMap::new(),
        }];
        let resolved = resolve_layer_transform_at(&static_chain, TimeCode(7));
        assert!((resolved.scale - 0.5).abs() < f64::EPSILON);
        assert!((resolved.offset_x - 0.4).abs() < f64::EPSILON);
        assert!((resolved.offset_y - 0.4).abs() < f64::EPSILON);
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

fn rgba_mean_absolute_difference_basis_points(
    left: &kinewright_core::RgbaImage,
    right: &kinewright_core::RgbaImage,
) -> Option<u16> {
    if left.width != right.width
        || left.height != right.height
        || left.pixels.len() != right.pixels.len()
        || !left.pixels.len().is_multiple_of(4)
    {
        return None;
    }
    let mut difference = 0_u128;
    let mut channels = 0_u128;
    for (left_pixel, right_pixel) in left
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(right.pixels.as_chunks::<4>().0.iter())
    {
        for channel in 0..3 {
            difference = difference.saturating_add(u128::from(
                left_pixel[channel].abs_diff(right_pixel[channel]),
            ));
            channels = channels.saturating_add(1);
        }
    }
    if channels == 0 {
        return None;
    }
    let denominator = channels.saturating_mul(u128::from(u8::MAX));
    let rounded = difference
        .saturating_mul(10_000)
        .saturating_add(denominator / 2)
        / denominator;
    Some(u16::try_from(rounded).unwrap_or(10_000).min(10_000))
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
        AssetBeats, AssetId, AssetSceneChanges, AssetTranscript, BeatMarker, Clip, ColorBitDepth,
        ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
        ColorWhitePoint, FrameTexture, Marker, MarkerId, MediaAsset, MediaAvailabilityKind,
        MediaAvailabilityStatus, MediaCacheClearResult, MediaCacheFamily, MediaCacheInventory,
        MediaError, MediaEvent, MediaKind, MediaSourceFingerprint, MonitorProof,
        MonitorProofMetadata, ParamValue, Rational, RgbaImage, SceneChange, SceneStatus,
        SilenceSpan, SilenceStatus, TimelineSceneChange, TimelineSilenceSpan, Title, Track,
        TrackId, TrackKind, TranscriptWord, VisualAssetResult,
    };
    use serde_json::json;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
        time::Instant,
    };

    #[derive(Default)]
    struct NoopMedia {
        probe_asset: Option<MediaAsset>,
        probe_paths: Mutex<Vec<PathBuf>>,
        cache_inventory: Option<MediaCacheInventory>,
        clear_cache_result: Option<MediaCacheClearResult>,
        availability_by_asset: BTreeMap<AssetId, MediaAvailabilityStatus>,
        availability_override: Option<Arc<Mutex<BTreeMap<AssetId, MediaAvailabilityStatus>>>>,
        transcript: Option<Arc<AssetTranscript>>,
        beat_statuses: BTreeMap<AssetId, BeatStatus>,
        scene_statuses: BTreeMap<AssetId, SceneStatus>,
        timeline_beats: Vec<TimelineBeat>,
        timeline_beat_error: Option<String>,
        beat_requests: Mutex<Vec<AssetId>>,
        scene_requests: Mutex<Vec<AssetId>>,
        thumbnail_frames: BTreeMap<TimeCode, RgbaImage>,
        candidate_thumbnail_frames: BTreeMap<TimeCode, RgbaImage>,
        candidate_effect_id: Option<EffectId>,
        /// Distinguish the candidate document by a stored primary parameter
        /// value rather than by node identity, so a proposal that corrects an
        /// existing node in place is still recognised as the candidate.
        candidate_primary_exposure_milli_stops: Option<i64>,
        render_error: Option<String>,
        proof_error: Option<MediaError>,
        /// Render a *different* frame whenever a node in the document carries
        /// `bypass = 1`, so a bypass path that is not actually lossless can be
        /// exercised (CC4 §8).
        bypass_leaks_pixel: Option<u8>,
        /// CC5 §4.1: the coverage raster this double answers a matte proof
        /// with. `None` keeps the trait's real `NotImplemented` default, which
        /// is what the production engine still returns, so both branches of
        /// every CC5 agent path are exercised.
        matte_coverage: Option<RgbaImage>,
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
        fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError> {
            self.probe_paths.lock().unwrap().push(path.to_path_buf());
            self.probe_asset
                .clone()
                .map_or(Err(MediaError::NotImplemented), |mut asset| {
                    asset.path = path.to_path_buf();
                    Ok(asset)
                })
        }

        fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
            if let Some(status) = self
                .availability_override
                .as_ref()
                .and_then(|statuses| statuses.lock().unwrap().get(&asset.id).cloned())
            {
                return status;
            }
            self.availability_by_asset
                .get(&asset.id)
                .cloned()
                .unwrap_or_else(|| MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::OnlineUnverified,
                    observed_fingerprint: None,
                    reason: Some("test backend does not inspect filesystem state".to_owned()),
                })
        }

        fn cache_inventory(&self) -> MediaCacheInventory {
            self.cache_inventory.clone().unwrap_or(MediaCacheInventory {
                families: Vec::new(),
            })
        }

        fn clear_cache(
            &self,
            family: MediaCacheFamily,
        ) -> Result<MediaCacheClearResult, MediaError> {
            if family == MediaCacheFamily::GeneratedProxy {
                return Err(MediaError::NotImplemented);
            }
            self.clear_cache_result
                .clone()
                .ok_or(MediaError::NotImplemented)
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
            document: Arc<Document>,
            t: TimeCode,
            _max_w: u32,
        ) -> Result<RgbaImage, MediaError> {
            if let Some(error) = &self.render_error {
                return Err(MediaError::Backend(error.clone()));
            }
            let candidate = document
                .tracks
                .iter()
                .flat_map(|track| track.clips.iter())
                .any(|clip| {
                    clip.effects.iter().any(|effect| {
                        effect.name == "primary_correction"
                            && self.candidate_effect_id.is_none_or(|id| effect.id == id)
                            && self
                                .candidate_primary_exposure_milli_stops
                                .is_none_or(|value| {
                                    effect.parameters.get("exposure_milli_stops")
                                        == Some(&ParamValue::Integer(value))
                                })
                    })
                });
            if candidate && let Some(image) = self.candidate_thumbnail_frames.get(&t) {
                return Ok(image.clone());
            }
            if let Some(pixel) = self.bypass_leaks_pixel
                && document
                    .tracks
                    .iter()
                    .flat_map(|track| track.clips.iter())
                    .any(|clip| {
                        clip.effects.iter().any(|effect| {
                            effect
                                .parameters
                                .get(kinewright_core::COLOR_NODE_BYPASS_PARAMETER)
                                == Some(&ParamValue::Integer(1))
                        })
                    })
            {
                return Ok(RgbaImage {
                    width: 2,
                    height: 2,
                    pixels: vec![pixel; 16],
                });
            }
            if let Some(image) = self.thumbnail_frames.get(&t) {
                return Ok(image.clone());
            }
            Ok(RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![0; 16],
            })
        }

        fn matte_proof_for_document(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
            clip: ClipId,
            effect: EffectId,
        ) -> Result<kinewright_core::MatteProof, MediaError> {
            let Some(coverage) = self.matte_coverage.clone() else {
                return Err(MediaError::NotImplemented);
            };
            Ok(kinewright_core::MatteProof {
                metadata: kinewright_core::MatteProofMetadata {
                    render: MonitorProofMetadata::test_double(),
                    clip,
                    effect,
                    node_kind: "color_wheels".to_owned(),
                    coverage_encoding: kinewright_core::MATTE_COVERAGE_ENCODING.to_owned(),
                    coverage_scale: kinewright_core::MATTE_COVERAGE_SCALE,
                    raster_aspect_millionths: 1_777_778,
                    matte_enabled: true,
                    window_count: 1,
                    qualifier_enabled: false,
                },
                coverage,
            })
        }

        fn monitor_proof_for_document(
            &self,
            document: Arc<Document>,
            t: TimeCode,
        ) -> Result<MonitorProof, MediaError> {
            if let Some(error) = &self.proof_error {
                return Err(error.clone());
            }
            let image = self.thumbnail_for_document(Arc::clone(&document), t, u32::MAX)?;
            let (width, height) = document.resolution;
            if width == 0 || height == 0 {
                return Err(MediaError::Backend(
                    "test proof document has an empty raster".to_owned(),
                ));
            }
            let proof_image = if image.width == width && image.height == height {
                image
            } else {
                let Some(source) =
                    image::RgbaImage::from_raw(image.width, image.height, image.pixels)
                else {
                    return Err(MediaError::Backend(
                        "test proof image has invalid RGBA dimensions".to_owned(),
                    ));
                };
                let resized = image::imageops::resize(
                    &source,
                    width,
                    height,
                    image::imageops::FilterType::Nearest,
                );
                RgbaImage {
                    width,
                    height,
                    pixels: resized.into_raw(),
                }
            };
            Ok(MonitorProof {
                image: proof_image,
                metadata: MonitorProofMetadata::test_double(),
            })
        }

        fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
        };
        let media = Arc::new(NoopMedia::default());
        (Core::spawn(document).unwrap(), media.clone(), media)
    }

    fn verified_source_analysis() -> Arc<dyn Analysis> {
        Arc::new(NoopMedia {
            availability_by_asset: BTreeMap::from([(
                AssetId(1),
                MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::OnlineVerified,
                    observed_fingerprint: None,
                    reason: Some("verified source fixture".to_owned()),
                },
            )]),
            ..NoopMedia::default()
        })
    }

    fn source_program_service_with_second_video_track() -> KinewrightMcp {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        document.tracks.push(Track {
            id: TrackId(9),
            kind: TrackKind::Video,
            sync_lock: false,
            // Keep a lower id after the overwrite so Core's post-clear id
            // allocator reuses the removed highest id (99). An id-only
            // before/after diff would mistake the valid replacement for the
            // old clip and fail to report the routed result.
            clips: vec![
                Clip {
                    id: ClipId(99),
                    asset: AssetId(1),
                    source_range: TimeCode::ZERO..TimeCode(20),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(10),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                },
                Clip {
                    id: ClipId(98),
                    asset: AssetId(1),
                    source_range: TimeCode(20)..TimeCode(30),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(40),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                },
            ],
        });
        KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            playback,
            verified_source_analysis(),
            ConfirmationBroker::default(),
        )
    }

    fn source_program_av_service() -> KinewrightMcp {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        document.media_pool[0].kind = MediaKind::AudioVideo;
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        });
        KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            playback,
            verified_source_analysis(),
            ConfirmationBroker::default(),
        )
    }

    fn fingerprint(byte_len: u64, nibble: char) -> MediaSourceFingerprint {
        MediaSourceFingerprint {
            content_sha256: Some(std::iter::repeat_n(nibble, 64).collect()),
            byte_len: Some(byte_len),
        }
    }

    fn relink_probe_asset(source_fingerprint: MediaSourceFingerprint) -> MediaAsset {
        MediaAsset {
            id: AssetId(99),
            path: PathBuf::from("probe-placeholder.mp4"),
            name: "replacement".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint,
            color_description: kinewright_core::ColorDescription::default(),
        }
    }

    fn relink_service(
        current_fingerprint: MediaSourceFingerprint,
        candidate_fingerprint: MediaSourceFingerprint,
    ) -> (KinewrightMcp, Core, Arc<NoopMedia>) {
        relink_service_with_probe(
            current_fingerprint,
            relink_probe_asset(candidate_fingerprint),
        )
    }

    fn relink_service_with_probe(
        current_fingerprint: MediaSourceFingerprint,
        probe_asset: MediaAsset,
    ) -> (KinewrightMcp, Core, Arc<NoopMedia>) {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        document.media_pool[0].source_fingerprint = current_fingerprint;
        let core = Core::spawn(document).unwrap();
        let media = Arc::new(NoopMedia {
            probe_asset: Some(probe_asset),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            media.clone(),
            ConfirmationBroker::default(),
        );
        (service, core, media)
    }

    fn relink_request(
        expected_revision: u64,
        asset_id: u64,
        path: &str,
        allow_unverified_source: bool,
    ) -> CallToolRequestParams {
        CallToolRequestParams::new("relink_media").with_arguments(
            json!({
                "expected_revision": expected_revision,
                "asset_id": asset_id,
                "path": path,
                "allow_unverified_source": allow_unverified_source,
            })
            .as_object()
            .unwrap()
            .clone(),
        )
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let music = MediaAsset {
            id: AssetId(9),
            path: PathBuf::from("montage-music.mp4"),
            name: "montage music".to_owned(),
            duration: TimeCode(180),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let music = MediaAsset {
            id: AssetId(9),
            path: PathBuf::from("music-structure-audio.wav"),
            name: "music structure audio".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            color_context: kinewright_core::ColorContext::default(),
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
            Arc::new(RwLock::new(None)),
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
        let qa_color_warning = readiness["qa"]["warning_issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|issue| issue["code"] == "source_color_metadata_uncertain")
            .expect("readiness should expose source colour review by asset");
        assert_eq!(qa_color_warning["asset"], 1);
        assert_eq!(qa_color_warning["severity"], "warning");
        let delivery_color_warning = readiness["delivery"]["warning_issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|issue| issue["code"] == "source_color_metadata_uncertain")
            .expect("delivery readiness should retain source colour review");
        assert_eq!(delivery_color_warning["asset"], 1);
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
    fn beat_montage_bounded_failure_returns_one_exact_feasible_retry() {
        let (core, analysis) = montage_fixture(ready_montage_status());
        let service = KinewrightMcp::new(
            core,
            analysis.clone(),
            analysis,
            ConfirmationBroker::default(),
        );
        let mut args = montage_plan_args();
        args.cut_anchor_frames = Some(vec![TimeCode(31)]);
        args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
            maximum_movement_frames: TimeCode::ZERO,
            locked_anchor_indices: Vec::new(),
        });

        let rejected = service.plan_beat_montage(&args).unwrap();
        assert_eq!(rejected.is_error, Some(true));
        assert!(
            rejected.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("reuse it instead of guessing")
        );
        let recovery = rejected.structured_content.unwrap();
        assert_eq!(recovery["status"], "bounded_anchor_repair_infeasible");
        let feasible = &recovery["nearest_globally_feasible"];
        assert_eq!(feasible["cut_anchor_frames"], json!([30]));
        assert_eq!(feasible["shot_durations"], json!([30, 30]));
        assert_eq!(
            feasible["exact_retry_patch"],
            json!({
                "cut_anchor_frames": [30],
                "anchor_repair": {
                    "maximum_movement_frames": 0,
                    "locked_anchor_indices": [],
                },
            })
        );

        args.cut_anchor_frames = Some(vec![TimeCode(30)]);
        let exact_retry = service.plan_beat_montage(&args).unwrap();
        assert_eq!(exact_retry.is_error, Some(false));
        assert_eq!(
            exact_retry.structured_content.unwrap()["plan"]["cut_anchors"][0]["beat"]["project_frame"],
            30
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
    fn m41_media_status_reports_dynamic_availability_jobs_and_preview_limits() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let result = service
            .call_blocking(CallToolRequestParams::new("get_media_status"))
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let value = result.structured_content.unwrap();
        assert_eq!(value["timeline_revision"], 0);
        assert_eq!(value["preview"]["mode"], "in_memory");
        assert_eq!(value["preview"]["max_width"], 1_280);
        assert_eq!(value["preview"]["persistent"], false);
        assert_eq!(value["preview"]["generated_proxy_supported"], false);
        assert_eq!(value["assets"].as_array().unwrap().len(), 1);
        assert_eq!(value["assets"][0]["path"], "fixture.mp4");
        assert_eq!(
            value["assets"][0]["availability"]["kind"],
            "online_unverified"
        );
        assert_eq!(
            value["assets"][0]["analysis_jobs"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn m41_timeline_proofs_ignore_offline_media_pool_assets_until_referenced() {
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        let mut unused = document.media_pool[0].clone();
        unused.id = AssetId(2);
        unused.path = PathBuf::from("unused-offline.mp4");
        document.media_pool.push(unused);
        document.validate().unwrap();

        let offline = MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OfflineMissing,
            observed_fingerprint: None,
            reason: Some("test source is offline".to_owned()),
        };
        let media = Arc::new(NoopMedia {
            availability_by_asset: BTreeMap::from([(AssetId(2), offline)]),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(
            Core::spawn(document.clone()).unwrap(),
            media.clone(),
            media,
            ConfirmationBroker::default(),
        );
        assert!(
            service
                .document_availability_error(&document, "frame proof")
                .is_none(),
            "an unused offline bin item must not block a timeline proof"
        );

        document.tracks[0].clips[0].asset = AssetId(2);
        assert!(
            service
                .document_availability_error(&document, "frame proof")
                .is_some(),
            "a referenced offline source must block the proof explicitly"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc1_color_proof_preflight_is_scoped_to_active_visual_layers() {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let managed_source = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = managed_source.clone();

        let mut later_video = document.media_pool[0].clone();
        later_video.id = AssetId(2);
        later_video.name = "later-offline-video".to_owned();
        later_video.path = PathBuf::from("later-offline.mp4");
        let mut offline_audio = later_video.clone();
        offline_audio.id = AssetId(3);
        offline_audio.name = "offline-audio".to_owned();
        offline_audio.path = PathBuf::from("offline-audio.wav");
        offline_audio.kind = MediaKind::Audio;
        let mut offline_overlay = later_video.clone();
        offline_overlay.id = AssetId(4);
        offline_overlay.name = "active-offline-overlay".to_owned();
        offline_overlay.path = PathBuf::from("active-offline-overlay.mp4");
        document
            .media_pool
            .extend([later_video, offline_audio, offline_overlay]);

        let mut later_clip = document.tracks[0].clips[0].clone();
        later_clip.id = ClipId(2);
        later_clip.asset = AssetId(2);
        later_clip.timeline_start = TimeCode(60);
        later_clip.source_range = TimeCode::ZERO..TimeCode(30);
        document.tracks[0].clips.push(later_clip);

        let mut audio_clip = document.tracks[0].clips[0].clone();
        audio_clip.id = ClipId(3);
        audio_clip.asset = AssetId(3);
        audio_clip.source_range = TimeCode::ZERO..TimeCode(60);
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![audio_clip],
        });
        document.duration = TimeCode(90);
        document.validate().unwrap();

        let proof_args = || RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        };
        let offline = |reason: &str| MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OfflineMissing,
            observed_fingerprint: None,
            reason: Some(reason.to_owned()),
        };
        let media_for = |availability_by_asset| {
            Arc::new(NoopMedia {
                availability_by_asset,
                thumbnail_frames: BTreeMap::from([(
                    TimeCode(12),
                    RgbaImage {
                        width: 2,
                        height: 2,
                        pixels: [32, 32, 32, 255].repeat(4),
                    },
                )]),
                candidate_thumbnail_frames: BTreeMap::from([(
                    TimeCode(12),
                    RgbaImage {
                        width: 2,
                        height: 2,
                        pixels: [96, 64, 32, 255].repeat(4),
                    },
                )]),
                ..NoopMedia::default()
            })
        };

        // A later offline video and an offline audio track are irrelevant to
        // the exact frame being proven.
        let media = media_for(BTreeMap::from([
            (AssetId(2), offline("later video is offline")),
            (AssetId(3), offline("audio is offline")),
        ]));
        let service = KinewrightMcp::new(
            Core::spawn(document.clone()).unwrap(),
            playback.clone(),
            media.clone(),
            ConfirmationBroker::default(),
        );
        let result = service.render_color_proof(&proof_args()).unwrap();
        assert_eq!(result.is_error, Some(false));
        let manifest = result.structured_content.unwrap();
        assert_eq!(
            manifest["active_rendered_sources"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(manifest["active_rendered_sources"][0]["asset_id"], 1);

        // An offline source on a second video track is an active overlay and
        // must block even though the selected clip itself is online.
        let mut overlay_document = document.clone();
        let mut overlay_clip = overlay_document.tracks[0].clips[0].clone();
        overlay_clip.id = ClipId(4);
        overlay_clip.asset = AssetId(4);
        overlay_document.tracks.push(Track {
            id: TrackId(3),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![overlay_clip],
        });
        overlay_document.validate().unwrap();
        let media = media_for(BTreeMap::from([(
            AssetId(4),
            offline("active overlay is offline"),
        )]));
        let service = KinewrightMcp::new(
            Core::spawn(overlay_document.clone()).unwrap(),
            playback.clone(),
            media.clone(),
            ConfirmationBroker::default(),
        );
        let result = service.render_color_proof(&proof_args()).unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "media_offline");
        assert_eq!(structured["details"]["clip_id"], 4);
        assert_eq!(structured["details"]["asset_id"], 4);

        // Freeze frames are source-backed visual layers too; their held frame
        // still requires the referenced asset to be available.
        let mut freeze_document = overlay_document;
        freeze_document.tracks[2].clips[0].content =
            kinewright_core::ClipContent::Freeze(kinewright_core::FreezeFrame {
                source_frame: TimeCode(3),
            });
        freeze_document.validate().unwrap();
        let media = media_for(BTreeMap::from([(
            AssetId(4),
            offline("active freeze source is offline"),
        )]));
        let service = KinewrightMcp::new(
            Core::spawn(freeze_document).unwrap(),
            playback.clone(),
            media,
            ConfirmationBroker::default(),
        );
        let result = service.render_color_proof(&proof_args()).unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "media_offline");
        assert_eq!(structured["details"]["clip_id"], 4);

        // The selected source remains an explicit hard failure when it is the
        // active source that is offline.
        let media = media_for(BTreeMap::from([(
            AssetId(1),
            offline("selected source is offline"),
        )]));
        let service = KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            playback,
            media,
            ConfirmationBroker::default(),
        );
        let result = service.render_color_proof(&proof_args()).unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "media_offline");
        assert_eq!(structured["details"]["clip_id"], 1);
        assert_eq!(structured["details"]["asset_id"], 1);
    }

    #[test]
    fn cc1_color_proof_blocks_an_unsupported_non_selected_active_layer() {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let managed_source = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = managed_source;

        // A second, non-selected video track composites into the same proof
        // raster with a source the managed pipeline cannot classify.
        let mut overlay_asset = document.media_pool[0].clone();
        overlay_asset.id = AssetId(2);
        overlay_asset.name = "unsupported-overlay".to_owned();
        overlay_asset.path = PathBuf::from("unsupported-overlay.mp4");
        overlay_asset.color_description = ColorDescription::unknown();
        document.media_pool.push(overlay_asset);
        let mut overlay_clip = document.tracks[0].clips[0].clone();
        overlay_clip.id = ClipId(4);
        overlay_clip.asset = AssetId(2);
        // A non-blocking post-primary stage on the same refused composite. The
        // error is the only place it can still be reported, because the
        // successful payload that normally carries it is never produced.
        overlay_clip.effects.push(Effect {
            id: EffectId(41),
            name: "look_lut".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        document.tracks.push(Track {
            id: TrackId(9),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![overlay_clip],
        });
        document.validate().unwrap();

        let media = Arc::new(NoopMedia {
            thumbnail_frames: BTreeMap::from([(
                TimeCode(12),
                RgbaImage {
                    width: 2,
                    height: 2,
                    pixels: [32, 32, 32, 255].repeat(4),
                },
            )]),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            playback,
            media,
            ConfirmationBroker::default(),
        );
        let result = service
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 500)]),
            })
            .unwrap();

        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "active_layer_needs_color_override");
        assert_eq!(structured["details"]["clip_id"], 4);
        assert_eq!(structured["details"]["asset_id"], 2);
        assert!(structured["details"]["field"].is_string());
        assert!(structured["details"]["observed"].is_string());
        assert!(structured["details"]["allowed"].is_string());

        // Non-blocking layer warnings ride along on the refusal instead of
        // being dropped with the success payload.
        let warnings = structured["details"]["unsupported_layer_warnings"]
            .as_array()
            .expect("the refusal carries the non-blocking layer warnings");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0]["code"], "legacy_lut_stage");
        assert_eq!(warnings[0]["clip_id"], 4);
        assert_eq!(warnings[0]["asset_id"], 2);
        assert_eq!(warnings[0]["effect_id"], 41);
        assert_eq!(
            warnings[0]["blocking"], false,
            "the blocking source is the error, never a warning"
        );
    }

    #[test]
    fn m41_cache_status_and_scoped_clear_are_typed_and_proxy_failure_is_explicit() {
        let (core, playback, _) = fixture();
        let media = Arc::new(NoopMedia {
            cache_inventory: Some(MediaCacheInventory {
                families: vec![kinewright_core::MediaCacheFamilyStatus {
                    family: MediaCacheFamily::VisualAssets,
                    supported: true,
                    root: Some(PathBuf::from("visual-assets/v1")),
                    file_count: 3,
                    bytes: 120,
                    may_repopulate: true,
                    note: Some("test inventory".to_owned()),
                }],
            }),
            clear_cache_result: Some(MediaCacheClearResult {
                family: MediaCacheFamily::VisualAssets,
                supported: true,
                removed_file_count: 3,
                removed_bytes: 120,
                may_repopulate: true,
                note: Some("test clear".to_owned()),
            }),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(core, playback, media, ConfirmationBroker::default());

        let status = service
            .call_blocking(CallToolRequestParams::new("get_cache_status"))
            .unwrap();
        assert_eq!(status.is_error, Some(false));
        assert_eq!(
            status.structured_content.unwrap()["families"][0]["file_count"],
            3
        );

        let clear = service
            .call_blocking(
                CallToolRequestParams::new("clear_media_cache").with_arguments(
                    json!({"family": "visual_assets"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap();
        assert_eq!(clear.is_error, Some(false));
        assert_eq!(clear.structured_content.unwrap()["removed_bytes"], 120);

        let unsupported = service
            .call_blocking(
                CallToolRequestParams::new("clear_media_cache").with_arguments(
                    json!({"family": "generated_proxy"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap();
        assert_eq!(unsupported.is_error, Some(true));
        let value = unsupported.structured_content.unwrap();
        assert_eq!(value["family"], "generated_proxy");
        assert_eq!(value["code"], "unsupported_generated_proxy");
        assert_eq!(value["supported"], false);
    }

    #[test]
    fn m41_relink_probes_applies_one_undoable_operation_and_rejects_known_mismatch() {
        let known = fingerprint(8, 'a');
        let (service, core, media) = relink_service(known.clone(), known.clone());
        let applied = service
            .call_blocking(relink_request(0, 1, "moved/replacement.mp4", false))
            .unwrap();
        assert_eq!(applied.is_error, Some(false));
        assert_eq!(
            media.probe_paths.lock().unwrap().as_slice(),
            [PathBuf::from("moved/replacement.mp4")]
        );
        let (revision, document) = service.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        assert_eq!(
            document.asset(AssetId(1)).unwrap().path,
            PathBuf::from("moved/replacement.mp4")
        );

        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("relink should be undoable");
        };
        assert_eq!(
            doc.asset(AssetId(1)).unwrap().path,
            PathBuf::from("fixture.mp4")
        );

        let (service, _, _) = relink_service(known, fingerprint(8, 'b'));
        let mismatch = service
            .call_blocking(relink_request(0, 1, "wrong-content.mp4", false))
            .unwrap();
        assert_eq!(mismatch.is_error, Some(true));
        assert!(
            mismatch.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("fingerprint")
        );
        assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));
        assert_eq!(
            service
                .snapshot()
                .unwrap()
                .1
                .asset(AssetId(1))
                .unwrap()
                .path,
            PathBuf::from("fixture.mp4")
        );

        let mut metadata_mismatch = relink_probe_asset(fingerprint(8, 'a'));
        metadata_mismatch.duration = TimeCode(59);
        let (service, _, _) = relink_service_with_probe(fingerprint(8, 'a'), metadata_mismatch);
        let mismatch = service
            .call_blocking(relink_request(0, 1, "wrong-duration.mp4", false))
            .unwrap();
        assert_eq!(mismatch.is_error, Some(true));
        assert!(
            mismatch.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("duration")
        );
        assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));
    }

    #[test]
    fn m41_relink_requires_legacy_opt_in_and_stale_revision_preflights_before_probe() {
        let candidate_fingerprint = fingerprint(8, 'a');
        let (service, _, media) = relink_service(
            MediaSourceFingerprint::unknown(),
            candidate_fingerprint.clone(),
        );
        let refused = service
            .call_blocking(relink_request(0, 1, "legacy-replacement.mp4", false))
            .unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert!(
            refused.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("allow_unverified_source")
        );
        assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));

        let accepted = service
            .call_blocking(relink_request(0, 1, "legacy-replacement.mp4", true))
            .unwrap();
        assert_eq!(accepted.is_error, Some(false));
        assert!(
            service
                .snapshot()
                .unwrap()
                .1
                .asset(AssetId(1))
                .unwrap()
                .source_fingerprint
                .is_verified()
        );

        let before_probe_count = media.probe_paths.lock().unwrap().len();
        let stale = service
            .call_blocking(relink_request(0, 1, "stale.mp4", true))
            .unwrap();
        assert_eq!(stale.is_error, Some(true));
        assert!(
            stale.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("revision conflict")
        );
        assert_eq!(media.probe_paths.lock().unwrap().len(), before_probe_count);
    }

    #[test]
    fn m41_relink_is_not_available_through_generated_operation_or_edit_plan() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        assert!(registry.iter().any(|tool| tool.name == "relink_media"));
        assert!(registry.iter().all(|tool| tool.name != "relink_asset"));
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let result = service
            .call_blocking(
                CallToolRequestParams::new("apply_edit_plan").with_arguments(
                    json!({
                        "expected_revision": 0,
                        "operations": [{
                            "op": "relink_asset",
                            "asset": 1,
                            "candidate": {
                                "path": "bypass.mp4",
                                "fingerprint": {},
                                "kind": "Video",
                                "fps": {"numerator": 30, "denominator": 1},
                                "duration": 60,
                                "resolution": [320, 180]
                            },
                            "allow_unverified_source": true
                        }]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("relink_media")
        );
    }

    // -----------------------------------------------------------------------
    // CC5 §4.2 / §5.2 / §7 — the matte agent surface
    // -----------------------------------------------------------------------

    /// A `width × height` coverage raster whose codes come from `code(x, y)`.
    ///
    /// Built as a plain RGBA buffer, so no CC5 code path can prove its own
    /// statistics.
    fn matte_coverage_raster(width: u32, height: u32, code: impl Fn(u32, u32) -> u8) -> RgbaImage {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                let value = code(x, y);
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// A service whose clip carries one matted `color_wheels` node.
    fn matte_service(coverage: Option<RgbaImage>) -> (KinewrightMcp, Core) {
        matte_service_with(coverage, BTreeMap::new(), Vec::new())
    }

    fn matte_service_with(
        coverage: Option<RgbaImage>,
        extra_matte_parameters: BTreeMap<String, i64>,
        extra_effects: Vec<Effect>,
    ) -> (KinewrightMcp, Core) {
        let (seed, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        // A managed CC1 source: an unknown-primaries fixture is refused before
        // any proof or matte work happens.
        document.media_pool[0].color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        document.lut_assets.push(kinewright_core::LutAsset {
            id: kinewright_core::LutAssetId(1),
            sha256: "b".repeat(64),
            title: "transform".to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 17,
            byte_len: 1_024,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: kinewright_core::LutAssetSource::Builtin {
                name: "neutral".to_owned(),
            },
        });
        let mut parameters = BTreeMap::from([
            (
                "gain_red_thousandths".to_owned(),
                ParamValue::Integer(1_200),
            ),
            ("matte_enabled".to_owned(), ParamValue::Integer(1)),
            ("matte_window_count".to_owned(), ParamValue::Integer(1)),
        ]);
        for (name, value) in extra_matte_parameters {
            parameters.insert(name, ParamValue::Integer(value));
        }
        // Extras go first so an Input-stage node such as `technical_lut` sits
        // ahead of the Correction-stage wheels node Core's ordering rule
        // requires (CC4 §3.2).
        document.tracks[0].clips[0].effects = extra_effects
            .into_iter()
            .chain(std::iter::once(Effect {
                id: EffectId(1),
                name: "color_wheels".to_owned(),
                parameters,
                keyframes: BTreeMap::new(),
            }))
            .collect();
        let media = Arc::new(NoopMedia {
            matte_coverage: coverage,
            ..NoopMedia::default()
        });
        let core = Core::spawn(document).unwrap();
        let service =
            KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
        (service, core)
    }

    /// A service whose clip carries one matted `color_wheels` node and whose
    /// analysis backend answers thumbnails from `frames`.
    fn matte_track_service(
        frames: BTreeMap<TimeCode, RgbaImage>,
        extra_matte_parameters: BTreeMap<String, i64>,
        extra_effects: Vec<Effect>,
    ) -> (KinewrightMcp, Core) {
        let (seed, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        let mut parameters = BTreeMap::from([
            ("matte_enabled".to_owned(), ParamValue::Integer(1)),
            ("matte_window_count".to_owned(), ParamValue::Integer(1)),
        ]);
        for (name, value) in extra_matte_parameters {
            parameters.insert(name, ParamValue::Integer(value));
        }
        document.tracks[0].clips[0].effects = std::iter::once(Effect {
            id: EffectId(1),
            name: "color_wheels".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        })
        .chain(extra_effects)
        .collect();
        let media = Arc::new(NoopMedia {
            thumbnail_frames: frames,
            ..NoopMedia::default()
        });
        let core = Core::spawn(document).unwrap();
        let service =
            KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
        (service, core)
    }

    /// CC5 §4.2: the statistics are measured off a hand-built coverage, so
    /// every expected value below is derived by hand rather than by the code
    /// that produced it.
    #[test]
    fn inspect_grade_matte_reports_the_cc5_coverage_statistics() {
        // A 4 × 2 coverage:
        //   row 0: 255 255 128   0
        //   row 1: 255 255 128   0
        // Hand-derived: 6 covered (m > 0), 4 full (code 255), 2 partial,
        // 8 total, floor(6 * 10000 / 8) = 7500 basis points. The bounding box
        // of the covered set is columns 0..3 of rows 0..2, i.e. x 0..7500 of
        // the width and the whole height. Buckets are
        // min(15, floor(code * 16 / 256)): code 0 -> 0, 128 -> 8, 255 -> 15.
        let coverage = matte_coverage_raster(4, 2, |x, _| match x {
            0 | 1 => 255,
            2 => 128,
            _ => 0,
        });
        let (service, _core) = matte_service(Some(coverage));

        let result = service
            .inspect_grade_matte(&InspectGradeMatteArgs {
                expected_revision: Some(TimelineRevision(0)),
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                timecode: TimeCode(10),
                include_image: None,
            })
            .unwrap();
        assert_ne!(result.is_error, Some(true));
        let structured = result.structured_content.clone().unwrap();

        let statistics = &structured["statistics"];
        assert_eq!(statistics["covered_pixel_count"], 6);
        assert_eq!(statistics["full_pixel_count"], 4);
        assert_eq!(statistics["partial_pixel_count"], 2);
        assert_eq!(statistics["total_pixel_count"], 8);
        assert_eq!(statistics["covered_basis_points"], 7_500);
        assert_eq!(statistics["weighted_by_coverage"], true);
        let histogram = statistics["coverage_histogram"].as_array().unwrap();
        assert_eq!(histogram.len(), 16);
        assert_eq!(histogram[0], 2, "the two code-0 pixels land in bucket 0");
        assert_eq!(histogram[8], 2, "the two code-128 pixels land in bucket 8");
        assert_eq!(
            histogram[15], 4,
            "the four code-255 pixels land in bucket 15"
        );
        let total = histogram
            .iter()
            .map(|count| count.as_u64().unwrap())
            .sum::<u64>();
        assert_eq!(total, 8, "the buckets cover every pixel, code 0 included");

        // CC5 §4.3's threshold is reported at the level a caller reads it.
        assert_eq!(structured["matte_threshold"], "coverage_greater_than_zero");
        assert_eq!(structured["covered_pixel_count"], 6);
        assert_eq!(structured["raster"], json!({"width": 4, "height": 2}));
        assert_eq!(structured["coverage_encoding"], "linear_coverage_u8");
        assert_eq!(structured["coverage_scale"], 255);
        assert_eq!(structured["kind"], "color_wheels");
        assert_eq!(structured["active"], true);
        assert_eq!(structured["inactive_reason"], serde_json::Value::Null);
        // CC5 §1: the two coverage concepts are named apart.
        assert_eq!(structured["surface"], "Matte (this correction)");
        assert!(
            structured["distinct_from"]
                .as_str()
                .unwrap()
                .contains("Mask (layer alpha)")
        );
        // The full 47 integers, as a compact object.
        let resolved = &structured["resolved_matte_parameters"];
        assert_eq!(resolved["matte_enabled"], 1);
        assert_eq!(resolved["matte_window_count"], 1);
        assert_eq!(resolved["matte_mix_basis_points"], 10_000);
        assert_eq!(resolved["matte_hue_width_centidegrees"], 18_000);
        assert_eq!(resolved["windows"].as_array().unwrap().len(), 1);
        assert_eq!(resolved["windows"][0]["half_width_basis_points"], 2_500);
        // Renderer provenance rides along, unchanged from the monitor proof.
        assert_eq!(structured["provenance"]["node_kind"], "color_wheels");
        assert_eq!(structured["provenance"]["window_count"], 1);
        assert_eq!(
            structured["provenance"]["render"]["render_kind"],
            "test_double"
        );

        // A PNG is attached by default and suppressed on request.
        assert_eq!(structured["image_included"], true);
        assert!(
            result
                .content
                .iter()
                .any(|block| block.as_image().is_some()),
            "include_image defaults to true"
        );
        let without = service
            .inspect_grade_matte(&InspectGradeMatteArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                timecode: TimeCode(10),
                include_image: Some(false),
            })
            .unwrap();
        assert!(
            without
                .content
                .iter()
                .all(|block| block.as_image().is_none())
        );
    }

    /// CC5 §4.1: a backend that cannot proof fails typed. It never returns a
    /// blank frame, and it never invents a coverage number.
    #[test]
    fn inspect_grade_matte_surfaces_an_unavailable_proof_as_a_typed_refusal() {
        let (service, _core) = matte_service(None);
        let result = service
            .inspect_grade_matte(&InspectGradeMatteArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                timecode: TimeCode(10),
                include_image: None,
            })
            .unwrap();

        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "matte_proof_unavailable");
        assert_eq!(structured["applied"], false);
        let details = &structured["details"];
        assert_eq!(details["field"], "effect_id");
        assert_eq!(details["observed"]["effect_id"], 1);
        assert_eq!(details["observed"]["node_kind"], "color_wheels");
        assert_eq!(details["observed"]["has_matte"], true);
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("no coverage is invented here")
        );
        // The resolved matte is still published: the refusal is about the
        // render, not about the request.
        assert_eq!(details["resolved_matte"]["matte_enabled"], 1);
    }

    /// CC5 §2.1: `technical_lut` carries no matte, and the layer `mask` effect
    /// is a compositing alpha operation, not a colour node.
    #[test]
    fn inspect_grade_matte_refuses_nodes_that_cannot_carry_a_matte() {
        let (service, _core) = matte_service_with(
            None,
            BTreeMap::new(),
            vec![
                Effect {
                    id: EffectId(2),
                    name: "mask".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                },
                Effect {
                    id: EffectId(3),
                    name: "technical_lut".to_owned(),
                    parameters: BTreeMap::from([(
                        "lut_asset_id".to_owned(),
                        ParamValue::Integer(1),
                    )]),
                    keyframes: BTreeMap::new(),
                },
            ],
        );

        let mask = service
            .inspect_grade_matte(&InspectGradeMatteArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(2),
                timecode: TimeCode(10),
                include_image: None,
            })
            .unwrap();
        let structured = mask.structured_content.unwrap();
        assert_eq!(structured["code"], "matte_effect_not_a_color_node");
        assert!(
            structured["details"]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("compositing alpha operation")
        );

        let technical = service
            .inspect_grade_matte(&InspectGradeMatteArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(3),
                timecode: TimeCode(10),
                include_image: None,
            })
            .unwrap();
        let structured = technical.structured_content.unwrap();
        assert_eq!(structured["code"], "matte_unsupported_node_kind");
        assert_eq!(structured["details"]["observed"], "technical_lut");
        assert_eq!(
            structured["details"]["allowed"],
            json!(crate::color_status::MATTE_CAPABLE_NODE_NAMES)
        );
    }

    /// CC5 §7: `matte_comparison` is valid only alongside `effect_id`, is
    /// mutually exclusive with `look_comparison`, and needs a node that both
    /// may carry a matte and actually does. Every check runs before any render.
    #[test]
    fn render_color_proof_validates_matte_comparison_before_rendering() {
        let (service, _core) = matte_service_with(
            None,
            BTreeMap::new(),
            vec![Effect {
                id: EffectId(3),
                name: "color_curves".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            }],
        );
        let proof = |effect_id: Option<EffectId>,
                     matte: Option<MatteComparison>,
                     look: Option<LookComparison>| {
            service
                .render_color_proof(&RenderColorProofArgs {
                    expected_revision: TimelineRevision(0),
                    clip_id: ClipId(1),
                    timecode: TimeCode(10),
                    profile_assumption: None,
                    parameters: BTreeMap::new(),
                    effect_id,
                    look_comparison: look,
                    matte_comparison: matte,
                })
                .unwrap()
                .structured_content
                .unwrap()
        };

        let without_effect = proof(None, Some(MatteComparison::Coverage), None);
        assert_eq!(
            without_effect["code"],
            "matte_comparison_requires_effect_id"
        );
        assert_eq!(without_effect["details"]["field"], "matte_comparison");

        let both = proof(
            Some(EffectId(1)),
            Some(MatteComparison::InsideOnly),
            Some(LookComparison::Before),
        );
        assert_eq!(
            both["code"],
            "matte_comparison_conflicts_with_look_comparison"
        );
        assert!(
            both["details"]["allowed"]
                .as_str()
                .unwrap()
                .contains("exactly one")
        );

        // A matte-capable node that carries no matte has no coverage to
        // partition, so the proof refuses rather than rendering a blank frame.
        let no_matte = proof(Some(EffectId(3)), Some(MatteComparison::OutsideOnly), None);
        assert_eq!(no_matte["code"], "matte_proof_no_matte");
        assert_eq!(no_matte["details"]["observed"]["has_matte"], false);
        assert!(
            no_matte["details"]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("plan_secondary_correction")
        );
    }

    /// CC5 §7: `matte_invert` is Hold-only but keyframable, so `outside_only`
    /// must get the curve out of the way on its scratch copy — otherwise the
    /// static write is dead, the "outside" cell renders the *inside*, and the
    /// manifest says `outside_only` about a picture that is not.
    #[test]
    fn render_color_proof_outside_only_clears_a_keyframed_matte_invert_on_the_scratch_copy() {
        let (service, core) = matte_service(None);
        // A Hold curve that turns the matte inversion *on* from frame 0. The
        // stored static value stays 0, so a planner reading only the static
        // value would toggle to 1 and render exactly the inside cell.
        let Event::DocumentChanged { revision, .. } = core
            .request(Command::Do(Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(1),
                name: "matte_invert".to_owned(),
                curve: kinewright_core::AutomationCurve {
                    keyframes: vec![kinewright_core::Keyframe {
                        at: TimeCode(0),
                        value: 1,
                        interpolation: kinewright_core::KeyframeInterpolation::Hold,
                    }],
                },
            }))
            .unwrap()
        else {
            panic!("expected the keyframe to apply");
        };
        assert_eq!(revision, TimelineRevision(1));

        let result = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(1),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: Some(MatteComparison::OutsideOnly),
            })
            .unwrap();
        let manifest = result.structured_content.unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "outside_only proof refused: {manifest}"
        );
        let comparison = &manifest["matte_comparison"];
        assert_eq!(comparison["variant"], "outside_only");
        // The curve is cleared first, then the complement of the value the
        // curve renders at this frame is written. The rendered value is 1, so
        // the outside cell writes 0 — not 1, which is what complementing the
        // stored static value would have produced.
        assert_eq!(
            comparison["after_operations"],
            json!([
                {"ClearEffectKeyframes": {"clip": 1, "effect": 1, "name": "matte_invert"}},
                {"SetEffectParam": {
                    "clip": 1,
                    "effect": 1,
                    "name": "matte_invert",
                    "value": 0,
                }},
            ])
        );
        assert_eq!(comparison["cleared_keyframes"], json!(["matte_invert"]));

        // Scratch only: the live document keeps its automation untouched.
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        let effect = &document.clip(ClipId(1)).unwrap().effects[0];
        assert_eq!(
            effect.keyframes["matte_invert"].keyframes[0].value, 1,
            "the live document must still carry the curve"
        );
        assert!(!effect.parameters.contains_key("matte_invert"));

        // A node with no `matte_invert` automation is byte-unchanged: one
        // operation, and an empty `cleared_keyframes`.
        let (plain, _) = matte_service(None);
        let plain = plain
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: Some(MatteComparison::OutsideOnly),
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(
            plain["matte_comparison"]["cleared_keyframes"],
            json!([]),
            "a node with no matte_invert curve clears nothing"
        );
    }

    /// CC5 §7: `outside_only` renders a scratch copy with `matte_invert`
    /// toggled, and the manifest states exactly which variant it rendered.
    #[test]
    fn render_color_proof_outside_only_toggles_matte_invert_on_a_scratch_copy() {
        let (service, core) = matte_service(None);
        let result = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: Some(MatteComparison::OutsideOnly),
            })
            .unwrap();
        let manifest = result.structured_content.unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "outside_only proof refused: {manifest}"
        );
        let comparison = &manifest["matte_comparison"];
        assert_eq!(comparison["variant"], "outside_only");
        assert_eq!(comparison["effect_id"], 1);
        assert_eq!(comparison["kind"], "color_wheels");
        assert!(
            comparison["after_cell"]
                .as_str()
                .unwrap()
                .contains("matte_invert toggled")
        );
        // The exact scratch operation, hand-written.
        assert_eq!(
            comparison["after_operations"],
            json!([{"SetEffectParam": {
                "clip": 1,
                "effect": 1,
                "name": "matte_invert",
                "value": 1,
            }}])
        );
        // `inside_only` renders the document exactly as stored, so it has no
        // scratch operation at all.
        let inside = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: Some(MatteComparison::InsideOnly),
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(inside["matte_comparison"]["variant"], "inside_only");
        assert_eq!(inside["matte_comparison"]["after_operations"], json!([]));
        assert_eq!(inside["applied"], false);

        // Read-only: the live document never gained `matte_invert`.
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert_eq!(
            document.clip(ClipId(1)).unwrap().effects[0]
                .parameters
                .get("matte_invert"),
            None
        );
    }

    /// CC5 §7: `coverage` replaces the AFTER cell with the §4.1 proof image
    /// itself, and reports the measured coverage next to it.
    #[test]
    fn render_color_proof_coverage_returns_the_matte_proof_image() {
        // 320 × 180 is the fixture raster; the left third is covered.
        let coverage = matte_coverage_raster(320, 180, |x, _| u8::from(x < 106) * 255);
        let (service, _core) = matte_service(Some(coverage));
        let manifest = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: Some(MatteComparison::Coverage),
            })
            .unwrap()
            .structured_content
            .unwrap();

        let comparison = &manifest["matte_comparison"];
        assert_eq!(comparison["variant"], "coverage");
        // 106 columns of 320, over 180 rows: 106 * 180 = 19080 of 57600, and
        // floor(19080 * 10000 / 57600) = 3312 basis points.
        assert_eq!(comparison["coverage"]["covered_pixel_count"], 19_080);
        assert_eq!(
            comparison["coverage"]["statistics"]["covered_basis_points"],
            3_312
        );
        assert_eq!(
            comparison["coverage"]["matte_threshold"],
            "coverage_greater_than_zero"
        );
        assert_eq!(comparison["coverage"]["coverage_scale"], 255);
    }

    /// CC5 §7: a CC4 proof is byte-unchanged — no `matte_comparison` key at all
    /// when none was requested.
    #[test]
    fn render_color_proof_omits_matte_comparison_when_none_was_requested() {
        let (service, _core) = matte_service(None);
        let manifest = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: None,
            })
            .unwrap()
            .structured_content
            .unwrap();
        assert!(manifest.get("matte_comparison").is_none());
    }

    /// A 320 × 180 frame carrying one 40 × 40 bright box centred on `centre`.
    ///
    /// The `box_frame` pattern from `mod tracking_tests`, at the fixture
    /// raster: a static dark background with one high-contrast subject, which
    /// is what pins a normalized SAD template match at zero displacement error.
    fn matte_box_frame(centre: [u32; 2]) -> RgbaImage {
        let (width, height) = (320_u32, 180_u32);
        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
            *pixel = [48, 48, 48, 255];
        }
        for y in centre[1].saturating_sub(20)..(centre[1] + 20).min(height) {
            for x in centre[0].saturating_sub(20)..(centre[0] + 20).min(width) {
                let index = ((y * width + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&[235, 235, 235, 255]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    // -----------------------------------------------------------------------
    // CC5 §5.2, the mask and reframe halves.
    //
    // `track_mask_region` and `track_reframe_subject` measure the *composited*
    // thumbnail and write controls the compositor evaluates in *layer* uv, so
    // both need the same composite → layer conversion `track_matte_window`
    // already does. The analysis double answers the composited thumbnail the
    // real compositor would produce, with the subject drawn at the position the
    // shader's forward map puts it — the double ignores the document, so the
    // placement is stated here by hand rather than rendered.
    // -----------------------------------------------------------------------

    /// A 320 × 180 frame carrying one white box of half extent `half` pixels
    /// centred on `centre`, over `matte_box_frame`'s dark background.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn transform_box_frame(centre: [i64; 2], half: i64) -> RgbaImage {
        let (width, height) = (320_i64, 180_i64);
        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
            *pixel = [48, 48, 48, 255];
        }
        for y in (centre[1] - half).max(0)..(centre[1] + half).min(height) {
            for x in (centre[0] - half).max(0)..(centre[0] + half).min(width) {
                let index = ((y * width + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&[235, 235, 235, 255]);
            }
        }
        RgbaImage {
            width: 320,
            height: 180,
            pixels,
        }
    }

    /// A service over the fixture's 320 × 180, 30 fps, 60-frame media clip
    /// carrying `effects`, whose analysis double answers `frames`.
    fn transform_track_service(
        effects: Vec<Effect>,
        frames: BTreeMap<TimeCode, RgbaImage>,
    ) -> (KinewrightMcp, Core) {
        let (seed, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed_document).clone();
        document.tracks[0].clips[0].effects = effects;
        let media = Arc::new(NoopMedia {
            thumbnail_frames: frames,
            ..NoopMedia::default()
        });
        let core = Core::spawn(document).unwrap();
        let service =
            KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
        (service, core)
    }

    /// CC5 §5.2's worked transform: `scale_percent 50`, `x_percent 20`,
    /// `y_percent 20`.
    ///
    /// The compositor accumulates `scale = 50 / 100 = 0.5` and
    /// `offset = 20 / 50 = 0.4` on both axes, so the shader's placement is
    /// `u_composite = 0.5·(u_layer − 0.5) + 0.4/2 + 0.5 = 0.5·u_layer + 0.45`
    /// and its inverse is `u_layer = 2·u_composite − 0.9`.
    fn half_scale_transform() -> Effect {
        Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([
                ("scale_percent".to_owned(), ParamValue::Integer(50)),
                ("x_percent".to_owned(), ParamValue::Integer(20)),
                ("y_percent".to_owned(), ParamValue::Integer(20)),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    /// A layer scale that ramps 100 → 50 percent, linearly, over frames 0..=40.
    fn keyframed_scale_transform() -> Effect {
        Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::from([(
                "scale_percent".to_owned(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: 100,
                            interpolation: KeyframeInterpolation::Linear,
                        },
                        Keyframe {
                            at: TimeCode(40),
                            value: 50,
                            interpolation: KeyframeInterpolation::Linear,
                        },
                    ],
                },
            )]),
        }
    }

    /// A bounded mask at `center` percent with a `size` percent region.
    fn tracking_mask_effect(center: [i64; 2], size: [i64; 2]) -> Effect {
        Effect {
            id: EffectId(1),
            name: "mask".to_owned(),
            parameters: BTreeMap::from([
                ("shape_token".to_owned(), ParamValue::Integer(1)),
                (
                    "center_x_percent".to_owned(),
                    ParamValue::Integer(center[0]),
                ),
                (
                    "center_y_percent".to_owned(),
                    ParamValue::Integer(center[1]),
                ),
                ("width_percent".to_owned(), ParamValue::Integer(size[0])),
                ("height_percent".to_owned(), ParamValue::Integer(size[1])),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    /// A 1:1 reframe whose focus starts at `focus` percent. The source is
    /// 320 × 180, so a 10000 bp target crops *horizontally* to a 5625 bp
    /// window and leaves the vertical axis whole.
    fn tracking_reframe_effect(focus: [i64; 2]) -> Effect {
        Effect {
            id: EffectId(1),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([
                (
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(10_000),
                ),
                ("focus_x_percent".to_owned(), ParamValue::Integer(focus[0])),
                ("focus_y_percent".to_owned(), ParamValue::Integer(focus[1])),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    /// The keyframe values one prepared curve carries, in order.
    fn curve_values(structured: &serde_json::Value, name: &str) -> Vec<i64> {
        structured["curves"][name]["keyframes"]
            .as_array()
            .unwrap_or_else(|| panic!("curve {name} must be prepared"))
            .iter()
            .map(|keyframe| keyframe["value"].as_i64().unwrap())
            .collect()
    }

    /// One field of every observation, in order.
    fn observation_values(structured: &serde_json::Value, key: &str, field: &str) -> Vec<i64> {
        structured[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} must be published"))
            .iter()
            .map(|sample| sample[field].as_i64().unwrap())
            .collect()
    }

    /// The five sample frames every test below uses: 0, 10, 20, 30, 40.
    const TRANSFORM_TRACK_SAMPLES: [i64; 5] = [0, 10, 20, 30, 40];

    fn mask_tracking_args() -> TrackMaskArgs {
        TrackMaskArgs {
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(41)),
            step_frames: Some(10),
            // A 5 percent radius is a 16 pixel horizontal search, whose coarse
            // grid lands exactly on a subject moving 8 pixels a sample, so the
            // template match is pixel-exact rather than plateaued.
            search_radius_percent: Some(5),
            max_width: Some(320),
        }
    }

    /// CC5 §5.2 (a): at the identity transform the written mask centres are the
    /// analytic box centre, read as a *fraction of the extent*.
    ///
    /// The subject centre is composite pixel `x = 140 + 0.8·frame`, so the
    /// samples land on 140, 148, 156, 164 and 172 of 320 and
    /// `round((pixel + 0.5) · 100 / 320)` is 44, 46, 49, 51, 54 — the analytic
    /// box centre read as a fraction of the extent. The vertical axis is static
    /// at pixel 90 of 180: `round(90.5 · 100 / 180) = 50`.
    #[test]
    fn track_mask_region_writes_layer_space_centres_at_the_identity() {
        let frames = (0..60)
            .map(|frame| {
                (
                    TimeCode(frame),
                    transform_box_frame([140 + frame * 4 / 5, 90], 5),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // Seed on the subject: 44 percent of 320 is pixel 140 exactly. The
        // region is deliberately small — a 6 × 11 percent region is a 21 × 21
        // pixel template, and `track_region` subsamples a template that size
        // every pixel, so the match is exact rather than plateaued.
        let (service, core) =
            transform_track_service(vec![tracking_mask_effect([44, 50], [6, 11])], frames);

        let result = service.track_mask_region(&mask_tracking_args()).unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        assert_eq!(
            curve_values(&structured, "center_x_percent"),
            vec![44, 46, 49, 51, 54]
        );
        assert_eq!(
            curve_values(&structured, "center_y_percent"),
            vec![50, 50, 50, 50, 50]
        );
        // The observations carry the same layer values under both names.
        assert_eq!(
            observation_values(&structured, "observations", "center_x_percent"),
            observation_values(&structured, "observations", "layer_center_x_percent"),
        );
        // At the identity the layer and the composite readings agree to the
        // percent, so nothing about this shot could hide a missing conversion —
        // which is exactly why the transformed cases below exist. They agree
        // *exactly*, on every observation and both axes, only because the
        // composite provenance is read with the same fraction-of-the-extent
        // convention `coordinate_space.pixel_to_unit` publishes: on the
        // `extent − 1` lattice pixel 172 of 320 would read 54 against the same
        // 54 here but pixel 32 of 64 would read 51 against 51 only by luck, and
        // the identity would stop being an identity in general.
        for axis in ["x", "y"] {
            assert_eq!(
                observation_values(
                    &structured,
                    "observations",
                    &format!("composite_center_{axis}_percent"),
                ),
                observation_values(
                    &structured,
                    "observations",
                    &format!("layer_center_{axis}_percent"),
                ),
                "at the identity the composite provenance must equal the written layer value on {axis}",
            );
        }
        assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 1.0);
        assert_eq!(
            structured["coordinate_space"]["box_percent"],
            json!([6, 11])
        );
        assert_eq!(
            structured["coordinate_space"]["unit_to_percent"],
            "center_percent = round(u_layer * 100), clamped to 0..=100"
        );

        // Nothing is committed.
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .all(|effect| effect.keyframes.is_empty()),
            "track_mask_region commits nothing"
        );
    }

    /// CC5 §5.2 (b): under a static `scale 50 / x 20 / y 20` layer transform the
    /// written mask centres are the *layer*-space centres, not the composite
    /// ones the tracker measured.
    ///
    /// The subject sits at layer `u = 0.103125, 0.153125, 0.203125, 0.253125,
    /// 0.303125`, which the forward map `u_c = 0.5·u_l + 0.45` puts at composite
    /// `u = 0.5015625, 0.5265625, 0.5515625, 0.5765625, 0.6015625`, i.e. pixel
    /// centres 160, 168, 176, 184 and 192 of 320 — where the fixture draws it.
    /// Converting back with `u_l = 2·u_c − 0.9` gives 10, 15, 20, 25, 30
    /// percent. The unconverted composite reading is 50, 53, 55, 58, 60
    /// percent, so this test fails by tens of percent if the conversion is
    /// removed.
    #[test]
    fn track_mask_region_converts_the_composite_centre_into_layer_space() {
        let frames = (0..60)
            .map(|frame| {
                (
                    TimeCode(frame),
                    transform_box_frame([160 + frame * 4 / 5, 125], 5),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // Seed on the subject through the forward map: layer 10 percent is
        // composite 0.5·0.10 + 0.45 = 0.50, which is pixel 160 of 320. Layer 49
        // percent is composite 0.695, which is pixel 125 of 180.
        let (service, core) = transform_track_service(
            vec![
                half_scale_transform(),
                tracking_mask_effect([10, 49], [12, 22]),
            ],
            frames,
        );

        let result = service.track_mask_region(&mask_tracking_args()).unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        // The written, layer-space curve.
        let written = curve_values(&structured, "center_x_percent");
        for (index, expected) in [10_i64, 15, 20, 25, 30].iter().enumerate() {
            assert!(
                (written[index] - expected).abs() <= 2,
                "sample {index}: wrote {} against the analytic layer {expected}",
                written[index]
            );
        }
        // 49.4444 percent of the layer, from composite pixel 125 of 180.
        for value in curve_values(&structured, "center_y_percent") {
            assert!(
                (value - 49).abs() <= 2,
                "vertical layer centre {value} against the analytic 49"
            );
        }
        // The raw composite reading is preserved as provenance, and is nowhere
        // near the written value: this is the whole point of the conversion.
        let composite =
            observation_values(&structured, "observations", "composite_center_x_percent");
        assert_eq!(composite, vec![50, 53, 55, 58, 60]);
        assert_eq!(
            observation_values(&structured, "observations", "layer_center_x_percent"),
            written
        );
        // The template is the stored region rescaled by the layer scale:
        // 12 × 0.5 = 6 and 22 × 0.5 = 11 percent of the composite.
        assert_eq!(
            structured["coordinate_space"]["box_percent"],
            json!([6, 11])
        );
        // Seeded through the forward map: layer 10/49 percent is composite
        // 50/70 percent.
        assert_eq!(
            structured["coordinate_space"]["seed_center_percent"],
            json!([50, 70])
        );
        let samples = structured["coordinate_space"]["samples"]
            .as_array()
            .unwrap();
        assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
        for sample in samples {
            assert_eq!(sample["scale"], 0.5);
            assert_eq!(sample["offset_x"], 0.4);
            assert_eq!(sample["offset_y"], 0.4);
        }

        // The plan is still exactly two non-destructive keyframe operations,
        // and nothing is committed.
        let preview = &structured["prepared_edit_plan"]["preview"];
        assert_eq!(preview["operation_count"], 2);
        assert_eq!(preview["destructive_operations"], json!([]));
        assert_eq!(preview["before_clips"], preview["after_clips"]);
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .all(|effect| effect.keyframes.is_empty()),
            "track_mask_region commits nothing"
        );
    }

    /// CC5 §5.2 (c): a *keyframed* layer scale is converted sample by sample
    /// rather than refused.
    ///
    /// The scale ramps 100 → 50 percent over frames 0..=40, so at the samples it
    /// is 1.0, 0.875, 0.75, 0.625, 0.5 and a subject pinned at layer `u = 0.25`
    /// walks across the composite: `u_c = s·(0.25 − 0.5) + 0.5` is 0.25,
    /// 0.28125, 0.3125, 0.34375, 0.375, i.e. pixel centres 80, 90, 100, 110 and
    /// 120 of 320. Per-frame conversion recovers 25 percent at every sample; a
    /// single static transform, or none at all, would report 25, 28, 31, 34, 38.
    #[test]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn track_mask_region_converts_a_keyframed_layer_transform_per_frame() {
        let frames = (0..60)
            .map(|frame| {
                let scale = 1.0 - 0.5 * (frame as f64) / 40.0;
                // The layer shrinks, so the subject drawn on the composite
                // shrinks with it: half of 40 px times the scale.
                let half = (5.0 * scale).round() as i64;
                (
                    TimeCode(frame),
                    transform_box_frame([80 + frame, 90], half.max(2)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let (service, _core) = transform_track_service(
            vec![
                keyframed_scale_transform(),
                tracking_mask_effect([25, 50], [6, 11]),
            ],
            frames,
        );

        let result = service.track_mask_region(&mask_tracking_args()).unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        for (index, value) in curve_values(&structured, "center_x_percent")
            .into_iter()
            .enumerate()
        {
            assert!(
                (value - 25).abs() <= 2,
                "sample {index}: wrote {value} against the analytic layer 25"
            );
        }
        // The composite reading walks away from it — from about 25 percent to
        // about 37 — which is exactly what the per-frame conversion undoes.
        let composite =
            observation_values(&structured, "observations", "composite_center_x_percent");
        assert!(
            (composite[0] - 25).abs() <= 1,
            "the first composite reading is the seed: {composite:?}"
        );
        assert!(
            composite[4] - composite[0] >= 11,
            "the composite reading must drift as the layer shrinks: {composite:?}"
        );
        // One resolved transform per sample, and it moves.
        let samples = structured["coordinate_space"]["samples"]
            .as_array()
            .unwrap();
        assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
        assert_eq!(samples[0]["local_frame"], 0);
        assert_eq!(samples[0]["scale"], 1.0);
        assert_eq!(samples[4]["local_frame"], 40);
        assert_eq!(samples[4]["scale"], 0.5);
        assert_eq!(structured["coordinate_space"]["per_frame_transform"], true);
    }

    /// CC5 §5.2 (d): the tracking template is the stored region *rescaled by the
    /// layer scale*, so a region that is legal in layer space can still be an
    /// illegal template on the composite — and the refusal says so.
    #[test]
    fn track_mask_region_refuses_a_template_the_layer_scale_pushes_out_of_range() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
            .collect::<BTreeMap<_, _>>();
        let doubled = Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(200))]),
            keyframes: BTreeMap::new(),
        };
        // 50 percent of the layer is a legal mask, and 50 × 2 = 100 percent of
        // the composite is not a legal template.
        let (service, _core) = transform_track_service(
            vec![doubled, tracking_mask_effect([50, 50], [50, 50])],
            frames,
        );

        let result = service.track_mask_region(&mask_tracking_args()).unwrap();
        assert_eq!(result.is_error, Some(true));
        let message = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.clone()))
            .unwrap_or_default();
        assert!(
            message.contains("layer scale 2"),
            "the refusal must name the layer scale: {message}"
        );
        assert!(
            message.contains("100x100 percent template"),
            "the refusal must name the composite template: {message}"
        );
    }

    fn reframe_tracking_args(subject: [u8; 2], initial: [u8; 2]) -> TrackReframeArgs {
        TrackReframeArgs {
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            subject_width_percent: subject[0],
            subject_height_percent: subject[1],
            initial_subject_x_percent: Some(initial[0]),
            initial_subject_y_percent: Some(initial[1]),
            minimum_focus_x_percent: None,
            maximum_focus_x_percent: None,
            minimum_focus_y_percent: None,
            maximum_focus_y_percent: None,
            focus_dead_zone_percent: Some(0),
            maximum_focus_step_percent: Some(25),
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(41)),
            step_frames: Some(10),
            search_radius_percent: Some(5),
            max_width: Some(320),
        }
    }

    /// CC5 §5.2 (a), reframe half: at the identity the planned focus is the
    /// analytic subject centre, read as a fraction of the extent.
    ///
    /// Composite pixel centres 140, 148, 156, 164 and 172 of 320 are
    /// `round((pixel + 0.5) · 10000 / 320)` = 4391, 4641, 4891, 5141, 5391 bp.
    #[test]
    fn track_reframe_subject_writes_layer_space_focus_at_the_identity() {
        let frames = (0..60)
            .map(|frame| {
                (
                    TimeCode(frame),
                    transform_box_frame([140 + frame * 4 / 5, 90], 5),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let (service, core) =
            transform_track_service(vec![tracking_reframe_effect([44, 50])], frames);

        let result = service
            .track_reframe_subject(&reframe_tracking_args([6, 11], [44, 50]))
            .unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
        for (index, expected) in [4_391_i64, 4_641, 4_891, 5_141, 5_391].iter().enumerate() {
            assert!(
                (layer[index] - expected).abs() <= 200,
                "sample {index}: converted {} against the analytic {expected}",
                layer[index]
            );
        }
        // At the identity the composite and the layer reading are the same
        // number, which is why the transformed case below is the real gate.
        assert_eq!(
            observation_values(&structured, "subject_samples", "composite_x_basis_points"),
            layer
        );
        assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 1.0);

        // The planned focus follows the subject: the three-sample median lags a
        // ramp by one inter-sample step, which is 312 bp here.
        let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
        for (index, expected) in layer.iter().enumerate() {
            assert!(
                (focus[index] - expected).abs() <= 700,
                "sample {index}: focus {} against the subject {expected}",
                focus[index]
            );
        }

        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .all(|effect| effect.keyframes.is_empty()),
            "track_reframe_subject commits nothing"
        );
        assert!(
            document.markers.is_empty(),
            "the provenance marker is prepared, not committed"
        );
    }

    /// CC5 §5.2 (b), reframe half: under `scale 50 / x 20 / y 20` the planner is
    /// fed *layer*-space subject centres.
    ///
    /// The fixture draws the subject at composite pixel centres 207, 215, 223,
    /// 231 and 239 of 320, which are composite 6484, 6734, 6984, 7234 and 7484
    /// bp; `u_l = 2·u_c − 0.9` makes them layer 3969, 4469, 4969, 5469 and 5969
    /// bp. Without the conversion the planner would see the composite numbers,
    /// which are 2500 bp away.
    #[test]
    fn track_reframe_subject_converts_the_composite_centre_into_layer_space() {
        let frames = (0..60)
            .map(|frame| {
                (
                    TimeCode(frame),
                    transform_box_frame([207 + frame * 4 / 5, 125], 5),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // Layer 40 percent is composite 0.5·0.40 + 0.45 = 0.65, which seeds at
        // pixel 207 of 320; layer 49 percent is composite 0.695, pixel 125.
        let (service, core) = transform_track_service(
            vec![half_scale_transform(), tracking_reframe_effect([40, 49])],
            frames,
        );

        let result = service
            .track_reframe_subject(&reframe_tracking_args([12, 22], [40, 49]))
            .unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
        for (index, expected) in [3_969_i64, 4_469, 4_969, 5_469, 5_969].iter().enumerate() {
            assert!(
                (layer[index] - expected).abs() <= 200,
                "sample {index}: converted {} against the analytic layer {expected}",
                layer[index]
            );
        }
        // The composite provenance is preserved, and is thousands of basis
        // points away from what was planned.
        let composite =
            observation_values(&structured, "subject_samples", "composite_x_basis_points");
        for (index, expected) in [6_484_i64, 6_734, 6_984, 7_234, 7_484].iter().enumerate() {
            assert!(
                (composite[index] - expected).abs() <= 200,
                "sample {index}: composite {} against the analytic {expected}",
                composite[index]
            );
            // The gap runs 2515 bp at the first sample down to 1515 at the
            // last, because the layer moves twice as far as the composite at
            // scale 0.5. Either end is far outside any tracker error.
            assert!(
                (composite[index] - layer[index]).abs() > 1_400,
                "the two spaces must not coincide, or this test proves nothing"
            );
        }
        // 49.4444 percent of the layer, from composite pixel 125 of 180.
        for value in observation_values(&structured, "subject_samples", "layer_y_basis_points") {
            assert!(
                (value - 4_944).abs() <= 200,
                "vertical layer centre {value} against the analytic 4944"
            );
        }
        // The subject template is rescaled onto the composite: 12 × 0.5 = 6 and
        // 22 × 0.5 = 11 percent.
        assert_eq!(
            structured["coordinate_space"]["box_percent"],
            json!([6, 11])
        );
        assert_eq!(
            structured["coordinate_space"]["seed_center_percent"],
            json!([65, 70])
        );
        assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 0.5);
        assert_eq!(
            structured["coordinate_space"]["samples"][0]["offset_x"],
            0.4
        );

        // The focus is planned in the same space it is written in, so it stays
        // near the layer-space subject and far from the composite reading.
        let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
        for (index, expected) in layer.iter().enumerate() {
            assert!(
                (focus[index] - expected).abs() <= 900,
                "sample {index}: focus {} against the layer subject {expected}",
                focus[index]
            );
        }

        // Nothing is committed: no keyframes, no provenance marker.
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .all(|effect| effect.keyframes.is_empty()),
            "track_reframe_subject commits nothing"
        );
        assert!(document.markers.is_empty());
    }

    /// CC5 §5.2 (d), reframe half: the subject template is rescaled by the layer
    /// scale, and a subject that maps past 75 percent of the composite is
    /// refused with both numbers named.
    #[test]
    fn track_reframe_subject_refuses_a_subject_the_layer_scale_pushes_out_of_range() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
            .collect::<BTreeMap<_, _>>();
        let doubled = Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(200))]),
            keyframes: BTreeMap::new(),
        };
        let (service, _core) =
            transform_track_service(vec![doubled, tracking_reframe_effect([50, 50])], frames);

        let result = service
            .track_reframe_subject(&reframe_tracking_args([60, 60], [50, 50]))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let message = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.clone()))
            .unwrap_or_default();
        assert!(
            message.contains("layer scale 2"),
            "the refusal must name the layer scale: {message}"
        );
        assert!(
            message.contains("120x120 percent template"),
            "the refusal must name the composite template: {message}"
        );
    }

    /// A layer scale that ramps 100 → 200 percent, linearly, over frames 0..=40.
    ///
    /// The twin of [`keyframed_scale_transform`]: it is *legal* at the seed and
    /// illegal at the far end, which is exactly the case a seed-only template
    /// gate lets through.
    fn growing_scale_transform() -> Effect {
        Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::from([(
                "scale_percent".to_owned(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: 100,
                            interpolation: KeyframeInterpolation::Linear,
                        },
                        Keyframe {
                            at: TimeCode(40),
                            value: 200,
                            interpolation: KeyframeInterpolation::Linear,
                        },
                    ],
                },
            )]),
        }
    }

    /// CC5 §5.2: the provenance box is the converted layer centre bracketed by
    /// the *declared* layer subject size — never the composite template, whose
    /// size is pinned to the seed frame's scale.
    ///
    /// Worked at `scale 0.5 / x 20 / y 20`, whose inverse is
    /// `u_layer = 2·u_composite − 0.9`: composite 0.65 is layer 0.40 (4000 bp)
    /// and composite 0.695 is layer 0.49 (4900 bp). A 12 × 22 percent subject
    /// has half extents of 600 and 1100 basis points, so the box is
    /// 3400..4600 horizontally and 3800..6000 vertically — exactly 1200 and
    /// 2200 wide, which is `percent · 100`.
    #[test]
    fn layer_subject_bounds_brackets_the_declared_subject_around_the_layer_centre() {
        let transform = LayerTransform {
            scale: 0.5,
            offset_x: 0.4,
            offset_y: 0.4,
        };
        let centre = transform.composite_to_layer_unit([0.65, 0.695]);
        let bounds = layer_subject_bounds(TimeCode(7), centre, [12, 22]);

        assert_eq!(bounds.at, TimeCode(7));
        assert_eq!(bounds.left_basis_points, 3_400);
        assert_eq!(bounds.right_basis_points, 4_600);
        // The vertical pair takes the *second* declared percent, not the first.
        assert_eq!(bounds.top_basis_points, 3_800);
        assert_eq!(bounds.bottom_basis_points, 6_000);
        assert_eq!(bounds.right_basis_points - bounds.left_basis_points, 1_200);
        assert_eq!(bounds.bottom_basis_points - bounds.top_basis_points, 2_200);

        // The composite template the tracker matched with is *not* the box: at
        // this scale a 12 percent layer subject is a 6 percent composite
        // template, and converting that back would halve the box.
        assert_eq!(tracked_box_percent(12, transform.scale), 6);
    }

    /// CC5 §5.2: a box whose edges do not land on the basis-point grid rounds
    /// **outward**, and a box that leaves the layer is clamped to `0..=10000`.
    #[test]
    fn layer_subject_bounds_rounds_outward_and_clamps_at_the_layer_edges() {
        // Layer centre 3968.5 bp, half extent 600: 3368.5 floors to 3368 and
        // 4568.5 ceils to 4569, so the box is one basis point wider than the
        // declared 1200 and never narrower.
        let bounds = layer_subject_bounds(TimeCode(0), [0.396_85, 0.5], [12, 12]);
        assert_eq!(bounds.left_basis_points, 3_368);
        assert_eq!(bounds.right_basis_points, 4_569);
        assert_eq!(bounds.right_basis_points - bounds.left_basis_points, 1_201);

        // 200 − 600 clamps to 0 and 9800 + 600 clamps to 10000: the crop can
        // only sample layer uv 0..1, so a subject hanging off the layer is
        // recorded up to the edge and no further.
        let clamped = layer_subject_bounds(TimeCode(0), [0.02, 0.98], [12, 12]);
        assert_eq!(clamped.left_basis_points, 0);
        assert_eq!(clamped.right_basis_points, 800);
        assert_eq!(clamped.top_basis_points, 9_200);
        assert_eq!(clamped.bottom_basis_points, 10_000);
    }

    /// CC5 §5.2: under a keyframed scale the provenance box stays the declared
    /// subject size in *layer* basis points at every sample, and the focus
    /// curve tracks the analytic layer centre.
    ///
    /// The regression this pins: bracketing each composite centre with the
    /// *seed-frame* template and converting the corners through the
    /// *per-observation* scale inflates the box by `seed_scale / scale`. Here
    /// the scale ramps 1.0 → 0.5, so the last sample's composite box becomes
    /// more than 6000 bp in layer space against a 5625 bp delivery crop and
    /// `focus_interval_for_subject_axis` refuses with "wider than the delivery
    /// crop", even though the declared subject is 3000 bp wide.
    ///
    /// The subject is drawn at a constant composite size so the frame-to-frame
    /// template match is a pure translation; the fixture's job is to exercise
    /// the conversion, not the matcher. It sits at layer 32 percent, which is
    /// as far off centre as a 30 percent subject can sit while the *pre-fix*
    /// converted box still fits inside 0..=10000 — otherwise the old clamp
    /// hides the inflation instead of refusing.
    ///
    /// Composite pixel centres are 102, 109, 116, 123 and 130 of 320, and
    /// `u_layer = (u_composite − 0.5)/scale + 0.5` at scales 1.0, 0.875, 0.75,
    /// 0.625 and 0.5 makes them layer 3203, 3196, 3188, 3175 and 3156 bp.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn track_reframe_subject_bounds_the_declared_subject_under_a_keyframed_scale() {
        let frames = (0..60)
            .map(|frame| {
                (
                    TimeCode(frame),
                    transform_box_frame([102 + frame * 18 / 25, 90], 5),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let (service, _core) = transform_track_service(
            vec![
                keyframed_scale_transform(),
                tracking_reframe_effect([32, 50]),
            ],
            frames,
        );

        let result = service
            .track_reframe_subject(&reframe_tracking_args([30, 30], [32, 50]))
            .unwrap();
        let message = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.clone()))
            .unwrap_or_default();
        assert!(
            !message.contains("wider than the delivery crop"),
            "the seed-template containment bug is back: {message}"
        );
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        let samples = structured["subject_samples"].as_array().unwrap();
        assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
        for (index, sample) in samples.iter().enumerate() {
            let bounds = &sample["layer_bounds_basis_points"];
            let left = bounds["left"].as_i64().unwrap();
            let right = bounds["right"].as_i64().unwrap();
            let top = bounds["top"].as_i64().unwrap();
            let bottom = bounds["bottom"].as_i64().unwrap();
            // 30 percent of the layer is 3000 basis points, plus at most the
            // one basis point the outward rounding adds when the centre does
            // not land on the grid. Nothing is clamped here: the box sits
            // between 1656 and 4703, well inside 0..=10000.
            assert!(
                (3_000..=3_001).contains(&(right - left)),
                "sample {index}: horizontal box {left}..{right} is not the declared 3000 bp"
            );
            assert!(
                (3_000..=3_001).contains(&(bottom - top)),
                "sample {index}: vertical box {top}..{bottom} is not the declared 3000 bp"
            );
            // The box is centred on the converted layer centre, not on a
            // composite reading.
            let centre = sample["layer_x_basis_points"].as_i64().unwrap();
            assert!(
                (i64::midpoint(left, right) - centre).abs() <= 1,
                "sample {index}: box {left}..{right} is not centred on {centre}"
            );
        }

        // The composite template stays the seed-scale one throughout, and at
        // the last sample converting *its* bounds through that sample's own
        // scale is what used to blow past the 5625 bp delivery crop.
        let last = samples.last().unwrap();
        let composite = &last["composite_bounds_basis_points"];
        let composite_width =
            composite["right"].as_i64().unwrap() - composite["left"].as_i64().unwrap();
        let last_scale = last["layer_transform"]["scale"].as_f64().unwrap();
        assert!((last_scale - 0.5).abs() < 1e-9, "last scale {last_scale}");
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let seed_template_layer_width = (composite_width as f64 / last_scale).round() as i64;
        assert!(
            seed_template_layer_width > 5_625,
            "the pre-fix construction must be out of range, or this test proves nothing: {seed_template_layer_width}"
        );

        // The converted layer centres follow the analytic values. The template
        // is a coarse 30 percent box, so the matcher is subsampled and lags by
        // a couple of composite pixels; 200 bp covers that at every scale here.
        let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
        let composite_centres =
            observation_values(&structured, "subject_samples", "composite_x_basis_points");
        for (index, expected) in [3_203_i64, 3_196, 3_188, 3_175, 3_156].iter().enumerate() {
            assert!(
                (layer[index] - expected).abs() <= 200,
                "sample {index}: converted {} against the analytic layer {expected}",
                layer[index]
            );
        }
        // The composite reading walks away from the layer reading as the layer
        // shrinks: 3203 bp at the seed against 4078 bp at the last sample.
        assert!(
            (composite_centres[4] - layer[4]).abs() > 700,
            "the two spaces must not coincide, or this test proves nothing: {composite_centres:?} against {layer:?}"
        );

        // The focus is planned in the same space, so it stays near the layer
        // subject; the three-sample median lags a ramp by one inter-sample
        // step, which is about 120 bp here.
        let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
        for (index, expected) in layer.iter().enumerate() {
            assert!(
                (focus[index] - expected).abs() <= 700,
                "sample {index}: focus {} against the layer subject {expected}",
                focus[index]
            );
        }

        // The published contract says the template is seed-sized while the
        // conversion is per frame, and names the resolved range.
        let note = structured["coordinate_space"]["keyframed_transform"]
            .as_str()
            .unwrap();
        assert!(note.contains("seed frame's scale 1"), "{note}");
        assert!(note.contains("0.5 at clip-local frame 40"), "{note}");
        assert!(note.contains("1 at clip-local frame 0"), "{note}");
    }

    /// CC5 §5.2: the `1..=75` template gate is applied at the smallest and the
    /// largest resolved scale, so a ramp that is legal at the seed and illegal
    /// at the far end is refused — naming the offending frame and scale.
    #[test]
    fn track_reframe_subject_refuses_a_template_the_largest_sampled_scale_pushes_out_of_range() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
            .collect::<BTreeMap<_, _>>();
        let (service, _core) = transform_track_service(
            vec![growing_scale_transform(), tracking_reframe_effect([50, 50])],
            frames,
        );

        // 50 × 1.0 = 50 percent is a legal template at the seed frame, so a
        // seed-only gate would accept this and then match a 100 percent
        // template at frame 40.
        let result = service
            .track_reframe_subject(&reframe_tracking_args([50, 50], [50, 50]))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let message = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.clone()))
            .unwrap_or_default();
        assert!(
            message.contains("100x100 percent template"),
            "the refusal must name the offending template: {message}"
        );
        assert!(
            message.contains("layer scale 2"),
            "the refusal must name the offending scale: {message}"
        );
        assert!(
            message.contains("clip-local frame 40"),
            "the refusal must name the offending frame: {message}"
        );
    }

    /// The mask half of the same gate: legal at the seed, illegal at frame 40.
    #[test]
    fn track_mask_region_refuses_a_template_the_largest_sampled_scale_pushes_out_of_range() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
            .collect::<BTreeMap<_, _>>();
        let (service, _core) = transform_track_service(
            vec![
                growing_scale_transform(),
                tracking_mask_effect([50, 50], [50, 50]),
            ],
            frames,
        );

        let result = service.track_mask_region(&mask_tracking_args()).unwrap();
        assert_eq!(result.is_error, Some(true));
        let message = result
            .content
            .iter()
            .find_map(|content| content.as_text().map(|text| text.text.clone()))
            .unwrap_or_default();
        assert!(
            message.contains("100x100 percent template"),
            "the refusal must name the offending template: {message}"
        );
        assert!(
            message.contains("layer scale 2 at clip-local frame 40"),
            "the refusal must name the offending frame and scale: {message}"
        );
    }

    /// CC5 §5.2: a seed whose forward map leaves the composited frame names no
    /// pixel, so the tracker refuses typed instead of clamping to the raster
    /// edge and following whatever sits in the corner.
    ///
    /// `scale_percent 100` with `x_percent 100` accumulates `offset_x = 2.0`,
    /// so `u_composite = (0.5 − 0.5)·1 + 2.0/2 + 0.5 = 1.5`.
    #[test]
    fn track_reframe_subject_refuses_a_seed_the_layer_transform_pushes_off_the_composite() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 5)))
            .collect::<BTreeMap<_, _>>();
        let pushed_off = Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([
                ("scale_percent".to_owned(), ParamValue::Integer(100)),
                ("x_percent".to_owned(), ParamValue::Integer(100)),
            ]),
            keyframes: BTreeMap::new(),
        };
        let (service, _core) =
            transform_track_service(vec![pushed_off, tracking_reframe_effect([50, 50])], frames);

        let result = service
            .track_reframe_subject(&reframe_tracking_args([6, 11], [50, 50]))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["code"], "tracking_seed_outside_composite");
        let details = &structured["details"];
        // Only the horizontal axis left the frame, so the refusal names exactly
        // the one repairable argument rather than both or a generic selector.
        assert_eq!(details["field"], json!("initial_subject_x_percent"));
        assert_eq!(details["observed"]["layer_center_unit"], json!([0.5, 0.5]));
        assert_eq!(
            details["observed"]["composite_center_unit"],
            json!([1.5, 0.5])
        );
        assert_eq!(details["observed"]["scale"], 1.0);
        assert_eq!(details["observed"]["offset_x"], 2.0);
        assert_eq!(details["observed"]["clip_local_frame"], 0);
        assert!(
            details["allowed"].as_str().unwrap().contains("0..=1"),
            "{details}"
        );
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("names none"),
            "{details}"
        );
    }

    /// CC5 §5.2: `track_matte_window` refuses an off-composite seed on exactly
    /// the same terms as `track_reframe_subject`, and names the window's own
    /// stored centre — the repairable parameter — rather than `window_index`.
    ///
    /// The fixture window is the neutral one, centred at 5000 bp on both axes.
    /// `scale_percent 100` with `x_percent 100` accumulates `offset_x = 2.0`,
    /// so `u_composite = (0.5 − 0.5)·1 + 2.0/2 + 0.5 = 1.5` horizontally while
    /// the vertical axis stays at 0.5: only the horizontal parameter is named.
    #[test]
    fn track_matte_window_refuses_a_seed_the_layer_transform_pushes_off_the_composite() {
        let pushed_off = Effect {
            id: EffectId(9),
            name: "transform".to_owned(),
            parameters: BTreeMap::from([
                ("scale_percent".to_owned(), ParamValue::Integer(100)),
                ("x_percent".to_owned(), ParamValue::Integer(100)),
            ]),
            keyframes: BTreeMap::new(),
        };
        let frames = BTreeMap::from([
            (TimeCode(0), matte_box_frame([160, 90])),
            (TimeCode(10), matte_box_frame([160, 90])),
        ]);
        let (service, _core) = matte_track_service(frames, BTreeMap::new(), vec![pushed_off]);

        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                window_index: 0,
                start_local_frame: Some(TimeCode(0)),
                end_local_frame: Some(TimeCode(11)),
                step_frames: Some(10),
                search_radius_percent: None,
                max_width: None,
                minimum_confidence_basis_points: None,
            })
            .unwrap();

        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["code"], "tracking_seed_outside_composite");
        let details = &structured["details"];
        // The offending *parameter*, not the selector that reached it.
        assert_eq!(
            details["field"],
            json!("matte_window0_center_x_basis_points")
        );
        // The selector is still available, as context.
        assert_eq!(details["observed"]["window_index"], 0);
        assert_eq!(details["observed"]["layer_center_unit"], json!([0.5, 0.5]));
        assert_eq!(
            details["observed"]["composite_center_unit"],
            json!([1.5, 0.5])
        );
        assert_eq!(details["observed"]["scale"], 1.0);
        assert_eq!(details["observed"]["offset_x"], 2.0);
        assert_eq!(details["observed"]["offset_y"], 0.0);
        assert_eq!(details["observed"]["clip_local_frame"], 0);
        assert!(
            details["allowed"].as_str().unwrap().contains("0..=1"),
            "{details}"
        );
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("names none"),
            "{details}"
        );
    }

    /// The reframe tool writes `focus_x/y_basis_points`, so re-tracking a clip
    /// it already touched must seed from those, not from the coarse percent
    /// twin the compositor itself overrides.
    ///
    /// Mirrors `compositor.rs`'s `ReframeFocusXBasisPoints` arm: an explicitly
    /// stored basis-point focus wins, a missing one falls back to the percent,
    /// and neither leaves the seed centred.
    #[test]
    fn track_reframe_subject_seeds_from_the_stored_focus_basis_points() {
        let frames = (0..60)
            .map(|frame| (TimeCode(frame), transform_box_frame([80, 90], 5)))
            .collect::<BTreeMap<_, _>>();
        let mut reframe = tracking_reframe_effect([50, 50]);
        reframe.parameters.insert(
            "focus_x_basis_points".to_owned(),
            ParamValue::Integer(2_500),
        );
        let (service, _core) = transform_track_service(vec![reframe], frames);

        let mut args = reframe_tracking_args([6, 11], [50, 50]);
        args.initial_subject_x_percent = None;
        args.initial_subject_y_percent = None;
        let result = service.track_reframe_subject(&args).unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        let initial = &structured["subject_template"]["initial_center_percent"];
        assert_eq!(
            initial["x"], 25,
            "the stored 2500 bp focus must seed at 25 percent, not the 50 percent twin"
        );
        // No `focus_y_basis_points` is stored, so the vertical axis falls back
        // to `focus_y_percent`.
        assert_eq!(initial["y"], 50);
        assert_eq!(
            structured["coordinate_space"]["seed_center_percent"],
            json!([25, 50])
        );
    }

    /// A 320 × 180 frame of deterministic per-frame noise.
    ///
    /// Every frame is unlike every other, so a SAD template match has nothing
    /// to lock onto and the confidence gate fires.
    fn matte_noise_frame(frame: i64) -> RgbaImage {
        let (width, height) = (320_u32, 180_u32);
        let seed = u32::try_from(frame.rem_euclid(4_096)).unwrap_or(0);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                // An avalanche mix, so the field has no translational symmetry
                // a shifted template could exploit.
                let mut hash = x.wrapping_mul(0x9E37_79B9)
                    ^ y.wrapping_mul(0x85EB_CA6B)
                    ^ seed.wrapping_mul(0xC2B2_AE35);
                hash ^= hash >> 15;
                hash = hash.wrapping_mul(0x2545_F491);
                hash ^= hash >> 13;
                let value = u8::try_from(hash & 0xFF).unwrap_or(0);
                pixels.extend_from_slice(&[value, value.wrapping_add(83), value, 255]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// CC5 §5.2: track one window on a synthetic moving subject and return a
    /// prepared plan of exactly two keyframe operations, committing nothing.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn track_matte_window_prepares_two_keyframe_operations_without_committing() {
        // The subject travels from x = 80 to x = 240 across frames 0..=40,
        // 4 pixels per frame, at a constant y = 90.
        let frames = (0..=40)
            .map(|frame| {
                (
                    TimeCode(frame),
                    matte_box_frame([u32::try_from(80 + frame * 4).unwrap(), 90]),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // Seed the window on the subject at frame 0: pixel 80 of 320 is 2500
        // basis points of the width, and pixel 90 of 180 is 5000 of the height.
        let (service, core) = matte_track_service(
            frames,
            BTreeMap::from([("matte_window0_center_x_basis_points".to_owned(), 2_500)]),
            Vec::new(),
        );

        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: Some(TimelineRevision(0)),
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                window_index: 0,
                start_local_frame: Some(TimeCode(0)),
                end_local_frame: Some(TimeCode(41)),
                step_frames: Some(10),
                search_radius_percent: Some(25),
                max_width: Some(320),
                minimum_confidence_basis_points: None,
            })
            .unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );

        // Five samples at 0, 10, 20, 30, 40.
        let observations = structured["observations"].as_array().unwrap();
        assert_eq!(observations.len(), 5);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation["local_frame"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40]
        );
        // Raw centres, hand-derived through CC5 §5.2's conversion at scale 1:
        // the subject centre is pixel x = 80 + 4 * frame, and
        // round((pixel + 0.5) * 10000 / 320) gives 2516, 3766, 5016, 6266,
        // 7516. The tracker seeds from the window centre, so the first sample
        // is the seeded position and the rest are matched.
        let raw = observations
            .iter()
            .map(|observation| observation["center_x_basis_points"].as_i64().unwrap())
            .collect::<Vec<_>>();
        for (index, expected) in [2_516_i64, 3_766, 5_016, 6_266, 7_516].iter().enumerate() {
            assert!(
                (raw[index] - expected).abs() <= 200,
                "sample {index}: raw {} against the analytic {expected}",
                raw[index]
            );
        }
        // A static subject on the vertical axis: every raw y stays at the
        // seeded centre, round((89.5 + 0.5) * 10000 / 180) = 5000.
        for observation in observations {
            assert!((observation["center_y_basis_points"].as_i64().unwrap() - 5_000).abs() <= 200);
            assert!(observation["confidence_basis_points"].as_u64().unwrap() >= 5_000);
        }

        // The smoothed curve differs from the raw observations, and the last
        // sample lags by one inter-sample displacement exactly as CC5 §5.2
        // states.
        let smoothed = structured["curves"]["matte_window0_center_x_basis_points"]["keyframes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|keyframe| keyframe["value"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(smoothed.len(), 5);
        assert!(
            smoothed[4] < raw[4],
            "the median filter must lag the final sample: smoothed {} against raw {}",
            smoothed[4],
            raw[4]
        );
        assert!(
            (raw[4] - smoothed[4]) <= 2 * (raw[4] - raw[3]),
            "the lag is bounded by one inter-sample displacement"
        );
        // Every keyframe is Linear: sustained movement gets continuous
        // velocity, and M40 rejected eased per-segment curves.
        for keyframe in structured["curves"]["matte_window0_center_x_basis_points"]["keyframes"]
            .as_array()
            .unwrap()
        {
            assert_eq!(keyframe["interpolation"], "linear");
        }

        // The pinned M40 constants ride in the response.
        let stabilization = &structured["window_stabilization"];
        assert_eq!(stabilization["median_filter"], true);
        assert_eq!(stabilization["dead_zone_basis_points"], 0);
        assert_eq!(stabilization["maximum_step_basis_points"], 800);
        assert_eq!(stabilization["minimum_basis_points"], -10_000);
        assert_eq!(stabilization["maximum_basis_points"], 20_000);
        assert_eq!(stabilization["interpolation"], "Linear");

        // CC5 §5.2's conversion is stated, not inferred.
        let space = &structured["coordinate_space"];
        assert_eq!(
            space["pixel_to_basis_points"],
            "centre_bp = round((pixel + 0.5) * 10000 / extent)"
        );
        assert_eq!(
            space["composite_to_layer"],
            "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5"
        );
        assert_eq!(space["layer_scale"], 1.0);
        // hw = 2500 bp at scale 1 is a 50 percent template.
        assert_eq!(space["box_percent"], json!([50, 50]));

        // CC5 §5.2's provenance marker.
        let boundary = structured["tracking_boundary"].as_str().unwrap();
        assert!(boundary.contains("normalized SAD template match"));
        assert!(boundary.contains("no learned object, face, or skin detection"));
        assert!(boundary.contains("rotation_centidegrees"));

        // The prepared plan carries exactly the two keyframe operations, and
        // neither is destructive.
        let preview = &structured["prepared_edit_plan"]["preview"];
        assert_eq!(preview["operation_count"], 2);
        assert_eq!(preview["destructive_operations"], json!([]));
        assert_eq!(preview["expected_revision"], 0);
        assert_eq!(preview["before_clips"], preview["after_clips"]);
        // The two parameters CC5 §5.2 writes, and no others: rotation and the
        // half extents are never written.
        assert_eq!(
            structured["parameters"],
            json!([
                "matte_window0_center_x_basis_points",
                "matte_window0_center_y_basis_points"
            ])
        );
        assert_eq!(
            structured["curves"].as_object().unwrap().len(),
            2,
            "exactly two curves are proposed"
        );
        assert_eq!(structured["applied"], false);

        // Nothing was committed: the live node still carries no automation.
        let Event::QueryResult(QueryResult::Document(document)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected a document");
        };
        assert!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .all(|effect| effect.keyframes.is_empty()),
            "track_matte_window commits nothing"
        );
    }

    // -----------------------------------------------------------------------
    // CC5 §9.2.11, agent half: the tracked shot.
    //
    // The media crate owns the generated clip and proves containment for a
    // *simulated* smoother; the real `track_matte_window` lives here, so the
    // same containment gate is run against the curve the tool actually emits.
    // The shot is the media crate's recipe, restated because its generator and
    // its analytic helpers are `pub(crate)` to `kinewright-media`.
    // -----------------------------------------------------------------------

    /// The §9.2.11 tracked shot's raster and subject, from the media recipe.
    const TRACKED_SHOT_WIDTH: u32 = 640;
    const TRACKED_SHOT_HEIGHT: u32 = 360;
    const TRACKED_SHOT_FRAMES: i64 = 100;
    const TRACKED_SHOT_FPS: i64 = 25;
    /// The white subject is 80 × 80 pixels.
    const TRACKED_SHOT_BOX: i64 = 80;
    /// §9.2.11's window half extents, in basis points of width and of height.
    const TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS: i64 = 1_300;
    const TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS: i64 = 1_800;
    /// The subject's own half extents in the same units: half of the 80 px box
    /// is `40 / 640 = 625` bp of the width and `40 / 360 = 1111.1` bp of the
    /// height, which is the `625` / `1111` §9.2.11 states. The exact fraction
    /// is used on the vertical axis because it is the stricter of the two.
    const TRACKED_SHOT_SUBJECT_HALF_WIDTH_BASIS_POINTS: f64 = 625.0;
    const TRACKED_SHOT_SUBJECT_HALF_HEIGHT_BASIS_POINTS: f64 = 40.0 * 10_000.0 / 360.0;
    /// §9.2.11's derived margin budget: `1300 − 625` and `1800 − 1111`.
    const TRACKED_SHOT_MARGIN_BUDGET_X_BASIS_POINTS: f64 = 675.0;
    const TRACKED_SHOT_MARGIN_BUDGET_Y_BASIS_POINTS: f64 = 689.0;
    /// §9.2.11's tolerances: the raw observations may miss the analytic centre
    /// by 200 bp, and the smoothed curve — which pays the median filter's lag
    /// on top of that error — by 600 bp.
    const TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS: f64 = 200.0;
    const TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS: f64 = 600.0;
    /// The measured worst `|raw − analytic|` of the real tracker on this shot,
    /// in basis points, at samples 9 (x) and 19 (y).
    const TRACKED_SHOT_WORST_RAW_X_BASIS_POINTS: f64 = 55.0;
    const TRACKED_SHOT_WORST_RAW_Y_BASIS_POINTS: f64 = 180.888_888_888_888_7;
    /// The measured worst `|smoothed − analytic|`, both at sample 99.
    const TRACKED_SHOT_WORST_SMOOTHED_X_BASIS_POINTS: f64 = 366.75;
    const TRACKED_SHOT_WORST_SMOOTHED_Y_BASIS_POINTS: f64 = 292.0;
    /// The measured worst containment margin over all 100 frames, both at
    /// frame 99: `675 − 366.75` and `688.9 − 292`.
    const TRACKED_SHOT_WORST_MARGIN_X_BASIS_POINTS: f64 = 308.25;
    const TRACKED_SHOT_WORST_MARGIN_Y_BASIS_POINTS: f64 = 396.888_888_888_888_7;

    /// The analytic top-left corner of the subject at clip-local `frame`.
    ///
    /// The media crate generates the shot with
    /// `overlay=x='320+120*sin(2*PI*t/8)-40':y='180+60*sin(2*PI*t/8)-40'` over
    /// a solid `0x303030` 640 × 360 background at 25 fps. `overlay` exposes
    /// `t` as *time*, so `t = frame / 25`, and the realised box snaps to even
    /// pixel offsets because that clip is muxed `yuv420p`; the expectation is
    /// therefore `2·floor(edge / 2)`. Restated rather than imported:
    /// `cc5_fixtures.rs::analytic_box_corner` is `pub(crate)` to the media
    /// crate and cannot be called from here.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn tracked_shot_box_corner(frame: i64) -> (i64, i64) {
        let seconds = frame as f64 / TRACKED_SHOT_FPS as f64;
        let phase = (2.0 * std::f64::consts::PI * seconds / 8.0).sin();
        let snap = |value: f64| 2 * (value / 2.0).floor() as i64;
        (
            snap(320.0 + 120.0 * phase - 40.0),
            snap(180.0 + 60.0 * phase - 40.0),
        )
    }

    /// The exact window centre, in basis points, that centres the analytic box
    /// in the window at `frame`. Kept fractional: rounding it to the integer
    /// the tool emits would hide up to half a basis point of the error this
    /// test measures.
    #[allow(clippy::cast_precision_loss)]
    fn tracked_shot_centre_basis_points(frame: i64) -> [f64; 2] {
        let (x, y) = tracked_shot_box_corner(frame);
        [
            (x + TRACKED_SHOT_BOX / 2) as f64 * 10_000.0 / f64::from(TRACKED_SHOT_WIDTH),
            (y + TRACKED_SHOT_BOX / 2) as f64 * 10_000.0 / f64::from(TRACKED_SHOT_HEIGHT),
        ]
    }

    /// One frame of the tracked shot as an RGBA thumbnail.
    ///
    /// The background is solid on purpose: §5.2's box rule makes the SAD
    /// template *window* sized rather than subject sized, and a featureless
    /// background is what pins the match on the subject instead of on some
    /// other piece of texture inside the template.
    fn tracked_shot_frame(frame: i64) -> RgbaImage {
        let (width, height) = (TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT);
        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
            *pixel = [0x30, 0x30, 0x30, 255];
        }
        let (left, top) = tracked_shot_box_corner(frame);
        for y in top..top + TRACKED_SHOT_BOX {
            for x in left..left + TRACKED_SHOT_BOX {
                let y = u32::try_from(y).expect("the subject stays inside the raster");
                let x = u32::try_from(x).expect("the subject stays inside the raster");
                let index = ((y * width + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// A service whose only clip is the §9.2.11 tracked shot — 100 frames of
    /// 640 × 360 at 25 fps — carrying one matted `color_wheels` node whose
    /// window 0 is the contract's 1300 × 1800 bp rect, and whose analysis
    /// backend answers thumbnails with the generated frames.
    ///
    /// The window centre is left at its neutral 5000 / 5000, which *is* the
    /// analytic centre at frame 0 — the tracker seeds from the stored centre,
    /// so seeding it anywhere else would inject an error the shot does not
    /// have. The clip starts at timeline frame 0, so clip-local and project
    /// frames coincide and the thumbnail map is keyed by either.
    fn tracked_shot_service() -> (KinewrightMcp, Core) {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("cc5-tracked-shot.mp4"),
            name: "cc5-tracked-shot".to_owned(),
            duration: TimeCode(TRACKED_SHOT_FRAMES),
            fps: Rational::new(u32::try_from(TRACKED_SHOT_FPS).unwrap(), 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: ColorDescription::default(),
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
                    source_range: TimeCode::ZERO..TimeCode(TRACKED_SHOT_FRAMES),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: vec![Effect {
                        id: EffectId(1),
                        name: "color_wheels".to_owned(),
                        parameters: BTreeMap::from([
                            ("matte_enabled".to_owned(), ParamValue::Integer(1)),
                            ("matte_window_count".to_owned(), ParamValue::Integer(1)),
                            (
                                "matte_window0_half_width_basis_points".to_owned(),
                                ParamValue::Integer(TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS),
                            ),
                            (
                                "matte_window0_half_height_basis_points".to_owned(),
                                ParamValue::Integer(TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS),
                            ),
                        ]),
                        keyframes: BTreeMap::new(),
                    }],
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
            fps: Rational::new(u32::try_from(TRACKED_SHOT_FPS).unwrap(), 1).unwrap(),
            resolution: (TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT),
            duration: TimeCode(TRACKED_SHOT_FRAMES),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
        };
        let media = Arc::new(NoopMedia {
            thumbnail_frames: (0..TRACKED_SHOT_FRAMES)
                .map(|frame| (TimeCode(frame), tracked_shot_frame(frame)))
                .collect(),
            ..NoopMedia::default()
        });
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media;
        let core = Core::spawn(document).unwrap();
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            analysis,
            ConfirmationBroker::default(),
        );
        (service, core)
    }

    /// CC5 §9.2.11, agent half. The *smoothed* curve `track_matte_window`
    /// prepares, linearly interpolated between its sample keyframes, keeps the
    /// analytic subject box inside the 1300 × 1800 bp window at **every**
    /// frame `0..=99` — not only at the 21 frames the tracker sampled.
    ///
    /// The media crate's
    /// `cc5_tracked_shot_window_contains_the_subject_at_every_frame` runs this
    /// gate against ground truth and against a *simulated* smoother; the real
    /// tool lives here, so this runs it against the curve the tool actually
    /// emitted for the same shot. Interpolation is
    /// [`AutomationCurve::value_at`], which is precisely the evaluator
    /// `Effect::evaluated_at` calls and therefore precisely the rule the media
    /// gate uses — a hand-rolled lerp here could agree with the contract and
    /// disagree with the timeline.
    #[test]
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn track_matte_window_smoothed_curve_contains_the_subject_at_every_frame() {
        let (service, _core) = tracked_shot_service();

        // The media recipe's tracking call: step 5, radius 25, max_width 512.
        // The analysis double answers thumbnails at the frames' own raster, so
        // the tracker measures 640 × 360 and `max_width` only records the
        // recipe.
        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: Some(TimelineRevision(0)),
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                window_index: 0,
                start_local_frame: Some(TimeCode(0)),
                end_local_frame: Some(TimeCode(TRACKED_SHOT_FRAMES)),
                step_frames: Some(5),
                search_radius_percent: Some(25),
                max_width: Some(512),
                minimum_confidence_basis_points: None,
            })
            .unwrap();
        let structured = result.structured_content.clone().unwrap();
        assert_ne!(
            result.is_error,
            Some(true),
            "tracking refused: {structured}"
        );
        // The window really is the contract's: `2 · hw · scale · 100` is 26
        // percent of the width and 36 percent of the height at scale 1.
        assert_eq!(
            structured["coordinate_space"]["box_percent"],
            json!([26, 36])
        );
        assert_eq!(
            structured["coordinate_space"]["thumbnail"],
            json!({"width": TRACKED_SHOT_WIDTH, "height": TRACKED_SHOT_HEIGHT})
        );

        // `tracking_sample_frames(0..100, 5)` distributes 20 even intervals
        // across the 99-frame span: 0, 4, 9, …, 94, 99. Not multiples of five,
        // and the media fixture's sequence exactly.
        let observations = structured["observations"].as_array().unwrap();
        let sample_frames = observations
            .iter()
            .map(|observation| observation["local_frame"].as_i64().unwrap())
            .collect::<Vec<_>>();
        let expected_samples = std::iter::once(0)
            .chain((4..TRACKED_SHOT_FRAMES).step_by(5))
            .collect::<Vec<_>>();
        assert_eq!(expected_samples.len(), 21);
        assert_eq!(*expected_samples.last().unwrap(), 99);
        assert_eq!(
            sample_frames, expected_samples,
            "every sample must survive the confidence floor, at the media fixture's own frames"
        );

        // --- §9.2.11: the raw observations stay within 200 bp --------------
        let mut worst_raw = [0.0_f64; 2];
        let mut worst_raw_frame = [0_i64; 2];
        for observation in observations {
            let frame = observation["local_frame"].as_i64().unwrap();
            let analytic = tracked_shot_centre_basis_points(frame);
            for (axis, name) in [
                (0_usize, "center_x_basis_points"),
                (1, "center_y_basis_points"),
            ] {
                let observed = observation[name].as_i64().unwrap() as f64;
                let error = (observed - analytic[axis]).abs();
                assert!(
                    error <= TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS,
                    "frame {frame}, axis {axis}: the raw observation {observed} bp misses the \
                     analytic {} bp by {error} bp, past §9.2.11's {} bp raw tolerance",
                    analytic[axis],
                    TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS
                );
                if error > worst_raw[axis] {
                    worst_raw[axis] = error;
                    worst_raw_frame[axis] = frame;
                }
            }
            assert!(observation["confidence_basis_points"].as_u64().unwrap() >= 5_000);
        }

        // --- the smoothed curves the tool prepared -------------------------
        let curve_for = |axis: usize| {
            let name = if axis == 0 {
                "matte_window0_center_x_basis_points"
            } else {
                "matte_window0_center_y_basis_points"
            };
            serde_json::from_value::<AutomationCurve>(structured["curves"][name].clone())
                .expect("the tool publishes ordinary automation curves")
        };
        let curves = [curve_for(0), curve_for(1)];
        let mut worst_smoothed = [0.0_f64; 2];
        let mut worst_smoothed_frame = [0_i64; 2];
        for (axis, curve) in curves.iter().enumerate() {
            curve.validate().expect("a valid curve");
            assert_eq!(
                curve
                    .keyframes
                    .iter()
                    .map(|keyframe| keyframe.at.0)
                    .collect::<Vec<_>>(),
                expected_samples,
                "axis {axis}: one keyframe per surviving sample"
            );
            for keyframe in &curve.keyframes {
                assert_eq!(keyframe.interpolation, KeyframeInterpolation::Linear);
                let analytic = tracked_shot_centre_basis_points(keyframe.at.0);
                let error = (keyframe.value as f64 - analytic[axis]).abs();
                assert!(
                    error <= TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS,
                    "frame {}, axis {axis}: the smoothed centre {} bp misses the analytic {} bp \
                     by {error} bp, past §9.2.11's {} bp smoothed tolerance",
                    keyframe.at.0,
                    keyframe.value,
                    analytic[axis],
                    TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS
                );
                if error > worst_smoothed[axis] {
                    worst_smoothed[axis] = error;
                    worst_smoothed_frame[axis] = keyframe.at.0;
                }
            }
        }
        // The smoother is not a pass-through: it costs lag, and the lag is
        // what the margin budget below is spent on.
        assert!(
            worst_smoothed[0] > 0.0 && worst_smoothed[1] > 0.0,
            "a smoothed curve identical to ground truth would not exercise the margin budget"
        );

        // --- §9.2.11: containment at EVERY frame, not only the samples -----
        //
        // The window is `[cx ± 1300, cy ± 1800]` and the subject box is
        // `[analytic ± (625, 1111.1)]`, both in basis points of the frame
        // extent, so the margin on each axis collapses to
        // `half_extent − subject_half_extent − |centre error|`. The four edge
        // comparisons are written out anyway: containment is the assertion the
        // contract makes, and the margin is the evidence.
        let half_extent = [
            TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS as f64,
            TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS as f64,
        ];
        let subject_half_extent = [
            TRACKED_SHOT_SUBJECT_HALF_WIDTH_BASIS_POINTS,
            TRACKED_SHOT_SUBJECT_HALF_HEIGHT_BASIS_POINTS,
        ];
        let budget = [
            TRACKED_SHOT_MARGIN_BUDGET_X_BASIS_POINTS,
            TRACKED_SHOT_MARGIN_BUDGET_Y_BASIS_POINTS,
        ];
        let mut worst_margin = [f64::INFINITY; 2];
        let mut worst_margin_frame = [0_i64; 2];
        let mut frames_asserted = 0_i64;
        for frame in 0..TRACKED_SHOT_FRAMES {
            let analytic = tracked_shot_centre_basis_points(frame);
            for axis in 0..2 {
                let centre = curves[axis]
                    .value_at(TimeCode(frame))
                    .expect("the curve covers the whole clip") as f64;
                let window = [centre - half_extent[axis], centre + half_extent[axis]];
                let subject = [
                    analytic[axis] - subject_half_extent[axis],
                    analytic[axis] + subject_half_extent[axis],
                ];
                assert!(
                    subject[0] >= window[0] && subject[1] <= window[1],
                    "frame {frame}, axis {axis}: the subject {subject:?} leaves the tracked \
                     window {window:?}"
                );
                let margin = (subject[0] - window[0]).min(window[1] - subject[1]);
                if margin < worst_margin[axis] {
                    worst_margin[axis] = margin;
                    worst_margin_frame[axis] = frame;
                }
            }
            frames_asserted += 1;
        }
        assert_eq!(
            frames_asserted, TRACKED_SHOT_FRAMES,
            "containment is asserted at every frame, interpolated between the 21 samples"
        );
        for axis in 0..2 {
            assert!(
                worst_margin[axis] > 0.0 && worst_margin[axis] <= budget[axis],
                "axis {axis}: the measured worst margin {} bp at frame {} must be positive and \
                 inside §9.2.11's {} bp budget",
                worst_margin[axis],
                worst_margin_frame[axis],
                budget[axis]
            );
        }
        // The measured evidence, pinned. Every number below is a measurement
        // of the real tool on the real shot rather than arithmetic on the
        // contract's constants, so a regression in the tracker or in the
        // smoother moves it. The tracker is integer SAD over synthetic frames
        // and the curve evaluator is integer, so the run is exactly
        // reproducible and an exact comparison is honest.
        //
        // Both smoothed peaks and both margin minima land on frame 99, which
        // is §5.2's stated last-sample median substitution: the filter
        // replaces `o[n-1]` with `median(o[n-3], o[n-2], o[n-1])`, so the last
        // value lags a moving subject and spends the most margin.
        for (label, measured, recorded, frame, expected_frame) in [
            (
                "raw_x",
                worst_raw[0],
                TRACKED_SHOT_WORST_RAW_X_BASIS_POINTS,
                worst_raw_frame[0],
                9,
            ),
            (
                "raw_y",
                worst_raw[1],
                TRACKED_SHOT_WORST_RAW_Y_BASIS_POINTS,
                worst_raw_frame[1],
                19,
            ),
            (
                "smoothed_x",
                worst_smoothed[0],
                TRACKED_SHOT_WORST_SMOOTHED_X_BASIS_POINTS,
                worst_smoothed_frame[0],
                99,
            ),
            (
                "smoothed_y",
                worst_smoothed[1],
                TRACKED_SHOT_WORST_SMOOTHED_Y_BASIS_POINTS,
                worst_smoothed_frame[1],
                99,
            ),
            (
                "margin_x",
                worst_margin[0],
                TRACKED_SHOT_WORST_MARGIN_X_BASIS_POINTS,
                worst_margin_frame[0],
                99,
            ),
            (
                "margin_y",
                worst_margin[1],
                TRACKED_SHOT_WORST_MARGIN_Y_BASIS_POINTS,
                worst_margin_frame[1],
                99,
            ),
        ] {
            assert!(
                (measured - recorded).abs() <= 1.0e-6,
                "{label}: the measured value is {measured} bp, not the recorded {recorded} bp"
            );
            assert_eq!(
                frame, expected_frame,
                "{label}: the worst frame moved from {expected_frame} to {frame}"
            );
        }
        // The margin and the lag are one measurement seen twice, not two
        // independent literals: the sample frame carrying the worst lag is one
        // of the hundred frames checked above, so the worst margin can never
        // exceed the budget less that lag. Here the two are equal to the bp,
        // because the worst lag falls on sample frame 99 rather than on an
        // interpolated frame between two samples.
        for axis in 0..2 {
            assert!(
                worst_margin[axis] <= budget[axis] - worst_smoothed[axis] + 1.0e-6,
                "axis {axis}: a worst margin of {} bp is larger than the {} bp budget less the \
                 {} bp worst lag, which no frame can be",
                worst_margin[axis],
                budget[axis],
                worst_smoothed[axis]
            );
        }
    }

    /// CC5 §5.2: fewer than two samples above the confidence floor is the
    /// roadmap's manual fallback, reported typed with field/observed/allowed.
    #[test]
    fn track_matte_window_refuses_when_confidence_is_too_low() {
        // Every frame carries a completely different deterministic pattern, so
        // no template matches its successor and the confidence floor rejects
        // every sample after the seeded first one. This is the shape of a real
        // failure: the tracker has no occlusion handling, so a subject that
        // vanishes leaves nothing to match.
        let frames = (0..=40)
            .map(|frame| (TimeCode(frame), matte_noise_frame(frame)))
            .collect::<BTreeMap<_, _>>();
        let (service, _core) = matte_track_service(frames, BTreeMap::new(), Vec::new());

        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                window_index: 0,
                start_local_frame: Some(TimeCode(0)),
                end_local_frame: Some(TimeCode(41)),
                step_frames: Some(10),
                search_radius_percent: Some(25),
                max_width: Some(320),
                // Only a perfect match survives, which the seeded first sample
                // alone reports.
                minimum_confidence_basis_points: Some(10_000),
            })
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(
            result.is_error,
            Some(true),
            "expected a refusal: {structured}"
        );
        assert_eq!(structured["code"], "tracking_confidence_too_low");
        let details = &structured["details"];
        assert_eq!(details["field"], "minimum_confidence_basis_points");
        assert_eq!(
            details["observed"]["minimum_confidence_basis_points"],
            10_000
        );
        assert_eq!(details["allowed"], json!({"minimum_surviving_samples": 2}));
        assert!(details["observed"]["surviving_samples"].as_u64().unwrap() < 2);
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("will not invent a position")
        );
    }

    /// CC5 §5.2: the composite → layer conversion is a single affine map, so a
    /// layer whose transform moves across the range is a typed refusal.
    #[test]
    fn track_matte_window_refuses_a_keyframed_layer_transform() {
        let mut transform = Effect {
            id: EffectId(2),
            name: "transform".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        };
        transform.keyframes.insert(
            "scale_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 50,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(40),
                        value: 100,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let frames = BTreeMap::from([(TimeCode(0), matte_box_frame([160, 90]))]);
        let (service, _core) = matte_track_service(frames, BTreeMap::new(), vec![transform]);

        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                window_index: 0,
                start_local_frame: Some(TimeCode(0)),
                end_local_frame: Some(TimeCode(41)),
                step_frames: Some(10),
                search_radius_percent: None,
                max_width: None,
                minimum_confidence_basis_points: None,
            })
            .unwrap();

        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(
            structured["code"],
            "matte_track_layer_transform_unsupported"
        );
        let details = &structured["details"];
        assert_eq!(details["field"], "scale");
        assert_eq!(details["observed"]["at_first_sample"], 0.5);
        assert!(
            details["allowed"]
                .as_str()
                .unwrap()
                .contains("one value across the whole tracked range")
        );
        // LOW C: the window is tracked with one fixed-size template, so the
        // contract asks for a static transform to keep the window
        // reproducible. It is *not* that a per-frame conversion is impossible
        // — `track_mask_region` and `track_reframe_subject` both do one.
        let recovery = details["recovery_action"].as_str().unwrap();
        assert!(
            recovery.contains("one template of one fixed size"),
            "the rationale must name the fixed template: {recovery}"
        );
        assert!(
            recovery.contains("reproducible"),
            "the rationale must name reproducibility: {recovery}"
        );
        assert!(
            !recovery.contains("single affine map"),
            "the false rationale must be gone: {recovery}"
        );
    }

    /// CC5 §2.2: a window at index >= `matte_window_count` is stored but never
    /// rendered, so tracking it would animate geometry that affects no pixel.
    #[test]
    fn track_matte_window_refuses_a_window_past_the_active_count() {
        let frames = BTreeMap::from([(TimeCode(0), matte_box_frame([160, 90]))]);
        let (service, _core) = matte_track_service(frames, BTreeMap::new(), Vec::new());

        let result = service
            .track_matte_window(&TrackMatteWindowArgs {
                expected_revision: None,
                clip_id: ClipId(1),
                effect_id: EffectId(1),
                // The fixture node resolves `matte_window_count = 1`.
                window_index: 2,
                start_local_frame: None,
                end_local_frame: None,
                step_frames: None,
                search_radius_percent: None,
                max_width: None,
                minimum_confidence_basis_points: None,
            })
            .unwrap();

        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "matte_window_not_active");
        assert_eq!(structured["details"]["field"], "window_index");
        assert_eq!(structured["details"]["observed"], 2);
        assert_eq!(structured["details"]["allowed"]["window_count"], 1);
    }

    /// CC5 §5.2: `excluded_effect` narrows the tracker's exclusion from *every*
    /// effect sharing a name to exactly the one being tracked.
    ///
    /// Two `mask` effects on one clip: tracking the first must leave the
    /// second's alpha in the tracking thumbnails, which is the correct
    /// behaviour and the delta CC5 §9.2.12 asserts.
    #[test]
    fn region_tracking_excludes_exactly_one_effect_by_id() {
        let frames = BTreeMap::from([
            (TimeCode(0), matte_box_frame([160, 90])),
            (TimeCode(10), matte_box_frame([160, 90])),
        ]);
        let masks = vec![
            Effect {
                id: EffectId(7),
                name: "mask".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
            Effect {
                id: EffectId(8),
                name: "mask".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
        ];
        let (service, _core) = matte_track_service(frames, BTreeMap::new(), masks);
        let (_, document) = service.snapshot().unwrap();

        let request = |excluded: EffectId| RegionTrackingRequest {
            document: &document,
            clip_id: ClipId(1),
            clip_timeline_start: TimeCode::ZERO,
            sample_frames: &[TimeCode(0), TimeCode(10)],
            center_percent: [50, 50],
            box_percent: [25, 25],
            search_radius_percent: 25,
            max_width: 320,
            excluded_effect: excluded,
        };
        // Both calls succeed; the point is that the *identity* selects which
        // effect is removed, so a second effect of the same name survives.
        assert!(service.track_clip_region(&request(EffectId(7))).is_ok());
        assert!(service.track_clip_region(&request(EffectId(8))).is_ok());
        // The document itself is never touched by tracking isolation.
        assert_eq!(
            document
                .clip(ClipId(1))
                .unwrap()
                .effects
                .iter()
                .filter(|effect| effect.name == "mask")
                .count(),
            2
        );
    }

    /// CC5 §2.6: a qualifier band whose low edge resolved above its high edge
    /// selects nothing, and `get_qa_report` surfaces Core's
    /// `matte_band_inverted_by_automation` issue for it.
    #[test]
    fn qa_report_surfaces_an_inverted_matte_band() {
        let (service, _core) = matte_service_with(
            None,
            BTreeMap::from([
                ("matte_qualifier_enabled".to_owned(), 1),
                ("matte_saturation_low_basis_points".to_owned(), 9_000),
                ("matte_saturation_high_basis_points".to_owned(), 1_000),
            ]),
            Vec::new(),
        );

        let report = service.qa_report().unwrap();
        let text = report.content[0].as_text().unwrap().text.clone();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        let issue = report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|issue| issue["code"] == "matte_band_inverted_by_automation")
            .unwrap_or_else(|| panic!("the inverted band must be reported: {report}"));
        assert!(
            issue["message"]
                .as_str()
                .unwrap()
                .contains("selects nothing")
        );

        // A band that is not inverted produces no issue at all.
        let (clean, _core) = matte_service_with(
            None,
            BTreeMap::from([
                ("matte_qualifier_enabled".to_owned(), 1),
                ("matte_saturation_low_basis_points".to_owned(), 1_000),
                ("matte_saturation_high_basis_points".to_owned(), 9_000),
            ]),
            Vec::new(),
        );
        let text = clean.qa_report().unwrap().content[0]
            .as_text()
            .unwrap()
            .text
            .clone();
        let clean_report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            !clean_report["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["code"] == "matte_band_inverted_by_automation")
        );
    }

    /// CC5 §7: the three matte tools are registered, read-only, and
    /// `inspect_grade_matte` is an Inspector by explicit override because the
    /// `inspect_` prefix matches no inference rule.
    ///
    /// CC6 §7 extends the same registry bookkeeping to `get_color_qc`, which
    /// needs **no** `CAPABILITY_KIND_OVERRIDES` entry because `get_` already
    /// infers `Inspector` — asserted below so the omission is a decision.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc5_matte_tools_are_registered_read_only_inspectors() {
        let tools = KinewrightMcp::tools().unwrap();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<BTreeSet<_>>();
        for name in [
            "inspect_grade_matte",
            "track_matte_window",
            "plan_secondary_correction",
            "get_color_qc",
        ] {
            assert!(names.contains(name), "missing CC5 tool {name}");
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            assert_eq!(
                tool.annotations.as_ref().unwrap().read_only_hint,
                Some(true),
                "{name} must be read-only"
            );
        }
        assert_eq!(crate::schema::INSPECTOR_TOOL_NAMES.len(), 75);

        // M36: every colour planner and every CC5 tool stays inside the
        // kilobyte description budget, measured on the *registered* descriptor
        // rather than on a copy of the literal, so a descriptor-derived legend
        // that grows is caught here. `plan_secondary_correction` carries a
        // pointer to the matte legend, not the legend itself; the four other
        // planners carry only `matte_parameter_pointer`.
        for name in [
            "plan_primary_correction",
            "plan_color_wheels",
            "plan_color_curves",
            "plan_creative_look",
            "plan_technical_lut",
            "plan_secondary_correction",
            "inspect_grade_matte",
            "track_matte_window",
            "track_mask_region",
            "track_reframe_subject",
            "get_color_qc",
        ] {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                description.len() < 1_024,
                "{name} description is {} bytes, over the M36 1 KB budget",
                description.len()
            );
        }
        // The matte's own planner must not repeat the 47-parameter legend, and
        // must not recommend itself.
        let secondary = tools
            .iter()
            .find(|tool| tool.name == "plan_secondary_correction")
            .unwrap();
        let secondary = secondary.description.as_deref().unwrap_or_default();
        assert!(
            !secondary.contains("matte_window{j}_*"),
            "plan_secondary_correction must not carry the full matte legend"
        );
        assert!(
            !secondary.contains("Prefer plan_secondary_correction"),
            "plan_secondary_correction must not recommend itself"
        );
        assert!(secondary.contains("details.matte_parameters"));
        // The legend itself is still served, in full, by the two enumerating
        // surfaces the pointer names.
        for name in ["add_effect", "set_effect_param"] {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            assert!(
                tool.description
                    .as_deref()
                    .unwrap_or_default()
                    .contains("matte_window{j}_*"),
                "{name} must still enumerate the matte legend"
            );
        }

        let capabilities = crate::runtime::capabilities(&tools);
        let kind = |name: &str| {
            capabilities
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("{name} must be a capability"))
                .kind
        };
        assert_eq!(
            kind("inspect_grade_matte"),
            crate::runtime::CapabilityKind::Inspector
        );
        // These two are inferred correctly by their name prefixes and need no
        // override entry.
        assert_eq!(
            kind("track_matte_window"),
            crate::runtime::CapabilityKind::Inspector
        );
        assert_eq!(
            kind("plan_secondary_correction"),
            crate::runtime::CapabilityKind::Planner
        );
        // CC6 §7: `get_` infers Inspector with no override entry.
        assert_eq!(
            kind("get_color_qc"),
            crate::runtime::CapabilityKind::Inspector
        );
        let color_qc = tools
            .iter()
            .find(|tool| tool.name == "get_color_qc")
            .unwrap();
        let annotations = color_qc.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        // CC6 R13: a working-stage measurement is full-resolution or refused,
        // so the tool must not offer a resolution knob of any spelling.
        let schema = serde_json::to_value(color_qc.input_schema.as_ref()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        for absent in ["resolution", "proxy_sampling", "max_width"] {
            assert!(
                !properties.contains_key(absent),
                "get_color_qc must not carry {absent}"
            );
        }
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
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
            "get_color_context",
            "render_color_proof",
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

        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let profiles = service.delivery_profiles().unwrap();
        let profiles = profiles.structured_content.unwrap();
        let source_master = profiles["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|profile| profile["id"] == "source_master")
            .unwrap();
        assert_eq!(source_master["delivery_color"]["primaries"], "bt709");
        assert_eq!(source_master["delivery_color"]["matrix"], "bt709");
        assert_eq!(source_master["delivery_color"]["range"], "limited");

        let conformance = service
            .delivery_conformance(&DeliveryConformanceArgs {
                profile: DeliveryProfile::SourceMaster,
                focus_x_percent: 50,
                focus_y_percent: 50,
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            })
            .unwrap();
        let conformance = conformance.structured_content.unwrap();
        assert_eq!(
            conformance["delivery_color"],
            source_master["delivery_color"]
        );
        assert_eq!(
            conformance["report"]["delivery_color"],
            source_master["delivery_color"]
        );
    }

    #[test]
    fn color_context_is_a_read_only_internal_capability_with_revisioned_source_data() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        assert!(registry.iter().any(|tool| tool.name == "get_color_context"));
        assert!(
            registry
                .iter()
                .any(|tool| tool.name == "plan_primary_correction")
        );
        let served = KinewrightMcp::served_tools().unwrap();
        assert!(served.iter().all(|tool| tool.name != "get_color_context"));

        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let before = service.snapshot().unwrap();
        let result = service
            .call_blocking(CallToolRequestParams::new("get_color_context"))
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let value = result.structured_content.unwrap();
        assert_eq!(value["timeline_revision"], 0);
        assert_eq!(value["color_context"]["working"]["primaries"], "bt709");
        assert_eq!(value["color_context"]["working"]["matrix"], "rgb");
        assert_eq!(
            value["color_context"]["working"]["confidence_basis_points"],
            10_000
        );
        assert_eq!(
            value["color_context"]["working"]["provenance"],
            "application_default"
        );
        assert_eq!(value["color_context"]["monitoring"]["range"], "full");
        assert_eq!(value["color_context"]["delivery"]["range"], "limited");
        assert_eq!(value["assets"].as_array().unwrap().len(), 1);
        assert_eq!(value["assets"][0]["id"], 1);
        assert_eq!(
            value["assets"][0]["source"]["raw_description"]["primaries"],
            "unknown"
        );
        assert_eq!(
            value["assets"][0]["source"]["raw_description"]["confidence_basis_points"],
            0
        );
        assert_eq!(
            value["assets"][0]["source"]["raw_description"]["provenance"],
            "unknown"
        );
        assert_eq!(
            value["assets"][0]["source"]["status"]["status"],
            "needs_color_override"
        );
        assert_eq!(value["assets"][0]["managed_blocking"], true);
        assert_eq!(service.snapshot().unwrap(), before);

        let invoked = service
            .call_blocking(
                CallToolRequestParams::new("invoke_capability").with_arguments(
                    json!({"name": "get_color_context", "arguments": {}})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .unwrap();
        assert_eq!(invoked.is_error, Some(false));
        assert_eq!(invoked.structured_content.unwrap()["timeline_revision"], 0);
    }

    /// CC3 §8: both planners join the internal registry as read-only planner
    /// capabilities, stay off the seven-tool served surface, and return exact
    /// unapplied operations through the compact dispatcher.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc3_planners_are_read_only_internal_planner_capabilities() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        let served = KinewrightMcp::served_tools().unwrap();
        let catalog = capabilities(&registry);
        for name in ["plan_color_wheels", "plan_color_curves"] {
            let tool = registry
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} must be an internal capability"));
            assert_eq!(
                tool.annotations
                    .as_ref()
                    .and_then(|annotations| annotations.read_only_hint),
                Some(true),
                "{name} is evidence-only"
            );
            assert!(
                served.iter().all(|tool| tool.name != name),
                "{name} must not enlarge the served surface"
            );
            let capability = catalog
                .iter()
                .find(|capability| capability.name == name)
                .unwrap();
            assert_eq!(capability.kind, CapabilityKind::Planner);
            assert!(is_invocable_capability(name));
        }

        let (seed_core, playback, analysis) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        let core = Core::spawn(document).unwrap();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let before = service.snapshot().unwrap();

        let invoke = |name: &str, arguments: serde_json::Value| {
            service
                .call_blocking(
                    CallToolRequestParams::new("invoke_capability").with_arguments(
                        json!({"name": name, "arguments": arguments})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .unwrap()
        };

        let wheels = invoke(
            "plan_color_wheels",
            json!({
                "expected_revision": 0,
                "clip_id": 1,
                "parameters": {"gain_red_thousandths": 1_200}
            }),
        );
        assert_eq!(wheels.is_error, Some(false));
        let wheels = wheels.structured_content.unwrap();
        assert_eq!(wheels["applied"], false);
        assert_eq!(wheels["evidence_only"], true);
        assert_eq!(wheels["kind"], "color_wheels");
        assert_eq!(wheels["created_new_node"], true);
        assert_eq!(wheels["existing_color_node_count"], 0);
        assert_eq!(wheels["resolved_parameters"]["gain_red_thousandths"], 1_200);
        assert_eq!(
            wheels["resolved_parameters"]["gain_blue_thousandths"],
            1_000
        );
        assert_eq!(wheels["operations"].as_array().unwrap().len(), 1);
        assert_eq!(wheels["after"]["color_node_count"], 1);

        let curves = invoke(
            "plan_color_curves",
            json!({
                "expected_revision": 0,
                "clip_id": 1,
                "curves": {"master": [[0, 0], [5_000, 6_000], [10_000, 10_000]]}
            }),
        );
        assert_eq!(curves.is_error, Some(false));
        let curves = curves.structured_content.unwrap();
        assert_eq!(curves["applied"], false);
        assert_eq!(curves["kind"], "color_curves");
        assert_eq!(
            curves["resolved_curves"]["master"],
            json!([[0, 0], [5_000, 6_000], [10_000, 10_000]])
        );
        assert_eq!(curves["requested_parameters"]["master_point_count"], 3);
        assert_eq!(curves["requested_parameters"]["master_y1"], 6_000);

        // A rejected request keeps the CC1/CC2 field/observed/allowed shape.
        let rejected = invoke(
            "plan_color_curves",
            json!({
                "expected_revision": 0,
                "clip_id": 1,
                "curves": {"red": [[0, 0], [5_000, 0], [5_000, 9_000]]}
            }),
        );
        assert_eq!(rejected.is_error, Some(true));
        let rejected = rejected.structured_content.unwrap();
        assert_eq!(rejected["code"], "invalid_curve_points");
        assert_eq!(rejected["details"]["field"], "curves.red[2].x");
        assert_eq!(rejected["details"]["observed"], 5_000);
        assert_eq!(rejected["applied"], false);

        assert_eq!(
            service.snapshot().unwrap(),
            before,
            "neither planner may touch the live document"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn render_color_proof_returns_mapped_before_after_evidence_without_mutating() {
        let (seed_core, playback, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(6),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(100),
            )]),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode::ZERO,
                            value: 100,
                            interpolation: KeyframeInterpolation::Hold,
                        },
                        Keyframe {
                            at: TimeCode(12),
                            value: 750,
                            interpolation: KeyframeInterpolation::Hold,
                        },
                    ],
                },
            )]),
        });
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(7),
            name: "look_lut".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(8),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([(
                "path".to_owned(),
                ParamValue::Text("fixture.cube".to_owned()),
            )]),
            keyframes: BTreeMap::new(),
        });
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(2),
                asset: AssetId::default(),
                source_range: TimeCode::ZERO..TimeCode(60),
                content: ClipContent::Title(Title {
                    text: "CC1 proof overlay".to_owned(),
                    font_size_token: 2,
                    color_token: 1,
                    position: TitlePosition::Top,
                    background_scrim: false,
                    fade_in_frames: TimeCode(3),
                    fade_out_frames: TimeCode(4),
                    caption_preset: None,
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        document.validate().unwrap();
        let proof_document = document.clone();
        let core = Core::spawn(document).unwrap();
        let before = RgbaImage {
            width: 2,
            height: 2,
            pixels: [32, 32, 32, 255].repeat(4),
        };
        let after = RgbaImage {
            width: 2,
            height: 2,
            pixels: [255, 64, 32, 255].repeat(4),
        };
        let media = Arc::new(NoopMedia {
            thumbnail_frames: BTreeMap::from([(TimeCode(12), before.clone())]),
            candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), after.clone())]),
            // The fixture clip already carries primary node 6, so the plan
            // corrects it in place instead of stacking a second node.
            candidate_effect_id: Some(EffectId(6)),
            candidate_primary_exposure_milli_stops: Some(1_000),
            ..NoopMedia::default()
        });
        let service =
            KinewrightMcp::new(core, playback, media.clone(), ConfirmationBroker::default());
        let before_snapshot = service.snapshot().unwrap();
        let result = service
            .call_blocking(
                CallToolRequestParams::new("render_color_proof").with_arguments(
                    json!({
                        "expected_revision": 0,
                        "clip_id": 1,
                        "timecode": 12,
                        "parameters": {"exposure_milli_stops": 1_000},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 2);
        let value = result.structured_content.unwrap();
        assert_eq!(value["timeline_revision"], 0);
        assert_eq!(value["clip_id"], 1);
        assert_eq!(value["asset_id"], 1);
        assert_eq!(value["project_frame"], 12);
        assert_eq!(value["render_kind"], "test_double");
        assert_eq!(value["renderer"], "analysis.monitor_proof_for_document");
        assert_eq!(value["backend"], "test_double");
        assert_eq!(value["adapter"], "test_double");
        assert_eq!(value["software_fallback"], true);
        assert_eq!(value["gpu_claim"], false);
        assert_eq!(value["full_resolution"], true);
        assert_eq!(
            value["legacy_stage_warnings"][0]["code"],
            "legacy_lut_stage"
        );
        assert_eq!(value["legacy_stage_warnings"][0]["effect_id"], 7);
        assert_eq!(
            value["legacy_stage_warnings"][1]["code"],
            "legacy_lut_stage"
        );
        assert_eq!(value["legacy_stage_warnings"][1]["effect_id"], 8);
        assert_eq!(value["cpu_reference"], false);
        assert_eq!(value["decoded_delivery"], false);
        assert_eq!(value["source_profile"], "rec709_video");
        assert_eq!(value["source"]["provenance"], "stream_metadata");
        let active_layers = value["active_rendered_layers"].as_array().unwrap();
        assert_eq!(active_layers.len(), 2);
        assert_eq!(active_layers[0]["track_id"], 1);
        assert_eq!(active_layers[0]["clip_id"], 1);
        assert_eq!(active_layers[0]["content"], "media");
        assert_eq!(active_layers[0]["asset_id"], 1);
        assert_eq!(active_layers[0]["source"]["provenance"], "stream_metadata");
        let effects = active_layers[0]["effects"].as_array().unwrap();
        assert_eq!(effects.len(), 3);
        assert_eq!(effects[0]["effect_index"], 0);
        assert_eq!(effects[0]["effect_id"], 6);
        assert_eq!(effects[0]["name"], "primary_correction");
        assert_eq!(effects[0]["parameters"]["exposure_milli_stops"], 750);
        assert_eq!(effects[0]["keyframes"], json!({}));
        assert_eq!(
            effects[0]["primary_parameters"]["exposure_milli_stops"],
            750
        );
        assert_eq!(
            effects[0]["primary_parameters"]["contrast_pivot_basis_points"],
            5_000
        );
        assert_eq!(active_layers[0]["color_nodes"].as_array().unwrap().len(), 1);
        assert_eq!(active_layers[0]["color_nodes"][0]["effect_id"], 6);
        assert_eq!(
            active_layers[0]["color_nodes"][0]["parameters"]["exposure_milli_stops"],
            750
        );
        assert_eq!(effects[1]["effect_index"], 1);
        assert_eq!(effects[1]["effect_id"], 7);
        assert_eq!(effects[1]["name"], "look_lut");
        assert_eq!(effects[2]["effect_index"], 2);
        assert_eq!(effects[2]["effect_id"], 8);
        assert_eq!(effects[2]["name"], "cube_lut");
        assert_eq!(
            active_layers[0]["availability"]["kind"],
            "online_unverified"
        );
        assert_eq!(active_layers[1]["track_id"], 2);
        assert_eq!(active_layers[1]["clip_id"], 2);
        assert_eq!(active_layers[1]["content"], "title");
        assert_eq!(active_layers[1]["title"]["text"], "CC1 proof overlay");
        assert_eq!(active_layers[1]["title"]["font_size_token"], 2);
        assert_eq!(active_layers[1]["title"]["color_token"], 1);
        assert_eq!(active_layers[1]["title"]["position"], "top");
        assert!(active_layers[1].get("asset_id").is_none());
        assert!(active_layers[1].get("source").is_none());
        assert!(active_layers[1].get("source_fingerprint").is_none());
        assert!(active_layers[1].get("availability").is_none());
        assert_eq!(
            value["active_rendered_sources"].as_array().unwrap().len(),
            1
        );
        assert_eq!(value["active_rendered_sources"][0]["track_id"], 1);
        assert_eq!(value["active_rendered_sources"][0]["clip_id"], 1);
        assert_eq!(value["active_rendered_sources"][0]["asset_id"], 1);
        assert_eq!(
            value["active_rendered_sources"][0]["source"]["provenance"],
            "stream_metadata"
        );
        assert_eq!(
            value["active_rendered_sources"][0]["availability"]["kind"],
            "online_unverified"
        );
        assert_eq!(
            value["active_rendered_sources"][0]["legacy_stage_warnings"],
            value["legacy_stage_warnings"]
        );
        assert_eq!(value["formats"]["input"]["bit_depth"], 8);
        assert_eq!(value["formats"]["output"]["bit_depth"], "rgba8");
        let resized_pixels = |image: &RgbaImage| {
            image::imageops::resize(
                &image::RgbaImage::from_raw(image.width, image.height, image.pixels.clone())
                    .unwrap(),
                320,
                180,
                image::imageops::FilterType::Nearest,
            )
            .into_raw()
        };
        assert_eq!(
            value["hashes"]["before_rgba8_pixels_sha256"],
            kinewright_media::sha256_bytes(&resized_pixels(&before))
        );
        assert_eq!(
            value["hashes"]["after_rgba8_pixels_sha256"],
            kinewright_media::sha256_bytes(&resized_pixels(&after))
        );
        for label in [
            "before_rgba8_pixels_sha256",
            "after_rgba8_pixels_sha256",
            "before_png_bytes_sha256",
            "after_png_bytes_sha256",
            "contact_sheet_rgba8_pixels_sha256",
            "contact_sheet_png_bytes_sha256",
        ] {
            assert_eq!(
                value["hashes"][label].as_str().unwrap().len(),
                64,
                "{label}"
            );
        }
        assert_eq!(
            value["primary_correction"]["resolved_parameters"]
                .as_object()
                .unwrap()
                .len(),
            10
        );
        // The clip already carries primary node 6, so the proposal corrects it
        // in place: one SetEffectParam and no second AddEffect.
        let operations = value["operations"].as_array().unwrap();
        assert_eq!(operations.len(), 1);
        assert!(operations[0].get("AddEffect").is_none());
        assert_eq!(operations[0]["SetEffectParam"]["effect"], 6);
        assert_eq!(
            operations[0]["SetEffectParam"]["name"],
            "exposure_milli_stops"
        );
        assert_eq!(operations[0]["SetEffectParam"]["value"], 1_000);
        assert_eq!(
            value["unsupported_layer_warnings"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "the two post-primary LUT stages must be reported: {}",
            value["unsupported_layer_warnings"]
        );
        assert_eq!(
            value["unsupported_layer_warnings"][0]["code"],
            "legacy_lut_stage"
        );
        assert_eq!(value["unsupported_layer_warnings"][0]["blocking"], false);
        assert_eq!(
            value["active_rendered_layers"][0]["source"]["status"]["status"],
            "supported"
        );
        assert_eq!(value["cells"][0]["cell"], "before");
        assert_eq!(value["cells"][1]["cell"], "after");
        assert_eq!(value["objective"]["max_channel_delta_code_values"], 223);
        assert_eq!(
            value["objective"]["mean_channel_delta_milli_code_values"],
            85_000
        );
        assert!(
            value["objective"]["clipping_basis_points"]["after"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(value["evidence_only"], true);
        assert_eq!(value["applied"], false);
        assert_eq!(service.snapshot().unwrap(), before_snapshot);

        // Freeze clips use the same source-backed production layer shape as
        // media clips. Keep an online freeze overlay in this focused manifest
        // check so its exact effect/primary fields cannot regress separately.
        let mut freeze_document = proof_document.clone();
        freeze_document.tracks[1].clips[0].asset = AssetId(1);
        freeze_document.tracks[1].clips[0].content =
            ClipContent::Freeze(kinewright_core::FreezeFrame {
                source_frame: TimeCode(3),
            });
        freeze_document.validate().unwrap();
        let freeze_service = KinewrightMcp::new(
            Core::spawn(freeze_document).unwrap(),
            Arc::new(NoopMedia::default()),
            Arc::new(NoopMedia {
                thumbnail_frames: BTreeMap::from([(TimeCode(12), before.clone())]),
                candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), after.clone())]),
                candidate_effect_id: Some(EffectId(9)),
                ..NoopMedia::default()
            }),
            ConfirmationBroker::default(),
        );
        let freeze_result = freeze_service
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(freeze_result.is_error, Some(false));
        let freeze_manifest = freeze_result.structured_content.unwrap();
        let freeze_layers = freeze_manifest["active_rendered_layers"]
            .as_array()
            .unwrap();
        assert_eq!(freeze_layers[1]["content"], "freeze");
        assert_eq!(freeze_layers[1]["source_frame"], 3);
        assert!(freeze_layers[1]["effects"].is_array());

        let stale = service
            .call_blocking(
                CallToolRequestParams::new("render_color_proof").with_arguments(
                    json!({
                        "expected_revision": 1,
                        "clip_id": 1,
                        "timecode": 12,
                        "parameters": {},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(stale.is_error, Some(true));
        assert_eq!(
            stale.structured_content.unwrap()["code"],
            "revision_conflict"
        );

        for (kind, code) in [
            (MediaAvailabilityKind::OfflineMissing, "media_offline"),
            (MediaAvailabilityKind::Changed, "media_changed"),
        ] {
            let media = Arc::new(NoopMedia {
                availability_by_asset: BTreeMap::from([(
                    AssetId(1),
                    MediaAvailabilityStatus {
                        kind,
                        observed_fingerprint: None,
                        reason: Some("test proof availability".to_owned()),
                    },
                )]),
                ..NoopMedia::default()
            });
            let unavailable = KinewrightMcp::new(
                Core::spawn(proof_document.clone()).unwrap(),
                Arc::new(NoopMedia::default()),
                media,
                ConfirmationBroker::default(),
            );
            let result = unavailable
                .render_color_proof(&RenderColorProofArgs {
                    effect_id: None,
                    look_comparison: None,
                    matte_comparison: None,
                    expected_revision: TimelineRevision(0),
                    clip_id: ClipId(1),
                    timecode: TimeCode(12),
                    profile_assumption: None,
                    parameters: BTreeMap::new(),
                })
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(result.structured_content.unwrap()["code"], code);
        }

        let mut incompatible_document = proof_document.clone();
        incompatible_document.color_context.pipeline_state =
            kinewright_core::ColorPipelineState::Legacy;
        let incompatible = KinewrightMcp::new(
            Core::spawn(incompatible_document).unwrap(),
            Arc::new(NoopMedia::default()),
            Arc::new(NoopMedia::default()),
            ConfirmationBroker::default(),
        );
        let result = incompatible
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "unsupported_color_pipeline"
        );

        let failed_media = Arc::new(NoopMedia {
            render_error: Some("test compositor failure".to_owned()),
            ..NoopMedia::default()
        });
        let failed = KinewrightMcp::new(
            Core::spawn(proof_document.clone()).unwrap(),
            Arc::new(NoopMedia::default()),
            failed_media,
            ConfirmationBroker::default(),
        );
        let result = failed
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "color_proof_render_failed"
        );

        let unsupported_media = Arc::new(NoopMedia {
            proof_error: Some(MediaError::UnsupportedDecoderFormat {
                path: PathBuf::from("fixture.mp4"),
                format: "yuv444p10le".to_owned(),
                declared_bit_depth: Some(8),
                decoder_bit_depth: Some(10),
                reason: "managed source depth mismatch".to_owned(),
            }),
            ..NoopMedia::default()
        });
        let unsupported = KinewrightMcp::new(
            Core::spawn(proof_document).unwrap(),
            Arc::new(NoopMedia::default()),
            unsupported_media,
            ConfirmationBroker::default(),
        );
        let result = unsupported
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "unsupported_decoder_format");
        assert_eq!(structured["details"]["format"], "yuv444p10le");
        assert_eq!(structured["details"]["decoder_bit_depth"], 10);
    }

    fn probed_color_description() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer: ColorTransfer::AribStdB67,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 8_765,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    fn user_color_override() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::DisplayP3,
            transfer: ColorTransfer::Gamma22,
            matrix: ColorMatrix::Rgb,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Twelve,
            confidence_basis_points: 9_321,
            provenance: ColorProvenance::UserOverride,
        }
    }

    fn color_override_request(
        expected_revision: u64,
        description: &ColorDescription,
    ) -> CallToolRequestParams {
        CallToolRequestParams::new("set_asset_color_description").with_arguments(
            json!({
                "expected_revision": expected_revision,
                "asset": 1,
                "color_description": description,
            })
            .as_object()
            .unwrap()
            .clone(),
        )
    }

    fn source_color(service: &KinewrightMcp) -> serde_json::Value {
        service
            .source_info(&SourceInfoArgs {
                asset_id: AssetId(1),
                source_in: None,
                source_out: None,
            })
            .unwrap()
            .structured_content
            .unwrap()["asset"]["color_description"]
            .clone()
    }

    fn assert_wire_color(value: &serde_json::Value, confidence: u16, provenance: &str) {
        assert_eq!(value["confidence_basis_points"], confidence);
        assert_eq!(value["provenance"], provenance);
    }

    /// The CC3 §10.3 item 12 stack: a managed primary, a *bypassed but
    /// non-neutral* wheels node, and a three-point curves node, in that
    /// serialized order.
    fn ordered_colour_node_document() -> Document {
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        let effects = &mut document.tracks[0].clips[0].effects;
        effects.push(Effect {
            id: EffectId(6),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(250),
            )]),
            keyframes: BTreeMap::new(),
        });
        // Bypassed but deliberately non-neutral: CC3 §5 keeps its slot, its
        // stage index, and every stored value while it renders as the exact
        // identity.
        effects.push(Effect {
            id: EffectId(7),
            name: "color_wheels".to_owned(),
            parameters: BTreeMap::from([
                (
                    "gain_red_thousandths".to_owned(),
                    ParamValue::Integer(1_400),
                ),
                ("bypass".to_owned(), ParamValue::Integer(1)),
            ]),
            keyframes: BTreeMap::new(),
        });
        effects.push(Effect {
            id: EffectId(8),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::from([
                ("master_point_count".to_owned(), ParamValue::Integer(3)),
                ("master_x1".to_owned(), ParamValue::Integer(5_000)),
                ("master_y1".to_owned(), ParamValue::Integer(6_000)),
            ]),
            keyframes: BTreeMap::new(),
        });
        document.validate().unwrap();
        document
    }

    /// CC3 §10.3 item 12, ordered-stage half: the proof manifest's colour-node
    /// stack is `clip.effects` order, which is the compositor's evaluation
    /// order. `kinewright-media` exposes no stage manifest, so the agent's
    /// `render_color_proof` surface is where the ordering is observable.
    #[test]
    fn render_color_proof_reports_the_ordered_colour_node_stack_in_clip_effect_order() {
        let (_, playback, _) = fixture();
        let document = ordered_colour_node_document();
        let frame = RgbaImage {
            width: 2,
            height: 2,
            pixels: [32, 32, 32, 255].repeat(4),
        };
        let media = Arc::new(NoopMedia {
            thumbnail_frames: BTreeMap::from([(TimeCode(12), frame.clone())]),
            candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), frame)]),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            playback,
            media,
            ConfirmationBroker::default(),
        );
        let result = service
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let value = result.structured_content.unwrap();
        let nodes = value["active_rendered_layers"][0]["color_nodes"]
            .as_array()
            .expect("the proof manifest carries an ordered colour-node stack")
            .clone();
        assert_eq!(nodes.len(), 3);
        assert_eq!(
            nodes
                .iter()
                .map(|node| (
                    node["stage_index"].as_u64(),
                    node["kind"].as_str(),
                    node["effect_id"].as_u64()
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some(0), Some("primary_correction"), Some(6)),
                (Some(1), Some("color_wheels"), Some(7)),
                (Some(2), Some("color_curves"), Some(8)),
            ],
            "the manifest order must equal clip.effects order",
        );

        assert_eq!(nodes[1]["bypass"], 1);
        assert_eq!(nodes[1]["active"], false);
        assert_eq!(nodes[1]["inactive_reason"], "bypassed");
        assert_eq!(
            nodes[1]["parameters"]["gain_red_thousandths"], 1_400,
            "a bypassed node keeps every stored value",
        );

        assert_eq!(nodes[2]["active"], true);
        assert_eq!(
            nodes[2]["curves"]["master"]["points"],
            json!([[0, 0], [5_000, 6_000], [10_000, 10_000]]),
            "the omitted third point resolves to its (10000, 10000) neutral",
        );
        assert_eq!(nodes[2]["curves"]["master"]["truncated"], false);
        assert_eq!(nodes[2]["curves"]["red"]["structural_identity"], true);
    }

    #[test]
    fn generated_color_override_is_revision_gated_and_undo_restores_probed_metadata() {
        let (seed_core, playback, analysis) = fixture();
        let Event::QueryResult(QueryResult::Document(seed_document)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let probed = probed_color_description();
        let mut document = (*seed_document).clone();
        document.media_pool[0].color_description = probed.clone();
        let core = Core::spawn(document).unwrap();
        let service = KinewrightMcp::new(
            core.clone(),
            playback,
            analysis,
            ConfirmationBroker::default(),
        );

        let override_description = user_color_override();
        let applied = service
            .call_blocking(color_override_request(0, &override_description))
            .unwrap();
        assert_eq!(applied.is_error, Some(false));
        let (revision, applied_document) = service.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        assert_eq!(
            applied_document
                .asset(AssetId(1))
                .unwrap()
                .color_description,
            override_description
        );

        assert_wire_color(&source_color(&service), 9_321, "user_override");
        let context = service
            .color_context(&ColorContextArgs::default())
            .unwrap()
            .structured_content
            .unwrap();
        assert_wire_color(
            &context["assets"][0]["source"]["raw_description"],
            9_321,
            "user_override",
        );

        let mut stale_description = override_description.clone();
        stale_description.confidence_basis_points = 9_999;
        let stale = service
            .call_blocking(color_override_request(0, &stale_description))
            .unwrap();
        assert_eq!(stale.is_error, Some(true));
        assert!(
            stale.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("revision conflict")
        );
        assert_eq!(service.snapshot().unwrap().0, TimelineRevision(1));

        let Event::DocumentChanged {
            doc,
            revision: TimelineRevision(2),
            ..
        } = core.request(Command::Undo).unwrap()
        else {
            panic!("undo should restore the probed colour description");
        };
        assert_eq!(doc.asset(AssetId(1)).unwrap().color_description, probed);
        assert_wire_color(&source_color(&service), 8_765, "stream_metadata");
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
            "patched_three_point_edit",
            "slip_clip",
            "roll_edit",
            "slide_clip",
            "replace_clip",
            "fit_to_fill",
            "get_source_info",
            "plan_source_program_edit",
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
        assert_eq!(source["timeline_revision"], 0);
        assert_eq!(source["destinations"]["video"][0]["track_id"], 1);
        assert!(
            source["destinations"]["audio"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(source["source_monitor"]["duration"], 20);
        assert_eq!(source["asset"]["color_description"]["primaries"], "unknown");
        assert_eq!(
            source["asset"]["color_description"]["confidence_basis_points"],
            0
        );
        assert_eq!(
            source["asset"]["color_description"]["provenance"],
            "unknown"
        );
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

    #[test]
    fn source_program_planner_honors_an_explicit_second_video_track_and_commits_revision_safely() {
        let service = source_program_service_with_second_video_track();
        let result = service
            .source_program_edit_plan(&SourceProgramEditArgs {
                expected_revision: TimelineRevision(0),
                asset: AssetId(1),
                source_in: Some(TimeCode(20)),
                source_out: Some(TimeCode(40)),
                timeline_in: Some(TimeCode(10)),
                timeline_out: None,
                mode: ThreePointMode::Overwrite,
                video_track: Some(TrackId(9)),
                audio_track: None,
            })
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["timeline_revision"], 0);
        assert_eq!(structured["destinations"]["video"]["track_id"], 9);
        assert_eq!(structured["source_range"], json!({"start": 20, "end": 40}));
        assert_eq!(
            structured["timeline_range"],
            json!({"start": 10, "end": 30})
        );
        assert_eq!(structured["linked"], false);
        let plan_id = structured["prepared_edit_plan"]["plan_id"]
            .as_u64()
            .expect("prepared plan id");
        assert_eq!(
            service
                .snapshot()
                .unwrap()
                .1
                .tracks
                .iter()
                .find(|track| track.id == TrackId(9))
                .unwrap()
                .clips[0]
                .id,
            ClipId(99)
        );

        let committed = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": plan_id,
                        "expected_revision": 0,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(committed.is_error, Some(false));
        let (revision, document) = service.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        let target = document
            .tracks
            .iter()
            .find(|track| track.id == TrackId(9))
            .unwrap();
        assert_eq!(target.clips.len(), 2);
        let replacement = target
            .clips
            .iter()
            .find(|clip| clip.timeline_start == TimeCode(10))
            .expect("overwrite replacement");
        assert_eq!(replacement.source_range, TimeCode(20)..TimeCode(40));
        assert_eq!(replacement.id, ClipId(99));
        assert!(target.clips.iter().any(|clip| clip.id == ClipId(98)));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn source_program_planner_prepares_dual_linked_ranges_and_rejects_bad_routes_before_storage() {
        let service = source_program_av_service();
        let empty = service
            .source_program_edit_plan(&SourceProgramEditArgs {
                expected_revision: TimelineRevision(0),
                asset: AssetId(1),
                source_in: Some(TimeCode(0)),
                source_out: Some(TimeCode(10)),
                timeline_in: Some(TimeCode(20)),
                timeline_out: None,
                mode: ThreePointMode::Insert,
                video_track: None,
                audio_track: None,
            })
            .unwrap();
        assert_eq!(empty.is_error, Some(true));
        assert_eq!(
            empty.structured_content.unwrap()["code"],
            "empty_source_patch"
        );

        let wrong_kind = service
            .source_program_edit_plan(&SourceProgramEditArgs {
                expected_revision: TimelineRevision(0),
                asset: AssetId(1),
                source_in: Some(TimeCode(0)),
                source_out: Some(TimeCode(10)),
                timeline_in: Some(TimeCode(20)),
                timeline_out: None,
                mode: ThreePointMode::Insert,
                video_track: Some(TrackId(2)),
                audio_track: None,
            })
            .unwrap();
        assert_eq!(wrong_kind.is_error, Some(true));
        assert_eq!(
            wrong_kind.structured_content.unwrap()["code"],
            "invalid_source_patch_route_kind"
        );

        let stale = service
            .source_program_edit_plan(&SourceProgramEditArgs {
                expected_revision: TimelineRevision(1),
                asset: AssetId(1),
                source_in: Some(TimeCode(0)),
                source_out: Some(TimeCode(10)),
                timeline_in: Some(TimeCode(20)),
                timeline_out: None,
                mode: ThreePointMode::Insert,
                video_track: Some(TrackId(1)),
                audio_track: Some(TrackId(2)),
            })
            .unwrap();
        assert_eq!(stale.is_error, Some(true));
        assert!(
            stale.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("revision conflict")
        );

        let planned = service
            .source_program_edit_plan(&SourceProgramEditArgs {
                expected_revision: TimelineRevision(0),
                asset: AssetId(1),
                source_in: Some(TimeCode(0)),
                source_out: Some(TimeCode(10)),
                timeline_in: Some(TimeCode(20)),
                timeline_out: None,
                mode: ThreePointMode::Insert,
                video_track: Some(TrackId(1)),
                audio_track: Some(TrackId(2)),
            })
            .unwrap();
        assert_eq!(planned.is_error, Some(false), "{planned:?}");
        let structured = planned.structured_content.unwrap();
        assert_eq!(structured["timeline_revision"], 0);
        assert_eq!(structured["source_range"], json!({"start": 0, "end": 10}));
        assert_eq!(
            structured["timeline_range"],
            json!({"start": 20, "end": 30})
        );
        assert_eq!(structured["destinations"]["video"]["track_id"], 1);
        assert_eq!(structured["destinations"]["audio"]["track_id"], 2);
        assert_eq!(structured["linked"], true);
        assert_eq!(
            structured["destinations"]["video"]["link_id"],
            structured["destinations"]["audio"]["link_id"]
        );
        assert_eq!(structured["prepared_edit_plan"]["expected_revision"], 0);
        let plan_id = structured["prepared_edit_plan"]["plan_id"]
            .as_u64()
            .expect("prepared plan id");

        let committed = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": plan_id,
                        "expected_revision": 0,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(committed.is_error, Some(false));
        let (revision, document) = service.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        let routed = document
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .filter(|clip| {
                clip.asset == AssetId(1) && clip.source_range == (TimeCode(0)..TimeCode(10))
            })
            .collect::<Vec<_>>();
        assert_eq!(routed.len(), 2);
        assert_eq!(routed[0].timeline_start, TimeCode(20));
        assert_eq!(routed[1].timeline_start, TimeCode(20));
        assert_eq!(routed[0].link, routed[1].link);

        let source = service
            .source_info(&SourceInfoArgs {
                asset_id: AssetId(1),
                source_in: None,
                source_out: None,
            })
            .unwrap();
        assert_eq!(source.structured_content.unwrap()["timeline_revision"], 1);
    }

    fn raw_patched_operation() -> serde_json::Value {
        json!({
            "op": "patched_three_point_edit",
            "asset": 1,
            "source_in": 0,
            "source_out": 10,
            "timeline_in": 20,
            "timeline_out": null,
            "mode": "insert",
            "video_track": 1,
            "audio_track": null,
        })
    }

    fn mutable_source_service() -> (
        KinewrightMcp,
        Arc<Mutex<BTreeMap<AssetId, MediaAvailabilityStatus>>>,
    ) {
        let (core, playback, _) = fixture();
        let statuses = Arc::new(Mutex::new(BTreeMap::from([(
            AssetId(1),
            MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OnlineVerified,
                observed_fingerprint: None,
                reason: Some("verified source fixture".to_owned()),
            },
        )])));
        let analysis = Arc::new(NoopMedia {
            availability_override: Some(Arc::clone(&statuses)),
            ..NoopMedia::default()
        });
        (
            KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default()),
            statuses,
        )
    }

    #[test]
    fn raw_prepare_rejects_patched_source_without_verified_media() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let (before_revision, before_document) = service.snapshot().unwrap();
        let result = service
            .call_blocking(
                CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                    json!({
                        "expected_revision": before_revision,
                        "operations": [raw_patched_operation()],
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("online_verified")
        );
        let (after_revision, after_document) = service.snapshot().unwrap();
        assert_eq!(after_revision, before_revision);
        assert_eq!(after_document, before_document);
    }

    #[test]
    fn prepared_patched_source_rechecks_media_at_commit_without_mutation() {
        let (service, statuses) = mutable_source_service();
        let (before_revision, before_document) = service.snapshot().unwrap();
        let prepared = service
            .call_blocking(
                CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                    json!({
                        "expected_revision": before_revision,
                        "operations": [raw_patched_operation()],
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(prepared.is_error, Some(false));
        let plan_id = prepared.structured_content.unwrap()["plan_id"]
            .as_u64()
            .expect("prepared patch plan id");

        statuses.lock().unwrap().insert(
            AssetId(1),
            MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::Changed,
                observed_fingerprint: None,
                reason: Some("source changed after planning".to_owned()),
            },
        );
        let committed = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": plan_id,
                        "expected_revision": before_revision,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(committed.is_error, Some(true));
        assert!(
            committed.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("online_verified")
        );
        let (after_revision, after_document) = service.snapshot().unwrap();
        assert_eq!(after_revision, before_revision);
        assert_eq!(after_document, before_document);
        assert!(
            service
                .prepared_plans
                .lock()
                .unwrap()
                .get(PreparedPlanId(plan_id))
                .is_some(),
            "failed commit should leave the opaque plan available for reinspection"
        );
    }

    #[test]
    fn verified_patched_source_prepares_and_commits_atomically() {
        let (service, _statuses) = mutable_source_service();
        let prepared = service
            .call_blocking(
                CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                    json!({
                        "expected_revision": 0,
                        "operations": [raw_patched_operation()],
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(prepared.is_error, Some(false));
        let plan_id = prepared.structured_content.unwrap()["plan_id"]
            .as_u64()
            .expect("prepared patch plan id");
        let committed = service
            .call_blocking(
                CallToolRequestParams::new("commit_edit_plan").with_arguments(
                    json!({
                        "plan_id": plan_id,
                        "expected_revision": 0,
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .unwrap();
        assert_eq!(committed.is_error, Some(false));
        let (revision, document) = service.snapshot().unwrap();
        assert_eq!(revision, TimelineRevision(1));
        assert!(document.tracks[0].clips.iter().any(|clip| {
            clip.asset == AssetId(1)
                && clip.source_range == (TimeCode(0)..TimeCode(10))
                && clip.timeline_start == TimeCode(20)
        }));
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
        assert_eq!(
            registry_metrics.tool_count,
            operation_tools().unwrap().len() + crate::schema::INSPECTOR_TOOL_NAMES.len()
        );
        assert_eq!(served_metrics.tool_count, 7);
        assert!(served_metrics.tool_count < registry_metrics.tool_count / 4);
        assert!(served_metrics.serialized_bytes < registry_metrics.serialized_bytes / 4);
        // CC7 §5.4, R2-MAJ-3: M36's registry byte count is only measurable from
        // inside the crate (`capability_tools` is private), so it is pinned
        // here beside the served figure CC7 asserts is byte-identical to CC6's.
        // Errata D-E9 claimed this test already did that; it did not until now.
        assert_eq!(
            (
                registry_metrics.serialized_bytes,
                served_metrics.serialized_bytes
            ),
            (1_280_060, 5_660),
            "registry={registry_metrics:?} served={served_metrics:?}"
        );

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
            Arc::new(RwLock::new(None)),
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
            Arc::new(RwLock::new(None)),
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
    fn rgba_difference_reports_full_range_and_rejects_mismatched_images() {
        let black = kinewright_core::RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        };
        let white = kinewright_core::RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![255, 255, 255, 255],
        };
        let mismatched = kinewright_core::RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![0; 8],
        };
        assert_eq!(
            rgba_mean_absolute_difference_basis_points(&black, &black),
            Some(0)
        );
        assert_eq!(
            rgba_mean_absolute_difference_basis_points(&black, &white),
            Some(10_000)
        );
        assert_eq!(
            rgba_mean_absolute_difference_basis_points(&black, &mismatched),
            None
        );
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
        assert_eq!(manifest["timeline_revision"], 0);
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
                source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
                color_description: kinewright_core::ColorDescription::default(),
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
    fn cut_neighborhoods_maps_exact_cut_edges_and_does_not_mutate() {
        let (core, playback, analysis) = fixture();
        core.request(Command::Do(Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(20),
        }))
        .unwrap();
        core.request(Command::Do(Operation::SplitClip {
            clip: ClipId(2),
            at: TimeCode(40),
        }))
        .unwrap();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let before = service.document().unwrap();
        let result = service
            .cut_neighborhoods(&CutNeighborhoodsArgs {
                track_id: TrackId(1),
                frames_before: Some(1),
                frames_after: Some(3),
                cut_offset: None,
                cut_count: None,
                maximum_secondary_change_basis_points: None,
                max_width: Some(64),
            })
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        assert!(
            result
                .content
                .iter()
                .any(|block| block.as_image().is_some())
        );
        let manifest = result.structured_content.unwrap();
        assert_eq!(manifest["timeline_revision"], 2);
        assert_eq!(manifest["track_id"], 1);
        assert_eq!(manifest["total_cut_count"], 2);
        assert_eq!(manifest["returned_cut_count"], 2);
        assert_eq!(manifest["clean"], true);
        assert_eq!(manifest["issue_count"], 0);
        assert_eq!(manifest["sheet"], json!({"width": 20, "height": 8}));
        let cells = manifest["cells"].as_array().unwrap();
        assert_eq!(
            cells
                .iter()
                .map(|cell| cell["project_frame"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [19, 20, 21, 22, 39, 40, 41, 42]
        );
        assert_eq!(cells[0]["side"], "outgoing");
        assert_eq!(cells[4]["side"], "outgoing");
        assert!(cells[1..4].iter().all(|cell| cell["side"] == "incoming"));
        assert!(cells[5..8].iter().all(|cell| cell["side"] == "incoming"));
        assert_eq!(&*service.document().unwrap(), &*before);
    }

    #[test]
    fn cut_neighborhoods_blocks_a_secondary_change_inside_the_incoming_handle() {
        let (core, playback, _) = fixture();
        core.request(Command::Do(Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(20),
        }))
        .unwrap();
        let black = RgbaImage {
            width: 2,
            height: 2,
            pixels: [0, 0, 0, 255].repeat(4),
        };
        let white = RgbaImage {
            width: 2,
            height: 2,
            pixels: [255, 255, 255, 255].repeat(4),
        };
        let analysis = Arc::new(NoopMedia {
            thumbnail_frames: BTreeMap::from([
                (TimeCode(20), black.clone()),
                (TimeCode(21), black),
                (TimeCode(22), white),
            ]),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let result = service
            .cut_neighborhoods(&CutNeighborhoodsArgs {
                track_id: TrackId(1),
                frames_before: Some(1),
                frames_after: Some(3),
                cut_offset: None,
                cut_count: Some(1),
                maximum_secondary_change_basis_points: Some(1_200),
                max_width: Some(64),
            })
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .starts_with("CUT EDGE REVIEW FAILED")
        );
        let manifest = result.structured_content.unwrap();
        assert_eq!(manifest["clean"], false);
        assert_eq!(manifest["issue_count"], 1);
        assert_eq!(manifest["issues"][0]["cut_frame"], 20);
        assert_eq!(manifest["issues"][0]["from_offset"], 1);
        assert_eq!(manifest["issues"][0]["to_offset"], 2);
        assert_eq!(manifest["issues"][0]["change_basis_points"], 10_000);
    }

    #[test]
    fn cut_neighborhoods_rejects_invalid_tracks_and_bounds() {
        let (core, playback, analysis) = fixture();
        core.request(Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            },
        }))
        .unwrap();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        for args in [
            CutNeighborhoodsArgs {
                track_id: TrackId(999),
                frames_before: None,
                frames_after: None,
                cut_offset: None,
                cut_count: None,
                maximum_secondary_change_basis_points: None,
                max_width: None,
            },
            CutNeighborhoodsArgs {
                track_id: TrackId(2),
                frames_before: None,
                frames_after: None,
                cut_offset: None,
                cut_count: None,
                maximum_secondary_change_basis_points: None,
                max_width: None,
            },
            CutNeighborhoodsArgs {
                track_id: TrackId(1),
                frames_before: Some(0),
                frames_after: None,
                cut_offset: None,
                cut_count: None,
                maximum_secondary_change_basis_points: None,
                max_width: None,
            },
            CutNeighborhoodsArgs {
                track_id: TrackId(1),
                frames_before: None,
                frames_after: None,
                cut_offset: None,
                cut_count: Some(13),
                maximum_secondary_change_basis_points: None,
                max_width: None,
            },
            CutNeighborhoodsArgs {
                track_id: TrackId(1),
                frames_before: None,
                frames_after: None,
                cut_offset: None,
                cut_count: None,
                maximum_secondary_change_basis_points: Some(10_001),
                max_width: None,
            },
        ] {
            assert_eq!(
                service.cut_neighborhoods(&args).unwrap().is_error,
                Some(true)
            );
        }
    }

    #[test]
    fn cut_neighborhoods_is_internal_registry_capability_not_compact_tool() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        assert!(
            registry
                .iter()
                .any(|tool| tool.name == "get_cut_neighborhoods")
        );
        let served = KinewrightMcp::served_tools().unwrap();
        assert!(
            served
                .iter()
                .all(|tool| tool.name != "get_cut_neighborhoods")
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
                        "candidate_selection": "page",
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
        assert_eq!(manifest["timeline_revision"], 0);
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
                candidate_selection: Some(ShotBoardCandidateSelection::Page),
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
                candidate_selection: None,
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
                source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
                color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
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

    // -----------------------------------------------------------------------
    // CC4 §10.3.14 — import authorization, plan rejection, and proof honesty
    // -----------------------------------------------------------------------

    fn cc4_project_directory(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "kinewright-cc4-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// A real, parseable `.cube` source: the pinned built-in bake, which round
    /// trips through the production parser by construction (CC4 §2.6).
    fn cc4_write_source_cube(directory: &Path) -> PathBuf {
        let path = directory.join("warm.cube");
        std::fs::write(&path, kinewright_media::BuiltinLook::Warm.canonical_text()).unwrap();
        path
    }

    fn cc4_service_with_project(
        broker: ConfirmationBroker,
        project_path: Option<PathBuf>,
    ) -> KinewrightMcp {
        let (core, playback, analysis) = fixture();
        KinewrightMcp::configured(
            core,
            playback,
            analysis,
            None,
            broker,
            true,
            Arc::new(RwLock::new(project_path)),
        )
    }

    fn cc4_import_request(path: &Path) -> CallToolRequestParams {
        CallToolRequestParams::new("import_lut_asset").with_arguments(serde_json::Map::from_iter([
            ("expected_revision".to_owned(), serde_json::json!(0)),
            ("path".to_owned(), serde_json::json!(path)),
        ]))
    }

    fn cc4_document_of(service: &KinewrightMcp) -> Arc<Document> {
        let Event::QueryResult(QueryResult::Document(document)) = service
            .core
            .request(Command::Query(Query::Document))
            .unwrap()
        else {
            panic!("expected a document query result");
        };
        document
    }

    /// CC4 §8, §13: the confirmation is requested before the first byte is
    /// read, so a refused import leaves no store file and no document change.
    #[test]
    fn cc4_refused_import_writes_no_store_file_and_changes_no_document() {
        let directory = cc4_project_directory("import-refused");
        let source = cc4_write_source_cube(&directory);
        let project = directory.join("edit.kinewright");
        let store_root = directory.join("edit.kinewright-assets");
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
        let service = cc4_service_with_project(broker.clone(), Some(project));

        let result = invoke_in_background(service.clone(), cc4_import_request(&source));
        let request = wait_for_request(&broker);
        assert_eq!(request.tool_name, "import_lut_asset");
        assert!(
            request.description.contains("edit.kinewright-assets"),
            "the operator is told exactly where the bytes would be written: {}",
            request.description
        );
        assert!(broker.reject(request.id, "rejected by user"));

        let result = result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "import_refused");
        assert_eq!(structured["details"]["store_file_written"], false);
        assert_eq!(structured["details"]["document_changed"], false);

        assert!(
            !store_root.exists(),
            "a refused import must not create the project LUT store"
        );
        let document = cc4_document_of(&service);
        assert!(document.lut_assets.is_empty());
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §2.4: an approved import parses, hashes, stores, and registers the
    /// asset as one undoable `AddLutAsset`.
    #[test]
    fn cc4_approved_import_registers_the_hashed_asset() {
        let directory = cc4_project_directory("import-approved");
        let source = cc4_write_source_cube(&directory);
        let project = directory.join("edit.kinewright");
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
        let service = cc4_service_with_project(broker.clone(), Some(project));

        let result = invoke_in_background(service.clone(), cc4_import_request(&source));
        let request = wait_for_request(&broker);
        assert!(broker.approve(request.id));
        let result = result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{:?}", result.content);

        let expected_sha = kinewright_media::BuiltinLook::Warm.pinned_sha256();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["lut_asset"]["lut_asset_id"], 1);
        assert_eq!(structured["lut_asset"]["sha256"], expected_sha);
        assert_eq!(structured["lut_asset"]["kind"], "cube_3d");
        assert_eq!(structured["applied"], true);

        let stored = directory
            .join("edit.kinewright-assets")
            .join("luts")
            .join(format!("{expected_sha}.cube"));
        assert!(
            stored.is_file(),
            "the hashed bytes land in the project store"
        );

        let document = cc4_document_of(&service);
        assert_eq!(document.lut_assets.len(), 1);
        assert_eq!(document.lut_assets[0].sha256, expected_sha);

        // The asset is immediately visible to the read-only look surface with
        // a verified availability, because the store root is now known.
        let listed = service.list_look_assets().unwrap();
        let listed = listed.structured_content.unwrap();
        assert_eq!(listed["store_root_known"], true);
        assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
        assert_eq!(listed["assets"][0]["referenced_by"], serde_json::json!([]));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §2.2: a project that has never been saved has no store root, and
    /// the refusal is typed rather than an invented temporary location.
    #[test]
    fn cc4_import_requires_a_saved_project() {
        let directory = cc4_project_directory("import-unsaved");
        let source = cc4_write_source_cube(&directory);
        let broker = ConfirmationBroker::with_timeout(Duration::from_millis(50));
        let service = cc4_service_with_project(broker, None);

        let result = service.call_blocking(cc4_import_request(&source)).unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "project_not_saved");
        assert_eq!(structured["details"]["field"], "project_path");
        assert!(
            structured["details"]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("Save the project")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A service whose clip 1 carries exactly `effects`.
    fn cc4_legacy_service(
        effects: Vec<Effect>,
        broker: ConfirmationBroker,
        project_path: Option<PathBuf>,
    ) -> KinewrightMcp {
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        document.tracks[0].clips[0].effects = effects;
        document
            .validate()
            .expect("the seeded legacy stack is valid");
        let media = Arc::new(NoopMedia::default());
        KinewrightMcp::configured(
            Core::spawn(document).unwrap(),
            media.clone(),
            media,
            None,
            broker,
            true,
            Arc::new(RwLock::new(project_path)),
        )
    }

    fn cc4_look_lut(id: u64, preset_token: i64, intensity_percent: i64) -> Effect {
        Effect {
            id: EffectId(id),
            name: "look_lut".to_owned(),
            parameters: BTreeMap::from([
                ("preset_token".to_owned(), ParamValue::Integer(preset_token)),
                (
                    "intensity_percent".to_owned(),
                    ParamValue::Integer(intensity_percent),
                ),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    fn cc4_convert_request(revision: u64, clip: u64, effect: u64) -> CallToolRequestParams {
        CallToolRequestParams::new("convert_legacy_look").with_arguments(
            serde_json::Map::from_iter([
                ("expected_revision".to_owned(), serde_json::json!(revision)),
                ("clip_id".to_owned(), serde_json::json!(clip)),
                ("effect_id".to_owned(), serde_json::json!(effect)),
            ]),
        )
    }

    /// CC4 §8, §9: the published `[AddLutAsset, ConvertLegacyLook]` batch is
    /// only `ready` because one tool can submit it. The built-in is registered
    /// exactly once; a second conversion of the same look reuses that record
    /// rather than allocating a duplicate id for identical bytes.
    #[test]
    fn cc4_convert_legacy_look_registers_the_builtin_once_and_reuses_the_record() {
        let service = cc4_legacy_service(
            vec![cc4_look_lut(5, 1, 50), cc4_look_lut(6, 1, 100)],
            ConfirmationBroker::default(),
            None,
        );

        // The evidence surface names the tool that can actually submit it.
        let context = service
            .call_blocking(CallToolRequestParams::new("get_color_context"))
            .unwrap();
        let context = context.structured_content.unwrap();
        let conversions = context["legacy_look_conversions"].as_array().unwrap();
        assert_eq!(conversions.len(), 2);
        assert_eq!(conversions[0]["status"], "ready");
        assert_eq!(conversions[0]["builtin_name"], "warm");
        assert_eq!(conversions[0]["mix_basis_points"], 5_000);
        assert!(
            conversions[0]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("convert_legacy_look"),
            "{}",
            conversions[0]
        );

        let first = service.call_blocking(cc4_convert_request(0, 1, 5)).unwrap();
        assert_eq!(first.is_error, Some(false), "{:?}", first.content);
        let first = first.structured_content.unwrap();
        assert_eq!(first["applied"], true);
        assert_eq!(first["bit_identical_to_legacy"], false);
        assert_eq!(first["conversion"]["source"], "builtin");
        assert_eq!(first["conversion"]["reused_existing_asset"], false);
        assert_eq!(first["conversion"]["store_file_written"], false);
        assert_eq!(first["conversion"]["mix_basis_points"], 5_000);
        assert_eq!(first["operations"].as_array().unwrap().len(), 2);
        // A built-in needs no store, so its availability is still verified.
        assert_eq!(first["lut_asset"]["availability"]["kind"], "verified");
        assert_eq!(
            first["lut_asset"]["recovery_action"],
            serde_json::Value::Null
        );

        let document = cc4_document_of(&service);
        assert_eq!(document.lut_assets.len(), 1);
        assert_eq!(
            document.lut_assets[0].sha256,
            kinewright_media::BuiltinLook::Warm.pinned_sha256()
        );
        let effects = &document.tracks[0].clips[0].effects;
        assert_eq!(effects[0].name, "creative_look");
        assert_eq!(effects[0].id, EffectId(5));
        assert_eq!(
            effects[0].parameters["mix_basis_points"],
            ParamValue::Integer(5_000)
        );
        assert_eq!(effects[1].name, "look_lut");

        let second = service.call_blocking(cc4_convert_request(1, 1, 6)).unwrap();
        assert_eq!(second.is_error, Some(false), "{:?}", second.content);
        let second = second.structured_content.unwrap();
        assert_eq!(second["conversion"]["reused_existing_asset"], true);
        assert_eq!(second["conversion"]["lut_asset_id"], 1);
        assert_eq!(
            second["operations"].as_array().unwrap().len(),
            1,
            "the registered record is reused, so no second AddLutAsset is emitted"
        );

        let document = cc4_document_of(&service);
        assert_eq!(
            document.lut_assets.len(),
            1,
            "identical bytes are one content-addressed asset"
        );
        assert!(
            document.tracks[0].clips[0]
                .effects
                .iter()
                .all(|effect| effect.name == "creative_look")
        );
    }

    /// CC4 §8: the conversion is revision-gated and fails closed.
    #[test]
    fn cc4_convert_legacy_look_fails_closed_on_a_stale_revision() {
        let service = cc4_legacy_service(
            vec![cc4_look_lut(5, 2, 100)],
            ConfirmationBroker::default(),
            None,
        );
        let stale = service.call_blocking(cc4_convert_request(9, 1, 5)).unwrap();
        assert_eq!(stale.is_error, Some(true));
        let stale = stale.structured_content.unwrap();
        assert_eq!(stale["code"], "revision_conflict");
        assert_eq!(stale["details"]["field"], "expected_revision");
        assert_eq!(stale["details"]["observed"], 9);
        assert_eq!(stale["details"]["allowed"], 0);
        assert_eq!(stale["applied"], false);

        let document = cc4_document_of(&service);
        assert!(document.lut_assets.is_empty());
        assert_eq!(document.tracks[0].clips[0].effects[0].name, "look_lut");
    }

    /// CC4 §8, §13: a `cube_lut` conversion imports through the same
    /// confirmation path as `import_lut_asset`, so a refusal leaves no store
    /// file and no document change.
    #[test]
    fn cc4_refused_legacy_cube_conversion_writes_nothing() {
        let directory = cc4_project_directory("convert-refused");
        let source = cc4_write_source_cube(&directory);
        let project = directory.join("edit.kinewright");
        let store_root = directory.join("edit.kinewright-assets");
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
        let service = cc4_legacy_service(
            vec![Effect {
                id: EffectId(5),
                name: "cube_lut".to_owned(),
                parameters: BTreeMap::from([(
                    "path".to_owned(),
                    ParamValue::Text(source.display().to_string()),
                )]),
                keyframes: BTreeMap::new(),
            }],
            broker.clone(),
            Some(project),
        );

        let result = invoke_in_background(service.clone(), cc4_convert_request(0, 1, 5));
        let request = wait_for_request(&broker);
        assert_eq!(request.tool_name, "convert_legacy_look");
        assert!(
            request.description.contains("edit.kinewright-assets"),
            "the operator is told exactly where the bytes would be written: {}",
            request.description
        );
        assert!(broker.reject(request.id, "rejected by user"));

        let result = result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "import_refused");
        assert_eq!(structured["details"]["store_file_written"], false);
        assert_eq!(structured["details"]["document_changed"], false);

        assert!(
            !store_root.exists(),
            "a refused conversion must not create the project LUT store"
        );
        let document = cc4_document_of(&service);
        assert!(document.lut_assets.is_empty());
        assert_eq!(document.tracks[0].clips[0].effects[0].name, "cube_lut");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §8: an approved `cube_lut` conversion imports the bytes and
    /// converts in one batch.
    #[test]
    fn cc4_approved_legacy_cube_conversion_imports_and_converts() {
        let directory = cc4_project_directory("convert-approved");
        let source = cc4_write_source_cube(&directory);
        let project = directory.join("edit.kinewright");
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
        let service = cc4_legacy_service(
            vec![Effect {
                id: EffectId(5),
                name: "cube_lut".to_owned(),
                parameters: BTreeMap::from([
                    (
                        "path".to_owned(),
                        ParamValue::Text(source.display().to_string()),
                    ),
                    ("intensity_percent".to_owned(), ParamValue::Integer(40)),
                ]),
                keyframes: BTreeMap::new(),
            }],
            broker.clone(),
            Some(project),
        );

        let result = invoke_in_background(service.clone(), cc4_convert_request(0, 1, 5));
        let request = wait_for_request(&broker);
        assert!(broker.approve(request.id));
        let result = result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{:?}", result.content);
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["conversion"]["source"], "imported");
        assert_eq!(structured["conversion"]["store_file_written"], true);
        assert_eq!(structured["conversion"]["mix_basis_points"], 4_000);
        assert_eq!(structured["lut_asset"]["availability"]["kind"], "verified");

        let expected_sha = kinewright_media::BuiltinLook::Warm.pinned_sha256();
        assert!(
            directory
                .join("edit.kinewright-assets")
                .join("luts")
                .join(format!("{expected_sha}.cube"))
                .is_file()
        );
        let document = cc4_document_of(&service);
        assert_eq!(document.lut_assets.len(), 1);
        assert_eq!(document.tracks[0].clips[0].effects[0].name, "creative_look");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §8: a node that cannot be converted carries the full typed
    /// rejection shape, not a bare message.
    ///
    /// The document-level `unconvertible` statuses (`invalid_preset_token`,
    /// `missing_external_lut_path`) are only reachable from a hand-edited
    /// project - Core rejects both at `validate` - so they are covered as a
    /// unit on `legacy_look_conversions_value` in `color_status`.
    #[test]
    fn cc4_unconvertible_legacy_look_reports_field_observed_and_allowed() {
        let service = cc4_legacy_service(
            vec![Effect {
                id: EffectId(6),
                name: "primary_correction".to_owned(),
                parameters: BTreeMap::from([(
                    "exposure_milli_stops".to_owned(),
                    ParamValue::Integer(100),
                )]),
                keyframes: BTreeMap::new(),
            }],
            ConfirmationBroker::default(),
            None,
        );

        // A managed node is not a legacy look, and says so with its own shape.
        let refused = service.call_blocking(cc4_convert_request(0, 1, 6)).unwrap();
        assert_eq!(refused.is_error, Some(true));
        let refused = refused.structured_content.unwrap();
        assert_eq!(refused["code"], "not_a_legacy_look");
        assert_eq!(refused["details"]["field"], "effect_id");
        assert_eq!(refused["details"]["observed"], "primary_correction");
        assert_eq!(
            refused["details"]["allowed"],
            serde_json::json!(["look_lut", "cube_lut"])
        );
        assert!(refused["details"]["recovery_action"].is_string());
    }

    /// CC4 §2.1: importing the same bytes twice is the same asset, so the
    /// second import reuses the record instead of allocating a second id.
    #[test]
    fn cc4_import_lut_asset_reuses_a_record_with_the_same_content_hash() {
        let directory = cc4_project_directory("import-dedup");
        let source = cc4_write_source_cube(&directory);
        let copy = directory.join("warm-copy.cube");
        std::fs::copy(&source, &copy).unwrap();
        let project = directory.join("edit.kinewright");
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(2));
        let service = cc4_service_with_project(broker.clone(), Some(project));

        let first = invoke_in_background(service.clone(), cc4_import_request(&source));
        assert!(broker.approve(wait_for_request(&broker).id));
        let first = first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(first.is_error, Some(false));
        assert_eq!(
            first.structured_content.unwrap()["reused_existing_asset"],
            false
        );

        // A different path, the same bytes: still one asset.
        let request = CallToolRequestParams::new("import_lut_asset").with_arguments(
            serde_json::Map::from_iter([
                ("expected_revision".to_owned(), serde_json::json!(1)),
                ("path".to_owned(), serde_json::json!(copy)),
            ]),
        );
        let second = invoke_in_background(service.clone(), request);
        assert!(broker.approve(wait_for_request(&broker).id));
        let second = second
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(second.is_error, Some(false), "{:?}", second.content);
        let second = second.structured_content.unwrap();
        assert_eq!(second["reused_existing_asset"], true);
        assert_eq!(second["applied"], false);
        assert_eq!(second["lut_asset"]["lut_asset_id"], 1);

        let document = cc4_document_of(&service);
        assert_eq!(document.lut_assets.len(), 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §8: every `import_lut_asset` rejection is structured, including the
    /// revision conflict.
    #[test]
    fn cc4_import_lut_asset_revision_conflict_is_structured() {
        let directory = cc4_project_directory("import-conflict");
        let source = cc4_write_source_cube(&directory);
        let project = directory.join("edit.kinewright");
        let broker = ConfirmationBroker::with_timeout(Duration::from_millis(50));
        let service = cc4_service_with_project(broker, Some(project));

        let request = CallToolRequestParams::new("import_lut_asset").with_arguments(
            serde_json::Map::from_iter([
                ("expected_revision".to_owned(), serde_json::json!(7)),
                ("path".to_owned(), serde_json::json!(source)),
            ]),
        );
        let result = service.call_blocking(request).unwrap();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "revision_conflict");
        assert_eq!(structured["details"]["field"], "expected_revision");
        assert_eq!(structured["details"]["observed"], 7);
        assert_eq!(structured["details"]["allowed"], 0);
        assert!(
            structured["details"]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("get_timeline_state")
        );
        assert!(
            !directory.join("edit.kinewright-assets").exists(),
            "a conflict is detected before the store is touched"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// CC4 §2.3: a built-in is `verified` from this binary's own bake, so an
    /// unsaved project reports it honestly instead of `unknown_no_store`.
    /// Only an *imported* asset needs a store to resolve.
    #[test]
    fn cc4_builtin_availability_needs_no_store_root() {
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        let mut stale =
            kinewright_media::BuiltinLook::Cool.to_lut_asset(kinewright_core::LutAssetId(2));
        stale.sha256 = "0".repeat(64);
        document.lut_assets = vec![
            kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1)),
            stale,
            LutAsset {
                id: kinewright_core::LutAssetId(3),
                sha256: "a".repeat(64),
                title: "Imported".to_owned(),
                kind: kinewright_core::LutAssetKind::Cube3d,
                size: 2,
                byte_len: 64,
                domain_min_millionths: [0; 3],
                domain_max_millionths: [1_000_000; 3],
                source: kinewright_core::LutAssetSource::Imported {
                    source_path: "vendor.cube".to_owned(),
                },
            },
        ];
        document
            .validate()
            .expect("the seeded asset table is valid");
        let media = Arc::new(NoopMedia::default());
        let service = KinewrightMcp::configured(
            Core::spawn(document).unwrap(),
            media.clone(),
            media,
            None,
            ConfirmationBroker::default(),
            true,
            Arc::new(RwLock::new(None)),
        );

        let listed = service.list_look_assets().unwrap();
        let listed = listed.structured_content.unwrap();
        assert_eq!(listed["store_root_known"], false);
        assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
        assert_eq!(
            listed["assets"][0]["recovery_action"],
            serde_json::Value::Null
        );
        assert_eq!(listed["assets"][1]["availability"]["kind"], "changed");
        assert!(
            listed["assets"][1]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("sha256")
        );
        assert_eq!(
            listed["assets"][2]["availability"]["kind"], "unknown_no_store",
            "only imported bytes need a store to resolve"
        );
        assert!(
            listed["assets"][2]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("Save the project")
        );
    }

    /// CC4 §8: the manifest asserts bypass identity, so a bypass variant that
    /// is not the byte-identical twin of the node-removed variant refuses the
    /// proof instead of publishing `bypass_matches_absent: false`.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc4_bypass_that_is_not_lossless_refuses_the_proof() {
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut document = (*seed).clone();
        document.media_pool[0].color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        document.lut_assets =
            vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
        document.tracks[0].clips[0].effects = vec![Effect {
            id: EffectId(9),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
            keyframes: BTreeMap::new(),
        }];
        document
            .validate()
            .expect("the CC4 stack is a valid document");
        let media = Arc::new(NoopMedia {
            bypass_leaks_pixel: Some(0x7f),
            ..NoopMedia::default()
        });
        let service = KinewrightMcp::new(
            Core::spawn(document).unwrap(),
            media.clone(),
            media,
            ConfirmationBroker::default(),
        );

        let refused = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(9)),
                look_comparison: Some(LookComparison::Bypass),
                matte_comparison: None,
            })
            .unwrap();
        assert_eq!(refused.is_error, Some(true));
        let structured = refused.structured_content.unwrap();
        assert_eq!(structured["code"], "bypass_not_lossless");
        assert_eq!(structured["details"]["field"], "look_comparison");
        assert_eq!(structured["details"]["effect_id"], 9);
        let observed = &structured["details"]["observed"];
        assert_ne!(
            observed["absent_rgba8_pixels_sha256"],
            observed["bypass_rgba8_pixels_sha256"]
        );
        assert_eq!(observed["absent_raster"]["width"], 320);
        assert_eq!(observed["bypass_raster"]["height"], 180);
        assert!(structured["details"]["recovery_action"].is_string());

        // The same node compares cleanly when the two variants agree.
        let clean_media = Arc::new(NoopMedia::default());
        let (seed_core, _, _) = fixture();
        let Event::QueryResult(QueryResult::Document(seed)) =
            seed_core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected fixture document");
        };
        let mut clean = (*seed).clone();
        clean.media_pool[0].color_description = document_color_description_for_managed_proof();
        clean.lut_assets =
            vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
        clean.tracks[0].clips[0].effects = vec![Effect {
            id: EffectId(9),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
            keyframes: BTreeMap::new(),
        }];
        clean.validate().unwrap();
        let clean_service = KinewrightMcp::new(
            Core::spawn(clean).unwrap(),
            clean_media.clone(),
            clean_media,
            ConfirmationBroker::default(),
        );
        let proof = clean_service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(9)),
                look_comparison: Some(LookComparison::Bypass),
                matte_comparison: None,
            })
            .unwrap();
        assert_eq!(proof.is_error, Some(false), "{:?}", proof.content);
        assert_eq!(
            proof.structured_content.unwrap()["look_comparison"]["bypass_matches_absent"],
            true
        );
    }

    fn document_color_description_for_managed_proof() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    /// CC4 §8: the media LUT failure text is parsed with anchored field keys.
    ///
    /// A bare substring search matched `line` inside a path component such as
    /// `baseline`, and splitting on the first `"; "` truncated any value that
    /// contained one - both of which a real filesystem path can produce.
    #[test]
    fn lut_error_fields_are_anchored_and_survive_semicolons_in_values() {
        let store = kinewright_core::MediaError::Backend(
            "lut_store_root_invalid: the derived store root is not a directory; observed=/home/e/baseline; takes/edit.kinewright-assets; allowed=a writable directory".to_owned(),
        );
        let result = lut_store_error_result("import_lut_asset", &store);
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "lut_store_root_invalid");
        assert_eq!(
            structured["message"],
            "the derived store root is not a directory"
        );
        assert_eq!(
            structured["details"]["observed"], "/home/e/baseline; takes/edit.kinewright-assets",
            "the value runs to the next anchored key, not to the first \"; \""
        );
        assert_eq!(structured["details"]["allowed"], "a writable directory");
        assert_eq!(
            structured["details"]["line"],
            serde_json::Value::Null,
            "`baseline` is not a `line` field"
        );

        // The parser's own shape, which leads with `observed` and ends with a
        // 1-based line number.
        let parse = kinewright_core::MediaError::Backend(
            "invalid_lut_sample: observed 1.0 2.0; allowed three floats in 0..=1; line 42"
                .to_owned(),
        );
        let structured = lut_store_error_result("import_lut_asset", &parse)
            .structured_content
            .unwrap();
        assert_eq!(structured["code"], "invalid_lut_sample");
        assert_eq!(structured["details"]["observed"], "1.0 2.0");
        assert_eq!(structured["details"]["allowed"], "three floats in 0..=1");
        assert_eq!(structured["details"]["line"], "42");
    }

    /// CC4 §8: a *value* that begins with another field's key is still a
    /// value.
    ///
    /// The anchor at offset 0 exists for the rendered remainder, which really
    /// can lead with a key. Applying it while scanning inside an extracted
    /// value made `observed=allowed=x` and `observed line 1 2 3 4` terminate
    /// immediately and report the empty string.
    #[test]
    fn a_lut_error_value_that_begins_with_another_key_is_not_truncated() {
        // The `.cube` sample the parser rejected literally begins with the
        // word `line`, and the trailing `line` field still has to be found.
        let parse = kinewright_core::MediaError::Backend(
            "invalid_lut_sample: observed line 1 2 3 4; allowed three floats in 0..=1; line 12"
                .to_owned(),
        );
        let structured = lut_store_error_result("import_lut_asset", &parse)
            .structured_content
            .unwrap();
        assert_eq!(structured["details"]["observed"], "line 1 2 3 4");
        assert_eq!(structured["details"]["allowed"], "three floats in 0..=1");
        assert_eq!(structured["details"]["line"], "12");

        // The unified `; <key>=<value>` shape, with a value that begins with
        // the next key's name.
        let store = kinewright_core::MediaError::Backend(
            "lut_store_root_invalid: the derived store root is a symbolic link; observed=allowed=x; allowed=a writable directory; line=3"
                .to_owned(),
        );
        let structured = lut_store_error_result("import_lut_asset", &store)
            .structured_content
            .unwrap();
        assert_eq!(
            structured["message"],
            "the derived store root is a symbolic link"
        );
        assert_eq!(structured["details"]["observed"], "allowed=x");
        assert_eq!(structured["details"]["allowed"], "a writable directory");
        assert_eq!(structured["details"]["line"], "3");
    }

    /// The anchor rules in isolation, so the two callers cannot drift apart.
    #[test]
    fn lut_error_field_anchors_only_at_a_field_boundary() {
        assert_eq!(
            lut_error_field_start("observed=x", "observed", true),
            Some(0)
        );
        assert_eq!(lut_error_field_start("observed=x", "observed", false), None);
        assert_eq!(
            lut_error_field_start("a; observed=x", "observed", false),
            Some(3)
        );
        // `baseline` is not a `line` field, at either anchor.
        assert_eq!(lut_error_field_start("baseline=x", "line", true), None);
        assert_eq!(lut_error_field_start("a; baseline=x", "line", false), None);
        // A leading detail sentence survives a value that starts with a key.
        assert_eq!(
            lut_error_detail("observed line 1 2 3 4; allowed three; line 12"),
            "observed line 1 2 3 4; allowed three; line 12",
            "a remainder that leads with a key has no detail sentence of its own"
        );
    }

    /// CC4 §8: `AddLutAsset` is blocked in all four places, exactly as
    /// `RelinkAsset` is, because only `import_lut_asset` can write the store.
    #[test]
    fn cc4_add_lut_asset_is_never_reachable_through_a_plan_or_generated_tool() {
        assert!(
            !operation_tools()
                .unwrap()
                .iter()
                .any(|definition| definition.tool.name == "add_lut_asset"),
            "the generated add_lut_asset operation tool must not exist"
        );
        assert!(crate::schema::UNGENERATED_OPERATION_VARIANTS.contains(&"AddLutAsset"));

        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let asset =
            kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1));
        let operations = vec![Operation::AddLutAsset {
            asset: asset.clone(),
        }];

        let applied = service
            .apply_edit_plan(TimelineRevision(0), &operations)
            .unwrap();
        assert_eq!(applied.is_error, Some(true));
        assert!(
            applied.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("import_lut_asset")
        );

        let (revision, document) = service.snapshot().unwrap();
        let prepared = PreparedPlanStore::default()
            .prepare_operations(revision, revision, &document, operations);
        let error = prepared.expect_err("a prepared plan cannot register a LUT asset");
        assert!(error.to_string().contains("import_lut_asset"), "{error}");

        // The dispatcher refuses the name outright rather than reporting an
        // unknown tool, so the recovery path is stated.
        let dispatched = service
            .call_blocking(CallToolRequestParams::new("add_lut_asset").with_arguments(
                serde_json::Map::from_iter([(
                    "expected_revision".to_owned(),
                    serde_json::json!(0),
                )]),
            ))
            .unwrap();
        assert_eq!(dispatched.is_error, Some(true));
        assert!(
            dispatched.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("import_lut_asset")
        );
        assert!(document.lut_assets.is_empty());
    }

    /// CC4 §8 / M36: the two LUT descriptors never enumerate the `2^53` asset
    /// id range, and both planner tools stay well under a kilobyte.
    #[test]
    fn cc4_lut_tool_descriptors_stay_compact() {
        for kind in [ColorNodeKind::TechnicalLut, ColorNodeKind::CreativeLook] {
            let summary = lut_node_parameter_summary(kind);
            assert!(
                summary.len() < 1_024,
                "{} summary is {} bytes",
                kind.effect_name(),
                summary.len()
            );
            assert!(summary.contains("see list_look_assets"));
            assert!(!summary.contains("9007199254740991"));
            assert!(summary.contains("0 display709, 1 linear, 2 grade709"));
        }
        let registry = KinewrightMcp::capability_tools().unwrap();
        for name in ["plan_technical_lut", "plan_creative_look"] {
            let tool = registry
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} must be an internal capability"));
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                description.len() < 1_024,
                "{name} description is {} bytes",
                description.len()
            );
            assert!(!description.contains("9007199254740991"));
            assert_eq!(
                tool.annotations.as_ref().unwrap().read_only_hint,
                Some(true)
            );
        }
        // The generated effect documentation shares the compact form, so the
        // range never reaches an AddEffect/SetEffectParam schema either.
        let add_effect = operation_tools()
            .unwrap()
            .into_iter()
            .find(|definition| definition.tool.name == "add_effect")
            .unwrap();
        let description = add_effect.tool.description.as_deref().unwrap_or_default();
        assert!(description.contains("lut_asset_id (project LUT asset id; see list_look_assets"));
        assert!(!description.contains("9007199254740991"));
    }

    /// CC4 §8, §9: the CC4 tools join the internal registry as read-only
    /// planners/inspectors plus two confirmed destructive actions, and none of
    /// them reaches the seven-tool served surface.
    #[test]
    fn cc4_agent_surface_registers_the_look_capabilities() {
        let registry = KinewrightMcp::capability_tools().unwrap();
        let served = KinewrightMcp::served_tools().unwrap();
        let catalog = capabilities(&registry);
        for (name, kind) in [
            ("plan_technical_lut", CapabilityKind::Planner),
            ("plan_creative_look", CapabilityKind::Planner),
            ("list_look_assets", CapabilityKind::Inspector),
            ("import_lut_asset", CapabilityKind::Action),
            // CC4 §9: the hand-written conversion capability replaces the
            // generated `ConvertLegacyLook` tool, whose published batch was
            // unsubmittable whenever it opened with `AddLutAsset`.
            ("convert_legacy_look", CapabilityKind::Action),
        ] {
            assert!(
                registry.iter().any(|tool| tool.name == name),
                "{name} must be an internal capability"
            );
            assert!(
                !served.iter().any(|tool| tool.name == name),
                "{name} must stay off the served surface"
            );
            let descriptor = catalog
                .iter()
                .find(|descriptor| descriptor.name == name)
                .unwrap_or_else(|| panic!("{name} must appear in the capability directory"));
            assert_eq!(descriptor.kind, kind);
        }
        for name in ["import_lut_asset", "convert_legacy_look"] {
            let action = registry.iter().find(|tool| tool.name == name).unwrap();
            let annotations = action.annotations.as_ref().unwrap();
            assert_eq!(annotations.read_only_hint, Some(false), "{name}");
            assert_eq!(annotations.destructive_hint, Some(true), "{name}");
        }
        assert_eq!(
            registry
                .iter()
                .filter(|tool| tool.name == "convert_legacy_look")
                .count(),
            1,
            "the hand-written capability must replace the generated operation tool, not duplicate its name"
        );
        assert!(
            crate::runtime::is_invocable_capability("convert_legacy_look"),
            "conversion must be reachable through the compact dispatcher"
        );
    }

    /// CC4 §8: `render_color_proof` refuses the argument combinations it
    /// cannot answer honestly, and a LUT node is *not* one of them: it is
    /// carried all the way to the renderer, which fails on its own terms when
    /// it has no lattice.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc4_render_color_proof_validates_look_arguments_and_renders_lut_nodes() {
        let (core, playback, analysis) = fixture();
        let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

        let conflict = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 100)]),
                effect_id: Some(EffectId(1)),
                look_comparison: None,
                matte_comparison: None,
            })
            .unwrap();
        assert_eq!(conflict.is_error, Some(true));
        assert_eq!(
            conflict.structured_content.unwrap()["code"],
            "look_proof_parameters_conflict"
        );

        let orphan = service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: None,
                look_comparison: Some(LookComparison::Bypass),
                matte_comparison: None,
            })
            .unwrap();
        assert_eq!(orphan.is_error, Some(true));
        assert_eq!(
            orphan.structured_content.unwrap()["code"],
            "look_comparison_requires_effect_id"
        );

        // CC3 §5: a CC1 primary has no bypass control, so the bypass variant
        // is refused rather than synthesized with an invalid SetEffectParam.
        let (_, seeded) = service.snapshot().unwrap();
        let mut with_primary = (*seeded).clone();
        with_primary.tracks[0].clips[0].effects = vec![Effect {
            id: EffectId(4),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(250),
            )]),
            keyframes: BTreeMap::new(),
        }];
        with_primary.validate().unwrap();
        let primary_service = KinewrightMcp::new(
            Core::spawn(with_primary).unwrap(),
            Arc::new(NoopMedia::default()),
            Arc::new(NoopMedia::default()),
            ConfirmationBroker::default(),
        );
        let unsupported = primary_service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(4)),
                look_comparison: Some(LookComparison::Bypass),
                matte_comparison: None,
            })
            .unwrap();
        assert_eq!(unsupported.is_error, Some(true));
        let structured = unsupported.structured_content.unwrap();
        assert_eq!(structured["code"], "bypass_unsupported_for_node");
        assert_eq!(
            structured["details"]["allowed"],
            serde_json::json!(["before", "after"])
        );

        // A LUT node reaches the renderer like any other managed node.
        let (_, document) = service.snapshot().unwrap();
        let mut with_look = (*document).clone();
        with_look.lut_assets =
            vec![kinewright_media::BuiltinLook::Warm.to_lut_asset(kinewright_core::LutAssetId(1))];
        with_look.tracks[0].clips[0].effects = vec![Effect {
            id: EffectId(9),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
            keyframes: BTreeMap::new(),
        }];
        with_look
            .validate()
            .expect("the CC4 stack is a valid document");
        let look_service = KinewrightMcp::new(
            Core::spawn(with_look).unwrap(),
            Arc::new(NoopMedia::default()),
            Arc::new(NoopMedia::default()),
            ConfirmationBroker::default(),
        );
        let refused = look_service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(1),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id: Some(EffectId(9)),
                look_comparison: Some(LookComparison::Bypass),
                matte_comparison: None,
            })
            .unwrap();
        // The LUT node is no longer refused up front. The proof proceeds to
        // the renderer, and the `NoopMedia` double has no decoder, so the
        // failure is the render-stage error that double produces - named
        // exactly, not asserted by exclusion.
        assert_eq!(refused.is_error, Some(true));
        let structured = refused.structured_content.unwrap();
        assert_eq!(
            structured["code"], "needs_color_override",
            "the fixture's default source description is what stops this proof, \
             not the LUT node: {structured}"
        );
    }
}
