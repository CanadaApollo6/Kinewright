//! Shared session runtime for ACP-speaking agent CLIs.
//!
//! `OpenCode`, Qwen Code, Kimi, Kiro, and Devin all serve the Agent Client
//! Protocol over stdio with the same core flow (`initialize`, `session/new`
//! with inline MCP servers, `session/prompt`, `session/cancel`), so one
//! runtime carries every turn while each harness keeps only its spawn
//! arguments, detection, and model catalog. Cursor rides here as well: the
//! one thing it does that the others do not — persisting
//! `session/set_config_option` values CLI-wide, so its configuration must be
//! snapshotted, leased to one turn and restored — is a spec field rather
//! than a second copy of the runtime.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use kinewright_core::{AgentError, AgentEvent, AgentSession, SessionConfig};
use serde_json::{Value, json};

use crate::{
    acp::{AcpClient, AcpIncoming, send_done},
    child_process::create_scratch_directory,
    drivers::KINEWRIGHT_SYSTEM_PROMPT,
};

const ACP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ACP_PROMPT_TIMEOUT: Duration = Duration::from_mins(30);
/// Total budget for putting a leased configuration back. It runs on the
/// teardown thread, but a harness that has stopped answering must not hold
/// the process-wide lease open for `n × 30 s`.
const CONFIG_RESTORE_BUDGET: Duration = Duration::from_secs(5);

/// Fail-fast check on an `initialize` result (Devin proves its MCP config
/// redirect landed before opening a session).
type VerifyInitialized = fn(&Value) -> Result<(), AgentError>;

/// How a harness's requested model reaches the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSelection {
    /// Chosen with spawn arguments; runtime configuration is never touched
    /// and the session's opening options are the whole story.
    SpawnArgument,
    /// Applied with `session/set_config_option`, refused unless the session
    /// advertises it (`OpenCode`, which has no model spawn flag).
    AdvertisedOption,
    /// Applied with `session/set_config_option` without that check, and each
    /// reply restates the option ladder, so later ids resolve against the
    /// live configuration. Cursor's picker comes from its own
    /// `cursor/list_available_models` call and changing its model changes
    /// which efforts exist.
    LiveOption,
}

impl ModelSelection {
    const fn applies_model(self) -> bool {
        !matches!(self, Self::SpawnArgument)
    }

    const fn validates_model(self) -> bool {
        matches!(self, Self::AdvertisedOption)
    }

    const fn tracks_live_config(self) -> bool {
        matches!(self, Self::LiveOption)
    }
}

/// Per-harness knobs for the shared ACP runtime.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AcpSessionSpec {
    /// CLI-facing name used in errors and fallbacks ("`OpenCode`").
    pub label: &'static str,
    /// Process name for stream diagnostics ("`OpenCode` ACP").
    pub process_name: &'static str,
    /// Scratch directory infix ("opencode").
    pub scratch_infix: &'static str,
    /// Whether `session/new` carries the Kinewright endpoint inline. Devin
    /// advertises no HTTP MCP capability and reads its config file instead.
    pub inline_mcp: bool,
    /// Whether to fail closed when the agent does not advertise HTTP MCP.
    pub require_http_mcp: bool,
    /// How the requested model reaches the session.
    pub model_selection: ModelSelection,
    /// Set when the harness persists configuration CLI-wide rather than per
    /// session. Kinewright then admits one configuring turn at a time through
    /// this process-wide flag and puts back every value the turn overwrote,
    /// on completion, failure, interrupt and drop. Cursor is the only such
    /// harness; this is an upstream limitation.
    pub config_lease: Option<&'static AtomicBool>,
    /// Optional fail-fast check on the `initialize` result (Devin uses it to
    /// prove its MCP config redirect landed before opening a session).
    pub verify_initialized: Option<VerifyInitialized>,
}

/// What one turn overwrote, as `(config id, the value that was in effect)`,
/// in the order it was written.
///
/// Restoring the *opening* `session/new` options is not enough and was the
/// defect this replaced: real Cursor answers `session/new` with only
/// `mode` and `model`, while `fast` and `effort` are per-model and appear in
/// the ladder only after a model is selected. A turn that switched Fast off
/// therefore had nothing to put back, and left the user's CLI-wide setting
/// changed for good. Capturing each option's `currentValue` from the ladder
/// the write is about to overwrite restores exactly what was taken.
type OverwrittenConfig = Vec<(String, Value)>;

/// The value a config option currently holds in an advertised ladder.
fn current_value(config_options: &[Value], id: &str) -> Option<Value> {
    config_option(config_options, id)
        .and_then(|option| option.get("currentValue"))
        .cloned()
}

/// The `set_config_option` calls that undo `overwritten`. The model goes
/// back first: it decides which other options exist, so restoring `fast` or
/// `effort` under the wrong model would be refused or land on the wrong one.
fn restore_writes(overwritten: &OverwrittenConfig) -> Vec<(String, Value)> {
    let (model, rest): (Vec<_>, Vec<_>) = overwritten
        .iter()
        .cloned()
        .partition(|(id, _)| id == "model");
    model.into_iter().chain(rest).collect()
}

