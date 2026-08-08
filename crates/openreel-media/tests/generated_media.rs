use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_core::{
    Clip, ClipId, Document, MediaAsset, MediaEngine, MediaEvent, MediaKind, PlaybackState,
    Rational, TimeCode, Track, TrackId, TrackKind,
};
use openreel_media::FfmpegMediaEngine;

struct TestClip(PathBuf);

impl TestClip {
    fn generate() -> Self {
        let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/ffmpeg/bin/ffmpeg.exe");
        assert!(ffmpeg.is_file(), "provisioned ffmpeg.exe is missing");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "openreel-m1-{}-{nonce}.mp4",
            std::process::id()
        ));
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
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(30)..TimeCode(40),
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                },
            ],
        }],
        media_pool: vec![asset.clone()],
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

    assert!(first_clip.rgba.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]));
    assert!(second_clip.rgba.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]));
    assert_ne!(first_clip.rgba, second_clip.rgba);
    for gap in [gap_start, gap_end] {
        assert!(gap
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]));
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
    assert_eq!(engine.position(), paused, "audio clock advanced while paused");
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
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(20)..TimeCode(40),
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                },
            ],
        }],
        media_pool: vec![asset.clone()],
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
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..asset.duration,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
            }],
        }],
        media_pool: vec![asset.clone()],
        fps: asset.fps,
        resolution: asset.resolution.unwrap(),
        duration: asset.duration,
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

fn wait_for_state(
    events: &crossbeam_channel::Receiver<MediaEvent>,
    expected: PlaybackState,
) {
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

fn wait_for_position(engine: &FfmpegMediaEngine, minimum: TimeCode) {
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
