use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use kinewright_core::{
    AssetId, ColorBitDepth, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange,
    ColorTransfer, ColorWhitePoint, Document, ExportCancellation, ExportSettings, MediaAsset,
    MediaKind, Operation, Rational, TimeCode, Track, TrackId, TrackKind,
    map_source_range_to_project,
};

use crate::{
    audio::decode_audio_range,
    cache::FrameCache,
    compositor::GpuContext,
    decode::{VideoDecoder, probe_path},
    export::export_document,
    initialize_ffmpeg,
    render::PREVIEW_MAX_WIDTH,
    test_support::{TempDirectory, ffmpeg_executable, run_ffmpeg},
};

type MatrixDirectory = TempDirectory;

fn ffprobe() -> PathBuf {
    crate::test_support::ffprobe_executable()
}

fn keyframe_count(path: &Path) -> u64 {
    let output = Command::new(ffprobe())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-skip_frame",
            "nokey",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .expect("provisioned ffprobe.exe should run");
    assert!(
        output.status.success(),
        "keyframe probe failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("ffprobe should report an integer keyframe count")
}

#[derive(Clone)]
struct MatrixCase {
    name: String,
    path: PathBuf,
    expected_fps: Rational,
    expected_duration: TimeCode,
    expected_resolution: Option<(u32, u32)>,
    expected_kind: MediaKind,
    has_video: bool,
    has_audio: bool,
}

fn video_case(
    name: &str,
    path: PathBuf,
    fps: Rational,
    resolution: (u32, u32),
    duration: i64,
    has_audio: bool,
) -> MatrixCase {
    MatrixCase {
        name: name.to_owned(),
        path,
        expected_fps: fps,
        expected_duration: TimeCode(duration),
        expected_resolution: Some(resolution),
        expected_kind: if has_audio {
            MediaKind::AudioVideo
        } else {
            MediaKind::Video
        },
        has_video: true,
        has_audio,
    }
}

fn audio_case(name: &str, path: PathBuf) -> MatrixCase {
    MatrixCase {
        name: name.to_owned(),
        path,
        expected_fps: Rational::default(),
        expected_duration: TimeCode(23),
        expected_resolution: None,
        expected_kind: MediaKind::Audio,
        has_video: false,
        has_audio: true,
    }
}

fn generate_cfr(output: &Path, rate: &str, size: &str, duration: &str) {
    let source = format!("testsrc2=size={size}:rate={rate}:duration={duration}");
    let arguments = vec![
        "-f".to_owned(),
        "lavfi".to_owned(),
        "-i".to_owned(),
        source,
        "-c:v".to_owned(),
        "libx264".to_owned(),
        "-preset".to_owned(),
        "ultrafast".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        "-an".to_owned(),
    ];
    run_ffmpeg(&arguments, output);
}

fn generate_video_with_audio(
    output: &Path,
    rate: &str,
    size: &str,
    duration: &str,
    codec: &str,
    gop: &str,
) {
    let video = format!("testsrc2=size={size}:rate={rate}:duration={duration}");
    let audio = format!("sine=frequency=523:sample_rate=44100:duration={duration}");
    let arguments = vec![
        "-f".to_owned(),
        "lavfi".to_owned(),
        "-i".to_owned(),
        video,
        "-f".to_owned(),
        "lavfi".to_owned(),
        "-i".to_owned(),
        audio,
        "-c:v".to_owned(),
        codec.to_owned(),
        "-preset".to_owned(),
        "ultrafast".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        "-g".to_owned(),
        gop.to_owned(),
        "-keyint_min".to_owned(),
        gop.to_owned(),
        "-sc_threshold".to_owned(),
        "0".to_owned(),
        "-c:a".to_owned(),
        "aac".to_owned(),
        "-shortest".to_owned(),
    ];
    run_ffmpeg(&arguments, output);
}

fn generate_audio(output: &Path, rate: u32, channels: u16) {
    let source = format!("sine=frequency=659:sample_rate={rate}:duration=0.75");
    let arguments = vec![
        "-f".to_owned(),
        "lavfi".to_owned(),
        "-i".to_owned(),
        source,
        "-c:a".to_owned(),
        "aac".to_owned(),
        "-ar".to_owned(),
        rate.to_string(),
        "-ac".to_owned(),
        channels.to_string(),
        "-vn".to_owned(),
    ];
    run_ffmpeg(&arguments, output);
}

fn encoder_available(name: &str) -> bool {
    Command::new(ffmpeg_executable())
        .args(["-hide_banner", "-encoders"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .any(|encoder| encoder == name)
        })
}

// Keeping the hostile-media recipe together makes the exact matrix contents reviewable.
#[allow(clippy::too_many_lines)]
fn generate_fast_matrix(directory: &MatrixDirectory) -> (Vec<MatrixCase>, Option<String>) {
    let mut cases = Vec::new();
    let vfr = directory.path("phone-vfr.mp4");
    generate_vfr(&vfr);
    cases.push(video_case(
        "phone-vfr",
        vfr,
        Rational::new(20, 1).unwrap(),
        (160, 90),
        30,
        true,
    ));

    for (name, rate, fps, duration) in [
        ("cfr-23.976", "24000/1001", (24_000, 1_001), 12),
        ("cfr-25", "25", (25, 1), 13),
        ("cfr-29.97", "30000/1001", (30_000, 1_001), 15),
        ("cfr-59.94", "60000/1001", (60_000, 1_001), 30),
        ("cfr-60", "60", (60, 1), 30),
    ] {
        let path = directory.path(&format!("{name}.mp4"));
        generate_cfr(&path, rate, "160x90", "0.5");
        cases.push(video_case(
            name,
            path,
            Rational::new(fps.0, fps.1).unwrap(),
            (160, 90),
            duration,
            false,
        ));
    }

    let long_gop = directory.path("long-gop-small.mp4");
    generate_video_with_audio(&long_gop, "30", "160x90", "2.5", "libx264", "300");
    assert_eq!(keyframe_count(&long_gop), 1);
    cases.push(video_case(
        "long-gop-small",
        long_gop,
        Rational::new(30, 1).unwrap(),
        (160, 90),
        75,
        true,
    ));

    let hevc_encoder = if encoder_available("libx265") {
        "libx265"
    } else {
        "libx264"
    };
    let hevc_note = (hevc_encoder != "libx265")
        .then(|| "libx265 unavailable; HEVC cases use libx264 fallback".to_owned());
    let hevc = directory.path("hevc-small.mp4");
    generate_video_with_audio(&hevc, "30", "160x90", "0.75", hevc_encoder, "60");
    cases.push(video_case(
        if hevc_encoder == "libx265" {
            "hevc-small"
        } else {
            "hevc-fallback-h264-small"
        },
        hevc,
        Rational::new(30, 1).unwrap(),
        (160, 90),
        22,
        true,
    ));

    let portrait = directory.path("portrait.mp4");
    generate_cfr(&portrait, "30", "54x96", "0.75");
    cases.push(video_case(
        "portrait",
        portrait,
        Rational::new(30, 1).unwrap(),
        (54, 96),
        23,
        false,
    ));

    let rotated = directory.path("rotation-metadata.mp4");
    generate_rotated(&rotated, directory);
    cases.push(video_case(
        "rotation-metadata",
        rotated,
        Rational::new(10, 1).unwrap(),
        (54, 96),
        10,
        false,
    ));

    for (name, rate, channels) in [
        ("audio-44.1-stereo", 44_100, 2),
        ("audio-44.1-mono", 44_100, 1),
        ("audio-22.05-mono", 22_050, 1),
    ] {
        let path = directory.path(&format!("{name}.m4a"));
        generate_audio(&path, rate, channels);
        cases.push(audio_case(name, path));
    }

    let video_only = directory.path("video-only.mp4");
    generate_cfr(&video_only, "30", "160x90", "0.75");
    cases.push(video_case(
        "video-only",
        video_only,
        Rational::new(30, 1).unwrap(),
        (160, 90),
        23,
        false,
    ));

    let short = directory.path("very-short.mp4");
    generate_cfr(&short, "30", "160x90", "0.2");
    cases.push(video_case(
        "very-short",
        short,
        Rational::new(30, 1).unwrap(),
        (160, 90),
        6,
        false,
    ));
    (cases, hevc_note)
}

fn add_full_matrix_cases(
    directory: &MatrixDirectory,
    cases: &mut Vec<MatrixCase>,
    hevc_note: &mut Option<String>,
) {
    let four_k = directory.path("4k-h264-g250.mp4");
    generate_video_with_audio(&four_k, "30", "3840x2160", "3", "libx264", "250");
    assert_eq!(keyframe_count(&four_k), 1);
    cases.push(video_case(
        "4k-h264-g250",
        four_k,
        Rational::new(30, 1).unwrap(),
        (3840, 2160),
        90,
        true,
    ));

    let encoder = if encoder_available("libx265") {
        "libx265"
    } else {
        *hevc_note = Some("libx265 unavailable; HEVC cases use libx264 fallback".to_owned());
        "libx264"
    };
    let hevc = directory.path("hevc-1080p.mp4");
    generate_video_with_audio(&hevc, "30", "1920x1080", "2", encoder, "120");
    cases.push(video_case(
        if encoder == "libx265" {
            "hevc-1080p"
        } else {
            "hevc-fallback-h264-1080p"
        },
        hevc,
        Rational::new(30, 1).unwrap(),
        (1920, 1080),
        60,
        true,
    ));
}

fn generate_vfr(output: &Path) {
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=1.5",
            "-vf",
            "setpts=(floor(N/2)*3+mod(N\\,2)*2)/(30*TB)",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "60",
            "-c:a",
            "aac",
            "-t",
            "1.5",
        ],
        output,
    );
}

