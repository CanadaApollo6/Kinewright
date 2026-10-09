use std::cell::{Cell, RefCell};

use super::*;
use crate::gpu_test_support::fixture_gpu_or_skip;
use crate::timeline::TransitionRenderParams;

type Delivery = Box<dyn FnOnce() + Send>;
type DeliverySender = std::sync::mpsc::SyncSender<Delivery>;

thread_local! {
    static COPIES: Cell<usize> = const { Cell::new(0) };
    static UPLOADS: RefCell<Vec<wgpu::Buffer>> = const { RefCell::new(Vec::new()) };
    static MAP_HOLD: RefCell<Option<DeliverySender>> = const { RefCell::new(None) };
    static COMPLETE_HOLD: RefCell<Option<DeliverySender>> = const { RefCell::new(None) };
}

pub(super) fn intermediate(bytes: usize) {
    COPIES.set(COPIES.get() + bytes);
}

pub(super) fn upload(buffer: &wgpu::Buffer) {
    UPLOADS.with_borrow_mut(|uploads| uploads.push(buffer.clone()));
}

pub(super) fn map_callback(
    callback: impl FnOnce(Result<(), wgpu::BufferAsyncError>) + Send + 'static,
) -> Box<dyn FnOnce(Result<(), wgpu::BufferAsyncError>) + Send> {
    match MAP_HOLD.take() {
        Some(tx) => Box::new(move |result| {
            tx.try_send(Box::new(move || callback(result))).unwrap();
        }),
        None => Box::new(callback),
    }
}

pub(super) fn complete_callback(
    callback: impl FnOnce() + Send + 'static,
) -> Box<dyn FnOnce() + Send> {
    match COMPLETE_HOLD.take() {
        Some(tx) => Box::new(move || {
            tx.try_send(Box::new(callback)).unwrap();
        }),
        None => Box::new(callback),
    }
}

fn staged(
    c: &Compositor,
    lease: &upload_ring::UploadLease,
) -> (Arc<Ledgered<wgpu::Buffer>>, wgpu::Buffer) {
    let source = lease.buffer(&c.gpu, 256);
    source
        .get_mapped_range_mut(..)
        .slice(..)
        .copy_from_slice(&[19; 256]);
    lease.unmap_current();
    let target = c.gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("S3b actual copy witness"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = c
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(&source, 0, &target, 0, 256);
    let (_, done) = c.submit_tracked([encoder.finish()]);
    lease.submitted(done);
    (source, target)
}

fn settled(c: &Compositor) {
    c.gpu
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
}

fn guard_witness(map: bool) {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let base = gpu.ledger().live_bytes();
    let lease = c.upload_ring.lock().unwrap().acquire().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if map {
        MAP_HOLD.set(Some(tx));
    } else {
        COMPLETE_HOLD.set(Some(tx));
    }
    let (buffer, target) = staged(&c, &lease);
    drop(lease);
    settled(&c);
    c.upload_ring.lock().unwrap().advance();
    settled(&c);
    let release = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("real callback delivered");
    let spare = c.upload_ring.lock().unwrap().acquire().unwrap();
    let refused = c.upload_ring.lock().unwrap().acquire().is_err();
    let exact = gpu.ledger().live_bytes() == base + buffer.1;
    release();
    drop(spare);
    c.upload_ring.lock().unwrap().advance();
    settled(&c);
    c.upload_ring.lock().unwrap().advance();
    let lease = c.upload_ring.lock().unwrap().acquire().unwrap();
    let recycled = lease.buffer(&gpu, 256);
    let identity = **recycled == **buffer;
    drop((lease, recycled, buffer, target));
    c.upload_ring.lock().unwrap().advance();
    assert_eq!(
        gpu.ledger().live_bytes(),
        base,
        "abandoned Writing returns exactly"
    );
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
    assert!(
        refused,
        "must await {} delivery",
        if map { "map" } else { "completion" }
    );
    assert!(
        exact && identity,
        "charge held once and original allocation reused"
    );
}

#[test]
fn s3b_u1_completion_observation_precedes_reuse() {
    guard_witness(false);
}

#[test]
fn s3b_u1_map_delivery_precedes_reuse() {
    guard_witness(true);
}

#[test]
fn s3b_u1_completion_old_map_cannot_unlock_another_generation() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let base = gpu.ledger().live_bytes();
    let old = c.upload_ring.lock().unwrap().acquire().unwrap();
    let (map_tx, map_rx) = std::sync::mpsc::sync_channel(1);
    MAP_HOLD.set(Some(map_tx));
    let old_resources = staged(&c, &old);
    drop(old);
    settled(&c);
    c.upload_ring.lock().unwrap().advance();
    settled(&c);
    let old_delivery = map_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let new = c.upload_ring.lock().unwrap().acquire().unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    COMPLETE_HOLD.set(Some(done_tx));
    let new_resources = staged(&c, &new);
    drop(new);
    settled(&c);
    let new_delivery = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    old_delivery();
    c.upload_ring.lock().unwrap().advance();
    settled(&c);
    let ready_old = c.upload_ring.lock().unwrap().acquire().unwrap();
    let refused = c.upload_ring.lock().unwrap().acquire().is_err();
    new_delivery();
    drop(ready_old);
    c.upload_ring.lock().unwrap().advance();
    settled(&c);
    c.upload_ring.lock().unwrap().advance();
    let ready_new = c.upload_ring.lock().unwrap().acquire().unwrap();
    drop((ready_new, old_resources, new_resources));
    c.upload_ring.lock().unwrap().advance();
    assert_eq!(
        gpu.ledger().live_bytes(),
        base,
        "both generations charged once and retired"
    );
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
    assert!(refused, "old callback cannot enable the newer generation");
}

