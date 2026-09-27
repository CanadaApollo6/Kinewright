//! PF1 V-5: the device-free audio output. It pops the ring through the
//! production [`render_output`] in 1,024-frame, 48 kHz stereo callbacks,
//! stepped by [`SimulatedAudio::advance`] (CI) or paced in real time (P-play).

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
    pub(crate) fn advance(&self, frames: usize) -> bool {
        let mut stream = lock(&self.0.stream);
        let Some(stream) = stream.as_mut().filter(|stream| stream.playing) else {
            return false;
        };
        let channels = usize::from(CHANNELS);
        let mut output = vec![0.0_f32; frames * channels];
        let gain = stream.gain.load(Ordering::Relaxed);
        let failed = render_output(
            &mut stream.consumer,
            &mut output,
            channels,
            &stream.position,
            gain,
        );
        stream.diagnostics.record_underrun(failed, channels);
        true
    }

    /// Deadlines the paced driver missed; a timing run with any is invalid.
    pub(crate) fn missed_periods(&self) -> u64 {
        self.0.missed_periods.load(Ordering::Relaxed)
    }

    /// Q-3 clock freeze: the paced driver runs no callback for `duration`.
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

    /// One callback per deadline. A late wake runs one callback, never a
    /// catch-up burst against audio refilled meanwhile, and counts the
    /// deadlines it missed (A-F2). Paused and frozen spans re-anchor.
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
            let missed = pacer.woke(now);
            if self.advance(CALLBACK_FRAMES) {
                self.0.missed_periods.fetch_add(missed, Ordering::Relaxed);
            } else {
                pacer.reanchor(now);
            }
        }
    }
}

/// The paced driver's deadlines, apart from the clock so a test can drive it.
struct Pacer {
    next: Instant,
}

impl Pacer {
    fn new(now: Instant) -> Self {
        Self { next: now + PERIOD }
    }

    /// How long to sleep before the next deadline.
    fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }

    /// A wake at `now` serves one deadline; returns how many later deadlines
    /// had also passed (missed), re-anchoring after a miss.
    fn woke(&mut self, now: Instant) -> u64 {
        let late = now.saturating_duration_since(self.next).as_nanos();
        let missed = u64::try_from(late / PERIOD.as_nanos()).unwrap_or(u64::MAX);
        self.next = if missed == 0 {
            self.next + PERIOD
        } else {
            now + PERIOD
        };
        missed
    }

    fn reanchor(&mut self, now: Instant) {
        self.next = now + PERIOD;
    }
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
        // Today's clock advances through an underrun (evidence T7); V-1 fixes
        // that in S1. 3 samples were left: 2,045 failed pops, 1,023 frames.
        assert!(audio.advance(CALLBACK_FRAMES));
        assert_eq!(
            (
                position.load(Ordering::Acquire),
                diagnostics.underrun_frames()
            ),
            (2_148, 1_023)
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
        let (mut requested, mut failed) = (0, 0);
        let mut callback = |consumer: &mut Consumer<f32>| {
            failed += render_output(consumer, &mut output, 2, &position, 0);
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

    /// A-F2: a late wake serves one deadline, counts the ones it missed and
    /// re-anchors, so it never bursts through audio refilled meanwhile.
    #[test]
    fn the_pacer_counts_missed_deadlines_and_never_bursts() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        assert_eq!(pacer.woke(start + PERIOD), 0, "on time");
        assert_eq!(
            pacer.wait(start + PERIOD),
            PERIOD,
            "the next deadline is one period on"
        );
        assert_eq!(
            pacer.woke(start + 2 * PERIOD + PERIOD / 2),
            0,
            "half a period late still meets it"
        );
        let late = start + 6 * PERIOD + PERIOD / 2;
        assert_eq!(pacer.woke(late), 3, "three later deadlines passed");
        assert_eq!(pacer.wait(late), PERIOD, "re-anchored, not a burst");
    }
}
