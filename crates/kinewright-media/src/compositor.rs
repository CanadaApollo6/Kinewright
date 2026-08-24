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
    Effect, EffectParameterDescriptor, EffectUniform, FrameTexture, MediaError,
    MonitorProofMetadata, MonitorProofRenderKind, ParamValue, effect_descriptor,
};

use crate::{
    color_pipeline::{PrimaryCorrection, encode_monitor_rgba8},
    frame::WorkingFrame,
    lut::{CubeLut, parse_cube_lut},
    timeline::TransitionRenderParams,
};

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UNIFORM_FLOATS: usize = 48;
const UNIFORM_SIZE: u64 = UNIFORM_FLOATS as u64 * 4;
const UNIFORM_BYTES: usize = UNIFORM_FLOATS * 4;
const PRIMARY_HEADER_BYTES: usize = 16;
const PRIMARY_NODE_BYTES: usize = 48;

/// The compositor's primary-correction ABI uses one read-only storage buffer
/// in the fragment stage.  Keep this requirement next to the bind-group
/// layout so native device setup cannot accidentally negotiate it away.
pub const COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE: u32 = 1;

/// The smallest primary buffer contains its 16-byte header and one 48-byte
/// neutral node.  A device must be able to bind at least that much storage.
pub const COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE: u64 =
    (PRIMARY_HEADER_BYTES + PRIMARY_NODE_BYTES) as u64;

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

    pub(crate) fn monitor_proof_metadata(&self) -> MonitorProofMetadata {
        MonitorProofMetadata {
            render_kind: MonitorProofRenderKind::GpuPreview,
            backend: self.provenance.backend.clone(),
            adapter: self.provenance.adapter.clone(),
            software_fallback: self.provenance.software_fallback,
            gpu_claim: self.provenance.gpu_claim,
            full_resolution: true,
        }
    }
}

pub struct CompositorLayer<'a, F = FrameTexture> {
    pub frame: &'a F,
    pub effects: &'a [Effect],
    pub transition: TransitionRenderParams,
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
    pipeline: wgpu::RenderPipeline,
    lut_cache: Mutex<HashMap<PathBuf, CachedCubeLut>>,
}

struct LayerResources {
    _texture: wgpu::Texture,
    _lut_texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
    _primary: wgpu::Buffer,
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
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
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
        Self {
            gpu,
            bind_group_layout,
            sampler,
            pipeline,
            lut_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Composite the supplied bottom-to-top layers into one RGBA frame.
    ///
    /// # Errors
    ///
    /// Returns a media error for invalid dimensions or a GPU mapping failure.
    pub fn render<F: CompositorInput>(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_, F>],
    ) -> Result<FrameTexture, MediaError> {
        let (width, height) = resolution;
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
        let resources = layers
            .iter()
            .map(|layer| self.layer_resources(layer))
            .collect::<Result<Vec<_>, _>>()?;
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
        self.readback(width, height, &output, encoder)
    }

