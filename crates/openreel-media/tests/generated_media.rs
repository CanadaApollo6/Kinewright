use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_core::{
    Analysis, Clip, ClipId, Document, Effect, EffectId, Export, ExportCancellation, ExportSettings,
    MediaAsset, MediaError, MediaEvent, MediaKind, ParamValue, Playback, PlaybackState, Rational,
    TimeCode, Track, TrackId, TrackKind, Transition,
};
use openreel_media::FfmpegMediaEngine;

#[path = "../src/test_support.rs"]
pub mod test_support;
use test_support::ffmpeg_executable;

struct TestClip(PathBuf);

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl TestClip {
    fn generate() -> Self {
        let ffmpeg = ffmpeg_executable();
        assert!(ffmpeg.is_file(), "provisioned ffmpeg.exe is missing");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output =
            std::env::temp_dir().join(format!("openreel-m1-{}-{nonce}.mp4", std::process::id()));
        let result = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30000/1001",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-frames:v",
                "60",
                "-t",
                "2.002",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "60",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&output)
            .status()
            .expect("failed to run provisioned ffmpeg.exe");
        assert!(result.success(), "test media generation failed");
        Self(output)
    }
}

impl Drop for TestClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn generate_solid(name: &str, color: &str, frequency: &str) -> TemporaryFile {
    let ffmpeg = ffmpeg_executable();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = std::env::temp_dir().join(format!(
        "openreel-{name}-{}-{nonce}.mp4",
        std::process::id()
    ));
    let video_source = format!("color=c={color}:size=64x64:rate=10:duration=1");
    let audio_source = format!("sine=frequency={frequency}:sample_rate=48000:duration=1");
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &video_source,
            "-f",
            "lavfi",
            "-i",
            &audio_source,
            "-frames:v",
            "10",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&output)
        .status()
        .expect("failed to generate solid-color media");
    assert!(status.success());
    TemporaryFile(output)
}

fn export_fixture(engine: &dyn Analysis) -> Document {
    let red = generate_solid("red", "red", "440");
    let blue = generate_solid("blue", "blue", "660");
    let mut red_asset = engine.probe(&red.0).unwrap();
    let mut blue_asset = engine.probe(&blue.0).unwrap();
    // Keep the generated files alive for the duration of the test by taking
    // ownership of their paths. The caller removes them with the document assets.
    red_asset.path.clone_from(&red.0);
    blue_asset.path.clone_from(&blue.0);
    std::mem::forget(red);
    std::mem::forget(blue);
    let document = Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: red_asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                }],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: blue_asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    timeline_start: TimeCode::ZERO,
                    effects: vec![Effect {
                        id: EffectId(1),
                        name: "opacity".to_owned(),
                        parameters: BTreeMap::from([(
                            "percent".to_owned(),
                            ParamValue::Integer(50),
                        )]),
                    }],
                    transition_in: Some(Transition {
                        name: "crossfade".to_owned(),
                        duration: TimeCode(5),
                    }),
                    link: None,
                }],
            },
        ],
        media_pool: vec![red_asset, blue_asset],
        markers: Vec::new(),
        fps: Rational::new(10, 1).unwrap(),
        resolution: (64, 64),
        duration: TimeCode(10),
    };
    document.validate().unwrap();
    document
}

fn remove_fixture_assets(document: &Document) {
    for asset in &document.media_pool {
        let _ = std::fs::remove_file(&asset.path);
    }
}

