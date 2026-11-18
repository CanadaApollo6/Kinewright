#![cfg(target_os = "linux")]
//! Full-stack escape witnesses: the real app runtime → supervisor → shim →
//! helper path, with a deliberately malicious helper (the witness executable's
//! `escape` stage) that attempts one escape at its first decoder packet, under
//! the complete boundary. Each attack must end the helper by a seccomp kill
//! (`Crashed`), and host canaries and the source must be unchanged. An errno
//! denial by a lower layer replies `UnsupportedMedia`, and a successful escape
//! replies `ProtocolViolation`, so removing a guard changes the outcome. The
//! `control` attack attempts nothing and must be admitted.
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

#[cfg(feature = "slow-tests")]
mod support {
    use kinewright_media_client::{Config, Failure, Limits, Request, Runtime, Source};
    use kinewright_media_ipc::{Kind, MediaSandboxError};
    use std::{
        fs::{self, File},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::{Arc, atomic::AtomicBool},
    };

    pub struct Directory(pub PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Input(File, u8);
    impl Source for Input {
        fn file(&self) -> &File {
            &self.0
        }
        fn identity(&self) -> [u8; 32] {
            [self.1; 32]
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            Ok(())
        }
    }
    fn binaries() -> PathBuf {
        std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR")
            .map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"),
                PathBuf::from,
            )
            .canonicalize()
            .unwrap()
    }
    /// A scratch directory with a PNG source and a host canary.
    pub fn workspace() -> (Directory, PathBuf, PathBuf) {
        let directory = Directory(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ms2-s1a")
                .join(format!("escape-{}", std::process::id())),
        );
        fs::create_dir_all(&directory.0).unwrap();
        let png = directory.0.join("source.png");
        let ffmpeg =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
        assert!(
            Command::new(ffmpeg)
                .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
                .args([
                    "color=c=red:s=128x72:d=0.1",
                    "-frames:v",
                    "1",
                    "-threads",
                    "1"
                ])
                .arg(&png)
                .stdin(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        let canary = directory.0.join("canary.txt");
        fs::write(&canary, b"ms2 canary\n").unwrap();
        fs::set_permissions(&canary, fs::Permissions::from_mode(0o600)).unwrap();
        (directory, png, canary)
    }
    /// Fixture child: one runtime per process, so each attack gets a fresh
    /// one and earlier crashes never pause or quarantine its admission.
    pub fn fixture(case: &str) {
        let mut parts = case.splitn(4, '=');
        let (Some(directory), Some(identity), Some(attack), Some(target)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            panic!("bad escape case")
        };
        let directory = Path::new(directory);
        let (result, degraded) = run(
            directory,
            &directory.join("source.png"),
            attack,
            target,
            identity.parse().unwrap(),
        );
        println!(
            "MS2_ESCAPE_RESULT {} {result:?}",
            if degraded { "degraded" } else { "primary" }
        );
    }
    /// Run one attack in a fixture child and return its result line.
    /// `degraded` points the session bus at a path with no listener, so the
    /// runtime cannot reach a user manager and confines without a scope.
    pub fn attack(
        directory: &Directory,
        degraded: bool,
        attack: &str,
        target: &str,
        identity: u8,
    ) -> String {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "escape_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(
                "KW_ESCAPE_CASE",
                format!("{}={identity}={attack}={target}", directory.0.display()),
            )
            .stdin(Stdio::null());
        if degraded {
            command.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", directory.0.join("no-bus").display()),
            );
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{attack} {target}: {output:?}");
        stdout
            .lines()
            .find_map(|line| {
                line.split_once("MS2_ESCAPE_RESULT ")
                    .map(|(_, result)| result)
            })
            .unwrap_or_else(|| panic!("{attack} {target}: no result: {stdout}"))
            .to_owned()
    }
    fn run(
        directory: &Path,
        png: &Path,
        attack: &str,
        target: &str,
        identity: u8,
    ) -> (Result<Kind, Failure>, bool) {
        assert!(!target.contains('\''));
        let binaries = binaries();
        let executable = binaries.join("kinewright-media-helper-witness");
        assert!(
            executable.is_file(),
            "build the slow-feature witness binary first"
        );
        let wrapper = directory.join(format!("helper-{identity}"));
        let executable = executable.to_str().unwrap().replace('\'', "'\\''");
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec '{executable}' \"$@\" 'escape={attack}={target}'\n"),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::start(Config::new(
            wrapper.canonicalize().unwrap(),
            binaries.join("kinewright-media-supervisor"),
            Limits::PROBE,
        ))
        .unwrap();
        let result = runtime
            .probe(Request {
                source: Box::new(Input(File::open(png).unwrap(), identity)),
                video: None,
                audio: None,
                cancel: Arc::new(AtomicBool::new(false)),
                explicit_retry: false,
            })
            .map(|probe| probe.metadata().kind);
        let degraded = runtime.snapshot().degraded;
        runtime.shutdown().unwrap();
        assert_eq!(
            (runtime.snapshot().active, runtime.snapshot().pending),
            (0, 0)
        );
        (result, degraded)
    }
    pub fn killed(line: &str) -> bool {
        line.ends_with(&format!(
            "{:?}",
            Err::<Kind, _>(Failure::Sandbox(MediaSandboxError::Crashed))
        ))
    }
}

