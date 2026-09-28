//! PF1 G18: the export lane. Export wall time of `typical_1080p`'s first
//! 4 s (3 tracks, 24 clips × 15, 120 frames at 1920 × 1080) through the
//! production `export_document`, on LH (`PF1_HARDWARE=1`) or LL. S0 exports
//! this workload at ~1.7 s a frame (the whole 60 s is ~85 min a run), so the
//! full programme is opt-in (`PF1_EXPORT_CLIPS=360`). Media generation is
//! excluded; every result line starts with `PF1 export`. The file is
//! self-contained so the same lane applies unchanged to the S0 commit.
//! Each run records its file's SHA-256 (review B S1), and every run of one
//! build must write the same bytes, so the S0 and candidate hashes compare
//! the exports themselves, not their lengths.

use std::time::Instant;

use kinewright_core::{ColorContext, ExportCancellation, ExportSettings};

use crate::{
    compositor::GpuContext,
    perf_fixtures::{self, Workload},
    test_support::TempDirectory,
};

#[test]
#[ignore = "PF1 G18: cargo test --release -p kinewright-media --lib pf1_export_lane -- --ignored --nocapture --test-threads=1"]
fn pf1_export_lane() {
    let release = !cfg!(debug_assertions);
    assert!(release, "G18 is release evidence");
    crate::initialize_ffmpeg().expect("FFmpeg initializes");
    let hardware = std::env::var_os("PF1_HARDWARE").is_some();
    let lane = if hardware { "LH" } else { "LL" };
    let runs: usize = std::env::var("PF1_RUNS").map_or(3, |runs| runs.parse().expect("a count"));
    let clips: u64 =
        std::env::var("PF1_EXPORT_CLIPS").map_or(24, |clips| clips.parse().expect("a count"));
    let Workload(document, _media) = perf_fixtures::cuts((1920, 1080), 600, 3, clips, 15);
    let frames = document.duration.0;
    let settings = ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 8_000_000,
        audio_bitrate: 192_000,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    };
    let directory = TempDirectory::new("pf1-export-lane");
    let (mut walls, mut hashes) = (Vec::new(), Vec::new());
    for run in 0..runs {
        let gpu = GpuContext::headless(!hardware).expect("an adapter");
        let out = directory.path(&format!("export-{run}.mp4"));
        let (progress, _received) = crossbeam_channel::unbounded();
        let started = Instant::now();
        crate::export::export_document(&document, &out, &settings, &progress, gpu)
            .expect("the lane exports");
        let wall = started.elapsed().as_secs_f64() * 1e3;
        let bytes = std::fs::metadata(&out).map_or(0, |meta| meta.len());
        let sha256 = crate::sha256::sha256_file(&out).expect("the export hashes");
        println!(
            "PF1 export lane={lane} workload=typical_1080p frames={frames} run={run} wall_ms={wall:.1} bytes={bytes} sha256={sha256}"
        );
        walls.push(wall);
        hashes.push(sha256);
    }
    hashes.dedup();
    assert_eq!(
        hashes.len(),
        1,
        "every run writes the same bytes: {hashes:?}"
    );
    walls.sort_by(f64::total_cmp);
    println!(
        "PF1 export lane={lane} workload=typical_1080p frames={frames} runs={runs} median_ms={:.1} min_ms={:.1} max_ms={:.1}",
        walls[walls.len() / 2],
        walls[0],
        walls[walls.len() - 1]
    );
    println!(
        "PF1 export lane={lane} workload=typical_1080p frames={frames} sha256={}",
        hashes[0]
    );
}
