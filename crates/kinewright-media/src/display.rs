//! SDR display leases. The preview owns allocations; only leases cross the exchange.
use crate::{
    compositor::{GpuContext, HeldTexture, Ledgered, frame_poll},
    conversion::{MONITOR, monitor_rgba8},
};
use half::f16;
use kinewright_core::{FrameStamp, MediaError, TimeCode};
use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub struct DisplayConfig {
    /// Indexed by alpha * 256 + colour byte, supplied by the app's Color32.
    pub premultiply: Box<[u8; 65536]>,
    pub repaint: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayStatus {
    Pending,
    Gpu,
    Cpu,
}
#[derive(Default)]
struct Exchange {
    ready: Option<DisplayFrame>,
    returned: Vec<usize>,
    terminal: bool,
    status: Option<DisplayStatus>,
}
pub(crate) struct Shared {
    state: Mutex<Exchange>,
    release: Condvar,
    repaint: Arc<dyn Fn() + Send + Sync>,
    #[cfg(test)]
    before_wait: Mutex<Option<(std::sync::mpsc::Sender<()>, Arc<std::sync::Barrier>)>>,
}
impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Exchange> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn give(&self, id: usize) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(state.returned.len() < 3);
        state.returned.push(id);
        self.release.notify_all();
    }
}
/// Non-cloneable reserved, Ready, Bound or Retiring lease. Drop returns it.
pub struct DisplayFrame {
    pub at: TimeCode,
    pub stamp: FrameStamp,
    pub dimensions: (u32, u32),
    pub view: wgpu::TextureView,
    pub allocation_id: usize,
    exchange: Weak<Shared>,
}
impl Drop for DisplayFrame {
    fn drop(&mut self) {
        if let Some(shared) = self.exchange.upgrade() {
            shared.give(self.allocation_id);
        }
    }
}
/// App-owned fence, including the receiver-disconnection fallback in Drop.
pub struct DisplaySession {
    shared: Arc<Shared>,
    bound: Option<DisplayFrame>,
    retiring: Option<(u64, DisplayFrame)>,
    last_rebind: Option<u64>,
}
impl DisplaySession {
    #[must_use]
    pub fn status(&self) -> DisplayStatus {
        self.shared.lock().status.unwrap_or(DisplayStatus::Pending)
    }
    #[must_use]
    pub fn can_bind(&self, epoch: u64) -> bool {
        self.last_rebind != Some(epoch)
    }
    #[must_use]
    pub fn reserve(&self) -> Option<DisplayFrame> {
        self.shared.lock().ready.take()
    }
    pub fn defer(&self, frame: DisplayFrame) {
        let old = {
            let mut state = self.shared.lock();
            if state.terminal
                || state
                    .ready
                    .as_ref()
                    .is_some_and(|new| new.stamp.seq > frame.stamp.seq)
            {
                Some(frame)
            } else {
                state.ready.replace(frame)
            }
        };
        drop(old);
    }
    /// # Panics
    /// Panics on a second rebind in an epoch or an unretired previous rebind.
    pub fn bind(&mut self, epoch: u64, frame: DisplayFrame) {
        assert!(self.can_bind(epoch));
        if let Some(old) = self.bound.replace(frame) {
            assert!(self.retiring.is_none());
            self.retiring = Some((epoch, old));
            self.last_rebind = Some(epoch);
        }
        (self.shared.repaint)();
    }
    pub fn advance(&mut self, epoch: u64) {
        if self.retiring.as_ref().is_some_and(|(e, _)| epoch > *e) {
            drop(self.retiring.take());
        }
    }
    /// Call after native-id removal, when no future paint can sample it.
    /// Transfers every fence lease and Terminal under one exchange lock.
    pub fn terminal(&mut self) {
        let bound = self.bound.take();
        let retiring = self.retiring.take().map(|(_, f)| f);
        let mut leases: Vec<_> = bound.into_iter().chain(retiring).collect();
        {
            let mut state = self.shared.lock();
            for lease in &mut leases {
                state.returned.push(lease.allocation_id);
                lease.exchange = Weak::new();
            }
            state.terminal = true;
            self.shared.release.notify_all();
        }
        drop(leases);
        (self.shared.repaint)();
    }
}
impl Drop for DisplaySession {
    fn drop(&mut self) {
        self.terminal();
    }
}

