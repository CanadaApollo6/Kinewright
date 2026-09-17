//! Meta Muse integration over the Muse Session Protocol (MSP).
//!
//! `muse serve` hosts MSP (JSON-RPC 2.0 over stdio, the same framing as ACP,
//! so the shared [`AcpClient`](crate::acp::AcpClient) transport carries it)
//! with a fixed sandbox posture. Each Kinewright thread starts one MSP
//! session in an empty scratch directory with the ephemeral Kinewright MCP
//! endpoint attached per-session, so the user's `~/.config/muse` files are
//! never touched. Model turns stream back as transcript items; tool approvals
//! are auto-decided because the host runs headless.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use kinewright_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use serde_json::{Value, json};

use crate::{
    acp::{AcpClient, AcpIncoming, send_done},
    child_process::{create_scratch_directory, process_output},
    drivers::{KINEWRIGHT_SYSTEM_PROMPT, find_on_path},
    models::{ModelChoice, ServiceTier},
};

const MSP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Closed reasoning-effort vocabulary from the MSP schema (`ReasoningEffort`).
const MUSE_EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

static MUSE_COMMAND_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const MUSE_SANDBOX_NOTICE: &str = "Muse sessions run from an empty scratch directory with shell and file-write tools disabled; tool approvals are auto-answered and only the Kinewright MCP endpoint is attached.";

#[derive(Debug, Default, Clone, Copy)]
pub struct MuseDriver;

impl AgentDriver for MuseDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("muse")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("muse")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication: muse_authentication(),
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("muse").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        MuseSession::spawn(executable, &endpoint, cfg).map(|session| Box::new(session) as _)
    }
}

/// Models the installed Muse host accepts, from its live `model/list`
/// catalog. When the host cannot be queried, the last verified Muse Spark
/// catalog is used so the picker still offers real model ids.
#[must_use]
pub fn muse_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("muse") else {
        return Vec::new();
    };
    let mut command = ProcessCommand::new(executable);
    serve_command(&mut command);
    let Ok(client) = AcpClient::spawn(command, "Muse") else {
        return Vec::new();
    };
    let models = initialize_muse(&client)
        .and_then(|_| client.request("model/list", &json!({}), MSP_REQUEST_TIMEOUT))
        .map(|catalog| parse_muse_models(&catalog))
        .unwrap_or_default();
    client.kill();
    if models.is_empty() {
        muse_fallback_models()
    } else {
        models
    }
}

/// Last verified `model/list` snapshot (Muse Code 1.3.0): the ids
/// `session/start` accepts when the live catalog is unreachable.
fn muse_fallback_models() -> Vec<ModelChoice> {
    const FALLBACK: [(&str, &str); 4] = [
        ("muse-spark-1.3", "muse-spark-1.3"),
        ("muse-spark-1.3-contributor", "muse-spark-1.3-contributor"),
        ("muse-spark-1.2", "muse-spark-1.2"),
        ("muse-spark-1.2-contributor", "muse-spark-1.2-contributor"),
    ];
    FALLBACK
        .iter()
        .map(|(id, label)| ModelChoice {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
            efforts: MUSE_EFFORTS
                .iter()
                .map(|effort| (*effort).to_owned())
                .collect(),
            tiers: Vec::<ServiceTier>::new(),
        })
        .collect()
}

fn parse_muse_models(catalog: &Value) -> Vec<ModelChoice> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("modelId").and_then(Value::as_str)?;
            let label = model
                .get("displayLabel")
                .and_then(Value::as_str)
                .unwrap_or(id);
            Some(ModelChoice {
                id: id.to_owned(),
                label: label.to_owned(),
                efforts: MUSE_EFFORTS
                    .iter()
                    .map(|effort| (*effort).to_owned())
                    .collect(),
                tiers: Vec::new(),
            })
        })
        .collect()
}

fn muse_authentication() -> AuthenticationStatus {
    if env::var_os("META_API_KEY").is_some_and(|key| !key.is_empty()) {
        return AuthenticationStatus::Authenticated;
    }
    muse_auth_path()
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .map_or(AuthenticationStatus::Unknown, |auth| {
            let meta = auth.pointer("/providers/meta");
            let has_key = meta
                .and_then(|meta| meta.get("api_key"))
                .and_then(Value::as_str)
                .is_some_and(|key| !key.is_empty())
                || meta
                    .and_then(|meta| meta.get("access_token"))
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty());
            if has_key {
                AuthenticationStatus::Authenticated
            } else {
                AuthenticationStatus::Unknown
            }
        })
}

fn muse_auth_path() -> Option<PathBuf> {
    muse_config_dir().map(|dir| dir.join("auth.json"))
}

