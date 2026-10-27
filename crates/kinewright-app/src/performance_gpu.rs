//! Timestamp boundaries around the actual egui light, grain, and content draws.
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(crate) struct Timings(Arc<Shared>);
struct Shared {
    queries: wgpu::QuerySet,
    painted: AtomicBool,
    period: f64,
    samples: Mutex<Vec<[f64; 3]>>,
}
impl Timings {
    pub(crate) fn new(state: &egui_wgpu::RenderState) -> Option<Self> {
        if !state.device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        ) {
            return None;
        }
        Some(Self(Arc::new(Shared {
            queries: state.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("PO0 draw boundaries"),
                ty: wgpu::QueryType::Timestamp,
                count: 5,
            }),
            painted: AtomicBool::new(false),
            period: f64::from(state.queue.get_timestamp_period()) / 1_000_000.0,
            samples: Mutex::new(Vec::new()),
        })))
    }
    pub(crate) fn boundary(&self, painter: &egui::Painter, index: u32) {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            painter.clip_rect(),
            Boundary {
                timings: self.clone(),
                index,
            },
        ));
    }
    pub(crate) fn samples(&self) -> Vec<[f64; 3]> {
        self.0
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
struct Boundary {
    timings: Timings,
    index: u32,
}
impl egui_wgpu::CallbackTrait for Boundary {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if self.index != 0 || !self.timings.0.painted.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PO0 timestamp resolve"),
            size: 40,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PO0 timestamp read"),
            size: 40,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.resolve_query_set(&self.timings.0.queries, 0..5, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, 40);
        let shared = Arc::clone(&self.timings.0);
        let buffer = read.clone();
        encoder.map_buffer_on_submit(&read, wgpu::MapMode::Read, .., move |result| {
            if result.is_ok() {
                let bytes = buffer.get_mapped_range(..);
                let stamps: Vec<_> = bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|v| u64::from_le_bytes(*v))
                    .collect();
                if stamps.windows(2).all(|pair| pair[1] >= pair[0]) {
                    let mut values = std::array::from_fn(|i| {
                        #[allow(clippy::cast_precision_loss)]
                        let delta = (stamps[i + 2] - stamps[i + 1]) as f64;
                        delta * shared.period
                    });
                    #[allow(clippy::cast_precision_loss)]
                    {
                        values[2] += (stamps[1] - stamps[0]) as f64 * shared.period;
                    }
                    let mut samples = shared
                        .samples
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if samples.len() < 100_000 {
                        samples.push(values);
                    }
                }
                drop(bytes);
                buffer.unmap();
            }
        });
        Vec::new()
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        pass.write_timestamp(&self.timings.0.queries, self.index);
        if self.index == 4 {
            self.timings.0.painted.store(true, Ordering::Release);
        }
    }
}