/// One long-lived ACP agent session: a single child process, a single ACP
/// session in an empty scratch directory, and one prompt at a time.
pub(crate) struct GenericAcpSession {
    client: AcpClient,
    session_id: String,
    requested: SessionConfig,
    spec: AcpSessionSpec,
    config_options: Vec<Value>,
    /// What this session's turns have overwritten and still owe back.
    overwritten: Arc<Mutex<OverwrittenConfig>>,
    scratch_directory: PathBuf,
    /// Extra harness-owned directories removed with the session (Devin's
    /// isolated MCP config, which must outlive `start_session`).
    pub cleanup_paths: Vec<PathBuf>,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    /// Whether this session currently holds `spec.config_lease`.
    turn_has_config_lease: Arc<AtomicBool>,
    /// Serializes the restore against a concurrent interrupt or drop.
    restore_lock: Arc<Mutex<()>>,
    turns: u32,
}

impl GenericAcpSession {
    /// Start `command` (already carrying the harness's model/effort spawn
    /// arguments and environment) and open one ACP session on it.
    pub(crate) fn start(
        mut command: ProcessCommand,
        endpoint: &str,
        requested: SessionConfig,
        spec: AcpSessionSpec,
    ) -> Result<Self, AgentError> {
        let scratch_directory = create_scratch_directory(spec.scratch_infix)?;
        command.current_dir(&scratch_directory);
        let client = match AcpClient::spawn(command, spec.process_name) {
            Ok(client) => client,
            Err(error) => {
                let _ = fs::remove_dir_all(&scratch_directory);
                return Err(error);
            }
        };
        let (session_id, config_options) =
            match open_acp_session(&client, endpoint, &scratch_directory, &spec) {
                Ok(opened) => opened,
                Err(error) => {
                    client.kill();
                    let _ = fs::remove_dir_all(&scratch_directory);
                    return Err(error);
                }
            };

        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(true));
        let incoming = client.incoming();
        let session = Self {
            client,
            session_id,
            requested,
            spec,
            config_options,
            overwritten: Arc::new(Mutex::new(OverwrittenConfig::new())),
            scratch_directory,
            cleanup_paths: Vec::new(),
            events_rx,
            events_tx,
            done,
            turn_has_config_lease: Arc::new(AtomicBool::new(false)),
            restore_lock: Arc::new(Mutex::new(())),
            turns: 0,
        };
        // Built after the session so the events thread carries the same
        // restore handle the turn thread does; on failure `Drop` cancels,
        // kills and removes the scratch directory.
        spawn_acp_incoming(
            session.client.clone(),
            incoming,
            session.session_id.clone(),
            session.events_tx.clone(),
            Arc::clone(&session.done),
            spec.label,
            session.turn_restore(),
        )?;
        Ok(session)
    }

    /// Take the process-wide configuration lease for this turn, if the
    /// harness needs one. A second Kinewright turn must not overwrite the
    /// first's CLI-wide configuration while it runs.
    fn acquire_config_lease(&self) -> Result<(), AgentError> {
        let Some(lease) = self.spec.config_lease else {
            return Ok(());
        };
        if lease
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AgentError::Unavailable(format!(
                "another Kinewright {} turn is active; wait for it to finish or stop it",
                self.spec.label
            )));
        }
        self.turn_has_config_lease.store(true, Ordering::Release);
        Ok(())
    }

    fn restore_configuration(&self) {
        self.turn_restore().run();
    }

    /// Wind the session down off the caller's thread: wait briefly for the
    /// turn to settle, put a leased configuration back (bounded by
    /// [`CONFIG_RESTORE_BUDGET`]), kill the child, then remove `paths`.
    /// The client is cloned in, so the child is still killed when the
    /// session itself is dropped first.
    fn spawn_teardown(&self, paths: Vec<PathBuf>) {
        let client = self.client.clone();
        let restore = self.turn_restore();
        let done = Arc::clone(&self.done);
        let teardown = move || {
            for _ in 0..20 {
                if done.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            restore.run();
            client.kill();
            for path in paths {
                if !path.as_os_str().is_empty() {
                    let _ = fs::remove_dir_all(path);
                }
            }
        };
        if let Err(error) = thread::Builder::new()
            .name(format!("kinewright-acp-teardown-{}", self.spec.label))
            .spawn(teardown)
        {
            // Out of threads: a stalled frame beats a leaked child.
            drop(error);
            self.restore_configuration();
            self.client.kill();
        }
    }
}

fn open_acp_session(
    client: &AcpClient,
    endpoint: &str,
    scratch_directory: &Path,
    spec: &AcpSessionSpec,
) -> Result<(String, Vec<Value>), AgentError> {
    acp_initialize(client, spec)?;
    let mcp_servers = if spec.inline_mcp {
        json!([{
            "type": "http",
            "name": "kinewright",
            "url": endpoint,
            "headers": [],
        }])
    } else {
        json!([])
    };
    let new_session = client.request(
        "session/new",
        &json!({
            "cwd": scratch_directory.to_string_lossy(),
            "mcpServers": mcp_servers,
        }),
        ACP_REQUEST_TIMEOUT,
    )?;
    let session_id = new_session
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AgentError::Protocol(format!("{} session/new omitted sessionId", spec.label))
        })?
        .to_owned();
    Ok((session_id, config_options_of(&new_session)))
}

