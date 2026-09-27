use std::collections::{BTreeMap, HashMap, VecDeque};

use kinewright_core::{FrameTexture, TimeCode};

use crate::frame::CachedFrame;

pub(crate) struct FrameCache<T = FrameTexture>
where
    T: CachedFrame,
{
    capacity: usize,
    byte_len: usize,
    frames: BTreeMap<TimeCode, T>,
    order: VecDeque<TimeCode>,
    /// Live entry count per distinct shared pixel allocation.
    ///
    /// A decoded picture that covers several grid frames is stored as `Arc`
    /// clones under several `TimeCode` keys. Those clones share one
    /// allocation, so `byte_len` counts each buffer exactly once and reports
    /// actual cache residency instead of a multiple of it.
    residency: HashMap<usize, usize>,
    /// PF1 K-5/K-6: the preview's demand points on this source for the frame
    /// being rendered. The frame shown at each point is pinned: neither
    /// capacity nor distance eviction takes it. Empty outside the preview.
    demand: Vec<TimeCode>,
    /// The last non-empty demand, kept across [`Self::clear_demand`] as the
    /// reference for the travel direction.
    last_demand: Vec<TimeCode>,
    travel: Travel,
    #[cfg(test)]
    evictions: usize,
}

/// PF1 K-5: the direction the demand on one source travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Travel {
    Forward,
    Backward,
}

