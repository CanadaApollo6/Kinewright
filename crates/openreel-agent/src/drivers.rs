use std::{
    env,
    ffi::OsString,
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use openreel_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use serde_json::{Value, json};

use crate::{
    protocol::{ClaudeProtocol, CodexProtocol},
    schema::all_tool_names,
};

pub(crate) const OPENREEL_SYSTEM_PROMPT: &str = "You are OpenReel's video editing agent. Inspect the live timeline before editing, then use transcript, silence, and scene-change inspectors when relevant. Resolve ordinal references such as first, second, and last against that initial timeline state, and decide all target clip ids before mutation unless the user explicitly says otherwise. Prefer one atomic apply_edit_plan containing the complete ordered edit over separate operation tool calls. Link enforcement is orchestration, not core behavior: moving, trimming, or deleting one linked clip requires the same atomic plan to edit every member reported in its link group. Ripple edits always shift the edited track plus every other track reported as sync_lock=true; tracks with sync_lock=false stay fixed, while project markers at or after the ripple point always shift regardless of sync locks. A clip that starts before the ripple point is neither shifted nor trimmed. For ripple delete, the ripple point is the deleted clip's pre-edit end; when deleting a linked group, use regular delete_clip operations for companion members and exactly one ripple_delete_clip so the cross-track shift happens once. Titles are first-class video-track clips: use add_title to create one and set_title_param to change its declarative style, position, text, or frame-based fades. When asked to review footage, prefer placing markers as suggestions over changing the edit unless the user asks for an edit. Use only the OpenReel MCP tools. All edit time values are exact integer project frames; use the reported fps to convert seconds. After applying a plan, verify the result with the SAME inspectors that motivated it - if the user asked for dead air to be removed, re-run the silence inspector on the edited timeline and submit a follow-up plan when long silences remain. Every apply_edit_plan result ends with the count of cuttable silence spans remaining on the timeline: when the user asked for silence or dead-air removal, keep submitting follow-up plans until that count reports zero. Do not declare the task done until the inspectors confirm it. Then answer briefly.";
const MINIMUM_CODEX_VERSION: (u64, u64, u64) = (0, 147, 0);
const CODEX_DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "view_image",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "in_app_browser",
    "computer_use",
    "image_generation",
    "apps",
    "enable_mcp_apps",
    "plugins",
    "recommended_plugins",
    "multi_agent",
    "goals",
    "memories",
    "skill_search",
    "skill_mcp_dependency_install",
    "hooks",
    "tool_suggest",
    "remote_plugin",
    "plugin_sharing",
    "code_mode",
    "code_mode_only",
    "code_mode_host",
    "shell_snapshot",
    "workspace_dependencies",
    "auth_elicitation",
    "tool_call_mcp_elicitation",
    "default_mode_request_user_input",
    "request_permissions_tool",
];
static CODEX_SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const CODEX_SANDBOX_NOTICE: &str = "Codex sessions use a read-only empty scratch sandbox; shell, file-write, and web tools are disabled.";

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeDriver;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexSpawnTarget {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
}

impl CodexSpawnTarget {
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

impl AgentDriver for CodexDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("codex")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let target = find_codex_spawn_target()?;
        let version = codex_process_output(&target, &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        let authentication = codex_authentication(&target);
        Some(HarnessInfo {
            id: self.id(),
            executable: target.executable,
            version,
            authentication,
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let target = find_codex_spawn_target().ok_or(AgentError::NotInstalled)?;
        let version = codex_process_output(&target, &["--version"]).ok_or_else(|| {
            AgentError::Unavailable(
                "OpenReel could not verify the installed Codex CLI version; refusing to launch it"
                    .to_owned(),
            )
        })?;
        if !codex_version_is_supported(&version) {
            return Err(AgentError::Unavailable(format!(
                "Codex CLI 0.147.0 or newer is required for OpenReel's restricted driver (found {})",
                version.trim()
            )));
        }
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        CodexSession::new(target, endpoint, cfg).map(|session| Box::new(session) as _)
    }
}

impl AgentDriver for ClaudeCodeDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("claude-code")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        detect_cli("claude", self.id(), claude_authentication)
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("claude").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        ClaudeSession::spawn(executable, &endpoint, &cfg).map(|session| Box::new(session) as _)
    }
}

