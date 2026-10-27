//! PO0 tokens and interruptible motion. Callers enter only through the opt-in probe.
use eframe::{egui, wgpu};

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) enum Token {
    Instant,
    Micro,
    Quick,
    Standard,
    Ambient,
    Spring,
    Panel,
    Flight,
    Landing,
    Mark,
    RowDelay,
    RowExit,
    RowFade,
    Ring,
    Fade,
    Badge,
    MarkFade,
    LabelExit,
    LabelRise,
}
impl Token {
    pub(crate) const fn seconds(self) -> f32 {
        match self {
            Self::Instant => 0.0,
            Self::Micro => 0.090,
            Self::Quick => 0.160,
            Self::Standard => 0.220,
            Self::Ambient => 0.420,
            Self::Spring => 0.488,
            Self::Panel => 0.514,
            Self::Flight => 0.640,
            Self::Mark => 0.580,
            Self::Landing => 0.520,
            Self::RowDelay => 0.040,
            Self::RowExit => 0.120,
            Self::RowFade | Self::LabelExit => 0.080,
            Self::Ring => 0.320,
            Self::Fade => 0.100,
            Self::Badge => 0.900,
            Self::MarkFade => 0.200,
            Self::LabelRise => 0.240,
        }
    }
    pub(crate) fn curve(self, t: f32) -> f32 {
        match self {
            Self::Micro | Self::LabelRise => ease_out(t),
            Self::Quick => ease_in(t),
            Self::Mark => bezier(t, [0.45, 0.05, 0.25, 1.0]),
            _ => ease_in_out(t),
        }
    }
}

