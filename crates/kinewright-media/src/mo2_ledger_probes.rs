//! MO2 final render verification (final-mo2-1 B1, B2, S1): the ledger's
//! accounting probes, retained as committed regressions. The two allocation
//! witnesses read wgpu's own buffer counters (the `counters` feature, a
//! dev-dependency), independent of the ledger.

#![allow(clippy::used_underscore_binding)]

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

fn layer(frame: &WorkingFrame) -> CompositorLayer<'_, WorkingFrame> {
    CompositorLayer {
        frame,
        effects: &[],
        transition: TransitionRenderParams::default(),
        mode: LayerMode::NORMAL,
    }
}

fn buffer_bytes(gpu: &GpuContext) -> u64 {
    let bytes = gpu.device.get_internal_counters().hal.buffer_memory.read();
    let bytes = u64::try_from(bytes).unwrap();
    assert!(bytes > 0, "wgpu counters are on in tests");
    bytes
}

fn settle(gpu: &GpuContext) {
    gpu.queue.submit([]);
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
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
    // 32 × 8 bytes is one 256-byte aligned row: no padding.
    let upload = 32 * 4 * 8;
    assert_eq!(
        held._staging.1,
        upload + held._uniform.size() + held._grade.size()
    );
    assert_eq!(held._uniform.1, held._uniform.size());
    assert_eq!(held._grade.1, held._grade.size());
    let expected = baseline + output.1 + held._uniform.1 + held._grade.1 + held._staging.1;
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

/// B1: an upload's staging charge bounds wgpu's padded staging from above,
/// by at most one copy alignment per row. Only `upload_layer` runs between
/// the counter reads.
#[test]
fn final_ledger_upload_padding_counterexample() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let slack = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    for width in [32, 1, 17, 33] {
        let src = frame(width, 64);
        let layers = [layer(&src)];
        c.render_working((width, 64), &layers).unwrap();
        settle(&gpu);
        let before = buffer_bytes(&gpu);
        let uploaded = c.upload_layer(&layers[0]).unwrap();
        let actual = buffer_bytes(&gpu) - before;
        let charged = upload_staging_bytes(&uploaded);
        println!("LEDGER_UPLOAD width={width} height=64 actual={actual} charged={charged}");
        assert!(charged >= actual, "width {width}: {charged} < {actual}");
        assert!(
            charged <= actual + slack * 64,
            "width {width}: {charged} ≫ {actual}"
        );
        drop(uploaded);
        settle(&gpu);
        // The frame's staging charge is this bound plus its two buffers.
        let (output, resources, encoder) = c.composite(width, 64, &layers, None, None).unwrap();
        let held = &resources.layers[0];
        let buffers = held._uniform.size() + held._grade.size();
        assert_eq!(
            held._staging.1,
            charged + buffers,
            "width {width}: frame staging"
        );
        drop((output, encoder));
        c.release_layer_textures(resources);
        settle(&gpu);
    }
}

/// B2: a failed frame's charges outlive its queued uploads — repeated
/// failures never leave uncharged staging behind.
#[test]
fn final_ledger_failed_frame_retains_pending_uploads() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let c = Compositor::new(gpu.clone());
    let src = frame(32, 64);
    c.render_working((32, 64), &[layer(&src)]).unwrap();
    settle(&gpu);
    let before = buffer_bytes(&gpu);
    let baseline = gpu.ledger().live_bytes();
    let broken = WorkingFrame {
        width: 32,
        height: 64,
        pixels: Arc::new(vec![]),
    };
    for attempt in 1..=4 {
        let failed = c.render_working((32, 64), &[layer(&src), layer(&broken)]);
        assert!(failed.is_err());
        let pending = buffer_bytes(&gpu).saturating_sub(before);
        let live = gpu.ledger().live_bytes();
        println!(
            "LEDGER_ERROR attempt={attempt} pending={pending} live={live} baseline={baseline}"
        );
        assert!(
            live >= baseline + pending,
            "queued uploads outlive the charges"
        );
    }
    settle(&gpu);
}
