//! Cursor Agent integration over the Agent Client Protocol (ACP).

use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use openreel_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use serde_json::{Map, Value, json};

use crate::{
    acp::{AcpClient, AcpIncoming, send_done},
    drivers::{OPENREEL_SYSTEM_PROMPT, find_on_path},
    models::{ModelChoice, ServiceTier},
};

const ACP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ACP_PROMPT_TIMEOUT: Duration = Duration::from_mins(30);
static CURSOR_SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);
// Cursor's ACP config RPCs currently write through to the user's CLI-wide
// model preference. Keep that mutation scoped to one active OpenReel turn,
// then restore the exact values captured at session creation.
static CURSOR_CONFIG_LEASED: AtomicBool = AtomicBool::new(false);

pub const CURSOR_SANDBOX_NOTICE: &str = "Cursor sessions receive only the OpenReel HTTP MCP endpoint and run from an empty scratch directory. Cursor model settings are restored after each turn.";

#[derive(Debug, Default, Clone, Copy)]
pub struct CursorAcpDriver;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorSpawnTarget {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
}

impl CursorSpawnTarget {
    fn native(executable: PathBuf) -> Self {
        Self {
            executable,
            prefix_arguments: Vec::new(),
        }
    }

    fn command(&self) -> ProcessCommand {
        let mut command = ProcessCommand::new(&self.executable);
        command.args(&self.prefix_arguments);
        command
    }
}

#[derive(Debug, Clone, Default)]
struct CursorConfigSnapshot {
    values: Vec<(String, Value)>,
    options: Vec<Value>,
}

impl CursorConfigSnapshot {
    fn from_result(result: &Value) -> Self {
        Self {
            values: config_options(result)
                .iter()
                .filter_map(|option| {
                    Some((
                        option.get("id")?.as_str()?.to_owned(),
                        option.get("currentValue")?.clone(),
                    ))
                })
                .collect(),
            options: config_options(result).to_vec(),
        }
    }

    fn model(&self) -> Option<&Value> {
        self.values
            .iter()
            .find(|(id, _)| id == "model")
            .map(|(_, value)| value)
    }
}