fn bezier(x: f32, [x1, y1, x2, y2]: [f32; 4]) -> f32 {
    let sample = |t: f32, a: f32, b: f32| {
        3.0 * (1.0 - t).powi(2) * t * a + 3.0 * (1.0 - t) * t * t * b + t.powi(3)
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..20 {
        let t = f32::midpoint(lo, hi);
        if sample(t, x1, x2) < x.clamp(0.0, 1.0) {
            lo = t;
        } else {
            hi = t;
        }
    }
    sample(f32::midpoint(lo, hi), y1, y2)
}
pub(crate) fn ease_out(t: f32) -> f32 {
    bezier(t, [0.16, 1.0, 0.3, 1.0])
}
pub(crate) fn ease_in(t: f32) -> f32 {
    bezier(t, [0.4, 0.0, 1.0, 1.0])
}
pub(crate) fn ease_in_out(t: f32) -> f32 {
    bezier(t, [0.4, 0.0, 0.2, 1.0])
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Spring {
    pub(crate) value: f32,
    pub(crate) velocity: f32,
    target: f32,
    damping: f32,
}
impl Spring {
    pub(crate) const fn new(value: f32, panel: bool) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            damping: if panel { 1.0 } else { 0.85 },
        }
    }
    pub(crate) fn target(&mut self, target: f32) {
        self.target = target;
    }
    pub(crate) fn active(self) -> bool {
        (self.value - self.target).abs() >= 0.25 || self.velocity.abs() >= 1.0
    }
    pub(crate) fn step(&mut self, dt: f32) -> bool {
        if !self.active() {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        let omega = std::f32::consts::TAU / 0.35;
        let mut remaining = dt.max(0.0);
        // A suspended window has passed the entire settling window.
        if remaining > 2.0 {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        while remaining > 0.0 {
            let h = remaining.min(0.00025);
            self.velocity += (omega * omega * (self.target - self.value)
                - 2.0 * self.damping * omega * self.velocity)
                * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if !self.active() {
            self.value = self.target;
            self.velocity = 0.0;
        }
        self.active()
    }
}

#[derive(Clone, Copy)]
struct Animation {
    spring: Spring,
    from: f32,
    target: f32,
    began: f64,
    value: f32,
    pass: u64,
    elapsed: f32,
}
#[derive(Clone, serde::Serialize)]
pub(crate) struct Completion {
    token: Token,
    travel_pt: f32,
    elapsed_ms: f64,
    nominal_ms: f64,
    integrated_ms: Option<f64>,
}
pub(crate) fn take_completions(ctx: &egui::Context) -> Vec<Completion> {
    ctx.data_mut(|data| data.remove_temp::<Vec<Completion>>(egui::Id::new("po0-completions")))
        .unwrap_or_default()
}
#[derive(Clone, Copy, Default)]
pub(crate) struct MotionFrame {
    pub(crate) requests: u32,
    pub(crate) finished: bool,
    pub(crate) light: bool,
    pub(crate) snap_residual: Option<[f32; 2]>,
}
pub(crate) fn begin(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("po0-motion-frame"), MotionFrame::default())
    });
}
pub(crate) fn deadline(ctx: &egui::Context, enabled: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("po0-deadline"), enabled));
}
pub(crate) fn snap_residual(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.data(|data| data.get_temp::<MotionFrame>(egui::Id::new("po0-motion-frame")))
        .and_then(|frame| frame.snap_residual)
}
pub(crate) fn finished(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<MotionFrame>(egui::Id::new("po0-motion-frame")))
        .is_some_and(|frame| frame.finished)
}
pub(crate) fn any_requests(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<MotionFrame>(egui::Id::new("po0-motion-frame")))
        .is_some_and(|frame| frame.requests > 0 || frame.light)
}
pub(crate) fn light_repaint(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<MotionFrame>(egui::Id::new("po0-motion-frame"))
            .light = true;
    });
    ctx.request_repaint();
}
pub(crate) fn requests(ctx: &egui::Context) -> u32 {
    ctx.data(|data| data.get_temp::<MotionFrame>(egui::Id::new("po0-motion-frame")))
        .unwrap_or_default()
        .requests
}
pub(crate) fn repaint(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        let frame = data.get_temp_mut_or_default::<MotionFrame>(egui::Id::new("po0-motion-frame"));
        frame.requests += 1;
    });
    ctx.request_repaint();
}
pub(crate) fn reduced(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(egui::Id::new("po0-reduce-motion")))
        .unwrap_or(false)
}
pub(crate) fn set_reduced(ctx: &egui::Context, value: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("po0-reduce-motion"), value));
}
pub(crate) fn animate(ui: &egui::Ui, id: egui::Id, target: f32, token: Token) -> f32 {
    let ctx = ui.ctx();
    let (time, dt) = ctx.input(|input| (input.time, input.stable_dt));
    let pass = ctx.cumulative_frame_nr();
    let reduced = reduced(ctx);
    let mut state = ctx.data_mut(|data| {
        *data.get_temp_mut_or_insert_with(id, || Animation {
            spring: Spring::new(target, matches!(token, Token::Panel)),
            from: target,
            target,
            began: time,
            value: target,
            pass: pass.wrapping_sub(1),
            elapsed: 0.0,
        })
    });
    if state.target.to_bits() != target.to_bits() {
        state.from = state.value;
        state.target = target;
        state.began = time;
        state.elapsed = 0.0;
        state.spring.target(target);
        state.pass = pass;
    }
    let was_moving =
        state.value.to_bits() != target.to_bits() || state.spring.velocity.abs() >= 1.0;
    let geometry = matches!(token, Token::Spring | Token::Panel | Token::Instant);
    let active = if matches!(token, Token::Instant) || (geometry && reduced) {
        state.spring = Spring::new(target, matches!(token, Token::Panel));
        state.value = target;
        false
    } else if geometry {
        if state.pass != pass {
            let deadline = ctx
                .data(|data| data.get_temp::<bool>(egui::Id::new("po0-deadline")))
                .unwrap_or(false);
            let step = if deadline {
                dt.min((0.520 - state.elapsed).max(0.0))
            } else {
                dt
            };
            state.spring.step(step);
            state.elapsed += step;
            if deadline && state.elapsed >= 0.520 && state.spring.active() {
                let residual = [
                    state.spring.target - state.spring.value,
                    state.spring.velocity,
                ];
                ctx.data_mut(|data| {
                    data.get_temp_mut_or_default::<MotionFrame>(egui::Id::new(
                        "po0-motion-frame",
                    ))
                    .snap_residual = Some(residual);
                });
                state.spring.value = target;
                state.spring.velocity = 0.0;
            }
        }
        state.value = state.spring.value;
        state.spring.active()
    } else {
        let duration = if reduced {
            token.seconds().min(Token::Fade.seconds())
        } else {
            token.seconds()
        };
        #[allow(clippy::cast_possible_truncation)]
        let t = ((time - state.began) / f64::from(duration)) as f32;
        state.value = if t >= 1.0 {
            target
        } else {
            egui::lerp(state.from..=target, token.curve(t))
        };
        t < 1.0
    };
    state.pass = pass;
    ctx.data_mut(|data| data.insert_temp(id, state));
    if active {
        repaint(ctx);
    } else if was_moving {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<MotionFrame>(egui::Id::new("po0-motion-frame"))
                .finished = true;
            let log =
                data.get_temp_mut_or_default::<Vec<Completion>>(egui::Id::new("po0-completions"));
            if log.len() < 256 {
                log.push(Completion {
                    token,
                    travel_pt: (target - state.from).abs(),
                    elapsed_ms: (time - state.began) * 1000.0,
                    nominal_ms: f64::from(token.seconds()) * 1000.0,
                    integrated_ms: geometry.then_some(f64::from(state.elapsed) * 1000.0),
                });
            }
        });
    }
    state.value
}

