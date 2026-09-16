//! Cursor Agent integration over the Agent Client Protocol (ACP).
//!
//! Turns run on the shared [`GenericAcpSession`](crate::acp_session). Cursor
//! differs from the other ACP harnesses in exactly one way, and it is a spec
//! field rather than a second runtime: `session/set_config_option` persists
//! CLI-wide even over ACP, so the opening configuration is snapshotted,
//! leased to one active turn through [`CURSOR_CONFIG_LEASED`], and restored
//! on completion, failure, interrupt and drop.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::atomic::AtomicBool,
    time::Duration,
};

use kinewright_core::{
    AgentDriver, AgentError, AgentSession, AuthenticationStatus, HarnessId, HarnessInfo,
    SessionConfig,
};
use serde_json::{Value, json};

use crate::{
    acp::AcpClient,
    acp_session::{AcpSessionSpec, GenericAcpSession, ModelSelection, acp_initialize},
    child_process::process_output,
    drivers::find_on_path,
    models::{ModelChoice, ServiceTier},
};

const ACP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Cursor persists `session/set_config_option` CLI-wide, so only one
/// Kinewright turn may hold the configuration at a time.
static CURSOR_CONFIG_LEASED: AtomicBool = AtomicBool::new(false);

/// ACP carries no per-tool allowlist, so the notice says what the other ACP
/// harnesses say: the endpoint is the only MCP server, not the only tooling.
pub const CURSOR_SANDBOX_NOTICE: &str = "Cursor sessions run from an empty scratch directory with the Kinewright MCP endpoint as their only MCP server; Cursor's own built-in tools remain available alongside it. Cursor model settings are restored after each turn.";

const CURSOR_SPEC: AcpSessionSpec = AcpSessionSpec {
    label: "Cursor",
    process_name: "Cursor Agent",
    scratch_infix: "cursor",
    inline_mcp: true,
    require_http_mcp: true,
    // Cursor has no model spawn flag, and its picker comes from
    // `cursor/list_available_models` rather than the session snapshot, so
    // the model is applied on the wire and not checked against that
    // snapshot. Setting it re-states the option ladder, which the effort
    // lookup must then read.
    model_selection: ModelSelection::LiveOption,
    config_lease: Some(&CURSOR_CONFIG_LEASED),
    verify_initialized: None,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct CursorAcpDriver;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CursorSpawnTarget {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
}

impl CursorSpawnTarget {
    fn native(executable: PathBuf) -> Self {
        Self {
            executable,
            prefix_arguments: Vec::new(),
        }
    }

    fn command(&self) -> ProcessCommand {
        let mut command = ProcessCommand::new(&self.executable);
        command.args(&self.prefix_arguments);
        command
    }
}

impl AgentDriver for CursorAcpDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("cursor")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        let target = find_cursor_spawn_target()?;
        let version = cursor_process_output(&target, &["--version"])
            .and_then(|output| output.lines().next().map(str::trim).map(str::to_owned));
        let status = cursor_process_output(&target, &["status", "--format", "json"])
            .and_then(|output| serde_json::from_str::<Value>(&output).ok());
        let authentication = status
            .as_ref()
            .map_or(AuthenticationStatus::Unknown, cursor_authentication_status);
        let subscription_tier = cursor_process_output(&target, &["about", "--format", "json"])
            .and_then(|output| serde_json::from_str::<Value>(&output).ok())
            .and_then(|about| {
                about
                    .get("subscriptionTier")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        Some(HarnessInfo {
            id: self.id(),
            executable: target.executable,
            version,
            authentication,
            subscription_tier,
        })
    }

    fn start_session(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        let target = find_cursor_spawn_target().ok_or(AgentError::NotInstalled)?;
        let endpoint = cfg.mcp_url.clone().ok_or(AgentError::MissingMcpEndpoint)?;
        let mut command = target.command();
        command.arg("acp");
        GenericAcpSession::start(command, &endpoint, cfg, CURSOR_SPEC)
            .map(|session| Box::new(session) as _)
    }
}

/// Models and per-model reasoning/speed controls advertised by the installed
/// Cursor ACP extension. Discovery is deliberately live: Cursor owns this
/// catalog and can update it independently of `Kinewright`.
#[must_use]
pub fn cursor_models() -> Vec<ModelChoice> {
    let Some(target) = find_cursor_spawn_target() else {
        return Vec::new();
    };
    cursor_catalog(&target).unwrap_or_default()
}

fn cursor_catalog(target: &CursorSpawnTarget) -> Result<Vec<ModelChoice>, AgentError> {
    let mut command = target.command();
    command.arg("acp");
    let client = AcpClient::spawn(command, CURSOR_SPEC.process_name)?;
    acp_initialize(&client, &CURSOR_SPEC)?;
    let catalog = client.request(
        "cursor/list_available_models",
        &json!({}),
        ACP_REQUEST_TIMEOUT,
    )?;
    client.kill();
    Ok(parse_cursor_models(&catalog))
}

fn parse_cursor_models(catalog: &Value) -> Vec<ModelChoice> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("value").and_then(Value::as_str)?;
            let label = model.get("name").and_then(Value::as_str).unwrap_or(id);
            let options = model
                .get("configOptions")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let efforts = options
                .iter()
                .find(|option| {
                    option.get("id").and_then(Value::as_str).is_some_and(|id| {
                        id.eq_ignore_ascii_case("effort") || id.eq_ignore_ascii_case("reasoning")
                    })
                })
                .and_then(|option| option.get("options"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("value").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
            let has_fast = options.iter().any(|option| {
                option.get("id").and_then(Value::as_str) == Some("fast")
                    && option
                        .get("options")
                        .and_then(Value::as_array)
                        .is_some_and(|values| {
                            values.iter().any(|value| {
                                value.get("value").is_some_and(|value| {
                                    value == "true" || value == &Value::Bool(true)
                                })
                            })
                        })
            });
            let tiers = if has_fast {
                vec![ServiceTier {
                    id: "true".to_owned(),
                    name: "Fast".to_owned(),
                }]
            } else {
                Vec::new()
            };
            Some(ModelChoice {
                id: id.to_owned(),
                label: label.to_owned(),
                efforts,
                tiers,
            })
        })
        .collect()
}

