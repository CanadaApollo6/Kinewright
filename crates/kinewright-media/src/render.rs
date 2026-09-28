use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{Arc, LazyLock},
};

use kinewright_core::{
    AssetId, ClipId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorRange, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, Document, Effect,
    EffectId, FrameTexture, LinearRgbaImage, MatteProofError, MediaError, MediaSourceFingerprint,
    Rational, SourceColorRefusal, TimeCode, Title, classify_source_with_assumption,
};

use crate::{
    TimelineVisualLayer,
    cache::FrameCache,
    compositor::{
        Compositor, CompositorLayer, DeliveryFrame, GpuContext, LayerMode, LayerRole,
        MatteRenderTarget, MonitorPurpose,
    },
    decode::VideoDecoder,
    derived_cache::CacheStats,
    frame::WorkingFrame,
    lut_store::LutLibrary,
    timeline::TransitionRenderParams,
    visual_layers_at,
};

/// MO2 R18: the compositor ignores an adjustment's frame and samples the
/// composite below it. Every adjustment layer shares this 1×1 frame, so a
/// render allocates nothing for one (review B F2: K-1 counts only what is
/// allocated).
static ADJUSTMENT_FRAME: LazyLock<WorkingFrame> = LazyLock::new(|| WorkingFrame {
    width: 1,
    height: 1,
    pixels: Arc::new(vec![half::f16::ZERO; 4]),
});

/// Preview decode and compositor output are capped at 720p for 16:9 media.
pub(crate) const PREVIEW_MAX_WIDTH: u32 = 1280;

const FRAME_CACHE_CAPACITY: usize = 32;
pub(crate) const PREFETCH_FRAMES: i64 = 15;
pub(crate) const FRAME_CACHE_BYTE_BUDGET: usize = 224 * 1024 * 1024;

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
            Self::Proxy { max_width } => Some(max_width.clamp(1, PREVIEW_MAX_WIDTH)),
        }
    }

    pub(crate) fn output_resolution(self, source: (u32, u32)) -> (u32, u32) {
        bounded_resolution(source, self.max_width())
    }
}

/// One staged visual layer, in production z-order, before any output
/// transform is selected.
///
/// The originating clip is carried alongside the decoded frame because a
/// matte proof addresses a *clip*, while the compositor addresses layers
/// positionally (CC5 §4.1): the two are reconciled here, on the same layer
/// slice the ordinary render composites, so a proof can never target a layer
/// the production path would not have produced.
struct DecodedLayer {
    clip: ClipId,
    frame: WorkingFrame,
    effects: Vec<Effect>,
    transition: TransitionRenderParams,
    mode: LayerMode,
}

/// MO2 R10: the compositor names the offending layer; the renderer knows
/// which clip and project frame that layer was.
fn attribute_layer(
    layers: &[DecodedLayer],
    at: TimeCode,
) -> impl Fn(MediaError) -> MediaError + '_ {
    move |error| match error {
        MediaError::NonFiniteRender { layer, .. } => MediaError::NonFiniteRender {
            layer,
            clip: layers.get(layer).map(|decoded| decoded.clip),
            at: Some(at),
        },
        other => other,
    }
}

