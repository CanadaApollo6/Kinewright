//! `MCP` tools and installed-agent CLI drivers for the live `Kinewright` process.

mod acp;
mod audio_qc_tool;
mod audio_repair_tool;
mod branch;
mod color_qc_tool;
mod color_scopes;
mod color_status;
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
mod models;
mod pacing;
mod protocol;
mod render;
mod runtime;
mod schema;
mod server;
mod silence;

pub use branch::{BranchApplyOutcome, BranchComparison, BranchError, TimelineBranch};
pub use cursor::{CURSOR_SANDBOX_NOTICE, CursorAcpDriver, cursor_models};
pub use drivers::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
pub use models::{
    CLAUDE_ULTRACODE, ModelChoice, ServiceTier, claude_models, codex_default_model, codex_models,
    common_efforts, common_tiers,
};
pub use render::{
    render_asset_transcript, render_clip_info, render_timeline_state, render_timeline_transcript,
};
pub use runtime::{
    CapabilityDescriptor, CapabilityKind, EditPlanPreview, PreparedPlanId, ToolSurfaceMetrics,
    compact_tool_names,
};
pub use schema::{capability_tool_names, operation_tools};
pub use server::{
    ConfirmationBroker, ConfirmationRequest, IN1_INCIDENT_SERIALIZED_BYTES,
    IN1_INCIDENT_SERIALIZED_CEILING_BYTES, IncidentLogHandle, MIX_MEASUREMENT_SAMPLE_RATE,
    McpServer, McpServerError, mirror_agent_cost,
};
pub use silence::{
    shrink_silence_span_for_cutting, shrink_silence_span_for_cutting_with_transcript,
    silence_cut_margin_frames,
};
