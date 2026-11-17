#![allow(clippy::missing_errors_doc)]
use kinewright_core::MediaError;
use kinewright_media_ipc::{MediaSandboxError, ValidatedProbe};
use std::{
    fmt,
    fs::File,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

mod actor;
mod controller;
mod worker;

pub const ACTIVE_PROBES: usize = 2;
pub const PENDING_PROBES: usize = 32;
const INITIAL_DEADLINE: Duration = Duration::from_secs(5);
const CANCEL_GRACE: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(100);

/// The actor owns this capability until native cleanup, independently of the
/// caller's lifetime. Only a duplicate file descriptor reaches the I/O worker.
pub trait Source: Send {
    fn file(&self) -> &File;
    fn identity(&self) -> [u8; 32];
    fn check(&self) -> Result<(), MediaError>;
}

/// Fixtures provide provisional limits. Production values are selected only
/// after the slice-1a measurements, before this client is wired into admission.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub address_space: u64,
    pub private_memory: u64,
}
pub struct Config {
    pub helper: PathBuf,
    pub supervisor: PathBuf,
    pub limits: Limits,
}
pub struct Request {
    pub source: Box<dyn Source>,
    pub video: Option<u32>,
    pub audio: Option<u32>,
    pub cancel: Arc<AtomicBool>,
    pub explicit_retry: bool,
}
#[derive(Debug)]
pub enum Failure {
    Sandbox(MediaSandboxError),
    Authority(MediaError),
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sandbox(error) => write!(f, "media sandbox: {error:?}"),
            Self::Authority(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Failure {}
pub type Outcome = Result<ValidatedProbe, Failure>;

/// Shared snapshots have one writer (the actor) and bounded scalar readers.
/// They contain no source authority and cannot complete a request.
#[derive(Default)]
struct Status {
    active: AtomicUsize,
    pending: AtomicUsize,
    degraded: AtomicBool,
    paused: AtomicBool,
    #[cfg(feature = "slow-tests")]
    supervisor: std::sync::atomic::AtomicU32,
    #[cfg(feature = "slow-tests")]
    shim: std::sync::atomic::AtomicU32,
    #[cfg(feature = "slow-tests")]
    payload: std::sync::atomic::AtomicU32,
    #[cfg(feature = "slow-tests")]
    ready: AtomicBool,
    #[cfg(feature = "slow-tests")]
    incomplete_reply: AtomicBool,
    #[cfg(feature = "slow-tests")]
    contained: AtomicBool,
    #[cfg(feature = "slow-tests")]
    observed_rss: std::sync::atomic::AtomicU64,
}
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub active: usize,
    pub pending: usize,
    pub degraded: bool,
    pub paused: bool,
}
/// Test observers carry no signalling or source authority.
#[cfg(feature = "slow-tests")]
#[derive(Clone, Copy, Debug)]
pub struct WitnessSnapshot {
    pub supervisor: u32,
    pub shim: u32,
    pub payload: u32,
    pub ready: bool,
    pub incomplete_reply: bool,
    pub contained: bool,
    pub observed_rss: u64,
}
pub struct Runtime {
    sender: SyncSender<actor::Command>,
    status: Arc<Status>,
}
static STARTED: AtomicBool = AtomicBool::new(false);

impl Runtime {
    pub fn start(config: Config) -> Result<Self, MediaSandboxError> {
        let valid = config.helper.is_absolute()
            && config.supervisor.is_absolute()
            && (64 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&config.limits.address_space)
            && (64 * 1024 * 1024..=config.limits.address_space)
                .contains(&config.limits.private_memory);
        if !valid || STARTED.swap(true, Ordering::AcqRel) {
            return Err(MediaSandboxError::SandboxUnavailable);
        }
        let (sender, receiver) = mpsc::sync_channel(PENDING_PROBES);
        let status = Arc::new(Status::default());
        // The actor writes status while the app reads it; this is real shared
        // process-lifetime observation, separate from each source capability.
        let observation = Arc::clone(&status);
        if std::thread::Builder::new()
            .name("media-spawner".into())
            .spawn(move || actor::run(config, receiver, observation))
            .is_err()
        {
            STARTED.store(false, Ordering::Release);
            return Err(MediaSandboxError::SandboxUnavailable);
        }
        Ok(Self { sender, status })
    }
    pub fn probe(&self, request: Request) -> Outcome {
        let (reply, receiver) = mpsc::sync_channel(1);
        // Caller unwind revokes the request; actor cleanup ownership survives it.
        loop {
            let count = self.status.pending.load(Ordering::Acquire);
            if count >= PENDING_PROBES {
                return Err(Failure::Sandbox(MediaSandboxError::ResourceLimit));
            }
            if self
                .status
                .pending
                .compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        let mut unwind = CancelOnDrop {
            flag: Arc::clone(&request.cancel),
            armed: false,
        };
        if self
            .sender
            .try_send(actor::Command::Probe(
                request,
                reply,
                Instant::now() + INITIAL_DEADLINE,
            ))
            .is_err()
        {
            self.status.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(Failure::Sandbox(MediaSandboxError::ResourceLimit));
        }
        unwind.armed = true;
        let result = receiver
            .recv()
            .unwrap_or(Err(Failure::Sandbox(MediaSandboxError::Crashed)));
        unwind.armed = false;
        result
    }
    pub fn resume(&self) -> Result<(), MediaSandboxError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .try_send(actor::Command::Resume(reply))
            .map_err(|_| MediaSandboxError::ResourceLimit)?;
        receiver.recv().unwrap_or(Err(MediaSandboxError::Crashed))
    }
    /// Close admission and wait for all native cleanup, keeping the spawner
    /// thread itself alive until process exit.
    pub fn shutdown(&self) -> Result<(), MediaSandboxError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(actor::Command::Close(reply))
            .map_err(|_| MediaSandboxError::Crashed)?;
        receiver.recv().map_err(|_| MediaSandboxError::Crashed)?
    }
    #[cfg(feature = "slow-tests")]
    #[must_use]
    pub fn witness(&self) -> WitnessSnapshot {
        WitnessSnapshot {
            supervisor: self.status.supervisor.load(Ordering::Acquire),
            shim: self.status.shim.load(Ordering::Acquire),
            payload: self.status.payload.load(Ordering::Acquire),
            ready: self.status.ready.load(Ordering::Acquire),
            incomplete_reply: self.status.incomplete_reply.load(Ordering::Acquire),
            contained: self.status.contained.load(Ordering::Acquire),
            observed_rss: self.status.observed_rss.load(Ordering::Acquire),
        }
    }
    #[cfg(feature = "slow-tests")]
    pub fn witness_abort_spawner(&self) -> Result<(), MediaSandboxError> {
        self.sender
            .send(actor::Command::Panic)
            .map_err(|_| MediaSandboxError::Crashed)
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            active: self.status.active.load(Ordering::Acquire),
            pending: self.status.pending.load(Ordering::Acquire),
            degraded: self.status.degraded.load(Ordering::Acquire),
            paused: self.status.paused.load(Ordering::Acquire),
        }
    }
}
struct CancelOnDrop {
    flag: Arc<AtomicBool>,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Input(File);
    impl Source for Input {
        fn file(&self) -> &File {
            &self.0
        }
        fn identity(&self) -> [u8; 32] {
            [1; 32]
        }
        fn check(&self) -> Result<(), MediaError> {
            Ok(())
        }
    }
    fn request() -> Request {
        Request {
            source: Box::new(Input(File::open("/dev/null").unwrap())),
            video: None,
            audio: None,
            cancel: Arc::new(AtomicBool::new(false)),
            explicit_retry: false,
        }
    }
    #[test]
    fn producer_reserves_the_global_pending_cap_before_the_actor_receives() {
        let (sender, receiver) = mpsc::sync_channel(PENDING_PROBES);
        let runtime = Runtime {
            sender,
            status: Arc::new(Status::default()),
        };
        std::thread::scope(|scope| {
            for _ in 0..PENDING_PROBES {
                let runtime = &runtime;
                scope.spawn(move || {
                    assert!(matches!(
                        runtime.probe(request()),
                        Err(Failure::Sandbox(MediaSandboxError::Crashed))
                    ));
                });
            }
            let mut admitted = Vec::with_capacity(PENDING_PROBES);
            for _ in 0..PENDING_PROBES {
                let Ok(command) = receiver.recv_timeout(Duration::from_secs(3)) else {
                    break;
                };
                admitted.push(command);
            }
            let reserved = runtime.snapshot().pending;
            let batch_len = admitted.len();
            let (send_result, rejection) = mpsc::sync_channel(1);
            let runtime_ref = &runtime;
            scope.spawn(move || {
                let _ = send_result.send(runtime_ref.probe(request()));
            });
            let outcome = rejection.recv_timeout(Duration::from_secs(1));
            // Release every backend-held reply before asserting, including if
            // a missing producer reservation made the 33rd caller block.
            drop(admitted);
            drop(receiver);
            assert_eq!(batch_len, PENDING_PROBES);
            assert_eq!(reserved, PENDING_PROBES);
            assert!(matches!(
                outcome,
                Ok(Err(Failure::Sandbox(MediaSandboxError::ResourceLimit)))
            ));
        });
    }
}