/// One rendered CC5 matte coverage raster: one byte per pixel, in row-major
/// order, carrying `round(255 · clamp(m, 0, 1))` with no transfer function.
///
/// The raster is reported rather than assumed so the caller can derive its
/// full-resolution claim from what actually came back, exactly as the monitor
/// proof does.
pub(crate) struct MatteCoverage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) coverage: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct VideoSourceKey {
    asset: AssetId,
    /// The path is part of the decoder identity even when two paths currently
    /// point at the same bytes. A relink is a runtime input and must not
    /// silently inherit the old decoder's state.
    path: std::path::PathBuf,
    /// Keep the imported content identity in the key so a changed/relinked
    /// source cannot reuse frames retained for the same asset id.
    fingerprint: SourceFingerprintKey,
    /// Managed conversion is configured from the raw description. Include all
    /// fields, including confidence/provenance, so a same-id colour override
    /// always opens a decoder with the new interpretation.
    description: ColorDescriptionKey,
    assumption: Option<ColorSourceProfileAssumption>,
    fps: Rational,
    max_width: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceFingerprintKey {
    content_sha256: Option<String>,
    byte_len: Option<u64>,
}

impl From<&MediaSourceFingerprint> for SourceFingerprintKey {
    fn from(fingerprint: &MediaSourceFingerprint) -> Self {
        Self {
            content_sha256: fingerprint.content_sha256.clone(),
            byte_len: fingerprint.byte_len,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ColorDescriptionKey {
    primaries: ColorPrimaries,
    transfer: ColorTransfer,
    matrix: ColorMatrix,
    range: ColorRange,
    white_point: ColorWhitePoint,
    bit_depth: ColorBitDepth,
    confidence_basis_points: u16,
    provenance: ColorProvenance,
}

impl From<&ColorDescription> for ColorDescriptionKey {
    fn from(description: &ColorDescription) -> Self {
        Self {
            primaries: description.primaries.clone(),
            transfer: description.transfer.clone(),
            matrix: description.matrix.clone(),
            range: description.range.clone(),
            white_point: description.white_point.clone(),
            bit_depth: description.bit_depth.clone(),
            confidence_basis_points: description.confidence_basis_points,
            provenance: description.provenance.clone(),
        }
    }
}

impl VideoSourceKey {
    fn new(
        asset: AssetId,
        path: &Path,
        fingerprint: &MediaSourceFingerprint,
        fps: Rational,
        description: &ColorDescription,
        assumption: Option<ColorSourceProfileAssumption>,
        max_width: Option<u32>,
    ) -> Self {
        Self {
            asset,
            path: path.to_path_buf(),
            fingerprint: fingerprint.into(),
            description: description.into(),
            assumption,
            fps,
            max_width,
        }
    }
}

/// Generated content cached in working space: titles and (MO2 R3) solids,
/// both through `WorkingFrame::from_display_frame`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Generated {
    Title(Title),
    Solid([u8; 3]),
}

type TitleCacheKey = (ClipId, (u32, u32), Generated);

/// PF1 S2b-1: the readers' frames for one render, by (source, time).
pub(crate) type SuppliedFrames = HashMap<(VideoSourceKey, i64), Result<WorkingFrame, MediaError>>;

/// Where a render's video layers come from.
#[derive(Clone, Copy)]
enum Video<'a> {
    /// This renderer's own decoders (today's path, K-6).
    Decode(DecodeStrategy),
    /// The scheduler's readers (S2b-1).
    Supplied(&'a SuppliedFrames),
}

fn source_key(asset: &kinewright_core::MediaAsset, scale: RenderScale) -> VideoSourceKey {
    let description = &asset.color_description;
    let assumption = d65_assumption(description);
    let (fingerprint, max_width) = (&asset.source_fingerprint, scale.max_width());
    let fps = asset.fps;
    VideoSourceKey::new(
        asset.id,
        &asset.path,
        fingerprint,
        fps,
        description,
        assumption,
        max_width,
    )
}

fn no_frame(asset: AssetId, at: TimeCode) -> MediaError {
    MediaError::Backend(format!("no video frame decoded for asset {asset} at {at}"))
}

/// A managed source decoder with `threads` frame threads, its errors
/// wrapped with the asset (IN1: the wrap's one production caller).
fn open_managed(
    asset: AssetId,
    path: &Path,
    fps: Rational,
    max_width: Option<u32>,
    description: &ColorDescription,
    threads: usize,
) -> Result<VideoDecoder, MediaError> {
    let assumption = d65_assumption(description);
    VideoDecoder::open_managed_threads(path, fps, max_width, description, assumption, threads)
        .map_err(|error| {
            contextual_managed_decode_error(asset, path, description, assumption, error)
        })
}

/// PF1 S2b-1: what a reader opens: today's managed decoder for one source.
#[derive(Clone)]
pub(crate) struct SourceSpec {
    asset: AssetId,
    path: std::path::PathBuf,
    fps: Rational,
    description: ColorDescription,
    max_width: Option<u32>,
    /// Working bytes of one frame (S1d's f).
    pub(crate) frame_bytes: usize,
}

impl SourceSpec {
    /// Open with `threads` frame threads; errors as the renderer's open.
    pub(crate) fn open(&self, threads: usize) -> Result<VideoDecoder, MediaError> {
        let (path, description) = (&self.path, &self.description);
        open_managed(
            self.asset,
            path,
            self.fps,
            self.max_width,
            description,
            threads,
        )
    }

    /// The frame at `at`, exactly as the renderer's Seek or Sequential
    /// window would cache it (continuing from the decoder's cursor).
    pub(crate) fn decode(
        &self,
        decoder: &mut VideoDecoder,
        at: i64,
    ) -> Result<WorkingFrame, MediaError> {
        let mut window = FrameCache::new(1);
        decoder.decode_window_sequential(TimeCode(at), TimeCode(at), &mut window)?;
        let frame = window.frame_at_or_before(TimeCode(at));
        frame.ok_or_else(|| no_frame(self.asset, TimeCode(at)))
    }
}

/// PF1 S2b-1: a transport job's demand on the readers.
#[derive(Default)]
pub(crate) struct ReaderDemand {
    /// Each video layer's (source, time), in z-order.
    pub(crate) required: Vec<(VideoSourceKey, i64)>,
    /// Per source: what opens it, and its lookahead times, nearest first.
    pub(crate) sources: HashMap<VideoSourceKey, (SourceSpec, Vec<i64>)>,
    /// K-1: G, the job's generated raster bytes (titles and solids).
    pub(crate) generated: usize,
}

/// The job's required frames at `at`, then up to `horizon` frames of
/// lookahead, each source's ring holding S1d's window
/// clamp(⌊(C − G) / (n·f)⌋, 1, `PREFETCH_FRAMES` + 1). A document the
/// render will refuse gives no demand (the render reports why).
pub(crate) fn reader_demand(
    document: &Document,
    at: TimeCode,
    resolution: (u32, u32),
    scale: RenderScale,
    horizon: i64,
) -> ReaderDemand {
    let mut demand = ReaderDemand::default();
    let mut generated = 0usize;
    let mut ahead: Vec<(VideoSourceKey, i64)> = Vec::new();
    let last = (at.0.saturating_add(horizon)).min(document.duration.0 - 1);
    for frame in at.0..=last.max(at.0) {
        let Ok(layers) = visual_layers_at(document, TimeCode(frame)) else {
            break;
        };
        for layer in layers {
            let video = match layer {
                TimelineVisualLayer::Video(video) => video,
                TimelineVisualLayer::Title(_) | TimelineVisualLayer::Solid(_) if frame == at.0 => {
                    generated = generated.saturating_add(working_bytes(resolution));
                    continue;
                }
                _ => continue,
            };
            let Some(asset) = document.asset(video.source.asset) else {
                continue;
            };
            let key = source_key(asset, scale);
            let max_width = key.max_width;
            demand.sources.entry(key.clone()).or_insert_with(|| {
                let frame_bytes = (asset.resolution)
                    .map_or(0, |size| working_bytes(bounded_resolution(size, max_width)));
                let spec = SourceSpec {
                    asset: asset.id,
                    path: asset.path.clone(),
                    fps: asset.fps,
                    description: asset.color_description.clone(),
                    max_width,
                    frame_bytes,
                };
                (spec, Vec::new())
            });
            let time = video.source.source_at.0;
            if frame == at.0 {
                demand.required.push((key, time));
            } else if !ahead.contains(&(key.clone(), time)) {
                ahead.push((key, time));
            }
        }
    }
    let n = demand.sources.len().max(1);
    let cap = usize::try_from(PREFETCH_FRAMES + 1).unwrap_or(1);
    for (key, (spec, lookahead)) in &mut demand.sources {
        let window = match spec.frame_bytes {
            0 => 1,
            f => (FRAME_CACHE_BYTE_BUDGET.saturating_sub(generated) / f.saturating_mul(n))
                .clamp(1, cap),
        };
        let required: Vec<i64> = (demand.required.iter())
            .filter_map(|(k, t)| (k == key).then_some(*t))
            .collect();
        let room = window.saturating_sub(required.len());
        let own = ahead
            .iter()
            .filter(|(k, t)| k == key && !required.contains(t));
        lookahead.extend(own.map(|(_, t)| *t).take(room));
    }
    demand.generated = generated;
    demand
}

struct VideoSource {
    decoder: VideoDecoder,
    cache: FrameCache<WorkingFrame>,
    /// Counts this open decoder in its engine's gauge while it lives.
    _counted: Option<DecoderHold>,
}

/// PF1 R-5 `sync_decoders` (review B F4): one engine's open decoders, over
/// every renderer it builds (preview, proofs, export).
#[derive(Debug, Clone, Default)]
pub(crate) struct DecoderGauge(Arc<std::sync::atomic::AtomicU64>);

thread_local! {
    /// The gauge renderers built on this thread count their decoders in.
    static DECODER_GAUGE: std::cell::RefCell<Option<DecoderGauge>> =
        const { std::cell::RefCell::new(None) };
}

impl DecoderGauge {
    /// Decoders open now.
    pub(crate) fn open(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Run `build` with this gauge current: the renderers it builds on this
    /// thread count their decoders here.
    pub(crate) fn scope<T>(&self, build: impl FnOnce() -> T) -> T {
        struct Restore(Option<DecoderGauge>);
        impl Drop for Restore {
            fn drop(&mut self) {
                DECODER_GAUGE.with(|gauge| *gauge.borrow_mut() = self.0.take());
            }
        }
        let previous = DECODER_GAUGE.with(|gauge| gauge.borrow_mut().replace(self.clone()));
        let _restore = Restore(previous);
        build()
    }

    fn hold(&self) -> DecoderHold {
        (self.0).fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        DecoderHold(Arc::clone(&self.0))
    }
}

/// One open decoder in a gauge, released when its source drops.
struct DecoderHold(Arc<std::sync::atomic::AtomicU64>);

impl Drop for DecoderHold {
    fn drop(&mut self) {
        (self.0).fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// PF1 K-6: the preview renderer's demand for the frame being rendered: its
/// active sources with their demand points, and its generated raster bytes.
#[derive(Default)]
struct PreviewDemand {
    sources: HashMap<VideoSourceKey, Vec<TimeCode>>,
    generated: usize,
}

impl PreviewDemand {
    /// The Sequential window clamp(⌊(C − G) / (n·f)⌋, 1, `PREFETCH_FRAMES` + 1),
    /// as a prefetch count past the demanded frame.
    fn prefetch(&self, budget: usize, frame_bytes: usize) -> i64 {
        if frame_bytes == 0 {
            return 0;
        }
        let per_source = frame_bytes.saturating_mul(self.sources.len().max(1));
        let window = budget.saturating_sub(self.generated) / per_source;
        let cap = usize::try_from(PREFETCH_FRAMES + 1).unwrap_or(1);
        i64::try_from(window.clamp(1, cap) - 1).unwrap_or(0)
    }
}

/// PF1 K-6 (re-review nit): the preview demand set aside for a thumbnail,
/// restored when this drops, on return or while unwinding.
struct PreviewAside<'a> {
    renderer: &'a mut FrameRenderer,
    preview: Option<PreviewDemand>,
}

impl<'a> PreviewAside<'a> {
    fn new(renderer: &'a mut FrameRenderer) -> Self {
        let preview = renderer.preview.take();
        Self { renderer, preview }
    }
}

impl Drop for PreviewAside<'_> {
    fn drop(&mut self) {
        self.renderer.preview = self.preview.take();
    }
}

/// The single frame-rendering path used by both playback preview and export.
pub(crate) struct FrameRenderer {
    video_sources: HashMap<VideoSourceKey, VideoSource>,
    source_order: VecDeque<VideoSourceKey>,
    compositor: Compositor,
    title_rasterizer: crate::title::TitleRasterizer,
    title_cache: HashMap<TitleCacheKey, WorkingFrame>,
    title_order: VecDeque<TitleCacheKey>,
    /// K-1: during a scheduled render, the title rasters it used; the
    /// cache keeps only these after it (the preview accounts for them).
    used_titles: Option<HashSet<TitleCacheKey>>,
    cache_budget: usize,
    /// PF1 K-6: `Some` for the preview renderer (window cap and eviction by
    /// distance); `None` keeps today's policy for proofs, export and benches.
    preview: Option<PreviewDemand>,
    /// CC4 2.4: the verified lattices every `technical_lut` / `creative_look`
    /// node resolves against. Bound by the caller to the asset hashes of the
    /// document it is about to render; the renderer never opens a LUT file for
    /// a managed node.
    ///
    /// The library is **document-local**. A `FrameRenderer` that is handed
    /// documents from more than one project - the playback worker, the
    /// thumbnail path - must rebind before each of them, because a
    /// `LutAssetId` means nothing outside the document that allocated it.
    ///
    /// An empty library is the honest default for a renderer nobody has bound:
    /// a document with an active LUT node then fails the render with
    /// `missing_lut_asset` instead of quietly dropping the look.
    lut_library: Arc<LutLibrary>,
    /// The engine gauge current when this renderer was built, if any.
    decoders: Option<DecoderGauge>,
}

impl FrameRenderer {
    pub(crate) fn new(gpu: GpuContext) -> Self {
        Self {
            decoders: DECODER_GAUGE.with(|gauge| gauge.borrow().clone()),
            video_sources: HashMap::new(),
            source_order: VecDeque::new(),
            compositor: Compositor::new(gpu),
            title_rasterizer: crate::title::TitleRasterizer::new(),
            title_cache: HashMap::new(),
            title_order: VecDeque::new(),
            used_titles: None,
            cache_budget: FRAME_CACHE_BYTE_BUDGET,
            preview: None,
            lut_library: Arc::new(LutLibrary::default()),
        }
    }

    /// PF1 K-6: the playback preview's renderer.
    pub(crate) fn new_preview(gpu: GpuContext) -> Self {
        Self {
            preview: Some(PreviewDemand::default()),
            ..Self::new(gpu)
        }
    }

    /// Bind the verified LUT library this renderer resolves LUT nodes against
    /// (CC4 2.4).
    ///
    /// The compositor's atlas cache keys on the identity of the verified
    /// lattices themselves and retains a strong `Arc` to each one, so a
    /// rebuilt library whose assets parsed into fresh allocations misses the
    /// cache and re-uploads: a restored or replaced asset can never be served
    /// from a stale atlas. An unchanged library that hands back the same
    /// lattices still hits, which is what keeps steady-state playback from
    /// re-uploading the atlas every frame.
    pub(crate) fn set_lut_library(&mut self, library: Arc<LutLibrary>) {
        self.lut_library = library;
    }

    pub(crate) fn clear(&mut self) -> CacheStats {
        let stats = self.cache_stats();
        self.video_sources.clear();
        self.source_order.clear();
        self.title_cache.clear();
        self.title_order.clear();
        stats
    }

    pub(crate) fn cache_stats(&self) -> CacheStats {
        let frame_count = self
            .video_sources
            .values()
            .map(|source| source.cache.len())
            .sum::<usize>();
        let frame_bytes = self.cache_bytes();
        let title_bytes = self.title_cache_bytes();
        CacheStats {
            file_count: u64::try_from(frame_count.saturating_add(self.title_cache.len()))
                .unwrap_or(u64::MAX),
            bytes: u64::try_from(frame_bytes.saturating_add(title_bytes)).unwrap_or(u64::MAX),
        }
    }

    /// Return the configured aggregate working-cache budget for objective
    /// media evidence. This remains crate-visible and test-only so production
    /// callers cannot make cache policy part of the public API.
    #[cfg(test)]
    pub(crate) const fn cache_budget_bytes(&self) -> usize {
        self.cache_budget
    }

    /// Return the number of real working-frame evictions observed by the
    /// managed renderer. This is a test-only diagnostic for the bounded-cache
    /// evidence fixture; production cache policy remains unchanged.
    #[cfg(test)]
    pub(crate) fn cache_eviction_count(&self) -> usize {
        self.video_sources
            .values()
            .map(|source| source.cache.eviction_count())
            .sum()
    }

    /// Total decoder seeks across video sources — the test-only churn
    /// signal. A pinned still decodes once (one seek) no matter how many
    /// frames render from it; production decode policy is unchanged.
    #[cfg(test)]
    pub(crate) fn video_seek_count(&self) -> u64 {
        self.video_sources
            .values()
            .map(|source| source.decoder.seek_count())
            .sum()
    }

    /// Composite one project frame for the document's monitoring target.
    ///
    /// CC1 2.2.6 requires the monitor transform to be selected from the
    /// monitoring `ColorDescription`, so the document's own description is
    /// handed to the compositor rather than a compositor default.
    ///
    /// # Errors
    ///
    /// Returns a media error when the colour context, decode, or GPU
    /// readback fails.
    pub(crate) fn render(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<FrameTexture, MediaError> {
        let purpose = MonitorPurpose::Proof;
        let rendered =
            self.render_timed(document, project_at, resolution, (scale, strategy), purpose);
        rendered.map(|(frame, _)| frame)
    }

    /// PF1 G-1: [`Self::render`] for the live preview monitor, the only
    /// caller whose encode goes through the exact BT.709 table.
    pub(crate) fn render_live(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<FrameTexture, MediaError> {
        let purpose = MonitorPurpose::LiveMonitor;
        let rendered =
            self.render_timed(document, project_at, resolution, (scale, strategy), purpose);
        rendered.map(|(frame, _)| frame)
    }

    /// PF1 K-6 (review B F5): a thumbnail on the preview renderer keeps
    /// today's policy: it neither rebuilds the preview's demand nor runs its
    /// window, pins or distance eviction.
    pub(crate) fn render_thumbnail(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
    ) -> Result<FrameTexture, MediaError> {
        let aside = PreviewAside::new(self);
        let strategy = DecodeStrategy::Seek;
        aside
            .renderer
            .render(document, project_at, resolution, scale, strategy)
    }

    /// MO2 R28 (ME13): [`Self::render`] and its compositor frame time —
    /// render, readback and monitor encode, the decoded layers resident.
    pub(crate) fn render_timed(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        (scale, strategy): (RenderScale, DecodeStrategy),
        purpose: MonitorPurpose,
    ) -> Result<(FrameTexture, std::time::Duration), MediaError> {
        self.render_monitor_from(
            document,
            project_at,
            resolution,
            (scale, Video::Decode(strategy)),
            purpose,
        )
    }

    /// PF1 S2b-1: the live monitor from the readers' frames (C-5: the same
    /// layers, compositor and encode as [`Self::render_live`]).
    pub(crate) fn render_scheduled(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        frames: &SuppliedFrames,
    ) -> Result<FrameTexture, MediaError> {
        let purpose = MonitorPurpose::LiveMonitor;
        let video = (scale, Video::Supplied(frames));
        self.used_titles = Some(HashSet::new());
        let rendered = self.render_monitor_from(document, project_at, resolution, video, purpose);
        let used = self.used_titles.take().unwrap_or_default();
        self.title_cache.retain(|key, _| used.contains(key));
        self.title_order.retain(|key| used.contains(key));
        rendered.map(|(frame, _)| frame)
    }

    /// H-7: whether a synchronous decoder is open.
    pub(crate) fn has_sources(&self) -> bool {
        !self.video_sources.is_empty()
    }

    /// H-7: close the synchronous decoders and drop their frames (the
    /// preview parks; G15).
    pub(crate) fn release_sources(&mut self) {
        self.video_sources.clear();
        self.source_order.clear();
    }

    /// K-1: the bytes of the title rasters cached now.
    pub(crate) fn title_bytes(&self) -> usize {
        self.title_cache_bytes()
    }

    /// K-5: drop the cached title rasters (the preview is draining).
    pub(crate) fn clear_titles(&mut self) {
        self.title_cache.clear();
        self.title_order.clear();
    }

    fn render_monitor_from(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        (scale, video): (RenderScale, Video<'_>),
        purpose: MonitorPurpose,
    ) -> Result<(FrameTexture, std::time::Duration), MediaError> {
        let decoded_layers = self.decoded_layers(document, project_at, resolution, scale, video)?;
        let started = std::time::Instant::now();
        let layers = compositor_layers(&decoded_layers);
        self.compositor
            .render_monitor_for(
                resolution,
                &layers,
                &document.color_context.monitoring,
                Some(&self.lut_library),
                purpose,
            )
            .map_err(attribute_layer(&decoded_layers, project_at))
            .map(|frame| (frame, started.elapsed()))
    }

    /// MO2 R28 (ME13): hold decoded sources resident for compositor frames.
    #[cfg(test)]
    pub(crate) fn set_cache_budget(&mut self, bytes: usize) {
        self.cache_budget = bytes;
    }

    /// Composite one project frame for the document's delivery target.
    ///
    /// CC1 5 permits only the final target, raster, and codec quantization to
    /// differ between preview/proof and export. Everything up to the output
    /// transform is the same production path as [`Self::render`].
    ///
    /// # Errors
    ///
    /// Returns a media error when the colour context, decode, or GPU
    /// readback fails.
    pub(crate) fn render_delivery(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<DeliveryFrame, MediaError> {
        let decoded_layers = self.decoded_layers(
            document,
            project_at,
            resolution,
            scale,
            Video::Decode(strategy),
        )?;
        let layers = compositor_layers(&decoded_layers);
        self.compositor
            .render_delivery_with_luts(
                resolution,
                &layers,
                &document.color_context.delivery,
                Some(&self.lut_library),
            )
            .map_err(attribute_layer(&decoded_layers, project_at))
    }

    /// Composite one project frame's **scene-linear working surface** (CC6
    /// §2.2).
    ///
    /// Layers are resolved exactly as [`Self::render`] and
    /// [`Self::render_delivery`] resolve them — the same `decoded_layers`, the
    /// same `compositor_layers`, the same LUT library — and the only
    /// difference is the readback that is asked for: the `Rgba16Float`
    /// composite target read back verbatim, with no transfer and no clamp.
    ///
    /// # Errors
    ///
    /// Returns a media error when the colour context, decode, or GPU
    /// readback fails.
    pub(crate) fn render_working(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
    ) -> Result<LinearRgbaImage, MediaError> {
        let decoded_layers = self.decoded_layers(
            document,
            project_at,
            resolution,
            scale,
            Video::Decode(strategy),
        )?;
        let layers = compositor_layers(&decoded_layers);
        self.compositor
            .render_working_with_luts(resolution, &layers, Some(&self.lut_library))
            .map_err(attribute_layer(&decoded_layers, project_at))
    }

    /// Render one clip's CC5 matte coverage instead of its colour.
    ///
    /// The layers are resolved exactly as [`Self::render`] resolves them — the
    /// same `visual_layers_at` order, the same decode, the same
    /// keyframe-evaluated effects — and the *target clip's* layer index is
    /// handed to the compositor, which composites that layer alone with the
    /// CC5 §3.2 matte-debug selector set. A clip that is not an active visual
    /// layer at this frame therefore cannot be proved, and fails typed rather
    /// than proving whatever layer happened to sit at that index.
    ///
    /// # Errors
    ///
    /// Returns the typed [`MatteProofError`] failures when the clip is not an
    /// active visual layer, or when its target node is missing, is not a
    /// colour node, is inactive at this frame, or carries no matte, plus the
    /// ordinary colour-context, decode, and GPU readback failures.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_matte(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        strategy: DecodeStrategy,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<MatteCoverage, MediaError> {
        let decoded_layers = self.decoded_layers(
            document,
            project_at,
            resolution,
            scale,
            Video::Decode(strategy),
        )?;
        let layer_index = decoded_layers
            .iter()
            .position(|layer| layer.clip == clip)
            .ok_or(MatteProofError::ClipNotVisible {
                clip,
                at: project_at,
            })?;
        let layers = compositor_layers(&decoded_layers);
        let coverage = self
            .compositor
            .render_matte(
                resolution,
                &layers,
                Some(&self.lut_library),
                MatteRenderTarget {
                    layer_index,
                    clip,
                    effect,
                },
            )
            .map_err(attribute_layer(&decoded_layers, project_at))?;
        let (width, height) = resolution;
        Ok(MatteCoverage {
            width,
            height,
            coverage,
        })
    }

    /// Decode and stage every visual layer for one project frame, in
    /// production z-order, before any output transform is selected.
    fn decoded_layers(
        &mut self,
        document: &Document,
        project_at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
        video: Video<'_>,
    ) -> Result<Vec<DecodedLayer>, MediaError> {
        validate_managed_context(document)?;
        // MO2 R8 (review-1 B1): every render root refuses an invalid
        // document defensively, disabled clips included, before resolving.
        document
            .validate()
            .map_err(|error| MediaError::InvalidDocument(Box::new(error)))?;
        let layer_specs = visual_layers_at(document, project_at)?;
        if let Video::Decode(_) = video {
            self.set_demand(document, &layer_specs, resolution, scale);
        }
        let mut decoded_layers = Vec::with_capacity(layer_specs.len());
        for layer in layer_specs {
            match layer {
                TimelineVisualLayer::Video(layer) => {
                    let asset = document.asset(layer.source.asset).ok_or_else(|| {
                        MediaError::Backend(format!(
                            "timeline asset {} disappeared",
                            layer.source.asset
                        ))
                    })?;
                    let frame = match video {
                        Video::Decode(strategy) => self.decode_video_frame(
                            asset.id,
                            &asset.path,
                            asset.fps,
                            asset.resolution,
                            layer.source.source_at,
                            layer.source.source_end,
                            scale,
                            strategy,
                            &asset.source_fingerprint,
                            &asset.color_description,
                        )?,
                        Video::Supplied(frames) => {
                            let at = layer.source.source_at;
                            let key = (source_key(asset, scale), at.0);
                            frames
                                .get(&key)
                                .cloned()
                                .unwrap_or_else(|| Err(no_frame(asset.id, at)))?
                        }
                    };
                    decoded_layers.push(DecodedLayer {
                        clip: layer.source.clip,
                        frame,
                        effects: layer.effects,
                        transition: layer.transition,
                        mode: pixels(layer.blend_mode),
                    });
                }
                TimelineVisualLayer::Title(layer) => {
                    let content = Generated::Title(layer.title);
                    decoded_layers.push(DecodedLayer {
                        clip: layer.clip,
                        frame: self.generated_frame(layer.clip, resolution, content)?,
                        effects: layer.effects,
                        transition: layer.transition,
                        mode: pixels(layer.blend_mode),
                    });
                }
                TimelineVisualLayer::Solid(layer) => {
                    let content = Generated::Solid([layer.color.r, layer.color.g, layer.color.b]);
                    decoded_layers.push(DecodedLayer {
                        clip: layer.clip,
                        frame: self.generated_frame(layer.clip, resolution, content)?,
                        effects: layer.effects,
                        transition: layer.transition,
                        mode: pixels(layer.blend_mode),
                    });
                }
                // MO2 R18: the compositor ignores an adjustment's frame and
                // samples the composite below it instead.
                TimelineVisualLayer::Adjustment(layer) => decoded_layers.push(DecodedLayer {
                    clip: layer.clip,
                    frame: ADJUSTMENT_FRAME.clone(),
                    effects: layer.effects,
                    transition: layer.transition,
                    mode: LayerMode {
                        blend: layer.blend_mode,
                        role: LayerRole::Adjustment,
                    },
                }),
            }
        }
        Ok(decoded_layers)
    }

    /// A title raster or (MO2 R3) an opaque solid fill, converted to working
    /// space by the shared generated-content path and cached.
    fn generated_frame(
        &mut self,
        clip: ClipId,
        resolution: (u32, u32),
        content: Generated,
    ) -> Result<WorkingFrame, MediaError> {
        let key = (clip, resolution, content);
        if let Some(used) = &mut self.used_titles {
            used.insert(key.clone());
        }
        if let Some(frame) = self.title_cache.get(&key).cloned() {
            self.touch_title(key);
            return Ok(frame);
        }
        let display_frame = match &key.2 {
            Generated::Title(title) => self.title_rasterizer.rasterize(title, resolution)?,
            Generated::Solid([r, g, b]) => FrameTexture {
                width: resolution.0,
                height: resolution.1,
                rgba: Arc::new([*r, *g, *b, u8::MAX].repeat(rgba_bytes(resolution) / 4)),
            },
        };
        let frame = WorkingFrame::from_display_frame(&display_frame)?;
        self.cache_title_frame(key, frame.clone());
        Ok(frame)
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
        fingerprint: &MediaSourceFingerprint,
        description: &ColorDescription,
    ) -> Result<WorkingFrame, MediaError> {
        let assumption = d65_assumption(description);
        let key = VideoSourceKey::new(
            asset,
            path,
            fingerprint,
            fps,
            description,
            assumption,
            scale.max_width(),
        );
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.video_sources.entry(key.clone())
        {
            let threads = crate::decode::default_threads();
            let decoder = open_managed(asset, path, fps, key.max_width, description, threads)?;
            let mut cache = FrameCache::new(FRAME_CACHE_CAPACITY);
            let demand = self
                .preview
                .as_ref()
                .and_then(|demand| demand.sources.get(&key));
            if let Some(points) = demand {
                cache.set_demand(points);
            }
            entry.insert(VideoSource {
                decoder,
                cache,
                _counted: self.decoders.as_ref().map(DecoderGauge::hold),
            });
        }

        let cache_miss = !self
            .video_sources
            .get(&key)
            .is_some_and(|source| source.cache.contains(source_at));
        if cache_miss {
            let frame_bytes = source_resolution
                .map(|resolution| bounded_resolution(resolution, key.max_width))
                .map_or(0, working_bytes);
            let prefetch = match strategy {
                DecodeStrategy::Seek => 0,
                DecodeStrategy::Sequential => match &self.preview {
                    Some(demand) => demand.prefetch(self.cache_budget, frame_bytes),
                    None => prefetch_frames(frame_bytes),
                },
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
            self.reserve_for(frame_bytes.saturating_mul(window_frames), Some(&key));
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
            .and_then(|source| {
                source
                    .cache
                    .frame_at_or_before_bounded(source_at, self.cache_budget)
            })
            .ok_or_else(|| no_frame(asset, source_at))?;
        self.touch_source(key.clone());
        self.reserve_for(0, Some(&key));
        Ok(frame)
    }

    /// PF1 K-6: the preview's demand for this frame (its active sources with
    /// their demand points, and its generated raster bytes), handed to the
    /// source caches as pins; outside the preview every cache pins nothing.
    fn set_demand(
        &mut self,
        document: &Document,
        layer_specs: &[TimelineVisualLayer],
        resolution: (u32, u32),
        scale: RenderScale,
    ) {
        if let Some(demand) = &mut self.preview {
            *demand = PreviewDemand::default();
            for layer in layer_specs {
                match layer {
                    TimelineVisualLayer::Video(layer) => {
                        let Some(asset) = document.asset(layer.source.asset) else {
                            continue;
                        };
                        let key = source_key(asset, scale);
                        let points = demand.sources.entry(key).or_default();
                        points.push(layer.source.source_at);
                    }
                    TimelineVisualLayer::Title(_) | TimelineVisualLayer::Solid(_) => {
                        demand.generated += working_bytes(resolution);
                    }
                    TimelineVisualLayer::Adjustment(_) => {}
                }
            }
        }
        self.pin_demand();
    }

    /// PF1 K-5/K-6: hand each source cache this render's demand points, so
    /// the frames they show are pinned and eviction knows the travel; a
    /// source with no demand, and every source outside the preview, pins
    /// nothing.
    fn pin_demand(&mut self) {
        let demand = self.preview.as_ref().map(|demand| &demand.sources);
        for (key, source) in &mut self.video_sources {
            match demand.and_then(|sources| sources.get(key)) {
                Some(points) => source.cache.set_demand(points),
                None => source.cache.clear_demand(),
            }
        }
    }

    fn touch_source(&mut self, key: VideoSourceKey) {
        self.source_order.retain(|entry| entry != &key);
        self.source_order.push_back(key);
    }

    fn touch_title(&mut self, key: TitleCacheKey) {
        self.title_order.retain(|entry| *entry != key);
        self.title_order.push_back(key);
    }

    fn cache_bytes(&self) -> usize {
        self.video_sources
            .values()
            .map(|source| source.cache.byte_len())
            .fold(0, usize::saturating_add)
    }

    fn title_cache_bytes(&self) -> usize {
        self.title_cache
            .values()
            .map(WorkingFrame::byte_len)
            .fold(0, usize::saturating_add)
    }

    fn total_cache_bytes(&self) -> usize {
        self.cache_bytes().saturating_add(self.title_cache_bytes())
    }

    fn cache_title_frame(&mut self, key: TitleCacheKey, frame: WorkingFrame) -> bool {
        let incoming = frame.byte_len();
        if incoming > self.cache_budget {
            return false;
        }
        if self.title_cache.remove(&key).is_some() {
            self.title_order.retain(|entry| *entry != key);
        }
        self.reserve_cache_bytes(incoming);
        if self.total_cache_bytes().saturating_add(incoming) > self.cache_budget {
            return false;
        }
        self.title_cache.insert(key.clone(), frame);
        self.touch_title(key);
        true
    }

    fn reserve_cache_bytes(&mut self, incoming: usize) {
        self.reserve_for(incoming, None);
    }

    /// Make room for `incoming` bytes that `requesting` is about to decode.
    fn reserve_for(&mut self, incoming: usize, requesting: Option<&VideoSourceKey>) {
        while self.total_cache_bytes().saturating_add(incoming) > self.cache_budget {
            if self.evict_video_frame(incoming, requesting) || self.evict_oldest_title_frame() {
                continue;
            }
            break;
        }
    }

    /// Today's round-robin eviction, or (K-6, preview) inactive sources
    /// first, then the sources over their share, farthest frame first.
    fn evict_video_frame(&mut self, incoming: usize, requesting: Option<&VideoSourceKey>) -> bool {
        let Some(demand) = &self.preview else {
            return self.evict_oldest_video_frame();
        };
        let inactive = self
            .video_sources
            .iter_mut()
            .find(|(key, source)| !demand.sources.contains_key(*key) && source.cache.len() > 0);
        if let Some((_, source)) = inactive {
            return source.cache.evict_oldest();
        }
        let share =
            self.cache_budget.saturating_sub(demand.generated) / demand.sources.len().max(1);
        let mut over = demand
            .sources
            .keys()
            .filter_map(|key| {
                let held = self.video_sources.get(key)?.cache.byte_len();
                let bytes = held.saturating_add(if requesting == Some(key) { incoming } else { 0 });
                (bytes > share).then_some((bytes, key))
            })
            .collect::<Vec<_>>();
        over.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
        over.into_iter().any(|(_, key)| {
            self.video_sources
                .get_mut(key)
                .is_some_and(|source| source.cache.evict_farthest())
        })
    }

    fn evict_oldest_video_frame(&mut self) -> bool {
        let Some(key) = self.source_order.pop_front() else {
            return false;
        };
        let Some(source) = self.video_sources.get_mut(&key) else {
            return true;
        };
        let _ = source.cache.evict_oldest();
        if source.cache.byte_len() > 0 {
            self.source_order.push_back(key);
        }
        true
    }

    fn evict_oldest_title_frame(&mut self) -> bool {
        let Some(key) = self.title_order.pop_front() else {
            return false;
        };
        self.title_cache.remove(&key);
        true
    }
}

/// Render the CC1 2.1 structured source-colour status for one asset.
///
/// The spec requires the asset, the unsupported field, the observed value,
/// the allowed values, and a recovery action. Core owns the classifier and
/// the allowed-value policy, so this only formats its structured accessors;
/// a supported description still reports so, because the wrapped failure may
/// be a decoder-format problem rather than a metadata problem.
fn managed_source_color_status(
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
) -> String {
    match classify_source_with_assumption(description, assumption) {
        Ok(profile) => format!("source_profile={profile:?}, source_color=supported"),
        Err(error) => format!(
            "source_color={}, field={}, observed={}, allowed={}, recovery={}",
            error.code(),
            error.field(),
            error.observed(),
            error.allowed_values(),
            error.recovery_action(),
        ),
    }
}

pub(crate) fn contextual_managed_decode_error(
    asset: AssetId,
    path: &Path,
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
    error: MediaError,
) -> MediaError {
    match error {
        MediaError::UnsupportedDecoderFormat {
            path: error_path,
            format,
            declared_bit_depth,
            decoder_bit_depth,
            reason,
        } => MediaError::UnsupportedDecoderFormat {
            path: error_path,
            format,
            declared_bit_depth,
            decoder_bit_depth,
            reason: format!(
                "managed decode for asset {asset} ({}) failed: {reason} [{}, assumption={assumption:?}, description={description:?}]. Recovery: apply an explicit supported source-colour override, transcode to a supported integer format, or relink to compatible media.",
                path.display(),
                managed_source_color_status(description, assumption)
            ),
        },
        // IN1 §4.2 rule 9. Placed before the catch-all so the typed refusal
        // keeps its type: carrying `description` and `assumption` on the
        // variant is what lets the app build the incident without re-probing
        // and without parsing (§5.2 rule 5), and what lets
        // `SourceColorRefusal`'s template render the same `[source_color=…]`
        // tail from the variant alone rather than by re-running the classifier
        // — which is why `managed_source_color_status` is now called inside the
        // two arms that need it rather than once above the `match`.
        MediaError::SourceColor(error) => {
            MediaError::SourceColorForAsset(Box::new(SourceColorRefusal {
                asset,
                path: path.to_path_buf(),
                error,
                description: description.clone(),
                assumption,
            }))
        }
        error => MediaError::Backend(format!(
            "managed decode for asset {asset} ({}) failed: {error} [{}, assumption={assumption:?}, description={description:?}]. Recovery: apply an explicit supported source-colour override, transcode to a supported integer format, or relink to compatible media.",
            path.display(),
            managed_source_color_status(description, assumption)
        )),
    }
}

fn validate_managed_context(document: &Document) -> Result<(), MediaError> {
    if document.color_context.is_managed_sdr_compatible() {
        return Ok(());
    }

    Err(MediaError::Backend(format!(
        "managed renderer cannot execute this project colour context: pipeline_state={:?}, working={:?}, monitoring={:?}, delivery={:?}; reset the project to Managed SDR v1 or choose an explicit compatible user override",
        document.color_context.pipeline_state,
        document.color_context.working,
        document.color_context.monitoring,
        document.color_context.delivery,
    )))
}

/// Borrow the staged layers as compositor layers, preserving z-order.
fn compositor_layers(layers: &[DecodedLayer]) -> Vec<CompositorLayer<'_, WorkingFrame>> {
    layers
        .iter()
        .map(|layer| CompositorLayer {
            frame: &layer.frame,
            effects: &layer.effects,
            transition: layer.transition,
            mode: layer.mode,
        })
        .collect()
}

const fn pixels(blend: kinewright_core::BlendMode) -> LayerMode {
    LayerMode {
        blend,
        role: LayerRole::Pixels,
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

fn working_bytes(resolution: (u32, u32)) -> usize {
    rgba_bytes(resolution).saturating_mul(2)
}

pub(crate) fn d65_assumption(
    description: &ColorDescription,
) -> Option<ColorSourceProfileAssumption> {
    (matches!(description.primaries, ColorPrimaries::Bt709)
        && matches!(
            description.white_point,
            kinewright_core::ColorWhitePoint::Unknown
        ))
    .then_some(ColorSourceProfileAssumption::D65)
}

fn prefetch_frames(frame_bytes: usize) -> i64 {
    if frame_bytes == 0 {
        return 0;
    }
    let budget_frames = (FRAME_CACHE_BYTE_BUDGET / frame_bytes).clamp(1, FRAME_CACHE_CAPACITY);
    i64::try_from(budget_frames.saturating_sub(1))
        .unwrap_or(i64::MAX)
        .min(PREFETCH_FRAMES)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use std::collections::BTreeMap;

    use half::f16;
    use kinewright_core::{
        Analysis, AssetId, Clip, ClipContent, ColorPipelineState, ColorTransfer, EffectId,
        LutAssetId, MediaAsset, ParamValue, Track, TrackId, TrackKind,
    };

    use super::*;
    use crate::{
        decode::probe_path,
        gpu_test_support::fixture_gpu_or_skip,
        initialize_ffmpeg,
        lut_store::LutStore,
        test_support::{GeneratedMedia, TempDirectory, single_clip_document},
    };

    fn test_renderer() -> Option<FrameRenderer> {
        Some(FrameRenderer::new(fixture_gpu_or_skip()?))
    }

    impl FrameRenderer {
        /// MO2 R16: the CPU twin over the production resolution and decode.
        pub(crate) fn twin_working(
            &mut self,
            document: &Document,
            project_at: TimeCode,
            resolution: (u32, u32),
        ) -> Result<LinearRgbaImage, MediaError> {
            let (scale, strategy) = (RenderScale::FullResolution, DecodeStrategy::Seek);
            let decoded = self.decoded_layers(
                document,
                project_at,
                resolution,
                scale,
                Video::Decode(strategy),
            )?;
            let layers = compositor_layers(&decoded);
            crate::compositor::twin::render_working(resolution, &layers, Some(&self.lut_library))
                .map_err(attribute_layer(&decoded, project_at))
        }

        /// MO2 ME9: the twin's 8-bit sub-texel envelope for the same frame.
        pub(crate) fn twin_envelope(
            &mut self,
            document: &Document,
            project_at: TimeCode,
            resolution: (u32, u32),
        ) -> Result<Vec<f32>, MediaError> {
            let (scale, strategy) = (RenderScale::FullResolution, DecodeStrategy::Seek);
            let decoded = self.decoded_layers(
                document,
                project_at,
                resolution,
                scale,
                Video::Decode(strategy),
            )?;
            let layers = compositor_layers(&decoded);
            let library = Some(&*self.lut_library);
            crate::compositor::twin::subtexel_envelope(resolution, &layers, library)
        }

        /// MO2 R12/R13: accumulator snapshots this renderer has taken.
        pub(crate) fn accumulator_copies(&self) -> u64 {
            self.compositor.accumulator_copies()
        }
    }

    fn working_frame_with_bytes(bytes: usize) -> WorkingFrame {
        assert_eq!(bytes % std::mem::size_of::<f16>(), 0);
        WorkingFrame {
            width: 1,
            height: 1,
            pixels: Arc::new(vec![f16::from_f32(0.0); bytes / std::mem::size_of::<f16>()]),
        }
    }

    fn title_key(id: u64) -> TitleCacheKey {
        (ClipId(id), (1, 1), Generated::Title(Title::default()))
    }

    /// A one-clip title timeline, so the LUT plumbing can be exercised without
    /// a decoder in the path.
    fn title_document(effects: Vec<Effect>) -> Document {
        Document {
            resolution: (320, 180),
            duration: TimeCode(4),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(1),
                    asset: AssetId::default(),
                    source_range: TimeCode(0)..TimeCode(4),
                    content: ClipContent::Title(Title {
                        text: "CC4".to_owned(),
                        ..Title::default()
                    }),
                    timeline_start: TimeCode::ZERO,
                    effects,
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: kinewright_core::BlendMode::Normal,
                }],
            }],
            ..Document::default()
        }
    }

    #[test]
    fn a_published_lut_library_reaches_the_compositor_through_the_frame_renderer() {
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        let mut look = Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        };
        look.parameters
            .insert("lut_asset_id".to_owned(), ParamValue::Integer(1));
        // MO2 R8: the render entry validates the document, so the look's
        // asset is registered; only the renderer's library is unpublished.
        let mut document = title_document(vec![look]);

        let directory = TempDirectory::new("cc4-renderer-library");
        let store = LutStore::for_project(&directory.path("project.kinewright"))
            .expect("a temporary project derives a store root");
        let source = directory.path("identity.cube");
        std::fs::write(
            &source,
            "LUT_3D_SIZE 2
             0.000000 0.000000 0.000000
1.000000 0.000000 0.000000
             0.000000 1.000000 0.000000
1.000000 1.000000 0.000000
             0.000000 0.000000 1.000000
1.000000 0.000000 1.000000
             0.000000 1.000000 1.000000
1.000000 1.000000 1.000000
",
        )
        .expect("the fixture LUT is written");
        let asset = store
            .import_lut_asset(&source)
            .expect("the fixture LUT imports")
            .into_lut_asset(LutAssetId(1));
        document.lut_assets.push(asset.clone());

        let error = renderer
            .render(
                &document,
                TimeCode::ZERO,
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect_err("an unpublished library blocks an active LUT node");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("missing_lut_asset:"),
            "unexpected message: {message}"
        );

        let (library, _) = LutLibrary::build(&[asset], Some(&store));
        assert_eq!(library.len(), 1);
        renderer.set_lut_library(Arc::new(library));

        let frame = renderer
            .render(
                &document,
                TimeCode::ZERO,
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect("a published library resolves the node");
        assert_eq!((frame.width, frame.height), document.resolution);

        renderer
            .render_delivery(
                &document,
                TimeCode::ZERO,
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect("the delivery path resolves the node too");
    }

    /// A hand-written `S = 2` `.cube` whose corner samples are chosen by
    /// `map`, so two fixtures are different *looks* rather than two spellings
    /// of one lattice.
    fn corner_cube(map: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        use std::fmt::Write as _;
        let mut text = String::from("LUT_3D_SIZE 2\n");
        for blue in [0.0_f32, 1.0] {
            for green in [0.0_f32, 1.0] {
                for red in [0.0_f32, 1.0] {
                    let [r, g, b] = map([red, green, blue]);
                    let _ = writeln!(text, "{r:.6} {g:.6} {b:.6}");
                }
            }
        }
        text
    }

    #[test]
    fn two_documents_sharing_one_asset_id_render_to_their_own_lattices() {
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        let directory = TempDirectory::new("cc4-per-document-library");
        let store = LutStore::for_project(&directory.path("project.kinewright"))
            .expect("a temporary project derives a store root");

        let import = |name: &str, text: &str| {
            let source = directory.path(name);
            std::fs::write(&source, text).expect("the fixture LUT is written");
            store
                .import_lut_asset(&source)
                .expect("the fixture LUT imports")
                .into_lut_asset(LutAssetId(1))
        };
        let identity = import("identity.cube", &corner_cube(|rgb| rgb));
        let inverted = import(
            "inverted.cube",
            &corner_cube(|[r, g, b]| [1.0 - r, 1.0 - g, 1.0 - b]),
        );
        assert_eq!(identity.id, inverted.id, "the ids collide on purpose");
        assert_ne!(identity.sha256, inverted.sha256, "the looks differ");

        let mut published = std::collections::HashMap::new();
        for asset in [&identity, &inverted] {
            let (library, _) = LutLibrary::build(std::slice::from_ref(asset), Some(&store));
            for (_, sha256, lut) in library.entries() {
                published.insert(sha256.to_owned(), Arc::clone(lut));
            }
        }
        assert_eq!(published.len(), 2);

        let mut look = Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        };
        look.parameters
            .insert("lut_asset_id".to_owned(), ParamValue::Integer(1));

        let mut render_with = |asset: &kinewright_core::LutAsset| {
            let mut document = title_document(vec![look.clone()]);
            document.lut_assets = vec![asset.clone()];
            let (library, unbound) =
                LutLibrary::from_document_assets(&document.lut_assets, &published);
            assert!(unbound.is_empty(), "both looks were published");
            renderer.set_lut_library(Arc::new(library));
            renderer
                .render(
                    &document,
                    TimeCode::ZERO,
                    document.resolution,
                    RenderScale::FullResolution,
                    DecodeStrategy::Seek,
                )
                .expect("the document-local library resolves its own node")
        };

        let first = render_with(&identity);
        let second = render_with(&inverted);
        assert_eq!((first.width, first.height), (second.width, second.height));
        assert_ne!(
            *first.rgba, *second.rgba,
            "two documents that both call their look asset 1 must not render the same frame"
        );

        let again = render_with(&identity);
        assert_eq!(*again.rgba, *first.rgba);
    }

    #[test]
    fn title_bytes_are_reserved_before_video_cache_growth() {
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        renderer.cache_budget = 100;
        assert!(renderer.cache_title_frame(title_key(1), working_frame_with_bytes(80)));
        assert_eq!(renderer.cache_stats().bytes, 80);

        renderer.reserve_cache_bytes(30);
        assert!(renderer.title_cache.is_empty());
        assert!(renderer.total_cache_bytes().saturating_add(30) <= renderer.cache_budget);
    }

    #[test]
    fn oversized_title_is_rendered_without_being_cached() {
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        renderer.cache_budget = 100;
        assert!(!renderer.cache_title_frame(title_key(1), working_frame_with_bytes(120)));
        assert!(renderer.title_cache.is_empty());
        assert_eq!(renderer.cache_stats().bytes, 0);
    }

    #[test]
    fn managed_renderer_rejects_legacy_and_future_contexts() {
        let mut legacy = Document::default();
        legacy.color_context.pipeline_state = ColorPipelineState::Legacy;
        let error = validate_managed_context(&legacy).expect_err("legacy must be rejected");
        assert!(error.to_string().contains("pipeline_state=Legacy"));

        let mut future = Document::default();
        future.color_context.pipeline_state = ColorPipelineState::Other("managed_sdr_v2".into());
        let error = validate_managed_context(&future).expect_err("future state must be rejected");
        assert!(error.to_string().contains("managed_sdr_v2"));
    }

    #[test]
    fn managed_renderer_rejects_incompatible_working_or_monitoring_targets() {
        let mut working = Document::default();
        working.color_context.working.transfer = ColorTransfer::Bt709;
        let error = validate_managed_context(&working).expect_err("working target must match");
        assert!(error.to_string().contains("working="));

        let mut monitoring = Document::default();
        monitoring.color_context.monitoring.transfer = ColorTransfer::Srgb;
        let error =
            validate_managed_context(&monitoring).expect_err("monitoring target must match");
        assert!(error.to_string().contains("monitoring="));
    }

    #[test]
    fn managed_renderer_accepts_exact_user_override_targets() {
        let mut document = Document::default();
        document.color_context.working.provenance = kinewright_core::ColorProvenance::UserOverride;
        document.color_context.monitoring.provenance =
            kinewright_core::ColorProvenance::UserOverride;
        document.color_context.delivery.provenance = kinewright_core::ColorProvenance::UserOverride;
        assert!(document.color_context.is_managed_sdr_compatible());
        validate_managed_context(&document).expect("exact user overrides remain executable");
    }

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
    fn proxy_resolution_is_capped_at_the_memory_proxy_width() {
        let proxy = RenderScale::Proxy { max_width: 8_192 };
        assert_eq!(proxy.output_resolution((3_840, 2_160)), (1_280, 720));
    }

    #[test]
    fn proxy_width_is_part_of_decoder_and_cache_identity() {
        let asset = AssetId(7);
        let path = Path::new("fixture.mkv");
        let fingerprint = MediaSourceFingerprint::unknown();
        let description = ColorDescription::unknown();
        assert_ne!(
            VideoSourceKey::new(
                asset,
                path,
                &fingerprint,
                Rational::new(30, 1).unwrap(),
                &description,
                None,
                Some(1280),
            ),
            VideoSourceKey::new(
                asset,
                path,
                &fingerprint,
                Rational::new(30, 1).unwrap(),
                &description,
                None,
                Some(640),
            )
        );
    }

    #[test]
    fn source_identity_changes_for_same_asset_id_when_runtime_inputs_change() {
        let asset = AssetId(7);
        let path = Path::new("fixture.mkv");
        let fingerprint = MediaSourceFingerprint {
            content_sha256: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            ),
            byte_len: Some(123),
        };
        let description = ColorDescription::unknown();
        let baseline = VideoSourceKey::new(
            asset,
            path,
            &fingerprint,
            Rational::new(30, 1).unwrap(),
            &description,
            None,
            Some(640),
        );

        let changed_path = VideoSourceKey::new(
            asset,
            Path::new("relinked.mkv"),
            &fingerprint,
            Rational::new(30, 1).unwrap(),
            &description,
            None,
            Some(640),
        );
        assert_ne!(baseline, changed_path, "relinks need a fresh decoder");

        let changed_fingerprint = VideoSourceKey::new(
            asset,
            path,
            &MediaSourceFingerprint {
                content_sha256: Some(
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                ),
                byte_len: Some(456),
            },
            Rational::new(30, 1).unwrap(),
            &description,
            None,
            Some(640),
        );
        assert_ne!(
            baseline, changed_fingerprint,
            "a verified content change needs a fresh decoder"
        );

        let changed_description = ColorDescription {
            transfer: kinewright_core::ColorTransfer::Srgb,
            ..description.clone()
        };
        let changed_color = VideoSourceKey::new(
            asset,
            path,
            &fingerprint,
            Rational::new(30, 1).unwrap(),
            &changed_description,
            None,
            Some(640),
        );
        assert_ne!(
            baseline, changed_color,
            "a same-id colour override needs a fresh managed decoder"
        );

        let mut decoder_cache = HashMap::new();
        decoder_cache.insert(baseline, "old");
        decoder_cache.insert(changed_path, "relinked");
        decoder_cache.insert(changed_fingerprint, "changed content");
        decoder_cache.insert(changed_color, "changed colour");
        assert_eq!(
            decoder_cache.len(),
            4,
            "each candidate document must resolve a distinct source cache entry"
        );
    }

    fn generated_solid_source(label: &str, color: &str) -> GeneratedMedia {
        let filter = format!("color=c={color}:size=16x16:rate=1:duration=1");
        GeneratedMedia::ffmpeg(
            label,
            &[
                "-f",
                "lavfi",
                "-i",
                &filter,
                "-frames:v",
                "1",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "yuv444p",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ],
            "mkv",
        )
    }

    fn mean_channel(pixels: &[u8], channel: usize) -> f32 {
        let count = u16::try_from(pixels.len() / 4).expect("test image fits in u16");
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| f32::from(pixel[channel]))
            .sum::<f32>()
            / f32::from(count)
    }

    #[test]
    fn thumbnail_for_document_reopens_same_id_after_relink() {
        initialize_ffmpeg().expect("FFmpeg should initialize for relink identity fixture");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let red = generated_solid_source("source-identity-red", "red");
        let blue = generated_solid_source("source-identity-blue", "blue");
        let mut red_asset = probe_path(red.path(), AssetId(11)).expect("red source should probe");
        let mut blue_asset =
            probe_path(blue.path(), AssetId(11)).expect("blue source should probe");
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: kinewright_core::ColorMatrix::Bt709,
            range: kinewright_core::ColorRange::Limited,
            white_point: kinewright_core::ColorWhitePoint::D65,
            bit_depth: kinewright_core::ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::UserOverride,
        };
        red_asset.color_description = description.clone();
        blue_asset.color_description = description;
        let red_document = Arc::new(single_clip_document(red_asset));
        let blue_document = Arc::new(single_clip_document(blue_asset));
        let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for relink identity fixture");

        let red_thumbnail = engine
            .thumbnail_for_document(Arc::clone(&red_document), TimeCode::ZERO, 16)
            .expect("red thumbnail should render");
        let blue_thumbnail = engine
            .thumbnail_for_document(Arc::clone(&blue_document), TimeCode::ZERO, 16)
            .expect("blue thumbnail should render");
        assert_eq!(
            (red_thumbnail.width, red_thumbnail.height),
            (blue_thumbnail.width, blue_thumbnail.height)
        );
        assert_ne!(
            red_thumbnail.pixels, blue_thumbnail.pixels,
            "thumbnail_for_document must not reuse a same-id decoder after relink"
        );
        assert!(
            mean_channel(&red_thumbnail.pixels, 0) > mean_channel(&red_thumbnail.pixels, 2),
            "red relink candidate should render red content"
        );
        assert!(
            mean_channel(&blue_thumbnail.pixels, 2) > mean_channel(&blue_thumbnail.pixels, 0),
            "blue relink candidate should render blue content"
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

    fn unsupported_source() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer: ColorTransfer::Smpte2084,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    #[test]
    fn managed_decode_error_names_asset_field_observed_allowed_and_recovery() {
        // IN1 §4.2 rule 10, nit 5: the input is the typed refusal the decoder
        // now raises. The old `MediaError::Backend("managed source profile
        // rejected")` still lands in the `error =>` catch-all after rule 9 and
        // still produces a `Backend`, so the two new assertions below could
        // not hold on it.
        let error = contextual_managed_decode_error(
            AssetId(7),
            Path::new("/media/hdr-master.mov"),
            &unsupported_source(),
            None,
            MediaError::SourceColor(kinewright_core::ColorSourceError::UnsupportedPrimaries(
                ColorPrimaries::Bt2020,
            )),
        );
        assert_eq!(error.recovery_code(), Some("unsupported_source_primaries"));
        assert!(
            matches!(error, MediaError::SourceColorForAsset(_)),
            "the typed refusal must keep its type through the contextual wrap"
        );
        let message = error.to_string();
        assert!(message.contains("asset 7"), "{message}");
        assert!(message.contains("/media/hdr-master.mov"), "{message}");
        assert!(
            message.contains("source_color=unsupported_source_"),
            "{message}"
        );
        assert!(message.contains("field="), "{message}");
        assert!(message.contains("observed="), "{message}");
        assert!(message.contains("allowed="), "{message}");
        assert!(message.contains("recovery="), "{message}");
        assert!(
            message.contains("Apply an explicit supported source-colour override"),
            "{message}"
        );
    }

    #[test]
    fn managed_decode_error_keeps_the_typed_decoder_format_variant_and_its_fields() {
        let error = contextual_managed_decode_error(
            AssetId(3),
            Path::new("/media/ten-bit.mov"),
            &unsupported_source(),
            None,
            MediaError::UnsupportedDecoderFormat {
                path: "/media/ten-bit.mov".into(),
                format: "yuv420p10le".to_owned(),
                declared_bit_depth: Some(10),
                decoder_bit_depth: Some(8),
                reason: "swscale would discard the declared depth".to_owned(),
            },
        );
        assert_eq!(error.recovery_code(), Some("unsupported_decoder_format"));
        let message = error.to_string();
        assert!(message.contains("unsupported_decoder_format"), "{message}");
        assert!(message.contains("asset 3"), "{message}");
        assert!(message.contains("field=primaries"), "{message}");
        assert!(message.contains("observed=Bt2020"), "{message}");
        assert!(message.contains("allowed=bt709"), "{message}");
        assert!(message.contains("Recovery:"), "{message}");
    }

    #[test]
    fn managed_decode_error_reports_a_supported_source_profile_when_metadata_is_fine() {
        let mut description = unsupported_source();
        description.primaries = ColorPrimaries::Bt709;
        description.transfer = ColorTransfer::Bt709;
        description.matrix = ColorMatrix::Bt709;
        let error = contextual_managed_decode_error(
            AssetId(1),
            Path::new("/media/ok.mov"),
            &description,
            None,
            MediaError::Backend("decoder could not open the stream".to_owned()),
        );
        let message = error.to_string();
        assert!(message.contains("source_color=supported"), "{message}");
        assert!(message.contains("source_profile="), "{message}");
    }

    /// Review-1 B1 (MO2 R8), ported from the reviewer's failing probe: the
    /// render entry refuses an invalid document even when the offending clip
    /// is disabled, and still omits a valid disabled clip.
    #[test]
    fn reviewer1_render_must_defensively_reject_disabled_invalid_adjustment() {
        use kinewright_core::{OpError, Operation, Transition};
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        let mut document = Document {
            resolution: (32, 18),
            ..Document::default()
        };
        Operation::AddTrack {
            track: Track {
                id: TrackId(901),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }
        .apply(&mut document)
        .unwrap();
        Operation::AddAdjustmentClip {
            track: TrackId(901),
            timeline_start: TimeCode(0),
            duration: TimeCode(30),
            effects: vec![],
        }
        .apply(&mut document)
        .unwrap();
        let clip = &mut document.tracks[0].clips[0];
        clip.enabled = false;
        let valid = document.clone();
        let clip = &mut document.tracks[0].clips[0];
        clip.transition_in = Some(Transition {
            name: "fade_from_black".into(),
            duration: TimeCode(5),
        });
        let expected = OpError::TransitionUnsupportedOnAdjustment {
            clip: clip.id,
            transition: "fade_from_black".into(),
        };
        assert_eq!(document.validate(), Err(expected.clone()));
        let (at, full) = (TimeCode(0), RenderScale::FullResolution);
        let refused = renderer.render(&document, at, (32, 18), full, DecodeStrategy::Seek);
        assert_eq!(
            refused.err(),
            Some(MediaError::InvalidDocument(Box::new(expected))),
            "invalid disabled adjustment rendered a frame successfully"
        );
        let working = renderer.render_working(&document, at, (32, 18), full, DecodeStrategy::Seek);
        assert!(matches!(working, Err(MediaError::InvalidDocument(_))));
        let omitted = renderer.render_working(&valid, at, (32, 18), full, DecodeStrategy::Seek);
        assert!(
            omitted
                .unwrap()
                .pixels
                .chunks(4)
                .all(|p| p == [0.0, 0.0, 0.0, 1.0])
        );
    }

    /// Three video tracks showing one source at once, from source frames 0,
    /// 100 and 200: one source cache, three demand points.
    fn three_demands_on_one_source(asset: MediaAsset) -> Document {
        let mut document = single_clip_document(asset);
        let track = document.tracks[0].clone();
        document.tracks.clear();
        for (index, start) in [0_i64, 100, 200].into_iter().enumerate() {
            let mut track = track.clone();
            let id = u64::try_from(index).expect("small") + 1;
            (track.id, track.clips[0].id) = (TrackId(id), ClipId(id));
            track.clips[0].source_range = TimeCode(start)..TimeCode(start + 40);
            document.tracks.push(track);
        }
        document.duration = TimeCode(40);
        document
    }

    /// The frames of the one source cache, and its charged bytes.
    fn resident(renderer: &FrameRenderer, frames: &[i64]) -> (Vec<i64>, usize) {
        let [source] = renderer.video_sources.values().collect::<Vec<_>>()[..] else {
            panic!("one source cache");
        };
        let held = frames.iter().copied();
        let held = held
            .filter(|at| source.cache.contains(TimeCode(*at)))
            .collect();
        (held, source.cache.byte_len())
    }

    /// PF1 K-5 (review B F3): three demand windows on one source overflow
    /// its 32-entry ring; capacity eviction must keep every frame the render
    /// shows resident and charged, the first window's frame 0 included.
    #[test]
    fn capacity_eviction_keeps_every_shown_frame_of_one_source() {
        initialize_ffmpeg().expect("FFmpeg should initialize");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let (_media, asset) = small_source("pf1-pins", "testsrc2", 1, 8);
        let document = three_demands_on_one_source(asset);
        let frame = working_bytes((64, 36));
        let mut renderer = FrameRenderer::new_preview(gpu);
        renderer.set_cache_budget(64 * frame);
        let scale = RenderScale::Proxy { max_width: 1280 };
        let strategy = DecodeStrategy::Sequential;
        let shown = renderer.render_live(&document, TimeCode(0), (64, 36), scale, strategy);
        shown.expect("a frame renders");
        let (held, bytes) = resident(&renderer, &[0, 100, 200]);
        assert_eq!(held, [0, 100, 200], "every shown frame is resident");
        let source = renderer.video_sources.values().next().expect("one source");
        assert!(
            source.cache.eviction_count() > 0,
            "the windows overflowed the ring"
        );
        assert_eq!(source.cache.len(), FRAME_CACHE_CAPACITY);
        // Charged once per distinct allocation (grid frames share one), the
        // three shown frames among them, within the budget.
        let charged = 3 * frame..=64 * frame;
        assert!(charged.contains(&bytes), "the shown frames are charged");
    }

    /// PF1 K-6 (review B S2): the preview's overshoot is bounded by demand
    /// points, not sources: with a one-frame budget and three points on one
    /// source, the three shown frames stay, live = max(C, P·f) = 3f.
    #[test]
    fn overshoot_is_bounded_by_the_demand_points() {
        initialize_ffmpeg().expect("FFmpeg should initialize");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let (_media, asset) = small_source("pf1-overshoot", "testsrc2", 1, 8);
        let document = three_demands_on_one_source(asset);
        let frame = working_bytes((64, 36));
        let mut renderer = FrameRenderer::new_preview(gpu);
        renderer.set_cache_budget(frame);
        let scale = RenderScale::Proxy { max_width: 1280 };
        for at in [0, 1, 2] {
            let strategy = DecodeStrategy::Sequential;
            let composed = renderer.render_live(&document, TimeCode(at), (64, 36), scale, strategy);
            composed.expect("a frame renders");
            let shown = [at, 100 + at, 200 + at];
            let (held, bytes) = resident(&renderer, &shown);
            assert_eq!(held, shown, "the shown frames stay");
            assert_eq!(renderer.total_cache_bytes(), bytes);
            assert_eq!(bytes, 3 * frame, "live = max(C, P·f) with P = 3");
        }
    }

    /// PF1 K-6 (re-review D4): documents S1's bound across sources, not a
    /// fix. C = 4f; source A demands 0/100/200 (three pins), B demands 0.
    /// Each share is 2f: A cannot evict a pin and B, at exactly its share,
    /// is not over it, so 5f stay resident: over C and over max(C, P·f) =
    /// 4f, and equal to the sum over sources of max(share, pinned) =
    /// 3f + 2f. S2b-3's I12 must close this.
    #[test]
    fn mixed_sources_hold_their_shares_and_pins_over_the_budget() {
        initialize_ffmpeg().expect("FFmpeg should initialize");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let (_a, a) = small_source("pf1-mixed-a", "testsrc2", 1, 8);
        let (_b, b) = small_source("pf1-mixed-b", "smptebars", 2, 8);
        let mut document = three_demands_on_one_source(a);
        let mut track = document.tracks[0].clone();
        (track.id, track.clips[0].id, track.clips[0].asset) = (TrackId(4), ClipId(4), b.id);
        track.clips[0].source_range = TimeCode(0)..TimeCode(40);
        document.tracks.push(track);
        document.media_pool.push(b);
        let frame = working_bytes((64, 36));
        let mut renderer = FrameRenderer::new_preview(gpu);
        renderer.set_cache_budget(4 * frame);
        let scale = RenderScale::Proxy { max_width: 1280 };
        let strategy = DecodeStrategy::Sequential;
        let shown = renderer.render_live(&document, TimeCode(0), (64, 36), scale, strategy);
        shown.expect("a frame renders");
        let mut held = renderer
            .video_sources
            .values()
            .map(|source| source.cache.byte_len() / frame)
            .collect::<Vec<_>>();
        held.sort_unstable();
        assert_eq!(held, [2, 3], "B its 2f share, A its three pins");
        assert_eq!(renderer.total_cache_bytes(), 5 * frame);
    }

    /// A 64×36 ffv1 BT.709 source of `seconds` at 30 fps.
    fn small_source(
        label: &str,
        filter: &str,
        id: u64,
        seconds: u32,
    ) -> (GeneratedMedia, MediaAsset) {
        let filter = format!("{filter}=size=64x36:rate=30:duration={seconds}");
        let args = [
            "-f", "lavfi", "-i", &filter, "-c:v", "ffv1", "-pix_fmt", "yuv420p",
        ];
        let media = GeneratedMedia::ffmpeg(label, &args, "mkv");
        let mut asset = probe_path(media.path(), AssetId(id)).expect("source probes");
        asset.color_description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: kinewright_core::ColorMatrix::Bt709,
            range: kinewright_core::ColorRange::Limited,
            white_point: kinewright_core::ColorWhitePoint::D65,
            bit_depth: kinewright_core::ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::UserOverride,
        };
        (media, asset)
    }

    /// PF1 K-6 (review B F5): a thumbnail on the preview renderer keeps
    /// today's policy: the preview's demand is not rebuilt by it.
    #[test]
    fn a_thumbnail_leaves_the_preview_demand_alone() {
        initialize_ffmpeg().expect("FFmpeg should initialize");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let (_media, asset) = small_source("pf1-thumb", "testsrc2", 1, 2);
        let document = single_clip_document(asset);
        let scale = RenderScale::Proxy { max_width: 1280 };
        let mut renderer = FrameRenderer::new_preview(gpu);
        let strategy = DecodeStrategy::Sequential;
        let live = renderer.render_live(&document, TimeCode(5), (64, 36), scale, strategy);
        live.expect("a preview frame");
        let demand = |renderer: &FrameRenderer| {
            let demand = renderer.preview.as_ref().expect("the preview renderer");
            (demand.sources.clone(), demand.generated)
        };
        let before = demand(&renderer);
        assert_eq!(before.0.values().collect::<Vec<_>>(), [&[TimeCode(5)]]);
        let thumbnail = renderer.render_thumbnail(&document, TimeCode(40), (64, 36), scale);
        thumbnail.expect("a thumbnail");
        assert_eq!(demand(&renderer), before);

        // An error return restores the demand too (re-review nit).
        let mut missing = document.clone();
        missing.media_pool[0].path = "/nonexistent/pf1-thumbnail.mkv".into();
        let failed = renderer.render_thumbnail(&missing, TimeCode(40), (64, 36), scale);
        assert!(failed.is_err(), "the source is missing");
        assert_eq!(demand(&renderer), before, "restored after an error");

        // And so does unwinding: the restoration is scoped.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let aside = PreviewAside::new(&mut renderer);
            assert!(aside.renderer.preview.is_none(), "set aside");
            panic!("a thumbnail render unwinds");
        }));
        assert!(unwound.is_err());
        assert_eq!(demand(&renderer), before, "restored while unwinding");
    }

    /// PF1 G5 (K-6): two continuous sources under a binding budget decode
    /// with 0 seeks after warm-up on the preview renderer; today's policy
    /// thrashes on the same budget (the control).
    #[test]
    fn preview_window_plays_two_continuous_sources_without_seeking() {
        initialize_ffmpeg().expect("FFmpeg should initialize");
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let (_a, lower) = small_source("pf1-g5-a", "testsrc2", 1, 2);
        let (_b, upper) = small_source("pf1-g5-b", "smptebars", 2, 2);
        let mut document = single_clip_document(lower);
        let mut track = document.tracks[0].clone();
        (track.id, track.clips[0].id, track.clips[0].asset) = (TrackId(2), ClipId(2), upper.id);
        document.tracks.push(track);
        document.media_pool.push(upper);
        let scale = RenderScale::Proxy { max_width: 1280 };
        let seeks_after_warm_up = |mut renderer: FrameRenderer| {
            renderer.set_cache_budget(12 * working_bytes((64, 36)));
            let mut warm = 0;
            for at in 0..60 {
                let strategy = DecodeStrategy::Sequential;
                let frame = renderer.render(&document, TimeCode(at), (64, 36), scale, strategy);
                frame.expect("frame renders");
                warm = if at == 0 {
                    renderer.video_seek_count()
                } else {
                    warm
                };
            }
            renderer.video_seek_count() - warm
        };
        assert_eq!(
            seeks_after_warm_up(FrameRenderer::new_preview(gpu.clone())),
            0
        );
        assert!(
            seeks_after_warm_up(FrameRenderer::new(gpu)) > 0,
            "the control"
        );
    }
}

/// MO2 R28 (ME14): a resident compositor frame's phases.
#[cfg(test)]
pub(crate) mod phases {
    use std::time::Duration;

