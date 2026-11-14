//! PCM witnesses for Core's audible clock. No physical audio device is opened.
use super::*;
use crate::test_support::{TempDirectory, wav_f32};
use kinewright_core::{
    AssetId, ColorContext, ColorDescription, ExactMicros, ExportSettings, MediaAsset, MediaKind,
    MediaSourceFingerprint, Track, TrackKind, audible_span,
};

fn sample(time: ExactMicros) -> usize {
    usize::try_from(time.numerator() * 48_000 / (u128::from(time.denominator()) * 1_000_000))
        .unwrap()
}
struct Case {
    source_rate: Rational,
    project_rate: Rational,
    source: std::ops::Range<TimeCode>,
    length: TimeCode,
    timeline_start: TimeCode,
    enabled_from: Option<TimeCode>,
    seek: TimeCode,
}
fn rate(n: u32, d: u32) -> Rational {
    Rational::new(n, d).unwrap()
}
#[allow(
    clippy::too_many_lines,
    reason = "one generated recording is compared through both production paths"
)]
fn check(case: Case) {
    crate::initialize_ffmpeg().unwrap();
    let root = TempDirectory::new("dq-media-clock");
    let source = root.path("source.wav");
    // Sparse, signed PCM markers are located exactly after AAC decode by their
    // peaks; the export mix before encoding also proves every padding sample.
    let mut pcm = vec![0.0_f32; 6 * 48_000 * 2];
    for (i, at) in (211..6 * 48_000).step_by(3079).enumerate() {
        let amplitude = if i % 2 == 0 { 0.5 } else { -0.5 };
        pcm[2 * at] = amplitude;
        pcm[2 * at + 1] = amplitude;
    }
    pcm[2 * 1100] = 0.5;
    pcm[2 * 1100 + 1] = 0.5; // outside the 60/30 padding case's stored source
    std::fs::write(&source, wav_f32(&pcm, 48_000, 2)).unwrap();
    let mut document = Document {
        fps: case.project_rate,
        resolution: (64, 64),
        duration: TimeCode(case.timeline_start.0 + case.length.0),
        ..Document::default()
    };
    document.media_pool.push(MediaAsset {
        id: AssetId(1),
        path: source,
        name: "clock markers".into(),
        duration: TimeCode(
            i64::try_from(
                (6 * u64::from(case.source_rate.numerator()))
                    .div_ceil(u64::from(case.source_rate.denominator())),
            )
            .unwrap(),
        ),
        fps: case.source_rate,
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
        assumed_from: None,
    });
    let mut clip: Clip = serde_json::from_value(serde_json::json!({"id":1,"asset":1,"source_range":{"start":case.source.start,"end":case.source.end},"timeline_start":case.timeline_start,"effects":[],"transition_in":null})).unwrap();
    let (audible_source, project_start) = if let Some(from) = case.enabled_from {
        clip.enabled_curve = Some(AutomationCurve {
            keyframes: [(TimeCode::ZERO, 0), (from, 1)]
                .into_iter()
                .map(|(at, value)| kinewright_core::Keyframe {
                    at,
                    value,
                    interpolation: kinewright_core::KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        });
        // Independent hand-computed enabled-run witness: floor(23*25/24)+12=35.
        assert_eq!(from, TimeCode(23));
        (TimeCode(35)..case.source.end, from)
    } else {
        (case.source, TimeCode::ZERO)
    };
    document.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![clip],
    });
    document.validate().unwrap();
    let span = audible_span(
        audible_source,
        case.source_rate,
        TimeCode(case.length.0 - project_start.0),
        case.project_rate,
    )
    .unwrap();
    let offset = sample(
        ExactMicros::frames(
            TimeCode(case.timeline_start.0 + project_start.0),
            case.project_rate,
        )
        .unwrap(),
    );
    let total = sample(ExactMicros::frames(document.duration, case.project_rate).unwrap());
    let source_start = sample(span.start);
    let source_end = sample(span.end);
    let mut expected = vec![0.0_f32; total * 2];
    let frames = (source_end - source_start).min(total - offset);
    expected[offset * 2..(offset + frames) * 2]
        .copy_from_slice(&pcm[source_start * 2..(source_start + frames) * 2]);
    let settings = ExportSettings {
        fps: case.project_rate,
        resolution: (64, 64),
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".into(),
        audio_codec: "aac".into(),
        video_bitrate: 300_000,
        audio_bitrate: 192_000,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    };
    let master_pcm = crate::export::mix_audio(&document, &settings).unwrap();
    assert_pcm(&master_pcm, &expected, "export pre-encode");
    let gpu = crate::compositor::GpuContext::headless(true).expect("software Vulkan/WARP adapter");
    let output = root.path("export.mp4");
    let (progress, _events) = crossbeam_channel::unbounded();
    crate::export::export_document(&document, &output, &settings, &progress, gpu).unwrap();
    let mut reader = AudioDecoder::open(&output, 48_000, 2, 0, u64::MAX).unwrap();
    let mut product_pcm = Vec::new();
    while let Some(chunk) = reader.next_chunk().unwrap() {
        product_pcm.extend(chunk);
    }
    let cli = std::process::Command::new(crate::test_support::ffmpeg_executable())
        .args(["-nostdin", "-v", "error", "-threads", "4", "-i"])
        .arg(&output)
        .args(["-map", "0:a:0", "-f", "f32le", "-c:a", "pcm_f32le", "-"])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert_eq!(cli.stdout.len() % 4, 0);
    let linear: Vec<f32> = cli
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    // AAC may ring around an impulse, but neither decoder may relocate its peak.
    for (lane, rendered_pcm) in [
        ("product export decode", &product_pcm),
        ("linear export decode", &linear),
    ] {
        for (at, frame) in expected
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame[0].abs() > 0.1)
        {
            let first = at.saturating_sub(4);
            let last = (at + 5).min(rendered_pcm.len() / 2);
            let peak = (first..last)
                .max_by(|a, b| {
                    rendered_pcm[*a * 2]
                        .abs()
                        .total_cmp(&rendered_pcm[*b * 2].abs())
                })
                .unwrap();
            assert!(
                rendered_pcm[peak * 2].abs() > 0.1,
                "{lane} lost marker at {at}, expected {}",
                frame[0]
            );
            assert!(
                peak.abs_diff(at) <= 1,
                "{lane} moved source marker from {at} to {peak}"
            );
        }
    }
    let seek = sample(ExactMicros::frames(case.seek, case.project_rate).unwrap());
    let mut mixer = AudioMixer::open(&document, case.seek, 48_000, 2, None).unwrap();
    let mut live = Vec::new();
    while let Some(chunk) = mixer.next_chunk().unwrap() {
        live.extend(chunk);
    }
    assert_pcm(&live, &expected[seek * 2..], "live/seek");
}
fn assert_pcm(actual: &[f32], expected: &[f32], lane: &str) {
    assert_eq!(
        actual.len() % 2,
        0,
        "{lane} returned an incomplete stereo frame"
    );
    assert!(
        actual.len().abs_diff(expected.len()) <= 2,
        "{lane} output length: {} vs {}",
        actual.len(),
        expected.len()
    );
    let events = |pcm: &[f32]| {
        pcm.as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.iter().any(|v| v.abs() > 0.00001))
            .map(|(at, frame)| (at, [frame[0], frame[1]]))
            .collect::<Vec<_>>()
    };
    let heard = events(actual);
    let wanted = events(expected);
    assert_eq!(
        heard.len(),
        wanted.len(),
        "{lane} lost/duplicated a marker or read past stored source into padding"
    );
    for ((actual_at, actual_frame), (expected_at, expected_frame)) in heard.iter().zip(&wanted) {
        assert!(
            actual_at.abs_diff(*expected_at) <= 1,
            "{lane} moved marker from {expected_at} to {actual_at}"
        );
        for (actual_sample, expected_sample) in actual_frame.iter().zip(expected_frame) {
            assert!(
                (actual_sample - expected_sample).abs() < 0.00001,
                "{lane} changed marker amplitude"
            );
        }
    }
}
#[test]
fn dq_media_clock_equal_rates() {
    check(Case {
        source_rate: rate(30, 1),
        project_rate: rate(30, 1),
        source: TimeCode(0)..TimeCode(60),
        length: TimeCode(60),
        timeline_start: TimeCode(7),
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_25_in_24() {
    check(Case {
        source_rate: rate(25, 1),
        project_rate: rate(24, 1),
        source: TimeCode(0)..TimeCode(50),
        length: TimeCode(48),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_ntsc_in_30() {
    check(Case {
        source_rate: rate(30_000, 1_001),
        project_rate: rate(30, 1),
        source: TimeCode(1)..TimeCode(61),
        length: TimeCode(60),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_60_in_30() {
    check(Case {
        source_rate: rate(60, 1),
        project_rate: rate(30, 1),
        source: TimeCode(0)..TimeCode(120),
        length: TimeCode(60),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_nonzero_source() {
    check(Case {
        source_rate: rate(25, 1),
        project_rate: rate(24, 1),
        source: TimeCode(12)..TimeCode(62),
        length: TimeCode(48),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_stored_source_padding() {
    check(Case {
        source_rate: rate(60, 1),
        project_rate: rate(30, 1),
        source: TimeCode(0)..TimeCode(1),
        length: TimeCode(1),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_enabled_run_inside_clip() {
    check(Case {
        source_rate: rate(25, 1),
        project_rate: rate(24, 1),
        source: TimeCode(12)..TimeCode(112),
        length: TimeCode(96),
        timeline_start: TimeCode::ZERO,
        enabled_from: Some(TimeCode(23)),
        seek: TimeCode::ZERO,
    });
}
#[test]
fn dq_media_clock_seek_inside_clip() {
    check(Case {
        source_rate: rate(25, 1),
        project_rate: rate(24, 1),
        source: TimeCode(12)..TimeCode(112),
        length: TimeCode(96),
        timeline_start: TimeCode::ZERO,
        enabled_from: None,
        seek: TimeCode(23),
    });
}

#[test]
fn dq_media_clock_refuses_invalid_playback_windows() {
    let document = Document {
        duration: TimeCode(30),
        ..Document::default()
    };
    for from in [TimeCode(-1), document.duration] {
        assert!(
            AudioMixer::open(&document, from, 48_000, 2, None).is_err(),
            "invalid cue {from:?} must be refused"
        );
    }
}
