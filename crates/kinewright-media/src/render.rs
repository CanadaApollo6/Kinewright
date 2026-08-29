use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
};

use kinewright_core::{
    AssetId, ClipId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorRange, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, Document, Effect,
    EffectId, FrameTexture, LinearRgbaImage, MatteProofError, MediaError, MediaSourceFingerprint,
    Rational, TimeCode, Title, classify_source_with_assumption, document_monitor_preview,
};

use crate::{
    TimelineVisualLayer,
    cache::FrameCache,
    compositor::{Compositor, CompositorLayer, DeliveryFrame, GpuContext, MatteRenderTarget},
    decode::VideoDecoder,
    derived_cache::CacheStats,
    frame::WorkingFrame,
    lut_store::LutLibrary,
    timeline::TransitionRenderParams,
    visual_layers_at,
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
struct VideoSourceKey {
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

type TitleCacheKey = (ClipId, (u32, u32), Title);

struct VideoSource {
    decoder: VideoDecoder,
    cache: FrameCache<WorkingFrame>,
}

/// The single frame-rendering path used by both playback preview and export.
pub(crate) struct FrameRenderer {
    // Scale is part of the key so changing preview size can never reuse frames
    // decoded for a different proxy width.
    video_sources: HashMap<VideoSourceKey, VideoSource>,
    source_order: VecDeque<VideoSourceKey>,
    compositor: Compositor,
    title_rasterizer: crate::title::TitleRasterizer,
    title_cache: HashMap<TitleCacheKey, WorkingFrame>,
    title_order: VecDeque<TitleCacheKey>,
    cache_budget: usize,
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
}

impl FrameRenderer {
    pub(crate) fn new(gpu: GpuContext) -> Self {
        Self {
            video_sources: HashMap::new(),
            source_order: VecDeque::new(),
            compositor: Compositor::new(gpu),
            title_rasterizer: crate::title::TitleRasterizer::new(),
            title_cache: HashMap::new(),
            title_order: VecDeque::new(),
            cache_budget: FRAME_CACHE_BYTE_BUDGET,
            lut_library: Arc::new(LutLibrary::default()),
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

    /// Composite one project frame for the document's monitoring target.
    ///
    /// CC1 2.2.6 requires the monitor transform to be selected from the
    /// monitoring `ColorDescription`, so the document's own description is
    /// handed to the compositor rather than a compositor default.
    ///
    /// CC8 §4 adds the second half of that selection: the
    /// [`MonitorPreview`](kinewright_core::MonitorPreview) arm, taken from the
    /// document's *source* profiles by `document_monitor_preview` — core's one
    /// classifier, shared with `get_color_qc` and the Colour QC window — so an
    /// HDR-profile project previews through §4's labelled tone map and an SDR
    /// project takes `Direct`, which is the transform it always had.
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
        let decoded_layers =
            self.decoded_layers(document, project_at, resolution, scale, strategy)?;
        let layers = compositor_layers(&decoded_layers);
        self.compositor.render_monitor_preview_with_luts(
            resolution,
            &layers,
            &document.color_context.monitoring,
            document_monitor_preview(document),
            Some(&self.lut_library),
        )
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
        let decoded_layers =
            self.decoded_layers(document, project_at, resolution, scale, strategy)?;
        let layers = compositor_layers(&decoded_layers);
        self.compositor.render_delivery_with_luts(
            resolution,
            &layers,
            &document.color_context.delivery,
            Some(&self.lut_library),
        )
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
        let decoded_layers =
            self.decoded_layers(document, project_at, resolution, scale, strategy)?;
        let layers = compositor_layers(&decoded_layers);
        self.compositor
            .render_working_with_luts(resolution, &layers, Some(&self.lut_library))
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
        let decoded_layers =
            self.decoded_layers(document, project_at, resolution, scale, strategy)?;
        // Not `EffectNotFound`: the effect id was never inspected here, and a
        // clip that is simply off screen at this frame is a different failure
        // with a different recovery than a node id that does not exist. CC5
        // §4.1 requires a typed refusal rather than a blank frame; it does not
        // require the refusal to misdescribe itself.
        let layer_index = decoded_layers
            .iter()
            .position(|layer| layer.clip == clip)
            .ok_or(MatteProofError::ClipNotVisible {
                clip,
                at: project_at,
            })?;
        let layers = compositor_layers(&decoded_layers);
        let coverage = self.compositor.render_matte(
            resolution,
            &layers,
            Some(&self.lut_library),
            MatteRenderTarget {
                layer_index,
                clip,
                effect,
            },
        )?;
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
        strategy: DecodeStrategy,
    ) -> Result<Vec<DecodedLayer>, MediaError> {
        validate_managed_context(document)?;
        let layer_specs = visual_layers_at(document, project_at)?;
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
                    let frame = self.decode_video_frame(
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
                    )?;
                    decoded_layers.push(DecodedLayer {
                        clip: layer.source.clip,
                        frame,
                        effects: layer.effects,
                        transition: layer.transition,
                    });
                }
                TimelineVisualLayer::Title(layer) => {
                    let key = (layer.clip, resolution, layer.title.clone());
                    let frame = if let Some(frame) = self.title_cache.get(&key).cloned() {
                        self.touch_title(key);
                        frame
                    } else {
                        let display_frame =
                            self.title_rasterizer.rasterize(&layer.title, resolution)?;
                        let frame = WorkingFrame::from_display_frame(&display_frame)?;
                        self.cache_title_frame(key, frame.clone());
                        frame
                    };
                    decoded_layers.push(DecodedLayer {
                        clip: layer.clip,
                        frame,
                        effects: layer.effects,
                        transition: layer.transition,
                    });
                }
            }
        }
        Ok(decoded_layers)
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
            let decoder = VideoDecoder::open_scaled_managed(
                path,
                fps,
                key.max_width,
                description,
                assumption,
            )
            .map_err(|error| {
                contextual_managed_decode_error(asset, path, description, assumption, error)
            })?;
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
                .map_or(0, working_bytes);
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
            .and_then(|source| {
                source
                    .cache
                    .frame_at_or_before_bounded(source_at, self.cache_budget)
            })
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
            // A single title larger than the aggregate budget is still
            // rendered for the current request, but is never retained.
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
        while self.total_cache_bytes().saturating_add(incoming) > self.cache_budget {
            if self.evict_oldest_video_frame() || self.evict_oldest_title_frame() {
                continue;
            }
            break;
        }
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

fn contextual_managed_decode_error(
    asset: AssetId,
    path: &Path,
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
    error: MediaError,
) -> MediaError {
    let status = managed_source_color_status(description, assumption);
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
                "managed decode for asset {asset} ({}) failed: {reason} [{status}, assumption={assumption:?}, description={description:?}]. Recovery: apply an explicit supported source-colour override, transcode to a supported integer format, or relink to compatible media.",
                path.display()
            ),
        },
        error => MediaError::Backend(format!(
            "managed decode for asset {asset} ({}) failed: {error} [{status}, assumption={assumption:?}, description={description:?}]. Recovery: apply an explicit supported source-colour override, transcode to a supported integer format, or relink to compatible media.",
            path.display()
        )),
    }
}

