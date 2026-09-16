//! Thin ACP drivers: `OpenCode`, Qwen Code, Kimi, Kiro, and Devin.
//!
//! Each driver owns detection (executable, version, authentication), its
//! spawn arguments (model/effort flags), and its model catalog. Turns run on
//! the shared [`GenericAcpSession`](crate::acp_session::GenericAcpSession).

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::Duration,
};

use kinewright_core::{
    AgentDriver, AgentError, AgentSession, AuthenticationStatus, HarnessId, HarnessInfo,
    SessionConfig,
};
use serde_json::{Value, json};

use crate::{
    acp::AcpClient,
    acp_session::{AcpSessionSpec, GenericAcpSession, ModelSelection},
    child_process::{create_scratch_directory, process_output, process_output_unchecked},
    drivers::find_on_path,
    models::{ModelChoice, ServiceTier},
};

const ACP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Kiro's `--effort` levels, from `kiro-cli acp --help`.
const KIRO_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub const OPENCODE_SANDBOX_NOTICE: &str = "OpenCode sessions run from an empty scratch directory; the model's built-in shell, file, and web tools remain available alongside the Kinewright MCP endpoint.";
pub const QWEN_SANDBOX_NOTICE: &str = "Qwen sessions run from an empty scratch directory; the model's built-in shell, file, and web tools remain available alongside the Kinewright MCP endpoint.";
pub const KIMI_SANDBOX_NOTICE: &str = "Kimi sessions run from an empty scratch directory; the model's built-in shell, file, and web tools remain available alongside the Kinewright MCP endpoint.";
pub const KIRO_SANDBOX_NOTICE: &str = "Kiro sessions run from an empty scratch directory with tool approvals auto-accepted; the model's built-in shell, file, and web tools remain available alongside the Kinewright MCP endpoint.";
pub const DEVIN_SANDBOX_NOTICE: &str = "Devin sessions run from an empty scratch directory with MCP served from an isolated config; Devin's own shell, file, and web tools remain available alongside the Kinewright MCP endpoint.";

#[derive(Debug, Default, Clone, Copy)]
pub struct OpenCodeDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct QwenDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct KimiDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct KiroDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct DevinDriver;

impl AgentDriver for OpenCodeDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("opencode")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("opencode")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication: opencode_authentication(),
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("opencode").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        let mut command = ProcessCommand::new(executable);
        command.arg("acp");
        GenericAcpSession::start(
            command,
            &endpoint,
            cfg,
            AcpSessionSpec {
                label: "OpenCode",
                process_name: "OpenCode ACP",
                scratch_infix: "opencode",
                inline_mcp: true,
                require_http_mcp: true,
                model_selection: ModelSelection::AdvertisedOption,
                config_lease: None,
                verify_initialized: None,
            },
        )
        .map(|session| Box::new(session) as _)
    }
}

impl AgentDriver for QwenDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("qwen")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("qwen")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication: qwen_authentication(),
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("qwen").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        let mut command = ProcessCommand::new(executable);
        if let Some(model) = cfg.model.as_deref() {
            command.arg("--model").arg(model);
        }
        command.arg("--acp");
        GenericAcpSession::start(
            command,
            &endpoint,
            cfg,
            AcpSessionSpec {
                label: "Qwen",
                process_name: "Qwen Code ACP",
                scratch_infix: "qwen",
                inline_mcp: true,
                require_http_mcp: true,
                model_selection: ModelSelection::SpawnArgument,
                config_lease: None,
                verify_initialized: None,
            },
        )
        .map(|session| Box::new(session) as _)
    }
}

impl AgentDriver for KimiDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("kimi")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("kimi")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication: kimi_authentication(),
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("kimi").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        let mut command = ProcessCommand::new(executable);
        if let Some(model) = cfg.model.as_deref() {
            command.arg("--model").arg(model);
        }
        command.arg("acp");
        GenericAcpSession::start(
            command,
            &endpoint,
            cfg,
            AcpSessionSpec {
                label: "Kimi",
                process_name: "Kimi ACP",
                scratch_infix: "kimi",
                inline_mcp: true,
                require_http_mcp: true,
                model_selection: ModelSelection::SpawnArgument,
                config_lease: None,
                verify_initialized: None,
            },
        )
        .map(|session| Box::new(session) as _)
    }
}