fn muse_config_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        return Some(dir.join("muse"));
    }
    #[cfg(windows)]
    if let Some(dir) = env::var_os("APPDATA").map(PathBuf::from) {
        return Some(dir.join("muse"));
    }
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(|home| PathBuf::from(home).join(".config").join("muse"))
}

/// Fixed sandbox posture for the MSP host: no shell, no file writes. The
/// approval mode is selected per session on the wire. Memory-only sessions
/// (`--no-session-log`) are deliberately not used: turns submitted to them
/// are accepted but never execute.
fn serve_command(command: &mut ProcessCommand) {
    command
        .arg("serve")
        .arg("--disable-shell")
        .arg("--disable-write");
}

fn initialize_muse(client: &AcpClient) -> Result<Value, AgentError> {
    let initialized = client.request(
        "initialize",
        &json!({
            "clientInfo": {
                "name": "kinewright",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "requestedCapabilities": ["sessionMcp"],
                "userInputDialogs": false,
            },
        }),
        MSP_REQUEST_TIMEOUT,
    )?;
    let granted = initialized
        .get("grantedCapabilities")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .any(|capability| capability == "sessionMcp");
    if !granted {
        return Err(AgentError::Unavailable(
            "the installed Muse host did not grant the sessionMcp capability".to_owned(),
        ));
    }
    client.notify("initialized", &Value::Null)?;
    Ok(initialized)
}

/// MSP idempotency handle: `UUIDv7` (48-bit unix millis, version, variant).
/// Random bits come from a process-wide counter folded with the current
/// nanos and pid: unique per host without a crypto dependency, which is all
/// an idempotency handle needs.
fn new_command_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let unix_ms = u64::try_from(millis).unwrap_or(u64::MAX) & 0xFFFF_FFFF_FFFF;
    let counter = MUSE_COMMAND_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = u64::from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos(),
    );
    let pid = u64::from(std::process::id());
    let rand_a = (counter ^ nanos ^ pid) & 0xFFF;
    let rand_b = (counter
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(nanos)
        ^ pid.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        & 0x3FFF_FFFF_FFFF_FFFF;
    let value = (u128::from(unix_ms) << 80)
        | (7 << 76)
        | (u128::from(rand_a) << 64)
        | (0b10 << 62)
        | u128::from(rand_b);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (value >> 96) as u32,
        ((value >> 80) & 0xFFFF) as u16,
        ((value >> 64) & 0xFFFF) as u16,
        ((value >> 48) & 0xFFFF) as u16,
        value & 0xFFFF_FFFF_FFFF,
    )
}

struct MuseSession {
    client: AcpClient,
    session_id: String,
    requested: SessionConfig,
    scratch_directory: PathBuf,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    turns: u32,
}

impl MuseSession {
    fn spawn(
        executable: PathBuf,
        endpoint: &str,
        requested: SessionConfig,
    ) -> Result<Self, AgentError> {
        let mut command = ProcessCommand::new(executable);
        serve_command(&mut command);
        Self::start(command, endpoint, requested)
    }

    /// Open one MSP session on an already-built host command.
    fn start(
        mut command: ProcessCommand,
        endpoint: &str,
        requested: SessionConfig,
    ) -> Result<Self, AgentError> {
        if let Some(effort) = requested.effort.as_deref()
            && !MUSE_EFFORTS.contains(&effort)
        {
            return Err(AgentError::Unavailable(format!(
                "unknown Muse reasoning effort {effort:?}; expected one of {}",
                MUSE_EFFORTS.join(", "),
            )));
        }
        let scratch_directory = create_scratch_directory("muse")?;
        command.current_dir(&scratch_directory);
        let client = match AcpClient::spawn(command, "Muse") {
            Ok(client) => client,
            Err(error) => {
                let _ = fs::remove_dir_all(&scratch_directory);
                return Err(error);
            }
        };
        let session_id = match start_muse_session(&client, endpoint, &requested, &scratch_directory)
        {
            Ok(session_id) => session_id,
            Err(error) => {
                client.kill();
                let _ = fs::remove_dir_all(&scratch_directory);
                return Err(error);
            }
        };
        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(true));
        spawn_muse_incoming(
            client.clone(),
            client.incoming(),
            session_id.clone(),
            events_tx.clone(),
            Arc::clone(&done),
        )?;

        Ok(Self {
            client,
            session_id,
            requested,
            scratch_directory,
            events_rx,
            events_tx,
            done,
            turns: 0,
        })
    }
}

