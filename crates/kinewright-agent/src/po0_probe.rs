//! A fixed-seed, read-only tracking request for the PO0 desktop probe.
use crate::{
    ConfirmationPolicy, McpServer, ScriptedCall, ScriptedDriver, ScriptedTurn, SessionCounters,
    SessionFlow, SessionLimits, SessionObserver, SharedCounters, pump_session,
};
use kinewright_core::{AgentDriver, AgentEvent, Analysis, Core, Playback, SessionConfig};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
struct ResultObserver {
    result: Option<Result<serde_json::Value, String>>,
}
impl SessionObserver for ResultObserver {
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow {
        match event {
            AgentEvent::ToolResult { result, .. } => {
                self.result = Some(result.find('{').ok_or_else(|| result.clone()).and_then(
                    |start| {
                        serde_json::from_str(&result[start..])
                            .map_err(|error| format!("tracker JSON: {error}"))
                    },
                ));
            }
            AgentEvent::Error(error) => self.result = Some(Err(error.clone())),
            _ => {}
        }
        SessionFlow::Continue
    }
    fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
        SessionFlow::Continue
    }
}
/// Runs the existing tracker and discards its prepared edit plan. It commits nothing.
///
/// # Errors
/// Returns the MCP, tracker, or bounded session failure.
pub fn track_pin(
    core: Core,
    playback: Arc<dyn Playback>,
    analysis: Arc<dyn Analysis>,
) -> Result<serde_json::Value, String> {
    let server = McpServer::start(core, playback, analysis).map_err(|e| e.to_string())?;
    let driver = ScriptedDriver::new(vec![ScriptedTurn::new(
        vec![ScriptedCall {
            tool: "invoke_capability".into(),
            arguments: serde_json::json!({"name":"track_reframe_subject","arguments":{
                "clip_id":1,"effect_id":1,"subject_width_percent":12,"subject_height_percent":20,
                "initial_subject_x_percent":50,"initial_subject_y_percent":45,
                "start_local_frame":0,"end_local_frame":120,"step_frames":5,"max_width":256
            }}),
        }],
        "",
    )]);
    let mut session = driver
        .start_session(SessionConfig {
            mcp_url: Some(server.endpoint().into()),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let events = session.events();
    let mut observer = ResultObserver { result: None };
    let _ = pump_session(
        session.as_mut(),
        &events,
        &mut std::iter::once("track the fixed seed".into()),
        None,
        ConfirmationPolicy::RejectAndStop,
        &SessionLimits {
            max_turns: 1,
            max_wall_time: Duration::from_secs(30),
            max_tokens: None,
        },
        &Arc::new(SharedCounters::default()),
        &Arc::new(AtomicBool::new(false)),
        &mut observer,
    );
    drop(session);
    server.shutdown();
    observer
        .result
        .unwrap_or_else(|| Err("tracker produced no result".into()))
}