#[test]
fn two_track_effect_export_matches_preview_after_h264_redecode() {
    let engine = FfmpegMediaEngine::new().unwrap();
    let document = export_fixture(&engine);
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document.clone()));
    engine.request_frame(TimeCode(0));
    let preview_start = receive_frame(&frames, TimeCode(0));
    engine.request_frame(TimeCode(4));
    let preview_blended = receive_frame(&frames, TimeCode(4));
    assert_frame_center_close(&preview_start, [255, 0, 0], 5);
    assert_frame_center_close(&preview_blended, [128, 0, 128], 8);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = TemporaryFile(std::env::temp_dir().join(format!(
        "openreel-export-{}-{nonce}.mp4",
        std::process::id()
    )));
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    engine
        .export(
            &output.0,
            ExportSettings {
                fps: document.fps,
                resolution: document.resolution,
                video_codec: "libx264".to_owned(),
                audio_codec: "aac".to_owned(),
                video_bitrate: 500_000,
                audio_bitrate: 128_000,
                cancellation: ExportCancellation::default(),
            },
            progress_tx,
        )
        .unwrap();
    let updates = progress_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(updates.first().unwrap().completed_frames, 0);
    assert_eq!(updates.last().unwrap().completed_frames, 10);
    assert_eq!(updates.last().unwrap().total_frames, 10);

    let decode_engine = FfmpegMediaEngine::new().unwrap();
    let exported_asset = decode_engine.probe(&output.0).unwrap();
    assert_eq!(exported_asset.kind, MediaKind::AudioVideo);
    assert_eq!(exported_asset.resolution, Some((64, 64)));
    assert_eq!(exported_asset.duration, TimeCode(10));
    let mixed_audio = decode_stereo_audio(&output.0);
    assert!(
        tone_amplitude(&mixed_audio, 48_000, 440.0) > 0.02,
        "the lower-track 440 Hz tone is missing from the export mix"
    );
    assert!(
        tone_amplitude(&mixed_audio, 48_000, 660.0) > 0.02,
        "the upper-track 660 Hz tone is missing from the export mix"
    );
    let exported_document = full_timeline(exported_asset);
    let exported_frames = decode_engine.frames();
    decode_engine.set_document(std::sync::Arc::new(exported_document));
    decode_engine.request_frame(TimeCode(0));
    let decoded_start = receive_frame(&exported_frames, TimeCode(0));
    decode_engine.request_frame(TimeCode(4));
    let decoded_blended = receive_frame(&exported_frames, TimeCode(4));

    assert_frame_sample_close(&preview_start, &decoded_start, 28);
    assert_frame_sample_close(&preview_blended, &decoded_blended, 28);
    remove_fixture_assets(&document);
}

fn decode_stereo_audio(path: &Path) -> Vec<f32> {
    let ffmpeg = ffmpeg_executable();
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ac",
            "2",
            "-ar",
            "48000",
            "pipe:1",
        ])
        .output()
        .expect("failed to decode exported audio");
    assert!(
        output.status.success(),
        "ffmpeg audio decode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .chunks_exact(8)
        .map(|stereo| {
            let left = f32::from_le_bytes(stereo[0..4].try_into().unwrap());
            let right = f32::from_le_bytes(stereo[4..8].try_into().unwrap());
            (left + right) * 0.5
        })
        .collect()
}

// The generated sample buffer is small; f64 indices preserve the intended spectral estimate.
#[allow(clippy::cast_precision_loss)]
fn tone_amplitude(samples: &[f32], sample_rate: u32, frequency: f64) -> f64 {
    let angular_step = std::f64::consts::TAU * frequency / f64::from(sample_rate);
    let (real, imaginary) =
        samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(real, imaginary), (index, sample)| {
                let phase = angular_step * index as f64;
                (
                    real + f64::from(*sample) * phase.cos(),
                    imaginary - f64::from(*sample) * phase.sin(),
                )
            });
    2.0 * real.hypot(imaginary) / samples.len().max(1) as f64
}

#[test]
fn cancelled_export_writes_no_output() {
    let engine = FfmpegMediaEngine::new().unwrap();
    let document = export_fixture(&engine);
    engine.set_document(std::sync::Arc::new(document.clone()));
    let output = TemporaryFile(std::env::temp_dir().join(format!(
        "openreel-cancelled-export-{}.mp4",
        std::process::id()
    )));
    let cancellation = ExportCancellation::default();
    cancellation.cancel();
    let (progress, _) = crossbeam_channel::unbounded();
    let result = engine.export(
        &output.0,
        ExportSettings {
            fps: document.fps,
            resolution: document.resolution,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 500_000,
            audio_bitrate: 128_000,
            cancellation,
        },
        progress,
    );
    assert_eq!(result, Err(MediaError::Cancelled));
    assert!(!output.0.exists());
    remove_fixture_assets(&document);
}

