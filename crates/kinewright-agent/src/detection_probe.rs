//! What Kinewright keeps from a detection probe (`--version`, `auth status`,
//! `login status`, `whoami`, `about`). Probe text is provider-written, and a
//! probe may print it on stderr. Only shape-validated values leave this
//! layer: a bounded version token, an authentication enum, and a fixed
//! subscription tier label. No probe byte reaches an error, event, status,
//! log, eval artifact or the UI.

use std::process::Command as ProcessCommand;

use crate::child_process::process_output;

/// Longest numeric component kept. Nine digits always fit in a `u64`.
const COMPONENT_DIGITS: usize = 9;
/// Pre-release words a version token may carry, as in `0.148.1-beta.2`.
const PRE_RELEASE_WORDS: [&str; 9] = [
    "alpha", "beta", "rc", "dev", "pre", "preview", "nightly", "canary", "next",
];
/// Longest pre-release number kept.
const PRE_RELEASE_DIGITS: usize = 6;

/// Run a version probe and keep only its version token.
pub(crate) fn probe_version(command: ProcessCommand, arguments: &[&str]) -> Option<String> {
    process_output(command, arguments)
        .as_deref()
        .and_then(version_token)
}

/// The first version-shaped token in probe output: an optional `v`, two to
/// four dot-separated numbers of at most nine digits, and optionally `-` and
/// an allowlisted pre-release word with an optional short number. A
/// malformed pre-release tag is dropped and the numbers are kept.
pub(crate) fn version_token(output: &str) -> Option<String> {
    output
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .find_map(version_shape)
}

fn version_shape(word: &str) -> Option<String> {
    let word = word.trim_end_matches(['.', '-']);
    let word = word.strip_prefix(['v', 'V']).unwrap_or(word);
    let (core, pre) = match word.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (word, None),
    };
    let parts = core.split('.').collect::<Vec<_>>();
    let numeric = |part: &&str| {
        !part.is_empty()
            && part.len() <= COMPONENT_DIGITS
            && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !(2..=4).contains(&parts.len()) || !parts.iter().all(numeric) {
        return None;
    }
    Some(match pre.and_then(pre_release) {
        Some(pre) => format!("{core}-{pre}"),
        None => core.to_owned(),
    })
}

fn pre_release(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let word = PRE_RELEASE_WORDS
        .iter()
        .filter(|word| lower.starts_with(*word))
        .max_by_key(|word| word.len())?;
    let rest = &lower[word.len()..];
    let number = rest.strip_prefix('.').unwrap_or(rest);
    let valid = number.is_empty()
        || (number.len() <= PRE_RELEASE_DIGITS && number.bytes().all(|byte| byte.is_ascii_digit()));
    valid.then_some(lower)
}

/// The first three numbers of a version token, with missing ones as zero.
pub(crate) fn version_triplet(token: &str) -> (u64, u64, u64) {
    let core = token.split('-').next().unwrap_or(token);
    let mut numbers = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));
    (
        numbers.next().unwrap_or(0),
        numbers.next().unwrap_or(0),
        numbers.next().unwrap_or(0),
    )
}

/// A fixed label for a reported subscription tier. The provider's own name
/// is ignored, so `Devin Pro` and `pro` are both `Pro`. Anything else is
/// `unknown`.
pub(crate) fn tier_label(raw: &str) -> &'static str {
    let lower = raw.to_ascii_lowercase();
    let key = lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '+'))
        .filter(|word| !word.is_empty() && !matches!(*word, "devin" | "cursor"))
        .collect::<String>();
    match key.as_str() {
        "free" => "Free",
        "hobby" => "Hobby",
        "pro" => "Pro",
        "pro+" | "proplus" => "Pro+",
        "plus" => "Plus",
        "ultra" => "Ultra",
        "max" => "Max",
        "core" => "Core",
        "team" | "teams" => "Teams",
        "business" => "Business",
        "enterprise" => "Enterprise",
        _ => "unknown",
    }
}