fn generate_color_video(output: &Path, filter: Option<&str>, pixel_format: &str) {
    let source = "testsrc2=size=64x64:rate=1:duration=1";
    let mut arguments = vec!["-f", "lavfi", "-i", source];
    if let Some(filter) = filter {
        arguments.extend(["-vf", filter]);
    }
    arguments.extend([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        pixel_format,
        "-an",
    ]);
    run_ffmpeg(&arguments, output);
}

#[test]
fn probe_preserves_tagged_and_untagged_source_color_metadata() {
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-color-probe");
    let tagged = directory.path("tagged-rec709.mp4");
    generate_color_video(
        &tagged,
        Some("setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709"),
        "yuv420p",
    );
    let tagged_asset = probe_path(&tagged, AssetId(1)).unwrap();
    assert_eq!(
        tagged_asset.color_description.primaries,
        ColorPrimaries::Bt709
    );
    assert_eq!(
        tagged_asset.color_description.transfer,
        ColorTransfer::Bt709
    );
    assert_eq!(tagged_asset.color_description.matrix, ColorMatrix::Bt709);
    assert_eq!(tagged_asset.color_description.range, ColorRange::Limited);
    assert_eq!(
        tagged_asset.color_description.white_point,
        ColorWhitePoint::Unknown
    );
    assert_eq!(
        tagged_asset.color_description.bit_depth,
        ColorBitDepth::Eight
    );
    assert_eq!(
        tagged_asset.color_description.provenance,
        ColorProvenance::StreamMetadata
    );
    assert_eq!(
        tagged_asset.color_description.confidence_basis_points,
        10_000
    );

    let partial = directory.path("partial-rec709.mp4");
    generate_color_video(&partial, Some("setparams=color_primaries=bt709"), "yuv420p");
    let partial_asset = probe_path(&partial, AssetId(2)).unwrap();
    assert_eq!(
        partial_asset.color_description.primaries,
        ColorPrimaries::Bt709
    );
    assert_eq!(
        partial_asset.color_description.transfer,
        ColorTransfer::Unknown
    );
    assert_eq!(partial_asset.color_description.matrix, ColorMatrix::Unknown);
    // FFmpeg's setparams filter preserves the format's limited-range default
    // alongside the sole explicit primaries tag, so this fixture has two of
    // four stream colorimetry fields plus inferred component depth.
    assert_eq!(partial_asset.color_description.range, ColorRange::Limited);
    assert_eq!(
        partial_asset.color_description.bit_depth,
        ColorBitDepth::Eight
    );
    assert_eq!(
        partial_asset.color_description.provenance,
        ColorProvenance::StreamMetadata
    );
    assert_eq!(
        partial_asset.color_description.confidence_basis_points,
        6_000
    );

    let untagged = directory.path("untagged.mp4");
    generate_color_video(&untagged, None, "yuv420p");
    let untagged_asset = probe_path(&untagged, AssetId(3)).unwrap();
    assert_eq!(
        untagged_asset.color_description.primaries,
        ColorPrimaries::Unknown
    );
    assert_eq!(
        untagged_asset.color_description.transfer,
        ColorTransfer::Unknown
    );
    assert_eq!(
        untagged_asset.color_description.matrix,
        ColorMatrix::Unknown
    );
    assert_eq!(untagged_asset.color_description.range, ColorRange::Unknown);
    assert_eq!(
        untagged_asset.color_description.white_point,
        ColorWhitePoint::Unknown
    );
    assert_eq!(
        untagged_asset.color_description.bit_depth,
        ColorBitDepth::Eight
    );
    assert_eq!(
        untagged_asset.color_description.provenance,
        ColorProvenance::Inferred
    );
    assert_eq!(
        untagged_asset.color_description.confidence_basis_points,
        2_000
    );
}

