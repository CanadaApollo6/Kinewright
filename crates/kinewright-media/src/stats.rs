//! PF1 R-5: playback health, one leaf lock shared by the engine, the worker
//! and the preview thread (H-4: nothing else is taken while it is held).
//!
//! R34 single-writer accounting: the worker is the only writer of due-frame
//! outcomes. It registers due frames from its own applied transport and
//! settles the paint acks, which `Playback::ack_presented` only queues.
//! Every structure here has a fixed cap; history past it is folded into
//! the aggregate counters (E11.8).

use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
    time::Instant,
};

use kinewright_core::PlaybackStats;

/// Pending due-frame records (R32): over 18 minutes of unacknowledged frames
/// at 60 fps, about 3 MB at worst. Past it the oldest is evicted, keeping
/// its identity; an evicted frame acked later counts late.
pub(crate) const DUE_RECORDS: usize = 65_536;
/// R34: queued acks, including those ahead of the worker's registrations.
/// When it is full the oldest is dropped (`acks_overflowed`); an ack never
/// waits.
pub(crate) const ACK_QUEUE: usize = 256;
/// R34: epochs whose evicted frames keep their identity. An older epoch's
/// are forgotten: they stay dropped, and a later ack of one is unmatched.
pub(crate) const EVICTED_EPOCHS: usize = 8;
/// R34: settled ranges kept per evicted epoch. Past it the two oldest merge:
/// the frames between them stay dropped, and a later ack of one is
/// unmatched.
pub(crate) const SETTLED_RANGES: usize = 64;

/// One paint (R-5): the image of frame `at` in playback epoch `epoch`,
/// painted at `painted`, `expired` if the clock had passed `at` by then.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ack {
    pub(crate) epoch: u64,
    pub(crate) at: i64,
    pub(crate) painted: Instant,
    pub(crate) expired: bool,
}

/// A due frame's key: its playback epoch and frame. Keys only increase:
/// a playback epoch never re-registers a frame, and a later epoch is newer.
type DueKey = (u64, i64);

/// A pending due frame: when the worker registered it, and whether it was
/// charged to a running agent job (re-review 2 D4).
#[derive(Clone, Copy)]
struct Due {
    at: Instant,
    agent: bool,
}

/// The frames of one epoch evicted by `DUE_RECORDS` (re-review A D4 / B
/// D5). An epoch registers a contiguous run and the oldest record goes
/// first, so its evicted frames are `frames` less those already settled.
struct Evicted {
    frames: Range<i64>,
    settled: Ranges,
}

/// Disjoint half-open frame ranges, `start → end`, at most
/// `SETTLED_RANGES` of them.
#[derive(Default)]
struct Ranges(BTreeMap<i64, i64>);

impl Ranges {
    fn contains(&self, frame: i64) -> bool {
        (self.0.range(..=frame).next_back()).is_some_and(|(_, end)| frame < *end)
    }

    /// Add `[start, end)`, merging neighbours; past the cap the two oldest
    /// ranges merge (R34).
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
        if self.0.len() > SETTLED_RANGES
            && let (Some((first, _)), Some((_, second_end))) =
                (self.0.pop_first(), self.0.pop_first())
        {
            self.0.insert(first, second_end);
        }
    }

    fn covers(&self, frames: &Range<i64>) -> bool {
        (self.0.range(..=frames.start).next_back()).is_some_and(|(_, end)| *end >= frames.end)
    }
}

/// Re-review 2 D4: an agent job running in playback epoch `epoch`, begun
/// after the preview published frame `published` of it.
#[derive(Clone, Copy)]
struct AgentWindow {
    epoch: u64,
    published: Option<i64>,
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
    /// Due frames awaiting an ack. They outlive a stop (review A F5): an
    /// image painted before it can be acknowledged after it.
    due: BTreeMap<DueKey, Due>,
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
    /// Acks the worker has not settled: new ones, and those ahead of its
    /// registrations in the open epoch.
    acks: VecDeque<Ack>,
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
            evicted: BTreeMap::new(),
            next_due: 0,
            end_frame: i64::MAX,
            shown: (Instant::now(), -1),
            agent: None,
            acks: VecDeque::with_capacity(ACK_QUEUE),
        }
    }
}

