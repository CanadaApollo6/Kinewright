//! MO2 final render verification (final-mo2-1 B1, B2, S1; rereview S2):
//! the ledger's accounting probes, retained as committed regressions.
//!
//! ME15: the ledger is API-level — it charges the bytes MO2 asks wgpu for.
//! wgpu's own buffer counters (the `counters` feature, a dev-dependency)
//! only report the backend's allocation, which may round up below the API
//! (DX12 64 KiB placement, Vulkan suballocation); they never gate.

#![allow(clippy::used_underscore_binding)]

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use super::*;
use crate::frame::WorkingFrame;
use crate::gpu_test_support::fixture_gpu_or_skip;
use half::f16;

type Poll = Result<wgpu::PollStatus, wgpu::PollError>;
type Hook = Box<dyn FnMut(&wgpu::Device, &wgpu::PollType) -> Option<Poll>>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static UPLOAD_COPIES: Cell<usize> = const { Cell::new(0) };
}

/// ME16: every frame-path poll on this thread passes here first; a hook may
/// observe it (`None`: poll for real) or answer in its place.
pub(super) fn hook(device: &wgpu::Device, wait: &wgpu::PollType) -> Option<Poll> {
    HOOK.with_borrow_mut(|hook| hook.as_mut().and_then(|hook| hook(device, wait)))
}

/// H10: counts the frame's layer-upload copies on this thread.
pub(super) fn count_upload_copy() {
    UPLOAD_COPIES.set(UPLOAD_COPIES.get() + 1);
}

fn with_hook<T>(
    hook: impl FnMut(&wgpu::Device, &wgpu::PollType) -> Option<Poll> + 'static,
    body: impl FnOnce() -> T,
) -> T {
    HOOK.set(Some(Box::new(hook)));
    let result = body();
    HOOK.set(None);
    result
}

/// The wait's timeout, for a `Wait` that names its submission.
fn bound(wait: &wgpu::PollType) -> Option<Duration> {
    match wait {
        wgpu::PollType::Wait {
            submission_index: Some(_),
            timeout,
        } => *timeout,
        _ => None,
    }
}

fn frame(width: u32, height: u32) -> WorkingFrame {
    let len = usize::try_from(width * height * 4).unwrap();
    WorkingFrame {
        width,
        height,
        pixels: Arc::new(vec![f16::from_f32(0.5); len]),
    }
}

fn broken(width: u32, height: u32) -> WorkingFrame {
    WorkingFrame {
        width,
        height,
        pixels: Arc::new(vec![]),
    }
}

fn layer(frame: &WorkingFrame) -> CompositorLayer<'_, WorkingFrame> {
    CompositorLayer {
        frame,
        effects: &[],
        transition: TransitionRenderParams::default(),
        mode: LayerMode::NORMAL,
    }
}

/// The backend's buffer bytes, where the `counters` feature reports them.
fn backend_bytes(gpu: &GpuContext) -> i64 {
    i64::try_from(gpu.device.get_internal_counters().hal.buffer_memory.read()).unwrap_or(0)
}

fn settle(gpu: &GpuContext) {
    gpu.queue.submit([]);
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
}

fn retired(c: &Compositor) -> usize {
    c.retired.lock().unwrap().len()
}

/// A good layer staged, then a malformed one refused: the failed frame.
fn failed_frame(c: &Compositor, good: &WorkingFrame) -> FrameResources {
    let bad = broken(good.width, good.height);
    let mut staged = FrameResources::default();
    let layers = [layer(good), layer(&bad)];
    let (w, h) = (good.width, good.height);
    assert!(
        c.stage_layers(w, h, &layers, None, None, &mut staged)
            .is_err()
    );
    staged
}

