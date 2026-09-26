//! MO2 R28 (gate 10): the preview benchmark — completed frames through the
//! `FrameRenderer` playback path (proxy raster, sequential decode, render,
//! readback), one in flight. Its workloads are fixtures (`mo2_perf_fixtures`).

use std::time::{Duration, Instant};

use kinewright_core::{Document, TimeCode};

use crate::{
    compositor::GpuContext,
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
};

/// One run's result (milliseconds); the caller reads its context's ledger.
pub(crate) struct Run {
    pub dims: (u32, u32),
    pub mean_ms: f64,
    pub p95_ms: f64,
}

/// R28's protocol: 30 warmup + 300 measured frames on a fresh renderer, plus
/// `delay` per frame (the slowdown control). `resident` (ME13) times only the
/// compositor frame, sources held resident; otherwise the whole preview frame.
pub(crate) fn run(gpu: &GpuContext, document: &Document, delay: Duration, resident: bool) -> Run {
    let scale = RenderScale::Proxy {
        max_width: PREVIEW_MAX_WIDTH,
    };
    let dims = scale.output_resolution(document.resolution);
    let mut renderer = FrameRenderer::new(gpu.clone());
    if resident {
        renderer.set_cache_budget(1 << 30);
    }
    let mut ms = Vec::with_capacity(300);
    for frame in 0..330 {
        let started = Instant::now();
        let at = TimeCode(frame % document.duration.0);
        let output = renderer.render_timed(document, at, dims, scale, DecodeStrategy::Sequential);
        let (output, composite) = output.expect("an R28 frame renders");
        assert_eq!(output.width, dims.0);
        let timed = if resident {
            composite
        } else {
            started.elapsed()
        };
        let paused = Instant::now();
        std::thread::sleep(delay);
        let elapsed = timed + paused.elapsed();
        (frame >= 30).then(|| ms.push(elapsed.as_secs_f64() * 1e3));
    }
    ms.sort_by(f64::total_cmp);
    let mean_ms = ms.iter().sum::<f64>() / 300.0;
    let p95_ms = ms[284];
    Run {
        dims,
        mean_ms,
        p95_ms,
    }
}
