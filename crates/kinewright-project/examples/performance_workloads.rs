//! Repeatable release-mode performance workloads for Kinewright.
//!
//! Three lanes, one binary:
//!
//! * `typical` — typical 1080p editing: generated 1920x1080 footage, dozens of
//!   clips, sequential frame-seek request-to-receipt timings through the real
//!   [`FfmpegMediaEngine`].
//! * `heavy` — heavy 4K editing: generated 3840x2160 footage, hundreds of
//!   clips, same seek protocol with fewer iterations to stay bounded.
//! * `agent` — repeated validated Core `DoBatch` edit plans plus undo/redo,
//!   measured as `Core::request` round trips. This is **local edit-plan
//!   execution**: it excludes model inference, network transport, and
//!   transcription generation. A cached-transcript mapping probe uses only
//!   synthetically registered transcript rows (no Whisper run).
//!
//! All footage is synthetic and license-free: deterministic `ffmpeg` `lavfi`
//! generator arguments, no downloads. Machine-readable JSON is written to
//! `--out` (default `<workdir>/report.json`) and echoed to stdout.
//!
//! Run (release only):
//!
//! ```bash
//! source scripts/setup-ffmpeg.sh
//! cargo run --release -p kinewright-project --example performance_workloads -- \
//!   --lane all --workdir /tmp/kinewright-perf/workloads
//! ```

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use kinewright_core::{
    Analysis, AssetTranscript, Clip, ClipContent, ClipId, Core, Document, Event,
    MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset, Operation, Playback, Rational,
    TimeCode, Track, TrackId, TrackKind, TranscriptWord,
};
use kinewright_media::FfmpegMediaEngine;
use kinewright_project::{load_document, min_required_format_version, write_project_document};

/// Per-frame receive timeout. A seek that never produces its frame is invalid
/// evidence, so the run fails instead of recording a timeout sample.
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Stats over one sample set, milliseconds.
#[derive(Debug, Clone, Copy)]
struct Stats {
    count: usize,
    min_ms: f64,
    max_ms: f64,
    mean_ms: f64,
    median_ms: f64,
    p95_ms: f64,
}

fn stats_of(samples: &[Duration]) -> Option<Stats> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let sum: f64 = sorted.iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let count_f = count as f64;
    let quantile = |percent: usize| sorted[(percent * count).div_ceil(100) - 1];
    Some(Stats {
        count,
        min_ms: sorted[0],
        max_ms: sorted[count - 1],
        mean_ms: sum / count_f,
        median_ms: quantile(50),
        p95_ms: quantile(95),
    })
}

fn stats_json(stats: Stats) -> serde_json::Value {
    serde_json::json!({
        "count": stats.count,
        "min_ms": stats.min_ms,
        "max_ms": stats.max_ms,
        "mean_ms": stats.mean_ms,
        "median_ms": stats.median_ms,
        "p95_ms": stats.p95_ms,
    })
}

/// Process RSS / peak RSS in KiB. Linux only; `None` elsewhere by design.
#[cfg(target_os = "linux")]
fn memory_kb() -> (Option<u64>, Option<u64>) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let mut rss = None;
    let mut peak = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            rss = value.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            peak = value.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }
    (rss, peak)
}

#[cfg(not(target_os = "linux"))]
fn memory_kb() -> (Option<u64>, Option<u64>) {
    (None, None)
}

fn memory_json(
    before: (Option<u64>, Option<u64>),
    after: (Option<u64>, Option<u64>),
) -> serde_json::Value {
    serde_json::json!({
        "rss_before_kb": before.0,
        "rss_after_kb": after.0,
        "peak_after_kb": after.1,
        "source": if cfg!(target_os = "linux") { "proc_self_status" } else { "unsupported" },
    })
}

fn ffmpeg_executable() -> PathBuf {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::var_os("FFMPEG_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../third_party/ffmpeg/bin")
                .join(name)
        },
        |directory| PathBuf::from(directory).join("bin").join(name),
    )
}

#[derive(Debug, Clone)]
struct FootageSpec {
    frames: i64,
    video_source: String,
    frequency: u32,
}