#[test]
fn probe_preserves_ten_bit_bt2020_source_metadata() {
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-color-depth");
    let path = directory.path("tagged-bt2020-pq-10bit.mp4");
    generate_color_video(
        &path,
        Some(
            "setparams=range=limited:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
        ),
        "yuv420p10le",
    );
    let asset = probe_path(&path, AssetId(1)).unwrap();
    assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt2020);
    assert_eq!(asset.color_description.transfer, ColorTransfer::Smpte2084);
    assert_eq!(asset.color_description.matrix, ColorMatrix::Bt2020Ncl);
    assert_eq!(asset.color_description.range, ColorRange::Limited);
    assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Ten);
    assert_eq!(
        asset.color_description.provenance,
        ColorProvenance::StreamMetadata
    );
}

fn generate_rotated(output: &Path, directory: &MatrixDirectory) {
    let encoded = directory.path("rotation-encoded.mp4");
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=96x54:rate=10:duration=1,drawbox=x=0:y=0:w=48:h=27:color=red:t=fill,drawbox=x=48:y=0:w=48:h=27:color=green:t=fill,drawbox=x=0:y=27:w=48:h=27:color=blue:t=fill,drawbox=x=48:y=27:w=48:h=27:color=yellow:t=fill",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-an",
        ],
        &encoded,
    );
    let encoded = encoded.to_string_lossy();
    run_ffmpeg(
        &["-display_rotation:v:0", "90", "-i", &encoded, "-c", "copy"],
        output,
    );
}