struct ClaudeSession {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    assistant_turns: Arc<AtomicU32>,
}

impl ClaudeSession {
    fn spawn(executable: PathBuf, endpoint: &str, cfg: &SessionConfig) -> Result<Self, AgentError> {
        let tool_allowlist = all_tool_names()
            .map_err(|error| AgentError::Harness(error.to_string()))?
            .into_iter()
            .map(|name| format!("mcp__openreel__{name}"))
            .collect::<Vec<_>>()
            .join(",");
        let mcp_config = json!({
            "mcpServers": {
                "openreel": {
                    "type": "http",
                    "url": endpoint,
                }
            }
        })
        .to_string();

        let mut command = ProcessCommand::new(executable);
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--strict-mcp-config",
            "--mcp-config",
            &mcp_config,
            "--tools",
            "",
            "--allowedTools",
            &tool_allowlist,
            "--permission-mode",
            "dontAsk",
            "--no-session-persistence",
            "--system-prompt",
            OPENREEL_SYSTEM_PROMPT,
        ]);
        if let Some(model) = &cfg.model {
            command.args(["--model", model]);
        }
        if let Some(effort) = &cfg.effort {
            if effort == crate::models::CLAUDE_ULTRACODE {
                // Ultracode is a session setting, not an --effort value:
                // xhigh effort plus standing dynamic-workflow orchestration.
                command.args(["--settings", r#"{"ultracode": true}"#]);
            } else {
                command.args(["--effort", effort]);
            }
        }
        if let Some(directory) = &cfg.working_directory {
            command.current_dir(directory);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);

        let mut child = command.spawn().map_err(|error| {
            AgentError::Harness(format!("could not start Claude Code: {error}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::Harness("Claude stdin was not available".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Harness("Claude stdout was not available".to_owned()))?;
        if let Some(stderr) = child.stderr.take() {
            thread::Builder::new()
                .name("openreel-claude-stderr".to_owned())
                .spawn(move || {
                    let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
                })
                .map_err(|error| AgentError::Harness(error.to_string()))?;
        }

        let child = Arc::new(Mutex::new(child));
        let stdin = Arc::new(Mutex::new(stdin));
        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(true));
        let assistant_turns = Arc::new(AtomicU32::new(0));
        spawn_claude_reader(
            stdout,
            Arc::clone(&child),
            events_tx.clone(),
            Arc::clone(&done),
            Arc::clone(&assistant_turns),
            cfg.max_turns.map(|cap| cap.max(1)),
        )?;

        Ok(Self {
            child,
            stdin,
            events_rx,
            events_tx,
            done,
            assistant_turns,
        })
    }

    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

struct CodexSession {
    target: CodexSpawnTarget,
    endpoint: String,
    model: Option<String>,
    effort: Option<String>,
    service_tier: Option<String>,
    max_turns: Option<u32>,
    turns: u32,
    prior_requests: Vec<String>,
    tool_names: Vec<String>,
    scratch_directory: PathBuf,
    model_catalog_directory: PathBuf,
    model_catalog_path: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
}

impl CodexSession {
    fn new(
        target: CodexSpawnTarget,
        endpoint: String,
        cfg: SessionConfig,
    ) -> Result<Self, AgentError> {
        let tool_names =
            all_tool_names().map_err(|error| AgentError::Harness(error.to_string()))?;
        let scratch_directory = create_codex_scratch_directory()?;
        let (model_catalog_directory, model_catalog_path) =
            match create_codex_direct_model_catalog(&target) {
                Ok(catalog) => catalog,
                Err(error) => {
                    let _ = fs::remove_dir_all(&scratch_directory);
                    return Err(error);
                }
            };
        let (events_tx, events_rx) = unbounded();
        Ok(Self {
            target,
            endpoint,
            model: cfg.model,
            effort: cfg.effort,
            service_tier: cfg.service_tier,
            max_turns: cfg.max_turns.map(|cap| cap.max(1)),
            turns: 0,
            prior_requests: Vec::new(),
            tool_names,
            scratch_directory,
            model_catalog_directory,
            model_catalog_path,
            child: Arc::new(Mutex::new(None)),
            events_rx,
            events_tx,
            done: Arc::new(AtomicBool::new(true)),
        })
    }

    fn kill_current(&self) {
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

impl AgentSession for CodexSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if let Some(cap) = self.max_turns
            && self.turns >= cap
        {
            return Err(AgentError::Harness(format!(
                "Turn cap reached ({cap}); start a new Codex session to continue."
            )));
        }
        {
            let mut child = self
                .child
                .lock()
                .map_err(|_| AgentError::Harness("Codex child lock was poisoned".to_owned()))?;
            if let Some(process) = child.as_mut()
                && process
                    .try_wait()
                    .map_err(|error| AgentError::Harness(error.to_string()))?
                    .is_none()
            {
                return Err(AgentError::Harness(
                    "Codex is still processing the previous turn".to_owned(),
                ));
            }
        }

        let prompt = codex_prompt(&self.prior_requests, &text);
        let mut command = build_codex_command(
            &self.target,
            &self.endpoint,
            self.model.as_deref(),
            self.effort.as_deref(),
            self.service_tier.as_deref(),
            &self.scratch_directory,
            &self.model_catalog_path,
            &self.tool_names,
            &prompt,
        );
        let mut process = command
            .spawn()
            .map_err(|error| AgentError::Harness(format!("could not start Codex: {error}")))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| AgentError::Harness("Codex stdout was not available".to_owned()))?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| AgentError::Harness("Codex stderr was not available".to_owned()))?;
        *self
            .child
            .lock()
            .map_err(|_| AgentError::Harness("Codex child lock was poisoned".to_owned()))? =
            Some(process);
        self.done.store(false, Ordering::Release);
        if let Err(error) = spawn_codex_reader(
            stdout,
            stderr,
            Arc::clone(&self.child),
            self.events_tx.clone(),
            Arc::clone(&self.done),
        ) {
            self.kill_current();
            return Err(error);
        }
        self.prior_requests.push(text);
        self.turns += 1;
        Ok(())
    }

    fn events(&self) -> Receiver<AgentEvent> {
        self.events_rx.clone()
    }

    fn interrupt(&mut self) {
        let was_running = !self.done.load(Ordering::Acquire);
        self.kill_current();
        if was_running {
            let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        }
        send_done(&self.events_tx, &self.done);
    }
}

fn codex_prompt(prior_requests: &[String], current: &str) -> String {
    if prior_requests.is_empty() {
        return format!("{OPENREEL_SYSTEM_PROMPT}\n\nUser request:\n{current}");
    }
    let context = prior_requests
        .iter()
        .enumerate()
        .map(|(index, request)| format!("{}. {request}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{OPENREEL_SYSTEM_PROMPT}\n\nEarlier user requests in this chat are below. The live timeline is authoritative; inspect it again before acting.\n{context}\n\nCurrent user request:\n{current}"
    )
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        self.kill_current();
        let _ = fs::remove_dir_all(&self.scratch_directory);
        let _ = fs::remove_dir_all(&self.model_catalog_directory);
    }
}

// One argument per independent launch input; a grouping struct would only
// exist for this call and its test.
#[allow(clippy::too_many_arguments)]
fn build_codex_command(
    target: &CodexSpawnTarget,
    endpoint: &str,
    model: Option<&str>,
    effort: Option<&str>,
    service_tier: Option<&str>,
    scratch_directory: &Path,
    model_catalog_path: &Path,
    tool_names: &[String],
    prompt: &str,
) -> ProcessCommand {
    let mut command = target.command();
    command
        .arg("exec")
        .arg("--ignore-user-config")
        .arg("--ignore-rules")
        .arg("--strict-config")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--sandbox")
        .arg("read-only");
    for feature in CODEX_DISABLED_FEATURES {
        command.arg("--disable").arg(feature);
    }
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    if let Some(effort) = effort {
        let effort = serde_json::to_string(effort).expect("serializing a string cannot fail");
        command
            .arg("-c")
            .arg(format!("model_reasoning_effort={effort}"));
    }
    if let Some(tier) = service_tier {
        let tier = serde_json::to_string(tier).expect("serializing a string cannot fail");
        command.arg("-c").arg(format!("service_tier={tier}"));
    }
    let endpoint = serde_json::to_string(endpoint).expect("serializing a string cannot fail");
    let model_catalog_path = serde_json::to_string(model_catalog_path)
        .expect("serializing a model catalog path cannot fail");
    let tool_names = serde_json::to_string(tool_names).expect("serializing tool names cannot fail");
    command
        .arg("-C")
        .arg(scratch_directory)
        .arg("-c")
        .arg("approval_policy='never'")
        .arg("-c")
        .arg("web_search='disabled'")
        .arg("-c")
        .arg("tools.update_plan.enabled=false")
        .arg("-c")
        .arg("agents.enabled=false")
        .arg("-c")
        .arg("analytics.enabled=false")
        .arg("-c")
        .arg("feedback.enabled=false")
        .arg("-c")
        .arg("history.persistence='none'")
        .arg("-c")
        .arg("shell_environment_policy.inherit='none'")
        .arg("-c")
        .arg("project_doc_max_bytes=0")
        .arg("-c")
        .arg(format!("model_catalog_json={model_catalog_path}"))
        .arg("-c")
        .arg(format!("mcp_servers.openreel.url={endpoint}"))
        .arg("-c")
        .arg(format!("mcp_servers.openreel.enabled_tools={tool_names}"))
        .arg("-c")
        .arg("mcp_servers.openreel.required=true")
        .arg("-c")
        .arg("mcp_servers.openreel.default_tools_approval_mode='approve'")
        .arg(prompt)
        .current_dir(scratch_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    command
}

fn spawn_codex_reader(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    child: Arc<Mutex<Option<Child>>>,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
) -> Result<(), AgentError> {
    let stderr_reader = thread::Builder::new()
        .name("openreel-codex-stderr".to_owned())
        .spawn(move || {
            let mut stderr_text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut stderr_text);
            stderr_text
        })
        .map_err(|error| AgentError::Harness(error.to_string()))?;
    thread::Builder::new()
        .name("openreel-codex-events".to_owned())
        .spawn(move || {
            let mut protocol = CodexProtocol::default();
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ =
                            events.send(AgentEvent::Error(format!("Codex stream error: {error}")));
                        break;
                    }
                };
                match protocol.parse_line(&line) {
                    Ok(parsed) => {
                        for event in parsed {
                            if event == AgentEvent::Done {
                                send_done(&events, &done);
                            } else {
                                let _ = events.send(event);
                            }
                        }
                    }
                    Err(error) => {
                        let _ = events.send(AgentEvent::Error(error.to_string()));
                    }
                }
            }
            let status = child
                .lock()
                .ok()
                .and_then(|mut child| child.as_mut().and_then(|child| child.wait().ok()));
            let stderr_text = stderr_reader.join().unwrap_or_default();
            if !done.load(Ordering::Acquire)
                && let Some(status) = status
                && !status.success()
            {
                let detail = stderr_text.trim();
                let message = if detail.is_empty() {
                    format!("Codex exited with {status}")
                } else {
                    format!("Codex exited with {status}: {detail}")
                };
                let _ = events.send(AgentEvent::Error(message));
            }
            send_done(&events, &done);
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

fn create_codex_scratch_directory() -> Result<PathBuf, AgentError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..16 {
        let counter = CODEX_SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "openreel-codex-{}-{now}-{counter}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(AgentError::Harness(format!(
                    "could not create the Codex scratch directory: {error}"
                )));
            }
        }
    }
    Err(AgentError::Harness(
        "could not allocate a unique Codex scratch directory".to_owned(),
    ))
}

