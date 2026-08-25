use std::{
    collections::HashMap,
    fs,
    num::NonZeroU64,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use half::f16;
use kinewright_core::{
    COLOR_NODE_LIMIT_PER_LAYER, ClipId, ColorContext, ColorCurveChannel, ColorDescription,
    ColorNodeKind, ColorWheelChannel, ColorWheelsParams, CurvePoints, Effect, EffectId,
    EffectParameterDescriptor, EffectUniform, FrameTexture, LutNodeParams, MATTE_WINDOW_LIMIT,
    MatteParams, MatteProofError, MediaError, MonitorProofMetadata, MonitorProofRenderKind,
    ParamValue, ResolvedCurves, classify_color_node, color_node_inactive_reason, effect_descriptor,
    managed_color_node_count,
};

use crate::{
    color_pipeline::{
        PrimaryCorrection, encode_delivery_for_description, encode_monitor_rgba8_for_description,
    },
    frame::WorkingFrame,
    lut::{CubeLut, parse_cube_lut},
    lut_store::LutLibrary,
    render::RenderScale,
    timeline::TransitionRenderParams,
};

/// The compositor's WGSL source.
///
/// Named rather than inlined into `create_shader_module` so the ABI fixtures
/// can assert against the very text the pipeline compiles — CC4 4.2 requires a
/// shader branch for every `ColorNodeKind`, and a test that read a second copy
/// could not prove that.
const COMPOSITOR_SHADER_SOURCE: &str = include_str!("compositor.wgsl");

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UNIFORM_FLOATS: usize = 48;
const UNIFORM_SIZE: u64 = UNIFORM_FLOATS as u64 * 4;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;
/// `vec4<u32>` header of the CC3 grade buffer: active node count, curve
/// payload word offset, ABI version, reserved.
const GRADE_HEADER_BYTES: usize = 16;
/// Node record stride in words (CC3 3.2): `[kind, payload_word_offset,
/// bypass, reserved, v0 .. v11]`.
const GRADE_NODE_WORDS: usize = 16;
/// Node record stride in bytes.
const GRADE_NODE_BYTES: usize = GRADE_NODE_WORDS * 4;
/// Word offset of `v0` inside a node record.
const GRADE_NODE_VALUE_OFFSET: usize = 4;
/// `v0 .. v11`, the per-kind value block of a node record.
const GRADE_NODE_VALUE_WORDS: usize = 12;
/// One curve slot: `[count, x0, y0, m0, ... x15, y15, m15]`.
const GRADE_CURVE_SLOT_WORDS: usize = 49;
/// A curve node owns four slots, ordered red, green, blue, master.
const GRADE_CURVE_PAYLOAD_WORDS: usize = 4 * GRADE_CURVE_SLOT_WORDS;
/// The storage-buffer ABI version written into `header.z`.
///
/// CC4 4.2 took this `1 -> 2`: the buffer carries node kinds whose
/// interpretation depends on a companion texture binding (the LUT atlas), so a
/// consumer that understands only the CC3 kinds cannot safely read it.
///
/// CC5 3.1 takes it `2 -> 3`: `v11` stopped being a reserved zero and became
/// the matte payload word offset, so a CC4 consumer would read a matte-gated
/// node as an unmasked correction applied to the whole raster.
const GRADE_ABI_VERSION: u32 = 3;

/// CC5 3.1: the matte block is 64 words (256 bytes) in the payload region.
const MATTE_BLOCK_WORDS: usize = 64;
/// CC5 3.1: one window occupies twelve words of the matte block.
const MATTE_WINDOW_WORDS: usize = 12;
/// CC5 3.1: window `j` starts at `MATTE_WINDOW_BASE_WORD + 12 * j`.
const MATTE_WINDOW_BASE_WORD: usize = 16;
const _: () = assert!(
    MATTE_WINDOW_BASE_WORD + MATTE_WINDOW_LIMIT * MATTE_WINDOW_WORDS == MATTE_BLOCK_WORDS,
    "the CC5 3.1 matte block is exactly its header plus four window slots"
);
/// Word index of `v11` inside a node record: the matte payload word offset.
///
/// `0` means *no matte*, unambiguous because `words[0]` is always the first
/// record's `kind`.
const GRADE_NODE_MATTE_OFFSET_WORD: usize = GRADE_NODE_VALUE_OFFSET + 11;
/// The `grade709` range of one curve-coordinate basis point.
const CURVE_BASIS_POINT_SCALE: f32 = 10_000.0;

/// CC4 4.1: managed LUT atlas slots per layer, one per *active* LUT node.
///
/// Mirrors Core's `LUT_NODE_LIMIT_PER_LAYER`, which is what the edit path
/// enforces; this constant is the GPU-side reason that limit exists, and the
/// limit-contract test asserts the two agree.
pub const COMPOSITOR_LUT_SLOTS_PER_LAYER: usize = 4;

/// CC4 4.1: the legacy external `cube_lut` compatibility stage owns the last
/// atlas slot, so no new binding is introduced for it.
pub const COMPOSITOR_LEGACY_LUT_SLOT: usize = 4;

/// CC4 4.1: total slots in the single depth-packed 3D atlas at binding 3.
pub const COMPOSITOR_LUT_ATLAS_SLOTS: usize = COMPOSITOR_LUT_SLOTS_PER_LAYER + 1;

/// CC4 4.1: the 3D texture dimension the compositor negotiates.
///
/// The worst-case atlas is `COMPOSITOR_LUT_ATLAS_SLOTS * MAX_CUBE_SIZE = 325`
/// texels deep, which exceeds the 256 that both `downlevel_defaults` and
/// `downlevel_webgl2_defaults` advertise.  Production negotiates
/// `wgpu::Limits::default()` (2048), so raising the floor to 512 changes no
/// production adapter's behaviour and makes the downlevel requirement explicit
/// instead of latent.
pub const COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D: u32 = 512;

/// How many distinct atlases the compositor keeps hot.
///
/// One entry would thrash whenever two layers of the same frame carry
/// different looks, which is the ordinary multi-layer case; a handful covers a
/// realistic stack.
const LUT_ATLAS_CACHE_ENTRIES: usize = 8;

/// The GPU memory the atlas cache may hold in retained atlases.
///
/// The count bound alone says nothing about size: eight worst-case
/// `65 x 65 x 325` atlases is about 168 MiB held purely as a reuse
/// optimization, the same failure mode `TEXTURE_POOL_MAX_BYTES` exists for. A
/// dropped atlas is simply rebuilt on demand, and the entry the current frame
/// is using is kept alive by its `Arc` regardless.
///
/// A cached atlas also retains a strong `Arc` to each source lattice (see
/// [`CachedLutSlot`]), so this budget bounds the retained CPU samples too: the
/// parsed lattice of a slot is the same 16 bytes per texel as the texel it was
/// uploaded to. Those lattices are normally already held by the published
/// [`LutLibrary`], so the retention usually costs nothing beyond the `Arc`.
const LUT_ATLAS_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The compositor's managed colour-node ABI uses one read-only storage buffer
/// in the fragment stage.  Keep this requirement next to the bind-group
/// layout so native device setup cannot accidentally negotiate it away.
/// CC3 3.2 keeps this at `1` deliberately: a second fragment-stage storage
/// binding is not available on every supported downlevel backend.
pub const COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE: u32 = 1;

/// CC5 3.1's worst case is sixteen curve nodes that each carry a matte:
/// `16 + 16 * 64 + 16 * 4 * 49 * 4 + 16 * 64 * 4 = 17680` bytes, which no
/// longer fits the CC3 binding size.  The negotiated limit is the next power
/// of two, following CC3's stated convention.  The binding *count* stays `1`.
pub const COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE: u64 = 32_768;

/// The largest buffer [`grade_buffer_bytes_with_matte`] can produce, asserted
/// against the negotiated binding size by
/// `grade_buffer_worst_case_fits_the_binding_size`.
const GRADE_BUFFER_WORST_CASE_BYTES: usize = GRADE_HEADER_BYTES
    + COLOR_NODE_LIMIT_PER_LAYER * GRADE_NODE_BYTES
    + COLOR_NODE_LIMIT_PER_LAYER * GRADE_CURVE_PAYLOAD_WORDS * 4
    + COLOR_NODE_LIMIT_PER_LAYER * MATTE_BLOCK_WORDS * 4;

/// Add the minimum limits required by the production compositor to a device
/// request. This deliberately preserves stronger caller requirements while
/// making the shader ABI explicit for both headless and windowed devices.
#[must_use]
pub fn compositor_required_limits(mut limits: wgpu::Limits) -> wgpu::Limits {
    limits.max_storage_buffers_per_shader_stage = limits
        .max_storage_buffers_per_shader_stage
        .max(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE);
    limits.max_storage_buffer_binding_size = limits
        .max_storage_buffer_binding_size
        .max(COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE);
    // CC4 4.1: the depth-packed LUT atlas needs more 3D depth than the
    // downlevel profiles advertise.
    limits.max_texture_dimension_3d = limits
        .max_texture_dimension_3d
        .max(COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D);
    limits
}

#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    provenance: GpuProvenance,
}

#[derive(Clone)]
struct GpuProvenance {
    backend: String,
    adapter: String,
    software_fallback: bool,
    gpu_claim: bool,
}

impl GpuContext {
    #[must_use]
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            provenance: GpuProvenance {
                backend: "unknown".to_owned(),
                adapter: "unknown".to_owned(),
                software_fallback: false,
                // A context built without adapter metadata cannot make a GPU
                // claim in a proof manifest.
                gpu_claim: false,
            },
        }
    }

    /// Build a context from an already-created adapter/device pair.
    ///
    /// The native app shares eframe's wgpu device with the media renderer.
    /// Keeping the adapter info alongside that device is important because a
    /// monitor proof must identify the backend that actually rendered it.
    /// Callers that do not have adapter metadata should use [`Self::new`],
    /// which deliberately makes no GPU claim.
    #[must_use]
    pub fn new_with_adapter_info(
        device: wgpu::Device,
        queue: wgpu::Queue,
        info: wgpu::AdapterInfo,
    ) -> Self {
        let software_fallback = info.device_type == wgpu::DeviceType::Cpu;
        Self {
            device,
            queue,
            provenance: GpuProvenance {
                backend: info.backend.to_string(),
                adapter: info.name,
                software_fallback,
                gpu_claim: !software_fallback,
            },
        }
    }

    /// Acquire a headless adapter and device for rendering.
    ///
    /// # Errors
    ///
    /// Returns a media error when no compatible adapter or device is available.
    pub fn headless(force_fallback_adapter: bool) -> Result<Self, MediaError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter,
            compatible_surface: None,
        }))
        .map_err(|error| {
            MediaError::Backend(format!("could not acquire a wgpu adapter: {error}"))
        })?;
        let info = adapter.get_info();
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("Kinewright compositor device"),
            required_limits: compositor_required_limits(wgpu::Limits::default()),
            ..Default::default()
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&descriptor)).map_err(|error| {
                MediaError::Backend(format!("could not create a wgpu device: {error}"))
            })?;
        let mut context = Self::new_with_adapter_info(device, queue, info);
        if force_fallback_adapter {
            context.provenance.software_fallback = true;
            context.provenance.gpu_claim = false;
        }
        Ok(context)
    }

    /// Describe a monitor proof rendered at the document's full raster.
    ///
    /// Test-only: fixtures that only need the adapter provenance render a
    /// 1x1 document raster at full scale by construction. Production callers
    /// go through [`Self::monitor_proof_metadata_for`], which derives the
    /// claim instead of asserting it.
    #[cfg(test)]
    pub(crate) fn monitor_proof_metadata(&self) -> MonitorProofMetadata {
        self.monitor_proof_metadata_for(RenderScale::FullResolution, (1, 1), (1, 1))
    }

    /// Describe a monitor proof, deriving `full_resolution` from the render
    /// scale that was requested and the raster that came back.
    ///
    /// CC1 5 says a thumbnail, proxy, or stale cache cannot establish
    /// conformance. Comparing the rendered raster against the document raster
    /// alone is not enough to prove that: a proxy render is sized from the
    /// document too, so the two agree whenever the proxy bound does not bite
    /// (a 1280-wide or smaller document, for instance). The requested scale is
    /// the part that says whether a proxy path was taken at all, so both must
    /// hold.
    pub(crate) fn monitor_proof_metadata_for(
        &self,
        scale: RenderScale,
        rendered: (u32, u32),
        document: (u32, u32),
    ) -> MonitorProofMetadata {
        MonitorProofMetadata {
            render_kind: MonitorProofRenderKind::GpuPreview,
            backend: self.provenance.backend.clone(),
            adapter: self.provenance.adapter.clone(),
            software_fallback: self.provenance.software_fallback,
            gpu_claim: self.provenance.gpu_claim,
            full_resolution: matches!(scale, RenderScale::FullResolution) && rendered == document,
        }
    }
}

/// A composited frame encoded for the CC1 delivery target.
///
/// RGBA64LE, full range, BT.709 transfer coded, quantized exactly once at 16
/// bits so the export path's only 8-bit quantization is the YUV420P step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryFrame {
    /// Raster width in pixels.
    pub width: u32,
    /// Raster height in pixels.
    pub height: u32,
    /// Interleaved little-endian 16-bit RGBA samples.
    pub rgba64le: Vec<u8>,
}

pub struct CompositorLayer<'a, F = FrameTexture> {
    pub frame: &'a F,
    pub effects: &'a [Effect],
    pub transition: TransitionRenderParams,
}

/// Which layer's grade buffer carries the CC5 3.2 matte-debug selector.
///
/// The selector is a word of the *layer's own* storage buffer, not a global
/// render mode, so it is resolved per layer and every other layer renders
/// colour exactly as it always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatteDebugSelection {
    /// Index into the composited layer slice.
    layer_index: usize,
    /// Zero-based index of the target node among the layer's *active* nodes.
    active_node: usize,
}

/// The colour node whose matte coverage [`Compositor::render_matte`] renders.
///
/// `clip` is carried only so the typed [`MatteProofError`] failures can name
/// the clip a caller asked about; the compositor itself addresses layers
/// positionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatteRenderTarget {
    /// Index into the composited layer slice.
    pub layer_index: usize,
    /// The clip the layer was staged from, for error reporting.
    pub clip: ClipId,
    /// The matte-carrying colour node on that layer.
    pub effect: EffectId,
}

/// Resolve a matte-proof target to its index among a layer's *active* nodes.
///
/// CC5 3.2: an inactive node is not written to the buffer and therefore shifts
/// the indices of the nodes after it, so the active index is resolved at the
/// requested frame from the same evaluated effects the serializer walks. A
/// target that is inactive, carries no matte, is not a colour node, or is
/// absent fails typed rather than silently selecting a different node.
fn matte_debug_active_index(
    effects: &[Effect],
    clip: ClipId,
    effect_id: EffectId,
) -> Result<usize, MatteProofError> {
    let mut active = 0_usize;
    for effect in effects {
        let is_target = effect.id == effect_id;
        let Some(_kind) = classify_color_node(effect) else {
            if is_target {
                return Err(MatteProofError::NotAColorNode {
                    clip,
                    effect: effect_id,
                    name: effect.name.clone(),
                });
            }
            continue;
        };
        if let Some(reason) = color_node_inactive_reason(effect) {
            if is_target {
                return Err(MatteProofError::NodeInactive {
                    reason: reason.as_str().to_owned(),
                });
            }
            continue;
        }
        if is_target {
            if !MatteParams::from_effect(effect).has_matte() {
                return Err(MatteProofError::NoMatte);
            }
            return Ok(active);
        }
        active += 1;
    }
    Err(MatteProofError::EffectNotFound {
        clip,
        effect: effect_id,
    })
}

#[doc(hidden)]
pub trait CompositorInput {
    const FORMAT: wgpu::TextureFormat;
    const BYTES_PER_PIXEL: u32;
    const LINEAR: bool;

    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn upload_bytes(&self) -> Vec<u8>;
}

impl CompositorInput for FrameTexture {
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
    const BYTES_PER_PIXEL: u32 = 4;
    const LINEAR: bool = false;

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn upload_bytes(&self) -> Vec<u8> {
        (*self.rgba).clone()
    }
}

impl CompositorInput for WorkingFrame {
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    const BYTES_PER_PIXEL: u32 = 8;
    const LINEAR: bool = true;

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn upload_bytes(&self) -> Vec<u8> {
        self.upload_bytes()
    }
}

pub struct Compositor {
    gpu: GpuContext,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The sampler used when a layer is a pixel-exact 1:1 blit; see
    /// [`Compositor::is_pixel_exact_blit`].
    point_sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    lut_cache: Mutex<HashMap<PathBuf, CachedCubeLut>>,
    /// The `S = 2` identity lattice bound in the legacy slot when no layer
    /// effect supplies one.  Held once, not rebuilt per frame, so its `Arc`
    /// identity is stable and the atlas cache below actually hits.
    identity_lut: Arc<CubeLut>,
    /// CC4 4.1's mandatory atlas cache, most recently used first. Keyed by the
    /// ordered slot signature, so playback re-uploads only when the bound set
    /// changes.
    lut_atlas_cache: Mutex<Vec<Arc<LutAtlas>>>,
    /// Per-layer source textures are recycled across `render` calls. Playback
    /// composites the same raster every frame, so allocating and destroying a
    /// texture per layer per frame is pure overhead; `write_texture` still
    /// replaces the full contents, so no stale pixels can survive.
    texture_pool: Mutex<TexturePool>,
}

/// The number of distinct (width, height, format) shapes the pool retains.
/// A resized preview or a proxy/full-raster switch must not accumulate
/// textures for every raster the session has ever used.
const TEXTURE_POOL_MAX_SHAPES: usize = 8;

/// The number of idle textures retained per shape. This bounds the pool by
/// the deepest layer stack that has actually been composited at that shape.
const TEXTURE_POOL_MAX_PER_SHAPE: usize = 8;

/// The total GPU memory the recycling pool may hold in idle textures.
///
/// The count bounds alone say nothing about size: eight shapes of eight
/// `Rgba16Float` 3840x2160 textures is roughly 4 GiB of resident VRAM held
/// purely as a reuse optimization, which is more than many cards have. 256 MiB
/// still covers a deep layer stack at one working raster (a 4K `Rgba16Float`
/// source texture is about 31.6 MiB) while staying a small fraction of a
/// modern GPU's memory.
const TEXTURE_POOL_MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TexturePoolKey {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

/// Bytes one idle texture of this shape occupies. Every pooled format is
/// uncompressed with a 1x1 block, so the block copy size is the pixel stride.
fn texture_pool_bytes(key: TexturePoolKey) -> u64 {
    let bytes_per_pixel = u64::from(key.format.block_copy_size(None).unwrap_or(8));
    u64::from(key.width)
        .saturating_mul(u64::from(key.height))
        .saturating_mul(bytes_per_pixel)
}

/// Idle source textures kept for recycling, bounded by shape count, per-shape
/// depth, and total bytes, and evicted least-recently-used shape first.
#[derive(Default)]
struct TexturePool {
    shapes: HashMap<TexturePoolKey, Vec<wgpu::Texture>>,
    /// Every retained shape, least recently used first.
    recency: Vec<TexturePoolKey>,
    bytes: u64,
}

impl TexturePool {
    fn touch(&mut self, key: TexturePoolKey) {
        if let Some(index) = self.recency.iter().position(|candidate| *candidate == key) {
            self.recency.remove(index);
        }
        self.recency.push(key);
    }

    fn take(&mut self, key: TexturePoolKey) -> Option<wgpu::Texture> {
        let texture = self.shapes.get_mut(&key)?.pop()?;
        self.bytes = self.bytes.saturating_sub(texture_pool_bytes(key));
        self.touch(key);
        Some(texture)
    }

    fn store(&mut self, key: TexturePoolKey, texture: wgpu::Texture) {
        self.touch(key);
        let textures = self.shapes.entry(key).or_default();
        if textures.len() >= TEXTURE_POOL_MAX_PER_SHAPE {
            texture.destroy();
            return;
        }
        textures.push(texture);
        self.bytes = self.bytes.saturating_add(texture_pool_bytes(key));
    }

    fn over_budget(&self) -> bool {
        self.bytes > TEXTURE_POOL_MAX_BYTES || self.shapes.len() > TEXTURE_POOL_MAX_SHAPES
    }

    fn drop_shape(&mut self, key: TexturePoolKey) {
        for texture in self.shapes.remove(&key).unwrap_or_default() {
            self.bytes = self.bytes.saturating_sub(texture_pool_bytes(key));
            texture.destroy();
        }
    }

    /// Bring the pool back inside its budgets.
    ///
    /// `hot` names the shapes the frame that just finished used. Clearing the
    /// whole pool on the ninth shape would throw those away and guarantee a
    /// reallocation on the very next frame, so cold shapes go first and their
    /// textures are explicitly destroyed rather than left for a later drop.
    fn evict(&mut self, hot: &[TexturePoolKey]) {
        let mut index = 0;
        while self.over_budget() && index < self.recency.len() {
            let key = self.recency[index];
            if hot.contains(&key) {
                index += 1;
                continue;
            }
            self.recency.remove(index);
            self.drop_shape(key);
        }
        // One shape can exceed the byte budget on its own at a large raster.
        // Trim idle depth from the least recently used shapes rather than
        // leaving the budget broken; a trimmed texture is simply recreated on
        // demand.
        let mut index = 0;
        while self.bytes > TEXTURE_POOL_MAX_BYTES && index < self.recency.len() {
            let key = self.recency[index];
            match self.shapes.get_mut(&key).and_then(Vec::pop) {
                Some(texture) => {
                    self.bytes = self.bytes.saturating_sub(texture_pool_bytes(key));
                    texture.destroy();
                }
                None => index += 1,
            }
        }
    }
}

struct LayerResources {
    texture: wgpu::Texture,
    pool_key: TexturePoolKey,
    /// The atlas this layer's bind group reads; held so the texture outlives
    /// the queue submission even if the cache evicts it meanwhile.
    _lut_atlas: Arc<LutAtlas>,
    _uniform: wgpu::Buffer,
    _grade: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct CachedCubeLut {
    modified: Option<SystemTime>,
    len: u64,
    lut: Arc<CubeLut>,
}

#[derive(Debug, Clone, Copy)]
struct LayerParams {
    brightness: f32,
    contrast: f32,
    saturation: f32,
    opacity: f32,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    fade_mix: f32,
    fade_white: f32,
    crop_left: f32,
    crop_right: f32,
    crop_top: f32,
    crop_bottom: f32,
    reframe_aspect: f32,
    reframe_focus_x: f32,
    reframe_focus_y: f32,
    exposure: f32,
    temperature: f32,
    tint: f32,
    lut_preset: f32,
    lut_intensity: f32,
    mask_shape: f32,
    mask_center_x: f32,
    mask_center_y: f32,
    mask_width: f32,
    mask_height: f32,
    mask_feather: f32,
    mask_invert: f32,
    key_red: f32,
    key_green: f32,
    key_blue: f32,
    key_threshold: f32,
    key_softness: f32,
    key_spill: f32,
    external_lut_enabled: f32,
    external_lut_intensity: f32,
    external_domain_min_r: f32,
    external_domain_min_g: f32,
    external_domain_min_b: f32,
    external_domain_max_r: f32,
    external_domain_max_g: f32,
    external_domain_max_b: f32,
    input_linear: f32,
    legacy_stage_active: f32,
    /// CC4 4.1: the two reclaimed `_uniform_padding` words. The legacy stage
    /// reads its slot's depth origin and edge length from here instead of
    /// calling `textureDimensions` on a texture it no longer owns alone.
    external_lut_z_origin: f32,
    external_lut_size: f32,
}

impl Default for LayerParams {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            opacity: 1.0,
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            fade_mix: 0.0,
            fade_white: 0.0,
            crop_left: 0.0,
            crop_right: 0.0,
            crop_top: 0.0,
            crop_bottom: 0.0,
            reframe_aspect: 0.0,
            reframe_focus_x: 0.5,
            reframe_focus_y: 0.5,
            exposure: 0.0,
            temperature: 0.0,
            tint: 0.0,
            lut_preset: 0.0,
            lut_intensity: 1.0,
            mask_shape: 0.0,
            mask_center_x: 0.5,
            mask_center_y: 0.5,
            mask_width: 1.0,
            mask_height: 1.0,
            mask_feather: 0.0,
            mask_invert: 0.0,
            key_red: 0.0,
            key_green: 1.0,
            key_blue: 0.0,
            key_threshold: -1.0,
            key_softness: 0.0,
            key_spill: 0.0,
            external_lut_enabled: 0.0,
            external_lut_intensity: 1.0,
            external_domain_min_r: 0.0,
            external_domain_min_g: 0.0,
            external_domain_min_b: 0.0,
            external_domain_max_r: 1.0,
            external_domain_max_g: 1.0,
            external_domain_max_b: 1.0,
            input_linear: 0.0,
            legacy_stage_active: 0.0,
            external_lut_z_origin: 0.0,
            external_lut_size: 2.0,
        }
    }
}

