//! The harness registry: one table naming every agent CLI Kinewright drives.
//!
//! The key is the single spelling shared by the app's picker memory, the
//! eval binary's `--harness` option, and each driver's [`HarnessId`], so a
//! harness that is registered here is reachable from all three. Callers that
//! need a driver construct it through [`harness_driver`] rather than writing
//! their own ten-arm match.

use kinewright_core::AgentDriver;

use crate::{
    acp_drivers::{DevinDriver, KimiDriver, KiroDriver, OpenCodeDriver, QwenDriver},
    copilot::CopilotDriver,
    cursor::CursorAcpDriver,
    drivers::{ClaudeCodeDriver, CodexDriver},
    muse::MuseDriver,
};

/// Every harness Kinewright ships, in the app's picker order.
pub const HARNESS_KEYS: [&str; 10] = [
    "claude-code",
    "codex",
    "cursor",
    "muse",
    "opencode",
    "qwen",
    "kimi",
    "kiro",
    "devin",
    "copilot",
];

/// The driver for one harness key, or `None` when the key is not registered.
///
/// The returned driver is stateless: it only detects the CLI and starts
/// sessions, so constructing one is cheap and never touches the filesystem.
#[must_use]
pub fn harness_driver(key: &str) -> Option<Box<dyn AgentDriver>> {
    let driver: Box<dyn AgentDriver> = match key {
        "claude-code" => Box::new(ClaudeCodeDriver),
        "codex" => Box::new(CodexDriver),
        "cursor" => Box::new(CursorAcpDriver),
        "muse" => Box::new(MuseDriver),
        "opencode" => Box::new(OpenCodeDriver),
        "qwen" => Box::new(QwenDriver),
        "kimi" => Box::new(KimiDriver),
        "kiro" => Box::new(KiroDriver),
        "devin" => Box::new(DevinDriver),
        "copilot" => Box::new(CopilotDriver),
        _ => return None,
    };
    Some(driver)
}

#[cfg(test)]
mod tests {
    use kinewright_core::HarnessId;

    use super::*;

    /// A harness that is listed but unbuildable, or buildable under a key
    /// that disagrees with its own `HarnessId`, fails here.
    #[test]
    fn every_registered_key_builds_a_driver_that_answers_to_it() {
        for key in HARNESS_KEYS {
            let driver = harness_driver(key).unwrap_or_else(|| panic!("no driver for {key}"));
            assert_eq!(driver.id(), HarnessId::new(key), "{key} reports another id");
        }
    }

    #[test]
    fn the_registry_has_no_duplicate_keys_and_refuses_unknown_ones() {
        let mut keys = HARNESS_KEYS.to_vec();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), HARNESS_KEYS.len(), "HARNESS_KEYS repeats a key");
        assert!(harness_driver("claude").is_none());
        assert!(harness_driver("").is_none());
    }
}
