//! `MCP` tools and installed-agent CLI drivers for the live `Kinewright` process.

mod acp;
mod acp_drivers;
mod acp_session;
mod audio_qc_tool;
mod audio_repair_tool;
mod branch;
mod child_process;
mod color_qc_tool;
mod color_scopes;
mod color_status;
mod copilot;
mod cursor;
mod drivers;
/// The eval harness. Gated with the fixture data it consumes: `eval.rs`
/// imports `kinewright_core::au6_scenarios` at module scope, which only
/// exists under core's `test-util`, so a build of this crate with
/// `default-features = false` (the app's) cannot compile the module and has
/// no caller for it. `kinewright-eval` keeps the default feature and sees it.
#[cfg(feature = "eval-harness")]
pub mod eval;
pub mod export_queue;
pub mod fixture_pack;
mod harness;
mod models;
mod muse;
mod pacing;
mod protocol;
mod render;
mod runtime;
mod schema;
/// The production scripted driver (IN2 §8). Not feature-gated: the
/// application's own tests reach it under `default-features = false`.
mod scripted;
mod server;
/// The session pump (IN2 §3.5). Not feature-gated, for the same reason.
mod session;
mod silence;

pub use acp_drivers::{
    DEVIN_SANDBOX_NOTICE, DevinDriver, KIMI_SANDBOX_NOTICE, KIRO_SANDBOX_NOTICE, KimiDriver,
    KiroDriver, OPENCODE_SANDBOX_NOTICE, OpenCodeDriver, QWEN_SANDBOX_NOTICE, QwenDriver,
    devin_models, kimi_models, kiro_models, opencode_models, qwen_models,
};
pub use branch::{
    BranchApplyOutcome, BranchComparison, BranchError, TimelineBranch, apply_to_live,
};
pub use copilot::{COPILOT_SANDBOX_NOTICE, CopilotDriver, copilot_models};
pub use cursor::{CURSOR_SANDBOX_NOTICE, CursorAcpDriver, cursor_models};
pub use drivers::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
pub use harness::{HARNESS_KEYS, harness_driver};
pub use models::{
    CLAUDE_ULTRACODE, ModelChoice, ServiceTier, claude_models, codex_default_model, codex_models,
    common_efforts, common_tiers,
};
pub use muse::{MUSE_SANDBOX_NOTICE, MuseDriver, muse_models};
pub use render::{
    render_asset_transcript, render_clip_info, render_timeline_state, render_timeline_transcript,
};
pub use runtime::{
    CapabilityDescriptor, CapabilityKind, EditPlanPreview, INVESTIGATOR_TOOL_NAMES, PreparedPlanId,
    ToolSurfaceMetrics, compact_tool_names, is_destructive_operation,
};
pub use schema::{capability_tool_names, operation_tools};
pub use scripted::{
    SCRIPTED_HARNESS_ID, ScriptedCall, ScriptedCost, ScriptedDriver, ScriptedSession, ScriptedTurn,
};
pub use server::{
    ConfirmationBroker, ConfirmationRequest, IN1_INCIDENT_SERIALIZED_BYTES,
    IN1_INCIDENT_SERIALIZED_CEILING_BYTES, INVESTIGATOR_BROKER_TIMEOUT,
    INVESTIGATOR_CAPABILITY_DENYLIST, INVESTIGATOR_WORKER_THREADS, IncidentLogHandle,
    InvestigatorSessionContext, MIX_MEASUREMENT_SAMPLE_RATE, McpServer, McpServerError,
    mirror_agent_cost,
};
pub use session::{
    BudgetKind, CANCELLED_BY_OWNER, ConfirmationPolicy, INVESTIGATOR_CONFIRMATION_REFUSAL,
    QuietObserver, SessionCost, SessionCounters, SessionFlow, SessionLimits, SessionObserver,
    SessionStop, SharedCounters, StopReason, pump_session,
};
pub use silence::{
    shrink_silence_span_for_cutting, shrink_silence_span_for_cutting_with_transcript,
    silence_cut_margin_frames,
};
