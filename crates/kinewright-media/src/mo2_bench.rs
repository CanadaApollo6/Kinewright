//! MO2 R28 (gate 10): the preview benchmark — completed frames through the
//! `FrameRenderer` playback path (proxy raster, sequential decode, render,
//! readback), one in flight. Its workloads are fixtures (`mo2_perf_fixtures`).

use std::time::{Duration, Instant};

use kinewright_core::{Document, TimeCode};

use crate::{
    compositor::GpuContext,
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
};

/// One run's result (milliseconds; bytes).
pub(crate) struct Run {
    pub dims: (u32, u32),
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub ledger_peak: u64,
}

/// R28's protocol: 30 warmup + 300 measured frames, `delay` injected per
/// frame (the slowdown control), on a fresh renderer.
pub(crate) fn run(gpu: &GpuContext, document: &Document, delay: Duration) -> Run {
    let scale = RenderScale::Proxy {
        max_width: PREVIEW_MAX_WIDTH,
    };
    let dims = scale.output_resolution(document.resolution);
    let mut renderer = FrameRenderer::new(gpu.clone());
    let mut ms = Vec::with_capacity(300);
    for frame in 0..330 {
        let started = Instant::now();
        let at = TimeCode(frame % document.duration.0);
        let output = renderer.render(document, at, dims, scale, DecodeStrategy::Sequential);
        assert_eq!(output.expect("an R28 frame renders").width, dims.0);
        std::thread::sleep(delay);
        (frame >= 30).then(|| ms.push(started.elapsed().as_secs_f64() * 1e3));
    }
    ms.sort_by(f64::total_cmp);
    let mean_ms = ms.iter().sum::<f64>() / 300.0;
    Run {
        dims,
        mean_ms,
        p95_ms: ms[284],
        ledger_peak: gpu.ledger().peak_bytes(),
    }
}
