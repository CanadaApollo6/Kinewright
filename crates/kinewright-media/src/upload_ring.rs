//! Two completion-owned upload batches; callbacks settle only their own batch.
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

use super::{Arc, AtomicBool, GpuContext, Ledgered, MediaError, Mutex, Ordering, PoisonError};

enum Phase {
    Mapped,
    Writing,
    Submitted(Arc<AtomicBool>),
    Remapping,
    Abandoned,
}

struct Batch {
    buffers: Vec<Arc<Ledgered<wgpu::Buffer>>>,
    used: usize,
    phase: Phase,
    maps: Vec<Receiver<Result<(), wgpu::BufferAsyncError>>>,
    failed: bool,
}

impl Default for Batch {
    fn default() -> Self {
        Self {
            buffers: Vec::new(),
            used: 0,
            phase: Phase::Mapped,
            maps: Vec::new(),
            failed: false,
        }
    }
}

#[derive(Default)]
pub(super) struct UploadRing(Vec<Arc<Mutex<Batch>>>, Option<Arc<dyn Fn() + Send + Sync>>);

pub(super) struct UploadLease(Arc<Mutex<Batch>>);

impl UploadRing {
    fn ready(batch: &Arc<Mutex<Batch>>) -> bool {
        Arc::strong_count(batch) == 1
            && matches!(
                batch.lock().unwrap_or_else(PoisonError::into_inner).phase,
                Phase::Mapped
            )
    }
    pub(super) fn available(&self) -> bool {
        self.0.len() < 2 || self.0.iter().any(Self::ready)
    }
    pub(super) fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.1 = Some(wake);
    }
    pub(super) fn wake(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        self.1.clone()
    }

    #[cfg(test)]
    pub(super) fn charged_bytes(&self) -> u64 {
        self.0
            .iter()
            .map(|batch| {
                batch
                    .lock()
                    .unwrap()
                    .buffers
                    .iter()
                    .map(|b| b.1)
                    .sum::<u64>()
            })
            .sum()
    }

    pub(super) fn acquire(&mut self) -> Result<UploadLease, MediaError> {
        self.advance();
        let batch = self
            .0
            .iter()
            .find(|batch| Self::ready(batch))
            .cloned()
            .or_else(|| {
                if self.0.len() == 2 {
                    return None;
                }
                let batch = Arc::new(Mutex::new(Batch::default()));
                self.0.push(batch.clone());
                Some(batch)
            })
            .ok_or_else(|| MediaError::Backend("upload ring exhausted".to_owned()))?;
        {
            let mut state = batch.lock().unwrap_or_else(PoisonError::into_inner);
            state.phase = Phase::Writing;
            state.used = 0;
        }
        Ok(UploadLease(batch))
    }

    pub(super) fn advance(&mut self) {
        for batch in &self.0 {
            let mut state = batch.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(&state.phase, Phase::Submitted(done) if done.load(Ordering::Acquire)) {
                state.phase = Phase::Remapping;
                state.failed = true;
                let buffers = state.buffers.clone();
                for buffer in buffers {
                    let (tx, rx) = sync_channel(1);
                    let wake = self.wake();
                    let callback = move |result| {
                        let _ = tx.try_send(result);
                        if let Some(wake) = wake {
                            wake();
                        }
                    };
                    #[cfg(test)]
                    let callback = super::s3b_tests::map_callback(callback);
                    state.maps.push(rx);
                    buffer.slice(..).map_async(wgpu::MapMode::Write, callback);
                }
                state.failed = false;
            }
            if matches!(state.phase, Phase::Remapping) {
                let mut pending = Vec::new();
                for rx in std::mem::take(&mut state.maps) {
                    match rx.try_recv() {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) | Err(TryRecvError::Disconnected) => state.failed = true,
                        Err(TryRecvError::Empty) => pending.push(rx),
                    }
                }
                state.maps = pending;
                if state.maps.is_empty() {
                    state.phase = if state.failed {
                        Phase::Abandoned
                    } else {
                        Phase::Mapped
                    };
                }
            }
        }
        self.0.retain(|batch| {
            Arc::strong_count(batch) != 1
                || !matches!(
                    batch.lock().unwrap_or_else(PoisonError::into_inner).phase,
                    Phase::Abandoned
                )
        });
    }

    pub(super) fn pending(&self) -> bool {
        self.0.iter().any(|batch| {
            !matches!(
                batch.lock().unwrap_or_else(PoisonError::into_inner).phase,
                Phase::Mapped
            )
        })
    }
}

impl UploadLease {
    pub(super) fn buffer(&self, gpu: &GpuContext, size: u64) -> Arc<Ledgered<wgpu::Buffer>> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let index = state.used;
        state.used += 1;
        if state.buffers.get(index).is_none_or(|b| b.size() != size) {
            let buffer = gpu.charge_buffer(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Kinewright layer upload staging"),
                size,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            }));
            if index == state.buffers.len() {
                state.buffers.push(Arc::new(buffer));
            } else {
                state.buffers[index] = Arc::new(buffer);
            }
        }
        state.buffers[index].clone()
    }

    pub(super) fn unmap_current(&self) {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.buffers[state.used - 1].unmap();
    }

    pub(super) fn submitted(&self, done: Arc<AtomicBool>) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let used = state.used;
        state.buffers.truncate(used);
        state.phase = Phase::Submitted(done);
    }
}

impl Drop for UploadLease {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(state.phase, Phase::Writing) {
            // Native buffer destruction releases mapped memory without a device call.
            state.phase = Phase::Abandoned;
        }
    }
}