impl Compositor {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new(gpu: GpuContext) -> Self {
        let device = &gpu.device;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Kinewright compositor layer layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Kinewright compositor pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Kinewright compositor shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITOR_SHADER_SOURCE.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Kinewright compositor pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Kinewright compositor sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // See `Compositor::is_pixel_exact_blit` for why a 1:1 layer must not
        // go through the bilinear sampler.
        let point_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Kinewright compositor 1:1 point sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            gpu,
            bind_group_layout,
            sampler,
            point_sampler,
            pipeline,
            lut_cache: Mutex::new(HashMap::new()),
            identity_lut: Arc::new(CubeLut::identity()),
            lut_atlas_cache: Mutex::new(Vec::new()),
            texture_pool: Mutex::new(TexturePool::default()),
        }
    }

    /// Composite the supplied bottom-to-top layers into one monitor RGBA8
    /// frame using the CC1 default monitoring description.
    ///
    /// This is a thin wrapper over [`Self::render_monitor`]; callers that own
    /// project state should pass their document's monitoring description so
    /// CC1 2.2.6 target selection is never a compositor default.
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions or a GPU mapping failure.
    pub fn render<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
    ) -> Result<FrameTexture, MediaError> {
        self.render_monitor(resolution, layers, &ColorContext::sdr_rec709().monitoring)
    }

    /// Composite the supplied bottom-to-top layers and encode the result with
    /// the supplied monitoring description (CC1 3, monitoring branch).
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions, an unsupported
    /// monitoring transfer, or a GPU mapping failure.
    pub fn render_monitor<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
        monitoring: &ColorDescription,
    ) -> Result<FrameTexture, MediaError> {
        self.render_monitor_with_luts(resolution, layers, monitoring, None)
    }

    /// [`Self::render_monitor`] with the verified CC4 LUT library the layers'
    /// `technical_lut` / `creative_look` nodes resolve against.
    ///
    /// `None` is the look-free path: a layer carrying an *active* LUT node
    /// then fails with `missing_lut_asset` rather than silently rendering the
    /// frame without the look (CC4 2.3).
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions, an unsupported
    /// monitoring transfer, an unresolvable LUT node, or a GPU mapping
    /// failure.
    pub fn render_monitor_with_luts<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
        monitoring: &ColorDescription,
        library: Option<&LutLibrary>,
    ) -> Result<FrameTexture, MediaError> {
        let (width, height) = resolution;
        let (output, resources, encoder) = self.composite(width, height, layers, library, None)?;
        let readback = self.readback_for(width, height, &output, encoder, monitoring);
        self.release_layer_textures(resources);
        readback
    }

    /// Composite the supplied bottom-to-top layers and encode the result with
    /// the supplied delivery description (CC1 3, delivery branch).
    ///
    /// The result is RGBA64LE: the BT.709 OETF is applied in f32 and the
    /// value is quantized exactly once, at 16 bits, so the only 8-bit
    /// quantization left in the export path is the YUV420P conversion.
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions, an unsupported delivery
    /// transfer, or a GPU mapping failure.
    pub fn render_delivery<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
        delivery: &ColorDescription,
    ) -> Result<DeliveryFrame, MediaError> {
        self.render_delivery_with_luts(resolution, layers, delivery, None)
    }

    /// [`Self::render_delivery`] with the verified CC4 LUT library. See
    /// [`Self::render_monitor_with_luts`] for the `None` contract.
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions, an unsupported delivery
    /// transfer, an unresolvable LUT node, or a GPU mapping failure.
    pub fn render_delivery_with_luts<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
        delivery: &ColorDescription,
        library: Option<&LutLibrary>,
    ) -> Result<DeliveryFrame, MediaError> {
        let (width, height) = resolution;
        let (output, resources, encoder) = self.composite(width, height, layers, library, None)?;
        let readback = self.readback_rgba16(width, height, &output, encoder, delivery);
        self.release_layer_textures(resources);
        readback
    }

    /// Render one layer's CC5 matte coverage instead of its colour.
    ///
    /// The named layer is composited **alone** into a cleared target, so no
    /// other layer can paint over the coverage, and its grade buffer carries
    /// the CC5 3.2 matte-debug selector: the shader returns
    /// `vec4(m, m, m, 1)` immediately after the node stack, before the legacy
    /// stage, the key, the fade, the crop, and the mask, so nothing downstream
    /// perturbs `m` and no alpha byte is consulted.
    ///
    /// The result is one coverage byte per pixel, `round(255 * clamp(m, 0, 1))`
    /// with **no transfer function at all** — an integer quantization of a
    /// coverage scalar, which is why it does not share the monitor readback.
    ///
    /// # Errors
    ///
    /// Returns the typed [`MatteProofError`] failures when the target effect
    /// is missing, is not a colour node, is inactive at this frame, or carries
    /// no matte, plus the ordinary composite and GPU mapping failures.
    pub fn render_matte<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
        library: Option<&LutLibrary>,
        target: MatteRenderTarget,
    ) -> Result<Vec<u8>, MediaError> {
        let layer = layers.get(target.layer_index).ok_or_else(|| {
            MediaError::Backend(format!(
                "matte_proof_layer_not_found: layer {} was requested, {} were composited",
                target.layer_index,
                layers.len()
            ))
        })?;
        let active_node = matte_debug_active_index(layer.effects, target.clip, target.effect)?;
        let (width, height) = resolution;
        let isolated = [CompositorLayer {
            frame: layer.frame,
            effects: layer.effects,
            transition: layer.transition,
        }];
        let (output, resources, encoder) = self.composite(
            width,
            height,
            &isolated,
            library,
            Some(MatteDebugSelection {
                layer_index: 0,
                active_node,
            }),
        )?;
        let readback = self.readback_matte(width, height, &output, encoder);
        self.release_layer_textures(resources);
        readback
    }

    /// Read one coverage byte per pixel off the `Rgba16Float` target.
    ///
    /// Deliberately not routed through the monitor or delivery readbacks: the
    /// coverage carries no transfer function, so applying one would report a
    /// display code where the contract requires `round(255 * m)`.
    fn readback_matte(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        encoder: wgpu::CommandEncoder,
    ) -> Result<Vec<u8>, MediaError> {
        let mut coverage = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default()),
        );
        self.for_each_linear_pixel(width, height, output, encoder, |linear| {
            // The three colour channels are written identically by the shader;
            // reading red keeps the quantization single-sourced.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            coverage.push((linear[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            Ok(())
        })?;
        Ok(coverage)
    }

    /// Record one composite pass into an `Rgba16Float` render target.
    ///
    /// The layer resources are returned alongside the encoder because the
    /// bind groups they own must outlive the queue submission performed by
    /// the readback, and their pooled textures are recycled afterwards.
    fn composite<F: CompositorInput>(
        &self,
        width: u32,
        height: u32,
        layers: &[CompositorLayer<'_, F>],
        library: Option<&LutLibrary>,
        matte_debug: Option<MatteDebugSelection>,
    ) -> Result<(wgpu::Texture, Vec<LayerResources>, wgpu::CommandEncoder), MediaError> {
        if width == 0 || height == 0 {
            return Err(MediaError::Backend(
                "compositor output resolution must be non-zero".to_owned(),
            ));
        }
        let output = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright compositor output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut resources = Vec::with_capacity(layers.len());
        for (index, layer) in layers.iter().enumerate() {
            // CC5 3.2: the selector lives in this layer's own grade buffer, so
            // only the targeted layer ever renders coverage.
            let debug_node = matte_debug
                .filter(|selection| selection.layer_index == index)
                .map(|selection| selection.active_node);
            match self.layer_resources(layer, width, height, library, debug_node) {
                Ok(resource) => resources.push(resource),
                Err(error) => {
                    self.release_layer_textures(resources);
                    return Err(error);
                }
            }
        }
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Kinewright compositor commands"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kinewright composite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            for resource in &resources {
                pass.set_bind_group(0, &resource.bind_group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        Ok((output, resources, encoder))
    }

    /// Take a source texture of the requested shape from the recycling pool,
    /// creating one when the pool has none.
    fn acquire_layer_texture(&self, key: TexturePoolKey) -> wgpu::Texture {
        if let Ok(mut pool) = self.texture_pool.lock()
            && let Some(texture) = pool.take(key)
        {
            return texture;
        }
        self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright compositor source"),
            size: wgpu::Extent3d {
                width: key.width,
                height: key.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: key.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// Return this frame's source textures to the recycling pool.
    ///
    /// A poisoned lock or a full pool simply destroys the textures; recycling
    /// is an optimization and must never change rendered output.
    fn release_layer_textures(&self, resources: Vec<LayerResources>) {
        let Ok(mut pool) = self.texture_pool.lock() else {
            return;
        };
        let mut hot = Vec::new();
        for resource in resources {
            let key = resource.pool_key;
            if !hot.contains(&key) {
                hot.push(key);
            }
            pool.store(key, resource.texture);
        }
        pool.evict(&hot);
    }

    /// Is this layer a pixel-exact 1:1 blit of its source raster?
    ///
    /// The full-screen quad maps output pixel centre `(x + 0.5, y + 0.5)` onto
    /// source texel centre `(x + 0.5, y + 0.5)` whenever the source raster has
    /// the output's shape and no geometric stage (`scale`, `offset`, or
    /// `reframe`) moves it, so in exact arithmetic bilinear filtering is the
    /// identity: every weight is 0 or 1.
    ///
    /// A sampler does not run in exact arithmetic. Vulkan only requires a few
    /// fractional bits of sub-texel precision, and a conformant
    /// implementation may reconstruct `uv * dimension - 0.5` in f32, so the
    /// weight that should be exactly zero can land one ULP of the *texel*
    /// coordinate away from it. Mesa lavapipe does exactly that: on the CC3
    /// parity raster it mixed `2^-15`--`2^-14` of the neighbouring block into
    /// 32 of 3072 pixels, while the NVIDIA adapter returned every texel
    /// exactly. That is a rounding difference no driver is obliged to avoid,
    /// and it is normally invisible -- but a managed colour node is allowed to
    /// be ill-conditioned (`color_wheels` with `power < 1` has an unbounded
    /// derivative at `y = 0`), so it can be amplified into a visible grade
    /// difference between two machines rendering the same project.
    ///
    /// Point sampling is the exact realization of the identity the bilinear
    /// path is only approximating here, so a 1:1 layer takes it and every
    /// resampling layer keeps the bilinear sampler.
    ///
    /// The float comparisons are exact on purpose: a scale of `1.0 + 1e-6` is
    /// a resample, and it must keep the bilinear sampler. An epsilon here
    /// would point-sample a layer that is genuinely being resized.
    #[allow(clippy::float_cmp)]
    fn is_pixel_exact_blit<F: CompositorInput>(
        layer: &CompositorLayer<'_, F>,
        params: &LayerParams,
        width: u32,
        height: u32,
    ) -> bool {
        layer.frame.width() == width
            && layer.frame.height() == height
            && params.scale == 1.0
            && params.offset_x == 0.0
            && params.offset_y == 0.0
            && params.reframe_aspect <= 0.0
    }

    #[allow(clippy::too_many_lines)]
    fn layer_resources<F: CompositorInput>(
        &self,
        layer: &CompositorLayer<'_, F>,
        width: u32,
        height: u32,
        library: Option<&LutLibrary>,
        matte_debug_node: Option<usize>,
    ) -> Result<LayerResources, MediaError> {
        let expected_len = usize::try_from(layer.frame.width())
            .unwrap_or_default()
            .saturating_mul(usize::try_from(layer.frame.height()).unwrap_or_default())
            .saturating_mul(usize::try_from(F::BYTES_PER_PIXEL).unwrap_or_default());
        let upload_bytes = layer.frame.upload_bytes();
        if upload_bytes.len() != expected_len || expected_len == 0 {
            return Err(MediaError::Backend(
                "invalid compositor input frame".to_owned(),
            ));
        }
        let pool_key = TexturePoolKey {
            width: layer.frame.width(),
            height: layer.frame.height(),
            format: F::FORMAT,
        };
        let texture = self.acquire_layer_texture(pool_key);
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &upload_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layer.frame.width().saturating_mul(F::BYTES_PER_PIXEL)),
                rows_per_image: Some(layer.frame.height()),
            },
            wgpu::Extent3d {
                width: layer.frame.width(),
                height: layer.frame.height(),
                depth_or_array_layers: 1,
            },
        );
        let binding = self.lut_binding(layer.effects, library)?;
        let cube_lut = &binding.legacy_lut;
        let mut params = params_for(layer.effects, layer.transition);
        params.external_lut_enabled = if binding.legacy_enabled { 1.0 } else { 0.0 };
        params.external_domain_min_r = cube_lut.domain_min[0];
        params.external_domain_min_g = cube_lut.domain_min[1];
        params.external_domain_min_b = cube_lut.domain_min[2];
        params.external_domain_max_r = cube_lut.domain_max[0];
        params.external_domain_max_g = cube_lut.domain_max[1];
        params.external_domain_max_b = cube_lut.domain_max[2];
        // CC4 4.1: the legacy stage addresses its own slot of the shared
        // atlas; `2 * 65` and `65` are both exact in f32.
        #[allow(clippy::cast_precision_loss)]
        {
            params.external_lut_z_origin = binding.legacy_z_origin as f32;
            params.external_lut_size = cube_lut.size as f32;
        }
        params.input_linear = f32::from(F::LINEAR);
        params.legacy_stage_active = if legacy_stage_active(layer.effects) {
            1.0
        } else {
            0.0
        };
        let grade_bytes =
            grade_buffer_bytes_for(layer.effects, library, (width, height), matte_debug_node)?;
        let grade = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright managed colour nodes"),
            size: u64::try_from(grade_bytes.len()).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&grade, 0, &grade_bytes);
        let uniform = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright compositor layer parameters"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&uniform, 0, &params.as_bytes());
        let sampler = if Self::is_pixel_exact_blit(layer, &params, width, height) {
            &self.point_sampler
        } else {
            &self.sampler
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kinewright compositor layer bindings"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&binding.atlas.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: grade.as_entire_binding(),
                    },
                ],
            });
        Ok(LayerResources {
            texture,
            pool_key,
            _lut_atlas: binding.atlas,
            _uniform: uniform,
            _grade: grade,
            bind_group,
        })
    }

    /// Resolve one layer's LUT atlas (CC4 4.1), reusing a cached one whenever
    /// the ordered slot signature is unchanged.
    ///
    /// The slot order is the managed nodes in `clip.effects` order followed by
    /// the legacy `cube_lut` slot, which is why the legacy stage can never
    /// displace a managed one and why managed `z_origin`s are computable from
    /// the managed nodes alone. Only *bound* slots are allocated, so the
    /// atlas depth is the sum of the sizes actually in use, never
    /// `5 * Smax`.
    ///
    /// A layer that binds nothing still gets the shared `S = 2` identity as a
    /// placeholder, so binding 3 stays valid without a "no LUT" code path and
    /// the disabled legacy uniforms address an in-bounds slot.
    ///
    /// # Errors
    ///
    /// Propagates `too_many_lut_nodes` / `missing_lut_asset` from
    /// [`managed_lut_slots`], the legacy `.cube` load failure, and a poisoned
    /// cache lock.
    pub(crate) fn lut_binding(
        &self,
        effects: &[Effect],
        library: Option<&LutLibrary>,
    ) -> Result<LayerLutBinding, MediaError> {
        let mut slots = managed_lut_slots(effects, library)?;
        let legacy_z_origin = slots.iter().map(|slot| slot.lut.size).sum::<u32>();
        let (legacy_lut, legacy_enabled) = self.cube_lut(effects)?;
        if legacy_enabled || slots.is_empty() {
            // `managed_lut_slots` already capped the managed slots, so the
            // legacy lattice always lands at slot index
            // `COMPOSITOR_LEGACY_LUT_SLOT` or earlier.
            debug_assert!(slots.len() <= COMPOSITOR_LEGACY_LUT_SLOT);
            slots.push(LutAtlasSlot {
                z_origin: legacy_z_origin,
                lut: Arc::clone(&legacy_lut),
            });
        }
        let atlas = self.lut_atlas(&slots)?;
        Ok(LayerLutBinding {
            atlas,
            legacy_enabled,
            // When the legacy stage is inactive these uniforms are never read.
            // They still point at slot 0, which every atlas has, so a stale
            // read could not wander outside the texture.
            legacy_z_origin: if legacy_enabled { legacy_z_origin } else { 0 },
            legacy_lut,
        })
    }

    /// Build or reuse the atlas for an ordered slot list.
    fn lut_atlas(&self, slots: &[LutAtlasSlot]) -> Result<Arc<LutAtlas>, MediaError> {
        {
            let mut cache = self
                .lut_atlas_cache
                .lock()
                .map_err(|_| MediaError::Backend("LUT atlas cache lock was poisoned".to_owned()))?;
            if let Some(index) = cache.iter().position(|atlas| atlas.matches(slots)) {
                let atlas = cache.remove(index);
                cache.insert(0, Arc::clone(&atlas));
                return Ok(atlas);
            }
        }
        let atlas = Arc::new(self.build_lut_atlas(slots)?);
        let mut cache = self
            .lut_atlas_cache
            .lock()
            .map_err(|_| MediaError::Backend("LUT atlas cache lock was poisoned".to_owned()))?;
        cache.insert(0, Arc::clone(&atlas));
        cache.truncate(LUT_ATLAS_CACHE_ENTRIES);
        let sizes = cache.iter().map(|entry| entry.bytes()).collect::<Vec<_>>();
        cache.truncate(atlas_cache_kept_entries(&sizes, LUT_ATLAS_CACHE_MAX_BYTES));
        Ok(atlas)
    }

    /// Allocate the depth-packed atlas and upload one slot at a time.
    fn build_lut_atlas(&self, slots: &[LutAtlasSlot]) -> Result<LutAtlas, MediaError> {
        let edge = slots.iter().map(|slot| slot.lut.size).max().unwrap_or(1);
        let depth = slots.iter().map(|slot| slot.lut.size).sum::<u32>().max(1);
        if slots.len() > COMPOSITOR_LUT_ATLAS_SLOTS
            || depth > COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D
        {
            return Err(MediaError::Backend(format!(
                "lut_atlas_too_large: {} slots totalling {depth} texels of depth exceed the \
                 negotiated {COMPOSITOR_LUT_ATLAS_SLOTS} slots / \
                 {COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D} texels",
                slots.len()
            )));
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright 3D LUT atlas"),
            size: wgpu::Extent3d {
                width: edge,
                height: edge,
                depth_or_array_layers: depth,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            // CC4 4.1: f32 keeps the texels bit-identical to the parsed
            // samples the CPU reference reads, so the only CPU/GPU divergence
            // inside a LUT node is arithmetic order.
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for slot in slots {
            let size = slot.lut.size;
            let bytes = slot
                .lut
                .rgba
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: slot.z_origin,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.saturating_mul(16)),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(LutAtlas {
            texture,
            view,
            // Retaining the sources is load-bearing, not incidental: it is
            // what keeps `CachedLutSlot::matches` from being an ABA compare.
            slots: slots
                .iter()
                .map(|slot| CachedLutSlot {
                    lut: Arc::clone(&slot.lut),
                    size: slot.lut.size,
                    z_origin: slot.z_origin,
                })
                .collect(),
        })
    }

    fn cube_lut(&self, effects: &[Effect]) -> Result<(Arc<CubeLut>, bool), MediaError> {
        let Some(effect) = effects
            .iter()
            .rev()
            .find(|effect| effect.name == "cube_lut")
        else {
            // The shared identity lattice, not a fresh allocation: the atlas
            // cache keys on `Arc` identity, so a new `Arc` every frame would
            // rebuild the atlas on every frame of look-free playback.
            return Ok((Arc::clone(&self.identity_lut), false));
        };
        let Some(ParamValue::Text(path)) = effect.parameters.get("path") else {
            return Err(MediaError::Backend(
                "cube_lut requires a non-empty text path parameter".to_owned(),
            ));
        };
        if path.trim().is_empty() {
            return Err(MediaError::Backend(
                "cube_lut requires a non-empty text path parameter".to_owned(),
            ));
        }
        self.load_cube_lut(Path::new(path)).map(|lut| (lut, true))
    }

    fn load_cube_lut(&self, path: &Path) -> Result<Arc<CubeLut>, MediaError> {
        let canonical = fs::canonicalize(path).map_err(|error| {
            MediaError::Backend(format!(
                "could not resolve .cube LUT {}: {error}",
                path.display()
            ))
        })?;
        let metadata = fs::metadata(&canonical).map_err(|error| {
            MediaError::Backend(format!(
                "could not inspect .cube LUT {}: {error}",
                canonical.display()
            ))
        })?;
        let modified = metadata.modified().ok();
        {
            let cache = self
                .lut_cache
                .lock()
                .map_err(|_| MediaError::Backend("3D LUT cache lock was poisoned".to_owned()))?;
            if let Some(cached) = cache.get(&canonical)
                && cached.modified == modified
                && cached.len == metadata.len()
            {
                return Ok(Arc::clone(&cached.lut));
            }
        }
        let source = fs::read_to_string(&canonical).map_err(|error| {
            MediaError::Backend(format!(
                "could not read .cube LUT {}: {error}",
                canonical.display()
            ))
        })?;
        let lut = Arc::new(parse_cube_lut(&source)?);
        self.lut_cache
            .lock()
            .map_err(|_| MediaError::Backend("3D LUT cache lock was poisoned".to_owned()))?
            .insert(
                canonical,
                CachedCubeLut {
                    modified,
                    len: metadata.len(),
                    lut: Arc::clone(&lut),
                },
            );
        Ok(lut)
    }

    /// Copy the `Rgba16Float` render target back and visit every pixel as
    /// linear f32 RGBA.
    ///
    /// The output transform is the caller's: this helper deliberately owns no
    /// colour policy, so monitoring and delivery encodings stay selected from
    /// their `ColorDescription`.
    fn for_each_linear_pixel(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
        mut visit: impl FnMut([f32; 4]) -> Result<(), MediaError>,
    ) -> Result<(), MediaError> {
        let row_bytes = width.saturating_mul(8);
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_size = u64::from(padded_row_bytes).saturating_mul(u64::from(height));
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright compositor readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| MediaError::Backend(format!("wgpu readback poll failed: {error}")))?;
        receiver
            .recv()
            .map_err(|_| MediaError::Backend("wgpu readback callback stopped".to_owned()))?
            .map_err(|error| MediaError::Backend(format!("wgpu readback map failed: {error}")))?;
        let mapped = slice.get_mapped_range();
        let mut outcome = Ok(());
        'rows: for row in 0..usize::try_from(height).unwrap_or_default() {
            let start = row.saturating_mul(usize::try_from(padded_row_bytes).unwrap_or_default());
            let end = start.saturating_add(usize::try_from(row_bytes).unwrap_or_default());
            for pixel in mapped[start..end].as_chunks::<8>().0 {
                let linear = [
                    f16::from_le_bytes([pixel[0], pixel[1]]).to_f32(),
                    f16::from_le_bytes([pixel[2], pixel[3]]).to_f32(),
                    f16::from_le_bytes([pixel[4], pixel[5]]).to_f32(),
                    f16::from_le_bytes([pixel[6], pixel[7]]).to_f32(),
                ];
                if let Err(error) = visit(linear) {
                    outcome = Err(error);
                    break 'rows;
                }
            }
        }
        drop(mapped);
        buffer.unmap();
        outcome
    }

    /// Encode the composited working surface for the supplied monitoring
    /// description (CC1 2.2.6).
    ///
    /// The BT.709 OETF is applied in f32 and RGB is clamped and quantized
    /// exactly once, here, at the display boundary.
    ///
    /// Deliberately no `powf` lookup table: a 4096-entry LUT was rejected
    /// because interpolation error near black, where the OETF slope is 4.5,
    /// does not provably stay inside the CC1 6.2 monitor gate (max <= 2,
    /// P99 <= 1, mean <= 0.5) against the CPU reference. The exact math stays.
    fn readback_for(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        encoder: wgpu::CommandEncoder,
        monitoring: &ColorDescription,
    ) -> Result<FrameTexture, MediaError> {
        let mut rgba = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default())
                .saturating_mul(4),
        );
        self.for_each_linear_pixel(width, height, output, encoder, |linear| {
            let monitor_code =
                encode_monitor_rgba8_for_description(linear, monitoring).map_err(|error| {
                    MediaError::Backend(format!(
                        "managed monitoring encode rejected (transfer={:?}): {error}",
                        monitoring.transfer
                    ))
                })?;
            rgba.extend_from_slice(&monitor_code);
            Ok(())
        })?;
        Ok(FrameTexture {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }

    /// Encode the composited working surface for the supplied delivery
    /// description as RGBA64LE (CC1 3, delivery branch).
    fn readback_rgba16(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        encoder: wgpu::CommandEncoder,
        delivery: &ColorDescription,
    ) -> Result<DeliveryFrame, MediaError> {
        let mut rgba64le = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default())
                .saturating_mul(8),
        );
        self.for_each_linear_pixel(width, height, output, encoder, |linear| {
            let delivery_code =
                encode_delivery_for_description(linear, delivery).map_err(|error| {
                    MediaError::Backend(format!(
                        "managed delivery encode rejected (transfer={:?}): {error}",
                        delivery.transfer
                    ))
                })?;
            for channel in delivery_code {
                rgba64le.extend_from_slice(&channel.to_le_bytes());
            }
            Ok(())
        })?;
        Ok(DeliveryFrame {
            width,
            height,
            rgba64le,
        })
    }

    /// Render and read back the production working surface for CC1 evidence.
    ///
    /// This is deliberately test-only: the public compositor contract ends at
    /// the monitor `FrameTexture`, while the fixture gate needs to compare the
    /// actual `Rgba16Float` render target against the canonical CPU reference.
    #[cfg(test)]
    pub(crate) fn render_working(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, WorkingFrame>],
    ) -> Result<Vec<f32>, MediaError> {
        self.render_working_with_luts(resolution, layers, None)
    }

    /// [`Self::render_working`] with the verified CC4 LUT library.
    #[cfg(test)]
    pub(crate) fn render_working_with_luts(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, WorkingFrame>],
        library: Option<&LutLibrary>,
    ) -> Result<Vec<f32>, MediaError> {
        let (width, height) = resolution;
        let (output, resources, encoder) = self.composite(width, height, layers, library, None)?;
        let mut values = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default())
                .saturating_mul(4),
        );
        let readback = self
            .for_each_linear_pixel(width, height, &output, encoder, |linear| {
                values.extend(linear);
                Ok(())
            })
            .map(|()| values);
        self.release_layer_textures(resources);
        readback
    }
}