fn create_codex_direct_model_catalog(
    target: &CodexSpawnTarget,
) -> Result<(PathBuf, PathBuf), AgentError> {
    let cached = codex_model_cache_path()
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|source| serde_json::from_str::<Value>(&source).ok());
    let bundled = || {
        codex_process_output(target, &["debug", "models", "--bundled"])
            .and_then(|source| serde_json::from_str::<Value>(&source).ok())
    };
    let mut catalog = cached.or_else(bundled).ok_or_else(|| {
        AgentError::Unavailable(
            "OpenReel could not load Codex model metadata to force direct MCP tool calling"
                .to_owned(),
        )
    })?;
    force_direct_tool_mode(&mut catalog)?;

    let directory = create_codex_scratch_directory()?;
    let path = directory.join("models-direct.json");
    let serialized = serde_json::to_vec(&catalog).map_err(|error| {
        AgentError::Harness(format!(
            "could not serialize the Codex model catalog: {error}"
        ))
    })?;
    if let Err(error) = fs::write(&path, serialized) {
        let _ = fs::remove_dir_all(&directory);
        return Err(AgentError::Harness(format!(
            "could not write the Codex direct-tool model catalog: {error}"
        )));
    }
    Ok((directory, path))
}

fn codex_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

pub(crate) fn codex_model_cache_path() -> Option<PathBuf> {
    codex_home().map(|home| home.join("models_cache.json"))
}

