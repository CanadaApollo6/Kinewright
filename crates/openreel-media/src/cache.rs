use std::collections::{BTreeMap, VecDeque};

use openreel_core::{FrameTexture, TimeCode};

pub(crate) struct FrameCache {
    capacity: usize,
    frames: BTreeMap<TimeCode, FrameTexture>,
    order: VecDeque<TimeCode>,
}

impl FrameCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frames: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn insert(&mut self, at: TimeCode, frame: FrameTexture) {
        if self.frames.insert(at, frame).is_some() {
            self.order.retain(|entry| *entry != at);
        }
        self.order.push_back(at);
        while self.frames.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.frames.remove(&oldest);
            }
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
}

/// Pick the most recent decoded frame whose presentation time is not after the audio clock.
#[must_use]
pub fn select_frame_for_position(
    available: &[TimeCode],
    position: TimeCode,
) -> Option<TimeCode> {
    available
        .iter()
        .copied()
        .filter(|frame| *frame <= position)
        .max()
        .or_else(|| available.iter().copied().min())
}

#[cfg(test)]
mod tests {
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
}