#[test]
fn vfr_grid_holds_the_previous_pts_frame_and_is_deterministic() {
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-focused");
    let path = directory.path("phone-vfr.mp4");
    generate_vfr(&path);
    let asset = probe_path(&path, AssetId(1)).unwrap();

    assert_eq!(asset.fps, Rational::new(20, 1).unwrap());
    assert_eq!(asset.duration, TimeCode(30));

    let mut decoder = VideoDecoder::open(&path, asset.fps).unwrap();
    let mut cache = FrameCache::new(4);
    decoder
        .decode_window(TimeCode(0), TimeCode(1), &mut cache)
        .unwrap();
    let first = cache.frame_at_or_before(TimeCode(0)).unwrap();
    let held = cache.frame_at_or_before(TimeCode(1)).unwrap();
    assert_eq!(
        first.rgba, held.rgba,
        "the 50ms grid slot must hold the 0ms frame"
    );

    let mut second_decoder = VideoDecoder::open(&path, asset.fps).unwrap();
    let mut second_cache = FrameCache::new(2);
    second_decoder
        .decode_window(TimeCode(1), TimeCode(1), &mut second_cache)
        .unwrap();
    assert_eq!(
        held.rgba,
        second_cache.frame_at_or_before(TimeCode(1)).unwrap().rgba,
        "repeated seeks must select the same source PTS"
    );

    let mut proxy_decoder = VideoDecoder::open_scaled(&path, asset.fps, Some(80)).unwrap();
    let mut proxy_cache = FrameCache::new(4);
    proxy_decoder
        .decode_window(TimeCode(0), TimeCode(0), &mut proxy_cache)
        .unwrap();
    proxy_decoder
        .decode_window_sequential(TimeCode(1), TimeCode(2), &mut proxy_cache)
        .unwrap();
    assert_eq!(proxy_decoder.seek_count(), 1);
    let proxy_first = proxy_cache.frame_at_or_before(TimeCode(0)).unwrap();
    let proxy_held = proxy_cache.frame_at_or_before(TimeCode(1)).unwrap();
    assert_eq!((proxy_first.width, proxy_first.height), (80, 45));
    assert_eq!(
        proxy_first.rgba, proxy_held.rgba,
        "sequential proxy decode must preserve the PTS hold across windows"
    );
}