pub(crate) fn codex_config_path() -> Option<PathBuf> {
    codex_home().map(|home| home.join("config.toml"))
}

fn force_direct_tool_mode(catalog: &mut Value) -> Result<(), AgentError> {
    let models = if catalog.is_array() {
        catalog.as_array_mut()
    } else {
        catalog.get_mut("models").and_then(Value::as_array_mut)
    }
    .ok_or_else(|| {
        AgentError::Harness("Codex model catalog does not contain a models array".to_owned())
    })?;
    if models.is_empty() {
        return Err(AgentError::Harness(
            "Codex model catalog contains no models".to_owned(),
        ));
    }
    for model in models {
        let model = model.as_object_mut().ok_or_else(|| {
            AgentError::Harness("Codex model catalog contains a non-object model".to_owned())
        })?;
        model.insert("tool_mode".to_owned(), Value::String("direct".to_owned()));
        model.insert("supports_search_tool".to_owned(), Value::Bool(false));
    }
    Ok(())
}

impl AgentSession for ClaudeSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if let Ok(mut child) = self.child.lock()
            && child
                .try_wait()
                .map_err(|error| AgentError::Harness(error.to_string()))?
                .is_some()
        {
            return Err(AgentError::Harness(
                "Claude Code session has already exited".to_owned(),
            ));
        }
        self.assistant_turns.store(0, Ordering::Release);
        self.done.store(false, Ordering::Release);
        let message = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}],
            }
        });
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| AgentError::Harness("Claude stdin lock was poisoned".to_owned()))?;
        serde_json::to_writer(&mut *stdin, &message)
            .map_err(|error| AgentError::Protocol(error.to_string()))?;
        stdin
            .write_all(b"\n")
            .and_then(|()| stdin.flush())
            .map_err(|error| AgentError::Harness(format!("could not send to Claude: {error}")))
    }

    fn events(&self) -> Receiver<AgentEvent> {
        self.events_rx.clone()
    }

    fn interrupt(&mut self) {
        self.kill();
        let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        send_done(&self.events_tx, &self.done);
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn spawn_claude_reader(
    stdout: impl std::io::Read + Send + 'static,
    child: Arc<Mutex<Child>>,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
    assistant_turns: Arc<AtomicU32>,
    max_turns: Option<u32>,
) -> Result<(), AgentError> {
    thread::Builder::new()
        .name("openreel-claude-events".to_owned())
        .spawn(move || {
            let mut protocol = ClaudeProtocol::default();
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ =
                            events.send(AgentEvent::Error(format!("Claude stream error: {error}")));
                        break;
                    }
                };
                if let Some(cap) = max_turns
                    && is_claude_assistant_message(&line)
                    && assistant_turns.fetch_add(1, Ordering::AcqRel) + 1 > cap
                {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    let _ = events.send(AgentEvent::Error(format!(
                        "Turn cap reached ({cap}); Claude was stopped."
                    )));
                    send_done(&events, &done);
                    return;
                }
                match protocol.parse_line(&line) {
                    Ok(parsed) => {
                        for event in parsed {
                            if event == AgentEvent::Done {
                                send_done(&events, &done);
                            } else {
                                let _ = events.send(event);
                            }
                        }
                    }
                    Err(error) => {
                        let _ = events.send(AgentEvent::Error(error.to_string()));
                    }
                }
            }
            if !done.load(Ordering::Acquire) {
                let status = child.lock().ok().and_then(|mut child| child.wait().ok());
                if status.is_some_and(|status| !status.success()) {
                    let _ = events.send(AgentEvent::Error(format!(
                        "Claude Code exited with {}",
                        status.unwrap()
                    )));
                }
                send_done(&events, &done);
            }
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

fn is_claude_assistant_message(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
        .as_deref()
        == Some("assistant")
}

fn send_done(events: &Sender<AgentEvent>, done: &AtomicBool) {
    if !done.swap(true, Ordering::AcqRel) {
        let _ = events.send(AgentEvent::Done);
    }
}

fn detect_cli(
    executable_name: &str,
    id: HarnessId,
    authentication: fn(&Path) -> AuthenticationStatus,
) -> Option<HarnessInfo> {
    let executable = find_on_path(executable_name)?;
    let version = process_output(&executable, &["--version"])
        .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
    let authentication = authentication(&executable);
    Some(HarnessInfo {
        id,
        executable,
        version,
        authentication,
        subscription_tier: None,
    })
}

fn claude_authentication(executable: &Path) -> AuthenticationStatus {
    let Some(output) = process_output(executable, &["auth", "status"]) else {
        return AuthenticationStatus::Unknown;
    };
    serde_json::from_str::<Value>(&output)
        .ok()
        .and_then(|value| value.get("loggedIn").and_then(Value::as_bool))
        .map_or(AuthenticationStatus::Unknown, |logged_in| {
            if logged_in {
                AuthenticationStatus::Authenticated
            } else {
                AuthenticationStatus::Unauthenticated
            }
        })
}

fn codex_authentication(target: &CodexSpawnTarget) -> AuthenticationStatus {
    let Some(output) = codex_process_output(target, &["login", "status"]) else {
        return AuthenticationStatus::Unknown;
    };
    codex_authentication_status(&output)
}

fn codex_authentication_status(output: &str) -> AuthenticationStatus {
    let output = output.to_ascii_lowercase();
    if output.contains("not logged in") {
        AuthenticationStatus::Unauthenticated
    } else if output.contains("logged in") {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unknown
    }
}

fn codex_version_is_supported(output: &str) -> bool {
    output
        .split_whitespace()
        .find_map(|part| {
            let mut components = part.trim_start_matches('v').split(['.', '-']);
            Some((
                components.next()?.parse::<u64>().ok()?,
                components.next()?.parse::<u64>().ok()?,
                components.next()?.parse::<u64>().ok()?,
            ))
        })
        .is_some_and(|version| version >= MINIMUM_CODEX_VERSION)
}

fn find_codex_spawn_target() -> Option<CodexSpawnTarget> {
    let launcher = find_on_path("codex")?;
    resolve_codex_spawn_target(&launcher, || find_on_path("node"))
}

fn codex_windows_platform() -> (&'static str, &'static str) {
    match env::consts::ARCH {
        "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
        _ => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
    }
}

fn resolve_codex_spawn_target(
    launcher: &Path,
    find_node: impl FnOnce() -> Option<PathBuf>,
) -> Option<CodexSpawnTarget> {
    let is_script_shim = launcher.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy();
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("ps1")
    });
    if !is_script_shim {
        return Some(CodexSpawnTarget::native(launcher.to_owned()));
    }

    let npm_bin = launcher.parent()?;
    let codex_package = npm_bin.join("node_modules").join("@openai").join("codex");
    let (platform_package, target_triple) = codex_windows_platform();
    let platform_package_roots = [
        codex_package
            .join("node_modules")
            .join("@openai")
            .join(platform_package),
        npm_bin
            .join("node_modules")
            .join("@openai")
            .join(platform_package),
    ];
    for package_root in platform_package_roots {
        for binary_directory in ["bin", "codex"] {
            let native = package_root
                .join("vendor")
                .join(target_triple)
                .join(binary_directory)
                .join("codex.exe");
            if native.is_file() {
                return Some(CodexSpawnTarget::native(native));
            }
        }
    }
    for binary_directory in ["bin", "codex"] {
        let native = codex_package
            .join("vendor")
            .join(target_triple)
            .join(binary_directory)
            .join("codex.exe");
        if native.is_file() {
            return Some(CodexSpawnTarget::native(native));
        }
    }

    let javascript_entrypoint = codex_package.join("bin").join("codex.js");
    if !javascript_entrypoint.is_file() {
        return None;
    }
    Some(CodexSpawnTarget {
        executable: find_node()?,
        prefix_arguments: vec![javascript_entrypoint.into_os_string()],
    })
}

