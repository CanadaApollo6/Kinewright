//! PF1 R-5: playback health, one leaf lock shared by the engine, the worker
//! and the preview thread (H-4: nothing else is taken while it is held).

use std::{collections::BTreeMap, time::Instant};

use kinewright_core::PlaybackStats;

/// E11.8 amendment (R32): pending due-frame records are bounded by count,
/// not age. 65,536 records is over 18 minutes of unacknowledged frames at
/// 60 fps (about 3 MB at worst); only then is the oldest record evicted,
/// and an evicted frame acknowledged later still counts late, never
/// dropped.
pub(crate) const DUE_RECORDS: usize = 65_536;

/// A due frame's key: its playback epoch and frame. Keys only increase:
/// a playback epoch never re-registers a frame, and a later epoch is newer.
type DueKey = (u64, i64);

/// The counters since the latest explicit `play` (a seek while playing only
/// re-anchors them).
pub(crate) struct Counters {
    pub(crate) stats: PlaybackStats,
    /// Diagnostics' (events, frames, post-end events, post-end frames) when
    /// the counters were cleared.
    pub(crate) underrun_base: [u64; 4],
    playing: bool,
    frame_ms: f64,
    /// The applied playback epoch the due frames below are registered in.
    epoch: u64,
    /// Due frames awaiting an ack, with the instant the clock reached them.
    /// They outlive a stop (review A F5): an image painted before it can be
    /// acknowledged after it.
    due: BTreeMap<DueKey, Instant>,
    /// Records evicted by `DUE_RECORDS` and not acknowledged since, and the
    /// newest evicted key.
    evicted: (u64, Option<DueKey>),
    next_due: i64,
    /// The programme's end (its duration in frames): the clock reaches it,
    /// but no frame there is due.
    end_frame: i64,
    /// When the image on screen was painted, and its frame; held age runs
    /// from the latest paint of a newer frame.
    shown: (Instant, i64),
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            stats: PlaybackStats::default(),
            underrun_base: [0; 4],
            playing: false,
            frame_ms: 0.0,
            epoch: 0,
            due: BTreeMap::new(),
            evicted: (0, None),
            next_due: 0,
            end_frame: i64::MAX,
            shown: (Instant::now(), -1),
        }
    }
}

impl Counters {
    /// An explicit `play`: fresh counters from the diagnostics' `underruns`.
    /// Frames of the playback before it no longer count.
    pub(crate) fn clear(&mut self, underruns: [u64; 4]) {
        (self.stats, self.underrun_base) = (PlaybackStats::default(), underruns);
        self.due.clear();
        self.evicted = (0, None);
    }

    /// Playback (re)started at `position` of a programme `end_frame` long,
    /// in playback epoch `epoch`: the starting frame is due at once (review
    /// A F5).
    pub(crate) fn begin(
        &mut self,
        now: Instant,
        position: i64,
        frame_ms: f64,
        end_frame: i64,
        epoch: u64,
    ) {
        (self.playing, self.frame_ms, self.end_frame) = (true, frame_ms, end_frame);
        (self.epoch, self.next_due) = (epoch, position);
        self.shown = (now, position - 1);
        self.register(now, position);
    }

    /// Playback stopped with the clock at `through`: the frames it reached
    /// since the last sample are due (the terminal stop passes the last
    /// frame). Pending records stay, so a later ack still counts.
    pub(crate) fn end(&mut self, now: Instant, through: i64) {
        if self.playing {
            self.sample(now, through);
            self.playing = false;
        }
    }

    fn register(&mut self, now: Instant, position: i64) {
        while self.next_due <= position && self.next_due < self.end_frame {
            self.due.insert((self.epoch, self.next_due), now);
            self.next_due += 1;
            self.stats.due_frames += 1;
            if self.due.len() > DUE_RECORDS
                && let Some((key, _)) = self.due.pop_first()
            {
                self.evicted = (self.evicted.0 + 1, Some(key));
            }
        }
        self.settle();
    }

    fn settle(&mut self) {
        let stats = &mut self.stats;
        stats.dropped = (stats.due_frames).saturating_sub(stats.on_time + stats.late);
    }

    /// Each worker tick (5 ms) while playing, and each ack: newly due frames
    /// and held age.
    pub(crate) fn sample(&mut self, now: Instant, position: i64) {
        if !self.playing {
            return;
        }
        self.register(now, position);
        let held = now.saturating_duration_since(self.shown.0).as_secs_f64() * 1e3;
        self.stats.max_held_ms = self.stats.max_held_ms.max(held);
    }

