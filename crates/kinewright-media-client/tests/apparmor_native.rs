#![cfg(target_os = "linux")]
//! Ruling 31's Ubuntu `AppArmor` gate. Ubuntu restricts unprivileged user
//! namespaces unless an `AppArmor` profile grants `userns` to the installed
//! helper (`packaging/linux/apparmor`). Without the profile the helper cannot
//! build its boundary, and admission must be `SandboxUnavailable` before any
//! media byte is parsed; with it, admission succeeds. CI runs this test before
//! and after installing the profile and names the expected outcome in
//! `KINEWRIGHT_MS2_USERNS_EXPECT` (`refused` or `admitted`). On a host without
//! the restriction (stock Arch) the expectation is `admitted`.
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

use kinewright_media_client::{Config, Failure, Limits, Request, Runtime, Source};
use kinewright_media_ipc::{Kind, MediaSandboxError};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

const PHASE_LIMIT: Duration = Duration::from_secs(20);
const PROBE_PHASE: &str = "apparmor_native Runtime::probe";

fn during<T>(name: &'static str, timeout: Duration, operation: impl FnOnce() -> T) -> T {
    let _deadline = deadline::start(name, timeout);
    operation()
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Input(File);
impl Source for Input {
    fn file(&self) -> &File {
        &self.0
    }
    fn identity(&self) -> [u8; 32] {
        [31; 32]
    }
    fn check(&self) -> Result<(), kinewright_core::MediaError> {
        Ok(())
    }
}

/// The expected outcome: explicit from CI, else derived from the host. A
/// restricted host without an explicit expectation fails loudly instead of
/// guessing whether the profile is installed.
fn expectation() -> &'static str {
    if let Some(value) = std::env::var_os("KINEWRIGHT_MS2_USERNS_EXPECT") {
        return match value.to_str() {
            Some("refused") => "refused",
            Some("admitted") => "admitted",
            other => {
                panic!("KINEWRIGHT_MS2_USERNS_EXPECT must be refused or admitted, not {other:?}")
            }
        };
    }
    let restricted = fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_ok_and(|value| value.trim() == "1");
    assert!(
        !restricted,
        "this host restricts unprivileged user namespaces: install the helper's AppArmor \
         profile (scripts/install-apparmor-profile.sh) and set KINEWRIGHT_MS2_USERNS_EXPECT"
    );
    "admitted"
}

#[test]
fn ms2_apparmor_userns_gate_refuses_without_the_profile_and_admits_with_it() {
    let _deadline = deadline::test();
    deadline::report_with(kinewright_media_client::diagnostics);
    let expected = expectation();
    let directory = Directory(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ms2-s1a")
            .join(format!("apparmor-{}", std::process::id())),
    );
    fs::create_dir_all(&directory.0).unwrap();
    let png = directory.0.join("source.png");
    let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
    let status = during("apparmor_native ffmpeg fixture", PHASE_LIMIT, || {
        Command::new(ffmpeg)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .args([
                "color=c=red:s=64x36:d=0.1",
                "-frames:v",
                "1",
                "-threads",
                "1",
            ])
            .arg(&png)
            .stdin(Stdio::null())
            .status()
            .unwrap()
    });
    assert!(status.success());
    let binaries = std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"),
            PathBuf::from,
        )
        .canonicalize()
        .expect("MS2 binaries: run `cargo build --workspace` first");
    let runtime = during("apparmor_native Runtime::start", PHASE_LIMIT, || {
        Runtime::start(Config::new(
            binaries.join("kinewright-media-helper"),
            binaries.join("kinewright-media-supervisor"),
            Limits::PROBE,
        ))
        .unwrap()
    });
    let result = during(PROBE_PHASE, PHASE_LIMIT, || {
        runtime.probe(Request {
            source: Box::new(Input(File::open(&png).unwrap())),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        })
    });
    println!("MS2_APPARMOR_GATE expected={expected} result={result:?}");
    match expected {
        "refused" => assert!(
            matches!(
                result,
                Err(Failure::Sandbox(MediaSandboxError::SandboxUnavailable))
            ),
            "a missing profile must refuse before parsing: {result:?}"
        ),
        _ => assert_eq!(result.unwrap().metadata().kind, Kind::Image),
    }
    during("apparmor_native Runtime::shutdown", PHASE_LIMIT, || {
        runtime.shutdown().unwrap();
    });
}

#[test]
fn blocked_probe_phase_fails_with_its_name() {
    let _deadline = deadline::test();
    deadline::report_with(kinewright_media_client::diagnostics);
    if std::env::var_os("KW_MS2_PROBE_DEADLINE_FIXTURE").is_some() {
        let (_sender, stalled) = std::sync::mpsc::channel::<()>();
        during(PROBE_PHASE, Duration::from_millis(50), || {
            stalled
                .recv_timeout(Duration::from_secs(2))
                .expect("the phase watchdog should have terminated the fixture");
        });
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "blocked_probe_phase_fails_with_its_name"])
        .env("KW_MS2_PROBE_DEADLINE_FIXTURE", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run the blocked probe phase fixture");
    assert_eq!(output.status.code(), Some(124), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("MS2_TEST_DEADLINE {PROBE_PHASE}")),
        "{output:?}"
    );
    // The dump names what is known: the client's view and this process tree.
    assert!(stderr.contains("MS2_CLIENT_VIEW"), "{output:?}");
    assert!(
        stderr.contains("MS2_TEST_PROCESS depth=0 pid=") && stderr.contains(" seccomp="),
        "{output:?}"
    );
}
