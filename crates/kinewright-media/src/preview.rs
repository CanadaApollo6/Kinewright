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
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender};
use kinewright_core::{
    Document, FrameStamp, FrameTexture, MediaError, PreviewFrame, Rational, RgbaImage, TimeCode,
};

use crate::{
    derived_cache::CacheStats,
    engine::{SharedClock, monitor_max_width, send_latest},
    lut_store::LutLibrary,
    render::{DecodeStrategy, FrameRenderer, RenderScale},
    stats::Counters,
};

/// R-4: at most this many agent jobs wait; a full queue replies at once.
pub(crate) const AGENT_QUEUE_LIMIT: usize = 8;
/// A playback hold re-reads the clock this often while it waits.
const HOLD_POLL: Duration = Duration::from_millis(1);

pub(crate) fn worker_stopped() -> MediaError {
    MediaError::Backend("media worker stopped".to_owned())
}

pub(crate) type Wakeup = Arc<dyn Fn() + Send + Sync>;

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
}

/// The worker/preview hand-off: one leaf lock and the `ready` condvar.
#[derive(Default)]
pub(crate) struct Lane {
    state: Mutex<LaneState>,
    ready: Condvar,
    /// R-5's counters: a separate leaf, never taken with `state`.
    counters: Mutex<Counters>,
}

impl Lane {
    pub(crate) fn lock(&self) -> MutexGuard<'_, LaneState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn counters(&self) -> MutexGuard<'_, Counters> {
        self.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        self.ready.notify_all();
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
    Paused(TransportJob),
    Playback(TransportJob, u64),
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
    #[cfg(test)]
    pub(crate) faults: Arc<crate::engine::Faults>,
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
                self.run_agent(job);
                if playing {
                    self.lane.counters().agent_finished();
                }
                self.agent_turn = false;
            }
            Work::Paused(job) => {
                self.run_paused(&job);
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
            let parked = self.parked == Some(state.version);
            let runnable = (state.transport.as_ref())
                .map(|job| job.kind)
                .filter(|kind| matches!(kind, JobKind::Paused(_)) || !parked);
            if !state.agent.is_empty() && (self.agent_turn || runnable.is_none()) {
                break state.agent.pop_front().map(Work::Agent);
            }
            match runnable {
                Some(JobKind::Paused(_)) => break state.transport.take().map(Work::Paused),
                Some(JobKind::Playback { .. }) => {
                    let version = state.version;
                    break state
                        .transport
                        .clone()
                        .map(|job| Work::Playback(job, version));
                }
                None if !wait => break None,
                None => {
                    state = self
                        .lane
                        .ready
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        };
        drop(state);
        // H-4: cancelled jobs' reply senders drop after unlock, unanswered.
        drop(discarded);
        work
    }

    fn bind(&mut self, generation: Option<u64>, lut: &Arc<LutLibrary>) {
        if generation.is_some() && generation != self.generation {
            self.renderer.clear();
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
        document: &Document,
        at: TimeCode,
        strategy: DecodeStrategy,
    ) -> Result<Option<FrameTexture>, MediaError> {
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        #[cfg(test)]
        if self.faults.fail_render.swap(false, Ordering::AcqRel) {
            return Err(MediaError::Backend("injected render failure".to_owned()));
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
        let frame = self
            .renderer
            .render_live(document, at, resolution, scale, strategy)?;
        #[cfg(test)]
        if !self.faults.publish_after_render() {
            return Ok(None);
        }
        Ok(Some(frame))
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
    pub(crate) fn run_paused(&mut self, job: &TransportJob) {
        let JobKind::Paused(at) = job.kind else {
            return;
        };
        self.bind(Some(job.scene.generation), &job.scene.lut);
        match self.render_monitor(&job.scene.document, at, DecodeStrategy::Seek) {
            Ok(Some(texture)) => self.publish(PreviewFrame {
                at,
                stamp: job.stamp,
                texture,
            }),
            Ok(None) => {}
            Err(error) => self.fail(job.stamp, error),
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
        if self.playback_version != Some(version) {
            self.playback_version = Some(version);
            self.next_at = from.0;
        }
        let frame_ms = frame_ms(document.fps);
        let lead = lead_frames(self.render_ewma_ms, frame_ms);
        let target = (self.clock.position().0 + lead).max(self.next_at);
        if target >= document.duration.0 {
            self.parked = Some(version);
            return Err(Attempt::Parked);
        }
        self.bind(Some(job.scene.generation), &job.scene.lut);
        let started = Instant::now();
        let at = TimeCode(target);
        let rendered = self.render_monitor(&document, at, DecodeStrategy::Sequential);
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
            Err(error) => {
                self.fail(job.stamp, error);
                self.parked = Some(version);
                return Err(Attempt::Parked);
            }
        };
        let frames = u32::try_from(lead + 1).unwrap_or(3);
        let wait = Duration::from_secs_f64(frame_ms * f64::from(frames) / 1e3);
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
            state = self
                .lane
                .ready
                .wait_timeout(state, HOLD_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// R-4: an agent job renders synchronously with `Seek`, outside every
    /// lock; its reply is sent exactly once, unless it was cancelled.
    pub(crate) fn run_agent(&mut self, job: AgentJob) {
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
                self.reply(&cancel, &reply, result);
            }
            AgentWork::CacheStats { clear, reply } => {
                let stats = if clear {
                    self.renderer.clear()
                } else {
                    self.renderer.cache_stats()
                };
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
            preview.run_paused(&job(&document, JobKind::Paused(TimeCode(at)), stamp(1, 1)));
        }
        assert_eq!(frames.try_iter().count(), 2);
        assert_eq!(live_table_frames(), before + 2, "two paused renders");
        let (reply, response) = bounded(1);
        preview.run_agent(AgentJob {
            work: AgentWork::Thumbnail {
                document: Arc::clone(&document),
                lut: Arc::default(),
                at: TimeCode(1),
                max_width: 64,
                reply,
            },
            cancel: Arc::default(),
        });
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
        let playback = JobKind::Playback { from: TimeCode(10) };
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
            let playback = JobKind::Playback { from: TimeCode(10) };
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
        let playback = JobKind::Playback { from: TimeCode(10) };
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
}