fn assert_frame_sample_close(
    expected: &openreel_core::FrameTexture,
    actual: &openreel_core::FrameTexture,
    tolerance: u8,
) {
    assert_eq!(
        (expected.width, expected.height),
        (actual.width, actual.height)
    );
    let center =
        usize::try_from(expected.width * (expected.height / 2) + expected.width / 2).unwrap() * 4;
    for channel in 0..3 {
        let difference = expected.rgba[center + channel].abs_diff(actual.rgba[center + channel]);
        assert!(
            difference <= tolerance,
            "preview {:?} differs from export {:?}",
            &expected.rgba[center..center + 4],
            &actual.rgba[center..center + 4]
        );
    }
}

fn assert_frame_center_close(
    frame: &openreel_core::FrameTexture,
    expected: [u8; 3],
    tolerance: u8,
) {
    let center = usize::try_from(frame.width * (frame.height / 2) + frame.width / 2).unwrap() * 4;
    for (actual, expected) in frame.rgba[center..center + 3].iter().zip(expected) {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "frame center {:?} differs from expected {expected:?}",
            &frame.rgba[center..center + 4]
        );
    }
}

#[test]
fn probe_reports_generated_asset_metadata_exactly() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();

    let asset = engine.probe(&clip.0).unwrap();

    assert_eq!(asset.kind, MediaKind::AudioVideo);
    assert_eq!(asset.fps, Rational::new(30_000, 1_001).unwrap());
    assert_eq!(asset.resolution, Some((320, 180)));
    assert_eq!(asset.duration, TimeCode(60));
}

#[test]
fn frame_requests_decode_exact_requested_frames_without_an_audio_device() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = full_timeline(asset);
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document));

    engine.request_frame(TimeCode(0));
    let first = receive_frame(&frames, TimeCode(0));
    engine.request_frame(TimeCode(30));
    let second = receive_frame(&frames, TimeCode(30));

    assert_eq!((first.width, first.height), (320, 180));
    assert_eq!(first.rgba.len(), 320 * 180 * 4);
    assert_ne!(first.rgba, second.rgba);
}

#[test]
fn timeline_decode_selects_two_clips_and_renders_the_gap_black() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(5)..TimeCode(15),
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(30)..TimeCode(40),
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
            ],
        }],
        media_pool: vec![asset.clone()],
        markers: Vec::new(),
        fps: asset.fps,
        resolution: asset.resolution.unwrap(),
        duration: TimeCode(25),
    };
    document.validate().unwrap();
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document));

    engine.request_frame(TimeCode(0));
    let first_clip = receive_frame(&frames, TimeCode(0));
    engine.request_frame(TimeCode(10));
    let gap_start = receive_frame(&frames, TimeCode(10));
    engine.request_frame(TimeCode(14));
    let gap_end = receive_frame(&frames, TimeCode(14));
    engine.request_frame(TimeCode(15));
    let second_clip = receive_frame(&frames, TimeCode(15));

    assert!(
        first_clip
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[..3] != [0, 0, 0])
    );
    assert!(
        second_clip
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[..3] != [0, 0, 0])
    );
    assert_ne!(first_clip.rgba, second_clip.rgba);
    for gap in [gap_start, gap_end] {
        assert!(
            gap.rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
    }
}

#[test]
fn audio_device_play_pause_and_seek_smoke_test() {
    if std::env::var_os("OPENREEL_AUDIO_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("skipped: set OPENREEL_AUDIO_TEST=1 on a machine with an audio device");
        return;
    }

    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = full_timeline(asset);
    let events = engine.events();
    engine.set_document(std::sync::Arc::new(document));

    engine.play(TimeCode::ZERO);
    wait_for_state(&events, PlaybackState::Playing);
    wait_for_position(&engine, TimeCode(5));

    engine.seek(TimeCode(30));
    wait_for_position(&engine, TimeCode(35));

    engine.pause();
    wait_for_state(&events, PlaybackState::Paused);
    let paused = engine.position();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        engine.position(),
        paused,
        "audio clock advanced while paused"
    );
}

