use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use eframe::egui;
use openreel_core::{AssetId, MediaAsset, TimeCode};
use openreel_media::{
    FfmpegMediaEngine, MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_FILES, ThumbnailKey, VisualAssetResult,
    VisualRequestKind, WaveformData,
};

pub(crate) struct VisualCache {
    results: crossbeam_channel::Receiver<VisualAssetResult>,
    waveforms: HashMap<AssetId, Arc<WaveformData>>,
    requested_waveforms: HashSet<AssetId>,
    failed_waveforms: HashSet<AssetId>,
    thumbnails: HashMap<ThumbnailKey, CachedTexture>,
    requested_thumbnails: HashSet<ThumbnailKey>,
    failed_thumbnails: HashSet<ThumbnailKey>,
    thumbnail_order: VecDeque<ThumbnailKey>,
    thumbnail_bytes: u64,
}

struct CachedTexture {
    texture: egui::TextureHandle,
    bytes: u64,
}

impl VisualCache {
    pub(crate) fn new(results: crossbeam_channel::Receiver<VisualAssetResult>) -> Self {
        Self {
            results,
            waveforms: HashMap::new(),
            requested_waveforms: HashSet::new(),
            failed_waveforms: HashSet::new(),
            thumbnails: HashMap::new(),
            requested_thumbnails: HashSet::new(),
            failed_thumbnails: HashSet::new(),
            thumbnail_order: VecDeque::new(),
            thumbnail_bytes: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.waveforms.clear();
        self.requested_waveforms.clear();
        self.failed_waveforms.clear();
        self.thumbnails.clear();
        self.requested_thumbnails.clear();
        self.failed_thumbnails.clear();
        self.thumbnail_order.clear();
        self.thumbnail_bytes = 0;
        while self.results.try_recv().is_ok() {}
    }

    pub(crate) fn waveform(
        &mut self,
        media: &FfmpegMediaEngine,
        asset: &MediaAsset,
    ) -> Option<Arc<WaveformData>> {
        if let Some(waveform) = self.waveforms.get(&asset.id) {
            return Some(Arc::clone(waveform));
        }
        if !self.failed_waveforms.contains(&asset.id) && self.requested_waveforms.insert(asset.id)
            && !media.request_waveform(asset.clone()) {
                self.requested_waveforms.remove(&asset.id);
            }
        None
    }

    pub(crate) fn thumbnail(
        &mut self,
        media: &FfmpegMediaEngine,
        asset: &MediaAsset,
        source_at: TimeCode,
        max_width: u32,
    ) -> Option<egui::TextureHandle> {
        let key = ThumbnailKey {
            asset: asset.id,
            source_at: TimeCode(
                source_at
                    .0
                    .clamp(0, asset.duration.0.saturating_sub(1).max(0)),
            ),
            max_width: max_width.clamp(1, 512),
        };
        if let Some(cached) = self.thumbnails.get(&key) {
            let texture = cached.texture.clone();
            self.touch_thumbnail(key);
            return Some(texture);
        }
        if !self.failed_thumbnails.contains(&key) && self.requested_thumbnails.insert(key)
            && !media.request_thumbnail(asset.clone(), key.source_at, key.max_width) {
                self.requested_thumbnails.remove(&key);
            }
        None
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) -> Vec<(AssetId, String)> {
        let mut failures = Vec::new();
        let mut changed = false;
        while let Ok(result) = self.results.try_recv() {
            changed = true;
            match result {
                VisualAssetResult::Waveform(waveform) => {
                    self.failed_waveforms.remove(&waveform.asset);
                    self.waveforms.insert(waveform.asset, waveform);
                }
                VisualAssetResult::Thumbnail(frame) => {
                    let width = usize::try_from(frame.image.width).unwrap_or_default();
                    let height = usize::try_from(frame.image.height).unwrap_or_default();
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [width, height],
                        &frame.image.pixels,
                    );
                    let texture = ctx.load_texture(
                        format!(
                            "openreel-thumbnail-{}-{}-{}",
                            frame.key.asset.0, frame.key.source_at.0, frame.key.max_width
                        ),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    let bytes = u64::from(frame.image.width)
                        .saturating_mul(u64::from(frame.image.height))
                        .saturating_mul(4);
                    if let Some(previous) = self
                        .thumbnails
                        .insert(frame.key, CachedTexture { texture, bytes })
                    {
                        self.thumbnail_bytes = self.thumbnail_bytes.saturating_sub(previous.bytes);
                    }
                    self.thumbnail_bytes = self.thumbnail_bytes.saturating_add(bytes);
                    self.touch_thumbnail(frame.key);
                    self.enforce_thumbnail_bounds();
                }
                VisualAssetResult::Failed {
                    asset,
                    request,
                    message,
                } => {
                    match request {
                        VisualRequestKind::Waveform => {
                            self.failed_waveforms.insert(asset);
                        }
                        VisualRequestKind::Thumbnail(key) => {
                            self.failed_thumbnails.insert(key);
                        }
                    }
                    failures.push((asset, message));
                }
            }
        }
        if changed {
            ctx.request_repaint();
        }
        failures
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.requested_waveforms.iter().any(|asset| {
            !self.waveforms.contains_key(asset) && !self.failed_waveforms.contains(asset)
        }) || self
            .requested_thumbnails
            .iter()
            .any(|key| !self.thumbnails.contains_key(key) && !self.failed_thumbnails.contains(key))
    }

    fn touch_thumbnail(&mut self, key: ThumbnailKey) {
        self.thumbnail_order.retain(|entry| *entry != key);
        self.thumbnail_order.push_back(key);
    }

    fn enforce_thumbnail_bounds(&mut self) {
        while self.thumbnails.len() > MAX_THUMBNAIL_FILES
            || self.thumbnail_bytes > MAX_THUMBNAIL_BYTES
        {
            let Some(oldest) = self.thumbnail_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.thumbnails.remove(&oldest) {
                self.thumbnail_bytes = self.thumbnail_bytes.saturating_sub(removed.bytes);
                self.requested_thumbnails.remove(&oldest);
                self.failed_thumbnails.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_lru_never_exceeds_entry_bound() {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let mut cache = VisualCache::new(rx);
        let ctx = egui::Context::default();
        for index in 0..=MAX_THUMBNAIL_FILES {
            let key = ThumbnailKey {
                asset: AssetId(u64::try_from(index).unwrap()),
                source_at: TimeCode::ZERO,
                max_width: 1,
            };
            let texture = ctx.load_texture(
                format!("test-thumbnail-{index}"),
                egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
                egui::TextureOptions::LINEAR,
            );
            cache
                .thumbnails
                .insert(key, CachedTexture { texture, bytes: 4 });
            cache.thumbnail_bytes += 4;
            cache.touch_thumbnail(key);
        }

        cache.enforce_thumbnail_bounds();

        assert_eq!(cache.thumbnails.len(), MAX_THUMBNAIL_FILES);
        assert_eq!(cache.thumbnail_order.len(), MAX_THUMBNAIL_FILES);
    }
}
