//! `MCP` tools and installed-agent CLI drivers for the live `OpenReel` process.

mod drivers;
mod protocol;
mod render;
mod schema;
mod server;

pub use drivers::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
pub use render::{
    render_asset_transcript, render_clip_info, render_timeline_state, render_timeline_transcript,
};
pub use schema::{all_tool_names, operation_tools};
pub use server::{ConfirmationBroker, ConfirmationRequest, McpServer, McpServerError};
