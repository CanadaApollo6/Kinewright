//! Sole process spawner and admission owner. No media parsing or scope RPC here.
use super::{
    ACTIVE_PROBES, CANCEL_GRACE, Config, Failure, Outcome, Phase, ProbeTiming, REAP_BUDGET,
    Request, Status, TICK, Unavailable, UnavailableReason as Reason,
    admission::{Health, Pending, Queue, attribute},
    controller::Controller,
    worker,
};
use kinewright_job_sys::{media_boundary::Mode, media_host};
use kinewright_media_ipc::MediaSandboxError as Error;
use std::{
    io,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, SyncSender},
    },
    time::Instant,
};

pub(super) enum Command {
    /// The instant is when the caller enqueued, never a decode deadline.
    Probe(Request, SyncSender<Outcome>, Instant),
    Close(SyncSender<Result<(), Error>>),
    #[cfg(feature = "slow-tests")]
    Panic,
}
struct Job {
    pending: Pending,
    /// Starts at launch: queue wait never spends another source's decode time.
    deadline: Instant,
    running: worker::Running,
    cause: Option<Failure>,
    kill_at: Option<Instant>,
    /// When a caused job's caller gets its typed failure even though native
    /// cleanup has not finished. The job, and so its reservation, stays active.
    answer_at: Option<Instant>,
    answered: bool,
    payload: Option<kinewright_job_sys::ProcessHandle>,
    shim_rss: ResidentSample,
    payload_rss: ResidentSample,
    next_rss: Instant,
    memory_violation: bool,
}
impl Job {
    fn launched(pending: Pending, running: worker::Running, now: Instant) -> Self {
        Self {
            deadline: pending.decode_deadline(now),
            pending,
            running,
            cause: None,
            kill_at: None,
            answer_at: None,
            answered: false,
            payload: None,
            shim_rss: ResidentSample::default(),
            payload_rss: ResidentSample::default(),
            next_rss: now,
            memory_violation: false,
        }
    }
    fn check_deadline_and_authority(&mut self, now: Instant) {
        if self.cause.is_none() && !self.answered {
            if let Err(error) = self.pending.request.source.check() {
                self.cause = Some(Failure::Authority(error));
                let _ = self.running.pin.kill();
            } else if self.pending.request.cancel.load(Ordering::Acquire) {
                self.cause = Some(Failure::Sandbox(Error::Cancelled));
                self.kill_at = Some(now + CANCEL_GRACE);
                self.running.shared.cancel.store(true, Ordering::Release);
            } else if now >= self.deadline {
                self.cause = Some(Failure::ProbeTimedOut(self.running.shared.timing()));
                self.kill_at = Some(now + CANCEL_GRACE);
                self.running.shared.cancel.store(true, Ordering::Release);
            }
        }
        if self.kill_at.is_some_and(|deadline| now >= deadline) {
            let _ = self.running.pin.kill();
        }
    }
    fn take_cause(&mut self) -> Option<Failure> {
        let cause = self.cause.take()?;
        if matches!(cause, Failure::ProbeTimedOut(_))
            && let Some(failure) = self.running.shared.failure_before(self.deadline)
        {
            return Some(failure);
        }
        Some(cause)
    }
    fn resident_total(&mut self, now: Instant) -> io::Result<Option<u64>> {
        let shim = self
            .shim_rss
            .observe(media_host::resident_bytes(&self.running.pin)?, now)?;
        if self.payload.is_none() {
            let pid = self.running.shared.payload.load(Ordering::Acquire);
            if pid != 0 {
                match kinewright_job_sys::ProcessHandle::open(pid) {
                    Ok(pin)
                        if pin.start()
                            == self.running.shared.payload_start.load(Ordering::Acquire) =>
                    {
                        self.payload = Some(pin);
                    }
                    // NotFound includes replacement during pinning. A different
                    // or missing identity can no longer contribute payload RSS.
                    Ok(_) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.raw_os_error()
                                == Some(rustix::io::Errno::SRCH.raw_os_error()) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let Some(pin) = &self.payload else {
            return Ok(shim);
        };
        let payload = self
            .payload_rss
            .observe(media_host::resident_bytes(pin)?, now)?;
        match (shim, payload) {
            (Some(shim), Some(payload)) => shim
                .checked_add(payload)
                .map(Some)
                .ok_or_else(|| io::Error::other("helper RSS sum overflow")),
            // Still enforce the limit on whichever member we have measured.
            (sample, None) | (None, sample) => Ok(sample),
        }
    }
}

#[derive(Default)]
struct ResidentSample {
    bytes: Option<u64>,
    missing_since: Option<Instant>,
}

impl ResidentSample {
    fn observe(&mut self, bytes: Option<u64>, now: Instant) -> io::Result<Option<u64>> {
        if let Some(bytes) = bytes {
            self.bytes = Some(bytes);
            self.missing_since = None;
        } else if now.duration_since(*self.missing_since.get_or_insert(now)) >= REAP_BUDGET {
            return Err(io::Error::other("helper RSS sample expired"));
        }
        Ok(self.bytes)
    }
}
struct AbortOnExit;
impl Drop for AbortOnExit {
    fn drop(&mut self) {
        std::process::abort();
    }
}

pub(super) fn run(config: Config, receiver: Receiver<Command>, status: Arc<Status>) -> ! {
    let _guard = AbortOnExit;
    let mut actor = Actor::new(config, receiver, status);
    loop {
        actor.supervisor_health();
        actor.poll_resources(Instant::now());
        actor.settle(Instant::now());
        actor.restart();
        actor.admit();
        actor.publish();
        actor.receive();
    }
}

struct Actor {
    config: Config,
    receiver: Receiver<Command>,
    status: Arc<Status>,
    available: bool,
    spawner: Option<std::os::fd::OwnedFd>,
    generation: u64,
    supervisor_generation: u64,
    supervisor_handshake: Option<(Instant, Instant)>,
    controller: Result<Controller, Unavailable>,
    scope_failure: Option<Error>,
    /// Decode slots held by leases whose scope emptiness was never proven.
    /// They are never released; the first such failure is reported at close.
    unproven: usize,
    cleanup_failure: Option<Error>,
    active: Vec<Job>,
    queue: Queue,
    closed: bool,
    close_reply: Option<SyncSender<Result<(), Error>>>,
}
impl Actor {
    fn new(config: Config, receiver: Receiver<Command>, status: Arc<Status>) -> Self {
        let initialized = kinewright_job_sys::install_subreaper().is_ok();
        let spawner = media_host::spawner_pidfd();
        let available = initialized && spawner.is_ok();
        let mut supervisor_handshake = None;
        let controller = if !initialized {
            Err(Phase::Runtime.unavailable(Reason::Subreaper))
        } else if spawner.is_err() {
            Err(Phase::Runtime.unavailable(Reason::SpawnerPidfd))
        } else {
            Controller::start(&config.supervisor, 1, &mut supervisor_handshake)
        };
        Self {
            config,
            receiver,
            status,
            available,
            spawner: spawner.ok().flatten(),
            generation: 1,
            supervisor_generation: 1,
            supervisor_handshake,
            controller,
            scope_failure: None,
            unproven: 0,
            cleanup_failure: None,
            active: Vec::with_capacity(ACTIVE_PROBES),
            queue: Queue::new(Instant::now()),
            closed: false,
            close_reply: None,
        }
    }
    fn supervisor_health(&mut self) {
        if self.controller.as_ref().is_ok_and(|value| {
            value.finished() || media_host::exited(&value.handle.pin).unwrap_or(true)
        }) && let Ok(controller) = std::mem::replace(
            &mut self.controller,
            Err(Phase::Supervisor.unavailable(Reason::SupervisorExited)),
        ) {
            if controller.scope_cleanup_pending() {
                self.scope_failure = Some(if controller.finished() {
                    controller.shutdown().err().unwrap_or(Error::Crashed)
                } else {
                    Error::Crashed
                });
            }
            for job in &mut self.active {
                if job.running.shared.native_clean.load(Ordering::Acquire) {
                    continue;
                }
                job.cause.get_or_insert(Failure::Sandbox(Error::Crashed));
                let _ = job.running.pin.kill();
            }
        }
    }
    fn poll_resources(&mut self, now: Instant) {
        for job in &mut self.active {
            if !job.pending.degraded
                || job.cause.is_some()
                || job.answered
                || job.running.thread.is_finished()
                || now < job.next_rss
            {
                continue;
            }
            job.next_rss = now + TICK;
            let total = match job.resident_total(now) {
                Ok(Some(total)) => total,
                Ok(None) => continue,
                Err(_) => {
                    job.cause = Some(
                        Phase::ResourceCheck
                            .unavailable(Reason::ResidentMemory)
                            .into(),
                    );
                    let _ = job.running.pin.kill();
                    continue;
                }
            };
            #[cfg(feature = "slow-tests")]
            self.status.observed_rss.fetch_max(total, Ordering::AcqRel);
            if total > self.config.limits.private_memory {
                job.memory_violation = true;
                job.cause = Some(Failure::Sandbox(Error::ResourceLimit));
                eprintln!(
                    "MS2_RSS_LIMIT observed={total} admitted={}",
                    self.config.limits.private_memory
                );
                let _ = job.running.pin.kill();
            }
        }
    }
    fn settle(&mut self, now: Instant) {
        for index in (0..self.active.len()).rev() {
            let job = &mut self.active[index];
            job.check_deadline_and_authority(now);
            if job.cause.is_some() && job.answer_at.is_none() {
                // Every cause sends the kill by `kill_at`, or already did when
                // the cause was set. Terminate, reap and empty within a further
                // REAP_BUDGET; the caller never waits on native cleanup past it.
                job.answer_at = Some(job.kill_at.unwrap_or(now) + REAP_BUDGET);
            }
            if !job.answered
                && !job.running.thread.is_finished()
                && job.answer_at.is_some_and(|deadline| now >= deadline)
            {
                // The cause moves to the caller; `answered` keeps the job from
                // being caused, charged or answered again when cleanup ends.
                job.answered = true;
                let Some(cause) = job.take_cause() else {
                    continue;
                };
                eprintln!(
                    "MS2_CLEANUP_PENDING shim={} cause={cause:?} reservation=retained",
                    job.running.pin.pid()
                );
                let result = match job.pending.request.source.check() {
                    Err(error) => Err(Failure::Authority(error)),
                    Ok(()) => Err(cause),
                };
                let identity = job.pending.request.source.identity();
                let paused = self
                    .queue
                    .circuit
                    .record(identity, Health::of(&result), now)
                    .paused;
                Self::answer(&job.pending, result, None);
                if paused {
                    self.queue.clear_for_pause(&self.status, now);
                }
                continue;
            }
            if job.running.thread.is_finished() {
                let mut job = self.active.swap_remove(index);
                let cause = job.take_cause();
                let uncaused = cause.is_none();
                let done = job.running.thread.join();
                let unproven = done.as_ref().is_ok_and(|done| done.unproven);
                if unproven {
                    self.unproven += 1;
                    self.cleanup_failure.get_or_insert(Error::Crashed);
                }
                let mut request = job.pending;
                let identity = request.request.source.identity();
                if job.memory_violation {
                    self.queue.circuit.quarantine(identity, now);
                }
                if job.answered {
                    // The caller already has its failure and the breaker its token.
                    continue;
                }
                let timing = done.as_ref().ok().map(|done| done.timing);
                let mut result = match done {
                    // A fallback run whose scope emptiness is unproven keeps
                    // its slot and gets its failure, like the retry below.
                    Ok(done) if uncaused && done.fallback && !unproven && !request.degraded => {
                        request.earlier_timing.add(done.timing);
                        request.degraded = true;
                        self.queue.continue_with(request, &self.status, now);
                        continue;
                    }
                    Ok(done) => attribute(cause, done.result),
                    Err(_) => Err(cause.unwrap_or(Failure::Sandbox(Error::Crashed))),
                };
                // This host-owned worker cannot finish until its cleanup guard
                // has reaped the shim and verified native scope emptiness. Keep
                // cleanup waits off the actor so other deadlines still fire.
                if let Err(error) = request.request.source.check() {
                    result = Err(Failure::Authority(error));
                }
                let recorded = self
                    .queue
                    .circuit
                    .record(identity, Health::of(&result), now);
                if recorded.paused {
                    self.queue.clear_for_pause(&self.status, now);
                }
                // An unproven cleanup is a host fault, not a helper one; a
                // retry would only spend the remaining slot on it.
                if uncaused && !unproven && recorded.retry && !recorded.paused {
                    if let Some(timing) = timing {
                        request.earlier_timing.add(timing);
                    }
                    self.queue.continue_with(request, &self.status, now);
                    continue;
                }
                Self::answer(&request, result, timing);
            }
        }
    }
    fn answer(request: &Pending, mut result: Outcome, completed: Option<ProbeTiming>) {
        match &mut result {
            Err(Failure::ProbeTimedOut(timing)) => {
                timing.add(request.earlier_timing);
                timing.report(false);
            }
            Ok(_) => {
                if let Some(mut timing) = completed {
                    timing.add(request.earlier_timing);
                    timing.report(true);
                }
            }
            Err(_) => {}
        }
        let _ = request.reply.send(result);
    }
    fn restart(&mut self) {
        if let Some(finished) = self.expired_handshake()
            && self
                .queue
                .pending
                .iter()
                .chain(&self.queue.continued)
                .any(|item| item.queued < finished)
        {
            // Let admission report the expired handshake to its waiters.
            // Only a later request can ask for a fresh handshake.
            return;
        }
        // Only restart after every old lease has independently completed native
        // cleanup. An absent manager still has a live lifecycle supervisor.
        if self.controller.is_err()
            && self.active.is_empty()
            && (self.scope_failure.is_some() || !self.queue.is_empty())
            && self.available
            && !self.closed
            && let Some(next) = self.supervisor_generation.checked_add(1)
        {
            self.supervisor_generation = next;
            self.controller = Controller::start(
                &self.config.supervisor,
                next,
                &mut self.supervisor_handshake,
            );
            if let Some(cause) = self.scope_failure.take() {
                let ready = self.controller.is_ok();
                eprintln!(
                    "MS2_SUPERVISOR_RESTART generation={next} scope_cleanup={cause:?} ready={ready}"
                );
            }
        }
    }
    fn expired_handshake(&self) -> Option<Instant> {
        let (_, finished) = self.supervisor_handshake?;
        self.controller
            .as_ref()
            .is_err_and(|error| error.reason == Reason::SupervisorHandshakeTimedOut)
            .then_some(finished)
    }
    fn admit(&mut self) {
        self.queue.expire(&self.status, Instant::now());
        // Decode slots are released after native cleanup. New scope creates
        // wait for the bounded stop/restart path to make a scope record available.
        if self
            .controller
            .as_ref()
            .is_ok_and(Controller::scope_cleanup_pending)
        {
            return;
        }
        while self.active.len() + self.unproven < ACTIVE_PROBES {
            // A supervisor restart may have consumed its bounded handshake time.
            let now = Instant::now();
            // One source identity has only one active cursor, even when
            // multiple clips or callers request it concurrently.
            let active = &self.active;
            let expired_handshake = self.expired_handshake();
            let Some(mut item) = self.queue.next(
                |item| {
                    expired_handshake.is_none_or(|finished| item.queued < finished)
                        && !active.iter().any(|job| {
                            job.pending.request.source.identity() == item.request.source.identity()
                        })
                },
                &self.status,
            ) else {
                break;
            };
            if let Some(error) = item.admission_refusal(self.closed) {
                let _ = item.reply.send(Err(error));
                continue;
            }
            item.record_handshake(self.supervisor_handshake);
            let control = match &self.controller {
                Ok(control) => control,
                Err(error) => {
                    Self::answer(&item, Err(supervisor_failure(*error)), None);
                    continue;
                }
            };
            if !self.queue.circuit.permits(
                item.request.source.identity(),
                item.request.explicit_retry,
                now,
            ) {
                let _ = item.reply.send(Err(Failure::Sandbox(Error::ResourceLimit)));
                continue;
            }
            let Some(next) = self.generation.checked_add(1) else {
                let _ = item.reply.send(Err(Failure::Sandbox(Error::ResourceLimit)));
                continue;
            };
            self.generation = next;
            item.request.explicit_retry = false;
            item.degraded |= !control.handle.available;
            let mode = if item.degraded {
                Mode::Degraded
            } else {
                Mode::Scope
            };
            self.status.degraded.store(item.degraded, Ordering::Release);
            if item.degraded {
                eprintln!(
                    "MS2_RESOURCE_MODE degraded rss_poll_ms=100 admitted={} address_space={}",
                    self.config.limits.private_memory, self.config.limits.address_space
                );
            }
            let launch = worker::Launch {
                helper: &self.config.helper,
                limits: self.config.limits,
                mode,
                controller: control.handle.clone(),
                generation: self.generation,
                selected: [item.request.video, item.request.audio],
                source: item.request.source.file(),
                spawner: self.spawner.as_ref().map(std::os::fd::AsFd::as_fd),
                // Measurement observers really are shared across independent
                // I/O workers; no observer or extra sharing ships by default.
                #[cfg(feature = "slow-tests")]
                observer: self.config.observer.as_ref().map(Arc::clone),
                #[cfg(feature = "slow-tests")]
                held_cleanup: self.config.held_cleanup.as_ref().map(Arc::clone),
                #[cfg(feature = "slow-tests")]
                scope_seam: self.config.scope_seam,
            };
            match worker::start(launch) {
                Ok(running) => {
                    #[cfg(feature = "slow-tests")]
                    self.status.launches.fetch_add(1, Ordering::AcqRel);
                    self.active.push(Job::launched(item, running, now));
                }
                Err(error) => {
                    let result = Err(error.into());
                    let identity = item.request.source.identity();
                    let recorded = self
                        .queue
                        .circuit
                        .record(identity, Health::of(&result), now);
                    if recorded.paused {
                        self.queue.clear_for_pause(&self.status, now);
                    }
                    let _ = item.reply.send(result);
                }
            }
        }
    }
    fn publish(&mut self) {
        #[cfg(feature = "slow-tests")]
        {
            self.status.supervisor.store(
                self.controller
                    .as_ref()
                    .map_or(0, |value| value.handle.pin.pid()),
                Ordering::Release,
            );
            let first = self.active.first();
            self.status.shim.store(
                first.map_or(0, |job| job.running.pin.pid()),
                Ordering::Release,
            );
            self.status.payload.store(
                first.map_or(0, |job| job.running.shared.payload.load(Ordering::Acquire)),
                Ordering::Release,
            );
            self.status.ready.store(
                first.is_some_and(|job| job.running.shared.ready.load(Ordering::Acquire)),
                Ordering::Release,
            );
            self.status.contained.store(
                first.is_some_and(|job| job.running.shared.contained.load(Ordering::Acquire)),
                Ordering::Release,
            );
            self.status.incomplete_reply.store(
                first
                    .is_some_and(|job| job.running.shared.incomplete_reply.load(Ordering::Acquire)),
                Ordering::Release,
            );
        }
        self.status.active.store(
            self.active.len() + self.unproven + self.queue.continued.len(),
            Ordering::Release,
        );
        self.status
            .paused
            .store(self.queue.circuit.paused(), Ordering::Release);
        if self.closed
            && self.active.is_empty()
            && self.queue.is_empty()
            && !self
                .controller
                .as_ref()
                .is_ok_and(Controller::scope_cleanup_pending)
        {
            let supervisor = std::mem::replace(
                &mut self.controller,
                Err(Phase::Supervisor.unavailable(Reason::SupervisorExited)),
            )
            .map_or(Ok(()), Controller::shutdown);
            if let Some(reply) = self.close_reply.take() {
                let failure = self.scope_failure.or(self.cleanup_failure);
                let _ = reply.send(failure.map_or(supervisor, Err));
            }
        }
    }
    fn receive(&mut self) {
        match self.receiver.recv_timeout(TICK) {
            #[cfg(feature = "slow-tests")]
            Ok(Command::Panic) => panic!("injected media spawner exit"),
            Ok(Command::Probe(request, reply, queued)) => {
                if self.closed {
                    self.status.pending.fetch_sub(1, Ordering::AcqRel);
                    let _ = reply.send(Err(Failure::Sandbox(Error::Cancelled)));
                } else {
                    self.queue.offer(
                        Pending {
                            request,
                            reply,
                            queued,
                            degraded: false,
                            earlier_timing: ProbeTiming::default(),
                        },
                        &self.status,
                        Instant::now(),
                    );
                }
            }
            Ok(Command::Close(reply)) => {
                if self.close_reply.is_some() {
                    let _ = reply.send(Err(Error::ResourceLimit));
                    return;
                }
                self.closed = true;
                self.close_reply = Some(reply);
                self.queue.close(&self.status);
                for job in &mut self.active {
                    job.cause.get_or_insert(Failure::Sandbox(Error::Cancelled));
                    let _ = job.running.pin.kill();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.closed = true;
                for job in &mut self.active {
                    job.cause.get_or_insert(Failure::Sandbox(Error::Cancelled));
                    let _ = job.running.pin.kill();
                }
                // The spawner remains alive until process exit even after its
                // last public sender disappears; returning would abort the app.
                if self.active.is_empty() {
                    std::thread::sleep(TICK);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn supervisor_failure(error: Unavailable) -> Failure {
    if error.reason == Reason::SupervisorHandshakeTimedOut {
        Failure::ProbeTimedOut(ProbeTiming {
            phase: Phase::Supervisor,
            ..ProbeTiming::default()
        })
    } else {
        error.into()
    }
}

#[cfg(test)]
mod ms_t10_tests {
    use super::*;

    struct Input(std::fs::File);
    impl super::super::Source for Input {
        fn file(&self) -> &std::fs::File {
            &self.0
        }
        fn identity(&self) -> [u8; 32] {
            [0; 32]
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            Ok(())
        }
    }

    #[test]
    fn ms_t10_handshake_timeout_only_answers_its_waiters() {
        use std::{path::PathBuf, sync::mpsc, time::Duration};
        let (_sender, receiver) = mpsc::sync_channel(2);
        let mut actor = Actor::new(
            Config::new(
                PathBuf::from("/dev/null"),
                PathBuf::from("/dev/null"),
                super::super::Limits::PROBE,
            ),
            receiver,
            Arc::default(),
        );
        let finished = Instant::now();
        let started = finished.checked_sub(Duration::from_millis(50)).unwrap();
        actor.supervisor_handshake = Some((started, finished));
        actor.controller = Err(Phase::Supervisor.unavailable(Reason::SupervisorHandshakeTimedOut));
        let mut enqueue = |queued| {
            let (reply, result) = mpsc::sync_channel(1);
            actor.status.pending.fetch_add(1, Ordering::Relaxed);
            actor.queue.offer(
                Pending {
                    request: Request {
                        source: Box::new(Input(std::fs::File::open("/dev/null").unwrap())),
                        video: None,
                        audio: None,
                        cancel: Arc::default(),
                        explicit_retry: false,
                    },
                    reply,
                    queued,
                    degraded: false,
                    earlier_timing: ProbeTiming::default(),
                },
                &actor.status,
                finished,
            );
            result
        };
        let waiter = enqueue(started);
        let later = enqueue(finished);
        actor.restart();
        actor.admit();
        assert!(matches!(
            waiter.try_recv().unwrap(),
            Err(Failure::ProbeTimedOut(timing)) if timing.supervisor_handshake == finished - started
        ));
        let waiting = later.try_recv();
        assert!(
            matches!(waiting, Err(mpsc::TryRecvError::Empty)),
            "{waiting:?}"
        );
        assert_eq!(actor.status.pending.load(Ordering::Relaxed), 1);
        actor.restart();
        actor.admit();
        assert_eq!(actor.supervisor_generation, 2);
        assert!(
            matches!(later.try_recv().unwrap(), Err(Failure::Unavailable(error))
            if error.reason == Reason::ExecutableUnavailable)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_gap_retains_the_last_sample_and_expires_without_recovery() {
        let mut sample = ResidentSample::default();
        let now = Instant::now();
        assert_eq!(sample.observe(None, now).unwrap(), None);
        assert_eq!(sample.observe(Some(4096), now + TICK).unwrap(), Some(4096));

        let gap = now + TICK * 2;
        let deadline = gap + REAP_BUDGET;
        let last_tick = deadline.checked_sub(TICK).unwrap();
        assert_eq!(sample.observe(None, gap).unwrap(), Some(4096));
        assert_eq!(sample.observe(None, last_tick).unwrap(), Some(4096));
        assert!(sample.observe(None, deadline).is_err());
        assert_eq!(sample.observe(Some(0), deadline).unwrap(), Some(0));
        assert_eq!(sample.observe(None, deadline + TICK).unwrap(), Some(0));
    }
}
