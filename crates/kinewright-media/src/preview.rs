//! PF1 S2a: the preview thread (H-1), its transport jobs (R-3, S-1) and the
//! agent lane (R-4).
//!
//! The media worker keeps `Control` handling, the audio fill and the clock;
//! it posts jobs into the [`Lane`] and never renders. The preview thread owns
//! the one synchronous preview [`FrameRenderer`] and serves every preview
//! render: transport jobs through the slot, agent jobs through a bounded FIFO.
//!
//! Lock order (H-4): the lane lock is a leaf. Nothing that could lock (reply
//! senders, documents, libraries) is dropped under it, and nothing renders,
//! sends or wakes while holding it.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    ops::{Deref, DerefMut},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use kinewright_core::{
    Document, FrameStamp, FrameTexture, MediaError, PreviewFrame, Rational, RgbaImage, TimeCode,
};

use crate::{
    decode::VideoDecoder,
    derived_cache::CacheStats,
    engine::{SharedClock, monitor_max_width, send_latest},
    frame::{CachedFrame, WorkingFrame},
    lut_store::LutLibrary,
    render::{
        DecodeStrategy, FRAME_CACHE_BYTE_BUDGET, FrameRenderer, FrameSizes, PREFETCH_FRAMES,
        ReaderDemand, RenderScale, SourceSpec, SuppliedFrames, TitleCacheKey, VideoSourceKey,
        document_source_keys, reader_demand,
    },
    sched::{
        Admission, Next, PermitBook, Poll, Posted, Readers, WaitStep, WaitView, Weighed,
        plan_regions, wait_step,
    },
    stats::{ACK_QUEUE, Ack, Counters},
};

/// R-4: at most this many agent jobs wait; a full queue replies at once.
pub(crate) const AGENT_QUEUE_LIMIT: usize = 8;
/// A playback hold re-reads the clock this often while it waits.
const HOLD_POLL: Duration = Duration::from_millis(1);

pub(crate) fn worker_stopped() -> MediaError {
    MediaError::Backend("media worker stopped".to_owned())
}

pub(crate) type Wakeup = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// H-4: this thread holds `Sched` (a drop guard asserts it does not).
    static HOLDING_SCHED: Cell<bool> = const { Cell::new(false) };
}

/// The `Sched` (lane state) guard; it marks its thread as holding the lock
/// so K-1's drop guard can assert it never releases under it (H-4).
pub(crate) struct Sched<'a>(Option<MutexGuard<'a, LaneState>>);

impl<'a> Sched<'a> {
    fn new(guard: MutexGuard<'a, LaneState>) -> Self {
        HOLDING_SCHED.set(true);
        Self(Some(guard))
    }

    fn take(&mut self) -> MutexGuard<'a, LaneState> {
        HOLDING_SCHED.set(false);
        self.0.take().expect("Sched is held")
    }

    /// Wait on `condvar`, `Sched` released meanwhile.
    pub(crate) fn wait(mut self, condvar: &Condvar) -> Self {
        let waited = condvar.wait(self.take());
        Self::new(waited.unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn wait_timeout(mut self, condvar: &Condvar, timeout: Duration) -> Self {
        let waited = condvar.wait_timeout(self.take(), timeout);
        Self::new(waited.unwrap_or_else(PoisonError::into_inner).0)
    }
}

impl Deref for Sched<'_> {
    type Target = LaneState;

    fn deref(&self) -> &LaneState {
        self.0.as_ref().expect("Sched is held")
    }
}

impl DerefMut for Sched<'_> {
    fn deref_mut(&mut self) -> &mut LaneState {
        self.0.as_mut().expect("Sched is held")
    }
}

impl Drop for Sched<'_> {
    fn drop(&mut self) {
        if self.0.take().is_some() {
            HOLDING_SCHED.set(false);
        }
    }
}

/// K-1's drop guard: bytes of the scheduler's live count, released under
/// `Sched` when the last owner drops, never by a thread holding it (H-4).
pub(crate) struct Hold {
    bytes: usize,
    lane: Weak<Lane>,
}

impl Hold {
    /// Adopt `bytes` already reserved under `Sched`.
    fn adopt(lane: &Arc<Lane>, bytes: usize) -> Self {
        let lane = Arc::downgrade(lane);
        Self { bytes, lane }
    }

    /// Release all but `bytes` (the title rasters still cached).
    fn shrink_to(&mut self, bytes: usize) {
        let excess = self.bytes.saturating_sub(bytes);
        self.bytes -= excess;
        release(&self.lane, excess);
    }
}

fn release(lane: &Weak<Lane>, bytes: usize) {
    debug_assert!(
        !HOLDING_SCHED.get(),
        "H-4: a reservation released under Sched"
    );
    let Some(lane) = lane.upgrade().filter(|_| bytes > 0) else {
        return;
    };
    lane.lock().readers.release(bytes);
    // H-3: a reservation release wakes both.
    lane.notify();
    lane.work.notify_all();
}

impl Drop for Hold {
    fn drop(&mut self) {
        release(&self.lane, self.bytes);
    }
}

/// A reader's frame and its reservation: the bytes stay live while any
/// clone does (ring, handoff, a render's pin).
#[derive(Clone)]
pub(crate) struct Pinned {
    pub(crate) frame: WorkingFrame,
    hold: Arc<Hold>,
}

impl Weighed for Pinned {
    fn bytes(&self) -> usize {
        self.hold.bytes
    }

    fn pinned(&self) -> bool {
        Arc::strong_count(&self.hold) > 1
    }
}

impl CachedFrame for Pinned {
    fn byte_len(&self) -> usize {
        self.frame.byte_len()
    }

    fn shared_buffer_id(&self) -> usize {
        self.frame.shared_buffer_id()
    }
}

