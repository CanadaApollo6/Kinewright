#![cfg(target_os = "linux")]
use kinewright_media_client::{Config, Limits, Request, Runtime, Source};
use kinewright_media_ipc::Kind;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
        [42; 32]
    }
    fn check(&self) -> Result<(), kinewright_core::MediaError> {
        Ok(())
    }
}
fn bin_dir() -> PathBuf {
    // Explicit harness configuration also supports custom Cargo target dirs;
    // no inference from Cargo's private unit-test executable layout.
    std::env::var_os("KINEWRIGHT_MS2_TEST_BIN_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"),
        PathBuf::from,
    )
}

#[test]
fn runtime_fixture() {
    let Some(case) = std::env::var_os("KW_RUNTIME_FIXTURE") else {
        return;
    };
    let directory = Directory(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ms2-s1a")
            .join(format!("runtime-{}", std::process::id())),
    );
    fs::create_dir_all(&directory.0).unwrap();
    let png = directory.0.join("source.png");
    let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
    assert!(
        Command::new(ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=128x72:d=0.1",
                "-frames:v",
                "1",
                "-threads",
                "1"
            ])
            .arg(&png)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let binaries = bin_dir().canonicalize().unwrap();
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
    let probe = result.unwrap();
    assert_eq!(probe.metadata().kind, Kind::Image);
    assert_eq!(probe.metadata().duration_frames, 1);
    let state = runtime.snapshot();
    if case == "degraded" {
        assert!(state.degraded);
    }
    println!("MS2_RUNTIME_MODE degraded={}", state.degraded);
    runtime.shutdown().unwrap();
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().pending),
        (0, 0)
    );
}
fn run_case(degraded: bool) {
    let mut bus = if degraded {
        Some(OwnedChild(
            Command::new("/usr/bin/dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        ))
    } else {
        None
    };
    let address = bus.as_mut().map(|bus| {
        let mut text = String::new();
        BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut text)
            .unwrap();
        text.trim_end().to_owned()
    });
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "runtime_fixture", "--nocapture"])
        .env(
            "KW_RUNTIME_FIXTURE",
            if degraded { "degraded" } else { "automatic" },
        )
        .stdin(Stdio::null())
        // The child's own libtest headers must not interleave with this
        // test's output; only its evidence lines are relayed below.
        .stdout(Stdio::piped());
    if let Some(address) = address {
        command.env("DBUS_SESSION_BUS_ADDRESS", address);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            let mut output = String::new();
            child
                .0
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut output)
                .unwrap();
            for line in output.lines().filter(|line| line.contains("MS2_")) {
                println!("{}", &line[line.find("MS2_").unwrap_or(0)..]);
            }
            assert!(status.success(), "runtime fixture: {status}");
            break;
        }
        assert!(
            Instant::now() < end,
            "runtime fixture exceeded native cleanup deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_png_automatic_admission_closes_and_cleans() {
    run_case(false);
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_png_degraded_admission_closes_and_cleans() {
    run_case(true);
}
