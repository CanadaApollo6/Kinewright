//! PF1 V-5: the device-free audio output. It pops the ring through the
//! production [`render_output`] in 1,024-frame, 48 kHz stereo callbacks,
//! stepped by `SimulatedAudio::advance` (CI) or paced in real time (P-play).

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rtrb::Consumer;

use super::{AudioDiagnostics, render_output};

pub(crate) const RATE: u32 = 48_000;
pub(crate) const CHANNELS: u16 = 2;
pub(crate) const CALLBACK_FRAMES: usize = 1_024;
/// One callback: 1,024 frames at 48 kHz.
const PERIOD: Duration = Duration::from_nanos(21_333_333);

/// The harness's handle, passed to the engine option: it reaches whichever
/// stream the worker has open.
#[derive(Clone, Default)]
pub(crate) struct SimulatedAudio(Arc<Shared>);

#[derive(Default)]
struct Shared {
    paced: bool,
    stream: Mutex<Option<Stream>>,
    next_id: AtomicU64,
    /// Callback deadlines the paced driver woke too late to meet (A-F2).
    missed_periods: AtomicU64,
    frozen_until: Mutex<Option<Instant>>,
}

struct Stream {
    id: u64,
    consumer: Consumer<f32>,
    position: Arc<AtomicU64>,
    gain: Arc<AtomicI32>,
    diagnostics: Arc<AudioDiagnostics>,
    playing: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SimulatedAudio {
    /// Callbacks run only when the test calls [`Self::advance`].
    #[cfg(test)]
    pub(crate) fn stepped() -> Self {
        Self::default()
    }

    /// Callbacks run on their own thread at the real-time rate.
    pub(crate) fn paced() -> Self {
        Self(Arc::new(Shared {
            paced: true,
            ..Shared::default()
        }))
    }

    /// One callback of `frames` if a playing stream is open; `false` if none.
    /// Its failed pops are recorded where they happen (R22).
    #[cfg(test)]
    pub(crate) fn advance(&self, frames: usize) -> bool {
        self.callback(frames, None)
    }

    /// One callback, under the stream lock. With a `pacer`, it is stamped as
    /// it executes (R25/D1): immediately before the consume, its misses are
    /// recorded before the clock advances (D2); immediately after it, a
    /// consume that was itself suspended for a period counts as well.
    fn callback(
        &self,
        frames: usize,
        mut pacer: Option<(&mut Pacer, &mut dyn FnMut() -> Instant)>,
    ) -> bool {
        let mut stream = lock(&self.0.stream);
        let Some(stream) = stream.as_mut().filter(|stream| stream.playing) else {
            return false;
        };
        if let Some((pacer, clock)) = pacer.as_mut() {
            let missed = pacer.start(clock());
            self.0.missed_periods.fetch_add(missed, Ordering::Relaxed);
        }
        let channels = usize::from(CHANNELS);
        let mut output = vec![0.0_f32; frames * channels];
        let gain = stream.gain.load(Ordering::Relaxed);
        render_output(
            &mut stream.consumer,
            &mut output,
            channels,
            &stream.position,
            gain,
            &stream.diagnostics,
        );
        if let Some((pacer, clock)) = pacer.as_mut() {
            let missed = pacer.finish(clock());
            self.0.missed_periods.fetch_add(missed, Ordering::Relaxed);
        }
        true
    }

    /// Deadlines the paced driver missed; a timing run with any is invalid.
    /// Read under the stream lock, so it includes every completed callback.
    #[cfg(test)]
    pub(crate) fn missed_periods(&self) -> u64 {
        let _completed = lock(&self.0.stream);
        self.0.missed_periods.load(Ordering::Relaxed)
    }

    /// Q-3 clock freeze: the paced driver runs no callback for `duration`.
    #[cfg(test)]
    pub(crate) fn freeze(&self, duration: Duration) {
        *lock(&self.0.frozen_until) = Some(Instant::now() + duration);
    }

    pub(super) fn attach(
        &self,
        consumer: Consumer<f32>,
        position: Arc<AtomicU64>,
        gain: Arc<AtomicI32>,
        diagnostics: Arc<AudioDiagnostics>,
    ) -> SimulatedOutput {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        *lock(&self.0.stream) = Some(Stream {
            id,
            consumer,
            position,
            gain,
            diagnostics,
            playing: false,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let pacer = self.0.paced.then(|| {
            let (audio, stop) = (self.clone(), Arc::clone(&stop));
            thread::spawn(move || audio.pace(&stop))
        });
        SimulatedOutput {
            audio: self.clone(),
            id,
            stop,
            pacer,
        }
    }

    /// One callback per deadline. A late callback runs once, never a
    /// catch-up burst against audio refilled meanwhile, and counts the
    /// deadlines it missed (A-F2, R25/D1). Paused and frozen spans re-anchor.
    fn pace(&self, stop: &AtomicBool) {
        let mut pacer = Pacer::new(Instant::now());
        while !stop.load(Ordering::Acquire) {
            thread::sleep(pacer.wait(Instant::now()));
            let now = Instant::now();
            let frozen = lock(&self.0.frozen_until).is_some_and(|until| now < until);
            if frozen {
                pacer.reanchor(now);
                continue;
            }
            if !self.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut Instant::now))) {
                pacer.reanchor(Instant::now());
            }
        }
    }
}

