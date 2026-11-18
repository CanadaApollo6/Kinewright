use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kinewright_core::{ProbeEvent, ProbeObserver};

use super::{MediaDeadlines, PREPARE_WORKERS, PrepareControl, Refusal, RefusalCode, cancelled};

// Replacements are allowed until eight calls from one preparation have expired.
const MAX_ABANDONED: usize = 8;
// An OS call can outlive its preparation. Keep the process-wide exposure bounded too.
const MAX_LIVE_WORKERS: usize = 16;
static LIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

const CANCEL_POLL: Duration = Duration::from_millis(25);

pub(super) struct Workers<'a> {
    control: &'a PrepareControl,
    deadlines: MediaDeadlines,
    abandoned: usize,
}

impl<'a> Workers<'a> {
    pub(super) const fn new(control: &'a PrepareControl, deadlines: MediaDeadlines) -> Self {
        Self {
            control,
            deadlines,
            abandoned: 0,
        }
    }

    pub(super) fn io<I: Send + 'static, O: Send + 'static>(
        &mut self,
        input: I,
        timeout: &str,
        work: impl Fn(I, &Ticket) -> Result<O, Refusal> + Send + Sync + 'static,
    ) -> Result<O, Refusal> {
        self.run(Phase::Stat, vec![input], work)?
            .pop()
            .flatten()
            .ok_or_else(|| Refusal::new(RefusalCode::IoTimeout, timeout))?
    }

    /// Results preserve input order. A vacant result denotes a missed deadline.
    pub(super) fn run<Input, Output>(
        &mut self,
        phase: Phase,
        jobs: Vec<Input>,
        work: impl Fn(Input, &Ticket) -> Output + Send + Sync + 'static,
    ) -> Result<Vec<Option<Output>>, Refusal>
    where
        Input: Send + 'static,
        Output: Send + 'static,
    {
        let mut outputs: Vec<_> = (0..jobs.len()).map(|_| None).collect();
        let mut pending: VecDeque<_> = jobs.into_iter().enumerate().collect();
        let (done, completed) = unbounded();
        let work = Arc::new(work);
        let mut pool = Vec::new();

        loop {
            if self.control.is_cancelled() {
                return Err(cancelled());
            }
            self.reap(&completed, &mut pool, &mut outputs);
            self.dispatch(phase, &done, &work, &mut pool, &mut pending);
            if pool.iter().all(|worker| worker.active.is_none()) {
                return Ok(outputs);
            }
            // Initial dispatch may have created jobs with an earlier deadline.
            let wait = pool
                .iter()
                .filter_map(|worker| worker.active.as_ref())
                .map(|(_, ticket)| ticket.remaining())
                .fold(CANCEL_POLL, Duration::min);
            if let Ok(result) = completed.recv_timeout(wait) {
                self.accept(result, &mut pool, &mut outputs);
            }
        }
    }

    fn dispatch<Input: Send + 'static, Output: Send + 'static>(
        &self,
        phase: Phase,
        done: &Sender<Completed<Output>>,
        work: &Arc<impl Fn(Input, &Ticket) -> Output + Send + Sync + 'static>,
        pool: &mut Vec<Worker<Input>>,
        pending: &mut VecDeque<(usize, Input)>,
    ) {
        let running = pool.iter().filter(|worker| worker.active.is_some()).count();
        let dispatch = pending
            .len()
            .min((MAX_ABANDONED - self.abandoned).saturating_sub(running))
            .min(PREPARE_WORKERS - running);
        while pool.len() < running + dispatch {
            let Some(worker) = Worker::start(done.clone(), Arc::clone(work)) else {
                break;
            };
            pool.push(worker);
        }
        for worker in pool
            .iter_mut()
            .filter(|worker| worker.active.is_none())
            .take(dispatch)
        {
            let Some((index, input)) = pending.pop_front() else {
                break;
            };
            let ticket = Arc::new(Ticket::new(phase, self.deadlines, self.control.clone()));
            if worker
                .input
                .as_ref()
                .is_some_and(|queue| queue.send((index, input, Arc::clone(&ticket))).is_ok())
            {
                worker.active = Some((index, ticket));
            }
        }
    }

