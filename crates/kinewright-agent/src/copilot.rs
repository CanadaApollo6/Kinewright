//! GitHub Copilot CLI integration over headless JSONL.
//!
//! Each user turn starts one ephemeral `copilot -p` process in an empty
//! scratch directory. The Kinewright endpoint arrives through the
//! per-session `--additional-mcp-config` flag, `--available-tools` restricts
//! the model to that server, and `--output-format json` streams the turn back
//! as JSONL behind `AgentSession`. No Copilot configuration file is touched.

use std::{
    env, fs,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use kinewright_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use serde_json::json;

use crate::{
    child_process::{
        create_scratch_directory, hide_console_window, process_output, spawn_stderr_capture,
    },
    drivers::{KINEWRIGHT_SYSTEM_PROMPT, find_on_path},
    models::{ModelChoice, ServiceTier},
    protocol::CopilotProtocol,
};

/// Copilot's `--reasoning-effort` levels, from `copilot --help`.
const COPILOT_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The honest notice, and the weakest of the ten.
/// `--disable-builtin-mcps` really does leave Kinewright as the only MCP
/// server, but `--available-tools` takes tool names rather than a server
/// name (`copilot --help`: "Only these tools will be available to the
/// model"), so the bare `kinewright` Kinewright passes narrows nothing, and
/// the recorded fixture shows Copilot's own `bash` running. With
/// `--allow-all-tools` on top — Copilot cannot prompt headlessly — the
/// person is told what the session can actually do rather than what the
/// flags were meant to do.
pub const COPILOT_SANDBOX_NOTICE: &str = "Copilot sessions run from an empty scratch directory with Kinewright as their only MCP server, but Copilot's own shell and file tools stay available and run auto-approved, because Copilot cannot ask headlessly.";

#[derive(Debug, Default, Clone, Copy)]
pub struct CopilotDriver;

impl AgentDriver for CopilotDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("copilot")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("copilot")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication: copilot_authentication(),
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        if let Some(effort) = cfg.effort.as_deref()
            && !COPILOT_EFFORTS.contains(&effort)
        {
            return Err(AgentError::Unavailable(format!(
                "unknown Copilot reasoning effort {effort:?}; expected one of {}",
                COPILOT_EFFORTS.join(", "),
            )));
        }
        let executable = find_on_path("copilot").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        CopilotSession::new(executable, endpoint, cfg).map(|session| Box::new(session) as _)
    }
}

/// Copilot keeps its token in the system credential store, which has no cheap
/// local probe; an explicit headless token is the only positive signal, and
/// anything else is honestly reported as unknown.
fn copilot_authentication() -> AuthenticationStatus {
    if env::var_os("COPILOT_GITHUB_TOKEN").is_some_and(|token| !token.is_empty()) {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unknown
    }
}

/// The models GitHub lists for the Copilot CLI surface, by `--model` slug.
/// Copilot ships no models-list command, so this list is curated from its
/// supported-models documentation; ids follow its hyphenated slug convention
/// (`GPT-5.6 Sol` runs as `gpt-5.6-sol`).
#[must_use]
pub fn copilot_models() -> Vec<ModelChoice> {
    const MODELS: [(&str, &str); 26] = [
        ("gpt-5.6-sol", "GPT-5.6 Sol"),
        ("gpt-5.6-luna", "GPT-5.6 Luna"),
        ("gpt-5.6-terra", "GPT-5.6 Terra"),
        ("gpt-6-astra", "GPT-6 Astra"),
        ("gpt-5.5", "GPT-5.5"),
        ("gpt-5.4", "GPT-5.4"),
        ("gpt-5.4-mini", "GPT-5.4 mini"),
        ("gpt-5.3-codex", "GPT-5.3-Codex"),
        ("gpt-5-mini", "GPT-5 mini"),
        ("claude-opus-5", "Claude Opus 5"),
        ("claude-opus-4-8", "Claude Opus 4.8"),
        ("claude-opus-4-7", "Claude Opus 4.7"),
        ("claude-sonnet-5", "Claude Sonnet 5"),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
        ("claude-fable-5-1", "Claude Fable 5.1"),
        ("claude-fable-5", "Claude Fable 5"),
        ("claude-haiku-4-5", "Claude Haiku 4.5"),
        ("gemini-3-8-flash", "Gemini 3.8 Flash"),
        ("gemini-3-7-flash", "Gemini 3.7 Flash"),
        ("gemini-3-6-flash", "Gemini 3.6 Flash"),
        ("gemini-3-5-flash", "Gemini 3.5 Flash"),
        ("grok-4-6", "Grok 4.6"),
        ("grok-4-5", "Grok 4.5"),
        ("kimi-k3", "Kimi K3"),
        ("kimi-k2-7-code", "Kimi K2.7 Code"),
        ("mai-code-1-1-flash", "MAI-Code-1.1-Flash"),
    ];
    MODELS
        .iter()
        .map(|(id, label)| ModelChoice {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
            efforts: COPILOT_EFFORTS
                .iter()
                .map(|effort| (*effort).to_owned())
                .collect(),
            tiers: Vec::<ServiceTier>::new(),
        })
        .collect()
}

