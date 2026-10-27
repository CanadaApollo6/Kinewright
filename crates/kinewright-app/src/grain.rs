//! Signed grain uses add and reverse-subtract draws so dark surfaces keep their mean.
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::Arc;
pub(crate) struct Grain(Arc<Resources>);
struct Resources {
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    add: wgpu::RenderPipeline,
    subtract: wgpu::RenderPipeline,
}
impl Grain {
    #[allow(
        clippy::too_many_lines,
        reason = "one GPU resource setup keeps shader and blend states together"
    )]
    pub(crate) fn new(state: &egui_wgpu::RenderState, pixels: &[u8]) -> Self {
        let device = &state.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PO0 zero-mean grain tile"),
            size: wgpu::Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        state.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(512),
                rows_per_image: Some(128),
            },
            texture.size(),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PO0 grain"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PO0 grain"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            }],
        });
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("PO0 signed grain"),source:wgpu::ShaderSource::Wgsl(r"
@group(0) @binding(0) var noise: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
 let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
 return vec4(p[i],0.0,1.0);
}
fn value(p:vec4<f32>)->f32 {
 let xy=vec2<i32>(p.xy)%128;
 return (textureLoad(noise,xy,0).r-0.5)*0.011;
}
@fragment fn positive(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {return vec4(vec3(max(value(p),0.0)),0.0);}
@fragment fn negative(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {return vec4(vec3(max(-value(p),0.0)),0.0);}
".into())});
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PO0 grain"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, operation| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("PO0 grain"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: state.target_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self(Arc::new(Resources {
            _texture: texture,
            bind,
            add: pipeline("positive", wgpu::BlendOperation::Add),
            subtract: pipeline("negative", wgpu::BlendOperation::ReverseSubtract),
        }))
    }
    pub(crate) fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Draw(Arc::clone(&self.0)),
        ));
    }
}
struct Draw(Arc<Resources>);
impl egui_wgpu::CallbackTrait for Draw {
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _: &egui_wgpu::CallbackResources,
    ) {
        pass.set_bind_group(0, &self.0.bind, &[]);
        pass.set_pipeline(&self.0.add);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.0.subtract);
        pass.draw(0..3, 0..1);
    }
}
