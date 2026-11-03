#![allow(clippy::missing_errors_doc)]
//! Bounded, size-only TE1 observations owned by the accounting publisher.
use crate::{bundle_records::SessionId, orchestration_accounting::AccountingRefusal};
use serde::{Deserialize, Serialize};

pub const DETAILS: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    pub definitions: u64,
    pub schemas: u64,
    pub descriptions: u64,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub attempt: SessionId,
    pub role: String,
    pub stream: SessionId,
    pub turn: u64,
    pub prompt_bytes: u64,
    pub wake_bytes: u64,
    pub prefix_identity: String,
    pub surface: Surface,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    pub attempt: SessionId,
    /// Provider call/turn identity is unobserved on the MCP HTTP connection.
    pub turn: Option<u64>,
    pub provider_call: Option<u64>,
    pub host_call: u64,
    pub tool: String,
    pub serialized_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurements {
    pub sends: Vec<Send>,
    pub results: Vec<ToolResult>,
    pub result_count: u64,
    pub result_bytes: u64,
    pub incomplete: bool,
    /// Neither CLI exposes all internal model steps or request ordering.
    pub provider_steps: Option<u64>,
    pub provider_request_order: Option<u64>,
    pub unreported_helper_tokens: Option<u64>,
}

impl Measurements {
    pub fn result(&mut self, mut result: ToolResult) {
        self.result_count = self.result_count.saturating_add(1);
        self.result_bytes = self.result_bytes.saturating_add(result.serialized_bytes);
        result.host_call = self.result_count;
        if self.results.len() == DETAILS {
            self.incomplete = true;
        } else {
            self.results.push(result);
        }
    }

    pub fn validate(&self) -> Result<(), AccountingRefusal> {
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if self.results.len() > DETAILS
            || self.sends.len() > crate::orchestration_accounting::TASK_SENDS
            || self.result_count < self.results.len() as u64
            || (!self.incomplete && self.result_count != self.results.len() as u64)
            || self.results.iter().any(|r| {
                r.tool.len() > 64 || !r.tool.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
            || self.sends.iter().any(|s| {
                !hash(&s.prefix_identity)
                    || !hash(&s.surface.identity)
                    || ![
                        "director", "editor", "motion", "audio", "colour", "reviewer",
                    ]
                    .contains(&s.role.as_str())
            })
        {
            return Err(AccountingRefusal::Invalid);
        }
        Ok(())
    }
}
