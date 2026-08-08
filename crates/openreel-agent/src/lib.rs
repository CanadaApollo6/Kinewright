//! MCP tools and installed-agent CLI drivers for the live OpenReel process.

mod drivers;
mod protocol;
mod render;
mod schema;
mod server;

pub use drivers::{CODEX_FAIL_CLOSED_REASON, ClaudeCodeDriver, CodexDriver};
pub use render::{render_clip_info, render_timeline_state};
pub use schema::{all_tool_names, operation_tools};
pub use server::{ConfirmationBroker, ConfirmationRequest, McpServer, McpServerError};

