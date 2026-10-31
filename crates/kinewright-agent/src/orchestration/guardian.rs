//! Native execution adapter. The Studio owner implements durable admission.
//! No project writes or scheduling decisions happen in this module.
use crate::{
    execution_claim::MachineClaim,
    job_guardian::protocol::{
        self, AttemptIdentity, GuardianMessage, HostMessage, Program, RecoveryIdentity,
    },
};
use kinewright_job_sys::ProcessHandle;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

static SPAWN: Mutex<()> = Mutex::new(());

/// C3 must supply a certified filesystem/IPC profile before provider launch.
/// C2 supplies no production constructor. Containment alone is not a sandbox.
///
/// ```compile_fail
/// use kinewright_agent::guardian::CertifiedProfile;
/// let profile = CertifiedProfile {};
/// ```
pub struct CertifiedProfile {
    _private: (),
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) missing_nsdelegate: bool,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) missing_pid_namespace: bool,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) manager_process_only: bool,
}
impl CertifiedProfile {
    #[cfg(target_os = "linux")]
    pub(crate) fn linux_allowlist() -> Self {
        Self {
            _private: (),
            #[cfg(test)]
            missing_nsdelegate: false,
            #[cfg(test)]
            missing_pid_namespace: false,
            #[cfg(test)]
            manager_process_only: false,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn fixture() -> Self {
        Self {
            _private: (),
            #[cfg(target_os = "linux")]
            missing_nsdelegate: false,
            #[cfg(target_os = "linux")]
            missing_pid_namespace: false,
            #[cfg(target_os = "linux")]
            manager_process_only: false,
        }
    }
}

/// The existing serialized Studio owner supplies both durable acknowledgments.
/// An implementation must return success only after exact-envelope root ACK.
/// C2's native fixture implements this port without claiming Studio integration.
pub trait DurableAttemptOwner {
    /// # Errors
    /// Refuses an attempt whose reservation did not durably commit.
    fn reserve(&mut self, identity: &AttemptIdentity) -> io::Result<()>;
    /// Recheck live authority and persist the exact native recovery identity.
    ///
    /// # Errors
    /// Refuses uncertainty, stale authority, changed ownership, or an unacknowledged root.
    fn authorize_commit(&mut self, recovery: &RecoveryIdentity) -> io::Result<()>;
    /// Serialize the native COMMIT write with revocation. The default is for
    /// standalone owners without concurrent Studio commands.
    ///
    /// # Errors
    /// Refuses a revoked attempt or a failed COMMIT write.
    fn commit_authorized(
        &mut self,
        _recovery: &RecoveryIdentity,
        send: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        send()
    }
}

/// Shared access to the sole lifetime writer, without duplicating its descriptor.
/// Stop never needs the session/cleanup mutex to revoke a startup or live child.
#[derive(Clone)]
pub struct StartupControl(Arc<Mutex<Option<ChildStdin>>>);
impl StartupControl {
    pub fn close(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    #[must_use]
    pub fn closed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    }
    fn send(&self, message: &HostMessage) -> io::Result<()> {
        let mut writer = self
            .0
            .lock()
            .map_err(|_| io::Error::other("lifetime poisoned"))?;
        protocol::write_frame(
            writer
                .as_mut()
                .ok_or_else(|| io::Error::other("lifetime closed"))?,
            message,
        )
    }
}

/// Exact retirement evidence. Only native observation can construct it.
pub struct CleanupProof {
    identity: AttemptIdentity,
    recovery: RecoveryIdentity,
    guardian_lost: bool,
}
impl CleanupProof {
    #[must_use]
    pub fn identity(&self) -> &AttemptIdentity {
        &self.identity
    }
    #[must_use]
    pub fn recovery(&self) -> &RecoveryIdentity {
        &self.recovery
    }
    #[must_use]
    pub fn guardian_lost(&self) -> bool {
        self.guardian_lost
    }
}

/// No native identity was bound. The start fence and complete native observation grant proof.
#[cfg(target_os = "linux")]
pub struct UnboundCleanupProof {
    identity: AttemptIdentity,
}
#[cfg(target_os = "linux")]
impl UnboundCleanupProof {
    #[must_use]
    pub fn identity(&self) -> &AttemptIdentity {
        &self.identity
    }
}
/// A prepared, contained execution. Dropping it closes the sole lifetime writer
/// and retains its machine reservation for recovery rather than granting a slot.
pub struct ContainedSession {
    identity: AttemptIdentity,
    recovery: RecoveryIdentity,
    root: PathBuf,
    control: StartupControl,
    replies: Receiver<io::Result<GuardianMessage>>,
    launcher: Launcher,
    guardian: ProcessHandle,
    payload: ProcessHandle,
    #[cfg(target_os = "linux")]
    observer: kinewright_job_sys::Cgroup,
    #[cfg(target_os = "linux")]
    service: kinewright_job_sys::Cgroup,
    observed: Vec<ProcessHandle>,
    executing: bool,
    cleaned: bool,
    phase_us: [u64; 3],
    cleanup_us: Option<[u64; 3]>,
}

fn next_reply(
    receiver: &Receiver<io::Result<GuardianMessage>>,
    deadline: Instant,
) -> io::Result<GuardianMessage> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::other("guardian startup deadline"));
        }
        match receiver.try_recv() {
            Ok(Ok(GuardianMessage::Refused { reason })) => return Err(io::Error::other(reason)),
            Ok(result) => return result,
            Err(TryRecvError::Disconnected) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Err(TryRecvError::Empty) => {}
        }
        thread::sleep(Duration::from_millis(1));
    }
}
/// Low-level transport for the common native launcher and protocol fault witnesses.
/// The caller still owns durable attempt admission and cleanup reconciliation.
///
/// # Errors
/// Refuses unavailable supervision or an inheritable lifetime writer.
#[allow(
    clippy::type_complexity,
    reason = "the transport owns one process and two bounded pipe endpoints"
)]
pub fn spawn_transport(
    profile: &CertifiedProfile,
    guardian: &Path,
    root: &Path,
    identity: &AttemptIdentity,
    claim: &mut MachineClaim,
) -> io::Result<(Launcher, ChildStdin, Receiver<io::Result<GuardianMessage>>)> {
    let _ = profile;
    #[cfg(target_os = "linux")]
    {
        let info = fs::read_to_string("/proc/self/mountinfo")?;
        #[cfg(test)]
        let info = if profile.missing_nsdelegate {
            info.replace("nsdelegate", "fixture-no-delegation")
        } else {
            info
        };
        kinewright_job_sys::verify_nsdelegate(&info)?;
        #[cfg(test)]
        if profile.missing_pid_namespace {
            kinewright_job_sys::verify_payload_pid_namespace(0)?;
        }
    }
    let _spawn_guard = SPAWN
        .lock()
        .map_err(|_| io::Error::other("native spawn lock poisoned"))?;
    #[cfg(target_os = "linux")]
    let mut command = {
        let intent = claim.begin_transport(identity)?;
        let mut command = Command::new(guardian);
        command.arg("--transport").arg(intent);
        let kill_mode = "--property=KillMode=control-group";
        #[cfg(test)]
        let kill_mode = if profile.manager_process_only {
            "--property=KillMode=process"
        } else {
            kill_mode
        };
        command
            .args([
                "--user",
                "--quiet",
                "--collect",
                "--wait",
                "--pipe",
                "--service-type=exec",
            ])
            .arg(format!("--unit={}", identity.unit()))
            .args([
                "--property=Delegate=yes",
                kill_mode,
                "--property=SendSIGKILL=yes",
                "--property=TimeoutStopSec=1s",
                "--property=Restart=no",
                "--property=ExitType=main",
                "--property=OOMPolicy=stop",
                "--property=TasksMax=64",
            ]);
        command.arg(guardian);
        command
    };
    #[cfg(not(target_os = "linux"))]
    let mut command = {
        let _ = claim;
        Command::new(guardian)
    };
    command
        .arg("--serve")
        .arg(root)
        .arg(identity.unit())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            #[cfg(target_os = "linux")]
            claim.cancel_unspawned_transport(identity)?;
            return Err(error);
        }
    };
    let mut launcher = Launcher(Some(child));
    let child = launcher
        .0
        .as_mut()
        .ok_or_else(|| io::Error::other("launcher missing"))?;
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("lifetime writer"))?;
    kinewright_job_sys::check_lifetime_writer(&writer)?;
    let mut output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("guardian output"))?;
    let (sender, receiver) = mpsc::sync_channel(2);
    thread::spawn(move || {
        loop {
            let message = protocol::read_frame(&mut output);
            let terminal = message.is_err();
            if sender.send(message).is_err() || terminal {
                break;
            }
        }
    });
    Ok((launcher, writer, receiver))
}