/// S1 (F11/F12): every per-frame buffer and the staging are charged, exactly
/// once, and released with the frame.
#[test]
fn final_ledger_exact_resources_and_lifetime() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(32, 4);
    let layers = [layer(&src)];
    c.render_working((32, 4), &layers).unwrap();
    let baseline = gpu.ledger().live_bytes();
    let (output, mut resources, encoder) = c.composite(32, 4, &layers, None, None).unwrap();
    let held = &resources.layers[0];
    let (upload, row) = held.upload.as_ref().unwrap();
    // 32 × 8 bytes is one 256-byte aligned row: no padding.
    assert_eq!(
        (upload.1, upload.size(), *row),
        (32 * 4 * 8, 32 * 4 * 8, 256)
    );
    assert_eq!(held._staging.1, held._uniform.size() + held._grade.size());
    assert_eq!(held._uniform.1, held._uniform.size());
    assert_eq!(held._grade.1, held._grade.size());
    let expected =
        baseline + output.1 + held._uniform.1 + held._grade.1 + held._staging.1 + upload.1;
    assert_eq!(
        gpu.ledger().live_bytes(),
        expected,
        "every per-frame resource counted"
    );
    let monitor = kinewright_core::ColorContext::sdr_rec709().monitoring;
    c.readback_for(32, 4, &output, encoder, &mut resources, &monitor)
        .unwrap();
    c.finish_frame(output, resources);
    assert_eq!(
        gpu.ledger().live_bytes(),
        baseline,
        "idle source and flags stay charged exactly once"
    );
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

/// B1 → ME15 (and G02): an upload's staging is the buffer MO2 creates — rows
/// padded to wgpu's 256-byte `copy_buffer_to_texture` contract — charged
/// exactly, on every backend. The backend delta is reported, never gated.
#[test]
fn final_ledger_upload_padding_counterexample() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    for width in [1, 17, 31, 32, 33, 63, 64, 65] {
        for height in [1, 3, 64] {
            let src = frame(width, height);
            let layers = [layer(&src)];
            settle(&gpu);
            let (before, live) = (backend_bytes(&gpu), gpu.ledger().live_bytes());
            let (_, texture, (staging, row)) = c.upload_layer(&layers[0]).unwrap();
            let backend = backend_bytes(&gpu) - before;
            let requested = u64::from(width * 8).next_multiple_of(256) * u64::from(height);
            let tag = format!("{width}x{height}");
            assert_eq!(
                u64::from(row),
                u64::from(width * 8).next_multiple_of(256),
                "{tag}"
            );
            assert_eq!(
                staging.size(),
                requested,
                "{tag}: the API bytes MO2 creates"
            );
            assert_eq!(staging.1, requested, "{tag}: charged exactly");
            assert_eq!(
                gpu.ledger().live_bytes() - live,
                requested + texture.1,
                "{tag}: staging + a new pooled texture"
            );
            if height == 64 {
                println!("LEDGER_UPLOAD {tag} charged={requested} backend_delta={backend}");
            }
            drop((texture, staging));
            // The frame copies exactly that staging into its source.
            let (output, resources, encoder) =
                c.composite(width, height, &layers, None, None).unwrap();
            let (upload, _) = resources.layers[0].upload.as_ref().unwrap();
            assert_eq!(upload.1, requested, "{tag}: frame staging");
            drop((output, encoder));
            c.release_layer_textures(resources);
        }
    }
    settle(&gpu);
}

/// B2: a failed frame releases its charges only after its queued writes
/// completed (the bounded wait normally succeeds): nothing accumulates.
#[test]
fn final_ledger_failed_frame_retains_pending_uploads() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(32, 64);
    let bad = broken(32, 64);
    // Warm the pools: a two-layer frame pools two validity slots.
    c.render_working((32, 64), &[layer(&src)]).unwrap();
    assert!(
        c.render_working((32, 64), &[layer(&src), layer(&bad)])
            .is_err()
    );
    settle(&gpu);
    let before = backend_bytes(&gpu);
    let baseline = gpu.ledger().live_bytes();
    for attempt in 1..=4 {
        let failed = c.render_working((32, 64), &[layer(&src), layer(&bad)]);
        assert!(failed.is_err());
        let live = gpu.ledger().live_bytes();
        let backend = backend_bytes(&gpu) - before;
        println!(
            "LEDGER_ERROR attempt={attempt} live={live} baseline={baseline} backend_delta={backend}"
        );
        assert_eq!(retired(&c), 0, "the bounded wait completed");
        // The failed frame's source texture went back to the pool.
        assert_eq!(live, baseline, "released after completion, no accumulation");
    }
    settle(&gpu);
}

