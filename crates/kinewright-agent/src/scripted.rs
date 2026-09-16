//! A scripted [`AgentDriver`] that makes **real** MCP tool calls (IN2 §8).
//!
//! Production code, outside `#[cfg(feature = "eval-harness")]` and outside
//! `#[cfg(test)]`, because it is the application's own test substrate as well
//! as this crate's: `kinewright-app` takes `kinewright-agent` with
//! `default-features = false`, so a fake behind the eval feature is a fake the
//! app's tests cannot reach (IN2 §8 rules 1–2).
//!
//! It is the fourth `AgentDriver` beside `ClaudeCodeDriver`, `CodexDriver` and
//! `CursorAcpDriver`, and the only one that needs no model, no authentication
//! and no network beyond loopback.
//!
//! **Why it holds its own MCP client.** `AgentSession` is
//! `send_user_message` / `events` / `interrupt` and nothing else
//! (`kinewright_core::agent`), so a driver that *makes* tool calls has to be
//! its own MCP client. This one is `rmcp`'s streamable-HTTP client on an owned
//! current-thread Tokio runtime, driven from an ordinary `std::thread` worker
//! so `send_user_message` stays non-blocking. It costs **zero** new
//! dependencies: `rmcp` is already a non-optional workspace dependency with
//! `transport-streamable-http-client-reqwest` on (IN2 §8 rule 3).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use kinewright_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
use rmcp::{
    ServiceExt as _,
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
    transport::StreamableHttpClientTransport,
};

/// The harness id every [`ScriptedDriver`] reports.
pub const SCRIPTED_HARNESS_ID: &str = "scripted";

/// One tool call the script makes, by name, through the session's own MCP
/// client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedCall {
    /// The served tool name, exactly as the harness would send it.
    pub tool: String,
    /// The call's arguments. A non-object value is sent as no arguments, which
    /// is what an empty-argument tool call looks like on the wire.
    pub arguments: serde_json::Value,
}

/// The six fields of `AgentEvent::Cost`.
///
/// Six and not two, because `mirror_agent_cost` passes the event's four
/// `Option` fields straight through: a two-field cost would leave four of the
/// six telemetry mirrors `None` for every scripted turn and make IN2 §9
/// clause 7 unsatisfiable by any test this contract specifies (IN2 §8 rule 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedCost {
    /// Provider input tokens.
    pub input_tokens: u64,
    /// Provider output tokens.
    pub output_tokens: u64,
    /// Input tokens served from a provider prompt cache.
    pub cached_input_tokens: Option<u64>,
    /// Input tokens written to a provider prompt cache.
    pub cache_creation_input_tokens: Option<u64>,
    /// Reasoning tokens included in `output_tokens`.
    pub reasoning_output_tokens: Option<u64>,
    /// Reported dollar cost.
    pub cost_usd: Option<f64>,
}

/// One turn of a script: the calls it makes, what it says, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptedTurn {
    /// Tool calls made in order, each through the session's own MCP client.
    pub calls: Vec<ScriptedCall>,
    /// The assistant's final message for this turn.
    pub message: String,
    /// The cost event this turn reports, or `None` for a harness that reports
    /// none (the Cursor shape).
    pub cost: Option<ScriptedCost>,
    /// How long the turn takes before it finishes.
    ///
    /// **Erratum D-R61.** IN2 §8 rule 4 spells three fields, and §8 script
    /// **S5** asks for *"one turn whose script sleeps past the bound"* — which
    /// a three-field turn cannot express. A wall-time budget proved by setting
    /// the bound to a nanosecond is a test of `Instant::elapsed`, not of the
    /// budget; this field makes S5 the sleep the contract describes.
    /// Defaults to zero, so every other script is unchanged.
    pub pause: Duration,
}

impl ScriptedTurn {
    /// A turn that makes `calls`, says `message`, reports nothing and sleeps
    /// for no time.
    #[must_use]
    pub fn new(calls: Vec<ScriptedCall>, message: impl Into<String>) -> Self {
        Self {
            calls,
            message: message.into(),
            cost: None,
            pause: Duration::ZERO,
        }
    }