#[test]
fn escape_fixture() {
    #[cfg(feature = "slow-tests")]
    if let Ok(case) = std::env::var("KW_ESCAPE_CASE") {
        support::fixture(&case);
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn full_stack_escapes_are_killed_and_canaries_unchanged() {
    let _deadline = deadline::test();
    #[cfg(feature = "slow-tests")]
    {
        use support::{attack, killed, workspace};
        rustix::process::setrlimit(
            rustix::process::Resource::Core,
            rustix::process::Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        )
        .unwrap();
        let (directory, png, canary) = workspace();
        let canary_before = std::fs::read(&canary).unwrap();
        let source_before = std::fs::read(&png).unwrap();
        let app = std::process::id().to_string();
        let canary = canary.to_str().unwrap();
        let source = png.to_str().unwrap();
        let parent_memory = format!("/proc/{app}/mem");
        let inherited_source = format!(
            "/proc/self/fd/{}",
            kinewright_job_sys::media_boundary::SOURCE
        );
        let attacks = [
            ("read", canary),
            ("write", canary),
            ("read", source),
            ("write", source),
            ("write", inherited_source.as_str()),
            ("read", parent_memory.as_str()),
            ("socket", "inet"),
            ("socket", "unix"),
            ("socket", "netlink"),
            ("socket", "vsock"),
            ("spawn", "/bin/sh"),
            ("signal", app.as_str()),
        ];
        let mut failures = Vec::new();
        let mut identity = 0_u8;
        for (degraded, mode) in [(false, "primary"), (true, "degraded")] {
            identity += 1;
            let control = attack(&directory, degraded, "control", "", identity);
            println!("MS2_ESCAPE requested={mode} attack=control result={control}");
            assert!(
                control.starts_with(&format!("{mode} "))
                    || (!degraded && control.starts_with("degraded ")),
                "requested {mode}, ran {control}"
            );
            assert!(
                control.ends_with(&format!(
                    "{:?}",
                    Ok::<_, ()>(kinewright_media_ipc::Kind::Image)
                )),
                "the attack harness itself must admit: {control}"
            );
            for (name, target) in attacks {
                identity += 1;
                let result = attack(&directory, degraded, name, target, identity);
                println!(
                    "MS2_ESCAPE requested={mode} attack={name} target={target} result={result}"
                );
                if !killed(&result) {
                    failures.push(format!("{mode} {name} {target}: {result}"));
                }
            }
        }
        assert_eq!(
            std::fs::read(&png).unwrap(),
            source_before,
            "source changed"
        );
        assert_eq!(
            std::fs::read(canary).unwrap(),
            canary_before,
            "canary changed"
        );
        assert!(failures.is_empty(), "escapes not killed: {failures:#?}");
    }
}