/// The paced driver's deadlines, apart from the clock so a test can drive it.
/// Every stamp is taken where the callback actually executes (R25/D1), and
/// each callback is accounted against its own scheduled deadline through
/// completion (R27): how late it is comes from the schedule, not from the
/// spans between stamps, so delays split across the wake, the start and the
/// consume add up.
struct Pacer {
    /// The deadline the next callback serves.
    next: Instant,
    /// The deadline the running callback serves.
    serving: Instant,
    /// Misses already reported for the running callback.
    reported: u64,
}

impl Pacer {
    fn new(now: Instant) -> Self {
        Self {
            next: now + PERIOD,
            serving: now + PERIOD,
            reported: 0,
        }
    }

    /// How long to sleep before the next deadline.
    fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }

    /// A callback starts consuming at `now`, serving the next deadline.
    /// Returns the whole periods it is already past that deadline (missed),
    /// reported before the consume advances the clock (R25/D2).
    fn start(&mut self, now: Instant) -> u64 {
        self.serving = self.next;
        self.reported = periods(now.saturating_duration_since(self.serving));
        self.reported
    }

    /// The callback finished consuming at `now`. Returns the further whole
    /// periods its completion is past its deadline (so the total is measured
    /// from the schedule), then schedules the next deadline: on time, one
    /// period on; after any miss, re-anchored on `now`, so nothing is caught
    /// up.
    fn finish(&mut self, now: Instant) -> u64 {
        let missed = periods(now.saturating_duration_since(self.serving));
        self.next = if missed == 0 {
            self.serving + PERIOD
        } else {
            now + PERIOD
        };
        missed.saturating_sub(self.reported)
    }

    fn reanchor(&mut self, now: Instant) {
        self.next = now + PERIOD;
    }
}

/// Whole callback periods in `span`.
fn periods(span: Duration) -> u64 {
    u64::try_from(span.as_nanos() / PERIOD.as_nanos()).unwrap_or(u64::MAX)
}

/// The runtime's end of the output; dropping it detaches the stream.
pub(crate) struct SimulatedOutput {
    audio: SimulatedAudio,
    id: u64,
    stop: Arc<AtomicBool>,
    pacer: Option<JoinHandle<()>>,
}

impl SimulatedOutput {
    pub(super) fn set_playing(&self, playing: bool) {
        let mut stream = lock(&self.audio.0.stream);
        if let Some(stream) = stream.as_mut().filter(|stream| stream.id == self.id) {
            stream.playing = playing;
        }
    }
}

