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

/// A caller-side transport snapshot taken at an ack, under the coalesced
/// lock: the newest issued epoch and the clock's position then. Every call
/// that moves the clock bumps the epoch as it does, under that lock, so the
/// position is playback of that epoch. Only a snapshot of the applied
/// epoch is sampled (re-review A D3 / B D1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Transport {
    pub(crate) now: Instant,
    pub(crate) epoch: u64,
    pub(crate) position: i64,
}

/// A due frame's key: its playback epoch and frame. Keys only increase:
/// a playback epoch never re-registers a frame, and a later epoch is newer.
type DueKey = (u64, i64);

/// Re-review A D4 / B D5: the frames of one epoch evicted by
/// `DUE_RECORDS`. An epoch registers a contiguous run of frames and the
/// oldest record goes first, so its evicted frames are `frames` less those
/// already settled: acknowledged before or after eviction.
struct Evicted {
    frames: std::ops::Range<i64>,
    settled: Ranges,
}

/// Disjoint half-open frame ranges, `start → end` (acks run in order, so
/// they coalesce).
#[derive(Default)]
struct Ranges(BTreeMap<i64, i64>);

impl Ranges {
    fn contains(&self, frame: i64) -> bool {
        (self.0.range(..=frame).next_back()).is_some_and(|(_, end)| frame < *end)
    }

    /// Add `[start, end)`, merging neighbours.
    fn insert(&mut self, mut start: i64, mut end: i64) {
        if start >= end {
            return;
        }
        if let Some((&before, &before_end)) = self.0.range(..=start).next_back()
            && before_end >= start
        {
            start = before;
            end = end.max(before_end);
        }
        while let Some((&next, &next_end)) = self.0.range(start..).next()
            && next <= end
        {
            end = end.max(next_end);
            self.0.remove(&next);
        }
        self.0.insert(start, end);
    }

    fn covers(&self, frames: &std::ops::Range<i64>) -> bool {
        (self.0.range(..=frames.start).next_back()).is_some_and(|(_, end)| *end >= frames.end)
    }
}

/// Re-review B D3: the due frames registered in one playback epoch while
/// an agent job runs, and the newest of them.
#[derive(Clone, Copy)]
struct AgentWindow {
    epoch: u64,
    generation: u64,
    registered: u64,
    newest: Option<i64>,
}

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
    /// The latest applied position the worker sampled in `epoch`.
    position: i64,
    /// Bumped by every `clear`: an agent window from before it charges
    /// nothing.
    generation: u64,
    /// Due frames awaiting an ack, with the instant the clock reached them.
    /// They outlive a stop (review A F5): an image painted before it can be
    /// acknowledged after it.
    due: BTreeMap<DueKey, Instant>,
    /// Evicted frames per epoch, each acknowledged at most once.
    evicted: BTreeMap<u64, Evicted>,
    next_due: i64,
    /// The programme's end (its duration in frames): the clock reaches it,
    /// but no frame there is due.
    end_frame: i64,
    /// When the image on screen was painted, and its frame; held age runs
    /// from the latest paint of a newer frame.
    shown: (Instant, i64),
    agent: Option<AgentWindow>,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            stats: PlaybackStats::default(),
            underrun_base: [0; 4],
            playing: false,
            frame_ms: 0.0,
            epoch: 0,
            position: 0,
            generation: 0,
            due: BTreeMap::new(),
            evicted: BTreeMap::new(),
            next_due: 0,
            end_frame: i64::MAX,
            shown: (Instant::now(), -1),
            agent: None,
        }
    }
}