fn find_cursor_spawn_target() -> Option<CursorSpawnTarget> {
    let launcher = find_on_path("agent").or_else(|| find_on_path("cursor-agent"))?;
    resolve_cursor_spawn_target(&launcher)
}

fn resolve_cursor_spawn_target(launcher: &Path) -> Option<CursorSpawnTarget> {
    let is_script_shim = launcher.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy();
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("ps1")
    });
    if !is_script_shim {
        return Some(CursorSpawnTarget::native(launcher.to_owned()));
    }
    let root = launcher.parent()?;
    let direct_node = root.join("node.exe");
    let direct_entry = root.join("index.js");
    if direct_node.is_file() && direct_entry.is_file() {
        return Some(CursorSpawnTarget {
            executable: direct_node,
            prefix_arguments: vec![direct_entry.into_os_string()],
        });
    }
    let version = fs::read_dir(root.join("versions"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            entry.path().join("node.exe").is_file() && entry.path().join("index.js").is_file()
        })
        .max_by_key(|entry| cursor_version_key(&entry.file_name().to_string_lossy()))?;
    Some(CursorSpawnTarget {
        executable: version.path().join("node.exe"),
        prefix_arguments: vec![version.path().join("index.js").into_os_string()],
    })
}

fn cursor_version_key(version: &str) -> (u32, u32, u32, u32, u32, u32) {
    let numbers = version
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    (
        numbers.first().copied().unwrap_or_default(),
        numbers.get(1).copied().unwrap_or_default(),
        numbers.get(2).copied().unwrap_or_default(),
        numbers.get(3).copied().unwrap_or_default(),
        numbers.get(4).copied().unwrap_or_default(),
        numbers.get(5).copied().unwrap_or_default(),
    )
}

fn cursor_process_output(target: &CursorSpawnTarget, arguments: &[&str]) -> Option<String> {
    process_output(target.command(), arguments)
}