impl FootageSpec {
    fn args(&self) -> Vec<String> {
        vec![
            "-f".to_owned(),
            "lavfi".to_owned(),
            "-i".to_owned(),
            self.video_source.clone(),
            "-f".to_owned(),
            "lavfi".to_owned(),
            "-i".to_owned(),
            format!("sine=frequency={}:sample_rate=48000", self.frequency),
            "-frames:v".to_owned(),
            self.frames.to_string(),
            "-c:v".to_owned(),
            "libx264".to_owned(),
            "-preset".to_owned(),
            "veryfast".to_owned(),
            "-pix_fmt".to_owned(),
            "yuv420p".to_owned(),
            "-color_primaries".to_owned(),
            "bt709".to_owned(),
            "-color_trc".to_owned(),
            "bt709".to_owned(),
            "-colorspace".to_owned(),
            "bt709".to_owned(),
            "-color_range".to_owned(),
            "tv".to_owned(),
            "-x264-params".to_owned(),
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv".to_owned(),
            "-g".to_owned(),
            "60".to_owned(),
            "-c:a".to_owned(),
            "aac".to_owned(),
            "-shortest".to_owned(),
        ]
    }
}

/// Generate one synthetic source file. Deterministic arguments; the only
/// per-run input is the output path.
/// Write a fixture through the shared R7 envelope (MO2 R7, review 2 S6), never
/// a raw `Document` serialization, and prove the stamp: the file reopens as
/// the same document at exactly the minimum-required format version, which
/// is returned.
fn write_fixture(document: &Document, path: &Path) -> Result<u32, Box<dyn Error>> {
    write_project_document(document, path, None)?;
    let (reopened, version, _) = load_document(path)?;
    let required = min_required_format_version(document);
    if reopened != *document || version != required {
        return Err(format!(
            "{} reopened as v{version}, expected the same document at v{required}",
            path.display()
        )
        .into());
    }
    Ok(version)
}

fn generate_source(spec: &FootageSpec, output: &Path) -> Result<Duration, Box<dyn Error>> {
    let ffmpeg = ffmpeg_executable();
    if !ffmpeg.is_file() {
        return Err(format!(
            "provisioned ffmpeg CLI is missing at {} (source scripts/setup-ffmpeg.sh)",
            ffmpeg.display()
        )
        .into());
    }
    let started = Instant::now();
    let arguments = spec.args();
    let result = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(&arguments)
        .arg(output)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "media generation failed for {}: {}",
            output.display(),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    if !output.is_file() || fs::metadata(output)?.len() == 0 {
        return Err(format!("ffmpeg produced no output at {}", output.display()).into());
    }
    Ok(started.elapsed())
}

/// Build a validated multi-track document over `assets`.
///
/// Clips are laid end to end per track (no overlaps) with `clip_len`-frame
/// source windows cycling through the asset. Reused windows across clips model
/// repeated B-roll selects.
fn build_document(
    assets: &[MediaAsset],
    fps: Rational,
    resolution: (u32, u32),
    track_count: usize,
    clip_total: usize,
    clip_len: i64,
) -> Result<Document, Box<dyn Error>> {
    if assets.is_empty() || track_count == 0 || clip_total == 0 || clip_len <= 0 {
        return Err(
            "build_document needs assets, tracks, clips, and a positive clip length".into(),
        );
    }
    let mut tracks = Vec::with_capacity(track_count);
    let mut cursors = vec![0_i64; track_count];
    let mut per_track: Vec<Vec<Clip>> = (0..track_count).map(|_| Vec::new()).collect();
    for index in 0..clip_total {
        let clip_id = u64::try_from(index)? + 1;
        let track = index % track_count;
        let asset = &assets[index % assets.len()];
        let span = asset.duration.0 - clip_len;
        if span <= 0 {
            return Err(format!(
                "asset {} duration {} is too short for a {clip_len}-frame window",
                asset.id, asset.duration.0
            )
            .into());
        }
        #[allow(clippy::cast_possible_wrap)]
        let source_start = (index as i64 * 7) % span;
        let start = cursors[track];
        per_track[track].push(Clip {
            enabled: true,
            enabled_curve: None,
            id: ClipId(clip_id),
            asset: asset.id,
            source_range: TimeCode(source_start)..TimeCode(source_start + clip_len),
            content: ClipContent::Media,
            timeline_start: TimeCode(start),
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
            blend_mode: kinewright_core::BlendMode::Normal,
        });
        cursors[track] = start + clip_len;
    }
    for (track_index, clips) in per_track.into_iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let id = track_index as u64 + 1;
        tracks.push(Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        });
    }
    let duration = TimeCode(cursors.iter().copied().max().unwrap_or(0));
    let document = Document {
        investigator: None,
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks,
        media_pool: assets.to_vec(),
        markers: Vec::new(),
        fps,
        resolution,
        duration,
    };
    document.validate()?;
    Ok(document)
}