impl Drop for SimulatedOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(pacer) = self.pacer.take() {
            let _ = pacer.join();
        }
        let mut stream = lock(&self.audio.0.stream);
        if stream.as_ref().is_some_and(|stream| stream.id == self.id) {
            *stream = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::{super::BUFFER_SECONDS, super::LIVE_FILL_MILLISECONDS, *};

    fn stepped_stream(capacity: usize) -> (SimulatedAudio, rtrb::Producer<f32>, SimulatedOutput) {
        let audio = SimulatedAudio::stepped();
        let (producer, consumer) = RingBuffer::new(capacity);
        let diagnostics = Arc::new(AudioDiagnostics::default());
        let output = audio.attach(consumer, Arc::default(), Arc::default(), diagnostics);
        (audio, producer, output)
    }

    fn underruns(audio: &SimulatedAudio) -> u64 {
        let stream = lock(&audio.0.stream);
        stream
            .as_ref()
            .expect("attached")
            .diagnostics
            .underrun_frames()
    }

    #[test]
    fn stepped_callbacks_pop_through_render_output_and_count_underruns() {
        let audio = SimulatedAudio::stepped();
        let (mut producer, consumer) = RingBuffer::new(8_192);
        let position = Arc::new(AtomicU64::new(100));
        let diagnostics = Arc::new(AudioDiagnostics::default());
        let output = audio.attach(
            consumer,
            Arc::clone(&position),
            Arc::default(),
            Arc::clone(&diagnostics),
        );
        assert!(
            !audio.advance(CALLBACK_FRAMES),
            "a paused stream runs no callback"
        );
        output.set_playing(true);
        for _ in 0..CALLBACK_FRAMES * 2 + 3 {
            producer.push(0.5).unwrap();
        }
        assert!(audio.advance(CALLBACK_FRAMES));
        assert_eq!(
            (
                position.load(Ordering::Acquire),
                diagnostics.underrun_frames()
            ),
            (1_124, 0)
        );
        // V-1: 3 samples were left; one whole frame pops, the clock counts it,
        // and 1,023 frames underrun (evidence T7 had the clock run through).
        assert!(audio.advance(CALLBACK_FRAMES));
        assert_eq!(
            (
                position.load(Ordering::Acquire),
                diagnostics.underrun_frames()
            ),
            (1_125, 1_023)
        );
        drop(output);
        assert!(!audio.advance(CALLBACK_FRAMES), "a dropped output detaches");
    }

    /// R22 / B-F1: a producer refilling while callbacks run. However the two
    /// threads interleave, the failed pops `render_output` reports are exactly
    /// the requested samples the producer never supplied; an availability
    /// snapshot taken before the pops would overstate them.
    #[test]
    fn failed_pops_are_exact_under_a_concurrent_refill() {
        let (mut producer, mut consumer) = RingBuffer::new(8_192);
        let pushed = 400_000_usize;
        let refill = thread::spawn(move || {
            for chunk in 0..pushed / 500 {
                let value = f32::from(u8::try_from(chunk % 7).expect("small"));
                while producer.push_entire_slice(&[value; 500]).is_err() {
                    thread::yield_now();
                }
                thread::yield_now();
            }
        });
        let (position, mut output) = (AtomicU64::new(0), vec![0.0_f32; 2 * CALLBACK_FRAMES]);
        let diagnostics = AudioDiagnostics::default();
        let (mut requested, mut failed) = (0, 0);
        let mut callback = |consumer: &mut Consumer<f32>| {
            failed += render_output(consumer, &mut output, 2, &position, 0, &diagnostics);
            requested += 2 * CALLBACK_FRAMES;
        };
        while !refill.is_finished() {
            callback(&mut consumer);
        }
        refill.join().expect("the refill thread");
        while !consumer.is_empty() {
            callback(&mut consumer);
        }
        assert_eq!(failed, requested - pushed);
    }

    /// A-F6: matched producer schedules through the simulated consumer. The
    /// producer tops the ring up to the live fill target every 5 ms, as
    /// `fill_ring` does, except over a 2 s stall; callbacks run on time.
    #[test]
    fn a_two_second_fill_stall_underruns_where_the_clean_schedule_does_not() {
        let schedule = |stall: Option<(f64, f64)>| {
            let channels = usize::from(CHANNELS);
            let capacity = RATE as usize * channels * BUFFER_SECONDS;
            let target = RATE as usize * channels * LIVE_FILL_MILLISECONDS / 1_000;
            let (audio, mut producer, output) = stepped_stream(capacity);
            output.set_playing(true);
            let callback_ms = PERIOD.as_secs_f64() * 1e3;
            let (mut now, mut callback_at) = (0.0, 0.0);
            while now < 6_000.0 {
                if stall.is_none_or(|(from, len)| !(from..from + len).contains(&now)) {
                    let queued = capacity - producer.slots();
                    let top_up = vec![0.1_f32; target.saturating_sub(queued)];
                    producer.push_entire_slice(&top_up).expect("room to top up");
                }
                while callback_at <= now {
                    assert!(audio.advance(CALLBACK_FRAMES));
                    callback_at += callback_ms;
                }
                now += 5.0;
            }
            underruns(&audio)
        };
        assert_eq!(schedule(None), 0, "the clean schedule never underruns");
        // 2 s without a fill against a 1 s cushion: about 1 s of underrun.
        let stalled = schedule(Some((2_000.0, 2_000.0)));
        assert!((40_000..=50_000).contains(&stalled), "{stalled}");
    }

    /// A-F2: a late callback serves one deadline, counts the ones it missed
    /// and re-anchors, so it never bursts through audio refilled meanwhile.
    #[test]
    fn the_pacer_counts_missed_deadlines_and_never_bursts() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        assert_eq!(pacer.start(start + PERIOD), 0, "on time");
        assert_eq!(pacer.finish(start + PERIOD), 0);
        assert_eq!(
            pacer.wait(start + PERIOD),
            PERIOD,
            "the next deadline is one period on"
        );
        let half_late = start + 2 * PERIOD + PERIOD / 2;
        assert_eq!(
            pacer.start(half_late),
            0,
            "half a period late still meets it"
        );
        assert_eq!(pacer.finish(half_late), 0);
        assert_eq!(
            pacer.wait(half_late),
            PERIOD.saturating_sub(PERIOD / 2),
            "and keeps the schedule"
        );
        let late = start + 6 * PERIOD + PERIOD / 2;
        assert_eq!(pacer.start(late), 3, "three later deadlines passed");
        assert_eq!(pacer.finish(late), 0, "reported once");
        assert_eq!(pacer.wait(late), PERIOD, "re-anchored, not a burst");
    }

    /// R25/D1: the reviewer's schedule. The driver wakes on time, is suspended
    /// for 32 ms before it consumes, and the producer refills the starved ring
    /// meanwhile. The refill hides the starvation from the underrun count, so
    /// the miss must be counted, where the callback executes, before its
    /// clock advance, and must invalidate the run; nothing is caught up. A
    /// suspension inside the consume counts the same way.
    #[test]
    fn a_suspension_with_a_refill_is_a_missed_deadline_that_invalidates_the_run() {
        let stall = Duration::from_millis(32);
        let (audio, mut producer, output) = stepped_stream(1 << 16);
        output.set_playing(true);
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        let samples = 2 * CALLBACK_FRAMES;
        producer
            .push_entire_slice(&vec![0.1; samples])
            .expect("room");
        let on_time = t0 + PERIOD;
        let mut stamps = [on_time, on_time].into_iter();
        let mut clock = || stamps.next().expect("two stamps");
        assert!(audio.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut clock))));
        assert_eq!((audio.missed_periods(), underruns(&audio)), (0, 0));
        // The ring is now empty. The next deadline passes during the stall,
        // while the producer refills.
        let deadline = on_time + PERIOD;
        producer
            .push_entire_slice(&vec![0.1; samples])
            .expect("room");
        let executes = deadline + stall;
        let mut stamps = [executes, executes].into_iter();
        let mut clock = || stamps.next().expect("two stamps");
        assert!(audio.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut clock))));
        assert_eq!(underruns(&audio), 0, "the refill hid the starvation");
        let missed = audio.missed_periods();
        assert_eq!(missed, 1, "the stall is a missed deadline");
        assert_eq!(pacer.wait(executes), PERIOD, "no catch-up after it");
        let nominal = 60_000.0;
        assert!(crate::pf1_harness::q2_valid(nominal, nominal, 0));
        assert!(!crate::pf1_harness::q2_valid(nominal, nominal, missed));
        // Suspended inside the consume: on time in, 32 ms out.
        let begins = executes + PERIOD;
        producer
            .push_entire_slice(&vec![0.1; samples])
            .expect("room");
        let mut stamps = [begins, begins + stall].into_iter();
        let mut clock = || stamps.next().expect("two stamps");
        assert!(audio.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut clock))));
        assert_eq!(audio.missed_periods(), 2);
        assert_eq!(pacer.wait(begins + stall), PERIOD, "re-anchored after it");
    }

    /// R27: the re-review's split suspension. After an on-time callback at
    /// 21.333 ms the next deadline is 42.667 ms; the driver is suspended
    /// 16 ms before it starts (58.667) and 16 ms more before it finishes
    /// (74.667), while the producer refills the starved ring. Neither span
    /// is a period, but the completion is 32 ms past the scheduled deadline:
    /// one miss, re-anchored, no immediate catch-up callback, and the run is
    /// invalid.
    #[test]
    fn a_split_suspension_with_a_refill_is_a_missed_deadline_that_invalidates_the_run() {
        let split = Duration::from_millis(16);
        let (audio, mut producer, output) = stepped_stream(1 << 16);
        output.set_playing(true);
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        let samples = 2 * CALLBACK_FRAMES;
        producer
            .push_entire_slice(&vec![0.1; samples])
            .expect("room");
        let on_time = t0 + PERIOD;
        let mut stamps = [on_time, on_time].into_iter();
        let mut clock = || stamps.next().expect("two stamps");
        assert!(audio.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut clock))));
        assert_eq!((audio.missed_periods(), underruns(&audio)), (0, 0));
        // The ring is now empty; it is refilled during the suspension.
        producer
            .push_entire_slice(&vec![0.1; samples])
            .expect("room");
        let deadline = on_time + PERIOD;
        let (starts, finishes) = (deadline + split, deadline + split + split);
        let mut stamps = [starts, finishes].into_iter();
        let mut clock = || stamps.next().expect("two stamps");
        assert!(audio.callback(CALLBACK_FRAMES, Some((&mut pacer, &mut clock))));
        assert_eq!(underruns(&audio), 0, "the refill hid the starvation");
        let missed = audio.missed_periods();
        assert_eq!(missed, 1, "32 ms past the deadline through completion");
        assert_eq!(
            pacer.wait(finishes),
            PERIOD,
            "re-anchored: no immediate catch-up callback"
        );
        let nominal = 60_000.0;
        assert!(!crate::pf1_harness::q2_valid(nominal, nominal, missed));
    }
}
