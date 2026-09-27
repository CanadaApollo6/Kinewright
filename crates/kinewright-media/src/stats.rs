//! PF1 R-5: playback health, one leaf lock shared by the engine, the worker
//! and the preview thread (H-4: nothing else is taken while it is held).

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use kinewright_core::PlaybackStats;

/// R-5's allowance for the ack's later root epoch (one 60 Hz pass).
const EPOCH_MS: f64 = 16.7;
/// A due frame unacked this long stays dropped (bounds the queue).
const DUE_WINDOW: Duration = Duration::from_secs(2);

/// The counters since the latest explicit `play` (a seek while playing only
/// re-anchors them).
pub(crate) struct Counters {
    pub(crate) stats: PlaybackStats,
    /// Diagnostics' (events, frames) when the counters were cleared.
    pub(crate) underrun_base: (u64, u64),
    playing: bool,
    frame_ms: f64,
    /// Due frames awaiting an ack, with the instant the clock reached them.
    due: VecDeque<(i64, Instant)>,
    next_due: i64,
    /// The programme's end (its duration in frames): the clock reaches it,
    /// but no frame there is due.
    end_frame: i64,
    /// The latest ack of a newer frame; held age runs from it.
    shown: (Instant, i64),
    /// The clock's latest change; a stall runs from it.
    moved: (Instant, i64),
}

impl Default for Counters {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            stats: PlaybackStats::default(),
            underrun_base: (0, 0),
            playing: false,
            frame_ms: 0.0,
            due: VecDeque::new(),
            next_due: 0,
            end_frame: i64::MAX,
            shown: (now, -1),
            moved: (now, 0),
        }
    }
}

impl Counters {
    /// An explicit `play`: fresh counters from the diagnostics' `underruns`.
    pub(crate) fn clear(&mut self, underruns: (u64, u64)) {
        (self.stats, self.underrun_base) = (PlaybackStats::default(), underruns);
    }

    /// Playback (re)started at `position` of a programme `end_frame` long.
    pub(crate) fn begin(&mut self, now: Instant, position: i64, frame_ms: f64, end_frame: i64) {
        (self.playing, self.frame_ms, self.end_frame) = (true, frame_ms, end_frame);
        self.due.clear();
        self.next_due = position;
        (self.shown, self.moved) = ((now, position - 1), (now, position));
    }

    pub(crate) fn end(&mut self) {
        self.playing = false;
        self.due.clear();
    }

    /// Each worker tick (5 ms) while playing: newly due frames, clock stall
    /// and held age.
    pub(crate) fn sample(&mut self, now: Instant, position: i64) {
        if !self.playing {
            return;
        }
        while self.next_due <= position && self.next_due < self.end_frame {
            self.due.push_back((self.next_due, now));
            self.next_due += 1;
            self.stats.due_frames += 1;
        }
        while (self.due.front()).is_some_and(|due| now.duration_since(due.1) > DUE_WINDOW) {
            self.due.pop_front();
        }
        if position != self.moved.1 {
            self.moved = (now, position);
        }
        let ms = |since: Instant| now.duration_since(since).as_secs_f64() * 1e3;
        let stats = &mut self.stats;
        stats.max_clock_stall_ms = stats.max_clock_stall_ms.max(ms(self.moved.0));
        stats.max_held_ms = stats.max_held_ms.max(ms(self.shown.0));
        stats.dropped = stats.due_frames - stats.on_time - stats.late;
    }

    /// An ack of the frame at `at` while playing: counted once per due frame,
    /// on time within due + 1 frame + 1 epoch, else late.
    pub(crate) fn ack(&mut self, now: Instant, position: i64, at: i64) {
        if !self.playing {
            return;
        }
        self.sample(now, position);
        if let Some(index) = self.due.iter().position(|due| due.0 == at) {
            let waited = now.duration_since(self.due[index].1).as_secs_f64() * 1e3;
            self.due.remove(index);
            let stats = &mut self.stats;
            if waited <= self.frame_ms + EPOCH_MS {
                stats.on_time += 1;
            } else {
                stats.late += 1;
            }
            stats.dropped = stats.due_frames - stats.on_time - stats.late;
            let frames = u32::try_from(position.abs_diff(at)).map_or(f64::MAX, f64::from);
            let offset = frames * self.frame_ms;
            stats.max_av_offset_ms = stats.max_av_offset_ms.max(offset);
        }
        if at > self.shown.1 {
            self.shown = (now, at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-5 with an injected clock: outcomes per due frame, one count per
    /// frame, held age from the latest newer ack, and the clock stall.
    #[test]
    fn due_frames_are_counted_once_by_their_ack() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.ack(at(0), 0, 0);
        assert_eq!(counters.stats, PlaybackStats::default(), "paused: ignored");
        counters.begin(at(0), 0, 33.0, 5);
        counters.sample(at(5), 1);
        counters.ack(at(40), 1, 0);
        counters.ack(at(41), 1, 0);
        counters.ack(at(100), 1, 1);
        counters.sample(at(300), 1);
        counters.ack(at(301), 4, 3);
        let stats = &counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.late, stats.dropped);
        assert_eq!(outcomes, (5, 2, 1, 2));
        assert!((stats.max_clock_stall_ms - 295.0).abs() < 1.0, "{stats:?}");
        assert!((stats.max_held_ms - 201.0).abs() < 1.0, "{stats:?}");
        assert!((stats.max_av_offset_ms - 33.0).abs() < 0.1, "{stats:?}");
        counters.sample(at(2_400), 4);
        counters.ack(at(2_401), 4, 2);
        assert_eq!(counters.stats.late, 1, "past the window: stays dropped");
        counters.sample(at(2_402), 5);
        assert_eq!(counters.stats.due_frames, 5, "the end is not a due frame");
    }
}