impl LayerParams {
    fn as_bytes(self) -> [u8; UNIFORM_BYTES] {
        let values = [
            self.brightness,
            self.contrast,
            self.saturation,
            self.opacity,
            self.scale,
            self.offset_x,
            self.offset_y,
            self.fade_mix,
            self.fade_white,
            self.crop_left,
            self.crop_right,
            self.crop_top,
            self.crop_bottom,
            self.reframe_aspect,
            self.reframe_focus_x,
            self.reframe_focus_y,
            self.exposure,
            self.temperature,
            self.tint,
            self.lut_preset,
            self.lut_intensity,
            self.mask_shape,
            self.mask_center_x,
            self.mask_center_y,
            self.mask_width,
            self.mask_height,
            self.mask_feather,
            self.mask_invert,
            self.key_red,
            self.key_green,
            self.key_blue,
            self.key_threshold,
            self.key_softness,
            self.key_spill,
            self.external_lut_enabled,
            self.external_lut_intensity,
            self.external_domain_min_r,
            self.external_domain_min_g,
            self.external_domain_min_b,
            self.external_domain_max_r,
            self.external_domain_max_g,
            self.external_domain_max_b,
            self.input_linear,
            self.legacy_stage_active,
            self.external_lut_z_origin,
            self.external_lut_size,
        ];
        let mut bytes = [0_u8; UNIFORM_BYTES];
        for (index, value) in values.into_iter().enumerate() {
            let start = index * 4;
            bytes[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

/// How many leading atlas-cache entries fit inside the retained-byte budget.
///
/// `sizes` is most-recently-used first, so this is a prefix decision: keep
/// entries until adding one more would exceed `max_bytes`, then drop the whole
/// tail. The head is *never* dropped — an atlas that alone exceeds the budget
/// still stays cached, because it is the one the current frame just built and
/// evicting it would guarantee a rebuild on the very next frame while freeing
/// nothing that is not already alive through the caller's `Arc`.
///
/// Split out from the cache write so the budget is testable without a GPU:
/// building a real [`LutAtlas`] needs a device, and the sizes that make the
/// trim interesting are hundreds of megabytes of texture.
fn atlas_cache_kept_entries(sizes: &[u64], max_bytes: u64) -> usize {
    let mut retained = 0_u64;
    for (index, bytes) in sizes.iter().enumerate() {
        retained = retained.saturating_add(*bytes);
        if retained > max_bytes && index > 0 {
            return index;
        }
    }
    sizes.len()
}

/// One bound slot: which lattice, where it starts in the atlas depth.
#[derive(Clone)]
struct LutAtlasSlot {
    z_origin: u32,
    lut: Arc<CubeLut>,
}

/// The retained identity of one lattice bound to a cached atlas (CC4 4.1).
///
/// The contract keys the atlas cache on the ordered `(sha256, size)` list of
/// bound slots. A [`LutLibrary`] is looked up by
/// [`kinewright_core::LutAssetId`], and the hash it retains per entry is not
/// carried down to a bound slot, so this uses the identity of the verified
/// lattice instead — and *retains a strong `Arc` to it*. Retention is what makes the
/// comparison sound: a cached atlas keeps its own sources alive, so no other
/// allocation can ever occupy those addresses while the atlas is cached, and
/// `Arc::ptr_eq` therefore proves the very same verified samples rather than
/// merely the same address. Comparing an unretained raw pointer would be an
/// ABA bug: an edited LUT reparsed into a freed allocation's address would
/// have hit a stale atlas and rendered the old look.
///
/// This is *stricter* than the contract's hash key, never looser: two distinct
/// allocations of identical bytes miss the cache and rebuild.
#[derive(Clone)]
struct CachedLutSlot {
    lut: Arc<CubeLut>,
    size: u32,
    z_origin: u32,
}

impl CachedLutSlot {
    /// Whether this cached slot is the same lattice bound the same way.
    fn matches(&self, slot: &LutAtlasSlot) -> bool {
        Arc::ptr_eq(&self.lut, &slot.lut)
            && self.size == slot.lut.size
            && self.z_origin == slot.z_origin
    }
}

/// CC4 4.1's single depth-packed `Rgba32Float` 3D atlas at binding 3.
///
/// Dimensions are `(Smax, Smax, sum of the bound slot sizes)`; a slot smaller
/// than `Smax` simply leaves the trailing texels of its `x`/`y` rows unused.
/// WGSL has no `texture_3d_array` and separate bindings are capped at sixteen
/// sampled textures per stage, so depth packing is what keeps the binding
/// count unchanged.
pub(crate) struct LutAtlas {
    /// The owning handle for `view`, and the source of the cache's size
    /// accounting.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The bound slots in slot order: managed slots `0..n`, then the legacy
    /// `cube_lut` slot. This vector *is* the cache key, and it holds a strong
    /// `Arc` to every lattice it was built from, so a cached atlas pins its
    /// own sources for as long as it can be served.
    slots: Vec<CachedLutSlot>,
}

impl LutAtlas {
    /// Whether this atlas was built from exactly this ordered slot list.
    fn matches(&self, slots: &[LutAtlasSlot]) -> bool {
        self.slots.len() == slots.len()
            && self
                .slots
                .iter()
                .zip(slots)
                .all(|(cached, slot)| cached.matches(slot))
    }

    /// The `(size, z_origin)` of each bound slot, in slot order.
    #[cfg(test)]
    pub(crate) fn slot_layout(&self) -> Vec<(u32, u32)> {
        self.slots
            .iter()
            .map(|slot| (slot.size, slot.z_origin))
            .collect()
    }

    /// The lattices this atlas retains, in slot order.
    #[cfg(test)]
    pub(crate) fn retained_slots(&self) -> Vec<Arc<CubeLut>> {
        self.slots
            .iter()
            .map(|slot| Arc::clone(&slot.lut))
            .collect()
    }

    /// The allocated atlas extent, `(width, height, depth)`.
    #[cfg(test)]
    pub(crate) fn extent(&self) -> (u32, u32, u32) {
        let size = self.texture.size();
        (size.width, size.height, size.depth_or_array_layers)
    }

    /// The GPU memory this atlas occupies, for the cache's byte budget.
    fn bytes(&self) -> u64 {
        let size = self.texture.size();
        u64::from(size.width)
            .saturating_mul(u64::from(size.height))
            .saturating_mul(u64::from(size.depth_or_array_layers))
            .saturating_mul(16)
    }
}

/// Everything one layer needs from the LUT atlas: the texture its bind group
/// reads, and where the legacy stage's slot lives inside it.
pub(crate) struct LayerLutBinding {
    atlas: Arc<LutAtlas>,
    legacy_enabled: bool,
    legacy_lut: Arc<CubeLut>,
    legacy_z_origin: u32,
}

/// How many *active* LUT nodes a layer carries (CC4 3.6: an inactive node is
/// the exact identity, is never written, and occupies no atlas slot).
fn active_lut_node_count(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            classify_color_node(effect).is_some_and(ColorNodeKind::is_lut)
                && color_node_inactive_reason(effect).is_none()
        })
        .count()
}

/// Assign atlas slots to the layer's active LUT nodes, in record order.
///
/// Slot `k` is the `k`-th active LUT node in `clip.effects` order and starts at
/// the running depth sum, so the ordered stack keeps its serialized order and
/// the legacy slot that follows can never displace a managed one.
///
/// # Errors
///
/// * `too_many_lut_nodes:` when more than [`COMPOSITOR_LUT_SLOTS_PER_LAYER`]
///   LUT nodes are active. Core rejects this on the edit path
///   (`LUT_NODE_LIMIT_PER_LAYER`); this is the defensive gate.
/// * `missing_lut_asset:` when an active node's asset is not in the verified
///   library. CC4 2.3 forbids silently dropping the node, so the render fails.
fn managed_lut_slots(
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Result<Vec<LutAtlasSlot>, MediaError> {
    let active = active_lut_node_count(effects);
    if active > COMPOSITOR_LUT_SLOTS_PER_LAYER {
        return Err(MediaError::Backend(format!(
            "too_many_lut_nodes: layer carries {active} active LUT nodes, \
             at most {COMPOSITOR_LUT_SLOTS_PER_LAYER} are allowed"
        )));
    }
    let mut slots: Vec<LutAtlasSlot> = Vec::with_capacity(active);
    let mut z_origin = 0_u32;
    for effect in effects {
        let Some(kind) = classify_color_node(effect) else {
            continue;
        };
        if !kind.is_lut() || color_node_inactive_reason(effect).is_some() {
            continue;
        }
        let params = LutNodeParams::from_effect(effect);
        let asset = params.lut_asset_id;
        let lut = library
            .and_then(|library| library.get(asset))
            .ok_or_else(|| {
                MediaError::Backend(format!(
                    "missing_lut_asset: {} node {} references LUT asset {}, which is not in the \
                 verified LUT library; restore or relink the asset before rendering",
                    kind.effect_name(),
                    effect.id.0,
                    asset.0
                ))
            })?;
        slots.push(LutAtlasSlot {
            z_origin,
            lut: Arc::clone(lut),
        });
        z_origin = z_origin.saturating_add(lut.size);
    }
    Ok(slots)
}

/// One managed colour node resolved to the words the shader reads.
struct GradeNodeRecord {
    kind: ColorNodeKind,
    /// `v0 .. v11` of the node record. Primary uses `v0..v9` exactly as CC1
    /// wrote them; wheels use `v0..v2` slope, `v3..v5` offset, `v6..v8` power;
    /// curves leave the whole block zero and carry a payload instead.
    ///
    /// CC5 3.1: `v11` is overwritten with the matte payload word offset by the
    /// serializer, so no kind may use it for a value.
    values: [f32; GRADE_NODE_VALUE_WORDS],
    /// The `4 * 49` curve payload words, empty for every other kind.
    payload: Vec<f32>,
    /// CC5 3.1: the 64-word matte block, `None` for a node whose matte is
    /// inactive.  An inactive matte writes no block at all and leaves `v11`
    /// zero, which is what makes a pre-CC5 project render bit-identically.
    matte: Option<[f32; MATTE_BLOCK_WORDS]>,
}

/// CC5 3.1: build one node's 64-word matte block.
///
/// Every stored integer is converted here, once, so the shader consumes plain
/// floats and never re-derives a unit.  `cosT` / `sinT` are solved in `f64`
/// and rounded once to `f32`, so the CPU reference and the shader consume
/// *identical* constants rather than two independently rounded rotations
/// (CC5 2.3).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn matte_block_words(matte: &MatteParams, raster_aspect: f32) -> [f32; MATTE_BLOCK_WORDS] {
    // Basis points to a `0..=1` scale. `0..=10000` is exactly representable in
    // `f32`, so both endpoints are exact; the `f64` divide keeps the single
    // rounding at the end.
    let basis_points = |value: i64| (value as f64 / 10_000.0) as f32;
    // Hundredths of a degree to degrees.
    let centidegrees = |value: i64| (value as f64 / 100.0) as f32;

    let mut words = [0.0_f32; MATTE_BLOCK_WORDS];
    words[0] = matte.window_count as f32;
    words[1] = if matte.intersects() { 1.0 } else { 0.0 };
    words[2] = if matte.qualifier.is_enabled() {
        1.0
    } else {
        0.0
    };
    words[3] = if matte.is_inverted() { 1.0 } else { 0.0 };
    words[4] = matte.mix();
    words[5] = raster_aspect;
    words[6] = centidegrees(matte.qualifier.hue_center_cd);
    words[7] = centidegrees(matte.qualifier.hue_width_cd);
    words[8] = centidegrees(matte.qualifier.hue_softness_cd);
    words[9] = basis_points(matte.qualifier.sat_low_bp);
    words[10] = basis_points(matte.qualifier.sat_high_bp);
    words[11] = basis_points(matte.qualifier.sat_softness_bp);
    words[12] = basis_points(matte.qualifier.luma_low_bp);
    words[13] = basis_points(matte.qualifier.luma_high_bp);
    words[14] = basis_points(matte.qualifier.luma_softness_bp);
    // word 15 stays the reserved 0.0.
    for (index, window) in matte.active_windows().enumerate() {
        let base = MATTE_WINDOW_BASE_WORD + index * MATTE_WINDOW_WORDS;
        let theta = (window.rotation_cd as f64 / 100.0).to_radians();
        words[base] = window.shape_token as f32;
        words[base + 1] = basis_points(window.center_x_bp);
        words[base + 2] = basis_points(window.center_y_bp);
        words[base + 3] = basis_points(window.half_width_bp);
        words[base + 4] = basis_points(window.half_height_bp);
        words[base + 5] = theta.cos() as f32;
        words[base + 6] = theta.sin() as f32;
        words[base + 7] = basis_points(window.feather_bp);
        words[base + 8] = if window.is_inverted() { 1.0 } else { 0.0 };
        // words `+9 .. +11` stay the reserved 0.0; windows at index
        // `>= window_count` stay entirely zero and are never read.
    }
    words
}

/// Serialize the ordered managed colour-node stack into one storage buffer
/// (CC3 3.2).
///
/// Nodes are never collapsed, reordered, or merged: the order in the
/// document's effect vector is the execution order in the shader. Inactive
/// nodes — bypassed or neutral — are not written at all (CC3 3.3), which is
/// what makes the identity gate bit-identical rather than tolerance-bounded.
/// A single zeroed node record keeps the storage binding valid when the stack
/// resolves to nothing.
///
/// The layout, little-endian throughout:
///
/// ```text
/// byte 0    header: [active_count, payload_word_offset, abi_version, 0] as u32
/// byte 16   words[0]: node record 0, then record 1, ... stride 64 bytes
///           record = [kind, payload_word_offset, bypass, reserved, v0 .. v11] as f32
/// byte 16 + 64 * active_count
///           payload region, in node-record order; for one node the curve
///           payload comes first, then its CC5 matte block
///           curve payload = 196 words,
///             slot = [count, x0, y0, m0, ... x15, y15, m15],
///             ordered red, green, blue, master
///           matte block  = 64 words (CC5 3.1), addressed by the record's v11
/// ```
///
/// Every stored word offset is an index into `words` (that is, into the buffer
/// with its 16-byte header removed), because that is what the shader's
/// `curve_eval` consumes directly.
///
/// # Errors
///
/// Returns `too_many_color_nodes` when the layer carries more than
/// [`COLOR_NODE_LIMIT_PER_LAYER`] managed nodes — Core rejects that on the
/// edit path, so this is a defensive gate — and a backend error when a
/// `primary_correction` node cannot be resolved.
///
/// A layer with an active LUT node is rejected here, because no library was
/// supplied and CC4 2.3 forbids rendering a look-free frame in place of a look
/// the project asked for. Callers that can carry looks use
/// [`grade_buffer_bytes_with_luts`].
///
/// Test-only: the production layer path always has a library handle, so this
/// arity exists for the CC1/CC3 fixtures, whose stacks predate LUT nodes.
#[cfg(test)]
pub(crate) fn grade_buffer_bytes(effects: &[Effect]) -> Result<Vec<u8>, MediaError> {
    grade_buffer_bytes_with_luts(effects, None)
}

/// The output raster aspect `a = W / H` the matte block carries (CC5 2.3).
///
/// Host-supplied rather than sniffed from `textureDimensions`, and computed
/// once per render from the resolution the compositor already owns, so the
/// shader and the CPU reference consume the same value.
#[allow(clippy::cast_precision_loss)]
fn raster_aspect_of(resolution: (u32, u32)) -> f32 {
    let (width, height) = resolution;
    if height == 0 {
        return 1.0;
    }
    width as f32 / height as f32
}

/// Serialize the ordered managed colour-node stack, resolving CC4 LUT nodes
/// against the verified library.
///
/// The LUT record (CC4 4.2) carries no payload: `payload_word_offset` stays
/// `0` and the twelve value words are
/// `[slot, mix, encoding, dmin rgb, dmax rgb, S, z_origin, 0]`. `slot` and
/// `z_origin` are assigned by [`managed_lut_slots`], so the buffer and the
/// atlas agree by construction.
///
/// # Errors
///
/// As [`grade_buffer_bytes`], plus `too_many_lut_nodes` and
/// `missing_lut_asset` from [`managed_lut_slots`], and
/// `matte_requires_raster_aspect` when any node carries an active matte:
/// matte serialization needs the raster aspect, so such stacks must go
/// through [`grade_buffer_bytes_with_matte`] / [`grade_buffer_bytes_for`].
#[cfg(test)]
pub(crate) fn grade_buffer_bytes_with_luts(
    effects: &[Effect],
    library: Option<&LutLibrary>,
) -> Result<Vec<u8>, MediaError> {
    if effects.iter().any(|effect| {
        classify_color_node(effect).is_some()
            && color_node_inactive_reason(effect).is_none()
            && MatteParams::from_effect(effect).has_matte()
    }) {
        return Err(MediaError::Backend(
            "matte_requires_raster_aspect: a matte-carrying stack must be serialized through \
             grade_buffer_bytes_for, which supplies the output raster aspect (CC5 3.2)"
                .to_owned(),
        ));
    }
    // No matte block is written, so the aspect word is never emitted and the
    // placeholder cannot reach a shader.
    grade_buffer_bytes_with_matte(effects, library, 1.0, None)
}

/// [`grade_buffer_bytes_with_matte`] with the raster aspect derived from the
/// output resolution the compositor already owns (CC5 3.2).
pub(crate) fn grade_buffer_bytes_for(
    effects: &[Effect],
    library: Option<&LutLibrary>,
    resolution: (u32, u32),
    matte_debug_node: Option<usize>,
) -> Result<Vec<u8>, MediaError> {
    grade_buffer_bytes_with_matte(
        effects,
        library,
        raster_aspect_of(resolution),
        matte_debug_node,
    )
}

/// Serialize the ordered managed colour-node stack with CC5 matte blocks.
///
/// `raster_aspect` is `W / H` of the *output* raster and is written into every
/// matte block's word 5; `matte_debug_node` is the zero-based index of an
/// **active** node whose coverage the shader should return instead of colour,
/// stored as `header.w = index + 1` (CC5 3.2). The index is resolved against
/// the records this call actually writes, so an inactive node earlier in the
/// stack shifts it — which is why callers resolve it from the same evaluated
/// effects.
///
/// A node whose matte is inactive (CC5 2.6) gets no block and keeps `v11 = 0`,
/// so the buffer, and therefore the render, is byte-identical to CC4.
///
/// # Errors
///
/// As [`grade_buffer_bytes_with_luts`], plus `matte_debug_node_out_of_range`
/// when the selector names a node this stack did not write.
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub(crate) fn grade_buffer_bytes_with_matte(
    effects: &[Effect],
    library: Option<&LutLibrary>,
    raster_aspect: f32,
    matte_debug_node: Option<usize>,
) -> Result<Vec<u8>, MediaError> {
    let managed = managed_color_node_count(effects);
    if managed > COLOR_NODE_LIMIT_PER_LAYER {
        return Err(MediaError::Backend(format!(
            "too_many_color_nodes: layer carries {managed} managed colour nodes, \
             at most {COLOR_NODE_LIMIT_PER_LAYER} are allowed"
        )));
    }
    let lut_slots = managed_lut_slots(effects, library)?;
    let mut next_lut_slot = 0_usize;
    let mut records = Vec::new();
    for effect in effects {
        let Some(kind) = classify_color_node(effect) else {
            continue;
        };
        // CC3 3.3 / CC4 3.6: an inactive node is the exact identity and must
        // not reach the GPU buffer or occupy an atlas slot.  Keyframes are
        // already resolved by the caller.
        if color_node_inactive_reason(effect).is_some() {
            continue;
        }
        // CC5 2.6 / 3.1: the matte is resolved from the same evaluated
        // integers the inactivity test above used.  `technical_lut` carries no
        // `matte_*` parameter, so `MatteParams::from_effect` returns the
        // neutral for it and no block is ever written.
        let matte = MatteParams::from_effect(effect);
        let matte_block = matte
            .has_matte()
            .then(|| matte_block_words(&matte, raster_aspect));
        records.push(match kind {
            ColorNodeKind::Primary => GradeNodeRecord {
                kind,
                values: primary_node_values(effect)?,
                payload: Vec::new(),
                matte: matte_block,
            },
            ColorNodeKind::Wheels => GradeNodeRecord {
                kind,
                values: wheels_node_values(&ColorWheelsParams::from_effect(effect)),
                payload: Vec::new(),
                matte: matte_block,
            },
            ColorNodeKind::Curves => GradeNodeRecord {
                kind,
                values: [0.0; GRADE_NODE_VALUE_WORDS],
                payload: curve_payload_words(&ResolvedCurves::from_effect(effect)),
                matte: matte_block,
            },
            ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
                let slot_index = next_lut_slot;
                next_lut_slot += 1;
                // `managed_lut_slots` walked the same effects in the same
                // order under the same activity test, so this index exists.
                let slot = lut_slots.get(slot_index).ok_or_else(|| {
                    MediaError::Backend(
                        "LUT slot assignment disagreed with the node record order".to_owned(),
                    )
                })?;
                GradeNodeRecord {
                    kind,
                    values: lut_node_values(&LutNodeParams::from_effect(effect), slot_index, slot),
                    payload: Vec::new(),
                    matte: matte_block,
                }
            }
        });
    }

    let count = records.len();
    if let Some(node) = matte_debug_node
        && node >= count
    {
        return Err(MediaError::Backend(format!(
            "matte_debug_node_out_of_range: active node {node} was requested, \
             the layer wrote {count} active colour nodes"
        )));
    }
    let payload_word_offset = count.saturating_mul(GRADE_NODE_WORDS);
    // A zero-node stack still allocates one record so the runtime-sized
    // `array<f32>` binding stays valid; the shader skips it on `header.x`.
    let record_words = count.max(1).saturating_mul(GRADE_NODE_WORDS);
    let payload_words: usize = records
        .iter()
        .map(|record| {
            record.payload.len()
                + if record.matte.is_some() {
                    MATTE_BLOCK_WORDS
                } else {
                    0
                }
        })
        .sum();
    let mut words = vec![0.0_f32; record_words.saturating_add(payload_words)];
    let mut next_payload = payload_word_offset;
    for (index, record) in records.iter().enumerate() {
        let base = index * GRADE_NODE_WORDS;
        words[base] = record.kind.storage_buffer_tag() as f32;
        words[base + 1] = if record.payload.is_empty() {
            0.0
        } else {
            next_payload as f32
        };
        // Bypassed nodes are filtered out above, so the shader's honoured
        // bypass word is always inactive in a buffer this function produced.
        words[base + 2] = 0.0;
        words[base + 3] = 0.0;
        let values = base + GRADE_NODE_VALUE_OFFSET;
        words[values..values + GRADE_NODE_VALUE_WORDS].copy_from_slice(&record.values);
        if !record.payload.is_empty() {
            // `record_words == payload_word_offset` whenever a record exists,
            // so the stored offset indexes `words` directly.
            let end = next_payload.saturating_add(record.payload.len());
            words[next_payload..end].copy_from_slice(&record.payload);
            next_payload = end;
        }
        // CC5 3.1: payloads are appended in node order; for one node the curve
        // payload comes first, then the matte block.  `v11` is the block's own
        // word index, so the shader needs no per-kind arithmetic to find it.
        if let Some(block) = record.matte.as_ref() {
            let end = next_payload.saturating_add(MATTE_BLOCK_WORDS);
            words[next_payload..end].copy_from_slice(block);
            words[base + GRADE_NODE_MATTE_OFFSET_WORD] = next_payload as f32;
            next_payload = end;
        }
    }

    let mut bytes = Vec::with_capacity(GRADE_HEADER_BYTES + words.len() * 4);
    let header = [
        u32::try_from(count).unwrap_or(u32::MAX),
        u32::try_from(payload_word_offset).unwrap_or(u32::MAX),
        GRADE_ABI_VERSION,
        // CC5 3.2: `header.w` is the matte-debug selector. `0` is normal
        // rendering; `k > 0` returns the coverage of active node `k - 1`.
        matte_debug_node.map_or(0, |node| u32::try_from(node + 1).unwrap_or(u32::MAX)),
    ];
    for value in header {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    debug_assert!(
        bytes.len() <= GRADE_BUFFER_WORST_CASE_BYTES,
        "grade buffer exceeded the CC3 worst case"
    );
    Ok(bytes)
}

/// CC4 4.2 LUT node values, `v0 .. v11`.
///
/// The domain and edge length come from the *verified bytes* the library
/// admitted, never from the document's informational mirrors (CC4 2.1), so a
/// hand-edited record can never change what is rendered.
#[allow(clippy::cast_precision_loss)]
fn lut_node_values(
    params: &LutNodeParams,
    slot_index: usize,
    slot: &LutAtlasSlot,
) -> [f32; GRADE_NODE_VALUE_WORDS] {
    let lut = &slot.lut;
    [
        // `slot_index <= 3` and `z_origin <= 4 * 65`, both exact in f32.
        slot_index as f32,
        params.mix(),
        params.input_encoding_token as f32,
        lut.domain_min[0],
        lut.domain_min[1],
        lut.domain_min[2],
        lut.domain_max[0],
        lut.domain_max[1],
        lut.domain_max[2],
        lut.size as f32,
        slot.z_origin as f32,
        0.0,
    ]
}

/// CC1 primary values, `v0 .. v9`, byte-identical to the pre-CC3 serializer.
#[allow(clippy::cast_precision_loss)]
fn primary_node_values(effect: &Effect) -> Result<[f32; GRADE_NODE_VALUE_WORDS], MediaError> {
    let correction = PrimaryCorrection::from_effect(effect).map_err(|error| {
        MediaError::Backend(format!("managed primary correction failed: {error}"))
    })?;
    Ok([
        correction.exposure_milli_stops as f32 / 1_000.0,
        correction.temperature_percent as f32 / 100.0,
        correction.tint_percent as f32 / 100.0,
        correction.contrast_percent as f32 / 100.0,
        correction.contrast_pivot_basis_points as f32 / 10_000.0,
        correction.blacks_percent as f32 / 100.0,
        correction.shadows_percent as f32 / 100.0,
        correction.highlights_percent as f32 / 100.0,
        correction.whites_percent as f32 / 100.0,
        correction.saturation_percent as f32 / 100.0,
        0.0,
        0.0,
    ])
}

/// CC3 2.2 slope/offset/power, resolved per channel:
/// `slope = (gain_c / 1000) * (gain_master / 1000)`,
/// `offset = (lift_c + lift_master) / 10000`,
/// `power = (gamma_c / 1000) * (gamma_master / 1000)`.
///
/// Master composes multiplicatively for gain and power — exponents compose by
/// multiplication, so `(x^a)^b = x^(a*b)` is exact — and additively for lift.
#[allow(clippy::cast_precision_loss)]
fn wheels_node_values(params: &ColorWheelsParams) -> [f32; GRADE_NODE_VALUE_WORDS] {
    let master_gain = params.gain_thousandths.master as f32 / 1_000.0;
    let master_gamma = params.gamma_thousandths.master as f32 / 1_000.0;
    let master_lift = params.lift_basis_points.master;
    let mut values = [0.0; GRADE_NODE_VALUE_WORDS];
    for (index, channel) in [
        ColorWheelChannel::Red,
        ColorWheelChannel::Green,
        ColorWheelChannel::Blue,
    ]
    .into_iter()
    .enumerate()
    {
        values[index] = params.gain_thousandths.channel(channel) as f32 / 1_000.0 * master_gain;
        values[index + 3] =
            (params.lift_basis_points.channel(channel) + master_lift) as f32 / 10_000.0;
        values[index + 6] =
            params.gamma_thousandths.channel(channel) as f32 / 1_000.0 * master_gamma;
    }
    values
}

/// The four `[count, x, y, m ...]` slots of a curve node, ordered red, green,
/// blue, master (CC3 3.2). Unused point slots stay zero.
fn curve_payload_words(curves: &ResolvedCurves) -> Vec<f32> {
    let mut payload = Vec::with_capacity(GRADE_CURVE_PAYLOAD_WORDS);
    for channel in [
        ColorCurveChannel::Red,
        ColorCurveChannel::Green,
        ColorCurveChannel::Blue,
        ColorCurveChannel::Master,
    ] {
        payload.extend_from_slice(&curve_slot_words(curves.curve(channel)));
    }
    payload
}

/// One curve slot: the point count, then `x`, `y`, and the host-solved tangent
/// of each point, in `grade709` units.
#[allow(clippy::cast_precision_loss)]
fn curve_slot_words(points: &CurvePoints) -> [f32; GRADE_CURVE_SLOT_WORDS] {
    let mut slot = [0.0; GRADE_CURVE_SLOT_WORDS];
    let xs = points
        .points
        .iter()
        .map(|(x, _)| *x as f32 / CURVE_BASIS_POINT_SCALE)
        .collect::<Vec<_>>();
    let ys = points
        .points
        .iter()
        .map(|(_, y)| *y as f32 / CURVE_BASIS_POINT_SCALE)
        .collect::<Vec<_>>();
    let tangents = fritsch_carlson_tangents(&xs, &ys);
    slot[0] = xs.len() as f32;
    for (index, ((x, y), tangent)) in xs.iter().zip(&ys).zip(&tangents).enumerate() {
        let base = 1 + index * 3;
        slot[base] = *x;
        slot[base + 1] = *y;
        slot[base + 2] = *tangent;
    }
    slot
}

/// Solve monotone cubic Hermite tangents with Fritsch-Carlson limiting
/// (CC3 2.3 steps 1-3), on the host, once per curve.
///
/// The forward, in-place ordering of the limiting pass is part of the
/// contract: a different visitation order produces different tangents for some
/// inputs, because step `i` reads the `m[i]` that step `i - 1` may have
/// rewritten. The arithmetic is f32 for the same reason the shader's is —
/// the buffer stores f32 and the fixture gate compares the two.
///
/// This is production code. The CPU reference in `color_pipeline.rs`
/// implements the same written algorithm independently and must not call this
/// function, so parity fixtures compare two implementations of the contract
/// rather than one implementation with itself.
fn fritsch_carlson_tangents(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let count = xs.len().min(ys.len());
    if count < 2 {
        return vec![0.0; count];
    }
    // Core guarantees strictly increasing `x` (CC3 3.4), so the span is
    // always positive in production. The guard is defence in depth shared with
    // the CPU reference: a zero or negative span must never turn into an
    // infinite or NaN tangent that would poison the storage buffer.
    let deltas = (0..count - 1)
        .map(|index| {
            let span = xs[index + 1] - xs[index];
            if span > 0.0 {
                (ys[index + 1] - ys[index]) / span
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let mut tangents = Vec::with_capacity(count);
    tangents.push(deltas[0]);
    for index in 1..count - 1 {
        // CC3 2.3 step 2 is normative as written.  `f32::midpoint` rounds
        // differently at the extremes, and the independent CPU reference
        // transcribes the same literal expression, so the two must not drift.
        #[allow(clippy::manual_midpoint)]
        tangents.push((deltas[index - 1] + deltas[index]) / 2.0);
    }
    tangents.push(deltas[count - 2]);
    for index in 0..count - 1 {
        let delta = deltas[index];
        if delta == 0.0 {
            tangents[index] = 0.0;
            tangents[index + 1] = 0.0;
            continue;
        }
        let a = tangents[index] / delta;
        let b = tangents[index + 1] / delta;
        if a < 0.0 {
            tangents[index] = 0.0;
        }
        if b < 0.0 {
            tangents[index + 1] = 0.0;
        }
        if a >= 0.0 && b >= 0.0 && a * a + b * b > 9.0 {
            let tau = 3.0 / (a * a + b * b).sqrt();
            tangents[index] = tau * a * delta;
            tangents[index + 1] = tau * b * delta;
        }
    }
    tangents
}

/// Report whether a layer needs the display-coded legacy compatibility branch.
///
/// `chroma_key` is deliberately absent: CC1 2.2.4 makes alpha and keying
/// independent of colour correction, and the legacy branch clamps RGB to
/// 0..1, which 2.2.5 forbids for a colour stage. Keying has its own shader
/// branch that never encodes, clamps, or decodes the colour that continues
/// down the pipeline.
///
/// `color_grade` is also absent: `Effect` deserialization canonicalises that
/// name to `primary_correction`, so no live project state can carry it.
/// Does this layer need the shader's legacy display-coded branch?
///
/// Both compatibility stages run there: the historical display-coded controls
/// and the post-primary LUTs. Core owns the classification, so the routing
/// here cannot drift from what QA, delivery conformance, and the inspector
/// report about the same effect.
fn legacy_stage_active(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| kinewright_core::effect_compatibility_stage(&effect.name).is_some())
}

#[allow(clippy::too_many_lines)]
fn params_for(effects: &[Effect], transition: TransitionRenderParams) -> LayerParams {
    let mut params = LayerParams {
        opacity: transition.alpha.clamp(0.0, 1.0),
        fade_mix: transition.fade_mix.clamp(0.0, 1.0),
        fade_white: transition.fade_white.clamp(0.0, 1.0),
        ..Default::default()
    };
    for effect in effects {
        let Some(descriptor) = effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            let value = parameter_value(effect, parameter);
            match parameter.uniform {
                EffectUniform::Brightness => params.brightness += value / 100.0,
                EffectUniform::Contrast => params.contrast *= 1.0 + value / 100.0,
                EffectUniform::Saturation => params.saturation *= 1.0 + value / 100.0,
                EffectUniform::Opacity => params.opacity *= value / 100.0,
                EffectUniform::Scale => params.scale *= value / 100.0,
                EffectUniform::OffsetX => params.offset_x += value / 50.0,
                EffectUniform::OffsetY => params.offset_y += value / 50.0,
                EffectUniform::CropLeft => params.crop_left += value / 100.0,
                EffectUniform::CropRight => params.crop_right += value / 100.0,
                EffectUniform::CropTop => params.crop_top += value / 100.0,
                EffectUniform::CropBottom => params.crop_bottom += value / 100.0,
                EffectUniform::ReframeAspect => params.reframe_aspect = value / 10_000.0,
                EffectUniform::ReframeFocusX => params.reframe_focus_x = value / 100.0,
                EffectUniform::ReframeFocusY => params.reframe_focus_y = value / 100.0,
                EffectUniform::ReframeFocusXBasisPoints => {
                    if effect.parameters.contains_key(parameter.name) {
                        params.reframe_focus_x = value / 10_000.0;
                    }
                }
                EffectUniform::ReframeFocusYBasisPoints => {
                    if effect.parameters.contains_key(parameter.name) {
                        params.reframe_focus_y = value / 10_000.0;
                    }
                }
                // `color_grade` is canonicalised to `primary_correction`
                // before an effect enters live project state, so these three
                // display-coded uniforms are unreachable and the shader no
                // longer reads them. Their uniform slots stay in
                // `LayerParams` so the 48-float ABI is byte-identical.
                EffectUniform::Exposure
                | EffectUniform::Temperature
                | EffectUniform::Tint
                // CC1 primary nodes are serialized separately and executed
                // in order by the shader's storage-buffer loop. They must
                // never be flattened into the legacy display controls.
                | EffectUniform::PrimaryExposure
                | EffectUniform::PrimaryTemperature
                | EffectUniform::PrimaryTint
                | EffectUniform::PrimaryContrast
                | EffectUniform::PrimaryPivot
                | EffectUniform::Blacks
                | EffectUniform::Shadows
                | EffectUniform::Highlights
                | EffectUniform::Whites
                | EffectUniform::PrimarySaturation
                | EffectUniform::AudioGain
                | EffectUniform::EqLowGain
                | EffectUniform::EqMidGain
                | EffectUniform::EqHighGain
                | EffectUniform::CompressorThreshold
                | EffectUniform::CompressorRatio
                | EffectUniform::CompressorAttack
                | EffectUniform::CompressorRelease
                | EffectUniform::CompressorMakeup
                | EffectUniform::LimiterCeiling
                | EffectUniform::DuckThreshold
                | EffectUniform::DuckReduction
                | EffectUniform::DuckAttack
                | EffectUniform::DuckRelease
                | EffectUniform::ColorNode => {}
                EffectUniform::LutPreset => params.lut_preset = value,
                EffectUniform::LutIntensity => params.lut_intensity = value / 100.0,
                EffectUniform::ExternalLutIntensity => {
                    params.external_lut_intensity = value / 100.0;
                }
                EffectUniform::MaskShape => params.mask_shape = value,
                EffectUniform::MaskCenterX => params.mask_center_x = value / 100.0,
                EffectUniform::MaskCenterY => params.mask_center_y = value / 100.0,
                EffectUniform::MaskWidth => params.mask_width = value / 100.0,
                EffectUniform::MaskHeight => params.mask_height = value / 100.0,
                EffectUniform::MaskFeather => params.mask_feather = value / 100.0,
                EffectUniform::MaskInvert => params.mask_invert = value,
                EffectUniform::KeyRed => params.key_red = value / 255.0,
                EffectUniform::KeyGreen => params.key_green = value / 255.0,
                EffectUniform::KeyBlue => params.key_blue = value / 255.0,
                EffectUniform::KeyThreshold => params.key_threshold = value / 100.0,
                EffectUniform::KeySoftness => params.key_softness = value / 200.0,
                EffectUniform::KeySpill => params.key_spill = value / 100.0,
            }
        }
    }
    params.crop_left = params.crop_left.clamp(0.0, 0.45);
    params.crop_right = params.crop_right.clamp(0.0, 0.45);
    params.crop_top = params.crop_top.clamp(0.0, 0.45);
    params.crop_bottom = params.crop_bottom.clamp(0.0, 0.45);
    params.reframe_focus_x = params.reframe_focus_x.clamp(0.0, 1.0);
    params.reframe_focus_y = params.reframe_focus_y.clamp(0.0, 1.0);
    params.lut_intensity = params.lut_intensity.clamp(0.0, 1.0);
    params.external_lut_intensity = params.external_lut_intensity.clamp(0.0, 1.0);
    params.mask_center_x = params.mask_center_x.clamp(0.0, 1.0);
    params.mask_center_y = params.mask_center_y.clamp(0.0, 1.0);
    params.mask_width = params.mask_width.clamp(0.01, 2.0);
    params.mask_height = params.mask_height.clamp(0.01, 2.0);
    params.mask_feather = params.mask_feather.clamp(0.0, 1.0);
    params
}

// Every uniform-carrying parameter is bounded to a small integer that is
// exactly representable as f32.
//
// CC4 3.3 adds the one exception, and it is deliberately harmless:
// `lut_asset_id` is bounded to `2^53 - 1`, which is exact in `i64` but not in
// `f32`. It uses `EffectUniform::ColorNode`, so `params_for`'s match arm
// discards the value without ever materializing it into `LayerParams`; the
// asset reference reaches the GPU through the node record's atlas slot index,
// never as a float. The lossy cast here is therefore computed and thrown away,
// and no rendered value depends on it.
#[allow(clippy::cast_precision_loss)]
fn parameter_value(effect: &Effect, descriptor: &EffectParameterDescriptor) -> f32 {
    match effect.parameters.get(descriptor.name) {
        Some(ParamValue::Integer(value)) => *value as f32,
        _ => descriptor.neutral as f32,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use std::fmt::Write as _;

    use kinewright_core::{
        EFFECT_DESCRIPTORS, EffectId, LUT_NODE_LIMIT_PER_LAYER, LutAssetId, LutAvailabilityKind,
        ParamValue, Title,
    };

    use super::*;
    use crate::{
        cc1_fixtures::LINEAR_CPU_GPU_MAX, gpu_test_support::fixture_gpu_or_skip,
        lut::MAX_CUBE_SIZE, lut_store::LutStore, test_support::TempDirectory,
    };

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> FrameTexture {
        FrameTexture {
            width,
            height,
            rgba: Arc::new(
                std::iter::repeat_n(rgba, usize::try_from(width * height).unwrap())
                    .flatten()
                    .collect(),
            ),
        }
    }

    /// Prefer the deterministic software fallback adapter (WARP on CI); use a
    /// real adapter when the operator opts in (developer machines). The pixel
    /// assertions carry tolerances, so hardware adapters remain valid test
    /// targets. Missing every adapter fails loudly unless skipping was
    /// explicitly permitted; see [`fixture_gpu_or_skip`].
    fn fallback() -> Option<Compositor> {
        Some(Compositor::new(fixture_gpu_or_skip()?))
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn compositor_limit_contract_requires_its_fragment_storage_buffer() {
        let limits = compositor_required_limits(wgpu::Limits::downlevel_webgl2_defaults());
        assert_eq!(
            limits.max_storage_buffers_per_shader_stage,
            COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE
        );
        // CC3 3.2 keeps exactly one fragment-stage storage binding and raises
        // the binding size instead, because a second storage binding is not
        // available on every supported downlevel backend.
        assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, 1);
        assert_eq!(
            limits.max_storage_buffer_binding_size,
            COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
        );
        // CC5 3.1: sixteen curve-plus-matte nodes no longer fit 16 KiB, so the
        // binding SIZE doubles while the binding COUNT stays 1.
        assert_eq!(COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE, 32_768);

        // CC4 4.1/10.3.8. The LUT atlas is one depth-packed 3D texture on the
        // binding that already existed, so the storage constants above are
        // asserted UNCHANGED and only the 3D dimension is raised.
        assert_eq!(COMPOSITOR_LUT_SLOTS_PER_LAYER, LUT_NODE_LIMIT_PER_LAYER);
        assert_eq!(COMPOSITOR_LEGACY_LUT_SLOT, COMPOSITOR_LUT_SLOTS_PER_LAYER);
        assert_eq!(
            COMPOSITOR_LUT_ATLAS_SLOTS,
            COMPOSITOR_LUT_SLOTS_PER_LAYER + 1
        );
        assert_eq!(COMPOSITOR_LUT_ATLAS_SLOTS, 5);
        let worst_case_depth = COMPOSITOR_LUT_ATLAS_SLOTS as u32 * MAX_CUBE_SIZE;
        assert_eq!(worst_case_depth, 325);
        assert!(worst_case_depth <= COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D);
        assert_eq!(COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D, 512);
        // The raise is load-bearing, not decorative: the downlevel profile the
        // rest of this test negotiates cannot hold the worst-case atlas.
        assert!(
            wgpu::Limits::downlevel_webgl2_defaults().max_texture_dimension_3d < worst_case_depth
        );
        assert!(wgpu::Limits::downlevel_defaults().max_texture_dimension_3d < worst_case_depth);
        assert_eq!(
            limits.max_texture_dimension_3d,
            COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D
        );
        // Production negotiates the default profile, which already clears it,
        // so no production adapter changes behaviour.
        assert!(
            wgpu::Limits::default().max_texture_dimension_3d
                >= COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D
        );
        // CC4 4.2 took the ABI to 2 (kinds whose meaning depends on a
        // companion texture binding); CC5 3.1 takes it to 3, because a CC4
        // consumer would read `v11` as a reserved zero and silently render an
        // unmasked correction over the whole raster.
        assert_eq!(GRADE_ABI_VERSION, 3);
    }

    #[test]
    fn every_color_node_kind_has_a_shader_branch() {
        // CC4 4.2: `apply_color_nodes` treats an unrecognized kind as the
        // identity, so a host that writes a kind the shader does not dispatch
        // would silently drop the node. Assert the dispatch exists for every
        // tag, against the very source the pipeline compiles.
        for kind in ColorNodeKind::ALL {
            let tag = kind.storage_buffer_tag();
            assert!(
                COMPOSITOR_SHADER_SOURCE.contains(&format!("kind == {tag}u")),
                "compositor.wgsl has no dispatch for {kind:?} (storage tag {tag})"
            );
        }
        // Tags 1..=5 are exactly the tags in use; nothing else is dispatched.
        let tags = ColorNodeKind::ALL.map(ColorNodeKind::storage_buffer_tag);
        assert_eq!(tags.len(), 5);
        for tag in 1..=5_u32 {
            assert!(tags.contains(&tag), "storage tag {tag} is unassigned");
        }
        assert!(!COMPOSITOR_SHADER_SOURCE.contains("kind == 6u"));
        // The atlas is read with `textureLoad` only; hardware filtering is
        // forbidden (CC4 3.5), and the sampler at binding 1 is never used
        // for it.
        assert!(!COMPOSITOR_SHADER_SOURCE.contains("textureSample(lut_texture"));
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::float_cmp)]
    fn grade_buffer_worst_case_fits_the_negotiated_binding_size() {
        // CC5 3.1's worst case, written out by hand: sixteen curve nodes that
        // each carry a matte.
        //   16 header
        // + 16 * 64                = 1024   node records
        // + 16 * (4 * 49 * 4)      = 12544  curve payloads
        // + 16 * (64 * 4)          = 4096   matte blocks
        //                          = 17680
        assert_eq!(16 + 16 * 64 + 16 * (4 * 49 * 4) + 16 * (64 * 4), 17_680);
        assert_eq!(GRADE_BUFFER_WORST_CASE_BYTES, 17_680);
        assert_eq!(MATTE_BLOCK_WORDS, 64);
        assert_eq!(MATTE_WINDOW_WORDS, 12);
        assert_eq!(MATTE_WINDOW_BASE_WORD, 16);
        assert!(
            GRADE_BUFFER_WORST_CASE_BYTES as u64 <= COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
        );
        let matte_free = (0..COLOR_NODE_LIMIT_PER_LAYER)
            .map(|index| {
                curves(
                    index as u64 + 1,
                    "master",
                    &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
                )
            })
            .collect::<Vec<_>>();
        // CC3's worst case is still exactly what it was: the matte block is
        // additional, never a widening of the record or of the payload.
        let matte_free_bytes =
            grade_buffer_bytes(&matte_free).expect("sixteen curve nodes fit the buffer");
        assert_eq!(matte_free_bytes.len(), 13_584);

        let stack = matte_free
            .iter()
            .cloned()
            .map(|effect| with_matte(effect, &[("matte_window_count", 1)]))
            .collect::<Vec<_>>();
        let bytes = grade_buffer_bytes_for(&stack, None, (64, 36), None)
            .expect("sixteen curve-plus-matte nodes fit the buffer");
        assert_eq!(bytes.len(), GRADE_BUFFER_WORST_CASE_BYTES);
        assert_eq!(grade_header(&bytes, 0), 16);
        assert_eq!(grade_header(&bytes, 1), 256);
        assert_eq!(grade_header(&bytes, 2), 3);
        assert_eq!(grade_header(&bytes, 3), 0);
        // Every node points at its own 196-word curve payload followed by its
        // own 64-word matte block, and no two regions overlap.
        let mut regions: Vec<(usize, usize)> = Vec::new();
        for index in 0..COLOR_NODE_LIMIT_PER_LAYER {
            let base = index * GRADE_NODE_WORDS;
            assert!((grade_word(&bytes, base) - 3.0).abs() < 1e-6);
            let expected_payload = 256 + index * (GRADE_CURVE_PAYLOAD_WORDS + MATTE_BLOCK_WORDS);
            let payload = grade_word(&bytes, base + 1);
            let matte = grade_word(&bytes, base + GRADE_NODE_MATTE_OFFSET_WORD);
            assert!(
                (payload - expected_payload as f32).abs() < 1e-6,
                "node {index} payload offset"
            );
            assert!(
                (matte - (expected_payload + GRADE_CURVE_PAYLOAD_WORDS) as f32).abs() < 1e-6,
                "node {index} matte offset"
            );
            regions.push((
                expected_payload,
                expected_payload + GRADE_CURVE_PAYLOAD_WORDS,
            ));
            regions.push((
                expected_payload + GRADE_CURVE_PAYLOAD_WORDS,
                expected_payload + GRADE_CURVE_PAYLOAD_WORDS + MATTE_BLOCK_WORDS,
            ));
        }
        regions.sort_unstable();
        for pair in regions.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "payload regions overlap: {pair:?}");
        }
        let (_, last_end) = *regions.last().expect("sixteen nodes wrote regions");
        assert_eq!(
            GRADE_HEADER_BYTES + last_end * 4,
            GRADE_BUFFER_WORST_CASE_BYTES
        );
    }

    #[test]
    fn grade_buffer_rejects_more_than_sixteen_managed_nodes() {
        let stack = (0..=COLOR_NODE_LIMIT_PER_LAYER)
            .map(|index| {
                effect_with(
                    index as u64 + 1,
                    "primary_correction",
                    &[("exposure_milli_stops", 100)],
                )
            })
            .collect::<Vec<_>>();
        let error = grade_buffer_bytes(&stack).expect_err("seventeen nodes are rejected");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("too_many_color_nodes:"),
            "unexpected message: {message}"
        );
        assert!(message.contains("17"), "unexpected message: {message}");
        assert!(message.contains("16"), "unexpected message: {message}");
        // A bypassed node still occupies one of the sixteen slots, so the
        // count is of managed nodes, not of active ones.
        let mut bypassed = stack.clone();
        for effect in &mut bypassed {
            effect.name = "color_wheels".to_owned();
            effect.parameters.clear();
            effect
                .parameters
                .insert("bypass".to_owned(), ParamValue::Integer(1));
        }
        assert!(grade_buffer_bytes(&bypassed).is_err());
        assert!(grade_buffer_bytes(&stack[..COLOR_NODE_LIMIT_PER_LAYER]).is_ok());
    }