/// Negotiate ACP v1 and check the harness advertises what the spec needs.
/// Shared with the catalog probes, which initialize without opening a
/// session.
pub(crate) fn acp_initialize(
    client: &AcpClient,
    spec: &AcpSessionSpec,
) -> Result<Value, AgentError> {
    let initialized = client.request(
        "initialize",
        &json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "_meta": {"parameterizedModelPicker": true}
            },
            "clientInfo": {
                "name": "Kinewright",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
        ACP_REQUEST_TIMEOUT,
    )?;
    if initialized.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return Err(AgentError::Protocol(format!(
            "{} did not negotiate ACP protocol version 1",
            spec.label
        )));
    }
    if let Some(verify) = spec.verify_initialized {
        verify(&initialized)?;
    }
    if spec.require_http_mcp
        && initialized
            .pointer("/agentCapabilities/mcpCapabilities/http")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(AgentError::Unavailable(format!(
            "the installed {} does not support HTTP MCP servers over ACP",
            spec.label
        )));
    }
    Ok(initialized)
}

impl AgentSession for GenericAcpSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if !self.done.load(Ordering::Acquire) {
            return Err(AgentError::Harness(format!(
                "{} is still processing the previous turn",
                self.spec.label
            )));
        }
        if let Some(cap) = self.requested.max_turns.map(|cap| cap.max(1))
            && self.turns >= cap
        {
            return Err(AgentError::Harness(format!(
                "Turn cap reached ({cap}); start a new {} session to continue.",
                self.spec.label
            )));
        }
        self.acquire_config_lease()?;
        if let Err(error) = apply_requested_configuration(
            &self.client,
            &self.session_id,
            &self.requested,
            &self.config_options,
            &self.spec,
            &self.overwritten,
        ) {
            self.restore_configuration();
            return Err(error);
        }

        let prompt = if self.turns == 0 {
            format!("{KINEWRIGHT_SYSTEM_PROMPT}\n\nUser request:\n{text}")
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

        let events = self.events_tx.clone();
        let done = Arc::clone(&self.done);
        let label = self.spec.label;
        let restore = self.turn_restore();
        thread::Builder::new()
            .name(format!("kinewright-acp-turn-{label}"))
            .spawn(move || {
                let reply = pending.wait(ACP_PROMPT_TIMEOUT);
                // Put a leased configuration back before anything else can
                // observe the turn as finished.
                restore.run();
                match reply {
                    Ok(result) => {
                        let stop_reason = result
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .unwrap_or("end_turn");
                        if !matches!(stop_reason, "end_turn" | "cancelled") {
                            let _ = events.send(AgentEvent::Error(format!(
                                "{label} stopped the turn: {stop_reason}"
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

    /// Stop is pressed on the egui frame thread, so nothing here may wait on
    /// the harness: cancelling, putting a leased configuration back and
    /// killing the child all move to a teardown worker.
    fn interrupt(&mut self) {
        let was_running = !self.done.load(Ordering::Acquire);
        if was_running {
            let _ = self
                .client
                .notify("session/cancel", &json!({"sessionId": self.session_id}));
            // Before `Done`, so the transcript reads in the order it happened.
            let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        }
        self.spawn_teardown(Vec::new());
        send_done(&self.events_tx, &self.done);
    }
}

impl Drop for GenericAcpSession {
    /// Closing a thread runs on the frame too, so the same teardown worker
    /// carries the restore, the kill and the directory removal — in that
    /// order, because the restore has to reach a live child and the
    /// directories are the child's working set.
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            let _ = self
                .client
                .notify("session/cancel", &json!({"sessionId": self.session_id}));
        }
        let mut paths = vec![std::mem::take(&mut self.scratch_directory)];
        paths.append(&mut self.cleanup_paths);
        self.spawn_teardown(paths);
    }
}

/// Everything the turn thread needs to hand a leased configuration back,
/// without borrowing the session it belongs to.
struct TurnRestore {
    client: AcpClient,
    session_id: String,
    overwritten: Arc<Mutex<OverwrittenConfig>>,
    has_lease: Arc<AtomicBool>,
    restore_lock: Arc<Mutex<()>>,
    lease: Option<&'static AtomicBool>,
}

impl TurnRestore {
    fn run(&self) {
        restore_configuration(
            &self.client,
            &self.session_id,
            &self.overwritten,
            &self.has_lease,
            &self.restore_lock,
            self.lease,
        );
    }
}

impl GenericAcpSession {
    fn turn_restore(&self) -> TurnRestore {
        TurnRestore {
            client: self.client.clone(),
            session_id: self.session_id.clone(),
            overwritten: Arc::clone(&self.overwritten),
            has_lease: Arc::clone(&self.turn_has_config_lease),
            restore_lock: Arc::clone(&self.restore_lock),
            lease: self.spec.config_lease,
        }
    }
}

/// Put back every value this session's turns overwrote. A no-op for every
/// harness whose configuration is session-scoped, and for a turn that never
/// took the lease.
///
/// The whole restore shares [`CONFIG_RESTORE_BUDGET`]: it can run while the
/// harness has stopped answering, and an unbounded restore there is a frozen
/// Stop button. Whatever is left unrestored when the budget runs out was
/// unreachable anyway.
fn restore_configuration(
    client: &AcpClient,
    session_id: &str,
    overwritten: &Mutex<OverwrittenConfig>,
    has_lease: &AtomicBool,
    restore_lock: &Mutex<()>,
    lease: Option<&'static AtomicBool>,
) {
    // The lock comes first, not the flag. Three threads can reach this — the
    // turn, the events loop on a fault, and the teardown worker — and each
    // one must see the restore *finished* before it reports the turn done.
    // Checking the flag first would let the loser return early and send
    // `Done` while the winner was still putting settings back.
    let _guard = restore_lock.lock().ok();
    if !has_lease.swap(false, Ordering::AcqRel) {
        return;
    }
    let owed = overwritten
        .lock()
        .map(|mut owed| std::mem::take(&mut *owed))
        .unwrap_or_default();
    let deadline = Instant::now() + CONFIG_RESTORE_BUDGET;
    for (id, value) in restore_writes(&owed) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let _ = set_config_option_within(client, session_id, &id, &value, remaining);
    }
    if let Some(lease) = lease {
        lease.store(false, Ordering::Release);
    }
}

/// Apply the requested model/effort/tier through `session/set_config_option`.
/// Only `OpenCode` and Cursor select their model this way (neither has a
/// model spawn flag); every other harness passes model and effort as spawn
/// arguments, so for them only an explicitly requested effort or tier that
/// the session advertises is applied, and anything unadvertised fails with a
/// clear error instead of silently running the wrong configuration.
fn apply_requested_configuration(
    client: &AcpClient,
    session_id: &str,
    requested: &SessionConfig,
    config_options: &[Value],
    spec: &AcpSessionSpec,
    overwritten: &Mutex<OverwrittenConfig>,
) -> Result<(), AgentError> {
    let mut state = config_options.to_vec();
    // Record what a write is about to displace *before* making it, so an
    // error part-way through still leaves a complete restore list.
    let record = |id: &str, state: &[Value]| {
        if spec.config_lease.is_some()
            && let Some(previous) = current_value(state, id)
            && let Ok(mut owed) = overwritten.lock()
            && !owed.iter().any(|(seen, _)| seen == id)
        {
            owed.push((id.to_owned(), previous));
        }
    };
    if spec.model_selection.applies_model()
        && let Some(model) = requested.model.as_deref()
    {
        if spec.model_selection.validates_model() {
            ensure_config_value(&state, "model", model, spec.label)?;
        }
        record("model", &state);
        let reply = set_config_option(
            client,
            session_id,
            "model",
            &Value::String(model.to_owned()),
        )?;
        if spec.model_selection.tracks_live_config() {
            state = config_options_of(&reply);
        }
    }
    if let Some(effort) = requested.effort.as_deref() {
        let Some(config_id) = ["effort", "reasoning"]
            .into_iter()
            .find(|id| config_option(&state, id).is_some())
        else {
            return Err(AgentError::Unavailable(format!(
                "the selected {} model does not expose an effort control",
                spec.label
            )));
        };
        ensure_config_value(&state, config_id, effort, spec.label)?;
        record(config_id, &state);
        let reply = set_config_option(
            client,
            session_id,
            config_id,
            &Value::String(effort.to_owned()),
        )?;
        if spec.model_selection.tracks_live_config() {
            state = config_options_of(&reply);
        }
    }
    if config_option(&state, "fast").is_some() {
        let fast = requested.service_tier.as_deref() == Some("true");
        let value = if config_uses_string_values(&state, "fast") {
            Value::String(fast.to_string())
        } else {
            Value::Bool(fast)
        };
        record("fast", &state);
        set_config_option(client, session_id, "fast", &value)?;
    } else if requested.service_tier.is_some() {
        return Err(AgentError::Unavailable(format!(
            "the selected {} model does not offer Fast mode",
            spec.label
        )));
    }
    Ok(())
}

fn config_options_of(result: &Value) -> Vec<Value> {
    result
        .get("configOptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn set_config_option(
    client: &AcpClient,
    session_id: &str,
    config_id: &str,
    value: &Value,
) -> Result<Value, AgentError> {
    set_config_option_within(client, session_id, config_id, value, ACP_REQUEST_TIMEOUT)
}

fn set_config_option_within(
    client: &AcpClient,
    session_id: &str,
    config_id: &str,
    value: &Value,
    timeout: Duration,
) -> Result<Value, AgentError> {
    client.request(
        "session/set_config_option",
        &json!({
            "sessionId": session_id,
            "configId": config_id,
            "value": value,
        }),
        timeout,
    )
}

fn config_option<'a>(config_options: &'a [Value], id: &str) -> Option<&'a Value> {
    config_options
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
}

fn config_uses_string_values(config_options: &[Value], id: &str) -> bool {
    config_option(config_options, id)
        .and_then(|option| option.get("options"))
        .and_then(Value::as_array)
        .and_then(|options| options.first())
        .and_then(|option| option.get("value"))
        .is_some_and(Value::is_string)
}

fn ensure_config_value(
    config_options: &[Value],
    id: &str,
    wanted: &str,
    label: &str,
) -> Result<(), AgentError> {
    let valid = config_option(config_options, id)
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
            "{label} does not offer {id}={wanted}"
        )))
    }
}