fn codex_process_output(target: &CodexSpawnTarget, arguments: &[&str]) -> Option<String> {
    let mut command = target.command();
    command.args(arguments).stdin(Stdio::null());
    hide_console_window(&mut command);
    process_command_output(command)
}

fn process_output(executable: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = ProcessCommand::new(executable);
    command.args(arguments).stdin(Stdio::null());
    hide_console_window(&mut command);
    process_command_output(command)
}

fn process_command_output(mut command: ProcessCommand) -> Option<String> {
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

pub(crate) fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };

    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let candidate = directory.join(format!("{executable}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
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
    fn driver_ids_match_the_public_harness_names() {
        assert_eq!(CodexDriver.id(), HarnessId::new("codex"));
        assert_eq!(ClaudeCodeDriver.id(), HarnessId::new("claude-code"));
    }

    #[test]
    fn codex_requires_the_policy_controls_added_by_0_147() {
        assert!(codex_version_is_supported("codex-cli 0.147.0"));
        assert!(codex_version_is_supported("codex-cli 0.148.1-beta.2"));
        assert!(!codex_version_is_supported("codex-cli 0.146.0"));
        assert!(!codex_version_is_supported("unexpected output"));
    }

    #[test]
    fn codex_authentication_status_does_not_misread_not_logged_in() {
        assert_eq!(
            codex_authentication_status("Logged in using ChatGPT"),
            AuthenticationStatus::Authenticated
        );
        assert_eq!(
            codex_authentication_status("Not logged in"),
            AuthenticationStatus::Unauthenticated
        );
        assert_eq!(
            codex_authentication_status("Status unavailable"),
            AuthenticationStatus::Unknown
        );
    }

    #[test]
    fn codex_command_is_read_only_and_has_an_exact_mcp_allowlist() {
        let scratch = Path::new("empty-codex-scratch");
        let model_catalog = Path::new("models-direct.json");
        let tools = vec!["get_timeline_state".to_owned(), "split_clip".to_owned()];
        let target = CodexSpawnTarget::native(PathBuf::from("codex"));
        let command = build_codex_command(
            &target,
            "http://127.0.0.1:43123/mcp",
            Some("gpt-test"),
            Some("xhigh"),
            Some("priority"),
            scratch,
            model_catalog,
            &tools,
            "test prompt",
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let joined = args.join(" ");

        assert!(joined.contains("exec --ignore-user-config --ignore-rules --strict-config"));
        assert!(joined.contains("--sandbox read-only"));
        assert!(joined.contains("--disable shell_tool"));
        assert!(joined.contains("--disable unified_exec"));
        assert!(joined.contains("--disable code_mode"));
        assert!(joined.contains("--disable code_mode_only"));
        assert!(joined.contains("--disable code_mode_host"));
        assert!(joined.contains("approval_policy='never'"));
        assert!(joined.contains("web_search='disabled'"));
        assert!(joined.contains("tools.update_plan.enabled=false"));
        assert!(joined.contains("project_doc_max_bytes=0"));
        assert!(joined.contains("--model gpt-test"));
        assert!(joined.contains("model_reasoning_effort=\"xhigh\""));
        assert!(joined.contains("service_tier=\"priority\""));
        assert!(joined.contains("model_catalog_json=\"models-direct.json\""));
        assert!(joined.contains("mcp_servers.openreel.required=true"));
        assert!(joined.contains(
            "mcp_servers.openreel.enabled_tools=[\"get_timeline_state\",\"split_clip\"]"
        ));
        assert_eq!(command.get_current_dir(), Some(scratch));
    }

    #[test]
    fn codex_model_catalog_forces_direct_non_deferred_tools() {
        let mut catalog = json!({
            "client_version": "0.147.0",
            "models": [
                {
                    "slug": "gpt-code-mode",
                    "tool_mode": "code_mode_only",
                    "supports_search_tool": true,
                    "use_responses_lite": true
                },
                {
                    "slug": "gpt-default",
                    "tool_mode": null,
                    "supports_search_tool": false
                }
            ]
        });

        force_direct_tool_mode(&mut catalog).unwrap();

        for model in catalog["models"].as_array().unwrap() {
            assert_eq!(model["tool_mode"], "direct");
            assert_eq!(model["supports_search_tool"], false);
        }
        assert_eq!(catalog["models"][0]["use_responses_lite"], true);
        assert_eq!(catalog["client_version"], "0.147.0");
    }

    #[test]
    fn codex_npm_shim_resolves_to_the_vendored_native_binary() {
        let npm_bin = create_codex_scratch_directory().unwrap();
        let shim = npm_bin.join("codex.cmd");
        fs::write(&shim, "@echo off\r\n").unwrap();
        let (platform_package, target_triple) = codex_windows_platform();
        let native = npm_bin
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe");
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&native, b"mock native codex").unwrap();

        let resolved = resolve_codex_spawn_target(&shim, || {
            panic!("node fallback must not be used when the native binary exists")
        })
        .unwrap();

        assert_eq!(resolved, CodexSpawnTarget::native(native));
        fs::remove_dir_all(npm_bin).unwrap();
    }

    #[test]
    fn codex_npm_shim_falls_back_to_node_and_the_javascript_entrypoint() {
        let npm_bin = create_codex_scratch_directory().unwrap();
        let shim = npm_bin.join("codex.ps1");
        fs::write(&shim, "#!/usr/bin/env pwsh\n").unwrap();
        let javascript_entrypoint = npm_bin
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        fs::create_dir_all(javascript_entrypoint.parent().unwrap()).unwrap();
        fs::write(&javascript_entrypoint, "#!/usr/bin/env node\n").unwrap();
        let node = npm_bin.join("node.exe");

        let resolved =
            resolve_codex_spawn_target(&shim, || Some(node.clone())).expect("node fallback");
        assert_eq!(resolved.executable, node);
        assert_eq!(
            resolved.prefix_arguments,
            vec![javascript_entrypoint.into_os_string()]
        );
        fs::remove_dir_all(npm_bin).unwrap();
    }

    #[test]
    fn codex_scratch_directories_are_unique_and_empty() {
        let first = create_codex_scratch_directory().unwrap();
        let second = create_codex_scratch_directory().unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_dir(&first).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&second).unwrap().count(), 0);
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn codex_replays_prior_user_requests_without_reusing_filesystem_state() {
        let prompt = codex_prompt(
            &[
                "split the first clip".to_owned(),
                "delete the second clip".to_owned(),
            ],
            "undo that deletion",
        );
        assert!(prompt.contains("1. split the first clip"));
        assert!(prompt.contains("2. delete the second clip"));
        assert!(prompt.contains("Current user request:\nundo that deletion"));
        assert!(prompt.contains("The live timeline is authoritative"));
        assert!(prompt.contains("use add_title to create one and set_title_param"));
    }
}
