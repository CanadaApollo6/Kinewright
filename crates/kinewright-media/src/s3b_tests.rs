use std::cell::{Cell, RefCell};

use super::*;
use crate::gpu_test_support::fixture_gpu_or_skip;
use crate::timeline::TransitionRenderParams;

pub(crate) type Delivery = Box<dyn FnOnce() + Send>;
type DeliverySender = std::sync::mpsc::SyncSender<Delivery>;

thread_local! {
    static SUBMITTED_BYTES: Cell<u64> = const { Cell::new(0) };
    static COPIES: Cell<usize> = const { Cell::new(0) };
    static UPLOADS: RefCell<Vec<wgpu::Buffer>> = const { RefCell::new(Vec::new()) };
    static MAP_HOLD: RefCell<Option<DeliverySender>> = const { RefCell::new(None) };
    static COMPLETE_HOLD: RefCell<Option<DeliverySender>> = const { RefCell::new(None) };
    static SWEPT: RefCell<Option<std::sync::mpsc::Sender<()>>> = const { RefCell::new(None) };
}

/// Teardown's first sweep of the retired frames (`Compositor::drop`).
pub(super) fn swept() {
    if let Some(tx) = SWEPT.take() {
        let _ = tx.send(());
    }
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

/// Review round 1 (Astra 2, Sonnet 3): a failed frame retires holding its
/// Writing lease; its real completion arrives only after teardown's first
/// sweep, and teardown must still end with every charge returned.
#[test]
fn s3b_u1_teardown_sweeps_a_failed_frame_completed_after_its_first_sweep() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let good = frame();
    render(&c, &good);
    let bad = WorkingFrame {
        width: 17,
        height: 3,
        pixels: Arc::new(Vec::new()),
    };
    let layer = |frame| CompositorLayer {
        frame,
        effects: &[],
        transition: TransitionRenderParams::default(),
        mode: LayerMode::NORMAL,
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    COMPLETE_HOLD.set(Some(tx));
    let failed = c.render_working((17, 3), &[layer(&good), layer(&bad)]);
    assert!(failed.is_err(), "real later staging failure");
    assert_eq!(c.retired.lock().unwrap().len(), 1, "retired with its lease");
    let delivery = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("real completion held");
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let (swept_tx, swept_rx) = std::sync::mpsc::channel();
    let teardown = std::thread::spawn(move || {
        SWEPT.set(Some(swept_tx));
        drop(c);
        done_tx.send(()).unwrap();
    });
    // Deliver only after teardown's first sweep found the frame unfinished.
    let first = swept_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let unfinished_at_sweep_observation = done_rx.try_recv().is_err();
    delivery();
    let ended = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    assert!(first, "teardown swept before the completion was delivered");
    assert!(
        unfinished_at_sweep_observation,
        "no teardown completion was queued when the first sweep was observed"
    );
    assert!(ended, "teardown ends once the completion is delivered");
    teardown.join().unwrap();
    assert_eq!(gpu.ledger().live_bytes(), 0, "every charge returned");
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

pub(super) fn submitted_uploads(frame: &FrameResources) {
    let bytes = frame
        .layers
        .iter()
        .filter(|layer| layer.upload.is_some())
        .filter_map(|layer| layer.source.as_ref())
        .map(|(key, _)| texture_pool_bytes(*key))
        .sum::<u64>();
    SUBMITTED_BYTES.set(SUBMITTED_BYTES.get() + bytes);
}

pub(crate) fn uploaded_bytes<T>(body: impl FnOnce() -> T) -> (T, u64) {
    SUBMITTED_BYTES.set(0);
    let result = body();
    (result, SUBMITTED_BYTES.get())
}

pub(crate) struct Deferred(Option<Delivery>);
impl Deferred {
    pub(crate) fn deliver(mut self) {
        self.0.take().expect("owned delivery")();
    }
}

pub(crate) fn delay_next_map() -> std::sync::mpsc::Receiver<Delivery> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    MAP_HOLD.set(Some(tx));
    rx
}

/// Delay delivery of real write-map results for both submitted batches.
pub(crate) fn parked_maps(c: &Compositor) -> Vec<Deferred> {
    (0..2)
        .map(|_| {
            let lease = c.upload_ring.lock().unwrap().acquire().unwrap();
            let resources = staged(c, &lease);
            drop((lease, resources));
            settled(c);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            MAP_HOLD.set(Some(tx));
            c.upload_ring.lock().unwrap().advance();
            settled(c);
            Deferred(Some(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("real map callback"),
            ))
        })
        .collect()
}
impl Drop for Deferred {
    fn drop(&mut self) {
        if let Some(delivery) = self.0.take() {
            delivery();
        }
    }
}

struct ResidentInput {
    frame: WorkingFrame,
    content: ResidentContent,
    at: kinewright_core::TimeCode,
}

impl ResidentInput {
    fn new(id: u64) -> Self {
        Self {
            frame: frame(),
            content: ResidentContent::Generated((
                ClipId(id),
                (17, 3),
                crate::render::Generated::Title(kinewright_core::Title::default()),
            )),
            at: kinewright_core::TimeCode::ZERO,
        }
    }
}

impl CompositorInput for ResidentInput {
    const FORMAT: wgpu::TextureFormat = WorkingFrame::FORMAT;
    const BYTES_PER_PIXEL: u32 = WorkingFrame::BYTES_PER_PIXEL;
    const LINEAR: bool = true;
    fn width(&self) -> u32 {
        self.frame.width
    }
    fn height(&self) -> u32 {
        self.frame.height
    }
    fn upload_bytes(&self) -> Vec<u8> {
        self.frame.upload_bytes()
    }
    fn write_upload(
        &self,
        mapped: &mut wgpu::BufferViewMut,
        pitch: usize,
    ) -> Result<(), MediaError> {
        self.frame.write_upload(mapped, pitch)
    }
    fn residency(&self) -> Option<(ResidencyKey, std::sync::Weak<Vec<f16>>)> {
        Some((
            ResidencyKey::new(self.content.clone(), self.at, &self.frame),
            Arc::downgrade(&self.frame.pixels),
        ))
    }
}

fn resident_layer(input: &ResidentInput) -> CompositorLayer<'_, ResidentInput> {
    CompositorLayer {
        frame: input,
        effects: &[],
        transition: TransitionRenderParams::default(),
        mode: LayerMode::NORMAL,
    }
}