struct CopilotSession {
    executable: PathBuf,
    endpoint: String,
    model: Option<String>,
    effort: Option<String>,
    max_turns: Option<u32>,
    turns: u32,
    prior_requests: Vec<String>,
    scratch_directory: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    events_rx: Receiver<AgentEvent>,
    events_tx: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
}

impl CopilotSession {
    fn new(executable: PathBuf, endpoint: String, cfg: SessionConfig) -> Result<Self, AgentError> {
        let scratch_directory = create_scratch_directory("copilot")?;
        let (events_tx, events_rx) = unbounded();
        Ok(Self {
            executable,
            endpoint,
            model: cfg.model,
            effort: cfg.effort,
            max_turns: cfg.max_turns.map(|cap| cap.max(1)),
            turns: 0,
            prior_requests: Vec::new(),
            scratch_directory,
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

impl AgentSession for CopilotSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Protocol("user message is empty".to_owned()));
        }
        if let Some(cap) = self.max_turns
            && self.turns >= cap
        {
            return Err(AgentError::Harness(format!(
                "Turn cap reached ({cap}); start a new Copilot session to continue."
            )));
        }
        {
            let mut child = self
                .child
                .lock()
                .map_err(|_| AgentError::Harness("Copilot child lock was poisoned".to_owned()))?;
            if let Some(process) = child.as_mut()
                && process
                    .try_wait()
                    .map_err(|error| AgentError::Harness(error.to_string()))?
                    .is_none()
            {
                return Err(AgentError::Harness(
                    "Copilot is still processing the previous turn".to_owned(),
                ));
            }
        }

        let prompt = copilot_prompt(&self.prior_requests, &text);
        let mut command = build_copilot_command(
            &self.executable,
            &self.endpoint,
            self.model.as_deref(),
            self.effort.as_deref(),
            &self.scratch_directory,
            &prompt,
        );
        let mut process = command
            .spawn()
            .map_err(|error| AgentError::Harness(format!("could not start Copilot: {error}")))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| AgentError::Harness("Copilot stdout was not available".to_owned()))?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| AgentError::Harness("Copilot stderr was not available".to_owned()))?;
        *self
            .child
            .lock()
            .map_err(|_| AgentError::Harness("Copilot child lock was poisoned".to_owned()))? =
            Some(process);
        self.done.store(false, Ordering::Release);
        if let Err(error) = spawn_copilot_reader(
            stdout,
            stderr,
            Arc::clone(&self.child),
            self.events_tx.clone(),
            Arc::clone(&self.done),
        ) {
            self.kill_current();
            // `done` was cleared above; without this the session stays
            // "running" for ever, so the next `Done` is swallowed and
            // `interrupt` reports a turn that never started.
            self.done.store(true, Ordering::Release);
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
        if was_running {
            // Before the kill, so it lands ahead of the `Done` the reader
            // emits when stdout closes.
            let _ = self.events_tx.send(AgentEvent::Text("Stopped.".to_owned()));
        }
        self.kill_current();
        send_done(&self.events_tx, &self.done);
    }
}