/// Evenly spaced seek positions across `0..duration_frames`, sequential order.
fn seek_positions(duration_frames: i64, count: usize) -> Vec<TimeCode> {
    if count == 0 || duration_frames <= 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![TimeCode(0)];
    }
    (0..count)
        .map(|i| {
            #[allow(clippy::cast_possible_wrap)]
            let at = (duration_frames - 1) * i as i64 / (count as i64 - 1);
            TimeCode(at)
        })
        .collect()
}

/// Request one frame and wait for its exact receipt, validating the evidence.
fn timed_seek(
    engine: &FfmpegMediaEngine,
    frames: &crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)>,
    at: TimeCode,
) -> Result<(Duration, (u32, u32)), Box<dyn Error>> {
    let started = Instant::now();
    engine.request_frame(at);
    let deadline = Instant::now() + FRAME_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("frame {} was not received within {FRAME_TIMEOUT:?}", at.0).into());
        }
        let (received_at, frame) = frames.recv_timeout(remaining)?;
        if received_at != at {
            continue;
        }
        if frame.width == 0 || frame.height == 0 || frame.rgba.is_empty() {
            return Err(format!("frame {} arrived with empty pixel evidence", at.0).into());
        }
        let expected = usize::try_from(frame.width)? * usize::try_from(frame.height)? * 4;
        if frame.rgba.len() != expected {
            return Err(format!(
                "frame {} has {} bytes for a {}x{} RGBA raster (expected {expected})",
                at.0,
                frame.rgba.len(),
                frame.width,
                frame.height
            )
            .into());
        }
        return Ok((started.elapsed(), (frame.width, frame.height)));
    }
}

#[derive(Debug, Clone)]
struct PlaybackLaneSpec {
    key: &'static str,
    label: &'static str,
    width: u32,
    height: u32,
    frames: i64,
    track_count: usize,
    clip_total: usize,
    clip_len: i64,
    warmup_seeks: usize,
    measured_seeks: usize,
}