    #[allow(clippy::too_many_lines)]
    fn layer_resources<F: CompositorInput>(
        &self,
        layer: &CompositorLayer<'_, F>,
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
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright compositor source"),
            size: wgpu::Extent3d {
                width: layer.frame.width(),
                height: layer.frame.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
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
        let (cube_lut, external_lut_enabled) = self.cube_lut(layer.effects)?;
        let lut_texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright external 3D LUT"),
            size: wgpu::Extent3d {
                width: cube_lut.size,
                height: cube_lut.size,
                depth_or_array_layers: cube_lut.size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lut_bytes = cube_lut
            .rgba
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect::<Vec<_>>();
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &lut_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lut_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cube_lut.size.saturating_mul(16)),
                rows_per_image: Some(cube_lut.size),
            },
            wgpu::Extent3d {
                width: cube_lut.size,
                height: cube_lut.size,
                depth_or_array_layers: cube_lut.size,
            },
        );
        let mut params = params_for(layer.effects, layer.transition);
        params.external_lut_enabled = if external_lut_enabled { 1.0 } else { 0.0 };
        params.external_domain_min_r = cube_lut.domain_min[0];
        params.external_domain_min_g = cube_lut.domain_min[1];
        params.external_domain_min_b = cube_lut.domain_min[2];
        params.external_domain_max_r = cube_lut.domain_max[0];
        params.external_domain_max_g = cube_lut.domain_max[1];
        params.external_domain_max_b = cube_lut.domain_max[2];
        params.input_linear = f32::from(F::LINEAR);
        params.legacy_stage_active = if legacy_stage_active(layer.effects) {
            1.0
        } else {
            0.0
        };
        let primary_bytes = primary_buffer_bytes(layer.effects)?;
        let primary = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright primary correction nodes"),
            size: u64::try_from(primary_bytes.len()).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&primary, 0, &primary_bytes);
        let uniform = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright compositor layer parameters"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&uniform, 0, &params.as_bytes());
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let lut_view = lut_texture.create_view(&wgpu::TextureViewDescriptor::default());
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
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&lut_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: primary.as_entire_binding(),
                    },
                ],
            });
        Ok(LayerResources {
            _texture: texture,
            _lut_texture: lut_texture,
            _uniform: uniform,
            _primary: primary,
            bind_group,
        })
    }

    fn cube_lut(&self, effects: &[Effect]) -> Result<(Arc<CubeLut>, bool), MediaError> {
        let Some(effect) = effects
            .iter()
            .rev()
            .find(|effect| effect.name == "cube_lut")
        else {
            return Ok((Arc::new(CubeLut::identity()), false));
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

    fn readback(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
    ) -> Result<FrameTexture, MediaError> {
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
        let mut rgba = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default())
                .saturating_mul(4),
        );
        for row in 0..usize::try_from(height).unwrap_or_default() {
            let start = row.saturating_mul(usize::try_from(padded_row_bytes).unwrap_or_default());
            let end = start.saturating_add(usize::try_from(row_bytes).unwrap_or_default());
            for pixel in mapped[start..end].as_chunks::<8>().0 {
                let linear = [
                    f16::from_le_bytes([pixel[0], pixel[1]]).to_f32(),
                    f16::from_le_bytes([pixel[2], pixel[3]]).to_f32(),
                    f16::from_le_bytes([pixel[4], pixel[5]]).to_f32(),
                    f16::from_le_bytes([pixel[6], pixel[7]]).to_f32(),
                ];
                rgba.extend_from_slice(&encode_monitor_rgba8(linear));
            }
        }
        drop(mapped);
        buffer.unmap();
        Ok(FrameTexture {
            width,
            height,
            rgba: Arc::new(rgba),
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
        let (width, height) = resolution;
        if width == 0 || height == 0 {
            return Err(MediaError::Backend(
                "compositor output resolution must be non-zero".to_owned(),
            ));
        }
        let output = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kinewright compositor working evidence output"),
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
        let resources = layers
            .iter()
            .map(|layer| self.layer_resources(layer))
            .collect::<Result<Vec<_>, _>>()?;
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Kinewright compositor working evidence commands"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Kinewright compositor working evidence pass"),
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
        self.readback_working(width, height, &output, encoder)
    }

    #[cfg(test)]
    fn readback_working(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
    ) -> Result<Vec<f32>, MediaError> {
        let row_bytes = width.saturating_mul(8);
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_size = u64::from(padded_row_bytes).saturating_mul(u64::from(height));
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinewright compositor working evidence readback"),
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
        let mut values = Vec::with_capacity(
            usize::try_from(width)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default())
                .saturating_mul(4),
        );
        for row in 0..usize::try_from(height).unwrap_or_default() {
            let start = row.saturating_mul(usize::try_from(padded_row_bytes).unwrap_or_default());
            let end = start.saturating_add(usize::try_from(row_bytes).unwrap_or_default());
            for pixel in mapped[start..end].as_chunks::<8>().0 {
                values.extend([
                    f16::from_le_bytes([pixel[0], pixel[1]]).to_f32(),
                    f16::from_le_bytes([pixel[2], pixel[3]]).to_f32(),
                    f16::from_le_bytes([pixel[4], pixel[5]]).to_f32(),
                    f16::from_le_bytes([pixel[6], pixel[7]]).to_f32(),
                ]);
            }
        }
        drop(mapped);
        buffer.unmap();
        Ok(values)
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
            0.0,
            0.0,
        ];
        let mut bytes = [0_u8; UNIFORM_BYTES];
        for (index, value) in values.into_iter().enumerate() {
            let start = index * 4;
            bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

/// Serialize primary nodes into a storage buffer without collapsing adjacent
/// nodes. The order in the document's effect vector is the execution order in
/// the shader. A single neutral node keeps the storage binding valid when no
/// primary effect is present.
#[allow(clippy::cast_precision_loss)]
fn primary_buffer_bytes(effects: &[Effect]) -> Result<Vec<u8>, MediaError> {
    let mut corrections = Vec::new();
    for effect in effects
        .iter()
        .filter(|effect| effect.name == "primary_correction")
    {
        corrections.push(PrimaryCorrection::from_effect(effect).map_err(|error| {
            MediaError::Backend(format!("managed primary correction failed: {error}"))
        })?);
    }
    let count = u32::try_from(corrections.len()).map_err(|_| {
        MediaError::Backend("too many primary correction nodes for one compositor layer".to_owned())
    })?;
    // `PrimaryBuffer` is a storage struct with a 16-byte `vec4<u32>` header
    // followed by tightly packed `PrimaryNode` values.  Each node contains
    // three `vec4<f32>` values, so its WGSL array stride is 48 bytes.
    let node_count = corrections.len().max(1);
    let mut bytes = vec![
        0_u8;
        PRIMARY_HEADER_BYTES
            .saturating_add(node_count.saturating_mul(PRIMARY_NODE_BYTES))
    ];
    bytes[0..4].copy_from_slice(&count.to_le_bytes());
    for (node_index, correction) in corrections.iter().enumerate() {
        let values = [
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
        ];
        let offset =
            PRIMARY_HEADER_BYTES.saturating_add(node_index.saturating_mul(PRIMARY_NODE_BYTES));
        for (index, value) in values.into_iter().enumerate() {
            let start = offset.saturating_add(index.saturating_mul(4));
            bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
        }
    }
    Ok(bytes)
}

fn legacy_stage_active(effects: &[Effect]) -> bool {
    effects.iter().any(|effect| {
        matches!(
            effect.name.as_str(),
            "color_grade"
                | "brightness"
                | "contrast"
                | "saturation"
                | "look_lut"
                | "cube_lut"
                | "chroma_key"
        )
    })
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
                EffectUniform::Exposure => params.exposure += value / 1_000.0,
                EffectUniform::Temperature => params.temperature += value / 100.0,
                EffectUniform::Tint => params.tint += value / 100.0,
                // CC1 primary nodes are serialized separately and executed
                // in order by the shader's storage-buffer loop. They must
                // never be flattened into the legacy display controls.
                EffectUniform::PrimaryExposure
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
                | EffectUniform::DuckRelease => {}
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
    params.exposure = params.exposure.clamp(-5.0, 5.0);
    params.temperature = params.temperature.clamp(-1.0, 1.0);
    params.tint = params.tint.clamp(-1.0, 1.0);
    params.lut_intensity = params.lut_intensity.clamp(0.0, 1.0);
    params.external_lut_intensity = params.external_lut_intensity.clamp(0.0, 1.0);
    params.mask_center_x = params.mask_center_x.clamp(0.0, 1.0);
    params.mask_center_y = params.mask_center_y.clamp(0.0, 1.0);
    params.mask_width = params.mask_width.clamp(0.01, 2.0);
    params.mask_height = params.mask_height.clamp(0.01, 2.0);
    params.mask_feather = params.mask_feather.clamp(0.0, 1.0);
    params
}

// Registered values are bounded to small integers that are exactly representable as f32.
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

    use kinewright_core::{EffectId, ParamValue, Title};

    use super::*;

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
    /// real adapter when no fallback exists (developer machines); skip only
    /// when the environment has no usable adapter at all. The pixel assertions
    /// carry tolerances, so hardware adapters remain valid test targets.
    fn fallback() -> Option<Compositor> {
        let gpu = GpuContext::headless(true)
            .or_else(|_| GpuContext::headless(false))
            .ok()?;
        Some(Compositor::new(gpu))
    }

    #[test]
    fn compositor_limit_contract_requires_its_fragment_storage_buffer() {
        let limits = compositor_required_limits(wgpu::Limits::downlevel_webgl2_defaults());
        assert_eq!(
            limits.max_storage_buffers_per_shader_stage,
            COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE
        );
        assert_eq!(
            limits.max_storage_buffer_binding_size,
            COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE
        );
    }

    #[test]
    fn headless_context_preserves_adapter_provenance() {
        let Some(gpu) = GpuContext::headless(true)
            .or_else(|_| GpuContext::headless(false))
            .ok()
        else {
            eprintln!("skipped: no usable wgpu adapter in this environment");
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

    fn primary_f32(bytes: &[u8], offset: usize) -> f32 {
        let end = offset.saturating_add(std::mem::size_of::<f32>());
        f32::from_ne_bytes(bytes[offset..end].try_into().expect("f32-aligned bytes"))
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

    #[test]
    fn primary_buffer_matches_wgsl_header_and_tight_node_stride() {
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
        let bytes = primary_buffer_bytes(&[first, second]).expect("valid primary nodes");

        assert_eq!(bytes.len(), PRIMARY_HEADER_BYTES + 2 * PRIMARY_NODE_BYTES);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 2);
        assert!(bytes[4..PRIMARY_HEADER_BYTES].iter().all(|byte| *byte == 0));

        let first_values = [
            1.0, 0.25, -0.1, -0.2, 0.42, 0.3, -0.4, 0.5, -0.6, 0.7, 0.0, 0.0,
        ];
        let second_values = [-1.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, -0.7, 0.0, 0.0];
        for (index, expected) in first_values.into_iter().enumerate() {
            assert!(
                (primary_f32(&bytes, PRIMARY_HEADER_BYTES + index * 4) - expected).abs() < 1e-6
            );
        }
        for (index, expected) in second_values.into_iter().enumerate() {
            assert!(
                (primary_f32(
                    &bytes,
                    PRIMARY_HEADER_BYTES + PRIMARY_NODE_BYTES + index * 4
                ) - expected)
                    .abs()
                    < 1e-6
            );
        }
    }

    #[test]
    fn primary_correction_nodes_execute_in_order_on_gpu() {
        let Some(compositor) = fallback() else {
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
    fn color_grade_and_look_lut_execute_in_the_shared_compositor() {
        let Some(compositor) = fallback() else {
            eprintln!("skipped: no usable wgpu adapter in this environment");
            return;
        };
        let gray = solid(4, 4, [64, 64, 64, 255]);
        let exposure = effect_with(1, "color_grade", &[("exposure_milli_stops", 1_000)]);
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
        assert_pixel_close(pixel(&output, 0, 0), [128, 128, 128, 255], 3);

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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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

    #[test]
    fn masks_and_chroma_key_create_real_alpha_for_lower_layers() {
        let Some(compositor) = fallback() else {
            eprintln!("skipped: no usable wgpu adapter in this environment");
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

    #[test]
    fn z_order_and_crossfade_alpha_blend_bottom_to_top() {
        let Some(compositor) = fallback() else {
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
            eprintln!("skipped: no usable wgpu adapter in this environment");
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
}
