use std::collections::{BTreeMap, VecDeque};

use kinewright_core::{FrameTexture, TimeCode};

pub(crate) struct FrameCache {
    capacity: usize,
    byte_len: usize,
    frames: BTreeMap<TimeCode, FrameTexture>,
    order: VecDeque<TimeCode>,
}

impl FrameCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            byte_len: 0,
            frames: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn insert(&mut self, at: TimeCode, frame: FrameTexture) {
        let frame_bytes = frame.rgba.len();
        if let Some(replaced) = self.frames.insert(at, frame) {
            self.byte_len = self.byte_len.saturating_sub(replaced.rgba.len());
            self.order.retain(|entry| *entry != at);
        }
        self.byte_len = self.byte_len.saturating_add(frame_bytes);
        self.order.push_back(at);
        while self.frames.len() > self.capacity {
            self.evict_oldest();
        }
    }

    pub(crate) fn frame_at_or_before(&mut self, at: TimeCode) -> Option<FrameTexture> {
        let key = self.frames.range(..=at).next_back().map(|(key, _)| *key)?;
        let frame = self.frames.get(&key)?.clone();
        self.order.retain(|entry| *entry != key);
        self.order.push_back(key);
        Some(frame)
    }

    pub(crate) fn contains(&self, at: TimeCode) -> bool {
        self.frames.contains_key(&at)
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.byte_len
    }

    pub(crate) fn evict_oldest(&mut self) -> bool {
        let Some(oldest) = self.order.pop_front() else {
            return false;
        };
        if let Some(frame) = self.frames.remove(&oldest) {
            self.byte_len = self.byte_len.saturating_sub(frame.rgba.len());
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
}