#[allow(clippy::too_many_lines)]
fn run_playback_lane(
    engine: &FfmpegMediaEngine,
    workdir: &Path,
    spec: &PlaybackLaneSpec,
    seek_override: Option<usize>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let measured = seek_override.unwrap_or(spec.measured_seeks);
    let lane_dir = workdir.join(spec.key);
    fs::create_dir_all(&lane_dir)?;
    let mem_before = memory_kb();

    let footage = [
        FootageSpec {
            frames: spec.frames,
            video_source: format!("testsrc2=size={}x{}:rate=30", spec.width, spec.height),
            frequency: 440,
        },
        FootageSpec {
            frames: spec.frames,
            video_source: format!("smptebars=size={}x{}:rate=30", spec.width, spec.height),
            frequency: 660,
        },
    ];
    let mut generate_ms = 0.0;
    let mut paths = Vec::new();
    let mut source_bytes = Vec::new();
    for (index, source) in footage.iter().enumerate() {
        let path = lane_dir.join(format!("source-{index}.mp4"));
        generate_ms += generate_source(source, &path)?.as_secs_f64() * 1000.0;
        source_bytes.push(fs::metadata(&path)?.len());
        paths.push(path);
    }

    let probe_started = Instant::now();
    let mut assets = Vec::new();
    for path in &paths {
        assets.push(engine.probe(path)?);
    }
    let probe_ms = probe_started.elapsed().as_secs_f64() * 1000.0;
    for asset in &assets {
        if asset.duration.0 < spec.clip_len {
            return Err(format!(
                "probed asset {} duration {} is shorter than the clip window",
                asset.id, asset.duration.0
            )
            .into());
        }
    }

    let fps = Rational::new(30, 1)?;
    let document = build_document(
        &assets,
        fps,
        (spec.width, spec.height),
        spec.track_count,
        spec.clip_total,
        spec.clip_len,
    )?;
    write_fixture(&document, &lane_dir.join("project.kinewright"))?;
    let duration_frames = document.duration.0;
    engine.set_document(Arc::new(document));

    let frames = engine.frames();
    for at in seek_positions(duration_frames, spec.warmup_seeks) {
        timed_seek(engine, &frames, at)?;
    }
    let mut samples = Vec::with_capacity(measured);
    let mut observed: Option<(u32, u32)> = None;
    for at in seek_positions(duration_frames, measured) {
        let (elapsed, dims) = timed_seek(engine, &frames, at)?;
        observed.get_or_insert(dims);
        if observed != Some(dims) {
            return Err(
                format!("frame dimensions changed mid-lane: {observed:?} vs {dims:?}").into(),
            );
        }
        samples.push(elapsed);
    }
    let Some(lane_stats) = stats_of(&samples) else {
        return Err("playback lane recorded no seek samples".into());
    };
    let (observed_w, observed_h) = observed.ok_or("playback lane observed no frames")?;

    let mem_after = memory_kb();
    let inventory = engine.cache_inventory();
    let cache = serde_json::to_value(&inventory)?;

    Ok(serde_json::json!({
        "key": spec.key,
        "label": spec.label,
        "footage": {
            "width": spec.width,
            "height": spec.height,
            "fps": "30/1",
            "frames_per_source": spec.frames,
            "sources": paths.len(),
            "source_bytes": source_bytes,
            "generator": "ffmpeg lavfi (testsrc2, smptebars, sine); deterministic args; no downloads",
            "generate_ms": generate_ms,
            "probe_ms": probe_ms,
        },
        "document": {
            "tracks": spec.track_count,
            "clips": spec.clip_total,
            "clip_len_frames": spec.clip_len,
            "duration_frames": duration_frames,
        },
        "seeks": {
            "warmup": spec.warmup_seeks,
            "measured": measured,
            "timeout_s": FRAME_TIMEOUT.as_secs(),
            "protocol": "sequential request_frame -> matching (TimeCode, FrameTexture) receipt; warmup excluded",
            "observed_frame": {"width": observed_w, "height": observed_h},
            "request_to_receipt_ms": stats_json(lane_stats),
        },
        "memory_kb": memory_json(mem_before, mem_after),
        "cache": cache,
    }))
}

/// One validated agent-style edit plan: two clip gain edits plus two marker
/// inserts. Layout-neutral, so every plan validates on any clip count.
#[allow(clippy::cast_possible_truncation)]
fn edit_plan(clips: &[ClipId], plan: usize, marker_base: u64) -> Vec<Operation> {
    let plan_frame = i64::try_from(plan).expect("plan count is bounded to 1000") * 5;
    let first = clips[plan % clips.len()];
    let second = clips[(plan * 3 + 1) % clips.len()];
    let gain = if plan.is_multiple_of(2) { -60 } else { 30 };
    vec![
        Operation::SetClipAudio {
            clip: first,
            gain_tenth_db: gain,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
        },
        Operation::SetClipAudio {
            clip: second,
            gain_tenth_db: -gain,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
        },
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(marker_base + plan as u64 * 2),
                position: TimeCode(plan_frame),
                label: format!("perf-plan-{plan}-a"),
                color_token: (plan % usize::from(MARKER_COLOR_TOKEN_COUNT)) as u8,
            },
        },
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(marker_base + plan as u64 * 2 + 1),
                position: TimeCode(plan_frame + 2),
                label: format!("perf-plan-{plan}-b"),
                color_token: ((plan + 1) % usize::from(MARKER_COLOR_TOKEN_COUNT)) as u8,
            },
        },
    ]
}