#[test]
fn multi_track_audio_device_play_pause_and_seek_smoke_test() {
    if std::env::var_os("OPENREEL_AUDIO_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("skipped: set OPENREEL_AUDIO_TEST=1 on a machine with an audio device");
        return;
    }

    let voice = generate_solid("device-voice", "navy", "440");
    let bed = generate_solid("device-bed", "black", "660");
    let engine = FfmpegMediaEngine::new().unwrap();
    let voice_asset = engine.probe(&voice.0).unwrap();
    let bed_asset = engine.probe(&bed.0).unwrap();
    let duration = voice_asset.duration.min(bed_asset.duration);
    assert!(
        duration >= TimeCode(8),
        "device fixtures are unexpectedly short"
    );
    let document = Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: voice_asset.id,
                    source_range: TimeCode::ZERO..duration,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                }],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: bed_asset.id,
                    source_range: TimeCode(2)..duration,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                }],
            },
        ],
        media_pool: vec![voice_asset, bed_asset],
        markers: Vec::new(),
        fps: Rational::new(10, 1).unwrap(),
        resolution: (64, 64),
        duration,
    };
    document.validate().unwrap();
    let events = engine.events();
    engine.set_document(std::sync::Arc::new(document));

    engine.play(TimeCode::ZERO);
    wait_for_state(&events, PlaybackState::Playing);
    wait_for_position(&engine, TimeCode(4));

    engine.seek(TimeCode(2));
    wait_for_position(&engine, TimeCode(6));

    engine.pause();
    wait_for_state(&events, PlaybackState::Paused);
    let paused = engine.position();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        engine.position(),
        paused,
        "audio clock advanced while paused"
    );
}

#[test]
fn timeline_audio_crosses_a_clip_boundary_and_gap_smoke_test() {
    if std::env::var_os("OPENREEL_AUDIO_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("skipped: set OPENREEL_AUDIO_TEST=1 on a machine with an audio device");
        return;
    }

    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(20)..TimeCode(40),
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
            ],
        }],
        media_pool: vec![asset.clone()],
        markers: Vec::new(),
        fps: asset.fps,
        resolution: asset.resolution.unwrap(),
        duration: TimeCode(35),
    };
    document.validate().unwrap();
    let events = engine.events();
    engine.set_document(std::sync::Arc::new(document));

    engine.play(TimeCode(8));
    wait_for_state(&events, PlaybackState::Playing);
    wait_for_position(&engine, TimeCode(20));
    while let Ok(event) = events.try_recv() {
        if let MediaEvent::Error(error) = event {
            panic!("timeline boundary playback failed: {error}");
        }
    }
    engine.pause();
    wait_for_state(&events, PlaybackState::Paused);
}

fn full_timeline(asset: MediaAsset) -> Document {
    let asset_id = asset.id;
    let asset_duration = asset.duration;
    let fps = asset.fps;
    let resolution = asset.resolution.unwrap();
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset_id,
                source_range: TimeCode::ZERO..asset_duration,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps,
        resolution,
        duration: asset_duration,
    };
    document.validate().unwrap();
    document
}

fn receive_frame(
    frames: &crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    requested: TimeCode,
) -> openreel_core::FrameTexture {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (at, frame) = frames
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("no frame {requested} arrived: {error}"));
        if at == requested {
            return frame;
        }
    }
}

fn wait_for_state(events: &crossbeam_channel::Receiver<MediaEvent>, expected: PlaybackState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(MediaEvent::PlaybackStateChanged(state)) if state == expected => return,
            Ok(MediaEvent::Error(error)) => panic!("playback failed: {error}"),
            Ok(_) => {}
            Err(error) => panic!("playback did not reach {expected:?}: {error}"),
        }
    }
}

fn wait_for_position(engine: &dyn Playback, minimum: TimeCode) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if engine.position() >= minimum {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "audio callback clock stopped at {}, expected at least {}",
        engine.position(),
        minimum
    );
}