    /// The same turn, reporting `cost`.
    #[must_use]
    pub fn with_cost(mut self, cost: ScriptedCost) -> Self {
        self.cost = Some(cost);
        self
    }

    /// The same turn, taking `pause` before it finishes (script S5).
    #[must_use]
    pub const fn with_pause(mut self, pause: Duration) -> Self {
        self.pause = pause;
        self
    }
}

/// A driver that replays a script of turns against a live `McpServer`.
///
/// The script is shared with the session it starts, so a test can read what is
/// left of it after the pump has stopped.
#[derive(Debug, Clone)]
pub struct ScriptedDriver {
    turns: Arc<Mutex<VecDeque<ScriptedTurn>>>,
    authentication: AuthenticationStatus,
}

impl ScriptedDriver {
    /// A driver whose `detect()` reports [`AuthenticationStatus::Unknown`] —
    /// the default two of the three production detectors return, and the one
    /// IN2 §2.2 rule 7 treats as available.
    #[must_use]
    pub fn new(turns: Vec<ScriptedTurn>) -> Self {
        Self {
            turns: Arc::new(Mutex::new(turns.into())),
            authentication: AuthenticationStatus::Unknown,
        }
    }

    /// The same driver, reporting `authentication` from `detect()`.
    ///
    /// Configurable so §2.2 rule 7's `Unknown`-is-available rule and rule 8's
    /// one-strike disable are both testable end to end (IN2 §8 rule 6).
    #[must_use]
    pub const fn with_authentication(mut self, authentication: AuthenticationStatus) -> Self {
        self.authentication = authentication;
        self
    }

    /// The turns the script has not played yet.
    #[must_use]
    pub fn remaining_turns(&self) -> usize {
        self.turns.lock().map_or(0, |turns| turns.len())
    }
}

impl AgentDriver for ScriptedDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new(SCRIPTED_HARNESS_ID)
    }

    fn detect(&self) -> Option<HarnessInfo> {
        Some(HarnessInfo {
            id: self.id(),
            executable: std::path::PathBuf::from(SCRIPTED_HARNESS_ID),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            authentication: self.authentication,
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let Some(mcp_url) = cfg.mcp_url.clone() else {
            return Err(AgentError::MissingMcpEndpoint);
        };
        Ok(Box::new(ScriptedSession::start(
            Arc::clone(&self.turns),
            &mcp_url,
        )?))
    }
}

/// One blocking MCP client: an owned current-thread runtime plus a connected
/// `rmcp` service.
///
/// `initialize` is `ServiceExt::serve`, `tools/call` is `call_tool`, and both
/// are driven with `block_on` from the worker thread that owns them, which is
/// what keeps `send_user_message` non-blocking (IN2 §8 rule 3).
struct ScriptedMcpClient {
    runtime: tokio::runtime::Runtime,
    service: Option<RunningService<RoleClient, ()>>,
}

impl ScriptedMcpClient {
    fn connect(mcp_url: &str) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let service = runtime.block_on(async {
            ().serve(StreamableHttpClientTransport::from_uri(mcp_url.to_owned()))
                .await
                .map_err(|error| error.to_string())
        })?;
        Ok(Self {
            runtime,
            service: Some(service),
        })
    }

    fn call_tool(&self, call: &ScriptedCall) -> Result<CallToolResult, String> {
        let service = self
            .service
            .as_ref()
            .ok_or("the scripted client is closed")?;
        let request = match &call.arguments {
            serde_json::Value::Object(map) if !map.is_empty() => {
                CallToolRequestParams::new(call.tool.clone()).with_arguments(map.clone())
            }
            _ => CallToolRequestParams::new(call.tool.clone()),
        };
        self.runtime.block_on(async {
            service
                .call_tool(request)
                .await
                .map_err(|error| error.to_string())
        })
    }

    /// The served tool list, so a driver can prove the allowlist it was given.
    fn list_tools(&self) -> Result<Vec<String>, String> {
        let service = self
            .service
            .as_ref()
            .ok_or("the scripted client is closed")?;
        self.runtime.block_on(async {
            service
                .list_tools(None)
                .await
                .map(|result| {
                    result
                        .tools
                        .into_iter()
                        .map(|tool| tool.name.to_string())
                        .collect()
                })
                .map_err(|error| error.to_string())
        })
    }

    fn close(&mut self) {
        if let Some(service) = self.service.take() {
            let _ = self.runtime.block_on(service.cancel());
        }
    }
}

