//! One pipe owner per probe. Its guard retains cleanup across caller/worker loss.
use super::{
    CANCEL_GRACE, Failure, INITIAL_DEADLINE, Limits, Outcome, SCOPE_PROOF_ERRORS, Unavailable,
    UnavailableReason as Reason, controller::Handle,
};
use kinewright_job_sys::{
    Cgroup, ProcessHandle,
    media_boundary::{self, Mode},
    media_host, media_scope,
};
use kinewright_media_ipc::{Frame, MediaSandboxError as Error, Message, ProbeSession, write_frame};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::fs::DirBuilderExt,
    },
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Diagnostics only: which step each live probe is in, so a stalled test can
/// say which reply it waits for. Bounded and lock-free; carries no authority.
static WAITING: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
const PHASES: [&str; Phase::ResourceCheck as usize + 1] = [
    "launch",
    "supervisor CreateScope reply",
    "supervisor VerifyScope reply",
    "helper Forked",
    "helper Boundary",
    "helper Ready",
    "helper ProbeResult",
    "helper Complete",
    "helper close EOF",
    "cleanup: shim exit",
    "cleanup: session members reaped",
    "cleanup: shim reaped",
    "cleanup: scope empty",
    "runtime setup",
    "supervisor startup",
    "resource check",
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Launch,
    CreateScope,
    VerifyScope,
    Forked,
    Boundary,
    Ready,
    ProbeResult,
    Complete,
    CloseEof,
    ShimExit,
    Members,
    Reaped,
    ScopeEmpty,
    Runtime,
    Supervisor,
    ResourceCheck,
}
impl Phase {
    #[must_use]
    pub const fn unavailable(self, reason: Reason) -> Unavailable {
        Unavailable {
            phase: self,
            reason,
        }
    }
}
impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(PHASES[*self as usize])
    }
}
/// Writes one claimed `WAITING` slot: `shim pid << 8 | phase`.
#[derive(Clone, Copy)]
struct Tracer(Option<&'static AtomicU64>, u32);
impl Tracer {
    fn set(self, phase: Phase) {
        if let Some(slot) = self.0 {
            slot.store(u64::from(self.1) << 8 | phase as u64, Ordering::Release);
        }
    }
}
/// The lease's claim on a `WAITING` slot, cleared on drop.
struct Trace(Tracer);
impl Trace {
    fn claim(shim: u32) -> Self {
        let value = u64::from(shim) << 8 | Phase::Launch as u64;
        let slot = WAITING.iter().find(|slot| {
            slot.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        });
        Self(Tracer(slot, shim))
    }
    fn set(&self, phase: Phase) {
        self.0.set(phase);
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        if let Some(slot) = self.0.0 {
            slot.store(0, Ordering::Release);
        }
    }
}
/// Every live probe's shim pid and the step it is in (test diagnostics).
#[must_use]
pub fn diagnostics() -> String {
    use std::fmt::Write as _;
    let mut out = String::from("MS2_CLIENT_VIEW");
    for slot in &WAITING {
        let value = slot.load(Ordering::Acquire);
        if value != 0 {
            let phase = usize::try_from(value & 0xff).unwrap_or(usize::MAX);
            let _ = write!(
                out,
                " [shim={} waiting={}]",
                value >> 8,
                PHASES.get(phase).unwrap_or(&"?")
            );
        }
    }
    out
}

#[derive(Default)]
pub(super) struct Shared {
    pub cancel: AtomicBool,
    pub payload: AtomicU32,
    pub payload_start: AtomicU64,
    pub ready: AtomicBool,
    pub native_clean: AtomicBool,
    #[cfg(feature = "slow-tests")]
    pub incomplete_reply: AtomicBool,
    #[cfg(feature = "slow-tests")]
    pub contained: AtomicBool,
}
pub(super) struct Running {
    pub pin: Arc<ProcessHandle>,
    pub shared: Arc<Shared>,
    pub thread: JoinHandle<Done>,
}
pub(super) struct Done {
    pub result: Outcome,
    pub fallback: bool,
    /// Cleanup could not prove the helper scope empty. The scope was not
    /// released, and the actor keeps its decode slot reserved.
    pub unproven: bool,
}
pub(super) struct Launch<'a> {
    pub helper: &'a Path,
    pub limits: Limits,
    pub mode: Mode,
    pub controller: Handle,
    pub generation: u64,
    pub selected: [Option<u32>; 2],
    pub source: &'a File,
    pub spawner: Option<BorrowedFd<'a>>,
    #[cfg(feature = "slow-tests")]
    pub observer: Option<Arc<super::ProbeObserver>>,
    #[cfg(feature = "slow-tests")]
    pub held_cleanup: Option<Arc<AtomicBool>>,
    #[cfg(feature = "slow-tests")]
    pub scope_seam: Option<super::ScopeSeam>,
}
struct Root(PathBuf);
impl Root {
    fn create(generation: u64) -> Result<Self, Unavailable> {
        let root = Self(std::env::temp_dir().join(format!(
            "kinewright-media-{}-{generation}",
            std::process::id()
        )));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .map_err(|error| Self::create_refusal(&error))?;
        Ok(root)
    }