impl AgentDriver for KiroDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("kiro")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("kiro-cli")?;
        let version = process_output(ProcessCommand::new(&executable), &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        let authentication = kiro_authentication(&executable);
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication,
            subscription_tier: None,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        if let Some(effort) = cfg.effort.as_deref()
            && !KIRO_EFFORTS.contains(&effort)
        {
            return Err(AgentError::Unavailable(format!(
                "unknown Kiro effort {effort:?}; expected one of {}",
                KIRO_EFFORTS.join(", "),
            )));
        }
        let executable = find_on_path("kiro-cli").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        let mut command = ProcessCommand::new(executable);
        command.arg("acp").arg("-a");
        if let Some(model) = cfg.model.as_deref() {
            command.arg("--model").arg(model);
        }
        if let Some(effort) = cfg.effort.as_deref() {
            command.arg("--effort").arg(effort);
        }
        GenericAcpSession::start(
            command,
            &endpoint,
            cfg,
            AcpSessionSpec {
                label: "Kiro",
                process_name: "Kiro ACP",
                scratch_infix: "kiro",
                inline_mcp: true,
                require_http_mcp: true,
                model_selection: ModelSelection::SpawnArgument,
                config_lease: None,
                verify_initialized: None,
            },
        )
        .map(|session| Box::new(session) as _)
    }
}

impl AgentDriver for DevinDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("devin")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let executable = find_on_path("devin")?;
        let version = process_output(ProcessCommand::new(&executable), &["version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        let status = process_output(ProcessCommand::new(&executable), &["auth", "status"]);
        let authentication = status
            .as_deref()
            .map_or(AuthenticationStatus::Unknown, devin_authentication_status);
        let subscription_tier = status.as_deref().and_then(devin_subscription_tier);
        Some(HarnessInfo {
            id: self.id(),
            executable,
            version,
            authentication,
            subscription_tier,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let executable = find_on_path("devin").ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        // Devin advertises no HTTP MCP capability over ACP and reads
        // `mcp_config.json` from its config directory instead, so the session
        // gets an isolated `XDG_CONFIG_HOME` containing only the Kinewright
        // endpoint. Credentials live under `XDG_DATA_HOME` and are untouched.
        let config_dir = create_devin_config_directory(&endpoint)?;
        let command = devin_command(&executable, &config_dir, cfg.model.as_deref());
        let mut session = GenericAcpSession::start(
            command,
            &endpoint,
            cfg,
            AcpSessionSpec {
                label: "Devin",
                process_name: "Devin ACP",
                scratch_infix: "devin",
                inline_mcp: false,
                require_http_mcp: false,
                model_selection: ModelSelection::SpawnArgument,
                config_lease: None,
                verify_initialized: Some(verify_devin_mcp_redirect),
            },
        )
        .inspect_err(|_| {
            let _ = fs::remove_dir_all(&config_dir);
        })?;
        session.cleanup_paths.push(config_dir);
        Ok(Box::new(session))
    }
}

/// Fail closed when Devin ignored the `XDG_CONFIG_HOME` redirect (it reports
/// the config path it actually used in `initialize._meta`): a session without
/// the Kinewright endpoint would otherwise run with no tools and no error.
fn verify_devin_mcp_redirect(initialized: &Value) -> Result<(), AgentError> {
    let expected = format!("kinewright-devin-cfg-{}", std::process::id());
    let path = initialized
        .pointer("/_meta/mcpConfigPath")
        .and_then(Value::as_str)
        .filter(|path| path.contains(&expected))
        .ok_or_else(|| {
            AgentError::Unavailable(
                "Devin did not load MCP servers from Kinewright's isolated config; this platform may not honor XDG_CONFIG_HOME"
                    .to_owned(),
            )
        })?;
    // The path alone only says the redirect landed somewhere of ours. Read
    // the file Devin says it loaded and confirm it really declares the
    // Kinewright endpoint, so a stale or half-written config is refused
    // rather than run with no tools and no error.
    devin_config_declares_kinewright(Path::new(path))
        .then_some(())
        .ok_or_else(|| {
            AgentError::Unavailable(format!(
                "Devin loaded {path}, which does not declare Kinewright's MCP endpoint"
            ))
        })
}

/// `devin acp`, pointed at Kinewright's isolated config directory.
///
/// Both config roots are set, unconditionally: `devin_config_path` already
/// knows Windows Devin reads `%APPDATA%\devin`, and setting only
/// `XDG_CONFIG_HOME` made `verify_devin_mcp_redirect` fail closed on every
/// Windows start. Setting the root the host ignores costs nothing and keeps
/// this checkable on the default lane.
fn devin_command(executable: &Path, config_dir: &Path, model: Option<&str>) -> ProcessCommand {
    let mut command = ProcessCommand::new(executable);
    command.env("XDG_CONFIG_HOME", config_dir);
    command.env("APPDATA", config_dir);
    command.arg("acp");
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    command
}

fn devin_config_declares_kinewright(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .and_then(|config| {
            config
                .pointer("/mcpServers/kinewright/url")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|url| url.contains("/mcp"))
}

fn create_devin_config_directory(endpoint: &str) -> Result<PathBuf, AgentError> {
    let parent = create_scratch_directory("devin-cfg")?;
    let directory = parent.join("devin");
    fs::create_dir(&directory).map_err(|error| {
        AgentError::Harness(format!(
            "could not create the Devin config directory: {error}"
        ))
    })?;
    let org_id = devin_org_id().unwrap_or_default();
    let config = if org_id.is_empty() {
        json!({})
    } else {
        json!({"devin": {"org_id": org_id}})
    };
    let mcp_config = json!({
        "mcpServers": {
            "kinewright": {
                "transport": "http",
                "url": endpoint,
            },
        },
    });
    for (name, value) in [("config.json", config), ("mcp_config.json", mcp_config)] {
        fs::write(
            directory.join(name),
            serde_json::to_string_pretty(&value).unwrap_or_default(),
        )
        .map_err(|error| AgentError::Harness(format!("could not write Devin's {name}: {error}")))?;
    }
    Ok(parent)
}

fn devin_org_id() -> Option<String> {
    let path = devin_config_path().map(|dir| dir.join("config.json"))?;
    let source = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&source)
        .ok()?
        .pointer("/devin/org_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn devin_config_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        return Some(dir.join("devin"));
    }
    #[cfg(windows)]
    if let Some(dir) = env::var_os("APPDATA").map(PathBuf::from) {
        return Some(dir.join("devin"));
    }
    home_dir().map(|home| home.join(".config").join("devin"))
}

/// Models advertised by an ephemeral `OpenCode` ACP session: the `model` select
/// options from `session/new`, which always match what the installed CLI and
/// its authenticated providers can run.
#[must_use]
pub fn opencode_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("opencode") else {
        return Vec::new();
    };
    ephemeral_config_options(&executable, &["acp"], true)
        .map(|options| models_from_config_options(&options))
        .unwrap_or_default()
}

/// Models advertised by an ephemeral Devin ACP session. Effort and speed are
/// part of Devin's model ids (e.g. `claude-opus-5-medium-fast`), so no effort
/// or tier axis is offered.
#[must_use]
pub fn devin_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("devin") else {
        return Vec::new();
    };
    ephemeral_config_options(&executable, &["acp"], false)
        .map(|options| models_from_config_options(&options))
        .unwrap_or_default()
}