/// Rereview B2: the failed-frame wait is finite.
#[test]
fn reverify_b2_requires_bounded_wait() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let staged = failed_frame(&c, &frame(32, 64));
    let mut waited = None;
    c.retire_failed(staged, |device, wait| {
        waited = Some(format!("{wait:?}"));
        let wgpu::PollType::Wait {
            timeout: Some(timeout),
            ..
        } = wait
        else {
            panic!("failed-frame flush has no finite timeout: {wait:?}");
        };
        assert!(timeout <= Duration::from_secs(1), "{timeout:?}");
        device.poll(wait)
    });
    assert!(waited.is_some(), "the flush polls");
    assert_eq!(retired(&c), 0);
}

/// Rereview B2: a poll that does not observe completion (a timeout) keeps
/// every charge; a later poll that does releases them.
#[test]
fn reverify_b2_poll_error_preserves_charges() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(32, 64);
    c.render_working((32, 64), &[layer(&src)]).unwrap();
    settle(&gpu);
    let base = gpu.ledger().live_bytes();
    let staged = failed_frame(&c, &src);
    let charged = gpu.ledger().live_bytes();
    assert!(charged > base, "the failed frame staged charges");
    c.retire_failed(staged, |_, _| Err(wgpu::PollError::Timeout));
    assert_eq!(retired(&c), 1, "retained, not released");
    assert_eq!(
        gpu.ledger().live_bytes(),
        charged,
        "a timeout is not completion"
    );
    // A frame before any poll keeps it: nothing observed completion yet.
    c.sweep_retired();
    assert_eq!(retired(&c), 1);
    // Later release: a poll completes the submission, the next frame sweeps.
    settle(&gpu);
    c.render_working((32, 64), &[layer(&src)]).unwrap();
    assert_eq!(retired(&c), 0, "released once completion was observed");
    assert_eq!(gpu.ledger().live_bytes(), base);
}

/// G08: the charges are still live when the flush polls — never released
/// ahead of completion.
#[test]
fn reverify_b2_flush_observes_live_charges() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(32, 64);
    c.render_working((32, 64), &[layer(&src)]).unwrap();
    let base = gpu.ledger().live_bytes();
    let staged = failed_frame(&c, &src);
    let charged = gpu.ledger().live_bytes();
    let mut observed = 0;
    c.retire_failed(staged, |device, wait| {
        observed = gpu.ledger().live_bytes();
        device.poll(wait)
    });
    println!(
        "FLUSH_LIFETIME base={base} at_poll={observed} after={}",
        gpu.ledger().live_bytes()
    );
    assert!(
        observed >= base + 32 * 64 * 8,
        "charges dropped before the poll"
    );
    assert_eq!(observed, charged);
}

/// G09/G10: the three printed phases partition the same measured frame.
#[test]
fn reverify_me14_phases_sum_same_frame() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let src = frame(160, 90);
    let layers = [layer(&src)];
    let monitor = kinewright_core::ColorContext::sdr_rec709().monitoring;
    for _ in 0..5 {
        let outer = Instant::now();
        let (phases, frame) =
            super::phases::monitor(&c, (160, 90), &layers, &monitor, None).unwrap();
        let wall = outer.elapsed();
        let sum: Duration = phases.into_iter().sum();
        assert_eq!(sum, frame, "{phases:?} partition the frame");
        assert!(sum <= wall);
        assert!(phases.iter().all(|phase| !phase.is_zero()), "{phases:?}");
    }
}

/// Rereview-2 H01: the cleanup wait is exactly 100 ms.
#[test]
fn rev2_exact_100ms_cleanup_argument() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let src = frame(17, 3);
    c.render_working((17, 3), &[layer(&src)]).unwrap();
    let staged = failed_frame(&c, &src);
    c.retire_failed(staged, |_, wait| {
        assert_eq!(bound(&wait), Some(Duration::from_millis(100)), "{wait:?}");
        Err(wgpu::PollError::Timeout)
    });
    assert_eq!(retired(&c), 1);
}

/// Rereview-2 G05: the cleanup waits on its own submission.
#[test]
fn rev2_failed_flush_has_submission_index() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let staged = failed_frame(&c, &frame(17, 3));
    c.retire_failed(staged, |device, wait| {
        assert!(
            bound(&wait).is_some(),
            "flush must target its writes: {wait:?}"
        );
        device.poll(wait)
    });
    assert_eq!(retired(&c), 0);
}