fn cursor_authentication_status(status: &Value) -> AuthenticationStatus {
    status
        .get("isAuthenticated")
        .or_else(|| status.get("authenticated"))
        .and_then(Value::as_bool)
        .map_or_else(
            || match status.get("status").and_then(Value::as_str) {
                Some("authenticated") => AuthenticationStatus::Authenticated,
                Some("unauthenticated") => AuthenticationStatus::Unauthenticated,
                _ => AuthenticationStatus::Unknown,
            },
            |authenticated| {
                if authenticated {
                    AuthenticationStatus::Authenticated
                } else {
                    AuthenticationStatus::Unauthenticated
                }
            },
        )
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crossbeam_channel::unbounded;
    use kinewright_core::AgentEvent;

    use super::*;
    use crate::{acp_session::translate_acp_update, child_process::create_scratch_directory};

    #[test]
    fn driver_id_matches_the_public_harness_name() {
        assert_eq!(CursorAcpDriver.id(), HarnessId::new("cursor"));
    }

    #[test]
    fn cursor_status_parses_current_and_legacy_authentication_shapes() {
        assert_eq!(
            cursor_authentication_status(&json!({"status":"authenticated","isAuthenticated":true})),
            AuthenticationStatus::Authenticated
        );
        assert_eq!(
            cursor_authentication_status(&json!({"authenticated":false})),
            AuthenticationStatus::Unauthenticated
        );
        assert_eq!(
            cursor_authentication_status(&json!({"status":"unavailable"})),
            AuthenticationStatus::Unknown
        );
    }

    #[test]
    fn cursor_catalog_maps_reasoning_and_fast_to_existing_pickers() {
        let models = parse_cursor_models(&json!({
            "models": [{
                "value": "gpt-5.6-sol",
                "name": "GPT-5.6 Sol",
                "configOptions": [
                    {"id":"reasoning","options":[{"value":"low"},{"value":"xhigh"}]},
                    {"id":"fast","options":[{"value":"false"},{"value":"true"}]}
                ]
            }]
        }));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].efforts, ["low", "xhigh"]);
        assert_eq!(
            models[0].tiers,
            [ServiceTier {
                id: "true".to_owned(),
                name: "Fast".to_owned()
            }]
        );
    }

    #[test]
    fn cursor_shim_resolves_to_the_newest_native_node_bundle() {
        let root = create_scratch_directory("cursor-test").unwrap();
        let shim = root.join("agent.cmd");
        fs::write(&shim, "@echo off\r\n").unwrap();
        for version in ["2026.07.9-old", "2026.08.11-new"] {
            let directory = root.join("versions").join(version);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("node.exe"), b"node").unwrap();
            fs::write(directory.join("index.js"), b"entry").unwrap();
        }
        let target = resolve_cursor_spawn_target(&shim).unwrap();
        assert!(
            target
                .executable
                .to_string_lossy()
                .contains("2026.08.11-new")
        );
        assert_eq!(target.prefix_arguments.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cursor_permission_result_uses_the_acp_selected_shape() {
        let outcome = json!({"outcome": "selected", "optionId": "allow-once"});
        assert_eq!(
            json!({"outcome": outcome})["outcome"]["optionId"],
            "allow-once"
        );
    }

    #[test]
    fn message_and_tool_updates_translate_to_existing_agent_events() {
        let (tx, rx) = unbounded();
        let mut names = HashMap::new();
        let mut finished = HashSet::new();
        translate_acp_update(
            &json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done"}}),
            &tx,
            &mut names,
            &mut finished,
            CURSOR_SPEC.label,
        );
        translate_acp_update(
            &json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"get_timeline_state","rawInput":{"include":"all"}}),
            &tx,
            &mut names,
            &mut finished,
            CURSOR_SPEC.label,
        );
        translate_acp_update(
            &json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","rawOutput":{"ok":true}}),
            &tx,
            &mut names,
            &mut finished,
            CURSOR_SPEC.label,
        );
        assert_eq!(rx.recv().unwrap(), AgentEvent::Text("Done".to_owned()));
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolCall { name, .. } if name == "get_timeline_state")
        );
        assert!(
            matches!(rx.recv().unwrap(), AgentEvent::ToolResult { name, .. } if name == "get_timeline_state")
        );
    }
}