    #[test]
    fn inactive_color_nodes_are_never_written_to_the_grade_buffer() {
        // CC3 3.3: a neutral node, a structurally identical curve node, and a
        // bypassed non-neutral node of each kind are all the exact identity.
        let stack = [
            wheels(1, &[]),
            curves(2, "master", &[(0, 0), (10_000, 10_000)]),
            wheels(3, &[("gain_red_thousandths", 1_200), ("bypass", 1)]),
            curves(4, "master", &[(0, 0), (5_000, 6_000), (10_000, 10_000)]),
        ];
        let mut bypassed_curves = stack[3].clone();
        bypassed_curves
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        let stack = [
            stack[0].clone(),
            stack[1].clone(),
            stack[2].clone(),
            bypassed_curves,
        ];
        let bytes = grade_buffer_bytes(&stack).expect("inactive nodes serialize");
        assert_eq!(grade_header(&bytes, 0), 0);
        assert_eq!(grade_header(&bytes, 1), 0);
        // One zeroed record keeps the runtime-sized storage binding valid.
        assert_eq!(bytes.len(), GRADE_HEADER_BYTES + GRADE_NODE_BYTES);
        assert!(bytes[GRADE_HEADER_BYTES..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn inactive_color_nodes_render_bit_identically_to_an_empty_stack() {
        let Some(compositor) = fallback() else {
            return;
        };
        let frame = linear_frame([0.18, 0.18, 0.18]);
        let primary = effect_with(1, "primary_correction", &[("exposure_milli_stops", 1_000)]);
        let baseline = render_linear(&compositor, &frame, std::slice::from_ref(&primary));
        let mut bypassed_curves = curves(5, "master", &[(0, 0), (5_000, 6_000), (10_000, 10_000)]);
        bypassed_curves
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        let padded = render_linear(
            &compositor,
            &frame,
            &[
                primary,
                wheels(2, &[]),
                curves(3, "master", &[(0, 0), (10_000, 10_000)]),
                wheels(4, &[("gain_red_thousandths", 1_200), ("bypass", 1)]),
                bypassed_curves,
            ],
        );
        assert_eq!(baseline.len(), padded.len());
        for (index, (expected, actual)) in baseline.iter().zip(&padded).enumerate() {
            assert_eq!(
                expected.to_bits(),
                actual.to_bits(),
                "channel {index}: {expected} != {actual}"
            );
        }
    }

    #[test]
    fn primary_only_stack_keeps_its_cc1_analytic_anchor() {
        let Some(compositor) = fallback() else {
            return;
        };
        // CC1: a one-stop exposure with every other control neutral is a pure
        // linear-light doubling, so the anchor is written out rather than
        // captured.  `f16` storage makes the input exactly 0.17993164.
        let frame = linear_frame([0.18, 0.18, 0.18]);
        let expected = f32::from(f16::from_f32(0.18)) * 2.0;
        let linear = render_linear(
            &compositor,
            &frame,
            &[effect_with(
                1,
                "primary_correction",
                &[("exposure_milli_stops", 1_000)],
            )],
        );
        for (channel, value) in linear.iter().take(3).enumerate() {
            assert!(
                (value - expected).abs() < 1.5e-3,
                "primary channel {channel} is {value} not {expected}"
            );
        }
    }

    #[test]
    fn wheels_node_matches_the_cc3_gain_anchor() {
        let Some(compositor) = fallback() else {
            return;
        };
        // CC3 2.1: 0.18 linear with `gain_red_thousandths = 1200` and every
        // other control neutral resolves to 0.250771 on red.  Green and blue
        // take the identity `slope = 1, offset = 0, power = 1` path, which is
        // an exact `grade709` round trip.
        let frame = linear_frame([0.18, 0.18, 0.18]);
        let linear = render_linear(
            &compositor,
            &frame,
            &[wheels(1, &[("gain_red_thousandths", 1_200)])],
        );
        assert!(
            (linear[0] - 0.250_771).abs() < 1.5e-3,
            "red is {} not 0.250771",
            linear[0]
        );
        for (channel, value) in linear.iter().take(3).enumerate().skip(1) {
            assert!(
                (value - 0.18).abs() < 1.5e-3,
                "channel {channel} is {value} not 0.18"
            );
        }
    }

    #[test]
    fn curves_node_matches_the_cc3_master_anchor() {
        let Some(compositor) = fallback() else {
            return;
        };
        // CC3 2.1: 0.18 linear through the master curve
        // (0,0) (5000,6000) (10000,10000) resolves to 0.262441 on every
        // channel, because the master curve is applied identically per
        // channel and the untouched red/green/blue curves are the identity.
        let frame = linear_frame([0.18, 0.18, 0.18]);
        let linear = render_linear(
            &compositor,
            &frame,
            &[curves(
                1,
                "master",
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
            )],
        );
        for (channel, value) in linear.iter().take(3).enumerate() {
            assert!(
                (value - 0.262_441).abs() < 1.5e-3,
                "channel {channel} is {value} not 0.262441"
            );
        }
    }

    #[test]
    fn wheels_and_curves_are_order_dependent() {
        let Some(compositor) = fallback() else {
            return;
        };
        // CC3 3.1: there is no fixed inter-kind precedence, so the two orders
        // are both correct and must differ.  Written out: slope acts on the
        // `grade709` value, so `curve(1.2 * e)` and `1.2 * curve(e)` differ
        // wherever the curve is not linear.
        let frame = linear_frame([0.18, 0.18, 0.18]);
        let wheels_node = wheels(1, &[("gain_red_thousandths", 1_200)]);
        let curves_node = curves(2, "master", &[(0, 0), (5_000, 6_000), (10_000, 10_000)]);
        let wheels_first = render_linear(
            &compositor,
            &frame,
            &[wheels_node.clone(), curves_node.clone()],
        );
        let curves_first = render_linear(&compositor, &frame, &[curves_node, wheels_node]);
        assert!(
            (wheels_first[0] - curves_first[0]).abs() > 1e-2,
            "orders should differ: {} vs {}",
            wheels_first[0],
            curves_first[0]
        );
        // Green is untouched by the red-only slope, so both orders agree there.
        assert!((wheels_first[1] - curves_first[1]).abs() < 1.5e-3);
    }

    #[test]
    fn fritsch_carlson_limits_a_steep_tangent() {
        // Points (0,0) (1,0.1) (2,10.1): delta = [0.1, 10], so the initial
        // tangents are m = [0.1, 5.05, 10].  At i = 0, a = 1 and b = 50.5, so
        // a^2 + b^2 = 2551.25 > 9 and tau = 3 / sqrt(2551.25) = 0.05939417.
        // m0 becomes 0.3 / sqrt(2551.25) = 0.00593942 and m1 becomes
        // 15.15 / sqrt(2551.25) = 0.29994086.  At i = 1 the rewritten m1 gives
        // a = 0.02999409 and b = 1, whose square sum is 1.0009, so nothing
        // else is limited.
        let tangents = fritsch_carlson_tangents(&[0.0, 1.0, 2.0], &[0.0, 0.1, 10.1]);
        assert!((tangents[0] - 0.005_939_42).abs() < 1e-7, "{tangents:?}");
        assert!((tangents[1] - 0.299_940_86).abs() < 1e-6, "{tangents:?}");
        assert!((tangents[2] - 10.0).abs() < 1e-6, "{tangents:?}");
    }

    #[test]
    fn fritsch_carlson_flattens_a_plateau_in_forward_order() {
        // Points (0,0) (1,1) (2,1) (3,2): delta = [1, 0, 1] and the initial
        // tangents are m = [1, 0.5, 0.5, 1].  i = 0 leaves them alone
        // (a = 1, b = 0.5).  i = 1 has a zero delta, so m1 and m2 both become
        // 0.  i = 2 then reads the rewritten m2 = 0, giving a = 0 and b = 1,
        // which needs no limiting.  Visiting the segments in any other order
        // would not zero the plateau's shared endpoints the same way.
        let tangents = fritsch_carlson_tangents(&[0.0, 1.0, 2.0, 3.0], &[0.0, 1.0, 1.0, 2.0]);
        assert_eq!(tangents, vec![1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn fritsch_carlson_reproduces_the_identity_curve_exactly() {
        assert_eq!(
            fritsch_carlson_tangents(&[0.0, 1.0], &[0.0, 1.0]),
            vec![1.0, 1.0]
        );
        // A descending y between two ascending segments has its sign-crossing
        // tangents zeroed, which is what keeps a monotone point sequence
        // monotone.
        let tangents = fritsch_carlson_tangents(&[0.0, 1.0, 2.0], &[0.0, 1.0, 0.5]);
        assert!((tangents[0] - 1.0).abs() < 1e-6, "{tangents:?}");
        assert!(tangents[1].abs() < 1e-6, "{tangents:?}");
        assert!((tangents[2] + 0.5).abs() < 1e-6, "{tangents:?}");
    }

    #[test]
    fn headless_context_preserves_adapter_provenance() {
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let metadata = gpu.monitor_proof_metadata();
        assert_ne!(metadata.backend, "unknown");
        assert_ne!(metadata.adapter, "unknown");
        assert!(!metadata.gpu_claim || !metadata.software_fallback);
    }

    fn assert_pixel_close(actual: &[u8], expected: [u8; 4], tolerance: u8) {
        for (channel, expected) in actual.iter().zip(expected) {
            assert!(
                channel.abs_diff(expected) <= tolerance,
                "pixel {actual:?} differs from expected {expected:?}"
            );
        }
    }

    fn effect(id: u64, name: &str, parameter: &str, value: i64) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: BTreeMap::from([(parameter.to_owned(), ParamValue::Integer(value))]),
            keyframes: BTreeMap::new(),
        }
    }

    fn effect_with(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                .collect(),
            keyframes: BTreeMap::new(),
        }
    }

    fn grade_word(bytes: &[u8], word: usize) -> f32 {
        let offset = GRADE_HEADER_BYTES + word * 4;
        let end = offset.saturating_add(std::mem::size_of::<f32>());
        f32::from_le_bytes(bytes[offset..end].try_into().expect("f32-aligned bytes"))
    }

    fn grade_header(bytes: &[u8], index: usize) -> u32 {
        let offset = index * 4;
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("header word"))
    }

    fn wheels(id: u64, parameters: &[(&str, i64)]) -> Effect {
        effect_with(id, "color_wheels", parameters)
    }

    /// Turn the master switch on and merge the supplied `matte_*` overrides
    /// into an existing colour node, leaving every other parameter alone.
    fn with_matte(mut effect: Effect, parameters: &[(&str, i64)]) -> Effect {
        effect
            .parameters
            .insert("matte_enabled".to_owned(), ParamValue::Integer(1));
        for (name, value) in parameters {
            effect
                .parameters
                .insert((*name).to_owned(), ParamValue::Integer(*value));
        }
        effect
    }

    /// The word index of node `index`'s matte block, read from its own `v11`.
    fn matte_base(bytes: &[u8], index: usize) -> usize {
        let word = grade_word(
            bytes,
            index * GRADE_NODE_WORDS + GRADE_NODE_MATTE_OFFSET_WORD,
        );
        assert!(word >= 0.0, "a matte offset is never negative");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let base = word.round() as usize;
        base
    }

    /// One word of node `index`'s matte block.
    fn matte_word(bytes: &[u8], index: usize, word: usize) -> f32 {
        grade_word(bytes, matte_base(bytes, index) + word)
    }

    /// The CC5 9.1 containment raster: 64 x 36, `a = 16/9`, every channel in
    /// `[0.05, 0.95]` and strictly varying, with no channel ever exactly 0 —
    /// a zero would let "exactly 0 outside changed" pass for the wrong reason
    /// under a gain node.
    #[allow(clippy::cast_precision_loss)]
    fn cc5_field_raster() -> WorkingFrame {
        let (width, height) = (64_u32, 36_u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let fx = x as f32 / 63.0;
                let fy = y as f32 / 35.0;
                pixels.push(f16::from_f32(0.05 + 0.9 * fx));
                pixels.push(f16::from_f32(0.05 + 0.9 * fy));
                pixels.push(f16::from_f32(0.05 + 0.45 * (fx + fy)));
                pixels.push(f16::from_f32(1.0));
            }
        }
        WorkingFrame {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    /// A uniform scene-linear frame of an arbitrary shape.
    fn uniform_frame(width: u32, height: u32, rgb: [f32; 3]) -> WorkingFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend(rgb.map(f16::from_f32));
            pixels.push(f16::from_f32(1.0));
        }
        WorkingFrame {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    /// CC3 2.1's `grade709` decode, transcribed here in f64 rather than
    /// called, so a qualifier fixture's input is its own statement.
    #[allow(clippy::cast_possible_truncation)]
    fn cc5_grade709_decode(v: f64) -> f32 {
        let (s, a) = (v.signum(), v.abs());
        if a < 0.081_242_86 {
            (s * a / 4.5) as f32
        } else {
            (s * ((a + 0.099_296_8) / 1.099_296_8).powf(2.222_222_3)) as f32
        }
    }

    /// Render one layer's matte coverage through the production path.
    fn render_coverage(
        compositor: &Compositor,
        frame: &WorkingFrame,
        effects: &[Effect],
        effect_id: u64,
    ) -> Result<Vec<u8>, MediaError> {
        compositor.render_matte(
            (frame.width, frame.height),
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
            }],
            None,
            MatteRenderTarget {
                layer_index: 0,
                clip: ClipId(1),
                effect: EffectId(effect_id),
            },
        )
    }

    /// A `color_curves` effect with one channel's point list expanded into the
    /// integer parameters CC3 4.2 defines.
    fn curves(id: u64, channel: &str, points: &[(i64, i64)]) -> Effect {
        let count = i64::try_from(points.len()).expect("curve point count");
        let mut parameters = vec![(format!("{channel}_point_count"), count)];
        for (index, (x, y)) in points.iter().enumerate() {
            parameters.push((format!("{channel}_x{index}"), *x));
            parameters.push((format!("{channel}_y{index}"), *y));
        }
        Effect {
            id: EffectId(id),
            name: "color_curves".to_owned(),
            parameters: parameters
                .into_iter()
                .map(|(name, value)| (name, ParamValue::Integer(value)))
                .collect(),
            keyframes: BTreeMap::new(),
        }
    }

    /// A solid scene-linear working frame, so a rendered value can be compared
    /// against a hand-derived CC3 anchor without a transfer round trip on the
    /// input side.  `f16` storage is the normative working surface, so the
    /// anchors below are evaluated at the quantized input, not at the literal.
    fn linear_frame(rgb: [f32; 3]) -> WorkingFrame {
        let mut pixels = Vec::with_capacity(4 * 4 * 4);
        for _ in 0..16 {
            pixels.extend(rgb.map(f16::from_f32));
            pixels.push(f16::from_f32(1.0));
        }
        WorkingFrame {
            width: 4,
            height: 4,
            pixels: Arc::new(pixels),
        }
    }

    fn render_linear(
        compositor: &Compositor,
        frame: &WorkingFrame,
        effects: &[Effect],
    ) -> Vec<f32> {
        compositor
            .render_working(
                (frame.width, frame.height),
                &[CompositorLayer {
                    frame,
                    effects,
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("production GPU working-surface readback")
    }

    fn crop(id: u64, left: i64, right: i64, top: i64, bottom: i64) -> Effect {
        Effect {
            id: EffectId(id),
            name: "crop".to_owned(),
            parameters: BTreeMap::from([
                ("left_percent".to_owned(), ParamValue::Integer(left)),
                ("right_percent".to_owned(), ParamValue::Integer(right)),
                ("top_percent".to_owned(), ParamValue::Integer(top)),
                ("bottom_percent".to_owned(), ParamValue::Integer(bottom)),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    fn reframe(id: u64, aspect_basis_points: i64, focus_x: i64, focus_y: i64) -> Effect {
        Effect {
            id: EffectId(id),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([
                (
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(aspect_basis_points),
                ),
                ("focus_x_percent".to_owned(), ParamValue::Integer(focus_x)),
                ("focus_y_percent".to_owned(), ParamValue::Integer(focus_y)),
            ]),
            keyframes: BTreeMap::new(),
        }
    }

    fn pixel(frame: &FrameTexture, x: u32, y: u32) -> &[u8] {
        let index = usize::try_from((y * frame.width + x) * 4).unwrap();
        &frame.rgba[index..index + 4]
    }

    /// A 1:1 layer must read every source texel exactly, on every adapter.
    ///
    /// The full-screen quad maps output pixel centre `(x + 0.5, y + 0.5)` onto
    /// source texel centre `(x + 0.5, y + 0.5)`, so bilinear filtering is the
    /// identity in exact arithmetic, but not in a driver's arithmetic. Mesa
    /// lavapipe reconstructs the sub-texel coordinate in f32 and lands one ULP
    /// of the *texel* coordinate off zero, mixing `2^-15`--`2^-14` of the
    /// neighbouring texel into the sample at block edges. NVIDIA does not.
    ///
    /// That leak is invisible through a well-conditioned node and fatal
    /// through an ill-conditioned one: `color_wheels` with `power = 0.1` has an
    /// unbounded derivative at `y = 0`, and it turned a `3.05e-5` leak into a
    /// `0.18` linear difference, 105 monitor codes, on the CC3 §10.2 raster.
    /// The raster below is deliberately the shape that exposed it: 8-pixel
    /// blocks alternating with black across a wide frame, so a return to
    /// bilinear 1:1 sampling fails here instead of inside a colour fixture.
    #[test]
    fn a_one_to_one_layer_samples_every_source_texel_exactly() {
        let Some(context) = crate::gpu_test_support::fixture_gpu_or_skip() else {
            return;
        };
        let compositor = Compositor::new(context);
        let width = 1_536_u32;
        let height = 2_u32;
        let block = 8_u32;
        // Every block is adjacent to a block that differs sharply in all three
        // channels, so any sub-texel leak has something to leak. The zero
        // channels are the sensitive detector: `Rgba16Float` resolves a leak
        // of `3e-5` into an exact zero as a subnormal, not as another zero.
        let texel = |x: u32| -> [f32; 3] {
            if (x / block).is_multiple_of(2) {
                [0.0, 0.0, 0.0]
            } else {
                [0.5, -0.25, 2.5]
            }
        };
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for index in 0..width * height {
            pixels.extend(texel(index % width).map(f16::from_f32));
            pixels.push(f16::from_f32(1.0));
        }
        let frame = WorkingFrame {
            width,
            height,
            pixels: Arc::new(pixels),
        };
        let rendered = render_linear(&compositor, &frame, &[]);
        let mismatches = rendered
            .as_chunks::<4>()
            .0
            .iter()
            .zip(frame.pixels.as_chunks::<4>().0.iter())
            .enumerate()
            .filter(|(_, (rendered, stored))| {
                (0..3).any(|channel| {
                    // Bit-exact on purpose: the whole point is that a 1:1
                    // composite reproduces the source texel, and a tolerance
                    // here would readmit the sub-texel leak this guards.
                    rendered[channel].to_bits() != stored[channel].to_f32().to_bits()
                })
            })
            .map(|(index, (rendered, stored))| {
                let position = u32::try_from(index).unwrap_or(u32::MAX);
                format!(
                    "pixel {index} (x={}, y={}): stored {:?}, sampled {:?}",
                    position % width,
                    position / width,
                    stored[..3].iter().map(|v| v.to_f32()).collect::<Vec<_>>(),
                    &rendered[..3],
                )
            })
            .collect::<Vec<_>>();
        assert!(
            mismatches.is_empty(),
            "a 1:1 composite must sample each texel exactly; {} of {} pixels differ. First: {}",
            mismatches.len(),
            width * height,
            mismatches[0],
        );
    }

    #[test]
    fn grade_buffer_matches_wgsl_header_and_node_record_stride() {
        let first = effect_with(
            1,
            "primary_correction",
            &[
                ("exposure_milli_stops", 1_000),
                ("temperature_percent", 25),
                ("tint_percent", -10),
                ("contrast_percent", -20),
                ("contrast_pivot_basis_points", 4_200),
                ("blacks_percent", 30),
                ("shadows_percent", -40),
                ("highlights_percent", 50),
                ("whites_percent", -60),
                ("saturation_percent", 70),
            ],
        );
        let second = effect_with(
            2,
            "primary_correction",
            &[
                ("exposure_milli_stops", -1_000),
                ("saturation_percent", -70),
            ],
        );
        let bytes = grade_buffer_bytes(&[first, second]).expect("valid primary nodes");

        assert_eq!(bytes.len(), GRADE_HEADER_BYTES + 2 * GRADE_NODE_BYTES);
        assert_eq!(grade_header(&bytes, 0), 2);
        // No curve node, so the payload region begins one word past the last
        // record and is never dereferenced.
        assert_eq!(grade_header(&bytes, 1), 32);
        assert_eq!(grade_header(&bytes, 2), GRADE_ABI_VERSION);
        assert_eq!(grade_header(&bytes, 3), 0);

        // CC1 regression: `v0 .. v9` are byte-for-byte what the pre-CC3
        // serializer wrote, so a primary-only stack renders unchanged.
        let first_values = [
            1.0, 0.25, -0.1, -0.2, 0.42, 0.3, -0.4, 0.5, -0.6, 0.7, 0.0, 0.0,
        ];
        let second_values = [-1.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, -0.7, 0.0, 0.0];
        for (record, expected) in [first_values, second_values].into_iter().enumerate() {
            let base = record * GRADE_NODE_WORDS;
            assert!((grade_word(&bytes, base) - 1.0).abs() < 1e-6, "kind tag");
            assert!(grade_word(&bytes, base + 1).abs() < 1e-6, "payload offset");
            assert!(grade_word(&bytes, base + 2).abs() < 1e-6, "bypass");
            assert!(grade_word(&bytes, base + 3).abs() < 1e-6, "reserved");
            for (index, expected) in expected.into_iter().enumerate() {
                let word = base + GRADE_NODE_VALUE_OFFSET + index;
                assert!(
                    (grade_word(&bytes, word) - expected).abs() < 1e-6,
                    "word {word} is {} not {expected}",
                    grade_word(&bytes, word)
                );
            }
        }
    }

    #[test]
    fn grade_buffer_lays_out_a_wheels_then_curves_stack_word_for_word() {
        let bytes = grade_buffer_bytes(&[
            wheels(1, &[("gain_red_thousandths", 1_200)]),
            curves(2, "master", &[(0, 0), (5_000, 6_000), (10_000, 10_000)]),
        ])
        .expect("valid two-node stack");

        // Two records plus one 196-word curve payload.
        assert_eq!(
            bytes.len(),
            GRADE_HEADER_BYTES + 2 * GRADE_NODE_BYTES + GRADE_CURVE_PAYLOAD_WORDS * 4
        );
        assert_eq!(grade_header(&bytes, 0), 2);
        assert_eq!(grade_header(&bytes, 1), 32);
        assert_eq!(grade_header(&bytes, 2), GRADE_ABI_VERSION);

        // Record 0: wheels.  `gain_red = 1200` with a neutral master is a
        // slope of 1.2 on red only; offsets are 0 and powers are 1.
        let wheels_words = [
            2.0, 0.0, 0.0, 0.0, // kind, payload offset, bypass, reserved
            1.2, 1.0, 1.0, // slope
            0.0, 0.0, 0.0, // offset
            1.0, 1.0, 1.0, // power
            0.0, 0.0, 0.0, // unused v9 .. v11
        ];
        for (word, expected) in wheels_words.into_iter().enumerate() {
            assert!(
                (grade_word(&bytes, word) - expected).abs() < 1e-6,
                "wheels word {word} is {} not {expected}",
                grade_word(&bytes, word)
            );
        }

        // Record 1: curves, pointing at the first payload word.
        assert!((grade_word(&bytes, 16) - 3.0).abs() < 1e-6);
        assert!((grade_word(&bytes, 17) - 32.0).abs() < 1e-6);
        assert!(grade_word(&bytes, 18).abs() < 1e-6);
        for word in 19..32 {
            assert!(grade_word(&bytes, word).abs() < 1e-6, "word {word}");
        }

        // Red, green, and blue are the untouched structural identity, whose
        // tangents are both 1.  Master is (0,0) (0.5,0.6) (1,1), whose
        // hand-solved tangents are 1.2, 1.0, and 0.8: no delta is zero and
        // neither `a^2 + b^2` exceeds 9, so the limiter never fires.
        let mut identity = vec![0.0; GRADE_CURVE_SLOT_WORDS];
        identity[0] = 2.0;
        identity[3] = 1.0;
        identity[4] = 1.0;
        identity[5] = 1.0;
        identity[6] = 1.0;
        let mut master = vec![0.0; GRADE_CURVE_SLOT_WORDS];
        master[0] = 3.0;
        master[3] = 1.2;
        master[4] = 0.5;
        master[5] = 0.6;
        master[6] = 1.0;
        master[7] = 1.0;
        master[8] = 1.0;
        master[9] = 0.8;
        for (slot, expected) in [&identity, &identity, &identity, &master]
            .into_iter()
            .enumerate()
        {
            let base = 32 + slot * GRADE_CURVE_SLOT_WORDS;
            for (index, expected) in expected.iter().enumerate() {
                assert!(
                    (grade_word(&bytes, base + index) - expected).abs() < 1e-6,
                    "curve slot {slot} word {index} is {} not {expected}",
                    grade_word(&bytes, base + index)
                );
            }
        }
    }

    #[test]
    fn primary_correction_nodes_execute_in_order_on_gpu() {
        let Some(compositor) = fallback() else {
            return;
        };
        let input = solid(4, 4, [64, 128, 192, 255]);
        let effects = [
            effect_with(1, "primary_correction", &[("exposure_milli_stops", 1_000)]),
            effect_with(2, "primary_correction", &[("exposure_milli_stops", -1_000)]),
        ];
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &input,
                    effects: &effects,
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("primary nodes should render");

        // The second exposure reverses the first.  A wrong 64-byte host
        // stride makes the shader read padding and half of the second node,
        // so this catches both the header and array-stride ABI errors.
        assert_pixel_close(&output.rgba[0..4], [64, 128, 192, 255], 3);
    }

    #[test]
    fn solid_color_effects_are_deterministic_on_fallback_adapter() {
        let Some(compositor) = fallback() else {
            return;
        };
        let input = solid(4, 4, [64, 128, 192, 255]);
        let brightness = effect(1, "brightness", "percent", 25);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &input,
                    effects: &[brightness],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [128, 192, 255, 255], 2);
    }

    #[test]
    fn contrast_saturation_opacity_and_transform_are_deterministic() {
        let Some(compositor) = fallback() else {
            return;
        };

        let contrast_input = solid(4, 4, [96, 128, 160, 255]);
        let contrast = effect(1, "contrast", "percent", 100);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &contrast_input,
                    effects: &[contrast],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [64, 128, 192, 255], 2);

        let saturated_input = solid(4, 4, [64, 128, 192, 255]);
        let saturation = effect(2, "saturation", "percent", -100);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &saturated_input,
                    effects: &[saturation],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [119, 119, 119, 255], 2);

        let red = solid(4, 4, [255, 0, 0, 255]);
        let opacity = effect(3, "opacity", "percent", 50);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[opacity],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        // Alpha compositing is linear-light; BT.709 monitor encoding maps
        // 50% red to approximately code value 180.
        assert_pixel_close(&output.rgba[0..4], [180, 0, 0, 255], 3);

        let transform = effect(4, "transform", "scale_percent", 50);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[transform],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [0, 0, 0, 255], 2);
        let center = (2 * 4 + 2) * 4;
        assert_pixel_close(&output.rgba[center..center + 4], [255, 0, 0, 255], 2);
    }

    #[test]
    fn crop_top_uses_uv_zero_as_the_top_row() {
        let Some(compositor) = fallback() else {
            return;
        };
        let red = solid(8, 8, [255, 0, 0, 255]);
        let output = compositor
            .render(
                (8, 8),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[crop(1, 0, 0, 25, 0)],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();

        for y in 0..2 {
            assert_pixel_close(pixel(&output, 4, y), [0, 0, 0, 255], 2);
        }
        for y in 2..8 {
            assert_pixel_close(pixel(&output, 4, y), [255, 0, 0, 255], 2);
        }
    }

    #[test]
    fn reframe_cover_crop_tracks_an_explicit_horizontal_focal_point() {
        let Some(compositor) = fallback() else {
            return;
        };
        let mut pixels = Vec::new();
        for _y in 0..4 {
            for x in 0..12 {
                let color = if x < 4 {
                    [255, 0, 0, 255]
                } else if x < 8 {
                    [0, 255, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                pixels.extend_from_slice(&color);
            }
        }
        let input = FrameTexture {
            width: 12,
            height: 4,
            rgba: Arc::new(pixels),
        };
        let left = reframe(1, 10_000, 0, 50);
        let right = reframe(2, 10_000, 100, 50);
        let render = |effect: &Effect| {
            compositor
                .render(
                    (4, 4),
                    &[CompositorLayer {
                        frame: &input,
                        effects: std::slice::from_ref(effect),
                        transition: TransitionRenderParams::default(),
                    }],
                )
                .unwrap()
        };
        assert_pixel_close(pixel(&render(&left), 2, 2), [255, 0, 0, 255], 4);
        assert_pixel_close(pixel(&render(&right), 2, 2), [0, 0, 255, 255], 4);
    }

    #[test]
    fn reframe_basis_points_override_legacy_percent_without_quantizing_motion() {
        let legacy = reframe(1, 10_000, 37, 63);
        let legacy_params = params_for(
            std::slice::from_ref(&legacy),
            TransitionRenderParams::default(),
        );
        assert!((legacy_params.reframe_focus_x - 0.37).abs() < f32::EPSILON);
        assert!((legacy_params.reframe_focus_y - 0.63).abs() < f32::EPSILON);

        let precise = effect_with(
            2,
            "reframe",
            &[
                ("target_aspect_basis_points", 10_000),
                ("focus_x_percent", 37),
                ("focus_y_percent", 63),
                ("focus_x_basis_points", 3_742),
                ("focus_y_basis_points", 6_319),
            ],
        );
        let one_basis_point_right = effect_with(
            3,
            "reframe",
            &[
                ("target_aspect_basis_points", 10_000),
                ("focus_x_basis_points", 3_743),
                ("focus_y_basis_points", 6_319),
            ],
        );
        let precise_params = params_for(
            std::slice::from_ref(&precise),
            TransitionRenderParams::default(),
        );
        let moved_params = params_for(
            std::slice::from_ref(&one_basis_point_right),
            TransitionRenderParams::default(),
        );

        assert!((precise_params.reframe_focus_x - 0.3742).abs() < 1e-6);
        assert!((precise_params.reframe_focus_y - 0.6319).abs() < 1e-6);
        let movement = moved_params.reframe_focus_x - precise_params.reframe_focus_x;
        assert!(movement > 0.0);
        assert!(movement < 0.001);
    }

    #[test]
    fn crop_all_edges_keeps_only_the_center_rectangle() {
        let Some(compositor) = fallback() else {
            return;
        };
        let red = solid(8, 8, [255, 0, 0, 255]);
        let output = compositor
            .render(
                (8, 8),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[crop(1, 25, 25, 25, 25)],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();

        for y in 0..8 {
            for x in 0..8 {
                let expected = if (2..6).contains(&x) && (2..6).contains(&y) {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 255]
                };
                assert_pixel_close(pixel(&output, x, y), expected, 2);
            }
        }
    }

    #[test]
    fn crop_transparency_wins_over_mid_fade_alpha_forcing() {
        let Some(compositor) = fallback() else {
            return;
        };
        let green = solid(8, 8, [0, 255, 0, 255]);
        let blue = solid(8, 8, [0, 0, 255, 255]);
        let crop = crop(1, 0, 0, 25, 0);
        let output = compositor
            .render(
                (8, 8),
                &[
                    CompositorLayer {
                        frame: &green,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &blue,
                        effects: &[crop],
                        transition: TransitionRenderParams {
                            alpha: 1.0,
                            fade_mix: 0.5,
                            fade_white: 0.0,
                        },
                    },
                ],
            )
            .unwrap();

        assert_pixel_close(pixel(&output, 4, 0), [0, 255, 0, 255], 2);
        assert_pixel_close(pixel(&output, 4, 4), [0, 0, 180, 255], 3);
    }

    #[test]
    fn crop_then_transform_composes_on_the_transformed_quad() {
        let Some(compositor) = fallback() else {
            return;
        };
        let red = solid(8, 8, [255, 0, 0, 255]);
        let effects = [
            crop(1, 25, 0, 0, 0),
            effect(2, "transform", "scale_percent", 50),
        ];
        let output = compositor
            .render(
                (8, 8),
                &[CompositorLayer {
                    frame: &red,
                    effects: &effects,
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();

        assert_pixel_close(pixel(&output, 1, 4), [0, 0, 0, 255], 2);
        assert_pixel_close(pixel(&output, 2, 4), [0, 0, 0, 255], 2);
        assert_pixel_close(pixel(&output, 3, 4), [255, 0, 0, 255], 2);
        assert_pixel_close(pixel(&output, 5, 4), [255, 0, 0, 255], 2);
        assert_pixel_close(pixel(&output, 6, 4), [0, 0, 0, 255], 2);
    }

    #[test]
    fn duplicate_crop_effects_add_then_clamp_each_inset() {
        let effects = [crop(1, 45, 0, 0, 0), crop(2, 45, 0, 0, 0)];
        let params = params_for(&effects, TransitionRenderParams::default());
        assert!((params.crop_left - 0.45).abs() < f32::EPSILON);
        assert!(params.crop_right.abs() < f32::EPSILON);
    }

    #[test]
    fn legacy_color_grade_name_has_no_compositor_branch_of_its_own() {
        // `Effect` deserialization canonicalises `color_grade` to
        // `primary_correction`, so the compositor's legacy display-coded
        // branch must not be reachable for it and its uniforms must stay
        // neutral.
        let legacy = effect_with(1, "color_grade", &[("exposure_milli_stops", 1_000)]);
        assert!(!legacy_stage_active(std::slice::from_ref(&legacy)));
        let params = params_for(
            std::slice::from_ref(&legacy),
            TransitionRenderParams::default(),
        );
        assert!(params.exposure.abs() < f32::EPSILON);
        assert!(params.temperature.abs() < f32::EPSILON);
        assert!(params.tint.abs() < f32::EPSILON);

        let mut canonical = legacy;
        canonical.name = "primary_correction".to_owned();
        assert_eq!(
            grade_buffer_bytes(std::slice::from_ref(&canonical))
                .expect("canonical primary node")
                .len(),
            GRADE_HEADER_BYTES + GRADE_NODE_BYTES
        );
    }

    #[test]
    fn primary_exposure_and_look_lut_execute_in_the_shared_compositor() {
        let Some(compositor) = fallback() else {
            return;
        };
        let gray = solid(4, 4, [64, 64, 64, 255]);
        let exposure = effect_with(1, "primary_correction", &[("exposure_milli_stops", 1_000)]);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &gray,
                    effects: &[exposure],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        // +1 stop in linear light, not the old display-coded doubling: the
        // BT.709 monitor code for 2 * decode_bt709(64/255) is ~97.
        assert_pixel_close(pixel(&output, 0, 0), [97, 97, 97, 255], 3);

        let red = solid(4, 4, [255, 0, 0, 255]);
        let monochrome = effect_with(
            2,
            "look_lut",
            &[("preset_token", 3), ("intensity_percent", 100)],
        );
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[monochrome],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 0, 0), [54, 54, 54, 255], 3);
    }

    #[test]
    fn external_cube_lut_is_sampled_in_red_fastest_order() {
        let Some(compositor) = fallback() else {
            return;
        };
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kinewright-swap-red-blue-{}-{unique}.cube",
            std::process::id()
        ));
        fs::write(
            &path,
            "LUT_3D_SIZE 2\n\
             0 0 0\n\
             0 0 1\n\
             0 1 0\n\
             0 1 1\n\
             1 0 0\n\
             1 0 1\n\
             1 1 0\n\
             1 1 1\n",
        )
        .unwrap();
        let cube_lut = Effect {
            id: EffectId(9),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([
                (
                    "path".to_owned(),
                    ParamValue::Text(path.to_string_lossy().into_owned()),
                ),
                ("intensity_percent".to_owned(), ParamValue::Integer(100)),
            ]),
            keyframes: BTreeMap::new(),
        };
        let red = solid(4, 4, [255, 0, 0, 255]);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[cube_lut],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        let _ = fs::remove_file(path);

        assert_pixel_close(pixel(&output, 0, 0), [0, 0, 255, 255], 3);
    }

    // -----------------------------------------------------------------
    // CC4 4: LUT atlas, node records, and shader evaluation.
    // -----------------------------------------------------------------

    /// A verified [`LutLibrary`] built the way production builds one: import
    /// real `.cube` bytes into a real project store, then admit them by hash.
    ///
    /// The library deliberately has no constructor that takes samples
    /// directly — CC4 2.4's whole point is that the renderer consumes
    /// hash-verified bytes — so these fixtures go through the store. The
    /// temporary directory is dropped with the fixture.
    struct TestLuts {
        _directory: TempDirectory,
        library: LutLibrary,
    }

    impl TestLuts {
        /// Import each `.cube` text in order, allocating ids `1 ..= n`.
        #[allow(clippy::cast_possible_truncation)]
        fn build(label: &str, sources: &[String]) -> Self {
            let directory = TempDirectory::new(label);
            let store = LutStore::for_project(&directory.path("project.kinewright"))
                .expect("a temporary project path derives a store root");
            let mut assets = Vec::with_capacity(sources.len());
            for (index, text) in sources.iter().enumerate() {
                let source = directory.path(&format!("lut-{index}.cube"));
                fs::write(&source, text).expect("the fixture LUT is written");
                let import = store
                    .import_lut_asset(&source)
                    .expect("the fixture LUT imports");
                assets.push(import.into_lut_asset(LutAssetId(index as u64 + 1)));
            }
            let (library, statuses) = LutLibrary::build(&assets, Some(&store));
            for (id, status) in &statuses {
                assert_eq!(
                    status.kind,
                    LutAvailabilityKind::Verified,
                    "fixture asset {} was not verified",
                    id.0
                );
            }
            assert_eq!(library.len(), sources.len());
            Self {
                _directory: directory,
                library,
            }
        }

        fn library(&self) -> &LutLibrary {
            &self.library
        }
    }

    /// Serialize a lattice as `.cube` text. `sample` receives lattice
    /// *indices* and returns the RGB triple stored there, red-fastest.
    fn cube_text(
        size: u32,
        domain: (f32, f32),
        sample: impl Fn(u32, u32, u32) -> [f32; 3],
    ) -> String {
        let (minimum, maximum) = domain;
        let mut text = format!(
            "LUT_3D_SIZE {size}\n\
             DOMAIN_MIN {minimum:.6} {minimum:.6} {minimum:.6}\n\
             DOMAIN_MAX {maximum:.6} {maximum:.6} {maximum:.6}\n"
        );
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let [r, g, b] = sample(red, green, blue);
                    let _ = writeln!(text, "{r:.6} {g:.6} {b:.6}");
                }
            }
        }
        text
    }

    /// A lattice whose value is `map` applied to the identity coordinate, over
    /// `[0, 1]`. Every `map` used below is affine, and tetrahedral
    /// interpolation reproduces an affine function exactly on every simplex,
    /// so the expected values are the closed form (CC4 3.5).
    #[allow(clippy::cast_precision_loss)]
    fn mapped_cube(size: u32, map: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let last = (size - 1) as f32;
        cube_text(size, (0.0, 1.0), |r, g, b| {
            map([r as f32 / last, g as f32 / last, b as f32 / last])
        })
    }

    /// An identity lattice over `[0, 1]`.
    fn identity_cube(size: u32) -> String {
        mapped_cube(size, |rgb| rgb)
    }

    /// CC4 10.3.3's LUT B: `S = 2`, domain `[0, 1]`, the eight pinned lattice
    /// values. Deliberately NOT affine, so tetrahedral and trilinear disagree
    /// at the anchor and the fixture can prove which one runs.
    fn lut_b_cube() -> String {
        cube_text(2, (0.0, 1.0), |r, g, b| {
            if r == 1 && g == 1 && b == 1 {
                [1.0, 1.0, 1.0]
            } else {
                [
                    if r == 1 { 0.5 } else { 0.0 },
                    if g == 1 { 0.5 } else { 0.0 },
                    if b == 1 { 0.5 } else { 0.0 },
                ]
            }
        })
    }

    /// CC4 10.3.3's LUT D: the separable `f = (0, 0.25, 1.0)` lattice over
    /// `DOMAIN_MIN = -0.5`, `DOMAIN_MAX = 1.5`. The domain-mapping and
    /// out-of-domain anchor.
    fn lut_d_cube() -> String {
        let f = [0.0_f32, 0.25, 1.0];
        cube_text(3, (-0.5, 1.5), |r, g, b| {
            [f[r as usize], f[g as usize], f[b as usize]]
        })
    }

    /// One `technical_lut` / `creative_look` effect bound to an asset id.
    fn lut_node(id: u64, name: &str, asset: i64, parameters: &[(&str, i64)]) -> Effect {
        let mut effect = effect_with(id, name, parameters);
        effect
            .parameters
            .insert("lut_asset_id".to_owned(), ParamValue::Integer(asset));
        effect
    }

    /// A `creative_look` with an explicit encoding token and mix.
    fn creative_look(id: u64, asset: i64, encoding: i64, mix: i64) -> Effect {
        lut_node(
            id,
            "creative_look",
            asset,
            &[
                ("input_encoding_token", encoding),
                ("mix_basis_points", mix),
            ],
        )
    }

    fn render_luts(
        compositor: &Compositor,
        frame: &WorkingFrame,
        effects: &[Effect],
        luts: &TestLuts,
    ) -> Vec<f32> {
        compositor
            .render_working_with_luts(
                (frame.width, frame.height),
                &[CompositorLayer {
                    frame,
                    effects,
                    transition: TransitionRenderParams::default(),
                }],
                Some(luts.library()),
            )
            .expect("production GPU working-surface readback")
    }

    /// Every pixel of a solid readback equals `expected` within `tolerance`.
    fn assert_solid_linear(actual: &[f32], expected: [f32; 3], tolerance: f32) {
        assert_eq!(actual.len() % 4, 0);
        assert!(!actual.is_empty());
        for pixel in actual.as_chunks::<4>().0 {
            for (channel, (observed, want)) in pixel.iter().zip(&expected).enumerate() {
                assert!(
                    (observed - want).abs() <= tolerance,
                    "channel {channel}: {observed} != {want} (tolerance {tolerance})"
                );
            }
        }
    }

    /// The first pixel's linear RGB.
    fn first_rgb(values: &[f32]) -> [f32; 3] {
        [values[0], values[1], values[2]]
    }

    #[test]
    fn lut_nodes_lay_out_a_technical_then_creative_stack_word_for_word() {
        // CC4 4.2: a LUT node owns no payload region, so `payload_word_offset`
        // stays 0 and the twelve value words carry the slot, the mix, the
        // encoding, the VERIFIED domain, the edge length, and the atlas depth
        // origin.  Every word below is written out by hand from the contract.
        let luts = TestLuts::build("cc4-record-layout", &[lut_b_cube(), lut_d_cube()]);
        let stack = [
            lut_node(1, "technical_lut", 1, &[("input_encoding_token", 1)]),
            creative_look(2, 2, 1, 5_000),
        ];
        let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
            .expect("a two-node LUT stack serializes");
        assert_eq!(bytes.len(), GRADE_HEADER_BYTES + 2 * GRADE_NODE_BYTES);
        assert_eq!(grade_header(&bytes, 0), 2);
        // No payload region exists, so the offset is simply the end of the
        // records, exactly as a payload-free CC3 stack reports it.
        assert_eq!(
            grade_header(&bytes, 1),
            u32::try_from(2 * GRADE_NODE_WORDS).expect("record words fit a u32")
        );
        assert_eq!(grade_header(&bytes, 2), GRADE_ABI_VERSION);
        assert_eq!(grade_header(&bytes, 3), 0);
        let expected: [f32; 32] = [
            // technical_lut: kind 4, no payload, not bypassed, reserved
            4.0, 0.0, 0.0, 0.0, //
            // slot 0, mix pinned at 1.0, linear encoding,
            // LUT B's domain [0,1], S = 2, z_origin 0, reserved
            0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, //
            // creative_look: kind 5
            5.0, 0.0, 0.0, 0.0, //
            // slot 1, mix 0.5, linear encoding, LUT D's domain [-0.5, 1.5],
            // S = 3, z_origin 2 (immediately after LUT B's two slices)
            1.0, 0.5, 1.0, -0.5, -0.5, -0.5, 1.5, 1.5, 1.5, 3.0, 2.0, 0.0,
        ];
        for (word, want) in expected.into_iter().enumerate() {
            let observed = grade_word(&bytes, word);
            assert_eq!(
                observed.to_bits(),
                want.to_bits(),
                "word {word}: {observed} != {want}"
            );
        }
    }

    #[test]
    fn a_mixed_size_lut_stack_packs_one_atlas_and_each_node_reads_its_own_slot() {
        // CC4 10.3.8: sizes 2, 17, 33, 65 pack to a 65 x 65 x 117 atlas.  Each
        // node's lattice is a DIFFERENT affine map, and the maps do not
        // commute, so any slot confusion changes the composed result.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build(
            "cc4-mixed-slots",
            &[
                mapped_cube(2, |[r, g, b]| [r * 0.5, g * 0.5, b * 0.5]),
                mapped_cube(17, |[r, g, b]| [r, b, g]),
                mapped_cube(33, |[r, g, b]| [r * 4.0, g, b]),
                mapped_cube(65, |[r, g, b]| [r, g, b * 0.5]),
            ],
        );
        let stack = [
            creative_look(1, 1, 1, 10_000),
            creative_look(2, 2, 1, 10_000),
            creative_look(3, 3, 1, 10_000),
            creative_look(4, 4, 1, 10_000),
        ];

        let binding = compositor
            .lut_binding(&stack, Some(luts.library()))
            .expect("four bound LUT nodes fit the atlas");
        // Depth is the sum of the BOUND slot sizes, not `5 * Smax`, and the
        // legacy slot is not allocated because no `cube_lut` is present.
        assert_eq!(binding.atlas.extent(), (65, 65, 2 + 17 + 33 + 65));
        assert_eq!(binding.atlas.extent().2, 117);
        assert_eq!(
            binding.atlas.slot_layout(),
            vec![(2, 0), (17, 2), (33, 19), (65, 52)]
        );

        let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
            .expect("four LUT nodes serialize");
        assert_eq!(grade_header(&bytes, 0), 4);
        let expected_slots: [(f32, f32); 4] = [(2.0, 0.0), (17.0, 2.0), (33.0, 19.0), (65.0, 52.0)];
        for (index, (size, z_origin)) in expected_slots.into_iter().enumerate() {
            let values = index * GRADE_NODE_WORDS + GRADE_NODE_VALUE_OFFSET;
            #[allow(clippy::cast_precision_loss)]
            let slot = index as f32;
            assert_eq!(grade_word(&bytes, values).to_bits(), slot.to_bits());
            assert_eq!(grade_word(&bytes, values + 9).to_bits(), size.to_bits());
            assert_eq!(
                grade_word(&bytes, values + 10).to_bits(),
                z_origin.to_bits()
            );
        }

        // (0.25, 0.75, 0.5)
        //   -> x0.5      (0.125, 0.375, 0.25)
        //   -> swap g,b  (0.125, 0.25,  0.375)
        //   -> red x4    (0.5,   0.25,  0.375)
        //   -> blue x0.5 (0.5,   0.25,  0.1875)
        let frame = linear_frame([0.25, 0.75, 0.5]);
        let rendered = render_luts(&compositor, &frame, &stack, &luts);
        assert_solid_linear(&rendered, [0.5, 0.25, 0.1875], LINEAR_CPU_GPU_MAX);
        // A shader that read slot 0 four times would produce this instead.
        let all_slot_zero = [0.25 * 0.0625, 0.75 * 0.0625, 0.5 * 0.0625];
        for (observed, wrong) in first_rgb(&rendered).iter().zip(&all_slot_zero) {
            assert!(
                (observed - wrong).abs() > LINEAR_CPU_GPU_MAX,
                "the stack collapsed onto a single atlas slot"
            );
        }
    }

    #[test]
    fn the_lut_atlas_is_reused_until_the_slot_signature_changes() {
        // CC4 4.1: the cache is not optional.  Playback composites the same
        // stack every frame and must not re-upload up to 21 MiB per frame.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-atlas-cache", &[lut_b_cube(), lut_d_cube()]);
        let stack = [creative_look(1, 1, 1, 10_000)];
        let first = compositor
            .lut_binding(&stack, Some(luts.library()))
            .expect("one bound node");
        let second = compositor
            .lut_binding(&stack, Some(luts.library()))
            .expect("one bound node");
        assert!(
            Arc::ptr_eq(&first.atlas, &second.atlas),
            "an unchanged slot signature must reuse the uploaded atlas"
        );

        let grown = [stack[0].clone(), creative_look(2, 2, 1, 10_000)];
        let rebuilt = compositor
            .lut_binding(&grown, Some(luts.library()))
            .expect("two bound nodes");
        assert!(
            !Arc::ptr_eq(&first.atlas, &rebuilt.atlas),
            "a changed slot signature must rebuild the atlas"
        );
        assert_eq!(first.atlas.slot_layout(), vec![(2, 0)]);
        assert_eq!(rebuilt.atlas.slot_layout(), vec![(2, 0), (3, 2)]);

        // Returning to the first signature hits the cache again rather than
        // rebuilding, so alternating layers do not thrash.
        let again = compositor
            .lut_binding(&stack, Some(luts.library()))
            .expect("one bound node");
        assert!(Arc::ptr_eq(&first.atlas, &again.atlas));
    }

    #[test]
    fn the_atlas_cache_byte_budget_keeps_the_head_and_holds_the_bound() {
        // CC4 4.1: the entry count says nothing about size, so the cache is
        // trimmed by retained bytes as well. The rule has two halves and both
        // are asserted here: the budget really bounds the tail, and the head -
        // the atlas the frame being rendered just built - is never the entry
        // that gets dropped.
        //
        // Real atlases are hundreds of megabytes and need a device to build,
        // so the decision is exercised on the sizes directly.
        const MIB: u64 = 1024 * 1024;

        // Exactly at the budget is kept; one byte over drops the tail.
        assert_eq!(atlas_cache_kept_entries(&[4 * MIB; 4], 16 * MIB), 4);
        assert_eq!(atlas_cache_kept_entries(&[4 * MIB; 4], 16 * MIB - 1), 3);

        // Eight worst-case `65 x 65 x 325` atlases are about 168 MiB; the
        // 64 MiB budget keeps the three that fit and no more.
        let worst = u64::from(65_u32) * 65 * 325 * 16;
        let kept =
            atlas_cache_kept_entries(&[worst; LUT_ATLAS_CACHE_ENTRIES], LUT_ATLAS_CACHE_MAX_BYTES);
        assert_eq!(kept, 3);
        assert!(
            u64::try_from(kept).unwrap_or(u64::MAX) * worst <= LUT_ATLAS_CACHE_MAX_BYTES,
            "the retained bytes must fit the budget"
        );

        // The head survives even when it alone exceeds the budget, because
        // dropping it would guarantee a rebuild on the very next frame while
        // freeing nothing the caller is not already holding through its `Arc`.
        assert_eq!(atlas_cache_kept_entries(&[512 * MIB, MIB], 64 * MIB), 1);
        assert_eq!(atlas_cache_kept_entries(&[512 * MIB], 64 * MIB), 1);

        // A saturating sum cannot wrap a huge tail back under the budget.
        assert_eq!(atlas_cache_kept_entries(&[MIB, u64::MAX, MIB], 64 * MIB), 1);

        // An empty cache and a zero budget are both well defined.
        assert_eq!(atlas_cache_kept_entries(&[], LUT_ATLAS_CACHE_MAX_BYTES), 0);
        assert_eq!(atlas_cache_kept_entries(&[MIB, MIB], 0), 1);
    }

    #[test]
    fn atlas_cache_retains_its_sources_so_no_recycled_address_can_serve_a_stale_look() {
        // CC4 4.1.  The cache compares lattice identity, and it RETAINS a
        // strong `Arc` to every bound lattice.  Retention is the whole
        // soundness argument: comparing a bare `Arc::as_ptr` would be an ABA
        // compare, because an edited LUT reparsed into a freed allocation's
        // address would match the old key and render the OLD look.  Because a
        // cached atlas pins its own sources, those addresses cannot be
        // recycled while the atlas is servable, so a hit really does mean the
        // very same verified samples.
        let Some(compositor) = fallback() else {
            return;
        };
        // Built directly, not through `LutLibrary`, so the process parse cache
        // holds no reference and every strong count below is ours.
        let first = Arc::new(
            parse_cube_lut(&mapped_cube(2, |[r, g, b]| [r * 0.5, g, b]))
                .expect("the fixture lattice parses"),
        );
        let second = Arc::new(
            parse_cube_lut(&mapped_cube(2, |[r, g, b]| [r, g * 0.25, b]))
                .expect("the fixture lattice parses"),
        );
        assert_eq!(first.size, second.size, "same size, different samples");
        assert_ne!(first.rgba, second.rgba);

        let first_slots = [LutAtlasSlot {
            z_origin: 0,
            lut: Arc::clone(&first),
        }];
        // Ours plus the slot list's.
        assert_eq!(Arc::strong_count(&first), 2);
        let first_atlas = compositor
            .lut_atlas(&first_slots)
            .expect("the fixture atlas uploads");
        assert_eq!(
            Arc::strong_count(&first),
            3,
            "the cached atlas must hold a strong reference of its own"
        );
        assert!(Arc::ptr_eq(&first_atlas.retained_slots()[0], &first));

        // Dropping every handle to the atlas leaves it in the cache, and the
        // retention survives with it.
        drop(first_atlas);
        assert_eq!(Arc::strong_count(&first), 3);

        // A different lattice of the same size misses, however the allocator
        // happened to place it.
        let second_slots = [LutAtlasSlot {
            z_origin: 0,
            lut: Arc::clone(&second),
        }];
        let second_atlas = compositor
            .lut_atlas(&second_slots)
            .expect("the fixture atlas uploads");
        assert!(Arc::ptr_eq(&second_atlas.retained_slots()[0], &second));
        assert_eq!(Arc::strong_count(&second), 3);

        // The first is still cached and still hits, and it still yields ITS
        // lattice rather than the one uploaded most recently.
        let again = compositor
            .lut_atlas(&first_slots)
            .expect("the cached atlas is served");
        assert!(Arc::ptr_eq(&again.retained_slots()[0], &first));
        assert_eq!(
            Arc::strong_count(&first),
            3,
            "ours, the slot list, and the one the cached atlas holds"
        );

        // No freshly parsed lattice can land on a retained address, and every
        // one of them misses and gets an atlas built from its own samples.
        for step in 0..32_u32 {
            #[allow(clippy::cast_precision_loss)]
            let scale = 1.0 / (step as f32 + 2.0);
            let fresh = Arc::new(
                parse_cube_lut(&mapped_cube(2, |[r, g, b]| [r * scale, g, b]))
                    .expect("the fixture lattice parses"),
            );
            assert!(!Arc::ptr_eq(&fresh, &first));
            assert!(!Arc::ptr_eq(&fresh, &second));
            let atlas = compositor
                .lut_atlas(&[LutAtlasSlot {
                    z_origin: 0,
                    lut: Arc::clone(&fresh),
                }])
                .expect("the fixture atlas uploads");
            let retained = &atlas.retained_slots()[0];
            assert!(
                Arc::ptr_eq(retained, &fresh),
                "a cache hit must be the very lattice that was asked for"
            );
            assert_eq!(retained.rgba, fresh.rgba);
        }
    }

    #[test]
    fn an_edited_lut_of_the_same_size_renders_its_new_samples() {
        // The end-to-end shape of the ABA hazard: the same asset id, the same
        // lattice size, edited samples.  The second render must show the new
        // look, never the atlas uploaded for the first.
        let Some(compositor) = fallback() else {
            return;
        };
        let stack = [creative_look(1, 1, 1, 10_000)];
        let frame = linear_frame([0.5, 0.5, 0.5]);

        let before = TestLuts::build(
            "cc4-atlas-edit-before",
            &[mapped_cube(2, |[r, g, b]| [r * 0.5, g, b])],
        );
        let rendered_before = render_luts(&compositor, &frame, &stack, &before);
        assert_solid_linear(&rendered_before, [0.25, 0.5, 0.5], 2e-3);

        // Drop the first library so its lattice is freed and its address is a
        // candidate for reuse by the edited parse - the exact ABA setup.
        drop(before);
        let after = TestLuts::build(
            "cc4-atlas-edit-after",
            &[mapped_cube(2, |[r, g, b]| [r, g * 0.25, b])],
        );
        let rendered_after = render_luts(&compositor, &frame, &stack, &after);
        assert_solid_linear(&rendered_after, [0.5, 0.125, 0.5], 2e-3);
    }

    #[test]
    fn lut_b_renders_every_tetrahedral_branch_and_excludes_the_trilinear_value() {
        // CC4 3.5/10.3.3.  LUT B is `S = 2` over `[0, 1]`, so with
        // `input_encoding = linear` the lattice fraction IS the input triple
        // and each row below selects one of the contract's six formulas by
        // construction.  Every expected value is an exact binary fraction
        // written out by hand from the branch it exercises.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-lut-b-anchor", &[lut_b_cube()]);
        let node = [creative_look(1, 1, 1, 10_000)];
        // branch, input, expected
        let anchors: [(&str, [f32; 3], [f32; 3]); 6] = [
            ("f_r > f_g > f_b", [0.75, 0.5, 0.25], [0.5, 0.375, 0.25]),
            ("f_r > f_b > f_g", [0.75, 0.25, 0.5], [0.5, 0.25, 0.375]),
            ("f_b >= f_r > f_g", [0.5, 0.25, 0.75], [0.375, 0.25, 0.5]),
            ("f_b > f_g >= f_r", [0.25, 0.5, 0.75], [0.25, 0.375, 0.5]),
            ("f_g >= f_b > f_r", [0.25, 0.75, 0.5], [0.25, 0.5, 0.375]),
            ("tie: f_r == f_g == f_b", [0.5, 0.5, 0.5], [0.5, 0.5, 0.5]),
        ];
        for (branch, input, expected) in anchors {
            let rendered = render_luts(&compositor, &linear_frame(input), &node, &luts);
            for (channel, (observed, want)) in
                first_rgb(&rendered).iter().zip(&expected).enumerate()
            {
                assert!(
                    (observed - want).abs() <= LINEAR_CPU_GPU_MAX,
                    "{branch} at {input:?}, channel {channel}: {observed} != {want}"
                );
            }
        }

        // Trilinear interpolation of the SAME lattice at the first anchor.
        // This is not a tolerance question: the two rules disagree by more
        // than an order of magnitude above the gate, so the fixture proves
        // tetrahedral is actually what runs.
        let rendered = render_luts(&compositor, &linear_frame([0.75, 0.5, 0.25]), &node, &luts);
        let trilinear = [0.421_875, 0.296_875, 0.171_875];
        for (channel, (observed, wrong)) in first_rgb(&rendered).iter().zip(&trilinear).enumerate()
        {
            assert!(
                (observed - wrong).abs() > LINEAR_CPU_GPU_MAX,
                "channel {channel} matched the trilinear value {wrong}: {observed}"
            );
        }

        // CC4 3.5 also claims the tie is WELL DEFINED: all six formulas agree
        // analytically on the shared faces.  Transcribed here in f64,
        // independently of the shader, over LUT B's lattice at f = (.5,.5,.5).
        let v = |r: usize, g: usize, b: usize| -> [f64; 3] {
            if r == 1 && g == 1 && b == 1 {
                [1.0, 1.0, 1.0]
            } else {
                [
                    if r == 1 { 0.5 } else { 0.0 },
                    if g == 1 { 0.5 } else { 0.0 },
                    if b == 1 { 0.5 } else { 0.0 },
                ]
            }
        };
        let (c000, c100, c010, c110) = (v(0, 0, 0), v(1, 0, 0), v(0, 1, 0), v(1, 1, 0));
        let (c001, c101, c011, c111) = (v(0, 0, 1), v(1, 0, 1), v(0, 1, 1), v(1, 1, 1));
        let (fr, fg, fb) = (0.5_f64, 0.5_f64, 0.5_f64);
        let blend = |a: [f64; 3], p: [f64; 3], q: [f64; 3], r: [f64; 3]| {
            let mut out = [0.0; 3];
            for channel in 0..3 {
                out[channel] = a[channel] + fr * p[channel] + fg * q[channel] + fb * r[channel];
            }
            out
        };
        let difference = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let formulas = [
            blend(
                c000,
                difference(c100, c000),
                difference(c110, c100),
                difference(c111, c110),
            ),
            blend(
                c000,
                difference(c100, c000),
                difference(c111, c101),
                difference(c101, c100),
            ),
            blend(
                c000,
                difference(c101, c001),
                difference(c111, c101),
                difference(c001, c000),
            ),
            blend(
                c000,
                difference(c111, c011),
                difference(c011, c001),
                difference(c001, c000),
            ),
            blend(
                c000,
                difference(c111, c011),
                difference(c010, c000),
                difference(c011, c010),
            ),
            blend(
                c000,
                difference(c110, c010),
                difference(c010, c000),
                difference(c111, c110),
            ),
        ];
        // Exact equality is the point: every operand is an exact binary
        // fraction, so the six formulas must agree bit-for-bit, not nearly.
        #[allow(clippy::float_cmp)]
        for (index, formula) in formulas.iter().enumerate() {
            assert_eq!(*formula, [0.5, 0.5, 0.5], "tetrahedral formula {index}");
        }
    }

    #[test]
    fn lut_input_encodings_apply_enc_dec_and_use_the_signed_display_inverse() {
        // CC4 3.4.  The lattice halves its input IN THE ENCODED DOMAIN, so a
        // node that skipped `ENC`/`DEC` would return a visibly different
        // number, and one that decoded with CC1's SOURCE decode would break on
        // the negative sample.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build(
            "cc4-encodings",
            &[mapped_cube(17, |[r, g, b]| [r * 0.5, g * 0.5, b * 0.5])],
        );
        let frame = linear_frame([0.25, 0.25, 0.25]);

        // display709: e = encode_bt709(0.25) = 0.48993948, halved to
        // 0.24496974, decoded by decode_display709 to 0.07567266.
        let display = render_luts(
            &compositor,
            &frame,
            &[creative_look(1, 1, 0, 10_000)],
            &luts,
        );
        assert_solid_linear(&display, [0.075_672_66; 3], 1e-4);
        // linear: the same lattice on the raw value gives 0.125.  The two
        // results are 0.049 apart, more than thirty times the gate, so "the
        // encoding is applied" is not a tolerance question.
        let linear = render_luts(
            &compositor,
            &frame,
            &[creative_look(2, 1, 1, 10_000)],
            &luts,
        );
        assert_solid_linear(&linear, [0.125; 3], LINEAR_CPU_GPU_MAX);
        assert!((first_rgb(&display)[0] - 0.125).abs() > 30.0 * LINEAR_CPU_GPU_MAX);
        // grade709: CC3's exact analytic pair, 0.07573866.  Tokens 0 and 2
        // agree to about 7e-5 by design — they are two roundings of the same
        // curve — so this pins that the token-2 branch encodes at all, not
        // which of the two near-identical curves ran.
        let grade = render_luts(
            &compositor,
            &frame,
            &[creative_look(3, 1, 2, 10_000)],
            &luts,
        );
        assert_solid_linear(&grade, [0.075_738_66; 3], 1e-4);

        // The sign-preserving inverse, on a sample below the domain.  With
        // `e = -0.48993948` the clamp gives `u = 0`, the lookup gives `0`, and
        // the whole excursion is restored: `z = -0.48993948`.  CC4's
        // `decode_display709` returns -0.25; CC1's `decode_bt709` would take
        // its unconditional linear branch and return -0.10887544.
        let negative = linear_frame([-0.25, -0.25, -0.25]);
        let rendered = render_luts(
            &compositor,
            &negative,
            &[creative_look(4, 1, 0, 10_000)],
            &luts,
        );
        assert_solid_linear(&rendered, [-0.25; 3], LINEAR_CPU_GPU_MAX);
        for observed in first_rgb(&rendered) {
            assert!(
                (observed - (-0.108_875_44)).abs() > LINEAR_CPU_GPU_MAX,
                "the node decoded with CC1's source decode: {observed}"
            );
        }
    }

    #[test]
    fn out_of_domain_excursions_are_restored_additively_not_clamped() {
        // CC4 10.3.4 with LUT D.  A pure-clamp implementation would return the
        // boundary lattice value; the additive rule keeps the excursion, which
        // is what makes an over-range highlight recoverable.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-out-of-domain", &[lut_d_cube()]);
        let node = [creative_look(1, 1, 1, 10_000)];

        // e = (2, 2, 2) clamps to dmax (1.5), whose lookup is (1, 1, 1); the
        // 0.5 excursion is restored on top of it.
        let high = render_luts(&compositor, &linear_frame([2.0, 2.0, 2.0]), &node, &luts);
        assert_solid_linear(&high, [1.5, 1.5, 1.5], LINEAR_CPU_GPU_MAX);
        for observed in first_rgb(&high) {
            assert!(
                (observed - 1.0).abs() > LINEAR_CPU_GPU_MAX,
                "a pure clamp would have returned the boundary value 1.0"
            );
            assert!(
                (observed - 2.0).abs() > LINEAR_CPU_GPU_MAX,
                "the node must not be the identity outside the domain"
            );
        }

        // e = (-1, -1, -1) clamps to dmin (-0.5), whose lookup is (0, 0, 0).
        let low = render_luts(&compositor, &linear_frame([-1.0, -1.0, -1.0]), &node, &luts);
        assert_solid_linear(&low, [-0.5, -0.5, -0.5], LINEAR_CPU_GPU_MAX);
        for observed in first_rgb(&low) {
            assert!(
                observed.abs() > LINEAR_CPU_GPU_MAX,
                "a pure clamp would have returned 0.0"
            );
        }

        // The in-domain mapping anchor, so the domain rescale itself is pinned
        // and not only its saturating ends: CC4 10.3.3's LUT D row.
        let inside = render_luts(&compositor, &linear_frame([0.5, 0.0, 1.0]), &node, &luts);
        assert_solid_linear(&inside, [0.25, 0.125, 0.625], LINEAR_CPU_GPU_MAX);
    }

    #[test]
    fn lut_mix_blends_in_linear_light_between_exact_endpoints() {
        // CC4 10.3.5 with LUT B at `x = (0.75, 0.5, 0.25)`.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-mix", &[lut_b_cube()]);
        let frame = linear_frame([0.75, 0.5, 0.25]);

        let full = render_luts(
            &compositor,
            &frame,
            &[creative_look(1, 1, 1, 10_000)],
            &luts,
        );
        assert_solid_linear(&full, [0.5, 0.375, 0.25], LINEAR_CPU_GPU_MAX);

        let half = render_luts(&compositor, &frame, &[creative_look(1, 1, 1, 5_000)], &luts);
        assert_solid_linear(&half, [0.625, 0.437_5, 0.25], LINEAR_CPU_GPU_MAX);

        // `mix = 0` is inactive (CC4 3.6): the node is not written at all, so
        // it is bit-identical to removing it, not merely close to it.
        let none = render_luts(&compositor, &frame, &[creative_look(1, 1, 1, 0)], &luts);
        let removed = render_luts(&compositor, &frame, &[], &luts);
        assert_eq!(none.len(), removed.len());
        for (index, (left, right)) in none.iter().zip(&removed).enumerate() {
            assert_eq!(left.to_bits(), right.to_bits(), "channel {index}");
        }
    }

    #[test]
    fn a_linear_identity_lattice_is_bit_exact_on_the_gpu() {
        // CC4 3.5's exactness claim: with `input_encoding = linear`, domain
        // `[0, 1]`, and `S - 1` a power of two, every lattice coordinate,
        // fraction, and interpolation weight is an exact binary fraction, so
        // the identity lattice reproduces the input BIT-exactly.  Asserted
        // with `to_bits`, never with a tolerance.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-identity", &[identity_cube(17)]);
        let node = [creative_look(1, 1, 1, 10_000)];
        for value in [0.0_f32, 0.0625, 0.25, 0.375, 0.5, 0.75, 0.9375, 1.0] {
            let frame = linear_frame([value, value * 0.5, 1.0 - value]);
            let through = render_luts(&compositor, &frame, &node, &luts);
            let baseline = render_luts(&compositor, &frame, &[], &luts);
            assert_eq!(through.len(), baseline.len());
            for (index, (left, right)) in through.iter().zip(&baseline).enumerate() {
                assert_eq!(
                    left.to_bits(),
                    right.to_bits(),
                    "value {value}, channel {index}: {left} != {right}"
                );
            }
        }
    }

    #[test]
    fn inactive_lut_nodes_are_never_written_and_take_no_atlas_slot() {
        // CC4 3.6: bypass and `mix = 0` are LOSSLESSLY identical to removing
        // the node, on the buffer, in the atlas, and in the rendered pixels.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build("cc4-inactive", &[lut_b_cube()]);
        let mut bypassed = creative_look(1, 1, 1, 10_000);
        bypassed
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        let neutral = creative_look(2, 1, 1, 0);
        let stack = [bypassed, neutral];

        let bytes = grade_buffer_bytes_with_luts(&stack, Some(luts.library()))
            .expect("inactive LUT nodes serialize");
        assert_eq!(grade_header(&bytes, 0), 0);
        assert_eq!(bytes.len(), GRADE_HEADER_BYTES + GRADE_NODE_BYTES);
        assert!(bytes[GRADE_HEADER_BYTES..].iter().all(|byte| *byte == 0));

        // No managed slot is allocated: the atlas is the shared `S = 2`
        // placeholder that keeps binding 3 valid.
        let binding = compositor
            .lut_binding(&stack, Some(luts.library()))
            .expect("an all-inactive stack binds the placeholder");
        assert_eq!(binding.atlas.slot_layout(), vec![(2, 0)]);
        assert!(!binding.legacy_enabled);

        let frame = linear_frame([0.75, 0.5, 0.25]);
        let with_nodes = render_luts(&compositor, &frame, &stack, &luts);
        let without = render_luts(&compositor, &frame, &[], &luts);
        assert_eq!(with_nodes.len(), without.len());
        for (index, (left, right)) in with_nodes.iter().zip(&without).enumerate() {
            assert_eq!(left.to_bits(), right.to_bits(), "channel {index}");
        }
    }

    #[test]
    fn a_fifth_active_lut_node_is_rejected() {
        // CC4 3.1/10.3.8: the limit exists because each ACTIVE node needs an
        // atlas slot, so an inactive fifth node is not a violation here even
        // though Core counts it against `LUT_NODE_LIMIT_PER_LAYER` on edit.
        let luts = TestLuts::build("cc4-slot-limit", &[lut_b_cube()]);
        let four = (0..4)
            .map(|index| creative_look(index + 1, 1, 1, 10_000))
            .collect::<Vec<_>>();
        assert!(grade_buffer_bytes_with_luts(&four, Some(luts.library())).is_ok());

        let mut five = four.clone();
        five.push(creative_look(5, 1, 1, 10_000));
        let error = grade_buffer_bytes_with_luts(&five, Some(luts.library()))
            .expect_err("a fifth active LUT node is rejected");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("too_many_lut_nodes:"),
            "unexpected message: {message}"
        );
        assert!(message.contains('5'), "unexpected message: {message}");
        assert!(message.contains('4'), "unexpected message: {message}");

        let mut bypassed_fifth = four;
        let mut fifth = creative_look(5, 1, 1, 10_000);
        fifth
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        bypassed_fifth.push(fifth);
        assert!(grade_buffer_bytes_with_luts(&bypassed_fifth, Some(luts.library())).is_ok());
    }

