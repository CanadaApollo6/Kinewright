use std::{
    collections::{HashMap, VecDeque},
    path::Path,
};

use openreel_core::{AssetId, Document, FrameTexture, MediaError, Rational, TimeCode};

use crate::{
    cache::FrameCache,
    compositor::{Compositor, CompositorLayer, GpuContext},
    decode::VideoDecoder,
    video_layers_at,
};

/// Preview decode and compositor output are capped at 720p for 16:9 media.
pub(crate) const PREVIEW_MAX_WIDTH: u32 = 1280;

// 32 entries retain two 15-frame prefetch windows for small/proxy
// sources. The aggregate byte budget, not this per-source count, is the hard
// memory bound for large frames.
const FRAME_CACHE_CAPACITY: usize = 32;
const PREFETCH_FRAMES: i64 = 15;
const FRAME_CACHE_BYTE_BUDGET: usize = 224 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodeStrategy {
    Seek,
    Sequential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderScale {
    FullResolution,
    Proxy { max_width: u32 },
}

impl RenderScale {
    fn max_width(self) -> Option<u32> {
        match self {
            Self::FullResolution => None,
            Self::Proxy { max_width } => Some(max_width.max(1)),
        }
    }

    pub(crate) fn output_resolution(self, source: (u32, u32)) -> (u32, u32) {
        bounded_resolution(source, self.max_width())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VideoSourceKey {
    asset: AssetId,
    max_width: Option<u32>,
}

struct VideoSource {
    decoder: VideoDecoder,
    cache: FrameCache,
}

/// The single frame-rendering path used by both playback preview and export.
pub(crate) struct FrameRenderer {
    // Scale is part of the key so changing preview size can never reuse frames
    // decoded for a different proxy width.
    video_sources: HashMap<VideoSourceKey, VideoSource>,
    source_order: VecDeque<VideoSourceKey>,
    compositor: Compositor,
}

impl FrameRenderer {
    pub(crate) fn new(gpu: GpuContext) -> Self {
        Self {
            video_sources: HashMap::new(),
            source_order: VecDeque::new(),
            compositor: Compositor::new(gpu),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.video_sources.clear();
        self.source_order.clear();
    }

    pub(crate) fn render(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<FrameTexture, MediaError> {
        let layer_specs = video_layers_at(document, project_at)?;
        let mut decoded_layers = Vec::with_capacity(layer_specs.len());
        for layer in layer_specs {
            let asset = document.asset(layer.source.asset).ok_or_else(|| {
                MediaError::Backend(format!("timeline asset {} disappeared", layer.source.asset))
            })?;
            let frame = self.decode_video_frame(
                asset.id,
                &asset.path,
                asset.fps,
                asset.resolution,
                layer.source.source_at,
                layer.source.source_end,
                scale,
                strategy,
            )?;
            decoded_layers.push((frame, layer.effects, layer.transition_alpha));
        }
        let layers = decoded_layers
            .iter()
            .map(|(frame, effects, transition_alpha)| CompositorLayer {
                frame,
                effects,
                transition_alpha: *transition_alpha,
            })
            .collect::<Vec<_>>();
        self.compositor.render(resolution, &layers)
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_video_frame(
        &mut self,
        asset: AssetId,
        path: &Path,
        fps: Rational,
        source_resolution: Option<(u32, u32)>,
        source_at: TimeCode,
        source_end: TimeCode,
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<FrameTexture, MediaError> {
        let key = VideoSourceKey {
            asset,
            max_width: scale.max_width(),
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = self.video_sources.entry(key) {
            let decoder = key.max_width.map_or_else(
                || VideoDecoder::open(path, fps),
                |max_width| VideoDecoder::open_scaled(path, fps, Some(max_width)),
            )?;
            entry.insert(VideoSource {
                decoder,
                cache: FrameCache::new(FRAME_CACHE_CAPACITY),
            });
        }

        let cache_miss = !self
            .video_sources
            .get(&key)
            .is_some_and(|source| source.cache.contains(source_at));
        if cache_miss {
            let frame_bytes = source_resolution
                .map(|resolution| bounded_resolution(resolution, key.max_width))
                .map_or(0, rgba_bytes);
            let prefetch = match strategy {
                // Scrub requests are coalesced and should return the selected
                // frame without decoding work the next mouse move may discard.
                DecodeStrategy::Seek => 0,
                DecodeStrategy::Sequential => prefetch_frames(frame_bytes),
            };
            let end = TimeCode(
                source_at
                    .0
                    .saturating_add(prefetch)
                    .min(source_end.0.saturating_sub(1)),
            );
            let window_frames =
                usize::try_from(end.0.saturating_sub(source_at.0).saturating_add(1))
                    .unwrap_or(usize::MAX);
            self.reserve_cache_bytes(frame_bytes.saturating_mul(window_frames));
            let source = self
                .video_sources
                .get_mut(&key)
                .ok_or_else(|| MediaError::Backend("video decoder cache disappeared".to_owned()))?;
            match strategy {
                DecodeStrategy::Seek => {
                    source
                        .decoder
                        .decode_window(source_at, end, &mut source.cache)?;
                }
                DecodeStrategy::Sequential => {
                    source
                        .decoder
                        .decode_window_sequential(source_at, end, &mut source.cache)?;
                }
            }
        }

        let frame = self
            .video_sources
            .get_mut(&key)
            .and_then(|source| source.cache.frame_at_or_before(source_at))
            .ok_or_else(|| {
                MediaError::Backend(format!(
                    "no video frame decoded for asset {asset} at {source_at}"
                ))
            })?;
        self.touch_source(key);
        self.reserve_cache_bytes(0);
        Ok(frame)
    }

    fn touch_source(&mut self, key: VideoSourceKey) {
        self.source_order.retain(|entry| *entry != key);
        self.source_order.push_back(key);
    }

    fn cache_bytes(&self) -> usize {
        self.video_sources
            .values()
            .map(|source| source.cache.byte_len())
            .fold(0, usize::saturating_add)
    }

    fn reserve_cache_bytes(&mut self, incoming: usize) {
        while self.cache_bytes().saturating_add(incoming) > FRAME_CACHE_BYTE_BUDGET {
            let Some(key) = self.source_order.pop_front() else {
                break;
            };
            let Some(source) = self.video_sources.get_mut(&key) else {
                continue;
            };
            if !source.cache.evict_oldest() {
                continue;
            }
            if source.cache.byte_len() > 0 {
                self.source_order.push_back(key);
            }
        }
    }
}

fn bounded_resolution(source: (u32, u32), max_width: Option<u32>) -> (u32, u32) {
    let source_width = source.0.max(1);
    let source_height = source.1.max(1);
    let width = max_width.unwrap_or(source_width).min(source_width).max(1);
    let height = u32::try_from(
        u64::from(source_height).saturating_mul(u64::from(width)) / u64::from(source_width),
    )
    .unwrap_or(source_height)
    .max(1);
    (width, height)
}

fn rgba_bytes(resolution: (u32, u32)) -> usize {
    usize::try_from(resolution.0)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::try_from(resolution.1).unwrap_or(usize::MAX))
        .saturating_mul(4)
}

fn prefetch_frames(frame_bytes: usize) -> i64 {
    if frame_bytes == 0 {
        return 0;
    }
    let budget_frames = (FRAME_CACHE_BYTE_BUDGET / frame_bytes)
        .min(FRAME_CACHE_CAPACITY)
        .max(1);
    i64::try_from(budget_frames.saturating_sub(1))
        .unwrap_or(i64::MAX)
        .min(PREFETCH_FRAMES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_resolution_preserves_aspect_and_never_upscales() {
        let proxy = RenderScale::Proxy {
            max_width: PREVIEW_MAX_WIDTH,
        };
        assert_eq!(proxy.output_resolution((3840, 2160)), (1280, 720));
        assert_eq!(proxy.output_resolution((640, 360)), (640, 360));
        assert_eq!(proxy.output_resolution((2160, 3840)), (1280, 2275));
    }

    #[test]
    fn proxy_width_is_part_of_decoder_and_cache_identity() {
        let asset = AssetId(7);
        assert_ne!(
            VideoSourceKey {
                asset,
                max_width: Some(1280),
            },
            VideoSourceKey {
                asset,
                max_width: Some(640),
            }
        );
    }

    #[test]
    fn proxy_prefetch_stays_below_the_cache_byte_budget() {
        let bytes = rgba_bytes((1280, 720));
        let cached_frames = usize::try_from(prefetch_frames(bytes) + 1).unwrap();
        assert_eq!(cached_frames, 16);
        assert!(bytes.saturating_mul(cached_frames) < FRAME_CACHE_BYTE_BUDGET);
    }

    #[test]
    fn full_resolution_prefetch_shrinks_to_fit_the_same_budget() {
        let bytes = rgba_bytes((3840, 2160));
        let cached_frames = usize::try_from(prefetch_frames(bytes) + 1).unwrap();
        assert_eq!(cached_frames, 7);
        assert!(bytes.saturating_mul(cached_frames) < FRAME_CACHE_BYTE_BUDGET);
    }
}
