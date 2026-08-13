use std::{
    collections::{BTreeMap, HashMap},
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

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{ColorType, ImageEncoder as _, codecs::png::PngEncoder};
use openreel_core::{
    Analysis, AnalysisKind, AssetId, BeatStatus, CaptionPreset, ClipId, Command, Core,
    DeliveryAspect, DeliveryVariant, Document, Event, Operation, Playback, Query, QueryResult,
    SceneStatus, SilenceStatus, TimeCode, TimelineBeat, TimelineRevision, TimelineSceneChange,
    TimelineSilenceSpan, TimelineTranscriptWord, TranscriptStatus, caption_cues,
    caption_title_operations, dedup_timeline_words, document_for_delivery_variant, qa_document,
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
    render::{
        render_asset_scene_changes, render_asset_silences, render_asset_transcript,
        render_clip_info, render_timeline_scene_changes, render_timeline_silences,
        render_timeline_state, render_timeline_transcript,
    },
    schema::{SchemaError, decode_operation, operation_tool_name, operation_tools, schema_object},
};

const THUMBNAIL_MAX_WIDTH: u32 = 512;
const STORYBOARD_DEFAULT_FRAMES: u8 = 9;
const STORYBOARD_MAX_FRAMES: u8 = 16;
const STORYBOARD_DEFAULT_CELL_WIDTH: u32 = 320;
const STORYBOARD_COLUMNS: u32 = 4;
const STORYBOARD_GUTTER: u32 = 4;
const DEFAULT_CONFIRMATION_TIMEOUT: Duration = Duration::from_mins(1);
const DEFAULT_MINIMUM_SILENCE_FRAMES: i64 = 6;
const DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;
const DEFAULT_BEAT_STRENGTH_BASIS_POINTS: u16 = 1_000;

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
    #[error("could not bind the OpenReel MCP server: {0}")]
    Bind(#[source] std::io::Error),
    #[error("could not configure the OpenReel MCP listener: {0}")]
    Listener(#[source] std::io::Error),
    #[error("could not start the OpenReel MCP server thread: {0}")]
    Thread(#[source] std::io::Error),
}

pub struct McpServer {
    endpoint: String,
    confirmations: ConfirmationBroker,
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
        Self::start_configured(core, playback, analysis, confirmations, true)
    }

    fn start_configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
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
        let handler = OpenReelMcp::configured(
            core,
            playback,
            analysis,
            confirmations.clone(),
            publish_to_playback,
        );
        let server_thread = thread::Builder::new()
            .name("openreel-mcp".to_owned())
            .spawn(move || run_server(listener, handler, shutdown_rx))
            .map_err(McpServerError::Thread)?;
        Ok(Self {
            endpoint,
            confirmations,
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

    pub fn shutdown(mut self) {
        self.signal_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn signal_shutdown(&mut self) {
        self.confirmations
            .reject_all("the OpenReel agent session was interrupted");
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

fn run_server(listener: TcpListener, handler: OpenReelMcp, shutdown: oneshot::Receiver<()>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("openreel-mcp-worker")
        .build()
        .expect("OpenReel MCP Tokio runtime must start");
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .expect("validated MCP listener must enter Tokio");
        let service: StreamableHttpService<OpenReelMcp, LocalSessionManager> =
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
struct OpenReelMcp {
    core: Core,
    playback: Arc<dyn Playback>,
    analysis: Arc<dyn Analysis>,
    confirmations: ConfirmationBroker,
    publish_to_playback: bool,
}

impl OpenReelMcp {
    #[cfg(test)]
    fn new(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Self {
        Self::configured(core, playback, analysis, confirmations, true)
    }

    fn configured(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
        publish_to_playback: bool,
    ) -> Self {
        Self {
            core,
            playback,
            analysis,
            confirmations,
            publish_to_playback,
        }
    }

    fn tools() -> Result<Vec<Tool>, SchemaError> {
        let mut tools = operation_tools()?
            .into_iter()
            .map(|definition| definition.tool)
            .collect::<Vec<_>>();
        tools.extend(inspector_tools());
        Ok(tools)
    }

    #[allow(clippy::too_many_lines)]
    fn call_blocking(&self, request: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
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
            "get_frame_at" => {
                let args: FrameAtArgs = decode_args("get_frame_at", arguments)?;
                self.frame_at(args.timecode)
            }
            "get_timeline_storyboard" => {
                let args: StoryboardArgs = decode_args("get_timeline_storyboard", arguments)?;
                self.timeline_storyboard(args)
            }
            "get_transcript" => {
                let args: TranscriptArgs = decode_args("get_transcript", arguments)?;
                self.asset_transcript(args.asset_id)
            }
            "get_timeline_transcript" => {
                let args: TimelineTranscriptArgs =
                    decode_args("get_timeline_transcript", arguments)?;
                self.timeline_transcript(args.range)
            }
            "get_silences" => {
                let args: SilencesArgs = decode_args("get_silences", arguments)?;
                self.asset_silences(args.asset_id, args.min_duration_frames)
            }
            "get_timeline_silences" => {
                let args: TimelineDerivedArgs = decode_args("get_timeline_silences", arguments)?;
                self.timeline_silences(args.range)
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
            "get_analysis_status" => {
                let args: AnalysisStatusArgs = decode_args("get_analysis_status", arguments)?;
                self.analysis_status(args.asset_id)
            }
            "get_caption_presets" => Ok(Self::caption_presets()),
            "add_styled_captions" => {
                let args: StyledCaptionsArgs = decode_args("add_styled_captions", arguments)?;
                self.add_styled_captions(args.expected_revision, args.preset)
            }
            "get_qa_report" => Ok(self.qa_report()?),
            "get_delivery_variants" => Ok(Self::delivery_variants()),
            "get_delivery_variant_storyboard" => {
                let args: DeliveryStoryboardArgs =
                    decode_args("get_delivery_variant_storyboard", arguments)?;
                self.delivery_variant_storyboard(args)
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

    fn request_asset_analysis(&self, asset: openreel_core::MediaAsset) {
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
            })
        });
        success_text(
            serde_json::to_string_pretty(&presets)
                .unwrap_or_else(|error| format!("could not serialize presets: {error}")),
        )
    }

    fn add_styled_captions(
        &self,
        expected_revision: TimelineRevision,
        preset: CaptionPreset,
    ) -> Result<CallToolResult, McpError> {
        let (actual_revision, document) = self.snapshot()?;
        if expected_revision != actual_revision {
            return Ok(revision_conflict_text(expected_revision, actual_revision));
        }
        let words = self
            .analysis
            .timeline_transcript(&document, None)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let words = dedup_timeline_words(words);
        let cues = caption_cues(&words, document.fps);
        let operations = match caption_title_operations(&document, &cues, preset) {
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
            "error_count": report.count(openreel_core::QaSeverity::Error),
            "warning_count": report.count(openreel_core::QaSeverity::Warning),
            "info_count": report.count(openreel_core::QaSeverity::Info),
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

    /// Deterministic completion feedback: the plan result itself reports how
    /// much cuttable silence remains, so an agent asked to remove dead air
    /// cannot mistake a partial plan for a finished one.
    fn remaining_silence_footer(&self, document: &openreel_core::Document) -> String {
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
                openreel_core::SilenceSpan {
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

    fn timeline_storyboard(&self, args: StoryboardArgs) -> Result<CallToolResult, McpError> {
        let (revision, document) = self.snapshot()?;
        self.storyboard_for_document(revision, &document, args, "timeline storyboard", None)
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

    fn timeline_silences(
        &self,
        requested: Option<TranscriptRangeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let range = validated_timeline_range(&document, requested, "timeline silence")?;
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
        let spans: Vec<TimelineSilenceSpan> = match self.analysis.timeline_silences(
            &document,
            Some(range.clone()),
            TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
        ) {
            Ok(spans) => spans,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_silences(&document, range, &spans, &transcripts);
        for asset in &document.media_pool {
            let status = self.analysis.silence_status(asset);
            if !matches!(status, SilenceStatus::Ready(_) | SilenceStatus::NoAudio) {
                rendered.push('\n');
                rendered.push_str(&render_asset_silences(
                    asset.id,
                    &status,
                    TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
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
}

impl ServerHandler for OpenReelMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openreel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
        "Inspect the timeline before editing and copy its timeline_revision into expected_revision on every mutation. Reinspect and re-plan after a revision conflict. Use the storyboard, transcript, silence, beat, and scene inspectors when relevant. Resolve ordinal targets against the inspected state. Frame values are exact project frames. Prefer one atomic apply_edit_plan after inspection instead of separate operation tools. Use add_title for first-class video-track titles and import_media for filesystem paths.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let result = Self::tools()
            .map(ListToolsResult::with_all_items)
            .map_err(|error| McpError::internal_error(error.to_string(), None));
        std::future::ready(result)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::tools()
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
            tokio::task::spawn_blocking(move || service.call_blocking(request))
                .await
                .map_err(|error| McpError::internal_error(error.to_string(), None))?
                .map(Into::into)
        }
    }
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

const fn default_delivery_focus() -> u8 {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ImportMediaArgs {
    /// Exact revision returned by `get_timeline_state` before planning this import.
    expected_revision: TimelineRevision,
    /// Absolute or working-directory-relative path on the user's machine.
    path: PathBuf,
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
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TimelineDerivedArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
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
struct TranscriptRangeArgs {
    start: TimeCode,
    end: TimeCode,
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
            "get_silences",
            "Return cached windowed-RMS silence spans for one asset in exact source frames and seconds, or background analysis status. For safe cutting, reported spans are clamped against cached transcribed words plus a 100 ms fps-aware margin; when no transcript is cached, the existing fixed 100 ms margin is used. Cached detector spans remain unchanged.",
            schema_object::<SilencesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_silences",
            "Return cached silence spans mapped through clips to exact project frames and seconds. For safe cutting, reported spans are clamped in source space against cached transcribed words plus a 100 ms fps-aware margin before project mapping; when no transcript is cached, the existing fixed 100 ms margin is used. Cached detector spans remain unchanged.",
            schema_object::<TimelineDerivedArgs>(),
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
            "add_styled_captions",
            "Build transcript-timed burned-in captions using one stable preset and apply them as one revision-gated undo entry.",
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
            "Run deterministic export-health checks for missing media, gaps, abrupt cuts, retimed audio, and caption readability.",
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
            "get_delivery_variant_storyboard",
            "Render a real-compositor storyboard for a non-destructive delivery aspect using an explicit 0..=100 focal point. This is deterministic cover framing, not learned subject tracking.",
            schema_object::<DeliveryStoryboardArgs>(),
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
            "Atomically validate and apply ordered OpenReel Operations as one undo entry, only when expected_revision matches. Accepts the generated enum envelope and compact objects such as {\"op\":\"split_clip\",\"clip\":1,\"at\":30}.",
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
            "get_frame_at",
            "Render an actual PNG image at an exact project frame, downscaled to at most 512 pixels wide.",
            schema_object::<FrameAtArgs>(),
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
            "get_timeline_transcript",
            "Return audible words mapped through clips to exact project frames and seconds. Use these boundaries for precise TrimClip, SplitClip, and DeleteClip edits.",
            schema_object::<TimelineTranscriptArgs>(),
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

fn encode_png(image: &openreel_core::RgbaImage) -> Result<Vec<u8>, McpError> {
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

fn compose_contact_sheet(
    images: &[openreel_core::RgbaImage],
) -> Result<openreel_core::RgbaImage, McpError> {
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
    Ok(openreel_core::RgbaImage {
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
    error: Option<&openreel_core::BatchError>,
    summary: Option<String>,
) -> String {
    let failed = match error {
        Some(openreel_core::BatchError::OperationFailed { op_number, .. }) => Some(*op_number),
        _ => None,
    };
    let mut output = String::new();
    for (index, operation) in operations.iter().enumerate() {
        let number = index + 1;
        let outcome = match (error, failed) {
            (None, _) => "applied".to_owned(),
            (Some(openreel_core::BatchError::Empty), _) => "not run: empty plan".to_owned(),
            (Some(openreel_core::BatchError::OperationFailed { error, .. }), Some(failed))
                if number == failed =>
            {
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
    use openreel_core::{
        AssetId, Clip, FrameTexture, Marker, MarkerId, MediaAsset, MediaError, MediaEvent,
        MediaKind, ParamValue, Rational, RgbaImage, SceneStatus, SilenceStatus,
        TimelineSceneChange, TimelineSilenceSpan, Title, Track, TrackId, TrackKind,
        VisualAssetResult,
    };
    use serde_json::json;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
        time::Instant,
    };

    struct NoopMedia;

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

        fn transcript_status(&self, _asset: &MediaAsset) -> TranscriptStatus {
            TranscriptStatus::NotRequested
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

        fn request_scene_detection(&self, _asset: MediaAsset) {}

        fn scene_status(&self, _asset: &MediaAsset) -> SceneStatus {
            SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
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
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode::ZERO..TimeCode(60),
                    content: openreel_core::ClipContent::Media,
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
        let media = Arc::new(NoopMedia);
        (Core::spawn(document).unwrap(), media.clone(), media)
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
        let service = OpenReelMcp::configured(
            core,
            playback.clone(),
            analysis,
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
        let names = OpenReelMcp::tools()
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();
        for name in [
            "get_caption_presets",
            "add_styled_captions",
            "get_qa_report",
            "get_delivery_variants",
            "get_delivery_variant_storyboard",
        ] {
            assert!(names.iter().any(|candidate| candidate == name));
        }

        let (core, playback, analysis) = fixture();
        let service = OpenReelMcp::new(core, playback, analysis, ConfirmationBroker::default());
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
        service: OpenReelMcp,
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
            OpenReelMcp::new(core.clone(), playback, analysis, broker.clone()),
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
            OpenReelMcp::new(core, playback, analysis, broker.clone()),
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
        let service = OpenReelMcp::new(core, playback, analysis, broker);
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
            OpenReelMcp::new(core, playback, analysis, broker.clone()),
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
            OpenReelMcp::new(core, playback, analysis, broker.clone()),
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
        let service = OpenReelMcp::new(core, playback, analysis, ConfirmationBroker::default());
        let document = service.document().unwrap();
        assert!(
            OpenReelMcp::confirmation_description(
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
            assert!(OpenReelMcp::confirmation_description(&document, &operation).is_none());
        }
    }

    #[test]
    fn generated_plan_schema_composes_the_operation_schema() {
        let tool = OpenReelMcp::tools()
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
        let red = openreel_core::RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 255, 0, 0, 255],
        };
        let blue = openreel_core::RgbaImage {
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
    fn edit_plan_applies_atomically_and_undoes_once() {
        let (core, playback, analysis) = fixture();
        let Event::QueryResult(QueryResult::Document(original)) =
            core.request(Command::Query(Query::Document)).unwrap()
        else {
            panic!("expected document");
        };
        let service = OpenReelMcp::new(
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
    fn mixed_validity_edit_plan_rejects_without_partial_state() {
        let (core, playback, analysis) = fixture();
        let service = OpenReelMcp::new(
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
                OpenReelMcp::new(core.clone(), playback, analysis, broker.clone()),
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