/// Models advertised by an ephemeral Kimi ACP session, falling back to the
/// `[models.*]` tables in the user's Kimi config when the CLI is present but
/// not logged in (listing is offline; running still needs authentication).
#[must_use]
pub fn kimi_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("kimi") else {
        return Vec::new();
    };
    if let Some(options) = ephemeral_config_options(&executable, &["acp"], false) {
        let models = models_from_config_options(&options);
        if !models.is_empty() {
            return models;
        }
    }
    kimi_config_models()
}

/// Models advertised by an ephemeral Kiro ACP session. Every model carries
/// Kiro's documented `--effort` levels.
#[must_use]
pub fn kiro_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("kiro-cli") else {
        return Vec::new();
    };
    ephemeral_config_options(&executable, &["acp"], false)
        .map(|options| {
            models_from_config_options(&options)
                .into_iter()
                .map(|mut model| {
                    model.efforts = KIRO_EFFORTS
                        .iter()
                        .map(|effort| (*effort).to_owned())
                        .collect();
                    model
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Models advertised by an ephemeral Qwen ACP session.
#[must_use]
pub fn qwen_models() -> Vec<ModelChoice> {
    let Some(executable) = find_on_path("qwen") else {
        return Vec::new();
    };
    ephemeral_config_options(&executable, &["--acp"], false)
        .map(|options| models_from_config_options(&options))
        .unwrap_or_default()
}

fn models_from_config_options(config_options: &[Value]) -> Vec<ModelChoice> {
    config_options
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some("model"))
        .and_then(|option| option.get("options"))
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let id = option.get("value").and_then(Value::as_str)?;
                    let label = option.get("name").and_then(Value::as_str).unwrap_or(id);
                    Some(ModelChoice {
                        id: id.to_owned(),
                        label: label.to_owned(),
                        efforts: Vec::new(),
                        tiers: Vec::new(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Open one throwaway ACP session and read its `session/new` config options.
/// No prompt is ever sent, so discovery costs no quota.
fn ephemeral_config_options(
    executable: &Path,
    arguments: &[&str],
    pass_cwd_flag: bool,
) -> Option<Vec<Value>> {
    let scratch = create_scratch_directory("discover").ok()?;
    let mut command = ProcessCommand::new(executable);
    command.args(arguments).current_dir(&scratch);
    if pass_cwd_flag {
        command.arg("--cwd").arg(&scratch);
    }
    let client = AcpClient::spawn(command, "ACP discovery").ok()?;
    let options = client
        .request(
            "initialize",
            &json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "Kinewright",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
            ACP_REQUEST_TIMEOUT,
        )
        .ok()
        .and_then(|_| {
            client
                .request(
                    "session/new",
                    &json!({
                        "cwd": scratch.to_string_lossy(),
                        "mcpServers": [],
                    }),
                    ACP_REQUEST_TIMEOUT,
                )
                .ok()
        })
        .and_then(|new| new.get("configOptions").and_then(Value::as_array).cloned());
    client.kill();
    let _ = fs::remove_dir_all(&scratch);
    options
}

/// `[models.*]` table names from the user's Kimi config: the model ids its
/// `--model` flag accepts. Only table headers are read; keys and other
/// tables (providers, credentials) are never inspected.
fn kimi_config_models() -> Vec<ModelChoice> {
    kimi_share_dir()
        .map(|dir| dir.join("config.toml"))
        .filter(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|source| parse_kimi_config_models(&source))
        .unwrap_or_default()
}

fn parse_kimi_config_models(source: &str) -> Vec<ModelChoice> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.strip_prefix("[models.")?.strip_suffix(']')?.trim();
            if name.is_empty() || name.contains([' ', '"', '\'']) {
                return None;
            }
            Some(ModelChoice {
                id: name.to_owned(),
                label: name.to_owned(),
                efforts: Vec::new(),
                tiers: Vec::<ServiceTier>::new(),
            })
        })
        .collect()
}

fn kimi_share_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("KIMI_SHARE_DIR").map(PathBuf::from) {
        return Some(dir);
    }
    home_dir().map(|home| home.join(".kimi"))
}

fn kimi_authentication() -> AuthenticationStatus {
    let Some(share) = kimi_share_dir() else {
        return AuthenticationStatus::Unknown;
    };
    if share
        .join("credentials")
        .read_dir()
        .is_ok_and(|mut entries| entries.any(|entry| entry.is_ok()))
    {
        return AuthenticationStatus::Authenticated;
    }
    // API-key setups keep the key in config.toml; only its presence (never
    // its value) is inspected.
    let has_key = share
        .join("config.toml")
        .is_file()
        .then(|| fs::read_to_string(share.join("config.toml")).ok())
        .flatten()
        .is_some_and(|source| {
            source.lines().any(|line| {
                let line = line.trim();
                line.starts_with("api_key")
                    && line.split_once('=').is_some_and(|(_, value)| {
                        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
                        !value.is_empty()
                    })
            })
        });
    if has_key {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unknown
    }
}

/// Whether `path` holds JSON with at least one member. Several harnesses
/// keep their credentials in such a file and the only safe probe is that
/// something is in it — never what.
fn json_object_is_populated(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .is_some_and(|value| value.as_object().is_some_and(|map| !map.is_empty()))
}

fn opencode_authentication() -> AuthenticationStatus {
    // `~/.local/share/opencode/auth.json` maps provider ids to stored
    // credentials; any entry means a provider is logged in.
    let has_credentials = opencode_auth_paths()
        .iter()
        .any(|path| json_object_is_populated(path));
    if has_credentials {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unknown
    }
}

fn opencode_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        paths.push(dir.join("opencode").join("auth.json"));
    }
    if let Some(home) = home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("auth.json"),
        );
    }
    #[cfg(windows)]
    for variable in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(dir) = env::var_os(variable).map(PathBuf::from) {
            paths.push(dir.join("opencode").join("auth.json"));
        }
    }
    paths
}