/// The real result of a real call, rendered the way a harness renders one into
/// `AgentEvent::ToolResult`: the text blocks first, then the structured body.
fn render_tool_result(result: &CallToolResult) -> String {
    let mut rendered = result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(structured) = result.structured_content.as_ref() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&serde_json::to_string(structured).unwrap_or_default());
    }
    rendered
}

/// What the worker thread is asked to do.
enum ScriptedWork {
    /// Play one turn.
    Turn(Box<ScriptedTurn>),
}

/// A session that replays scripted turns against a live MCP server.
pub struct ScriptedSession {
    turns: Arc<Mutex<VecDeque<ScriptedTurn>>>,
    events_rx: Receiver<AgentEvent>,
    work_tx: Option<Sender<ScriptedWork>>,
    worker: Option<thread::JoinHandle<()>>,
    /// Served tool names read at connect time, so a test can prove the session
    /// really spoke to the server before it played a turn.
    served_tools: Vec<String>,
}

impl ScriptedSession {
    fn start(turns: Arc<Mutex<VecDeque<ScriptedTurn>>>, mcp_url: &str) -> Result<Self, AgentError> {
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let (work_tx, work_rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let url = mcp_url.to_owned();
        let worker = thread::Builder::new()
            .name("kinewright-scripted".to_owned())
            .spawn(move || scripted_worker(&url, &ready_tx, &work_rx, &events_tx))
            .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(served_tools)) => Ok(Self {
                turns,
                events_rx,
                work_tx: Some(work_tx),
                worker: Some(worker),
                served_tools,
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(AgentError::Harness(error))
            }
            Err(_) => {
                let _ = worker.join();
                Err(AgentError::Harness(
                    "the scripted session worker stopped before it connected".to_owned(),
                ))
            }
        }
    }

    /// The served tool names this session's own client read from the server.
    #[must_use]
    pub fn served_tools(&self) -> &[String] {
        &self.served_tools
    }
}

impl AgentSession for ScriptedSession {
    fn send_user_message(&mut self, _text: String) -> Result<(), AgentError> {
        let turn = self
            .turns
            .lock()
            .map_err(|_| AgentError::Harness("the scripted script stopped".to_owned()))?
            .pop_front();
        // IN2 §8 rule 5: an exhausted script is a real stop the pump can
        // observe, rather than a hang.
        let Some(turn) = turn else {
            return Err(AgentError::Harness(
                "scripted driver has no turn left".to_owned(),
            ));
        };
        let work_tx = self
            .work_tx
            .as_ref()
            .ok_or_else(|| AgentError::Harness("the scripted session is closed".to_owned()))?;
        work_tx
            .send(ScriptedWork::Turn(Box::new(turn)))
            .map_err(|_| AgentError::Harness("the scripted session worker stopped".to_owned()))
    }

    fn events(&self) -> Receiver<AgentEvent> {
        self.events_rx.clone()
    }

