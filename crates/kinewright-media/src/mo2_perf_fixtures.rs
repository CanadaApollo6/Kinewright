//! MO2 §13 gate 10 (`blend_heavy_holds_floors`) and R28's ledger.
//!
//! Lanes: the floors are release-mode evidence on three backends — the
//! `--ignored` fallback test is lavapipe on Linux and WARP in the Windows CI
//! lane, the `--ignored` hardware test is the RTX 3090. The ledger ceilings
//! and both failing controls also run in the default (lavapipe) lane.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

use std::time::{Duration, Instant};

use kinewright_core::{
    AssetId, BlendMode, Clip, ClipContent, Document, MediaAsset, TimeCode, Track, TrackId,
    TrackKind,
};

use crate::{
    compositor::GpuContext,
    decode::probe_path,
    gpu_test_support::fixture_gpu_or_skip,
    mo2_bench::{Run, run},
    mo2_fixtures::{clip, effect, with_transition},
    render::{DecodeStrategy, FrameRenderer, RenderScale},
    test_support::GeneratedMedia,
};

const MIB: u64 = 1 << 20;

/// One generated 30 fps BT.709 H.264 source (`performance_workloads`' args).
fn source(filter: &str, (w, h): (u32, u32), frames: i64, id: u64) -> (GeneratedMedia, MediaAsset) {
    let args = format!(
        "-f lavfi -i {filter}=size={w}x{h}:rate=30 -frames:v {frames} -c:v libx264 -preset \
         veryfast -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 \
         -color_range tv -x264-params colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv -g 60"
    );
    let media = GeneratedMedia::ffmpeg("mo2-r28", &args.split(' ').collect::<Vec<_>>(), "mp4");
    let asset = probe_path(media.path(), AssetId(id)).expect("the R28 source probes");
    (media, asset)
}

fn span(mut clip: Clip, asset: &MediaAsset, source: i64, len: i64, start: i64) -> Clip {
    (clip.asset, clip.timeline_start) = (asset.id, TimeCode(start));
    clip.source_range = TimeCode(source)..TimeCode(source + len);
    clip
}

