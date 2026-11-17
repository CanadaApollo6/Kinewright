//! Queue and launch breaker (ruling 35). Pure state over caller-supplied time:
//! no process, pipe, or scope authority. Healthy launches are bounded by the
//! active slots and this queue; only unhealthy outcomes consume breaker tokens.
use super::{
    Busy, Failure, INITIAL_DEADLINE, Outcome, PENDING_PROBES, QUEUE_WAIT, Request, Status,
};
use kinewright_media_ipc::MediaSandboxError as Error;
use std::{
    collections::VecDeque,
    sync::{atomic::Ordering, mpsc::SyncSender},
    time::{Duration, Instant},
};

pub(super) const COOLDOWN: Duration = Duration::from_secs(60);
const BURST: f64 = 4.0;
const REFILL_PER_SECOND: f64 = 2.0;
const PER_MINUTE: usize = 20;
const HISTORY: usize = 256;

pub(super) struct Pending {
    pub request: Request,
    pub reply: SyncSender<Outcome>,
    /// Queue wait is bounded separately; the decode deadline starts at launch.
    pub queued: Instant,
    pub degraded: bool,
}
impl Pending {
    /// The decode window opens at launch, never at enqueue: time spent behind
    /// another source's slow decode is the queue's bound, not this one's.
    pub fn decode_deadline(&self, launched: Instant) -> Instant {
        debug_assert!(launched >= self.queued, "launched before it was queued");
        launched + INITIAL_DEADLINE
    }
}