#[test]
fn display_matrix_rotation_changes_probe_and_decode_dimensions() {
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-focused");
    let path = directory.path("rotated.mp4");
    generate_rotated(&path, &directory);
    let asset = probe_path(&path, AssetId(1)).unwrap();

    assert_eq!(asset.resolution, Some((54, 96)));
    let mut decoder = VideoDecoder::open(&path, asset.fps).unwrap();
    let mut cache = FrameCache::new(2);
    decoder
        .decode_window(TimeCode::ZERO, TimeCode::ZERO, &mut cache)
        .unwrap();
    let frame = cache.frame_at_or_before(TimeCode::ZERO).unwrap();
    assert_eq!((frame.width, frame.height), (54, 96));
    assert_rotated_quadrants(&frame);

    let thumbnail = crate::decode::thumbnail(&path, asset.fps, TimeCode::ZERO, 54).unwrap();
    assert_eq!((thumbnail.width, thumbnail.height), (54, 96));
    assert_eq!(thumbnail.pixels, *frame.rgba);
}

fn assert_rotated_quadrants(frame: &kinewright_core::FrameTexture) {
    let sample = |x: u32, y: u32| {
        let offset = usize::try_from(y * frame.width + x).unwrap() * 4;
        [
            frame.rgba[offset],
            frame.rgba[offset + 1],
            frame.rgba[offset + 2],
        ]
    };
    let top_left = sample(frame.width / 4, frame.height / 4);
    let top_right = sample(frame.width * 3 / 4, frame.height / 4);
    let bottom_left = sample(frame.width / 4, frame.height * 3 / 4);
    let bottom_right = sample(frame.width * 3 / 4, frame.height * 3 / 4);
    assert!(top_left[2] > 180 && top_left[0] < 50 && top_left[1] < 50);
    assert!(top_right[0] > 180 && top_right[1] < 50 && top_right[2] < 50);
    assert!(bottom_left[0] > 180 && bottom_left[1] > 180 && bottom_left[2] < 50);
    assert!(bottom_right[1] > 70 && bottom_right[0] < 50 && bottom_right[2] < 50);
}

fn decode_at(path: &Path, asset: &MediaAsset, at: TimeCode) -> kinewright_core::FrameTexture {
    let mut decoder = VideoDecoder::open(path, asset.fps).unwrap();
    let mut cache = FrameCache::new(2);
    decoder.decode_window(at, at, &mut cache).unwrap();
    assert!(
        cache.contains(at),
        "decoder did not populate exact CFR grid frame {at}"
    );
    cache.frame_at_or_before(at).unwrap()
}

fn validate_video_case(case: &MatrixCase, asset: &MediaAsset) {
    let positions = [
        TimeCode::ZERO,
        TimeCode(asset.duration.0 / 2),
        TimeCode(asset.duration.0.saturating_sub(1)),
    ];
    for at in positions {
        let frame = decode_at(&case.path, asset, at);
        assert_eq!(
            Some((frame.width, frame.height)),
            case.expected_resolution,
            "wrong decoded dimensions for {} at {at}",
            case.name
        );
        assert_eq!(
            frame.rgba.len(),
            usize::try_from(frame.width).unwrap() * usize::try_from(frame.height).unwrap() * 4,
            "wrong RGBA byte count for {} at {at}",
            case.name
        );
    }
    let middle = positions[1];
    let first_seek = decode_at(&case.path, asset, middle);
    let second_seek = decode_at(&case.path, asset, middle);
    assert_eq!(
        first_seek.rgba, second_seek.rgba,
        "seek was not deterministic for {} at {middle}",
        case.name
    );
}

// Generated fixture lengths are small and exactly adequate for f32 energy normalization.
#[allow(clippy::cast_precision_loss)]
fn validate_audio_case(case: &MatrixCase, asset: &MediaAsset) {
    let samples = decode_audio_range(
        &case.path,
        asset.fps,
        TimeCode::ZERO,
        asset.duration,
        48_000,
        2,
        &ExportCancellation::default(),
    )
    .unwrap_or_else(|error| panic!("audio decode failed for {}: {error}", case.name));
    assert!(
        !samples.is_empty(),
        "audio decode was empty for {}",
        case.name
    );
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "audio decode produced non-finite samples for {}",
        case.name
    );
    let energy = samples.iter().map(|sample| sample.abs()).sum::<f32>() / samples.len() as f32;
    assert!(energy > 0.01, "audio decode was silent for {}", case.name);
}