    use super::*;

    /// [`FrameRenderer::render_timed`]'s compositor frame, split by
    /// [`crate::compositor::phases::monitor`].
    pub(crate) fn render(
        renderer: &mut FrameRenderer,
        document: &Document,
        at: TimeCode,
        resolution: (u32, u32),
        scale: RenderScale,
    ) -> Result<([Duration; 3], Duration), MediaError> {
        let strategy = DecodeStrategy::Sequential;
        let decoded = renderer.decoded_layers(
            document,
            at,
            resolution,
            scale,
            super::Video::Decode(strategy),
        )?;
        crate::compositor::phases::monitor(
            &renderer.compositor,
            resolution,
            &compositor_layers(&decoded),
            &document.color_context.monitoring,
            Some(&renderer.lut_library),
        )
    }
}

/// K-1 (review B F2): what a render allocates against what admission
/// reserved for it.
#[cfg(test)]
mod k1_allocation {
    use kinewright_core::{BlendMode, ClipContent, SolidColor, Track, TrackId, TrackKind};

    use super::*;
    use crate::{cc1_fixtures::fallback_gpu, engine::monitor_max_width, frame::CachedFrame};

    /// A solid, a title and two adjustments allocate exactly G, the
    /// generated bytes `reader_demand` reserves; the adjustments share one
    /// process-wide frame.
    #[test]
    fn a_render_allocates_exactly_its_generated_reservation() {
        let clip = |id: u64, content| {
            crate::mo2_fixtures::clip(id, content, BlendMode::Normal, Vec::new())
        };
        let solid = ClipContent::Solid(SolidColor { r: 1, g: 2, b: 3 });
        let title = ClipContent::Title(Title {
            text: "K-1".to_owned(),
            ..Title::default()
        });
        let clips = [
            clip(1, solid),
            clip(2, ClipContent::Adjustment),
            clip(3, title),
            clip(4, ClipContent::Adjustment),
        ];
        let document = Document {
            resolution: (160, 88),
            duration: TimeCode(9),
            tracks: (1..)
                .zip(clips)
                .map(|(id, clip)| Track {
                    id: TrackId(id),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip],
                })
                .collect(),
            ..Document::default()
        };
        document.validate().expect("valid");
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let resolution = scale.output_resolution(document.resolution);
        let demand = reader_demand(&document, TimeCode(0), resolution, scale, 0);
        let mut renderer = FrameRenderer::new_preview(fallback_gpu().context());
        let supplied = SuppliedFrames::new();
        let video = Video::Supplied(&supplied);
        let layers = (renderer.decoded_layers(&document, TimeCode(0), resolution, scale, video))
            .expect("the layers");
        assert_eq!(layers.len(), 4);
        let shared = ADJUSTMENT_FRAME.shared_buffer_id();
        let mut seen = HashSet::new();
        let allocated: usize = (layers.iter().map(|layer| &layer.frame))
            .filter(|frame| frame.shared_buffer_id() != shared)
            .filter(|frame| seen.insert(frame.shared_buffer_id()))
            .map(CachedFrame::byte_len)
            .sum();
        assert_eq!(allocated, demand.generated, "allocated = reserved G");
        assert_eq!(demand.generated, 2 * working_bytes(resolution));
    }
}