    /// The image of frame `at` in playback epoch `epoch`, painted at
    /// `painted`, acknowledged with the clock at `position` (R-5): counted
    /// once per due frame, on time if painted within due + 1 frame, else
    /// late. A frame acknowledged after its record was evicted is late.
    pub(crate) fn ack(
        &mut self,
        now: Instant,
        painted: Instant,
        epoch: u64,
        at: i64,
        position: i64,
    ) {
        let current = self.playing && epoch == self.epoch;
        if current {
            self.sample(now, position);
        }
        let key = (epoch, at);
        if let Some(due) = self.due.remove(&key) {
            let waited = painted.saturating_duration_since(due).as_secs_f64() * 1e3;
            if waited <= self.frame_ms {
                self.stats.on_time += 1;
            } else {
                self.stats.late += 1;
            }
        } else if self.evicted.0 > 0 && self.evicted.1.is_some_and(|newest| key <= newest) {
            self.evicted.0 -= 1;
            self.stats.late += 1;
        }
        self.settle();
        if current {
            let frames = u32::try_from(position.abs_diff(at)).map_or(f64::MAX, f64::from);
            let offset = frames * self.frame_ms;
            self.stats.max_av_offset_ms = self.stats.max_av_offset_ms.max(offset);
            if at > self.shown.1 {
                self.shown = (painted, at);
            }
        }
    }

    pub(crate) const fn playing(&self) -> bool {
        self.playing
    }

    /// Pending due records (tests).
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.due.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// R-5 with an injected clock: outcomes per due frame, one count per
    /// frame, judged by the paint instant, held age from the latest paint of
    /// a newer frame.
    #[test]
    fn due_frames_are_counted_once_by_their_ack() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.ack(at(0), at(0), 1, 0, 0);
        assert_eq!(counters.stats, PlaybackStats::default(), "never due");
        counters.begin(at(0), 0, 33.0, 5, 1);
        assert_eq!(counters.stats.due_frames, 1, "the first frame is due");
        counters.sample(at(5), 1);
        counters.ack(at(40), at(30), 1, 0, 1);
        counters.ack(at(41), at(30), 1, 0, 1);
        counters.ack(at(100), at(90), 1, 1, 1);
        counters.sample(at(300), 1);
        counters.ack(at(301), at(301), 1, 3, 4);
        let stats = &counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.late, stats.dropped);
        assert_eq!(outcomes, (5, 2, 1, 2));
        assert!((stats.max_held_ms - 211.0).abs() < 1.0, "{stats:?}");
        assert!((stats.max_av_offset_ms - 33.0).abs() < 0.1, "{stats:?}");
        counters.sample(at(2_402), 5);
        assert_eq!(counters.stats.due_frames, 5, "the end is not a due frame");
    }

    /// Review A/B F6: the ack is judged by its paint, not by when the next
    /// root epoch acknowledges it (100 ms later here), and an ack 2.1 s
    /// later still counts: late, never dropped.
    #[test]
    fn a_late_acknowledgment_is_judged_by_its_paint_and_never_expires() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.begin(at(0), 0, 33.0, 900, 1);
        counters.sample(at(33), 1);
        counters.ack(at(130), at(30), 1, 0, 4);
        assert_eq!(counters.stats.on_time, 1, "painted on time, acked late");
        for ms in (0..2_200).step_by(5) {
            counters.sample(at(33 + ms), 1 + i64::try_from(ms).unwrap() / 33);
        }
        counters.ack(at(2_233), at(2_232), 1, 1, 67);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.late), (1, 1), "{stats:?}");
        assert_eq!(stats.dropped, stats.due_frames - 2);
    }

    /// Review A F5: the terminal stop registers the last frames, and an ack
    /// after it (the next root epoch comes after EOS) still counts.
    #[test]
    fn an_ack_after_the_stop_counts() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.begin(at(0), 0, 100.0, 3, 7);
        counters.end(at(10), 2);
        assert_eq!(counters.stats.due_frames, 3, "0, 1 and 2 are due");
        counters.ack(at(20), at(12), 7, 2, 3);
        counters.ack(at(21), at(12), 7, 1, 3);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.dropped), (2, 1), "{stats:?}");
    }

    /// The E11.8 bound: past `DUE_RECORDS` pending records the oldest is
    /// evicted; its ack is late, and a frame never acknowledged is dropped.
    #[test]
    fn an_evicted_record_acknowledged_later_is_late() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        let over = i64::try_from(DUE_RECORDS).unwrap() + 1;
        counters.begin(t0, 0, 16.0, over + 1, 1);
        counters.sample(t0, over);
        assert_eq!(counters.pending(), DUE_RECORDS);
        counters.ack(t0, t0, 1, 0, over);
        counters.ack(t0, t0, 1, over, over);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.late), (1, 1), "{stats:?}");
        assert_eq!(stats.dropped, stats.due_frames - 2);
    }
}
