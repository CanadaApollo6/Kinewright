//! MO2 §13 gate 10 (`blend_heavy_holds_floors`) and R28's ledger.
//!
//! Lanes: the floors are release-mode evidence on three backends — the
//! `--ignored` fallback test is lavapipe on Linux and WARP on a local
//! Windows VM, run by hand (ME14), the `--ignored` hardware test is the
//! RTX 3090. The ledger ceilings
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
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
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

/// Pre-MO2 end-to-end `Normal` baselines (adapter, workload, mean ms over
/// three runs), measured at f241aa5 on the RTX 3090 workstation with the
/// same protocol and `FFmpeg` build. R28's 5% rule (ME13).
const BASELINES: &[(&str, &str, f64)] = &[
    ("llvmpipe", "typical_1080p", 497.86),
    ("RTX 3090", "typical_1080p", 494.39),
];

fn workloads(only: Option<&str>) -> Vec<(&'static str, Workload, bool)> {
    let all: [(_, fn() -> Workload, _); 3] = [
        ("typical_1080p", || cuts((1920, 1080), 600, 3, 24, 15), true),
        ("blend_heavy_1080p", || blend_heavy(330), true),
        ("heavy_4k", || cuts((3840, 2160), 300, 4, 200, 12), false),
    ];
    let wanted = |key: &str| only.is_none_or(|only| only.split(',').any(|k| k == key));
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

/// Three runs of each selected workload (`R28_ONLY=a,b` narrows a lane):
/// compositor frames (`resident`) hold the floors, end-to-end frames the 5%
/// rule; every run holds its ledger ceiling. Returns the adapter's floor and
/// every failure.
fn lane(acquire: fn() -> GpuContext, resident: bool) -> (f64, Vec<String>) {
    let release = !cfg!(debug_assertions);
    assert!(
        release,
        "R28 floors are release evidence: cargo test --release"
    );
    let probe = acquire();
    let (floor, adapter) = (floor_fps(&probe), probe.monitor_proof_metadata().adapter);
    drop(probe);
    // End to end defaults to the pinned typical lane; others on request (ME13).
    let end_to_end = (!resident).then(|| "typical_1080p".to_owned());
    let only = std::env::var("R28_ONLY").ok().or(end_to_end);
    let mut failures = Vec::new();
    for (key, Workload(document, _media), floored) in workloads(only.as_deref()) {
        let mut means = Vec::new();
        for index in 0..3 {
            let gpu = acquire();
            let result = run(&gpu, &document, Duration::ZERO, resident);
            let ledger_peak = gpu.ledger().peak_bytes();
            println!(
                "R28 adapter={adapter} resident={resident} workload={key} run={index} dims={:?} mean_ms={:.2} fps={:.1} p95_ms={:.2} ledger_peak_mib={:.1} validate_us={:.1}",
                result.dims,
                result.mean_ms,
                1e3 / result.mean_ms,
                result.p95_ms,
                ledger_peak as f64 / MIB as f64,
                validate_us(&document)
            );
            let verdicts = [
                if floored && resident {
                    throughput(&result, floor)
                } else {
                    Ok(())
                },
                within_ceiling(ledger_peak, &document),
            ];
            failures.extend(
                verdicts
                    .into_iter()
                    .filter_map(Result::err)
                    .map(|e| format!("{key} run {index}: {e}")),
            );
            means.push(result.mean_ms);
        }
        if resident {
            print_phases(acquire(), &document, key);
        }
        let mean = means.iter().sum::<f64>() / 3.0;
        let pinned = BASELINES
            .iter()
            .filter(|(a, k, _)| !resident && adapter.contains(a) && *k == key);
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
    (floor, failures)
}

/// ME14: a resident frame's mean phases over 30 frames after 10 warm-up —
/// printed, never gated — so a backend pathology shows in the log.
fn print_phases(gpu: GpuContext, document: &Document, key: &str) {
    let scale = RenderScale::Proxy {
        max_width: PREVIEW_MAX_WIDTH,
    };
    let dims = scale.output_resolution(document.resolution);
    let mut renderer = FrameRenderer::new(gpu);
    renderer.set_cache_budget(1 << 30);
    let mut means = [0.0; 3];
    for frame in 0..40 {
        let at = TimeCode(frame % document.duration.0);
        let phases = crate::render::phases::render(&mut renderer, document, at, dims, scale);
        let (phases, _) = phases.expect("an R28 frame renders");
        if frame >= 10 {
            for (mean, phase) in means.iter_mut().zip(phases) {
                *mean += phase.as_secs_f64() * 1e3 / 30.0;
            }
        }
    }
    let [upload, gpu, encode] = means;
    println!(
        "R28 phases workload={key} upload_ms={upload:.2} gpu_passes_readback_ms={gpu:.2} monitor_encode_ms={encode:.2}"
    );
}

/// Gate 10 on compositor frames (ME13), then the slowdown control: a
/// per-frame delay of 2.5× the frame-time floor must fail throughput.
fn gate(acquire: fn() -> GpuContext) {
    let (floor, failures) = lane(acquire, true);
    let Workload(document, _media) = blend_heavy(330);
    let delay = Duration::from_secs_f64(2.5 / floor);
    let slowed = run(&acquire(), &document, delay, true);
    let verdict = throughput(&slowed, floor);
    println!(
        "R28 control=slowdown delay_ms={:.1} mean_ms={:.2} p95_ms={:.2} verdict={verdict:?}",
        delay.as_secs_f64() * 1e3,
        slowed.mean_ms,
        slowed.p95_ms
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

/// ME13: red on the RTX 3090 until PF1 — the pre-existing CPU monitor
/// encode and layer upload exceed the 60 fps frame time on their own.
#[test]
#[ignore = "R28 hardware lane (the RTX 3090)"]
fn blend_heavy_holds_floors_on_hardware() {
    gate(|| GpuContext::headless(false).expect("a hardware adapter"));
}

/// ME13: the end-to-end preview (decode included) is a tracked, non-gating
/// baseline; only the 5% no-regression rule against f241aa5 gates it.
#[test]
#[ignore = "R28 end-to-end tracked lane (decode-bound until PF1)"]
fn r28_end_to_end_tracked() {
    let hardware = std::env::var("R28_HARDWARE").is_ok();
    let acquire = if hardware {
        || GpuContext::headless(false).expect("a hardware adapter")
    } else {
        || GpuContext::headless(true).expect("a lavapipe/WARP adapter")
    };
    let (_, failures) = lane(acquire, false);
    assert!(failures.is_empty(), "R28 end-to-end failed: {failures:#?}");
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

/// G11 (rereview-final-mo2-1): the WARP floor is 20 fps with p95 ≤ 3× the
/// mean, pinned on synthetic DX12 CPU metadata — no Windows run claimed.
#[test]
fn reverify_me14_warp_floor_boundary() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let info = wgpu::AdapterInfo {
        name: "Synthetic WARP metadata only".into(),
        vendor: 0,
        device: 0,
        device_type: wgpu::DeviceType::Cpu,
        device_pci_bus_id: String::new(),
        driver: String::new(),
        driver_info: String::new(),
        backend: wgpu::Backend::Dx12,
        subgroup_min_size: 4,
        subgroup_max_size: 128,
        transient_saves_memory: false,
    };
    let warp = GpuContext::new_with_adapter_info(gpu.device.clone(), gpu.queue.clone(), info);
    let floor = floor_fps(&warp);
    assert!((floor - 20.0).abs() < f64::EPSILON, "WARP floor {floor}");
    for (mean_ms, p95_ms, holds) in [
        (50.0, 150.0, true),
        (50.01, 50.01, false),
        (49.0, 147.01, false),
    ] {
        let run = Run {
            dims: (1280, 720),
            mean_ms,
            p95_ms,
        };
        assert_eq!(
            throughput(&run, floor).is_ok(),
            holds,
            "mean {mean_ms} p95 {p95_ms}"
        );
    }
}
