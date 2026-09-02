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

mod captions;
mod color_nodes;
mod color_proof;
mod color_qc;
mod delivery;
mod inspection;
mod look_assets;
mod mattes;
mod media_relink;
mod planning;
mod source_program;
mod storyboards;
#[cfg(test)]
mod tests;
mod tracking;

use mattes::{
    MATTE_TRACK_DEAD_ZONE_BASIS_POINTS, MATTE_TRACK_MAX_STEP_BASIS_POINTS, MATTE_TRACKING_BOUNDARY,
};

#[cfg(test)]
pub(crate) use tracking::encode_reframe_subject_provenance;
pub(crate) use tracking::{
    ReframeSubjectProvenance, TrackedSubjectBounds, decode_reframe_subject_provenance,
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

/// The complete internal capability registry, for crate-level contract tests.
#[cfg(test)]
pub(crate) fn capability_registry_tools() -> Vec<Tool> {
    KinewrightMcp::capability_tools().expect("the capability registry must build")
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

/// The one stale-revision envelope every revision-gated tool publishes
/// (CC7 D-E2 closed 2026-09-02): the prose the older tools always carried,
/// plus the typed `stale_revision` code and both revisions in
/// `structured_content`, the shape `analyze_color_shot`, `plan_shot_match`,
/// and `get_color_qc` already used.
fn revision_conflict_text(expected: TimelineRevision, actual: TimelineRevision) -> CallToolResult {
    let message = format!("timeline revision conflict: expected {expected}, actual {actual}");
    error_structured(
        format!("{message}; call get_timeline_state and re-plan against the current revision"),
        serde_json::json!({
            "code": "stale_revision",
            "message": message,
            "details": {
                "expected_revision": expected.0,
                "actual_revision": actual.0,
            },
            "applied": false,
        }),
    )
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
fn percentage_to_basis_points(value: f64, field: &str) -> Result<u16, String> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(format!("{field} must be between 0 and 100 percent"));
    }
    Ok((value * 100.0).round() as u16)
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
