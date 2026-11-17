#![cfg(target_os = "linux")]
//! Ruling 31's Ubuntu `AppArmor` gate. Ubuntu restricts unprivileged user
//! namespaces unless an `AppArmor` profile grants `userns` to the installed
//! helper (`packaging/linux/apparmor`). Without the profile the helper cannot
//! build its boundary, and admission must be `SandboxUnavailable` before any
//! media byte is parsed; with it, admission succeeds. CI runs this test before
//! and after installing the profile and names the expected outcome in
//! `KINEWRIGHT_MS2_USERNS_EXPECT` (`refused` or `admitted`). On a host without
//! the restriction (stock Arch) the expectation is `admitted`.
use kinewright_media_client::{Config, Failure, Limits, Request, Runtime, Source};
use kinewright_media_ipc::{Kind, MediaSandboxError};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, atomic::AtomicBool},
};

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
    let expected = expectation();
    let directory = Directory(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ms2-s1a")
            .join(format!("apparmor-{}", std::process::id())),
    );
    fs::create_dir_all(&directory.0).unwrap();
    let png = directory.0.join("source.png");
    let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
    let status = Command::new(ffmpeg)
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
        .unwrap();
    assert!(status.success());
    let binaries = std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"),
            PathBuf::from,
        )
        .canonicalize()
        .expect("MS2 binaries: run `cargo build --workspace` first");
    let runtime = Runtime::start(Config::new(
        binaries.join("kinewright-media-helper"),
        binaries.join("kinewright-media-supervisor"),
        Limits::PROBE,
    ))
    .unwrap();
    let result = runtime.probe(Request {
        source: Box::new(Input(File::open(&png).unwrap())),
        video: None,
        audio: None,
        cancel: Arc::new(AtomicBool::new(false)),
        explicit_retry: false,
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
    runtime.shutdown().unwrap();
}
