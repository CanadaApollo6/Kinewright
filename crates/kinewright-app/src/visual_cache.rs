use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};

use eframe::egui;
use kinewright_core::{
    Analysis, AssetId, MediaAsset, TimeCode, VisualAssetResult, VisualRequestKind, WaveformData,
};
use kinewright_media::{MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_FILES};

/// Cache keys are the asset's PATH, never its id: asset ids are per-document,
/// so with several projects open the same id can name different files, and an
/// id-keyed cache would paint one project's pixels on another's clips. As a
/// bonus, projects sharing media share cached visuals.
type ThumbKey = (PathBuf, TimeCode, u32);

pub(crate) struct VisualCache {
    results: crossbeam_channel::Receiver<VisualAssetResult>,
    waveforms: HashMap<PathBuf, Arc<WaveformData>>,
    requested_waveforms: HashSet<PathBuf>,
    failed_waveforms: HashSet<PathBuf>,
    thumbnails: HashMap<ThumbKey, CachedTexture>,
    requested_thumbnails: HashSet<ThumbKey>,
    failed_thumbnails: HashSet<ThumbKey>,
    thumbnail_order: VecDeque<ThumbKey>,
    thumbnail_bytes: u64,
    blocked_paths: HashSet<PathBuf>,
    path_generations: HashMap<PathBuf, u64>,
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
            blocked_paths: HashSet::new(),
            path_generations: HashMap::new(),
        }
    }

    fn generation(&self, path: &std::path::Path) -> u64 {
        self.path_generations.get(path).copied().unwrap_or(0)
    }

    fn advance_generation(&mut self, path: &std::path::Path) {
        let generation = self.path_generations.entry(path.to_path_buf()).or_default();
        *generation = generation
            .checked_add(1)
            .expect("visual source generation exhausted");
    }

    pub(crate) fn waveform(
        &mut self,
        media: &dyn Analysis,
        asset: &MediaAsset,
    ) -> Option<Arc<WaveformData>> {
        if self.blocked_paths.contains(&asset.path) {
            return None;
        }
        if let Some(waveform) = self.waveforms.get(&asset.path) {
            return Some(Arc::clone(waveform));
        }
        if !self.failed_waveforms.contains(&asset.path)
            && self.requested_waveforms.insert(asset.path.clone())
            && !media.request_waveform(asset.clone(), self.generation(&asset.path))
        {
            self.requested_waveforms.remove(&asset.path);
        }
        None
    }

    pub(crate) fn thumbnail(
        &mut self,
        media: &dyn Analysis,
        asset: &MediaAsset,
        source_at: TimeCode,
        max_width: u32,
    ) -> Option<egui::TextureHandle> {
        if self.blocked_paths.contains(&asset.path) {
            return None;
        }
        let source_at = TimeCode(
            source_at
                .0
                .clamp(0, asset.duration.0.saturating_sub(1).max(0)),
        );
        let max_width = max_width.clamp(1, 512);
        let key: ThumbKey = (asset.path.clone(), source_at, max_width);
        if let Some(cached) = self.thumbnails.get(&key) {
            let texture = cached.texture.clone();
            self.touch_thumbnail(&key);
            return Some(texture);
        }
        if !self.failed_thumbnails.contains(&key)
            && self.requested_thumbnails.insert(key.clone())
            && !media.request_thumbnail(
                asset.clone(),
                source_at,
                max_width,
                self.generation(&asset.path),
            )
        {
            self.requested_thumbnails.remove(&key);
        }
        None
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) -> Vec<(AssetId, String)> {
        let mut failures = Vec::new();
        let mut changed = false;
        while let Ok(result) = self.results.try_recv() {
            changed = true;
            let (path, request_generation) = match &result {
                VisualAssetResult::Waveform(waveform) => {
                    (&waveform.path, waveform.request_generation)
                }
                VisualAssetResult::Thumbnail(frame) => (&frame.path, frame.request_generation),
                VisualAssetResult::Failed {
                    path,
                    request_generation,
                    ..
                } => (path, *request_generation),
            };
            if self.blocked_paths.contains(path) || request_generation != self.generation(path) {
                continue;
            }
            match result {
                VisualAssetResult::Waveform(waveform) => {
                    self.requested_waveforms.remove(&waveform.path);
                    self.failed_waveforms.remove(&waveform.path);
                    self.waveforms.insert(waveform.path.clone(), waveform);
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
                            "kinewright-thumbnail-{}-{}-{}",
                            frame.path.display(),
                            frame.key.source_at.0,
                            frame.key.max_width
                        ),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    let bytes = u64::from(frame.image.width)
                        .saturating_mul(u64::from(frame.image.height))
                        .saturating_mul(4);
                    let key: ThumbKey = (frame.path, frame.key.source_at, frame.key.max_width);
                    self.requested_thumbnails.remove(&key);
                    if let Some(previous) = self
                        .thumbnails
                        .insert(key.clone(), CachedTexture { texture, bytes })
                    {
                        self.thumbnail_bytes = self.thumbnail_bytes.saturating_sub(previous.bytes);
                    }
                    self.thumbnail_bytes = self.thumbnail_bytes.saturating_add(bytes);
                    self.touch_thumbnail(&key);
                    self.enforce_thumbnail_bounds();
                }
                VisualAssetResult::Failed {
                    asset,
                    request_generation: _,
                    path,
                    request,
                    message,
                } => {
                    match request {
                        VisualRequestKind::Waveform => {
                            self.requested_waveforms.remove(&path);
                            self.failed_waveforms.insert(path);
                        }
                        VisualRequestKind::Thumbnail(key) => {
                            let key = (path, key.source_at, key.max_width);
                            self.requested_thumbnails.remove(&key);
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
        self.requested_waveforms
            .iter()
            .any(|path| !self.waveforms.contains_key(path) && !self.failed_waveforms.contains(path))
            || self.requested_thumbnails.iter().any(|key| {
                !self.thumbnails.contains_key(key) && !self.failed_thumbnails.contains(key)
            })
    }

    /// Drop every in-memory visual derived from a source path. Relink can
    /// reuse the same path with a newly verified fingerprint, so path-only UI
    /// caches must not survive the document change even though the persistent
    /// media cache remains content-addressed.
    pub(crate) fn invalidate_path(&mut self, path: &std::path::Path) {
        self.advance_generation(path);
        self.waveforms.remove(path);
        self.requested_waveforms.remove(path);
        self.failed_waveforms.remove(path);

        let keys: Vec<ThumbKey> = self
            .thumbnails
            .keys()
            .filter(|(cached_path, _, _)| cached_path == path)
            .cloned()
            .collect();
        for key in keys {
            if let Some(removed) = self.thumbnails.remove(&key) {
                self.thumbnail_bytes = self.thumbnail_bytes.saturating_sub(removed.bytes);
            }
        }
        self.requested_thumbnails
            .retain(|(cached_path, _, _)| cached_path != path);
        self.failed_thumbnails
            .retain(|(cached_path, _, _)| cached_path != path);
        self.thumbnail_order
            .retain(|(cached_path, _, _)| cached_path != path);
    }

    /// Invalidate a changed source and quarantine its path. While blocked,
    /// callers cannot request visuals and late worker results are discarded.
    pub(crate) fn block_path(&mut self, path: &std::path::Path) {
        self.invalidate_path(path);
        self.blocked_paths.insert(path.to_path_buf());
    }

    /// A fresh online observation or document relink establishes a new source
    /// identity for this path. Invalidate once more before accepting results.
    pub(crate) fn invalidate_and_unblock_path(&mut self, path: &std::path::Path) {
        self.invalidate_path(path);
        self.blocked_paths.remove(path);
    }

    fn touch_thumbnail(&mut self, key: &ThumbKey) {
        self.thumbnail_order.retain(|entry| entry != key);
        self.thumbnail_order.push_back(key.clone());
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

    fn texture(ctx: &egui::Context, name: &str) -> egui::TextureHandle {
        ctx.load_texture(
            name,
            egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
            egui::TextureOptions::LINEAR,
        )
    }

    fn waveform(path: &std::path::Path, hash: &str, request_generation: u64) -> Arc<WaveformData> {
        Arc::new(WaveformData {
            asset: AssetId(1),
            request_generation,
            path: path.to_path_buf(),
            content_sha256: hash.to_owned(),
            source_fps: kinewright_core::Rational::new(24, 1).expect("valid fps"),
            source_frames: TimeCode(10),
            peaks: Vec::new(),
        })
    }

    #[test]
    fn thumbnail_lru_never_exceeds_entry_bound() {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let mut cache = VisualCache::new(rx);
        let ctx = egui::Context::default();
        for index in 0..=MAX_THUMBNAIL_FILES {
            let key: ThumbKey = (
                PathBuf::from(format!("clip-{index}.mp4")),
                TimeCode::ZERO,
                1,
            );
            let texture = ctx.load_texture(
                format!("test-thumbnail-{index}"),
                egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
                egui::TextureOptions::LINEAR,
            );
            cache
                .thumbnails
                .insert(key.clone(), CachedTexture { texture, bytes: 4 });
            cache.thumbnail_bytes += 4;
            cache.touch_thumbnail(&key);
        }

        cache.enforce_thumbnail_bounds();

        assert_eq!(cache.thumbnails.len(), MAX_THUMBNAIL_FILES);
        assert_eq!(cache.thumbnail_order.len(), MAX_THUMBNAIL_FILES);
    }

    #[test]
    fn invalidate_path_drops_all_visual_state_for_only_that_source() {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let mut cache = VisualCache::new(rx);
        let ctx = egui::Context::default();
        let stale_path = PathBuf::from("replaced.mov");
        let retained_path = PathBuf::from("other.mov");
        let stale_key = (stale_path.clone(), TimeCode::ZERO, 64);
        let stale_pending_key = (stale_path.clone(), TimeCode(1), 64);
        let retained_key = (retained_path.clone(), TimeCode::ZERO, 64);

        cache
            .waveforms
            .insert(stale_path.clone(), waveform(&stale_path, "stale", 0));
        cache.requested_waveforms.insert(stale_path.clone());
        cache.failed_waveforms.insert(stale_path.clone());
        cache.thumbnails.insert(
            stale_key.clone(),
            CachedTexture {
                texture: texture(&ctx, "stale-thumbnail"),
                bytes: 4,
            },
        );
        cache.thumbnails.insert(
            retained_key.clone(),
            CachedTexture {
                texture: texture(&ctx, "retained-thumbnail"),
                bytes: 4,
            },
        );
        cache.thumbnail_bytes = 8;
        cache.requested_thumbnails.insert(stale_pending_key.clone());
        cache.failed_thumbnails.insert(stale_pending_key.clone());
        cache.thumbnail_order.push_back(stale_key.clone());
        cache.thumbnail_order.push_back(stale_pending_key);
        cache.thumbnail_order.push_back(retained_key.clone());

        cache.invalidate_path(&stale_path);

        assert!(!cache.waveforms.contains_key(&stale_path));
        assert!(!cache.requested_waveforms.contains(&stale_path));
        assert!(!cache.failed_waveforms.contains(&stale_path));
        assert!(!cache.thumbnails.contains_key(&stale_key));
        assert!(cache.thumbnails.contains_key(&retained_key));
        assert!(
            cache
                .requested_thumbnails
                .iter()
                .all(|(path, _, _)| path != &stale_path)
        );
        assert!(
            cache
                .failed_thumbnails
                .iter()
                .all(|(path, _, _)| path != &stale_path)
        );
        assert!(
            cache
                .thumbnail_order
                .iter()
                .all(|(path, _, _)| path != &stale_path)
        );
        assert_eq!(cache.thumbnail_bytes, 4);
    }

    #[test]
    fn blocked_path_drops_late_results_until_an_online_identity_unblocks_it() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut cache = VisualCache::new(rx);
        let ctx = egui::Context::default();
        let blocked_path = PathBuf::from("replaced.mov");
        let sibling_path = PathBuf::from("other.mov");
        let thumbnail_key = kinewright_core::ThumbnailKey {
            asset: AssetId(1),
            source_at: TimeCode::ZERO,
            max_width: 64,
        };

        let stale_generation = cache.generation(&blocked_path);
        cache.block_path(&blocked_path);
        tx.send(VisualAssetResult::Waveform(waveform(
            &blocked_path,
            "late-stale",
            stale_generation,
        )))
        .expect("send late waveform");
        tx.send(VisualAssetResult::Thumbnail(
            kinewright_core::ThumbnailFrame {
                key: thumbnail_key,
                request_generation: stale_generation,
                path: blocked_path.clone(),
                image: Arc::new(kinewright_core::RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![0, 0, 0, 255],
                }),
            },
        ))
        .expect("send late thumbnail");
        tx.send(VisualAssetResult::Failed {
            asset: AssetId(1),
            request_generation: stale_generation,
            path: blocked_path.clone(),
            request: VisualRequestKind::Waveform,
            message: "late failure".to_owned(),
        })
        .expect("send late failure");
        tx.send(VisualAssetResult::Waveform(waveform(
            &sibling_path,
            "sibling",
            cache.generation(&sibling_path),
        )))
        .expect("send sibling waveform");

        assert!(cache.poll(&ctx).is_empty());
        assert!(!cache.waveforms.contains_key(&blocked_path));
        assert!(cache.waveforms.contains_key(&sibling_path));
        assert!(
            cache
                .thumbnails
                .keys()
                .all(|(path, _, _)| path != &blocked_path)
        );
        assert!(!cache.failed_waveforms.contains(&blocked_path));

        cache.invalidate_and_unblock_path(&blocked_path);
        tx.send(VisualAssetResult::Waveform(waveform(
            &blocked_path,
            "fresh",
            cache.generation(&blocked_path),
        )))
        .expect("send fresh waveform");
        assert!(cache.poll(&ctx).is_empty());
        assert_eq!(
            cache
                .waveforms
                .get(&blocked_path)
                .map(|waveform| waveform.content_sha256.as_str()),
            Some("fresh")
        );
    }

    #[test]
    fn stale_generation_delivered_after_same_path_unblock_cannot_repopulate_cache() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut cache = VisualCache::new(rx);
        let ctx = egui::Context::default();
        let path = PathBuf::from("restored.mov");
        let key = kinewright_core::ThumbnailKey {
            asset: AssetId(1),
            source_at: TimeCode::ZERO,
            max_width: 64,
        };
        let stale_generation = cache.generation(&path);

        cache.block_path(&path);
        cache.invalidate_and_unblock_path(&path);
        let current_generation = cache.generation(&path);
        assert!(current_generation > stale_generation);
        let app_key = (path.clone(), TimeCode::ZERO, 64);
        cache.requested_waveforms.insert(path.clone());
        cache.requested_thumbnails.insert(app_key.clone());

        tx.send(VisualAssetResult::Waveform(waveform(
            &path,
            "stale",
            stale_generation,
        )))
        .expect("send stale waveform");
        tx.send(VisualAssetResult::Thumbnail(
            kinewright_core::ThumbnailFrame {
                key,
                request_generation: stale_generation,
                path: path.clone(),
                image: Arc::new(kinewright_core::RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![255, 0, 0, 255],
                }),
            },
        ))
        .expect("send stale thumbnail");
        tx.send(VisualAssetResult::Failed {
            asset: AssetId(1),
            request_generation: stale_generation,
            path: path.clone(),
            request: VisualRequestKind::Waveform,
            message: "stale failure".to_owned(),
        })
        .expect("send stale failure");

        assert!(cache.poll(&ctx).is_empty());
        assert!(cache.requested_waveforms.contains(&path));
        assert!(cache.requested_thumbnails.contains(&app_key));
        assert!(!cache.failed_waveforms.contains(&path));
        assert!(!cache.waveforms.contains_key(&path));
        assert!(!cache.thumbnails.contains_key(&app_key));

        tx.send(VisualAssetResult::Waveform(waveform(
            &path,
            "fresh",
            current_generation,
        )))
        .expect("send fresh waveform");
        tx.send(VisualAssetResult::Thumbnail(
            kinewright_core::ThumbnailFrame {
                key,
                request_generation: current_generation,
                path: path.clone(),
                image: Arc::new(kinewright_core::RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![0, 255, 0, 255],
                }),
            },
        ))
        .expect("send fresh thumbnail");

        assert!(cache.poll(&ctx).is_empty());
        assert_eq!(
            cache
                .waveforms
                .get(&path)
                .map(|waveform| waveform.content_sha256.as_str()),
            Some("fresh")
        );
        assert!(cache.thumbnails.contains_key(&app_key));
        assert!(!cache.requested_waveforms.contains(&path));
        assert!(!cache.requested_thumbnails.contains(&app_key));
        assert!(!cache.failed_waveforms.contains(&path));
    }
}