pub(crate) struct ListRow {
    pub(crate) id: u64,
    pub(crate) removing: bool,
}
pub(crate) struct AnimatedList {
    pub(crate) rows: std::collections::VecDeque<ListRow>,
    pub(crate) queued: u32,
    pending: std::collections::VecDeque<ListRow>,
}
impl AnimatedList {
    pub(crate) fn new() -> Self {
        Self {
            rows: (0..40)
                .map(|id| ListRow {
                    id,
                    removing: false,
                })
                .collect(),
            queued: 0,
            pending: std::collections::VecDeque::new(),
        }
    }
    pub(crate) fn insert(&mut self, id: u64, at_bottom: bool) {
        if !at_bottom {
            self.queued = self.queued.saturating_add(1);
            if self.pending.len() == 40 {
                self.pending.pop_front();
            }
            self.pending.push_back(ListRow {
                id,
                removing: false,
            });
            return;
        }
        self.release();
        self.push(id);
    }
    pub(crate) fn release(&mut self) {
        while let Some(row) = self.pending.pop_front() {
            self.push(row.id);
        }
        self.queued = 0;
    }
    fn push(&mut self, id: u64) {
        if self.rows.len() >= 41 {
            self.rows.pop_front();
        }
        if self.rows.len() == 40
            && let Some(row) = self.rows.front_mut()
        {
            row.removing = true;
        }
        self.rows.push_back(ListRow {
            id,
            removing: false,
        });
    }
}

