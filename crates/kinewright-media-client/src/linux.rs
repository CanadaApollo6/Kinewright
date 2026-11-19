#![allow(clippy::missing_errors_doc)]
#[cfg(test)]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

use kinewright_core::MediaError;
use kinewright_media_ipc::{MediaSandboxError, ValidatedProbe};
use std::{
    fmt,
    fs::File,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

mod actor;
mod admission;
mod controller;
mod worker;
pub use worker::{Phase, diagnostics};

pub const ACTIVE_PROBES: usize = 2;
/// Queue bound (ruling 35): 32 requests behind two slots. The queue deadline
/// is independent of launch timing and still applies while cleanup is pending.
pub const PENDING_PROBES: usize = 32;
pub const QUEUE_WAIT: Duration = Duration::from_secs(120);
const INITIAL_DEADLINE: Duration = Duration::from_secs(5);
const CANCEL_GRACE: Duration = Duration::from_millis(250);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_millis(100);
/// After the kill, termination, reap and native emptiness get this long before
/// the caller receives its typed failure (design: "Cancellation and kill",
/// `docs/MS2-MEDIA-SANDBOX.md` performance table). The reservation stays until
/// cleanup really ends.
const REAP_BUDGET: Duration = Duration::from_secs(2);
/// Consecutive emptiness-proof errors tolerated before cleanup stops polling
/// and keeps the scope reserved as unproven. An error from the retained
/// descriptors is "unproven", not "not yet": no later read makes a broken
/// descriptor answer again, so the bound only rides out a one-off failed
/// read. It sits well inside `REAP_BUDGET`, so the caller's typed failure is
/// not delayed by it.
const SCOPE_PROOF_ERRORS: Duration = Duration::from_millis(500);

/// The actor owns this capability until native cleanup, independently of the
/// caller's lifetime. Only a duplicate file descriptor reaches the I/O worker.
pub trait Source: Send {
    fn file(&self) -> &File;
    fn identity(&self) -> [u8; 32];
    fn check(&self) -> Result<(), MediaError>;
}

/// Native address-space ceiling, kernel/RSS ceiling, and the decoder component
/// inside that reservation. No component may borrow the runtime headroom.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub address_space: u64,
    pub private_memory: u64,
    pub allocation: u64,
}
impl Limits {
    /// Qualified release probe measurements and margins: docs/reports/ms2-s1a.md.
    pub const PROBE: Self = Self {
        address_space: 1536 * 1024 * 1024,
        private_memory: 1024 * 1024 * 1024,
        allocation: 896 * 1024 * 1024,
    };
}
pub struct Config {
    pub helper: PathBuf,
    pub supervisor: PathBuf,
    pub limits: Limits,
    #[cfg(feature = "slow-tests")]
    observer: Option<Arc<ProbeObserver>>,
    #[cfg(feature = "slow-tests")]
    held_cleanup: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(feature = "slow-tests")]
    scope_seam: Option<ScopeSeam>,
}
/// Witness seams on the helper scope's native cleanup proof.
#[cfg(feature = "slow-tests")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeSeam {
    /// The retained scope cgroup reports two links after removal, as the
    /// hosted kernels do. Cleanup waits for the removal, prints
    /// `MS2_SCOPE_PROOF` with both readings, then proves emptiness.
    HostedKernel,
    /// Every emptiness proof of the helper scope answers an error, as a
    /// retained descriptor that the kernel or manager broke would.
    PersistentError,
    /// The shim is attached to its scope and the lease owns that cgroup, but
    /// the run then takes the scope fallback, as `ScopeCreateFailed` with the
    /// scope id does when the manager's reply is lost after attach. Every
    /// emptiness proof of that scope then errors as with `PersistentError`.
    AttachedReportedFailed,
}
impl Config {
    #[must_use]
    pub fn new(helper: PathBuf, supervisor: PathBuf, limits: Limits) -> Self {
        Self {
            helper,
            supervisor,
            limits,
            #[cfg(feature = "slow-tests")]
            observer: None,
            #[cfg(feature = "slow-tests")]
            held_cleanup: None,
            #[cfg(feature = "slow-tests")]
            scope_seam: None,
        }
    }
    #[cfg(feature = "slow-tests")]
    #[must_use]
    pub fn scope_seam(mut self, seam: ScopeSeam) -> Self {
        self.scope_seam = Some(seam);
        self
    }
    #[cfg(feature = "slow-tests")]
    #[must_use]
    pub fn observe(mut self, observer: Arc<ProbeObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
    /// While `held` is set, each worker waits after its shim has exited and
    /// before native membership cleanup: a cleanup that does not end.
    #[cfg(feature = "slow-tests")]
    #[must_use]
    pub fn hold_cleanup(mut self, held: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.held_cleanup = Some(held);
        self
    }
}
#[cfg(feature = "slow-tests")]
pub type ProbeObserver = dyn Fn(ProbeMemory) -> std::io::Result<()> + Send + Sync;
#[cfg(feature = "slow-tests")]
#[derive(Debug)]
pub struct CgroupMemory {
    pub current: u64,
    pub peak: u64,
    pub events: Vec<(String, u64)>,
}
#[cfg(feature = "slow-tests")]
#[derive(Debug)]
pub struct ProbeMemory {
    pub shim: kinewright_job_sys::media_host::MemoryProfile,
    pub payload: kinewright_job_sys::media_host::MemoryProfile,
    pub cgroup: Option<CgroupMemory>,
}
pub struct Request {
    pub source: Box<dyn Source>,
    pub video: Option<u32>,
    pub audio: Option<u32>,
    pub cancel: Arc<AtomicBool>,
    pub explicit_retry: bool,
}
/// Typed, retryable admission refusals. None of them launched a helper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Busy {
    /// More than `PENDING_PROBES` requests are waiting.
    QueueFull,
    /// The request waited `QUEUE_WAIT` without reaching a slot.
    QueueWait,
    /// Repeated unhealthy launches paused admission; the first request after
    /// `retry_after` resumes it.
    Paused { retry_after: Duration },
}

