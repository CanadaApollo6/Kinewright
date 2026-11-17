#![cfg(target_os = "linux")]
#![allow(unsafe_code)]
use kinewright_job_sys::{ProcessHandle, media_scope};
use kinewright_media_ipc::{
    Frame, MediaSandboxError, Message, ScopeUnavailableReason, read_frame, write_frame,
};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
const GENERATION: u64 = 719;

fn evidence_path(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ms2-s1a");
    fs::create_dir_all(&directory).unwrap();
    directory.join(name)
}

struct ManagedChild {
    child: Child,
    reaped: bool,
    marker: PathBuf,
}
impl ManagedChild {
    fn blocked() -> Self {
        let marker = evidence_path(&format!(
            "scope-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "blocked_child_fixture", "--nocapture"])
            .env_clear()
            .env("KW_SCOPE_BLOCK", "1")
            .env("KW_SCOPE_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child,
            reaped: false,
            marker,
        }
    }
    fn kill(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
    fn release(&mut self) {
        self.child.stdin.take().unwrap().write_all(&[1]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                self.reaped = true;
                break;
            }
            assert!(
                Instant::now() < deadline,
                "blocked scope fixture did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            self.marker.exists(),
            "fixture never crossed its release barrier"
        );
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.kill();
        let _ = fs::remove_file(&self.marker);
    }
}
#[test]
fn blocked_child_fixture() {
    if std::env::var_os("KW_SCOPE_BLOCK").is_none() {
        return;
    }
    let mut token = [0];
    std::io::stdin().read_exact(&mut token).unwrap();
    assert_eq!(token, [1]);
    // The parent observes only an empty marker's existence, not in-place contents.
    fs::write(std::env::var_os("KW_SCOPE_MARKER").unwrap(), b"").unwrap();
}
struct Supervisor {
    child: Child,
    socket: UnixStream,
    request: u32,
    records: Vec<u32>,
    start: u64,
    reaped: bool,
}
impl Supervisor {
    fn start(manager: bool) -> (Self, Message) {
        Self::with_address(manager.then(media_scope::session_bus_address))
    }
    fn with_address(address: Option<String>) -> (Self, Message) {
        let (socket, inherited) = media_scope::socketpair().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(6)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(6)))
            .unwrap();
        let stderr_path = evidence_path(&format!(
            "supervisor-{}-{}.log",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let stderr = fs::File::create(stderr_path).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kinewright-media-supervisor"));
        command
            .args([std::process::id().to_string(), GENERATION.to_string()])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        if let Some(address) = address {
            command.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
        // SAFETY: inherited stays owned through spawn; the audited wrapper
        // creates a child-only copy at the reserved supervisor endpoint.
        unsafe {
            media_scope::inherit_socket(&mut command, inherited.as_raw_fd());
        }
        let child = command.spawn().unwrap();
        drop(inherited);
        let start = ProcessHandle::open(child.id()).unwrap().start();
        let mut supervisor = Self {
            child,
            socket,
            request: 1,
            records: vec![],
            start,
            reaped: false,
        };
        let first = read_frame(&mut supervisor.socket).unwrap();
        assert_eq!((first.request, first.generation), (1, GENERATION));
        (supervisor, first.message)
    }
    fn rpc(&mut self, message: Message) -> Message {
        self.request += 1;
        write_frame(
            &mut self.socket,
            &Frame {
                request: self.request,
                generation: GENERATION,
                message,
            },
        )
        .unwrap();
        let frame = read_frame(&mut self.socket).unwrap();
        assert_eq!(
            (frame.request, frame.generation),
            (self.request, GENERATION)
        );
        frame.message
    }
    fn create(&mut self, child: &ManagedChild) -> u32 {
        let message = self.rpc(Message::CreateScope {
            pid: child.child.id(),
            memory_bytes: 64 * 1024 * 1024,
        });
        match message {
            Message::ScopeCreated { scope } => {
                self.records.push(scope);
                scope
            }
            Message::ScopeCreateFailed { scope, error } => {
                self.records.push(scope);
                panic!("scope create failed {error:?}");
            }
            other => panic!("native primary scope gate failed: {other:?}"),
        }
    }
    fn stop(&mut self, scope: u32) {
        assert!(
            matches!(self.rpc(Message::StopScope{scope}),Message::ScopeStopped{scope:actual} if actual==scope)
        );
        self.records.retain(|value| *value != scope);
    }
    fn unit(&self, id: u32) -> String {
        format!(
            "kinewright-media-{}-{}-{}-{}-{id}.scope",
            std::process::id(),
            self.child.id(),
            self.start,
            GENERATION
        )
    }
    fn finish(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                self.reaped = true;
                break;
            }
            assert!(
                Instant::now() < deadline,
                "supervisor did not finish cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let deadline = Instant::now() + Duration::from_secs(6);
        while !self.reaped && Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                self.reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // Exception-path cleanup names only units generated by this supervisor.
        // The production implementation never invokes systemctl or systemd-run.
        for id in &self.records {
            let unit = self.unit(*id);
            let _ = Command::new("systemctl")
                .args(["--user", "stop", &unit])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = Command::new("systemctl")
                .args(["--user", "reset-failed", &unit])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn primary_supervisor(test: &str) -> Option<Supervisor> {
    let (mut supervisor, ready) = Supervisor::start(true);
    match ready {
        Message::Ready {} => Some(supervisor),
        Message::ScopeUnavailable {
            scope: None,
            reason: ScopeUnavailableReason::NoUserManager,
        } => {
            println!("MS2_NATIVE_NOT_RUN binary=scope_native test={test} reason=no_user_manager");
            supervisor.finish();
            None
        }
        other => panic!("unexpected supervisor availability: {other:?}"),
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn native_scope_limits_join_before_release_and_supervisor_stays_outside() {
    let Some(mut supervisor) =
        primary_supervisor("native_scope_limits_join_before_release_and_supervisor_stays_outside")
    else {
        return;
    };
    let mut child = ManagedChild::blocked();
    let scope = supervisor.create(&child);
    assert!(
        !child.marker.exists(),
        "child ran before verified resource join"
    );
    let reply = supervisor.rpc(Message::VerifyScope { scope });
    assert!(
        matches!(reply,Message::ScopeVerified{scope:actual,starttime} if actual==scope && starttime==ProcessHandle::open(child.child.id()).unwrap().start()),
        "{reply:?}"
    );
    let group = media_scope::membership(child.child.id()).unwrap();
    assert_ne!(
        media_scope::membership(supervisor.child.id()).unwrap(),
        group
    );
    assert_ne!(media_scope::membership(std::process::id()).unwrap(), group);
    let native = kinewright_job_sys::Cgroup::open(&group).unwrap();
    media_scope::verify_controls(&native, 64 * 1024 * 1024).unwrap();
    child.release();
    supervisor.stop(scope);
    supervisor.finish();
}
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn native_scope_records_are_bounded_and_self_or_stale_requests_are_refused() {
    let Some(mut supervisor) = primary_supervisor(
        "native_scope_records_are_bounded_and_self_or_stale_requests_are_refused",
    ) else {
        return;
    };
    for pid in [std::process::id(), supervisor.child.id()] {
        assert!(matches!(
            supervisor.rpc(Message::CreateScope {
                pid,
                memory_bytes: 64 * 1024 * 1024
            }),
            Message::Failed {
                error: MediaSandboxError::ProtocolViolation
            }
        ));
    }
    let mut first = ManagedChild::blocked();
    let mut second = ManagedChild::blocked();
    let third = ManagedChild::blocked();
    let a = supervisor.create(&first);
    let b = supervisor.create(&second);
    assert!(matches!(
        supervisor.rpc(Message::CreateScope {
            pid: third.child.id(),
            memory_bytes: 64 * 1024 * 1024
        }),
        Message::Failed {
            error: MediaSandboxError::ResourceLimit
        }
    ));
    assert!(matches!(
        supervisor.rpc(Message::VerifyScope { scope: u32::MAX }),
        Message::Failed {
            error: MediaSandboxError::ProtocolViolation
        }
    ));
    first.kill();
    second.kill();
    supervisor.stop(a);
    supervisor.stop(b);
    supervisor.finish();
}
#[test]
fn missing_user_manager_is_typed_and_creates_no_scope() {
    let (mut supervisor, ready) = Supervisor::start(false);
    assert!(matches!(
        ready,
        Message::ScopeUnavailable {
            scope: None,
            reason: ScopeUnavailableReason::NoUserManager
        }
    ));
    let child = ManagedChild::blocked();
    assert!(matches!(
        supervisor.rpc(Message::CreateScope {
            pid: child.child.id(),
            memory_bytes: 64 * 1024 * 1024
        }),
        Message::ScopeUnavailable {
            scope: None,
            reason: ScopeUnavailableReason::NoUserManager
        }
    ));
    supervisor.finish();
}

#[test]
fn stale_identity_and_non_lifecycle_messages_close_the_contract() {
    for (request, generation, message) in [
        (1, GENERATION, Message::StopScope { scope: 1 }),
        (2, GENERATION + 1, Message::StopScope { scope: 1 }),
        (
            2,
            GENERATION,
            Message::Probe {
                video: None,
                audio: None,
            },
        ),
    ] {
        let (mut supervisor, _) = Supervisor::start(false);
        write_frame(
            &mut supervisor.socket,
            &Frame {
                request,
                generation,
                message,
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = supervisor.child.try_wait().unwrap() {
                supervisor.reaped = true;
                assert_eq!(status.code(), Some(125));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "invalid lifecycle contract remained open"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn native_socket_eof_stops_scope_and_waits_for_empty() {
    let Some(mut supervisor) =
        primary_supervisor("native_socket_eof_stops_scope_and_waits_for_empty")
    else {
        return;
    };
    let mut child = ManagedChild::blocked();
    let scope = supervisor.create(&child);
    assert!(matches!(
        supervisor.rpc(Message::VerifyScope { scope }),
        Message::ScopeVerified { .. }
    ));
    let mut group =
        kinewright_job_sys::Cgroup::open(&media_scope::membership(child.child.id()).unwrap())
            .unwrap();
    assert!(!group.empty().unwrap());
    supervisor.finish();
    let status = child.child.wait().unwrap();
    child.reaped = true;
    assert!(!status.success());
    assert!(!child.marker.exists());
    assert!(group.empty().unwrap());
    supervisor.records.clear();
}

struct PrivateBus(Child);
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn bus_without_user_manager_is_typed() {
    let mut bus = PrivateBus(
        Command::new("/usr/bin/dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = bus.0.stdout.take().unwrap();
    kinewright_job_sys::media_boundary::wait_readable(stdout.as_fd(), Duration::from_secs(2))
        .unwrap();
    let mut address = String::new();
    BufReader::new(stdout.take(1024))
        .read_line(&mut address)
        .unwrap();
    assert!(address.ends_with('\n'));
    let (mut supervisor, ready) = Supervisor::with_address(Some(address.trim_end().into()));
    assert!(matches!(
        ready,
        Message::ScopeUnavailable {
            scope: None,
            reason: ScopeUnavailableReason::NoUserManager
        }
    ));
    supervisor.finish();
}