impl Drop for CopilotSession {
    fn drop(&mut self) {
        self.kill_current();
        let _ = fs::remove_dir_all(&self.scratch_directory);
    }
}

fn copilot_prompt(prior_requests: &[String], current: &str) -> String {
    if prior_requests.is_empty() {
        return format!("{KINEWRIGHT_SYSTEM_PROMPT}\n\nUser request:\n{current}");
    }
    let context = prior_requests
        .iter()
        .enumerate()
        .map(|(index, request)| format!("{}. {request}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{KINEWRIGHT_SYSTEM_PROMPT}\n\nEarlier user requests in this chat are below. The live timeline is authoritative; inspect it again before acting.\n{context}\n\nCurrent user request:\n{current}"
    )
}

fn build_copilot_command(
    executable: &Path,
    endpoint: &str,
    model: Option<&str>,
    effort: Option<&str>,
    scratch_directory: &Path,
    prompt: &str,
) -> ProcessCommand {
    let mcp_config = json!({
        "mcpServers": {
            "kinewright": {
                "transport": "http",
                "url": endpoint,
            },
        },
    })
    .to_string();
    let mut command = ProcessCommand::new(executable);
    command
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--silent")
        .arg("--allow-all-tools")
        .arg("--disable-builtin-mcps")
        .arg("--additional-mcp-config")
        .arg(mcp_config)
        .arg("-C")
        .arg(scratch_directory);
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    if let Some(effort) = effort {
        command.arg("--reasoning-effort").arg(effort);
    }
    // `--available-tools` is variadic, so it goes last and nothing after it
    // can be swallowed as another tool name.
    command.arg("--available-tools").arg("kinewright");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    command
}

/// Read one `copilot -p` run. Its stderr is drained on a second thread and
/// kept for the failure message: an undrained pipe blocks the child once it
/// fills, which would hang the turn instead of ending it.
fn spawn_copilot_reader(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    child: Arc<Mutex<Option<Child>>>,
    events: Sender<AgentEvent>,
    done: Arc<AtomicBool>,
) -> Result<(), AgentError> {
    let stderr_reader = spawn_stderr_capture(stderr, "kinewright-copilot-stderr")?;
    thread::Builder::new()
        .name("kinewright-copilot-events".to_owned())
        .spawn(move || {
            let mut protocol = CopilotProtocol::default();
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = events
                            .send(AgentEvent::Error(format!("Copilot stream error: {error}")));
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
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
            // Take the child out from under the lock before waiting on it:
            // `interrupt` needs the same mutex from the frame thread, and a
            // `copilot -p` run that closes stdout while it flushes its
            // session store would otherwise hold Stop open indefinitely.
            let owned = child.lock().ok().and_then(|mut child| child.take());
            let status = owned.and_then(|mut child| child.wait().ok());
            let stderr_text = stderr_reader.join().unwrap_or_default();
            if !done.load(Ordering::Acquire) {
                if let Some(status) = status
                    && !status.success()
                {
                    let detail = stderr_text.trim();
                    let message = if detail.is_empty() {
                        format!("Copilot exited with {status}")
                    } else {
                        format!("Copilot exited with {status}: {detail}")
                    };
                    let _ = events.send(AgentEvent::Error(message));
                }
                send_done(&events, &done);
            }
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

fn send_done(events: &Sender<AgentEvent>, done: &AtomicBool) {
    if !done.swap(true, Ordering::AcqRel) {
        let _ = events.send(AgentEvent::Done);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Read},
        sync::atomic::AtomicUsize,
        time::{Duration, Instant},
    };

    use super::*;

    /// More than any platform's pipe buffer (64 KiB on Linux, 4 KiB on the
    /// Windows default), so an undrained stderr certainly blocks the child.
    const FLOOD_BYTES: usize = 256 * 1024;
    const TURN_DEADLINE: Duration = Duration::from_secs(30);
    /// How long the fake stdout pipe stays blocked before it gives up, so a
    /// regression fails quickly instead of sitting on the turn deadline.
    const DRAIN_DEADLINE: Duration = Duration::from_secs(5);

    #[test]
    fn driver_id_matches_the_public_harness_name() {
        assert_eq!(CopilotDriver.id(), HarnessId::new("copilot"));
    }

    #[test]
    fn curated_models_use_documented_slugs_with_full_efforts() {
        let models = copilot_models();
        assert_eq!(models.len(), 26);
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        for expected in [
            "gpt-5.6-sol",
            "claude-haiku-4-5",
            "gpt-5.3-codex",
            "mai-code-1-1-flash",
        ] {
            assert!(ids.contains(&expected), "missing {expected}");
        }
        let efforts: Vec<_> = COPILOT_EFFORTS
            .iter()
            .map(|effort| (*effort).to_owned())
            .collect();
        assert!(models.iter().all(|model| model.efforts == efforts));
        assert!(models.iter().all(|model| model.tiers.is_empty()));
    }

    #[test]
    fn copilot_command_is_headless_with_an_exact_mcp_allowlist() {
        let scratch = Path::new("empty-copilot-scratch");
        let command = build_copilot_command(
            Path::new("copilot"),
            "http://127.0.0.1:43123/mcp",
            Some("gpt-5.6-sol"),
            Some("xhigh"),
            scratch,
            "test prompt",
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let joined = args.join(" ");
        assert!(joined.contains("-p test prompt"));
        assert!(joined.contains("--output-format json"));
        assert!(joined.contains("--disable-builtin-mcps"));
        assert!(joined.contains("--available-tools kinewright"));
        assert!(joined.contains("http://127.0.0.1:43123/mcp"));
        assert!(joined.contains("--model gpt-5.6-sol"));
        assert!(joined.contains("--reasoning-effort xhigh"));
        // The two safety-relevant flags the docs and the notice are about,
        // and the empty working directory they are justified against. Each
        // could be deleted with every other assertion still green.
        assert!(joined.contains("--allow-all-tools"), "{joined}");
        assert!(joined.contains("--silent"), "{joined}");
        assert!(
            joined.contains("-C empty-copilot-scratch"),
            "the run must start in the scratch directory: {joined}"
        );
        // `--available-tools` takes a list, so anything after it would be
        // read as another tool name.
        assert!(joined.ends_with("--available-tools kinewright"), "{joined}");
    }

    /// A stderr pipe carrying `FLOOD_BYTES` before it closes.
    struct FloodingStderr {
        remaining: Arc<AtomicUsize>,
    }

    impl Read for FloodingStderr {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let left = self.remaining.load(Ordering::Acquire);
            if left == 0 {
                return Ok(0);
            }
            let take = buf.len().min(left);
            buf[..take].fill(b'x');
            self.remaining.fetch_sub(take, Ordering::AcqRel);
            Ok(take)
        }
    }

    /// A stdout pipe that yields nothing until the stderr flood has been
    /// consumed: a real child cannot write its answer while it is blocked
    /// writing diagnostics, so the two streams are coupled exactly this way.
    struct StdoutAfterFlood {
        flood: Arc<AtomicUsize>,
        payload: io::Cursor<Vec<u8>>,
    }

    impl Read for StdoutAfterFlood {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let deadline = Instant::now() + DRAIN_DEADLINE;
            while self.flood.load(Ordering::Acquire) > 0 {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("stderr was never drained"));
                }
                thread::sleep(Duration::from_millis(1));
            }
            self.payload.read(buf)
        }
    }

    fn drain_turn(events: &Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut seen = Vec::new();
        loop {
            match events.recv_timeout(TURN_DEADLINE) {
                Ok(event) => {
                    let finished = event == AgentEvent::Done;
                    seen.push(event);
                    if finished {
                        return seen;
                    }
                }
                Err(error) => {
                    panic!("the turn never finished ({error}): Copilot's stderr was not drained")
                }
            }
        }
    }

    /// The regression this guards: `build_copilot_command` pipes stderr, so
    /// a turn that never reads it wedges once the child fills the pipe.
    #[test]
    fn a_flooded_stderr_pipe_does_not_block_the_turn() {
        let flood = Arc::new(AtomicUsize::new(FLOOD_BYTES));
        let stdout = StdoutAfterFlood {
            flood: Arc::clone(&flood),
            payload: io::Cursor::new(
                concat!(
                    r#"{"type":"assistant.message","data":{"content":"flooded","toolRequests":[]}}"#,
                    "\n",
                    r#"{"type":"result","exitCode":0}"#,
                    "\n",
                )
                .as_bytes()
                .to_vec(),
            ),
        };
        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(false));
        spawn_copilot_reader(
            stdout,
            FloodingStderr {
                remaining: Arc::clone(&flood),
            },
            Arc::new(Mutex::new(None)),
            events_tx,
            Arc::clone(&done),
        )
        .unwrap();
        let events = drain_turn(&events_rx);
        assert!(
            events.contains(&AgentEvent::Text("flooded".to_owned())),
            "{events:?}"
        );
        assert_eq!(flood.load(Ordering::Acquire), 0, "stderr was left unread");
    }

    /// The same property against a real OS pipe. Unix only: there is no
    /// `cmd.exe` one-liner that emits 256 KiB without tripping its command
    /// and environment-variable length limits, and the fake-pipe test above
    /// already covers the Windows lane.
    #[cfg(unix)]
    #[test]
    fn a_real_child_flooding_stderr_still_finishes_its_turn() {
        // Doubling keeps this one `printf`: 16 bytes shifted left 14 times.
        let script = concat!(
            "s=0123456789abcdef; i=0; ",
            "while [ $i -lt 14 ]; do s=\"$s$s\"; i=$((i+1)); done; ",
            "printf '%s\\n' \"$s\" >&2; ",
            r#"printf '%s\n' '{"type":"assistant.message","data":{"content":"flooded","toolRequests":[]}}'; "#,
            r#"printf '%s\n' '{"type":"result","exitCode":0}'"#,
        );
        let mut child = ProcessCommand::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a POSIX shell is available on this platform");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let (events_tx, events_rx) = unbounded();
        let done = Arc::new(AtomicBool::new(false));
        spawn_copilot_reader(
            stdout,
            stderr,
            Arc::new(Mutex::new(Some(child))),
            events_tx,
            done,
        )
        .unwrap();
        let events = drain_turn(&events_rx);
        assert!(
            events.contains(&AgentEvent::Text("flooded".to_owned())),
            "{events:?}"
        );
    }

    /// Stop needs the child mutex from the egui frame thread. The reader
    /// used to hold it across `wait()`, so a `copilot -p` run that closed
    /// stdout and kept going — flushing its session store, waiting on an MCP
    /// server — froze the button for as long as the child lived.
    #[cfg(unix)]
    #[test]
    fn the_reader_never_holds_the_child_lock_while_waiting_on_it() {
        let mut child = ProcessCommand::new("sh")
            .arg("-c")
            .arg("exec 1>&- 2>&-; sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a POSIX shell is available on this platform");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let handle = Arc::new(Mutex::new(Some(child)));
        let (events_tx, _events_rx) = unbounded();
        spawn_copilot_reader(
            stdout,
            stderr,
            Arc::clone(&handle),
            events_tx,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        // Both pipes are at EOF immediately, so give the reader time to
        // reach its wait before probing — otherwise the probe wins the race
        // and the lock looks free whatever the reader does with it. The
        // child lives for five seconds, so anything that answers well
        // inside that proves the wait is not holding the lock.
        thread::sleep(Duration::from_millis(700));
        let deadline = Instant::now() + Duration::from_millis(800);
        let mut free = false;
        while Instant::now() < deadline {
            if handle.try_lock().is_ok() {
                free = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            free,
            "the reader held the child lock while waiting on the child"
        );
        if let Ok(mut child) = handle.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
    }
}
