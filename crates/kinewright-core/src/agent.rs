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
    /// Provider subscription reported by the harness, when it exposes one.
    pub subscription_tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionConfig {
    pub working_directory: Option<PathBuf>,
    pub model: Option<String>,
    /// Reasoning effort passed to the harness CLI; `None` uses its default.
    pub effort: Option<String>,
    /// Service tier id passed to the harness CLI (e.g. Codex `priority` =
    /// its Fast mode); `None` runs at the provider's standard tier.
    pub service_tier: Option<String>,
    pub max_turns: Option<u32>,
    /// Streamable HTTP endpoint for the live `Kinewright` MCP server.
    pub mcp_url: Option<String>,
    /// Exact MCP tool names exposed to this harness session. `None` uses the
    /// standard compact `Kinewright` runtime.
    pub tool_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Text(String),
    Error(String),
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        result: String,
    },
    Cost {
        /// Provider-reported total input tokens. For providers that report
        /// cache reads separately, adapters normalize those reads into this total.
        input_tokens: u64,
        /// Input tokens served from a provider prompt cache, when exposed.
        cached_input_tokens: Option<u64>,
        /// Input tokens written to a provider prompt cache, when exposed.
        cache_creation_input_tokens: Option<u64>,
        output_tokens: u64,
        /// Reasoning tokens included in `output_tokens`, when exposed.
        reasoning_output_tokens: Option<u64>,
        cost_usd: Option<f64>,
    },
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentError {
    #[error("agent harness is not installed")]
    NotInstalled,
    #[error("agent session requires the Kinewright MCP endpoint")]
    MissingMcpEndpoint,
    #[error("agent driver is unavailable: {0}")]
    Unavailable(String),
    #[error("agent harness error: {0}")]
    Harness(String),
    #[error("agent protocol error: {0}")]
    Protocol(String),
}

pub trait AgentDriver: Send + Sync {
    fn id(&self) -> HarnessId;
    fn detect(&self) -> Option<HarnessInfo>;
    /// Start a harness session using the supplied project context.
    ///
    /// # Errors
    ///
    /// Returns an agent error when the harness is unavailable or rejects setup.
    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError>;
}

pub trait AgentSession: Send {
    /// Send one user turn to the running harness.
    ///
    /// # Errors
    ///
    /// Returns an agent error when the harness cannot accept the message.
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError>;
    fn events(&self) -> Receiver<AgentEvent>;
    fn interrupt(&mut self);
    /// Nonblocking. Existing sessions have no certified live request channel.
    /// # Errors
    /// Reports connector protocol failure without inventing a callback.
    fn poll_runtime_request(&mut self) -> Result<Option<RuntimeRequest>, AgentError> {
        Ok(None)
    }
    fn runtime_request_binding(&self) -> Option<RuntimeRequestBinding> {
        None
    }
    /// # Errors
    /// Unsupported unless the connector implements a verified live callback.
    fn respond_runtime_request(
        &mut self,
        _request: &RuntimeRequest,
        _response: RuntimeResponse,
    ) -> Result<(), AgentError> {
        Err(AgentError::Unavailable(
            "live_runtime_controls_unsupported".into(),
        ))
    }
}

/// Nonsecret live identity. These DTOs carry no callback or approval authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequestBinding {
    pub task: String,
    pub attempt: String,
    pub session: String,
    pub turn: u64,
    pub owner_epoch: u64,
}
impl RuntimeRequestBinding {
    fn valid(&self) -> bool {
        [&self.task, &self.attempt, &self.session]
            .into_iter()
            .all(|value| {
                value.len() == 32
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            && self.turn > 0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRequestKind {
    Question,
    Approval,
    Secret,
}

/// Constructed after a connector's bounded read. Secret requests contain a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequest {
    id: String,
    binding: RuntimeRequestBinding,
    kind: RuntimeRequestKind,
    text: String,
}
impl RuntimeRequest {
    /// # Errors
    /// Refuses malformed identities or a payload above 16 KiB.
    pub fn new(
        id: String,
        binding: RuntimeRequestBinding,
        kind: RuntimeRequestKind,
        text: String,
    ) -> Result<Self, AgentError> {
        if id.is_empty() || id.len() > 128 || !binding.valid() || text.len() > 16 * 1024 {
            return Err(AgentError::Protocol("runtime_request_bounds".into()));
        }
        Ok(Self {
            id,
            binding,
            kind,
            text,
        })
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub fn binding(&self) -> &RuntimeRequestBinding {
        &self.binding
    }
    #[must_use]
    pub const fn kind(&self) -> RuntimeRequestKind {
        self.kind
    }
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeResponse {
    Pending,
    QuestionAnswer(String),
    Unsupported,
    H3AttentionUnavailable,
    Expired,
}
impl RuntimeResponse {
    /// # Errors
    /// A question answer cannot stand in for approval or contain an unbounded payload.
    pub fn validate(&self, request: &RuntimeRequest) -> Result<(), AgentError> {
        if *self == Self::Pending {
            return Err(AgentError::Protocol(
                "pending_is_not_a_final_response".into(),
            ));
        }
        if let Self::QuestionAnswer(text) = self
            && (request.kind != RuntimeRequestKind::Question || text.len() > 16 * 1024)
        {
            return Err(AgentError::Protocol(
                "runtime_response_kind_or_bounds".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResponseControl {
    request: RuntimeRequest,
    response: RuntimeResponse,
}
impl RuntimeResponseControl {
    /// # Errors
    /// Refuses a response with the wrong type or size before queue admission.
    pub fn new(request: RuntimeRequest, response: RuntimeResponse) -> Result<Self, AgentError> {
        response.validate(&request)?;
        Ok(Self { request, response })
    }
    #[must_use]
    pub const fn request(&self) -> &RuntimeRequest {
        &self.request
    }
    #[must_use]
    pub const fn response(&self) -> &RuntimeResponse {
        &self.response
    }
}