fn expect_document_changed(event: Event, what: &str) -> Result<Arc<Document>, Box<dyn Error>> {
    match event {
        Event::DocumentChanged { doc, .. } => Ok(doc),
        Event::BatchRejected { error, .. } => {
            Err(format!("{what} was rejected by validation: {error}").into())
        }
        Event::OpRejected { error, .. } => {
            Err(format!("{what} was rejected by validation: {error}").into())
        }
        Event::RevisionConflict {
            expected, actual, ..
        } => Err(
            format!("{what} hit a revision conflict: expected {expected}, actual {actual}").into(),
        ),
        Event::QueryResult(_) => Err(format!("{what} returned a query result").into()),
    }
}

#[allow(clippy::too_many_lines)]
fn run_agent_lane(
    engine: &FfmpegMediaEngine,
    workdir: &Path,
    plans: usize,
) -> Result<serde_json::Value, Box<dyn Error>> {
    const TRANSCRIPT_CALLS: usize = 10;
    let lane_dir = workdir.join("agent");
    fs::create_dir_all(&lane_dir)?;
    let mem_before = memory_kb();

    // One tiny source gives the edit document valid probed asset identity.
    // Generation and probing are setup, excluded from edit timings.
    let setup_started = Instant::now();
    let source_path = lane_dir.join("agent-source.mp4");
    generate_source(
        &FootageSpec {
            frames: 150,
            video_source: "testsrc2=size=320x180:rate=30".to_owned(),
            frequency: 440,
        },
        &source_path,
    )?;
    let asset = engine.probe(&source_path)?;
    let fps = Rational::new(30, 1)?;
    let assets = vec![asset.clone()];
    let document = build_document(&assets, fps, (320, 180), 4, 1_200, 15)?;
    let setup_ms = setup_started.elapsed().as_secs_f64() * 1000.0;
    let clip_ids: Vec<ClipId> = document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().map(|clip| clip.id))
        .collect();

    write_fixture(&document, &lane_dir.join("project.kinewright"))?;
    let core = Core::spawn(document)?;
    let mut do_batch = Vec::with_capacity(plans);
    let mut undo = Vec::with_capacity(plans);
    let mut redo = Vec::with_capacity(plans);
    for plan in 0..plans {
        let mut operations = edit_plan(&clip_ids, plan, 1_000_000);
        operations.extend((0..16).map(|offset| Operation::SetClipAudio {
            clip: clip_ids[(plan * 17 + offset) % clip_ids.len()],
            gain_tenth_db: -30,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
        }));
        let started = Instant::now();
        let event = core.request(kinewright_core::Command::DoBatch(operations))?;
        do_batch.push(started.elapsed());
        expect_document_changed(event, "DoBatch")?;

        let started = Instant::now();
        let event = core.request(kinewright_core::Command::Undo)?;
        undo.push(started.elapsed());
        expect_document_changed(event, "Undo")?;

        let started = Instant::now();
        let event = core.request(kinewright_core::Command::Redo)?;
        redo.push(started.elapsed());
        let final_doc = expect_document_changed(event, "Redo")?;
        final_doc.validate()?;
    }
    let (Some(do_stats), Some(undo_stats), Some(redo_stats)) =
        (stats_of(&do_batch), stats_of(&undo), stats_of(&redo))
    else {
        return Err("agent lane recorded no edit samples".into());
    };

    // Cached-transcript mapping probe: synthetic rows registered through the
    // public ingestion seam, then mapped through the timeline. No model load,
    // no network, no transcription run.
    let content_sha256 = kinewright_media::sha256_file(&source_path)?;
    let mut words = Vec::new();
    let mut start = 0_i64;
    let mut index = 0;
    while start + 4 <= asset.duration.0 {
        words.push(TranscriptWord {
            text: format!("word{index}"),
            source_start: TimeCode(start),
            source_end: TimeCode(start + 4),
            speaker: None,
        });
        start += 5;
        index += 1;
    }
    engine.register_transcript(
        &asset,
        AssetTranscript {
            asset: asset.id,
            content_sha256,
            source_fps: asset.fps,
            words,
        },
    )?;
    let event = core.request(kinewright_core::Command::Query(
        kinewright_core::Query::Document,
    ))?;
    let Event::QueryResult(kinewright_core::QueryResult::Document(live)) = event else {
        return Err("document query did not return a document".into());
    };
    let mut map_samples = Vec::with_capacity(TRANSCRIPT_CALLS);
    let mut mapped_words = 0;
    for _ in 0..TRANSCRIPT_CALLS {
        let started = Instant::now();
        let mapped = engine.timeline_transcript(&live, None)?;
        map_samples.push(started.elapsed());
        mapped_words = mapped.len();
    }
    if mapped_words == 0 {
        return Err("cached transcript mapping returned no timeline words".into());
    }
    let Some(map_stats) = stats_of(&map_samples) else {
        return Err("agent lane recorded no transcript-map samples".into());
    };

    let mem_after = memory_kb();
    Ok(serde_json::json!({
        "key": "agent",
        "label": "local edit-plan execution; excludes model inference, network transport, and transcription generation",
        "document": {"tracks": 4, "clips": 1_200, "setup_ms": setup_ms},
        "plans": {
            "count": plans,
            "ops_per_plan": 20,
            "protocol": "Core::request round trip per DoBatch/Undo/Redo; every plan validated, undone, and redone",
            "do_batch_ms": stats_json(do_stats),
            "undo_ms": stats_json(undo_stats),
            "redo_ms": stats_json(redo_stats),
        },
        "cached_transcript_map": {
            "calls": TRANSCRIPT_CALLS,
            "mapped_words": mapped_words,
            "protocol": "register_transcript once (synthetic rows), then timeline_transcript; no Whisper run",
            "map_ms": stats_json(map_stats),
        },
        "memory_kb": memory_json(mem_before, mem_after),
    }))
}

