#![cfg(target_os = "linux")]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

#[cfg(feature = "slow-tests")]
mod support {
    use kinewright_job_sys::media_host;
    use kinewright_media_client::{Config, Failure, Limits, Request, Runtime, Source};
    use kinewright_media_ipc::{Kind, MediaSandboxError};
    use std::{
        fs::{self, File},
        io::{BufRead, BufReader, Read},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };
    const ADMITTED: u64 = 128 * 1024 * 1024;
    pub struct ChildGuard(Child);
    impl Drop for ChildGuard {
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
    struct Input(File, Arc<AtomicUsize>);
    impl Source for Input {
        fn file(&self) -> &File {
            self.1.fetch_add(1, Ordering::AcqRel);
            &self.0
        }
        fn identity(&self) -> [u8; 32] {
            [44; 32]
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
    fn request(path: &Path, passes: &Arc<AtomicUsize>) -> Request {
        Request {
            source: Box::new(Input(File::open(path).unwrap(), Arc::clone(passes))),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        }
    }
    pub fn fixture(case: &str) {
        assert_eq!(
            media_host::children_peak_resident().unwrap(),
            0,
            "fresh app must have no encoder/other child peak"
        );
        let root = PathBuf::from(std::env::var_os("KW_RSS_ROOT").unwrap());
        let source = root.join("source.png");
        let binaries = binaries();
        let helper = if case == "burst" {
            let wrapper = root.join(format!("helper-{}", std::process::id()));
            let path = binaries.join("kinewright-media-helper-witness");
            let path = path.to_str().unwrap().replace('\'', "'\\''");
            fs::write(
                &wrapper,
                format!("#!/bin/sh\nexec '{path}' \"$@\" rss-burst\n"),
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            wrapper
        } else {
            binaries.join("kinewright-media-helper")
        };
        let runtime = Runtime::start(Config::new(
            helper,
            binaries.join("kinewright-media-supervisor"),
            Limits {
                address_space: 2 * 1024 * 1024 * 1024,
                private_memory: ADMITTED,
                allocation: ADMITTED / 2,
            },
        ))
        .unwrap();
        // Descriptor extraction occurs only after the actor admits a launch.
        // The caller observes the same counter across both source capabilities.
        let passes = Arc::new(AtomicUsize::new(0));
        let outcome = runtime.probe(request(&source, &passes));
        assert_eq!(passes.load(Ordering::Acquire), 1);
        assert!(runtime.snapshot().degraded);
        let observed = runtime.witness().observed_rss;
        if case == "burst" {
            assert!(
                matches!(
                    outcome,
                    Err(Failure::Sandbox(MediaSandboxError::ResourceLimit))
                ),
                "{outcome:?}"
            );
            assert!(
                observed > ADMITTED,
                "the real RSS poll did not observe excess residency"
            );
            assert!(
                !runtime.snapshot().paused,
                "source quarantine must not be a global circuit pause"
            );
            assert!(matches!(
                runtime.probe(request(&source, &passes)),
                Err(Failure::Sandbox(MediaSandboxError::ResourceLimit))
            ));
        } else {
            assert_eq!(outcome.unwrap().metadata().kind, Kind::Image);
            assert!(observed <= ADMITTED);
        }
        assert_eq!(
            passes.load(Ordering::Acquire),
            1,
            "quarantined source reached another native launch"
        );
        runtime.shutdown().unwrap();
        assert_eq!(
            (runtime.snapshot().active, runtime.snapshot().pending),
            (0, 0)
        );
        let peak = media_host::children_peak_resident().unwrap();
        if case == "burst" {
            assert!(peak > ADMITTED);
        }
        println!(
            "MS2_RSS case={case} admitted={ADMITTED} observed_sum={observed} kernel_child_peak={peak} overshoot_lower_bound={}",
            peak.saturating_sub(ADMITTED)
        );
    }
    fn directory() -> Directory {
        let root = Directory(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ms2-s1a")
                .join(format!("rss-{}", std::process::id())),
        );
        fs::create_dir_all(&root.0).unwrap();
        let ffmpeg =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg");
        assert!(
            Command::new(ffmpeg)
                .args([
                    "-nostdin",
                    "-y",
                    "-v",
                    "error",
                    "-filter_threads",
                    "1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=red:s=128x72:d=0.1",
                    "-frames:v",
                    "1",
                    "-threads",
                    "1"
                ])
                .arg(root.0.join("source.png"))
                .status()
                .unwrap()
                .success()
        );
        root
    }
    fn private_bus() -> (ChildGuard, String) {
        let mut bus = ChildGuard(
            Command::new("/usr/bin/dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let stdout = bus.0.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut address = String::new();
            BufReader::new(stdout.take(4096))
                .read_line(&mut address)
                .unwrap();
            let _ = send.send(address);
        });
        let address = receive.recv_timeout(Duration::from_secs(3)).unwrap();
        (bus, address.trim_end().to_owned())
    }
    fn run(root: &Path, address: &str, case: &str) -> String {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "rss_fixture", "--nocapture"])
                .env("KW_RSS_CASE", case)
                .env("KW_RSS_ROOT", root)
                .env("DBUS_SESSION_BUS_ADDRESS", address)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let end = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < end,
                "RSS fixture exceeded cleanup deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut output = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .take(8192)
            .read_to_string(&mut output)
            .unwrap();
        let mut errors = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .take(8192)
            .read_to_string(&mut errors)
            .unwrap();
        assert!(status.success(), "{status}\n{output}\n{errors}");
        let line = output
            .lines()
            .find(|line| line.contains("MS2_RSS "))
            .unwrap();
        let line = &line[line.find("MS2_RSS ").unwrap()..];
        println!("{line}");
        line.to_owned()
    }
    pub fn repeated() {
        let root = directory();
        let (_bus, address) = private_bus();
        for _ in 0..5 {
            run(&root.0, &address, "control");
            run(&root.0, &address, "burst");
        }
    }
}
#[test]
fn rss_fixture() {
    #[cfg(feature = "slow-tests")]
    if let Ok(case) = std::env::var("KW_RSS_CASE") {
        support::fixture(&case);
    }
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn degraded_rss_polling_records_overshoot_and_quarantines_source() {
    let _deadline = deadline::test();
    #[cfg(feature = "slow-tests")]
    support::repeated();
}
