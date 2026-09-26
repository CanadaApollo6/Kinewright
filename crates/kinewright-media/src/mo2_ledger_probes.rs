//! MO2 final render verification (final-mo2-1 B1, B2, S1; rereview S2):
//! the ledger's accounting probes, retained as committed regressions.
//!
//! ME15: the ledger is API-level — it charges the bytes MO2 asks wgpu for.
//! wgpu's own buffer counters (the `counters` feature, a dev-dependency)
//! only report the backend's allocation, which may round up below the API
//! (DX12 64 KiB placement, Vulkan suballocation); they never gate.

#![allow(clippy::used_underscore_binding)]

use std::time::{Duration, Instant};

use super::*;
use crate::frame::WorkingFrame;
use crate::gpu_test_support::fixture_gpu_or_skip;
use half::f16;

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
    let (output, resources, encoder) = c.composite(32, 4, &layers, None, None).unwrap();
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
    c.readback_for(32, 4, &output, encoder, &resources, &monitor)
        .unwrap();
    drop(output);
    c.release_layer_textures(resources);
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