impl AgentDriver for CursorAcpDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("cursor")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let target = find_cursor_spawn_target()?;
        let version = cursor_process_output(&target, &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        let status = cursor_process_output(&target, &["status", "--format", "json"])
            .and_then(|output| serde_json::from_str::<Value>(&output).ok());
        let authentication = status
            .as_ref()
            .map_or(AuthenticationStatus::Unknown, cursor_authentication_status);
        let subscription_tier = cursor_process_output(&target, &["about", "--format", "json"])
            .and_then(|output| serde_json::from_str::<Value>(&output).ok())
            .and_then(|about| {
                about
                    .get("subscriptionTier")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        Some(HarnessInfo {
            id: self.id(),
            executable: target.executable,
            version,
            authentication,
            subscription_tier,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let target = find_cursor_spawn_target().ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        CursorSession::spawn(&target, &endpoint, cfg).map(|session| Box::new(session) as _)
    }
}

/// Models and per-model reasoning/speed controls advertised by the installed
/// Cursor ACP extension. Discovery is deliberately live: Cursor owns this
/// catalog and can update it independently of `OpenReel`.
#[must_use]
pub fn cursor_models() -> Vec<ModelChoice> {
    let Some(target) = find_cursor_spawn_target() else {
        return Vec::new();
    };
    cursor_catalog(&target).unwrap_or_default()
}

fn cursor_catalog(target: &CursorSpawnTarget) -> Result<Vec<ModelChoice>, AgentError> {
    let mut command = target.command();
    command.arg("acp");
    let client = AcpClient::spawn(command, "Cursor Agent")?;
    initialize_cursor(&client)?;
    let catalog = client.request(
        "cursor/list_available_models",
        &json!({}),
        ACP_REQUEST_TIMEOUT,
    )?;
    client.kill();
    Ok(parse_cursor_models(&catalog))
}

fn parse_cursor_models(catalog: &Value) -> Vec<ModelChoice> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("value").and_then(Value::as_str)?;
            let label = model.get("name").and_then(Value::as_str).unwrap_or(id);
            let options = model
                .get("configOptions")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let efforts = options
                .iter()
                .find(|option| {
                    option.get("id").and_then(Value::as_str).is_some_and(|id| {
                        id.eq_ignore_ascii_case("effort") || id.eq_ignore_ascii_case("reasoning")
                    })
                })
                .and_then(|option| option.get("options"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("value").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
            let has_fast = options.iter().any(|option| {
                option.get("id").and_then(Value::as_str) == Some("fast")
                    && option
                        .get("options")
                        .and_then(Value::as_array)
                        .is_some_and(|values| {
                            values.iter().any(|value| {
                                value.get("value").is_some_and(|value| {
                                    value == "true" || value == &Value::Bool(true)
                                })
                            })
                        })
            });
            let tiers = if has_fast {
                vec![ServiceTier {
                    id: "true".to_owned(),
                    name: "Fast".to_owned(),
                }]
            } else {
                Vec::new()
            };
            Some(ModelChoice {
                id: id.to_owned(),
                label: label.to_owned(),
                efforts,
                tiers,
            })
        })
        .collect()
}

struct CursorSession {
    client: AcpClient,
    session_id: String,
    requested: SessionConfig,
    original_config: CursorConfigSnapshot,
    scratch_directory: PathBuf,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    turn_has_config_lease: Arc<AtomicBool>,
    restore_lock: Arc<Mutex<()>>,
    turns: u32,
}

impl CursorSession {
    fn spawn(
        target: &CursorSpawnTarget,
        endpoint: &str,
        requested: SessionConfig,
    ) -> Result<Self, AgentError> {
        let scratch_directory = create_cursor_scratch_directory()?;
        let mut command = target.command();
        command.arg("acp").current_dir(&scratch_directory);
        let client = match AcpClient::spawn(command, "Cursor Agent") {
            Ok(client) => client,
            Err(error) => {
                let _ = fs::remove_dir_all(&scratch_directory);
                return Err(error);
            }
        };
        if let Err(error) = initialize_cursor(&client) {
            client.kill();
            let _ = fs::remove_dir_all(&scratch_directory);
            return Err(error);
        }
        let new_session = client.request(
            "session/new",
            &json!({
                "cwd": scratch_directory.to_string_lossy(),
                "mcpServers": [{
                    "type": "http",
                    "name": "openreel",
                    "url": endpoint,
                    "headers": [],
                }],
            }),
            ACP_REQUEST_TIMEOUT,
        )?;
        let session_id = new_session
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Protocol("Cursor session/new omitted sessionId".to_owned()))?
            .to_owned();
        let original_config = CursorConfigSnapshot::from_result(&new_session);

        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(true));
        let turn_has_config_lease = Arc::new(AtomicBool::new(false));
        let restore_lock = Arc::new(Mutex::new(()));
        spawn_cursor_incoming(
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
            original_config,
            scratch_directory,
            events_rx,
            events_tx,
            done,
            turn_has_config_lease,
            restore_lock,
            turns: 0,
        })
    }

    fn restore_configuration(&self) {
        restore_configuration(
            &self.client,
            &self.session_id,
            &self.original_config,
            &self.turn_has_config_lease,
            &self.restore_lock,
        );
    }
}

