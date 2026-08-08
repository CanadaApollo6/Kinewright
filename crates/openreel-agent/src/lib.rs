use std::{env, path::PathBuf};

use openreel_core::{
    AgentDriver, AgentError, AgentSession, AuthenticationStatus, HarnessId, HarnessInfo,
    SessionConfig,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexDriver;

#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeDriver;

impl AgentDriver for CodexDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("codex")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        detect("codex", self.id())
    }

    fn start_session(&self, _cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        Err(AgentError::NotImplemented)
    }
}

impl AgentDriver for ClaudeCodeDriver {
    fn id(&self) -> HarnessId {
        HarnessId::new("claude-code")
    }

    fn detect(&self) -> Option<HarnessInfo> {
        detect("claude", self.id())
    }

    fn start_session(&self, _cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
        Err(AgentError::NotImplemented)
    }
}

fn detect(executable: &str, id: HarnessId) -> Option<HarnessInfo> {
    find_on_path(executable).map(|executable| HarnessInfo {
        id,
        executable,
        version: None,
        // M0 only proves PATH detection; authentication probing belongs to the
        // protocol-aware driver work in M3.
        authentication: AuthenticationStatus::Unknown,
    })
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };

    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let candidate = directory.join(format!("{executable}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_ids_match_the_public_harness_names() {
        assert_eq!(CodexDriver.id(), HarnessId::new("codex"));
        assert_eq!(ClaudeCodeDriver.id(), HarnessId::new("claude-code"));
    }
}