fn spawn_acp_incoming(
    client: AcpClient,
    incoming: Receiver<AcpIncoming>,
    session_id: String,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    label: &'static str,
    restore: TurnRestore,
) -> Result<(), AgentError> {
    thread::Builder::new()
        .name(format!("kinewright-acp-events-{label}"))
        .spawn(move || {
            let mut tool_names = HashMap::new();
            let mut finished_tools = HashSet::new();
            let mut reported_malformed = false;
            for message in incoming {
                match message {
                    AcpIncoming::Request { id, method, params }
                        if method == "session/request_permission" =>
                    {
                        answer_permission(&client, &id, &params);
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
                            translate_acp_update(
                                update,
                                &events,
                                &mut tool_names,
                                &mut finished_tools,
                                label,
                            );
                        }
                    }
                    AcpIncoming::Notification { .. } => {}
                    // One line the transport could not read is not a dead
                    // stream: skipping the loop here used to leave the
                    // session unable to translate any later update or answer
                    // any permission request, so the next turn ran blind for
                    // the full 30-minute prompt timeout. Report the first one
                    // and keep reading.
                    AcpIncoming::Malformed(message) => {
                        if !reported_malformed {
                            reported_malformed = true;
                            let _ = events.send(AgentEvent::Error(message));
                        }
                    }
                    AcpIncoming::Fault(message) => {
                        if !done.load(Ordering::Acquire) {
                            let _ = events.send(AgentEvent::Error(message));
                            // A leased configuration is put back before the
                            // turn is observable as finished, whichever
                            // thread gets here first.
                            restore.run();
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

/// Headless permission policy, mirroring Cursor: take the one-shot allow
/// option, else the always-allow option, else cancel the request so the turn
/// fails fast instead of hanging on an invisible prompt.
fn answer_permission(client: &AcpClient, id: &Value, params: &Value) {
    let option_id = params
        .get("options")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .find(|option| option.get("kind").and_then(Value::as_str) == Some("allow_once"))
                .or_else(|| {
                    options.iter().find(|option| {
                        option.get("kind").and_then(Value::as_str) == Some("allow_always")
                    })
                })
        })
        .and_then(|option| option.get("optionId"))
        .cloned();
    let outcome = option_id.map_or_else(
        || json!({"outcome": "cancelled"}),
        |option_id| json!({"outcome": "selected", "optionId": option_id}),
    );
    let _ = client.respond(id, &json!({"outcome": outcome}));
}

pub(crate) fn translate_acp_update(
    update: &Value,
    events: &Sender<AgentEvent>,
    tool_names: &mut HashMap<String, String>,
    finished_tools: &mut HashSet<String>,
    label: &str,
) {
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            if let Some(text) = acp_content_text(update.get("content"))
                && !text.is_empty()
            {
                let _ = events.send(AgentEvent::Text(text));
            }
        }
        Some("tool_call") => {
            let id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("acp-tool")
                .to_owned();
            let raw_name = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or(label);
            // Agents qualify MCP tools with the server name
            // (`kinewright_get_timeline_state`); Devin goes further and
            // narrates the call (`Calling get_timeline_state from
            // kinewright`). The UI shows the bare tool either way.
            let raw_name = raw_name.strip_prefix("kinewright_").unwrap_or(raw_name);
            let raw_name = devin_call_title(raw_name).unwrap_or(raw_name);
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
                .unwrap_or("acp-tool");
            let status = update.get("status").and_then(Value::as_str);
            if matches!(status, Some("completed" | "failed"))
                && finished_tools.insert(id.to_owned())
            {
                let name = tool_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| label.to_owned());
                let result = update
                    .get("rawOutput")
                    .filter(|value| !value.is_null())
                    .map(Value::to_string)
                    .or_else(|| acp_content_text(update.get("content")))
                    .unwrap_or_else(|| status.unwrap_or("completed").to_owned());
                let _ = events.send(AgentEvent::ToolResult { name, result });
            }
        }
        _ => {}
    }
}

/// Unwrap Devin's narrated tool titles (`Calling get_timeline_state from
/// kinewright`) to the bare tool name. Anything else passes through.
fn devin_call_title(title: &str) -> Option<&str> {
    let called = title.strip_prefix("Calling ")?;
    let (tool, _) = called.rsplit_once(" from ")?;
    (!tool.is_empty() && !tool.contains(' ')).then_some(tool)
}

fn acp_content_text(content: Option<&Value>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_tool_updates_translate_to_existing_agent_events() {
        let (tx, rx) = unbounded();
        let mut names = HashMap::new();
        let mut finished = HashSet::new();
        translate_acp_update(
            &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done"}}),
            &tx,
            &mut names,
            &mut finished,
            "OpenCode",
        );
        translate_acp_update(
            &json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"get_timeline_state","rawInput":{"include":"all"}}),
            &tx,
            &mut names,
            &mut finished,
            "OpenCode",
        );
        translate_acp_update(
            &json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"ok":true}}),
            &tx,
            &mut names,
            &mut finished,
            "OpenCode",
        );
        // A repeated terminal update for the same tool is reported once.
        translate_acp_update(
            &json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"ok":true}}),
            &tx,
            &mut names,
            &mut finished,
            "OpenCode",
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Text("Done".to_owned()));
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolCall { name, .. } if name == "get_timeline_state")
        );
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolResult { name, .. } if name == "get_timeline_state")
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn devin_narrated_titles_unwrap_to_bare_tools() {
        assert_eq!(
            devin_call_title("Calling get_timeline_state from kinewright"),
            Some("get_timeline_state")
        );
        assert_eq!(devin_call_title("Listed MCP tools for kinewright"), None);
        assert_eq!(devin_call_title("get_timeline_state"), None);
    }

    #[test]
    fn unknown_model_values_fail_closed_with_the_harness_name() {
        let options = vec![json!({
            "id": "model",
            "options": [{"value": "opencode-go/glm-5.3"}],
        })];
        assert!(ensure_config_value(&options, "model", "opencode-go/glm-5.3", "OpenCode").is_ok());
        assert_eq!(
            ensure_config_value(&options, "model", "nope", "OpenCode").unwrap_err(),
            AgentError::Unavailable("OpenCode does not offer model=nope".to_owned())
        );
    }

    /// Portable half of the restore contract (the fake-host drive below is
    /// unix-only): what a write displaces is read out of the ladder that
    /// write is about to change, and the model always goes back first.
    #[test]
    fn a_restore_reads_current_values_and_puts_the_model_back_first() {
        let ladder = config_options_of(&json!({
            "configOptions": [
                {"id": "model", "currentValue": "old-model", "options": [{"value": "old-model"}]},
                {"id": "fast", "currentValue": "true", "options": [{"value": "true"}]},
                {"id": "mode", "options": [{"value": "build"}]},
            ],
        }));
        assert_eq!(current_value(&ladder, "model"), Some(json!("old-model")));
        assert_eq!(current_value(&ladder, "fast"), Some(json!("true")));
        // An option with no `currentValue` offers nothing to put back.
        assert_eq!(current_value(&ladder, "mode"), None);
        assert_eq!(current_value(&ladder, "absent"), None);
        assert!(current_value(&config_options_of(&json!({})), "model").is_none());

        // Written fast-then-model; restored model-then-fast, because the
        // model decides which other options exist.
        let overwritten = vec![
            ("fast".to_owned(), json!("true")),
            ("effort".to_owned(), json!("high")),
            ("model".to_owned(), json!("old-model")),
        ];
        assert_eq!(
            restore_writes(&overwritten),
            [
                ("model".to_owned(), json!("old-model")),
                ("fast".to_owned(), json!("true")),
                ("effort".to_owned(), json!("high")),
            ]
        );
        assert!(restore_writes(&OverwrittenConfig::new()).is_empty());
    }

    /// A fake ACP agent that records every `session/set_config_option` it is
    /// given, answers `session/new` with `$KW_OPENING` and every config reply
    /// with `$KW_LADDER`. With `$KW_DEAF` it accepts the prompt and never
    /// answers it; with `$KW_MUTE` it stops reading altogether once the
    /// prompt arrives, so every later request waits out its own timeout —
    /// the harness Stop exists for. Unix only: it is a POSIX shell script,
    /// and `cmd.exe`
    /// has no equivalent one-liner that reads a JSON-RPC line and answers it.
    /// The portable half of the same contract is
    /// `a_restore_reads_current_values_and_puts_the_model_back_first`.
    #[cfg(unix)]
    const RECORDING_ACP_AGENT: &str = concat!(
        "while IFS= read -r line; do ",
        r#"id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); "#,
        "case \"$line\" in ",
        r#"*'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"mcpCapabilities":{"http":true}}}}\n' "$id" ;; "#,
        r#"*'"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s1","configOptions":%s}}\n' "$id" "$KW_OPENING" ;; "#,
        r#"*'"session/set_config_option"'*) printf '%s\n' "$line" | sed -n 's/.*"configId":"\([^"]*\)".*"value":\([^},]*\).*/\1=\2/p' >> "$KW_LOG"; "#,
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":%s}}\n' "$id" "$KW_LADDER" ;; "#,
        r#"*'"session/prompt"'*) if [ -n "$KW_HANGUP" ]; then exec 1>&- 2>&-; sleep 600; "#,
        r#"elif [ -n "$KW_MUTE" ]; then sleep 600; "#,
        r#"elif [ -z "$KW_DEAF" ]; then "#,
        r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"; fi ;; "#,
        "esac; done",
    );

    /// The lease is process-wide by design, so each test gets its own
    /// rather than racing the others through one static.
    #[cfg(unix)]
    static RESTORE_TEST_LEASE: AtomicBool = AtomicBool::new(false);
    #[cfg(unix)]
    static STOP_TEST_LEASE: AtomicBool = AtomicBool::new(false);
    #[cfg(unix)]
    static CONTENTION_TEST_LEASE: AtomicBool = AtomicBool::new(false);
    #[cfg(unix)]
    static MUTE_TEST_LEASE: AtomicBool = AtomicBool::new(false);
    #[cfg(unix)]
    static HANGUP_TEST_LEASE: AtomicBool = AtomicBool::new(false);

    /// How long a Stop may leave the configuration leased. Deliberately not
    /// derived from `CONFIG_RESTORE_BUDGET`: widening that budget has to
    /// fail this test rather than move the goalposts with it.
    #[cfg(unix)]
    const STOP_DEADLINE: Duration = Duration::from_secs(15);

    /// Real Cursor answers `session/new` with only `mode` and `model`; `fast`
    /// and `effort` are per-model and appear in the ladder a
    /// `set_config_option` reply restates. The fake host is shaped that way
    /// on purpose — the fixture that returned `fast` from `session/new` is
    /// what hid the defect this pins.
    #[cfg(unix)]
    /// How the fake host behaves once the prompt arrives.
    #[cfg(unix)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum AfterPrompt {
        /// Answers `session/prompt` and everything after it.
        Answer,
        /// Leaves the prompt unanswered but still serves config calls.
        Deaf,
        /// Stops reading: every later request waits out its timeout.
        Mute,
        /// Closes both output pipes and stops reading, so the transport
        /// reports the stream gone while every later request still waits out
        /// its timeout. (Both pipes, because the transport joins its stderr
        /// capture before it reports the fault.)
        HangUp,
    }

    #[cfg(unix)]
    fn recording_session(
        log_file: &Path,
        after_prompt: AfterPrompt,
        requested: SessionConfig,
        lease: &'static AtomicBool,
    ) -> GenericAcpSession {
        let opening = json!([
            {"id": "mode", "currentValue": "build", "options": [{"value": "build"}]},
            {"id": "model", "currentValue": "old-model",
             "options": [{"value": "old-model"}, {"value": "new-model"}]},
        ]);
        let ladder = json!([
            {"id": "mode", "currentValue": "build", "options": [{"value": "build"}]},
            {"id": "model", "currentValue": "new-model",
             "options": [{"value": "old-model"}, {"value": "new-model"}]},
            {"id": "effort", "currentValue": "high",
             "options": [{"value": "low"}, {"value": "high"}]},
            {"id": "fast", "currentValue": "true",
             "options": [{"value": "false"}, {"value": "true"}]},
        ]);
        let mut command = ProcessCommand::new("sh");
        command
            .arg("-c")
            .arg(RECORDING_ACP_AGENT)
            .env("KW_LOG", log_file)
            .env("KW_OPENING", opening.to_string())
            .env("KW_LADDER", ladder.to_string());
        match after_prompt {
            AfterPrompt::Answer => {}
            AfterPrompt::Deaf => {
                command.env("KW_DEAF", "1");
            }
            AfterPrompt::Mute => {
                command.env("KW_MUTE", "1");
            }
            AfterPrompt::HangUp => {
                command.env("KW_HANGUP", "1");
            }
        }
        GenericAcpSession::start(
            command,
            "http://127.0.0.1:43123/mcp",
            requested,
            AcpSessionSpec {
                label: "Fake",
                process_name: "Fake ACP",
                scratch_infix: "acp-lease",
                inline_mcp: true,
                require_http_mcp: true,
                model_selection: ModelSelection::LiveOption,
                config_lease: Some(lease),
                verify_initialized: None,
            },
        )
        .expect("the fake agent opens a session")
    }

    #[cfg(unix)]
    fn leased_turn_request() -> SessionConfig {
        SessionConfig {
            working_directory: None,
            model: Some("new-model".to_owned()),
            effort: Some("low".to_owned()),
            service_tier: None,
            max_turns: None,
            mcp_url: None,
            tool_names: None,
        }
    }

    /// Wait for the fake agent to have recorded `lines` config writes.
    #[cfg(unix)]
    fn recorded_writes(log_file: &Path, lines: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let recorded = fs::read_to_string(log_file).unwrap_or_default();
            let seen: Vec<String> = recorded.lines().map(str::to_owned).collect();
            if seen.len() >= lines || Instant::now() >= deadline {
                return seen;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Cursor's whole reason for a configuration lease: it persists
    /// `session/set_config_option` CLI-wide, so a turn that switches Fast off
    /// or pins an effort must hand the user's own settings back untouched.
    #[cfg(unix)]
    #[test]
    fn a_leasing_turn_restores_every_setting_it_overwrote() {
        let scratch = create_scratch_directory("acp-lease-test").unwrap();
        let log_file = scratch.join("set-config.log");
        let mut session = recording_session(
            &log_file,
            AfterPrompt::Answer,
            leased_turn_request(),
            &RESTORE_TEST_LEASE,
        );
        let events = session.events();
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");
        assert!(
            RESTORE_TEST_LEASE.load(Ordering::Acquire),
            "the turn holds the configuration lease"
        );
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(AgentEvent::Done) => break,
                Ok(AgentEvent::Error(error)) => panic!("the fake turn failed: {error}"),
                Ok(_) => {}
                Err(error) => panic!("the fake turn never finished: {error}"),
            }
        }
        assert!(
            !RESTORE_TEST_LEASE.load(Ordering::Acquire),
            "the lease is released with the turn"
        );
        assert_eq!(
            recorded_writes(&log_file, 6),
            [
                // What the turn asked for...
                "model=\"new-model\"",
                "effort=\"low\"",
                "fast=\"false\"",
                // ...and byte-identically what it displaced. `effort` and
                // `fast` are only ever in the per-model ladder, so restoring
                // the opening `session/new` options would miss both.
                "model=\"old-model\"",
                "effort=\"high\"",
                "fast=\"true\"",
            ]
        );
        drop(session);
        fs::remove_dir_all(scratch).unwrap();
    }

    /// Stop is pressed on the egui frame thread, and it is pressed precisely
    /// when the harness has stopped answering. `interrupt` must return at
    /// once and still put the configuration back.
    #[cfg(unix)]
    #[test]
    fn an_interrupted_leasing_turn_restores_without_blocking_the_caller() {
        let scratch = create_scratch_directory("acp-lease-stop").unwrap();
        let log_file = scratch.join("set-config.log");
        let mut session = recording_session(
            &log_file,
            AfterPrompt::Deaf,
            leased_turn_request(),
            &STOP_TEST_LEASE,
        );
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");
        assert_eq!(recorded_writes(&log_file, 3).len(), 3);

        let started = Instant::now();
        session.interrupt();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "interrupt blocked the frame for {elapsed:?}"
        );

        assert_eq!(
            recorded_writes(&log_file, 6)[3..],
            ["model=\"old-model\"", "effort=\"high\"", "fast=\"true\"",]
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while STOP_TEST_LEASE.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !STOP_TEST_LEASE.load(Ordering::Acquire),
            "an interrupted turn still releases the lease"
        );
        let started = Instant::now();
        drop(session);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "Drop blocked the frame"
        );
        fs::remove_dir_all(scratch).unwrap();
    }

    /// The lease is process-wide because the setting it protects is: two
    /// Kinewright turns configuring the same CLI would overwrite each
    /// other's restore. The second must be refused, by name, and must not
    /// take the lease the first is holding.
    #[cfg(unix)]
    #[test]
    fn a_second_leasing_turn_is_refused_while_the_first_holds_the_lease() {
        let scratch = create_scratch_directory("acp-lease-contention").unwrap();
        let log_file = scratch.join("set-config.log");
        let mut first = recording_session(
            &log_file,
            AfterPrompt::Deaf,
            leased_turn_request(),
            &CONTENTION_TEST_LEASE,
        );
        let mut second = recording_session(
            &scratch.join("second.log"),
            AfterPrompt::Deaf,
            leased_turn_request(),
            &CONTENTION_TEST_LEASE,
        );
        first
            .send_user_message("tighten the pauses".to_owned())
            .expect("the first turn is submitted");
        assert_eq!(
            second
                .send_user_message("and again".to_owned())
                .unwrap_err(),
            AgentError::Unavailable(
                "another Kinewright Fake turn is active; wait for it to finish or stop it"
                    .to_owned()
            )
        );
        assert!(CONTENTION_TEST_LEASE.load(Ordering::Acquire));

        first.interrupt();
        let deadline = Instant::now() + Duration::from_secs(10);
        while CONTENTION_TEST_LEASE.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        // Once the first turn hands the lease back, the second can take it.
        second
            .send_user_message("and again".to_owned())
            .expect("the lease is free again");
        drop(first);
        drop(second);
        fs::remove_dir_all(scratch).unwrap();
    }

    /// Stop is pressed because the harness has gone quiet, so the restore it
    /// triggers is exactly the case where every `set_config_option` waits out
    /// its own 30-second timeout. Neither `interrupt` nor `Drop` may carry
    /// that on the frame, and the lease must come back inside its budget
    /// rather than `n × 30 s` later.
    #[cfg(unix)]
    #[test]
    fn a_stop_against_a_mute_harness_returns_at_once_and_bounds_the_restore() {
        let scratch = create_scratch_directory("acp-lease-mute").unwrap();
        let log_file = scratch.join("set-config.log");
        let mut session = recording_session(
            &log_file,
            AfterPrompt::Mute,
            leased_turn_request(),
            &MUTE_TEST_LEASE,
        );
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");
        assert_eq!(recorded_writes(&log_file, 3).len(), 3);
        assert!(MUTE_TEST_LEASE.load(Ordering::Acquire));

        let started = Instant::now();
        session.interrupt();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "interrupt blocked the frame for {:?}",
            started.elapsed()
        );
        // The session is still alive here on purpose: dropping it would kill
        // the child, which would fail every pending restore request at once
        // and hide an unbounded budget. A literal deadline, not one derived
        // from `CONFIG_RESTORE_BUDGET`, so widening the budget fails here.
        let deadline = Instant::now() + STOP_DEADLINE;
        while MUTE_TEST_LEASE.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !MUTE_TEST_LEASE.load(Ordering::Acquire),
            "the lease was still held {STOP_DEADLINE:?} after Stop"
        );

        let started = Instant::now();
        drop(session);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "Drop blocked the frame for {:?}",
            started.elapsed()
        );
        fs::remove_dir_all(scratch).unwrap();
    }

    /// A harness that dies mid-turn is reported by the events loop, not by
    /// the turn thread, and that loop used to announce `Done` while the
    /// configuration was still leased — so the very next turn was refused
    /// with "another Kinewright turn is active" on a session that had just
    /// finished. Whoever sends `Done` must see the restore finished first.
    #[cfg(unix)]
    #[test]
    fn a_turn_that_dies_mid_flight_is_done_only_once_the_lease_is_back() {
        let scratch = create_scratch_directory("acp-lease-hangup").unwrap();
        let log_file = scratch.join("set-config.log");
        let mut session = recording_session(
            &log_file,
            AfterPrompt::HangUp,
            leased_turn_request(),
            &HANGUP_TEST_LEASE,
        );
        let events = session.events();
        session
            .send_user_message("tighten the pauses".to_owned())
            .expect("the turn is submitted");
        assert_eq!(recorded_writes(&log_file, 3).len(), 3);

        loop {
            match events.recv_timeout(Duration::from_secs(30)) {
                Ok(AgentEvent::Done) => break,
                Ok(_) => {}
                Err(error) => panic!("the broken turn never finished: {error}"),
            }
        }
        assert!(
            !HANGUP_TEST_LEASE.load(Ordering::Acquire),
            "`Done` was announced while the configuration was still leased"
        );
        drop(session);
        fs::remove_dir_all(scratch).unwrap();
    }
}