/// An admission refusal diagnosed by the host or by trusted bootstrap before
/// the helper receives permission to parse media.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub phase: Phase,
    pub reason: UnavailableReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    NotConfigured,
    AlreadyConfigured,
    InvalidConfiguration,
    SpawnerThread,
    Subreaper,
    SpawnerPidfd,
    SupervisorSocket,
    SupervisorSpawn,
    SupervisorPin,
    SupervisorHandshake,
    SupervisorHandshakeTimedOut,
    SupervisorThread,
    SupervisorExited,
    SourceDescriptor,
    ScratchDirectory,
    ScratchDirectoryDenied,
    HelperPipes,
    DuplicateSource,
    HelperSpawn,
    ExecutableUnavailable,
    HelperPin,
    ProbeThread,
    ScopeCreateFailed,
    ScopeUnavailable(kinewright_media_ipc::ScopeUnavailableReason),
    ScopeVerifyFailed,
    ScopeVerifyTimedOut,
    RequiredNamespacesUnavailable,
    NamespacePolicyDenied,
    EarlyEof,
    BootstrapRead,
    BoundaryObservation,
    ResidentMemory,
    #[cfg(feature = "slow-tests")]
    MemoryWitness,
}

impl Unavailable {
    /// Configuration and runtime prerequisites that a new probe cannot change.
    #[must_use]
    pub const fn is_unsupported(self) -> bool {
        matches!(
            self.reason,
            UnavailableReason::NotConfigured
                | UnavailableReason::AlreadyConfigured
                | UnavailableReason::InvalidConfiguration
                | UnavailableReason::Subreaper
                | UnavailableReason::SpawnerPidfd
                | UnavailableReason::ScratchDirectoryDenied
                | UnavailableReason::ExecutableUnavailable
                | UnavailableReason::RequiredNamespacesUnavailable
                | UnavailableReason::NamespacePolicyDenied
        )
    }
}

fn spawn_refusal(error: &io::Error, phase: Phase, retryable: UnavailableReason) -> Unavailable {
    use rustix::io::Errno;

    let reason = match Errno::from_io_error(error) {
        Some(
            Errno::NOENT
            | Errno::ACCESS
            | Errno::PERM
            | Errno::NOEXEC
            | Errno::NOTDIR
            | Errno::LOOP,
        ) => UnavailableReason::ExecutableUnavailable,
        _ => retryable,
    };
    phase.unavailable(reason)
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "media sandbox unavailable ({}, {:?})",
            self.phase, self.reason
        )?;
        if !self.is_unsupported() {
            f.write_str(". Try again.")?;
        }
        Ok(())
    }
}

impl std::error::Error for Unavailable {}

