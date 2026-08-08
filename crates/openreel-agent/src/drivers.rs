use std::{
    env,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use openreel_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use serde_json::{Value, json};

use crate::{
    protocol::ClaudeProtocol,
    schema::all_tool_names,
};

const CLAUDE_SYSTEM_PROMPT: &str = "You are OpenReel's video editing agent. Inspect the live timeline before editing. Resolve ordinal references such as first, second, and last against that initial timeline state, and decide all target clip ids before the first mutation unless the user explicitly says otherwise. Use only the OpenReel MCP tools. All edit time values are exact integer project frames; use the reported fps to convert seconds. Make the requested edits, verify the resulting timeline, then answer briefly.";

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeDriver;

impl AgentDriver for CodexDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("codex")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        detect_cli("codex", self.id(), codex_authentication)
    }

    fn start_session(&self, _cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        Err(AgentError::Unavailable(
            "Codex exec 0.142.5 cannot disable its built-in shell/file tools while retaining MCP tools, and non-interactive MCP approval remains unsafe. OpenReel will not launch an unrestricted video-editing session."
                .to_owned(),
        ))
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
        let endpoint = cfg
            .mcp_url
            .clone()
            .ok_or(AgentError::MissingMcpEndpoint)?;
        ClaudeSession::spawn(executable, endpoint, cfg).map(|session| Box::new(session) as _)
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
    fn spawn(
        executable: PathBuf,
        endpoint: String,
        cfg: SessionConfig,
    ) -> Result<Self, AgentError> {
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
            CLAUDE_SYSTEM_PROMPT,
        ]);
        if let Some(model) = &cfg.model {
            command.args(["--model", model]);
        }
        if let Some(directory) = &cfg.working_directory {
            command.current_dir(directory);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|error| AgentError::Harness(format!("could not start Claude Code: {error}")))?;
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
            cfg.max_turns.unwrap_or(8).max(1),
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
    max_turns: u32,
) -> Result<(), AgentError> {
    thread::Builder::new()
        .name("openreel-claude-events".to_owned())
        .spawn(move || {
            let mut protocol = ClaudeProtocol::default();
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = events.send(AgentEvent::Text(format!(
                            "Claude stream error: {error}"
                        )));
                        break;
                    }
                };
                if is_claude_assistant_message(&line)
                    && assistant_turns.fetch_add(1, Ordering::AcqRel) + 1 > max_turns
                {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    let _ = events.send(AgentEvent::Text(format!(
                        "Turn cap reached ({max_turns}); Claude was stopped."
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
                        let _ = events.send(AgentEvent::Text(error.to_string()));
                    }
                }
            }
            if !done.load(Ordering::Acquire) {
                let status = child.lock().ok().and_then(|mut child| child.wait().ok());
                if status.is_some_and(|status| !status.success()) {
                    let _ = events.send(AgentEvent::Text(format!(
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

fn codex_authentication(executable: &Path) -> AuthenticationStatus {
    let Some(output) = process_output(executable, &["login", "status"]) else {
        return AuthenticationStatus::Unknown;
    };
    if output.to_ascii_lowercase().contains("logged in") {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unauthenticated
    }
}

fn process_output(executable: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = ProcessCommand::new(executable);
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

fn find_on_path(executable: &str) -> Option<PathBuf> {
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
    fn codex_fails_closed_while_mcp_only_tool_restriction_is_unavailable() {
        let Err(error) = CodexDriver.start_session(SessionConfig::default()) else {
            panic!("Codex must not start an unrestricted session");
        };
        assert!(error.to_string().contains("cannot disable its built-in"));
    }
}