    fn reap<Input, Output>(
        &mut self,
        completed: &Receiver<Completed<Output>>,
        pool: &mut Vec<Worker<Input>>,
        outputs: &mut [Option<Output>],
    ) {
        for result in completed.try_iter() {
            self.accept(result, pool, outputs);
        }
        pool.retain(|worker| {
            if let Some((_, ticket)) = &worker.active
                && ticket.remaining().is_zero()
            {
                ticket.stop.store(true, Ordering::Relaxed);
                self.abandoned += 1;
                return false;
            }
            true
        });
    }

    fn accept<Input, Output>(
        &mut self,
        result: Completed<Output>,
        pool: &mut Vec<Worker<Input>>,
        outputs: &mut [Option<Output>],
    ) {
        if let Some(position) = pool.iter().position(|worker| {
            worker
                .active
                .as_ref()
                .is_some_and(|(index, _)| *index == result.index)
        }) {
            if result.in_time {
                outputs[result.index] = Some(result.output);
                pool[position].active = None;
            } else {
                self.abandoned += 1;
                pool.swap_remove(position);
            }
        }
    }
}

struct Worker<Input> {
    input: Option<Sender<Job<Input>>>,
    thread: Option<JoinHandle<()>>,
    active: Option<(usize, Arc<Ticket>)>,
}

type Job<Input> = (usize, Input, Arc<Ticket>);

impl<Input> Drop for Worker<Input> {
    fn drop(&mut self) {
        self.input.take();
        if let Some((_, ticket)) = &self.active {
            ticket.stop.store(true, Ordering::Relaxed);
        } else if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Completed<Output> {
    index: usize,
    in_time: bool,
    output: Output,
}

impl<Input: Send + 'static> Worker<Input> {
    fn start<Output: Send + 'static>(
        done: Sender<Completed<Output>>,
        work: Arc<impl Fn(Input, &Ticket) -> Output + Send + Sync + 'static>,
    ) -> Option<Self> {
        let permit = WorkerPermit::acquire()?;
        let (input, jobs) = bounded::<Job<Input>>(1);
        let thread = thread::Builder::new()
            .name("ix-locator".into())
            .spawn(move || {
                let _permit = permit;
                for (index, input, ticket) in &jobs {
                    let output = work(input, &ticket);
                    let in_time = !ticket.remaining().is_zero();
                    if done
                        .send(Completed {
                            index,
                            in_time,
                            output,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            input: Some(input),
            thread: Some(thread),
            active: None,
        })
    }
}

struct WorkerPermit;

impl WorkerPermit {
    fn acquire() -> Option<Self> {
        if LIVE_WORKERS.fetch_add(1, Ordering::Relaxed) < MAX_LIVE_WORKERS {
            return Some(Self);
        }
        LIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
        None
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        LIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Stat,
    Probe,
}

pub(super) struct Ticket {
    stop: Arc<AtomicBool>,
    control: PrepareControl,
    deadlines: MediaDeadlines,
    state: Mutex<State>,
}

struct State {
    changed: Instant,
    timeout: Duration,
    hashed: u64,
}

impl Ticket {
    fn new(phase: Phase, deadlines: MediaDeadlines, control: PrepareControl) -> Self {
        let timeout = match phase {
            Phase::Stat => deadlines.stat,
            Phase::Probe => deadlines.hash_stall,
        };
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            control,
            deadlines,
            state: Mutex::new(State {
                changed: Instant::now(),
                timeout,
                hashed: 0,
            }),
        }
    }

    fn remaining(&self) -> Duration {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.timeout.saturating_sub(state.changed.elapsed())
    }

    pub(super) fn note_io_progress(&self) -> bool {
        self.update(None)
    }

    fn update(&self, event: Option<ProbeEvent>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if self.stop.load(Ordering::Relaxed)
            || self.control.is_cancelled()
            || now.duration_since(state.changed) >= state.timeout
        {
            return false;
        }
        state.changed = now;
        match event {
            Some(ProbeEvent::HashedBytes(bytes)) => {
                self.control.add_hashed(bytes.saturating_sub(state.hashed));
                state.hashed = bytes;
            }
            Some(ProbeEvent::MetadataStarted) => state.timeout = self.deadlines.metadata,
            None => {}
        }
        true
    }
}

impl ProbeObserver for Ticket {
    fn observe(&self, event: ProbeEvent) -> bool {
        self.update(Some(event))
    }

    fn interrupt_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.stop))
    }

    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
            || self.control.is_cancelled()
            || self.remaining().is_zero()
    }
}
