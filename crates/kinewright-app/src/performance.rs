//! Opt-in desktop measurements. Normal launches allocate no sample buffer.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui;
use serde::Serialize;

pub(crate) struct PerformanceProbe {
    started: Instant,
    output: PathBuf,
    duration: Duration,
    first_ui_ms: Option<f64>,
    first_preview_ms: Option<f64>,
    samples: Vec<f64>,
    finished: bool,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    first_ui_ms: Option<f64>,
    first_preview_ms: Option<f64>,
    elapsed_seconds: f64,
    ui_samples_after_warmup: usize,
    ui_median_ms: Option<f64>,
    ui_p95_ms: Option<f64>,
    ui_max_ms: Option<f64>,
}

impl PerformanceProbe {
    pub(crate) fn from_environment(started: Instant) -> Option<Self> {
        let output = std::env::var_os("KINEWRIGHT_PERF_REPORT")?;
        let seconds = std::env::var("KINEWRIGHT_PERF_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10)
            .clamp(3, 300);
        Some(Self {
            started,
            output: output.into(),
            duration: Duration::from_secs(seconds),
            first_ui_ms: None,
            first_preview_ms: None,
            samples: Vec::new(),
            finished: false,
        })
    }

    /// Measures elapsed time spent building the UI, excluding GPU presentation.
    /// The first two seconds are warmup. The deadline requests a clean exit.
    pub(crate) fn frame(&mut self, ctx: &egui::Context, began: Instant, preview: bool) -> bool {
        if self.finished {
            return true;
        }
        let elapsed = self.started.elapsed();
        self.first_ui_ms
            .get_or_insert(elapsed.as_secs_f64() * 1_000.0);
        if preview {
            self.first_preview_ms
                .get_or_insert(elapsed.as_secs_f64() * 1_000.0);
        }
        if elapsed >= Duration::from_secs(2) && self.samples.len() < 100_000 {
            self.samples.push(began.elapsed().as_secs_f64() * 1_000.0);
        }
        if elapsed < self.duration {
            // Only schedule the deadline: do not turn an idle measurement into
            // a continuously repainting workload.
            ctx.request_repaint_after(self.duration.saturating_sub(elapsed));
            return false;
        }
        self.samples.sort_by(f64::total_cmp);
        let report = Report {
            schema_version: 1,
            first_ui_ms: self.first_ui_ms,
            first_preview_ms: self.first_preview_ms,
            elapsed_seconds: elapsed.as_secs_f64(),
            ui_samples_after_warmup: self.samples.len(),
            ui_median_ms: percentile(&self.samples, 50),
            ui_p95_ms: percentile(&self.samples, 95),
            ui_max_ms: self.samples.last().copied(),
        };
        match serde_json::to_vec_pretty(&report)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                std::fs::write(&self.output, bytes).map_err(|error| error.to_string())
            }) {
            Ok(()) => {}
            Err(error) => eprintln!("could not write performance report: {error}"),
        }
        self.finished = true;
        true
    }
}

fn percentile(sorted: &[f64], percent: usize) -> Option<f64> {
    let index = sorted
        .len()
        .saturating_mul(percent)
        .div_ceil(100)
        .saturating_sub(1);
    sorted.get(index).copied()
}
