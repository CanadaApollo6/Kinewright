#![cfg(target_os = "linux")]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;
#[path = "../../kinewright-job-sys/test_support/ms2_diag.rs"]
mod ms2_diag;

#[cfg(feature = "slow-tests")]
mod support {
    use kinewright_job_sys::{Cgroup, ProcessHandle, media_host, media_scope};
    use kinewright_media_client::{Config, Failure, Limits, Request, Runtime, Source};
    use kinewright_media_ipc::MediaSandboxError;
    use std::{
        fs::{self, File},
        io::{BufRead, BufReader, Read, Write},
        os::unix::process::ExitStatusExt,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{Arc, atomic::AtomicBool, mpsc},
        time::{Duration, Instant},
    };
    pub struct OwnedChild(pub Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    struct OwnedProcess(ProcessHandle);
    impl std::ops::Deref for OwnedProcess {
        type Target = ProcessHandle;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let end = Instant::now() + Duration::from_secs(5);
            while !media_host::reap_adopted(&self.0).unwrap_or(false) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    struct Paused<'a>(&'a ProcessHandle);
    impl Drop for Paused<'_> {
        fn drop(&mut self) {
            let _ = rustix::process::pidfd_send_signal(self.0, rustix::process::Signal::CONT);
        }
    }
    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Input {
        file: File,
        revoked: Arc<AtomicBool>,
    }
    impl Source for Input {
        fn file(&self) -> &File {
            &self.file
        }
        fn identity(&self) -> [u8; 32] {
            [42; 32]
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            if self.revoked.load(std::sync::atomic::Ordering::Acquire) {
                Err(kinewright_core::MediaError::MediaReadTemporarilyUnavailable)
            } else {
                Ok(())
            }
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
    fn workspace(stage: &str) -> (Directory, Config, PathBuf) {
        let directory = Directory(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ms2-s1a")
                .join(format!("loss-{}", std::process::id())),
        );
        fs::create_dir_all(&directory.0).unwrap();
        let png = directory.0.join("source.png");
        let ffmpeg =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
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
                .status()
                .unwrap()
                .success()
        );
        let binaries = binaries();
        let executable = binaries.join("kinewright-media-helper-witness");
        assert!(
            executable.is_file(),
            "build the slow-feature witness binary first"
        );
        let config = Config::new(
            crate::ms2_diag::wrap(&directory.0, &executable, "helper", &[stage]),
            crate::ms2_diag::wrap(
                &directory.0,
                &binaries.join("kinewright-media-supervisor"),
                "supervisor",
                &[],
            ),
            Limits {
                address_space: 2 * 1024 * 1024 * 1024,
                private_memory: 2 * 1024 * 1024 * 1024,
                allocation: 1024 * 1024 * 1024,
            },
        );
        (directory, config, png)
    }
    fn await_stage(runtime: &Runtime, mode: &str, stage: &str) -> bool {
        let end = Instant::now() + Duration::from_secs(4);
        loop {
            if mode == "primary" && runtime.snapshot().degraded {
                println!("MS2_LOSS_UNAVAILABLE");
                std::io::stdout().flush().unwrap();
                return false;
            }
            let observed = runtime.witness();
            if observed.contained
                && observed.supervisor != 0
                && observed.shim != 0
                && (observed.payload != 0 || stage == "before-shim-arm")
                && (if stage == "decode" {
                    observed.incomplete_reply
                } else {
                    !observed.ready
                })
            {
                println!(
                    "MS2_LOSS_BOUNDARY {} {} {} {}",
                    observed.supervisor,
                    observed.shim,
                    observed.payload,
                    u8::from(runtime.snapshot().degraded)
                );
                std::io::stdout().flush().unwrap();
                return true;
            }
            assert!(
                Instant::now() < end,
                "witness did not reach {stage}: {observed:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn fixture(case: &str) {
        let parts: Vec<_> = case.split(':').collect();
        let [mode, stage, loss] = parts.as_slice() else {
            panic!("bad fixture case")
        };
        rustix::process::setrlimit(
            rustix::process::Resource::Core,
            rustix::process::Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        )
        .unwrap();
        crate::ms2_diag::fixture(kinewright_media_client::diagnostics);
        let (_directory, config, png) = workspace(stage);
        let runtime = Runtime::start(config).unwrap();
        // Prints only if this fixture panics, for example when the witness
        // never reaches its stage.
        let sampler = crate::ms2_diag::sample(&format!("witness_native {mode}:{stage}:{loss}"));
        let cancel = Arc::new(AtomicBool::new(false));
        let revoked = Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            let runtime_ref = &runtime;
            let probe = scope.spawn(|| {
                runtime_ref.probe(Request {
                    source: Box::new(Input {
                        file: File::open(&png).unwrap(),
                        revoked: Arc::clone(&revoked),
                    }),
                    video: None,
                    audio: None,
                    cancel: Arc::clone(&cancel),
                    explicit_retry: false,
                })
            });
            if !await_stage(&runtime, mode, stage) {
                runtime.shutdown().unwrap();
                assert!(probe.join().unwrap().is_err());
                return;
            }
            if matches!(*loss, "cancel" | "authority") {
                let mut token = [0];
                std::io::stdin().read_exact(&mut token).unwrap();
                if *loss == "cancel" {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                } else {
                    revoked.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            if *loss == "spawner" {
                let mut token = [0];
                std::io::stdin().read_exact(&mut token).unwrap();
                runtime.witness_abort_spawner().unwrap();
                loop {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            if *loss == "app" {
                loop {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let outcome = probe.join().unwrap();
            match *loss {
                "supervisor" => assert!(
                    matches!(outcome, Err(Failure::Sandbox(MediaSandboxError::Crashed))),
                    "{outcome:?}"
                ),
                "shim" => assert!(outcome.is_err(), "partial result was published"),
                "cancel" => assert!(
                    matches!(outcome, Err(Failure::Sandbox(MediaSandboxError::Cancelled))),
                    "{outcome:?}"
                ),
                "deadline" => assert!(
                    matches!(outcome, Err(Failure::Sandbox(MediaSandboxError::TimedOut))),
                    "{outcome:?}"
                ),
                "authority" => assert!(
                    matches!(
                        outcome,
                        Err(Failure::Authority(
                            kinewright_core::MediaError::MediaReadTemporarilyUnavailable
                        ))
                    ),
                    "{outcome:?}"
                ),
                _ => panic!("unknown loss kind"),
            }
            runtime.shutdown().unwrap();
            assert_eq!(
                (runtime.snapshot().active, runtime.snapshot().pending),
                (0, 0)
            );
        });
        sampler.finish(false);
    }
    struct CapturedLine {
        text: String,
        _reader: Box<dyn Read + Send>,
    }
    fn line(
        reader: impl Read + Send + 'static,
        prefix: &'static str,
    ) -> mpsc::Receiver<CapturedLine> {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader.take(8192));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) != 0 {
                if let Some(position) = line.find(prefix) {
                    line.drain(..position);
                    let _ = sender.send(CapturedLine {
                        text: line,
                        _reader: Box::new(reader),
                    });
                    return;
                }
                line.clear();
            }
        });
        receiver
    }
    fn bus() -> (OwnedChild, String) {
        let mut child = OwnedChild(
            Command::new("/usr/bin/dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let address = line(child.0.stdout.take().unwrap(), "unix:")
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        (child, address.text.trim_end().to_owned())
    }
    fn wait(child: &mut OwnedChild, end: Instant) -> std::process::ExitStatus {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < end,
                "fixture did not finish native cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    struct Boundary {
        app: ProcessHandle,
        supervisor: OwnedProcess,
        shim: OwnedProcess,
        payload: Option<OwnedProcess>,
        group: Option<Cgroup>,
        _capture: CapturedLine,
    }
    impl Boundary {
        fn observe(app: &mut OwnedChild, degraded: bool) -> Option<Self> {
            let app_pin = ProcessHandle::open(app.0.id()).unwrap();
            let capture = line(app.0.stdout.take().unwrap(), "MS2_LOSS_")
                .recv_timeout(Duration::from_secs(12))
                .unwrap();
            let line = &capture.text;

            if line.trim_end() == "MS2_LOSS_UNAVAILABLE" {
                assert!(!degraded);
                assert!(wait(app, Instant::now() + Duration::from_secs(10)).success());
                return None;
            }
            let values: Vec<u32> = line
                .strip_prefix("MS2_LOSS_BOUNDARY ")
                .unwrap()
                .split_whitespace()
                .map(|word| word.parse().unwrap())
                .collect();
            let [supervisor, shim, payload, mode] = values.as_slice() else {
                panic!("bad witness boundary")
            };
            assert_eq!(*mode, u32::from(degraded));
            let supervisor = OwnedProcess(ProcessHandle::open(*supervisor).unwrap());
            let shim = OwnedProcess(media_scope::child(app_pin.pid(), *shim).unwrap());
            let payload = (*payload != 0)
                .then(|| OwnedProcess(media_scope::child(shim.pid(), *payload).unwrap()));
            let group = if degraded {
                None
            } else {
                let path = media_scope::membership(shim.pid()).unwrap();
                assert!(
                    path.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with("kinewright-media-")
                );
                media_scope::outside(app_pin.pid(), &path).unwrap();
                media_scope::outside(supervisor.pid(), &path).unwrap();
                Some(Cgroup::open(&path).unwrap())
            };
            Some(Self {
                app: app_pin,
                supervisor,
                shim,
                payload,
                group,
                _capture: capture,
            })
        }
        fn signal(&self, app: &mut OwnedChild, loss: &str) {
            match loss {
                "app" => self.app.kill().unwrap(),
                "supervisor" => self.supervisor.kill().unwrap(),
                "shim" => self.shim.kill().unwrap(),
                "spawner" | "cancel" | "authority" => {
                    app.0.stdin.take().unwrap().write_all(&[1]).unwrap();
                }
                "deadline" => {}
                _ => panic!("bad loss kind"),
            }
        }
        fn finish(&mut self, end: Instant) {
            wait_reaped(&self.supervisor, end);
            if let Some(group) = &mut self.group {
                assert!(group.empty().unwrap());
            }
            let prefix = format!("kinewright-media-{}-", self.app.pid());
            for entry in fs::read_dir(std::env::temp_dir()).unwrap().flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&prefix))
                {
                    fs::remove_dir(entry.path()).unwrap();
                }
            }
        }
    }
    fn wait_reaped(process: &ProcessHandle, end: Instant) {
        loop {
            if media_host::reap_adopted(process).unwrap() {
                return;
            }
            assert!(
                Instant::now() < end,
                "live or unreaped owned process {}",
                process.pid()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// A helper that is contained and released but never forks or replies.
    /// The caller gets `TimedOut` (a breaker token) within the design's probe
    /// bound: 5 s initial probe, 250 ms grace, 2 s for kill, reap and native
    /// emptiness. Then the same while cleanup cannot end, and the reservation
    /// stays active until it does.
    pub fn silent() {
        // The design bound is 7.25 s. The actor notices each instant up to one
        // 100 ms tick late, twice; launch time also counts from the call.
        let bound = Duration::from_millis(5_000 + 250 + 2_000 + 2 * 100 + 500);
        // One runtime per test process: the spawner is process-wide.
        let (_directory, config, png) = workspace("silent");
        let hold = Arc::new(AtomicBool::new(false));
        let runtime = Runtime::start(config.hold_cleanup(Arc::clone(&hold))).unwrap();
        for held in [false, true] {
            hold.store(held, std::sync::atomic::Ordering::Release);
            let started = Instant::now();
            let sampler = crate::ms2_diag::sample(&format!("witness_native silent held={held}"));
            let outcome = runtime.probe(Request {
                source: Box::new(Input {
                    file: File::open(&png).unwrap(),
                    revoked: Arc::new(AtomicBool::new(false)),
                }),
                video: None,
                audio: None,
                cancel: Arc::new(AtomicBool::new(false)),
                explicit_retry: false,
            });
            let elapsed = started.elapsed();
            sampler.finish(
                !matches!(outcome, Err(Failure::Sandbox(MediaSandboxError::TimedOut)))
                    || elapsed > bound,
            );
            let active = runtime.snapshot().active;
            println!(
                "MS2_SILENT_PROBE held={held} degraded={} elapsed_ms={} active={active} outcome={outcome:?}",
                runtime.snapshot().degraded,
                elapsed.as_millis()
            );
            hold.store(false, std::sync::atomic::Ordering::Release);
            // This test requests no mode, so it must run the mode the
            // environment offers: primary whenever a user manager answers.
            assert_eq!(
                runtime.snapshot().degraded,
                !crate::ms2_diag::user_manager(),
                "silent helper ran the other resource mode"
            );
            assert!(
                matches!(outcome, Err(Failure::Sandbox(MediaSandboxError::TimedOut))),
                "silent helper held={held}: {outcome:?}"
            );
            assert!(
                elapsed <= bound,
                "silent helper held={held} answered after {elapsed:?}, bound {bound:?}"
            );
            // Unheld, the actor answers as it removes the job and publishes
            // the count a tick later, so only the held reservation is checked.
            if held {
                assert_eq!(active, 1, "reservation released before cleanup ended");
            }
        }
        crate::deadline::announce(&format!(
            "MS2_MODES_RAN binary=witness_native test=silent modes={}",
            if runtime.snapshot().degraded {
                "degraded"
            } else {
                "primary"
            }
        ));
        runtime.shutdown().unwrap();
        assert_eq!(
            (runtime.snapshot().active, runtime.snapshot().pending),
            (0, 0)
        );
    }
    pub fn loss(degraded: bool, stage: &str, loss: &str) -> bool {
        kinewright_job_sys::install_subreaper().unwrap();
        let private_bus = degraded.then(bus);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "loss_fixture", "--nocapture"])
            .env(
                "KW_LOSS_CASE",
                format!(
                    "{}:{stage}:{loss}",
                    if degraded { "degraded" } else { "primary" }
                ),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some((_, address)) = &private_bus {
            command.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
        let mut app = OwnedChild(command.spawn().unwrap());
        let Some(mut boundary) = Boundary::observe(&mut app, degraded) else {
            return false;
        };
        println!(
            "MS2_KILL_TARGET mode={} stage={stage} loss={loss} app={} shim={} payload={}",
            if degraded { "degraded" } else { "primary" },
            boundary.app.pid(),
            boundary.shim.pid(),
            boundary.payload.as_ref().map_or(0, |process| process.pid())
        );
        let end = Instant::now() + Duration::from_secs(10);
        let paused = if loss == "app" && !degraded {
            rustix::process::pidfd_send_signal(
                &*boundary.supervisor,
                rustix::process::Signal::STOP,
            )
            .unwrap();
            Some(Paused(&boundary.supervisor))
        } else {
            None
        };
        let paused_app = if loss == "shim" {
            rustix::process::pidfd_send_signal(&boundary.app, rustix::process::Signal::STOP)
                .unwrap();
            Some(Paused(&boundary.app))
        } else {
            None
        };
        boundary.signal(&mut app, loss);
        if paused_app.is_some() {
            let payload = boundary.payload.as_ref().expect("pre-arm payload");
            let deadline = Instant::now() + Duration::from_secs(1);
            while !media_host::exited(payload).unwrap() {
                assert!(
                    Instant::now() < deadline,
                    "payload failed its own pre-arm pidfd check while the app was stopped"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        drop(paused_app);
        let status = wait(&mut app, end);
        match loss {
            "app" => assert_eq!(status.signal(), Some(9)),
            "spawner" => assert_eq!(status.signal(), Some(6)),
            _ => assert!(status.success(), "{status}"),
        }
        wait_reaped(&boundary.shim, end);
        if let Some(payload) = &boundary.payload {
            wait_reaped(payload, end);
        }
        drop(paused);
        boundary.finish(end);
        println!(
            "MS2_KILL_WITNESS mode={} stage={stage} loss={loss} shim_payload_reaped=true watchdog=false",
            if degraded { "degraded" } else { "primary" }
        );
        true
    }
}
#[test]
fn loss_fixture() {
    #[cfg(feature = "slow-tests")]
    if let Ok(case) = std::env::var("KW_LOSS_CASE") {
        support::fixture(&case);
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn watchdog_disabled_primary_app_and_supervisor_loss() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    #[cfg(feature = "slow-tests")]
    for stage in ["before-ready", "decode"] {
        for loss in ["app", "supervisor"] {
            if !support::loss(false, stage, loss) {
                assert!(
                    !ms2_diag::user_manager(),
                    "a user manager answers but the primary pass ran degraded"
                );
                println!(
                    "MS2_NATIVE_NOT_RUN binary=witness_native test=watchdog_disabled_primary_app_and_supervisor_loss reason=no_user_manager"
                );
                return;
            }
        }
    }
    #[cfg(feature = "slow-tests")]
    for (stage, loss) in [
        ("before-shim-arm", "spawner"),
        ("decode", "spawner"),
        ("before-arm", "shim"),
    ] {
        if !support::loss(false, stage, loss) {
            assert!(
                !ms2_diag::user_manager(),
                "a user manager answers but the primary pass ran degraded"
            );
            println!(
                "MS2_NATIVE_NOT_RUN binary=witness_native test=watchdog_disabled_primary_app_and_supervisor_loss reason=no_user_manager"
            );
            return;
        }
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn watchdog_disabled_degraded_app_and_supervisor_loss() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    #[cfg(feature = "slow-tests")]
    for stage in ["before-ready", "decode"] {
        for loss in ["app", "supervisor"] {
            assert!(support::loss(true, stage, loss));
        }
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn watchdog_disabled_spawner_abort_and_payload_prearm_race() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    #[cfg(feature = "slow-tests")]
    for stage in ["before-shim-arm", "decode"] {
        assert!(support::loss(true, stage, "spawner"));
    }
    #[cfg(feature = "slow-tests")]
    assert!(support::loss(true, "before-arm", "shim"));
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn a_silent_helper_fails_typed_within_the_probe_bound() {
    // Two probes of at most about 8 s each, plus launch and shutdown.
    let _deadline = deadline::start(
        "silent helper probe bound",
        std::time::Duration::from_secs(40),
    );
    ms2_diag::install(kinewright_media_client::diagnostics);
    #[cfg(feature = "slow-tests")]
    support::silent();
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn cancellation_deadline_and_revocation_discard_partial_results() {
    let _deadline = deadline::test();
    ms2_diag::install(kinewright_media_client::diagnostics);
    #[cfg(feature = "slow-tests")]
    for loss in ["cancel", "deadline", "authority"] {
        assert!(support::loss(true, "decode", loss));
    }
}
