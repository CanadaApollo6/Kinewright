use std::{num::NonZeroU64, sync::Arc};

use openreel_core::{Effect, FrameTexture, MediaError, ParamValue};

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const UNIFORM_SIZE: u64 = 8 * 4;

#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    #[must_use]
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue }
    }

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
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("OpenReel compositor device"),
            ..Default::default()
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&descriptor)).map_err(|error| {
                MediaError::Backend(format!("could not create a wgpu device: {error}"))
            })?;
        Ok(Self { device, queue })
    }
}

pub struct CompositorLayer<'a> {
    pub frame: &'a FrameTexture,
    pub effects: &'a [Effect],
    /// Additional clip alpha, normally the transition-in crossfade progress.
    pub transition_alpha: f32,
}

pub struct Compositor {
    gpu: GpuContext,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

struct LayerResources {
    _texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
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
        }
    }
}

impl Compositor {
    #[must_use]
    pub fn new(gpu: GpuContext) -> Self {
        let device = &gpu.device;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("OpenReel compositor layer layout"),
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("OpenReel compositor pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OpenReel compositor shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("OpenReel compositor pipeline"),
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
            label: Some("OpenReel compositor sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            gpu,
            bind_group_layout,
            sampler,
            pipeline,
        }
    }

    pub fn render(
        &self,
        resolution: (u32, u32),
        layers: &[CompositorLayer<'_>],
    ) -> Result<FrameTexture, MediaError> {
        let (width, height) = resolution;
        if width == 0 || height == 0 {
            return Err(MediaError::Backend(
                "compositor output resolution must be non-zero".to_owned(),
            ));
        }
        let output = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("OpenReel compositor output"),
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
                label: Some("OpenReel compositor commands"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("OpenReel composite pass"),
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

    fn layer_resources(&self, layer: &CompositorLayer<'_>) -> Result<LayerResources, MediaError> {
        let expected_len = usize::try_from(layer.frame.width)
            .unwrap_or_default()
            .saturating_mul(usize::try_from(layer.frame.height).unwrap_or_default())
            .saturating_mul(4);
        if layer.frame.rgba.len() != expected_len || expected_len == 0 {
            return Err(MediaError::Backend(
                "invalid compositor input frame".to_owned(),
            ));
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("OpenReel compositor source"),
            size: wgpu::Extent3d {
                width: layer.frame.width,
                height: layer.frame.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
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
            layer.frame.rgba.as_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layer.frame.width.saturating_mul(4)),
                rows_per_image: Some(layer.frame.height),
            },
            wgpu::Extent3d {
                width: layer.frame.width,
                height: layer.frame.height,
                depth_or_array_layers: 1,
            },
        );
        let params = params_for(layer.effects, layer.transition_alpha);
        let uniform = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("OpenReel compositor layer parameters"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&uniform, 0, &params.as_bytes());
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("OpenReel compositor layer bindings"),
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
                ],
            });
        Ok(LayerResources {
            _texture: texture,
            _uniform: uniform,
            bind_group,
        })
    }

    fn readback(
        &self,
        width: u32,
        height: u32,
        output: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
    ) -> Result<FrameTexture, MediaError> {
        let row_bytes = width.saturating_mul(4);
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_size = u64::from(padded_row_bytes).saturating_mul(u64::from(height));
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("OpenReel compositor readback"),
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
            usize::try_from(row_bytes)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(height).unwrap_or_default()),
        );
        for row in 0..usize::try_from(height).unwrap_or_default() {
            let start = row.saturating_mul(usize::try_from(padded_row_bytes).unwrap_or_default());
            let end = start.saturating_add(usize::try_from(row_bytes).unwrap_or_default());
            rgba.extend_from_slice(&mapped[start..end]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(FrameTexture {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }
}

impl LayerParams {
    fn as_bytes(self) -> [u8; UNIFORM_SIZE as usize] {
        let values = [
            self.brightness,
            self.contrast,
            self.saturation,
            self.opacity,
            self.scale,
            self.offset_x,
            self.offset_y,
            0.0,
        ];
        let mut bytes = [0_u8; UNIFORM_SIZE as usize];
        for (index, value) in values.into_iter().enumerate() {
            let start = index * 4;
            bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

fn params_for(effects: &[Effect], transition_alpha: f32) -> LayerParams {
    let mut params = LayerParams {
        opacity: transition_alpha.clamp(0.0, 1.0),
        ..Default::default()
    };
    for effect in effects {
        match effect.name.as_str() {
            "brightness" => params.brightness += percent(effect, "percent", 0) / 100.0,
            "contrast" => params.contrast *= 1.0 + percent(effect, "percent", 0) / 100.0,
            "saturation" => params.saturation *= 1.0 + percent(effect, "percent", 0) / 100.0,
            "opacity" => params.opacity *= percent(effect, "percent", 100) / 100.0,
            "transform" => {
                params.scale *= percent(effect, "scale_percent", 100) / 100.0;
                params.offset_x += percent(effect, "x_percent", 0) / 50.0;
                params.offset_y += percent(effect, "y_percent", 0) / 50.0;
            }
            _ => {}
        }
    }
    params
}

fn percent(effect: &Effect, name: &str, default: i64) -> f32 {
    match effect.parameters.get(name) {
        Some(ParamValue::Integer(value)) => *value as f32,
        _ => default as f32,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openreel_core::{EffectId, ParamValue};

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
        }
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
                    transition_alpha: 1.0,
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
                    transition_alpha: 1.0,
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
                    transition_alpha: 1.0,
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
                    transition_alpha: 1.0,
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [128, 0, 0, 255], 2);

        let transform = effect(4, "transform", "scale_percent", 50);
        let output = compositor
            .render(
                (4, 4),
                &[CompositorLayer {
                    frame: &red,
                    effects: &[transform],
                    transition_alpha: 1.0,
                }],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [0, 0, 0, 255], 2);
        let center = (2 * 4 + 2) * 4;
        assert_pixel_close(&output.rgba[center..center + 4], [255, 0, 0, 255], 2);
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
                        transition_alpha: 1.0,
                    },
                    CompositorLayer {
                        frame: &blue,
                        effects: &[],
                        transition_alpha: 0.5,
                    },
                ],
            )
            .unwrap();
        assert_pixel_close(&output.rgba[0..4], [128, 0, 128, 255], 2);
    }
}