/// Rereview-2 H06: the completion callback, not the poll status, decides.
#[test]
fn rev2_callback_completion_overrules_error_status() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(17, 3);
    c.render_working((17, 3), &[layer(&src)]).unwrap();
    let base = gpu.ledger().live_bytes();
    let staged = failed_frame(&c, &src);
    c.retire_failed(staged, |device, wait| {
        device.poll(wait).unwrap();
        Err(wgpu::PollError::Timeout)
    });
    assert_eq!(retired(&c), 0);
    // The two-layer frame pools a two-slot flag buffer; all else releases.
    assert_eq!(gpu.ledger().live_bytes(), base + c.validity_stride);
}

/// Rereview-2 G07: every staging refusal — malformed pixels, a missing LUT,
/// a grade stack over its limit; normal and special; all three entries —
/// cleans up with exactly one 100 ms wait and no readback wait.
#[test]
fn rev2_all_staging_error_paths_have_100ms_cleanup() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let (src, bad) = (frame(17, 3), broken(17, 3));
    let missing = vec![crate::mo2_fixtures::effect(1, "cube_lut", &[])];
    let grade = (0..65)
        .map(|i| {
            let exposure = [("exposure_milli_stops", 1000)];
            crate::mo2_fixtures::effect(i, "primary_correction", &exposure)
        })
        .collect::<Vec<_>>();
    let waits = std::rc::Rc::new(RefCell::new(Vec::new()));
    let seen = std::rc::Rc::clone(&waits);
    let monitor = kinewright_core::ColorContext::sdr_rec709().monitoring;
    with_hook(
        move |_, wait| {
            seen.borrow_mut().push(bound(wait));
            None
        },
        || {
            for special in [false, true] {
                for kind in 0..3 {
                    for entry in 0..3 {
                        let mut good = layer(&src);
                        if special {
                            good.mode.blend = BlendMode::Screen;
                            good.transition.backdrop = Some([0.5, 0.0]);
                        }
                        let failing = match kind {
                            0 => layer(&bad),
                            1 => CompositorLayer {
                                effects: &missing,
                                ..layer(&src)
                            },
                            _ => CompositorLayer {
                                effects: &grade,
                                ..layer(&src)
                            },
                        };
                        let layers = [good, failing];
                        let before = waits.borrow().len();
                        let failed = match entry {
                            0 => c.render_working((17, 3), &layers).is_err(),
                            1 => c.render_monitor((17, 3), &layers, &monitor).is_err(),
                            _ => c.render_delivery((17, 3), &layers, &monitor).is_err(),
                        };
                        let case = format!("special={special} kind={kind} entry={entry}");
                        assert!(failed, "{case}");
                        let new = waits.borrow()[before..].to_vec();
                        assert_eq!(new, [Some(Duration::from_millis(100))], "{case}");
                    }
                }
            }
        },
    );
    assert_eq!(waits.borrow().len(), 18);
}

/// Rereview-2 H10: one upload copy per pixel layer, and the pixels arrive.
#[test]
fn rev2_upload_pixels_rgba8_rgba16_and_copy_count() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    for (width, height) in [(1, 1), (17, 3), (33, 64)] {
        let mut src = frame(width, height);
        let pixels = Arc::make_mut(&mut src.pixels);
        for (i, p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let quarter = |n: usize| f16::from_f32([0.0, 0.25, 0.5, 0.75][n % 4]);
            let (x, y) = (quarter(i), quarter(i / width as usize));
            *p = [x, y, f16::ZERO, f16::ONE];
        }
        UPLOAD_COPIES.set(0);
        let output = c.render_working((width, height), &[layer(&src)]).unwrap();
        assert_eq!(UPLOAD_COPIES.get(), 1, "RGBA16F {width}x{height}");
        let expected = src.pixels.iter().map(|v| v.to_f32());
        assert!(
            output.pixels.iter().copied().eq(expected),
            "RGBA16F {width}x{height}"
        );
        let rgba = (0..width * height)
            .flat_map(|i| [if i % 2 == 0 { 0 } else { 255 }, 0, 255, 255])
            .collect::<Vec<u8>>();
        let src = FrameTexture {
            width,
            height,
            rgba: Arc::new(rgba),
        };
        let layers = [CompositorLayer {
            frame: &src,
            effects: &[],
            transition: TransitionRenderParams::default(),
            mode: LayerMode::NORMAL,
        }];
        UPLOAD_COPIES.set(0);
        let output = c.render_working((width, height), &layers).unwrap();
        assert_eq!(UPLOAD_COPIES.get(), 1, "RGBA8 {width}x{height}");
        let expected = src.rgba.iter().map(|&b| f32::from(b) / 255.0);
        assert!(
            output.pixels.iter().copied().eq(expected),
            "RGBA8 {width}x{height}"
        );
    }
}