fn qwen_authentication() -> AuthenticationStatus {
    // Presence is not enough: a zero-byte or truncated `oauth_creds.json`
    // was reporting Authenticated while every other harness degraded to
    // Unknown. The values are never read, only that there are some.
    if home_dir()
        .is_some_and(|home| json_object_is_populated(&home.join(".qwen").join("oauth_creds.json")))
    {
        return AuthenticationStatus::Authenticated;
    }
    if env::var_os("DASHSCOPE_API_KEY").is_some_and(|key| !key.is_empty()) {
        return AuthenticationStatus::Authenticated;
    }
    AuthenticationStatus::Unknown
}

fn kiro_authentication(executable: &Path) -> AuthenticationStatus {
    // `whoami` answers "Not logged in" on stdout and (on the installed
    // build) still exits 0, so the output is read regardless of status:
    // the text is the definitive answer, the exit code is not.
    let Some(output) = process_output_unchecked(ProcessCommand::new(executable), &["whoami"])
    else {
        return AuthenticationStatus::Unknown;
    };
    if output.to_ascii_lowercase().contains("not logged in") {
        AuthenticationStatus::Unauthenticated
    } else if output.trim().is_empty() {
        AuthenticationStatus::Unknown
    } else {
        AuthenticationStatus::Authenticated
    }
}

