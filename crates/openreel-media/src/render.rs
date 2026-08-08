use std::{collections::HashMap, path::Path};

use openreel_core::{AssetId, Document, FrameTexture, MediaError, Rational, TimeCode};

use crate::{
    cache::FrameCache,
    compositor::{Compositor, CompositorLayer, GpuContext},
    decode::VideoDecoder,
    video_layers_at,
};

const FRAME_CACHE_CAPACITY: usize = 36;
const PREFETCH_FRAMES: i64 = 16;

struct VideoSource {
    decoder: VideoDecoder,
    cache: FrameCache,
}

/// The single frame-rendering path used by both playback preview and export.
pub(crate) struct FrameRenderer {
    video_sources: HashMap<AssetId, VideoSource>,
    compositor: Compositor,
}

impl FrameRenderer {
    pub(crate) fn new(gpu: GpuContext) -> Self {
        Self {
            video_sources: HashMap::new(),
            compositor: Compositor::new(gpu),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.video_sources.clear();
    }

    pub(crate) fn render(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
    ) -> Result<FrameTexture, MediaError> {
        let layer_specs = video_layers_at(document, project_at)?;
        let mut decoded_layers = Vec::with_capacity(layer_specs.len());
        for layer in layer_specs {
            let asset = document.asset(layer.source.asset).ok_or_else(|| {
                MediaError::Backend(format!(
                    "timeline asset {} disappeared",
                    layer.source.asset
                ))
            })?;
            let frame = self.decode_video_frame(
                asset.id,
                &asset.path,
                asset.fps,
                layer.source.source_at,
                layer.source.source_end,
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

    fn decode_video_frame(
        &mut self,
        asset: AssetId,
        path: &Path,
        fps: Rational,
        source_at: TimeCode,
        source_end: TimeCode,
    ) -> Result<FrameTexture, MediaError> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.video_sources.entry(asset) {
            entry.insert(VideoSource {
                decoder: VideoDecoder::open(path, fps)?,
                cache: FrameCache::new(FRAME_CACHE_CAPACITY),
            });
        }
        let source = self
            .video_sources
            .get_mut(&asset)
            .ok_or_else(|| MediaError::Backend("video decoder cache disappeared".to_owned()))?;
        if !source.cache.contains(source_at) {
            let end = TimeCode(
                source_at
                    .0
                    .saturating_add(PREFETCH_FRAMES)
                    .min(source_end.0.saturating_sub(1)),
            );
            source
                .decoder
                .decode_window(source_at, end, &mut source.cache)?;
        }
        source.cache.frame_at_or_before(source_at).ok_or_else(|| {
            MediaError::Backend(format!("no video frame decoded for asset {asset} at {source_at}"))
        })
    }
}