fn buffer(gpu: &GpuContext, size: u64, usage: wgpu::BufferUsages) -> Ledgered<wgpu::Buffer> {
    gpu.charge_buffer(gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("display buffer"),
        size,
        usage,
        mapped_at_creation: usage.contains(wgpu::BufferUsages::MAP_WRITE),
    }))
}
fn upload(gpu: &GpuContext, bytes: &[u8]) -> Ledgered<wgpu::Buffer> {
    let buffer = buffer(
        gpu,
        bytes.len() as u64,
        wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
    );
    buffer.get_mapped_range_mut(..).copy_from_slice(bytes);
    buffer.unmap();
    buffer
}
fn texture(
    gpu: &GpuContext,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> HeldTexture {
    gpu.charge_texture(gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("display texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    }))
}
pub(crate) struct DisplaySlot {
    pub(crate) texture: HeldTexture,
    pub(crate) flags: Ledgered<wgpu::Buffer>,
}
impl DisplaySlot {
    fn new(gpu: &GpuContext, dims: (u32, u32)) -> Self {
        Self {
            texture: texture(
                gpu,
                dims,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
            ),
            flags: buffer(
                gpu,
                4,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
        }
    }
    pub(crate) fn flags(&mut self, gpu: &GpuContext, bytes: u64) {
        if self.flags.size() != bytes {
            self.flags = buffer(
                gpu,
                bytes,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            );
        }
    }
}
/// Owns the table and staging until completion, including a failed self-check.
pub(crate) struct DisplayEncoder {
    gpu: GpuContext,
    pipeline: wgpu::ComputePipeline,
    table: Ledgered<wgpu::Buffer>,
    pub(crate) executions: AtomicU64,
}
impl DisplayEncoder {
    pub(crate) fn new(gpu: GpuContext, config: &DisplayConfig) -> Result<Self, MediaError> {
        Self::with_table(gpu, config, |_| {})
    }
    fn with_table(
        gpu: GpuContext,
        config: &DisplayConfig,
        change: impl FnOnce(&mut [u8]),
    ) -> Result<Self, MediaError> {
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("display encode"),
                source: wgpu::ShaderSource::Wgsl(include_str!("display.wgsl").into()),
            });
        let pipeline = gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("display encode"),
                layout: None,
                module: &shader,
                entry_point: Some("encode"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let (rgb, alpha) = &*MONITOR;
        let mut bytes: Vec<_> = rgb
            .iter()
            .chain(alpha)
            .chain(config.premultiply.iter())
            .copied()
            .collect();
        change(&mut bytes);
        let table = buffer(
            &gpu,
            bytes.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let staging = upload(&gpu, &bytes);
        let mut commands = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        commands.copy_buffer_to_buffer(&staging, 0, &table, 0, table.size());
        let index = gpu.queue.submit([commands.finish()]);
        complete(&gpu, index);
        drop(staging);
        let encoder = Self {
            gpu,
            pipeline,
            table,
            executions: AtomicU64::new(0),
        };
        let checked = encoder.self_check(config);
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(MediaError::Backend(format!("display: {error}")));
        }
        checked?;
        Ok(encoder)
    }
    pub(crate) fn encode(
        &self,
        commands: &mut wgpu::CommandEncoder,
        input: &wgpu::Texture,
        output: &wgpu::Texture,
    ) {
        self.executions.fetch_add(1, Ordering::Relaxed);
        let bindings = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display bindings"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &input.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &output.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.table.as_entire_binding(),
                    },
                ],
            });
        let mut pass = commands.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("display encode"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(input.width().div_ceil(8), input.height().div_ceil(8), 1);
        drop(pass);
    }
    fn self_check(&self, config: &DisplayConfig) -> Result<(), MediaError> {
        // Opaque asymmetric RGB, independent alpha, then every reachable premultiply pair.
        let opaque = (0..=u16::MAX).map(|b| [b, b.rotate_left(5), !b, f16::ONE.to_bits()]);
        let alpha = (0..=u16::MAX).map(|b| [0x3800, 0x3400, 0x3c00, b]);
        let inverse = |table: &[u8]| {
            std::array::from_fn::<_, 256, _>(|v| {
                u16::try_from(table.iter().position(|b| usize::from(*b) == v).unwrap_or(0))
                    .expect("f16 table index")
            })
        };
        let (rgb_bits, alpha_bits) = (inverse(&MONITOR.0), inverse(&MONITOR.1));
        let reachable = (0..=u16::MAX).map(|b| {
            let v = rgb_bits[usize::from(b & 255)];
            let a = alpha_bits[usize::from(b >> 8)];
            [v, v, v, a]
        });
        for inputs in [
            opaque.collect::<Vec<_>>(),
            alpha.collect(),
            reachable.collect(),
        ] {
            let texture = texture(
                &self.gpu,
                (256, 256),
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let bytes: Vec<u8> = inputs
                .iter()
                .flatten()
                .flat_map(|b| b.to_le_bytes())
                .collect();
            let staging = upload(&self.gpu, &bytes);
            let output = DisplaySlot::new(&self.gpu, (256, 256));
            let mut commands = self
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            commands.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256 * 8),
                        rows_per_image: Some(256),
                    },
                },
                texture.as_image_copy(),
                texture.size(),
            );
            self.encode(&mut commands, &texture, &output.texture);
            let index = self.gpu.queue.submit([commands.finish()]);
            complete(&self.gpu, index);
            let actual = read_texture(&self.gpu, &output.texture)?;
            for (bits, actual) in inputs.into_iter().zip(actual.as_chunks::<4>().0) {
                let [r, g, b, a] = monitor_rgba8(bits);
                let pre = |v| config.premultiply[usize::from(a) * 256 + usize::from(v)];
                if *actual != [pre(r), pre(g), pre(b), a] {
                    return Err(MediaError::Backend(format!(
                        "display: exact self-check mismatch at {bits:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}
impl Drop for DisplayEncoder {
    fn drop(&mut self) {
        complete(&self.gpu, self.gpu.queue.submit([]));
    }
}

/// Deadline-free completion ownership: poll errors do not authorize uncharging.
/// Device loss runs wgpu's pending callbacks; otherwise keep polling and charged.
pub(crate) fn complete(gpu: &GpuContext, index: wgpu::SubmissionIndex) {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    gpu.queue
        .on_submitted_work_done(move || flag.store(true, Ordering::Release));
    let mut wait_index = Some(index);
    while !done.load(Ordering::Acquire) {
        let result = frame_poll(
            &gpu.device,
            wgpu::PollType::Wait {
                submission_index: wait_index.clone(),
                timeout: None,
            },
        );
        // A submit rejected after device loss has no successful index. Drain
        // the last successful submission; the callback still owns completion.
        if matches!(result, Err(wgpu::PollError::WrongSubmissionIndex(..))) {
            wait_index = None;
        }
        if !done.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}
pub(crate) fn mapped_bytes(
    gpu: &GpuContext,
    buffer: &wgpu::Buffer,
    index: wgpu::SubmissionIndex,
) -> Result<Vec<u8>, MediaError> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    complete(gpu, index);
    let result = rx
        .recv()
        .map_err(|e| MediaError::Backend(format!("display: map callback {e}")))?;
    result.map_err(|e| MediaError::Backend(format!("display: map {e}")))?;
    let bytes = buffer.get_mapped_range(..).to_vec();
    buffer.unmap();
    Ok(bytes)
}

// Diagnostic copies are excluded from the production full-frame counter.
fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture) -> Result<Vec<u8>, MediaError> {
    let (w, h) = (texture.width(), texture.height());
    let stride = (w * 4).next_multiple_of(256);
    let readback = buffer(
        gpu,
        u64::from(stride * h),
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let mut commands = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    commands.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
    );
    let bytes = mapped_bytes(gpu, &readback, gpu.queue.submit([commands.finish()]))?;
    Ok(bytes
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..(w * 4) as usize].iter().copied())
        .collect())
}

pub(crate) struct DisplayPool {
    pub(crate) encoder: DisplayEncoder,
    slots: [Option<DisplaySlot>; 3],
    free: Vec<usize>,
    shared: Arc<Shared>,
}
impl DisplayPool {
    pub(crate) fn session(config: &DisplayConfig) -> (DisplaySession, Arc<Shared>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(Exchange::default()),
            release: Condvar::new(),
            repaint: Arc::clone(&config.repaint),
            #[cfg(test)]
            before_wait: Mutex::default(),
        });
        (
            DisplaySession {
                shared: Arc::clone(&shared),
                bound: None,
                retiring: None,
                last_rebind: None,
            },
            shared,
        )
    }
    pub(crate) fn new(
        gpu: GpuContext,
        config: &DisplayConfig,
        shared: Arc<Shared>,
    ) -> Result<Self, MediaError> {
        Self::initialized(DisplayEncoder::new(gpu, config), shared)
    }
    fn initialized(
        result: Result<DisplayEncoder, MediaError>,
        shared: Arc<Shared>,
    ) -> Result<Self, MediaError> {
        shared.lock().status = Some(if result.is_ok() {
            DisplayStatus::Gpu
        } else {
            DisplayStatus::Cpu
        });
        (shared.repaint)();
        Ok(Self {
            encoder: result?,
            slots: Default::default(),
            free: vec![0, 1, 2],
            shared,
        })
    }
    pub(crate) fn terminal(&self) -> bool {
        self.shared.lock().terminal
    }
    pub(crate) fn acquire(
        &mut self,
        dims: (u32, u32),
        timeout: Duration,
        cancelled: impl Fn() -> bool,
    ) -> Option<usize> {
        let deadline = Instant::now() + timeout;
        loop {
            let (returned, terminal) = {
                let mut state = self.shared.lock();
                (std::mem::take(&mut state.returned), state.terminal)
            };
            self.free.extend(returned);
            if terminal || cancelled() {
                return None;
            }
            if let Some(id) = self.free.pop() {
                if self.slots[id]
                    .as_ref()
                    .is_none_or(|s| (s.texture.width(), s.texture.height()) != dims)
                {
                    complete(&self.encoder.gpu, self.encoder.gpu.queue.submit([]));
                    self.slots[id] = Some(DisplaySlot::new(&self.encoder.gpu, dims));
                }
                return Some(id);
            }
            (self.shared.repaint)();
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let state = self.shared.lock();
            if state.returned.is_empty() && !state.terminal {
                drop(
                    self.shared
                        .release
                        .wait_timeout(state, left.min(Duration::from_millis(5)))
                        .unwrap_or_else(PoisonError::into_inner),
                );
            }
        }
    }
    pub(crate) fn slot(&mut self, id: usize) -> (&DisplayEncoder, &mut DisplaySlot) {
        (&self.encoder, self.slots[id].as_mut().expect("acquired"))
    }
    pub(crate) fn candidate(&self, id: usize, at: TimeCode, stamp: FrameStamp) -> DisplayFrame {
        let texture = &self.slots[id].as_ref().expect("acquired").texture;
        DisplayFrame {
            at,
            stamp,
            dimensions: (texture.width(), texture.height()),
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            allocation_id: id,
            exchange: Arc::downgrade(&self.shared),
        }
    }
    pub(crate) fn publish(&self, frame: DisplayFrame) {
        let old = {
            let mut state = self.shared.lock();
            if state.terminal {
                Some(frame)
            } else {
                state.ready.replace(frame)
            }
        };
        drop(old);
        (self.shared.repaint)();
    }
    pub(crate) fn abandon(&mut self, id: usize) {
        self.free.push(id);
    }
}
impl Drop for DisplayPool {
    fn drop(&mut self) {
        let ready = self.shared.lock().ready.take();
        drop(ready);
        // App Drop transfers Bound/Retiring even when on_exit was never called.
        while self.free.len() < 3 {
            let mut state = self.shared.lock();
            self.free.extend(std::mem::take(&mut state.returned));
            if self.free.len() < 3 {
                #[cfg(test)]
                if let Some((reached, barrier)) = self.shared.before_wait.lock().unwrap().take() {
                    reached.send(()).unwrap();
                    barrier.wait();
                }
                drop(
                    self.shared
                        .release
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner),
                );
            }
        }
        complete(&self.encoder.gpu, self.encoder.gpu.queue.submit([]));
    }
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