impl AgentSession for CursorSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if !self.done.load(Ordering::Acquire) {
            return Err(AgentError::Harness(
                "Cursor is still processing the previous turn".to_owned(),
            ));
        }
        if let Some(cap) = self.requested.max_turns.map(|cap| cap.max(1))
            && self.turns >= cap
        {
            return Err(AgentError::Harness(format!(
                "Turn cap reached ({cap}); start a new Cursor session to continue."
            )));
        }
        if CURSOR_CONFIG_LEASED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AgentError::Unavailable(
                "another OpenReel Cursor turn is active; wait for it to finish or stop it"
                    .to_owned(),
            ));
        }
        self.turn_has_config_lease.store(true, Ordering::Release);
        if let Err(error) = apply_requested_configuration(
            &self.client,
            &self.session_id,
            &self.requested,
            &self.original_config,
        ) {
            self.restore_configuration();
            return Err(error);
        }

        let prompt = if self.turns == 0 {
            format!("{OPENREEL_SYSTEM_PROMPT}\n\nUser request:\n{text}")
        } else {
            text
        };
        let pending = match self.client.begin_request(
            "session/prompt",
            &json!({
                "sessionId": self.session_id,
                "prompt": [{"type": "text", "text": prompt}],
            }),
        ) {
            Ok(pending) => pending,
            Err(error) => {
                self.restore_configuration();
                return Err(error);
            }
        };
        self.done.store(false, Ordering::Release);
        self.turns += 1;

        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let original_config = self.original_config.clone();
        let events = self.events_tx.clone();
        let done = Arc::clone(&self.done);
        let has_lease = Arc::clone(&self.turn_has_config_lease);
        let restore_lock = Arc::clone(&self.restore_lock);
        thread::Builder::new()
            .name("openreel-cursor-turn".to_owned())
            .spawn(move || {
                let reply = pending.wait(ACP_PROMPT_TIMEOUT);
                restore_configuration(
                    &client,
                    &session_id,
                    &original_config,
                    &has_lease,
                    &restore_lock,
                );
                match reply {
                    Ok(result) => {
                        let stop_reason = result
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .unwrap_or("end_turn");
                        if !matches!(stop_reason, "end_turn" | "cancelled") {
                            let _ = events.send(AgentEvent::Error(format!(
                                "Cursor stopped the turn: {stop_reason}"
                            )));
                        }
                    }
                    Err(error) => {
                        let _ = events.send(AgentEvent::Error(error.to_string()));
                    }
                }
                send_done(&events, &done);
            })
            .map_err(|error| {
                self.restore_configuration();
                self.done.store(true, Ordering::Release);
                AgentError::Harness(error.to_string())
            })?;
        Ok(())
    }

    fn events(&self) -> Receiver<AgentEvent> {
        self.events_rx.clone()
    }

    fn interrupt(&mut self) {
        let was_running = !self.done.load(Ordering::Acquire);
        if was_running {
            let _ = self
                .client
                .notify("session/cancel", &json!({"sessionId": self.session_id}));
            // Give Cursor a brief chance to acknowledge cancellation so the
            // turn worker can restore the user's model configuration cleanly.
            for _ in 0..20 {
                if self.done.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            self.restore_configuration();
            let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        }
        // Cursor ACP is long-lived. Closing a finished OpenReel session must
        // still end the child so its HTTP MCP connection cannot hold project
        // shutdown open indefinitely.
        self.client.kill();
        send_done(&self.events_tx, &self.done);
    }
}

impl Drop for CursorSession {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            let _ = self
                .client
                .notify("session/cancel", &json!({"sessionId": self.session_id}));
        }
        self.restore_configuration();
        self.client.kill();
        let _ = fs::remove_dir_all(&self.scratch_directory);
    }
}

fn initialize_cursor(client: &AcpClient) -> Result<Value, AgentError> {
    let initialized = client.request(
        "initialize",
        &json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "_meta": {"parameterizedModelPicker": true}
            },
            "clientInfo": {
                "name": "OpenReel",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
        ACP_REQUEST_TIMEOUT,
    )?;
    if initialized.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return Err(AgentError::Protocol(
            "Cursor did not negotiate ACP protocol version 1".to_owned(),
        ));
    }
    if initialized
        .pointer("/agentCapabilities/mcpCapabilities/http")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(AgentError::Unavailable(
            "the installed Cursor Agent does not support HTTP MCP servers over ACP".to_owned(),
        ));
    }
    Ok(initialized)
}

fn apply_requested_configuration(
    client: &AcpClient,
    session_id: &str,
    requested: &SessionConfig,
    original: &CursorConfigSnapshot,
) -> Result<(), AgentError> {
    let mut state = Value::Object(Map::from_iter([(
        "configOptions".to_owned(),
        Value::Array(original.options.clone()),
    )]));
    if let Some(model) = &requested.model {
        state = set_cursor_config(client, session_id, "model", &Value::String(model.clone()))?;
    }
    if let Some(effort) = &requested.effort {
        let config_id = ["effort", "reasoning"]
            .into_iter()
            .find(|id| config_option(&state, id).is_some())
            .ok_or_else(|| {
                AgentError::Unavailable(
                    "the selected Cursor model does not expose an effort control".to_owned(),
                )
            })?;
        ensure_config_value(&state, config_id, effort)?;
        state = set_cursor_config(
            client,
            session_id,
            config_id,
            &Value::String(effort.clone()),
        )?;
    }
    if config_option(&state, "fast").is_some() {
        let fast = requested.service_tier.as_deref() == Some("true");
        let value = if config_uses_string_values(&state, "fast") {
            Value::String(fast.to_string())
        } else {
            Value::Bool(fast)
        };
        let _ = set_cursor_config(client, session_id, "fast", &value)?;
    } else if requested.service_tier.is_some() {
        return Err(AgentError::Unavailable(
            "the selected Cursor model does not offer Fast mode".to_owned(),
        ));
    }
    Ok(())
}

