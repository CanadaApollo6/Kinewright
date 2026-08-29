use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use kinewright_core::{
    Analysis, AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorDescription, ColorMatrix,
    ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer, ColorWhitePoint, Document, Effect,
    EffectId, Export, ExportCancellation, ExportSettings, FreezeFrame, MediaAsset,
    MediaAvailabilityKind, MediaCacheFamily, MediaError, MediaEvent, MediaKind, Operation,
    ParamValue, Playback, PlaybackState, Rational, RelinkCandidate, TimeCode, Title, Track,
    TrackId, TrackKind, Transition,
};
use kinewright_media::{FfmpegMediaEngine, source_fingerprint};

#[path = "../src/test_support.rs"]
pub mod test_support;
use test_support::ffmpeg_executable;

/// The audio-device smoke tests are `#[ignore]`d so a run that does not
/// exercise them cannot report a pass. The opt-in is still required, because
/// `--ignored` alone does not mean a device exists.
fn require_audio_device_opt_in() {
    assert_eq!(
        std::env::var_os("KINEWRIGHT_AUDIO_TEST").as_deref(),
        Some(std::ffi::OsStr::new("1")),
        "set KINEWRIGHT_AUDIO_TEST=1 on a machine with an audio device to run this test"
    );
}

struct TestClip(PathBuf);

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl TestClip {
    fn path(&self) -> &Path {
        &self.0
    }

    fn generate() -> Self {
        let ffmpeg = ffmpeg_executable();
        assert!(
            ffmpeg.is_file(),
            "provisioned ffmpeg CLI is missing at {}",
            ffmpeg.display()
        );
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output =
            std::env::temp_dir().join(format!("kinewright-m1-{}-{nonce}.mp4", std::process::id()));
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
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
                "-x264-params",
                "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv",
                "-g",
                "60",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&output)
            .status()
            .expect("failed to run provisioned ffmpeg CLI");
        assert!(result.success(), "test media generation failed");
        Self(output)
    }
}

impl Drop for TestClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An independent SHA-256, used so the probe fingerprint is checked against a
/// second implementation instead of against the same crate function that
/// produced it.
mod reference_sha256 {
    use std::fmt::Write as _;

    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    /// Hash `bytes` and return the lowercase hex digest.
    pub fn hex_digest(bytes: &[u8]) -> String {
        let mut state: [u32; 8] = [
            0x6a09_e667,
            0xbb67_ae85,
            0x3c6e_f372,
            0xa54f_f53a,
            0x510e_527f,
            0x9b05_688c,
            0x1f83_d9ab,
            0x5be0_cd19,
        ];
        let mut message = bytes.to_vec();
        let bit_length = (bytes.len() as u64) * 8;
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_length.to_be_bytes());
        for block in message.as_chunks::<64>().0 {
            let mut schedule = [0_u32; 64];
            for (index, word) in block.as_chunks::<4>().0.iter().enumerate() {
                schedule[index] = u32::from_be_bytes(*word);
            }
            for index in 16..64 {
                let previous = schedule[index - 15];
                let ahead = schedule[index - 2];
                let s0 = previous.rotate_right(7) ^ previous.rotate_right(18) ^ (previous >> 3);
                let s1 = ahead.rotate_right(17) ^ ahead.rotate_right(19) ^ (ahead >> 10);
                schedule[index] = schedule[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(schedule[index - 7])
                    .wrapping_add(s1);
            }
            let mut working = state;
            for index in 0..64 {
                let s1 = working[4].rotate_right(6)
                    ^ working[4].rotate_right(11)
                    ^ working[4].rotate_right(25);
                let choose = (working[4] & working[5]) ^ (!working[4] & working[6]);
                let temp1 = working[7]
                    .wrapping_add(s1)
                    .wrapping_add(choose)
                    .wrapping_add(K[index])
                    .wrapping_add(schedule[index]);
                let s0 = working[0].rotate_right(2)
                    ^ working[0].rotate_right(13)
                    ^ working[0].rotate_right(22);
                let majority = (working[0] & working[1])
                    ^ (working[0] & working[2])
                    ^ (working[1] & working[2]);
                let temp2 = s0.wrapping_add(majority);
                working[7] = working[6];
                working[6] = working[5];
                working[5] = working[4];
                working[4] = working[3].wrapping_add(temp1);
                working[3] = working[2];
                working[2] = working[1];
                working[1] = working[0];
                working[0] = temp1.wrapping_add(temp2);
            }
            for (slot, value) in state.iter_mut().zip(working) {
                *slot = slot.wrapping_add(value);
            }
        }
        let mut digest = String::with_capacity(64);
        for word in state {
            let _ = write!(digest, "{word:08x}");
        }
        digest
    }
}