impl Counters {
    /// An explicit `play`: fresh counters from the diagnostics' `underruns`.
    /// Frames and acks of the playback before it no longer count.
    pub(crate) fn clear(&mut self, underruns: [u64; 4]) {
        (self.stats, self.underrun_base) = (PlaybackStats::default(), underruns);
        self.due.clear();
        self.evicted.clear();
        self.acks.clear();
        (self.playing, self.agent) = (false, None);
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
        self.sample(now, Some(position));
    }

    /// Playback stopped with its own clock at `through`: the frames it
    /// reached since the last sample are due (the terminal stop passes the
    /// last frame), and the epoch closes: an ack ahead of it is unmatched.
    /// Pending records stay, so a later ack still counts.
    pub(crate) fn end(&mut self, now: Instant, through: Option<i64>) {
        if self.playing {
            self.sample(now, through);
            self.playing = false;
        }
        self.drain();
    }

    /// The worker, each tick (5 ms) and at every transport change: the due
    /// frames its applied playback reached through `position`, then the
    /// queued acks, then held age.
    pub(crate) fn sample(&mut self, now: Instant, position: Option<i64>) {
        if let Some(position) = position.filter(|_| self.playing) {
            self.register(now, position);
        }
        self.drain();
        if self.playing {
            let held = now.saturating_duration_since(self.shown.0).as_secs_f64() * 1e3;
            self.stats.max_held_ms = self.stats.max_held_ms.max(held);
        }
    }

    fn register(&mut self, now: Instant, position: i64) {
        while self.next_due <= position && self.next_due < self.end_frame {
            let frame = self.next_due;
            let agent = self.agent.is_some_and(|window| {
                window.epoch == self.epoch && window.published.is_none_or(|p| frame > p)
            });
            self.due.insert((self.epoch, frame), Due { at: now, agent });
            self.stats.dropped_agent += u64::from(agent);
            self.next_due += 1;
            self.stats.due_frames += 1;
            if self.due.len() > DUE_RECORDS
                && let Some(((epoch, frame), _)) = self.due.pop_first()
            {
                self.evict(epoch, frame);
            }
        }
    }

    /// Frames of `epoch` between the last evicted one and `frame` left the
    /// records by an ack: they are settled. Past `EVICTED_EPOCHS` the oldest
    /// epoch is forgotten.
    fn evict(&mut self, epoch: u64, frame: i64) {
        let evicted = self.evicted.entry(epoch).or_insert_with(|| Evicted {
            frames: frame..frame,
            settled: Ranges::default(),
        });
        evicted.settled.insert(evicted.frames.end, frame);
        evicted.frames.end = frame + 1;
        if self.evicted.len() > EVICTED_EPOCHS {
            self.evicted.pop_first();
        }
    }

    /// Queue one paint's ack for the worker (R34): it samples no clock and
    /// registers nothing. Taken under this leaf lock for O(1), never behind
    /// a render; a full queue drops its oldest.
    pub(crate) fn ack(&mut self, ack: Ack) {
        if self.acks.len() == ACK_QUEUE {
            self.acks.pop_front();
            self.stats.acks_overflowed += 1;
        }
        self.acks.push_back(ack);
    }

    /// The worker settles the queued acks against its own registrations.
    /// An ack ahead of them in the open epoch waits for the registration;
    /// any other that matches no record (a duplicate, a frame never due,
    /// history past the caps) is unmatched.
    fn drain(&mut self) {
        for _ in 0..self.acks.len() {
            let Some(ack) = self.acks.pop_front() else {
                break;
            };
            if self.settle(ack) {
                continue;
            }
            let open = self.playing && ack.epoch == self.epoch;
            if open && (self.next_due..self.end_frame).contains(&ack.at) {
                self.acks.push_back(ack);
            } else {
                self.stats.acks_unmatched += 1;
            }
        }
        let stats = &mut self.stats;
        stats.dropped = (stats.due_frames).saturating_sub(stats.on_time + stats.late);
    }