/// C5 fix 6 witness. A fake provider executable answers every detection
/// probe with a synthetic credential-looking value, once on stderr with stdout
/// empty and once on stdout. Each probe runs through the production probe
/// function, and the witness returns everything Kinewright keeps.
#[cfg(feature = "eval-harness")]
pub mod witness {
    use kinewright_core::{AuthenticationStatus, HarnessId, HarnessInfo};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command as ProcessCommand,
    };

    /// The synthetic value the fake executable prints. Never a real credential.
    pub const PROBE_SECRET: &str = "synthetic-secret";

    /// What Kinewright kept from one fake executable.
    #[derive(Debug)]
    pub struct ProbeWitness {
        /// `stderr` (stdout empty) or `stdout`.
        pub mode: &'static str,
        /// The harness records the detectors would publish.
        pub infos: Vec<HarnessInfo>,
        /// The Codex start-session refusal.
        pub codex_refusal: Option<kinewright_core::AgentError>,
        /// Each eval environment stamp, as written to an eval artifact.
        pub eval_stamps: Vec<String>,
    }

    #[cfg(unix)]
    fn write_fake(directory: &Path, mode: &str) -> PathBuf {
        use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
        let to = if mode == "stderr" { " >&2" } else { "" };
        let script = format!(
            "#!/bin/sh\nS={PROBE_SECRET}\ncase \"$1\" in\n\
             --version|version) printf '%s\\n' \"tool 0.146.0-$S\" \"fixture: refresh_token=$S\"{to};;\n\
             auth) printf '%s\\n' \"Logged in as refresh_token=$S\" \"Tier: refresh_token=$S\"{to};;\n\
             login) printf '%s\\n' \"Logged in using ChatGPT refresh_token=$S\"{to};;\n\
             status) printf '%s\\n' '{{\"status\":\"authenticated\",\"account\":\"refresh_token={PROBE_SECRET}\"}}'{to};;\n\
             about) printf '%s\\n' '{{\"subscriptionTier\":\"refresh_token={PROBE_SECRET}\"}}'{to};;\n\
             *) printf '%s\\n' \"refresh_token=$S\"{to};;\n\
             esac\nexit 0\n"
        );
        let path = directory.join(format!("fake-{mode}"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
            .expect("the fake provider is writable");
        file.write_all(script.as_bytes()).unwrap();
        file.sync_all().unwrap();
        path
    }

    #[cfg(windows)]
    fn write_fake(directory: &Path, mode: &str) -> PathBuf {
        let to = if mode == "stderr" { " 1>&2" } else { "" };
        let script = format!(
            "@echo off\r\nset S={PROBE_SECRET}\r\n\
             if \"%~1\"==\"--version\" goto v\r\nif \"%~1\"==\"version\" goto v\r\n\
             if \"%~1\"==\"auth\" goto a\r\nif \"%~1\"==\"login\" goto l\r\n\
             if \"%~1\"==\"status\" goto s\r\nif \"%~1\"==\"about\" goto b\r\n\
             echo refresh_token=%S%{to}\r\nexit /b 0\r\n\
             :v\r\necho tool 0.146.0-%S%{to}\r\necho fixture: refresh_token=%S%{to}\r\nexit /b 0\r\n\
             :a\r\necho Logged in as refresh_token=%S%{to}\r\necho Tier: refresh_token=%S%{to}\r\nexit /b 0\r\n\
             :l\r\necho Logged in using ChatGPT refresh_token=%S%{to}\r\nexit /b 0\r\n\
             :s\r\necho {{\"status\":\"authenticated\",\"account\":\"refresh_token=%S%\"}}{to}\r\nexit /b 0\r\n\
             :b\r\necho {{\"subscriptionTier\":\"refresh_token=%S%\"}}{to}\r\nexit /b 0\r\n"
        );
        let path = directory.join(format!("fake-{mode}.cmd"));
        fs::write(&path, script).expect("the fake provider is writable");
        path
    }

    /// Wait out a transient "text file busy" from a sibling test's fork.
    fn ready(fake: &Path) {
        for _ in 0..200 {
            if ProcessCommand::new(fake).arg("--version").output().is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the fake provider never became runnable");
    }

    fn info(id: &str, executable: &Path, version: Option<String>) -> HarnessInfo {
        HarnessInfo {
            id: HarnessId::new(id),
            executable: executable.to_owned(),
            version,
            authentication: AuthenticationStatus::Unknown,
            subscription_tier: None,
        }
    }

    /// Run every probe kind against both fake executables.
    ///
    /// # Panics
    ///
    /// When the scratch directory or the fake executable cannot be created.
    /// This is a test witness, not a product path.
    #[must_use]
    pub fn detection_probe_witness() -> Vec<ProbeWitness> {
        let directory = crate::child_process::create_scratch_directory("probe-witness")
            .expect("a scratch directory for the fake provider");
        let mut out = Vec::new();
        for mode in ["stderr", "stdout"] {
            let fake = write_fake(&directory, mode);
            ready(&fake);
            let version =
                |arguments: &[&str]| super::probe_version(ProcessCommand::new(&fake), arguments);
            let codex = crate::drivers::CodexSpawnTarget::native(fake.clone());
            let mut infos = Vec::new();
            for id in ["copilot", "muse", "opencode", "qwen", "kimi"] {
                infos.push(info(id, &fake, version(&["--version"])));
            }
            let mut claude = info("claude-code", &fake, version(&["--version"]));
            claude.authentication = crate::drivers::claude_authentication(&fake);
            infos.push(claude);
            let mut codex_info = info(
                "codex",
                &fake,
                super::probe_version(codex.command(), &["--version"]),
            );
            codex_info.authentication = crate::drivers::codex_authentication_from_cli(&codex);
            infos.push(codex_info);
            let mut kiro = info("kiro", &fake, version(&["--version"]));
            kiro.authentication = crate::acp_drivers::kiro_authentication(&fake);
            infos.push(kiro);
            let mut devin = info("devin", &fake, version(&["version"]));
            (devin.authentication, devin.subscription_tier) =
                crate::acp_drivers::devin_status(&fake);
            infos.push(devin);
            let cursor_target = crate::cursor::CursorSpawnTarget::native(fake.clone());
            let mut cursor = info(
                "cursor",
                &fake,
                super::probe_version(cursor_target.command(), &["--version"]),
            );
            (cursor.authentication, cursor.subscription_tier) =
                crate::cursor::cursor_status(&cursor_target);
            infos.push(cursor);
            let codex_refusal = crate::drivers::codex_start_gate(&codex).err();
            let eval_stamps = infos
                .iter()
                .map(|info| {
                    serde_json::to_string(&crate::eval::EnvironmentStamp::capture(
                        Some(info),
                        info.id.0.as_str(),
                        None,
                    ))
                    .unwrap()
                })
                .collect();
            out.push(ProbeWitness {
                mode,
                infos,
                codex_refusal,
                eval_stamps,
            });
        }
        let _ = fs::remove_dir_all(&directory);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4b_c5_fix6_version_token_keeps_only_a_validated_shape() {
        for (output, token) in [
            ("codex-cli 0.166.0", Some("0.166.0")),
            ("codex-cli 0.148.1-beta.2", Some("0.148.1-beta.2")),
            ("v2.1.4 (Claude Code)", Some("2.1.4")),
            ("opencode@1.2.3", Some("1.2.3")),
            ("Version 1.2.3.", Some("1.2.3")),
            ("1.0.0-rc1", Some("1.0.0-rc1")),
            ("tool 0.146.0-synthetic-secret", Some("0.146.0")),
            (
                "refresh_token=synthetic-secret\ncodex 0.146.0",
                Some("0.146.0"),
            ),
            ("refresh_token=abc1.2def", None),
            ("1.2.3.4.5", None),
            ("1234567890.1", None),
            ("unexpected output", None),
            ("", None),
        ] {
            assert_eq!(version_token(output).as_deref(), token, "{output:?}");
        }
        assert_eq!(version_triplet("0.148.1-beta.2"), (0, 148, 1));
        assert_eq!(version_triplet("2.1"), (2, 1, 0));
    }

    #[test]
    fn s4b_c5_fix6_tier_label_is_fixed() {
        assert_eq!(tier_label("Devin Pro"), "Pro");
        assert_eq!(tier_label("pro_plus"), "Pro+");
        assert_eq!(tier_label("Enterprise"), "Enterprise");
        assert_eq!(tier_label("refresh_token=synthetic-secret"), "unknown");
        assert_eq!(tier_label(""), "unknown");
    }

    /// C5 fix 6: no detection probe carries provider bytes into what
    /// Kinewright keeps, whether the provider writes to stderr or stdout.
    #[cfg(feature = "eval-harness")]
    #[test]
    fn s4b_c5_fix6_detection_probes_keep_no_provider_bytes() {
        use super::witness::{PROBE_SECRET, detection_probe_witness};
        let witnesses = detection_probe_witness();
        assert_eq!(witnesses.len(), 2, "C5_FIX6_MODES");
        for witness in witnesses {
            let mode = witness.mode;
            assert_eq!(
                witness
                    .codex_refusal
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref(),
                Some(
                    "agent driver is unavailable: Codex 0.146.0 is older than the supported 0.147.0"
                ),
                "C5_FIX6_VERSION_ERROR {mode}"
            );
            for info in &witness.infos {
                let kept = format!("{info:?}");
                assert!(
                    !kept.contains(PROBE_SECRET),
                    "C5_FIX6_PROBE_BYTES_IN_INFO {mode} {kept}"
                );
                assert_eq!(
                    info.version.as_deref(),
                    Some("0.146.0"),
                    "C5_FIX6_VERSION_TOKEN {mode} {}",
                    info.id.0.as_str()
                );
            }
            for stamp in &witness.eval_stamps {
                assert!(
                    !stamp.contains(PROBE_SECRET),
                    "C5_FIX6_PROBE_BYTES_IN_EVAL {mode} {stamp}"
                );
            }
            let tier = |id: &str| {
                witness
                    .infos
                    .iter()
                    .find(|info| info.id.0.as_str() == id)
                    .and_then(|info| info.subscription_tier.clone())
            };
            assert_eq!(tier("devin").as_deref(), Some("unknown"), "{mode}");
            assert_eq!(tier("cursor").as_deref(), Some("unknown"), "{mode}");
            eprintln!(
                "C5_FIX6_PROBES mode={mode} refusal={:?} infos={:?}",
                witness.codex_refusal, witness.infos
            );
        }
    }
}
