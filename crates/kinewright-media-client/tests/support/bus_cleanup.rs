use super::{Config, File, Kind, Limits, Path, Request, Runtime, Source, fs};
use std::{
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

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

fn fifo(path: &Path) -> File {
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    File::options().read(true).write(true).open(path).unwrap()
}

struct ResumeBus(File, u8);
impl Drop for ResumeBus {
    fn drop(&mut self) {
        let _ = self.0.write_all(&[self.1]);
    }
}

fn wait_for_disconnect(event: &mut File) {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let mut fds = [PollFd::new(&*event, PollFlags::IN)];
    assert_ne!(
        poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 3,
                tv_nsec: 0
            })
        )
        .unwrap(),
        0,
        "supervisor did not close its session bus connection"
    );
    let mut message = [0];
    event.read_exact(&mut message).unwrap();
    assert_eq!(message, [1]);
}

fn wait_for(message: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::yield_now();
    }
}

struct Scopes(String);
impl Scopes {
    fn of_supervisor(supervisor: u32) -> Self {
        let start = kinewright_job_sys::ProcessHandle::open(supervisor)
            .unwrap()
            .start();
        let app = std::process::id();
        println!("MS2_BUS_FIXTURE app={app} supervisor={supervisor}");
        Self(format!(
            "kinewright-media-{app}-{supervisor}-{start}-1-*.scope"
        ))
    }
}
impl Drop for Scopes {
    fn drop(&mut self) {
        for action in ["stop", "reset-failed"] {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", action, &self.0])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

fn supervisor_wrapper(directory: &Path, binaries: &Path) -> std::path::PathBuf {
    let wrapper = directory.join("scope-supervisor");
    let executable = binaries.join("kinewright-media-supervisor");
    let quote = |path: &Path| path.to_str().unwrap().replace('\'', "'\\''");
    let script = format!(
        "#!/bin/sh\nif [ \"$2\" = 1 ]; then export KW_MS2_SCOPE_TEST_DISCONNECT='{}'; fi\nexec '{}' \"$@\"\n",
        quote(directory),
        quote(&executable)
    );
    fs::write(&wrapper, script).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    wrapper
}

pub(super) fn exercise(directory: &Path, binaries: &Path, png: &Path, restart: bool) {
    let mut disconnected = fifo(&directory.join("disconnected"));
    let reconnect = ResumeBus(fifo(&directory.join("reconnect")), u8::from(!restart));
    let supervisor = supervisor_wrapper(directory, binaries);
    let (at_close, observing) = mpsc::sync_channel(2);
    let (allow_close, close_allowed) = mpsc::sync_channel(2);
    let close_allowed = Mutex::new(close_allowed);
    let arrivals = AtomicUsize::new(0);
    let config = Config::new(
        binaries.join("kinewright-media-helper"),
        supervisor,
        Limits::PROBE,
    )
    .observe(Arc::new(move |memory| {
        if memory.cgroup.is_none() {
            return Err(io::Error::other(
                "disconnect fixture requires primary resource mode",
            ));
        }
        if arrivals.fetch_add(1, Ordering::Relaxed) < 2 {
            at_close.send(()).map_err(io::Error::other)?;
            close_allowed
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(4))
                .expect("bus cleanup probe observer release deadline");
        }
        Ok(())
    }));
    let runtime = Runtime::start(config).unwrap();
    wait_for("supervisor did not start", || {
        runtime.witness().supervisor != 0
    });
    let supervisor = runtime.witness().supervisor;
    let _scopes = Scopes::of_supervisor(supervisor);
    let (result_sender, replies) = mpsc::channel();
    let probe = |identity| {
        let result = runtime.probe(Request {
            source: Box::new(Input(File::open(png).unwrap(), identity)),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        });
        result_sender.send((identity, result)).unwrap();
    };
    std::thread::scope(|scope| {
        // If an assertion fails, both controls are released before scope joins.
        let reconnect = reconnect;
        let allow_close = allow_close;
        scope.spawn(|| probe(1));
        scope.spawn(|| probe(2));
        for _ in 0..2 {
            observing.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        assert_eq!(runtime.snapshot().active, 2);
        scope.spawn(|| probe(3));
        wait_for("third import was never queued", || {
            runtime.snapshot().pending == 1
        });
        allow_close.send(()).unwrap();
        allow_close.send(()).unwrap();
        wait_for_disconnect(&mut disconnected);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut completed = [
            replies
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("a slot still waited for D-Bus"),
            replies
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("the second slot still waited for D-Bus"),
        ];
        completed.sort_by_key(|(id, _)| *id);
        for ((identity, result), expected) in completed.into_iter().zip([1, 2]) {
            assert_eq!(identity, expected);
            assert_eq!(result.unwrap().metadata().kind, Kind::Image);
        }
        wait_for("natively clean probes still occupy launch slots", || {
            runtime.snapshot().active == 0
        });
        assert_eq!(runtime.snapshot().pending, 1);
        drop(reconnect);
        let (identity, result) = replies.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(identity, 3);
        assert_eq!(result.unwrap().metadata().kind, Kind::Image);
        let current = runtime.witness().supervisor;
        assert_ne!(current, 0);
        assert_eq!(current != supervisor, restart);
        println!("MS2_BUS_RECOVERED restart={restart} supervisor={current}");
    });
    assert_eq!(runtime.shutdown(), Ok(()));
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().pending),
        (0, 0)
    );
}
