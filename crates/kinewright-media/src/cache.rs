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
    #[cfg(test)]
    evictions: usize,
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
            #[cfg(test)]
            evictions: 0,
        }
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
        while self.frames.len() > self.capacity {
            self.evict_oldest();
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

    pub(crate) fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self.order.pop_front() else {
            return false;
        };
        if let Some(frame) = self.frames.remove(&oldest) {
            self.release_bytes(&frame);
            #[cfg(test)]
            {
                self.evictions = self.evictions.saturating_add(1);
            }
        }
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
        // The decoder inserts `Arc` clones of one decoded picture for every
        // grid frame it covers. Charging each clone would report several
        // times the memory that is actually resident.
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

        // Evicting one of three references frees nothing: the buffer is
        // still resident behind the other two entries.
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
}