/// The launch outcome as the breaker sees it. Cancellation and lost source
/// authority describe the caller, not the helper, so they are neither.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Health {
    Healthy,
    Neutral,
    Unhealthy(Error),
}
impl Health {
    pub fn of(result: &Outcome) -> Self {
        match result {
            Ok(_) => Self::Healthy,
            Err(Failure::Sandbox(Error::Cancelled) | Failure::Authority(_) | Failure::Busy(_)) => {
                Self::Neutral
            }
            Err(Failure::Sandbox(error)) => Self::Unhealthy(*error),
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
pub(super) struct Circuit {
    failures: VecDeque<Instant>,
    history: Vec<History>,
    tokens: f64,
    refill: Instant,
    paused: Option<Instant>,
}
/// What one recorded outcome means for its request and for admission.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct Recorded {
    pub retry: bool,
    pub paused: bool,
}
impl Circuit {
    pub fn new(now: Instant) -> Self {
        Self {
            failures: VecDeque::with_capacity(PER_MINUTE),
            history: Vec::with_capacity(HISTORY),
            tokens: BURST,
            refill: now,
            paused: None,
        }
    }
    pub fn paused(&self) -> bool {
        self.paused.is_some()
    }
    /// Per-source admission only (quarantine and explicit retry); it spends
    /// no breaker token, so healthy imports never approach a pause.
    pub fn permits(&mut self, identity: [u8; 32], explicit: bool, now: Instant) -> bool {
        if self.paused.is_some() {
            return false;
        }
        self.forget(now);
        let Some(entry) = self
            .history
            .iter_mut()
            .find(|entry| entry.identity == identity)
        else {
            return true;
        };
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
        true
    }
    fn forget(&mut self, now: Instant) {
        self.history.retain(|entry| {
            entry.crashes != 0
                || entry.quarantine
                || now.saturating_duration_since(entry.seen) < Duration::from_secs(300)
        });
    }
    /// Charge one finished launch. Only an unhealthy outcome spends a token;
    /// an unhealthy outcome with none left (2/s, burst 4, 20 per rolling
    /// minute) pauses all admission.
    pub fn record(&mut self, identity: [u8; 32], health: Health, now: Instant) -> Recorded {
        let Health::Unhealthy(error) = health else {
            return Recorded::default();
        };
        self.failures
            .retain(|stamp| now.saturating_duration_since(*stamp) < Duration::from_secs(60));
        self.tokens = (self.tokens
            + now.saturating_duration_since(self.refill).as_secs_f64() * REFILL_PER_SECOND)
            .min(BURST);
        self.refill = now;
        if self.tokens < 1.0 || self.failures.len() >= PER_MINUTE {
            self.paused.get_or_insert(now);
            return Recorded {
                retry: false,
                paused: true,
            };
        }
        self.tokens -= 1.0;
        self.failures.push_back(now);
        let retry = self.source_failed(identity, error, now);
        Recorded {
            retry,
            paused: self.paused.is_some(),
        }
    }
    fn source_failed(&mut self, identity: [u8; 32], error: Error, now: Instant) -> bool {
        if !matches!(error, Error::Crashed | Error::ProtocolViolation) {
            return false;
        }
        self.forget(now);
        let index = if let Some(index) = self
            .history
            .iter()
            .position(|entry| entry.identity == identity)
        {
            index
        } else if self.history.len() < HISTORY {
            self.history.push(History {
                identity,
                crashes: 0,
                quarantine: false,
                automatic: None,
                retry: VecDeque::with_capacity(3),
                seen: now,
            });
            self.history.len() - 1
        } else {
            // A full table of failing identities is itself a hostile flood.
            self.paused.get_or_insert(now);
            return false;
        };
        let entry = &mut self.history[index];
        entry.seen = now;
        if error == Error::ProtocolViolation {
            entry.quarantine = true;
            return false;
        }
        entry.crashes = entry.crashes.saturating_add(1);
        if entry.crashes >= 2 {
            entry.quarantine = true;
            false
        } else if entry
            .automatic
            .is_none_or(|stamp| now.saturating_duration_since(stamp) >= Duration::from_secs(300))
        {
            entry.automatic = Some(now);
            true
        } else {
            false
        }
    }
    pub fn quarantine(&mut self, identity: [u8; 32], now: Instant) {
        if let Some(entry) = self
            .history
            .iter_mut()
            .find(|entry| entry.identity == identity)
        {
            entry.quarantine = true;
        } else if self.history.len() < HISTORY {
            self.history.push(History {
                identity,
                crashes: 0,
                quarantine: true,
                automatic: None,
                retry: VecDeque::with_capacity(3),
                seen: now,
            });
        } else {
            self.paused.get_or_insert(now);
        }
    }
    /// Remaining cooldown, or `None` once the next request may resume.
    fn cooldown(&self, now: Instant) -> Option<Duration> {
        self.paused
            .map(|stamp| COOLDOWN.saturating_sub(now.saturating_duration_since(stamp)))
            .filter(|left| !left.is_zero())
    }
    fn resume(&mut self, now: Instant) {
        self.paused = None;
        self.tokens = BURST;
        self.refill = now;
    }
}

/// Queued requests. Every removal of a fresh caller request releases the
/// producer's pending reservation exactly once.
pub(super) struct Queue {
    pub pending: VecDeque<Pending>,
    pub continued: VecDeque<Pending>,
    pub circuit: Circuit,
}
impl Queue {
    pub fn new(now: Instant) -> Self {
        Self {
            pending: VecDeque::with_capacity(PENDING_PROBES),
            continued: VecDeque::with_capacity(super::ACTIVE_PROBES),
            circuit: Circuit::new(now),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.continued.is_empty()
    }
    /// A new caller request. After the cooldown, this request is the explicit
    /// act that resumes admission; nothing queued survives a pause to drain.
    pub fn offer(&mut self, item: Pending, status: &Status, now: Instant) {
        if self.circuit.paused() {
            if let Some(left) = self.circuit.cooldown(now) {
                refuse(
                    &item,
                    Failure::Busy(Busy::Paused { retry_after: left }),
                    true,
                    status,
                );
                return;
            }
            self.circuit.resume(now);
        }
        if self.pending.len() >= PENDING_PROBES {
            refuse(&item, Failure::Busy(Busy::QueueFull), true, status);
            return;
        }
        self.pending.push_back(item);
    }
    /// Relaunch of an already-admitted request (scope fallback or one
    /// automatic crash retry). A pause refuses it like any queued request.
    pub fn continue_with(&mut self, item: Pending, status: &Status, now: Instant) {
        if let Some(left) = self.circuit.cooldown(now) {
            refuse(
                &item,
                Failure::Busy(Busy::Paused { retry_after: left }),
                false,
                status,
            );
            return;
        }
        self.continued.push_back(item);
    }
    /// Pause clears the queue: every queued request learns the cooldown.
    pub fn clear_for_pause(&mut self, status: &Status, now: Instant) {
        let left = self.circuit.cooldown(now).unwrap_or(COOLDOWN);
        for item in self.continued.drain(..) {
            refuse(
                &item,
                Failure::Busy(Busy::Paused { retry_after: left }),
                false,
                status,
            );
        }
        for item in self.pending.drain(..) {
            refuse(
                &item,
                Failure::Busy(Busy::Paused { retry_after: left }),
                true,
                status,
            );
        }
    }
    /// Queue wait has its own bound and typed retryable refusal; a caller's
    /// cancellation also leaves the queue without waiting for a slot.
    pub fn expire(&mut self, status: &Status, now: Instant) {
        let stale = |item: &Pending| {
            now.saturating_duration_since(item.queued) >= QUEUE_WAIT
                || item.request.cancel.load(Ordering::Acquire)
        };
        for (queue, fresh) in [(&mut self.continued, false), (&mut self.pending, true)] {
            let mut index = 0;
            while index < queue.len() {
                if stale(&queue[index]) {
                    let item = queue.remove(index).expect("index is in bounds");
                    let failure = if item.request.cancel.load(Ordering::Acquire) {
                        Failure::Sandbox(Error::Cancelled)
                    } else {
                        Failure::Busy(Busy::QueueWait)
                    };
                    refuse(&item, failure, fresh, status);
                } else {
                    index += 1;
                }
            }
        }
    }
    /// The oldest request whose source has no active cursor. Returns whether
    /// it was a fresh caller request (its reservation is released here).
    pub fn next(&mut self, busy: impl Fn(&[u8; 32]) -> bool, status: &Status) -> Option<Pending> {
        let eligible = |item: &Pending| !busy(&item.request.source.identity());
        if let Some(index) = self.continued.iter().position(eligible) {
            return self.continued.remove(index);
        }
        let index = self.pending.iter().position(eligible)?;
        let item = self.pending.remove(index)?;
        status.pending.fetch_sub(1, Ordering::AcqRel);
        Some(item)
    }
    pub fn close(&mut self, status: &Status) {
        for item in self.continued.drain(..) {
            refuse(&item, Failure::Sandbox(Error::Cancelled), false, status);
        }
        for item in self.pending.drain(..) {
            refuse(&item, Failure::Sandbox(Error::Cancelled), true, status);
        }
    }
}
fn refuse(item: &Pending, failure: Failure, fresh: bool, status: &Status) {
    if fresh {
        status.pending.fetch_sub(1, Ordering::AcqRel);
    }
    let _ = item.reply.send(Err(failure));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        sync::{Arc, atomic::AtomicBool, mpsc},
    };
    struct Input([u8; 32], File);
    impl super::super::Source for Input {
        fn file(&self) -> &File {
            &self.1
        }
        fn identity(&self) -> [u8; 32] {
            self.0
        }
        fn check(&self) -> Result<(), kinewright_core::MediaError> {
            Ok(())
        }
    }
    fn item(id: u8, queued: Instant) -> (Pending, mpsc::Receiver<Outcome>) {
        let (reply, receiver) = mpsc::sync_channel(1);
        (
            Pending {
                request: Request {
                    source: Box::new(Input([id; 32], File::open("/dev/null").unwrap())),
                    video: None,
                    audio: None,
                    cancel: Arc::new(AtomicBool::new(false)),
                    explicit_retry: false,
                },
                reply,
                queued,
                degraded: false,
            },
            receiver,
        )
    }
    #[test]
    fn a_queued_request_is_not_timed_out_by_another_sources_slow_decode() {
        // Queued at t0 behind another source whose decode used its whole
        // 5 s; launched just as that slot frees, it still gets a full window.
        let queued = Instant::now();
        let (pending, _reply) = item(1, queued);
        let launched = queued + INITIAL_DEADLINE + Duration::from_millis(900);
        let deadline = pending.decode_deadline(launched);
        assert!(
            launched < deadline,
            "deadline expired before the decode started"
        );
        assert_eq!(deadline - launched, INITIAL_DEADLINE);
        // The queue's own bound is separate and longer than a decode window.
        assert!(QUEUE_WAIT > INITIAL_DEADLINE + super::super::CANCEL_GRACE);
    }
    fn offered(
        queue: &mut Queue,
        status: &Status,
        id: u8,
        now: Instant,
    ) -> mpsc::Receiver<Outcome> {
        status.pending.fetch_add(1, Ordering::AcqRel);
        let (pending, receiver) = item(id, now);
        queue.offer(pending, status, now);
        receiver
    }

    #[test]
    fn hundred_healthy_admissions_in_a_row_never_pause() {
        let start = Instant::now();
        let mut circuit = Circuit::new(start);
        for n in 0..100_u8 {
            // Back-to-back launches, far faster than the 2/s refill.
            let now = start + Duration::from_millis(u64::from(n));
            assert!(circuit.permits([n; 32], false, now), "launch {n} refused");
            let recorded = circuit.record([n; 32], Health::of(&Ok(fixture_probe())), now);
            assert_eq!(recorded, Recorded::default(), "launch {n}");
            assert!(!circuit.paused(), "healthy launch {n} paused admission");
        }
    }
    #[test]
    fn unhealthy_outcomes_spend_the_unchanged_rates() {
        let start = Instant::now();
        let mut circuit = Circuit::new(start);
        let bad = Health::Unhealthy(Error::UnsupportedMedia);
        for n in 0..4 {
            assert!(!circuit.record([n; 32], bad, start).paused, "burst {n}");
        }
        assert!(
            circuit.record([9; 32], bad, start).paused,
            "fifth in a burst"
        );
        let mut circuit = Circuit::new(start);
        for n in 0..20_u8 {
            let at = start + Duration::from_secs(u64::from(n) * 2);
            assert!(!circuit.record([n; 32], bad, at).paused, "minute {n}");
        }
        assert!(
            circuit
                .record([30; 32], bad, start + Duration::from_secs(40))
                .paused,
            "twenty-first in a rolling minute"
        );
    }
    #[test]
    fn twenty_crashing_sources_pause_clear_the_queue_refuse_then_resume() {
        let start = Instant::now();
        let status = Status::default();
        let mut queue = Queue::new(start);
        let waiting: Vec<_> = (100..110)
            .map(|n| offered(&mut queue, &status, n, start))
            .collect();
        let mut paused_at = None;
        for n in 0..20_u8 {
            let now = start + Duration::from_millis(u64::from(n) * 10);
            if queue
                .circuit
                .record([n; 32], Health::Unhealthy(Error::Crashed), now)
                .paused
            {
                queue.clear_for_pause(&status, now);
                paused_at = Some(now);
                break;
            }
        }
        let paused_at = paused_at.expect("twenty crashing sources pause admission");
        assert!(queue.is_empty(), "pause leaves nothing to drain");
        assert_eq!(status.pending.load(Ordering::Acquire), 0);
        for receiver in waiting {
            assert!(matches!(
                receiver.try_recv(),
                Ok(Err(Failure::Busy(Busy::Paused { retry_after }))) if retry_after == COOLDOWN
            ));
        }
        let during = offered(
            &mut queue,
            &status,
            200,
            paused_at + Duration::from_secs(59),
        );
        assert!(matches!(
            during.try_recv(),
            Ok(Err(Failure::Busy(Busy::Paused { retry_after }))) if retry_after == Duration::from_secs(1)
        ));
        assert!(queue.is_empty());
        let after = paused_at + COOLDOWN;
        let resumed = offered(&mut queue, &status, 201, after);
        assert!(resumed.try_recv().is_err(), "the next request is admitted");
        assert!(!queue.circuit.paused(), "that request resumed admission");
        assert!(queue.circuit.permits([201; 32], false, after));
        assert!(queue.next(|_| false, &status).is_some());
        assert_eq!(status.pending.load(Ordering::Acquire), 0);
    }
    #[test]
    fn queue_over_its_bound_refuses_typed_and_waits_are_bounded() {
        let start = Instant::now();
        let status = Status::default();
        let mut queue = Queue::new(start);
        let held: Vec<_> = (0..PENDING_PROBES)
            .map(|n| offered(&mut queue, &status, u8::try_from(n).unwrap(), start))
            .collect();
        let over = offered(&mut queue, &status, 250, start);
        assert!(matches!(
            over.try_recv(),
            Ok(Err(Failure::Busy(Busy::QueueFull)))
        ));
        queue.expire(
            &status,
            start + QUEUE_WAIT.checked_sub(Duration::from_millis(1)).unwrap(),
        );
        assert_eq!(queue.pending.len(), PENDING_PROBES);
        queue.expire(&status, start + QUEUE_WAIT);
        assert!(queue.is_empty());
        assert_eq!(status.pending.load(Ordering::Acquire), 0);
        for receiver in held {
            assert!(matches!(
                receiver.try_recv(),
                Ok(Err(Failure::Busy(Busy::QueueWait)))
            ));
        }
    }
    #[test]
    fn crash_and_protocol_quarantine_require_explicit_retry() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        let source = [1; 32];
        let crash = Health::Unhealthy(Error::Crashed);
        assert!(circuit.permits(source, false, now));
        assert!(circuit.record(source, crash, now).retry);
        let later = now + Duration::from_secs(1);
        assert!(circuit.permits(source, false, later));
        assert!(!circuit.record(source, crash, later).retry);
        assert!(!circuit.permits(source, false, now + Duration::from_secs(2)));
        assert!(circuit.permits(source, true, now + Duration::from_secs(2)));
        let protocol = Health::Unhealthy(Error::ProtocolViolation);
        assert!(
            !circuit
                .record(source, protocol, now + Duration::from_secs(3))
                .retry
        );
        assert!(!circuit.permits(source, false, now + Duration::from_secs(4)));
        assert!(circuit.permits(source, true, now + Duration::from_secs(4)));
        assert!(!circuit.permits(source, true, now + Duration::from_millis(4500)));
    }
    #[test]
    fn neutral_outcomes_spend_nothing() {
        let now = Instant::now();
        let mut circuit = Circuit::new(now);
        for outcome in [
            Err(Failure::Sandbox(Error::Cancelled)),
            Err(Failure::Authority(
                kinewright_core::MediaError::MediaReadTemporarilyUnavailable,
            )),
            Err(Failure::Busy(Busy::QueueFull)),
        ] {
            for n in 0..10 {
                assert_eq!(
                    circuit.record([n; 32], Health::of(&outcome), now),
                    Recorded::default()
                );
            }
        }
        assert!(!circuit.paused());
    }
    fn fixture_probe() -> kinewright_media_ipc::ValidatedProbe {
        use kinewright_core::{
            ColorBitDepth, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
            ColorWhitePoint,
        };
        use kinewright_media_ipc::{Color, Kind, ProbeResult, Stream, StreamKind};
        kinewright_media_ipc::ValidatedProbe::new(ProbeResult {
            streams: vec![Stream {
                index: 0,
                kind: StreamKind::Video,
                time_base: [1, 25],
                timestamp_origin: 0,
                duration: 1,
                average_frame_rate: [25, 1],
            }],
            duration_microseconds: 40_000,
            video_stream: Some(0),
            audio_stream: None,
            duration_frames: 1,
            fps: [30, 1],
            kind: Kind::Image,
            resolution: Some([16, 16]),
            color: Color {
                primaries: ColorPrimaries::Unknown,
                transfer: ColorTransfer::Unknown,
                matrix: ColorMatrix::Unknown,
                range: ColorRange::Unknown,
                white_point: ColorWhitePoint::Unknown,
                bit_depth: ColorBitDepth::Unknown,
                confidence_basis_points: 0,
                provenance: ColorProvenance::Unknown,
            },
        })
        .unwrap()
    }
}