impl<T> FrameCache<T>
where
    T: CachedFrame,
{
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            byte_len: 0,
            frames: BTreeMap::new(),
            order: VecDeque::new(),
            residency: HashMap::new(),
            demand: Vec::new(),
            last_demand: Vec::new(),
            travel: Travel::Forward,
            #[cfg(test)]
            evictions: 0,
        }
    }

    /// PF1 K-5: set this render's demand points, pinning the frame shown at
    /// each, and update the travel direction.
    ///
    /// Travel follows the demand point that moved least: for each point, its
    /// step from the nearest previous point; the smallest non-zero step (a
    /// forward one on a tie) gives the direction. A discontinuous seek is a
    /// step like any other, so the frames the jump left behind go first.
    /// Unmoved points keep the direction, as does the first demand, which
    /// starts Forward.
    pub(crate) fn set_demand(&mut self, points: &[TimeCode]) {
        let step = points
            .iter()
            .filter_map(|point| {
                let steps = self.last_demand.iter().map(|last| point.0 - last.0);
                steps.min_by_key(|step| step.unsigned_abs())
            })
            .filter(|step| *step != 0)
            .min_by_key(|step| (step.unsigned_abs(), *step < 0));
        if let Some(step) = step {
            self.travel = if step > 0 {
                Travel::Forward
            } else {
                Travel::Backward
            };
        }
        self.demand.clear();
        self.demand.extend_from_slice(points);
        if !points.is_empty() {
            self.last_demand.clear();
            self.last_demand.extend_from_slice(points);
        }
    }

    /// PF1 K-6: no demand on this source (inactive, or a render under
    /// today's policy): nothing pinned; the travel direction is kept.
    pub(crate) fn clear_demand(&mut self) {
        self.demand.clear();
    }

    #[cfg(test)]
    pub(crate) const fn travel(&self) -> Travel {
        self.travel
    }

    /// The frame shown at each demand point: the one at or before it.
    fn pinned(&self) -> Vec<TimeCode> {
        let shown = |point: &TimeCode| self.frames.range(..=*point).next_back().map(|(at, _)| *at);
        self.demand.iter().filter_map(shown).collect()
    }

    /// Record one more cache entry for a frame's buffer, charging its bytes
    /// only when this is the buffer's first entry.
    fn retain_bytes(&mut self, frame: &T) {
        let entries = self.residency.entry(frame.shared_buffer_id()).or_insert(0);
        *entries = entries.saturating_add(1);
        if *entries == 1 {
            self.byte_len = self.byte_len.saturating_add(frame.byte_len());
        }
    }

    /// Drop one cache entry for a frame's buffer, releasing its bytes only
    /// when the last entry referencing that buffer is gone.
    fn release_bytes(&mut self, frame: &T) {
        let id = frame.shared_buffer_id();
        let Some(entries) = self.residency.get_mut(&id) else {
            return;
        };
        *entries = entries.saturating_sub(1);
        if *entries == 0 {
            self.residency.remove(&id);
            self.byte_len = self.byte_len.saturating_sub(frame.byte_len());
        }
    }

    pub(crate) fn insert(&mut self, at: TimeCode, frame: T) {
        self.retain_bytes(&frame);
        if let Some(replaced) = self.frames.insert(at, frame) {
            self.release_bytes(&replaced);
            self.order.retain(|entry| *entry != at);
        }
        self.order.push_back(at);
        if self.frames.len() <= self.capacity {
            return;
        }
        // PF1 K-5 (review B F3): capacity eviction takes the oldest unpinned
        // entry; pinned frames stay resident and charged, even over capacity.
        let pinned = self.pinned();
        while self.frames.len() > self.capacity {
            let Some(index) = self.order.iter().position(|at| !pinned.contains(at)) else {
                break;
            };
            if let Some(oldest) = self.order.remove(index) {
                self.remove_entry(oldest);
            }
        }
    }

    fn remove_entry(&mut self, at: TimeCode) {
        if let Some(frame) = self.frames.remove(&at) {
            self.release_bytes(&frame);
            #[cfg(test)]
            {
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }

    pub(crate) fn frame_at_or_before(&mut self, at: TimeCode) -> Option<T> {
        let key = self.frames.range(..=at).next_back().map(|(key, _)| *key)?;
        let frame = self.frames.get(&key)?.clone();
        self.order.retain(|entry| *entry != key);
        self.order.push_back(key);
        Some(frame)
    }

    /// Return the most recent frame without retaining a single entry larger
    /// than the caller's aggregate cache budget.
    pub(crate) fn frame_at_or_before_bounded(
        &mut self,
        at: TimeCode,
        max_retained_bytes: usize,
    ) -> Option<T> {
        let key = self.frames.range(..=at).next_back().map(|(key, _)| *key)?;
        let oversized = self
            .frames
            .get(&key)
            .is_some_and(|frame| frame.byte_len() > max_retained_bytes);
        if !oversized {
            return self.frame_at_or_before(at);
        }

        self.order.retain(|entry| *entry != key);
        let frame = self.frames.remove(&key)?;
        self.release_bytes(&frame);
        #[cfg(test)]
        {
            self.evictions = self.evictions.saturating_add(1);
        }
        Some(frame)
    }

    #[cfg(test)]
    pub(crate) const fn eviction_count(&self) -> usize {
        self.evictions
    }

    pub(crate) fn contains(&self, at: TimeCode) -> bool {
        self.frames.contains_key(&at)
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.byte_len
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    /// PF1 K-5/K-6: evict the frame farthest from every demand point, frames
    /// behind travel first (below the nearest point travelling forward, above
    /// it travelling backward). The frame shown at each demand point is
    /// pinned.
    pub(crate) fn evict_farthest(&mut self) -> bool {
        let pinned = self.pinned();
        let demand = &self.demand;
        let travel = self.travel;
        let rank = |at: &TimeCode| {
            let nearest = demand.iter().min_by_key(|point| at.0.abs_diff(point.0));
            nearest.map(|point| {
                let behind = match travel {
                    Travel::Forward => *at < *point,
                    Travel::Backward => *at > *point,
                };
                (behind, at.0.abs_diff(point.0))
            })
        };
        let victim = self
            .frames
            .keys()
            .filter(|at| !pinned.contains(at))
            .max_by_key(|at| rank(at))
            .copied();
        let Some(victim) = victim else {
            return false;
        };
        self.order.retain(|entry| *entry != victim);
        self.remove_entry(victim);
        true
    }

    pub(crate) fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self.order.pop_front() else {
            return false;
        };
        self.remove_entry(oldest);
        true
    }
}

/// Pick the most recent decoded frame whose presentation time is not after the audio clock.
#[must_use]
pub fn select_frame_for_position(available: &[TimeCode], position: TimeCode) -> Option<TimeCode> {
    available
        .iter()
        .copied()
        .filter(|frame| *frame <= position)
        .max()
        .or_else(|| available.iter().copied().min())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn selection_never_leads_the_audio_clock_when_a_prior_frame_exists() {
        let frames = [TimeCode(10), TimeCode(11), TimeCode(12)];
        assert_eq!(
            select_frame_for_position(&frames, TimeCode(11)),
            Some(TimeCode(11))
        );
        assert_eq!(
            select_frame_for_position(&frames, TimeCode(11)),
            Some(TimeCode(11))
        );
        assert_eq!(
            select_frame_for_position(&frames, TimeCode(9)),
            Some(TimeCode(10))
        );
    }

    #[test]
    fn byte_accounting_tracks_insert_replace_and_eviction() {
        let frame = |bytes| FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; bytes]),
        };
        let mut cache = FrameCache::new(2);
        cache.insert(TimeCode(0), frame(4));
        cache.insert(TimeCode(1), frame(8));
        assert_eq!(cache.byte_len(), 12);

        cache.insert(TimeCode(1), frame(16));
        assert_eq!(cache.byte_len(), 20);
        cache.insert(TimeCode(2), frame(32));
        assert_eq!(cache.byte_len(), 48);
        assert!(!cache.contains(TimeCode(0)));

        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 32);
    }

    #[test]
    fn oversized_frame_is_returned_without_being_retained() {
        let frame = FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 32]),
        };
        let mut cache = FrameCache::new(2);
        cache.insert(TimeCode(0), frame.clone());

        assert_eq!(
            cache.frame_at_or_before_bounded(TimeCode(0), 16),
            Some(frame)
        );
        assert_eq!(cache.byte_len(), 0);
        assert_eq!(cache.len(), 0);
        assert!(!cache.contains(TimeCode(0)));
    }

    #[test]
    fn arc_shared_grid_frames_are_charged_once_for_actual_residency() {
        let shared = FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 64]),
        };
        let other = FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 16]),
        };
        let mut cache = FrameCache::new(8);
        cache.insert(TimeCode(0), shared.clone());
        cache.insert(TimeCode(1), shared.clone());
        cache.insert(TimeCode(2), shared.clone());
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.byte_len(), 64);

        cache.insert(TimeCode(3), other);
        assert_eq!(cache.byte_len(), 80);

        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 80);
        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 80);

        // The last reference releases the shared allocation exactly once.
        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 16);
        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 0);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn replacing_one_grid_frame_of_a_shared_buffer_keeps_the_rest_charged() {
        let shared = FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 64]),
        };
        let replacement = FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 8]),
        };
        let mut cache = FrameCache::new(8);
        cache.insert(TimeCode(0), shared.clone());
        cache.insert(TimeCode(1), shared);
        assert_eq!(cache.byte_len(), 64);

        cache.insert(TimeCode(1), replacement);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.byte_len(), 72);

        // Dropping the surviving shared entry releases its 64 bytes.
        assert!(cache.evict_oldest());
        assert_eq!(cache.byte_len(), 8);
    }

    fn tiny() -> FrameTexture {
        FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 4]),
        }
    }

    /// `frames` cached, then evicted by distance until only pins are left:
    /// the victims in order, and what is left.
    fn distance_victims(cache: &mut FrameCache, frames: &[i64]) -> (Vec<i64>, Vec<i64>) {
        let (mut victims, mut left) = (Vec::new(), frames.to_vec());
        while cache.evict_farthest() {
            let gone = left.iter().position(|at| !cache.contains(TimeCode(*at)));
            victims.push(left.remove(gone.expect("one frame was evicted")));
        }
        (victims, left)
    }

    fn cache_of(frames: &[i64]) -> FrameCache {
        let mut cache = FrameCache::new(8);
        for at in frames {
            cache.insert(TimeCode(*at), tiny());
        }
        cache
    }

    /// PF1 K-5: behind travel first, farthest first; the shown frame stays.
    #[test]
    fn distance_eviction_spares_the_shown_frame_and_drops_behind_travel_first() {
        let frames = [0, 1, 2, 4, 5, 6];
        let mut cache = cache_of(&frames);
        cache.set_demand(&[TimeCode(3)]);
        assert_eq!(cache.travel(), Travel::Forward, "the first demand");
        let expected = (vec![0, 1, 6, 5, 4], vec![2]);
        assert_eq!(distance_victims(&mut cache, &frames), expected);
    }

    /// PF1 K-5 (review B F4): travelling backward, the frames above the
    /// demand are behind; the shown frame still stays.
    #[test]
    fn backward_travel_drops_the_frames_above_the_demand_first() {
        let frames = [0, 1, 2, 4, 5, 6];
        let mut cache = cache_of(&frames);
        cache.set_demand(&[TimeCode(6)]);
        cache.set_demand(&[TimeCode(3)]);
        assert_eq!(cache.travel(), Travel::Backward);
        let expected = (vec![6, 5, 4, 0, 1], vec![2]);
        assert_eq!(distance_victims(&mut cache, &frames), expected);
    }

    /// PF1 K-5 (review B F4): the direction follows each step, a reversal
    /// included; an unmoved demand and a cleared one keep it; a
    /// discontinuous seek travels in the jump's direction; with several
    /// points the one that moved least decides.
    #[test]
    fn travel_follows_reversals_and_seeks() {
        let mut cache = cache_of(&[]);
        let mut step = |points: &[i64]| {
            cache.set_demand(&points.iter().copied().map(TimeCode).collect::<Vec<_>>());
            cache.travel()
        };
        assert_eq!(step(&[10]), Travel::Forward, "the first demand");
        assert_eq!(step(&[9]), Travel::Backward, "a step back");
        assert_eq!(step(&[9]), Travel::Backward, "unmoved");
        assert_eq!(step(&[10]), Travel::Forward, "the reversal");
        assert_eq!(step(&[300]), Travel::Forward, "a seek forward");
        assert_eq!(step(&[3]), Travel::Backward, "a seek backward");
        assert_eq!(
            step(&[4, 100]),
            Travel::Forward,
            "a new point is not a step"
        );
        assert_eq!(step(&[3, 99]), Travel::Backward);
        cache.clear_demand();
        assert_eq!(cache.travel(), Travel::Backward, "clearing keeps it");
        cache.set_demand(&[TimeCode(4)]);
        assert_eq!(cache.travel(), Travel::Forward, "stepped from 3");
    }

    /// PF1 K-5 (review B F3): capacity eviction skips pinned frames, which
    /// stay resident and charged; with no demand it is today's LRU.
    #[test]
    fn capacity_eviction_keeps_the_pinned_frames() {
        let mut pinned = FrameCache::new(2);
        pinned.set_demand(&[TimeCode(0)]);
        for at in 0..3 {
            pinned.insert(TimeCode(at), tiny());
        }
        let resident = |cache: &FrameCache| {
            let frames = (0..3).filter(|at| cache.contains(TimeCode(*at)));
            frames.collect::<Vec<_>>()
        };
        assert_eq!(resident(&pinned), [0, 2]);
        assert_eq!(pinned.byte_len(), 8);

        // Pins over capacity are all kept.
        pinned.set_demand(&[TimeCode(0), TimeCode(1), TimeCode(2)]);
        pinned.insert(TimeCode(1), tiny());
        assert_eq!(resident(&pinned), [0, 1, 2]);
        assert_eq!(pinned.byte_len(), 12);

        let mut legacy = FrameCache::new(2);
        for at in 0..3 {
            legacy.insert(TimeCode(at), tiny());
        }
        assert_eq!(resident(&legacy), [1, 2]);
    }
}