/// The host reaps its own transport process even after a partial startup failure.
pub struct Launcher(Option<Child>);
impl Launcher {
    /// # Errors
    /// Returns a native process wait error.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.0
            .as_mut()
            .ok_or_else(|| io::Error::other("launcher already reaped"))?
            .try_wait()
    }
    /// # Errors
    /// Returns a native process wait error.
    pub fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.0
            .as_mut()
            .ok_or_else(|| io::Error::other("launcher already reaped"))?
            .wait()
    }
}
impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

impl ContainedSession {
    /// Launch only the native guardian and blocked payload. Provider code stays blocked.
    ///
    /// # Errors
    /// Retains the reservation on every uncertain/partial launch failure.
    pub fn prepare(
        claim: &mut MachineClaim,
        owner: &mut impl DurableAttemptOwner,
        identity: AttemptIdentity,
        program: Program,
        executable: &Path,
        profile: &CertifiedProfile,
    ) -> io::Result<Self> {
        identity.validate()?;
        program.validate()?;
        if !executable.is_absolute() {
            return Err(io::Error::other("absolute guardian path required"));
        }
        claim.reserve(&identity)?;
        #[cfg(target_os = "linux")]
        let cleanup_identity = identity.clone();
        let result = Self::prepare_reserved(claim, owner, identity, program, executable, profile);
        if let Err(error) = result {
            #[cfg(target_os = "linux")]
            if let Err(cleanup) = cleanup_failed_prepare(claim, &cleanup_identity) {
                return Err(io::Error::other(format!(
                    "{error}; cleanup_blocked: {cleanup}"
                )));
            }
            return Err(error);
        }
        result
    }
    fn prepare_reserved(
        claim: &mut MachineClaim,
        owner: &mut impl DurableAttemptOwner,
        identity: AttemptIdentity,
        program: Program,
        executable: &Path,
        profile: &CertifiedProfile,
    ) -> io::Result<Self> {
        owner.reserve(&identity)?;
        let root = claim.root().join(format!("job-{}", identity.nonce));
        fs::create_dir(&root)?;
        let (launcher, mut writer, replies) =
            spawn_transport(profile, executable, &root, &identity, claim)?;
        protocol::write_frame(
            &mut writer,
            &HostMessage::Prepare {
                identity: identity.clone(),
                program: Box::new(program),
            },
        )?;
        let GuardianMessage::Owned {
            recovery,
            at_us: owned_us,
        } = next_reply(&replies, Instant::now() + protocol::STARTUP)?
        else {
            return Err(io::Error::other("OWNED required"));
        };
        recovery.validate()?;
        if recovery.attempt != identity {
            return Err(io::Error::other("OWNED scope mismatch"));
        }
        let guardian = ProcessHandle::open(recovery.guardian_pid)?;
        if guardian.start() != recovery.guardian_start {
            return Err(io::Error::other("guardian identity changed"));
        }
        #[cfg(target_os = "linux")]
        let observer = {
            let path = Path::new(&recovery.containment);
            if path.file_name().is_none_or(|part| part != "payload")
                || path
                    .parent()
                    .and_then(Path::file_name)
                    .is_none_or(|part| part != identity.unit().as_str())
            {
                return Err(io::Error::other("foreign guardian cgroup"));
            }
            let observer = kinewright_job_sys::Cgroup::open(path)?;
            if observer.identity() != (recovery.device, recovery.inode) {
                return Err(io::Error::other("cgroup identity changed"));
            }
            observer
        };
        #[cfg(target_os = "linux")]
        let service = open_service(&recovery)?;
        claim.bind(&recovery)?;
        protocol::write_frame(
            &mut writer,
            &HostMessage::Contain {
                nonce: identity.nonce.clone(),
            },
        )?;
        let GuardianMessage::Contained {
            nonce,
            pid,
            start,
            at_us: contained_us,
        } = next_reply(&replies, Instant::now() + protocol::STARTUP)?
        else {
            return Err(io::Error::other("CONTAINED required"));
        };
        let payload = ProcessHandle::open(pid)?;
        if nonce != identity.nonce || payload.start() != start {
            return Err(io::Error::other("CONTAINED scope/identity"));
        }
        #[cfg(target_os = "linux")]
        if !observer
            .members()?
            .iter()
            .any(|member| member.pid() == pid && member.start() == start)
        {
            return Err(io::Error::other("payload membership missing"));
        }
        Ok(Self {
            identity,
            recovery,
            root,
            control: StartupControl(Arc::new(Mutex::new(Some(writer)))),
            replies,
            launcher,
            guardian,
            payload,
            #[cfg(target_os = "linux")]
            observer,
            #[cfg(target_os = "linux")]
            service,
            observed: Vec::new(),
            executing: false,
            cleaned: false,
            phase_us: [owned_us, contained_us, 0],
            cleanup_us: None,
        })
    }