#[test]
fn reference_sha256_matches_the_published_test_vectors() {
    assert_eq!(
        reference_sha256::hex_digest(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        reference_sha256::hex_digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        reference_sha256::hex_digest(&vec![b'a'; 1_000_000]),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn probe_records_the_source_sha256_and_byte_length() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(clip.path()).unwrap();
    let bytes = std::fs::read(clip.path()).unwrap();
    // The digest half is checked against an independent SHA-256 rather than
    // against `source_fingerprint`, which is the function under test.
    assert_eq!(
        asset.source_fingerprint.content_sha256.as_deref(),
        Some(reference_sha256::hex_digest(&bytes).as_str())
    );
    assert_eq!(
        asset.source_fingerprint.byte_len,
        Some(std::fs::metadata(clip.path()).unwrap().len())
    );
    assert_eq!(asset.source_fingerprint.byte_len, Some(bytes.len() as u64));
    assert_eq!(
        asset.source_fingerprint,
        source_fingerprint(clip.path()).unwrap(),
        "probe and the standalone fingerprint helper must agree"
    );
}

#[test]
fn cache_inventory_is_honest_and_scoped_clear_is_idempotent() {
    let directory = test_support::TempDirectory::new("cache-inventory");
    let visual_root = directory.path("visual-assets/v1");
    std::fs::create_dir_all(&visual_root).unwrap();
    std::fs::write(visual_root.join("fixture.rgba"), b"visual-cache").unwrap();
    let source = directory.path("source.mp4");
    std::fs::write(&source, b"source-media").unwrap();

    let engine = FfmpegMediaEngine::new_with_data_dir(directory.root().to_path_buf()).unwrap();
    let inventory = engine.cache_inventory();
    assert_eq!(inventory.families.len(), 5);
    let visual = inventory
        .families
        .iter()
        .find(|family| family.family == MediaCacheFamily::VisualAssets)
        .unwrap();
    assert!(visual.supported);
    assert_eq!(visual.file_count, 1);
    assert_eq!(visual.bytes, 12);
    let transcript_root = directory.root().join("transcripts/v2");
    let transcripts = inventory
        .families
        .iter()
        .find(|family| family.family == MediaCacheFamily::Transcripts)
        .unwrap();
    assert_eq!(transcripts.root.as_deref(), Some(transcript_root.as_path()));
    let proxy = inventory
        .families
        .iter()
        .find(|family| family.family == MediaCacheFamily::GeneratedProxy)
        .unwrap();
    assert!(!proxy.supported);
    assert_eq!(proxy.file_count, 0);
    assert!(proxy.note.as_deref().unwrap().contains("not supported"));

    let cleared = engine.clear_cache(MediaCacheFamily::VisualAssets).unwrap();
    assert_eq!(cleared.removed_file_count, 1);
    assert_eq!(cleared.removed_bytes, 12);
    assert!(cleared.may_repopulate);
    assert_eq!(std::fs::read(&source).unwrap(), b"source-media");
    let cleared_again = engine.clear_cache(MediaCacheFamily::VisualAssets).unwrap();
    assert_eq!(cleared_again.removed_file_count, 0);
    assert_eq!(cleared_again.removed_bytes, 0);
    let unsupported = engine
        .clear_cache(MediaCacheFamily::GeneratedProxy)
        .unwrap();
    assert!(!unsupported.supported);
    assert_eq!(unsupported.removed_file_count, 0);
}

fn generate_solid(name: &str, color: &str, frequency: &str) -> TemporaryFile {
    let ffmpeg = ffmpeg_executable();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = std::env::temp_dir().join(format!(
        "kinewright-{name}-{}-{nonce}.mp4",
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
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv",
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
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: red_asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    content: kinewright_core::ClipContent::Media,
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
                    asset: blue_asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: vec![Effect {
                        id: EffectId(1),
                        name: "opacity".to_owned(),
                        parameters: BTreeMap::from([(
                            "percent".to_owned(),
                            ParamValue::Integer(50),
                        )]),
                        keyframes: BTreeMap::new(),
                    }],
                    transition_in: Some(Transition {
                        name: "crossfade".to_owned(),
                        duration: TimeCode(5),
                    }),
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
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
#[allow(clippy::too_many_lines)]
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
    assert_frame_center_close(&preview_blended, [180, 0, 180], 8);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = TemporaryFile(std::env::temp_dir().join(format!(
        "kinewright-export-{}-{nonce}.mp4",
        std::process::id()
    )));
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    engine
        .export(
            &output.0,
            ExportSettings {
                fps: document.fps,
                resolution: document.resolution,
                delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
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
    assert_eq!(
        exported_asset.color_description,
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
            hdr_static_metadata: kinewright_core::HdrStaticMetadata::unknown(),
        },
        "the encoded stream must re-probe with every representable Rec.709 stream tag; H.264 has no separate white-point tag"
    );
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

    let mut fade_document = document.clone();
    fade_document.tracks[1].clips[0].transition_in = Some(Transition {
        name: "fade_from_white".to_owned(),
        duration: TimeCode(5),
    });
    fade_document.validate().unwrap();
    engine.set_document(std::sync::Arc::new(fade_document.clone()));
    engine.request_frame(TimeCode(0));
    let fade_preview_start = receive_frame(&frames, TimeCode(0));
    engine.request_frame(TimeCode(2));
    let fade_preview_middle = receive_frame(&frames, TimeCode(2));
    assert_frame_center_close(&fade_preview_start, [255, 255, 255], 5);
    assert_frame_center_close(&fade_preview_middle, [180, 180, 255], 8);

    let fade_output = TemporaryFile(std::env::temp_dir().join(format!(
        "kinewright-fade-export-{}-{nonce}.mp4",
        std::process::id()
    )));
    let (fade_progress_tx, _) = crossbeam_channel::unbounded();
    engine
        .export(
            &fade_output.0,
            ExportSettings {
                fps: fade_document.fps,
                resolution: fade_document.resolution,
                delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
                video_codec: "libx264".to_owned(),
                audio_codec: "aac".to_owned(),
                video_bitrate: 500_000,
                audio_bitrate: 128_000,
                cancellation: ExportCancellation::default(),
            },
            fade_progress_tx,
        )
        .unwrap();
    let fade_asset = decode_engine.probe(&fade_output.0).unwrap();
    decode_engine.set_document(std::sync::Arc::new(full_timeline(fade_asset)));
    decode_engine.request_frame(TimeCode(0));
    let fade_decoded_start = receive_frame(&exported_frames, TimeCode(0));
    decode_engine.request_frame(TimeCode(2));
    let fade_decoded_middle = receive_frame(&exported_frames, TimeCode(2));
    assert_frame_sample_close(&fade_preview_start, &fade_decoded_start, 28);
    assert_frame_sample_close(&fade_preview_middle, &fade_decoded_middle, 28);
    remove_fixture_assets(&document);
}

#[test]
fn title_export_pixels_match_preview_after_h264_redecode() {
    let engine = FfmpegMediaEngine::new().unwrap();
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: AssetId::default(),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Title(Title {
                    text: "Preview = Export".to_owned(),
                    ..Title::default()
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps: Rational::new(10, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(10),
    };
    document.validate().unwrap();
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document.clone()));
    engine.request_frame(TimeCode(5));
    let preview = receive_frame(&frames, TimeCode(5));
    assert!(
        preview
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..3] != [0, 0, 0])
            .count()
            > 500,
        "preview contains no visible title pixels"
    );

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = TemporaryFile(std::env::temp_dir().join(format!(
        "kinewright-title-export-{}-{nonce}.mp4",
        std::process::id()
    )));
    let (progress, _) = crossbeam_channel::unbounded();
    engine
        .export(
            &output.0,
            ExportSettings {
                fps: document.fps,
                resolution: document.resolution,
                delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
                video_codec: "libx264".to_owned(),
                audio_codec: "aac".to_owned(),
                video_bitrate: 1_000_000,
                audio_bitrate: 128_000,
                cancellation: ExportCancellation::default(),
            },
            progress,
        )
        .unwrap();

    let decode_engine = FfmpegMediaEngine::new().unwrap();
    let exported_asset = decode_engine.probe(&output.0).unwrap();
    let exported_document = full_timeline(exported_asset);
    let exported_frames = decode_engine.frames();
    decode_engine.set_document(std::sync::Arc::new(exported_document));
    decode_engine.request_frame(TimeCode(5));
    let decoded = receive_frame(&exported_frames, TimeCode(5));
    assert_title_frame_close(&preview, &decoded);
}

#[test]
fn freeze_export_pixels_match_preview_after_h264_redecode() {
    let input = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&input.0).unwrap();
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Freeze(FreezeFrame {
                    source_frame: TimeCode(20),
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(30_000, 1_001).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(30),
    };
    document.validate().unwrap();
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document.clone()));
    engine.request_frame(TimeCode(2));
    let preview_start = receive_frame(&frames, TimeCode(2));
    engine.request_frame(TimeCode(24));
    let preview_end = receive_frame(&frames, TimeCode(24));
    assert_frame_sample_close(&preview_start, &preview_end, 0);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = TemporaryFile(std::env::temp_dir().join(format!(
        "kinewright-freeze-export-{}-{nonce}.mp4",
        std::process::id()
    )));
    let (progress, _) = crossbeam_channel::unbounded();
    engine
        .export(
            &output.0,
            ExportSettings {
                fps: document.fps,
                resolution: document.resolution,
                delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
                video_codec: "libx264".to_owned(),
                audio_codec: "aac".to_owned(),
                video_bitrate: 1_000_000,
                audio_bitrate: 128_000,
                cancellation: ExportCancellation::default(),
            },
            progress,
        )
        .unwrap();

    let decode_engine = FfmpegMediaEngine::new().unwrap();
    let exported_asset = decode_engine.probe(&output.0).unwrap();
    let exported_document = full_timeline(exported_asset);
    let exported_frames = decode_engine.frames();
    decode_engine.set_document(std::sync::Arc::new(exported_document));
    decode_engine.request_frame(TimeCode(15));
    let decoded = receive_frame(&exported_frames, TimeCode(15));
    assert_frame_sample_close(&preview_start, &decoded, 28);
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
        .as_chunks::<8>()
        .0
        .iter()
        .map(|stereo| {
            let left = f32::from_le_bytes(stereo[0..4].try_into().unwrap());
            let right = f32::from_le_bytes(stereo[4..8].try_into().unwrap());
            left.midpoint(right)
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
        "kinewright-cancelled-export-{}.mp4",
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
            delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
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
    expected: &kinewright_core::FrameTexture,
    actual: &kinewright_core::FrameTexture,
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

fn assert_title_frame_close(
    preview: &kinewright_core::FrameTexture,
    exported: &kinewright_core::FrameTexture,
) {
    assert_eq!(
        (preview.width, preview.height),
        (exported.width, exported.height)
    );
    let differences = preview
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(exported.rgba.as_chunks::<4>().0.iter())
        .flat_map(|(left, right)| (0..3).map(move |channel| left[channel].abs_diff(right[channel])))
        .collect::<Vec<_>>();
    let total = differences
        .iter()
        .map(|value| u64::from(*value))
        .sum::<u64>();
    #[allow(clippy::cast_precision_loss)]
    let mean = total as f64 / differences.len() as f64;
    let outliers = differences
        .iter()
        .filter(|difference| **difference > 40)
        .count();
    assert!(
        mean <= 8.0,
        "mean preview/export channel delta was {mean:.2}"
    );
    assert!(
        outliers * 100 <= differences.len(),
        "more than one percent of title channels exceeded codec tolerance"
    );
}

fn assert_frame_center_close(
    frame: &kinewright_core::FrameTexture,
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
    let events = engine.events();
    engine.set_document(std::sync::Arc::new(document));

    engine.request_frame(TimeCode(0));
    let first = receive_frame_checked(&frames, &events, TimeCode(0));
    engine.request_frame(TimeCode(30));
    let second = receive_frame_checked(&frames, &events, TimeCode(30));

    assert_eq!((first.width, first.height), (320, 180));
    assert_eq!(first.rgba.len(), 320 * 180 * 4);
    assert_ne!(first.rgba, second.rgba);
}

#[test]
fn relinked_moved_source_round_trip_renders_identical_frame() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let original_asset = engine.probe(clip.path()).unwrap();
    let document = full_timeline(original_asset.clone());
    let frames = engine.frames();
    engine.set_document(std::sync::Arc::new(document.clone()));
    engine.request_frame(TimeCode(30));
    let before_move = receive_frame(&frames, TimeCode(30));

    // Exercise the persisted project boundary before the source path changes.
    let encoded = serde_json::to_vec(&document).unwrap();
    let mut relinked_document: Document = serde_json::from_slice(&encoded).unwrap();
    // Flush the worker's decoder before renaming. This also keeps the test
    // valid on Windows, where an open FFmpeg handle can prevent a rename.
    engine.set_document(std::sync::Arc::new(Document::default()));
    engine.request_frame(TimeCode::ZERO);
    let _ = receive_frame(&frames, TimeCode::ZERO);
    let moved = TemporaryFile(std::env::temp_dir().join(format!(
        "kinewright-relinked-{}-{}.mp4",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    std::fs::rename(clip.path(), &moved.0).unwrap();

    assert_eq!(
        engine.media_availability(&original_asset).kind,
        MediaAvailabilityKind::OfflineMissing
    );

    let replacement = engine.probe(&moved.0).unwrap();
    let candidate = RelinkCandidate {
        path: moved.0.clone(),
        fingerprint: replacement.source_fingerprint,
        kind: replacement.kind,
        fps: replacement.fps,
        duration: replacement.duration,
        resolution: replacement.resolution,
    };
    Operation::RelinkAsset {
        asset: original_asset.id,
        candidate,
        allow_unverified_source: false,
    }
    .apply(&mut relinked_document)
    .unwrap();
    let relinked_asset = relinked_document.asset(original_asset.id).unwrap();
    assert_eq!(relinked_asset.path, moved.0);
    assert_eq!(
        relinked_asset.source_fingerprint,
        original_asset.source_fingerprint
    );

    engine.set_document(std::sync::Arc::new(relinked_document));
    engine.request_frame(TimeCode(30));
    let after_relink = receive_frame(&frames, TimeCode(30));
    assert_eq!(
        (before_move.width, before_move.height),
        (after_relink.width, after_relink.height)
    );
    assert_eq!(before_move.rgba.as_ref(), after_relink.rgba.as_ref());
}

#[test]
fn timeline_decode_selects_two_clips_and_renders_the_gap_black() {
    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(5)..TimeCode(15),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(30)..TimeCode(40),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
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
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3] != [0, 0, 0])
    );
    assert!(
        second_clip
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3] != [0, 0, 0])
    );
    assert_ne!(first_clip.rgba, second_clip.rgba);
    for gap in [gap_start, gap_end] {
        let pixels = gap.rgba.as_chunks::<4>().0;
        // An empty raster would satisfy `all` vacuously, which is exactly the
        // failure this assertion exists to catch.
        assert_eq!(
            pixels.len(),
            (gap.width * gap.height) as usize,
            "gap frame raster is empty or ragged"
        );
        assert!(!pixels.is_empty(), "gap frame has no pixels to check");
        assert!(pixels.iter().all(|pixel| *pixel == [0, 0, 0, 255]));
    }
}

#[test]
#[ignore = "requires an audio device; run with KINEWRIGHT_AUDIO_TEST=1 and --ignored"]
fn audio_device_play_pause_and_seek_smoke_test() {
    require_audio_device_opt_in();

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
#[ignore = "requires an audio device; run with KINEWRIGHT_AUDIO_TEST=1 and --ignored"]
fn multi_track_audio_device_play_pause_and_seek_smoke_test() {
    require_audio_device_opt_in();

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
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: voice_asset.id,
                    source_range: TimeCode::ZERO..duration,
                    content: kinewright_core::ClipContent::Media,
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
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: bed_asset.id,
                    source_range: TimeCode(2)..duration,
                    content: kinewright_core::ClipContent::Media,
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
#[ignore = "requires an audio device; run with KINEWRIGHT_AUDIO_TEST=1 and --ignored"]
fn timeline_audio_crosses_a_clip_boundary_and_gap_smoke_test() {
    require_audio_device_opt_in();

    let clip = TestClip::generate();
    let engine = FfmpegMediaEngine::new().unwrap();
    let asset = engine.probe(&clip.0).unwrap();
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(10),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(20)..TimeCode(40),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(15),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
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
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset_id,
                source_range: TimeCode::ZERO..asset_duration,
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
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
    frames: &crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)>,
    requested: TimeCode,
) -> kinewright_core::FrameTexture {
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

fn receive_frame_checked(
    frames: &crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)>,
    events: &crossbeam_channel::Receiver<MediaEvent>,
    requested: TimeCode,
) -> kinewright_core::FrameTexture {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(MediaEvent::Error(error)) = events.try_iter().find_map(|event| match event {
            MediaEvent::Error(error) => Some(MediaEvent::Error(error)),
            _ => None,
        }) {
            panic!("render failed before frame {requested}: {error}");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (at, frame) = match frames.recv_timeout(remaining) {
            Ok(value) => value,
            Err(error) => {
                if let Some(MediaEvent::Error(media_error)) =
                    events.try_iter().find_map(|event| match event {
                        MediaEvent::Error(media_error) => Some(MediaEvent::Error(media_error)),
                        _ => None,
                    })
                {
                    panic!("render failed before frame {requested}: {media_error}");
                }
                panic!("no frame {requested} arrived: {error}");
            }
        };
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