fn validate_managed_context(document: &Document) -> Result<(), MediaError> {
    // CC8 §5.1: the renderer executes CC1's managed context **or** the same
    // context with §5.1's HDR delivery lane. §3.1 keeps the working and
    // monitoring descriptions byte-identical to CC1's, so the widening is on
    // the delivery side only, and `is_managed_compatible` is the one function
    // that says so.
    if document.color_context.is_managed_compatible() {
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
        })
        .collect()
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

fn d65_assumption(description: &ColorDescription) -> Option<ColorSourceProfileAssumption> {
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
        LutAssetId, ParamValue, Track, TrackId, TrackKind,
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

    fn working_frame_with_bytes(bytes: usize) -> WorkingFrame {
        assert_eq!(bytes % std::mem::size_of::<f16>(), 0);
        WorkingFrame {
            width: 1,
            height: 1,
            pixels: Arc::new(vec![f16::from_f32(0.0); bytes / std::mem::size_of::<f16>()]),
        }
    }

    fn title_key(id: u64) -> TitleCacheKey {
        (ClipId(id), (1, 1), Title::default())
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
                }],
            }],
            ..Document::default()
        }
    }

    #[test]
    fn a_published_lut_library_reaches_the_compositor_through_the_frame_renderer() {
        // CC4 2.4: the renderer resolves LUT nodes against a library the
        // owning layer publishes; it never opens a LUT file for a managed
        // node.  Until one is published, an active LUT node BLOCKS the render
        // instead of quietly producing a look-free frame.
        let Some(mut renderer) = test_renderer() else {
            return;
        };
        let mut look = Effect {
            id: EffectId(1),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        };
        look.parameters
            .insert("lut_asset_id".to_owned(), ParamValue::Integer(1));
        let document = title_document(vec![look]);

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

        // The delivery path takes the same library, so export cannot render a
        // look the preview refused.
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
        // CC4 2.4.  `LutAssetId(1)` names a different look in every project,
        // so a shared render path must resolve looks by content hash and bind
        // the result per document.  Both looks are published into one table;
        // each document then renders through the library IT binds, and the two
        // frames must not agree.
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
                // Both projects allocated id 1 for their own look, which is
                // the collision the content hash has to survive.
                .into_lut_asset(LutAssetId(1))
        };
        let identity = import("identity.cube", &corner_cube(|rgb| rgb));
        let inverted = import(
            "inverted.cube",
            &corner_cube(|[r, g, b]| [1.0 - r, 1.0 - g, 1.0 - b]),
        );
        assert_eq!(identity.id, inverted.id, "the ids collide on purpose");
        assert_ne!(identity.sha256, inverted.sha256, "the looks differ");

        // Publication is what the engine's `set_lut_library` does: merge each
        // project's verified library into one content-addressed table.
        let mut published = std::collections::HashMap::new();
        for asset in [&identity, &inverted] {
            let (library, _) = LutLibrary::build(std::slice::from_ref(asset), Some(&store));
            for (_, sha256, lut) in library.entries() {
                published.insert(sha256.to_owned(), Arc::clone(lut));
            }
        }
        assert_eq!(published.len(), 2);

        let mut look = Effect {
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

        // Re-binding the first document after the second has rendered gives
        // the first frame back, so nothing about render order aliases either.
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

        // This is the reservation made before a video decode window.  The
        // title must be evicted rather than allowing aggregate residency to
        // exceed the bound reported by cache_stats().
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
            hdr_static_metadata: kinewright_core::HdrStaticMetadata::unknown(),
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

    /// A completely specified source description that matches no profile.
    ///
    /// It used to be `bt2020` / `smpte2084` / `bt2020_ncl` / `limited` / `d65`
    /// / 10-bit, standing in for "an HDR source, which is unsupported". CC8
    /// §2.1 makes exactly that tuple the `pq_rec2020` profile, so it no longer
    /// demonstrates the thing these two tests are about — the *shape* of a
    /// managed decode error. Pairing Rec.2020 primaries with a BT.709 transfer
    /// keeps the subject and keeps the reported field: §2.1 lists a mismatched
    /// primaries/transfer pair among the "explicit CC8 failures, not guesses",
    /// and because the pair is not one of §2.1's rows it is diagnosed by CC1's
    /// primaries rule, which is where `observed=Bt2020` / `allowed=bt709` below
    /// still comes from.
    fn unsupported_source() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
            hdr_static_metadata: kinewright_core::HdrStaticMetadata::unknown(),
        }
    }

    #[test]
    fn managed_decode_error_names_asset_field_observed_allowed_and_recovery() {
        // CC1 2.1: the error must name the asset, the unsupported field, the
        // observed value, and the allowed values, plus a recovery action.
        let error = contextual_managed_decode_error(
            AssetId(7),
            Path::new("/media/hdr-master.mov"),
            &unsupported_source(),
            None,
            MediaError::Backend("managed source profile rejected".to_owned()),
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
}
