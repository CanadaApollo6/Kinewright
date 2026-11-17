//! Sole process spawner and admission owner. No media parsing or scope RPC here.
use super::{
    ACTIVE_PROBES, CANCEL_GRACE, Config, Failure, Outcome, PENDING_PROBES, Request, Status, TICK,
    controller::Controller, worker,
};
use kinewright_job_sys::{media_boundary::Mode, media_host};
use kinewright_media_ipc::MediaSandboxError as Error;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub(super) enum Command {
    Probe(Request, SyncSender<Outcome>, Instant),
    Resume(SyncSender<Result<(), Error>>),
    Close(SyncSender<Result<(), Error>>),
    #[cfg(feature = "slow-tests")]
    Panic,
}
struct Pending {
    request: Request,
    reply: SyncSender<Outcome>,
    deadline: Instant,
    degraded: bool,
}
struct Job {
    pending: Pending,
    running: worker::Running,
    cause: Option<Failure>,
    kill_at: Option<Instant>,
    payload: Option<kinewright_job_sys::ProcessHandle>,
    next_rss: Instant,
    memory_violation: bool,
}
impl Job {
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
    active: Vec<Job>,
    pending: VecDeque<Pending>,
    continued: VecDeque<Pending>,
    circuit: Circuit,
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
            active: Vec::with_capacity(ACTIVE_PROBES),
            pending: VecDeque::with_capacity(PENDING_PROBES),
            continued: VecDeque::with_capacity(ACTIVE_PROBES),
            circuit: Circuit::new(Instant::now()),
            closed: false,
            close_reply: None,
        }
    }
    fn supervisor_health(&mut self) {
        if self.controller.as_ref().is_some_and(|value| {
            value.finished() || media_host::exited(&value.handle.pin).unwrap_or(true)
        }) {
            self.controller = None;
            for job in &mut self.active {
                job.cause.get_or_insert(Failure::Sandbox(Error::Crashed));
                let _ = job.running.pin.kill();
            }
        }
    }
    fn poll_resources(&mut self, now: Instant) {
        for job in &mut self.active {
            if !job.pending.degraded
                || job.cause.is_some()
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
            if job.cause.is_none() {
                if let Err(error) = job.pending.request.source.check() {
                    job.cause = Some(Failure::Authority(error));
                    let _ = job.running.pin.kill();
                } else if job.pending.request.cancel.load(Ordering::Acquire) {
                    job.cause = Some(Failure::Sandbox(Error::Cancelled));
                    job.kill_at = Some(now + CANCEL_GRACE);
                    job.running.shared.cancel.store(true, Ordering::Release);
                } else if now >= job.pending.deadline {
                    job.cause = Some(Failure::Sandbox(Error::TimedOut));
                    job.kill_at = Some(now + CANCEL_GRACE);
                    job.running.shared.cancel.store(true, Ordering::Release);
                }
            }
            if job.kill_at.is_some_and(|deadline| now >= deadline) {
                let _ = job.running.pin.kill();
            }
            if job.running.thread.is_finished() {
                let job = self.active.swap_remove(index);
                let uncaused = job.cause.is_none();
                let done = job.running.thread.join();
                let mut request = job.pending;
                if job.memory_violation {
                    self.circuit.quarantine(request.request.source.identity());
                }
                let mut result = match done {
                    Ok(done) if job.cause.is_none() && done.fallback && !request.degraded => {
                        request.degraded = true;
                        self.continued.push_back(request);
                        continue;
                    }
                    Ok(done) => job.cause.map_or(done.result, Err),
                    Err(_) => Err(job.cause.unwrap_or(Failure::Sandbox(Error::Crashed))),
                };
                // This host-owned worker cannot finish until its cleanup guard
                // has reaped the shim and verified native scope emptiness. Keep
                // cleanup waits off the actor so other deadlines still fire.
                if let Err(error) = request.request.source.check() {
                    result = Err(Failure::Authority(error));
                }
                if let Err(Failure::Sandbox(error)) = &result {
                    let identity = request.request.source.identity();
                    if uncaused
                        && self.circuit.failed(identity, *error, now)
                        && now < request.deadline
                    {
                        self.continued.push_back(request);
                        continue;
                    }
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
            && (!self.pending.is_empty() || !self.continued.is_empty())
            && self.available
            && !self.closed
            && let Some(next) = self.supervisor_generation.checked_add(1)
        {
            self.supervisor_generation = next;
            self.controller = Controller::start(&self.config.supervisor, next).ok();
        }
    }
    fn next_pending(&mut self) -> Option<(Pending, bool)> {
        // One source identity has only one active cursor, even when
        // multiple clips or callers request it concurrently.
        let eligible = |item: &Pending| {
            self.active
                .iter()
                .all(|job| job.pending.request.source.identity() != item.request.source.identity())
        };
        self.continued
            .iter()
            .position(&eligible)
            .and_then(|index| self.continued.remove(index))
            .map(|item| (item, false))
            .or_else(|| {
                self.pending
                    .iter()
                    .position(eligible)
                    .and_then(|index| self.pending.remove(index))
                    .map(|item| (item, true))
            })
    }
    fn admit(&mut self) {
        while self.active.len() < ACTIVE_PROBES {
            // A supervisor restart may have consumed its bounded handshake time.
            let now = Instant::now();
            let next = self.next_pending();
            let Some((mut item, fresh)) = next else {
                break;
            };
            if fresh {
                self.status.pending.fetch_sub(1, Ordering::AcqRel);
            }
            let refuse = if self.closed {
                Some(Failure::Sandbox(Error::Cancelled))
            } else if now >= item.deadline {
                Some(Failure::Sandbox(Error::TimedOut))
            } else if item.request.cancel.load(Ordering::Acquire) {
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
            if !self.circuit.admit(
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
            };
            match worker::start(launch) {
                Ok(running) => self.active.push(Job {
                    pending: item,
                    running,
                    cause: None,
                    kill_at: None,
                    payload: None,
                    next_rss: now,
                    memory_violation: false,
                }),
                Err(_) => {
                    let _ = item
                        .reply
                        .send(Err(Failure::Sandbox(Error::SandboxUnavailable)));
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
        self.status
            .active
            .store(self.active.len() + self.continued.len(), Ordering::Release);
        self.status
            .paused
            .store(self.circuit.paused.is_some(), Ordering::Release);
        if self.closed
            && self.active.is_empty()
            && self.pending.is_empty()
            && self.continued.is_empty()
        {
            if let Some(value) = self.controller.take() {
                value.shutdown();
            }
            if let Some(reply) = self.close_reply.take() {
                let _ = reply.send(Ok(()));
            }
        }
    }
    fn receive(&mut self) {
        match self.receiver.recv_timeout(TICK) {
            #[cfg(feature = "slow-tests")]
            Ok(Command::Panic) => panic!("injected media spawner exit"),
            Ok(Command::Probe(request, reply, deadline)) => {
                if self.pending.len() >= PENDING_PROBES || self.closed {
                    self.status.pending.fetch_sub(1, Ordering::AcqRel);
                    let _ = reply.send(Err(Failure::Sandbox(Error::ResourceLimit)));
                } else {
                    self.pending.push_back(Pending {
                        request,
                        reply,
                        deadline,
                        degraded: false,
                    });
                }
            }
            Ok(Command::Resume(reply)) => {
                let _ = reply.send(self.circuit.resume(Instant::now()));
            }
            Ok(Command::Close(reply)) => {
                if self.close_reply.is_some() {
                    let _ = reply.send(Err(Error::ResourceLimit));
                    return;
                }
                self.closed = true;
                self.close_reply = Some(reply);
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

struct History {
    identity: [u8; 32],
    crashes: u8,
    quarantine: bool,
    automatic: Option<Instant>,
    retry: VecDeque<Instant>,
    seen: Instant,
}
struct Circuit {
    launches: VecDeque<Instant>,
    history: Vec<History>,
    tokens: f64,
    refill: Instant,
    paused: Option<Instant>,
}
impl Circuit {
    fn new(now: Instant) -> Self {
        Self {
            launches: VecDeque::with_capacity(20),
            history: Vec::with_capacity(256),
            tokens: 4.0,
            refill: now,
            paused: None,
        }
    }
    fn admit(&mut self, identity: [u8; 32], explicit: bool, now: Instant) -> bool {
        if self.paused.is_some() {
            return false;
        }
        self.launches
            .retain(|stamp| now.saturating_duration_since(*stamp) < Duration::from_secs(60));
        self.tokens =
            (self.tokens + now.saturating_duration_since(self.refill).as_secs_f64() * 2.0).min(4.0);
        self.refill = now;
        if self.launches.len() >= 20 || self.tokens < 1.0 {
            self.paused = Some(now);
            return false;
        }
        self.history.retain(|entry| {
            entry.crashes != 0
                || entry.quarantine
                || now.saturating_duration_since(entry.seen) < Duration::from_secs(300)
        });
        let position = self
            .history
            .iter()
            .position(|entry| entry.identity == identity);
        let index = match position {
            Some(index) => index,
            None if self.history.len() < 256 => {
                self.history.push(History {
                    identity,
                    crashes: 0,
                    quarantine: false,
                    automatic: None,
                    retry: VecDeque::with_capacity(3),
                    seen: now,
                });
                self.history.len() - 1
            }
            None => {
                self.paused = Some(now);
                return false;
            }
        };
        let entry = &mut self.history[index];
        if explicit {
            entry
                .retry
                .retain(|stamp| now.saturating_duration_since(*stamp) < Duration::from_secs(60));
            if entry.retry.len() >= 3
                || entry.retry.back().is_some_and(|stamp| {
                    now.saturating_duration_since(*stamp) < Duration::from_secs(1)
                })
            {
                return false;
            }
            entry.retry.push_back(now);
            entry.quarantine = false;
            entry.crashes = 0;
        } else if entry.quarantine {
            return false;
        }
        entry.seen = now;
        self.tokens -= 1.0;
        self.launches.push_back(now);
        true
    }
    fn quarantine(&mut self, identity: [u8; 32]) {
        if let Some(entry) = self
            .history
            .iter_mut()
            .find(|entry| entry.identity == identity)
        {
            entry.quarantine = true;
        }
    }
    fn failed(&mut self, identity: [u8; 32], error: Error, now: Instant) -> bool {
        let Some(entry) = self
            .history
            .iter_mut()
            .find(|entry| entry.identity == identity)
        else {
            return false;
        };
        match error {
            Error::ProtocolViolation => entry.quarantine = true,
            Error::Crashed => {
                entry.crashes = entry.crashes.saturating_add(1);
                if entry.crashes >= 2 {
                    entry.quarantine = true;
                } else if entry.automatic.is_none_or(|stamp| {
                    now.saturating_duration_since(stamp) >= Duration::from_secs(300)
                }) {
                    entry.automatic = Some(now);
                    return true;
                }
            }
            _ => {}
        }
        false
    }
    fn resume(&mut self, now: Instant) -> Result<(), Error> {
        if self.paused.is_none() {
            return Ok(());
        }
        if self
            .paused
            .is_some_and(|stamp| now.saturating_duration_since(stamp) < Duration::from_secs(60))
        {
            return Err(Error::ResourceLimit);
        }
        self.paused = None;
        self.tokens = 4.0;
        self.refill = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_burst_stays_paused_until_explicit_cooldown_resume() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        for n in 0..4 {
            assert!(circuit.admit([n; 32], false, now));
        }
        assert!(!circuit.admit([9; 32], false, now));
        assert!(!circuit.admit([10; 32], false, now + Duration::from_secs(61)));
        assert_eq!(
            circuit.resume(now + Duration::from_secs(59)),
            Err(Error::ResourceLimit)
        );
        circuit.resume(now + Duration::from_secs(60)).unwrap();
        assert!(circuit.admit([11; 32], false, now + Duration::from_secs(60)));
    }
    #[test]
    fn resume_without_a_pause_does_not_refill_launch_tokens() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        for n in 0..4 {
            assert!(circuit.admit([n; 32], false, now));
        }
        circuit.resume(now).unwrap();
        assert!(!circuit.admit([9; 32], false, now));
    }
    #[test]
    fn twenty_launches_across_sources_exhaust_the_rolling_minute() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        for n in 0..20 {
            assert!(circuit.admit([n; 32], false, now + Duration::from_secs(u64::from(n) * 2)));
        }
        assert!(!circuit.admit([30; 32], false, now + Duration::from_secs(40)));
    }
    #[test]
    fn crash_and_protocol_quarantine_require_explicit_retry() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        let source = [1; 32];
        assert!(circuit.admit(source, false, now));
        assert!(circuit.failed(source, Error::Crashed, now));
        assert!(circuit.admit(source, false, now + Duration::from_secs(1)));
        assert!(!circuit.failed(source, Error::Crashed, now + Duration::from_secs(1)));
        assert!(!circuit.admit(source, false, now + Duration::from_secs(2)));
        assert!(circuit.admit(source, true, now + Duration::from_secs(2)));
        assert!(!circuit.failed(
            source,
            Error::ProtocolViolation,
            now + Duration::from_secs(2)
        ));
        assert!(!circuit.admit(source, false, now + Duration::from_secs(3)));
        assert!(circuit.admit(source, true, now + Duration::from_secs(3)));
        assert!(!circuit.admit(source, true, now + Duration::from_millis(3500)));
    }
}