fn build_mixed_document(assets: &[MediaAsset]) -> (Document, HashSet<AssetId>) {
    let mut document = Document {
        fps: Rational::new(30, 1).unwrap(),
        resolution: (160, 90),
        ..Document::default()
    };
    Operation::AddTrack {
        track: Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    for asset in assets {
        Operation::AddAsset {
            asset: asset.clone(),
        }
        .apply(&mut document)
        .unwrap();
    }

    let mut included = HashSet::new();
    let mut video_at = TimeCode::ZERO;
    for asset in assets
        .iter()
        .filter(|asset| matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo))
    {
        let source_end = TimeCode(asset.duration.0.min(6));
        Operation::AddClip {
            track: TrackId(1),
            asset: asset.id,
            at: video_at,
            source: TimeCode::ZERO..source_end,
        }
        .apply(&mut document)
        .unwrap();
        let length =
            map_source_range_to_project(TimeCode::ZERO..source_end, asset.fps, document.fps)
                .unwrap();
        video_at = video_at.checked_add(length).unwrap();
        included.insert(asset.id);
    }

    let mut audio_at = TimeCode::ZERO;
    for asset in assets.iter().filter(|asset| asset.kind == MediaKind::Audio) {
        let source_end = TimeCode(asset.duration.0.min(12));
        Operation::AddClip {
            track: TrackId(2),
            asset: asset.id,
            at: audio_at,
            source: TimeCode::ZERO..source_end,
        }
        .apply(&mut document)
        .unwrap();
        let length =
            map_source_range_to_project(TimeCode::ZERO..source_end, asset.fps, document.fps)
                .unwrap();
        audio_at = audio_at.checked_add(length).unwrap();
        included.insert(asset.id);
    }
    document.validate().unwrap();
    (document, included)
}

fn validate_mixed_export(directory: &MatrixDirectory, assets: &[MediaAsset]) -> HashSet<AssetId> {
    let (document, included) = build_mixed_document(assets);
    assert_eq!(
        included.len(),
        assets.len(),
        "every matrix asset must be exported"
    );
    let output = directory.path("mixed-hostile-export.mp4");
    let gpu = GpuContext::headless(false)
        .or_else(|_| GpuContext::headless(true))
        .unwrap();
    let (progress, updates) = crossbeam_channel::unbounded();
    let settings = ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 750_000,
        audio_bitrate: 128_000,
        cancellation: ExportCancellation::default(),
    };
    export_document(&document, &output, &settings, &progress, gpu).unwrap();
    let final_progress = updates.try_iter().last().unwrap();
    assert_eq!(
        final_progress.completed_frames, final_progress.total_frames,
        "mixed export did not complete"
    );
    let exported = probe_path(&output, AssetId(u64::MAX)).unwrap();
    assert_eq!(exported.kind, MediaKind::AudioVideo);
    assert_eq!(exported.fps, document.fps);
    assert_eq!(exported.resolution, Some(document.resolution));
    validate_video_case(
        &video_case(
            "mixed-hostile-export",
            output,
            document.fps,
            document.resolution,
            document.duration.0,
            true,
        ),
        &exported,
    );
    included
}

fn validate_matrix(directory: &MatrixDirectory, cases: &[MatrixCase], note: Option<&str>) {
    let mut assets = Vec::with_capacity(cases.len());
    for (index, case) in cases.iter().enumerate() {
        let asset = probe_path(&case.path, AssetId(index as u64 + 1))
            .unwrap_or_else(|error| panic!("probe failed for {}: {error}", case.name));
        assert_eq!(asset.fps, case.expected_fps, "wrong fps for {}", case.name);
        assert_eq!(
            asset.duration, case.expected_duration,
            "wrong duration for {}",
            case.name
        );
        assert_eq!(
            asset.resolution, case.expected_resolution,
            "wrong display resolution for {}",
            case.name
        );
        assert_eq!(
            asset.kind, case.expected_kind,
            "wrong kind for {}",
            case.name
        );
        if case.has_video {
            validate_video_case(case, &asset);
        }
        if case.has_audio {
            validate_audio_case(case, &asset);
        }
        assets.push(asset);
    }

    let included = validate_mixed_export(directory, &assets);
    println!("| file | probe | first/middle/last | seek | audio | export inclusion |");
    println!("|---|---|---|---|---|---|");
    for (case, asset) in cases.iter().zip(&assets) {
        let resolution = asset.resolution.map_or_else(
            || "audio-only".to_owned(),
            |(width, height)| format!("{width}x{height}"),
        );
        println!(
            "| {} | {}/{} fps, {} frames, {} | {} | {} | {} | {} |",
            case.name,
            asset.fps.numerator(),
            asset.fps.denominator(),
            asset.duration.0,
            resolution,
            if case.has_video { "pass" } else { "n/a" },
            if case.has_video { "pass" } else { "n/a" },
            if case.has_audio { "pass" } else { "n/a" },
            if included.contains(&asset.id) {
                "pass"
            } else {
                "fail"
            },
        );
    }
    if let Some(note) = note {
        println!("matrix note: {note}");
    }
}

