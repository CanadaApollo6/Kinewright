//! MS2 slice 1a: IX admission end to end through the confined helper.
use crate::{
    bundle::BundleError, bundle_copy::SessionStore, owned_inputs::OwnedInputEntry,
    storage_budget::StorageLimits,
};
use kinewright_core::{AssetId, MediaAsset, MediaKind};
use kinewright_media::{
    owned_io::observe_stream_info_probes,
    test_support::{TempDirectory, run_ffmpeg, workspace_target_dir},
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

fn session(label: &str) -> (TempDirectory, SessionStore) {
    let root = TempDirectory::new_in(&workspace_target_dir(), label);
    let owner = SessionStore::create(root.root(), StorageLimits::default()).unwrap();
    (root, owner)
}
fn generated(directory: &TempDirectory, name: &str, arguments: &[&str]) -> Vec<u8> {
    let path = directory.path(name);
    run_ffmpeg(arguments, &path);
    std::fs::read(path).unwrap()
}
fn import(
    owner: &mut SessionStore,
    bytes: &[u8],
    name: &str,
) -> Result<(MediaAsset, OwnedInputEntry), BundleError> {
    let mut reader = bytes;
    owner
        .store_mut()
        .import_media(
            &mut reader,
            bytes.len() as u64,
            AssetId(7),
            Path::new(name),
            None,
        )
        .map(|(asset, entry, _pins)| (asset, entry))
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
/// `png` with one ancillary chunk inserted right after IHDR.
fn with_chunk(png: &[u8], kind: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend(data);
    chunk.extend(&body);
    chunk.extend(crc32(&body).to_be_bytes());
    let ihdr_end = 8 + 4 + 4 + 13 + 4;
    let mut out = png[..ihdr_end].to_vec();
    out.extend(chunk);
    out.extend(&png[ihdr_end..]);
    out
}
/// An sRGB-tagged PNG whose eXIf chunk carries `orientation`.
fn oriented_png(png: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x01".to_vec();
    tiff.extend([0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1]);
    tiff.extend(orientation.to_be_bytes());
    tiff.extend([0, 0, 0, 0, 0, 0]);
    with_chunk(png, *b"eXIf", &tiff)
}

/// Video, audio-video, still (with EXIF orientation) and audio admission
/// all pass through the helper with the host's interpretation checks, run
/// no in-process `FFmpeg` stream probe, and equal the retired in-process probe.
#[test]
fn ms2_admission_passes_video_still_and_audio_through_the_confined_helper() {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "ms2-admission-fixtures");
    let video = generated(
        &directory,
        "video.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=25:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ],
    );
    let audio_video = generated(
        &directory,
        "av.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
    );
    let png = generated(
        &directory,
        "still.png",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=64x36",
            "-frames:v",
            "1",
        ],
    );
    let wav = generated(
        &directory,
        "tone.wav",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:sample_rate=48000:duration=1",
        ],
    );
    // Stills need known primaries; the sRGB chunk supplies them.
    let png = with_chunk(&png, *b"sRGB", &[0]);
    let rotated = oriented_png(&png, 6);
    let cases: [Case; 5] = [
        ("video.mp4", &video, MediaKind::Video, Some((320, 180))),
        (
            "av.mkv",
            &audio_video,
            MediaKind::AudioVideo,
            Some((160, 90)),
        ),
        ("still.png", &png, MediaKind::Image, Some((64, 36))),
        ("rotated.png", &rotated, MediaKind::Image, Some((36, 64))),
        ("tone.wav", &wav, MediaKind::Audio, None),
    ];
    for case in cases {
        admit_through_the_helper(case);
    }
}
type Case<'a> = (&'a str, &'a [u8], MediaKind, Option<(u32, u32)>);
/// One admission case: confined, typed, and equal to the retired probe.
fn admit_through_the_helper((name, bytes, kind, resolution): Case) {
    let (_root, mut owner) = session("ms2-admission");
    let (result, in_process) = observe_stream_info_probes(|| import(&mut owner, bytes, name));
    let (asset, entry) = result.unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(in_process, 0, "{name}: admission probed in the host");
    assert_eq!((asset.kind, asset.resolution), (kind, resolution), "{name}");
    assert!(asset.duration.0 > 0, "{name}");
    // Re-certification without a memo runs the confined probe and the
    // host's interpretation match against the admitted entry.
    let admitted = owner.store().certify_media_entry(&entry).unwrap();
    let source = admitted.playback_source();
    let confined = super::probe(&source, Arc::new(AtomicBool::new(false))).unwrap();
    let mut retired = source.probe(Arc::new(AtomicBool::new(false))).unwrap();
    // Compare the probes after expressing absent metadata in the wire's
    // canonical form. Stills occupy one frame of the nominal 30 fps grid.
    if kind == MediaKind::Image {
        retired.duration_microseconds = 33_334;
    }
    for stream in &mut retired.streams {
        stream.duration = stream.duration.max(0);
        if stream.kind == kinewright_media::owned_io::StreamKind::Audio
            || stream.average_frame_rate.0 == 0
        {
            stream.average_frame_rate = (0, 1);
        }
    }
    assert_eq!(
        confined, retired,
        "{name}: confined probe differs from host probe"
    );
    assert!(entry.matches_media_asset(&asset), "{name}");
}

/// Malformed inputs are refused by the helper with the typed refusal, and
/// nothing is admitted.
#[test]
fn ms2_admission_refuses_malformed_inputs_in_the_helper() {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "ms2-malformed-fixtures");
    let video = generated(
        &directory,
        "video.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=25:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let noise: Vec<u8> = (0..70_000_u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13).to_le_bytes()[0])
        .collect();
    let mut truncated_png = generated(
        &directory,
        "still.png",
        &["-f", "lavfi", "-i", "testsrc2=size=64x36", "-frames:v", "1"],
    );
    truncated_png.truncate(40);
    for (name, bytes) in [
        ("noise.bin", noise.as_slice()),
        ("truncated.mp4", &video[..video.len() / 3]),
        ("truncated.png", truncated_png.as_slice()),
    ] {
        let (_root, mut owner) = session("ms2-malformed");
        let (result, in_process) = observe_stream_info_probes(|| import(&mut owner, bytes, name));
        assert_eq!(in_process, 0, "{name}: refusal probed in the host");
        match result {
            Err(BundleError::Corrupt(message)) => assert_eq!(
                message, "media sandbox: UnsupportedMedia",
                "{name}: wrong refusal"
            ),
            other => panic!("{name}: expected the helper's typed refusal, got {other:?}"),
        }
    }
}

/// With no configured runtime, admission fails closed; it never parses in
/// the host.
#[test]
fn ms2_unconfigured_admission_is_sandbox_unavailable() {
    let (_root, mut owner) = session("ms2-unconfigured");
    let wav = kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1);
    let (_, entry) = import(&mut owner, &wav, "tone.wav").unwrap();
    let source = owner
        .store()
        .certify_media_entry(&entry)
        .unwrap()
        .playback_source();
    let (result, in_process) = observe_stream_info_probes(|| {
        super::probe_with(None, &source, Arc::new(AtomicBool::new(false)))
    });
    assert_eq!(in_process, 0);
    assert!(
        matches!(&result, Err(BundleError::Unsupported(message)) if message.starts_with("media sandbox unavailable")),
        "{result:?}"
    );
}
