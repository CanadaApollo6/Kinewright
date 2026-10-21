//! Forward the real driver, session and pump. This is a recording boundary,
//! not an implementation of future orchestration or provider behavior.
#![allow(clippy::missing_errors_doc)]
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, bounded};
use kinewright_core::{
    AgentDriver, AgentError, AgentEvent, AgentSession, HarnessId, HarnessInfo, SessionConfig,
};
use rmcp::{
    ServiceExt as _, model::CallToolRequestParams, transport::StreamableHttpClientTransport,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::protocol::{CONTROL_BYTES, INDEX_BYTES, PreparedArgs, unique_json};
use crate::{
    ConfirmationPolicy, McpServer, ScriptedCall, ScriptedCost, ScriptedDriver, ScriptedTurn,
    SessionCounters, SessionFlow, SessionLimits, SessionObserver, SharedCounters, pump_session,
};

#[derive(Default)]
struct Log {
    entries: Vec<Value>,
    bytes: u64,
    overflowed: bool,
}
type SharedLog = Arc<Mutex<Log>>;

fn record(log: &SharedLog, entry: Value) {
    let mut log = log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = serde_json::to_vec(&entry).map_or(usize::MAX, |v| v.len());
    log.bytes = log
        .bytes
        .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    if bytes > CONTROL_BYTES || log.bytes > INDEX_BYTES || log.entries.len() >= 10_000 {
        log.overflowed = true;
    } else {
        log.entries.push(entry);
    }
}

struct RecordingDriver<D> {
    inner: D,
    log: SharedLog,
}

impl<D: AgentDriver> AgentDriver for RecordingDriver<D> {
    fn id(&self) -> HarnessId {
        self.inner.id()
    }
    fn detect(&self) -> Option<HarnessInfo> {
        self.inner.detect()
    }
    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        record(
            &self.log,
            json!({"kind":"start_session","model":cfg.model,"effort":cfg.effort,"service_tier":cfg.service_tier,"max_turns":cfg.max_turns,"mcp_url":cfg.mcp_url,"tool_names":cfg.tool_names}),
        );
        let inner = self.inner.start_session(cfg)?;
        let source = inner.events();
        let (sender, events) = bounded(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let log = Arc::clone(&self.log);
        let forwarder = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match source.recv_timeout(Duration::from_millis(10)) {
                    Ok(event) => {
                        record(
                            &log,
                            json!({"kind":"forwarded_event","event":event_value(&event)}),
                        );
                        if sender.try_send(event).is_err() {
                            log.lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .overflowed = true;
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Ok(Box::new(RecordingSession {
            inner,
            events,
            stop,
            forwarder: Some(forwarder),
            log: Arc::clone(&self.log),
        }))
    }
}

struct RecordingSession {
    inner: Box<dyn AgentSession>,
    events: Receiver<AgentEvent>,
    stop: Arc<AtomicBool>,
    forwarder: Option<thread::JoinHandle<()>>,
    log: SharedLog,
}
impl AgentSession for RecordingSession {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        record(&self.log, json!({"kind":"send_user_message","text":text}));
        self.inner.send_user_message(text)
    }
    fn events(&self) -> Receiver<AgentEvent> {
        self.events.clone()
    }
    fn interrupt(&mut self) {
        record(&self.log, json!({"kind":"interrupt"}));
        self.inner.interrupt();
    }
}
impl Drop for RecordingSession {
    fn drop(&mut self) {
        self.inner.interrupt();
        self.stop.store(true, Ordering::Release);
        if let Some(forwarder) = self.forwarder.take() {
            let _ = forwarder.join();
        }
    }
}

struct Observer(SharedLog);
impl SessionObserver for Observer {
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow {
        record(
            &self.0,
            json!({"kind":"pump_event","event":event_value(event),"decision":"continue"}),
        );
        SessionFlow::Continue
    }
    fn on_tick(&mut self, counters: &SessionCounters) -> SessionFlow {
        record(
            &self.0,
            json!({"kind":"pump_tick","turns":counters.turns,"input_tokens":counters.input_tokens,"output_tokens":counters.output_tokens,"tool_calls":counters.tool_calls,"decision":"continue"}),
        );
        SessionFlow::Continue
    }
    fn on_confirmation_error(&mut self, message: String) {
        record(
            &self.0,
            json!({"kind":"confirmation_error","message":message}),
        );
    }
}

fn event_value(event: &AgentEvent) -> Value {
    match event {
        AgentEvent::Text(text) => json!({"kind":"text","text":text}),
        AgentEvent::Error(text) => json!({"kind":"error","text":text}),
        AgentEvent::ToolCall { name, arguments } => {
            json!({"kind":"tool_call","name":name,"arguments":arguments})
        }
        AgentEvent::ToolResult { name, result } => {
            json!({"kind":"tool_result","name":name,"result":result})
        }
        AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            cost_usd,
        } => {
            json!({"kind":"cost","input_tokens":input_tokens,"cached_input_tokens":cached_input_tokens,"cache_creation_input_tokens":cache_creation_input_tokens,"output_tokens":output_tokens,"reasoning_output_tokens":reasoning_output_tokens,"cost_usd":cost_usd})
        }
        AgentEvent::Done => json!({"kind":"done"}),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    sessions: Vec<Session>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    turns: Vec<Turn>,
    prompts: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    calls: Vec<WireCall>,
    message: String,
    cost: Option<WireUsage>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCall {
    tool: String,
    arguments: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    cost_usd: Option<f64>,
}

pub fn scripted_session(server: &McpServer, bytes: &[u8]) -> Result<Value, String> {
    let spec: Script = serde_json::from_value(unique_json(bytes)?).map_err(|e| e.to_string())?;
    if spec.sessions.is_empty() || spec.sessions.len() > 8 {
        return Err("script session quota".into());
    }
    let mut results = Vec::new();
    let before = server.ja_request_counts();
    for session in spec.sessions {
        if session.turns.len() > 8
            || session.prompts.len() != session.turns.len()
            || session.turns.iter().any(|t| t.calls.len() > 64)
        {
            return Err("script turn/call quota".into());
        }
        let turns = session
            .turns
            .into_iter()
            .map(|turn| {
                let mut t = ScriptedTurn::new(
                    turn.calls
                        .into_iter()
                        .map(|c| ScriptedCall {
                            tool: c.tool,
                            arguments: c.arguments,
                        })
                        .collect(),
                    turn.message,
                );
                t.cost = turn.cost.map(|c| ScriptedCost {
                    input_tokens: c.input_tokens,
                    output_tokens: c.output_tokens,
                    cached_input_tokens: c.cached_input_tokens,
                    cache_creation_input_tokens: c.cache_creation_input_tokens,
                    reasoning_output_tokens: c.reasoning_output_tokens,
                    cost_usd: c.cost_usd,
                });
                t
            })
            .collect();
        let log = Arc::new(Mutex::new(Log::default()));
        let driver = RecordingDriver {
            inner: ScriptedDriver::new(turns),
            log: Arc::clone(&log),
        };
        let mut session_driver = driver
            .start_session(SessionConfig {
                mcp_url: Some(server.endpoint().into()),
                ..SessionConfig::default()
            })
            .map_err(|e| e.to_string())?;
        let events = session_driver.events();
        let (stop, counters) = pump_session(
            session_driver.as_mut(),
            &events,
            &mut session.prompts.into_iter(),
            Some(&server.confirmations()),
            ConfirmationPolicy::RejectAndStop,
            &SessionLimits {
                max_turns: 8,
                max_wall_time: Duration::from_secs(10),
                max_tokens: None,
            },
            &Arc::new(SharedCounters::default()),
            &Arc::new(AtomicBool::new(false)),
            &mut Observer(Arc::clone(&log)),
        );
        drop(session_driver);
        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if log.overflowed {
            return Err("recording overflow; incomplete/NOT RUN".into());
        }
        let forwarded = log
            .entries
            .iter()
            .filter(|v| v["kind"] == "forwarded_event")
            .map(|v| &v["event"])
            .collect::<Vec<_>>();
        let pumped = log
            .entries
            .iter()
            .filter(|v| v["kind"] == "pump_event")
            .map(|v| &v["event"])
            .collect::<Vec<_>>();
        results.push(json!({"stop":format!("{stop:?}"),"turns":counters.turns,"tool_calls":counters.tool_calls,
            "forwarded_equals_pumped":forwarded == pumped,"events":log.entries,"provider_certification":null}));
    }
    let after = server.ja_request_counts();
    Ok(
        json!({"sessions":results,"server_requests":after[0]-before[0],"server_results":after[1]-before[1]}),
    )
}

pub fn prepared_commit(
    server: &McpServer,
    args: &PreparedArgs,
    bytes: &[u8],
) -> Result<Value, String> {
    let operations = unique_json(bytes)?;
    let before = server.ja_request_counts();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = runtime.block_on(async {
        let client = ().serve(StreamableHttpClientTransport::from_uri(server.endpoint())).await.map_err(|e| e.to_string())?;
        let prepare_request = json!({"expected_revision":args.expected_revision,"operations":operations});
        let prepared = client.call_tool(CallToolRequestParams::new("prepare_edit_plan").with_arguments(prepare_request.as_object().ok_or("prepare object")?.clone())).await.map_err(|e| e.to_string())?;
        let plan_id = prepared.structured_content.as_ref().and_then(|v| v.get("plan_id")).ok_or("real server did not prepare a plan")?;
        let commit_request = json!({"expected_revision":args.expected_revision,"plan_id":plan_id});
        let commit = client.call_tool(CallToolRequestParams::new("commit_edit_plan").with_arguments(commit_request.as_object().ok_or("commit object")?.clone()));
        let (committed, confirmations) = if let Some(approve) = args.confirmation {
            let broker = server.confirmations();
            let answer = async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                loop {
                    let requests = broker.pending_requests();
                    if !requests.is_empty() {
                        let mut observations = Vec::new();
                        for request in requests {
                            observations.push(json!({"id":request.id,"tool_name":request.tool_name,"description":request.description,"approved":approve}));
                            let answered = if approve { broker.approve(request.id) } else { broker.reject(request.id, "S0 fixture refuses") };
                            if !answered { return Err("real confirmation disappeared before answer".into()); }
                        }
                        return Ok::<_, String>(observations);
                    }
                    if tokio::time::Instant::now() >= deadline { return Err("confirmation prerequisite not observed".into()); }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            };
            let (result, confirmations) = tokio::join!(commit, answer);
            (result.map_err(|e| e.to_string())?, Some(confirmations?))
        } else { (commit.await.map_err(|e| e.to_string())?, None) };
        let value = json!({"prepare_request":prepare_request,"prepare_result":prepared,"commit_request":commit_request,"commit_result":committed,"commit_succeeded":committed.is_error == Some(false),"confirmations":confirmations});
        client.cancel().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(value)
    })?;
    let after = server.ja_request_counts();
    Ok(
        json!({"trace":result,"server_requests":after[0]-before[0],"server_results":after[1]-before[1]}),
    )
}