fn resident_draw(c: &Compositor, input: &ResidentInput) -> Vec<f32> {
    c.render_working((input.width(), input.height()), &[resident_layer(input)])
        .unwrap()
        .pixels
}

pub(crate) fn residents(c: &Compositor) -> (usize, u64, Vec<std::sync::Weak<Vec<f16>>>) {
    let pool = c.texture_pool.lock().unwrap();
    (
        pool.residents.len(),
        pool.resident_bytes,
        pool.residents
            .values()
            .map(|entry| entry.pixels.clone())
            .collect(),
    )
}

#[test]
fn s3b_u2_identity_requires_the_allocation_shape_and_time() {
    let c = Compositor::new(fixture_gpu_or_skip().expect("actual GPU required"));
    let original = ResidentInput::new(1);
    let original_key = original.residency().unwrap().0;
    let first = resident_draw(&c, &original);
    let (hit, bytes) = uploaded_bytes(|| resident_draw(&c, &original));
    assert_eq!(first, hit);
    assert_eq!(bytes, 0);
    let mut replacement = ResidentInput::new(1);
    assert_ne!(
        replacement.residency().unwrap().0,
        original_key,
        "allocation identity"
    );
    let (_, bytes) = uploaded_bytes(|| resident_draw(&c, &replacement));
    assert_eq!(bytes, 17 * 3 * 8);
    replacement.frame.pixels = original.frame.pixels.clone();
    replacement.frame.width = 3;
    replacement.frame.height = 17;
    assert_ne!(
        replacement.residency().unwrap().0,
        original_key,
        "same allocation changed shape"
    );
    let (_, bytes) = uploaded_bytes(|| resident_draw(&c, &replacement));
    assert_eq!(bytes, 17 * 3 * 8);
    replacement.frame = original.frame.clone();
    replacement.at = kinewright_core::TimeCode(1);
    assert_ne!(
        replacement.residency().unwrap().0,
        original_key,
        "source time"
    );
    let (_, bytes) = uploaded_bytes(|| resident_draw(&c, &replacement));
    assert_eq!(bytes, 17 * 3 * 8);
    assert_eq!(c.gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
}

#[test]
fn s3b_u2_failed_later_layer_never_publishes_the_first_upload() {
    let c = Compositor::new(fixture_gpu_or_skip().expect("actual GPU required"));
    let input = ResidentInput::new(1);
    let mut invalid = ResidentInput::new(2);
    invalid.frame.pixels = Arc::new(vec![]);
    let result = c.render_working((17, 3), &[resident_layer(&input), resident_layer(&invalid)]);
    assert!(result.is_err(), "real later staging failure");
    assert_eq!(residents(&c).0, 0, "no partially published frame");
    let (first, bytes) = uploaded_bytes(|| resident_draw(&c, &input));
    assert_eq!(bytes, 17 * 3 * 8, "retry performs a real upload");
    let (second, bytes) = uploaded_bytes(|| resident_draw(&c, &input));
    assert_eq!(first, second);
    assert_eq!(bytes, 0);
    assert_eq!(c.gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
}

#[test]
fn s3b_u2_eviction_waits_for_observed_sampling_completion() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    resident_draw(&c, &ResidentInput::new(0)); // atlas initialized before the barrier
    let input = ResidentInput::new(1);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    COMPLETE_HOLD.set(Some(tx));
    resident_draw(&c, &input);
    let delivery = Deferred(Some(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("real queue callback"),
    ));
    c.clear_residency();
    assert_eq!(residents(&c).0, 0, "eligibility ends immediately");
    assert_eq!(
        residents(&c).1,
        17 * 3 * 8,
        "in-flight eviction stays charged"
    );
    assert_eq!(
        gpu.ledger().live_bytes(),
        ledger_probes::inventory(&c),
        "each charge held once"
    );
    delivery.deliver();
    settled(&c);
    c.sweep_retired();
    c.texture_pool.lock().unwrap().maintain(false);
    assert_eq!(residents(&c).1, 0);
    assert_eq!(gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn s3b_u2_an_unsubmitted_pin_prevents_recycling() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    let input = ResidentInput::new(1);
    resident_draw(&c, &input);
    let (output, frame, encoder) = c
        .composite(17, 3, &[resident_layer(&input)], None, None)
        .unwrap();
    let old = frame.layers[0].source.as_ref().unwrap().1.0.clone();
    c.clear_residency();
    assert_eq!(residents(&c).1, 17 * 3 * 8, "unsubmitted sample pin held");
    let next = ResidentInput::new(2);
    resident_draw(&c, &next);
    let texture = c
        .texture_pool
        .lock()
        .unwrap()
        .residents
        .values()
        .next()
        .unwrap()
        .texture
        .0
        .clone();
    assert_ne!(old, texture, "pinned texture cannot be overwritten");
    drop((old, texture, output, encoder));
    c.retire_failed(frame, frame_poll);
    c.texture_pool.lock().unwrap().maintain(false);
    assert_eq!(residents(&c).1, 17 * 3 * 8, "only the newer entry remains");
    assert_eq!(gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

fn resident_upload(c: &Compositor, input: &ResidentInput) {
    let mut frame = FrameResources::default();
    c.stage_layers(
        input.width(),
        input.height(),
        &[resident_layer(input)],
        None,
        None,
        &mut frame,
    )
    .unwrap();
    let mut encoder = c
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    copy_uploads(&mut encoder, &frame);
    let (index, done) = c.submit_frame(encoder.finish(), &frame);
    c.gpu
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
    if done.load(Ordering::Acquire) {
        c.release_layer_textures(frame);
    } else {
        c.retain(done, frame);
    }
    c.sweep_retired();
    c.upload_ring.lock().unwrap().advance();
}

#[test]
fn s3b_u2_resident_and_retiring_bytes_share_the_budget() {
    let gpu = fixture_gpu_or_skip().expect("actual GPU required");
    let c = Compositor::new(gpu.clone());
    resident_draw(&c, &ResidentInput::new(100));
    c.clear_residency();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    COMPLETE_HOLD.set(Some(tx));
    let mut delivery = None;
    let pixels = Arc::new(vec![f16::from_f32(0.5); 1024 * 1024 * 4]);
    for id in 0..9 {
        let mut input = ResidentInput::new(id);
        input.frame = WorkingFrame {
            width: 1024,
            height: 1024,
            pixels: pixels.clone(),
        };
        let ((), bytes) = uploaded_bytes(|| resident_upload(&c, &input));
        if id == 0 {
            delivery = Some(Deferred(Some(
                rx.recv_timeout(Duration::from_secs(5))
                    .expect("actual sampling completion"),
            )));
        }
        assert_eq!(bytes, 8 * 1024 * 1024);
        assert!(
            residents(&c).1 <= 64 * 1024 * 1024,
            "64 MiB includes retirement"
        );
        assert_eq!(gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
    }
    assert_eq!(
        residents(&c).0,
        7,
        "in-flight retirement takes one entry's bytes"
    );
    assert_eq!(residents(&c).1, 64 * 1024 * 1024);
    delivery.take().unwrap().deliver();
    settled(&c);
    c.sweep_retired();
    c.texture_pool.lock().unwrap().maintain(false);
    assert_eq!(residents(&c).1, 56 * 1024 * 1024);
    assert_eq!(gpu.ledger().live_bytes(), ledger_probes::inventory(&c));
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}