fn start_muse_session(
    client: &AcpClient,
    endpoint: &str,
    requested: &SessionConfig,
    scratch_directory: &Path,
) -> Result<String, AgentError> {
    initialize_muse(client)?;
    // No `approvalMode`: the host seals a startup ceiling and rejects any
    // explicitly named mode above it, so the server default is selected and
    // tool approvals are answered over the wire instead.
    let mut params = json!({
        "commandId": new_command_id(),
        "workspaceRoot": scratch_directory.to_string_lossy(),
        "config": {
            "mcpServers": {
                "kinewright": {
                    "transport": "streamableHttp",
                    "url": endpoint,
                    "mode": "required",
                },
            },
        },
    });
    if let Some(model) = requested.model.as_deref() {
        params["modelId"] = Value::String(model.to_owned());
    }
    let started = client.request("session/start", &params, MSP_REQUEST_TIMEOUT)?;
    started
        .pointer("/session/sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            AgentError::Protocol("Muse session/start omitted session.sessionId".to_owned())
        })
}

impl AgentSession for MuseSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if !self.done.load(Ordering::Acquire) {
            return Err(AgentError::Harness(
                "Muse is still processing the previous turn".to_owned(),
            ));
        }
        if let Some(cap) = self.requested.max_turns.map(|cap| cap.max(1))
            && self.turns >= cap
        {
            return Err(AgentError::Harness(format!(
                "Turn cap reached ({cap}); start a new Muse session to continue."
            )));
        }
        let prompt = if self.turns == 0 {
            format!("{KINEWRIGHT_SYSTEM_PROMPT}\n\nUser request:\n{text}")
        } else {
            text
        };
        let mut params = json!({
            "commandId": new_command_id(),
            "sessionId": self.session_id,
            "input": [{"type": "text", "text": prompt}],
        });
        if let Some(effort) = self.requested.effort.as_deref() {
            params["reasoningEffort"] = Value::String(effort.to_owned());
        }
        // `turn/start` answers with an admission ack (`status`, `turnId`,
        // `disposition`), not the finished turn: the turn ends later with
        // `turn/completed`. The ack is therefore waited for on its own
        // thread, so a slow host cannot block the caller's frame, and the
        // 30 s budget stays a deadline on that one request. A successful
        // ack deliberately does not end the turn.
        let pending = self.client.begin_request("turn/start", &params)?;
        self.done.store(false, Ordering::Release);
        self.turns += 1;

        let events = self.events_tx.clone();
        let done = Arc::clone(&self.done);
        thread::Builder::new()
            .name("kinewright-muse-turn".to_owned())
            .spawn(move || {
                if let Err(error) = pending.wait(MSP_REQUEST_TIMEOUT) {
                    let _ = events.send(AgentEvent::Error(error.to_string()));
                    send_done(&events, &done);
                }
            })
            .map_err(|error| {
                self.done.store(true, Ordering::Release);
                AgentError::Harness(error.to_string())
            })?;
        Ok(())
    }

    fn events(&self) -> Receiver<AgentEvent> {
        self.events_rx.clone()
    }

    /// Stop runs on the egui frame thread. `turn/cancel` takes effect when
    /// the host reads it, so the ack is not waited for: a host that has
    /// stopped answering — the reason Stop was pressed — would otherwise
    /// freeze the whole UI for the full 30-second request budget.
    fn interrupt(&mut self) {
        let was_running = !self.done.load(Ordering::Acquire);
        if was_running {
            let _ = self.client.begin_request(
                "turn/cancel",
                &json!({
                    "commandId": new_command_id(),
                    "sessionId": self.session_id,
                }),
            );
            let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        }
        send_done(&self.events_tx, &self.done);
    }
}

impl Drop for MuseSession {
    fn drop(&mut self) {
        self.client.kill();
        let _ = fs::remove_dir_all(&self.scratch_directory);
    }
}

