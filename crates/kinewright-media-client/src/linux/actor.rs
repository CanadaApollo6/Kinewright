//! Sole process spawner and admission owner. No media parsing or scope RPC here.
use super::{
    ACTIVE_PROBES, CANCEL_GRACE, Config, Failure, Outcome, REAP_BUDGET, Request, Status, TICK,
    admission::{Health, Pending, Queue, attribute},
    controller::Controller,
    worker,
};
use kinewright_job_sys::{media_boundary::Mode, media_host};
use kinewright_media_ipc::MediaSandboxError as Error;
use std::{
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
            next_rss: now,
            memory_violation: false,
        }
    }
    fn resident_total(&mut self) -> std::io::Result<u64> {
        let shim = media_host::resident_bytes(&self.running.pin)?;
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
                    // A different/missing identity means that old payload has
                    // already gone. Never sample or signal its replacement.
                    Ok(_) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.raw_os_error()
                                == Some(rustix::io::Errno::SRCH.raw_os_error()) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let payload = self
            .payload
            .as_ref()
            .map_or(Ok(0), media_host::resident_bytes)?;
        shim.checked_add(payload)
            .ok_or_else(|| std::io::Error::other("helper RSS sum overflow"))
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
    controller: Option<Controller>,
    scope_failure: Option<Error>,
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
        let controller = if available {
            Controller::start(&config.supervisor, 1).ok()
        } else {
            None
        };
        Self {
            config,
            receiver,
            status,
            available,
            spawner: spawner.ok().flatten(),
            generation: 1,
            supervisor_generation: 1,
            controller,
            scope_failure: None,
            active: Vec::with_capacity(ACTIVE_PROBES),
            queue: Queue::new(Instant::now()),
            closed: false,
            close_reply: None,
        }
    }
    fn supervisor_health(&mut self) {
        if self.controller.as_ref().is_some_and(|value| {
            value.finished() || media_host::exited(&value.handle.pin).unwrap_or(true)
        }) && let Some(controller) = self.controller.take()
        {
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
            if let Ok(total) = job.resident_total() {
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
            } else {
                job.cause = Some(Failure::Sandbox(Error::SandboxUnavailable));
                let _ = job.running.pin.kill();
            }
        }
    }
    fn settle(&mut self, now: Instant) {
        for index in (0..self.active.len()).rev() {
            let job = &mut self.active[index];
            if job.cause.is_none() && !job.answered {
                if let Err(error) = job.pending.request.source.check() {
                    job.cause = Some(Failure::Authority(error));
                    let _ = job.running.pin.kill();
                } else if job.pending.request.cancel.load(Ordering::Acquire) {
                    job.cause = Some(Failure::Sandbox(Error::Cancelled));
                    job.kill_at = Some(now + CANCEL_GRACE);
                    job.running.shared.cancel.store(true, Ordering::Release);
                } else if now >= job.deadline {
                    job.cause = Some(Failure::Sandbox(Error::TimedOut));
                    job.kill_at = Some(now + CANCEL_GRACE);
                    job.running.shared.cancel.store(true, Ordering::Release);
                }
            }
            if job.kill_at.is_some_and(|deadline| now >= deadline) {
                let _ = job.running.pin.kill();
            }
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
                let Some(cause) = job.cause.take() else {
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
                let _ = job.pending.reply.send(result);
                if paused {
                    self.queue.clear_for_pause(&self.status, now);
                }
                continue;
            }
            if job.running.thread.is_finished() {
                let job = self.active.swap_remove(index);
                let uncaused = job.cause.is_none();
                let done = job.running.thread.join();
                let mut request = job.pending;
                let identity = request.request.source.identity();
                if job.memory_violation {
                    self.queue.circuit.quarantine(identity, now);
                }
                if job.answered {
                    // The caller already has its failure and the breaker its token.
                    continue;
                }
                let mut result = match done {
                    Ok(done) if job.cause.is_none() && done.fallback && !request.degraded => {
                        request.degraded = true;
                        self.queue.continue_with(request, &self.status, now);
                        continue;
                    }
                    Ok(done) => attribute(job.cause, done.result),
                    Err(_) => Err(job.cause.unwrap_or(Failure::Sandbox(Error::Crashed))),
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
                if uncaused && recorded.retry && !recorded.paused {
                    self.queue.continue_with(request, &self.status, now);
                    continue;
                }
                let _ = request.reply.send(result);
            }
        }
    }
    fn restart(&mut self) {
        // Only restart after every old lease has independently completed native
        // cleanup. An absent manager still has a live lifecycle supervisor.
        if self.controller.is_none()
            && self.active.is_empty()
            && (self.scope_failure.is_some() || !self.queue.is_empty())
            && self.available
            && !self.closed
            && let Some(next) = self.supervisor_generation.checked_add(1)
        {
            self.supervisor_generation = next;
            self.controller = Controller::start(&self.config.supervisor, next).ok();
            if let Some(cause) = self.scope_failure.take() {
                let ready = self.controller.is_some();
                eprintln!(
                    "MS2_SUPERVISOR_RESTART generation={next} scope_cleanup={cause:?} ready={ready}"
                );
            }
        }
    }
    fn admit(&mut self) {
        self.queue.expire(&self.status, Instant::now());
        // Decode slots are released after native cleanup. New scope creates
        // wait for the bounded stop/restart path to make a scope record available.
        if self
            .controller
            .as_ref()
            .is_some_and(Controller::scope_cleanup_pending)
        {
            return;
        }
        while self.active.len() < ACTIVE_PROBES {
            // A supervisor restart may have consumed its bounded handshake time.
            let now = Instant::now();
            // One source identity has only one active cursor, even when
            // multiple clips or callers request it concurrently.
            let active = &self.active;
            let Some(mut item) = self.queue.next(
                |identity| {
                    active
                        .iter()
                        .any(|job| job.pending.request.source.identity() == *identity)
                },
                &self.status,
            ) else {
                break;
            };
            let refuse = if self.closed || item.request.cancel.load(Ordering::Acquire) {
                Some(Failure::Sandbox(Error::Cancelled))
            } else {
                item.request.source.check().err().map(Failure::Authority)
            };
            if let Some(error) = refuse {
                let _ = item.reply.send(Err(error));
                continue;
            }
            let Some(control) = self.controller.as_ref() else {
                let _ = item
                    .reply
                    .send(Err(Failure::Sandbox(Error::SandboxUnavailable)));
                continue;
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
            if let Ok(running) = worker::start(launch) {
                self.active.push(Job::launched(item, running, now));
            } else {
                let failure = Failure::Sandbox(Error::SandboxUnavailable);
                let identity = item.request.source.identity();
                if self
                    .queue
                    .circuit
                    .record(identity, Health::Unhealthy(Error::SandboxUnavailable), now)
                    .paused
                {
                    self.queue.clear_for_pause(&self.status, now);
                }
                let _ = item.reply.send(Err(failure));
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
            self.active.len() + self.queue.continued.len(),
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
                .is_some_and(Controller::scope_cleanup_pending)
        {
            let supervisor = self.controller.take().map_or(Ok(()), Controller::shutdown);
            if let Some(reply) = self.close_reply.take() {
                let _ = reply.send(self.scope_failure.map_or(supervisor, Err));
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
