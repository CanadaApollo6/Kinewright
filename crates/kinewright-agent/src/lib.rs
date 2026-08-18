//! `MCP` tools and installed-agent CLI drivers for the live `Kinewright` process.

mod acp;
mod branch;
mod cursor;
mod drivers;
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
pub use server::{ConfirmationBroker, ConfirmationRequest, McpServer, McpServerError};
pub use silence::{
    shrink_silence_span_for_cutting, shrink_silence_span_for_cutting_with_transcript,
    silence_cut_margin_frames,
};