fn devin_authentication_status(output: &str) -> AuthenticationStatus {
    let output = output.to_ascii_lowercase();
    if output.contains("not logged in") {
        AuthenticationStatus::Unauthenticated
    } else if output.contains("logged in") {
        AuthenticationStatus::Authenticated
    } else {
        AuthenticationStatus::Unknown
    }
}

fn devin_subscription_tier(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let value = line.trim().strip_prefix("Tier:")?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_ids_match_the_public_harness_names() {
        assert_eq!(OpenCodeDriver.id(), HarnessId::new("opencode"));
        assert_eq!(QwenDriver.id(), HarnessId::new("qwen"));
        assert_eq!(KimiDriver.id(), HarnessId::new("kimi"));
        assert_eq!(KiroDriver.id(), HarnessId::new("kiro"));
        assert_eq!(DevinDriver.id(), HarnessId::new("devin"));
    }

    #[test]
    fn model_select_options_map_to_untiered_choices() {
        let options = vec![
            json!({"id": "mode", "options": [{"value": "build"}]}),
            json!({"id": "model", "options": [
                {"value": "opencode-go/glm-5.3", "name": "OpenCode Go/GLM-5.3"},
                {"value": "opencode/big-pickle"},
            ]}),
        ];
        assert_eq!(
            models_from_config_options(&options),
            vec![
                ModelChoice {
                    id: "opencode-go/glm-5.3".to_owned(),
                    label: "OpenCode Go/GLM-5.3".to_owned(),
                    efforts: Vec::new(),
                    tiers: Vec::new(),
                },
                ModelChoice {
                    id: "opencode/big-pickle".to_owned(),
                    label: "opencode/big-pickle".to_owned(),
                    efforts: Vec::new(),
                    tiers: Vec::new(),
                },
            ]
        );
        assert!(models_from_config_options(&[]).is_empty());
    }

    #[test]
    fn devin_status_parses_authentication_and_tier() {
        assert_eq!(
            devin_authentication_status("Logged in (via Devin)."),
            AuthenticationStatus::Authenticated
        );
        assert_eq!(
            devin_authentication_status("Not logged in"),
            AuthenticationStatus::Unauthenticated
        );
        assert_eq!(
            devin_authentication_status("unexpected"),
            AuthenticationStatus::Unknown
        );
        assert_eq!(
            devin_subscription_tier("Account:\n  Tier:              Devin Pro\n"),
            Some("Devin Pro".to_owned())
        );
        assert_eq!(devin_subscription_tier("no tier here"), None);
    }

    #[test]
    fn devin_redirect_verification_requires_the_isolated_config_path() {
        // A path outside Kinewright's isolated directory is refused before
        // the file is ever read; the accepted direction, which now also
        // reads the endpoint out of that file, is covered by
        // `devin_verification_reads_the_endpoint_not_just_the_path`.
        assert!(
            verify_devin_mcp_redirect(&json!({
                "_meta": {"mcpConfigPath": "/home/user/.config/devin/mcp_config.json"},
            }))
            .is_err()
        );
        assert!(verify_devin_mcp_redirect(&json!({})).is_err());
    }

    #[test]
    fn kimi_config_lists_only_model_table_names() {
        let source = "[providers.kimi]\ntype = \"kimi\"\n\n[models.kimi-k3]\nprovider = \"kimi\"\n\n[models.kimi-k2.7-code]\nprovider = \"kimi\"\n";
        let ids: Vec<_> = parse_kimi_config_models(source)
            .iter()
            .map(|model| model.id.clone())
            .collect();
        assert_eq!(ids, ["kimi-k3", "kimi-k2.7-code"]);
        assert!(parse_kimi_config_models("[models.bad name]\n").is_empty());
    }

    #[test]
    fn kiro_effort_ladder_matches_the_cli_help() {
        assert_eq!(KIRO_EFFORTS, ["low", "medium", "high", "xhigh", "max"]);
    }

    /// Devin reads its MCP config from disk rather than the ACP handshake,
    /// so the redirect has to reach whichever config root the host honours.
    #[test]
    fn a_devin_session_redirects_both_config_roots() {
        let config_dir = Path::new("kinewright-devin-cfg-test");
        let command = devin_command(Path::new("devin"), config_dir, Some("devin-fast"));
        let environment: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for root in ["XDG_CONFIG_HOME", "APPDATA"] {
            assert!(
                environment.contains(&(
                    root.to_owned(),
                    Some(config_dir.to_string_lossy().into_owned())
                )),
                "{root} was not redirected: {environment:?}"
            );
        }
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(arguments, ["acp", "--model", "devin-fast"]);
    }

    /// The reported config path proves only that the redirect landed
    /// somewhere of ours; the file it names has to carry the endpoint.
    #[test]
    fn devin_verification_reads_the_endpoint_not_just_the_path() {
        let scratch = create_scratch_directory("devin-verify-test").unwrap();
        let declared = scratch.join(format!(
            "kinewright-devin-cfg-{}-mcp.json",
            std::process::id()
        ));
        let reported = |path: &Path| json!({"_meta": {"mcpConfigPath": path.to_string_lossy()}});

        // A path that is not ours at all.
        let stranger = scratch.join("someone-elses.json");
        fs::write(&stranger, "{}").unwrap();
        assert!(verify_devin_mcp_redirect(&reported(&stranger)).is_err());
        assert!(verify_devin_mcp_redirect(&json!({})).is_err());

        // Our path, but a config that declares no Kinewright endpoint.
        fs::write(&declared, r#"{"mcpServers":{}}"#).unwrap();
        let refused = verify_devin_mcp_redirect(&reported(&declared)).unwrap_err();
        assert!(
            matches!(&refused, AgentError::Unavailable(message)
                if message.contains("does not declare Kinewright")),
            "{refused:?}"
        );

        // Our path, carrying the endpoint the session was started with.
        fs::write(
            &declared,
            r#"{"mcpServers":{"kinewright":{"transport":"http","url":"http://127.0.0.1:43123/mcp"}}}"#,
        )
        .unwrap();
        assert!(verify_devin_mcp_redirect(&reported(&declared)).is_ok());

        fs::remove_dir_all(scratch).unwrap();
    }

    /// A zero-byte or truncated credentials file is not a login.
    #[test]
    fn empty_or_broken_credentials_are_not_authentication() {
        let scratch = create_scratch_directory("qwen-creds-test").unwrap();
        let creds = scratch.join("oauth_creds.json");
        assert!(!json_object_is_populated(&creds), "a missing file");
        fs::write(&creds, "").unwrap();
        assert!(!json_object_is_populated(&creds), "a zero-byte file");
        fs::write(&creds, r#"{"access_token": "#).unwrap();
        assert!(!json_object_is_populated(&creds), "a truncated file");
        fs::write(&creds, "{}").unwrap();
        assert!(!json_object_is_populated(&creds), "an empty object");
        fs::write(&creds, r#"{"access_token":"x"}"#).unwrap();
        assert!(json_object_is_populated(&creds));
        fs::remove_dir_all(scratch).unwrap();
    }
}