#[test]
fn s3b_u1_abandoned_and_pre_submit_failure_return_exact_charges() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let base = gpu.ledger().live_bytes();
    for fail in [false, true] {
        let lease = c.upload_ring.lock().unwrap().acquire().unwrap();
        let buffer = lease.buffer(&gpu, 768);
        assert_eq!(gpu.ledger().live_bytes(), base + 768);
        if fail {
            lease.unmap_current();
        }
        drop((lease, buffer));
        c.upload_ring.lock().unwrap().advance();
        let live = gpu.ledger().live_bytes();
        if live != base {
            std::mem::forget(c);
            assert_eq!(live, base, "no unregistered callback awaited");
            return;
        }
    }
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn s3b_u1_device_loss_settles_registered_maps_and_teardown() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let base = gpu.ledger().live_bytes();
    let lease = c.upload_ring.lock().unwrap().acquire().unwrap();
    let (buffer, target) = staged(&c, &lease);
    drop((lease, buffer, target));
    settled(&c);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    MAP_HOLD.set(Some(tx));
    c.upload_ring.lock().unwrap().advance();
    gpu.device.destroy();
    let _ = gpu.device.poll(wgpu::PollType::Poll);
    let delivery = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("actual loss callback");
    c.upload_ring.lock().unwrap().advance();
    let held = gpu.ledger().live_bytes() == base + 256 && c.upload_ring.lock().unwrap().pending();
    delivery();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        let mut ring = c.upload_ring.lock().unwrap();
        ring.advance();
        if !ring.pending() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            drop(ring);
            std::mem::forget(c);
            panic!("device-loss maps did not settle within five seconds");
        }
    }
    drop(c);
    assert_eq!(
        gpu.ledger().live_bytes(),
        0,
        "loss teardown returns every charge"
    );
    assert!(
        held,
        "loss retains exact charges until registered callback delivery"
    );
}

fn frame() -> WorkingFrame {
    WorkingFrame {
        width: 17,
        height: 3,
        pixels: Arc::new(
            (0..17 * 3 * 4)
                .map(|i| f16::from_f32(f32::from(u16::try_from(i % 13).unwrap()) / 16.0))
                .collect(),
        ),
    }
}

fn render(c: &Compositor, f: &WorkingFrame) -> Vec<f32> {
    c.render_working(
        (f.width, f.height),
        &[CompositorLayer {
            frame: f,
            effects: &[],
            transition: TransitionRenderParams::default(),
            mode: LayerMode::NORMAL,
        }],
    )
    .unwrap()
    .pixels
}

#[test]
fn s3b_u1_reuses_the_completed_mapped_buffer() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu);
    let f = frame();
    UPLOADS.with_borrow_mut(Vec::clear);
    let first = render(&c, &f);
    let second = render(&c, &f);
    assert_eq!(first, second, "odd-width padded upload parity");
    UPLOADS.with_borrow(|uploads| {
        assert_eq!(uploads.len(), 2, "two production uploads observed");
        assert_eq!(uploads[0], uploads[1], "must reuse the staging allocation");
    });
}

#[test]
fn s3b_u1_fills_staging_without_an_intermediate_copy() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu);
    COPIES.set(0);
    assert!(!render(&c, &frame()).is_empty());
    assert_eq!(COPIES.get(), 0, "intermediate upload allocation bytes");
}