fn restore_configuration(
    client: &AcpClient,
    session_id: &str,
    original: &CursorConfigSnapshot,
    has_lease: &AtomicBool,
    restore_lock: &Mutex<()>,
) {
    if !has_lease.swap(false, Ordering::AcqRel) {
        return;
    }
    let _guard = restore_lock.lock().ok();
    if let Some(model) = original.model() {
        let _ = set_cursor_config(client, session_id, "model", model);
    }
    for (id, value) in &original.values {
        if matches!(id.as_str(), "mode" | "model") {
            continue;
        }
        let _ = set_cursor_config(client, session_id, id, value);
    }
    CURSOR_CONFIG_LEASED.store(false, Ordering::Release);
}

fn set_cursor_config(
    client: &AcpClient,
    session_id: &str,
    config_id: &str,
    value: &Value,
) -> Result<Value, AgentError> {
    client.request(
        "session/set_config_option",
        &json!({
            "sessionId": session_id,
            "configId": config_id,
            "value": value,
        }),
        ACP_REQUEST_TIMEOUT,
    )
}

fn config_options(result: &Value) -> &[Value] {
    result
        .get("configOptions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn config_option<'a>(result: &'a Value, id: &str) -> Option<&'a Value> {
    config_options(result)
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
}

fn config_uses_string_values(result: &Value, id: &str) -> bool {
    config_option(result, id)
        .and_then(|option| option.get("options"))
        .and_then(Value::as_array)
        .and_then(|options| options.first())
        .and_then(|option| option.get("value"))
        .is_some_and(Value::is_string)
}

fn ensure_config_value(result: &Value, id: &str, wanted: &str) -> Result<(), AgentError> {
    let valid = config_option(result, id)
        .and_then(|option| option.get("options"))
        .and_then(Value::as_array)
        .is_some_and(|options| {
            options
                .iter()
                .any(|option| option.get("value").and_then(Value::as_str) == Some(wanted))
        });
    if valid {
        Ok(())
    } else {
        Err(AgentError::Unavailable(format!(
            "Cursor model does not support {id}={wanted}"
        )))
    }
}

fn spawn_cursor_incoming(
    client: AcpClient,
    incoming: Receiver<AcpIncoming>,
    session_id: String,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
) -> Result<(), AgentError> {
    thread::Builder::new()
        .name("openreel-cursor-events".to_owned())
        .spawn(move || {
            let mut tool_names = HashMap::new();
            let mut finished_tools = HashSet::new();
            for message in incoming {
                match message {
                    AcpIncoming::Request { id, method, params }
                        if method == "session/request_permission" =>
                    {
                        let option_id = params
                            .get("options")
                            .and_then(Value::as_array)
                            .and_then(|options| {
                                options
                                    .iter()
                                    .find(|option| {
                                        option.get("kind").and_then(Value::as_str)
                                            == Some("allow_once")
                                    })
                                    .or_else(|| {
                                        options.iter().find(|option| {
                                            option.get("kind").and_then(Value::as_str)
                                                == Some("allow_always")
                                        })
                                    })
                            })
                            .and_then(|option| option.get("optionId"))
                            .cloned();
                        let outcome = option_id.map_or_else(
                            || json!({"outcome": "cancelled"}),
                            |option_id| json!({"outcome": "selected", "optionId": option_id}),
                        );
                        let _ = client.respond(&id, &json!({"outcome": outcome}));
                    }
                    AcpIncoming::Request { id, .. } => {
                        let _ = client.respond(&id, &Value::Null);
                    }
                    AcpIncoming::Notification { method, params }
                        if method == "session/update"
                            && params.get("sessionId").and_then(Value::as_str)
                                == Some(session_id.as_str()) =>
                    {
                        if let Some(update) = params.get("update") {
                            translate_update(update, &events, &mut tool_names, &mut finished_tools);
                        }
                    }
                    AcpIncoming::Notification { .. } => {}
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

fn translate_update(
    update: &Value,
    events: &Sender<AgentEvent>,
    tool_names: &mut HashMap<String, String>,
    finished_tools: &mut HashSet<String>,
) {
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            if let Some(text) = content_text(update.get("content"))
                && !text.is_empty()
            {
                let _ = events.send(AgentEvent::Text(text));
            }
        }
        Some("tool_call") => {
            let id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("cursor-tool")
                .to_owned();
            let raw_name = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or("Cursor tool");
            let raw_input = update.get("rawInput").filter(|value| !value.is_null());
            let name = if raw_name.ends_with("invoke_capability") {
                raw_input
                    .and_then(|value| value.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(raw_name)
                    .to_owned()
            } else {
                raw_name.to_owned()
            };
            let arguments = raw_input.map_or_else(|| "{}".to_owned(), Value::to_string);
            tool_names.insert(id, name.clone());
            let _ = events.send(AgentEvent::ToolCall { name, arguments });
        }
        Some("tool_call_update") => {
            let id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("cursor-tool");
            let status = update.get("status").and_then(Value::as_str);
            if matches!(status, Some("completed" | "failed"))
                && finished_tools.insert(id.to_owned())
            {
                let name = tool_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| "Cursor tool".to_owned());
                let result = update
                    .get("rawOutput")
                    .filter(|value| !value.is_null())
                    .map(Value::to_string)
                    .or_else(|| content_text(update.get("content")))
                    .unwrap_or_else(|| status.unwrap_or("completed").to_owned());
                let _ = events.send(AgentEvent::ToolResult { name, result });
            }
        }
        _ => {}
    }
}

fn content_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    content.as_array().map(|blocks| {
        blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn create_cursor_scratch_directory() -> Result<PathBuf, AgentError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..16 {
        let counter = CURSOR_SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "openreel-cursor-{}-{now}-{counter}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(AgentError::Harness(format!(
                    "could not create the Cursor scratch directory: {error}"
                )));
            }
        }
    }
    Err(AgentError::Harness(
        "could not allocate a unique Cursor scratch directory".to_owned(),
    ))
}