/// What a transport job renders: the document and LUT library current at
/// its stamp, bound by the worker when it posts (R-1).
#[derive(Clone)]
pub(crate) struct Scene {
    pub(crate) document: Arc<Document>,
    pub(crate) lut: Arc<LutLibrary>,
    /// Bumped by every `set_document`: the preview clears its caches.
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobKind {
    /// Exactly `at`, once (S-1).
    Paused(TimeCode),
    /// Clock-following frames from `from` (R-3); stays in the slot.
    Playback { from: TimeCode },
}

#[derive(Clone)]
pub(crate) struct TransportJob {
    pub(crate) kind: JobKind,
    pub(crate) stamp: FrameStamp,
    pub(crate) scene: Scene,
}

pub(crate) enum AgentWork {
    Thumbnail {
        document: Arc<Document>,
        lut: Arc<LutLibrary>,
        at: TimeCode,
        max_width: u32,
        reply: Sender<Result<RgbaImage, MediaError>>,
    },
    CacheStats {
        clear: bool,
        reply: Sender<Result<CacheStats, MediaError>>,
    },
    /// Test support: occupy the preview until `release` disconnects.
    #[cfg(any(test, feature = "test-util"))]
    Hold {
        started: Sender<()>,
        release: Receiver<()>,
    },
}

pub(crate) struct AgentJob {
    pub(crate) work: AgentWork,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl AgentJob {
    /// Send `error` as this job's one reply.
    pub(crate) fn reply_error(self, error: MediaError) {
        match self.work {
            AgentWork::Thumbnail { reply, .. } => drop(reply.send(Err(error))),
            AgentWork::CacheStats { reply, .. } => drop(reply.send(Err(error))),
            #[cfg(any(test, feature = "test-util"))]
            AgentWork::Hold { .. } => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct LaneState {
    pub(crate) shutdown: bool,
    /// The transport slot: the newest job replaces a pending one (R-3).
    pub(crate) transport: Option<TransportJob>,
    /// Bumped by every post, so a playback hold sees it was superseded.
    pub(crate) version: u64,
    pub(crate) agent: VecDeque<AgentJob>,
    /// Stamped transport failures, for the worker's R-2 test.
    pub(crate) failures: Vec<(FrameStamp, MediaError)>,
    pub(crate) wakeup: Option<Wakeup>,
    /// H-2/H-3 (S2b-1): the readers' plans, states, rings and failures;
    /// K-1 (S2b-3): the scheduler's live bytes.
    pub(crate) readers: Readers<VideoSourceKey, Pinned>,
    /// K-3: synchronous fallback frames by reason: more sources than
    /// readers, and a required set over C.
    pub(crate) fallbacks: [u64; 2],
}

impl LaneState {
    /// K-3: an empty plan (every unpinned lookahead drops, to drop after
    /// unlock), counted by `reason`: 0 more sources than readers, 1 a
    /// required set over C.
    fn fall_back(&mut self, reason: usize, now: Duration) -> Posted<VideoSourceKey, Pinned> {
        self.fallbacks[reason] += 1;
        self.readers.post(Vec::new(), (HashMap::new(), 0), now)
    }
}

/// The worker/preview hand-off: one leaf lock and the `ready` condvar.
pub(crate) struct Lane {
    state: Mutex<LaneState>,
    ready: Condvar,
    /// H-2: the readers' condvar.
    pub(crate) work: Condvar,
    /// The readers' clock origin (H-2 quiescence).
    epoch: Instant,
    /// I10/I15: an injected advance of the readers' clock.
    #[cfg(test)]
    pub(crate) skew: Mutex<Duration>,
    /// Test support: each reader decode first takes one message from this
    /// gate (or proceeds once its sender is gone).
    #[cfg(test)]
    pub(crate) gate: Mutex<Option<Receiver<()>>>,
    /// Test support: a reader decode of exactly this time waits until the
    /// sender is gone.
    #[cfg(test)]
    pub(crate) hold_at: Mutex<Option<(i64, Receiver<()>)>>,
    /// K-2: reader decodes that ended `Cancelled`.
    #[cfg(test)]
    pub(crate) cancelled: std::sync::atomic::AtomicUsize,
    /// I10: preview threads an engine started.
    #[cfg(test)]
    pub(crate) previews: std::sync::atomic::AtomicUsize,
    /// Review A F3's barrier: called by a permit waiter each time it wakes,
    /// with `Permits` unlocked.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) woke: Mutex<Option<Arc<dyn Fn(u64) + Send + Sync>>>,
    /// Amendment R41's witness: each reader's open decoder and its source.
    #[cfg(test)]
    decoders: Mutex<HashMap<u64, VideoSourceKey>>,
    /// H-5: the `Permits` monitor, a leaf never taken with `state` (H-4).
    permits: Mutex<PermitBook>,
    permits_cv: Condvar,
    /// R-5's counters: a separate leaf, never taken with `state`.
    counters: Mutex<Counters>,
    /// R35 (re-review 3 D3): paint acks, handed to the worker without a
    /// lock. A full channel drops the newest ack (`acks_overflowed`).
    acks: (Sender<Ack>, Receiver<Ack>),
    acks_overflowed: AtomicU64,
    /// Amendment R37 (start-up): the newest epoch a `play` was issued with,
    /// stored by the caller under `state` (R-3/S-1: a paused job issued
    /// before it is superseded).
    play_issued: AtomicU64,
}

impl Default for Lane {
    fn default() -> Self {
        Self::with_parallelism(thread::available_parallelism().map_or(1, usize::from))
    }
}

impl Lane {
    /// A lane for P = `parallelism`: R readers, a pool of P permits (H-5).
    pub(crate) fn with_parallelism(parallelism: usize) -> Self {
        Self::with_budget(parallelism, FRAME_CACHE_BYTE_BUDGET)
    }

    /// K-1: C = `budget` for the scheduler path (tests shrink it).
    pub(crate) fn with_budget(parallelism: usize, budget: usize) -> Self {
        let state = LaneState {
            readers: Readers::new(parallelism).with_budget(budget),
            ..LaneState::default()
        };
        Self {
            state: Mutex::new(state),
            ready: Condvar::new(),
            work: Condvar::new(),
            epoch: Instant::now(),
            #[cfg(test)]
            skew: Mutex::default(),
            #[cfg(test)]
            gate: Mutex::default(),
            #[cfg(test)]
            hold_at: Mutex::default(),
            #[cfg(test)]
            cancelled: std::sync::atomic::AtomicUsize::default(),
            #[cfg(test)]
            previews: std::sync::atomic::AtomicUsize::default(),
            #[cfg(test)]
            woke: Mutex::default(),
            #[cfg(test)]
            decoders: Mutex::default(),
            permits: Mutex::new(PermitBook::new(parallelism)),
            permits_cv: Condvar::new(),
            counters: Mutex::default(),
            acks: crossbeam_channel::bounded(ACK_QUEUE),
            acks_overflowed: AtomicU64::new(0),
            play_issued: AtomicU64::new(0),
        }
    }

    /// I10: preview threads an engine started on this lane.
    #[cfg(test)]
    pub(crate) fn previews(&self) -> usize {
        self.previews.load(Ordering::Acquire)
    }

    /// Amendment R41's witness: reader `id` opened (`true`) or closed its
    /// decoder.
    #[cfg(test)]
    fn decoder_open(&self, id: u64, open: bool) {
        let key = open.then(|| {
            let state = self.lock();
            let slot = state.readers.slots.iter().find(|slot| slot.id == id);
            slot.map(|slot| slot.key.clone())
        });
        let mut decoders = self.decoders.lock().expect("decoders");
        match key.flatten() {
            Some(key) => decoders.insert(id, key),
            None => decoders.remove(&id),
        };
    }

    /// Amendment R41's witness: the readers' open decoders on sources
    /// `which` selects.
    #[cfg(test)]
    pub(crate) fn open_decoders(&self, which: impl Fn(&VideoSourceKey) -> bool) -> usize {
        let decoders = self.decoders.lock().expect("decoders");
        decoders.values().filter(|key| which(key)).count()
    }

    fn book(&self) -> MutexGuard<'_, PermitBook> {
        self.permits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// H-5 `PermitWait`: queue for `want` permits and wait at the FIFO; `None`
    /// once the request is cancelled or the lane stops.
    fn acquire(&self, id: u64, want: usize) -> Option<usize> {
        let mut book = self.book();
        book.enqueue(id, want);
        let granted = loop {
            match book.poll(id) {
                Poll::Granted(granted) => break Some(granted),
                Poll::Cancelled => break None,
                Poll::Wait => {
                    book = self
                        .permits_cv
                        .wait(book)
                        .unwrap_or_else(PoisonError::into_inner);
                    #[cfg(test)]
                    {
                        let hook = self.woke.lock().expect("woke").clone();
                        if let Some(hook) = hook {
                            drop(book);
                            hook(id);
                            book = self.book();
                        }
                    }
                }
            }
        };
        drop(book);
        // The next head may be granted from what is left (M58: a waiter
        // that woke before this grant and slept again has no other wake;
        // `a_grant_wakes_the_next_ticket`).
        self.permits_cv.notify_all();
        granted
    }

    /// H-5: a book change (release, exit, cancel), then the waiters look.
    fn permits_change(&self, change: impl FnOnce(&mut PermitBook)) {
        change(&mut self.book());
        self.permits_cv.notify_all();
    }

    /// R-5 permits in use: reader frame threads (≤ P, I11).
    pub(crate) fn permits_in_use(&self) -> usize {
        self.book().in_use()
    }

    pub(crate) fn lock(&self) -> Sched<'_> {
        Sched::new(self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn counters(&self) -> MutexGuard<'_, Counters> {
        self.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Playback::ack_presented` (R35): never waits, and takes no lock.
    pub(crate) fn ack(&self, ack: Ack) {
        if let Err(TrySendError::Full(_)) = self.acks.0.try_send(ack) {
            self.acks_overflowed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Acks the full channel dropped since the counters were cleared.
    pub(crate) fn acks_overflowed(&self) -> u64 {
        self.acks_overflowed.load(Ordering::Relaxed)
    }

    /// The worker's counters, with the acks handed over since its last
    /// pass moved in for settlement (R34: the worker alone settles).
    pub(crate) fn settle(&self) -> MutexGuard<'_, Counters> {
        let mut counters = self.counters();
        for ack in self.acks.1.try_iter() {
            counters.receive(ack);
        }
        counters
    }

    /// An explicit `play`: fresh counters, and the acks of the playback
    /// before it discarded.
    pub(crate) fn clear_counters(&self, underruns: [u64; 4]) {
        let mut counters = self.counters();
        for _ in self.acks.1.try_iter() {}
        counters.clear(underruns);
        self.acks_overflowed.store(0, Ordering::Relaxed);
    }

    fn notify(&self) {
        self.ready.notify_all();
    }

    /// Amendment R37 (start-up): a `play` was issued with `epoch`. Stored
    /// under `state`, then `ready` is notified, so a paused `FrameWait`
    /// cannot miss it.
    pub(crate) fn issue_play(&self, epoch: u64) {
        {
            let _state = self.lock();
            self.play_issued.fetch_max(epoch, Ordering::AcqRel);
        }
        self.notify();
    }

    /// Amendment R37 (start-up): a paused job stamped `stamp` was issued
    /// before the newest `play`, which supersedes it (issuance order).
    pub(crate) fn superseded_by_play(&self, stamp: FrameStamp) -> bool {
        self.play_issued.load(Ordering::Acquire) > stamp.epoch
    }

    /// The readers' clock (H-2), advanced by tests (never a sleep).
    pub(crate) fn now(&self) -> Duration {
        let now = self.epoch.elapsed();
        #[cfg(test)]
        let now = now + *self.skew.lock().expect("skew");
        now
    }

    /// Replace the transport slot (`None` clears it).
    pub(crate) fn post(&self, job: Option<TransportJob>) {
        let replaced = {
            let mut state = self.lock();
            state.version += 1;
            std::mem::replace(&mut state.transport, job)
        };
        self.notify();
        drop(replaced);
    }

    /// Rebind the pending transport job's library (the table gained
    /// lattices); its stamp and document are unchanged.
    pub(crate) fn rebind(&self, generation: u64, lut: &Arc<LutLibrary>) {
        let replaced = {
            let mut state = self.lock();
            state
                .transport
                .as_mut()
                .filter(|job| job.scene.generation == generation)
                .map(|job| std::mem::replace(&mut job.scene.lut, Arc::clone(lut)))
        };
        drop(replaced);
    }

    /// R-4 push: never waits. A full queue or a stopped lane replies at
    /// once, after unlock; `false` means the job was refused.
    pub(crate) fn try_push(&self, job: AgentJob) -> bool {
        let refusal = {
            let mut state = self.lock();
            if state.shutdown {
                Some(worker_stopped())
            } else if state.agent.len() >= AGENT_QUEUE_LIMIT {
                let full = "preview-thread: agent queue full".to_owned();
                Some(MediaError::Backend(full))
            } else {
                state.agent.push_back(job);
                self.ready.notify_all();
                return true;
            }
        };
        job.reply_error(refusal.unwrap_or_else(worker_stopped));
        false
    }

    /// Queued agent jobs, and how many are cancelled (test support).
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn waiting(&self) -> (usize, usize) {
        let state = self.lock();
        let cancelled = state
            .agent
            .iter()
            .filter(|job| job.cancel.load(Ordering::Acquire))
            .count();
        (state.agent.len(), cancelled)
    }

    pub(crate) fn set_wakeup(&self, wakeup: Wakeup) {
        let replaced = self.lock().wakeup.replace(wakeup);
        drop(replaced);
    }

    pub(crate) fn take_failures(&self) -> Vec<(FrameStamp, MediaError)> {
        std::mem::take(&mut self.lock().failures)
    }

    /// H-6 (1): stop the lane and hand back every queued agent job.
    pub(crate) fn shut_down(&self) -> (Vec<AgentJob>, Option<TransportJob>) {
        let moved = {
            let mut state = self.lock();
            state.shutdown = true;
            let agent = state.agent.drain(..).collect();
            (agent, state.transport.take())
        };
        self.notify();
        self.work.notify_all();
        self.permits_change(|book| book.shutdown = true);
        moved
    }

    /// R-4 cancellation: set the flag under the lock and notify `ready`.
    fn cancel(&self, flag: &AtomicBool) {
        {
            let _state = self.lock();
            flag.store(true, Ordering::Release);
        }
        self.notify();
    }
}

/// R-4: held by a `thumbnail_*` caller; dropping it cancels the job.
pub(crate) struct CancelOnDrop {
    lane: Arc<Lane>,
    flag: Arc<AtomicBool>,
}

impl CancelOnDrop {
    pub(crate) fn new(lane: &Arc<Lane>) -> Self {
        Self {
            lane: Arc::clone(lane),
            flag: Arc::default(),
        }
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.lane.cancel(&self.flag);
    }
}

pub(crate) enum Work {
    Agent(AgentJob),
    Paused(TransportJob, u64),
    Playback(TransportJob, u64),
}

/// Why a scheduled render did not produce a frame.
enum Halt {
    Failed(MediaError),
    /// A newer post or shutdown.
    Superseded,
    /// Playback: the frame's time passed (or an agent job's deadline) with
    /// a required layer missing, so the previous image is held (R-3, R-4).
    Held {
        agent: bool,
    },
}

/// A scheduled render's inputs: the frames, their pins (K-1: live until the
/// render is done) and the job's generated-raster reservation.
type Scheduled = (SuppliedFrames, Vec<Pinned>, Option<Hold>);

/// The job's frames by (source, time), and the pins that keep them live.
fn supplied(
    demand: &ReaderDemand,
    results: Vec<Result<Pinned, MediaError>>,
) -> (SuppliedFrames, Vec<Pinned>) {
    let pins = results.iter().flatten().cloned().collect();
    let frames = (demand.required.iter().cloned())
        .zip(results.into_iter().map(|r| r.map(|pin| pin.frame)))
        .collect();
    (frames, pins)
}

/// H-5/K-3: per source its required and lookahead times, each source's
/// frame bytes, and the required set's bytes (distinct frames plus G).
#[allow(clippy::type_complexity)]
fn demand_plan(
    demand: &ReaderDemand,
) -> (
    Vec<(VideoSourceKey, Vec<i64>, Vec<i64>)>,
    HashMap<VideoSourceKey, usize>,
    usize,
) {
    let per_source = (demand.sources.iter())
        .map(|(key, (_, lookahead))| {
            let required = demand.required.iter().filter(|(k, _)| k == key);
            let times = required.map(|(_, t)| *t).collect();
            (key.clone(), times, lookahead.clone())
        })
        .collect();
    let sizes: HashMap<_, _> = (demand.sources.iter())
        .map(|(key, (spec, _))| (key.clone(), spec.frame_bytes))
        .collect();
    let distinct: HashSet<_> = demand.required.iter().collect();
    let set = (distinct.iter())
        .map(|(key, _)| sizes.get(key).copied().unwrap_or(0))
        .fold(demand.generated, usize::saturating_add);
    (per_source, sizes, set)
}

/// A transport render's `FrameWait` terms (H-2): its lane version, and for
/// playback the agent deadline (lead + 1 frames, R-4).
struct FrameWait {
    version: u64,
    playback: Option<Instant>,
    /// A paused job's stamp: a `play` issued after it supersedes the wait
    /// (Amendment R37).
    paused: Option<FrameStamp>,
}

#[cfg(test)]
thread_local! {
    /// E-2 witness: the next reader spawn on this thread fails.
    pub(crate) static FAIL_READER_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// H-1: start reader `id`. E-2: a spawn failure is prefixed `decode-reader:`.
fn spawn_reader(
    lane: &Arc<Lane>,
    (id, spec): (u64, SourceSpec),
    stop: &Arc<AtomicBool>,
) -> Result<JoinHandle<()>, MediaError> {
    #[cfg(test)]
    if FAIL_READER_SPAWN.with(std::cell::Cell::take) {
        return Err(MediaError::Backend(
            "decode-reader: spawn failed: injected".to_owned(),
        ));
    }
    let (lane, stop) = (Arc::clone(lane), Arc::clone(stop));
    let spawned = thread::Builder::new().name(format!("kinewright-decode-{id}"));
    spawned
        .spawn(move || read(&lane, id, &spec, &stop))
        .map_err(|error| MediaError::Backend(format!("decode-reader: spawn failed: {error}")))
}

/// A reader's loop (H-2): every step is decided under the lock; decoding,
/// opening and closing run outside it, and what `deliver` returns is
/// dropped after unlock (H-4).
fn read(lane: &Arc<Lane>, id: u64, spec: &SourceSpec, stop: &Arc<AtomicBool>) {
    let mut decoder: Option<VideoDecoder> = None;
    let mut threads = 0;
    let mut state = lane.lock();
    loop {
        let now = lane.now();
        let shutdown = state.shutdown;
        match state.readers.next(id, now, shutdown) {
            Next::Retire => break,
            Next::Wait { until } => {
                state = state.wait_timeout(&lane.work, until.saturating_sub(now));
            }
            // H-5: permits before a decoder exists (X-5), outside `Sched`.
            Next::Open { want } => {
                drop(state);
                // A ticket waits: inactive readers shrink or retire (H-5).
                lane.work.notify_all();
                #[cfg(test)]
                lane.notify(); // a test waiting for `PermitWait`
                let granted = lane.acquire(id, want);
                threads = granted.unwrap_or(0);
                state = lane.lock();
                state.readers.granted(id, granted, lane.now());
                lane.work.notify_all();
                #[cfg(test)]
                lane.notify(); // a test waiting for a grant
            }
            Next::Close => {
                drop(state);
                drop(decoder.take());
                #[cfg(test)]
                lane.decoder_open(id, false);
                lane.permits_change(|book| book.release(id));
                state = lane.lock();
                state.readers.closed(id);
                #[cfg(test)]
                lane.notify(); // a test waiting for a reopen
            }
            Next::Decode { at, version, bytes } => {
                // A stop meant for an earlier lookahead decode (K-2) lapses.
                stop.store(false, Ordering::Release);
                drop(state);
                let hold = Hold::adopt(lane, bytes);
                #[cfg(test)]
                {
                    let gate = lane.gate.lock().expect("gate").clone();
                    if let Some(gate) = gate {
                        lane.notify(); // a test waiting for `Decoding`
                        let _ = gate.recv();
                    }
                    let hold_at = lane.hold_at.lock().expect("hold").clone();
                    if let Some((_, release)) = hold_at.filter(|(time, _)| *time == at) {
                        lane.notify(); // a test waiting for `Decoding`
                        let _ = release.recv();
                    }
                }
                let result = match &mut decoder {
                    Some(decoder) => spec.decode(decoder, at),
                    None => spec.open(threads).and_then(|mut opened| {
                        opened.set_stop(Arc::clone(stop));
                        #[cfg(test)]
                        lane.decoder_open(id, true);
                        spec.decode(decoder.insert(opened), at)
                    }),
                };
                // Stopped: the reader retires, or K-2 stopped its lookahead.
                if matches!(result, Err(MediaError::Cancelled)) {
                    #[cfg(test)]
                    lane.cancelled.fetch_add(1, Ordering::AcqRel);
                    lane.lock().readers.stopped(id, lane.now());
                    drop(hold);
                    #[cfg(test)]
                    lane.notify(); // a test waiting for the stop
                    state = lane.lock();
                    continue;
                }
                // K-1 (review B F1): f is exact; a frame of any other size
                // is a failure `deliver` cleans up, never an undercharge.
                let result = result.and_then(|frame| match frame.byte_len() {
                    len if len == bytes => Ok(frame),
                    len => Err(MediaError::Backend(format!(
                        "decode-reader: a frame of {len} bytes, {bytes} reserved (K-1)"
                    ))),
                });
                let result = result.map(|frame| {
                    let hold = Arc::new(hold);
                    Pinned { frame, hold }
                });
                deliver(lane, id, (at, version), result);
                state = lane.lock();
            }
        }
    }
    drop(state);
    drop(decoder);
    #[cfg(test)]
    lane.decoder_open(id, false);
    lane.permits_change(|book| book.forget(id));
    lane.lock().readers.exited(id);
    lane.notify();
}

/// A reader's result for `at` (H-3), then the wakes: the preview, and the
/// readers if a dropped failure's time is re-demanded or (Amendment R43)
/// another reader walking its plan waits for it. What `deliver` returns
/// drops after unlock (H-4).
fn deliver(lane: &Lane, id: u64, (at, version): (i64, u64), result: Result<Pinned, MediaError>) {
    let now = lane.now();
    let (delivered, awaited) = {
        let mut state = lane.lock();
        let delivered = state.readers.deliver(id, at, version, result, now);
        (delivered, state.readers.awaited(id, at))
    };
    lane.notify();
    if delivered.1.is_some() || awaited {
        lane.work.notify_all();
    }
    drop(delivered);
}

/// How a playback attempt ended (R-4's "attempt").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attempt {
    Published,
    /// The clock passed `at + 1` frame first, or an agent job's deadline
    /// ended the hold.
    Dropped {
        agent: bool,
    },
    /// A newer post or shutdown replaced the job.
    Superseded,
    /// Nothing left to render (end of programme, or a failed render).
    Parked,
    /// A stepped test's hold is undecided; the next attempt resumes it.
    #[cfg(test)]
    Pending,
}

/// A rendered playback frame waiting for its due time.
struct Held {
    version: u64,
    frame: PreviewFrame,
    deadline: Instant,
}

/// The preview thread's state; every method runs on that thread (or on a
/// test thread driving it step by step).
pub(crate) struct Preview {
    lane: Arc<Lane>,
    renderer: FrameRenderer,
    lut: Arc<LutLibrary>,
    generation: Option<u64>,
    clock: Arc<SharedClock>,
    frames_tx: Sender<PreviewFrame>,
    frames_drop_rx: Receiver<PreviewFrame>,
    /// R-3 lead: an EWMA of playback render time.
    render_ewma_ms: f64,
    /// The playback job version the cursor below belongs to.
    playback_version: Option<u64>,
    next_at: i64,
    /// A version whose playback job has nothing more to render.
    parked: Option<u64>,
    held: Option<Held>,
    /// R-4 fairness: one agent job after each transport attempt.
    agent_turn: bool,
    /// Re-review 2 D4: the newest playback frame (epoch, frame) published;
    /// an agent job never displaces it.
    published: Option<(u64, i64)>,
    /// H-1: this preview's reader threads, joined when it goes.
    readers: Vec<JoinHandle<()>>,
    /// H-2/K-2: each reader's stop flag (a packet boundary): set for all
    /// when the preview goes, for one to stop its lookahead decode.
    stops: HashMap<u64, Arc<AtomicBool>>,
    /// K-1: the title rasters this path keeps cached (G).
    titles: Option<Hold>,
    /// R38 D3: the cached rasters `titles` charges. A raster cached by
    /// another path (a thumbnail, K-3's synchronous render) is adopted into
    /// it with its charge, or dropped, before anything else runs.
    charged: HashSet<TitleCacheKey>,
    /// K-1 (review B F1): each source's measured frame size.
    sizes: FrameSizes,
    #[cfg(test)]
    pub(crate) faults: Arc<crate::engine::Faults>,
}

impl Drop for Preview {
    /// H-6 (3)/(5): every reader retires, closes its decoder and exits
    /// before the preview does (the worker joins the preview).
    fn drop(&mut self) {
        let frames = {
            let mut state = self.lane.lock();
            state.readers.retire_all();
            // Under `Sched`, so no reader resets its flag after (K-2).
            for stop in self.stops.values() {
                stop.store(true, Ordering::Release);
            }
            state.readers.clear_rings()
        };
        self.lane.work.notify_all();
        drop(frames);
        self.titles = None;
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Preview {
    pub(crate) fn new(
        lane: Arc<Lane>,
        renderer: FrameRenderer,
        clock: Arc<SharedClock>,
        frames: (Sender<PreviewFrame>, Receiver<PreviewFrame>),
    ) -> Self {
        Self {
            lane,
            renderer,
            lut: Arc::new(LutLibrary::default()),
            generation: None,
            clock,
            frames_tx: frames.0,
            frames_drop_rx: frames.1,
            render_ewma_ms: 0.0,
            playback_version: None,
            next_at: 0,
            parked: None,
            held: None,
            agent_turn: false,
            published: None,
            readers: Vec::new(),
            stops: HashMap::new(),
            titles: None,
            charged: HashSet::new(),
            sizes: FrameSizes::default(),
            #[cfg(test)]
            faults: Arc::default(),
        }
    }

    /// Forget a previous model run's playback cursor (tests reuse one
    /// renderer).
    #[cfg(test)]
    pub(crate) fn reset_cursor(&mut self) {
        (self.playback_version, self.parked, self.held) = (None, None, None);
        (self.next_at, self.agent_turn) = (0, false);
        self.published = None;
    }

    pub(crate) fn run(mut self) {
        while let Some(work) = self.next_work(true) {
            self.execute(work);
        }
    }

    /// R-4 fair selection: one agent job after each transport attempt,
    /// back to back while no transport job is runnable.
    pub(crate) fn execute(&mut self, work: Work) {
        match work {
            Work::Agent(job) => {
                let playing = self.playback_running();
                if playing {
                    self.lane.counters().agent_started(self.published);
                }
                self.run_agent(job, false);
                if playing {
                    self.lane.counters().agent_finished();
                }
                self.agent_turn = false;
            }
            Work::Paused(job, version) => {
                self.run_paused(&job, version);
                self.agent_turn = true;
            }
            Work::Playback(job, version) => {
                self.run_playback(&job, version);
                self.agent_turn = true;
            }
        }
    }

    /// A runnable playback job: an agent job run now displaces its due
    /// frames. Re-review B D3 / 2 D4: the worker charges `dropped_agent`
    /// with the due frames it registers in that playback's epoch while the
    /// job runs, past the newest published frame, until a paint settles
    /// them (`Counters::agent_started`); a seek's jump, and frames before
    /// an explicit `play` cleared the counters, are not.
    fn playback_running(&self) -> bool {
        let state = self.lane.lock();
        let runnable = self.parked != Some(state.version);
        runnable
            && (state.transport.as_ref())
                .is_some_and(|job| matches!(job.kind, JobKind::Playback { .. }))
    }

    /// The next piece of work, waiting for one if `wait`; `None` once the
    /// lane shuts down (or, without `wait`, when there is none).
    pub(crate) fn next_work(&mut self, wait: bool) -> Option<Work> {
        let mut discarded = Vec::new();
        let mut superseded_jobs = Vec::new();
        let mut state = self.lane.lock();
        let work = loop {
            if state.shutdown {
                break None;
            }
            while let Some(job) = state.agent.pop_front() {
                if !job.cancel.load(Ordering::Acquire) {
                    state.agent.push_front(job);
                    break;
                }
                discarded.push(job);
            }
            // Amendment R37: a paused job a `play` superseded is dropped
            // untaken (after unlock); the worker posts the playback.
            let superseded = (state.transport.as_ref()).is_some_and(|job| {
                matches!(job.kind, JobKind::Paused(_)) && self.lane.superseded_by_play(job.stamp)
            });
            if superseded {
                superseded_jobs.extend(state.transport.take());
            }
            let parked = self.parked == Some(state.version);
            let runnable = (state.transport.as_ref())
                .map(|job| job.kind)
                .filter(|kind| matches!(kind, JobKind::Paused(_)) || !parked);
            if !state.agent.is_empty() && (self.agent_turn || runnable.is_none()) {
                break state.agent.pop_front().map(Work::Agent);
            }
            match runnable {
                Some(JobKind::Paused(_)) => {
                    let version = state.version;
                    break state.transport.take().map(|job| Work::Paused(job, version));
                }
                Some(JobKind::Playback { .. }) => {
                    let version = state.version;
                    break state
                        .transport
                        .clone()
                        .map(|job| Work::Playback(job, version));
                }
                // H-1/H-7: nothing queued, so the synchronous renderer's
                // decoders close (after unlock) before the preview parks.
                None if self.renderer.has_sources() => {
                    drop(state);
                    self.renderer.release_sources();
                    state = self.lane.lock();
                    self.lane.ready.notify_all();
                }
                None if !wait => break None,
                None => state = state.wait(&self.lane.ready),
            }
        };
        drop(state);
        // H-4: cancelled jobs' reply senders drop after unlock, unanswered.
        drop((discarded, superseded_jobs));
        work
    }

    fn bind(&mut self, generation: Option<u64>, lut: &Arc<LutLibrary>) {
        if generation.is_some() && generation != self.generation {
            self.renderer.clear();
            self.titles = None;
            self.charged.clear();
            self.generation = generation;
        }
        if !Arc::ptr_eq(&self.lut, lut) {
            self.lut = Arc::clone(lut);
            self.renderer.set_lut_library(Arc::clone(lut));
        }
    }

    /// Render one monitor frame; `Ok(None)` when a test fault withholds it.
    fn render_monitor(
        &mut self,
        scene: &Scene,
        at: TimeCode,
        wait: &FrameWait,
    ) -> Result<Option<FrameTexture>, Halt> {
        let document = &*scene.document;
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        #[cfg(test)]
        if self.faults.fail_render.swap(false, Ordering::AcqRel) {
            let error = MediaError::Backend("injected render failure".to_owned());
            return Err(Halt::Failed(error));
        } else if self.faults.fake_render.load(Ordering::Acquire) {
            // The I8 oracle's witness of which document rendered it.
            let duration = u32::try_from(document.duration.0).unwrap_or(u32::MAX);
            let rgba = Arc::new(duration.to_le_bytes().to_vec());
            let (width, height) = (1, 1);
            return Ok(Some(FrameTexture {
                width,
                height,
                rgba,
            }));
        }
        let horizon = if wait.playback.is_some() {
            PREFETCH_FRAMES
        } else {
            0
        };
        if self.generation != Some(scene.generation) {
            self.forget_sources(document, scale);
        }
        let demand = reader_demand(document, at, resolution, scale, horizon, &mut self.sizes);
        let demand = demand.map_err(Halt::Failed)?;
        // Bound first: a new generation's cleared titles are not resident
        // when admission counts them (review B S2).
        self.bind(Some(scene.generation), &scene.lut);
        let frames = self.schedule(&demand, at, wait)?;
        // R38 D2: an agent job run while the wait was suspended binds its
        // own document's LUT library; composite with this scene's.
        self.bind(Some(scene.generation), &scene.lut);
        let frame = if let Some((frames, pins, generated)) = frames {
            let frame = (self.renderer).render_scheduled(document, at, resolution, scale, &frames);
            drop((frames, pins));
            // K-1: the job's generated bytes join the titles kept,
            // then shrink to the rasters still cached. Each cached
            // raster is one of the job's: charged before, or admitted
            // now (R38 D3).
            let titles = self
                .titles
                .get_or_insert_with(|| Hold::adopt(&self.lane, 0));
            if let Some(mut generated) = generated {
                titles.bytes += std::mem::take(&mut generated.bytes);
            }
            let cached = self.renderer.title_entries();
            debug_assert!(
                cached.iter().map(|(_, bytes)| bytes).sum::<usize>() <= titles.bytes,
                "K-1: a cached raster is uncharged"
            );
            self.charged = cached.into_iter().map(|(key, _)| key).collect();
            titles.shrink_to(self.renderer.title_bytes());
            frame
        } else {
            // More sources than readers: today's synchronous render (K-3).
            let strategy = if wait.playback.is_some() {
                DecodeStrategy::Sequential
            } else {
                DecodeStrategy::Seek
            };
            let frame = (self.renderer).render_live(document, at, resolution, scale, strategy);
            self.settle_titles();
            frame
        };
        let frame = frame.map_err(Halt::Failed)?;
        #[cfg(test)]
        if !self.faults.publish_after_render() {
            return Ok(None);
        }
        Ok(Some(frame))
    }

    /// Amendment R41 (Windows CI run 36528931046; U-1): a new document
    /// forgets the sources it no longer has. Their sizes and travel go, and
    /// their readers retire and are waited for here, on the preview thread
    /// (never the UI's), so each has closed its decoder, and released its
    /// file, before this document's first frame is published. The
    /// synchronous renderer's decoders close in `bind` (a new generation).
    fn forget_sources(&mut self, document: &Document, scale: RenderScale) {
        let keep = document_source_keys(document, scale);
        self.sizes.retain(|key| keep.contains(key));
        let (retired, posted) = {
            let mut state = self.lane.lock();
            let forgot = state.readers.forget(|key| keep.contains(key));
            // Under `Sched`, so no reader resets its flag after (K-2).
            self.stop(&forgot.0);
            forgot
        };
        self.close(&retired, posted);
    }

    /// Amendment R41: after unlock, wake the `retired` readers, cancel
    /// their tickets and drop what they held (`posted`), then wait until
    /// each has closed its decoder and exited. A retired reader leaves at
    /// its next check or packet boundary (its stop flag is set).
    fn close(&self, retired: &[u64], posted: Posted<VideoSourceKey, Pinned>) {
        let spawn = self.posted(posted);
        debug_assert!(spawn.is_empty(), "a retirement starts no reader");
        if retired.is_empty() {
            return;
        }
        let mut state = self.lane.lock();
        while (state.readers.slots.iter()).any(|slot| retired.contains(&slot.id)) {
            state = state.wait(&self.lane.ready);
        }
    }

    /// H-2/H-3: post the job's plan, start the readers it needs, admit its
    /// required set (K-2), then `FrameWait` until every required frame has
    /// a result or a current failure. `None`: K-3's synchronous fallback.
    fn schedule(
        &mut self,
        demand: &ReaderDemand,
        at: TimeCode,
        wait: &FrameWait,
    ) -> Result<Option<Scheduled>, Halt> {
        let (per_source, sizes, set) = demand_plan(demand);
        let lane = Arc::clone(&self.lane);
        let now = lane.now();
        let mut state = lane.lock();
        let played = || {
            wait.paused
                .is_some_and(|stamp| lane.superseded_by_play(stamp))
        };
        if state.shutdown || state.version != wait.version || played() {
            return Err(Halt::Superseded);
        }
        // K-3: more sources than readers, or a required set over C.
        let regions = plan_regions(&per_source, state.readers.limit());
        let reason = match &regions {
            None => Some(0),
            Some(_) if !state.readers.fits(set) => Some(1),
            Some(_) => None,
        };
        if let Some(reason) = reason {
            let posted = state.fall_back(reason, now);
            drop(state);
            return Ok(self.fell_back(posted, demand));
        }
        let plan = (sizes, demand.generated);
        let planned = regions.expect("K-3 returned above");
        // The set is admitted under the post's lock, before a reader starts.
        let mut posted = Some(state.readers.post_planned(planned, plan, now));
        // Review B S2: only the rasters not resident are reserved.
        let mut generated = Some(demand.generated_uncharged(&self.charged));
        let mut granted = None;
        loop {
            if generated.is_some() || state.readers.unreserved() {
                let unlocked;
                (state, unlocked) = self.admit((&lane, state), &mut generated, &mut granted);
                if unlocked {
                    self.owe_generated(demand, &mut generated, granted.as_ref());
                    continue; // a release while unlocked woke no one
                }
            }
            if let Some(posted) = posted.take() {
                drop(state);
                self.started(posted, demand);
                state = lane.lock();
                continue;
            }
            // K-2: nothing renders before its whole set is reserved.
            let resolved =
                (state.readers.resolve(&demand.required)).filter(|_| generated.is_none());
            let agent = |job: &AgentJob| !job.cancel.load(Ordering::Acquire);
            let view = WaitView {
                shutdown: state.shutdown,
                superseded: state.version != wait.version || played(),
                resolved: resolved.is_some(),
                playback: wait.playback.is_some(),
                agent_waiting: state.agent.iter().any(agent),
                expired: self.clock.position().0 > at.0,
                agent_due: wait.playback.is_some_and(|due| Instant::now() >= due),
            };
            match wait_step(view) {
                // H-4: `granted` drops after unlock.
                WaitStep::Shutdown | WaitStep::Superseded => {
                    let withdrawn =
                        (!state.shutdown && played()).then(|| self.withdraw(&mut state));
                    drop(state);
                    if let Some(posted) = withdrawn {
                        self.started(posted, demand);
                    }
                    return Err(Halt::Superseded);
                }
                WaitStep::Held { agent } => {
                    drop(state);
                    return Err(Halt::Held { agent });
                }
                WaitStep::Ready => {
                    self.drain_counts(state);
                    let (frames, pins) = supplied(demand, resolved.unwrap_or_default());
                    return Ok(Some((frames, pins, granted)));
                }
                WaitStep::Suspend => {
                    // R-4: a paused wait runs the agent job, demands posted.
                    let position = state.agent.iter().position(agent);
                    let discarded: Vec<_> = state.agent.drain(..position.unwrap_or(0)).collect();
                    let job = state.agent.pop_front();
                    drop(state);
                    drop(discarded);
                    if let Some(job) = job {
                        self.run_agent(job, true);
                        self.owe_generated(demand, &mut generated, granted.as_ref());
                    }
                    state = lane.lock();
                }
                WaitStep::Wait if state.readers.waiting() => {
                    let assigned = state.readers.assign(self.lane.now());
                    if assigned.spawn.is_empty() && assigned.cancel.is_empty() && !assigned.retired
                    {
                        state = self.wait_ready(state, wait);
                    } else {
                        drop(state);
                        self.started(assigned, demand);
                        state = lane.lock();
                    }
                }
                WaitStep::Wait => state = self.wait_ready(state, wait),
            }
        }
    }

    /// K-2 / R38 / R41: the plans' refusal, merge, fold and rewind counts, taken
    /// under `Sched`, join the engine's stats after unlock.
    fn drain_counts(&self, mut state: Sched<'_>) {
        let starved = std::mem::take(&mut state.readers.starved);
        let merged = std::mem::take(&mut state.readers.regions_merged);
        let folded = std::mem::take(&mut state.readers.regions_folded);
        let rewinds = std::mem::take(&mut state.readers.merged_rewinds);
        drop(state);
        let mut counters = self.lane.counters();
        counters.stats.lookahead_starved += starved;
        counters.stats.regions_merged += merged;
        counters.stats.regions_folded += folded;
        counters.stats.merged_rewinds += rewinds;
    }

    /// Amendment R37: a paused wait a `play` superseded withdraws its
    /// demand (an empty plan, like K-3's) and stops its decodes, so no reader
    /// seeks to the stale frame. What the plan removed drops after unlock.
    fn withdraw(&self, state: &mut LaneState) -> Posted<VideoSourceKey, Pinned> {
        let posted = (state.readers).post(Vec::new(), (HashMap::new(), 0), self.lane.now());
        self.stop(&state.readers.decoding());
        posted
    }

    /// K-2: admit the job's set (its G once). Draining stops the lookahead
    /// decodes, then drops the evicted frames and title rasters (K-5) after
    /// unlock; `true` if it unlocked.
    fn admit<'a>(
        &mut self,
        (lane, mut state): (&'a Lane, Sched<'a>),
        generated: &mut Option<usize>,
        granted: &mut Option<Hold>,
    ) -> (Sched<'a>, bool) {
        match state.readers.admit(generated.unwrap_or(0)) {
            Admission::Ready { generated: bytes } => {
                if generated.take().is_some() {
                    match granted {
                        Some(hold) => hold.bytes += bytes,
                        None => *granted = Some(Hold::adopt(&self.lane, bytes)),
                    }
                }
                self.lane.work.notify_all();
                (state, false)
            }
            Admission::Wait { evicted, stop } => {
                self.stop(&stop);
                if evicted.is_empty() && self.titles.is_none() {
                    return (state, false); // wait under this same lock
                }
                drop(state);
                drop(evicted);
                self.renderer.clear_titles();
                self.titles = None;
                self.charged.clear();
                (lane.lock(), true)
            }
        }
    }

    /// R38 D3: after a path that caches generated rasters outside admission
    /// (a thumbnail, K-3's synchronous render), every uncharged raster is
    /// adopted — its bytes charged to `titles` under `Sched`, if they fit in
    /// C beside everything live — or dropped. Charges whose raster the
    /// renderer evicted are released. Afterwards `titles` charges exactly
    /// the rasters cached.
    fn settle_titles(&mut self) {
        let cached = self.renderer.title_entries();
        let present: HashSet<&TitleCacheKey> = cached.iter().map(|(key, _)| key).collect();
        self.charged.retain(|key| present.contains(key));
        let (mut adopted, mut dropped) = (0usize, HashSet::new());
        let mut state = self.lane.lock();
        for (key, bytes) in &cached {
            if self.charged.contains(key) {
                continue;
            }
            if state.readers.adopt(*bytes) {
                adopted += bytes;
                self.charged.insert(key.clone());
            } else {
                dropped.insert(key.clone());
            }
        }
        drop(state);
        // H-4: the dropped rasters and any released charge go after unlock.
        self.renderer.drop_titles(&dropped);
        let charged = (cached.iter())
            .filter(|(key, _)| self.charged.contains(key))
            .map(|(_, bytes)| bytes)
            .sum::<usize>();
        let titles = self
            .titles
            .get_or_insert_with(|| Hold::adopt(&self.lane, 0));
        titles.bytes += adopted;
        titles.shrink_to(charged);
    }

    /// Review B S2: resident rasters that went while unlocked (a drain, a
    /// cache clear) are owed again: G beyond what is admitted is reserved.
    fn owe_generated(
        &self,
        demand: &ReaderDemand,
        generated: &mut Option<usize>,
        granted: Option<&Hold>,
    ) {
        let need = demand.generated_uncharged(&self.charged);
        let admitted = granted.map_or(0, |hold| hold.bytes);
        if need > admitted {
            *generated = Some(need - admitted);
        }
    }

    /// K-5: ask the named readers to stop at their next packet boundary.
    fn stop(&self, readers: &[u64]) {
        for id in readers {
            if let Some(flag) = self.stops.get(id) {
                flag.store(true, Ordering::Release);
            }
        }
    }

    /// K-3, after unlock: count the frame and drop what the empty plan
    /// removed; the synchronous renderer takes it.
    fn fell_back(
        &mut self,
        posted: Posted<VideoSourceKey, Pinned>,
        demand: &ReaderDemand,
    ) -> Option<Scheduled> {
        self.lane.counters().stats.sync_fallback_frames += 1;
        self.started(posted, demand);
        None
    }

    fn wait_ready<'a>(&self, state: Sched<'a>, wait: &FrameWait) -> Sched<'a> {
        let ready = &self.lane.ready;
        if wait.playback.is_some() {
            state.wait_timeout(ready, HOLD_POLL)
        } else {
            state.wait(ready)
        }
    }

    /// After a post or an assignment, outside the lock: wake the readers
    /// (plans changed, obsolete ones retire), cancel obsolete tickets
    /// (H-5), drop what the plan removed (H-4) and start new readers.
    fn started(&mut self, posted: Posted<VideoSourceKey, Pinned>, demand: &ReaderDemand) {
        let spawn = self.posted(posted);
        self.spawn(spawn, demand);
    }

    /// `started` without a demand to start readers for: the readers the
    /// post would start (none for an empty plan).
    fn posted(&self, posted: Posted<VideoSourceKey, Pinned>) -> Vec<(u64, VideoSourceKey)> {
        let Posted {
            spawn,
            cancel,
            dropped,
            ..
        } = posted;
        self.lane.work.notify_all();
        if !cancel.is_empty() {
            self.lane
                .permits_change(|book| cancel.iter().for_each(|id| book.cancel(*id)));
        }
        drop(dropped);
        spawn
    }

    /// Start readers outside the lock; one that cannot start fails its
    /// required frames in the current plan (E-2).
    fn spawn(&mut self, spawn: Vec<(u64, VideoSourceKey)>, demand: &ReaderDemand) {
        self.readers.retain(|reader| !reader.is_finished());
        let live: Vec<u64> = self
            .lane
            .lock()
            .readers
            .slots
            .iter()
            .map(|slot| slot.id)
            .collect();
        self.stops.retain(|id, _| live.contains(id));
        for (id, key) in spawn {
            let spec = demand.sources.get(&key).map(|(spec, _)| spec.clone());
            let stop = Arc::<AtomicBool>::default();
            // Amendment R43: known to `Permits` before its thread runs.
            self.lane.book().join(id);
            let started = spec
                .ok_or_else(|| MediaError::Backend("decode-reader: no source".to_owned()))
                .and_then(|spec| spawn_reader(&self.lane, (id, spec), &stop));
            match started {
                Ok(reader) => {
                    self.readers.push(reader);
                    self.stops.insert(id, stop);
                }
                Err(error) => {
                    self.lane.permits_change(|book| book.forget(id));
                    self.lane.lock().readers.fail_start(id, &error);
                    self.lane.notify();
                }
            }
        }
    }

    fn publish(&self, frame: PreviewFrame) {
        send_latest(&self.frames_tx, &self.frames_drop_rx, frame);
        let wakeup = self.lane.lock().wakeup.clone();
        if let Some(wakeup) = wakeup {
            wakeup();
        }
    }

    fn fail(&self, stamp: FrameStamp, error: MediaError) {
        self.lane.lock().failures.push((stamp, error));
    }

    /// S-1: one exact render of the paused target, stamped with its job.
    pub(crate) fn run_paused(&mut self, job: &TransportJob, version: u64) {
        let JobKind::Paused(at) = job.kind else {
            return;
        };
        let wait = FrameWait {
            version,
            playback: None,
            paused: Some(job.stamp),
        };
        match self.render_monitor(&job.scene, at, &wait) {
            Ok(Some(texture)) => self.publish(PreviewFrame {
                at,
                stamp: job.stamp,
                texture,
            }),
            Ok(None) | Err(Halt::Superseded | Halt::Held { .. }) => {}
            Err(Halt::Failed(error)) => self.fail(job.stamp, error),
        }
    }

    /// R-3: one playback attempt. Render at clock + lead, hold until the
    /// clock reaches it (never early), drop it once the clock passes it.
    pub(crate) fn run_playback(&mut self, job: &TransportJob, version: u64) -> Attempt {
        let JobKind::Playback { from } = job.kind else {
            return Attempt::Parked;
        };
        let held = match self.held.take() {
            Some(held) if held.version == version => held,
            _ => match self.render_playback(job, version, from) {
                Ok(held) => held,
                Err(attempt) => return attempt,
            },
        };
        let attempt = self.hold(&held);
        match attempt {
            Attempt::Published => {
                self.published = Some((held.frame.stamp.epoch, held.frame.at.0));
                self.publish(held.frame);
            }
            #[cfg(test)]
            Attempt::Pending => self.held = Some(held),
            _ => {}
        }
        attempt
    }

    fn render_playback(
        &mut self,
        job: &TransportJob,
        version: u64,
        from: TimeCode,
    ) -> Result<Held, Attempt> {
        let document = Arc::clone(&job.scene.document);
        let first = self.playback_version != Some(version);
        if first {
            self.playback_version = Some(version);
            self.next_at = from.0;
        }
        let frame_ms = frame_ms(document.fps);
        let lead = lead_frames(self.render_ewma_ms, frame_ms);
        // Amendment R37: the first target is `from` (the clock's frame if it
        // already passed it), not clock + lead.
        let ahead = if first { 0 } else { lead };
        let target = (self.clock.position().0 + ahead).max(self.next_at);
        if target >= document.duration.0 {
            self.parked = Some(version);
            return Err(Attempt::Parked);
        }
        let started = Instant::now();
        let at = TimeCode(target);
        let frames = u32::try_from(lead + 1).unwrap_or(3);
        let wait = Duration::from_secs_f64(frame_ms * f64::from(frames) / 1e3);
        let terms = FrameWait {
            version,
            playback: Some(started + wait),
            paused: None,
        };
        let rendered = self.render_monitor(&job.scene, at, &terms);
        let took = started.elapsed().as_secs_f64() * 1_000.0;
        self.render_ewma_ms = if self.render_ewma_ms == 0.0 {
            took
        } else {
            0.8f64.mul_add(self.render_ewma_ms, 0.2 * took)
        };
        self.next_at = target + 1;
        let texture = match rendered {
            Ok(Some(texture)) => texture,
            Ok(None) => return Err(Attempt::Dropped { agent: false }),
            Err(Halt::Superseded) => return Err(Attempt::Superseded),
            Err(Halt::Held { agent }) => return Err(Attempt::Dropped { agent }),
            Err(Halt::Failed(error)) => {
                self.fail(job.stamp, error);
                self.parked = Some(version);
                return Err(Attempt::Parked);
            }
        };
        let frame = PreviewFrame {
            at,
            stamp: job.stamp,
            texture,
        };
        let deadline = Instant::now() + wait;
        Ok(Held {
            version,
            frame,
            deadline,
        })
    }

    /// The playback `FrameWait`: publish once `position() ≥ at`; drop once it
    /// passes `at`, or at the deadline when an agent job waits (R-4).
    fn hold(&self, held: &Held) -> Attempt {
        #[cfg(test)]
        if let Some(holding) = (self.faults.on_hold.lock().ok()).and_then(|mut hold| hold.take()) {
            let _ = holding.send(());
        }
        let target = held.frame.at.0;
        let mut state = self.lane.lock();
        loop {
            if state.shutdown || state.version != held.version {
                return Attempt::Superseded;
            }
            let position = self.clock.position().0;
            if position > target {
                return Attempt::Dropped { agent: false };
            }
            if position == target {
                return Attempt::Published;
            }
            if !state.agent.is_empty() && Instant::now() >= held.deadline {
                return Attempt::Dropped { agent: true };
            }
            #[cfg(test)]
            if self.faults.step_hold.load(Ordering::Acquire) {
                return Attempt::Pending;
            }
            state = state.wait_timeout(&self.lane.ready, HOLD_POLL);
        }
    }

    /// R-4: an agent job renders synchronously with `Seek`, outside every
    /// lock; its reply is sent exactly once, unless it was cancelled.
    /// `active`: a paused `FrameWait` suspended for it (review A F1).
    pub(crate) fn run_agent(&mut self, job: AgentJob, active: bool) {
        let AgentJob { work, cancel } = job;
        #[cfg(test)]
        if let Some(hook) = self.faults.on_agent.lock().expect("fault state").take() {
            hook();
        }
        match work {
            AgentWork::Thumbnail {
                document,
                lut,
                at,
                max_width,
                reply,
            } => {
                self.bind(None, &lut);
                let scale = RenderScale::Proxy { max_width };
                let resolution = scale.output_resolution(document.resolution);
                let result = self
                    .renderer
                    .render_thumbnail(&document, at, resolution, scale)
                    .map(|frame| RgbaImage {
                        width: frame.width,
                        height: frame.height,
                        pixels: (*frame.rgba).clone(),
                    });
                self.settle_titles();
                self.reply(&cancel, &reply, result);
            }
            AgentWork::CacheStats { clear, reply } => {
                let (rings, cleared) = {
                    let mut state = self.lane.lock();
                    let rings = state.readers.ring_bytes();
                    let now = self.lane.now();
                    let cleared = clear.then(|| state.readers.clear_cache(active, now));
                    if let Some((retired, _)) = &cleared {
                        // Under `Sched`, so no reader resets its flag (K-2).
                        self.stop(retired);
                        // Amendment R41 (U-1): only the plan's sizes stay.
                        self.sizes.retain(|key| state.readers.planned(key));
                    }
                    (rings, cleared)
                };
                if let Some((retired, posted)) = cleared {
                    self.close(&retired, posted);
                }
                let mut stats = if clear {
                    self.titles = None;
                    self.charged.clear();
                    self.renderer.clear()
                } else {
                    self.renderer.cache_stats()
                };
                let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
                stats.file_count = stats.file_count.saturating_add(count(rings.0));
                stats.bytes = stats.bytes.saturating_add(count(rings.1));
                self.reply(&cancel, &reply, Ok(stats));
            }
            #[cfg(any(test, feature = "test-util"))]
            AgentWork::Hold { started, release } => {
                let _ = started.send(());
                let _ = release.recv();
            }
        }
    }

    /// H-6 (4): a job still running at shutdown answers "media worker
    /// stopped"; a cancelled one answers nothing.
    fn reply<T>(
        &self,
        cancel: &AtomicBool,
        reply: &Sender<Result<T, MediaError>>,
        result: Result<T, MediaError>,
    ) {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let stopped = self.lane.lock().shutdown;
        let _ = reply.send(if stopped {
            Err(worker_stopped())
        } else {
            result
        });
    }
}

/// One frame's duration in milliseconds.
pub(crate) fn frame_ms(fps: Rational) -> f64 {
    let fps = f64::from(fps.numerator()) / f64::from(fps.denominator().max(1));
    if fps > 0.0 {
        1_000.0 / fps
    } else {
        1_000.0 / 30.0
    }
}

/// R-3: the lead in frames, `ceil(EWMA / frame)`, at least one, at most two.
#[allow(clippy::cast_possible_truncation)]
fn lead_frames(render_ewma_ms: f64, frame_ms: f64) -> i64 {
    ((render_ewma_ms / frame_ms).ceil() as i64).clamp(1, 2)
}

#[cfg(test)]
pub(crate) mod tests {
    use crossbeam_channel::bounded;
    use kinewright_core::Analysis;

    use super::*;
    use crate::{
        cc1_fixtures::fallback_gpu, compositor::live_table_frames, engine::FfmpegMediaEngine,
        perf_fixtures::title_card, test_support::TempDirectory,
    };

    /// A preview driven step by step on the test thread (no `run` loop).
    pub(crate) fn test_preview(clock: Arc<SharedClock>) -> (Preview, Receiver<PreviewFrame>) {
        test_preview_on(Arc::default(), clock)
    }

    pub(crate) fn test_preview_on(
        lane: Arc<Lane>,
        clock: Arc<SharedClock>,
    ) -> (Preview, Receiver<PreviewFrame>) {
        let (frames_tx, frames_rx) = bounded(2);
        let renderer = FrameRenderer::new_preview(fallback_gpu().context());
        let preview = Preview::new(lane, renderer, clock, (frames_tx, frames_rx.clone()));
        (preview, frames_rx)
    }

    pub(crate) fn job(document: &Arc<Document>, kind: JobKind, stamp: FrameStamp) -> TransportJob {
        let lut = Arc::default();
        let scene = Scene {
            document: Arc::clone(document),
            lut,
            generation: 1,
        };
        TransportJob { kind, stamp, scene }
    }

    type StatsReply = Receiver<Result<CacheStats, MediaError>>;

    fn stats_job() -> (AgentJob, StatsReply, Arc<AtomicBool>) {
        let (reply, response) = bounded(1);
        let cancel = Arc::<AtomicBool>::default();
        let work = AgentWork::CacheStats {
            clear: false,
            reply,
        };
        let job = AgentJob {
            work,
            cancel: Arc::clone(&cancel),
        };
        (job, response, cancel)
    }

    /// A `clear_preview_cache` job (review A F1/S2).
    fn clear_job() -> (AgentJob, StatsReply) {
        let (reply, response) = bounded(1);
        let work = AgentWork::CacheStats { clear: true, reply };
        let cancel = Arc::default();
        (AgentJob { work, cancel }, response)
    }

    /// Exactly one reply was sent and the sender is gone.
    fn one_reply<T>(response: &Receiver<Result<T, MediaError>>) -> Result<T, MediaError> {
        let reply = response.try_recv().expect("one reply");
        assert!(response.try_recv().is_err(), "a second reply");
        reply
    }

    fn agent_flag(work: Option<Work>) -> Arc<AtomicBool> {
        match work {
            Some(Work::Agent(job)) => job.cancel,
            _ => panic!("expected an agent job"),
        }
    }

    const fn stamp(epoch: u64, seq: u64) -> FrameStamp {
        FrameStamp { epoch, seq }
    }

    /// PF1 G-1/K-6 (review A F1): only the live preview monitor encodes
    /// through the table; agent thumbnails (`thumbnail_at`,
    /// `thumbnail_for_document`, the agent's frame tools) and
    /// `monitor_proof_for_document` keep the f32 encode.
    #[test]
    fn only_the_live_monitor_encodes_through_the_table() {
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        let document = Arc::new(title_card((64, 64), 3));
        let before = live_table_frames();
        for at in [0, 1] {
            preview.run_paused(
                &job(&document, JobKind::Paused(TimeCode(at)), stamp(1, 1)),
                0,
            );
        }
        assert_eq!(frames.try_iter().count(), 2);
        assert_eq!(live_table_frames(), before + 2, "two paused renders");
        let (reply, response) = bounded(1);
        preview.run_agent(
            AgentJob {
                work: AgentWork::Thumbnail {
                    document: Arc::clone(&document),
                    lut: Arc::default(),
                    at: TimeCode(1),
                    max_width: 64,
                    reply,
                },
                cancel: Arc::default(),
            },
            false,
        );
        let thumbnail = one_reply(&response).expect("a thumbnail");
        assert_eq!((thumbnail.width, thumbnail.height), (64, 64));
        let temp = TempDirectory::new("pf1-g1-routing");
        let gpu = fallback_gpu().context();
        let engine = FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu, temp.root().into()).unwrap();
        let proof = engine.monitor_proof_for_document(document, TimeCode(1));
        assert_eq!(proof.expect("a proof").image.width, 64);
        assert_eq!(live_table_frames(), before + 2, "thumbnail and proof: f32");
    }

    /// S-1/L-6: the paused slot keeps only the newest stamp; one render is in
    /// flight at a time, and the final target is the one rendered.
    #[test]
    fn the_paused_slot_keeps_the_newest_target_and_renders_the_last() {
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        preview.faults.fake_render.store(true, Ordering::Release);
        let lane = Arc::clone(&preview.lane);
        let document = Arc::new(title_card((64, 64), 30));
        for (seq, at) in (1..=5).zip([3, 6, 9, 12, 15]) {
            let at = JobKind::Paused(TimeCode(at));
            lane.post(Some(job(&document, at, stamp(1, seq))));
        }
        let in_flight = preview.next_work(false).expect("the newest job");
        assert!(lane.lock().transport.is_none(), "taken, not copied");
        // A drag posts behind the render in flight; it waits in the slot.
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(20)),
            stamp(1, 6),
        )));
        preview.execute(in_flight);
        let work = preview.next_work(false).expect("the drag's last target");
        preview.execute(work);
        assert!(preview.next_work(false).is_none(), "nothing else renders");
        let shown: Vec<_> = frames.try_iter().map(|f| (f.at.0, f.stamp.seq)).collect();
        assert_eq!(shown, [(15, 5), (20, 6)]);
    }

    /// Amendment R37 (start-up): a paused job issued before a `play` is
    /// superseded by it: dropped untaken, or, once taken, it posts no plan
    /// and publishes nothing; one the `play` did not precede still renders.
    #[test]
    fn a_play_supersedes_the_paused_job_issued_before_it() {
        let (document, _workload) = cut_document();
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        let lane = Arc::clone(&preview.lane);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(9)),
            stamp(1, 1),
        )));
        lane.issue_play(2);
        assert!(preview.next_work(false).is_none(), "dropped untaken");
        assert!(lane.lock().transport.is_none());
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(9)),
            stamp(2, 3),
        )));
        let taken = preview
            .next_work(false)
            .expect("the play's own resting frame");
        lane.issue_play(3);
        preview.execute(taken);
        assert!(
            frames.try_recv().is_err(),
            "a superseded job publishes nothing"
        );
        let state = lane.lock();
        assert_eq!(state.readers.version(), 0, "and posts no plan");
        assert!(state.readers.slots.is_empty(), "no reader started");
        drop(state);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(9)),
            stamp(3, 4),
        )));
        let work = preview.next_work(false).expect("issued with the play");
        preview.execute(work);
        assert_eq!(frames.try_recv().expect("rendered").at, TimeCode(9));
    }

    /// Amendment R37 (start-up): a `play` issued while a paused job waits
    /// for its readers withdraws that demand and stops the decodes, so no
    /// reader decodes the stale frame.
    #[test]
    fn a_play_withdraws_a_waiting_paused_demand() {
        let (document, _workload) = cut_document();
        let lane = Arc::new(Lane::default());
        let (gate, frames, thread) = gated_preview(&lane);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(0)),
            stamp(1, 1),
        )));
        // Frame 0: source 0 at 0 and 14 (two regions), source 1 at 7.
        wait_until(&lane, |state| state.readers.decoding().len() == 3);
        lane.issue_play(2);
        // The withdrawal notifies only the readers: poll for it.
        let deadline = Instant::now() + Duration::from_secs(60);
        let withdrawn = |state: &LaneState| {
            let slots = &state.readers.slots;
            slots.iter().all(|slot| slot.plan.required.is_empty())
        };
        while !withdrawn(&lane.lock()) {
            assert!(
                Instant::now() < deadline,
                "the play never withdrew the demand"
            );
            thread::yield_now();
        }
        drop(gate);
        wait_until(&lane, |state| state.readers.decoding().is_empty());
        assert_eq!(
            lane.cancelled.load(Ordering::Acquire),
            3,
            "every decode stopped"
        );
        let state = lane.lock();
        assert_eq!(state.readers.ring_bytes(), (0, 0), "no stale frame decoded");
        drop(state);
        assert!(frames.try_recv().is_err(), "nothing published");
        lane.shut_down();
        join_within(thread);
    }

    /// Amendment R37 (start-up): playback's first target is its start
    /// frame, not clock + lead; once the clock passed it, the clock's frame.
    #[test]
    fn playback_first_targets_its_start_frame() {
        for (clock_at, from, first) in [(0, 0, 0), (12, 12, 12), (5, 3, 5)] {
            let clock = Arc::new(SharedClock::new());
            let (mut preview, _frames) = test_preview(Arc::clone(&clock));
            preview.faults.fake_render.store(true, Ordering::Release);
            preview.faults.step_hold.store(true, Ordering::Release);
            preview.render_ewma_ms = 60.0; // a lead of two frames
            let lane = Arc::clone(&preview.lane);
            let document = Arc::new(title_card((64, 64), 1_000));
            clock.set_fps(document.fps);
            clock.set_frame(TimeCode(clock_at));
            let playback = JobKind::Playback {
                from: TimeCode(from),
            };
            lane.post(Some(job(&document, playback, stamp(2, 2))));
            let Some(Work::Playback(job, version)) = preview.next_work(false) else {
                panic!("the playback attempt");
            };
            assert_eq!(preview.run_playback(&job, version), Attempt::Published);
            assert_eq!(preview.published, Some((2, first)), "from {from}");
            // Later targets lead the clock again.
            assert_eq!(preview.run_playback(&job, version), Attempt::Pending);
            let held = preview.held.as_ref().expect("held").frame.at.0;
            assert_eq!(held, clock_at + 2, "from {from}");
        }
    }

    /// I13: FIFO, a bound of eight with an immediate prefixed refusal, and
    /// exactly one reply per job.
    #[test]
    fn the_agent_lane_is_fifo_bounded_and_replies_exactly_once() {
        let (mut preview, _frames) = test_preview(Arc::new(SharedClock::new()));
        let lane = Arc::clone(&preview.lane);
        let mut queued = Vec::new();
        for _ in 0..AGENT_QUEUE_LIMIT {
            let (job, response, flag) = stats_job();
            assert!(lane.try_push(job));
            queued.push((response, flag));
        }
        let (job, response, _flag) = stats_job();
        assert!(!lane.try_push(job), "the ninth job was refused");
        let refused = one_reply(&response).expect_err("refused");
        let MediaError::Backend(message) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(message, "preview-thread: agent queue full");
        for (response, flag) in &queued {
            let work = preview.next_work(false);
            let Some(Work::Agent(job)) = work else {
                panic!("expected an agent job");
            };
            assert!(Arc::ptr_eq(&job.cancel, flag), "FIFO order");
            preview.execute(Work::Agent(job));
            assert!(one_reply(response).is_ok());
        }
        assert!(preview.next_work(false).is_none());
    }

    /// I13: a dropped guard cancels under the lock; the job is discarded at
    /// dequeue and its reply sender dropped unanswered.
    #[test]
    fn a_cancelled_agent_job_is_discarded_unanswered() {
        let (mut preview, _frames) = test_preview(Arc::new(SharedClock::new()));
        let lane = Arc::clone(&preview.lane);
        let guard = CancelOnDrop::new(&lane);
        let (reply, cancelled) = bounded::<Result<CacheStats, MediaError>>(1);
        let work = AgentWork::CacheStats { clear: true, reply };
        assert!(lane.try_push(AgentJob {
            work,
            cancel: guard.flag()
        }));
        let (job, kept, flag) = stats_job();
        assert!(lane.try_push(job));
        drop(guard);
        assert!(Arc::ptr_eq(&agent_flag(preview.next_work(false)), &flag));
        assert!(matches!(
            cancelled.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ));
        drop(kept);
    }

    /// I13 fairness and bound: one agent job after each transport attempt;
    /// a playback hold ends at its deadline when an agent job waits; with
    /// no transport job, agent jobs run back to back.
    #[test]
    fn agent_jobs_take_turns_with_transport_attempts() {
        let clock = Arc::new(SharedClock::new());
        let (mut preview, frames) = test_preview(Arc::clone(&clock));
        preview.faults.fake_render.store(true, Ordering::Release);
        let lane = Arc::clone(&preview.lane);
        let document = Arc::new(title_card((64, 64), 100));
        clock.set_fps(document.fps);
        clock.set_frame(TimeCode(10));
        // R37: the first target is `from`, here one frame ahead of the clock.
        let playback = JobKind::Playback { from: TimeCode(11) };
        lane.post(Some(job(&document, playback, stamp(2, 2))));
        let flags: Vec<_> = (0..4)
            .map(|_| {
                let (job, _response, flag) = stats_job();
                assert!(lane.try_push(job));
                flag
            })
            .collect();
        for flag in &flags[..2] {
            let Some(Work::Playback(job, version)) = preview.next_work(false) else {
                panic!("a transport attempt comes first");
            };
            let attempt = preview.run_playback(&job, version);
            assert_eq!(attempt, Attempt::Dropped { agent: true }, "the deadline");
            preview.agent_turn = true;
            let work = preview.next_work(false);
            assert!(Arc::ptr_eq(&agent_flag(work), flag), "then one agent job");
            preview.agent_turn = false;
        }
        lane.post(None);
        for flag in &flags[2..] {
            assert!(Arc::ptr_eq(&agent_flag(preview.next_work(false)), flag));
        }
        assert!(
            frames.try_recv().is_err(),
            "a dropped frame is never published"
        );
    }

    /// Review B, re-review B D3: the due frames R-5 registers in the
    /// playback's epoch while an agent job renders are counted in
    /// `dropped_agent`, each once; the held frame the next attempt then
    /// finds expired is not counted again. A seek, a pause or a replay
    /// during the job charges only the frames the playback passed before
    /// it (the old count charged the clock's jump: 890 for the seek).
    #[test]
    fn an_agent_job_counts_the_due_frames_it_displaces() {
        type During = fn(&Lane, &SharedClock);
        let cases: [(&str, During, u64); 4] = [
            (
                "advance",
                |lane, clock| {
                    clock.set_frame(TimeCode(16));
                    lane.counters().sample(Instant::now(), Some(16));
                },
                6,
            ),
            (
                "seek",
                |lane, clock| {
                    lane.counters().sample(Instant::now(), Some(12));
                    clock.set_frame(TimeCode(900));
                    lane.counters().begin(Instant::now(), 900, 33.3, 1_000, 3);
                    lane.counters().sample(Instant::now(), Some(905));
                },
                2,
            ),
            (
                "pause",
                |lane, clock| {
                    clock.set_frame(TimeCode(12));
                    lane.counters().end(Instant::now(), Some(12));
                },
                2,
            ),
            (
                "replay",
                |lane, clock| {
                    lane.counters().sample(Instant::now(), Some(12));
                    lane.counters().clear([0; 4]);
                    clock.set_frame(TimeCode(0));
                    lane.counters().begin(Instant::now(), 0, 33.3, 1_000, 3);
                    lane.counters().sample(Instant::now(), Some(5));
                },
                0,
            ),
        ];
        for (case, during, charged) in cases {
            let clock = Arc::new(SharedClock::new());
            let (mut preview, _frames) = test_preview(Arc::clone(&clock));
            preview.faults.fake_render.store(true, Ordering::Release);
            preview.faults.step_hold.store(true, Ordering::Release);
            let lane = Arc::clone(&preview.lane);
            let document = Arc::new(title_card((64, 64), 1_000));
            clock.set_fps(document.fps);
            clock.set_frame(TimeCode(10));
            lane.counters().begin(Instant::now(), 10, 33.3, 1_000, 2);
            // R37: the first target is `from`, here one frame ahead of the clock.
            let playback = JobKind::Playback { from: TimeCode(11) };
            lane.post(Some(job(&document, playback, stamp(2, 2))));
            let Some(Work::Playback(job, version)) = preview.next_work(false) else {
                panic!("the playback attempt");
            };
            assert_eq!(preview.run_playback(&job, version), Attempt::Pending);
            let (agent, _response, _flag) = stats_job();
            assert!(lane.try_push(agent));
            let (moved, clocked) = (Arc::clone(&lane), Arc::clone(&clock));
            let hook = Box::new(move || during(&moved, &clocked));
            *preview.faults.on_agent.lock().unwrap() = Some(hook);
            preview.agent_turn = true;
            let work = preview.next_work(false).expect("the agent job");
            preview.execute(work);
            assert_eq!(lane.counters().stats.dropped_agent, charged, "{case}");
            if case == "advance" {
                let Some(Work::Playback(job, version)) = preview.next_work(false) else {
                    panic!("the next attempt");
                };
                assert_eq!(
                    preview.run_playback(&job, version),
                    Attempt::Dropped { agent: false }
                );
                assert_eq!(lane.counters().stats.dropped_agent, 6, "counted once");
            }
        }
    }

    /// Re-review 2 D4: a frame the preview published before an agent job,
    /// which the worker registers only while the job runs, is not charged
    /// to it; the frames past it are.
    #[test]
    fn an_agent_job_is_not_charged_a_frame_published_before_it() {
        let clock = Arc::new(SharedClock::new());
        let (mut preview, _frames) = test_preview(Arc::clone(&clock));
        preview.faults.fake_render.store(true, Ordering::Release);
        preview.faults.step_hold.store(true, Ordering::Release);
        let lane = Arc::clone(&preview.lane);
        let document = Arc::new(title_card((64, 64), 1_000));
        clock.set_fps(document.fps);
        clock.set_frame(TimeCode(10));
        lane.counters().begin(Instant::now(), 10, 33.3, 1_000, 2);
        // R37: the first target is `from`, here one frame ahead of the clock.
        let playback = JobKind::Playback { from: TimeCode(11) };
        lane.post(Some(job(&document, playback, stamp(2, 2))));
        let Some(Work::Playback(job, version)) = preview.next_work(false) else {
            panic!("the playback attempt");
        };
        assert_eq!(preview.run_playback(&job, version), Attempt::Pending);
        let published = preview.held.as_ref().expect("held").frame.at;
        assert!(published > TimeCode(10), "{published:?}");
        clock.set_frame(published);
        assert_eq!(preview.run_playback(&job, version), Attempt::Published);
        let (agent, _response, _flag) = stats_job();
        assert!(lane.try_push(agent));
        let moved = Arc::clone(&lane);
        let hook = Box::new(move || moved.counters().sample(Instant::now(), Some(14)));
        *preview.faults.on_agent.lock().unwrap() = Some(hook);
        preview.agent_turn = true;
        let work = preview.next_work(false).expect("the agent job");
        preview.execute(work);
        let charged = u64::try_from(14 - published.0).unwrap();
        assert_eq!(lane.counters().stats.dropped_agent, charged);
    }

    /// H-6 on one thread: queued agent jobs are answered "media worker
    /// stopped" exactly once, a job running at shutdown answers the same,
    /// the preview takes no further work and the lane refuses new jobs.
    #[test]
    fn shutdown_answers_every_agent_job_exactly_once() {
        let (mut preview, _frames) = test_preview(Arc::new(SharedClock::new()));
        let lane = Arc::clone(&preview.lane);
        let (running, running_reply, _) = stats_job();
        assert!(lane.try_push(running));
        let running = preview.next_work(false).expect("the running job");
        let queued: Vec<_> = (0..3)
            .map(|_| {
                let (job, response, _) = stats_job();
                assert!(lane.try_push(job));
                response
            })
            .collect();
        let document = Arc::new(title_card((64, 64), 3));
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(1)),
            stamp(1, 1),
        )));
        let (moved, transport) = lane.shut_down();
        assert!(transport.is_some());
        for job in moved {
            job.reply_error(worker_stopped());
        }
        for response in &queued {
            assert_eq!(one_reply(response), Err(worker_stopped()));
        }
        assert!(preview.next_work(true).is_none(), "no wait after shutdown");
        preview.execute(running);
        assert_eq!(one_reply(&running_reply), Err(worker_stopped()));
        let (job, response, _) = stats_job();
        assert!(!lane.try_push(job));
        assert_eq!(one_reply(&response), Err(worker_stopped()));
    }

    /// H-6 kill test on real threads: a preview thread parked in a playback
    /// hold on a frozen clock leaves it at shutdown and exits.
    #[test]
    fn the_preview_thread_leaves_a_playback_hold_at_shutdown() {
        let (lane, clock) = (Arc::<Lane>::default(), Arc::new(SharedClock::new()));
        let document = Arc::new(title_card((64, 64), 100));
        clock.set_fps(document.fps);
        let (holding, held) = bounded(1);
        let (done_tx, done) = bounded(1);
        let thread = {
            let (lane, clock) = (Arc::clone(&lane), Arc::clone(&clock));
            std::thread::spawn(move || {
                let (preview, _frames) = test_preview_on(lane, clock);
                preview.faults.fake_render.store(true, Ordering::Release);
                *preview.faults.on_hold.lock().unwrap() = Some(holding);
                preview.run();
                let _ = done_tx.send(());
            })
        };
        let playback = JobKind::Playback { from: TimeCode(0) };
        lane.post(Some(job(&document, playback, stamp(1, 1))));
        held.recv_timeout(Duration::from_secs(10))
            .expect("the hold began");
        drop(lane.shut_down());
        done.recv_timeout(Duration::from_secs(10))
            .expect("the preview left the hold");
        thread.join().unwrap();
    }

    /// A three-track, two-source cut document (sources alternate on top).
    fn cut_document() -> (Arc<Document>, crate::perf_fixtures::Workload) {
        let workload = crate::perf_fixtures::cuts((160, 90), 60, 3, 9, 10);
        (Arc::new(workload.0.clone()), workload)
    }

    /// Wait on `ready` (no sleep) until `what` holds; 60 s is a hang.
    pub(crate) fn wait_until(lane: &Lane, what: impl Fn(&LaneState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut state = lane.lock();
        while !what(&state) {
            let left = deadline.checked_duration_since(Instant::now());
            let left = left.expect("the lane never reached the condition");
            state = state.wait_timeout(&lane.ready, left);
        }
    }

    fn decoding(state: &LaneState) -> bool {
        let slots = &state.readers.slots;
        slots
            .iter()
            .any(|slot| matches!(slot.state, crate::sched::ReaderState::Decoding { .. }))
    }

    /// A preview thread on `lane`, its readers gated; returns the gate.
    fn gated_preview(
        lane: &Arc<Lane>,
    ) -> (Sender<()>, Receiver<PreviewFrame>, thread::JoinHandle<()>) {
        let (gate, gated) = bounded(0);
        *lane.gate.lock().expect("gate") = Some(gated);
        let (frames, thread) = threaded_preview(lane);
        (gate, frames, thread)
    }

    /// A preview thread on `lane`.
    fn threaded_preview(lane: &Arc<Lane>) -> (Receiver<PreviewFrame>, thread::JoinHandle<()>) {
        // The renderer is not `Send`: build the preview on its thread.
        let (lane, (handoff, frames)) = (Arc::clone(lane), bounded(1));
        let thread = thread::spawn(move || {
            let (preview, frames) = test_preview_on(lane, Arc::new(SharedClock::new()));
            handoff.send(frames).expect("frames");
            preview.run();
        });
        (frames.recv().expect("the preview started"), thread)
    }

    /// H-6: join within 60 s, or the test fails as a hang.
    fn join_within(thread: thread::JoinHandle<()>) {
        let (done, joined) = bounded(1);
        thread::spawn(move || done.send(thread.join().is_ok()));
        let joined = joined.recv_timeout(Duration::from_secs(60));
        assert_eq!(joined, Ok(true), "the preview did not exit cleanly");
    }

    /// C-5 (S2b-1): frames the readers decode composite to the synchronous
    /// renderer's bytes: consecutive (continuation), a jump and the cuts.
    #[test]
    fn scheduled_frames_match_the_synchronous_renderer() {
        let (document, _workload) = cut_document();
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        let mut reference = FrameRenderer::new_preview(fallback_gpu().context());
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        for at in [0, 1, 2, 25, 9, 10, 29, 3] {
            let job = job(&document, JobKind::Paused(TimeCode(at)), stamp(1, 1));
            preview.run_paused(&job, 0);
            assert!(preview.lane.take_failures().is_empty(), "frame {at}");
            let shown = frames.try_recv().expect("published");
            let expected = reference.render_live(
                &document,
                TimeCode(at),
                resolution,
                scale,
                DecodeStrategy::Seek,
            );
            assert_eq!(
                shown.texture.rgba,
                expected.expect("reference").rgba,
                "frame {at}"
            );
        }
        assert!(
            !preview.lane.lock().readers.slots.is_empty(),
            "readers decoded them"
        );
        let synchronous = preview.renderer.cache_stats();
        assert_eq!(
            synchronous.file_count, 0,
            "the synchronous renderer decoded nothing"
        );
    }

    /// Review B S3's sources: an encoded 30-frame 160×90 H.264 source with
    /// `filter`'s pixels, tagged by `vf` and in `pixel_format`.
    fn c5_source(
        filter: &str,
        vf: &str,
        pixel_format: &str,
        id: u64,
    ) -> (
        crate::test_support::GeneratedMedia,
        kinewright_core::MediaAsset,
    ) {
        let input = format!("{filter}=size=160x90:rate=30");
        let args = [
            "-f",
            "lavfi",
            "-i",
            &input,
            "-frames:v",
            "30",
            "-vf",
            vf,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-pix_fmt",
            pixel_format,
            "-g",
            "30",
        ];
        let media = crate::test_support::GeneratedMedia::ffmpeg("pf1-c5", &args, "mp4");
        let asset = crate::decode::probe_path(media.path(), kinewright_core::AssetId(id));
        (media, asset.expect("the C-5 source probes"))
    }

    /// Review B S3: a 160×90 document whose seven layers all show — three
    /// video sources on three accepted colour branches (Rec.709 limited
    /// 8-bit, full-range 8-bit, limited 10-bit; each under the D65
    /// assumption) and an sRGB-tagged PNG still (`SrgbFull`), a push and a
    /// crossfade, Screen / Multiply / Overlay, opacity and transforms, a
    /// managed `creative_look` and a legacy `cube_lut`, a solid, a title
    /// and an adjustment.
    #[allow(clippy::too_many_lines)]
    fn c5_document(
        directory: &TempDirectory,
    ) -> (
        Arc<Document>,
        Arc<LutLibrary>,
        Vec<crate::test_support::GeneratedMedia>,
    ) {
        use kinewright_core::{
            BlendMode, ClipContent, ColorBitDepth, ColorRange, ColorTransfer, LutAssetId,
            ParamValue, SolidColor, Title, TitlePosition, Track, TrackId, TrackKind,
        };

        use crate::mo2_fixtures::{clip, effect, with_transition};
        let tag = "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709";
        let limited = c5_source("testsrc2", &format!("{tag}:range=limited"), "yuv420p", 1);
        let full = c5_source("smptebars", &format!("{tag}:range=full"), "yuv420p", 2);
        let ten = c5_source(
            "gradients",
            &format!("{tag}:range=limited"),
            "yuv420p10le",
            3,
        );
        let still_path = directory.path("srgb.png");
        crate::test_support::run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=80x45:rate=1:duration=1",
                "-frames:v",
                "1",
                "-vf",
                "setparams=color_primaries=bt709:color_trc=iec61966-2-1",
                "-c:v",
                "png",
            ],
            &still_path,
        );
        let still = crate::decode::probe_path(&still_path, kinewright_core::AssetId(4));
        let still = still.expect("the still probes");
        let descriptions = [&limited.1, &full.1, &ten.1, &still].map(|a| &a.color_description);
        assert_eq!(
            descriptions[0].range,
            ColorRange::Limited,
            "{descriptions:?}"
        );
        assert_eq!(descriptions[1].range, ColorRange::Full, "{descriptions:?}");
        assert_eq!(
            descriptions[2].bit_depth,
            ColorBitDepth::Ten,
            "{descriptions:?}"
        );
        assert_eq!(
            descriptions[3].transfer,
            ColorTransfer::Srgb,
            "{descriptions:?}"
        );
        for description in descriptions {
            let assumption = crate::render::d65_assumption(description);
            let profile = kinewright_core::classify_source_with_assumption(description, assumption);
            assert!(
                profile.is_ok(),
                "an accepted branch: {description:?} {profile:?}"
            );
        }

        // A managed LUT asset, and a legacy `.cube` on disk.
        let store = crate::lut_store::LutStore::for_project(&directory.path("c5.kinewright"))
            .expect("the store");
        let cube = |scale: [f64; 3]| {
            let rows = (0..8).map(|corner: u8| {
                let [r, g, b] = [corner & 1, corner >> 1 & 1, corner >> 2 & 1].map(f64::from);
                let rgb = [r * scale[0], g * scale[1] + 0.1, b * scale[2]];
                format!("{:.6} {:.6} {:.6}\n", rgb[0], rgb[1], rgb[2])
            });
            std::iter::once("LUT_3D_SIZE 2\n".to_owned())
                .chain(rows)
                .collect::<String>()
        };
        let look_path = directory.path("look.cube");
        std::fs::write(&look_path, cube([0.7, 0.8, 1.0])).expect("the look");
        let import = store
            .import_lut_asset(&look_path)
            .expect("the look imports");
        let lut_asset = import.into_lut_asset(LutAssetId(1));
        let (library, _) = LutLibrary::build(std::slice::from_ref(&lut_asset), Some(&store));
        assert_eq!(library.len(), 1, "the managed LUT is verified");
        let legacy_path = directory.path("legacy.cube");
        std::fs::write(&legacy_path, cube([1.0, 0.6, 0.8])).expect("the legacy LUT");

        let linear = crate::color_pipeline::LutInputEncoding::Linear.token();
        let look = effect(
            1,
            "creative_look",
            &[
                ("lut_asset_id", 1),
                ("mix_basis_points", 10_000),
                ("input_encoding_token", linear),
            ],
        );
        let mut legacy = effect(2, "cube_lut", &[("intensity_percent", 100)]);
        let legacy_text = ParamValue::Text(legacy_path.to_string_lossy().into_owned());
        legacy.parameters.insert("path".to_owned(), legacy_text);
        let pip = effect(3, "transform", &[("scale_percent", 50), ("x_percent", 25)]);
        let corner = [("scale_percent", 30), ("x_percent", -30), ("y_percent", 25)];
        let span = |mut clip: kinewright_core::Clip, asset, source, len, start| {
            (clip.asset, clip.timeline_start) = (asset, TimeCode(start));
            clip.source_range = TimeCode(source)..TimeCode(source + len);
            clip
        };
        let media = |id, blend, effects| clip(id, ClipContent::Media, blend, effects);
        let base = span(
            media(1, BlendMode::Normal, vec![look]),
            limited.1.id,
            2,
            24,
            0,
        );
        let pushed = span(media(2, BlendMode::Screen, vec![pip]), full.1.id, 0, 20, 4);
        let opacity = effect(4, "opacity", &[("percent", 70)]);
        let multiplied = media(3, BlendMode::Multiply, vec![opacity, legacy]);
        let multiplied = span(multiplied, ten.1.id, 5, 24, 0);
        let freeze = ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode::ZERO,
        });
        let pinned = clip(
            4,
            freeze,
            BlendMode::Normal,
            vec![effect(5, "transform", &corner)],
        );
        let pinned = span(pinned, still.id, 0, 24, 0);
        let solid = ClipContent::Solid(SolidColor {
            r: 0xF0,
            g: 0x90,
            b: 0x30,
        });
        let solid = clip(
            5,
            solid,
            BlendMode::Overlay,
            vec![effect(6, "opacity", &[("percent", 50)])],
        );
        let solid = span(solid, kinewright_core::AssetId::default(), 0, 24, 0);
        let title = ClipContent::Title(Title {
            text: "C-5".to_owned(),
            position: TitlePosition::Center,
            ..Title::default()
        });
        let title = span(
            clip(6, title, BlendMode::Normal, Vec::new()),
            kinewright_core::AssetId::default(),
            0,
            22,
            2,
        );
        let saturation = effect(7, "primary_correction", &[("saturation_percent", 40)]);
        let adjustment = clip(
            7,
            ClipContent::Adjustment,
            BlendMode::Normal,
            vec![saturation],
        );
        let adjustment = span(adjustment, kinewright_core::AssetId::default(), 0, 24, 0);
        let clips = [
            base,
            with_transition(pushed, "push_left", 8),
            multiplied,
            pinned,
            solid,
            with_transition(title, "crossfade", 6),
            adjustment,
        ];
        let document = Document {
            resolution: (160, 90),
            duration: TimeCode(24),
            tracks: (1..)
                .zip(clips)
                .map(|(id, clip)| Track {
                    id: TrackId(id),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip],
                })
                .collect(),
            media_pool: vec![limited.1, full.1, ten.1, still],
            lut_assets: vec![lut_asset],
            ..Document::default()
        };
        document.validate().expect("the C-5 document is valid");
        (
            Arc::new(document),
            Arc::new(library),
            vec![limited.0, full.0, ten.0],
        )
    }

    /// Review B S3 / C-5: over visible multilayer frames — every layer
    /// changes the image (removing any one of the seven changes the bytes)
    /// — frames the readers supply composite to the synchronous renderer's
    /// bytes, paused and at playback horizon, inside and after both
    /// transitions and on a jump back; no K-3 fallback, and the preview's
    /// synchronous renderer decodes nothing.
    #[test]
    fn scheduled_multilayer_frames_match_the_synchronous_renderer() {
        let directory = TempDirectory::new("pf1-c5");
        let (document, lut, _media) = c5_document(&directory);
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        let mut reference = FrameRenderer::new_preview(fallback_gpu().context());
        reference.set_lut_library(Arc::clone(&lut));
        let mut synchronous = |document: &Document, at: i64| {
            let strategy = DecodeStrategy::Seek;
            let frame = reference.render_live(document, TimeCode(at), resolution, scale, strategy);
            frame.expect("the reference").rgba.to_vec()
        };
        // Visible: at 8 the push is half way and the crossfade complete.
        let full = synchronous(&document, 8);
        for track in 0..document.tracks.len() {
            let mut without = (*document).clone();
            without.tracks.remove(track);
            assert_ne!(synchronous(&without, 8), full, "track {track} is hidden");
            // Every effect shows too (both LUTs, opacity, transforms, grade).
            for index in 0..document.tracks[track].clips[0].effects.len() {
                let mut without = (*document).clone();
                without.tracks[track].clips[0].effects.remove(index);
                let shown = synchronous(&without, 8);
                assert_ne!(shown, full, "track {track}'s effect {index} is inert");
            }
        }
        // And both transitions, mid-way (5: the push; 4: the crossfade).
        for (track, at) in [(1, 5), (5, 4)] {
            let mut cut = (*document).clone();
            cut.tracks[track].clips[0].transition_in = None;
            let shown = synchronous(&cut, at);
            assert_ne!(
                shown,
                synchronous(&document, at),
                "track {track}'s transition"
            );
        }
        let lane = Arc::new(Lane::with_budget(20, FRAME_CACHE_BYTE_BUDGET));
        let (mut preview, frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        let scene = |at| {
            let mut job = job(&document, JobKind::Paused(TimeCode(at)), stamp(1, 1));
            job.scene.lut = Arc::clone(&lut);
            job
        };
        for at in [0, 4, 5, 8, 12, 23, 3] {
            preview.run_paused(&scene(at), 0);
            assert!(preview.lane.take_failures().is_empty(), "frame {at}");
            let shown = frames.try_recv().expect("published");
            assert_eq!(
                *shown.texture.rgba,
                synchronous(&document, at),
                "paused {at}"
            );
        }
        for at in [6, 7, 9, 10, 16] {
            let job = scene(at);
            let wait = FrameWait {
                version: lane.lock().version,
                playback: Some(Instant::now() + Duration::from_secs(60)),
                paused: None,
            };
            let shown = preview.render_monitor(&job.scene, TimeCode(at), &wait);
            let Ok(Some(shown)) = shown else {
                panic!("frame {at} did not render");
            };
            assert_eq!(*shown.rgba, synchronous(&document, at), "playback {at}");
        }
        assert_eq!(lane.lock().fallbacks, [0, 0], "no K-3 fallback");
        assert!(
            !lane.lock().readers.slots.is_empty(),
            "readers decoded them"
        );
        let decoded = preview.renderer.has_sources();
        assert!(!decoded, "the synchronous renderer opened no decoder");
    }

    /// H-2 (S2b-1): an idle reader retires at the injected quiescence
    /// deadline, closing its decoder.
    #[test]
    fn idle_readers_retire_at_the_quiescence_deadline() {
        let (document, _workload) = cut_document();
        let (mut preview, _frames) = test_preview(Arc::new(SharedClock::new()));
        preview.run_paused(
            &job(&document, JobKind::Paused(TimeCode(0)), stamp(1, 1)),
            0,
        );
        let lane = Arc::clone(&preview.lane);
        // R37: source 0's two layers (0 and 14) are two regions.
        assert_eq!(lane.lock().readers.slots.len(), 3, "one reader per region");
        *lane.skew.lock().expect("skew") = crate::sched::QUIESCENCE;
        lane.work.notify_all();
        wait_until(&lane, |state| state.readers.slots.is_empty());
        let rings = lane.lock().readers.ring_bytes();
        // Source 0 at 0 and 14 (two layers), source 1 at 7.
        assert_eq!(rings.0, 3, "retirement keeps the frames");
    }

    /// Review A F3 (M58), on the real condvar: at P = 3, A holds every
    /// permit; B then C queue for one each. A exits and notifies; C wakes
    /// first, sees B at the head and sleeps again; only then does B wake and
    /// take one. B's grant must wake C: two permits are free, and nothing
    /// else notifies `Permits`.
    #[test]
    fn a_grant_wakes_the_next_ticket() {
        let lane = Arc::new(Lane::with_parallelism(3));
        lane.permits_change(|book| {
            book.enqueue(0, 3);
            assert_eq!(book.poll(0), Poll::Granted(3));
        });
        let polls = |lane: &Lane, id| lane.book().polls.get(&id).copied().unwrap_or(0);
        let until = |what: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !what() {
                assert!(Instant::now() < deadline, "the barrier was never reached");
                thread::yield_now();
            }
        };
        let weak = Arc::downgrade(&lane);
        let barrier: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |id| {
            // B waits, unlocked, until C has woken and polled again.
            if let Some(lane) = weak.upgrade().filter(|_| id == 1) {
                until(&|| polls(&lane, 2) >= 2);
            }
        });
        *lane.woke.lock().expect("woke") = Some(barrier);
        let acquire = |id| {
            let (lane, (granted, grant)) = (Arc::clone(&lane), bounded(1));
            thread::spawn(move || granted.send(lane.acquire(id, 1)));
            grant
        };
        let b = acquire(1);
        until(&|| polls(&lane, 1) >= 1);
        let c = acquire(2);
        until(&|| polls(&lane, 2) >= 1);
        lane.permits_change(|book| book.forget(0));
        assert_eq!(b.recv_timeout(Duration::from_secs(60)), Ok(Some(1)));
        let granted = c.recv_timeout(Duration::from_secs(10));
        lane.permits_change(|book| book.shutdown = true);
        assert_eq!(granted, Ok(Some(1)), "C slept through B's grant (M58)");
        assert!(polls(&lane, 2) >= 3, "C woke first, then at B's grant");
    }

    /// Review A F1 (idle): a cache clear after a paused frame completed
    /// invalidates its demand, so the readers are owed nothing and retire
    /// at the injected quiescence deadline, releasing their permits.
    #[test]
    fn a_cleared_idle_reader_still_retires() {
        let (document, _workload) = cut_document();
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        let paused = job(&document, JobKind::Paused(TimeCode(0)), stamp(1, 1));
        preview.run_paused(&paused, 0);
        assert!(frames.try_recv().is_ok(), "the paused frame");
        let lane = Arc::clone(&preview.lane);
        assert_eq!(lane.lock().readers.slots.len(), 3);
        let (clear, response) = clear_job();
        assert!(lane.try_push(clear));
        let work = preview.next_work(false).expect("the clear");
        preview.execute(work);
        assert!(one_reply(&response).is_ok());
        assert_eq!(lane.lock().readers.ring_bytes(), (0, 0), "cleared");
        *lane.skew.lock().expect("skew") = crate::sched::QUIESCENCE;
        lane.work.notify_all();
        wait_until(&lane, |state| state.readers.slots.is_empty());
        assert_eq!(lane.permits_in_use(), 0, "no permits kept");
        assert_eq!(lane.lock().readers.live().0, 0, "no bytes kept");
    }

    /// Review A F1 (hang): a paused wait for sources 0 and 1; source 0's
    /// frames arrive and its readers retire; a cache clear suspends the
    /// wait. The active job's frames stay, so once source 1's decode lands
    /// the frame publishes (clearing them left no reader to re-decode it).
    #[test]
    fn a_clear_during_a_paused_wait_keeps_its_frames() {
        let (document, _workload) = cut_document();
        let lane = Arc::new(Lane::default());
        let (release, held) = bounded::<()>(0);
        // Frame 0: source 0 at 0 and 14, source 1 at 7 (held).
        *lane.hold_at.lock().expect("hold") = Some((7, held));
        let (gate, frames, thread) = gated_preview(&lane);
        drop(gate);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(0)),
            stamp(1, 1),
        )));
        wait_until(&lane, |state| state.readers.ring_bytes().0 == 2);
        *lane.skew.lock().expect("skew") = crate::sched::QUIESCENCE;
        lane.work.notify_all();
        wait_until(&lane, |state| state.readers.slots.len() == 1);
        let (clear, response) = clear_job();
        assert!(lane.try_push(clear));
        let cleared = response.recv_timeout(Duration::from_secs(60));
        assert!(cleared.expect("the clear ran").is_ok());
        assert_eq!(
            lane.lock().readers.ring_bytes().0,
            2,
            "the wait's frames stay"
        );
        drop(release);
        let shown = frames.recv_timeout(Duration::from_secs(60));
        assert_eq!(shown.expect("the paused frame").at, TimeCode(0));
        lane.shut_down();
        join_within(thread);
    }

    /// R38 (review B F3): the reader-limit merge is counted end to end. At
    /// P = 2, cut frame 0 needs source 0 at 0 and 14 and source 1 at 7; the
    /// frame renders the synchronous bytes with one merge in the engine's
    /// `regions_merged`, and at P = 3 with none.
    #[test]
    fn a_reader_limit_merge_is_counted() {
        let (document, _workload) = cut_document();
        let expected = reference(&document, 0).0;
        for (parallelism, merges) in [(2, 1), (3, 0)] {
            let lane = Arc::new(Lane::with_parallelism(parallelism));
            let (mut preview, _frames) =
                test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
            let shown = render_paused(&mut preview, &document, 0);
            let Ok(Some(shown)) = shown else {
                panic!("P = {parallelism}: frame 0 did not render");
            };
            assert_eq!(*shown.rgba, expected, "C-5 at P = {parallelism}");
            assert_eq!(lane.lock().fallbacks, [0, 0], "no K-3 fallback");
            let merged = lane.counters().stats.regions_merged;
            assert_eq!(merged, merges, "P = {parallelism}: merges counted");
        }
    }

    /// Amendment R41: fallback regions' merges, folds and rewinds reach the
    /// engine's stats. Played ahead for 8 frames at P = 2, the cut
    /// document's two same-source playheads share a reader, which rewinds as
    /// they advance (at most once per job per fallback region, each counted
    /// once or more); at P = 3 nothing merges and nothing rewinds, though
    /// the cuts' pre-roll folds past H-1's two readers per source.
    #[test]
    fn merged_rewinds_reach_the_engine_stats() {
        let (document, _workload) = cut_document();
        for parallelism in [2, 3] {
            let lane = Arc::new(Lane::with_parallelism(parallelism));
            let (mut preview, _frames) =
                test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
            for at in 0..8 {
                render_ahead(&mut preview, &document, at);
            }
            let stats = lane.counters().stats;
            let (merged, folded) = (stats.regions_merged, stats.regions_folded);
            let rewinds = stats.merged_rewinds;
            let counts =
                format!("P = {parallelism}: {merged} merged, {folded} folded, {rewinds} rewinds");
            assert!(folded > 0, "{counts}: the pre-roll folds are counted");
            if parallelism == 2 {
                assert!(rewinds > 0, "{counts}: rewinds counted");
                assert!(
                    rewinds <= merged + folded,
                    "{counts}: ≤ 1 per fallback region per job"
                );
            } else {
                assert_eq!((merged, rewinds), (0, 0), "{counts}: no merge, no rewind");
            }
        }
    }

    /// R38 D2: two documents whose managed look (the same `LutAssetId`)
    /// resolves to different lattices. A's paused wait suspends for an agent
    /// thumbnail of B, which binds B's library; A's frame is still A's
    /// synchronous render, not B's.
    #[test]
    fn a_suspended_wait_composites_with_its_own_lut() {
        use kinewright_core::LutAssetId;
        let (document, _workload) = cut_document();
        let directory = TempDirectory::new("pf1-r38-d2");
        let store = crate::lut_store::LutStore::for_project(&directory.path("d2.kinewright"))
            .expect("the store");
        let look = |name: &str, scale: [f64; 3]| {
            let rows = (0..8).map(|corner: u8| {
                let [r, g, b] = [corner & 1, corner >> 1 & 1, corner >> 2 & 1].map(f64::from);
                format!(
                    "{:.6} {:.6} {:.6}\n",
                    r * scale[0],
                    g * scale[1],
                    b * scale[2]
                )
            });
            let text: String = std::iter::once("LUT_3D_SIZE 2\n".to_owned())
                .chain(rows)
                .collect();
            let path = directory.path(name);
            std::fs::write(&path, text).expect("the look");
            let import = store.import_lut_asset(&path).expect("the look imports");
            let asset = import.into_lut_asset(LutAssetId(1));
            let (library, _) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
            assert_eq!(library.len(), 1, "the managed LUT is verified");
            let mut looked = (*document).clone();
            let linear = crate::color_pipeline::LutInputEncoding::Linear.token();
            let effect = crate::mo2_fixtures::effect(
                1,
                "creative_look",
                &[
                    ("lut_asset_id", 1),
                    ("mix_basis_points", 10_000),
                    ("input_encoding_token", linear),
                ],
            );
            for clip in looked.tracks.iter_mut().flat_map(|track| &mut track.clips) {
                clip.effects.push(effect.clone());
            }
            looked.lut_assets = vec![asset];
            looked.validate().expect("the looked document is valid");
            (Arc::new(looked), Arc::new(library))
        };
        let (a, a_lut) = look("a.cube", [0.6, 0.9, 1.0]);
        let (b, b_lut) = look("b.cube", [1.0, 0.5, 0.7]);
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(a.resolution),
        };
        let resolution = scale.output_resolution(a.resolution);
        let reference = |library: &Arc<LutLibrary>| {
            let mut renderer = FrameRenderer::new_preview(fallback_gpu().context());
            renderer.set_lut_library(Arc::clone(library));
            let frame =
                renderer.render_live(&a, TimeCode(0), resolution, scale, DecodeStrategy::Seek);
            frame.expect("the reference").rgba.to_vec()
        };
        let expected = reference(&a_lut);
        assert_ne!(expected, reference(&b_lut), "not vacuous: the looks differ");

        let lane = Arc::new(Lane::default());
        let (release, held) = bounded::<()>(0);
        // Frame 0: source 0 at 0 and 14, source 1 at 7 (held).
        *lane.hold_at.lock().expect("hold") = Some((7, held));
        let (gate, frames, thread) = gated_preview(&lane);
        drop(gate);
        let mut paused = job(&a, JobKind::Paused(TimeCode(0)), stamp(1, 1));
        paused.scene.lut = Arc::clone(&a_lut);
        lane.post(Some(paused));
        wait_until(&lane, |state| state.readers.ring_bytes().0 == 2);
        let (reply, response) = bounded(1);
        let thumbnail = AgentJob {
            work: AgentWork::Thumbnail {
                document: Arc::clone(&b),
                lut: Arc::clone(&b_lut),
                at: TimeCode(0),
                max_width: 64,
                reply,
            },
            cancel: Arc::default(),
        };
        assert!(lane.try_push(thumbnail));
        let thumbnail = response.recv_timeout(Duration::from_secs(60));
        assert!(thumbnail.expect("the thumbnail ran").is_ok());
        assert_eq!(lane.lock().readers.ring_bytes().0, 2, "A is still waiting");
        drop(release);
        let shown = frames.recv_timeout(Duration::from_secs(60));
        let shown = shown.expect("A's paused frame");
        assert_eq!(shown.at, TimeCode(0));
        assert!(
            *shown.texture.rgba == expected,
            "A composited with A's look"
        );
        lane.shut_down();
        join_within(thread);
    }

    /// G13 / I11 (S2b-2): four active sources at P = 20 hold 5 × 4 frame
    /// threads, never more than P; a synchronous decoder (a thumbnail) is
    /// outside the pool and completes while every permit is held.
    #[test]
    fn four_sources_share_the_permit_pool() {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let document = Arc::new(workload.0.clone());
        let lane = Arc::new(Lane::with_parallelism(20));
        let (gate, frames, thread) = gated_preview(&lane);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(3)),
            stamp(1, 1),
        )));
        let four = |state: &LaneState| {
            let slots = &state.readers.slots;
            let decoding = slots
                .iter()
                .filter(|slot| matches!(slot.state, crate::sched::ReaderState::Decoding { .. }));
            decoding.count() == 4
        };
        wait_until(&lane, four);
        let threads: Vec<usize> = (lane.lock().readers.slots.iter())
            .map(|slot| slot.threads)
            .collect();
        assert_eq!(threads, [5, 5, 5, 5], "w = ⌊20 / 4⌋");
        assert_eq!(lane.permits_in_use(), 20, "G13: reader threads ≤ P");
        let (_, asset) = &workload.1[0];
        let thumbnail = crate::decode::thumbnail(&asset.path, asset.fps, TimeCode(3), 64);
        assert!(thumbnail.is_ok(), "a synchronous decoder waited on permits");
        drop(gate);
        let shown = frames
            .recv_timeout(Duration::from_secs(60))
            .expect("published");
        assert_eq!(shown.at, TimeCode(3));
        assert!(lane.take_failures().is_empty());
        lane.shut_down();
        join_within(thread);
        assert_eq!(
            lane.permits_in_use(),
            0,
            "the readers released every permit"
        );
    }

    /// H-5 / H-6 (S2b-2): a reader holding 16 of 20 permits while the plan
    /// widens to four sources leaves a newcomer short and others queued;
    /// shutdown wakes the queued readers, which exit while it still decodes.
    #[test]
    fn shutdown_wakes_the_readers_waiting_for_permits() {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let four = Arc::new(workload.0.clone());
        let mut one = workload.0.clone();
        one.tracks.truncate(1);
        let lane = Arc::new(Lane::with_parallelism(20));
        let (gate, _frames, thread) = gated_preview(&lane);
        lane.post(Some(job(
            &Arc::new(one),
            JobKind::Paused(TimeCode(3)),
            stamp(1, 1),
        )));
        wait_until(&lane, decoding);
        assert_eq!(lane.permits_in_use(), 16);
        lane.post(Some(job(&four, JobKind::Paused(TimeCode(3)), stamp(2, 2))));
        let queued = |state: &LaneState| -> Vec<u64> {
            let slots = state.readers.slots.iter();
            let waiting = slots.filter(|slot| slot.state == crate::sched::ReaderState::PermitWait);
            waiting.map(|slot| slot.id).collect()
        };
        // The newcomer's grant is in the book before its slot records it.
        let granted = |state: &LaneState| state.readers.slots.iter().any(|slot| slot.threads == 4);
        wait_until(&lane, |state| queued(state).len() == 2 && granted(state));
        let waiters = queued(&lane.lock());
        assert_eq!(lane.permits_in_use(), 20, "the newcomer is granted 4");
        lane.shut_down();
        // The queued readers exit while the two granted ones still decode
        // (gated): no permit was freed.
        wait_until(&lane, |state| {
            let slots = &state.readers.slots;
            !slots.iter().any(|slot| waiters.contains(&slot.id))
        });
        drop(gate);
        join_within(thread);
        assert!(lane.lock().readers.slots.is_empty());
        assert_eq!(lane.permits_in_use(), 0);
    }

    /// H-5 (S2b-2): on real threads, one source reads on 16 threads; when
    /// the plan widens to two, the wide reader closes (shrink) and the
    /// short newcomer closes (close-before-growth); fresh work reopens both
    /// on 10 + 10.
    #[test]
    fn a_widening_plan_rebalances_the_permits() {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let (mut one, mut two) = (workload.0.clone(), workload.0.clone());
        one.tracks.truncate(1);
        two.tracks.truncate(2);
        let (one, two) = (Arc::new(one), Arc::new(two));
        let lane = Arc::new(Lane::with_parallelism(20));
        let (gate, frames, thread) = gated_preview(&lane);
        *lane.gate.lock().expect("gate") = None;
        drop(gate);
        let threads = |lane: &Lane| -> Vec<usize> {
            let mut threads: Vec<usize> = (lane.lock().readers.slots.iter())
                .map(|slot| slot.threads)
                .collect();
            threads.sort_unstable();
            threads
        };
        let show = |document: &Arc<Document>, at: i64, seq: u64| {
            let paused = JobKind::Paused(TimeCode(at));
            lane.post(Some(job(document, paused, stamp(1, seq))));
            let shown = frames.recv_timeout(Duration::from_secs(60));
            assert_eq!(shown.expect("published").at, TimeCode(at));
        };
        show(&one, 3, 1);
        assert_eq!((threads(&lane), lane.permits_in_use()), (vec![16], 16));
        show(&two, 3, 2);
        wait_until(&lane, |state| {
            let slots = &state.readers.slots;
            slots.len() == 2 && slots.iter().all(|slot| slot.threads == 0)
        });
        assert_eq!(lane.permits_in_use(), 0, "both closed and released");
        show(&two, 4, 3);
        assert_eq!((threads(&lane), lane.permits_in_use()), (vec![10, 10], 20));
        assert!(lane.take_failures().is_empty());
        lane.shut_down();
        join_within(thread);
    }

    /// A playback-horizon render of `at` on the test thread (lookahead on).
    fn render_ahead(preview: &mut Preview, document: &Arc<Document>, at: i64) -> FrameTexture {
        let scene = job(document, JobKind::Paused(TimeCode(at)), stamp(1, 1)).scene;
        let wait = FrameWait {
            version: preview.lane.lock().version,
            playback: Some(Instant::now() + Duration::from_secs(60)),
            paused: None,
        };
        let rendered = preview.render_monitor(&scene, TimeCode(at), &wait);
        let Ok(Some(frame)) = rendered else {
            panic!("frame {at} did not render");
        };
        frame
    }

    /// The synchronous renderer's bytes for `at` (C-5's reference), and f.
    fn reference(document: &Document, at: i64) -> (Vec<u8>, usize) {
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        let demand = reader_demand(
            document,
            TimeCode(at),
            resolution,
            scale,
            0,
            &mut FrameSizes::default(),
        )
        .expect("the demand");
        assert_eq!(demand.generated, 0, "no generated rasters here");
        let f = demand
            .sources
            .values()
            .map(|(spec, _)| spec.frame_bytes)
            .max();
        let mut renderer = FrameRenderer::new_preview(fallback_gpu().context());
        let frame = renderer.render_live(
            document,
            TimeCode(at),
            resolution,
            scale,
            DecodeStrategy::Seek,
        );
        (
            frame.expect("reference").rgba.to_vec(),
            f.expect("a source"),
        )
    }

    /// I12 / C-5 (S2b-3): with C a few frames, playback-horizon renders
    /// over cuts keep live bytes ≤ C, render the synchronous bytes, and every
    /// reservation returns when the preview goes.
    #[test]
    fn scheduler_live_bytes_stay_within_c() {
        let (document, _workload) = cut_document();
        let f = reference(&document, 0).1;
        // C − H = 5 f: one required set (3 f) plus about two lookahead.
        let budget = 8 * f;
        let lane = Arc::new(Lane::with_budget(20, budget));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        for at in [0, 1, 2, 3, 8, 9, 10, 11, 25, 26, 4] {
            let shown = render_ahead(&mut preview, &document, at);
            assert_eq!(*shown.rgba, reference(&document, at).0, "C-5: frame {at}");
            let (live, peak) = lane.lock().readers.live();
            assert!(live <= budget && peak <= budget, "I12: {peak} > {budget}");
        }
        assert_eq!(lane.lock().fallbacks, [0, 0], "no K-3 fallback");
        // Not vacuous: lookahead runs past the required set, within C.
        wait_until(&lane, |state| state.readers.live().0 > 3 * f);
        assert!(lane.lock().readers.live().1 <= budget, "I12");
        drop(preview);
        assert_eq!(lane.lock().readers.live().0, 0, "K-1: a reservation leaked");
    }

    /// K-2 / K-5 (S2b-3): a job whose required set does not fit beside the
    /// last job's lookahead drains: the farthest unpinned lookahead goes,
    /// no new lookahead starts, and the set is admitted within C.
    #[test]
    fn admission_drains_lookahead_to_fit_the_required_set() {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let (mut one, mut three) = (workload.0.clone(), workload.0.clone());
        one.tracks.truncate(1);
        three.tracks.truncate(3);
        let (one, three) = (Arc::new(one), Arc::new(three));
        let f = reference(&one, 3).1;
        // C − H = 2.5 f: one lookahead frame fits beside source 0's frame.
        let budget = 7 * f / 2;
        let lane = Arc::new(Lane::with_budget(20, budget));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        render_ahead(&mut preview, &one, 3);
        let key = lane.lock().readers.slots[0].key.clone();
        let times = |state: &LaneState| -> Vec<i64> { state.readers.ring_times(&key) };
        wait_until(&lane, |state| times(state) == [3, 4] && !decoding(state));
        let shown = render_ahead(&mut preview, &three, 3);
        assert_eq!(*shown.rgba, reference(&three, 3).0, "C-5");
        let state = lane.lock();
        assert_eq!(times(&state), [3], "the lookahead was evicted for the set");
        assert!(state.readers.live().1 <= budget, "I12");
        drop(state);
        let starved = lane.counters().stats.lookahead_starved;
        assert!(starved > 0, "lookahead_starved counts the refusals");
    }

    /// K-2 / K-1 (S2b-3): a drain stops the lookahead decode in flight, and
    /// a job whose frame is resolved still waits for its titles' G: once
    /// admitted, live bytes are exactly the ring and the title rasters.
    /// The stop then lapses: the reader decodes the next job's frame
    /// (S2b-4; a watchdog fails a livelock).
    #[test]
    fn a_drain_stops_lookahead_and_holds_the_render_for_its_titles() {
        let (done, finished) = bounded(1);
        thread::spawn(move || {
            drain_then_decode();
            let _ = done.send(());
        });
        let watchdog = finished.recv_timeout(Duration::from_secs(120));
        assert_eq!(watchdog, Ok(()), "the drain run hung or failed");
    }

    fn drain_then_decode() {
        let workload = crate::perf_fixtures::titled((160, 90), 30, 2);
        let mut one = workload.0.clone();
        one.tracks.truncate(1);
        let (one, titled) = (Arc::new(one), Arc::new(workload.0.clone()));
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(titled.resolution),
        };
        let size = scale.output_resolution(titled.resolution);
        let demand = reader_demand(
            &titled,
            TimeCode(3),
            size,
            scale,
            0,
            &mut FrameSizes::default(),
        )
        .expect("the demand");
        let f = (demand.sources.values())
            .map(|(spec, _)| spec.frame_bytes)
            .max();
        let (f, g) = (f.expect("a source"), demand.generated);
        assert!(g > f, "two title rasters outweigh a frame: {g} vs {f}");
        // One lookahead frame fits beside frame 3 (C − H ≥ 2f); with it in
        // flight the titles do not (2f + G > C), without it they do.
        let budget = (3 * f).max(f + g);
        let lane = Arc::new(Lane::with_budget(20, budget));
        let (gate, gated) = bounded(0);
        *lane.gate.lock().expect("gate") = Some(gated);
        let feeder = {
            let lane = Arc::clone(&lane);
            thread::spawn(move || {
                gate.send(()).expect("frame 3 decodes");
                // Draining notifies no one: poll it (60 s is a hang).
                let deadline = Instant::now() + Duration::from_secs(60);
                while !lane.lock().readers.draining() {
                    assert!(Instant::now() < deadline, "the titled job never drained");
                    thread::yield_now();
                }
                drop(gate); // frame 4, stopped, proceeds
            })
        };
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        render_ahead(&mut preview, &one, 3);
        wait_until(&lane, decoding);
        render_ahead(&mut preview, &titled, 3);
        feeder.join().expect("the feeder");
        wait_until(&lane, |state| !decoding(state));
        assert!(
            lane.cancelled.load(Ordering::Acquire) >= 1,
            "K-2: the lookahead stopped"
        );
        let key = lane.lock().readers.slots[0].key.clone();
        let state = lane.lock();
        let ring = state.readers.ring_times(&key).len() * f;
        let (live, peak) = state.readers.live();
        drop(state);
        assert_eq!(
            live,
            ring + preview.renderer.title_bytes(),
            "K-1: G was reserved"
        );
        assert!(peak <= budget, "I12");
        render_ahead(&mut preview, &one, 20);
    }

    /// I15 (S2b-4, H-8): a seeded stress on real threads, 10,000 sequences
    /// over eight (P, C) lanes: P ∈ {1, 2, 3, 20}, C from 2.5 frames to
    /// 224 MiB. A sequence is one to four events (paused, playback and
    /// empty posts over one to four sources and the cut document, an agent
    /// push, a quiescence tick); every eighth ends in a paused post that
    /// must publish. After each: peak live bytes ≤ C (I12), readers ≤ R,
    /// permits ≤ P (I11). Shutdown returns every reservation, permit and reader. A
    /// watchdog fails a hang.
    #[test]
    fn the_scheduler_survives_a_seeded_stress_on_real_threads() {
        let (done, finished) = bounded(1);
        thread::spawn(move || {
            stress(10_000);
            let _ = done.send(());
        });
        let watchdog = finished.recv_timeout(Duration::from_secs(600));
        assert_eq!(watchdog, Ok(()), "the stress run hung or failed");
    }

    fn stress(sequences: u64) {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let mut documents: Vec<Arc<Document>> = (1..=4)
            .map(|tracks| {
                let mut document = workload.0.clone();
                document.tracks.truncate(tracks);
                Arc::new(document)
            })
            .collect();
        let (cut, _cut_media) = cut_document();
        documents.push(cut);
        let f = reference(&documents[0], 0).1;
        let c = FRAME_CACHE_BYTE_BUDGET;
        let lanes = [(1, 5 * f / 2), (2, 4 * f), (3, 8 * f), (20, 5 * f / 2)];
        let lanes = lanes
            .into_iter()
            .chain([(20, 4 * f), (20, 8 * f), (2, c), (20, c)]);
        for (seed, (parallelism, budget)) in (1u64..).zip(lanes) {
            let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut below = |bound: usize| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let bound = u64::try_from(bound).expect("a bound");
                usize::try_from(state % bound).expect("below the bound")
            };
            let lane = Arc::new(Lane::with_budget(parallelism, budget));
            let (gate, frames, thread) = gated_preview(&lane);
            *lane.gate.lock().expect("gate") = None;
            drop(gate);
            let (mut seq, mut clears) = (0, false);
            for sequence in 0..sequences / 8 {
                let mut post = |kind: Option<JobKind>, document: &Arc<Document>| {
                    seq += 1;
                    lane.post(kind.map(|kind| job(document, kind, stamp(seed, seq))));
                    stamp(seed, seq)
                };
                for _ in 0..=below(4) {
                    let document = &documents[below(documents.len())];
                    let at = TimeCode(i64::try_from(below(28)).expect("a time"));
                    match below(6) {
                        0 => post(Some(JobKind::Paused(at)), document),
                        1 | 2 => post(Some(JobKind::Playback { from: at }), document),
                        3 => post(None, document),
                        4 => {
                            // Review A S2: every other agent job clears.
                            clears = !clears;
                            let job = if clears { clear_job().0 } else { stats_job().0 };
                            let _ = lane.try_push(job);
                            continue;
                        }
                        _ => {
                            *lane.skew.lock().expect("skew") += crate::sched::QUIESCENCE;
                            lane.work.notify_all();
                            continue;
                        }
                    };
                }
                if sequence % 8 == 7 {
                    // Liveness: a paused post publishes (H-2/H-3, K-2).
                    let document = &documents[below(documents.len())];
                    let at = TimeCode(i64::try_from(below(28)).expect("a time"));
                    let stamped = post(Some(JobKind::Paused(at)), document);
                    let deadline = Instant::now() + Duration::from_secs(60);
                    while frames.recv_deadline(deadline).map(|shown| shown.stamp) != Ok(stamped) {
                        assert!(
                            Instant::now() < deadline,
                            "seed {seed} {sequence}: no frame"
                        );
                    }
                }
                let (peak, slots) = {
                    let state = lane.lock();
                    (state.readers.live().1, state.readers.slots.len())
                };
                assert!(peak <= budget, "I12: seed {seed}: {peak} > {budget}");
                assert!(slots <= crate::sched::reader_limit(parallelism), "R");
                assert!(lane.permits_in_use() <= parallelism, "I11: permits over P");
            }
            lane.shut_down();
            join_within(thread);
            assert!(lane.lock().readers.slots.is_empty(), "a reader outlived");
            assert_eq!(lane.lock().readers.live().0, 0, "K-1: a reservation leaked");
            assert_eq!(lane.permits_in_use(), 0, "a permit leaked");
        }
    }

    /// H-4 (S2b-3): the drop guard asserts it never releases under `Sched`
    /// (without the assertion it would deadlock: a watchdog fails that).
    #[test]
    #[cfg(debug_assertions)]
    fn a_release_under_sched_panics() {
        let (done, finished) = bounded(1);
        thread::spawn(move || {
            let lane = Arc::new(Lane::default());
            let hold = Hold::adopt(&lane, 1);
            let state = lane.lock();
            let released = std::panic::AssertUnwindSafe(|| drop(hold));
            let panicked = std::panic::catch_unwind(released).is_err();
            drop(state);
            let _ = done.send(panicked);
        });
        let panicked = finished.recv_timeout(Duration::from_secs(10));
        assert_eq!(panicked, Ok(true), "H-4: a release under Sched must panic");
    }

    /// K-1 (S2b-3): the renderer keeps only the rasters of the job it just
    /// rendered, so the titles' reservation shrinks to that job's G.
    #[test]
    fn title_rasters_are_trimmed_to_the_job() {
        let two = crate::perf_fixtures::titled((160, 90), 30, 2);
        let mut one = two.0.clone();
        one.tracks.truncate(2);
        let (one, two) = (Arc::new(one), Arc::new(two.0.clone()));
        let lane = Arc::new(Lane::with_parallelism(20));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        render_ahead(&mut preview, &two, 3);
        let both = preview.renderer.title_bytes();
        render_ahead(&mut preview, &one, 3);
        let kept = preview.renderer.title_bytes();
        assert!(
            0 < kept && kept < both,
            "one raster of two is kept: {kept} of {both}"
        );
    }

    /// A paused render of `at` on the test thread (no lookahead).
    fn render_paused(
        preview: &mut Preview,
        document: &Arc<Document>,
        at: i64,
    ) -> Result<Option<FrameTexture>, Halt> {
        let scene = job(document, JobKind::Paused(TimeCode(at)), stamp(1, 1)).scene;
        let version = preview.lane.lock().version;
        let (playback, paused) = (None, None);
        let wait = FrameWait {
            version,
            playback,
            paused,
        };
        preview.render_monitor(&scene, TimeCode(at), &wait)
    }

    /// Review B F1: f is measured from the file, never the asset's optional
    /// resolution. With it missing or wrong, every frame the readers hold
    /// is reserved at exactly its allocation (live bytes = ring bytes, each
    /// frame f) and the frame is the synchronous renderer's.
    #[test]
    fn reservations_measure_frames_not_metadata() {
        let (document, _workload) = cut_document();
        let (expected, f) = reference(&document, 0);
        for resolution in [None, Some((64, 36))] {
            let mut misdescribed = (*document).clone();
            for asset in &mut misdescribed.media_pool {
                asset.resolution = resolution;
            }
            misdescribed.validate().expect("the metadata is optional");
            let misdescribed = Arc::new(misdescribed);
            let lane = Arc::new(Lane::with_budget(20, FRAME_CACHE_BYTE_BUDGET));
            let (mut preview, _frames) =
                test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
            let shown = render_paused(&mut preview, &misdescribed, 0);
            let Ok(Some(shown)) = shown else {
                panic!("{resolution:?}: the frame did not render");
            };
            assert_eq!(*shown.rgba, expected, "C-5 with {resolution:?}");
            let state = lane.lock();
            let (count, bytes) = state.readers.ring_bytes();
            assert!(count > 0, "not vacuous: the readers hold frames");
            assert_eq!(bytes, count * f, "{resolution:?}: each frame is f");
            assert_eq!(state.readers.live().0, bytes, "{resolution:?}: K-1");
            drop(state);
            drop(preview);
            assert_eq!(lane.lock().readers.live().0, 0, "K-1: a reservation leaked");
        }
    }

    /// A paused render of `at` in the scene of `generation` (each
    /// `set_document` is a new generation).
    fn render_generation(
        preview: &mut Preview,
        document: &Arc<Document>,
        at: i64,
        generation: u64,
    ) -> FrameTexture {
        let mut scene = job(document, JobKind::Paused(TimeCode(at)), stamp(1, 1)).scene;
        scene.generation = generation;
        let version = preview.lane.lock().version;
        let (playback, paused) = (None, None);
        let wait = FrameWait {
            version,
            playback,
            paused,
        };
        let Ok(Some(frame)) = preview.render_monitor(&scene, TimeCode(at), &wait) else {
            panic!("frame {at} of generation {generation} did not render");
        };
        frame
    }

    /// `document` with only the assets `keep` selects, and their clips.
    fn only(document: &Document, keep: impl Fn(u64) -> bool) -> Arc<Document> {
        let mut only = document.clone();
        only.media_pool.retain(|asset| keep(asset.id.0));
        for track in &mut only.tracks {
            track.clips.retain(|clip| keep(clip.asset.0));
        }
        let ends = (only.tracks.iter().flat_map(|track| &track.clips)).map(|clip| {
            clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0
        });
        only.duration = TimeCode(ends.max().unwrap_or(0));
        only.validate().expect("a subset is valid");
        Arc::new(only)
    }

    /// Amendment R43 (re-review R41 blocker 4): at P = 2 reader A holds
    /// both permits, held in its decode of 0; B, started for a job at frame
    /// 10, waits for permits. A third job no longer wants B's source: B's
    /// ticket is cancelled and it leaves `PermitWait` while A still holds
    /// the pool. So the preview's cancel reached `Permits`, which knows
    /// each reader from before its thread runs.
    #[test]
    fn an_obsolete_ticket_is_cancelled_while_the_pool_is_held() {
        let crate::perf_fixtures::Workload(document, _media) =
            crate::perf_fixtures::cuts((160, 90), 30, 1, 2, 10);
        let document = Arc::new(document);
        let lane = Arc::new(Lane::with_parallelism(2));
        let (release, held) = bounded::<()>(0);
        *lane.hold_at.lock().expect("hold") = Some((0, held));
        let (frames, thread) = threaded_preview(&lane);
        let paused = |at, seq| job(&document, JobKind::Paused(TimeCode(at)), stamp(1, seq));
        lane.post(Some(paused(0, 1)));
        let decoding_zero = |slot: &crate::sched::Slot<VideoSourceKey>| {
            matches!(
                slot.state,
                crate::sched::ReaderState::Decoding { at: 0, .. }
            )
        };
        wait_until(&lane, |state| state.readers.slots.iter().any(decoding_zero));
        assert_eq!(lane.permits_in_use(), 2, "A holds the pool");
        lane.post(Some(paused(10, 2)));
        let waiting = |slot: &crate::sched::Slot<VideoSourceKey>| {
            slot.state == crate::sched::ReaderState::PermitWait
        };
        wait_until(&lane, |state| state.readers.slots.iter().any(waiting));
        let b = (lane.lock().readers.slots.iter())
            .find(|slot| waiting(slot))
            .map(|slot| slot.id)
            .expect("B waits for permits");
        lane.post(Some(paused(0, 3)));
        wait_until(&lane, |state| {
            (state.readers.slots.iter()).all(|slot| slot.id != b || !waiting(slot))
        });
        assert_eq!(
            lane.permits_in_use(),
            2,
            "cancelled, not granted: A holds the pool"
        );
        drop(release);
        let frame = frames.recv_timeout(Duration::from_secs(60));
        assert_eq!(frame.expect("frame 0").at, TimeCode(0));
        lane.shut_down();
        join_within(thread);
    }

    /// Amendment R41 (U-1): the preview's per-source memory (frame sizes,
    /// travel) is bounded and forgets removed sources. N = 24 distinct
    /// sources pass a cap of 3: (1) all in the document, previewed in
    /// turn, each map stays within the cap, the newest remembered; (2) each
    /// imported, previewed and removed in turn (a new document each), the
    /// maps hold the current source only, and no reader or synchronous
    /// decoder of a removed one is open; (3) a cache clear forgets both.
    #[test]
    fn source_memory_is_bounded_and_forgets_removed_sources() {
        use std::collections::BTreeSet;
        const N: u64 = 24;
        const CAP: usize = 3;
        let workload = crate::perf_fixtures::many_sources((160, 90), N, 2);
        let all = Arc::new(workload.0.clone());
        let lane = Arc::new(Lane::with_budget(4, FRAME_CACHE_BYTE_BUDGET));
        lane.lock().readers.remember(CAP);
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        preview.sizes = FrameSizes::with_cap(CAP);
        let ids = |keys: &[VideoSourceKey]| -> BTreeSet<u64> {
            keys.iter().map(|key| key.asset().0).collect()
        };
        let remembered = |preview: &Preview| {
            let sizes: Vec<_> = preview.sizes.keys().cloned().collect();
            let travel = lane.lock().readers.travel_keys();
            (ids(&sizes), ids(&travel))
        };
        for id in 1..=N {
            render_generation(&mut preview, &all, (id.cast_signed() - 1) * 2, 1);
            let (sizes, travel) = remembered(&preview);
            let bounded = sizes.len() <= CAP && travel.len() <= CAP;
            assert!(bounded, "(1) source {id}: {sizes:?} {travel:?}");
            let newest = sizes.contains(&id) && travel.contains(&id);
            assert!(
                newest,
                "(1) source {id} is remembered: {sizes:?} {travel:?}"
            );
        }
        for id in 1..=N {
            let document = only(&all, |asset| asset == id);
            render_generation(&mut preview, &document, (id.cast_signed() - 1) * 2, id + 1);
            let current = BTreeSet::from([id]);
            let removed = |key: &VideoSourceKey| key.asset().0 != id;
            assert_eq!(
                remembered(&preview),
                (current.clone(), current),
                "(2) source {id}: a removed source is remembered"
            );
            assert_eq!(
                lane.open_decoders(removed),
                0,
                "(2) source {id}: a removed source's reader decoder is open"
            );
            let alive = (lane.lock().readers.slots.iter()).any(|slot| removed(&slot.key));
            assert!(
                !alive,
                "(2) source {id}: a removed source's reader is alive"
            );
            let synchronous = preview.renderer.source_keys();
            assert!(!synchronous.iter().any(removed), "(2) source {id}");
        }
        let open = lane.open_decoders(|_| true);
        assert!(open > 0, "not vacuous: the current source's reader is open");
        let (clear, response) = clear_job();
        preview.run_agent(clear, false);
        one_reply(&response).expect("cleared");
        let empty = (BTreeSet::new(), BTreeSet::new());
        assert_eq!(remembered(&preview), empty, "(3) a clear forgets");
        let open = lane.open_decoders(|_| true);
        assert_eq!(open, 0, "(3) a clear closes the readers' decoders");
    }

    /// Review B F1: a decoded frame whose size is not its reservation fails
    /// the job through `deliver` (no assertion, no undercharge), and the
    /// reader's slot, permits and bytes are all released when it goes.
    #[test]
    fn a_frame_larger_than_its_reservation_fails_and_cleans_up() {
        let (document, _workload) = cut_document();
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        let sizes = &mut FrameSizes::default();
        let demand = reader_demand(&document, TimeCode(0), resolution, scale, 0, sizes);
        assert!(demand.is_ok(), "the demand");
        let lane = Arc::new(Lane::with_budget(20, FRAME_CACHE_BYTE_BUDGET));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        // Seed a size one row short: the reservation undercharges.
        preview.sizes = (sizes.iter())
            .map(|(key, (width, height))| (key.clone(), (*width, height - 1)))
            .collect();
        let failed = render_paused(&mut preview, &document, 0);
        let Err(Halt::Failed(MediaError::Backend(message))) = failed else {
            panic!("the undercharged frame was accepted");
        };
        assert!(message.contains("decode-reader: a frame of"), "{message}");
        drop(preview);
        let state = lane.lock();
        assert!(state.readers.slots.is_empty(), "every reader exited");
        assert_eq!(state.readers.live().0, 0, "K-1: a reservation leaked");
        drop(state);
        assert_eq!(lane.permits_in_use(), 0, "H-5: permits leaked");
    }

    /// R38 D1: a lookahead-only source that cannot be read does not fail the
    /// frame being shown. A valid source plays at frame 0 while an unreadable
    /// source B starts ten frames later, inside the playback horizon; frame 0
    /// renders the synchronous bytes, and B fails only the job that requires
    /// it (frame 10).
    #[test]
    fn an_unreadable_lookahead_source_does_not_fail_the_current_frame() {
        let (document, _workload) = cut_document();
        let directory = TempDirectory::new("pf1-r38-d1");
        let garbage = directory.path("unreadable.mp4");
        std::fs::write(&garbage, b"not a video").expect("the file");
        let mut with_b = (*document).clone();
        let mut asset = with_b.media_pool[0].clone();
        (asset.id, asset.path) = (kinewright_core::AssetId(99), garbage);
        let mut clip = with_b.tracks[0].clips[0].clone();
        clip.id = kinewright_core::ClipId(99);
        (clip.asset, clip.timeline_start) = (asset.id, TimeCode(10));
        clip.source_range = TimeCode(0)..TimeCode(10);
        let mut track = with_b.tracks[0].clone();
        (track.id, track.clips) = (kinewright_core::TrackId(99), vec![clip]);
        with_b.media_pool.push(asset);
        with_b.tracks.push(track);
        with_b.validate().expect("B is valid as a document");
        let with_b = Arc::new(with_b);
        let lane = Arc::new(Lane::with_budget(20, FRAME_CACHE_BYTE_BUDGET));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        // Frame 0's horizon reaches frame 10, where B is the top layer.
        let shown = render_ahead(&mut preview, &with_b, 0);
        assert_eq!(*shown.rgba, reference(&with_b, 0).0, "C-5: frame 0");
        assert!(lane.take_failures().is_empty(), "no reader failed");
        // Not vacuous: B is required at frame 10, and that job fails.
        let failed = render_paused(&mut preview, &with_b, 10);
        assert!(
            matches!(failed, Err(Halt::Failed(_))),
            "frame 10 requires the unreadable source"
        );
    }

    /// Review B S2: a paused frame shown again reserves nothing for its
    /// cached title rasters (the preview's titles already hold them), so at
    /// C = f + G + f/2 it neither drains nor re-rasterizes them. The
    /// renderer's rasterization count is the witness (R38: a drain would
    /// clear the rasters and the second render would make them again).
    #[test]
    fn a_resident_title_is_not_reserved_again() {
        let workload = crate::perf_fixtures::titled((160, 90), 30, 2);
        let titled = Arc::new(workload.0.clone());
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(titled.resolution),
        };
        let size = scale.output_resolution(titled.resolution);
        let demand = reader_demand(
            &titled,
            TimeCode(3),
            size,
            scale,
            0,
            &mut FrameSizes::default(),
        )
        .expect("the demand");
        let f = (demand.sources.values())
            .map(|(spec, _)| spec.frame_bytes)
            .max();
        let (f, g) = (f.expect("a source"), demand.generated);
        let budget = f + g + f / 2;
        let lane = Arc::new(Lane::with_budget(20, budget));
        let (mut preview, frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        let mut rasterized = Vec::new();
        for seq in 1..=2 {
            let before = preview.renderer.rasterized;
            let paused = job(&titled, JobKind::Paused(TimeCode(3)), stamp(1, seq));
            preview.run_paused(&paused, 0);
            assert!(frames.try_recv().is_ok(), "shown {seq}");
            rasterized.push(preview.renderer.rasterized - before);
        }
        assert_eq!(rasterized, [2, 0], "two rasters made once, then reused");
        assert_eq!(preview.renderer.title_bytes(), g, "both stay cached");
        let (live, peak) = lane.lock().readers.live();
        assert_eq!(live, f + g, "frame 3 and the titles");
        assert!(peak <= budget, "I12");
    }

    /// R38 D3 (the re-review's sequence, scaled): an empty frame, then an
    /// agent thumbnail of a later frame of n solids at the monitor's size
    /// (G = C), then a paused preview of that frame, then a seek to a
    /// video-only frame, then the thumbnail again (only part fits). After
    /// every step the preview's live bytes are exactly what it holds — ring
    /// frames and cached rasters — and stay ≤ C: the thumbnail's rasters are
    /// adopted with their charge while they fit, and dropped when they do
    /// not.
    #[test]
    fn thumbnail_rasters_are_charged_or_dropped() {
        use kinewright_core::{BlendMode, ClipContent, SolidColor, Track, TrackId, TrackKind};
        let n = 4u8;
        let workload = crate::perf_fixtures::titled((160, 90), 10, 0);
        let mut document = workload.0.clone();
        document.tracks[0].clips[0].timeline_start = TimeCode(10);
        for index in 0..n {
            let color = SolidColor {
                r: 40 * index,
                g: 200,
                b: 90,
            };
            let mut solid = crate::mo2_fixtures::clip(
                100 + u64::from(index),
                ClipContent::Solid(color),
                BlendMode::Normal,
                Vec::new(),
            );
            (solid.timeline_start, solid.source_range) = (TimeCode(5), TimeCode(0)..TimeCode(5));
            document.tracks.push(Track {
                id: TrackId(10 + u64::from(index)),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![solid],
            });
        }
        document.duration = TimeCode(20);
        document.validate().expect("the D3 document is valid");
        let document = Arc::new(document);
        let max_width = monitor_max_width(document.resolution);
        let scale = RenderScale::Proxy { max_width };
        let size = scale.output_resolution(document.resolution);
        let demand = reader_demand(
            &document,
            TimeCode(5),
            size,
            scale,
            0,
            &mut FrameSizes::default(),
        )
        .expect("the demand");
        let budget = demand.generated;
        assert_eq!(
            budget,
            usize::from(n) * size.0 as usize * size.1 as usize * 8
        );
        let lane = Arc::new(Lane::with_budget(20, budget));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        let held = |preview: &Preview, step: &str| {
            let state = lane.lock();
            let ((live, peak), rings) = (state.readers.live(), state.readers.ring_bytes().1);
            drop(state);
            let titles = preview.renderer.title_bytes();
            assert_eq!(
                live,
                rings + titles,
                "{step}: live is what the preview holds"
            );
            assert!(peak <= budget, "{step}: I12 ({peak} > {budget})");
            (rings, titles)
        };
        let thumbnail = |preview: &mut Preview| {
            let (reply, response) = bounded(1);
            let work = AgentWork::Thumbnail {
                document: Arc::clone(&document),
                lut: Arc::default(),
                at: TimeCode(5),
                max_width,
                reply,
            };
            let cancel = Arc::default();
            preview.run_agent(AgentJob { work, cancel }, false);
            assert!(one_reply(&response).is_ok(), "the thumbnail rendered");
        };
        assert!(matches!(
            render_paused(&mut preview, &document, 0),
            Ok(Some(_))
        ));
        assert_eq!(held(&preview, "empty frame"), (0, 0));
        thumbnail(&mut preview);
        assert_eq!(held(&preview, "thumbnail"), (0, budget), "adopted: G = C");
        assert!(matches!(
            render_paused(&mut preview, &document, 5),
            Ok(Some(_))
        ));
        assert_eq!(
            held(&preview, "solids"),
            (0, budget),
            "reused, not reserved again"
        );
        let video = render_paused(&mut preview, &document, 10);
        assert!(matches!(video, Ok(Some(_))), "the video frame rendered");
        let (rings, titles) = held(&preview, "video");
        assert!(rings > 0 && titles == 0, "the seek drained the rasters");
        // Beside the video frame (f = g here) three rasters fit: those are
        // adopted, the fourth is dropped.
        thumbnail(&mut preview);
        let (_, adopted) = held(&preview, "thumbnail over video");
        assert_eq!(rings + adopted, budget, "adopted while they fit");
        assert_eq!(adopted, budget / usize::from(n) * usize::from(n - 1));
        drop(preview);
        assert_eq!(lane.lock().readers.live().0, 0, "K-1: a charge leaked");
    }

    /// K-3 (S2b-3): a required set over C renders synchronously, counted
    /// by reason, with the synchronous renderer's bytes.
    #[test]
    fn a_required_set_over_c_falls_back_to_the_synchronous_renderer() {
        let workload = crate::perf_fixtures::four_sources((160, 90), 30);
        let mut three = workload.0.clone();
        three.tracks.truncate(3);
        let three = Arc::new(three);
        let (expected, f) = reference(&three, 3);
        let lane = Arc::new(Lane::with_budget(20, 5 * f / 2));
        let (mut preview, _frames) =
            test_preview_on(Arc::clone(&lane), Arc::new(SharedClock::new()));
        let shown = render_ahead(&mut preview, &three, 3);
        assert_eq!(*shown.rgba, expected, "C-5");
        assert_eq!(lane.lock().fallbacks, [0, 1], "one budget fallback");
        assert_eq!(lane.counters().stats.sync_fallback_frames, 1);
        assert!(lane.lock().readers.slots.is_empty(), "no reader started");
        assert!(
            preview.renderer.cache_stats().file_count > 0,
            "the synchronous path decoded"
        );
    }

    /// E-2 (S2b-1): a reader that cannot start fails its required frame
    /// with the `decode-reader:` prefix, stamped with the job.
    #[test]
    fn a_reader_spawn_failure_is_prefixed() {
        let (document, _workload) = cut_document();
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        FAIL_READER_SPAWN.with(|fail| fail.set(true));
        preview.run_paused(
            &job(&document, JobKind::Paused(TimeCode(0)), stamp(4, 9)),
            0,
        );
        let failures = preview.lane.take_failures();
        let [(stamped, MediaError::Backend(message))] = failures.as_slice() else {
            panic!("{failures:?}");
        };
        assert_eq!(*stamped, stamp(4, 9));
        assert_eq!(message, "decode-reader: spawn failed: injected");
        assert!(frames.try_recv().is_err());
    }

    /// R-4 / H-8 (S2b-1): while a paused `FrameWait` waits on a reader, an
    /// agent push suspends it and is answered; a cancelled one is not run;
    /// the wait then resumes and publishes.
    #[test]
    fn an_agent_push_suspends_a_paused_frame_wait() {
        let (document, _workload) = cut_document();
        let lane = Arc::<Lane>::default();
        let (gate, frames, thread) = gated_preview(&lane);
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode(0)),
            stamp(1, 1),
        )));
        wait_until(&lane, decoding);
        let (cancelled, cancelled_reply, flag) = stats_job();
        flag.store(true, Ordering::Release);
        let (live, response, _) = stats_job();
        assert!(lane.try_push(cancelled) && lane.try_push(live));
        let reply = response.recv_timeout(Duration::from_secs(60));
        assert!(reply.expect("answered during the wait").is_ok());
        assert!(cancelled_reply.try_recv().is_err(), "a cancelled job ran");
        assert!(frames.try_recv().is_err(), "the frame is still waiting");
        drop(gate);
        let shown = frames
            .recv_timeout(Duration::from_secs(60))
            .expect("published");
        assert_eq!((shown.at, shown.stamp), (TimeCode(0), stamp(1, 1)));
        lane.shut_down();
        join_within(thread);
    }

    /// H-6 (S2b-1): shutdown with the preview in `FrameWait` and a reader
    /// decoding, with readers idle, or with a newer job decoding, ends
    /// every reader and then the preview; no agent job is lost.
    #[test]
    fn shutdown_ends_every_reader_in_every_phase() {
        let (document, _workload) = cut_document();
        for phase in 0..4 {
            let lane = Arc::<Lane>::default();
            let (mut gate, frames, thread) = gated_preview(&lane);
            lane.post(Some(job(
                &document,
                JobKind::Paused(TimeCode(0)),
                stamp(1, 1),
            )));
            if phase >= 1 {
                wait_until(&lane, decoding);
            }
            if phase >= 2 {
                // Open the gate: the readers finish and go idle.
                *lane.gate.lock().expect("gate") = None;
                drop(gate);
                frames
                    .recv_timeout(Duration::from_secs(60))
                    .expect("published");
                wait_until(&lane, |state| !decoding(state));
                let (closed, gated) = bounded(0);
                *lane.gate.lock().expect("gate") = Some(gated);
                gate = closed;
            }
            if phase == 3 {
                lane.post(Some(job(
                    &document,
                    JobKind::Paused(TimeCode(25)),
                    stamp(2, 2),
                )));
                wait_until(&lane, decoding);
            }
            let (agent, response, _) = stats_job();
            let queued = lane.try_push(agent);
            let (moved, _) = lane.shut_down();
            drop(gate);
            for job in moved {
                job.reply_error(worker_stopped());
            }
            join_within(thread);
            assert!(
                lane.lock().readers.slots.is_empty(),
                "phase {phase}: a reader outlived"
            );
            if queued {
                assert!(
                    response.recv_timeout(Duration::from_secs(1)).is_ok(),
                    "phase {phase}"
                );
            }
        }
    }
}