    #[test]
    fn an_unresolvable_lut_asset_fails_the_render_rather_than_dropping_the_look() {
        // CC4 2.3: a missing asset blocks with a typed error.  It must never
        // be silently skipped, because a look-free frame is indistinguishable
        // from a correctly graded one to everything downstream.
        let luts = TestLuts::build("cc4-missing-asset", &[lut_b_cube()]);
        let dangling = [creative_look(1, 99, 1, 10_000)];
        let error = grade_buffer_bytes_with_luts(&dangling, Some(luts.library()))
            .expect_err("a dangling asset reference is rejected");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("missing_lut_asset:"),
            "unexpected message: {message}"
        );
        assert!(message.contains("creative_look"), "{message}");
        assert!(message.contains("99"), "{message}");

        // No library at all is the same failure, not a quiet identity.
        let bound = [creative_look(1, 1, 1, 10_000)];
        let error = grade_buffer_bytes_with_luts(&bound, None)
            .expect_err("an unpublished library is rejected");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("missing_lut_asset:"),
            "unexpected message: {message}"
        );

        // An INACTIVE node never resolves an asset, so a bypassed node bound
        // to a missing asset does not block the render.
        let mut bypassed = creative_look(1, 99, 1, 10_000);
        bypassed
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        assert!(grade_buffer_bytes_with_luts(&[bypassed], None).is_ok());
    }

    #[test]
    fn a_legacy_cube_lut_and_a_managed_look_coexist_with_the_legacy_stage_last() {
        // CC4 10.3.9: the legacy branch runs after every managed node, in atlas
        // slot 4, REGARDLESS of the two effects' relative order in
        // `clip.effects`.
        let Some(compositor) = fallback() else {
            return;
        };
        let luts = TestLuts::build(
            "cc4-legacy-coexist",
            &[mapped_cube(2, |[r, g, b]| [r * 0.5, g * 0.5, b * 0.5])],
        );
        let directory = TempDirectory::new("cc4-legacy-swap");
        let swap = directory.path("swap.cube");
        fs::write(
            &swap,
            "LUT_3D_SIZE 2\n\
             0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n",
        )
        .expect("the legacy LUT is written");
        let legacy = Effect {
            id: EffectId(9),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([
                (
                    "path".to_owned(),
                    ParamValue::Text(swap.to_string_lossy().into_owned()),
                ),
                ("intensity_percent".to_owned(), ParamValue::Integer(100)),
            ]),
            keyframes: BTreeMap::new(),
        };
        let managed = creative_look(1, 1, 1, 10_000);

        let frame = linear_frame([0.25, 0.0, 0.0]);
        let managed_first = render_luts(
            &compositor,
            &frame,
            &[managed.clone(), legacy.clone()],
            &luts,
        );
        let legacy_first = render_luts(
            &compositor,
            &frame,
            &[legacy.clone(), managed.clone()],
            &luts,
        );
        assert_eq!(managed_first.len(), legacy_first.len());
        for (index, (left, right)) in managed_first.iter().zip(&legacy_first).enumerate() {
            assert_eq!(
                left.to_bits(),
                right.to_bits(),
                "vector order changed the result at channel {index}"
            );
        }

        // The managed node halves the red channel in linear light, then the
        // legacy stage swaps red and blue in display code and round-trips
        // through BT.709.  Both stages therefore ran, in that order.
        assert_solid_linear(&managed_first, [0.0, 0.0, 0.125], LINEAR_CPU_GPU_MAX);
        let managed_only = render_luts(&compositor, &frame, std::slice::from_ref(&managed), &luts);
        assert_solid_linear(&managed_only, [0.125, 0.0, 0.0], LINEAR_CPU_GPU_MAX);
        let legacy_only = render_luts(&compositor, &frame, std::slice::from_ref(&legacy), &luts);
        assert_solid_linear(&legacy_only, [0.0, 0.0, 0.25], LINEAR_CPU_GPU_MAX);

        // Slot 4 is the legacy one in both orders: one managed slice first,
        // then the legacy lattice.
        for order in [
            vec![managed.clone(), legacy.clone()],
            vec![legacy.clone(), managed.clone()],
        ] {
            let binding = compositor
                .lut_binding(&order, Some(luts.library()))
                .expect("legacy and managed coexist");
            assert_eq!(binding.atlas.slot_layout(), vec![(2, 0), (2, 2)]);
            assert_eq!(binding.legacy_z_origin, 2);
            assert!(binding.legacy_enabled);
        }
    }

    #[test]
    fn a_look_free_layer_binds_the_identity_placeholder() {
        // The binding must stay valid with no LUT of any kind, and the atlas
        // must not be rebuilt on every frame of look-free playback.
        let Some(compositor) = fallback() else {
            return;
        };
        let first = compositor
            .lut_binding(&[], None)
            .expect("an empty stack binds");
        assert_eq!(first.atlas.slot_layout(), vec![(2, 0)]);
        assert_eq!(first.atlas.extent(), (2, 2, 2));
        assert!(!first.legacy_enabled);
        let second = compositor
            .lut_binding(
                &[effect(1, "primary_correction", "exposure_milli_stops", 500)],
                None,
            )
            .expect("a look-free managed stack binds");
        assert!(
            Arc::ptr_eq(&first.atlas, &second.atlas),
            "look-free playback must not re-upload the placeholder every frame"
        );
    }

    #[test]
    fn masks_and_chroma_key_create_real_alpha_for_lower_layers() {
        let Some(compositor) = fallback() else {
            return;
        };
        let blue = solid(8, 8, [0, 0, 255, 255]);
        let red = solid(8, 8, [255, 0, 0, 255]);
        let mask = effect_with(
            1,
            "mask",
            &[
                ("shape_token", 2),
                ("width_percent", 50),
                ("height_percent", 50),
            ],
        );
        let output = compositor
            .render(
                (8, 8),
                &[
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &red,
                        effects: &[mask],
                        transition: TransitionRenderParams::default(),
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 4, 4), [255, 0, 0, 255], 3);
        assert_pixel_close(pixel(&output, 0, 0), [0, 0, 255, 255], 3);

        let green = solid(8, 8, [0, 255, 0, 255]);
        let key = effect_with(2, "chroma_key", &[("threshold_percent", 15)]);
        // CC1 2.2.4: keying is coverage, not colour correction, so it must
        // not put the layer through the legacy display-coded branch.
        assert!(!legacy_stage_active(std::slice::from_ref(&key)));
        let output = compositor
            .render(
                (8, 8),
                &[
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &green,
                        effects: &[key],
                        transition: TransitionRenderParams::default(),
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 4, 4), [0, 0, 255, 255], 3);
    }

    /// Build a one-pixel-per-texel linear working frame.
    fn working(width: u32, height: u32, rgba: [f32; 4]) -> WorkingFrame {
        let pixels = std::iter::repeat_n(rgba, usize::try_from(width * height).unwrap())
            .flatten()
            .map(f16::from_f32)
            .collect::<Vec<_>>();
        WorkingFrame {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    #[test]
    fn chroma_key_alone_never_clamps_the_working_colour_it_passes_through() {
        // This asserts an over-range half-float value survives the actual
        // render target; either adapter class demonstrates that.
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let compositor = Compositor::new(gpu);

        // Far enough from the default green key colour that alpha stays 1,
        // and over-range in red so a display-space round trip would clip it.
        let over_range = working(4, 4, [1.5, 0.2, 0.2, 1.0]);
        let key = effect_with(1, "chroma_key", &[("threshold_percent", 15)]);
        assert!(!legacy_stage_active(std::slice::from_ref(&key)));

        let keyed = compositor
            .render_working(
                (4, 4),
                &[CompositorLayer {
                    frame: &over_range,
                    effects: std::slice::from_ref(&key),
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("chroma-key working-surface readback");
        let unkeyed = compositor
            .render_working(
                (4, 4),
                &[CompositorLayer {
                    frame: &over_range,
                    effects: &[],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("neutral working-surface readback");

        // CC1 2.2.5: no colour stage clamps RGB, and CC1 2.2.4: keying does
        // not change the colour of pixels it keeps.
        assert!(
            keyed[0] > 1.4,
            "chroma_key clamped an over-range working value: {:?}",
            &keyed[0..4]
        );
        assert!(
            (keyed[3] - 1.0).abs() < 1e-3,
            "keyed alpha was {}",
            keyed[3]
        );
        for (index, (keyed_value, unkeyed_value)) in
            keyed.iter().zip(unkeyed.iter()).enumerate().take(16)
        {
            assert!(
                (keyed_value - unkeyed_value).abs() < 1e-3,
                "chroma_key changed channel {index}: {keyed_value} vs {unkeyed_value}"
            );
        }
    }

    /// CC1 5: only a genuine full-scale render at the document raster may
    /// claim `full_resolution`. Comparing rasters alone is not enough, because
    /// a proxy render of a document already at or below the proxy bound comes
    /// back at exactly the document raster.
    #[test]
    fn monitor_proof_claims_full_resolution_only_for_a_full_scale_render() {
        let Some(gpu) = fixture_gpu_or_skip() else {
            return;
        };
        let proxy = RenderScale::Proxy { max_width: 1280 };
        assert_eq!(proxy.output_resolution((640, 360)), (640, 360));
        assert!(
            !gpu.monitor_proof_metadata_for(proxy, (640, 360), (640, 360))
                .full_resolution,
            "a proxy render must never claim the full raster"
        );
        assert!(
            gpu.monitor_proof_metadata_for(RenderScale::FullResolution, (640, 360), (640, 360))
                .full_resolution
        );
        assert!(
            !gpu.monitor_proof_metadata_for(RenderScale::FullResolution, (320, 180), (640, 360))
                .full_resolution,
            "a short render must never claim the full raster"
        );
    }

    /// CC1 2.2.4: keying does not change the colour of pixels it keeps. A kept
    /// pixel with a NEGATIVE working channel is the sharp case: the old
    /// `max(0.0, ...)` spill clamp reduced to `max(0.0, g)` at `key_alpha == 1`
    /// and quietly crushed the sign.
    #[test]
    fn chroma_key_keeps_a_negative_green_working_value_bit_identical() {
        let Some(compositor) = fallback() else {
            return;
        };

        // Display-coded distance from the default green key is about 0.70,
        // well past the default 0.15/0.10 band, so this pixel is fully kept.
        let negative_green = working(4, 4, [0.3, -0.05, 0.2, 1.0]);
        let key = effect_with(1, "chroma_key", &[("threshold_percent", 15)]);
        assert!(!legacy_stage_active(std::slice::from_ref(&key)));

        let keyed = compositor
            .render_working(
                (4, 4),
                &[CompositorLayer {
                    frame: &negative_green,
                    effects: std::slice::from_ref(&key),
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("chroma-key working-surface readback");
        let unkeyed = compositor
            .render_working(
                (4, 4),
                &[CompositorLayer {
                    frame: &negative_green,
                    effects: &[],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("neutral working-surface readback");

        assert!(
            keyed[1] < 0.0,
            "chroma_key crushed a negative working green to {}",
            keyed[1]
        );
        // A kept pixel is not attenuated by coverage, so the over-range red
        // survives at full strength. (The composited alpha is always 1: layers
        // blend over an opaque black clear.)
        assert!(
            keyed[0] > 0.29,
            "the pixel should be fully kept, red was {}",
            keyed[0]
        );
        for (index, (keyed_value, unkeyed_value)) in
            keyed.iter().zip(unkeyed.iter()).enumerate().take(16)
        {
            assert_eq!(
                keyed_value.to_bits(),
                unkeyed_value.to_bits(),
                "chroma_key changed channel {index}: {keyed_value} vs {unkeyed_value}"
            );
        }
    }

    /// An edge pixel keeps the pre-CC1 DISPLAY-CODED spill strength. Moving the
    /// dominance into linear light would silently restrengthen spill in every
    /// project that was keyed before CC1, so the expected value below is
    /// written out from the display-space formula by hand.
    #[test]
    fn chroma_key_edge_pixel_gets_the_display_coded_spill_amount() {
        // The layer blends over an opaque black clear with
        // `src.rgb * srcAlpha`, so the readback is the working colour scaled
        // by coverage and the readback alpha is always 1.
        const EXPECTED_KEY_ALPHA: f32 = 0.132_483_8;
        const EXPECTED_GREEN_LINEAR: f32 = 0.080_793_0;
        const EXPECTED_RED: f32 = 0.05 * EXPECTED_KEY_ALPHA;
        const EXPECTED_GREEN: f32 = EXPECTED_GREEN_LINEAR * EXPECTED_KEY_ALPHA;
        // The pre-fix linear-dominance form would have left green at 0.10962
        // linear, i.e. 0.014523 after coverage.
        const LINEAR_DOMINANCE_GREEN: f32 = 0.109_62 * EXPECTED_KEY_ALPHA;

        let Some(compositor) = fallback() else {
            return;
        };

        // Working linear (0.05, 0.5, 0.05).  BT.709 display codes:
        //   e(0.05) = 1.099 * 0.05^0.45 - 0.099 = 0.1864627
        //   e(0.50) = 1.099 * 0.50^0.45 - 0.099 = 0.7055147
        // Distance from the (0, 1, 0) key colour:
        //   |(0.1864627, -0.2944853, 0.1864627)| / sqrt(3) = 0.2282206
        // threshold 50% and softness 100% give the band [0.0, 1.0], so
        //   key_alpha = d * d * (3 - 2 * d) = 0.1324838
        // Spill at 100%:
        //   dominance = 0.7055147 - 0.1864627               = 0.5190520
        //   g_display = 0.7055147 - dominance * (1 - alpha)  = 0.2552297
        //   g_linear  = ((0.2552297 + 0.099) / 1.099)^(1/0.45) = 0.0807930
        // The pre-fix linear-dominance form would have produced 0.1096, which
        // the tolerance below excludes.
        let edge = working(4, 4, [0.05, 0.5, 0.05, 1.0]);
        let key = effect_with(
            1,
            "chroma_key",
            &[
                ("threshold_percent", 50),
                ("softness_percent", 100),
                ("spill_percent", 100),
            ],
        );
        let keyed = compositor
            .render_working(
                (4, 4),
                &[CompositorLayer {
                    frame: &edge,
                    effects: std::slice::from_ref(&key),
                    transition: TransitionRenderParams::default(),
                }],
            )
            .expect("chroma-key edge working-surface readback");

        let [red, green, blue, ..] = keyed[0..4] else {
            panic!("four channels");
        };
        // Coverage is real: the edge pixel is neither fully keyed nor fully
        // kept, so it is attenuated to about 13% of its working colour.
        assert!(
            (red - EXPECTED_RED).abs() < 2.0e-4,
            "edge red was {red}, expected {EXPECTED_RED}"
        );
        assert!(
            (blue - EXPECTED_RED).abs() < 2.0e-4,
            "spill suppression changed blue: {blue}"
        );
        assert!(
            (green - EXPECTED_GREEN).abs() < 2.0e-4,
            "edge green was {green}, expected {EXPECTED_GREEN} (linear-dominance spill would give {LINEAR_DOMINANCE_GREEN})"
        );
    }

    /// The compositor must not keep a private opinion about which effects run
    /// in the legacy display-coded shader branch: core owns the classification
    /// that QA, delivery conformance, and the inspector already report.
    #[test]
    fn legacy_shader_routing_agrees_with_core_effect_classification() {
        let mut routed = Vec::new();
        for descriptor in EFFECT_DESCRIPTORS {
            let candidate = effect_with(1, descriptor.name, &[]);
            let active = legacy_stage_active(std::slice::from_ref(&candidate));
            assert_eq!(
                active,
                kinewright_core::effect_compatibility_stage(descriptor.name).is_some(),
                "{} routes to the legacy branch as {active} but core disagrees",
                descriptor.name
            );
            if active {
                routed.push(descriptor.name);
            }
        }
        routed.sort_unstable();
        // Both compatibility stages go through the legacy branch: the
        // display-coded controls and the post-primary LUTs.
        assert_eq!(
            routed,
            [
                "brightness",
                "contrast",
                "cube_lut",
                "look_lut",
                "saturation"
            ]
        );
    }

    /// The pool is a reuse optimization, so it must be bounded by the memory it
    /// actually holds, not by a texture count that says nothing at 4K.
    #[test]
    fn texture_pool_holds_its_byte_budget_and_keeps_the_hot_shape() {
        let Some(compositor) = fallback() else {
            return;
        };
        // Pool accounting is derived entirely from the key, so a 1x1
        // placeholder can stand in for a 4K texture. That keeps this a test of
        // the budget policy instead of a several-gigabyte allocation.
        let placeholder = || {
            compositor
                .gpu
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("texture pool budget probe"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: OUTPUT_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
        };
        // 3840 * 2160 * 8 bytes is about 63 MiB, so exactly four of these fit
        // in the budget and a fifth does not.
        let shape = |index: u32| TexturePoolKey {
            width: 3_840 - index,
            height: 2_160,
            format: OUTPUT_FORMAT,
        };
        assert!(texture_pool_bytes(shape(0)) * 4 <= TEXTURE_POOL_MAX_BYTES);
        assert!(texture_pool_bytes(shape(0)) * 5 > TEXTURE_POOL_MAX_BYTES);

        let mut pool = TexturePool::default();
        let shapes = (0..6_u32).map(shape).collect::<Vec<_>>();
        for key in &shapes {
            // One frame at this shape returns two layer textures.
            pool.store(*key, placeholder());
            pool.store(*key, placeholder());
            pool.evict(std::slice::from_ref(key));
            assert!(
                pool.bytes <= TEXTURE_POOL_MAX_BYTES,
                "pool held {} bytes after shape {}x{}",
                pool.bytes,
                key.width,
                key.height
            );
            assert!(pool.shapes.len() <= TEXTURE_POOL_MAX_SHAPES);
            // Eviction must never take the shape the frame just used: doing so
            // guarantees a reallocation on the next frame at the same raster.
            assert!(
                pool.shapes
                    .get(key)
                    .is_some_and(|textures| !textures.is_empty()),
                "the hot shape was evicted"
            );
        }
        assert!(pool.take(*shapes.last().expect("six shapes")).is_some());

        // A single shape can bust the budget on its own; it is trimmed rather
        // than left over budget, and the per-shape depth cap still applies.
        let mut pool = TexturePool::default();
        let key = shape(0);
        for _ in 0..(TEXTURE_POOL_MAX_PER_SHAPE + 3) {
            pool.store(key, placeholder());
        }
        assert_eq!(pool.shapes[&key].len(), TEXTURE_POOL_MAX_PER_SHAPE);
        pool.evict(std::slice::from_ref(&key));
        assert!(pool.bytes <= TEXTURE_POOL_MAX_BYTES);
        assert!(
            !pool.shapes[&key].is_empty(),
            "trimming must not empty the only shape"
        );
    }

    #[test]
    fn layer_textures_are_recycled_without_leaking_pixels_between_frames() {
        let Some(compositor) = fallback() else {
            return;
        };
        let pooled = |compositor: &Compositor| -> usize {
            compositor
                .texture_pool
                .lock()
                .expect("texture pool lock")
                .shapes
                .values()
                .map(Vec::len)
                .sum()
        };

        assert_eq!(pooled(&compositor), 0);
        let red = solid(4, 4, [255, 0, 0, 255]);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 0, 0), [255, 0, 0, 255], 2);
        assert_eq!(pooled(&compositor), 1, "the layer texture was not recycled");

        // The recycled texture is fully overwritten, so no red survives.
        let blue = solid(4, 4, [0, 0, 255, 255]);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &blue,
                    effects: &[],
                    transition: TransitionRenderParams::default(),
                }],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 0, 0), [0, 0, 255, 255], 2);
        assert_eq!(pooled(&compositor), 1, "the pool grew for one reused shape");

        // Two layers of the same shape need two distinct live textures.
        let output = compositor
            .render(
                (4, 4),
                &[
                    CompositorLayer {
                        frame: &red,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 0, 0), [0, 0, 255, 255], 2);
        assert_eq!(pooled(&compositor), 2);

        // A different shape is a different pool key, and the pool stays
        // bounded by the shape budget.
        for width in 1..=(u32::try_from(TEXTURE_POOL_MAX_SHAPES).unwrap() + 4) {
            let frame = solid(width, 2, [8, 8, 8, 255]);
            compositor
                .render(
                    (4, 4),
                    &[CompositorLayer {
                        frame: &frame,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    }],
                )
                .unwrap();
        }
        let shapes = compositor
            .texture_pool
            .lock()
            .expect("texture pool lock")
            .shapes
            .len();
        assert!(
            shapes <= TEXTURE_POOL_MAX_SHAPES,
            "texture pool retained {shapes} shapes"
        );
    }

    #[test]
    fn chroma_key_still_composites_after_an_active_legacy_display_stage() {
        let Some(compositor) = fallback() else {
            return;
        };
        let blue = solid(8, 8, [0, 0, 255, 255]);
        let green = solid(8, 8, [0, 255, 0, 255]);
        // A legacy display stage plus a key must still key: the key branch
        // runs after the legacy branch so the stacked behaviour is preserved.
        let effects = [
            effect(1, "saturation", "percent", 0),
            effect_with(2, "chroma_key", &[("threshold_percent", 15)]),
        ];
        assert!(legacy_stage_active(&effects));
        let output = compositor
            .render(
                (8, 8),
                &[
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &green,
                        effects: &effects,
                        transition: TransitionRenderParams::default(),
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(pixel(&output, 4, 4), [0, 0, 255, 255], 3);
    }

    #[test]
    fn z_order_and_crossfade_alpha_blend_bottom_to_top() {
        let Some(compositor) = fallback() else {
            return;
        };
        let red = solid(2, 2, [255, 0, 0, 255]);
        let blue = solid(2, 2, [0, 0, 255, 255]);
        let output = compositor
            .render(
                (2, 2),
                &[
                    CompositorLayer {
                        frame: &red,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition: TransitionRenderParams {
                            alpha: 0.5,
                            ..TransitionRenderParams::default()
                        },
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [180, 0, 180, 255], 3);

        for (name, fade_white, expected) in [
            ("fade_from_black", 0.0, [0, 0, 180, 255]),
            ("fade_from_white", 1.0, [180, 180, 255, 255]),
        ] {
            let output = compositor
                .render(
                    (2, 2),
                    &[
                        CompositorLayer {
                            frame: &red,
                            effects: &[],
                            transition: TransitionRenderParams::default(),
                        },
                        CompositorLayer {
                            frame: &blue,
                            effects: &[],
                            transition: TransitionRenderParams {
                                alpha: 1.0,
                                fade_mix: 0.5,
                                fade_white,
                            },
                        },
                    ],
                )
                .unwrap();
            assert_pixel_close(&output.rgba[0..4], expected, 2);
            assert_eq!(output.rgba[3], 255, "{name} must occlude lower layers");
        }
    }

    #[test]
    fn rasterized_title_is_composited_by_the_existing_layer_pipeline() {
        let Some(compositor) = fallback() else {
            return;
        };
        let background = solid(320, 180, [12, 18, 24, 255]);
        let title = crate::title::TitleRasterizer::new()
            .rasterize(
                &Title {
                    text: "Kinewright".to_owned(),
                    ..Title::default()
                },
                (320, 180),
            )
            .unwrap();
        let output = compositor
            .render(
                (320, 180),
                &[
                    CompositorLayer {
                        frame: &background,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                    CompositorLayer {
                        frame: &title,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [12, 18, 24, 255], 2);
        assert!(
            output
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != [12, 18, 24]),
            "title layer did not change any compositor output pixels"
        );
    }

    // ================= CC5 secondaries: matte block and shader =============

    /// CC5 3.1's word map, written out by hand.
    ///
    /// Every word is a literal derived from the stored integer by the unit
    /// conversion the contract states, so a silent reordering of the block —
    /// which the shader would happily read as a different matte — fails here
    /// rather than as a wrong picture.
    #[test]
    #[allow(clippy::float_cmp)]
    fn matte_block_layout_is_the_cc5_word_map() {
        let node = with_matte(
            wheels(1, &[("gain_master_thousandths", 1_500)]),
            &[
                ("matte_window_count", 1),
                ("matte_combine_token", 1),
                ("matte_invert", 1),
                ("matte_mix_basis_points", 6_000),
                ("matte_qualifier_enabled", 1),
                ("matte_hue_center_centidegrees", 3_000),
                ("matte_hue_width_centidegrees", 1_500),
                ("matte_hue_softness_centidegrees", 500),
                ("matte_saturation_low_basis_points", 2_000),
                ("matte_saturation_high_basis_points", 9_000),
                ("matte_saturation_softness_basis_points", 500),
                ("matte_luma_low_basis_points", 1_000),
                ("matte_luma_high_basis_points", 8_000),
                ("matte_luma_softness_basis_points", 250),
                ("matte_window0_shape_token", 1),
                ("matte_window0_center_x_basis_points", 3_000),
                ("matte_window0_center_y_basis_points", 7_000),
                ("matte_window0_half_width_basis_points", 1_250),
                ("matte_window0_half_height_basis_points", 2_000),
                ("matte_window0_rotation_centidegrees", 4_500),
                ("matte_window0_feather_basis_points", 1_000),
                ("matte_window0_invert", 1),
            ],
        );
        let bytes = grade_buffer_bytes_for(std::slice::from_ref(&node), None, (64, 36), None)
            .expect("a wheels node with a matte serializes");

        // One record, no curve payload, so the block starts at word 16 and
        // `v11` says so rather than the shader deriving it per kind.
        assert_eq!(grade_header(&bytes, 0), 1);
        assert_eq!(grade_header(&bytes, 1), 16);
        assert_eq!(grade_header(&bytes, 2), 3);
        assert_eq!(grade_header(&bytes, 3), 0);
        assert_eq!(matte_base(&bytes, 0), 16);
        assert_eq!(
            bytes.len(),
            GRADE_HEADER_BYTES + (16 + MATTE_BLOCK_WORDS) * 4
        );

        // The kind's own value words are untouched: `gain_master 1500` is a
        // 1.5 slope on all three channels, unit gamma, zero lift.
        for channel in 0..3 {
            assert_eq!(grade_word(&bytes, GRADE_NODE_VALUE_OFFSET + channel), 1.5);
            assert_eq!(
                grade_word(&bytes, GRADE_NODE_VALUE_OFFSET + 3 + channel),
                0.0
            );
            assert_eq!(
                grade_word(&bytes, GRADE_NODE_VALUE_OFFSET + 6 + channel),
                1.0
            );
        }

        let word = |index: usize| matte_word(&bytes, 0, index);
        assert_eq!(word(0), 1.0, "window_count");
        assert_eq!(word(1), 1.0, "combine_token: intersection");
        assert_eq!(word(2), 1.0, "qualifier_enabled");
        assert_eq!(word(3), 1.0, "matte_invert");
        assert_eq!(word(4), 0.6, "matte_mix 6000 bp");
        // `a = W / H` is host supplied, never sniffed from the texture.
        assert_eq!(word(5), 64.0_f32 / 36.0_f32, "raster_aspect");
        assert_eq!(word(5), 16.0_f32 / 9.0_f32);
        assert_eq!(word(6), 30.0, "hue_center 3000 cd");
        assert_eq!(word(7), 15.0, "hue_width 1500 cd");
        assert_eq!(word(8), 5.0, "hue_softness 500 cd");
        assert_eq!(word(9), 0.2, "sat_low");
        assert_eq!(word(10), 0.9, "sat_high");
        assert_eq!(word(11), 0.05, "sat_softness");
        assert_eq!(word(12), 0.1, "luma_low");
        assert_eq!(word(13), 0.8, "luma_high");
        assert_eq!(word(14), 0.025, "luma_softness");
        assert_eq!(word(15), 0.0, "reserved");

        assert_eq!(MATTE_WINDOW_BASE_WORD, 16);
        assert_eq!(word(16), 1.0, "shape: rect");
        assert_eq!(word(17), 0.3, "cx");
        assert_eq!(word(18), 0.7, "cy");
        assert_eq!(word(19), 0.125, "hw");
        assert_eq!(word(20), 0.2, "hh");
        // cos 45 = sin 45 = sqrt(2)/2 = 0.70710678118654752440, whose nearest
        // f32 is 0x3f3504f3. Solved on the host in f64 and rounded once, so
        // the shader and the CPU reference consume the same constant.
        assert_eq!(word(21).to_bits(), 0x3f35_04f3, "cosT");
        assert_eq!(word(22).to_bits(), 0x3f35_04f3, "sinT");
        assert_eq!(word(21), 0.707_106_77_f32);
        assert_eq!(word(23), 0.1, "feather");
        assert_eq!(word(24), 1.0, "per-window invert");
        for reserved in 25..28 {
            assert_eq!(word(reserved), 0.0, "window 0 reserved word {reserved}");
        }
        // Windows at index >= window_count are written as zeros.
        for index in 28..MATTE_BLOCK_WORDS {
            assert_eq!(word(index), 0.0, "inactive window word {index}");
        }
    }

    /// CC5 2.6 / 3.1: `v11 == 0` whenever the node carries no live matte, and
    /// `technical_lut` can never carry one at all.
    #[test]
    #[allow(clippy::float_cmp)]
    fn matte_offset_is_zero_without_a_live_matte() {
        let plain = wheels(1, &[("gain_master_thousandths", 1_500)]);
        let bytes = grade_buffer_bytes_for(std::slice::from_ref(&plain), None, (64, 36), None)
            .expect("a matte-free wheels node serializes");
        assert_eq!(grade_word(&bytes, GRADE_NODE_MATTE_OFFSET_WORD), 0.0);
        assert_eq!(bytes.len(), GRADE_HEADER_BYTES + 16 * 4);

        // The master switch off ignores every other matte control.
        let mut disabled = with_matte(plain.clone(), &[("matte_window_count", 2)]);
        disabled
            .parameters
            .insert("matte_enabled".to_owned(), ParamValue::Integer(0));
        let disabled_bytes =
            grade_buffer_bytes_for(std::slice::from_ref(&disabled), None, (64, 36), None)
                .expect("a disabled matte serializes");
        assert_eq!(disabled_bytes, bytes, "an inactive matte writes no block");

        // Enabled but selecting everything at full strength is equally
        // inactive: no window, no qualifier, no invert, full mix.
        let vacuous = with_matte(plain, &[("matte_mix_basis_points", 10_000)]);
        let vacuous_bytes =
            grade_buffer_bytes_for(std::slice::from_ref(&vacuous), None, (64, 36), None)
                .expect("a vacuous matte serializes");
        assert_eq!(vacuous_bytes, bytes);

        // `technical_lut` carries no `matte_*` parameter, so a hand-edited
        // file naming one cannot make a source normalization partial.
        let luts = TestLuts::build("cc5-technical-lut-matte", &[lut_b_cube()]);
        let technical = with_matte(
            effect_with(
                7,
                "technical_lut",
                &[("lut_asset_id", 1), ("mix_basis_points", 10_000)],
            ),
            &[("matte_window_count", 1), ("matte_mix_basis_points", 2_500)],
        );
        let technical_bytes = grade_buffer_bytes_for(
            std::slice::from_ref(&technical),
            Some(luts.library()),
            (64, 36),
            None,
        )
        .expect("a technical LUT node serializes against its library");
        assert_eq!(
            grade_header(&technical_bytes, 0),
            1,
            "the LUT node is active"
        );
        assert_eq!(
            grade_word(&technical_bytes, GRADE_NODE_MATTE_OFFSET_WORD),
            0.0,
            "v11 is always zero on a technical_lut record"
        );
        // The node is not excluded either: a matte parameter on a technical
        // LUT is inert, not a way to switch the source normalization off.
        assert!(color_node_inactive_reason(&technical).is_none());
    }

    /// CC5 8.1: a pre-CC5 document renders bit-identically.
    ///
    /// The provable form of "before and after": a stack that stores no
    /// `matte_*` parameter and the same stack carrying a *disabled* matte
    /// serialize to the same bytes and render to the same `to_bits` — because
    /// 2.5.4 makes the shader skip the blend entirely rather than multiplying
    /// by a coverage of 1.
    #[test]
    fn a_pre_cc5_document_renders_bit_identically() {
        let Some(compositor) = fallback() else {
            return;
        };
        let frame = cc5_field_raster();
        let pre_cc5 = vec![
            wheels(
                1,
                &[
                    ("gain_master_thousandths", 1_500),
                    ("lift_master_basis_points", 200),
                ],
            ),
            curves(2, "master", &[(0, 0), (5_000, 6_000), (10_000, 10_000)]),
        ];
        let with_disabled = pre_cc5
            .iter()
            .cloned()
            .map(|effect| {
                let mut effect = with_matte(
                    effect,
                    &[
                        ("matte_window_count", 2),
                        ("matte_qualifier_enabled", 1),
                        ("matte_mix_basis_points", 3_000),
                    ],
                );
                effect
                    .parameters
                    .insert("matte_enabled".to_owned(), ParamValue::Integer(0));
                effect
            })
            .collect::<Vec<_>>();

        let before = grade_buffer_bytes_for(&pre_cc5, None, (64, 36), None).expect("pre-CC5 stack");
        let after =
            grade_buffer_bytes_for(&with_disabled, None, (64, 36), None).expect("disabled matte");
        assert_eq!(before, after, "an inactive matte changes no buffer byte");

        let baseline = render_linear(&compositor, &frame, &pre_cc5);
        let rendered = render_linear(&compositor, &frame, &with_disabled);
        assert_eq!(baseline.len(), 64 * 36 * 4);
        for (index, (a, b)) in baseline.iter().zip(rendered.iter()).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "channel {index} moved: {a} vs {b}"
            );
        }
    }

    /// CC5 9.2.1, the central gate: exactly the pixels inside the window
    /// change, and every pixel outside is `to_bits`-identical to the render
    /// with no node at all.
    #[test]
    fn matte_containment_changes_only_inside_pixels() {
        let Some(compositor) = fallback() else {
            return;
        };
        assert_matte_containment(&compositor);
    }

    fn assert_matte_containment(compositor: &Compositor) {
        let frame = cc5_field_raster();
        // The window is |u.x - 0.5| <= 0.25 and |u.y - 0.5| <= 0.25, which on
        // a 64 x 36 raster of pixel centres is x in 16..=47 (32 columns) and
        // y in 9..=26 (18 rows): 576 of 2304 pixels, 2500 basis points. No
        // pixel centre lies on the boundary.
        let inside = |x: usize, y: usize| (16..=47).contains(&x) && (9..=26).contains(&y);
        let mut expected_inside = 0_usize;
        for y in 0..36 {
            for x in 0..64 {
                if inside(x, y) {
                    expected_inside += 1;
                }
            }
        }
        assert_eq!(expected_inside, 576);
        assert_eq!(2304 - expected_inside, 1728);

        let window = [
            ("matte_window_count", 1),
            ("matte_window0_shape_token", 1),
            ("matte_window0_center_x_basis_points", 5_000),
            ("matte_window0_center_y_basis_points", 5_000),
            ("matte_window0_half_width_basis_points", 2_500),
            ("matte_window0_half_height_basis_points", 2_500),
            ("matte_window0_feather_basis_points", 0),
        ];
        let node = with_matte(wheels(1, &[("gain_master_thousandths", 1_500)]), &window);
        let mut inverted = node.clone();
        inverted
            .parameters
            .insert("matte_invert".to_owned(), ParamValue::Integer(1));

        let baseline = render_linear(compositor, &frame, &[]);
        let matted = render_linear(compositor, &frame, std::slice::from_ref(&node));
        let complement = render_linear(compositor, &frame, std::slice::from_ref(&inverted));

        let mut changed = 0_usize;
        let mut complement_changed = 0_usize;
        for y in 0..36_usize {
            for x in 0..64_usize {
                let pixel = (y * 64 + x) * 4;
                let rgb_changed =
                    (0..3).any(|c| baseline[pixel + c].to_bits() != matted[pixel + c].to_bits());
                let rgb_changed_inv = (0..3)
                    .any(|c| baseline[pixel + c].to_bits() != complement[pixel + c].to_bits());
                if inside(x, y) {
                    assert!(rgb_changed, "inside pixel ({x}, {y}) did not change");
                    for c in 0..3 {
                        assert_eq!(
                            baseline[pixel + c].to_bits(),
                            complement[pixel + c].to_bits(),
                            "inverted matte moved inside pixel ({x}, {y}) channel {c}"
                        );
                    }
                    changed += 1;
                } else {
                    for c in 0..3 {
                        assert_eq!(
                            baseline[pixel + c].to_bits(),
                            matted[pixel + c].to_bits(),
                            "outside pixel ({x}, {y}) channel {c} moved"
                        );
                    }
                    assert!(
                        rgb_changed_inv,
                        "inverted matte left outside pixel ({x}, {y}) unchanged"
                    );
                    complement_changed += 1;
                }
                // CC1 2.2.4: no CC5 code path writes alpha.
                assert_eq!(baseline[pixel + 3].to_bits(), matted[pixel + 3].to_bits());
                assert_eq!(
                    baseline[pixel + 3].to_bits(),
                    complement[pixel + 3].to_bits()
                );
            }
        }
        assert_eq!(changed, 576);
        assert_eq!(complement_changed, 1728);
    }

    /// CC5 2.5.5: at exactly zero coverage the node's transform is not
    /// blended in, so `-0.0` keeps its sign bit and a non-finite node output
    /// cannot poison a pixel the matte never selected.
    #[test]
    fn zero_coverage_is_an_exact_identity() {
        let Some(compositor) = fallback() else {
            return;
        };
        assert_zero_coverage_identity(&compositor);
    }

    fn assert_zero_coverage_identity(compositor: &Compositor) {
        // Red carries `-0.0`, green carries `4.0`, blue a plain negative. A
        // wheels node with slope 16 and power 16 drives `4.0` to a non-finite
        // value through `grade709`, which CC1 2.2.5's no-clamp rule makes
        // reachable, and `x + (node(x) - x) * 0.0` would map every outside
        // pixel of that channel to NaN.
        //
        // The `-0.0` sample rides along, but the assertion below is
        // bit-equality against the *no-node* render rather than a sign claim:
        // the working surface's own upload and sample path normalizes `-0.0`
        // to `+0.0` before the node stack ever sees it, so the sign gate of
        // CC5 2.5.5 belongs to the CPU reference. What this fixture proves is
        // that the matte does not perturb the outside pixel either way.
        let mut pixels = Vec::with_capacity(64 * 36 * 4);
        for _ in 0..(64 * 36) {
            pixels.push(f16::from_f32(-0.0));
            pixels.push(f16::from_f32(4.0));
            pixels.push(f16::from_f32(-0.25));
            pixels.push(f16::from_f32(1.0));
        }
        let frame = WorkingFrame {
            width: 64,
            height: 36,
            pixels: Arc::new(pixels),
        };
        let extreme = [
            ("gain_master_thousandths", 4_000),
            ("gain_red_thousandths", 4_000),
            ("gain_green_thousandths", 4_000),
            ("gain_blue_thousandths", 4_000),
            ("gamma_master_thousandths", 4_000),
            ("gamma_red_thousandths", 4_000),
            ("gamma_green_thousandths", 4_000),
            ("gamma_blue_thousandths", 4_000),
        ];
        let node = with_matte(
            wheels(1, &extreme),
            &[
                ("matte_window_count", 1),
                ("matte_window0_center_x_basis_points", 5_000),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 2_500),
            ],
        );
        let baseline = render_linear(compositor, &frame, &[]);
        let matted = render_linear(compositor, &frame, std::slice::from_ref(&node));
        let inside = |x: usize, y: usize| (16..=47).contains(&x) && (9..=26).contains(&y);
        let mut saw_non_finite = false;
        let mut saw_negative_zero = false;
        for y in 0..36_usize {
            for x in 0..64_usize {
                let pixel = (y * 64 + x) * 4;
                if inside(x, y) {
                    saw_non_finite |= !matted[pixel + 1].is_finite();
                } else {
                    for c in 0..4 {
                        assert_eq!(
                            baseline[pixel + c].to_bits(),
                            matted[pixel + c].to_bits(),
                            "outside pixel ({x}, {y}) channel {c} moved"
                        );
                    }
                    assert_eq!(
                        matted[pixel + 2].to_bits(),
                        (-0.25_f32).to_bits(),
                        "a negative value did not survive outside the matte at ({x}, {y})"
                    );
                    saw_negative_zero = true;
                }
            }
        }
        assert!(
            saw_non_finite,
            "the over-range sample must actually reach a non-finite node output"
        );
        assert!(
            saw_negative_zero,
            "the fixture must exercise outside pixels"
        );
    }

    /// CC5 9.2.2: hand-counted window geometry, read straight off the matte
    /// render at `feather = 0`, so every code is an exact `0` or `255`.
    ///
    /// The rotated case is the aspect gate: without the `a = W / H`
    /// correction a 45 degree rotation on a 16:9 raster shears the window into
    /// a parallelogram and the covered set stops being symmetric under
    /// `(dx, dy) -> (dy, dx)`.
    #[test]
    #[allow(clippy::float_cmp)]
    fn matte_window_geometry_anchors() {
        let Some(compositor) = fallback() else {
            return;
        };
        assert_matte_window_geometry(&compositor);
    }

    fn assert_matte_window_geometry(compositor: &Compositor) {
        let frame = uniform_frame(64, 36, [0.25, 0.5, 0.75]);
        // hw * a = 0.1125 * 16/9 = 0.2 = hh, so the field is isotropic in
        // pixels: 0.2 * 36 = 7.2 px in both directions.
        let square = |extra: &[(&str, i64)]| {
            let mut parameters = vec![
                ("matte_window_count", 1),
                ("matte_window0_center_x_basis_points", 5_000),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 1_125),
                ("matte_window0_half_height_basis_points", 2_000),
                ("matte_window0_feather_basis_points", 0),
            ];
            parameters.extend_from_slice(extra);
            with_matte(
                wheels(1, &[("gain_master_thousandths", 1_500)]),
                &parameters,
            )
        };
        let covered = |coverage: &[u8]| {
            for (index, code) in coverage.iter().enumerate() {
                assert!(
                    *code == 0 || *code == 255,
                    "feather 0 must be an exact step, pixel {index} read {code}"
                );
            }
            coverage
                .iter()
                .enumerate()
                .filter(|(_, code)| **code == 255)
                .map(|(index, _)| (index % 64, index / 64))
                .collect::<Vec<_>>()
        };

        // Rect, rotation 0: |dx| <= 7.2 and |dy| <= 7.2 with dx = x - 31.5 and
        // dy = y - 17.5, so 14 columns x 14 rows = 196 pixels.
        let axis_aligned = render_coverage(
            compositor,
            &frame,
            std::slice::from_ref(&square(&[("matte_window0_shape_token", 1)])),
            1,
        )
        .expect("axis-aligned rect coverage");
        let axis_set = covered(&axis_aligned);
        assert_eq!(axis_set.len(), 196);

        // Rect, rotation 45 degrees: |dx + dy| <= 7.2 * sqrt(2) = 10.18234 and
        // |dy - dx| <= 10.18234. Both sums are integers, so the condition is
        // |s| <= 10, |t| <= 10, s + t odd: 11 * 10 + 10 * 11 = 220 pixels.
        let rotated = render_coverage(
            compositor,
            &frame,
            std::slice::from_ref(&square(&[
                ("matte_window0_shape_token", 1),
                ("matte_window0_rotation_centidegrees", 4_500),
            ])),
            1,
        )
        .expect("rotated rect coverage");
        let rotated_set = covered(&rotated);
        assert_eq!(rotated_set.len(), 220);
        // Symmetric under (dx, dy) -> (dy, dx), i.e. (x, y) -> (y + 14, x - 14).
        for (x, y) in &rotated_set {
            let mirrored = (y + 14, x.wrapping_sub(14));
            assert!(
                rotated_set.contains(&mirrored),
                "the rotated window is not symmetric at ({x}, {y}); the aspect \
                 correction was not applied"
            );
        }

        // Ellipse, rotation 0: dx^2 + dy^2 <= 0.04 * 36^2 = 51.84. Counted per
        // quadrant 7 + 7 + 7 + 6 + 6 + 5 + 3 = 41, so 4 * 41 = 164 pixels.
        // (2i+1)^2 + (2j+1)^2 = 207.36 has no integer solution, so no pixel
        // centre lies on the boundary; the smallest interior margin is 1.34
        // px^2 and the smallest exterior margin 2.66 px^2.
        let ellipse = render_coverage(
            compositor,
            &frame,
            std::slice::from_ref(&square(&[("matte_window0_shape_token", 2)])),
            1,
        )
        .expect("ellipse coverage");
        let ellipse_set = covered(&ellipse);
        assert_eq!(ellipse_set.len(), 164);
        let min_x = ellipse_set.iter().map(|(x, _)| *x).min().expect("coverage");
        let max_x = ellipse_set.iter().map(|(x, _)| *x).max().expect("coverage");
        let min_y = ellipse_set.iter().map(|(_, y)| *y).min().expect("coverage");
        let max_y = ellipse_set.iter().map(|(_, y)| *y).max().expect("coverage");
        assert_eq!((max_x - min_x + 1, max_y - min_y + 1), (14, 14));
        // Circular in pixels: `hw * a` and `hh` are the same f32 bit pattern.
        let aspect = 64.0_f32 / 36.0_f32;
        assert_eq!((0.1125_f32 * aspect).to_bits(), 0.2_f32.to_bits());
        assert_eq!(
            (0.1125_f32 * (16.0_f32 / 9.0_f32)).to_bits(),
            0.2_f32.to_bits()
        );
        assert_eq!(
            (0.1125_f32 * (1920.0_f32 / 1080.0_f32)).to_bits(),
            0.2_f32.to_bits()
        );
    }

    /// CC5 9.2.3: the feather is `1 - smoothstep(1 - f, 1 + f, D)`, so the
    /// band straddles the edge and `w = 0.5` exactly at `D = 1`.
    ///
    /// The raster is 40 wide, where `(x + 0.5) * 10000 / 40 = 250x + 125` is
    /// an integer, so `D = |250x + 125 - cx| / hw` lands on exact tenths. With
    /// `cx = 5125` and `hw = 2500`, `D = |x - 20| / 10`.
    #[test]
    fn matte_feather_coverage_codes() {
        let Some(compositor) = fallback() else {
            return;
        };
        assert_matte_feather_codes(&compositor);
    }

    fn assert_matte_feather_codes(compositor: &Compositor) {
        let frame = uniform_frame(40, 8, [0.25, 0.5, 0.75]);
        let node = with_matte(
            wheels(1, &[("gain_master_thousandths", 1_500)]),
            &[
                ("matte_window_count", 1),
                ("matte_window0_shape_token", 1),
                ("matte_window0_center_x_basis_points", 5_125),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 10_000),
                ("matte_window0_feather_basis_points", 4_000),
            ],
        );
        let coverage = render_coverage(compositor, &frame, std::slice::from_ref(&node), 1)
            .expect("feathered coverage");
        assert_eq!(coverage.len(), 40 * 8);
        let code_at = |x: usize| {
            let column = (0..8).map(|y| coverage[y * 40 + x]).collect::<Vec<_>>();
            for code in &column {
                assert_eq!(*code, column[0], "column {x} is not constant: {column:?}");
            }
            column[0]
        };
        // f = 0.4, so t = (D - 0.6) / 0.8.
        // D = 0.8: t = 0.25, smoothstep = 0.0625 * 2.5 = 0.15625,
        //          w = 0.84375, round(255 * w) = round(215.15625) = 215.
        assert_eq!(code_at(12), 215);
        assert_eq!(code_at(28), 215);
        // D = 1.0: t = 0.5, smoothstep = 0.25 * 2.0 = 0.5, w = 0.5,
        //          round(255 * 0.5) = round(127.5) = 128 (half away from zero).
        assert_eq!(code_at(10), 128);
        assert_eq!(code_at(30), 128);
        // D = 1.2: t = 0.75, smoothstep = 0.5625 * 1.5 = 0.84375,
        //          w = 0.15625, round(255 * w) = round(39.84375) = 40.
        assert_eq!(code_at(8), 40);
        assert_eq!(code_at(32), 40);
        // The affected set is exactly {D < 1.4}: D = 1.4 gives t = 1 and w = 0.
        assert_eq!(code_at(6), 0);
        assert_eq!(code_at(34), 0);
        assert_eq!(code_at(7), 11, "D = 1.3 gives w = 0.04296875");
        for x in 0..6 {
            assert_eq!(code_at(x), 0, "column {x} lies beyond the band");
        }
        for x in 35..40 {
            assert_eq!(code_at(x), 0, "column {x} lies beyond the band");
        }
        // The interior of the band is saturated and the symmetry
        // w(1 - d) + w(1 + d) = 1 holds on the sampled pairs.
        assert_eq!(code_at(20), 255, "D = 0 is fully covered");
        assert_eq!(u16::from(code_at(12)) + u16::from(code_at(8)), 255);

        // `feather = 0` takes the hard branch and yields exact 0 / 255.
        let mut hard = node.clone();
        hard.parameters.insert(
            "matte_window0_feather_basis_points".to_owned(),
            ParamValue::Integer(0),
        );
        let hard_coverage = render_coverage(compositor, &frame, std::slice::from_ref(&hard), 1)
            .expect("hard-edged coverage");
        for (index, code) in hard_coverage.iter().enumerate() {
            assert!(
                *code == 0 || *code == 255,
                "feather 0 must be an exact step, pixel {index} read {code}"
            );
        }
    }

    /// CC5 9.2.5: qualifier anchors, hand-computed from encoded triples and
    /// fed to the compositor as `grade709_decode(e)`.
    #[test]
    fn matte_qualifier_anchors() {
        let Some(compositor) = fallback() else {
            return;
        };
        assert_matte_qualifier_anchors(&compositor);
    }

    #[allow(clippy::too_many_lines)]
    fn assert_matte_qualifier_anchors(compositor: &Compositor) {
        let qualifier = |extra: &[(&str, i64)]| {
            let mut parameters = vec![
                ("matte_qualifier_enabled", 1),
                ("matte_window_count", 0),
                ("matte_hue_center_centidegrees", 0),
                ("matte_hue_width_centidegrees", 3_000),
                ("matte_hue_softness_centidegrees", 0),
                ("matte_saturation_low_basis_points", 7_000),
                ("matte_saturation_high_basis_points", 8_000),
                ("matte_saturation_softness_basis_points", 0),
                ("matte_luma_low_basis_points", 3_000),
                ("matte_luma_high_basis_points", 3_500),
                ("matte_luma_softness_basis_points", 0),
            ];
            parameters.extend_from_slice(extra);
            with_matte(
                wheels(1, &[("gain_master_thousandths", 1_500)]),
                &parameters,
            )
        };
        let uniform = |e: [f64; 3]| {
            uniform_frame(
                16,
                9,
                [
                    cc5_grade709_decode(e[0]),
                    cc5_grade709_decode(e[1]),
                    cc5_grade709_decode(e[2]),
                ],
            )
        };
        let all = |coverage: &[u8], code: u8| {
            assert_eq!(coverage.len(), 16 * 9);
            for (index, actual) in coverage.iter().enumerate() {
                assert_eq!(*actual, code, "pixel {index}");
            }
        };

        // e = (0.8, 0.2, 0.2): M = c.r, C = 0.6, S = C / M = 0.75,
        // Y = 0.2126*0.8 + 0.7152*0.2 + 0.0722*0.2 = 0.32756, H = 0 degrees.
        // Hue |0 - 0| = 0 <= 30, S inside 0.70..0.80, Y inside 0.30..0.35, so
        // q = 1 at every pixel.
        let selected = qualifier(&[]);
        all(
            &render_coverage(
                compositor,
                &uniform([0.8, 0.2, 0.2]),
                std::slice::from_ref(&selected),
                1,
            )
            .expect("qualifier coverage"),
            255,
        );
        // Move the saturation band off 0.75 and the same pixel is rejected.
        all(
            &render_coverage(
                compositor,
                &uniform([0.8, 0.2, 0.2]),
                std::slice::from_ref(&qualifier(&[
                    ("matte_saturation_low_basis_points", 8_000),
                    ("matte_saturation_high_basis_points", 10_000),
                ])),
                1,
            )
            .expect("qualifier coverage"),
            0,
        );
        // Hue: e = (0.2, 0.8, 0.2) has M = c.g, so H = 60 * ((b - r)/C + 2)
        // = 120 degrees, which is 120 away from a 30 degree half-width at 0.
        all(
            &render_coverage(
                compositor,
                &uniform([0.2, 0.8, 0.2]),
                std::slice::from_ref(&qualifier(&[
                    ("matte_saturation_low_basis_points", 0),
                    ("matte_saturation_high_basis_points", 10_000),
                    ("matte_luma_low_basis_points", 0),
                    ("matte_luma_high_basis_points", 10_000),
                ])),
                1,
            )
            .expect("qualifier coverage"),
            0,
        );
        // CC5 2.4's achromatic rule, both branches. e = (0.5, 0.5, 0.5) has
        // C = 0 exactly, so a named hue excludes it and a 180 degree
        // half-width includes it.
        let achromatic_bands = [
            ("matte_saturation_low_basis_points", 0),
            ("matte_saturation_high_basis_points", 10_000),
            ("matte_luma_low_basis_points", 0),
            ("matte_luma_high_basis_points", 10_000),
        ];
        let mut named_hue = achromatic_bands.to_vec();
        named_hue.push(("matte_hue_width_centidegrees", 3_000));
        all(
            &render_coverage(
                compositor,
                &uniform([0.5, 0.5, 0.5]),
                std::slice::from_ref(&qualifier(&named_hue)),
                1,
            )
            .expect("qualifier coverage"),
            0,
        );
        let mut hue_off = achromatic_bands.to_vec();
        hue_off.push(("matte_hue_width_centidegrees", 18_000));
        all(
            &render_coverage(
                compositor,
                &uniform([0.5, 0.5, 0.5]),
                std::slice::from_ref(&qualifier(&hue_off)),
                1,
            )
            .expect("qualifier coverage"),
            255,
        );
        // A degenerate resolved band evaluates to 0, with no clamp and no
        // reordering (CC5 2.6).
        let mut degenerate = achromatic_bands.to_vec();
        degenerate.push(("matte_hue_width_centidegrees", 18_000));
        degenerate.push(("matte_saturation_low_basis_points", 9_000));
        degenerate.push(("matte_saturation_high_basis_points", 1_000));
        all(
            &render_coverage(
                compositor,
                &uniform([0.5, 0.5, 0.5]),
                std::slice::from_ref(&qualifier(&degenerate)),
                1,
            )
            .expect("qualifier coverage"),
            0,
        );
    }

    /// CC5 3.2: `header.w` names the node's index among the *active* nodes, so
    /// an inactive node earlier in the stack shifts it. A proof request for an
    /// inactive or matte-free node fails typed rather than selecting a
    /// different node.
    #[test]
    #[allow(clippy::float_cmp)]
    fn matte_debug_selector_resolves_the_active_index() {
        let Some(compositor) = fallback() else {
            return;
        };
        let neutral = wheels(1, &[]);
        let left = with_matte(
            wheels(2, &[("gain_master_thousandths", 1_500)]),
            &[
                ("matte_window_count", 1),
                ("matte_window0_center_x_basis_points", 2_500),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 10_000),
            ],
        );
        let right = with_matte(
            wheels(3, &[("gain_master_thousandths", 1_500)]),
            &[
                ("matte_window_count", 1),
                ("matte_window0_center_x_basis_points", 7_500),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 10_000),
            ],
        );
        let matte_free = wheels(4, &[("gain_master_thousandths", 1_500)]);
        let stack = vec![neutral, left, right, matte_free];

        // The neutral node is inactive and is not written, so the two matted
        // nodes are active 0 and 1.
        assert!(color_node_inactive_reason(&stack[0]).is_some());
        assert_eq!(
            matte_debug_active_index(&stack, ClipId(1), EffectId(2)).expect("left is active"),
            0
        );
        assert_eq!(
            matte_debug_active_index(&stack, ClipId(1), EffectId(3)).expect("right is active"),
            1
        );

        let frame = uniform_frame(64, 36, [0.25, 0.5, 0.75]);
        let coverage = render_coverage(&compositor, &frame, &stack, 3).expect("right coverage");
        // |u.x - 0.75| <= 0.25 is x in 32..=63; the left window would have
        // selected x in 0..=31.
        for y in 0..36_usize {
            for x in 0..64_usize {
                let expected = u8::from(x >= 32) * 255;
                assert_eq!(
                    coverage[y * 64 + x],
                    expected,
                    "pixel ({x}, {y}) selected the wrong node's window"
                );
            }
        }

        // Typed failures, never a blank frame.
        let inactive = render_coverage(&compositor, &frame, &stack, 1)
            .expect_err("an inactive node cannot be proved");
        let MediaError::Backend(message) = inactive else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("matte_proof_node_inactive:"),
            "unexpected message: {message}"
        );
        assert!(message.contains("neutral"), "unexpected message: {message}");
        let no_matte = render_coverage(&compositor, &frame, &stack, 4)
            .expect_err("a matte-free node cannot be proved");
        let MediaError::Backend(message) = no_matte else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("matte_proof_no_matte:"),
            "unexpected message: {message}"
        );
        let missing = render_coverage(&compositor, &frame, &stack, 99)
            .expect_err("an absent effect cannot be proved");
        let MediaError::Backend(message) = missing else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("matte_proof_effect_not_found:"),
            "unexpected message: {message}"
        );
    }

    /// CC5 3.2 / 9.2.13: the layer quad is scaled uniformly in NDC, so its
    /// pixel aspect equals the OUTPUT raster aspect at every `params.scale`.
    ///
    /// The quad spans NDC `[-s, s]` on both axes, which is `s * W` by `s * H`
    /// pixels, so the per-pixel step of the height-normalized offset `d` is
    /// `a / (s * W) = 1 / (s * H)` on x and `1 / (s * H)` on y — isotropic in
    /// pixels for every `s`. A circular window therefore stays circular.
    #[test]
    #[allow(clippy::cast_precision_loss, clippy::naive_bytecount)]
    fn layer_quad_pixel_aspect_equals_the_raster_aspect() {
        let Some(compositor) = fallback() else {
            return;
        };
        let (width, height) = (64_usize, 36_usize);
        let aspect = 64.0_f64 / 36.0_f64;
        for (scale_percent, scale) in [(50_i64, 0.5_f64), (100, 1.0), (200, 2.0)] {
            // The vertex math, restated: quad width and height in pixels.
            let quad_width = scale * 64.0_f64;
            let quad_height = scale * 36.0_f64;
            assert!(
                (quad_width / quad_height - aspect).abs() < 1e-12,
                "quad pixel aspect at scale {scale} is not the raster aspect"
            );

            let frame = uniform_frame(64, 36, [0.25, 0.5, 0.75]);
            let stack = vec![
                effect_with(9, "transform", &[("scale_percent", scale_percent)]),
                with_matte(
                    wheels(1, &[("gain_master_thousandths", 1_500)]),
                    &[
                        ("matte_window_count", 1),
                        ("matte_window0_shape_token", 2),
                        ("matte_window0_center_x_basis_points", 5_000),
                        ("matte_window0_center_y_basis_points", 5_000),
                        ("matte_window0_half_width_basis_points", 1_125),
                        ("matte_window0_half_height_basis_points", 2_000),
                        ("matte_window0_feather_basis_points", 0),
                    ],
                ),
            ];
            let coverage =
                render_coverage(&compositor, &frame, &stack, 1).expect("scaled coverage");

            // Independent f64 transcription of CC5 2.3 for this quad.
            let mut expected = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let dx = (x as f64 - 31.5) / (36.0 * scale);
                    let dy = (y as f64 - 17.5) / (36.0 * scale);
                    if dx * dx + dy * dy <= 0.04 {
                        expected.push((x, y));
                    }
                }
            }
            let observed = coverage
                .iter()
                .enumerate()
                .filter(|(_, code)| **code == 255)
                .map(|(index, _)| (index % width, index / width))
                .collect::<Vec<_>>();
            assert_eq!(observed, expected, "coverage at scale {scale}");
            assert!(!expected.is_empty());
            let min_x = expected.iter().map(|(x, _)| *x).min().expect("coverage");
            let max_x = expected.iter().map(|(x, _)| *x).max().expect("coverage");
            let min_y = expected.iter().map(|(_, y)| *y).min().expect("coverage");
            let max_y = expected.iter().map(|(_, y)| *y).max().expect("coverage");
            assert_eq!(
                max_x - min_x,
                max_y - min_y,
                "the circle is not circular in pixels at scale {scale}"
            );
        }
        // The scale-1 anchor is the hand-counted 164.
        let frame = uniform_frame(64, 36, [0.25, 0.5, 0.75]);
        let coverage = render_coverage(
            &compositor,
            &frame,
            std::slice::from_ref(&with_matte(
                wheels(1, &[("gain_master_thousandths", 1_500)]),
                &[
                    ("matte_window_count", 1),
                    ("matte_window0_shape_token", 2),
                    ("matte_window0_center_x_basis_points", 5_000),
                    ("matte_window0_center_y_basis_points", 5_000),
                    ("matte_window0_half_width_basis_points", 1_125),
                    ("matte_window0_half_height_basis_points", 2_000),
                ],
            )),
            1,
        )
        .expect("unscaled coverage");
        assert_eq!(coverage.iter().filter(|code| **code == 255).count(), 164);
    }

    /// CC5 3.3: every matte-capable kind takes the matte branch, because the
    /// kind dispatch is single-sourced in `apply_node_kind` and both arms of
    /// the `matte_base` test call it.
    #[test]
    fn every_matte_capable_kind_takes_the_matte_branch() {
        for kind in ColorNodeKind::ALL {
            let tag = kind.storage_buffer_tag();
            assert!(
                COMPOSITOR_SHADER_SOURCE.contains(&format!("kind == {tag}u")),
                "compositor.wgsl has no dispatch for {kind:?}"
            );
        }
        // Exactly one dispatch site, reached from both arms.
        assert_eq!(
            COMPOSITOR_SHADER_SOURCE
                .matches("fn apply_node_kind(")
                .count(),
            1
        );
        assert_eq!(
            COMPOSITOR_SHADER_SOURCE
                .matches("apply_node_kind(kind, base, values, corrected)")
                .count(),
            2,
            "the matte branch and the matte-free branch must share one dispatch"
        );
        assert!(COMPOSITOR_SHADER_SOURCE.contains("if matte_base == 0u {"));
        assert!(COMPOSITOR_SHADER_SOURCE.contains("matte_coverage(matte_base, uv, corrected)"));
        // CC5 2.5.5's exact-identity branch is normative, not an optimization.
        assert!(COMPOSITOR_SHADER_SOURCE.contains("if m == 0.0 {"));
        // The debug return happens right after the node stack.
        assert!(COMPOSITOR_SHADER_SOURCE.contains("if grade_buffer.header.w > 0u {"));
    }

    /// The legacy two-argument serializer refuses a matte-carrying stack
    /// rather than inventing a raster aspect for it.
    #[test]
    fn the_matte_free_serializer_refuses_a_matte() {
        let node = with_matte(
            wheels(1, &[("gain_master_thousandths", 1_500)]),
            &[("matte_window_count", 1)],
        );
        let error = grade_buffer_bytes_with_luts(std::slice::from_ref(&node), None)
            .expect_err("a matte needs the raster aspect");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("matte_requires_raster_aspect:"),
            "unexpected message: {message}"
        );
    }

    /// The matte-debug selector is validated against the records this stack
    /// actually wrote.
    #[test]
    fn matte_debug_selector_is_bounds_checked() {
        let node = wheels(1, &[("gain_master_thousandths", 1_500)]);
        let error = grade_buffer_bytes_for(std::slice::from_ref(&node), None, (64, 36), Some(1))
            .expect_err("only one active node was written");
        let MediaError::Backend(message) = error else {
            panic!("expected a backend error");
        };
        assert!(
            message.starts_with("matte_debug_node_out_of_range:"),
            "unexpected message: {message}"
        );
        let bytes = grade_buffer_bytes_for(std::slice::from_ref(&node), None, (64, 36), Some(0))
            .expect("active node 0 exists");
        assert_eq!(grade_header(&bytes, 3), 1, "header.w is index + 1");
    }

    /// The CC5 GPU anchors rerun on a real adapter.
    ///
    /// The default lane is the deterministic software rasterizer; this lane
    /// proves the same hand-derived counts and coverage codes on hardware,
    /// where the distance field, the smoothstep, and the `f16` render target
    /// are a different implementation entirely. Ignored by default because a
    /// physical adapter is not universally available.
    #[test]
    #[ignore = "requires a real (non-fallback) GPU adapter"]
    fn cc5_matte_gpu_anchors_hold_on_hardware() {
        let context = GpuContext::headless(false)
            .expect("CC5 hardware matte evidence requires a non-fallback adapter");
        let metadata = context.monitor_proof_metadata();
        println!(
            "CC_GPU_LANE lane=hardware backend={};adapter={};software_fallback={}",
            metadata.backend, metadata.adapter, metadata.software_fallback
        );
        assert!(
            !metadata.software_fallback,
            "this lane must acquire a real non-CPU adapter; observed {}",
            metadata.adapter
        );
        let compositor = Compositor::new(context);
        assert_matte_containment(&compositor);
        assert_zero_coverage_identity(&compositor);
        assert_matte_window_geometry(&compositor);
        assert_matte_feather_codes(&compositor);
        assert_matte_qualifier_anchors(&compositor);
    }
}