fn find_cursor_spawn_target() -> Option<CursorSpawnTarget> {
    let launcher = find_on_path("agent").or_else(|| find_on_path("cursor-agent"))?;
    resolve_cursor_spawn_target(&launcher)
}

fn resolve_cursor_spawn_target(launcher: &Path) -> Option<CursorSpawnTarget> {
    let is_script_shim = launcher.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy();
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("ps1")
    });
    if !is_script_shim {
        return Some(CursorSpawnTarget::native(launcher.to_owned()));
    }
    let root = launcher.parent()?;
    let direct_node = root.join("node.exe");
    let direct_entry = root.join("index.js");
    if direct_node.is_file() && direct_entry.is_file() {
        return Some(CursorSpawnTarget {
            executable: direct_node,
            prefix_arguments: vec![direct_entry.into_os_string()],
        });
    }
    let version = fs::read_dir(root.join("versions"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            entry.path().join("node.exe").is_file() && entry.path().join("index.js").is_file()
        })
        .max_by_key(|entry| cursor_version_key(&entry.file_name().to_string_lossy()))?;
    Some(CursorSpawnTarget {
        executable: version.path().join("node.exe"),
        prefix_arguments: vec![version.path().join("index.js").into_os_string()],
    })
}

fn cursor_version_key(version: &str) -> (u32, u32, u32, u32, u32, u32) {
    let numbers = version
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    (
        numbers.first().copied().unwrap_or_default(),
        numbers.get(1).copied().unwrap_or_default(),
        numbers.get(2).copied().unwrap_or_default(),
        numbers.get(3).copied().unwrap_or_default(),
        numbers.get(4).copied().unwrap_or_default(),
        numbers.get(5).copied().unwrap_or_default(),
    )
}