#[test]
fn fast_media_matrix_covers_hostile_probe_decode_audio_seek_and_export() {
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-focused");
    let (cases, note) = generate_fast_matrix(&directory);
    validate_matrix(&directory, &cases, note.as_deref());

    let broken = directory.path("truncated.mp4");
    fs::write(&broken, b"incomplete media container").unwrap();
    let error = probe_path(&broken, AssetId(999)).unwrap_err().to_string();
    assert!(error.contains("truncated.mp4"));
    assert!(error.contains("truncated") || error.contains("unsupported"));
}

#[test]
fn full_media_matrix_covers_4k_hevc_and_long_gop_sources() {
    if std::env::var_os("KINEWRIGHT_MEDIA_MATRIX").as_deref() != Some(OsStr::new("1")) {
        eprintln!("skipped: set KINEWRIGHT_MEDIA_MATRIX=1 for the full hostile-media matrix");
        return;
    }
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-focused");
    let (mut cases, mut note) = generate_fast_matrix(&directory);
    add_full_matrix_cases(&directory, &mut cases, &mut note);
    validate_matrix(&directory, &cases, note.as_deref());
}

const PERF_SEEKS: usize = 20;
const COLD_SEEK_P95_BUDGET: Duration = Duration::from_millis(250);
const SCRUB_STEP_P95_BUDGET: Duration = Duration::from_millis(250);
const SEQUENTIAL_MIN_FPS: f64 = 60.0;

struct PerfResult {
    name: String,
    proxy: (u32, u32),
    cold_p50: Duration,
    cold_p95: Duration,
    sequential_fps: f64,
    sequential_seeks: u64,
    scrub_p50: Duration,
    scrub_p95: Duration,
}

fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
    samples.sort_unstable();
    let rank = samples
        .len()
        .saturating_mul(percent)
        .div_ceil(100)
        .saturating_sub(1);
    samples[rank.min(samples.len().saturating_sub(1))]
}

fn assert_proxy_frame(cache: &mut FrameCache, at: TimeCode) -> (u32, u32) {
    assert!(
        cache.contains(at),
        "proxy decode did not cache exact frame {at}"
    );
    let frame = cache.frame_at_or_before(at).unwrap();
    assert!(frame.width <= PREVIEW_MAX_WIDTH);
    (frame.width, frame.height)
}