fn spawn_muse_incoming(
    client: AcpClient,
    incoming: Receiver<AcpIncoming>,
    session_id: String,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
) -> Result<(), AgentError> {
    thread::Builder::new()
        .name("kinewright-muse-events".to_owned())
        .spawn(move || {
            let mut tool_names = HashMap::new();
            let mut turn_saw_usage = false;
            let mut reported_malformed = false;
            for message in incoming {
                match message {
                    AcpIncoming::Request { id, method, params } => {
                        handle_muse_request(&client, &session_id, &id, &method, &params);
                    }
                    AcpIncoming::Notification { method, params }
                        if method == "approval/requested"
                            && params.get("sessionId").and_then(Value::as_str)
                                == Some(session_id.as_str()) =>
                    {
                        decide_muse_approval(&client, &session_id, &params);
                    }
                    AcpIncoming::Notification { method, params }
                        if notification_targets_session(&method, &params, &session_id) =>
                    {
                        translate_muse_notification(
                            &method,
                            &params,
                            &events,
                            &done,
                            &mut tool_names,
                            &mut turn_saw_usage,
                        );
                    }
                    AcpIncoming::Notification { .. } => {}
                    // One unreadable line is not a dead host: ending the
                    // loop here would leave the session unable to translate
                    // any later transcript item or answer any approval.
                    AcpIncoming::Malformed(message) => {
                        if !reported_malformed {
                            reported_malformed = true;
                            let _ = events.send(AgentEvent::Error(message));
                        }
                    }
                    AcpIncoming::Fault(message) => {
                        if !done.load(Ordering::Acquire) {
                            let _ = events.send(AgentEvent::Error(message));
                            send_done(&events, &done);
                        }
                        break;
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

/// View notifications carry the owning `sessionId`; global host events (usage
/// observations, skill changes) are intentionally ignored.
fn notification_targets_session(method: &str, params: &Value, session_id: &str) -> bool {
    params.get("sessionId").and_then(Value::as_str) == Some(session_id)
        && matches!(
            method,
            "item/started"
                | "item/delta"
                | "item/updated"
                | "item/completed"
                | "turn/started"
                | "turn/completed"
                | "turn/unqueued"
                | "session/tokenUsage"
                | "session/modelRouteUnserved"
        )
}

fn handle_muse_request(
    client: &AcpClient,
    session_id: &str,
    id: &Value,
    method: &str,
    params: &Value,
) {
    match method {
        "approval/request" => {
            let _ = client.respond(id, &json!({}));
            decide_muse_approval(client, session_id, params);
        }
        "userInput/request" => {
            let _ = client.respond(id, &json!({}));
            if let Some(user_input_id) = params.get("userInputId").and_then(Value::as_str) {
                let _ = client.request(
                    "userInput/cancel",
                    &json!({
                        "commandId": new_command_id(),
                        "sessionId": session_id,
                        "userInputId": user_input_id,
                        "reason": "Kinewright runs Muse headless; user questions are auto-declined",
                    }),
                    MSP_REQUEST_TIMEOUT,
                );
            }
        }
        _ => {
            let _ = client.respond(id, &Value::Null);
        }
    }
}

/// Decide a pending approval. The host announces approvals with the
/// `approval/requested` notification (the must-answer `approval/request` may
/// never follow), which already carries everything `approval/decide` needs,
/// so both paths decide immediately: a headless turn must never wait on an
/// invisible prompt.
fn decide_muse_approval(client: &AcpClient, session_id: &str, params: &Value) {
    let Some(choice_id) = approval_decision(params) else {
        return;
    };
    let decide = json!({
        "commandId": new_command_id(),
        "sessionId": session_id,
        "approvalId": params.get("approvalId").cloned().unwrap_or(Value::Null),
        "requirementId": params.get("currentRequirementId").cloned().unwrap_or(Value::Null),
        "choiceId": choice_id,
    });
    if decide.get("approvalId").is_some_and(Value::is_null)
        || decide.get("requirementId").is_some_and(Value::is_null)
    {
        return;
    }
    let _ = client.request("approval/decide", &decide, MSP_REQUEST_TIMEOUT);
}

/// Auto-approval policy: prefer a one-shot `approved` choice, then a
/// session-scoped one. Policy amendments (which would persist rules to the
/// user's approval policy) are never selected; when nothing approves, the
/// denial choice is taken so the tool call fails fast instead of hanging the
/// headless turn.
fn approval_decision(params: &Value) -> Option<String> {
    let choices = params.get("availableChoices")?.as_array()?;
    let rank = |choice: &Value| -> Option<u32> {
        match (
            choice.get("decision").and_then(Value::as_str),
            choice.get("scope").and_then(Value::as_str),
        ) {
            (Some("approved"), Some("once")) => Some(0),
            (Some("approved"), Some("session")) => Some(1),
            (Some("approvedForSession"), _) => Some(2),
            (Some("approved"), _) => Some(3),
            (Some("denied"), _) => Some(4),
            _ => None,
        }
    };
    choices
        .iter()
        .filter_map(|choice| Some((rank(choice)?, choice.get("choiceId")?.as_str()?.to_owned())))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, choice_id)| choice_id)
}

fn translate_muse_notification(
    method: &str,
    params: &Value,
    events: &Sender<AgentEvent>,
    done: &AtomicBool,
    tool_names: &mut HashMap<String, String>,
    turn_saw_usage: &mut bool,
) {
    match method {
        "item/started" => {
            if let Some(item) = params.get("item") {
                translate_muse_item_started(item, events, tool_names);
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item") {
                translate_muse_item_completed(item, events, tool_names);
            }
        }
        "turn/started" => {
            *turn_saw_usage = false;
        }
        // An input that was queued and then dropped never gets its own
        // `turn/started`/`turn/completed`, so this is the only signal the
        // turn is over. Without it the session stays "running" for good.
        "turn/unqueued" => {
            send_done(events, done);
        }
        "session/tokenUsage" => {
            events.send(muse_cost(params)).ok();
            *turn_saw_usage = true;
        }
        "turn/completed" => {
            if !*turn_saw_usage && let Some(usage) = params.get("usage") {
                events.send(muse_cost(usage)).ok();
            }
            if params.get("terminal").and_then(Value::as_str) == Some("failed") {
                let detail = params
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        params
                            .get("reason")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "Muse turn failed".to_owned());
                events.send(AgentEvent::Error(detail)).ok();
            }
            send_done(events, done);
        }
        "session/modelRouteUnserved" if !done.load(Ordering::Acquire) => {
            events
                .send(AgentEvent::Error(
                    "Muse cannot serve the selected model on this account".to_owned(),
                ))
                .ok();
        }
        _ => {}
    }
}

fn translate_muse_item_started(
    item: &Value,
    events: &Sender<AgentEvent>,
    tool_names: &mut HashMap<String, String>,
) {
    if item.get("kind").and_then(Value::as_str) != Some("toolCall") {
        return;
    }
    let id = item
        .get("itemId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = tool_display_name(
        item.get("tool")
            .and_then(Value::as_str)
            .unwrap_or("Muse tool"),
        item.get("args").and_then(Value::as_str),
    );
    if !id.is_empty() {
        tool_names.insert(id.to_owned(), name.clone());
    }
    events
        .send(AgentEvent::ToolCall {
            name,
            arguments: item
                .get("args")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_owned(),
        })
        .ok();
}

fn translate_muse_item_completed(
    item: &Value,
    events: &Sender<AgentEvent>,
    tool_names: &mut HashMap<String, String>,
) {
    match item.get("kind").and_then(Value::as_str) {
        Some("agentMessage") => {
            if let Some(text) = item.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                events.send(AgentEvent::Text(text.to_owned())).ok();
            }
        }
        Some("toolCall") => {
            let id = item
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = tool_names.get(id).cloned().unwrap_or_else(|| {
                tool_display_name(
                    item.get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("Muse tool"),
                    item.get("args").and_then(Value::as_str),
                )
            });
            let result = item
                .get("visibleOutput")
                .and_then(Value::as_str)
                .filter(|output| !output.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    item.get("failureReason")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| {
                    item.get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                        .to_owned()
                });
            tool_names.remove(id);
            events.send(AgentEvent::ToolResult { name, result }).ok();
        }
        _ => {}
    }
}

/// The compact dispatcher reports its underlying capability, mirroring the
/// other harnesses. `args` is verbatim model JSON text, so a name lookup
/// parses it leniently and falls back to the tool name.
fn tool_display_name(tool: &str, args: Option<&str>) -> String {
    let display = tool.strip_prefix("mcp__kinewright__").unwrap_or(tool);
    if display == "invoke_capability" {
        args.and_then(|args| serde_json::from_str::<Value>(args).ok())
            .as_ref()
            .and_then(|args| args.get("name"))
            .and_then(Value::as_str)
            .map_or_else(|| display.to_owned(), str::to_owned)
    } else {
        display.to_owned()
    }
}

/// Meta reports cached tokens as a subset of input tokens and reasoning
/// tokens as a subset of output tokens; the mapping keeps that convention.
/// Per-call `session/tokenUsage` records carry the counted-once
/// `promptTokens` instead of the raw `inputTokens`, so both are accepted.
/// Map one MSP usage record to a `Cost` event.
///
/// The raw provider counters always sit in a `usage` object (MSP's
/// `TokenUsage`): `session/tokenUsage` carries it beside the server's
/// counted-once `promptTokens`, and `turn/completed` carries the turn
/// aggregate in the same shape. Only `promptTokens` is normalized for the
/// provider's cache convention, which is exactly Kinewright's `input_tokens`
/// contract, so it wins where the record has one; the aggregate has no
/// counted-once figure and falls back to the provider's own `inputTokens`.
fn muse_cost(record: &Value) -> AgentEvent {
    let raw = record.get("usage").unwrap_or(record);
    let cached = raw
        .get("cacheReadTokens")
        .and_then(Value::as_u64)
        .or_else(|| raw.get("cachedTokens").and_then(Value::as_u64));
    AgentEvent::Cost {
        input_tokens: record
            .get("promptTokens")
            .and_then(Value::as_u64)
            .or_else(|| raw.get("inputTokens").and_then(Value::as_u64))
            .unwrap_or(0),
        cached_input_tokens: cached,
        cache_creation_input_tokens: raw.get("cacheWriteTokens").and_then(Value::as_u64),
        output_tokens: raw.get("outputTokens").and_then(Value::as_u64).unwrap_or(0),
        reasoning_output_tokens: raw.get("reasoningTokens").and_then(Value::as_u64),
        cost_usd: None,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Instant;

    /// What a call on the caller's thread — the egui frame, in the app — may
    /// take. Generous on purpose: a small, busy CI runner can leave a thread
    /// off-CPU for a good fraction of a second, and this number only has to
    /// tell a stalled frame from a scheduling gap. Both behaviours it guards
    /// against cost whole seconds: the fake host below delays its ack by
    /// three, and a `turn/cancel` that waited would cost
    /// `MSP_REQUEST_TIMEOUT` (30 s, measured at 30.0 s before the fix).
    #[cfg(unix)]
    const FRAME_BOUND: Duration = Duration::from_secs(1);

    use super::*;

    #[test]
    fn driver_id_matches_the_public_harness_name() {
        assert_eq!(MuseDriver.id(), HarnessId::new("muse"));
    }

    #[test]
    fn command_ids_are_unique_uuid7() {
        let first = new_command_id();
        let second = new_command_id();
        assert_ne!(first, second);
        for id in [first, second] {
            assert_eq!(id.len(), 36);
            let parts: Vec<_> = id.split('-').collect();
            assert_eq!(parts.len(), 5);
            assert_eq!(&parts[2][..1], "7");
            assert!(matches!(&parts[3][..1], "8" | "9" | "a" | "b"));
        }
    }

    #[test]
    fn model_catalog_maps_verified_rows_with_full_efforts() {
        let models = parse_muse_models(&json!({
            "models": [
                {"modelId": "muse-spark-1.3", "displayLabel": "muse-spark-1.3"},
                {"modelId": "muse-spark-1.3-contributor"},
            ],
            "providerId": "meta",
        }));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "muse-spark-1.3");
        assert_eq!(models[0].label, "muse-spark-1.3");
        assert_eq!(models[1].label, "muse-spark-1.3-contributor");
        let efforts: Vec<_> = MUSE_EFFORTS
            .iter()
            .map(|effort| (*effort).to_owned())
            .collect();
        assert_eq!(models[0].efforts, efforts);
        assert!(models[0].tiers.is_empty());
        assert!(parse_muse_models(&json!({})).is_empty());
    }

    #[test]
    fn fallback_catalog_lists_the_verified_spark_models() {
        let ids: Vec<_> = muse_fallback_models()
            .iter()
            .map(|model| model.id.clone())
            .collect();
        assert_eq!(
            ids,
            [
                "muse-spark-1.3",
                "muse-spark-1.3-contributor",
                "muse-spark-1.2",
                "muse-spark-1.2-contributor",
            ]
        );
    }

    #[test]
    fn approval_policy_prefers_once_denies_fast_and_never_amends_policy() {
        let params = json!({
            "availableChoices": [
                {"choiceId": "deny", "decision": "denied", "scope": "once"},
                {"choiceId": "allow-session", "decision": "approved", "scope": "session"},
                {"choiceId": "allow-once", "decision": "approved", "scope": "once"},
                {"choiceId": "amend", "decision": "approvedPolicyAmendment", "scope": "localPersistent"},
            ],
        });
        assert_eq!(approval_decision(&params).as_deref(), Some("allow-once"));
        let deny_only = json!({
            "availableChoices": [
                {"choiceId": "amend", "decision": "approvedPolicyAmendment", "scope": "localPersistent"},
                {"choiceId": "deny", "decision": "denied", "scope": "once"},
            ],
        });
        assert_eq!(approval_decision(&deny_only).as_deref(), Some("deny"));
        assert_eq!(approval_decision(&json!({})), None);
    }

    #[test]
    fn transcript_items_translate_to_existing_agent_events() {
        let (tx, rx) = unbounded();
        let done = AtomicBool::new(false);
        let mut names = HashMap::new();
        let mut turn_saw_usage = false;
        translate_muse_notification(
            "item/started",
            &json!({"sessionId": "s1", "item": {
                "kind": "toolCall", "itemId": "t1",
                "tool": "mcp__kinewright__invoke_capability",
                "args": r#"{"name":"get_timeline_state","arguments":{}}"#,
            }}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(
            rx.recv().unwrap(),
            AgentEvent::ToolCall {
                name: "get_timeline_state".to_owned(),
                arguments: r#"{"name":"get_timeline_state","arguments":{}}"#.to_owned(),
            }
        );
        translate_muse_notification(
            "item/completed",
            &json!({"sessionId": "s1", "item": {
                "kind": "toolCall", "itemId": "t1", "status": "completed",
                "tool": "mcp__kinewright__invoke_capability",
                "visibleOutput": "project fps=30/1",
            }}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(
            rx.recv().unwrap(),
            AgentEvent::ToolResult {
                name: "get_timeline_state".to_owned(),
                result: "project fps=30/1".to_owned(),
            }
        );
        translate_muse_notification(
            "item/completed",
            &json!({"sessionId": "s1", "item": {
                "kind": "agentMessage", "itemId": "m1", "text": "Done.",
            }}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Text("Done.".to_owned()));
    }

    #[test]
    fn turn_completion_reports_usage_failure_and_done() {
        let (tx, rx) = unbounded();
        let done = AtomicBool::new(false);
        let mut names = HashMap::new();
        let mut turn_saw_usage = false;
        translate_muse_notification(
            "turn/completed",
            &json!({"sessionId": "s1", "terminal": "completed", "usage": {
                "inputTokens": 100, "cachedTokens": 20,
                "outputTokens": 30, "reasoningTokens": 5,
            }}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(
            rx.recv().unwrap(),
            AgentEvent::Cost {
                input_tokens: 100,
                cached_input_tokens: Some(20),
                cache_creation_input_tokens: None,
                output_tokens: 30,
                reasoning_output_tokens: Some(5),
                cost_usd: None,
            }
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Done);

        let done = AtomicBool::new(false);
        translate_muse_notification(
            "turn/completed",
            &json!({"sessionId": "s1", "terminal": "failed",
                "error": {"kind": "modelError", "message": "overloaded", "retryable": true}}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(
            rx.recv().unwrap(),
            AgentEvent::Error("overloaded".to_owned())
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Done);
    }

    #[test]
    fn per_call_usage_reports_once_and_suppresses_the_turn_aggregate() {
        let (tx, rx) = unbounded();
        let done = AtomicBool::new(false);
        let mut names = HashMap::new();
        let mut turn_saw_usage = false;
        translate_muse_notification(
            "turn/started",
            &json!({"sessionId": "s1"}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        // The shape is MSP's `SessionTokenUsageParams`: counted-once
        // `promptTokens` and `totalTokens` at the top, the raw provider
        // counters in `usage`.
        translate_muse_notification(
            "session/tokenUsage",
            &json!({
                "sessionId": "s1",
                "turnId": "t1",
                "promptTokens": 50,
                "totalTokens": 60,
                "usage": {
                    "inputTokens": 45,
                    "cachedTokens": 5,
                    "outputTokens": 10,
                    "reasoningTokens": 4,
                },
            }),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        assert_eq!(
            rx.recv().unwrap(),
            AgentEvent::Cost {
                // `promptTokens` wins: it is the cache-convention-normalized
                // figure, which is what `input_tokens` promises.
                input_tokens: 50,
                cached_input_tokens: Some(5),
                cache_creation_input_tokens: None,
                output_tokens: 10,
                reasoning_output_tokens: Some(4),
                cost_usd: None,
            }
        );
        translate_muse_notification(
            "turn/completed",
            &json!({"sessionId": "s1", "terminal": "completed", "usage": {
                "inputTokens": 50, "outputTokens": 10,
            }}),
            &tx,
            &done,
            &mut names,
            &mut turn_saw_usage,
        );
        // The aggregate is skipped: per-call records already reported it.
        assert_eq!(rx.recv().unwrap(), AgentEvent::Done);
    }

    #[test]
    fn off_session_notifications_are_ignored() {
        assert!(notification_targets_session(
            "item/completed",
            &json!({"sessionId": "s1"}),
            "s1"
        ));
        assert!(!notification_targets_session(
            "item/completed",
            &json!({"sessionId": "s2"}),
            "s1"
        ));
        assert!(!notification_targets_session(
            "usage/changed",
            &json!({}),
            "s1"
        ));
    }

    /// A minimal MSP host that admits a turn only after three seconds. Unix
    /// only:
    /// it is a POSIX shell script, and `cmd.exe` has no equivalent one-liner
    /// that reads a JSON-RPC line and answers it.
    #[cfg(unix)]
    const SLOW_MSP_HOST: &str = concat!(
        "while IFS= read -r line; do ",
        r#"id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); "#,
        "case \"$line\" in ",
        r#"*'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"grantedCapabilities":["sessionMcp"]}}\n' "$id" ;; "#,
        r#"*'"session/start"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"session":{"sessionId":"s1"}}}\n' "$id" ;; "#,
        r#"*'"turn/start"'*) sleep 3; printf '{"jsonrpc":"2.0","id":%s,"result":{"commandId":"c1","turnId":"t1","status":"accepted","disposition":"started","startedNewTurn":true}}\n' "$id" ;; "#,
        "esac; done",
    );

    /// `send_user_message` used to be a blocking `turn/start` request on the
    /// egui frame thread, so a host that took its time admitting the turn
    /// froze the UI for up to the full 30 s budget.
    #[cfg(unix)]
    #[test]
    fn a_slow_turn_admission_does_not_block_the_caller() {
        let mut command = ProcessCommand::new("sh");
        command.arg("-c").arg(SLOW_MSP_HOST);
        let mut session = MuseSession::start(
            command,
            "http://127.0.0.1:43123/mcp",
            SessionConfig {
                working_directory: None,
                model: None,
                effort: None,
                service_tier: None,
                max_turns: None,
                mcp_url: None,
                tool_names: None,
            },
        )
        .expect("the fake MSP host opens a session");
        let events = session.events();

        let started = Instant::now();
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");
        let elapsed = started.elapsed();
        assert!(
            elapsed < FRAME_BOUND,
            "send_user_message blocked the frame for {elapsed:?} waiting for \
             an ack this host holds back for three seconds"
        );
        assert!(!session.done.load(Ordering::Acquire), "the turn is running");

        // The ack lands about a second later. It must not report an error
        // and must not end the turn: only `turn/completed` does that.
        assert!(
            matches!(
                events.recv_timeout(Duration::from_secs(5)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout)
            ),
            "the admission ack must be silent"
        );
        assert!(!session.done.load(Ordering::Acquire));
    }

    /// A fake MSP host that greets on stdout before speaking JSON-RPC, prints
    /// progress mid-turn, and then runs one complete turn. `opencode acp` has
    /// `--print-logs` and Kimi and Qwen are Node CLIs, so a stray stdout line
    /// is the realistic case, not the exotic one.
    #[cfg(unix)]
    const CHATTY_MSP_HOST: &str = concat!(
        "printf 'muse serve is listening on stdio\\n'; ",
        "while IFS= read -r line; do ",
        r#"id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); "#,
        "case \"$line\" in ",
        r#"*'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"grantedCapabilities":["sessionMcp"]}}\n' "$id" ;; "#,
        r#"*'"session/start"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"session":{"sessionId":"s1"}}}\n' "$id" ;; "#,
        r#"*'"turn/start"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"commandId":"c1","turnId":"t1","status":"accepted","disposition":"started","startedNewTurn":true}}\n' "$id"; "#,
        "printf 'progress: thinking\\n'; ",
        r#"printf '{"jsonrpc":"2.0","method":"item/completed","params":{"sessionId":"s1","item":{"itemId":"i1","kind":"agentMessage","status":"completed","text":"Tightened."}}}\n'; "#,
        r#"printf '{"jsonrpc":"2.0","method":"turn/completed","params":{"sessionId":"s1","turnId":"t1","terminal":"completed"}}\n' ;; "#,
        "esac; done",
    );

    #[cfg(unix)]
    fn fake_muse_session(script: &str) -> MuseSession {
        let mut command = ProcessCommand::new("sh");
        command.arg("-c").arg(script);
        MuseSession::start(
            command,
            "http://127.0.0.1:43123/mcp",
            SessionConfig {
                working_directory: None,
                model: None,
                effort: None,
                service_tier: None,
                max_turns: None,
                mcp_url: None,
                tool_names: None,
            },
        )
        .expect("the fake MSP host opens a session")
    }

    /// One unreadable stdout line used to end the events loop for good, so
    /// every later transcript item was dropped and the turn ran blind to its
    /// timeout. It must be reported once and stepped over.
    #[cfg(unix)]
    #[test]
    fn a_banner_on_stdout_is_reported_once_and_the_session_keeps_translating() {
        let mut session = fake_muse_session(CHATTY_MSP_HOST);
        let events = session.events();
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");

        let mut seen = Vec::new();
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(event) => {
                    let finished = event == AgentEvent::Done;
                    seen.push(event);
                    if finished {
                        break;
                    }
                }
                Err(error) => panic!("the turn never finished ({error}): {seen:?}"),
            }
        }
        assert!(
            seen.contains(&AgentEvent::Text("Tightened.".to_owned())),
            "the transcript item after the banner was lost: {seen:?}"
        );
        let reported = seen
            .iter()
            .filter(|event| matches!(event, AgentEvent::Error(_)))
            .count();
        assert_eq!(reported, 1, "two banners must be reported once: {seen:?}");
    }

    /// Stop is pressed on the egui frame thread, and the host that needs
    /// stopping is exactly the one that has stopped answering. Waiting for
    /// the `turn/cancel` ack froze the UI for the full 30-second budget.
    #[cfg(unix)]
    #[test]
    fn a_stop_does_not_wait_on_a_host_that_ignores_the_cancel() {
        let mut session = fake_muse_session(SLOW_MSP_HOST);
        let events = session.events();
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");

        let started = Instant::now();
        session.interrupt();
        let elapsed = started.elapsed();
        assert!(
            elapsed < FRAME_BOUND,
            "interrupt blocked the frame for {elapsed:?}; this host never \
             answers `turn/cancel`, so waiting for it costs the full 30 s"
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            AgentEvent::Text("Stopped.".to_owned()),
            "the stop notice belongs before the completion marker"
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            AgentEvent::Done
        );
    }
}