fn cursor_process_output(target: &CursorSpawnTarget, arguments: &[&str]) -> Option<String> {
    let mut command = target.command();
    command.args(arguments).stdin(Stdio::null());
    hide_console_window(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Some(if stdout.trim().is_empty() {
        stderr.into_owned()
    } else {
        stdout.into_owned()
    })
}

fn cursor_authentication_status(status: &Value) -> AuthenticationStatus {
    status
        .get("isAuthenticated")
        .or_else(|| status.get("authenticated"))
        .and_then(Value::as_bool)
        .map_or_else(
            || match status.get("status").and_then(Value::as_str) {
                Some("authenticated") => AuthenticationStatus::Authenticated,
                Some("unauthenticated") => AuthenticationStatus::Unauthenticated,
                _ => AuthenticationStatus::Unknown,
            },
            |authenticated| {
                if authenticated {
                    AuthenticationStatus::Authenticated
                } else {
                    AuthenticationStatus::Unauthenticated
                }
            },
        )
}

#[cfg(windows)]
fn hide_console_window(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut ProcessCommand) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_id_matches_the_public_harness_name() {
        assert_eq!(CursorAcpDriver.id(), HarnessId::new("cursor"));
    }

    #[test]
    fn cursor_status_parses_current_and_legacy_authentication_shapes() {
        assert_eq!(
            cursor_authentication_status(&json!({"status":"authenticated","isAuthenticated":true})),
            AuthenticationStatus::Authenticated
        );
        assert_eq!(
            cursor_authentication_status(&json!({"authenticated":false})),
            AuthenticationStatus::Unauthenticated
        );
        assert_eq!(
            cursor_authentication_status(&json!({"status":"unavailable"})),
            AuthenticationStatus::Unknown
        );
    }

    #[test]
    fn cursor_catalog_maps_reasoning_and_fast_to_existing_pickers() {
        let models = parse_cursor_models(&json!({
            "models": [{
                "value": "gpt-5.6-sol",
                "name": "GPT-5.6 Sol",
                "configOptions": [
                    {"id":"reasoning","options":[{"value":"low"},{"value":"xhigh"}]},
                    {"id":"fast","options":[{"value":"false"},{"value":"true"}]}
                ]
            }]
        }));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].efforts, ["low", "xhigh"]);
        assert_eq!(
            models[0].tiers,
            [ServiceTier {
                id: "true".to_owned(),
                name: "Fast".to_owned()
            }]
        );
    }

    #[test]
    fn cursor_shim_resolves_to_the_newest_native_node_bundle() {
        let root = create_cursor_scratch_directory().unwrap();
        let shim = root.join("agent.cmd");
        fs::write(&shim, "@echo off\r\n").unwrap();
        for version in ["2026.07.9-old", "2026.08.11-new"] {
            let directory = root.join("versions").join(version);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("node.exe"), b"node").unwrap();
            fs::write(directory.join("index.js"), b"entry").unwrap();
        }
        let target = resolve_cursor_spawn_target(&shim).unwrap();
        assert!(
            target
                .executable
                .to_string_lossy()
                .contains("2026.08.11-new")
        );
        assert_eq!(target.prefix_arguments.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cursor_permission_result_uses_the_acp_selected_shape() {
        let outcome = json!({"outcome": "selected", "optionId": "allow-once"});
        assert_eq!(
            json!({"outcome": outcome})["outcome"]["optionId"],
            "allow-once"
        );
    }

    #[test]
    fn message_and_tool_updates_translate_to_existing_agent_events() {
        let (tx, rx) = unbounded();
        let mut names = HashMap::new();
        let mut finished = HashSet::new();
        translate_update(
            &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done"}}),
            &tx,
            &mut names,
            &mut finished,
        );
        translate_update(
            &json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"get_timeline_state","rawInput":{"include":"all"}}),
            &tx,
            &mut names,
            &mut finished,
        );
        translate_update(
            &json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"ok":true}}),
            &tx,
            &mut names,
            &mut finished,
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Text("Done".to_owned()));
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolCall { name, .. } if name == "get_timeline_state")
        );
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolResult { name, .. } if name == "get_timeline_state")
        );
    }
}
