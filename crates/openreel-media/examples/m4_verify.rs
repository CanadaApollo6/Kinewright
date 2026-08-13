use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use openreel_core::{
    Analysis, Clip, ClipId, Document, Effect, EffectId, Export, ExportCancellation, ExportSettings,
    MediaEvent, ParamValue, Playback, Rational, TimeCode, Track, TrackId, TrackKind, Transition,
};
use openreel_media::FfmpegMediaEngine;

// This manual verifier deliberately keeps its complete preview/export scenario together.
#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/m4-manual"), PathBuf::from);
    fs::create_dir_all(&output_dir)?;
    let ffmpeg =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ffmpeg/bin/ffmpeg.exe");
    let red_path = output_dir.join("red-source.mp4");
    let blue_path = output_dir.join("blue-source.mp4");
    generate_source(&ffmpeg, &red_path, "red", 440)?;
    generate_source(&ffmpeg, &blue_path, "blue", 660)?;

    let engine = FfmpegMediaEngine::new()?;
    let red = engine.probe(&red_path)?;
    let blue = engine.probe(&blue_path)?;
    let mut document = Document {
        catalog: openreel_core::MediaCatalog::default(),
        audio_mix: openreel_core::AudioMix::default(),
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: red.id,
                    source_range: TimeCode(0)..TimeCode(60),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: blue.id,
                    source_range: TimeCode(0)..TimeCode(60),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            },
        ],
        media_pool: vec![red, blue],
        markers: Vec::new(),
        fps: Rational::new(30, 1)?,
        resolution: (128, 72),
        duration: TimeCode(60),
    };
    openreel_core::Operation::AddEffect {
        clip: ClipId(2),
        effect: Effect {
            id: EffectId(1),
            name: "opacity".to_owned(),
            parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(65))]),
            keyframes: BTreeMap::new(),
        },
    }
    .apply(&mut document)?;
    openreel_core::Operation::AddTransition {
        clip: ClipId(2),
        transition: Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(15),
        },
    }
    .apply(&mut document)?;
    fs::write(
        output_dir.join("two-track.openreel"),
        serde_json::to_string_pretty(&document)?,
    )?;

    let frames = engine.frames();
    let events = engine.events();
    engine.set_document(Arc::new(document.clone()));
    for at in [TimeCode(0), TimeCode(7), TimeCode(30)] {
        engine.request_frame(at);
        let frame = receive_frame(&frames, at)?;
        let center = usize::try_from(frame.width * (frame.height / 2) + frame.width / 2)? * 4;
        println!(
            "preview frame {} center={:?}",
            at.0,
            &frame.rgba[center..center + 4]
        );
    }

    engine.play(TimeCode::ZERO);
    let playback = wait_for_playback(&engine, &events, TimeCode(8));
    engine.pause();
    match playback {
        Ok(()) => println!(
            "preview playback advanced through frame {}",
            engine.position().0
        ),
        Err(error) => println!("preview playback unavailable: {error}"),
    }

    let export_path = output_dir.join("two-track-export.mp4");
    let (progress, updates) = crossbeam_channel::unbounded();
    engine.export(
        &export_path,
        ExportSettings {
            fps: document.fps,
            resolution: document.resolution,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 2_000_000,
            audio_bitrate: 192_000,
            cancellation: ExportCancellation::default(),
        },
        progress,
    )?;
    let final_progress = updates.try_iter().last().ok_or("no export progress")?;
    println!(
        "exported {} frames to {}",
        final_progress.completed_frames,
        export_path.display()
    );
    Ok(())
}

fn generate_source(
    ffmpeg: &Path,
    output: &Path,
    color: &str,
    frequency: u32,
) -> Result<(), Box<dyn Error>> {
    let video = format!("color=c={color}:size=128x72:rate=30:duration=2");
    let audio = format!("sine=frequency={frequency}:sample_rate=48000:duration=2");
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &video,
            "-f",
            "lavfi",
            "-i",
            &audio,
            "-frames:v",
            "60",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(output)
        .status()?;
    if !status.success() {
        return Err(format!("source generation failed for {}", output.display()).into());
    }
    Ok(())
}

fn receive_frame(
    frames: &crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    requested: TimeCode,
) -> Result<openreel_core::FrameTexture, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (at, frame) = frames.recv_timeout(remaining)?;
        if at == requested {
            return Ok(frame);
        }
    }
}

fn wait_for_playback(
    engine: &dyn Playback,
    events: &crossbeam_channel::Receiver<MediaEvent>,
    minimum: TimeCode,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Ok(event) = events.try_recv() {
            if let MediaEvent::Error(error) = event {
                return Err(error.to_string());
            }
        }
        if engine.position() >= minimum {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!(
        "audio clock stopped at frame {} before frame {}",
        engine.position().0,
        minimum.0
    ))
}
