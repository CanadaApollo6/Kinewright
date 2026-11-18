//! One pipe owner per probe. Its guard retains cleanup across caller/worker loss.
use super::{CANCEL_GRACE, Failure, INITIAL_DEADLINE, Limits, Outcome, controller::Handle};
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
        fd::{AsFd, BorrowedFd},
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
const PHASES: [&str; 13] = [
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
];
#[derive(Clone, Copy)]
enum Phase {
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
        let value = u64::from(shim) << 8;
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
}
struct Root(PathBuf);
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
    trace: Trace,
    #[cfg(feature = "slow-tests")]
    shim_smaps: Option<File>,
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

pub(super) fn start(launch: Launch<'_>) -> io::Result<Running> {
    use rustix::pipe::{PipeFlags, pipe_with};
    media_host::readonly_source(launch.source)?;
    let root = Root(std::env::temp_dir().join(format!(
        "kinewright-media-{}-{}",
        std::process::id(),
        launch.generation
    )));
    fs::DirBuilder::new().mode(0o700).create(&root.0)?;
    let (command_read, command_write) = pipe_with(PipeFlags::CLOEXEC)?;
    let (response_read, response_write) = pipe_with(PipeFlags::CLOEXEC)?;
    let (lifetime_read, lifetime_write) = pipe_with(PipeFlags::CLOEXEC)?;
    let (release_read, release_write) = pipe_with(PipeFlags::CLOEXEC)?;
    let source = launch.source.try_clone()?;
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
    )?;
    drop((command_read, response_write, lifetime_read, release_read));
    let (child, pin) = pin_child(child)?;
    // Actor signalling and worker cleanup share the same pinned identity. Neither
    // resolves a numeric PID to signal it after this point.
    let observed_pin = Arc::clone(&pin);
    let shared = Arc::new(Shared::default());
    let observed_shared = Arc::clone(&shared);
    let trace = Trace::claim(pin.pid());
    trace.set(Phase::Launch);
    let tracer = trace.0;
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
        trace,
        #[cfg(feature = "slow-tests")]
        shim_smaps: None,
    };
    #[cfg(feature = "slow-tests")]
    if launch.observer.is_some() {
        lease.shim_smaps = Some(media_host::open_smaps(&lease.pin)?);
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
                tracer,
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
            let result = lease.finish(result, &shared, end);
            Done { result, fallback }
        })?;
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
                return Err(Failure::Sandbox(Error::SandboxUnavailable));
            }
            Message::ScopeUnavailable { scope, .. } => {
                self.scope = scope;
                *fallback = true;
                return Err(Failure::Sandbox(Error::SandboxUnavailable));
            }
            Message::Failed {
                error: Error::SandboxUnavailable,
            } => {
                *fallback = true;
                return Err(Failure::Sandbox(Error::SandboxUnavailable));
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
            Message::ScopeUnavailable { .. }
            | Message::Failed {
                error: Error::SandboxUnavailable,
            } => {
                *fallback = true;
                return Err(Failure::Sandbox(Error::SandboxUnavailable));
            }
            Message::Failed { error } => return Err(Failure::Sandbox(error)),
            _ => return Err(bad()),
        }
        self.owned_group().map_err(|_| bad())?;
        let group = self.group.as_ref().ok_or_else(bad)?;
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
        let forked = protocol.receive(Phase::Forked)?;
        if (forked.request, forked.generation) != (1, 1) {
            return Err(bad());
        }
        let Message::Forked { pid } = forked.message else {
            return Err(bad());
        };
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
            .map_err(|_| Failure::Sandbox(Error::SandboxUnavailable))?;
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
            let unavailable = |_| Failure::Sandbox(Error::SandboxUnavailable);
            let shim_smaps = self
                .shim_smaps
                .as_mut()
                .ok_or(Failure::Sandbox(Error::SandboxUnavailable))?;
            let payload_smaps = observation
                .smaps
                .as_mut()
                .ok_or(Failure::Sandbox(Error::SandboxUnavailable))?;
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
    fn finish(&mut self, result: Outcome, shared: &Shared, end: Instant) -> Outcome {
        let orderly = result.is_ok();
        let success = self.cleanup(!orderly, end);
        shared.native_clean.store(true, Ordering::Release);
        self.release_scope();
        if orderly && !success {
            Err(Failure::Sandbox(Error::Crashed))
        } else {
            result
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
        if let Some(group) = self.group.as_mut() {
            while !group.empty().unwrap_or(false) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.complete = self.reaped;
        success
    }

    fn release_scope(&mut self) {
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
        frame.map_err(|error| {
            Failure::Sandbox(match error.kind() {
                io::ErrorKind::InvalidData => Error::ProtocolViolation,
                io::ErrorKind::TimedOut => Error::TimedOut,
                _ if !self.shared.ready.load(Ordering::Acquire) => Error::SandboxUnavailable,
                _ => Error::Crashed,
            })
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