    fn create_refusal(error: &io::Error) -> Unavailable {
        let reason = if matches!(
            error.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
        ) {
            Reason::ScratchDirectoryDenied
        } else {
            Reason::ScratchDirectory
        };
        Phase::Launch.unavailable(reason)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
struct Lease {
    child: Child,
    pin: Arc<ProcessHandle>,
    controller: Handle,
    scope: Option<u32>,
    group: Option<Cgroup>,
    _lifetime: File,
    _root: Root,
    complete: bool,
    reaped: bool,
    unproven: bool,
    trace: Trace,
    #[cfg(feature = "slow-tests")]
    shim_smaps: Option<File>,
    #[cfg(feature = "slow-tests")]
    seams: Seams,
}
/// Test-only cleanup and scope fault hooks a lease carries.
#[cfg(feature = "slow-tests")]
struct Seams {
    held_cleanup: Option<Arc<AtomicBool>>,
    scope: Option<super::ScopeSeam>,
}
#[cfg(feature = "slow-tests")]
impl Seams {
    fn new(held_cleanup: Option<Arc<AtomicBool>>, scope: Option<super::ScopeSeam>) -> Self {
        Self {
            held_cleanup,
            scope,
        }
    }
}

fn pin_child(child: Child) -> io::Result<(Child, Arc<ProcessHandle>)> {
    let pin = match ProcessHandle::open(child.id()) {
        Ok(pin) => Arc::new(pin),
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok((child, pin))
}

fn helper_pipe() -> Result<(OwnedFd, OwnedFd), Unavailable> {
    use rustix::pipe::{PipeFlags, pipe_with};
    pipe_with(PipeFlags::CLOEXEC).map_err(|_| Phase::Launch.unavailable(Reason::HelperPipes))
}

pub(super) fn start(launch: Launch<'_>) -> Result<Running, Unavailable> {
    let launch_error = |reason| move |_| Phase::Launch.unavailable(reason);
    media_host::readonly_source(launch.source).map_err(launch_error(Reason::SourceDescriptor))?;
    let root = Root::create(launch.generation)?;
    let (command_read, command_write) = helper_pipe()?;
    let (response_read, response_write) = helper_pipe()?;
    let (lifetime_read, lifetime_write) = helper_pipe()?;
    let (release_read, release_write) = helper_pipe()?;
    let source = launch
        .source
        .try_clone()
        .map_err(launch_error(Reason::DuplicateSource))?;
    let child = media_host::spawn_helper(
        launch.helper,
        &root.0,
        launch.mode,
        launch.limits.address_space,
        launch.limits.allocation,
        [
            command_read.as_fd(),
            response_write.as_fd(),
            lifetime_read.as_fd(),
            source.as_fd(),
            release_read.as_fd(),
        ],
        launch.spawner,
    )
    .map_err(|error| super::spawn_refusal(&error, Phase::Launch, Reason::HelperSpawn))?;
    drop((command_read, response_write, lifetime_read, release_read));
    let (child, pin) = pin_child(child).map_err(launch_error(Reason::HelperPin))?;
    // Actor signalling and worker cleanup share the same pinned identity. Neither
    // resolves a numeric PID to signal it after this point.
    let observed_pin = Arc::clone(&pin);
    let shared = Arc::new(Shared::default());
    let observed_shared = Arc::clone(&shared);
    let trace = Trace::claim(pin.pid());
    let mut lease = Lease {
        child,
        pin,
        controller: launch.controller,
        scope: None,
        group: None,
        _lifetime: File::from(lifetime_write),
        _root: root,
        complete: false,
        reaped: false,
        unproven: false,
        trace,
        #[cfg(feature = "slow-tests")]
        shim_smaps: None,
        #[cfg(feature = "slow-tests")]
        seams: Seams::new(launch.held_cleanup, launch.scope_seam),
    };
    #[cfg(feature = "slow-tests")]
    if launch.observer.is_some() {
        lease.shim_smaps =
            Some(media_host::open_smaps(&lease.pin).map_err(launch_error(Reason::MemoryWitness))?);
    }
    let generation = launch.generation;
    let selected = launch.selected;
    let limits = launch.limits;
    let mode = launch.mode;
    #[cfg(feature = "slow-tests")]
    let observer = launch.observer;
    let thread = std::thread::Builder::new()
        .name("media-probe-io".into())
        .spawn(move || {
            let end = Instant::now() + INITIAL_DEADLINE + CANCEL_GRACE;
            let mut command = File::from(command_write);
            let mut response = File::from(response_read);
            let mut release = File::from(release_write);
            let mut fallback = false;
            let mut protocol = Protocol {
                command: &mut command,
                response: &mut response,
                shared: &shared,
                end,
                generation,
                cancelled: false,
                traffic: ProbeSession::new(1, generation, selected),
                tracer: lease.trace.0,
            };
            let result = lease.probe(
                &Settings {
                    mode,
                    limits,
                    selected,
                    #[cfg(feature = "slow-tests")]
                    observer,
                },
                &source,
                &mut release,
                &mut protocol,
                &mut fallback,
            );
            lease.finish(result, fallback, &shared, end)
        })
        .map_err(launch_error(Reason::ProbeThread))?;
    Ok(Running {
        pin: observed_pin,
        shared: observed_shared,
        thread,
    })
}
impl Lease {
    fn owned_group(&mut self) -> io::Result<()> {
        let Some(scope) = self.scope else {
            return Ok(());
        };
        let group = media_scope::membership(self.pin.pid())?;
        let name = format!(
            "kinewright-media-{}-{}-{}-{}-{scope}.scope",
            std::process::id(),
            self.controller.pin.pid(),
            self.controller.pin.start(),
            self.controller.generation
        );
        if group.file_name().and_then(|part| part.to_str()) == Some(name.as_str()) {
            media_scope::outside(std::process::id(), &group)?;
            if media_scope::membership(self.controller.pin.pid())?.starts_with(&group) {
                return Err(io::Error::other("supervisor entered helper scope"));
            }
            self.group = Some(Cgroup::open(&group)?);
            #[cfg(feature = "slow-tests")]
            if self.seams.scope == Some(super::ScopeSeam::HostedKernel)
                && let Some(group) = self.group.as_mut()
            {
                group.hosted_link_count(2);
            }
        }
        Ok(())
    }
    fn join_scope(&mut self, limits: Limits, fallback: &mut bool) -> Result<(), Failure> {
        let bad = || Failure::Sandbox(Error::ProtocolViolation);
        self.trace.set(Phase::CreateScope);
        match self
            .controller
            .call(Message::CreateScope {
                pid: self.pin.pid(),
                memory_bytes: limits.private_memory,
            })
            .map_err(Failure::Sandbox)?
        {
            Message::ScopeCreated { scope } => self.scope = Some(scope),
            Message::ScopeCreateFailed { scope, .. } => {
                self.scope = Some(scope);
                let _ = self.owned_group();
                *fallback = true;
                return Err(Phase::CreateScope
                    .unavailable(Reason::ScopeCreateFailed)
                    .into());
            }
            Message::ScopeUnavailable { scope, reason } => {
                self.scope = scope;
                *fallback = true;
                return Err(Phase::CreateScope
                    .unavailable(Reason::ScopeUnavailable(reason))
                    .into());
            }
            Message::Failed {
                error: Error::SandboxUnavailable,
            } => {
                *fallback = true;
                return Err(Phase::CreateScope
                    .unavailable(Reason::ScopeCreateFailed)
                    .into());
            }
            Message::Failed { error } => return Err(Failure::Sandbox(error)),
            _ => return Err(bad()),
        }
        self.owned_group().map_err(|_| bad())?;
        let scope = self.scope.ok_or_else(bad)?;
        self.trace.set(Phase::VerifyScope);
        match self
            .controller
            .call(Message::VerifyScope { scope })
            .map_err(Failure::Sandbox)?
        {
            Message::ScopeVerified {
                scope: actual,
                starttime,
            } if actual == scope && starttime == self.pin.start() => {}
            Message::ScopeUnavailable { reason, .. } => {
                *fallback = true;
                return Err(Phase::VerifyScope
                    .unavailable(Reason::ScopeUnavailable(reason))
                    .into());
            }
            Message::Failed {
                error: error @ (Error::SandboxUnavailable | Error::TimedOut),
            } => {
                *fallback = true;
                let reason = if error == Error::TimedOut {
                    Reason::ScopeVerifyTimedOut
                } else {
                    Reason::ScopeVerifyFailed
                };
                return Err(Phase::VerifyScope.unavailable(reason).into());
            }
            Message::Failed { error } => return Err(Failure::Sandbox(error)),
            _ => return Err(bad()),
        }
        self.owned_group().map_err(|_| bad())?;
        let group = self.group.as_ref().ok_or_else(bad)?;
        #[cfg(feature = "slow-tests")]
        if self.seams.scope == Some(super::ScopeSeam::AttachedReportedFailed) {
            // Attached and owned, then reported failed: the fallback shape of
            // a reply lost after attach, with the scope held by this lease.
            *fallback = true;
            return Err(Phase::VerifyScope
                .unavailable(Reason::ScopeVerifyFailed)
                .into());
        }
        media_scope::verify_controls(group, limits.private_memory).map_err(|_| bad())?;
        let members = group.members().map_err(|_| bad())?;
        if members.len() != 1
            || members[0].pid() != self.pin.pid()
            || members[0].start() != self.pin.start()
        {
            return Err(bad());
        }
        Ok(())
    }
    fn probe(
        &mut self,
        settings: &Settings,
        source: &File,
        release: &mut File,
        protocol: &mut Protocol<'_>,
        fallback: &mut bool,
    ) -> Outcome {
        let generation = protocol.generation;
        let bad = || Failure::Sandbox(Error::ProtocolViolation);
        if settings.mode == Mode::Scope {
            self.join_scope(settings.limits, fallback)?;
        }
        #[cfg(feature = "slow-tests")]
        protocol.shared.contained.store(true, Ordering::Release);
        release
            .write_all(&[1])
            .map_err(|_| Failure::Sandbox(Error::Crashed))?;
        let pid = protocol.receive_forked()?;
        let payload = media_scope::child(self.pin.pid(), pid).map_err(|_| bad())?;
        protocol
            .shared
            .payload_start
            .store(payload.start(), Ordering::Relaxed);
        protocol.shared.payload.store(pid, Ordering::Release);
        protocol.send(Message::EnterBoundary {})?;
        let boundary = protocol.receive(Phase::Boundary)?;
        if boundary.request != 1 || boundary.generation != generation {
            return Err(bad());
        }
        let Message::Boundary { landlock } = boundary.message else {
            return Err(bad());
        };
        if !landlock {
            eprintln!("MS2_LANDLOCK_UNAVAILABLE namespace_seccomp_required=true");
        }
        let observation = media_host::observe_boundary(&self.pin, &payload, source)
            .map_err(|_| Phase::Boundary.unavailable(Reason::BoundaryObservation))?;
        #[cfg(feature = "slow-tests")]
        let mut observation = observation;
        for property in &observation.unobservable {
            eprintln!("MS2_BOUNDARY_UNOBSERVABLE {property}");
        }
        if let Some(group) = &self.group
            && media_scope::membership(pid).map_err(|_| bad())? != group.path()
        {
            return Err(bad());
        }
        protocol.send(Message::ConfirmBoundary {})?;
        let ready = protocol.receive(Phase::Ready)?;
        protocol.traffic.accept(ready).map_err(Failure::Sandbox)?;
        protocol.shared.ready.store(true, Ordering::Release);
        protocol.send(Message::Probe {
            video: settings.selected[0],
            audio: settings.selected[1],
        })?;
        let result_frame = protocol.receive(Phase::ProbeResult)?;
        let result = protocol
            .traffic
            .accept(result_frame)
            .map_err(Failure::Sandbox)?;
        let result = result.ok_or_else(bad)?;
        #[cfg(feature = "slow-tests")]
        protocol
            .shared
            .incomplete_reply
            .store(true, Ordering::Release);
        let complete = protocol.receive(Phase::Complete)?;
        if !matches!(complete.message, Message::Complete {}) {
            return Err(bad());
        }
        protocol
            .traffic
            .accept(complete)
            .map_err(Failure::Sandbox)?;
        #[cfg(feature = "slow-tests")]
        if let Some(observer) = &settings.observer {
            let failure = Phase::Complete.unavailable(Reason::MemoryWitness);
            let unavailable = |_| failure;
            let shim_smaps = self.shim_smaps.as_mut().ok_or(failure)?;
            let payload_smaps = observation.smaps.as_mut().ok_or(failure)?;
            let memory = super::ProbeMemory {
                shim: media_host::memory_profile(&self.pin, shim_smaps).map_err(unavailable)?,
                payload: media_host::memory_profile(&payload, payload_smaps)
                    .map_err(unavailable)?,
                cgroup: self
                    .group
                    .as_ref()
                    .map(cgroup_memory)
                    .transpose()
                    .map_err(unavailable)?,
            };
            observer(memory).map_err(unavailable)?;
        }
        protocol.close()?;
        // observation (including the retained smaps fd) stays live through the
        // whole native probe; measurement capture is added before limit selection.
        drop(observation);
        Ok(result)
    }
    fn finish(&mut self, result: Outcome, fallback: bool, shared: &Shared, end: Instant) -> Done {
        let orderly = result.is_ok();
        let success = self.cleanup(!orderly, end);
        shared.native_clean.store(true, Ordering::Release);
        self.release_scope();
        Done {
            result: if orderly && !success {
                Err(Failure::Sandbox(Error::Crashed))
            } else {
                result
            },
            fallback,
            unproven: self.unproven,
        }
    }

    fn cleanup(&mut self, force: bool, end: Instant) -> bool {
        if force {
            let _ = self.pin.kill();
        }
        self.trace.set(Phase::ShimExit);
        while !media_host::exited(&self.pin).unwrap_or(false) {
            if Instant::now() >= end {
                let _ = self.pin.kill();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        #[cfg(feature = "slow-tests")]
        while self
            .seams
            .held_cleanup
            .as_ref()
            .is_some_and(|held| held.load(Ordering::Acquire))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.trace.set(Phase::Members);
        loop {
            if let Ok(members) = media_host::session_members(&self.pin) {
                for process in &members {
                    let _ = process.kill();
                }
                let clean = members
                    .iter()
                    .all(|process| media_host::reap_adopted(process).unwrap_or(false));
                if clean {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut success = false;
        self.trace.set(Phase::Reaped);
        while !self.reaped {
            match self.child.wait() {
                Ok(status) => {
                    self.reaped = true;
                    success = status.success();
                    if !success {
                        eprintln!("MS2_HELPER_EXIT shim={} status={status}", self.pin.pid());
                    }
                }
                Err(_) if self.pin.reaped().unwrap_or(false) => {
                    self.reaped = true;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        self.trace.set(Phase::ScopeEmpty);
        #[cfg(feature = "slow-tests")]
        self.await_hosted_removal();
        #[cfg(feature = "slow-tests")]
        let broken = matches!(
            self.seams.scope,
            Some(super::ScopeSeam::PersistentError | super::ScopeSeam::AttachedReportedFailed)
        );
        #[cfg(not(feature = "slow-tests"))]
        let broken = false;
        if let Some(group) = self.group.as_mut() {
            // Ok(false) is "not yet": members are still leaving. An error is
            // "unproven" and is tolerated only for SCOPE_PROOF_ERRORS in a row.
            let mut first_error = None;
            loop {
                let empty = if broken {
                    Err(io::Error::other("injected scope proof error"))
                } else {
                    group.empty()
                };
                match empty {
                    Ok(true) => break,
                    Ok(false) => first_error = None,
                    Err(error) => {
                        let errors_since = *first_error.get_or_insert_with(Instant::now);
                        if errors_since.elapsed() >= SCOPE_PROOF_ERRORS {
                            eprintln!(
                                "MS2_CLEANUP_UNPROVEN shim={} error={error} reservation=retained",
                                self.pin.pid()
                            );
                            self.unproven = true;
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.complete = self.reaped;
        success && !self.unproven
    }

    /// Hosted-kernel seam: wait (bounded) until the manager has removed the
    /// empty scope, so the proof below must go through the removed cgroup's
    /// readings, and print them.
    #[cfg(feature = "slow-tests")]
    fn await_hosted_removal(&self) {
        let (Some(group), Some(super::ScopeSeam::HostedKernel)) = (&self.group, self.seams.scope)
        else {
            return;
        };
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            match group.deletion_reading() {
                Ok(reading)
                    if matches!(reading.events, kinewright_job_sys::CgroupEvents::Removed) =>
                {
                    let text = reading.to_string();
                    println!(
                        "MS2_SCOPE_PROOF seam=hosted_kernel {text} deleted={:?}",
                        reading.deleted()
                    );
                    return;
                }
                Ok(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
                other => {
                    println!("MS2_SCOPE_PROOF seam=hosted_kernel not_removed={other:?}");
                    return;
                }
            }
        }
    }

    fn release_scope(&mut self) {
        // Never release a scope whose emptiness is unproven. It stays
        // reserved, and the actor keeps the decode slot (`Done::unproven`).
        if self.unproven {
            return;
        }
        if let Some(scope) = self.scope.take() {
            self.controller.release_scope(scope);
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.complete {
            self.cleanup(true, Instant::now());
        }
        self.release_scope();
    }
}
fn send(writer: &mut File, generation: u64, message: Message) -> Result<(), Failure> {
    write_frame(
        writer,
        &Frame {
            request: 1,
            generation,
            message,
        },
    )
    .map_err(|_| Failure::Sandbox(Error::Crashed))
}
struct Interruptible<'a> {
    file: &'a mut File,
    command: &'a mut File,
    shared: &'a Shared,
    end: Instant,
    generation: u64,
    cancelled: &'a mut bool,
}
impl Read for Interruptible<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        loop {
            if Instant::now() >= self.end {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "probe deadline"));
            }
            if self.shared.cancel.load(Ordering::Acquire) && !*self.cancelled {
                send(self.command, self.generation, Message::Cancel {})
                    .map_err(io::Error::other)?;
                *self.cancelled = true;
            }
            match media_boundary::wait_readable(self.file.as_fd(), Duration::from_millis(100)) {
                Ok(()) => return self.file.read(output),
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error),
            }
        }
    }
}
struct Settings {
    mode: Mode,
    limits: Limits,
    selected: [Option<u32>; 2],
    #[cfg(feature = "slow-tests")]
    observer: Option<Arc<super::ProbeObserver>>,
}
struct Protocol<'a> {
    response: &'a mut File,
    command: &'a mut File,
    shared: &'a Shared,
    end: Instant,
    generation: u64,
    cancelled: bool,
    traffic: ProbeSession,
    tracer: Tracer,
}
impl Protocol<'_> {
    fn send(&mut self, message: Message) -> Result<(), Failure> {
        send(self.command, self.generation, message)
    }
    fn receive_forked(&mut self) -> Result<u32, Failure> {
        let forked = self.receive(Phase::Forked)?;
        if (forked.request, forked.generation) != (1, 1) {
            return Err(Failure::Sandbox(Error::ProtocolViolation));
        }
        match forked.message {
            Message::Forked { pid } => Ok(pid),
            Message::Failed {
                error: Error::SandboxUnavailable,
            } => Err(Phase::Forked
                .unavailable(Reason::RequiredNamespacesUnavailable)
                .into()),
            Message::Failed {
                error: Error::NamespacePolicyDenied,
            } => Err(Phase::Forked
                .unavailable(Reason::NamespacePolicyDenied)
                .into()),
            _ => Err(Failure::Sandbox(Error::ProtocolViolation)),
        }
    }
    fn receive(&mut self, phase: Phase) -> Result<Frame, Failure> {
        self.tracer.set(phase);
        let mut reader = Interruptible {
            file: self.response,
            command: self.command,
            shared: self.shared,
            end: self.end,
            generation: self.generation,
            cancelled: &mut self.cancelled,
        };
        let frame = self.traffic.read(&mut reader, Instant::now());
        if self.cancelled {
            self.traffic.cancel_sent();
        }
        frame.map_err(|error| match error.kind() {
            io::ErrorKind::InvalidData => Failure::Sandbox(Error::ProtocolViolation),
            io::ErrorKind::TimedOut => Failure::Sandbox(Error::TimedOut),
            kind if !self.shared.ready.load(Ordering::Acquire) => {
                let reason = if kind == io::ErrorKind::UnexpectedEof {
                    Reason::EarlyEof
                } else {
                    Reason::BootstrapRead
                };
                phase.unavailable(reason).into()
            }
            _ => Failure::Sandbox(Error::Crashed),
        })
    }
    fn close(&mut self) -> Result<(), Failure> {
        self.send(Message::Close {})?;
        self.tracer.set(Phase::CloseEof);
        let mut extra = [0];
        let mut reader = Interruptible {
            file: self.response,
            command: self.command,
            shared: self.shared,
            end: self.end,
            generation: self.generation,
            cancelled: &mut self.cancelled,
        };
        if reader
            .read(&mut extra)
            .map_err(|_| Failure::Sandbox(Error::Crashed))?
            != 0
        {
            return Err(Failure::Sandbox(Error::ProtocolViolation));
        }
        Ok(())
    }
}

#[cfg(feature = "slow-tests")]
fn cgroup_memory(group: &Cgroup) -> io::Result<super::CgroupMemory> {
    let read = |name: &str| -> io::Result<String> {
        let mut text = String::new();
        File::open(group.path().join(name))?
            .take(4097)
            .read_to_string(&mut text)?;
        if text.len() > 4096 {
            return Err(io::Error::other("oversized cgroup counter"));
        }
        Ok(text)
    };
    let scalar = |name| -> io::Result<u64> { read(name)?.trim().parse().map_err(io::Error::other) };
    let text = read("memory.events")?;
    let mut events = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let name = fields
            .next()
            .ok_or_else(|| io::Error::other("memory event name"))?;
        let value = fields
            .next()
            .ok_or_else(|| io::Error::other("memory event value"))?
            .parse()
            .map_err(io::Error::other)?;
        if fields.next().is_some() || events.len() == 32 {
            return Err(io::Error::other("memory event shape"));
        }
        events.push((name.to_owned(), value));
    }
    Ok(super::CgroupMemory {
        current: scalar("memory.current")?,
        peak: scalar("memory.peak")?,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_directory_denial_is_unsupported() {
        use rustix::io::Errno;

        for errno in [Errno::PERM, Errno::ACCESS, Errno::ROFS] {
            let refusal = Root::create_refusal(&errno.into());
            assert_eq!(
                refusal,
                Phase::Launch.unavailable(Reason::ScratchDirectoryDenied)
            );
            assert!(refusal.is_unsupported());
        }
        let refusal = Root::create_refusal(&Errno::NOSPC.into());
        assert_eq!(refusal, Phase::Launch.unavailable(Reason::ScratchDirectory));
        assert!(!refusal.is_unsupported());
    }

    #[test]
    fn pre_fork_reply_carries_a_policy_denial() {
        let (reader, writer) = helper_pipe().unwrap();
        let mut response = File::from(reader);
        write_frame(
            &mut File::from(writer),
            &Frame {
                request: 1,
                generation: 1,
                message: Message::Failed {
                    error: Error::NamespacePolicyDenied,
                },
            },
        )
        .unwrap();
        let mut command = File::options().write(true).open("/dev/null").unwrap();
        let shared = Shared::default();
        let mut protocol = Protocol {
            response: &mut response,
            command: &mut command,
            shared: &shared,
            end: Instant::now() + INITIAL_DEADLINE,
            generation: 2,
            cancelled: false,
            traffic: ProbeSession::new(1, 2, [None, None]),
            tracer: Tracer(None, 0),
        };
        let failure = protocol.receive_forked().unwrap_err();
        assert!(
            matches!(
                failure,
                Failure::Unavailable(Unavailable {
                    phase: Phase::Forked,
                    reason: Reason::NamespacePolicyDenied,
                })
            ),
            "{failure:?}"
        );
    }

    #[test]
    fn bootstrap_eof_preserves_the_phase_and_stays_retryable() {
        let mut response = File::open("/dev/null").unwrap();
        let mut command = File::options().write(true).open("/dev/null").unwrap();
        let shared = Shared::default();
        let mut protocol = Protocol {
            response: &mut response,
            command: &mut command,
            shared: &shared,
            end: Instant::now() + INITIAL_DEADLINE,
            generation: 2,
            cancelled: false,
            traffic: ProbeSession::new(1, 2, [None, None]),
            tracer: Tracer(None, 0),
        };
        for waiting in [Phase::Forked, Phase::Boundary, Phase::Ready] {
            let error = protocol.receive(waiting).unwrap_err();
            assert!(
                matches!(
                    error,
                    Failure::Unavailable(Unavailable {
                        phase,
                        reason: Reason::EarlyEof,
                    }) if phase == waiting
                ),
                "{error:?}"
            );
        }
    }
}