#[derive(Debug)]
struct Config {
    lanes: Vec<String>,
    workdir: PathBuf,
    out: Option<PathBuf>,
    seeks: Option<usize>,
    plans: usize,
}

fn parse_args() -> Result<Config, Box<dyn Error>> {
    let mut lanes = vec!["all".to_owned()];
    let mut workdir = std::env::temp_dir().join("kinewright-perf/workloads");
    let mut out = None;
    let mut seeks = None;
    let mut plans = 25_usize;
    let mut args = std::env::args_os().skip(1).peekable();
    while let Some(arg) = args.next() {
        let arg = arg.into_string().map_err(|_| "arguments must be UTF-8")?;
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "usage: performance_workloads [--lane all|typical|heavy|agent]... \
                     [--workdir DIR] [--out FILE] [--seeks N] [--plans N]"
                );
                std::process::exit(0);
            }
            "--lane" => {
                let value = args
                    .next()
                    .ok_or("--lane needs a value")?
                    .into_string()
                    .map_err(|_| "--lane value must be UTF-8")?;
                if lanes == ["all"] {
                    lanes.clear();
                }
                lanes.push(value);
            }
            "--workdir" => {
                workdir = args.next().ok_or("--workdir needs a value")?.into();
            }
            "--out" => {
                out = Some(args.next().ok_or("--out needs a value")?.into());
            }
            "--seeks" => {
                let value = args
                    .next()
                    .ok_or("--seeks needs a value")?
                    .into_string()
                    .map_err(|_| "--seeks value must be UTF-8")?;
                seeks = Some(value.parse().map_err(|_| "--seeks must be a number")?);
            }
            "--plans" => {
                let value = args
                    .next()
                    .ok_or("--plans needs a value")?
                    .into_string()
                    .map_err(|_| "--plans value must be UTF-8")?;
                plans = value.parse().map_err(|_| "--plans must be a number")?;
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    if lanes.iter().any(|lane| lane == "all") {
        lanes = vec!["typical".to_owned(), "heavy".to_owned(), "agent".to_owned()];
    }
    for lane in &lanes {
        if lane != "typical" && lane != "heavy" && lane != "agent" {
            return Err(format!("unknown lane: {lane} (typical|heavy|agent|all)").into());
        }
    }
    if seeks == Some(0) || seeks.is_some_and(|n| n > 1_000) || plans == 0 || plans > 1_000 {
        return Err("--seeks and --plans must be in 1..=1000".into());
    }
    Ok(Config {
        lanes,
        workdir,
        out,
        seeks,
        plans,
    })
}

