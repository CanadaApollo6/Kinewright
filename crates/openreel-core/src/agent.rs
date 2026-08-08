use std::path::PathBuf;

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HarnessId(pub String);

impl HarnessId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationStatus {
    Unknown,
    Authenticated,
    Unauthenticated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessInfo {
    pub id: HarnessId,
    pub executable: PathBuf,
    pub version: Option<String>,
    pub authentication: AuthenticationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionConfig {
    pub working_directory: Option<PathBuf>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Text(String),
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, result: String },
    Cost { input_tokens: u64, output_tokens: u64 },
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentError {
    #[error("agent session protocol is not implemented in M0")]
    NotImplemented,
    #[error("agent harness is not installed")]
    NotInstalled,
    #[error("agent harness error: {0}")]
    Harness(String),
}

pub trait AgentDriver: Send + Sync {
    fn id(&self) -> HarnessId;
    fn detect(&self) -> Option<HarnessInfo>;
    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError>;
}

pub trait AgentSession: Send {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError>;
    fn events(&self) -> Receiver<AgentEvent>;
    fn interrupt(&mut self);
}