/// Rereview-2 B1 → ME16: the atlas upload's staging is charged — API
/// bytes, 256-padded rows — until a poll observes its copy done.
#[test]
fn rev2_api_atlas_write_staging_is_charged() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    settle(&gpu);
    let before = gpu.ledger().live_bytes();
    let slots = [LutAtlasSlot {
        z_origin: 0,
        lut: Arc::clone(&c.identity_lut),
    }];
    let size = u64::from(c.identity_lut.size);
    let data = u64::try_from(c.identity_lut.rgba.len()).unwrap() * 4;
    let atlas = c.build_lut_atlas(&slots).unwrap();
    let delta = gpu.ledger().live_bytes() - before;
    let staging = (size * 16).next_multiple_of(256) * size * size;
    println!(
        "ATLAS_API texture={} write_data={data} staging={staging} charged={delta}",
        atlas.texture.1
    );
    assert!(
        delta >= atlas.texture.1 + data,
        "the atlas write is charged"
    );
    assert_eq!(delta, atlas.texture.1 + staging);
    assert_eq!(retired(&c), 1, "held until its copy completes");
    settle(&gpu);
    c.sweep_retired();
    assert_eq!(gpu.ledger().live_bytes() - before, atlas.texture.1);
}

/// The readback wait: its own submission, no deadline (N26).
fn waits_for_completion(wait: &wgpu::PollType) -> bool {
    matches!(
        wait,
        wgpu::PollType::Wait {
            submission_index: Some(_),
            timeout: None,
        }
    )
}

/// Every charge a retained frame still holds.
fn owned(r: &FrameResources) -> u64 {
    let layers = r.layers.iter().map(|l| {
        l.source.as_ref().map_or(0, |(_, t)| t.1)
            + l.upload.as_ref().map_or(0, |(b, _)| b.1)
            + l._uniform.1
            + l._grade.1
            + l._staging.1
    });
    layers.sum::<u64>()
        + r.snapshots.iter().map(|(_, t)| t.1).sum::<u64>()
        + r.validity.as_ref().map_or(0, |b| b.1)
        + r.pooled_flags.as_ref().map_or(0, |b| b.1)
        + r.output.as_ref().map_or(0, |t| t.1)
        + r.buffers.iter().map(|b| b.1).sum::<u64>()
}

/// Everything the compositor holds charged, listed independently.
fn inventory(c: &Compositor) -> u64 {
    let atlases = c.lut_atlas_cache.lock().unwrap();
    let pool = c.texture_pool.lock().unwrap();
    c.dummy_accumulator.1.1
        + c.dummy_validity.1
        + atlases.iter().map(|a| a.texture.1).sum::<u64>()
        + pool.shapes.values().flatten().map(|t| t.1).sum::<u64>()
        + c.flag_pool.lock().unwrap().as_ref().map_or(0, |b| b.1)
        + c.retired
            .lock()
            .unwrap()
            .iter()
            .map(|(_, r)| owned(r))
            .sum::<u64>()
}