fn document(
    resolution: (u32, u32),
    media: &[(GeneratedMedia, MediaAsset)],
    tracks: Vec<Vec<Clip>>,
) -> Document {
    let track = |(index, clips): (usize, Vec<Clip>)| Track {
        id: TrackId(index as u64 + 1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips,
    };
    let end =
        |clip: &Clip| clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0;
    let duration = tracks.iter().flatten().map(end).max().unwrap_or(0);
    let document = Document {
        resolution,
        duration: TimeCode(duration),
        tracks: tracks.into_iter().enumerate().map(track).collect(),
        media_pool: media.iter().map(|(_, asset)| asset.clone()).collect(),
        ..Document::default()
    };
    document.validate().expect("the R28 workloads are valid");
    document
}

/// A workload and the generated media it plays.
struct Workload(Document, Vec<(GeneratedMedia, MediaAsset)>);

/// `performance_workloads`' typical (1080p, 3 tracks, 24 clips × 15) and heavy
/// (4K, 4 tracks, 200 clips × 12) lanes: clips end to end, windows cycling.
fn cuts(resolution: (u32, u32), frames: i64, tracks: usize, clips: u64, len: i64) -> Workload {
    let media = [("testsrc2", 1), ("smptebars", 2)]
        .map(|(filter, id)| source(filter, resolution, frames, id));
    let mut lanes = vec![Vec::new(); tracks];
    for index in 0..clips {
        let asset = &media[index as usize % 2].1;
        let lane = &mut lanes[index as usize % tracks];
        let base = clip(index + 1, ClipContent::Media, BlendMode::Normal, Vec::new());
        let start = lane.len() as i64 * len;
        lane.push(span(
            base,
            asset,
            (index as i64 * 7) % (asset.duration.0 - len),
            len,
            start,
        ));
    }
    Workload(document(resolution, &media, lanes), media.into())
}

/// R28 `blend_heavy`: 4-track 1080p — presenter, picture-in-picture, a `Screen` leak whose
/// 30-frame clips each enter by a full-length `push_left`, and an adjustment.
fn blend_heavy(frames: i64) -> Workload {
    let filters = [("testsrc2", 1), ("smptebars", 2), ("gradients", 3)];
    let media = filters.map(|(filter, id)| source(filter, (1920, 1080), frames, id));
    let pip = vec![effect(
        1,
        "transform",
        &[("scale_percent", 35), ("x_percent", 30)],
    )];
    let leak = |at: i64| {
        let base = clip(
            10 + at.cast_unsigned(),
            ClipContent::Media,
            BlendMode::Screen,
            Vec::new(),
        );
        with_transition(span(base, &media[2].1, at, 30, at), "push_left", 30)
    };
    let look = vec![effect(
        2,
        "primary_correction",
        &[("saturation_percent", 40)],
    )];
    let mut adjustment = clip(3, ClipContent::Adjustment, BlendMode::Normal, look);
    adjustment.source_range = TimeCode(0)..TimeCode(frames);
    let whole = |id, effects, asset| {
        span(
            clip(id, ClipContent::Media, BlendMode::Normal, effects),
            asset,
            0,
            frames,
            0,
        )
    };
    let lanes = vec![
        vec![whole(1, Vec::new(), &media[0].1)],
        vec![whole(2, pip, &media[1].1)],
        (0..frames / 30).map(|at| leak(at * 30)).collect(),
        vec![adjustment],
    ];
    Workload(document((1920, 1080), &media, lanes), media.into())
}

/// R28 ceilings: 384 MiB at 1080p, 1,536 MiB at 4K.
fn ceiling(document: &Document) -> u64 {
    let (w, h) = document.resolution;
    if u64::from(w) * u64::from(h) <= 1920 * 1080 {
        384 * MIB
    } else {
        1536 * MIB
    }
}

/// R28 floors (fps): lavapipe 8, Windows/WARP 20, hardware (the RTX 3090) 60.
fn floor_fps(gpu: &GpuContext) -> f64 {
    let metadata = gpu.monitor_proof_metadata();
    match (metadata.software_fallback, metadata.backend.as_str()) {
        (false, _) => 60.0,
        (true, "dx12") => 20.0,
        (true, _) => 8.0,
    }
}

/// R28: mean fps ≥ the floor and p95 ≤ 3× the mean frame time.
fn throughput(run: &Run, floor: f64) -> Result<(), String> {
    let fps = 1e3 / run.mean_ms;
    if fps < floor || run.p95_ms > 3.0 * run.mean_ms {
        return Err(format!(
            "{fps:.1} fps (floor {floor}), p95 {:.1} ms vs mean {:.1} ms",
            run.p95_ms, run.mean_ms
        ));
    }
    Ok(())
}

/// R28: live + idle compositor bytes inside the workload's ceiling.
fn within_ceiling(peak: u64, document: &Document) -> Result<(), String> {
    let limit = ceiling(document);
    (peak <= limit)
        .then_some(())
        .ok_or(format!("ledger peak {peak} B > ceiling {limit} B"))
}

/// Pre-MO2 `Normal` baselines (mean ms over three runs, same protocol),
/// measured at f241aa5 on this machine's adapters. R28's 5% rule.
const BASELINES: &[(&str, &str, f64)] = &[];

fn workloads(only: Option<&str>) -> Vec<(&'static str, Workload, bool)> {
    let all: [(_, fn() -> Workload, _); 3] = [
        ("typical_1080p", || cuts((1920, 1080), 600, 3, 24, 15), true),
        ("blend_heavy_1080p", || blend_heavy(330), true),
        ("heavy_4k", || cuts((3840, 2160), 300, 4, 200, 12), false),
    ];
    let wanted = |key: &str| only.is_none_or(|only| only == key);
    all.into_iter()
        .filter(|(key, ..)| wanted(key))
        .map(|(key, make, floored)| (key, make(), floored))
        .collect()
}

/// Mean per-call `Document::validate()` cost (N11-4), microseconds.
fn validate_us(document: &Document) -> f64 {
    let started = Instant::now();
    for _ in 0..200 {
        document.validate().expect("valid");
    }
    started.elapsed().as_secs_f64() * 1e6 / 200.0
}

fn gate(acquire: fn() -> GpuContext) {
    let release = !cfg!(debug_assertions);
    assert!(
        release,
        "R28 floors are release evidence: cargo test --release"
    );
    let probe = acquire();
    let (floor, adapter) = (floor_fps(&probe), probe.monitor_proof_metadata().adapter);
    drop(probe);
    // Diagnostics only: `R28_ONLY=<workload>` narrows a local run.
    let only = std::env::var("R28_ONLY").ok();
    let mut failures = Vec::new();
    for (key, Workload(document, _media), floored) in workloads(only.as_deref()) {
        let mut means = Vec::new();
        for index in 0..3 {
            let result = run(&acquire(), &document, Duration::ZERO);
            println!(
                "R28 adapter={adapter} workload={key} run={index} dims={:?} mean_ms={:.2} fps={:.1} p95_ms={:.2} ledger_peak_mib={:.1} validate_us={:.1}",
                result.dims,
                result.mean_ms,
                1e3 / result.mean_ms,
                result.p95_ms,
                result.ledger_peak as f64 / MIB as f64,
                validate_us(&document)
            );
            let verdicts = [
                if floored {
                    throughput(&result, floor)
                } else {
                    Ok(())
                },
                within_ceiling(result.ledger_peak, &document),
            ];
            failures.extend(
                verdicts
                    .into_iter()
                    .filter_map(Result::err)
                    .map(|e| format!("{key} run {index}: {e}")),
            );
            means.push(result.mean_ms);
        }
        let mean = means.iter().sum::<f64>() / 3.0;
        let pinned = BASELINES
            .iter()
            .filter(|(a, k, _)| adapter.contains(a) && *k == key);
        for (_, _, baseline) in pinned {
            let delta = (mean / baseline - 1.0) * 100.0;
            println!(
                "R28 adapter={adapter} workload={key} baseline_ms={baseline:.2} delta={delta:+.1}%"
            );
            if delta > 5.0 {
                failures.push(format!(
                    "{key}: {mean:.2} ms is {delta:+.1}% over pre-MO2 {baseline:.2} ms"
                ));
            }
        }
    }
    // The slowdown control: a per-frame delay of 2.5× the frame-time floor.
    let Workload(document, _media) = blend_heavy(330);
    let delay = Duration::from_secs_f64(2.5 / floor);
    let slowed = run(&acquire(), &document, delay);
    let verdict = throughput(&slowed, floor);
    println!(
        "R28 adapter={adapter} control=slowdown delay_ms={:.0} verdict={verdict:?}",
        delay.as_secs_f64() * 1e3
    );
    assert!(
        verdict.is_err(),
        "the slowdown control must fail throughput"
    );
    assert!(failures.is_empty(), "R28 gate 10 failed: {failures:#?}");
}

#[test]
#[ignore = "R28 perf lane: cargo test --release -p kinewright-media --lib blend_heavy_holds_floors -- --ignored"]
fn blend_heavy_holds_floors_on_the_fallback_adapter() {
    gate(|| GpuContext::headless(true).expect("a lavapipe/WARP adapter"));
}

#[test]
#[ignore = "R28 hardware lane (the RTX 3090)"]
fn blend_heavy_holds_floors_on_hardware() {
    gate(|| GpuContext::headless(false).expect("a hardware adapter"));
}

/// Full-resolution frames of each workload stay inside their ceilings, and
/// every charge is released: the ledger returns to exactly zero.
#[test]
fn r28_ledger_holds_the_ceilings_and_releases_every_charge() {
    let gpu = fixture_gpu_or_skip().expect("the ledger gate needs an adapter");
    let workloads = [
        cuts((1920, 1080), 32, 3, 24, 15),
        blend_heavy(32),
        cuts((3840, 2160), 32, 4, 16, 12),
    ];
    for Workload(document, _media) in workloads {
        let context = GpuContext::new(gpu.device.clone(), gpu.queue.clone());
        let mut renderer = FrameRenderer::new(context.clone());
        let resolution = document.resolution;
        for at in [0, 1, 31] {
            let (scale, strategy) = (RenderScale::FullResolution, DecodeStrategy::Seek);
            renderer
                .render(&document, TimeCode(at), resolution, scale, strategy)
                .expect("renders");
        }
        let peak = context.ledger().peak_bytes();
        let layers = document.tracks.len() as u64 + 1;
        let frame = u64::from(resolution.0) * u64::from(resolution.1) * 8;
        println!(
            "R28 ledger {resolution:?} peak_mib={:.1}",
            peak as f64 / MIB as f64
        );
        // Sources, output and readback at least: the ledger does count.
        assert!(
            peak >= layers * frame,
            "{peak} B undercounts {layers} rasters"
        );
        within_ceiling(peak, &document).unwrap();
        drop(renderer);
        assert_eq!(
            context.ledger().live_bytes(),
            0,
            "a charge outlived its resource"
        );
    }
}

/// The ledger control: four 4096×4096 RGBA16F textures (512 MiB) reject the
/// 1080p budget.
#[test]
fn r28_ledger_control_rejects_the_1080p_budget() {
    let gpu = fixture_gpu_or_skip().expect("the ledger control needs an adapter");
    let context = GpuContext::new(gpu.device.clone(), gpu.queue.clone());
    let reservations: Vec<_> = (0..4)
        .map(|_| {
            context.charge_texture(context.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("R28 ledger control"),
                size: wgpu::Extent3d {
                    width: 4096,
                    height: 4096,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }))
        })
        .collect();
    let peak = context.ledger().peak_bytes();
    assert_eq!(peak, 512 * MIB);
    let at_1080p = Document {
        resolution: (1920, 1080),
        ..Document::default()
    };
    assert!(
        within_ceiling(peak, &at_1080p).is_err(),
        "512 MiB must reject the 1080p budget"
    );
    drop(reservations);
    assert_eq!(context.ledger().live_bytes(), 0);
}