/// An independent copy, never a second player or a retained display-pool lease.
pub(crate) struct HeldFrame {
    texture: wgpu::Texture,
    pub(crate) id: egui::TextureId,
}
impl HeldFrame {
    pub(crate) fn capture(
        state: &eframe::egui_wgpu::RenderState,
        source: &wgpu::TextureView,
    ) -> Self {
        let source = source.texture();
        let texture = state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PO0 outgoing still"),
            size: source.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source.format(),
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            texture.as_image_copy(),
            source.size(),
        );
        state.queue.submit([encoder.finish()]);
        let id = state.renderer.write().register_native_texture(
            &state.device,
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            wgpu::FilterMode::Linear,
        );
        Self { texture, id }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one offscreen copy operation with its GPU resources"
    )]
    pub(crate) fn blend(
        state: &eframe::egui_wgpu::RenderState,
        old: &Self,
        incoming: &wgpu::TextureView,
        weight: f32,
    ) -> Self {
        let device = &state.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PO0 interrupted blend"),
            size: incoming.texture().size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: incoming.texture().format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("PO0 flatten current blend"),
            source: wgpu::ShaderSource::Wgsl(
                r"
@group(0) @binding(0) var old: texture_2d<f32>;
@group(0) @binding(1) var incoming: texture_2d<f32>;
@group(0) @binding(2) var<uniform> weight:vec4<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
 let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));return vec4(p[i],0.0,1.0);
}
@fragment fn fragment(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
 let xy=vec2<i32>(p.xy);return mix(textureLoad(old,xy,0),textureLoad(incoming,xy,0),weight.x);
}
"
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("PO0 blend copy"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: texture.format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PO0 blend weight"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        uniform
            .get_mapped_range_mut(..4)
            .copy_from_slice(&weight.to_ne_bytes());
        uniform.unmap();
        let old_view = old
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PO0 blend inputs"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&old_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(incoming),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("PO0 flatten"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        state.queue.submit([encoder.finish()]);
        let id =
            state
                .renderer
                .write()
                .register_native_texture(device, &view, wgpu::FilterMode::Linear);
        Self { texture, id }
    }
    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.texture.width()) * u64::from(self.texture.height()) * 4
    }
    pub(crate) fn release(self, renderer: &eframe::egui_wgpu::RenderState) {
        renderer.renderer.write().free_texture(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commentary_queues_without_changing_the_rows_being_read() {
        let mut list = AnimatedList::new();
        let before: Vec<_> = list.rows.iter().map(|row| row.id).collect();
        for id in 40..240 {
            list.insert(id, false);
        }
        assert_eq!(
            list.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            before
        );
        assert_eq!(list.queued, 200);
        list.release();
        assert_eq!(
            list.rows
                .iter()
                .filter(|row| !row.removing)
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            (200..240).collect::<Vec<_>>()
        );
        assert_eq!(list.queued, 0);
    }

    #[test]
    fn deadline_variant_reports_the_unsnapped_residual() {
        let mut spring = Spring::new(0.0, true);
        spring.target(336.0);
        spring.step(0.520);
        assert!((336.0 - spring.value - 0.3066).abs() < 0.03);
        assert!((spring.velocity - 4.9707).abs() < 0.5);
        assert!(spring.active());
    }

    #[test]
    fn springs_settle_with_the_specified_equations_and_stopping_thresholds() {
        for panel in [false, true] {
            for hz in [60.0, 120.0, 144.0] {
                let mut spring = Spring::new(0.0, panel);
                spring.target(336.0);
                let mut elapsed = 0.0;
                while spring.step(1.0 / hz) && elapsed < 2.0 {
                    elapsed += 1.0 / hz;
                }
                assert!(elapsed <= 0.650 + 1.0 / hz, "{panel} at {hz}: {elapsed}");
                assert_eq!(spring.value, 336.0);
                assert!(!spring.step(1.0 / hz));
            }
        }
    }
    #[test]
    fn interrupted_spring_keeps_velocity() {
        let mut spring = Spring::new(0.0, false);
        spring.target(336.0);
        spring.step(0.1);
        let velocity = spring.velocity;
        spring.target(0.0);
        assert_eq!(velocity, spring.velocity);
        spring.step(0.01);
        assert!(spring.value > 0.0);
    }
    #[test]
    fn no_motion_repaint_one_frame_after_settle() {
        let ctx = egui::Context::default();
        for frame in 0..100 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    time: Some(f64::from(frame) / 60.0),
                    ..Default::default()
                },
                |root| {
                    begin(root.ctx());
                    egui::CentralPanel::default().show(root, |ui| {
                        animate(
                            ui,
                            egui::Id::new("rail"),
                            if frame == 0 { 0.0 } else { 336.0 },
                            Token::Panel,
                        );
                    });
                    if frame > 45 {
                        assert_eq!(requests(root.ctx()), 0);
                    }
                },
            );
        }
    }
}