// Performance throughput is intentionally reported as an approximate f64 rate.
#[allow(clippy::cast_precision_loss)]
fn measure_proxy_performance(case: &MatrixCase) -> PerfResult {
    let asset = probe_path(&case.path, AssetId(1)).unwrap();
    let last = asset.duration.0.saturating_sub(1).max(1);
    let random_positions = (0..PERF_SEEKS)
        .map(|index| TimeCode(1 + (i64::try_from(index).unwrap() * 47 + 13) % last))
        .collect::<Vec<_>>();

    let mut seek_decoder =
        VideoDecoder::open_scaled(&case.path, asset.fps, Some(PREVIEW_MAX_WIDTH)).unwrap();
    let mut cold_samples = Vec::with_capacity(PERF_SEEKS);
    let mut proxy = (0, 0);
    for at in random_positions {
        let mut cache = FrameCache::new(2);
        let started = Instant::now();
        seek_decoder.decode_window(at, at, &mut cache).unwrap();
        cold_samples.push(started.elapsed());
        proxy = assert_proxy_frame(&mut cache, at);
    }

    let mut sequential_decoder =
        VideoDecoder::open_scaled(&case.path, asset.fps, Some(PREVIEW_MAX_WIDTH)).unwrap();
    let mut sequential_cache = FrameCache::new(4);
    let sequential_started = Instant::now();
    let mut start = TimeCode::ZERO;
    while start < asset.duration {
        let end = TimeCode(
            start
                .0
                .saturating_add(14)
                .min(asset.duration.0.saturating_sub(1)),
        );
        if start == TimeCode::ZERO {
            sequential_decoder
                .decode_window(start, end, &mut sequential_cache)
                .unwrap();
        } else {
            sequential_decoder
                .decode_window_sequential(start, end, &mut sequential_cache)
                .unwrap();
        }
        start = TimeCode(end.0.saturating_add(1));
    }
    let sequential_elapsed = sequential_started.elapsed();
    let sequential_fps = asset.duration.0 as f64 / sequential_elapsed.as_secs_f64();
    let sequential_seeks = sequential_decoder.seek_count();

    let scrub_step = (last / i64::try_from(PERF_SEEKS).unwrap()).max(1);
    let mut scrub_decoder =
        VideoDecoder::open_scaled(&case.path, asset.fps, Some(PREVIEW_MAX_WIDTH)).unwrap();
    let mut scrub_samples = Vec::with_capacity(PERF_SEEKS);
    for index in 0..PERF_SEEKS {
        let at = TimeCode((1 + i64::try_from(index).unwrap().saturating_mul(scrub_step)).min(last));
        let mut cache = FrameCache::new(2);
        let started = Instant::now();
        scrub_decoder.decode_window(at, at, &mut cache).unwrap();
        scrub_samples.push(started.elapsed());
        assert_proxy_frame(&mut cache, at);
    }

    PerfResult {
        name: case.name.clone(),
        proxy,
        cold_p50: percentile(&mut cold_samples.clone(), 50),
        cold_p95: percentile(&mut cold_samples, 95),
        sequential_fps,
        sequential_seeks,
        scrub_p50: percentile(&mut scrub_samples.clone(), 50),
        scrub_p95: percentile(&mut scrub_samples, 95),
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[test]
fn proxy_preview_performance_on_heavy_matrix_media() {
    if std::env::var_os("KINEWRIGHT_PERF_TEST").as_deref() != Some(OsStr::new("1")) {
        eprintln!("skipped: set KINEWRIGHT_PERF_TEST=1 for proxy preview measurements");
        return;
    }
    initialize_ffmpeg().unwrap();
    let directory = MatrixDirectory::new("m8-focused");
    let mut cases = Vec::new();
    let mut note = None;
    add_full_matrix_cases(&directory, &mut cases, &mut note);
    let results = cases
        .iter()
        .map(measure_proxy_performance)
        .collect::<Vec<_>>();

    println!(
        "| file | proxy | cold seek p50 | cold seek p95 | sequential decode | decoder seeks | scrub step p50 | scrub step p95 |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for result in &results {
        println!(
            "| {} | {}x{} | {:.1} ms | {:.1} ms | {:.1} fps | {} | {:.1} ms | {:.1} ms |",
            result.name,
            result.proxy.0,
            result.proxy.1,
            milliseconds(result.cold_p50),
            milliseconds(result.cold_p95),
            result.sequential_fps,
            result.sequential_seeks,
            milliseconds(result.scrub_p50),
            milliseconds(result.scrub_p95),
        );
    }
    if let Some(note) = note {
        println!("performance note: {note}");
    }

    for result in &results {
        assert!(
            result.cold_p95 < COLD_SEEK_P95_BUDGET,
            "{} proxy cold-seek p95 {:.1} ms exceeded {:.1} ms",
            result.name,
            milliseconds(result.cold_p95),
            milliseconds(COLD_SEEK_P95_BUDGET),
        );
        assert!(
            result.sequential_fps >= SEQUENTIAL_MIN_FPS,
            "{} sequential proxy decode {:.1} fps was below {:.1} fps",
            result.name,
            result.sequential_fps,
            SEQUENTIAL_MIN_FPS,
        );
        assert_eq!(
            result.sequential_seeks, 1,
            "{} sequential proxy decode re-sought between windows",
            result.name,
        );
        assert!(
            result.scrub_p95 < SCRUB_STEP_P95_BUDGET,
            "{} proxy scrub-step p95 {:.1} ms exceeded {:.1} ms",
            result.name,
            milliseconds(result.scrub_p95),
            milliseconds(SCRUB_STEP_P95_BUDGET),
        );
    }
}
