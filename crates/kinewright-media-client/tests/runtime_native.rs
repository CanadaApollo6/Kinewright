#![cfg(target_os = "linux")]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;
#[path = "../../kinewright-job-sys/test_support/ms2_diag.rs"]
mod ms2_diag;

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

#[cfg(feature = "slow-tests")]
#[path = "support/bus_cleanup.rs"]
mod bus_cleanup;

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
    let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/ffmpeg-testgen/bin/ffmpeg-testgen");
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
    #[cfg(feature = "slow-tests")]
    if case == "bus-cleanup" || case == "bus-restart" {
        bus_cleanup::exercise(&directory.0, &binaries, &png, case == "bus-restart");
        return;
    }
    ms2_diag::fixture(kinewright_media_client::diagnostics);
    let config = Config::new(
        ms2_diag::wrap(
            &directory.0,
            &binaries.join("kinewright-media-helper"),
            "helper",
            &[],
        ),
        ms2_diag::wrap(
            &directory.0,
            &binaries.join("kinewright-media-supervisor"),
            "supervisor",
            &[],
        ),
        Limits::PROBE,
    );
    #[cfg(feature = "slow-tests")]
    let Some(config) = seamed_case(&case, config, &png) else {
        return;
    };
    let runtime = Runtime::start(config).unwrap();
    let sampler = ms2_diag::sample("runtime_native");
    let result = runtime.probe(Request {
        source: Box::new(Input(File::open(&png).unwrap())),
        video: None,
        audio: None,
        cancel: Arc::new(AtomicBool::new(false)),
        explicit_retry: false,
    });
    sampler.finish(result.is_err());
    let probe = result.unwrap();
    assert_eq!(probe.metadata().kind, Kind::Image);
    assert_eq!(probe.metadata().duration_frames, 1);
    let state = runtime.snapshot();
    #[cfg(feature = "slow-tests")]
    if case == "verify-timeout" {
        assert!(state.degraded);
        assert_eq!(runtime.witness().launches, 2);
        runtime.shutdown().unwrap();
        return;
    }
    // The parent names the mode this environment must give; the other mode
    // never stands in for it.
    let expected = std::env::var("KW_RUNTIME_EXPECT").unwrap();
    assert!(case != "degraded" || expected == "degraded");
    println!("MS2_RUNTIME_MODE degraded={}", state.degraded);
    assert_eq!(
        if state.degraded {
            "degraded"
        } else {
            "primary"
        },
        expected,
        "runtime fixture ran the wrong resource mode"
    );
    runtime.shutdown().unwrap();
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().pending),
        (0, 0)
    );
}