    /// The serialized host authorizes the exact recovery identity immediately before COMMIT.
    ///
    /// # Errors
    /// Refuses duplicate execution, uncertain persistence, or a lost startup acknowledgment.
    pub fn commit(&mut self, owner: &mut impl DurableAttemptOwner) -> io::Result<()> {
        if self.executing {
            return Err(io::Error::other("attempt already executing"));
        }
        let outcome = self.commit_once(owner);
        if outcome.is_err() {
            self.close_lifetime();
        }
        outcome
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn commit_without_reply(
        &mut self,
        owner: &mut impl DurableAttemptOwner,
    ) -> io::Result<()> {
        owner.authorize_commit(&self.recovery)?;
        self.send_commit(owner)
    }

    fn send_commit(&self, owner: &mut impl DurableAttemptOwner) -> io::Result<()> {
        owner.commit_authorized(&self.recovery, &mut || {
            self.control.send(&HostMessage::Commit {
                nonce: self.identity.nonce.clone(),
            })
        })
    }

    fn commit_once(&mut self, owner: &mut impl DurableAttemptOwner) -> io::Result<()> {
        owner.authorize_commit(&self.recovery)?;
        self.send_commit(owner)?;
        let message = next_reply(&self.replies, Instant::now() + protocol::STARTUP)?;
        match message {
            GuardianMessage::Executing { nonce, at_us } if nonce == self.identity.nonce => {
                self.executing = true;
                self.phase_us[2] = at_us;
                Ok(())
            }
            _ => Err(io::Error::other("EXECUTING scope/handshake")),
        }
    }

    #[must_use]
    pub fn recovery(&self) -> &RecoveryIdentity {
        &self.recovery
    }
    #[must_use]
    pub fn phase_timestamps_us(&self) -> [u64; 3] {
        self.phase_us
    }
    #[must_use]
    pub fn cleanup_timestamps_us(&self) -> Option<[u64; 3]> {
        self.cleanup_us
    }
    #[must_use]
    pub fn payload_identity(&self) -> (u32, u64) {
        (self.payload.pid(), self.payload.start())
    }
    #[must_use]
    pub fn private_root(&self) -> &Path {
        &self.root
    }

    /// Pin a protocol fixture's independently reported descendant identity.
    /// The Linux check validates actual membership, including after setsid.
    ///
    /// # Errors
    /// Refuses changed, foreign, or excessive observed identities.
    pub fn observe_descendant(&mut self, pid: u32, start: u64) -> io::Result<()> {
        if self.observed.len() == 64 {
            return Err(io::Error::other("native observer capacity"));
        }
        let process = ProcessHandle::open(pid)?;
        if process.start() != start {
            return Err(io::Error::other("descendant identity changed"));
        }
        #[cfg(target_os = "linux")]
        if !self
            .observer
            .members()?
            .iter()
            .any(|member| member.pid() == pid && member.start() == start)
        {
            return Err(io::Error::other("foreign descendant"));
        }
        self.observed.push(process);
        Ok(())
    }

    /// Stop with grace; host death/drop closes the lifetime pipe without grace.
    ///
    /// # Errors
    /// Returns a closed-pipe failure without releasing ownership.
    pub fn request_cancel(&mut self) -> io::Result<()> {
        self.control.send(&HostMessage::Cancel {
            nonce: self.identity.nonce.clone(),
        })
    }

    #[must_use]
    pub fn startup_control(&self) -> StartupControl {
        self.control.clone()
    }

    /// Close the host-owned lifetime writer. Cleanup gets no cancellation grace.
    pub fn close_lifetime(&mut self) {
        self.control.close();
    }

    /// # Errors
    /// Missing native observation persists `cleanup_blocked` and retains the slot.
    pub fn cleanup(&mut self) -> io::Result<CleanupProof> {
        let deadline = Instant::now() + protocol::GRACE + protocol::CLEANUP;
        loop {
            if Instant::now() >= deadline {
                return self.blocked("native emptiness/reaping unavailable");
            }
            match self.replies.try_recv() {
                Ok(Ok(GuardianMessage::Cleaned {
                    nonce,
                    killed_at_us,
                    empty_at_us,
                    reaped_at_us,
                })) if nonce == self.identity.nonce => {
                    if empty_at_us < killed_at_us
                        || reaped_at_us < killed_at_us
                        || empty_at_us - killed_at_us > 2_000_000
                        || reaped_at_us - killed_at_us > 2_000_000
                    {
                        return self.blocked("native cleanup deadline");
                    }
                    self.cleaned = true;
                    self.cleanup_us = Some([killed_at_us, empty_at_us, reaped_at_us]);
                }
                Ok(Ok(GuardianMessage::CleanupBlocked { .. })) => {
                    return self.blocked("native cleanup_blocked");
                }
                _ => {}
            }
            // A failed sample retains ownership. Only a fresh complete sample
            // within the deadline can establish a later independent proof.
            let empty = self.independent_empty().unwrap_or(false);
            if empty
                && let Some(status) = self.launcher.try_wait()?
                && Instant::now() < deadline
            {
                return Ok(CleanupProof {
                    identity: self.identity.clone(),
                    recovery: self.recovery.clone(),
                    guardian_lost: !self.cleaned || !status.success(),
                });
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn independent_empty(&mut self) -> io::Result<bool> {
        let exited = self.guardian.reaped()?
            && self.payload.reaped()?
            && self.observed.iter().all(|p| p.reaped().unwrap_or(false));
        #[cfg(target_os = "linux")]
        {
            Ok(exited
                && self.observer.empty()?
                && self.service.empty()?
                && self.service.no_processes()?)
        }
        #[cfg(windows)]
        {
            Ok(exited && self.cleaned)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            Ok(false)
        }
    }
    #[cfg(all(test, target_os = "linux"))]
    #[allow(
        dead_code,
        reason = "the native integration target includes this module to isolate each service observation guard"
    )]
    pub(crate) fn fixture_alias_service(
        &mut self,
        observer: &mut RecoveryObserver,
    ) -> io::Result<()> {
        for service in [&mut self.service, &mut observer.service] {
            let alias = service
                .path()
                .parent()
                .ok_or_else(|| io::Error::other("service parent"))?
                .join(".")
                .join(
                    service
                        .path()
                        .file_name()
                        .ok_or_else(|| io::Error::other("service name"))?,
                );
            let aliased = kinewright_job_sys::Cgroup::open(&alias)?;
            if aliased.identity() != service.identity() {
                return Err(io::Error::other("service alias identity"));
            }
            *service = aliased;
        }
        Ok(())
    }

    fn blocked<T>(&self, reason: &str) -> io::Result<T> {
        let message = GuardianMessage::CleanupBlocked {
            nonce: self.identity.nonce.clone(),
            reason: reason.into(),
        };
        let path = self.root.join("host-cleanup-blocked.json");
        if !path.exists() {
            let bytes = serde_json::to_vec(&message)?;
            if bytes.len() > protocol::RECOVERY_CAP {
                return Err(io::Error::other("cleanup record cap"));
            }
            fs::write(path.with_extension("partial"), bytes)?;
            fs::rename(path.with_extension("partial"), path)?;
        }
        Err(io::Error::other(format!("cleanup_blocked: {reason}")))
    }
}
impl Drop for ContainedSession {
    fn drop(&mut self) {
        self.control.close();
    }
}

/// Bound the complete manager query after the original transport is reaped.
#[cfg(target_os = "linux")]
fn manager_start_idle(unit: &str, deadline: Instant) -> io::Result<bool> {
    use std::io::Read;
    let mut child = Command::new("systemctl")
        .args([
            "--user",
            "list-jobs",
            "--no-pager",
            "--no-legend",
            "--plain",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("manager output"))?;
    let mut query = Launcher(Some(child));
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(protocol::WIRE_CAP as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    loop {
        if Instant::now() >= deadline {
            if let Some(child) = query.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(io::Error::other("cleanup_blocked: manager query deadline"));
        }
        if let Some(status) = query.try_wait()? {
            if !status.success() {
                return Err(io::Error::other("cleanup_blocked: manager query failed"));
            }
            match receiver.try_recv() {
                Ok(bytes) => {
                    let bytes = bytes?;
                    if bytes.len() > protocol::WIRE_CAP {
                        return Err(io::Error::other("manager jobs cap"));
                    }
                    let jobs = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
                    for line in jobs.lines().filter(|line| !line.trim().is_empty()) {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        if fields.len() != 4 || fields[0].parse::<u32>().is_err() {
                            return Err(io::Error::other("ambiguous manager job"));
                        }
                        if fields[1] == unit {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(io::Error::other("manager output lost"));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Independent process-only observer retained across host or guardian loss.
/// It cannot release a claim on EOF or on an unlocked execution file.
#[cfg(target_os = "linux")]
pub struct RecoveryObserver {
    recovery: RecoveryIdentity,
    guardian: ProcessHandle,
    cgroup: kinewright_job_sys::Cgroup,
    service: kinewright_job_sys::Cgroup,
    members: Vec<ProcessHandle>,
}
#[cfg(target_os = "linux")]
impl RecoveryObserver {
    /// # Errors
    /// An existing service/payload name or any surviving member, including zombies, retains the claim.
    pub fn unbound_cleanup(
        claim: &mut MachineClaim,
        identity: &AttemptIdentity,
    ) -> io::Result<UnboundCleanupProof> {
        identity.validate()?;
        let deadline = Instant::now() + protocol::CLEANUP;
        loop {
            use crate::job_guardian::protocol::TransportState;
            let settled = match claim.transport_intent(identity)? {
                TransportState::Unstarted | TransportState::Closed => true,
                TransportState::Starting => false,
                TransportState::Started { pid, start } => {
                    kinewright_job_sys::process_identity_reaped(pid, start)?
                        && manager_start_idle(&identity.unit(), deadline)?
                }
            };
            if settled {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("cleanup_blocked: starting transport"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        claim.fence_transport(identity)?;
        loop {
            if kinewright_job_sys::unit_tree_absent(&identity.unit())? && Instant::now() < deadline
            {
                return Ok(UnboundCleanupProof {
                    identity: identity.clone(),
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("cleanup_blocked: unbound native unit"));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    /// Reopen after a host crash, including an already deleted native tree.
    /// The live path pins the original kernel identities. The late path requires
    /// both original cgroups deleted on their original cgroup-v2 mount AND the
    /// original guardian reaped. Neither a completion frame nor a free lock is proof.
    /// # Errors
    /// Keeps the machine claim on every ambiguous observation or missed deadline.
    pub fn recover_cleanup(recovery: &RecoveryIdentity) -> io::Result<CleanupProof> {
        recovery.validate()?;
        let deadline = Instant::now() + protocol::CLEANUP;
        loop {
            if let Ok(mut observer) = Self::open(recovery) {
                observer.stop_tree()?;
                return observer.wait_for_cleanup();
            }
            let leaf = Path::new(&recovery.containment);
            let service = leaf
                .parent()
                .ok_or_else(|| io::Error::other("missing service"))?;
            if leaf.file_name().is_none_or(|p| p != "payload")
                || service
                    .file_name()
                    .is_none_or(|p| p != recovery.attempt.unit().as_str())
            {
                return Err(io::Error::other("foreign service cgroup"));
            }
            if kinewright_job_sys::process_identity_reaped(
                recovery.guardian_pid,
                recovery.guardian_start,
            )? && kinewright_job_sys::cgroup_removed(leaf, recovery.device)?
                && kinewright_job_sys::cgroup_removed(service, recovery.service_device)?
            {
                return Ok(CleanupProof {
                    identity: recovery.attempt.clone(),
                    recovery: recovery.clone(),
                    guardian_lost: true,
                });
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("cleanup_blocked: late recovery"));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    /// # Errors
    /// Refuses a recycled process, cgroup, or mismatching native identity.
    pub fn open(recovery: &RecoveryIdentity) -> io::Result<Self> {
        recovery.validate()?;
        let guardian = ProcessHandle::open(recovery.guardian_pid)?;
        if guardian.start() != recovery.guardian_start {
            return Err(io::Error::other("replaced guardian"));
        }
        let cgroup = kinewright_job_sys::Cgroup::open(Path::new(&recovery.containment))?;
        if cgroup.identity() != (recovery.device, recovery.inode) {
            return Err(io::Error::other("replaced cgroup"));
        }
        let members = cgroup.members()?;
        let service = open_service(recovery)?;
        Ok(Self {
            recovery: recovery.clone(),
            guardian,
            cgroup,
            service,
            members,
        })
    }
    /// # Errors
    /// Returns `cleanup_blocked` if any native observation fails or misses two seconds.
    pub fn wait_for_cleanup(&mut self) -> io::Result<CleanupProof> {
        let deadline = Instant::now() + protocol::CLEANUP;
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "cleanup_blocked: independent recovery observation",
                ));
            }
            let observed: io::Result<bool> = (|| {
                Ok(self.guardian.reaped()?
                    && self.cgroup.empty()?
                    && self.service.empty()?
                    && self.service.no_processes()?
                    && self.members.iter().all(|p| p.reaped().unwrap_or(false)))
            })();
            if matches!(observed, Ok(true)) && Instant::now() < deadline {
                return Ok(CleanupProof {
                    identity: self.recovery.attempt.clone(),
                    recovery: self.recovery.clone(),
                    guardian_lost: true,
                });
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    /// A retained native kill handle is useful if a fault witness intentionally disables manager cleanup.
    ///
    /// # Errors
    /// Returns a whole-cgroup kill failure.
    pub fn stop_tree(&mut self) -> io::Result<()> {
        self.service.kill()
    }
}

#[cfg(target_os = "linux")]
fn cleanup_failed_prepare(claim: &mut MachineClaim, identity: &AttemptIdentity) -> io::Result<()> {
    match claim.recovery(identity) {
        Ok(recovery) => claim.retire(&RecoveryObserver::recover_cleanup(&recovery)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let proof = RecoveryObserver::unbound_cleanup(claim, identity)?;
            claim.retire_unbound(&proof)
        }
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn open_service(recovery: &RecoveryIdentity) -> io::Result<kinewright_job_sys::Cgroup> {
    let leaf = Path::new(&recovery.containment);
    let path = leaf
        .parent()
        .ok_or_else(|| io::Error::other("missing service cgroup"))?;
    if leaf.file_name().is_none_or(|part| part != "payload")
        || path
            .file_name()
            .is_none_or(|part| part != recovery.attempt.unit().as_str())
    {
        return Err(io::Error::other("foreign service cgroup"));
    }
    let service = kinewright_job_sys::Cgroup::open(path)?;
    if service.identity() != (recovery.service_device, recovery.service_inode) {
        return Err(io::Error::other("replaced service cgroup"));
    }
    Ok(service)
}

#[cfg(test)]
mod tests {
    use super::{GuardianMessage, next_reply};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn queued_acknowledgment_cannot_extend_an_expired_startup() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(GuardianMessage::Executing {
                nonce: "late".into(),
                at_us: 1,
            }))
            .unwrap();
        assert!(
            next_reply(
                &receiver,
                Instant::now().checked_sub(Duration::from_secs(1)).unwrap()
            )
            .is_err()
        );
    }
}