/// A two-layer frame through read-back entry `which`: working, monitor,
/// delivery or matte; the top layer carries a matte, `special` blends it.
fn entry(c: &Compositor, f: &WorkingFrame, which: usize, special: bool) -> Result<(), MediaError> {
    let matte = [
        ("exposure_milli_stops", 0),
        ("matte_enabled", 1),
        ("matte_window_count", 1),
        ("matte_window0_shape_token", 1),
        ("matte_window0_half_width_basis_points", 2500),
        ("matte_window0_half_height_basis_points", 2500),
    ];
    let effects = [crate::mo2_fixtures::effect(1, "primary_correction", &matte)];
    let mut top = CompositorLayer {
        effects: &effects,
        ..layer(f)
    };
    if special {
        top.mode.blend = BlendMode::Screen;
        top.transition.backdrop = Some([0.5, 0.0]);
    }
    let layers = [layer(f), top];
    let dims = (f.width, f.height);
    let monitor = kinewright_core::ColorContext::sdr_rec709().monitoring;
    match which {
        0 => c.render_working(dims, &layers).map(|_| ()),
        1 => c.render_monitor(dims, &layers, &monitor).map(|_| ()),
        2 => c.render_delivery(dims, &layers, &monitor).map(|_| ()),
        _ => {
            let target = MatteRenderTarget {
                layer_index: 1,
                clip: kinewright_core::ClipId(1),
                effect: kinewright_core::EffectId(1),
            };
            c.render_matte(dims, &layers, None, target).map(|_| ())
        }
    }
}

/// N26: the readback, the wait an R10 refusal passes through, waits for its
/// own submission to complete — no application deadline.
#[test]
fn rev3_runtime_refusal_readback_waits_for_completion() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu);
    let mut src = frame(17, 3);
    Arc::make_mut(&mut src.pixels)[3] = f16::NAN;
    let waits = std::rc::Rc::new(RefCell::new(Vec::new()));
    let seen = std::rc::Rc::clone(&waits);
    let result = with_hook(
        move |_, wait| {
            seen.borrow_mut().push(waits_for_completion(wait));
            None
        },
        || c.render_working((17, 3), &[layer(&src)]),
    );
    assert!(
        matches!(result, Err(MediaError::NonFiniteRender { .. })),
        "{result:?}"
    );
    assert_eq!(*waits.borrow(), [true], "one wait, for completion");
}

/// N26: a readback poll that errors (device loss, a wgpu error) without the
/// map completing refuses and keeps every submitted charge (output,
/// readback, layers) until completion is observed, then releases them
/// exactly once.
#[test]
fn rev3_readback_poll_error_keeps_submitted_charges() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(17, 64);
    c.render_working((17, 64), &[layer(&src)]).unwrap();
    settle(&gpu);
    let base = gpu.ledger().live_bytes();
    let at_poll = std::rc::Rc::new(Cell::new(0));
    let seen = std::rc::Rc::clone(&at_poll);
    let ledger = gpu.clone();
    let result = with_hook(
        move |_, wait| {
            assert!(waits_for_completion(wait), "{wait:?}");
            seen.set(ledger.ledger().live_bytes());
            Some(Err(wgpu::PollError::Timeout))
        },
        || c.render_working((17, 64), &[layer(&src)]),
    );
    let Err(MediaError::Backend(message)) = result else {
        panic!("an unfinished readback refuses: {result:?}");
    };
    assert!(
        message.starts_with("wgpu readback poll failed"),
        "{message}"
    );
    let live = gpu.ledger().live_bytes();
    println!(
        "READBACK_POLL_ERROR base={base} at_poll={} returned={live}",
        at_poll.get()
    );
    assert!(
        at_poll.get() > base + 17 * 64 * 8,
        "output + readback charged"
    );
    assert_eq!(live, at_poll.get(), "nothing released before completion");
    assert_eq!(retired(&c), 1);
    c.sweep_retired();
    assert_eq!(
        gpu.ledger().live_bytes(),
        live,
        "no completion observed yet"
    );
    // Later: a poll observes completion; the next sweep releases it once.
    settle(&gpu);
    c.sweep_retired();
    assert_eq!(retired(&c), 0);
    c.render_working((17, 64), &[layer(&src)]).unwrap();
    settle(&gpu);
    c.sweep_retired();
    assert_eq!(gpu.ledger().live_bytes(), base, "released exactly once");
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