#[cfg(feature = "slow-tests")]
fn held_helper_failure(config: Config, path: &Path) {
    use kinewright_media_client::Failure;
    use kinewright_media_ipc::MediaSandboxError;
    use std::sync::atomic::Ordering;
    fs::write(path, b"A regular file without a supported media format.").unwrap();
    let hold = Arc::new(AtomicBool::new(false));
    let runtime = Runtime::start(config.hold_cleanup(Arc::clone(&hold))).unwrap();
    let probe =
        || runtime.probe_until_deadline(Box::new(Input(File::open(path).unwrap())), None, None);
    let control = probe();
    assert!(
        matches!(
            control,
            Err(Failure::Sandbox(MediaSandboxError::UnsupportedMedia))
        ),
        "{control:?}"
    );
    println!("MS2_HELD_FAILURE_CONTROL {control:?}");
    hold.store(true, Ordering::Release);
    let result = probe();
    let active = runtime.snapshot().active;
    hold.store(false, Ordering::Release);
    runtime.shutdown().unwrap();
    assert_eq!(
        active, 1,
        "the answer must leave the held reservation active"
    );
    assert!(
        matches!(
            result,
            Err(Failure::Sandbox(MediaSandboxError::UnsupportedMedia))
        ),
        "{result:?}"
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ms_t10_finished_helper_failure_survives_held_cleanup() {
    let _deadline = deadline::test();
    run_case("held-failure");
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ms_t10_scope_verification_timeout_falls_back() {
    let _deadline = deadline::test();
    if !ms2_diag::user_manager() {
        println!(
            "MS2_NATIVE_NOT_RUN binary=runtime_native test=ms_t10_scope_verification_timeout_falls_back reason=no_user_manager"
        );
        return;
    }
    run_case("verify-timeout");
}
/// Two probes whose scope emptiness proof always errors. Each must be
/// answered with a typed failure inside the cleanup budget, keep its decode
/// slot, and leave the other slot usable; close reports the failure.
#[cfg(feature = "slow-tests")]
fn unproven_cleanup(config: Config, png: &Path) {
    use kinewright_media_client::Failure;
    use kinewright_media_ipc::MediaSandboxError;
    let runtime = Runtime::start(config).unwrap();
    let probe = || {
        let started = Instant::now();
        let result = runtime.probe(Request {
            source: Box::new(Input(File::open(png).unwrap())),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        });
        (result.err(), started.elapsed(), runtime.snapshot().active)
    };
    let first = probe();
    let second = probe();
    let shutdown = runtime.shutdown();
    let held = runtime.snapshot().active;
    println!(
        "MS2_UNPROVEN_CLEANUP first={first:?} second={second:?} shutdown={shutdown:?} held={held}"
    );
    for (error, elapsed, _) in [&first, &second] {
        assert!(matches!(
            error,
            Some(Failure::Sandbox(MediaSandboxError::Crashed))
        ));
        assert!(*elapsed < Duration::from_secs(4), "{elapsed:?}");
    }
    assert_eq!((first.2, second.2, held), (1, 2, 2));
    assert_eq!(shutdown, Err(MediaSandboxError::Crashed));
}
/// Selects the scope seams and runs cases with a specialized outcome check.
#[cfg(feature = "slow-tests")]
fn seamed_case(case: &std::ffi::OsStr, config: Config, png: &Path) -> Option<Config> {
    use kinewright_media_client::ScopeSeam;
    match case.to_str().unwrap() {
        "hosted-kernel" => return Some(config.scope_seam(ScopeSeam::HostedKernel)),
        "verify-timeout" => return Some(config.scope_seam(ScopeSeam::VerifyTimedOut)),
        "held-failure" => held_helper_failure(config, png),
        "attached-failed" => {
            unproven_fallback(config.scope_seam(ScopeSeam::AttachedReportedFailed), png)
        }
        "persistent-error" => unproven_cleanup(config.scope_seam(ScopeSeam::PersistentError), png),
        _ => return Some(config),
    }
    None
}
/// The scope is attached and then reported failed, and every emptiness
/// proof errors. The run is unproven, so the degraded fallback must not
/// relaunch it: one launch, one slot kept, a failure inside the cleanup
/// budget plus the error bound, and close reports the cleanup failure.
#[cfg(feature = "slow-tests")]
fn unproven_fallback(config: Config, png: &Path) {
    use kinewright_media_client::Failure;
    use kinewright_media_ipc::MediaSandboxError;
    let runtime = Runtime::start(config).unwrap();
    let started = Instant::now();
    let result = runtime.probe(Request {
        source: Box::new(Input(File::open(png).unwrap())),
        video: None,
        audio: None,
        cancel: Arc::new(AtomicBool::new(false)),
        explicit_retry: false,
    });
    let elapsed = started.elapsed();
    let launches = runtime.witness().launches;
    let held = runtime.snapshot().active;
    let shutdown = runtime.shutdown();
    println!(
        "MS2_UNPROVEN_FALLBACK result={:?} elapsed={elapsed:?} launches={launches} held={held} shutdown={shutdown:?}",
        result.as_ref().err()
    );
    assert_eq!(launches, 1, "an unproven fallback run was relaunched");
    assert_eq!(held, 1);
    assert!(
        matches!(result, Err(Failure::Sandbox(MediaSandboxError::Crashed))),
        "{result:?}"
    );
    // REAP_BUDGET (2 s) plus SCOPE_PROOF_ERRORS (500 ms).
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
    assert_eq!(shutdown, Err(MediaSandboxError::Crashed));
}
/// Runs one fixture case in a child and returns its relayed `MS2_` lines.
fn run_case(case: &str) -> String {
    let degraded = case == "degraded";
    // `automatic` must select primary whenever a user manager is reachable.
    let expected = if degraded || !ms2_diag::user_manager() {
        "degraded"
    } else {
        "primary"
    };
    if case == "automatic" && expected == "degraded" {
        deadline::announce(
            "MS2_PRIMARY_UNAVAILABLE binary=runtime_native test=automatic reason=no_user_manager",
        );
    }
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
        .env("KW_RUNTIME_FIXTURE", case)
        .env("KW_RUNTIME_EXPECT", expected)
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
            let mut relayed = String::new();
            for line in output.lines().filter(|line| line.contains("MS2_")) {
                let line = &line[line.find("MS2_").unwrap_or(0)..];
                println!("{line}");
                relayed.push_str(line);
                relayed.push('\n');
            }
            assert!(status.success(), "runtime fixture: {status}");
            deadline::announce(&format!(
                "MS2_MODES_RAN binary=runtime_native case={case} modes={expected}"
            ));
            return relayed;
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
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    run_case("automatic");
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_png_degraded_admission_closes_and_cleans() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    run_case("degraded");
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_releases_both_slots_during_bus_disconnect() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    if !ms2_diag::user_manager() {
        println!(
            "MS2_NATIVE_NOT_RUN binary=runtime_native test=runtime_releases_both_slots_during_bus_disconnect reason=no_user_manager"
        );
        return;
    }
    run_case("bus-cleanup");
    run_case("bus-restart");
}

/// The hosted-kernel shape (`6.17` azure, `7.0` aws): a removed scope
/// cgroup keeps two links. The seam reports two links for the helper scope's
/// retained directory; cleanup waits for the manager to remove the scope and
/// must then prove it deleted through ENODEV on the retained events
/// descriptor, prove it empty, and return.
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_scope_proof_holds_when_a_removed_scope_keeps_two_links() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    if !ms2_diag::user_manager() {
        println!(
            "MS2_NATIVE_NOT_RUN binary=runtime_native test=runtime_scope_proof_holds_when_a_removed_scope_keeps_two_links reason=no_user_manager"
        );
        return;
    }
    let relayed = run_case("hosted-kernel");
    assert!(
        relayed
            .contains("MS2_SCOPE_PROOF seam=hosted_kernel nlink=2 events=ENODEV deleted=Ok(true)"),
        "cleanup did not prove the removed two-link scope deleted through ENODEV:\n{relayed}"
    );
}

/// A scope whose emptiness proof keeps failing is unproven, not "not yet":
/// cleanup stops after the bounded error run, answers a typed failure,
/// never releases the scope or its decode slot, and close reports it.
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_scope_proof_error_keeps_the_reservation() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    if !ms2_diag::user_manager() {
        println!(
            "MS2_NATIVE_NOT_RUN binary=runtime_native test=runtime_scope_proof_error_keeps_the_reservation reason=no_user_manager"
        );
        return;
    }
    let relayed = run_case("persistent-error");
    assert!(
        relayed.contains("MS2_UNPROVEN_CLEANUP"),
        "the unproven cleanup case did not report:\n{relayed}"
    );
}

/// A run whose scope was attached and then reported failed takes the
/// degraded fallback, but its cleanup could not prove the scope empty: the
/// request must get its failure instead of a degraded relaunch.
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn runtime_unproven_fallback_is_not_relaunched() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    if !ms2_diag::user_manager() {
        println!(
            "MS2_NATIVE_NOT_RUN binary=runtime_native test=runtime_unproven_fallback_is_not_relaunched reason=no_user_manager"
        );
        return;
    }
    let relayed = run_case("attached-failed");
    assert!(
        relayed.contains("MS2_UNPROVEN_FALLBACK"),
        "the unproven fallback case did not report:\n{relayed}"
    );
}
