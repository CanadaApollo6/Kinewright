//! Product-owned, serialized named-project IO and authenticated UI-free sessions.
#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]
mod auth;
mod manual;
mod protocol;
pub use auth::HostAuth;
use auth::PinnedPath;
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use kinewright_core::{AgentDriver, AgentEvent, AttachmentState, Core, Document, SessionConfig};
use kinewright_project::{
    bundle::{BundleError, BundleSaveReport},
    bundle_copy::{BundleCopy, CopyStatus},
    bundle_records::{BranchId, ObjectRef},
    bundle_rescue::{RescueConsent, RescueSelection},
    bundle_switch::ProjectSelectionResult,
    editing_session::Ja1EditingSession,
    source_boundary::{
        BorrowedSnapshotBinding, BorrowedSourceDescriptor, BorrowedSourceReader,
        SOURCE_READ_CHUNK_BYTES,
    },
    storage_budget::StorageLimits,
};
pub use protocol::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    io::{BufRead, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Refused(#[from] HostRefusal),
    #[error("host protocol: {0}")]
    Protocol(String),
}
impl From<BundleError> for HostError {
    fn from(value: BundleError) -> Self {
        HostRefusal::from(value).into()
    }
}
impl HostError {
    fn refusal(&self) -> HostRefusal {
        match self {
            Self::Refused(r) => r.clone(),
            Self::Io(e) => HostRefusal {
                code: "io".into(),
                message: e.to_string(),
                native_code: e.raw_os_error(),
            },
            Self::Json(_) => HostRefusal::new("invalid_request", "invalid bounded protocol value"),
            Self::Protocol(s) => HostRefusal::new("host_protocol", s),
        }
    }
}

#[derive(Clone)]
pub struct HostClient {
    bootstrap: HostBootstrap,
    editor: Option<String>,
}
impl std::fmt::Debug for HostClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostClient")
            .field("endpoint", &self.bootstrap.endpoint)
            .finish_non_exhaustive()
    }
}
impl HostClient {
    #[allow(
        clippy::needless_pass_by_value,
        reason = "Client construction owns all attachment inputs and retains canonical owned roots"
    )]
    pub fn connect(
        endpoint: HostEndpoint,
        auth: HostAuth,
        roots: SessionRoots,
    ) -> Result<Self, HostError> {
        if !endpoint.address.ip().is_loopback()
            || endpoint.protocol != HOST_PROTOCOL
            || endpoint.project.is_empty()
            || endpoint.project.len() > 256
        {
            return Err(HostRefusal::new(
                "attachment_refused",
                "loopback, exact protocol and project identity required",
            )
            .into());
        }
        Ok(Self {
            editor: None,
            bootstrap: HostBootstrap {
                endpoint,
                auth,
                roots: roots.canonicalize()?,
            },
        })
    }
    #[must_use]
    pub fn with_roots(&self, roots: SessionRoots) -> Self {
        Self {
            editor: self.editor.clone(),
            bootstrap: HostBootstrap {
                roots,
                ..self.bootstrap.clone()
            },
        }
    }
    #[must_use]
    pub fn endpoint(&self) -> &HostEndpoint {
        &self.bootstrap.endpoint
    }
    #[must_use]
    pub fn bootstrap(&self) -> HostBootstrap {
        self.bootstrap.clone()
    }
    #[must_use]
    pub fn with_manual_editor(&self, private_client: String) -> Self {
        Self {
            bootstrap: self.bootstrap.clone(),
            editor: Some(private_client),
        }
    }
    pub fn request(&self, command: HostCommand) -> Result<HostReply, HostError> {
        let request = HostRequest {
            editor: self.editor.clone(),
            protocol: self.endpoint().protocol,
            project: self.endpoint().project.clone(),
            roots: self.bootstrap.roots.clone(),
            command,
        };
        let response = self.post("host", &serde_json::to_vec(&request)?)?;
        Ok(serde_json::from_slice(&response)?)
    }
    /// Admit one destination chosen explicitly by the person. Connector/MCP ingress refuses
    /// this command; returned roots only enable this exact retained destination capability.
    pub fn admit_destination(&self, destination: &Path) -> Result<Self, HostError> {
        let reply = self.request(HostCommand::AdmitDestination {
            destination: destination.into(),
        })?;
        if let Some(error) = reply.error {
            return Err(error.into());
        }
        Ok(self.with_roots(reply.status.admitted_roots))
    }
    fn post(&self, route: &str, body: &[u8]) -> Result<Vec<u8>, HostError> {
        let roots = self.bootstrap.roots.wire_header()?;
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| HostError::Protocol("could not construct loopback transport".into()))?
            .post(format!("http://{}/{route}", self.endpoint().address))
            .header("authorization", self.bootstrap.auth.bearer())
            .header("x-kinewright-project", &self.endpoint().project)
            .header("x-kinewright-protocol", HOST_PROTOCOL.to_string())
            .header("x-kinewright-roots", roots)
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .map_err(|_| {
                HostError::Protocol(
                    "host connection failed; retained ownership must be re-established".into(),
                )
            })?;
        if !response.status().is_success() {
            return Err(HostRefusal::new(
                "attachment_refused",
                "authentication, project, protocol, roots or ingress admission refused",
            )
            .into());
        }
        let mut bytes = Vec::new();
        response
            .take((HOST_BODY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > HOST_BODY_BYTES {
            return Err(
                HostRefusal::new("capacity", "host reply exceeds protocol capacity").into(),
            );
        }
        Ok(bytes)
    }
    /// Bootstrap is sent through a pipe; the child prints only public discovery.
    pub fn launch(executable: &Path, config: HostConfig) -> Result<(Self, Child), HostError> {
        let auth = HostAuth::generate()?;
        let bootstrap = serde_json::to_vec(&HostLaunch {
            config: config.clone(),
            auth: auth.clone(),
        })?;
        let mut child = Command::new(executable)
            .arg("host")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let pipes = child.stdin.take().zip(child.stdout.take());
        let Some((mut stdin, stdout)) = pipes else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HostError::Protocol("missing bootstrap pipes".into()));
        };
        let (ready, result) = crossbeam_channel::bounded(1);
        let reader = match thread::Builder::new()
            .name("kinewright-host-bootstrap".into())
            .spawn(move || {
                let read = || -> Result<HostEndpoint, HostError> {
                    stdin.write_all(&bootstrap)?;
                    stdin.write_all(b"\n")?;
                    drop(stdin);
                    let mut line = String::new();
                    std::io::BufReader::new(stdout)
                        .take(16_385)
                        .read_line(&mut line)?;
                    if line.len() > 16_384 {
                        return Err(HostError::Protocol("oversized discovery".into()));
                    }
                    Ok(serde_json::from_str(&line)?)
                };
                let _ = ready.send(read());
            }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let endpoint = loop {
            match result.try_recv() {
                Ok(result) => break result,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    break Err(HostError::Protocol("host bootstrap stopped".into()));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
            if std::time::Instant::now() >= deadline {
                break Err(HostError::Protocol(
                    "host startup exceeded five seconds".into(),
                ));
            }
            thread::park_timeout(Duration::from_millis(5));
        };
        let client = endpoint.and_then(|endpoint| {
            if endpoint.project != config.project {
                return Err(HostRefusal::new(
                    "project_refused",
                    "host discovery differs from requested project",
                )
                .into());
            }
            Self::connect(endpoint, auth, config.roots)
        });
        if client.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Killing and reaping closes both pipes, so even a timed-out writer/reader retires.
        let _ = reader.join();
        client.map(|client| (client, child))
    }
    /// One JSON-RPC line in, one response line out. Notifications have no stdout reply.
    pub fn proxy_stdio(
        &self,
        input: impl BufRead,
        mut output: impl Write,
    ) -> Result<(), HostError> {
        let mut input = input;
        loop {
            let mut bytes = Vec::new();
            let read = (&mut input)
                .take((HOST_BODY_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)?;
            if read == 0 {
                break;
            }
            if bytes.len() > HOST_BODY_BYTES {
                return Err(HostRefusal::new(
                    "capacity",
                    "stdio message exceeds protocol capacity",
                )
                .into());
            }
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            let response = self.post("mcp", &bytes)?;
            if value.get("id").is_some() {
                output.write_all(&response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostLaunch {
    pub config: HostConfig,
    pub auth: HostAuth,
}

struct ServerThread {
    stop: Arc<tokio::sync::Notify>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for ServerThread {
    fn drop(&mut self) {
        self.stop.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub struct WriterHost {
    endpoint: HostEndpoint,
    auth: HostAuth,
    roots: SessionRoots,
    server: ServerThread,
}
impl WriterHost {
    pub fn start(config: HostConfig) -> Result<Self, HostError> {
        Self::start_with_auth(config, HostAuth::generate()?)
    }
    pub fn start_with_auth(config: HostConfig, auth: HostAuth) -> Result<Self, HostError> {
        let owner = HostCommandOwner::open(config.clone())?;
        let roots = config.roots.canonicalize()?;
        let (listener, address) = bind_loopback()?;
        let endpoint = HostEndpoint {
            address,
            project: config.project,
            protocol: HOST_PROTOCOL,
        };
        let stop = Arc::new(tokio::sync::Notify::new());
        let state = Arc::new(HttpState {
            stop: Arc::clone(&stop),
            bootstrap: HostBootstrap {
                endpoint: endpoint.clone(),
                auth: auth.clone(),
                roots: roots.clone(),
            },
            owner: Mutex::new(owner),
            ingress: Arc::new(tokio::sync::Semaphore::new(1)),
        });
        let router = axum::Router::new()
            .route("/host", post(host_request))
            .route("/mcp", post(mcp_request))
            .with_state(state);
        let server = serve(listener, router, "kinewright-writer-host", stop)?;
        Ok(Self {
            endpoint,
            auth,
            roots,
            server,
        })
    }
    #[must_use]
    pub fn client(&self) -> HostClient {
        HostClient {
            editor: None,
            bootstrap: HostBootstrap {
                endpoint: self.endpoint.clone(),
                auth: self.auth.clone(),
                roots: self.roots.clone(),
            },
        }
    }
    #[must_use]
    pub fn endpoint(&self) -> &HostEndpoint {
        &self.endpoint
    }
    pub fn shutdown(self) -> Result<(), HostError> {
        drop(self);
        Ok(())
    }
    /// Waits for an authenticated Shutdown command; dropping the router runs all owner Drops.
    pub fn wait(self) {
        if let Some(thread) = &self.server.thread {
            while !thread.is_finished() {
                thread::park_timeout(Duration::from_millis(20));
            }
        }
    }
}
impl Drop for WriterHost {
    fn drop(&mut self) {
        self.server.stop.notify_one();
        if let Some(thread) = self.server.thread.take() {
            let _ = thread.join();
        }
        // All named destination/session owners retired in the server thread. No GUI source
        // namespace, reservation, pin, cleanup hook or release message exists in this owner.
    }
}
fn bind_loopback() -> Result<(TcpListener, std::net::SocketAddr), HostError> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    Ok((listener, address))
}
fn serve(
    listener: TcpListener,
    router: axum::Router,
    name: &str,
    stop: Arc<tokio::sync::Notify>,
) -> Result<ServerThread, HostError> {
    let stopped = Arc::clone(&stop);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()?;
    let thread = thread::Builder::new().name(name.into()).spawn(move || {
        runtime.block_on(async move {
            if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                // HTTP requests are bounded and command-local; dropping this future also drops all
                // destination owners on orderly exit and unwinding. No process::exit/abort path.
                let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                    stopped.notified().await;
                });
                let _ = server.await;
            }
        });
    })?;
    Ok(ServerThread {
        stop,
        thread: Some(thread),
    })
}
struct HttpState {
    stop: Arc<tokio::sync::Notify>,
    bootstrap: HostBootstrap,
    owner: Mutex<HostCommandOwner>,
    ingress: Arc<tokio::sync::Semaphore>,
}
fn check_headers(headers: &HeaderMap, bootstrap: &HostBootstrap) -> Result<(), StatusCode> {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if !bootstrap.auth.accepts(value("authorization")) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if value("x-kinewright-project") != Some(bootstrap.endpoint.project.as_str())
        || value("x-kinewright-protocol") != Some("1")
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
async fn host_request(State(state): State<Arc<HttpState>>, request: Request) -> Response {
    if let Err(status) = check_headers(request.headers(), &state.bootstrap) {
        return status.into_response();
    }
    let Ok(permit) = Arc::clone(&state.ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(bytes) = to_bytes(request.into_body(), HOST_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(request) = serde_json::from_slice::<HostRequest>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if request.protocol != HOST_PROTOCOL || request.project != state.bootstrap.endpoint.project {
        return StatusCode::FORBIDDEN.into_response();
    }
    if bytes.len() > 512 * 1024 && matches!(&request.command, HostCommand::CommitHistory { .. }) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    if bytes.len() > 256 * 1024
        && !matches!(
            &request.command,
            HostCommand::CommitHistory { .. }
                | HostCommand::Save { .. }
                | HostCommand::SaveAs { .. }
                | HostCommand::FirstSave { .. }
                | HostCommand::PreviewRescue { .. }
                | HostCommand::PreviewRecovery { .. }
        )
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let reply = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let shutting_down = matches!(request.command, HostCommand::Shutdown);
        let reply = state.owner.lock().map_or_else(
            |_| HostReply {
                error: Some(HostRefusal::new("retention_lost", RETENTION_LOST)),
                ..HostReply::default()
            },
            |mut owner| {
                owner.execute_client(request.command, request.roots, request.editor.as_deref())
            },
        );
        if shutting_down && reply.error.is_none() {
            state.stop.notify_one();
        }
        reply
    })
    .await;
    match reply {
        Ok(reply) => json_response(&reply),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
fn json_response(value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => ([("content-type", "application/json")], bytes).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn mcp_request(State(state): State<Arc<HttpState>>, request: Request) -> Response {
    if let Err(status) = check_headers(request.headers(), &state.bootstrap) {
        return status.into_response();
    }
    let roots = request
        .headers()
        .get("x-kinewright-roots")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str::<SessionRoots>(v).ok());
    let Some(roots) = roots else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if roots
        .read
        .iter()
        .chain(&roots.write)
        .any(|root| !state.bootstrap.roots.permits(root, false))
        || roots
            .write
            .iter()
            .any(|root| !state.bootstrap.roots.permits(root, true))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = Arc::clone(&state.ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(bytes) = to_bytes(request.into_body(), HOST_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let result = match value.get("method").and_then(serde_json::Value::as_str) {
        Some("initialize") => {
            serde_json::json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"kinewright-writer-host","version":env!("CARGO_PKG_VERSION")}})
        }
        Some("notifications/initialized" | "notifications/cancelled") => serde_json::Value::Null,
        Some("ping") => serde_json::json!({}),
        Some("tools/list") => {
            serde_json::json!({"tools":[{"name":"host_command","description":"Read Studio state and control isolated agent sessions; manual mutation belongs to the editor attachment","inputSchema":{"type":"object","properties":{"command":{"type":"object"}},"required":["command"]}}]})
        }
        Some("tools/call") => {
            if value
                .pointer("/params/name")
                .and_then(serde_json::Value::as_str)
                != Some("host_command")
            {
                return json_response(
                    &serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown host tool"}}),
                );
            }
            let command = value
                .pointer("/params/arguments/command")
                .cloned()
                .and_then(|v| serde_json::from_value::<HostCommand>(v).ok());
            let Some(command) = command else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            if !manual::connector_command(&command) {
                return StatusCode::FORBIDDEN.into_response();
            }

            let joined = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                state
                    .owner
                    .lock()
                    .map(|mut owner| owner.execute(command, roots))
                    .map_err(|_| ())
            })
            .await;
            let Ok(Ok(reply)) = joined else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            serde_json::json!({"content":[{"type":"text","text":serde_json::to_string(&reply).unwrap_or_default()}],"structuredContent":reply,"isError":reply.error.is_some()})
        }
        _ => {
            return json_response(
                &serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unknown host method"}}),
            );
        }
    };
    json_response(&serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum SourceRequest {
    SnapshotProof {
        instance: String,
        source: BorrowedSourceDescriptor,
        snapshot: BorrowedSnapshotBinding,
    },
    Prove {
        instance: String,
        source: Option<BorrowedSourceDescriptor>,
        inputs: Option<ObjectRef>,
    },
    Read {
        instance: String,
        source: BorrowedSourceDescriptor,
        object: ObjectRef,
        offset: u64,
        length: usize,
    },
    Inspect {
        instance: String,
        source: BorrowedSourceDescriptor,
        path: std::path::PathBuf,
    },
}
#[derive(Serialize, Deserialize)]
struct SourceReply {
    bytes: Vec<u8>,
    destination: Option<HostDestination>,
    error: Option<HostRefusal>,
}
impl SourceReply {
    fn refused(e: impl Into<HostRefusal>) -> Self {
        Self {
            bytes: vec![],
            destination: None,
            error: Some(e.into()),
        }
    }
}
struct SourceState {
    bootstrap: HostBootstrap,
    reader: Mutex<Option<Box<dyn BorrowedSourceReader + Send>>>,
    context: Option<Box<dyn Fn() -> Result<(), BundleError> + Send + Sync>>,
    bound: Mutex<Option<String>>,
    ingress: Arc<tokio::sync::Semaphore>,
}
/// `BorrowedSourceService` uses `Weak` access to the actual GUI admission. This server has no
/// originating-close method; dropping it cannot clean source bytes or uncharge reservations.
pub struct SourceServer {
    bootstrap: HostBootstrap,
    _server: ServerThread,
}
impl SourceServer {
    pub fn start(reader: impl BorrowedSourceReader + Send + 'static) -> Result<Self, HostError> {
        Self::start_inner(Some(Box::new(reader)), None)
    }
    /// For named GUI handover with no temporary source objects. The callback must verify the
    /// actual originating context; a different host runtime can never reuse its initial claim.
    pub fn start_context(
        verify: impl Fn() -> Result<(), BundleError> + Send + Sync + 'static,
    ) -> Result<Self, HostError> {
        Self::start_inner(None, Some(Box::new(verify)))
    }
    fn start_inner(
        reader: Option<Box<dyn BorrowedSourceReader + Send>>,
        context: Option<Box<dyn Fn() -> Result<(), BundleError> + Send + Sync>>,
    ) -> Result<Self, HostError> {
        let (listener, address) = bind_loopback()?;
        let bootstrap = HostBootstrap {
            endpoint: HostEndpoint {
                address,
                project: "gui-source".into(),
                protocol: HOST_PROTOCOL,
            },
            auth: HostAuth::generate()?,
            roots: SessionRoots::default(),
        };
        let state = Arc::new(SourceState {
            bootstrap: bootstrap.clone(),
            reader: Mutex::new(reader),
            context,
            bound: Mutex::new(None),
            ingress: Arc::new(tokio::sync::Semaphore::new(1)),
        });
        let router = axum::Router::new()
            .route("/source", post(source_request))
            .with_state(state);
        Ok(Self {
            bootstrap,
            _server: serve(
                listener,
                router,
                "kinewright-source-reads",
                Arc::new(tokio::sync::Notify::new()),
            )?,
        })
    }
    #[must_use]
    pub fn bootstrap(&self) -> HostBootstrap {
        self.bootstrap.clone()
    }
}
async fn source_request(State(state): State<Arc<SourceState>>, request: Request) -> Response {
    if let Err(status) = check_headers(request.headers(), &state.bootstrap) {
        return status.into_response();
    }
    let Ok(permit) = Arc::clone(&state.ingress).try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4 * 1024 * 1024).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(request) = serde_json::from_slice::<SourceRequest>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let reply = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let run = || -> Result<SourceReply, BundleError> {
            let instance = match &request {
                SourceRequest::Prove { instance, .. }
                | SourceRequest::SnapshotProof { instance, .. }
                | SourceRequest::Read { instance, .. }
                | SourceRequest::Inspect { instance, .. } => instance,
            };
            if instance.is_empty() || instance.len() > 64 {
                return Err(BundleError::Identity(
                    "invalid host attachment instance".into(),
                ));
            }
            let mut bound = state.bound.lock().map_err(|_| BundleError::Unsettled)?;
            if bound.as_ref().is_some_and(|original| original != instance) {
                return Err(BundleError::Identity(
                    "replacement host retention was not re-established".into(),
                ));
            }
            if bound.is_none() && !matches!(request, SourceRequest::Prove { .. }) {
                return Err(BundleError::Identity(
                    "source attachment requires owner proof first".into(),
                ));
            }
            let mut reader = state.reader.lock().map_err(|_| BundleError::Unsettled)?;
            let mut reply = SourceReply {
                bytes: vec![],
                destination: None,
                error: None,
            };
            match &request {
                SourceRequest::SnapshotProof {
                    source, snapshot, ..
                } => {
                    reader
                        .as_mut()
                        .ok_or(BundleError::Unsettled)?
                        .prove_snapshot(source, snapshot)?;
                }
                SourceRequest::Prove {
                    source: Some(source),
                    inputs: Some(inputs),
                    ..
                } => {
                    reader.as_mut().ok_or(BundleError::Unsettled)?.read_source(
                        source,
                        inputs,
                        0,
                        &mut [],
                    )?;
                }
                SourceRequest::Prove {
                    source: None,
                    inputs: None,
                    ..
                } => {
                    state.context.as_ref().ok_or_else(|| {
                        BundleError::Identity("no actual originating context proof".into())
                    })?()?;
                }
                SourceRequest::Prove { .. } => {
                    return Err(BundleError::Identity("incomplete source proof".into()));
                }
                SourceRequest::Read {
                    source,
                    object,
                    offset,
                    length,
                    ..
                } => {
                    if *length > SOURCE_READ_CHUNK_BYTES {
                        return Err(BundleError::Capacity("source read chunk"));
                    }
                    reply.bytes.resize(*length, 0);
                    let n = reader.as_mut().ok_or(BundleError::Unsettled)?.read_source(
                        source,
                        object,
                        *offset,
                        &mut reply.bytes,
                    )?;
                    if n > reply.bytes.len() {
                        return Err(BundleError::Identity("invalid source read length".into()));
                    }
                    reply.bytes.truncate(n);
                }
                SourceRequest::Inspect { source, path, .. } => {
                    let identity = reader
                        .as_mut()
                        .ok_or(BundleError::Unsettled)?
                        .inspect_destination(source, path)?;
                    reply.destination = Some(HostDestination {
                        path: identity.resolved_root,
                        parent_device: identity.parent_device,
                        parent_inode: identity.parent_inode,
                    });
                }
            }
            if bound.is_none() {
                *bound = Some(instance.clone());
            }
            Ok(reply)
        };
        run().unwrap_or_else(SourceReply::refused)
    })
    .await;
    match reply {
        Ok(reply) => json_response(&reply),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
struct RemoteSource {
    binding: HostSourceBinding,
    instance: String,
    client: HostClient,
    destination: Option<PinnedPath>,
}
impl RemoteSource {
    fn new(binding: HostSourceBinding, instance: &str) -> Result<Self, HostError> {
        let client = HostClient::connect(
            binding.callback.endpoint.clone(),
            binding.callback.auth.clone(),
            binding.callback.roots.clone(),
        )?;
        Ok(Self {
            binding,
            instance: instance.into(),
            client,
            destination: None,
        })
    }
    fn send(&self, request: &SourceRequest) -> Result<SourceReply, HostError> {
        let reply: SourceReply =
            serde_json::from_slice(&self.client.post("source", &serde_json::to_vec(request)?)?)?;
        if let Some(ref error) = reply.error {
            return Err(error.clone().into());
        }
        Ok(reply)
    }
    fn prove(&mut self) -> Result<(), HostError> {
        self.send(&SourceRequest::Prove {
            instance: self.instance.clone(),
            source: self.binding.source.clone(),
            inputs: self.binding.inputs.clone(),
        })
        .map(|_| ())
    }
}
impl BorrowedSourceReader for RemoteSource {
    fn prove_snapshot(
        &mut self,
        source: &BorrowedSourceDescriptor,
        snapshot: &BorrowedSnapshotBinding,
    ) -> Result<(), BundleError> {
        self.send(&SourceRequest::SnapshotProof {
            instance: self.instance.clone(),
            source: source.clone(),
            snapshot: snapshot.clone(),
        })
        .map(|_| ())
        .map_err(|error| BundleError::Identity(error.to_string()))
    }
    fn inspect_destination(
        &mut self,
        source: &BorrowedSourceDescriptor,
        path: &Path,
    ) -> Result<kinewright_project::namespace::NamespaceIdentity, BundleError> {
        let reply = self
            .send(&SourceRequest::Inspect {
                instance: self.instance.clone(),
                source: source.clone(),
                path: path.into(),
            })
            .map_err(|e| BundleError::Identity(e.to_string()))?;
        let target = reply.destination.ok_or_else(|| {
            BundleError::Identity("missing actual owner destination admission".into())
        })?;
        let held = self
            .destination
            .as_ref()
            .ok_or_else(|| BundleError::Identity("missing host destination pin".into()))?;
        if !held
            .matches(&target)
            .map_err(|e| BundleError::Identity(e.to_string()))?
        {
            return Err(BundleError::Identity(
                "destination differs from the host's admitted native parent".into(),
            ));
        }
        Ok(kinewright_project::namespace::NamespaceIdentity {
            resolved_root: target.path,
            parent_device: target.parent_device,
            parent_inode: target.parent_inode,
        })
    }
    fn read_source(
        &mut self,
        source: &BorrowedSourceDescriptor,
        object: &ObjectRef,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BundleError> {
        if buffer.len() > SOURCE_READ_CHUNK_BYTES {
            return Err(BundleError::Capacity("source read chunk"));
        }
        let reply = self
            .send(&SourceRequest::Read {
                instance: self.instance.clone(),
                source: source.clone(),
                object: object.clone(),
                offset,
                length: buffer.len(),
            })
            .map_err(|_| {
                BundleError::Identity(
                    "retention was not re-established by the originating source owner".into(),
                )
            })?;
        if reply.bytes.len() > buffer.len() {
            return Err(BundleError::Identity("source reply exceeds request".into()));
        }
        buffer[..reply.bytes.len()].copy_from_slice(&reply.bytes);
        Ok(reply.bytes.len())
    }
}

/// The same serialized command owner used by HTTP. Public for scoped observations of real IO;
/// it creates no transport or background worker until a session is explicitly requested.
pub struct HostCommandOwner {
    config: HostConfig,
    owner: Option<Ja1EditingSession>,
    initial_core: Core,
    pins: Vec<PinnedPath>,
    remote: Option<RemoteSource>,
    instance: String,
    retention_proven: bool,
    choice: Option<Choice>,
    recovery_choice: bool,
    foreground_history: Option<kinewright_core::HistorySnapshot>,
    floor_disclosure: String,
    admitted: BTreeMap<std::path::PathBuf, PinnedPath>,
    message: String,
    sessions: BTreeMap<u64, HostOwnedSession>,
    next_session: u64,
    manual: Option<kinewright_project::manual_history::ManualLease>,
    next_manual_generation: u64,
}
enum Choice {
    Rescue {
        selection: Box<RescueSelection>,
        consent: RescueConsent,
        trim: bool,
        omission: bool,
    },
    Remove {
        branch: BranchId,
        root: String,
    },
    Forget {
        point: ObjectRef,
        root: String,
    },
    Release(std::path::PathBuf),
}
impl HostCommandOwner {
    pub fn open(mut config: HostConfig) -> Result<Self, HostError> {
        if config.project.is_empty() || config.project.len() > 256 {
            return Err(
                HostRefusal::new("project_refused", "bounded project identity required").into(),
            );
        }
        config.roots = config.roots.canonicalize()?;
        let bootstrap_snapshot = match &config.origin {
            HostOrigin::Gui {
                initial_snapshot, ..
            } => {
                initial_snapshot.verify()?;
                Some(initial_snapshot.clone())
            }
            HostOrigin::Headless => None,
        };
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|_| HostError::Protocol("OS randomness unavailable".into()))?;
        let mut instance = String::with_capacity(nonce.len() * 2);
        for byte in nonce {
            write!(&mut instance, "{byte:02x}")
                .map_err(|_| HostError::Protocol("host instance formatting failed".into()))?;
        }
        let mut remote = if let HostOrigin::Gui { retention, .. } = &config.origin {
            Some(RemoteSource::new(retention.clone(), &instance)?)
        } else {
            None
        };
        // Owner proof precedes namespace writer acquisition. The source service binds the first
        // runtime only; a new lock, PID, descriptor or project string cannot renew that proof.
        if let Some(remote) = &mut remote {
            remote
                .prove()
                .map_err(|_| HostRefusal::new("retention_lost", RETENTION_LOST))?;
            if let Some(initial_snapshot) = &bootstrap_snapshot
                && let Some(source) = remote.binding.source.clone()
            {
                remote.prove_snapshot(
                    &source,
                    &BorrowedSnapshotBinding::from_history(&initial_snapshot.history)?,
                )?;
            }
        }
        let mut pins = Vec::new();
        let owner = if let Some(path) = &config.path {
            let pin = PinnedPath::admit(path, &config.roots, &config.roots, true)?;
            let mut owner = Ja1EditingSession::open(&pin.io)?;
            if let HostOrigin::Gui {
                expected_root_sha256,
                expected_project_id,
                ..
            } = &config.origin
            {
                if expected_root_sha256.as_deref() != Some(owner.root_sha256()?.as_str()) {
                    return Err(HostRefusal::new(
                        "checkpoint_changed",
                        "named handover checkpoint changed before writer acquisition",
                    )
                    .into());
                }
                let loaded = kinewright_project::bundle::LoadedBundle::read(&pin.io)?;
                if expected_project_id.as_ref() != Some(&loaded.root()?.project_id) {
                    return Err(HostRefusal::new(
                        "project_refused",
                        "named handover project identity changed",
                    )
                    .into());
                }
            }
            owner.set_path_spelling(path);
            pins.push(pin);
            Some(owner)
        } else {
            None
        };
        let initial_core = if let Some(owner) = &owner {
            owner.core().clone()
        } else {
            Core::spawn(Document::default()).map_err(|e| HostError::Protocol(e.to_string()))?
        };
        let mut host = Self {
            config,
            owner,
            initial_core,
            pins,
            remote,
            instance,
            retention_proven: true,
            choice: None,
            recovery_choice: false,
            foreground_history: None,
            floor_disclosure: String::new(),
            admitted: BTreeMap::new(),
            message: "Authenticated writer attachment. Saved means a verified durable save.".into(),
            sessions: BTreeMap::new(),
            next_session: 1,
            manual: None,
            next_manual_generation: 1,
        };
        // Initial attachment seeds the one validation shadow before it can serve sessions.
        // This is a bootstrap snapshot, not the C9 continuous manual stream.
        if let Some(initial_snapshot) = bootstrap_snapshot {
            host.install(initial_snapshot)?;
        }
        Ok(host)
    }
    fn owner(&self) -> Result<&Ja1EditingSession, HostError> {
        self.owner.as_ref().ok_or_else(|| {
            HostRefusal::new("project_not_saved", "choose a first-save destination").into()
        })
    }
    fn owner_mut(&mut self) -> Result<&mut Ja1EditingSession, HostError> {
        self.owner.as_mut().ok_or_else(|| {
            HostRefusal::new("project_not_saved", "choose a first-save destination").into()
        })
    }
    fn core(&self) -> &Core {
        self.owner
            .as_ref()
            .map_or(&self.initial_core, Ja1EditingSession::core)
    }
    fn prove_retention(&mut self) -> Result<(), HostError> {
        if !self.retention_proven {
            return Err(HostRefusal::new("retention_lost", RETENTION_LOST).into());
        }
        if let Some(remote) = &mut self.remote
            && remote.prove().is_err()
        {
            self.retention_proven = false;
            self.message = RETENTION_LOST.into();
            return Err(HostRefusal::new("retention_lost", RETENTION_LOST).into());
        }
        Ok(())
    }
    fn install(&mut self, snapshot: HostSnapshot) -> Result<(), HostError> {
        snapshot.verify()?;
        if self.manual.is_some()
            && self
                .core()
                .export_history()
                .map_err(|e| HostError::Protocol(e.to_string()))?
                != snapshot.history
        {
            return Err(HostRefusal::new(
                "manual_stream_required",
                "accepted edits must arrive through the manual history stream",
            )
            .into());
        }
        self.foreground_history = Some(snapshot.history.clone());
        if let Some(owner) = &mut self.owner {
            if owner
                .core()
                .export_history()
                .map_err(|e| HostError::Protocol(e.to_string()))?
                != snapshot.history
            {
                owner.install_host_snapshot(snapshot.history)?;
            }
        } else {
            self.initial_core
                .restore_history(snapshot.history)
                .map_err(|e| HostError::Protocol(e.to_string()))?;
        }
        Ok(())
    }
    /// Errors stay typed in the reply; source retention loss is latched for the whole context.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "The same owned request boundary serves HTTP ingress and scoped observer callers"
    )]
    pub fn execute(&mut self, command: HostCommand, roots: SessionRoots) -> HostReply {
        let mut reply = HostReply::default();
        let result = roots.canonicalize().and_then(|roots| {
            let write = !matches!(
                command,
                HostCommand::Attach
                    | HostCommand::Status
                    | HostCommand::ReadHistory
                    | HostCommand::SessionStatus { .. }
                    | HostCommand::Detach
                    | HostCommand::InterruptSession { .. }
                    | HostCommand::Shutdown
            );
            if write {
                self.prove_retention()?;
            }
            // Every named access honors the forwarded roots again. Its IO owner already holds
            // pinned descriptors; narrowed roots cannot retain an older wider authorization.
            for pin in &self.pins {
                if !roots.permits(&pin.anchor, write) {
                    return Err(
                        HostRefusal::roots("forwarded roots exclude the named project").into(),
                    );
                }
            }
            self.execute_inner(command, &roots, &mut reply)
        });
        if let Err(error) = result {
            self.message = error.to_string();
            reply.error = Some(error.refusal());
        }
        if let Some(saved) = &reply.saved
            && !saved.floor_disclosure.is_empty()
        {
            self.message.push(' ');
            self.message.push_str(&saved.floor_disclosure);
        }
        if reply.activation.is_some()
            || matches!(reply.saved.as_ref(), Some(saved) if self.manual.as_ref().is_some_and(|lease| self.owner().ok().and_then(|o| o.active_manual_project().ok()).as_ref() != Some(&lease.project)) && !saved.root_sha256.is_empty())
        {
            self.manual = None;
        }
        reply.status = self.status();
        reply
    }
    fn saved(
        &mut self,
        report: &BundleSaveReport,
        reply: &mut HostReply,
        foreground: bool,
    ) -> Result<(), HostError> {
        let mut published: HostSaveReport = report.into();
        if foreground {
            // Exact publisher history, never a post-IO live Core read.
            let history = self.owner()?.published_history(report)?;
            published.history_floor_offset = Some(history.floor_offset);
            if let Some(before) = &self.foreground_history {
                let discarded = history.floor_offset.saturating_sub(before.floor_offset);
                let ordinary = before == &history
                    || (discarded > 0
                        && usize::try_from(discarded)
                            .ok()
                            .and_then(|n| {
                                kinewright_core::HistoryTransition::floor(before.clone(), n).ok()
                            })
                            .is_some_and(|t| t.after() == &history));
                if ordinary {
                    published.removed_undo = before
                        .cursor
                        .undo
                        .len()
                        .saturating_sub(history.cursor.undo.len())
                        as u64;
                    published.floor_disclosure = if published.removed_undo > 0 {
                        format!(
                            "Saved with Undo floor {}. {} oldest Undo entries were removed; current content and Redo are retained.",
                            history.floor_offset, published.removed_undo
                        )
                    } else {
                        "Saved without removing Undo entries.".into()
                    };
                    self.floor_disclosure
                        .clone_from(&published.floor_disclosure);
                }
            }
            reply.history = Some(history);
        }
        reply.saved = Some(published);
        Ok(())
    }
    fn copy_status(&mut self, status: &CopyStatus, reply: &mut HostReply) -> Result<(), HostError> {
        match status {
            CopyStatus::Committed(report) => self.saved(report, reply, true),
            CopyStatus::DurabilityUnknown => Err(BundleError::DurabilityUnknown.into()),
            CopyStatus::DefinitelyUnpublished => {
                Err(HostRefusal::new("definitely_unpublished", "save did not publish").into())
            }
            CopyStatus::CleanupPending { diagnostic } => {
                Err(BundleError::CleanupPending(diagnostic.clone()).into())
            }
            CopyStatus::CommittedWithError { diagnostic } => {
                Err(HostRefusal::new("committed_with_error", diagnostic).into())
            }
        }
    }
    fn execute_inner(
        &mut self,
        command: HostCommand,
        roots: &SessionRoots,
        reply: &mut HostReply,
    ) -> Result<(), HostError> {
        let close_session = matches!(&command, HostCommand::CloseSession { .. });
        match command {
            HostCommand::AcquireManual { client } => {
                reply.manual_lease = Some(self.acquire_manual(client)?);
                reply.manual_inputs = Some(self.owner()?.manual_input_context()?);
            }
            HostCommand::CommitHistory { client, messages } => {
                self.commit_history(&client, &messages, reply)?;
            }
            HostCommand::Attach | HostCommand::Status | HostCommand::Detach => {}
            HostCommand::AdmitDestination { destination } => {
                self.admit_person_destination(destination)?;
                self.message = "Chosen destination admitted under its pinned identity.".into();
            }
            HostCommand::ReadHistory => {
                reply.history = Some(
                    self.core()
                        .export_history()
                        .map_err(|e| HostError::Protocol(e.to_string()))?,
                );
            }
            HostCommand::Save { snapshot } => {
                self.install(snapshot)?;
                let report = self.owner_mut()?.save_active()?;
                self.saved(&report, reply, true)?;
                self.message = format!("Saved {}.", report.path.display());
            }
            HostCommand::SaveAs {
                destination,
                snapshot,
            } => {
                self.owner()?.check_destination_change()?;
                let pin = self.admit_path(&destination, roots)?;
                self.install(snapshot)?;
                let inspection = self
                    .owner()?
                    .inspect_rescue_destination(&pin.inspected.path)?;
                let actual = inspection.identity()?;
                let target = HostDestination {
                    path: actual.resolved_root,
                    parent_device: actual.parent_device,
                    parent_inode: actual.parent_inode,
                };
                if !pin.matches(&target)? {
                    return Err(HostRefusal::roots(
                        "Save As target differs from the admitted native parent",
                    )
                    .into());
                }
                let status = self
                    .owner_mut()?
                    .save_as_admitted(&inspection, StorageLimits::default())?;
                self.owner_mut()?.set_path_spelling(&destination);
                self.keep_pin(pin);
                self.copy_status(&status, reply)?;
                self.message = format!("Saved {}.", destination.display());
            }
            HostCommand::FirstSave {
                destination,
                snapshot,
                source,
                inputs,
                callback,
            } => {
                if self.owner.is_some() {
                    return Err(BundleError::HistoryChanged.into());
                }
                let pin = self.admit_path(&destination, roots)?;
                let binding = HostSourceBinding {
                    source: Some(source.clone()),
                    inputs: Some(inputs.clone()),
                    callback,
                };
                if let Some(remote) = &self.remote {
                    if remote.binding.source.as_ref() != Some(&source)
                        || remote.binding.inputs.as_ref() != Some(&inputs)
                        || remote.binding.callback.endpoint.address
                            != binding.callback.endpoint.address
                    {
                        return Err(HostRefusal::new(
                            "retention_lost",
                            "first-save source differs from originating retained admission",
                        )
                        .into());
                    }
                } else {
                    let mut remote = RemoteSource::new(binding, &self.instance)?;
                    if remote.prove().is_err() {
                        self.retention_proven = false;
                        return Err(HostRefusal::new("retention_lost", RETENTION_LOST).into());
                    }
                    self.remote = Some(remote);
                }
                self.install(snapshot)?;
                let history = self
                    .initial_core
                    .export_history()
                    .map_err(|e| HostError::Protocol(e.to_string()))?;
                let remote = self
                    .remote
                    .as_mut()
                    .ok_or_else(|| HostRefusal::new("retention_lost", RETENTION_LOST))?;
                // The project's no-follow inspector intentionally rejects proc-fd symlinks.
                // Its pathname comes from our actual held parent fd; every authenticated GUI
                // inspection must match this native pin, and the project compares its own pin
                // before the first mutation. No mutable caller alias is used for acquisition.
                remote.destination = Some(pin.duplicate()?);
                let copy = BundleCopy::first_save_borrowed(
                    &pin.inspected.path,
                    source,
                    remote,
                    &history,
                    &inputs,
                    StorageLimits::default(),
                )?;
                let status = copy.status().clone();
                let mut owner =
                    Ja1EditingSession::from_borrowed_copy(copy, &self.initial_core, inputs)?;
                owner.set_path_spelling(&destination);
                self.owner = Some(owner);
                self.keep_pin(pin);
                self.copy_status(&status, reply)?;
                if let Some(history) = &reply.history {
                    self.owner_mut()?.install_host_snapshot(history.clone())?;
                }
                self.message = format!("Saved {}.", destination.display());
            }
            HostCommand::Retry => {
                if self.owner()?.has_prepared_selection() {
                    self.execute_inner(HostCommand::OpenRecovery, roots, reply)?;
                } else {
                    self.owner_mut()?.reconcile_active()?;
                    self.message = "Destination checked; current edits and their Saved baseline are unchanged.".into();
                }
            }
            HostCommand::PreviewRescue { snapshot } => {
                // An unresolved host publication already holds its exact actor/capture. Do not
                // replace that actor from a fresh GUI history during unknown reconciliation.
                if !self.owner()?.has_unresolved_publication() {
                    self.install(snapshot)?;
                }
                let owner = self.owner_mut()?;
                let probe = owner.probe_active()?.ok_or(BundleError::HistoryChanged)?;
                let capture = owner
                    .prepare_rescue(&probe)?
                    .ok_or(BundleError::HistoryChanged)?;
                let selection = owner.preview_rescue(capture, owner.store()?.limits().clone())?;
                self.message = format!(
                    "Save rescue copy: {} Undo entries removed, {} unavailable recovery points omitted. {}",
                    selection.removed_undo(),
                    selection.omissions().len(),
                    owner.rescue_handoff_disclosure(&selection)?
                );
                let consent = selection.consent();
                if let Some(Choice::Rescue {
                    selection: previous,
                    ..
                }) = self.choice.take()
                {
                    self.owner()?.cancel_rescue_selection(*previous)?;
                }
                self.choice = Some(Choice::Rescue {
                    selection: Box::new(selection),
                    consent,
                    trim: false,
                    omission: false,
                });
            }
            HostCommand::ConsentTrim => {
                let Some(Choice::Rescue {
                    selection, consent, ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged.into());
                };
                selection.consent_trim(consent)?;
                if let Some(Choice::Rescue { trim, .. }) = &mut self.choice {
                    *trim = true;
                }
            }
            HostCommand::ConsentOmission => {
                let Some(Choice::Rescue {
                    selection, consent, ..
                }) = &mut self.choice
                else {
                    return Err(BundleError::HistoryChanged.into());
                };
                selection.consent_omission(consent)?;
                if let Some(Choice::Rescue { omission, .. }) = &mut self.choice {
                    *omission = true;
                }
            }
            HostCommand::RescueTo { destination } => {
                let pin = self.admit_path(&destination, roots)?;
                let inspection = self
                    .owner()?
                    .inspect_rescue_destination(&pin.inspected.path)?;
                let actual = inspection.identity()?;
                if actual.resolved_root != pin.inspected.path
                    || actual.parent_device != pin.inspected.parent_device
                    || actual.parent_inode != pin.inspected.parent_inode
                {
                    return Err(HostRefusal::roots(
                        "rescue target differs from the admitted native parent",
                    )
                    .into());
                }
                let Some(Choice::Rescue {
                    selection, consent, ..
                }) = self.choice.take()
                else {
                    return Err(BundleError::HistoryChanged.into());
                };
                let outcome = self
                    .owner_mut()?
                    .rescue(*selection, &consent, &inspection)?;
                self.keep_pin(pin);
                if outcome.activated {
                    self.initial_core = self.owner()?.core().clone();
                    self.owner_mut()?.set_path_spelling(&destination);
                    if let CopyStatus::Committed(report) = &outcome.status {
                        self.saved(report, reply, true)?;
                        reply.activation = Some(HostActivation {
                            kind: HostActivationKind::Rescue,
                            history_sha256: report.history_sha256.clone(),
                        });
                    }
                }
                self.message = outcome.disclosure;
                if !outcome.activated {
                    self.copy_status(&outcome.status, reply)?;
                }
            }
            HostCommand::Cancel => {
                self.recovery_choice = false;
                if let Some(Choice::Rescue { selection, .. }) = self.choice.take() {
                    self.owner()?.cancel_rescue_selection(*selection)?;
                }
                self.message = if self.owner()?.has_unresolved_publication() {
                    "Cancelled. Editing remains read-only while the publication is unresolved."
                } else {
                    "Cancelled. Current edits are unchanged."
                }
                .into();
            }
            HostCommand::PreviewRecovery { target, snapshot } => {
                self.install(snapshot)?;
                let owner = self.owner_mut()?;
                owner.prepare_recovery(target.into())?;
                self.recovery_choice = true;
                let owner = self.owner()?;
                self.message = format!(
                    "Save current work, then open the selected point. {} {}",
                    owner.recovery_save_disclosure()?,
                    owner.selection_availability()?
                );
            }
            HostCommand::OpenRecovery => {
                self.recovery_choice = false;
                let result = self.owner_mut()?.select_local();
                if matches!(&result, ProjectSelectionResult::Activated(_)) {
                    let _ = self.owner_mut()?.take_preliminary_save_report();
                } else if let Some(report) = self.owner_mut()?.take_preliminary_save_report() {
                    self.saved(&report, reply, true)?;
                }
                match result {
                    ProjectSelectionResult::Activated(report) => {
                        self.initial_core = self.owner()?.core().clone();
                        self.saved(&report, reply, true)?;
                        reply.activation = Some(HostActivation {
                            kind: HostActivationKind::Recovery,
                            history_sha256: report.history_sha256.clone(),
                        });
                        self.message = "Opened recovery point. Prior work remains saved in its ordinary working branch.".into();
                    }
                    ProjectSelectionResult::Unknown => {
                        return Err(BundleError::DurabilityUnknown.into());
                    }
                    ProjectSelectionResult::CommittedWithError(error)
                    | ProjectSelectionResult::Refused(error) => return Err(error.into()),
                }
            }
            HostCommand::Abandon => {
                self.recovery_choice = false;
                self.message = self.owner_mut()?.abandon_recovery()?;
            }
            HostCommand::PreviewRetirement { branch } => {
                self.message = self.owner()?.retirement_disclosure(&branch)?;
                self.choice = Some(Choice::Remove {
                    branch,
                    root: self.owner()?.root_sha256()?,
                });
            }
            HostCommand::RemoveBranch => {
                let Some(Choice::Remove { branch, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged.into());
                };
                let report = self
                    .owner_mut()?
                    .retire_working_branch(&branch, &root, true)?;
                self.saved(&report, reply, false)?;
                self.message = "Working branch removed. Independent recovery, readers and source holds may retain its bytes.".into();
            }
            HostCommand::PreviewForget => {
                let point = self
                    .owner()?
                    .acknowledged_recovery_point()?
                    .ok_or(BundleError::HistoryChanged)?;
                self.message = format!(
                    "Forget recovery point {}. Independent references may retain these bytes; this does not settle the original destination.",
                    point.sha256
                );
                self.choice = Some(Choice::Forget {
                    point,
                    root: self.owner()?.root_sha256()?,
                });
            }
            HostCommand::Forget => {
                let Some(Choice::Forget { point, root }) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged.into());
                };
                let report = self
                    .owner_mut()?
                    .retire_rescue_recovery_point(&point, &root, true)?;
                self.saved(&report, reply, false)?;
                self.message =
                    "Recovery point explicitly forgotten. Independent references remain.".into();
            }
            HostCommand::Reconcile { destination } => {
                self.admit_existing(&destination, roots)?;
                let state = self
                    .owner_mut()?
                    .reconcile_retained_destination(&destination)?;
                self.message = format!(
                    "Destination {state:?}; current edits and Saved baseline are unchanged. A late committed copy does not activate editing."
                );
            }
            HostCommand::PreviewRelease { destination } => {
                self.admit_existing(&destination, roots)?;
                self.message = "Release a completed destination for a separate editable open. This does not switch editing or release originating source retention.".into();
                self.choice = Some(Choice::Release(destination));
            }
            HostCommand::Release => {
                let Some(Choice::Release(path)) = self.choice.take() else {
                    return Err(BundleError::HistoryChanged.into());
                };
                self.owner_mut()?.release_completed_capture(&path)?;
                self.message =
                    "Completed capture owner released. Originating source retention is unchanged."
                        .into();
            }
            HostCommand::SaveSession { branch } | HostCommand::CloseSession { branch } => {
                let close = close_session;
                let (_, mut guard) = self
                    .owner()?
                    .durable_sessions()
                    .into_iter()
                    .find(|(_, g)| g.branch_id() == &branch)
                    .ok_or(BundleError::HistoryChanged)?;
                if close {
                    for session in self
                        .sessions
                        .values_mut()
                        .filter(|s| s.branch.as_ref() == Some(&branch))
                    {
                        session.stop();
                    }
                }
                self.owner_mut()?.persist_durable_branch(&mut guard)?;
                if close {
                    self.owner_mut()?.close_durable_branch(&guard)?;
                }
                self.message = "Session saved; retirement eligibility rechecked.".into();
            }
            HostCommand::StartScripted { turns, prompts } => {
                if turns.len() > 64
                    || prompts.len() > 64
                    || turns.iter().any(|t| t.pause > Duration::from_secs(30))
                {
                    return Err(HostRefusal::new(
                        "capacity",
                        "script exceeds bounded host session capacity",
                    )
                    .into());
                }
                let driver = Box::new(crate::ScriptedDriver::new(turns));
                let session =
                    self.start_session(driver, HostSessionConfig::default(), Some(prompts))?;
                reply.session = Some(self.session_status(session)?);
            }
            HostCommand::StartSession { harness, config } => {
                let driver = crate::harness_driver(&harness).ok_or_else(|| {
                    HostRefusal::new("harness_unavailable", "unknown installed harness")
                })?;
                let session = self.start_session(driver, config, None)?;
                reply.session = Some(self.session_status(session)?);
            }
            HostCommand::SendMessage { session, message } => {
                let session = self
                    .sessions
                    .get_mut(&session)
                    .ok_or_else(|| HostRefusal::new("session_missing", "host session is absent"))?;
                if message.len() > 256 * 1024 {
                    return Err(
                        HostRefusal::new("capacity", "prompt exceeds command capacity").into(),
                    );
                }
                session
                    .prompts
                    .as_ref()
                    .ok_or_else(|| HostRefusal::new("session_closed", "host session is closed"))?
                    .try_send(message)
                    .map_err(|_| {
                        HostRefusal::new(
                            "session_backpressure",
                            "host session prompt queue is full or closed",
                        )
                    })?;
                if let Ok(mut state) = session.state.lock() {
                    state.running = true;
                }
            }
            HostCommand::InterruptSession { session } => {
                self.sessions
                    .get_mut(&session)
                    .ok_or_else(|| HostRefusal::new("session_missing", "host session is absent"))?
                    .stop();
            }
            HostCommand::SessionStatus { session } => {
                reply.session = Some(self.session_status(session)?);
            }
            HostCommand::Shutdown => {
                for session in self.sessions.values_mut() {
                    session.stop();
                }
                self.message = "Host shutting down.".into();
            }
        }
        Ok(())
    }
    fn keep_pin(&mut self, pin: PinnedPath) {
        if !self
            .pins
            .iter()
            .any(|old| old.display == pin.display && old.inspected.path == pin.inspected.path)
        {
            self.pins.push(pin);
        }
    }
    fn admitted_roots(&self) -> SessionRoots {
        let mut roots = self.config.roots.clone();
        for pin in self.admitted.values() {
            let parent = &pin.anchor;
            if !roots.write.iter().any(|root| parent.starts_with(root)) {
                roots.write.push(parent.clone());
            }
        }
        roots
    }
    fn admit_path(&self, path: &Path, roots: &SessionRoots) -> Result<PinnedPath, HostError> {
        if self.pins.len() >= 64 && !self.pins.iter().any(|pin| pin.display == path) {
            return Err(HostRefusal::new("capacity", "named destination capacity reached").into());
        }
        if let Some(pin) = self.admitted.get(path) {
            if !roots.permits(&pin.anchor, true) {
                return Err(
                    HostRefusal::roots("forwarded roots exclude chosen destination").into(),
                );
            }
            return pin.duplicate();
        }
        PinnedPath::admit(path, &self.config.roots, roots, true)
    }
    fn admit_person_destination(&mut self, path: std::path::PathBuf) -> Result<(), HostError> {
        if !matches!(&self.config.origin, HostOrigin::Gui { .. }) {
            return Err(HostRefusal::new(
                "gui_route_required",
                "destination admission requires the originating GUI attachment",
            )
            .into());
        }
        if self.admitted.len() >= 32
            || self.admitted_roots().read.len() + self.admitted_roots().write.len() >= 64
        {
            return Err(
                HostRefusal::new("capacity", "destination admission capacity reached").into(),
            );
        }
        if !matches!(&self.choice, Some(Choice::Rescue { .. }))
            && let Some(owner) = &self.owner
        {
            owner.check_destination_change()?;
        }
        // A read-only broad scope is used only to form this person's exact pinned target;
        // it is never installed as an agent root. The actual source owners inspect separation.
        let root = path
            .ancestors()
            .last()
            .filter(|root| root.is_absolute())
            .ok_or_else(|| HostRefusal::roots("absolute chosen destination required"))?
            .to_path_buf();
        let scope = SessionRoots {
            read: vec![],
            write: vec![root],
        };
        let pin = PinnedPath::admit(&path, &scope, &scope, true)?;
        let held = &pin.inspected;
        if let Some(owner) = &self.owner {
            let actual = owner.inspect_rescue_destination(&held.path)?.identity()?;
            if actual.resolved_root != held.path
                || actual.parent_device != held.parent_device
                || actual.parent_inode != held.parent_inode
            {
                return Err(HostRefusal::roots(
                    "destination owner inspection differs from host pin",
                )
                .into());
            }
        }
        if let Some(remote) = &mut self.remote
            && let Some(source) = remote.binding.source.clone()
        {
            remote.destination = Some(pin.duplicate()?);
            remote.inspect_destination(&source, &held.path)?;
        }
        self.admitted.insert(path, pin);
        Ok(())
    }
    fn admit_existing(&self, destination: &Path, roots: &SessionRoots) -> Result<(), HostError> {
        let _pin = self.admit_path(destination, roots)?;
        Ok(())
    }
    fn status(&self) -> HostStatus {
        let mut status = HostStatus {
            retention_proven: self.retention_proven,
            message: self.message.clone(),
            admitted_roots: self.admitted_roots(),
            choice: if self.recovery_choice {
                Some(HostChoiceKind::Recovery)
            } else {
                None
            },
            ..HostStatus::default()
        };
        if let Some(owner) = &self.owner {
            status.association = owner.association().map(|a| a.path().to_path_buf());
            status.saved_digest = owner
                .association()
                .and_then(|a| a.saved_digest())
                .map(str::to_owned);
            status.frozen = !self.retention_proven
                || owner.core().attachment_state() != Ok(AttachmentState::Live);
            status.prepared_selection = owner.has_prepared_selection();
            status.unresolved = owner.has_unresolved_publication();
            status.recovery_points = owner
                .list_recovery_points()
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect();
            status.branches = owner.list_working_branches().unwrap_or_default();
            status.retirement_candidates = owner.retirement_candidates().unwrap_or_default();
            status.destinations = owner
                .retained_destinations()
                .unwrap_or_default()
                .into_iter()
                .map(|(p, s)| (p, s.into()))
                .collect();
            status.close_disclosure = owner.close_disclosure().unwrap_or_else(|e| e.to_string());
            status.destination_change = owner
                .check_destination_change()
                .err()
                .map(|e| e.to_string());
            status.selection_availability = owner.selection_availability().unwrap_or_default();
            status.save_disclosure = if self.floor_disclosure.is_empty() {
                owner.recovery_save_disclosure().unwrap_or_default()
            } else {
                self.floor_disclosure.clone()
            };
        }
        if !self.retention_proven {
            status.frozen = true;
            status.message = RETENTION_LOST.into();
            status.destination_change = Some(RETENTION_LOST.into());
        }
        status.choice = match &self.choice {
            Some(Choice::Rescue { .. }) => Some(HostChoiceKind::Rescue),
            Some(Choice::Remove { .. }) => Some(HostChoiceKind::Remove),
            Some(Choice::Forget { .. }) => Some(HostChoiceKind::Forget),
            Some(Choice::Release(_)) => Some(HostChoiceKind::Release),
            None => status.choice,
        };
        if let Some(Choice::Rescue {
            selection,
            trim,
            omission,
            ..
        }) = &self.choice
        {
            status.rescue_trim_required = selection.removed_undo() > 0;
            status.rescue_trim_accepted = *trim;
            status.rescue_omission_required = !selection.omissions().is_empty();
            status.rescue_omission_accepted = *omission;
            status.rescue_removed_undo = selection.removed_undo();
            status.rescue_omissions = selection
                .omissions()
                .iter()
                .map(|o| format!("{o:?}"))
                .collect();
        }
        status
    }
}
impl Drop for HostCommandOwner {
    fn drop(&mut self) {
        for session in self.sessions.values_mut() {
            session.stop();
        }
    }
}

struct SessionState {
    events: VecDeque<HostAgentEvent>,
    bytes: usize,
    overflowed: bool,
    running: bool,
    stop: Option<String>,
}
struct HostOwnedSession {
    core: Core,
    branch: Option<BranchId>,
    prompts: Option<crossbeam_channel::Sender<String>>,
    cancel: Arc<AtomicBool>,
    counters: Arc<crate::SharedCounters>,
    state: Arc<Mutex<SessionState>>,
    thread: Option<JoinHandle<()>>,
}
impl HostOwnedSession {
    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.prompts.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for HostOwnedSession {
    fn drop(&mut self) {
        self.stop();
    }
}
struct HostObserver {
    state: Arc<Mutex<SessionState>>,
}
fn bounded_text(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
impl crate::SessionObserver for HostObserver {
    fn on_event(&mut self, event: &AgentEvent) -> crate::SessionFlow {
        let (kind, text) = match event {
            AgentEvent::Text(text) => ("text", bounded_text(text, 64 * 1024)),
            AgentEvent::Error(text) => ("error", bounded_text(text, 64 * 1024)),
            AgentEvent::ToolCall { name, arguments } => (
                "tool_call",
                bounded_text(&format!("{name}: {arguments}"), 64 * 1024),
            ),
            AgentEvent::ToolResult { name, result } => (
                "tool_result",
                bounded_text(&format!("{name}: {result}"), 64 * 1024),
            ),
            AgentEvent::Cost {
                input_tokens,
                output_tokens,
                ..
            } => (
                "cost",
                format!("{input_tokens} input, {output_tokens} output tokens"),
            ),
            AgentEvent::Done => ("done", String::new()),
        };
        if let Ok(mut state) = self.state.lock() {
            if matches!(event, AgentEvent::Done) {
                state.running = false;
            }
            if state.events.len() >= 1024 || state.bytes + text.len() > 256 * 1024 {
                state.overflowed = true;
                return crate::SessionFlow::Stop(
                    "host event capacity reached; inspect the retained session".into(),
                );
            }
            state.bytes += text.len();
            state.events.push_back(HostAgentEvent {
                kind: kind.into(),
                text,
            });
        }
        crate::SessionFlow::Continue
    }
    fn on_tick(&mut self, _counters: &crate::SessionCounters) -> crate::SessionFlow {
        crate::SessionFlow::Continue
    }
}
impl HostCommandOwner {
    fn start_session(
        &mut self,
        driver: Box<dyn AgentDriver>,
        cfg: HostSessionConfig,
        scripted_prompts: Option<Vec<String>>,
    ) -> Result<u64, HostError> {
        if self.sessions.len() >= 16 {
            return Err(HostRefusal::new("capacity", "host session limit reached").into());
        }
        if scripted_prompts
            .as_ref()
            .is_some_and(|p| p.iter().map(String::len).sum::<usize>() > 256 * 1024)
        {
            return Err(HostRefusal::new(
                "capacity",
                "script prompts exceed bounded command ingress",
            )
            .into());
        }
        let id = self.next_session;
        self.next_session = id
            .checked_add(1)
            .ok_or_else(|| HostRefusal::new("capacity", "host session identity exhausted"))?;
        let (core, branch) = if let Some(owner) = &mut self.owner {
            let guard = owner.fork_durable_branch(&format!("host-session-{id}"))?;
            (guard.core().clone(), Some(guard.branch_id().clone()))
        } else {
            let history = self
                .initial_core
                .export_history()
                .map_err(|e| HostError::Protocol(e.to_string()))?;
            let core = Core::spawn((*history.document).clone())
                .map_err(|e| HostError::Protocol(e.to_string()))?;
            (core, None)
        };
        let (prompts_tx, prompts_rx) = crossbeam_channel::bounded::<String>(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(crate::SharedCounters::default());
        let state = Arc::new(Mutex::new(SessionState {
            events: VecDeque::new(),
            bytes: 0,
            overflowed: false,
            running: scripted_prompts.is_some(),
            stop: None,
        }));
        let worker_state = Arc::clone(&state);
        let worker_cancel = Arc::clone(&cancel);
        let worker_counters = Arc::clone(&counters);
        let worker_core = core.clone();
        let project = self.config.project.clone();
        let roots = self.config.roots.clone();
        let thread = thread::Builder::new()
            .name(format!("kinewright-host-session-{id}"))
            .spawn(move || {
                let run = || -> Result<String, HostError> {
                    // Media is requested only by explicit session creation, never attach/proxy.
                    // Host-scoped MCP refuses every file/media operation until it has a pinned
                    // media input/output capability; roots alone do not make a bare path safe.
                    let media = Arc::new(LazyHostMedia::default());
                    let server = crate::McpServer::start_host_isolated(
                        worker_core,
                        media.clone(),
                        media,
                        &project,
                        roots,
                    )?;
                    let cfg = SessionConfig {
                        model: cfg.model,
                        effort: cfg.effort,
                        service_tier: cfg.service_tier,
                        max_turns: Some(cfg.max_turns.unwrap_or(64).min(64)),
                        mcp_url: Some(server.endpoint().into()),
                        tool_names: Some(crate::compact_tool_names()),
                        ..SessionConfig::default()
                    };
                    let mut session = driver
                        .start_session(cfg)
                        .map_err(|e| HostError::Protocol(e.to_string()))?;
                    let events = session.events();
                    let mut observer = HostObserver {
                        state: Arc::clone(&worker_state),
                    };
                    let limits = crate::SessionLimits {
                        max_turns: 64,
                        max_wall_time: Duration::from_mins(30),
                        max_tokens: None,
                    };
                    let mut prompts: Box<dyn Iterator<Item = String>> =
                        if let Some(prompts) = scripted_prompts {
                            Box::new(prompts.into_iter())
                        } else {
                            Box::new(prompts_rx.into_iter())
                        };
                    let (stop, _) = crate::pump_session(
                        session.as_mut(),
                        &events,
                        prompts.as_mut(),
                        Some(&server.confirmations()),
                        crate::ConfirmationPolicy::RejectAndStop,
                        &limits,
                        &worker_counters,
                        &worker_cancel,
                        &mut observer,
                    );
                    drop(session);
                    server.shutdown();
                    Ok(format!("{stop:?}"))
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
                let stop = match result {
                    Ok(Ok(stop)) => stop,
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => {
                        "host session panicked; destination and source ownership were unwound"
                            .into()
                    }
                };
                if let Ok(mut state) = worker_state.lock() {
                    state.running = false;
                    state.stop = Some(stop);
                }
            })?;
        self.sessions.insert(
            id,
            HostOwnedSession {
                core,
                branch,
                prompts: Some(prompts_tx),
                cancel,
                counters,
                state,
                thread: Some(thread),
            },
        );
        Ok(id)
    }
    fn session_status(&mut self, id: u64) -> Result<HostSessionStatus, HostError> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| HostRefusal::new("session_missing", "host session is absent"))?;
        let mut state = session.state.lock().map_err(|_| {
            HostRefusal::new("session_failed", "host session status is unavailable")
        })?;
        let events = state.events.drain(..).collect();
        state.bytes = 0;
        let history = session
            .core
            .export_history()
            .map_err(|e| HostError::Protocol(e.to_string()))?;
        Ok(HostSessionStatus {
            id,
            running: state.running,
            turns: session.counters.snapshot().turns,
            events,
            overflowed: state.overflowed,
            stop: state.stop.clone(),
            history: Some(history),
        })
    }
}

// The real media engine is constructed only on a media operation. Scoped MCP rejects
// unpinned file operations before this boundary; a metadata-only scripted session needs
// neither a GPU nor a Whisper model, just like attachment/proxy.
#[derive(Default)]
struct LazyHostMedia {
    engine: OnceLock<Result<Arc<kinewright_media::FfmpegMediaEngine>, kinewright_core::MediaError>>,
}
impl LazyHostMedia {
    fn get(
        &self,
    ) -> Result<&Arc<kinewright_media::FfmpegMediaEngine>, kinewright_core::MediaError> {
        self.engine
            .get_or_init(|| kinewright_media::FfmpegMediaEngine::new().map(Arc::new))
            .as_ref()
            .map_err(Clone::clone)
    }
}
impl kinewright_core::Playback for LazyHostMedia {
    fn set_document(&self, doc: Arc<Document>) {
        if let Ok(e) = self.get() {
            kinewright_core::Playback::set_document(e.as_ref(), doc);
        }
    }
    fn request_frame(&self, t: kinewright_core::TimeCode) {
        if let Ok(e) = self.get() {
            kinewright_core::Playback::request_frame(e.as_ref(), t);
        }
    }
    fn frames(&self) -> crossbeam_channel::Receiver<kinewright_core::PreviewFrame> {
        self.get().map_or_else(
            |_| crossbeam_channel::never(),
            |e| kinewright_core::Playback::frames(e.as_ref()),
        )
    }
    fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::MediaEvent> {
        self.get().map_or_else(
            |_| crossbeam_channel::never(),
            |e| kinewright_core::Playback::events(e.as_ref()),
        )
    }
    fn play(&self, t: kinewright_core::TimeCode) {
        if let Ok(e) = self.get() {
            kinewright_core::Playback::play(e.as_ref(), t);
        }
    }
    fn pause(&self) {
        if let Ok(e) = self.get() {
            kinewright_core::Playback::pause(e.as_ref());
        }
    }
    fn seek(&self, t: kinewright_core::TimeCode) {
        if let Ok(e) = self.get() {
            kinewright_core::Playback::seek(e.as_ref(), t);
        }
    }
    fn position(&self) -> kinewright_core::TimeCode {
        self.get().map_or(kinewright_core::TimeCode(0), |e| {
            kinewright_core::Playback::position(e.as_ref())
        })
    }
    fn output_peaks(&self) -> [f32; 2] {
        self.get().map_or([0.0; 2], |e| {
            kinewright_core::Playback::output_peaks(e.as_ref())
        })
    }
}
impl kinewright_core::Analysis for LazyHostMedia {
    fn probe(
        &self,
        path: &Path,
    ) -> Result<kinewright_core::MediaAsset, kinewright_core::MediaError> {
        kinewright_core::Analysis::probe(self.get()?.as_ref(), path)
    }
    fn thumbnail_at(
        &self,
        t: kinewright_core::TimeCode,
        max_w: u32,
    ) -> Result<kinewright_core::RgbaImage, kinewright_core::MediaError> {
        kinewright_core::Analysis::thumbnail_at(self.get()?.as_ref(), t, max_w)
    }
    fn request_transcription(&self, asset: kinewright_core::MediaAsset) {
        if let Ok(e) = self.get() {
            kinewright_core::Analysis::request_transcription(e.as_ref(), asset);
        }
    }
    fn transcript_status(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> kinewright_core::TranscriptStatus {
        self.get().map_or_else(
            |e| kinewright_core::TranscriptStatus::Failed(e.to_string()),
            |e| kinewright_core::Analysis::transcript_status(e.as_ref(), asset),
        )
    }
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<kinewright_core::TimeCode>>,
    ) -> Result<Vec<kinewright_core::TimelineTranscriptWord>, kinewright_core::MediaError> {
        kinewright_core::Analysis::timeline_transcript(self.get()?.as_ref(), document, range)
    }
    fn request_silence_detection(&self, asset: kinewright_core::MediaAsset) {
        if let Ok(e) = self.get() {
            kinewright_core::Analysis::request_silence_detection(e.as_ref(), asset);
        }
    }
    fn silence_status(
        &self,
        asset: &kinewright_core::MediaAsset,
    ) -> kinewright_core::SilenceStatus {
        self.get().map_or_else(
            |e| kinewright_core::SilenceStatus::Failed(e.to_string()),
            |e| kinewright_core::Analysis::silence_status(e.as_ref(), asset),
        )
    }
    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<kinewright_core::TimeCode>>,
        minimum_source_frames: kinewright_core::TimeCode,
    ) -> Result<Vec<kinewright_core::TimelineSilenceSpan>, kinewright_core::MediaError> {
        kinewright_core::Analysis::timeline_silences(
            self.get()?.as_ref(),
            document,
            range,
            minimum_source_frames,
        )
    }
    fn request_scene_detection(&self, asset: kinewright_core::MediaAsset) {
        if let Ok(e) = self.get() {
            kinewright_core::Analysis::request_scene_detection(e.as_ref(), asset);
        }
    }
    fn scene_status(&self, asset: &kinewright_core::MediaAsset) -> kinewright_core::SceneStatus {
        self.get().map_or_else(
            |e| kinewright_core::SceneStatus::Failed(e.to_string()),
            |e| kinewright_core::Analysis::scene_status(e.as_ref(), asset),
        )
    }
    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<kinewright_core::TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<kinewright_core::TimelineSceneChange>, kinewright_core::MediaError> {
        kinewright_core::Analysis::timeline_scene_changes(
            self.get()?.as_ref(),
            document,
            range,
            minimum_confidence_basis_points,
        )
    }
    fn request_thumbnail(
        &self,
        asset: kinewright_core::MediaAsset,
        source_at: kinewright_core::TimeCode,
        max_width: u32,
        request_generation: u64,
    ) -> bool {
        self.get().is_ok_and(|e| {
            kinewright_core::Analysis::request_thumbnail(
                e.as_ref(),
                asset,
                source_at,
                max_width,
                request_generation,
            )
        })
    }
    fn request_waveform(
        &self,
        asset: kinewright_core::MediaAsset,
        request_generation: u64,
    ) -> bool {
        self.get().is_ok_and(|e| {
            kinewright_core::Analysis::request_waveform(e.as_ref(), asset, request_generation)
        })
    }
    fn visual_asset_results(
        &self,
    ) -> crossbeam_channel::Receiver<kinewright_core::VisualAssetResult> {
        self.get().map_or_else(
            |_| crossbeam_channel::never(),
            |e| kinewright_core::Analysis::visual_asset_results(e.as_ref()),
        )
    }
}

// Existing drivers discover credentials by exact in-process endpoint identity, never by
// project/discovery files or URL query tokens. The registry does not launch or own sessions.
fn credentials() -> &'static Mutex<BTreeMap<String, HostBootstrap>> {
    static CREDENTIALS: OnceLock<Mutex<BTreeMap<String, HostBootstrap>>> = OnceLock::new();
    CREDENTIALS.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub(crate) fn register_mcp(endpoint: &str, bootstrap: HostBootstrap) {
    if let Ok(mut credentials) = credentials().lock() {
        credentials.insert(endpoint.into(), bootstrap);
    }
}
pub(crate) fn unregister_mcp(endpoint: &str) {
    if let Ok(mut credentials) = credentials().lock() {
        credentials.remove(endpoint);
    }
}
pub(crate) fn mcp_credentials(endpoint: &str) -> Option<HostBootstrap> {
    credentials().lock().ok()?.get(endpoint).cloned()
}
pub(crate) fn mcp_headers(
    bootstrap: &HostBootstrap,
) -> Result<reqwest::header::HeaderMap, HostError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in [
        ("authorization", bootstrap.auth.bearer()),
        ("x-kinewright-project", bootstrap.endpoint.project.clone()),
        ("x-kinewright-protocol", HOST_PROTOCOL.to_string()),
        ("x-kinewright-roots", bootstrap.roots.wire_header()?),
    ] {
        headers.insert(
            reqwest::header::HeaderName::from_static(key),
            reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| HostError::Protocol("invalid attachment header".into()))?,
        );
    }
    Ok(headers)
}
/// Middleware executes before MCP initialization/list/read/call, including GET and DELETE.
pub(crate) async fn guard_mcp(
    State(bootstrap): State<HostBootstrap>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if let Err(status) = check_headers(request.headers(), &bootstrap) {
        return status.into_response();
    }
    let roots = request
        .headers()
        .get("x-kinewright-roots")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str::<SessionRoots>(v).ok());
    if roots.and_then(|r| r.canonicalize().ok()).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_refusals_preserve_native_error_and_do_not_disclose_credentials() {
        let error = HostRefusal::from(BundleError::Io(std::io::Error::from_raw_os_error(13)));
        assert_eq!(error.code, "io");
        assert_eq!(error.native_code, Some(13));
        let auth = HostAuth::generate().unwrap();
        let bootstrap = HostBootstrap {
            endpoint: HostEndpoint {
                address: "127.0.0.1:1".parse().unwrap(),
                project: "test".into(),
                protocol: HOST_PROTOCOL,
            },
            auth,
            roots: SessionRoots::default(),
        };
        let discovery = serde_json::to_string(&bootstrap.endpoint).unwrap();
        assert!(!discovery.contains("auth"));
        assert!(!format!("{bootstrap:?}").contains(&bootstrap.auth.bearer()));
    }
    #[test]
    fn ordinary_attachment_and_history_use_real_authenticated_loopback() {
        let host = WriterHost::start(HostConfig {
            project: "unit-host".into(),
            path: None,
            roots: SessionRoots::default(),
            origin: HostOrigin::Headless,
        })
        .unwrap();
        let client = host.client();
        assert!(client.request(HostCommand::Attach).unwrap().error.is_none());
        let history = client
            .request(HostCommand::ReadHistory)
            .unwrap()
            .history
            .unwrap();
        assert!(history.batches.is_empty());
        let bad = HostClient::connect(
            client.endpoint().clone(),
            HostAuth::generate().unwrap(),
            SessionRoots::default(),
        )
        .unwrap();
        assert!(bad.request(HostCommand::ReadHistory).is_err());
        host.shutdown().unwrap();
    }
    #[test]
    fn snapshot_digest_refusal_precedes_any_owner_install() {
        let core = Core::spawn(Document::default()).unwrap();
        let mut snapshot = HostSnapshot::capture(&core).unwrap();
        snapshot.history_sha256 = "0".repeat(64);
        let mut owner = HostCommandOwner::open(HostConfig {
            project: "unit-snapshot".into(),
            path: None,
            roots: SessionRoots::default(),
            origin: HostOrigin::Headless,
        })
        .unwrap();
        let reply = owner.execute(HostCommand::Save { snapshot }, SessionRoots::default());
        assert_eq!(reply.error.unwrap().code, "history_changed");
        assert!(owner.core().export_history().unwrap().batches.is_empty());
    }
}