#[derive(Debug)]
pub enum Failure {
    Sandbox(MediaSandboxError),
    Unavailable(Unavailable),
    Authority(MediaError),
    Busy(Busy),
}
impl From<Unavailable> for Failure {
    fn from(error: Unavailable) -> Self {
        Self::Unavailable(error)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sandbox(error) => write!(f, "media sandbox: {error:?}"),
            Self::Unavailable(error) => error.fmt(f),
            Self::Authority(error) => error.fmt(f),
            Self::Busy(Busy::QueueFull) => {
                f.write_str("media admission queue is full; try again shortly")
            }
            Self::Busy(Busy::QueueWait) => {
                f.write_str("media admission waited too long for a free helper; try again shortly")
            }
            Self::Busy(Busy::Paused { retry_after }) => write!(
                f,
                "media admission paused after repeated helper failures; try again in {} s",
                retry_after.as_secs().max(1)
            ),
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
    #[cfg(feature = "slow-tests")]
    launches: std::sync::atomic::AtomicU32,
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
    /// Helper launches since start, relaunches included.
    pub launches: u32,
}
pub struct Runtime {
    sender: SyncSender<actor::Command>,
    status: Arc<Status>,
}
static STARTED: AtomicBool = AtomicBool::new(false);

impl Runtime {
    pub fn start(config: Config) -> Result<Self, Unavailable> {
        let valid = config.helper.is_absolute()
            && config.supervisor.is_absolute()
            && (64 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&config.limits.address_space)
            && (64 * 1024 * 1024..=config.limits.address_space)
                .contains(&config.limits.private_memory)
            && (32 * 1024 * 1024..=config.limits.private_memory)
                .contains(&config.limits.allocation);
        if !valid {
            return Err(Phase::Runtime.unavailable(UnavailableReason::InvalidConfiguration));
        }
        if STARTED.swap(true, Ordering::AcqRel) {
            return Err(Phase::Runtime.unavailable(UnavailableReason::AlreadyConfigured));
        }
        let (sender, receiver) = mpsc::sync_channel(PENDING_PROBES + 1);
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
            return Err(Phase::Runtime.unavailable(UnavailableReason::SpawnerThread));
        }
        Ok(Self { sender, status })
    }
    /// Probe without a caller signal. The runtime still cancels on caller unwind.
    pub fn probe_until_deadline(
        &self,
        source: Box<dyn Source>,
        video: Option<u32>,
        audio: Option<u32>,
    ) -> Outcome {
        self.probe(Request {
            source,
            video,
            audio,
            cancel: Arc::default(),
            explicit_retry: false,
        })
    }

    pub fn probe(&self, request: Request) -> Outcome {
        let (reply, receiver) = mpsc::sync_channel(1);
        // Caller unwind revokes the request; actor cleanup ownership survives it.
        loop {
            let count = self.status.pending.load(Ordering::Acquire);
            if count >= PENDING_PROBES {
                return Err(Failure::Busy(Busy::QueueFull));
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
            .try_send(actor::Command::Probe(request, reply, Instant::now()))
            .is_err()
        {
            self.status.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(Failure::Busy(Busy::QueueFull));
        }
        unwind.armed = true;
        let result = receiver
            .recv()
            .unwrap_or(Err(Failure::Sandbox(MediaSandboxError::Crashed)));
        unwind.armed = false;
        result
    }
    /// Close admission and wait up to 15 seconds for native cleanup.
    /// On timeout the spawner retains any process that still needs cleanup.
    pub fn shutdown(&self) -> Result<(), MediaSandboxError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        // Probe ingress reserves one channel entry for this close command.
        self.sender
            .try_send(actor::Command::Close(reply))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => MediaSandboxError::ResourceLimit,
                mpsc::TrySendError::Disconnected(_) => MediaSandboxError::Crashed,
            })?;
        receiver
            .recv_timeout(SHUTDOWN_WAIT)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => MediaSandboxError::TimedOut,
                mpsc::RecvTimeoutError::Disconnected => MediaSandboxError::Crashed,
            })?
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
            launches: self.status.launches.load(Ordering::Acquire),
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

    #[test]
    fn spawn_refusal_separates_installation_from_resource_errors() {
        use UnavailableReason::{ExecutableUnavailable, HelperSpawn, SupervisorSpawn};
        use rustix::io::Errno;

        for (phase, retryable) in [
            (Phase::Launch, HelperSpawn),
            (Phase::Supervisor, SupervisorSpawn),
        ] {
            for errno in [
                Errno::NOENT,
                Errno::ACCESS,
                Errno::PERM,
                Errno::NOEXEC,
                Errno::NOTDIR,
                Errno::LOOP,
            ] {
                let refusal = spawn_refusal(&errno.into(), phase, retryable);
                assert_eq!(
                    refusal,
                    phase.unavailable(ExecutableUnavailable),
                    "{errno:?}"
                );
                assert!(refusal.is_unsupported());
            }
            for errno in [Errno::AGAIN, Errno::NOMEM, Errno::MFILE, Errno::NFILE] {
                let refusal = spawn_refusal(&errno.into(), phase, retryable);
                assert_eq!(refusal, phase.unavailable(retryable), "{errno:?}");
                assert!(!refusal.is_unsupported());
            }
        }
    }

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
        let _deadline = deadline::test();
        let (sender, receiver) = mpsc::sync_channel(PENDING_PROBES + 1);
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
            assert!(matches!(outcome, Ok(Err(Failure::Busy(Busy::QueueFull)))));
        });
    }
}