    /// Count one ack against its due frame, once: on time if painted current
    /// within due + 1 frame, else late; an evicted frame's first ack is
    /// late. The A/V offset is the whole frames the clock had passed `at`
    /// when it was painted, from the frame's due instant.
    fn settle(&mut self, ack: Ack) -> bool {
        if let Some(due) = self.due.remove(&(ack.epoch, ack.at)) {
            let waited = ack.painted.saturating_duration_since(due.at).as_secs_f64() * 1e3;
            if !ack.expired && waited <= self.frame_ms {
                self.stats.on_time += 1;
            } else {
                self.stats.late += 1;
            }
            let stats = &mut self.stats;
            stats.dropped_agent = stats.dropped_agent.saturating_sub(u64::from(due.agent));
            let behind = (waited / self.frame_ms)
                .floor()
                .max(f64::from(u8::from(ack.expired)));
            let offset = behind * self.frame_ms;
            self.stats.max_av_offset_ms = self.stats.max_av_offset_ms.max(offset);
        } else if let Some(evicted) = self.evicted.get_mut(&ack.epoch)
            && evicted.frames.contains(&ack.at)
            && !evicted.settled.contains(ack.at)
        {
            evicted.settled.insert(ack.at, ack.at + 1);
            if evicted.settled.covers(&evicted.frames) {
                self.evicted.remove(&ack.epoch);
            }
            self.stats.late += 1;
        } else {
            return false;
        }
        if ack.epoch == self.epoch && ack.at > self.shown.1 {
            let held = ack.painted.saturating_duration_since(self.shown.0);
            self.stats.max_held_ms = self.stats.max_held_ms.max(held.as_secs_f64() * 1e3);
            self.shown = (ack.painted, ack.at);
        }
        true
    }

    /// Re-review 2 D4: an agent job starts while playback runs, after the
    /// preview published `published`. Each due frame the worker registers
    /// in that epoch while it runs, past `published`, is charged to
    /// `dropped_agent` until its paint settles it.
    pub(crate) fn agent_started(&mut self, published: Option<DueKey>) {
        let epoch = self.epoch;
        let published = published
            .filter(|(e, _)| *e == epoch)
            .map(|(_, frame)| frame);
        self.agent = self.playing.then_some(AgentWindow { epoch, published });
    }

    pub(crate) fn agent_finished(&mut self) {
        self.agent = None;
    }

    pub(crate) const fn playing(&self) -> bool {
        self.playing
    }

