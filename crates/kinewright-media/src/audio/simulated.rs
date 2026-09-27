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

use super::{render_output, short_frames};

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
    underrun_frames: AtomicU64,
    frozen_until: Mutex<Option<Instant>>,
}

struct Stream {
    id: u64,
    consumer: Consumer<f32>,
    position: Arc<AtomicU64>,
    gain: Arc<AtomicI32>,
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
    pub(crate) fn advance(&self, frames: usize) -> bool {
        let mut stream = lock(&self.0.stream);
        let Some(stream) = stream.as_mut().filter(|stream| stream.playing) else {
            return false;
        };
        let channels = usize::from(CHANNELS);
        let short = short_frames(&stream.consumer, frames, channels);
        self.0.underrun_frames.fetch_add(short, Ordering::Relaxed);
        let mut output = vec![0.0_f32; frames * channels];
        let gain = stream.gain.load(Ordering::Relaxed);
        render_output(
            &mut stream.consumer,
            &mut output,
            channels,
            &stream.position,
            gain,
        );
        true
    }

    /// Frames callbacks found missing from the ring (V-1's `underrun_frames`).
    pub(crate) fn underrun_frames(&self) -> u64 {
        self.0.underrun_frames.load(Ordering::Relaxed)
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
    ) -> SimulatedOutput {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        *lock(&self.0.stream) = Some(Stream {
            id,
            consumer,
            position,
            gain,
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

    /// Absolute deadlines, so the mean rate is exact; re-anchored whenever
    /// no callback ran (paused, frozen), so a resume never bursts.
    fn pace(&self, stop: &AtomicBool) {
        let mut next = Instant::now();
        while !stop.load(Ordering::Acquire) {
            next += PERIOD;
            thread::sleep(next.saturating_duration_since(Instant::now()));
            let frozen = lock(&self.0.frozen_until).is_some_and(|until| Instant::now() < until);
            if frozen || !self.advance(CALLBACK_FRAMES) {
                next = Instant::now();
            }
        }
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

    use super::*;

    #[test]
    fn stepped_callbacks_pop_through_render_output_and_count_underruns() {
        let audio = SimulatedAudio::stepped();
        let (mut producer, consumer) = RingBuffer::new(8_192);
        let position = Arc::new(AtomicU64::new(100));
        let output = audio.attach(consumer, Arc::clone(&position), Arc::default());
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
            (position.load(Ordering::Acquire), audio.underrun_frames()),
            (1_124, 0)
        );
        // Today's clock advances through an underrun (evidence T7); V-1 fixes
        // that in S1, while the driver already counts it: 1 whole frame left.
        assert!(audio.advance(CALLBACK_FRAMES));
        assert_eq!(
            (position.load(Ordering::Acquire), audio.underrun_frames()),
            (2_148, 1_023)
        );
        drop(output);
        assert!(!audio.advance(CALLBACK_FRAMES), "a dropped output detaches");
    }
}
