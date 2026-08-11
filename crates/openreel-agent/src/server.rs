use std::{
    collections::HashMap,
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
    Analysis, AssetId, ClipId, Command, Core, Document, Event, Operation, Playback, Query,
    QueryResult, SceneStatus, SilenceStatus, TimeCode, TimelineSceneChange, TimelineSilenceSpan,
    TimelineTranscriptWord, TranscriptStatus,
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
const DEFAULT_CONFIRMATION_TIMEOUT: Duration = Duration::from_mins(1);
const DEFAULT_MINIMUM_SILENCE_FRAMES: i64 = 6;
const DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;

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

    fn start_with_broker(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Result<Self, McpServerError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(McpServerError::Bind)?;
        let address = listener.local_addr().map_err(McpServerError::Listener)?;
        listener
            .set_nonblocking(true)
            .map_err(McpServerError::Listener)?;
        let endpoint = format!("http://{address}/mcp");
        let (shutdown, shutdown_rx) = oneshot::channel();
        let handler = OpenReelMcp::new(core, playback, analysis, confirmations.clone());
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
}

impl OpenReelMcp {
    fn new(
        core: Core,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        confirmations: ConfirmationBroker,
    ) -> Self {
        Self {
            core,
            playback,
            analysis,
            confirmations,
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

    fn call_blocking(&self, request: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "get_timeline_state" => {
                let document = self.document()?;
                Ok(success_text(render_timeline_state(&document)))
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
            "apply_edit_plan" => {
                let args: EditPlanArgs = decode_args("apply_edit_plan", arguments)?;
                self.apply_edit_plan(&args.operations)
            }
            "import_media" => {
                let args: ImportMediaArgs = decode_args("import_media", arguments)?;
                Ok(self.import_media(&args.path))
            }
            tool_name => {
                let operation = decode_operation(tool_name, arguments)
                    .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
                if let Some(description) = self.confirmation_description(&operation)?
                    && let Err(reason) = self.confirmations.confirm(tool_name, description)
                {
                    return Ok(error_text(format!(
                        "refused destructive tool {tool_name}: {reason}"
                    )));
                }
                Ok(self.apply_operation(tool_name, operation))
            }
        }
    }

    fn confirmation_description(&self, operation: &Operation) -> Result<Option<String>, McpError> {
        match operation {
            Operation::DeleteClip { clip } | Operation::RippleDeleteClip { clip } => Ok(Some(
                format!("The agent wants to delete clip {clip}. This edit can be undone."),
            )),
            Operation::RemoveTrack { track } => {
                let document = self.document()?;
                let Some(track) = document
                    .tracks
                    .iter()
                    .find(|candidate| candidate.id == *track)
                else {
                    return Ok(None);
                };
                if track.clips.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(format!(
                        "The agent wants to remove track {} and its {} clip(s). This edit can be undone.",
                        track.id,
                        track.clips.len()
                    )))
                }
            }
            _ => Ok(None),
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

    fn apply_operation(&self, tool_name: &str, operation: Operation) -> CallToolResult {
        let imported_asset = match &operation {
            Operation::AddAsset { asset } => Some(asset.clone()),
            _ => None,
        };
        let before = self.document().ok();
        match self.core.request(Command::Do(operation)) {
            Ok(Event::DocumentChanged { doc, .. }) => {
                self.playback.set_document(Arc::clone(&doc));
                if let Some(asset) = imported_asset {
                    self.request_asset_analysis(asset);
                }
                success_text(state_delta(tool_name, before.as_deref(), &doc))
            }
            Ok(Event::OpRejected { error, .. }) => error_text(error.to_string()),
            Ok(Event::BatchRejected { error, .. }) => error_text(error.to_string()),
            Ok(_) => error_text("Core returned the wrong operation result"),
            Err(error) => error_text(error.to_string()),
        }
    }

    fn import_media(&self, path: &Path) -> CallToolResult {
        let asset = match self.analysis.probe(path) {
            Ok(asset) => asset,
            Err(error) => return error_text(error.to_string()),
        };
        self.apply_operation("import_media", Operation::AddAsset { asset })
    }

    fn apply_edit_plan(&self, operations: &[Operation]) -> Result<CallToolResult, McpError> {
        let before = self.document()?;
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
            .request(Command::DoBatch(operations.to_vec()))
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(match event {
            Event::DocumentChanged { doc, .. } => {
                self.playback.set_document(Arc::clone(&doc));
                for asset in added_assets {
                    self.request_asset_analysis(asset);
                }
                success_text(render_plan_outcomes(
                    operations,
                    None,
                    Some(state_delta("apply_edit_plan", Some(&before), &doc)),
                ))
            }
            Event::BatchRejected { error, .. } => {
                error_text(render_plan_outcomes(operations, Some(&error), None))
            }
            _ => error_text("Core returned the wrong edit-plan result"),
        })
    }

    fn request_asset_analysis(&self, asset: openreel_core::MediaAsset) {
        self.analysis.request_transcription(asset.clone());
        self.analysis.request_silence_detection(asset.clone());
        self.analysis.request_scene_detection(asset);
    }

    fn frame_at(&self, timecode: TimeCode) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if timecode < TimeCode::ZERO || timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                timecode.0, document.duration.0
            )));
        }
        self.playback.set_document(document);
        let image = match self.analysis.thumbnail_at(timecode, THUMBNAIL_MAX_WIDTH) {
            Ok(image) => image,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                &image.pixels,
                image.width,
                image.height,
                ColorType::Rgba8.into(),
            )
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![
            ContentBlock::text(format!(
                "project frame {} ({}x{})",
                timecode.0, image.width, image.height
            )),
            ContentBlock::image(BASE64.encode(png), "image/png"),
        ]))
    }

    fn asset_transcript(&self, asset_id: AssetId) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        let Some(asset) = document.asset(asset_id) else {
            return Ok(error_text(format!("asset {asset_id} does not exist")));
        };
        let mut status = self.analysis.transcript_status(asset_id);
        if status == TranscriptStatus::NotRequested {
            self.analysis.request_transcription(asset.clone());
            status = self.analysis.transcript_status(asset_id);
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
        let mut status = self.analysis.silence_status(asset_id);
        if status == SilenceStatus::NotRequested {
            self.analysis.request_silence_detection(asset.clone());
            status = self.analysis.silence_status(asset_id);
        }
        Ok(success_text(render_asset_silences(
            asset_id, &status, minimum,
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
        let mut status = self.analysis.scene_status(asset_id);
        if status == SceneStatus::NotRequested {
            self.analysis.request_scene_detection(asset.clone());
            status = self.analysis.scene_status(asset_id);
        }
        Ok(success_text(render_asset_scene_changes(
            asset_id, &status, minimum,
        )))
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
            if self.analysis.transcript_status(asset.id) == TranscriptStatus::NotRequested {
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
            let status = self.analysis.transcript_status(asset.id);
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
            if self.analysis.silence_status(asset.id) == SilenceStatus::NotRequested {
                self.analysis.request_silence_detection(asset.clone());
            }
        }
        let spans: Vec<TimelineSilenceSpan> = match self.analysis.timeline_silences(
            &document,
            Some(range.clone()),
            TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
        ) {
            Ok(spans) => spans,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        let mut rendered = render_timeline_silences(&document, range, &spans);
        for asset in &document.media_pool {
            let status = self.analysis.silence_status(asset.id);
            if !matches!(status, SilenceStatus::Ready(_) | SilenceStatus::NoAudio) {
                rendered.push('\n');
                rendered.push_str(&render_asset_silences(
                    asset.id,
                    &status,
                    TimeCode(DEFAULT_MINIMUM_SILENCE_FRAMES),
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
            if self.analysis.scene_status(asset.id) == SceneStatus::NotRequested {
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
            let status = self.analysis.scene_status(asset.id);
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
}

impl ServerHandler for OpenReelMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openreel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Inspect the timeline, transcript, silences, and scene changes before editing. Resolve ordinal targets against the initial timeline state. Frame values are exact project frames. Prefer one atomic apply_edit_plan after inspection instead of separate operation tools. Use import_media for filesystem paths.",
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
struct ImportMediaArgs {
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
struct TimelineDerivedArgs {
    /// Optional half-open range in exact project frames. Omit for the full timeline.
    #[serde(default)]
    range: Option<TranscriptRangeArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditPlanArgs {
    /// Ordered operations. Each item uses the generated Operation schema and sees prior effects.
    operations: Vec<Operation>,
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
            "Return the compact live project state: tracks, clips, ids, timeline/source ranges in frames and seconds, and assets.",
            schema_object::<EmptyArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_silences",
            "Return cached windowed-RMS silence spans for one asset in exact source frames and seconds, or background analysis status. Reported spans are pre-shrunk by a 100 ms speech-safety margin on each side for safe cutting; cached detector spans remain unchanged.",
            schema_object::<SilencesArgs>(),
        )
        .with_annotations(read_only()),
        Tool::new(
            "get_timeline_silences",
            "Return cached silence spans mapped through clips to exact project frames and seconds. Reported spans are pre-shrunk in source space by a 100 ms speech-safety margin on each side for safe cutting before project mapping; cached detector spans remain unchanged.",
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
            "apply_edit_plan",
            "Atomically validate and apply an ordered array of generated OpenReel Operations as one undo entry.",
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
            "Probe a media path, then add the resulting asset metadata through Operation::AddAsset.",
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

fn error_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn state_delta(tool_name: &str, before: Option<&Document>, after: &Document) -> String {
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
            "applied {tool_name}; tracks {before_tracks}->{after_tracks}, clips {before_clips}->{after_clips}, assets {before_assets}->{after_assets}, duration {}f->{}f",
            before.duration.0, after.duration.0
        )
    } else {
        format!(
            "applied {tool_name}; tracks={after_tracks}, clips={after_clips}, assets={after_assets}, duration={}f",
            after.duration.0
        )
    }
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
    if !confidence.is_finite() || !(0.0..=100.0).contains(&confidence) {
        return Err("min_confidence must be between 0 and 100 percent".to_owned());
    }
    Ok((confidence * 100.0).round() as u16)
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
        MediaKind, Rational, RgbaImage, SceneStatus, SilenceStatus, TimelineSceneChange,
        TimelineSilenceSpan, Track, TrackId, TrackKind, VisualAssetResult,
    };
    use serde_json::json;
    use std::{path::Path, time::Instant};

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
    }

    impl Analysis for NoopMedia {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn request_transcription(&self, _asset: MediaAsset) {}

        fn transcript_status(&self, _asset: AssetId) -> TranscriptStatus {
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

        fn silence_status(&self, _asset: AssetId) -> SilenceStatus {
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

        fn scene_status(&self, _asset: AssetId) -> SceneStatus {
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
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode::ZERO..TimeCode(60),
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
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

    fn delete_request() -> CallToolRequestParams {
        CallToolRequestParams::new("delete_clip")
            .with_arguments(json!({"clip": 1}).as_object().unwrap().clone())
    }

    fn plan_request(operations: serde_json::Value) -> CallToolRequestParams {
        CallToolRequestParams::new("apply_edit_plan").with_arguments(serde_json::Map::from_iter([
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
        let request = CallToolRequestParams::new("remove_track")
            .with_arguments(json!({"track": 1}).as_object().unwrap().clone());
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
    fn ripple_delete_is_destructive_while_marker_edits_are_suggestions() {
        let (core, playback, analysis) = fixture();
        let service = OpenReelMcp::new(core, playback, analysis, ConfirmationBroker::default());
        assert!(
            service
                .confirmation_description(&Operation::RippleDeleteClip { clip: ClipId(1) })
                .unwrap()
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
        ] {
            assert!(
                service
                    .confirmation_description(&operation)
                    .unwrap()
                    .is_none()
            );
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