fn typical_spec() -> PlaybackLaneSpec {
    PlaybackLaneSpec {
        key: "typical_1080p",
        label: "typical 1080p editing: 2 generated sources, dozens of clips",
        width: 1920,
        height: 1080,
        frames: 600,
        track_count: 3,
        clip_total: 24,
        clip_len: 15,
        warmup_seeks: 3,
        measured_seeks: 20,
    }
}

fn heavy_spec() -> PlaybackLaneSpec {
    PlaybackLaneSpec {
        key: "heavy_4k",
        label: "heavy 4K editing: 2 generated sources, hundreds of clips",
        width: 3840,
        height: 2160,
        frames: 300,
        track_count: 4,
        clip_total: 200,
        clip_len: 12,
        warmup_seeks: 2,
        measured_seeks: 10,
    }
}

fn run() -> Result<serde_json::Value, Box<dyn Error>> {
    let config = parse_args()?;
    fs::create_dir_all(&config.workdir)?;
    let engine = FfmpegMediaEngine::new_with_data_dir(config.workdir.join("engine-cache"))?;
    let mut lanes = serde_json::Map::new();
    for lane in &config.lanes {
        let value = match lane.as_str() {
            "typical" => {
                run_playback_lane(&engine, &config.workdir, &typical_spec(), config.seeks)?
            }
            "heavy" => run_playback_lane(&engine, &config.workdir, &heavy_spec(), config.seeks)?,
            "agent" => run_agent_lane(&engine, &config.workdir, config.plans)?,
            _ => return Err(format!("unknown lane: {lane}").into()),
        };
        lanes.insert(lane.clone(), value);
    }
    Ok(serde_json::json!({
        "tool": "performance_workloads",
        "version": 1,
        "mode": if cfg!(debug_assertions) { "debug" } else { "release" },
        "metric_boundaries": {
            "playback": "sequential request_frame dispatch -> matching frame receipt; includes decode, render, and channel wait; warmup excluded; footage generation and probing reported as setup, not in seek stats",
            "agent": "Core::request round trip per DoBatch/Undo/Redo including validation, revision advance, and document clone; excludes model inference, network, and transcription generation",
            "memory": "Linux /proc VmRSS and VmHWM snapshots per lane; null on unsupported platforms",
            "cache": "post-lane Analysis::cache_inventory snapshot (public API)",
            "evidence": "any empty frame, dimension change, rejected edit, or unmapped transcript fails the run with a nonzero exit",
        },
        "lanes": lanes,
        "errors": Vec::<String>::new(),
    }))
}

fn main() -> Result<(), Box<dyn Error>> {
    // Resolve `--out` early so even a failed run can leave a partial report.
    let out = parse_args().map_or_else(
        |_| std::env::temp_dir().join("kinewright-perf/workloads/report.json"),
        |config| {
            config
                .out
                .unwrap_or_else(|| config.workdir.join("report.json"))
        },
    );
    match run() {
        Ok(report) => {
            let text = serde_json::to_string_pretty(&report)?;
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out, &text)?;
            println!("{text}");
            println!("report: {}", out.display());
            Ok(())
        }
        Err(error) => {
            let partial = serde_json::json!({
                "tool": "performance_workloads",
                "version": 1,
                "mode": if cfg!(debug_assertions) { "debug" } else { "release" },
                "lanes": {},
                "errors": [error.to_string()],
            });
            if let Some(parent) = out.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&out, serde_json::to_string_pretty(&partial)?);
            Err(error)
        }
    }
}
