use std::{
    collections::HashMap,
    future::Future,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::PathBuf,
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
    ClipId, Command, Core, Document, Event, MediaEngine, Operation, Query, QueryResult, TimeCode,
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
    render::{render_clip_info, render_timeline_state},
    schema::{SchemaError, decode_operation, operation_tools, schema_object},
};

const THUMBNAIL_MAX_WIDTH: u32 = 512;
const DEFAULT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);

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
            .map(|mut pending| pending.drain().map(|(_, sender)| sender).collect::<Vec<_>>())
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

    fn confirm(
        &self,
        tool_name: &str,
        description: String,
    ) -> Result<(), String> {
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
    pub fn start(core: Core, media: Arc<dyn MediaEngine>) -> Result<Self, McpServerError> {
        Self::start_with_broker(core, media, ConfirmationBroker::default())
    }

    fn start_with_broker(
        core: Core,
        media: Arc<dyn MediaEngine>,
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
        let handler = OpenReelMcp::new(core, media, confirmations.clone());
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
                Default::default(),
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
    media: Arc<dyn MediaEngine>,
    confirmations: ConfirmationBroker,
}

impl OpenReelMcp {
    fn new(
        core: Core,
        media: Arc<dyn MediaEngine>,
        confirmations: ConfirmationBroker,
    ) -> Self {
        Self {
            core,
            media,
            confirmations,
        }
    }

    fn tools(&self) -> Result<Vec<Tool>, SchemaError> {
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
            "import_media" => {
                let args: ImportMediaArgs = decode_args("import_media", arguments)?;
                self.import_media(args.path)
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

    fn confirmation_description(
        &self,
        operation: &Operation,
    ) -> Result<Option<String>, McpError> {
        match operation {
            Operation::DeleteClip { clip } => Ok(Some(format!(
                "The agent wants to delete clip {clip}. This edit can be undone."
            ))),
            Operation::RemoveTrack { track } => {
                let document = self.document()?;
                let Some(track) = document.tracks.iter().find(|candidate| candidate.id == *track)
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
        let before = self.document().ok();
        match self.core.request(Command::Do(operation)) {
            Ok(Event::DocumentChanged { doc, .. }) => {
                self.media.set_document(Arc::clone(&doc));
                success_text(state_delta(tool_name, before.as_deref(), &doc))
            }
            Ok(Event::OpRejected { error, .. }) => error_text(error.to_string()),
            Ok(_) => error_text("Core returned the wrong operation result"),
            Err(error) => error_text(error.to_string()),
        }
    }

    fn import_media(&self, path: PathBuf) -> Result<CallToolResult, McpError> {
        let asset = match self.media.probe(&path) {
            Ok(asset) => asset,
            Err(error) => return Ok(error_text(error.to_string())),
        };
        Ok(self.apply_operation("import_media", Operation::AddAsset { asset }))
    }

    fn frame_at(&self, timecode: TimeCode) -> Result<CallToolResult, McpError> {
        let document = self.document()?;
        if timecode < TimeCode::ZERO || timecode >= document.duration {
            return Ok(error_text(format!(
                "frame {} is outside project range 0..{}",
                timecode.0, document.duration.0
            )));
        }
        self.media.set_document(document);
        let image = match self.media.thumbnail_at(timecode, THUMBNAIL_MAX_WIDTH) {
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
}

impl ServerHandler for OpenReelMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openreel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Inspect the timeline before editing. Frame values are exact project frames. Use import_media for filesystem paths.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let result = self
            .tools()
            .map(ListToolsResult::with_all_items)
            .map_err(|error| McpError::internal_error(error.to_string(), None));
        std::future::ready(result)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools()
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
    /// Stable clip id shown by get_timeline_state.
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

#[cfg(test)]
mod tests {
    use super::*;
    use openreel_core::{
        AssetId, Clip, ExportSettings, FrameTexture, MediaAsset, MediaError,
        MediaEvent, MediaKind, ProgressSink, Rational, RgbaImage, Track, TrackId, TrackKind,
    };
    use serde_json::json;
    use std::{path::Path, time::Instant};

    struct NoopMedia;

    impl MediaEngine for NoopMedia {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

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

        fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn export(
            &self,
            _out: &Path,
            _settings: ExportSettings,
            _progress: ProgressSink,
        ) -> Result<(), MediaError> {
            Err(MediaError::NotImplemented)
        }
    }

    fn fixture() -> (Core, Arc<dyn MediaEngine>) {
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
                }],
            }],
            media_pool: vec![asset],
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(60),
        };
        (Core::spawn(document).unwrap(), Arc::new(NoopMedia))
    }

    fn delete_request() -> CallToolRequestParams {
        CallToolRequestParams::new("delete_clip").with_arguments(
            json!({"clip": 1}).as_object().unwrap().clone(),
        )
    }

    fn wait_for_request(broker: &ConfirmationBroker) -> ConfirmationRequest {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(request) = broker.pending_requests().into_iter().next() {
                return request;
            }
            assert!(Instant::now() < deadline, "confirmation request was not published");
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
        let (core, media) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let result = invoke_in_background(
            OpenReelMcp::new(core.clone(), media, broker.clone()),
            delete_request(),
        );
        let request = wait_for_request(&broker);
        assert!(broker.approve(request.id));
        let result = result.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
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
        let (core, media) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let result = invoke_in_background(
            OpenReelMcp::new(core, media, broker.clone()),
            delete_request(),
        );
        let request = wait_for_request(&broker);
        assert!(broker.reject(request.id, "rejected by user"));
        let result = result.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("rejected by user"));
    }

    #[test]
    fn confirmation_timeout_rejects_the_operation() {
        let (core, media) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_millis(10));
        let service = OpenReelMcp::new(core, media, broker);
        let result = service.call_blocking(delete_request()).unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("timed out"));
    }

    #[test]
    fn interrupting_a_pending_confirmation_does_not_deadlock() {
        let (core, media) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(30));
        let result = invoke_in_background(
            OpenReelMcp::new(core, media, broker.clone()),
            delete_request(),
        );
        let _request = wait_for_request(&broker);
        broker.reject_all("session interrupted");
        let result = result.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("session interrupted"));
    }

    #[test]
    fn removing_a_nonempty_track_requires_confirmation() {
        let (core, media) = fixture();
        let broker = ConfirmationBroker::with_timeout(Duration::from_secs(1));
        let request = CallToolRequestParams::new("remove_track").with_arguments(
            json!({"track": 1}).as_object().unwrap().clone(),
        );
        let result = invoke_in_background(
            OpenReelMcp::new(core, media, broker.clone()),
            request,
        );
        let request = wait_for_request(&broker);
        assert!(request.description.contains("1 clip(s)"));
        assert!(broker.reject(request.id, "keep the track"));
        let result = result.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(result.is_error, Some(true));
    }
}