    fn interrupt(&mut self) {
        drop(self.work_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ScriptedSession {
    fn drop(&mut self) {
        self.interrupt();
    }
}

fn scripted_worker(
    mcp_url: &str,
    ready: &Sender<Result<Vec<String>, String>>,
    work: &Receiver<ScriptedWork>,
    events: &Sender<AgentEvent>,
) {
    let mut client = match ScriptedMcpClient::connect(mcp_url) {
        Ok(client) => client,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let served = client.list_tools().unwrap_or_default();
    if ready.send(Ok(served)).is_err() {
        client.close();
        return;
    }
    while let Ok(ScriptedWork::Turn(turn)) = work.recv() {
        for call in &turn.calls {
            let _ = events.send(AgentEvent::ToolCall {
                name: call.tool.clone(),
                arguments: call.arguments.to_string(),
            });
            let rendered = match client.call_tool(call) {
                Ok(result) => render_tool_result(&result),
                Err(error) => error,
            };
            let _ = events.send(AgentEvent::ToolResult {
                name: call.tool.clone(),
                result: rendered,
            });
        }
        if !turn.pause.is_zero() {
            thread::sleep(turn.pause);
        }
        let _ = events.send(AgentEvent::Text(turn.message.clone()));
        if let Some(cost) = turn.cost {
            let _ = events.send(AgentEvent::Cost {
                input_tokens: cost.input_tokens,
                cached_input_tokens: cost.cached_input_tokens,
                cache_creation_input_tokens: cost.cache_creation_input_tokens,
                output_tokens: cost.output_tokens,
                reasoning_output_tokens: cost.reasoning_output_tokens,
                cost_usd: cost.cost_usd,
            });
        }
        let _ = events.send(AgentEvent::Done);
    }
    client.close();
}

#[cfg(test)]
mod tests {
    use kinewright_core::{Core, Document};
    use kinewright_media::FfmpegMediaEngine;

    use super::*;
    use crate::{McpServer, mirror_agent_cost};

    fn live_server() -> McpServer {
        let core = Core::spawn(Document::default()).expect("the fixture core starts");
        let media = Arc::new(FfmpegMediaEngine::new().expect("the media engine starts"));
        McpServer::start(core, Arc::clone(&media) as _, media as _).expect("the MCP server starts")
    }

    fn call(tool: &str, arguments: serde_json::Value) -> ScriptedCall {
        ScriptedCall {
            tool: tool.to_owned(),
            arguments,
        }
    }

    fn start(server: &McpServer, script: Vec<ScriptedTurn>) -> Box<dyn AgentSession> {
        ScriptedDriver::new(script)
            .start_session(SessionConfig {
                mcp_url: Some(server.endpoint().to_owned()),
                ..SessionConfig::default()
            })
            .expect("the scripted session starts")
    }

    /// Drain events until `AgentEvent::Done`, or give up after ten seconds.
    ///
    /// A **tick** timeout is not a stop: the worker has to connect, make a real
    /// loopback round trip and answer, and under a parallel `cargo test` that
    /// can take longer than one 100 ms tick. Only a disconnect — the worker
    /// thread having ended — ends the drain early. Giving up on the first tick
    /// made this helper flake about one run in five.
    fn drain_turn(events: &Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut collected = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    let done = matches!(event, AgentEvent::Done);
                    collected.push(event);
                    if done {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        collected
    }

    /// IN2 §9.1 item 21, §8 rules 3 and 5, and §9 clause 17: the driver makes
    /// **real** tool calls through its own MCP client.
    ///
    /// The results come from the server and not from the script, which is the
    /// whole difference between this and `eval.rs`'s `#[cfg(test)]` fake: a
    /// two-turn script produces four `ToolCall`/`ToolResult` pairs whose
    /// contents only the real dispatcher could have produced.
    #[test]
    fn in2_the_scripted_driver_makes_real_tool_calls_through_its_own_client() {
        let server = live_server();
        let script = vec![
            ScriptedTurn::new(
                vec![
                    call("get_timeline_state", serde_json::json!({})),
                    call(
                        "search_capabilities",
                        serde_json::json!({"queries": ["incident"]}),
                    ),
                ],
                "read the cut",
            ),
            ScriptedTurn::new(
                vec![
                    call("get_timeline_state", serde_json::json!({})),
                    call(
                        "get_capability",
                        serde_json::json!({"names": ["get_incidents"]}),
                    ),
                ],
                "read the capability",
            ),
        ];
        let mut session = start(&server, script);
        let events = session.events();

        let mut calls = 0_usize;
        let mut results = Vec::new();
        for turn in 0..2 {
            session
                .send_user_message(format!("turn {turn}"))
                .expect("the scripted turn is accepted");
            for event in drain_turn(&events) {
                match event {
                    AgentEvent::ToolCall { .. } => calls += 1,
                    AgentEvent::ToolResult { name, result } => results.push((name, result)),
                    _ => {}
                }
            }
        }
        assert_eq!(calls, 4, "two turns of two calls each");
        assert_eq!(results.len(), 4);
        assert!(
            results
                .iter()
                .any(|(name, result)| name == "get_timeline_state"
                    && result.contains("timeline_revision=")),
            "the result came from the server, not from the script: {results:?}"
        );
        assert!(
            results
                .iter()
                .any(|(name, result)| name == "get_capability" && result.contains("get_incidents")),
            "{results:?}"
        );

        // IN2 §8 rule 5: an exhausted script is a real stop, not a hang.
        assert_eq!(
            session.send_user_message("turn 2".to_owned()),
            Err(AgentError::Harness(
                "scripted driver has no turn left".to_owned()
            ))
        );
        session.interrupt();
        server.shutdown();
    }

    /// IN2 §9.1 item 22 and §8 rule 6: `detect()` reports whichever
    /// authentication status the test asked for.
    ///
    /// All three variants, so §2.2 rule 7's `Unknown`-is-available rule and
    /// rule 8's one-strike disable are both testable end to end.
    #[test]
    fn in2_a_scripted_driver_reports_whichever_authentication_status_it_was_given() {
        for status in [
            AuthenticationStatus::Unknown,
            AuthenticationStatus::Authenticated,
            AuthenticationStatus::Unauthenticated,
        ] {
            let driver = ScriptedDriver::new(Vec::new()).with_authentication(status);
            let info = driver
                .detect()
                .expect("a scripted driver is always present");
            assert_eq!(info.authentication, status);
            assert_eq!(info.id, HarnessId::new(SCRIPTED_HARNESS_ID));
        }
        // The default is `Unknown`, which is what two of the three production
        // detectors return and what IN2 §2.2 rule 7 treats as available.
        assert_eq!(
            ScriptedDriver::new(Vec::new())
                .detect()
                .unwrap()
                .authentication,
            AuthenticationStatus::Unknown
        );
        // A session without an endpoint is refused rather than faked.
        assert_eq!(
            ScriptedDriver::new(Vec::new())
                .start_session(SessionConfig::default())
                .err(),
            Some(AgentError::MissingMcpEndpoint)
        );
    }

    /// IN2 §9.1 item 23, §8 rule 4 and §9 clause 7: a `ScriptedCost` with all
    /// six fields set fills all six `mirror_agent_cost` mirrors.
    ///
    /// Six fields and not two: `mirror_agent_cost` passes the event's four
    /// `Option`s straight through, so a two-field cost would leave four of the
    /// six telemetry mirrors `None` for every scripted turn and make clause 7
    /// unsatisfiable by any test this contract specifies.
    #[test]
    fn in2_a_scripted_cost_fills_all_six_mirror_fields() {
        let server = live_server();
        let cost = ScriptedCost {
            input_tokens: 1_200,
            output_tokens: 340,
            cached_input_tokens: Some(64),
            cache_creation_input_tokens: Some(32),
            reasoning_output_tokens: Some(16),
            cost_usd: Some(0.012_5),
        };
        let mut session = start(
            &server,
            vec![
                ScriptedTurn::new(
                    vec![call("get_timeline_state", serde_json::json!({}))],
                    "done",
                )
                .with_cost(cost),
            ],
        );
        let events = session.events();
        session
            .send_user_message("investigate".to_owned())
            .expect("the scripted turn is accepted");
        let drained = drain_turn(&events);
        let costs = drained
            .iter()
            .filter(|event| matches!(event, AgentEvent::Cost { .. }))
            .collect::<Vec<_>>();
        assert_eq!(costs.len(), 1, "one cost event per turn: {drained:?}");
        let AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            cost_usd,
        } = costs[0]
        else {
            unreachable!("filtered above")
        };
        assert_eq!(*input_tokens, 1_200);
        assert_eq!(*output_tokens, 340);
        assert_eq!(*cached_input_tokens, Some(64));
        assert_eq!(*cache_creation_input_tokens, Some(32));
        assert_eq!(*reasoning_output_tokens, Some(16));
        assert_eq!(*cost_usd, Some(0.012_5));

        let mut telemetry = kinewright_core::IncidentTelemetry::default();
        mirror_agent_cost(&mut telemetry, costs[0]);
        assert_eq!(telemetry.input_tokens, Some(1_200));
        assert_eq!(telemetry.output_tokens, Some(340));
        assert_eq!(telemetry.cached_input_tokens, Some(64));
        assert_eq!(telemetry.cache_creation_input_tokens, Some(32));
        assert_eq!(telemetry.reasoning_output_tokens, Some(16));
        assert_eq!(telemetry.cost_usd_millionths, Some(12_500));

        session.interrupt();
        server.shutdown();
    }

    /// IN2 erratum D-R67: a server shut down while a client is still connected
    /// completes in **seconds, not minutes**.
    ///
    /// `axum`'s graceful shutdown waits for live connections, and a
    /// streamable-HTTP MCP client holds one open until it is closed — measured
    /// at **300 s** by review 2 against an un-closed [`ScriptedSession`]. IN2
    /// is the first slice to ship a production in-process MCP client and the
    /// first whose per-incident server is torn down by a user-interface
    /// thread, so an unbounded wait here is a five-minute freeze. The session
    /// below is deliberately **not** interrupted first: this asserts the floor
    /// under a mis-ordered teardown, not the designed order.
    #[test]
    fn in2_shutting_down_with_a_live_client_is_bounded() {
        let server = live_server();
        let session = start(&server, vec![ScriptedTurn::new(Vec::new(), "idle")]);
        assert_eq!(
            server.confirmations().incident(),
            None,
            "an ordinary server stamps nothing"
        );

        let started = std::time::Instant::now();
        server.shutdown();
        let elapsed = started.elapsed();
        println!("IN2_SHUTDOWN with_live_client={elapsed:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "shutdown with a live client took {elapsed:?}; the graceful wait is bounded at 2 s"
        );
        drop(session);
    }

    /// IN2 §3.4 rule 25 and reviewers' N6: the CLI allowlist is **not** the
    /// served filter.
    ///
    /// `SessionConfig.tool_names` is what a harness CLI is told; the server
    /// applies one global `COMPACT_TOOL_NAMES` filter for everybody. A session
    /// given the six-name investigator allowlist therefore still sees all
    /// **seven** served tools on the wire, which is why §3.4 rule 25 says a
    /// six-tool allowlist reduces the CLI argument and not the served list.
    #[test]
    fn in2_the_investigator_allowlist_does_not_shrink_the_served_surface() {
        let server = live_server();
        let driver = ScriptedDriver::new(vec![ScriptedTurn::new(Vec::new(), "idle")]);
        let session = ScriptedSession::start(Arc::clone(&driver.turns), server.endpoint())
            .expect("the scripted session starts");
        assert_eq!(
            session.served_tools().len(),
            7,
            "the served surface is the server's, not the CLI allowlist's: {:?}",
            session.served_tools()
        );
        assert_eq!(crate::runtime::INVESTIGATOR_TOOL_NAMES.len(), 6);
        for name in crate::runtime::INVESTIGATOR_TOOL_NAMES {
            assert!(
                session.served_tools().iter().any(|served| served == name),
                "{name} must be reachable on the wire"
            );
        }
        drop(session);
        server.shutdown();
    }
}