    /// Pending due records, evicted epochs, their settled ranges and queued
    /// acks (tests: the caps).
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.due.len()
    }

    #[cfg(test)]
    fn storage(&self) -> (usize, usize, usize) {
        let ranges = self.evicted.values().map(|e| e.settled.0.len()).max();
        (self.evicted.len(), ranges.unwrap_or(0), self.acks.len())
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// One paint's ack, then the worker's drain (R34: an ack only queues).
    fn ack(counters: &mut Counters, painted: Instant, epoch: u64, at: i64, expired: bool) {
        (counters).ack(Ack {
            epoch,
            at,
            painted,
            expired,
        });
        counters.drain();
    }

    /// R-5 with an injected clock: outcomes per due frame, one count per
    /// frame, judged by the paint instant, held age from the latest paint of
    /// a newer frame, the offset the whole frames the paint trailed its due
    /// instant.
    #[test]
    fn due_frames_are_counted_once_by_their_ack() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        ack(&mut counters, at(0), 1, 0, false);
        let never_due = PlaybackStats {
            acks_unmatched: 1,
            ..PlaybackStats::default()
        };
        assert_eq!(counters.stats, never_due, "never due");
        counters.begin(at(0), 0, 33.0, 5, 1);
        assert_eq!(counters.stats.due_frames, 1, "the first frame is due");
        counters.sample(at(33), Some(1));
        ack(&mut counters, at(30), 1, 0, false);
        ack(&mut counters, at(30), 1, 0, false);
        ack(&mut counters, at(90), 1, 1, false);
        counters.sample(at(300), Some(4));
        ack(&mut counters, at(301), 1, 3, false);
        let stats = &counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.late, stats.dropped);
        assert_eq!(outcomes, (5, 2, 1, 2));
        assert_eq!(stats.acks_unmatched, 2, "never due, then a duplicate");
        assert!((stats.max_held_ms - 211.0).abs() < 1.0, "{stats:?}");
        assert!((stats.max_av_offset_ms - 33.0).abs() < 0.1, "{stats:?}");
        counters.sample(at(2_402), Some(5));
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
        counters.sample(at(33), Some(1));
        ack(&mut counters, at(30), 1, 0, false);
        assert_eq!(counters.stats.on_time, 1, "painted on time, acked late");
        for ms in (0..2_200).step_by(5) {
            let frame = 1 + i64::try_from(ms).unwrap() / 33;
            counters.sample(at(33 + ms), Some(frame));
        }
        ack(&mut counters, at(2_232), 1, 1, false);
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
        counters.sample(t0, Some(11));
        ack(&mut counters, t0 + Duration::from_millis(1), 1, 10, true);
        ack(&mut counters, t0 + Duration::from_millis(1), 1, 11, false);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.late), (1, 1), "{stats:?}");
    }

    /// R34 (re-review 2 D2): an image painted before the worker registered
    /// its frame waits in the queue until the worker does: at its next tick,
    /// or where a pause, a seek or a new document ends the epoch through its
    /// applied position. It counts on time, with no offset. An ack ahead of
    /// an epoch that closes short of its frame is unmatched.
    #[test]
    fn an_ack_ahead_of_the_tick_waits_for_its_registration() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.begin(at(0), 10, 33.0, 900, 1);
        ack(&mut counters, at(34), 1, 11, false);
        let stats = counters.stats;
        assert_eq!((stats.due_frames, stats.on_time), (1, 0), "held");
        counters.sample(at(35), Some(11));
        let stats = counters.stats;
        let outcomes = (stats.due_frames, stats.on_time, stats.dropped);
        assert_eq!(outcomes, (2, 1, 1), "the tick: {stats:?}");
        assert!(stats.max_av_offset_ms < 0.1, "{stats:?}");

        // A pause, a seek, a new document: each ends the epoch through its
        // applied position (a seek then begins the next epoch).
        for (n, transition) in (0..).zip(["a pause", "a seek", "a document"]) {
            let epoch = 2 + 2 * n;
            counters.begin(at(40), 20, 33.0, 900, epoch);
            ack(&mut counters, at(74), epoch, 21, false);
            counters.end(at(75), Some(21));
            if transition == "a seek" {
                counters.begin(at(75), 500, 33.0, 900, epoch + 1);
            }
            let stats = counters.stats;
            assert_eq!(stats.on_time, n + 2, "{transition}: {stats:?}");
            assert!(stats.max_av_offset_ms < 0.1, "{transition}: {stats:?}");
        }
        assert_eq!(counters.stats.acks_unmatched, 0);
        counters.begin(at(80), 30, 33.0, 900, 8);
        ack(&mut counters, at(81), 8, 40, false);
        counters.end(at(82), Some(31));
        assert_eq!(counters.stats.acks_unmatched, 1, "its epoch closed");
        assert_eq!(counters.storage().2, 0, "none left queued");
    }

    /// Review A F5: the terminal stop registers the last frames, and an ack
    /// after it (the next root epoch comes after EOS) still counts.
    #[test]
    fn an_ack_after_the_stop_counts() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut counters = Counters::default();
        counters.begin(at(0), 0, 100.0, 3, 7);
        counters.end(at(10), Some(2));
        assert_eq!(counters.stats.due_frames, 3, "0, 1 and 2 are due");
        ack(&mut counters, at(12), 7, 2, false);
        ack(&mut counters, at(12), 7, 1, false);
        let stats = counters.stats;
        assert_eq!((stats.on_time, stats.dropped), (2, 1), "{stats:?}");
    }

    /// The E11.8 bound (re-review A D4 / B D5): past `DUE_RECORDS` pending
    /// records the oldest is evicted, keeping its identity. Its first ack is
    /// late; a duplicate ack, a key never due and a key of an epoch that
    /// registered nothing count nothing (unmatched), so the next evicted
    /// frame's own ack still counts; a frame acked before its eviction is
    /// not counted again.
    #[test]
    fn an_evicted_record_acknowledged_later_is_late() {
        let t0 = Instant::now();
        let outcomes = |counters: &Counters| (counters.stats.on_time, counters.stats.late);
        let mut counters = Counters::default();
        let over = i64::try_from(DUE_RECORDS).unwrap() + 1;
        counters.begin(t0, 0, 16.0, over + 1, 1);
        counters.sample(t0, Some(over));
        assert_eq!(counters.pending(), DUE_RECORDS, "frames 0 and 1 evicted");
        ack(&mut counters, t0, 1, 0, false);
        assert_eq!(outcomes(&counters), (0, 1));
        for (unmatched, (epoch, frame, case)) in [
            (1, 0, "a duplicate"),
            (1, over + 5, "never due: past the clock"),
            (1, -3, "never due: before the start"),
            (0, 1, "an epoch that registered nothing"),
        ]
        .into_iter()
        .enumerate()
        {
            ack(&mut counters, t0, epoch, frame, false);
            assert_eq!(outcomes(&counters), (0, 1), "{case}");
            assert_eq!(counters.stats.acks_unmatched, unmatched as u64 + 1);
        }
        ack(&mut counters, t0, 1, 1, false);
        ack(&mut counters, t0, 1, 1, false);
        assert_eq!(outcomes(&counters), (0, 2), "frame 1's own ack, once");
        ack(&mut counters, t0, 1, over, false);
        assert_eq!(outcomes(&counters), (1, 2));
        let stats = counters.stats;
        assert_eq!(stats.dropped, stats.due_frames - 3);

        let mut counters = Counters::default();
        let records = i64::try_from(DUE_RECORDS).unwrap();
        counters.begin(t0, 0, 16.0, records + 10, 2);
        counters.sample(t0, Some(2));
        ack(&mut counters, t0, 2, 1, false);
        counters.sample(t0, Some(records + 3));
        assert_eq!(counters.pending(), DUE_RECORDS, "0, 2 and 3 evicted");
        ack(&mut counters, t0, 2, 1, false);
        assert_eq!(outcomes(&counters), (1, 0), "acked before its eviction");
        for frame in [2, 0, 3, 3, 2] {
            ack(&mut counters, t0, 2, frame, false);
        }
        assert_eq!(outcomes(&counters), (1, 3), "each evicted frame once");
        assert!(counters.evicted.is_empty(), "all settled");
    }

    /// R34 (re-review 2 D3): playing seeks past `DUE_RECORDS` unacknowledged
    /// frames keep evicted identity for `EVICTED_EPOCHS` epochs, no more; a
    /// forgotten epoch's frames stay dropped and its late ack is unmatched,
    /// while a remembered epoch's still counts late.
    #[test]
    fn eviction_keeps_a_bounded_number_of_epochs() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        let run = 4_000;
        let epochs = 40;
        for epoch in 1..=epochs {
            counters.begin(t0, 0, 16.0, i64::MAX, epoch);
            counters.sample(t0, Some(run - 1));
            let (evicted, ranges, acks) = counters.storage();
            assert!(evicted <= EVICTED_EPOCHS, "epoch {epoch}: {evicted}");
            assert_eq!((ranges, acks), (0, 0));
        }
        assert_eq!(counters.storage().0, EVICTED_EPOCHS, "at the cap");
        assert_eq!(counters.pending(), DUE_RECORDS);
        ack(&mut counters, t0, 1, 7, false);
        assert_eq!(counters.stats.acks_unmatched, 1, "epoch 1 is forgotten");
        let remembered = *counters.evicted.keys().next().unwrap();
        ack(&mut counters, t0, remembered, 7, false);
        let stats = counters.stats;
        assert_eq!((stats.late, stats.acks_unmatched), (1, 1), "{stats:?}");
        let due = u64::try_from(run).unwrap() * epochs;
        assert_eq!((stats.due_frames, stats.dropped), (due, due - 1));
    }

    /// R34 (re-review 2 D3): alternating acks of evicted frames keep at most
    /// `SETTLED_RANGES` ranges; the two oldest merge, so a later ack of a
    /// frame between them is unmatched (it stays dropped) while one past
    /// the merged range still counts late.
    #[test]
    fn sparse_acks_keep_a_bounded_number_of_settled_ranges() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        let records = i64::try_from(DUE_RECORDS).unwrap();
        counters.begin(t0, 0, 16.0, i64::MAX, 1);
        counters.sample(t0, Some(records + 300));
        assert_eq!(counters.pending(), DUE_RECORDS, "0..=300 evicted");
        for frame in (0..=300).step_by(2) {
            ack(&mut counters, t0, 1, frame, false);
            assert!(counters.storage().1 <= SETTLED_RANGES, "at {frame}");
        }
        assert_eq!(counters.storage().1, SETTLED_RANGES, "at the cap");
        assert_eq!(counters.stats.late, 151);
        ack(&mut counters, t0, 1, 1, false);
        assert_eq!(counters.stats.acks_unmatched, 1, "merged away");
        ack(&mut counters, t0, 1, 299, false);
        let stats = counters.stats;
        assert_eq!((stats.late, stats.acks_unmatched), (152, 1), "{stats:?}");
    }

    /// R34: the ack queue holds `ACK_QUEUE` acks; a push never waits, and a
    /// full queue drops its oldest (`acks_overflowed`), whose frame stays
    /// dropped. The held acks settle when the worker registers their frames.
    #[test]
    fn a_full_ack_queue_drops_its_oldest() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        counters.begin(t0, 0, 16.0, 10_000, 1);
        let extra = 44;
        let pushed = i64::try_from(ACK_QUEUE).unwrap() + extra;
        for frame in 1..=pushed {
            counters.ack(Ack {
                epoch: 1,
                at: frame,
                painted: t0,
                expired: false,
            });
        }
        let overflowed = u64::try_from(extra).unwrap();
        assert_eq!(counters.stats.acks_overflowed, overflowed);
        assert_eq!(counters.storage().2, ACK_QUEUE);
        counters.drain();
        assert_eq!(counters.storage().2, ACK_QUEUE, "all ahead: held");
        counters.sample(t0, Some(pushed));
        let stats = counters.stats;
        assert_eq!(counters.storage().2, 0);
        assert_eq!(stats.on_time, u64::try_from(ACK_QUEUE).unwrap());
        assert_eq!(stats.dropped, overflowed + 1, "0 and the overflowed");
    }

    /// Re-review B D3 / re-review 2 D4: an agent job is charged each due
    /// frame registered in its playback epoch while it runs and not painted:
    /// not a seek's jump, nothing once an explicit `play` cleared the
    /// counters, not a frame the preview had already published, and not
    /// one whose paint, acked before the worker's tick, settles it.
    #[test]
    fn an_agent_job_is_charged_only_its_epochs_registered_frames() {
        let t0 = Instant::now();
        let mut counters = Counters::default();
        counters.begin(t0, 10, 33.0, 1_000, 1);
        counters.agent_started(Some((1, 10)));
        counters.sample(t0, Some(12));
        counters.begin(t0, 900, 33.0, 1_000, 2);
        counters.sample(t0, Some(905));
        counters.agent_finished();
        assert_eq!(counters.stats.dropped_agent, 2, "11 and 12, not the seek");

        counters.agent_started(Some((1, 12)));
        counters.end(t0, Some(907));
        counters.agent_finished();
        assert_eq!(counters.stats.dropped_agent, 4, "906 and 907: a pause");

        counters.begin(t0, 910, 33.0, 1_000, 3);
        counters.agent_started(Some((3, 910)));
        counters.sample(t0, Some(912));
        counters.clear([0; 4]);
        counters.begin(t0, 0, 33.0, 1_000, 4);
        counters.sample(t0, Some(5));
        counters.agent_finished();
        assert_eq!(counters.stats.dropped_agent, 0, "a replay");

        // The preview published 10 and painted 11 before the worker's tick
        // registered either; the job then ran through 13.
        counters.begin(t0, 9, 33.0, 1_000, 5);
        counters.agent_started(Some((5, 11)));
        ack(&mut counters, t0, 5, 11, false);
        counters.sample(t0, Some(13));
        counters.agent_finished();
        let stats = counters.stats;
        assert_eq!(stats.dropped_agent, 2, "12 and 13: {stats:?}");
        assert_eq!(stats.on_time, 1, "11 was painted");

        // A frame registered in the window and painted after is settled.
        counters.begin(t0, 20, 33.0, 1_000, 6);
        counters.agent_started(None);
        counters.sample(t0, Some(22));
        ack(&mut counters, t0, 6, 21, false);
        counters.agent_finished();
        assert_eq!(counters.stats.dropped_agent, 3, "22, not 21");
    }
}
