//! Opt-in PO0 observation of decoded pixels on the existing preview worker.
use kinewright_core::TimeCode;
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

type Observer = dyn Fn(usize, TimeCode, u32, u32, &dyn Fn(u32, u32) -> [f32; 3]) + Send + Sync;
static OBSERVER: OnceLock<Box<Observer>> = OnceLock::new();
static LAST: Mutex<Option<(Instant, usize)>> = Mutex::new(None);

/// Installs one bounded, nonblocking sink. Normal launches never install it.
pub fn install(observer: Box<Observer>) -> bool {
    OBSERVER.set(observer).is_ok()
}

pub(crate) fn observe(
    document: &kinewright_core::Document,
    at: TimeCode,
    frame: &crate::frame::WorkingFrame,
) {
    let Some(observer) = OBSERVER.get() else {
        return;
    };
    let key = version_key(document);
    let now = Instant::now();
    {
        let mut last = LAST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|(time, version)| {
            version == key && now.duration_since(time) < Duration::from_millis(250)
        }) {
            return;
        }
        *last = Some((now, key));
    }
    observer(key, at, frame.width, frame.height, &|x, y| {
        let offset = ((y * frame.width + x) * 4) as usize;
        std::array::from_fn(|channel| {
            crate::color_pipeline::encode_bt709(frame.pixels[offset + channel].to_f32())
        })
    });
}

/// Stable across the worker's document clone for the probe's fixed A/B source offsets.
#[must_use]
pub fn version_key(document: &kinewright_core::Document) -> usize {
    document
        .tracks
        .first()
        .and_then(|track| track.clips.first())
        .and_then(|clip| usize::try_from(clip.source_range.start.0).ok())
        .unwrap_or(0)
}