impl Counters {
    /// An explicit `play`: fresh counters from the diagnostics' `underruns`.
    /// Frames of the playback before it no longer count.
    pub(crate) fn clear(&mut self, underruns: [u64; 4]) {
        (self.stats, self.underrun_base) = (PlaybackStats::default(), underruns);
        self.due.clear();
        self.evicted.clear();
        self.generation += 1;
        self.agent = None;
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
        (self.epoch, self.next_due, self.position) = (epoch, position, position);
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
            let frame = self.next_due;
            self.due.insert((self.epoch, frame), now);
            self.next_due += 1;
            self.stats.due_frames += 1;
            if let Some(window) = &mut self.agent
                && (window.epoch, window.generation) == (self.epoch, self.generation)
            {
                window.registered += 1;
                window.newest = Some(frame);
            }
            if self.due.len() > DUE_RECORDS
                && let Some(((epoch, frame), _)) = self.due.pop_first()
            {
                self.evict(epoch, frame);
            }
        }
        self.settle();
    }

    /// Frames of `epoch` between the last evicted one and `frame` left the
    /// records by an ack: they are settled.
    fn evict(&mut self, epoch: u64, frame: i64) {
        let evicted = self.evicted.entry(epoch).or_insert_with(|| Evicted {
            frames: frame..frame,
            settled: Ranges::default(),
        });
        evicted.settled.insert(evicted.frames.end, frame);
        evicted.frames.end = frame + 1;
    }

    fn settle(&mut self) {
        let stats = &mut self.stats;
        stats.dropped = (stats.due_frames).saturating_sub(stats.on_time + stats.late);
    }

    /// The worker, each tick (5 ms) while playing, from the applied
    /// position only (review B F3): newly due frames and held age.
    pub(crate) fn sample(&mut self, now: Instant, position: i64) {
        if !self.playing {
            return;
        }
        self.position = position;
        self.register(now, position);
        let held = now.saturating_duration_since(self.shown.0).as_secs_f64() * 1e3;
        self.stats.max_held_ms = self.stats.max_held_ms.max(held);
    }

    /// The image of frame `at` in playback epoch `epoch`, painted at
    /// `painted` (R-5), `expired` if the clock had passed `at` by then.
    /// Counted once per due frame: on time if painted current within due + 1
    /// frame, else late; an evicted frame's first ack is late.
    ///
    /// The ack first samples `transport`, as the worker's tick does, but
    /// only if it is a snapshot of the applied epoch: a caller-side clock a
    /// call the worker has not applied moved (a seek) is never sampled
    /// (re-review A D3 / B D1). An image painted before the tick reached
    /// its frame is then still counted. The A/V offset is taken against
    /// that snapshot only.
    pub(crate) fn ack(
        &mut self,
        painted: Instant,
        epoch: u64,
        at: i64,
        expired: bool,
        transport: Option<Transport>,
    ) {
        let applied = transport.filter(|clock| self.playing && clock.epoch == self.epoch);
        if let Some(clock) = applied {
            self.sample(clock.now, clock.position.max(self.position));
        }
        if let Some(due) = self.due.remove(&(epoch, at)) {
            let waited = painted.saturating_duration_since(due).as_secs_f64() * 1e3;
            if !expired && waited <= self.frame_ms {
                self.stats.on_time += 1;
            } else {
                self.stats.late += 1;
            }
        } else if let Some(evicted) = self.evicted.get_mut(&epoch)
            && evicted.frames.contains(&at)
            && !evicted.settled.contains(at)
        {
            evicted.settled.insert(at, at + 1);
            if evicted.settled.covers(&evicted.frames) {
                self.evicted.remove(&epoch);
            }
            self.stats.late += 1;
        }
        self.settle();
        if self.playing && epoch == self.epoch {
            if let Some(clock) = applied {
                let frames = u32::try_from(clock.position.abs_diff(at)).map_or(f64::MAX, f64::from);
                let offset = frames * self.frame_ms;
                self.stats.max_av_offset_ms = self.stats.max_av_offset_ms.max(offset);
            }
            if at > self.shown.1 {
                let held = painted.saturating_duration_since(self.shown.0);
                let held = held.as_secs_f64() * 1e3;
                self.stats.max_held_ms = self.stats.max_held_ms.max(held);
                self.shown = (painted, at);
            }
        }
    }

    /// Re-review B D3: an agent job starts while playback runs.
    pub(crate) fn agent_started(&mut self) {
        self.agent = self.playing.then_some(AgentWindow {
            epoch: self.epoch,
            generation: self.generation,
            registered: 0,
            newest: None,
        });
    }

    /// The job ended: the due frames registered in its playback epoch
    /// meanwhile are displaced by it, counted in `dropped_agent` (unless
    /// an explicit `play` cleared the counters). Returns the newest.
    pub(crate) fn agent_finished(&mut self) -> Option<DueKey> {
        let window = self.agent.take()?;
        if window.generation != self.generation {
            return None;
        }
        self.stats.dropped_agent += window.registered;
        window.newest.map(|frame| (window.epoch, frame))
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
    /// a newer frame, the offset against the ack's applied snapshot.
    #[test]
    fn due_frames_are_counted_once_by_their_ack() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.ack(at(0), 1, 0, false, None);
        assert_eq!(counters.stats, PlaybackStats::default(), "never due");
        counters.begin(at(0), 0, 33.0, 5, 1);
        assert_eq!(counters.stats.due_frames, 1, "the first frame is due");
        counters.sample(at(5), 1);
        counters.ack(at(30), 1, 0, false, None);
        counters.ack(at(30), 1, 0, false, None);
        counters.ack(at(90), 1, 1, false, None);
        counters.sample(at(300), 4);
        let transport = Transport {
            now: at(301),
            epoch: 1,
            position: 4,
        };
        counters.ack(at(301), 1, 3, false, Some(transport));
        let stats = &counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.late, stats.dropped);
        assert_eq!(outcomes, (5, 2, 1, 2));
        assert!((stats.max_held_ms - 211.0).abs() < 1.0, "{stats:?}");
        assert!((stats.max_av_offset_ms - 33.0).abs() < 0.1, "{stats:?}");
        counters.sample(at(2_402), 5);
        assert_eq!(counters.stats.due_frames, 5, "the end is not a due frame");
    }

    /// Review A/B F6: the ack is judged by its paint, not by when the next
    /// root epoch acknowledges it, and an ack 2.1 s later still counts:
    /// late, never dropped.
    #[test]
    fn a_late_acknowledgment_is_judged_by_its_paint_and_never_expires() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.begin(at(0), 0, 33.0, 900, 1);
        counters.sample(at(33), 1);
        counters.ack(at(30), 1, 0, false, None);
        assert_eq!(counters.stats.on_time, 1, "painted on time, acked late");
        for ms in (0..2_200).step_by(5) {
            counters.sample(at(33 + ms), 1 + i64::try_from(ms).unwrap() / 33);
        }
        counters.ack(at(2_232), 1, 1, false, None);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.late), (1, 1), "{stats:?}");
        assert_eq!(stats.dropped, stats.due_frames - 2);
    }

    /// R33 (re-review A D1): an image bound just before its frame expired
    /// and painted after is judged at its paint: late, never on time, even
    /// within due + 1 frame.
    #[test]
    fn a_frame_expired_at_its_paint_is_late() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        counters.begin(t0, 10, 33.0, 900, 1);
        counters.sample(t0, 11);
        counters.ack(t0 + Duration::from_millis(1), 1, 10, true, None);
        counters.ack(t0 + Duration::from_millis(1), 1, 11, false, None);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.late), (1, 1), "{stats:?}");
    }

    /// R33 (E11.11.4): an image painted before the worker's tick reached its
    /// frame is acked with a snapshot of the applied epoch, which the ack
    /// samples: the frame is due and counted on time, with no offset (the
    /// settle-only ack found no record, and the tick then made it dropped).
    /// A snapshot of a newer epoch (a seek not yet applied) is not sampled.
    #[test]
    fn an_ack_ahead_of_the_tick_samples_its_applied_snapshot() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let snapshot = |ms: u64, epoch: u64, position: i64| {
            Some(Transport {
                now: at(ms),
                epoch,
                position,
            })
        };
        let mut counters = Counters::default();
        counters.begin(at(0), 10, 33.0, 900, 1);
        counters.ack(at(34), 1, 11, false, snapshot(35, 1, 11));
        let stats = counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.dropped);
        assert_eq!(outcomes, (2, 1, 1), "{stats:?}");
        assert!(stats.max_av_offset_ms < 0.1, "{stats:?}");
        counters.ack(at(40), 1, 11, false, snapshot(41, 2, 900));
        let stats = counters.stats;
        assert_eq!(stats.due_frames, 2, "a seek not applied: {stats:?}");
        counters.sample(at(70), 12);
        let stats = counters.stats;
        assert_eq!((stats.due_frames, stats.on_time), (3, 1), "{stats:?}");
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
        counters.ack(at(12), 7, 2, false, None);
        counters.ack(at(12), 7, 1, false, None);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.dropped), (2, 1), "{stats:?}");
    }

    /// The E11.8 bound (re-review A D4 / B D5): past `DUE_RECORDS` pending
    /// records the oldest is evicted, keeping its identity. Its first ack is
    /// late; a duplicate ack, a key never due and a key of an epoch that
    /// registered nothing count nothing, so the next evicted frame's own ack
    /// still counts; a frame acked before its eviction is not counted again.
    #[test]
    fn an_evicted_record_acknowledged_later_is_late() {
        let t0 = Instant::now();
        let outcomes = |counters: &Counters| (counters.stats.on_time, counters.stats.late);
        let mut counters = Counters::default();
        let over = i64::try_from(DUE_RECORDS).unwrap() + 1;
        counters.begin(t0, 0, 16.0, over + 1, 1);
        counters.sample(t0, over);
        assert_eq!(counters.pending(), DUE_RECORDS, "frames 0 and 1 evicted");
        counters.ack(t0, 1, 0, false, None);
        assert_eq!(outcomes(&counters), (0, 1));
        for (epoch, frame, case) in [
            (1, 0, "a duplicate"),
            (1, over + 5, "never due: past the clock"),
            (1, -3, "never due: before the start"),
            (0, 1, "an epoch that registered nothing"),
        ] {
            counters.ack(t0, epoch, frame, false, None);
            assert_eq!(outcomes(&counters), (0, 1), "{case}");
        }
        counters.ack(t0, 1, 1, false, None);
        counters.ack(t0, 1, 1, false, None);
        assert_eq!(outcomes(&counters), (0, 2), "frame 1's own ack, once");
        counters.ack(t0, 1, over, false, None);
        assert_eq!(outcomes(&counters), (1, 2));
        let stats = counters.stats;
        assert_eq!(stats.dropped, stats.due_frames - 3);

        let mut counters = Counters::default();
        let records = i64::try_from(DUE_RECORDS).unwrap();
        counters.begin(t0, 0, 16.0, records + 10, 2);
        counters.sample(t0, 2);
        counters.ack(t0, 2, 1, false, None);
        counters.sample(t0, records + 3);
        assert_eq!(counters.pending(), DUE_RECORDS, "0, 2 and 3 evicted");
        counters.ack(t0, 2, 1, false, None);
        assert_eq!(outcomes(&counters), (1, 0), "acked before its eviction");
        for frame in [2, 0, 3, 3, 2] {
            counters.ack(t0, 2, frame, false, None);
        }
        assert_eq!(outcomes(&counters), (1, 3), "each evicted frame once");
        assert!(counters.evicted.is_empty(), "all settled");
    }

    /// Re-review B D3: an agent job is charged the due frames registered in
    /// its playback epoch while it runs: not a seek's jump, and nothing once
    /// an explicit `play` cleared the counters.
    #[test]
    fn an_agent_job_is_charged_only_its_epochs_registered_frames() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        counters.begin(t0, 10, 33.0, 1_000, 1);
        counters.agent_started();
        counters.sample(t0, 12);
        counters.begin(t0, 900, 33.0, 1_000, 2);
        counters.sample(t0, 905);
        assert_eq!(counters.agent_finished(), Some((1, 12)));
        assert_eq!(counters.stats.dropped_agent, 2, "11 and 12, not the seek");

        counters.agent_started();
        counters.end(t0, 907);
        assert_eq!(counters.agent_finished(), Some((2, 907)), "a pause");
        assert_eq!(counters.stats.dropped_agent, 4, "906 and 907");

        counters.begin(t0, 910, 33.0, 1_000, 3);
        counters.agent_started();
        counters.sample(t0, 912);
        counters.clear([0; 4]);
        counters.begin(t0, 0, 33.0, 1_000, 4);
        counters.sample(t0, 5);
        assert_eq!(counters.agent_finished(), None, "a replay");
        assert_eq!(counters.stats.dropped_agent, 0);
    }
}