/// Rereview-3 I06: repeated unfinished readbacks through every entry, normal
/// and special, each retain all their charges; completion recovers to the
/// baseline; teardown releases what is still pending.
#[test]
fn rev3_repeated_readback_poll_errors_all_entries() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = std::rc::Rc::new(Compositor::new(gpu.clone()));
    let src = frame(17, 3);
    let empty_pools = |c: &Compositor| {
        *c.texture_pool.lock().unwrap() = TexturePool::default();
        *c.flag_pool.lock().unwrap() = None;
    };
    entry(&c, &src, 0, true).unwrap();
    empty_pools(&c);
    let base = gpu.ledger().live_bytes();
    for cycle in 0..3 {
        for i in 0..16 {
            let weak = std::rc::Rc::downgrade(&c);
            let expected = std::rc::Rc::new(Cell::new(0));
            let seen = std::rc::Rc::clone(&expected);
            let ledger = gpu.clone();
            let result = with_hook(
                move |_, wait| {
                    assert!(waits_for_completion(wait), "{wait:?}");
                    let c = weak.upgrade().unwrap();
                    // A submit may run old callbacks; completed frames may be
                    // swept at finish, this one may not.
                    let retired = c.retired.lock().unwrap();
                    let completed = retired
                        .iter()
                        .filter(|(done, _)| done.load(Ordering::Acquire));
                    let completed = completed.map(|(_, r)| owned(r)).sum::<u64>();
                    seen.set(ledger.ledger().live_bytes() - completed);
                    // Non-completion: a poll error, or a poll that returned
                    // without the map callback.
                    Some(if i % 2 == 0 {
                        Err(wgpu::PollError::Timeout)
                    } else {
                        Ok(wgpu::PollStatus::Poll)
                    })
                },
                || entry(&c, &src, i % 4, i % 8 >= 4),
            );
            let case = format!("cycle={cycle} i={i}");
            assert!(
                matches!(result, Err(MediaError::Backend(_))),
                "{case}: {result:?}"
            );
            assert_eq!(
                gpu.ledger().live_bytes(),
                expected.get(),
                "{case}: all retained"
            );
            assert_eq!(gpu.ledger().live_bytes(), inventory(&c), "{case}");
            let before = gpu.ledger().live_bytes();
            c.sweep_retired();
            c.sweep_retired();
            assert_eq!(gpu.ledger().live_bytes(), before, "{case}");
        }
        if cycle == 2 {
            break;
        }
        settle(&gpu);
        c.sweep_retired();
        assert_eq!(retired(&c), 0);
        assert_eq!(gpu.ledger().live_bytes(), base, "cycle={cycle}: recovered");
        for which in 0..4 {
            entry(&c, &src, which, true).unwrap();
        }
        empty_pools(&c);
        assert_eq!(gpu.ledger().live_bytes(), base);
    }
    assert!(retired(&c) > 0, "teardown with frames pending");
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
    settle(&gpu);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

/// Rereview-3 I07: a completed map wins over an error status, on every entry.
#[test]
fn rev3_readback_callback_overrules_poll_error() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(17, 3);
    for which in 0..4 {
        let result = with_hook(
            |device, wait| {
                device.poll(wait.clone()).unwrap();
                Some(Err(wgpu::PollError::Timeout))
            },
            || entry(&c, &src, which, true),
        );
        result.unwrap();
        assert_eq!(retired(&c), 0);
    }
    drop(c);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

/// Rereview-3 I08: the frame issues one upload copy per layer — at the API,
/// which the H10 counter alone cannot see.
#[test]
fn rev3_actual_upload_callsite_count() {
    let text = include_str!("compositor.rs").replace("\r\n", "\n");
    let function = text.split("fn copy_uploads(").nth(1).unwrap();
    let function = function.split("pub(crate) struct Ledgered").next().unwrap();
    assert_eq!(function.matches(".copy_buffer_to_texture(").count(), 1);
}

/// N26 (the CI frame-14 failure): another thread's submit can collect a
/// completed readback's map callback and run it after this thread's wait
/// returned. Readbacks on threads sharing one device must never refuse.
#[test]
fn rev3_concurrent_readbacks_never_refuse() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let threads = (0..8)
        .map(|_| {
            let gpu = gpu.clone();
            std::thread::spawn(move || {
                let c = Compositor::new(gpu);
                let src = frame(8, 8);
                (0..600)
                    .filter(|_| c.render_working((8, 8), &[layer(&src)]).is_err())
                    .count()
            })
        })
        .collect::<Vec<_>>();
    let refused = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .sum::<usize>();
    assert_eq!(refused, 0, "completed readbacks refused");
}
