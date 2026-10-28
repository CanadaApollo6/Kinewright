use super::*;
use crate::{cache::FrameCache, decode::VideoDecoder, frame::WorkingFrame};

fn fixture_interpretation(
    description: &kinewright_core::ColorDescription,
) -> kinewright_core::ColorDescription {
    let kinewright_core::Operation::SetAssetColorDescription {
        color_description, ..
    } = kinewright_core::assume_rec709_operation(kinewright_core::AssetId(1), description)
    else {
        unreachable!()
    };
    color_description
}

#[test]
fn s3_pf1_certified_seek_and_reopen_refuse_tampering() {
    let (root, path, source) = media_fixture("red");
    let probe = source.probe(running()).unwrap();
    let (_, chunk) = packet_chunk(&path, ffmpeg::media::Type::Video);
    let locator = root.path("external-locator-must-not-open.avi");
    let mut writer = OpenOptions::new().write(true).open(&path).unwrap();
    let gate = Arc::new(Gate::new());
    let worker_gate = Arc::clone(&gate);
    let worker_source = source.clone();
    let worker_locator = locator.clone();
    let fps = probe.fps;
    let description = fixture_interpretation(&probe.color_description);
    let worker = thread::spawn(move || {
        let mut decoder = VideoDecoder::open_certified(
            &worker_locator,
            fps,
            None,
            &description,
            None,
            &worker_source,
            None,
        )
        .unwrap();
        let mut frames = FrameCache::<WorkingFrame>::new(2);
        decoder
            .decode_window(TimeCode::ZERO, TimeCode::ZERO, &mut frames)
            .unwrap();
        assert_eq!(frames.len(), 1, "C2_PF1_DESCRIPTOR_ROUTE");
        assert!(
            !decoder.certified_chunks().contains(&chunk),
            "uncached seek chunk"
        );
        worker_gate.hold();
        let mut frames = FrameCache::<WorkingFrame>::new(2);
        let first = decoder.decode_window(TimeCode(30), TimeCode(30), &mut frames);
        let second = decoder.decode_window_sequential(TimeCode(31), TimeCode(31), &mut frames);
        (first, second, frames.len())
    });
    gate.wait();
    overwrite(
        &mut writer,
        chunk * CHUNK_BYTES as u64,
        &vec![0xff; CHUNK_BYTES],
    );
    gate.release();
    let (first, second, count) = worker.join().unwrap();
    assert!(first.is_err(), "C2_PF1_TAMPER_SEEK");
    assert!(second.is_err(), "C2_PF1_LATCHED");
    assert_eq!(count, 0, "C2_PF1_NO_PARTIAL_FRAME");
    let reopened = VideoDecoder::open_certified(
        &locator,
        probe.fps,
        None,
        &fixture_interpretation(&probe.color_description),
        None,
        &source,
        None,
    );
    assert!(reopened.is_err(), "C2_PF1_REOPEN_TAMPER");
}
#[test]
fn s3_certified_failure_quarantines_the_shared_namespace() {
    let (root, path, source) = media_fixture("red");
    let probe = source.probe(running()).unwrap();
    let mut video = VideoDecoder::open_certified(
        &root.path("no-video-locator"),
        probe.fps,
        None,
        &fixture_interpretation(&probe.color_description),
        None,
        &source,
        None,
    )
    .unwrap();
    let mut audio = crate::audio::AudioDecoder::open_with_source(
        &root.path("no-audio-locator"),
        48_000,
        2,
        0,
        48_000 * 12,
        Some(&source),
    )
    .unwrap();
    let writer = OpenOptions::new().write(true).open(&path).unwrap();
    let identity = source.identity();
    writer.set_len(identity.source_length + 1).unwrap();
    assert!(video.check_certified().is_err());
    writer.set_len(identity.source_length).unwrap();
    assert!(
        audio.check_certified().is_err(),
        "C2_SHARED_CERTIFICATE_QUARANTINE"
    );
    assert!(
        source.cursor(running()).is_err(),
        "C2_SHARED_REOPEN_REFUSED"
    );
    let fresh = CertifiedSource::verify(
        File::open(path).unwrap(),
        identity.source_sha256,
        identity.source_length,
        identity.table_sha256,
        Arc::clone(&source.0.chunks),
        Arc::new(()),
        Arc::new(|_| Ok(Arc::new(()))),
    )
    .unwrap();
    assert!(fresh.probe(running()).is_ok(), "C2_EXPLICIT_READMISSION");
}
#[test]
fn s3_pf1_latched_video_rejects_buffered_output() {
    let (root, path, source) = media_fixture("blue");
    let probe = source.probe(running()).unwrap();
    let mut decoder = VideoDecoder::open_certified(
        &root.path("no-locator"),
        probe.fps,
        None,
        &fixture_interpretation(&probe.color_description),
        None,
        &source,
        None,
    )
    .unwrap();
    decoder.buffer_certified_packet();
    let writer = OpenOptions::new().write(true).open(&path).unwrap();
    let length = writer.metadata().unwrap().len();
    writer.set_len(length + 1).unwrap();
    assert!(decoder.check_certified().is_err());
    writer.set_len(length).unwrap();
    assert!(
        decoder.receive_certified_buffered().is_err(),
        "C2_PF1_BUFFERED_VIDEO_REJECTED"
    );
}
#[test]
fn s3_pf1_latched_audio_rejects_buffered_output() {
    let (root, path, source) = media_fixture("red");
    let mut decoder = crate::audio::AudioDecoder::open_with_source(
        &root.path("no-audio-locator"),
        48_000,
        2,
        0,
        48_000 * 12,
        Some(&source),
    )
    .unwrap();
    decoder.buffer_certified_packet();
    let writer = OpenOptions::new().write(true).open(&path).unwrap();
    let length = writer.metadata().unwrap().len();
    writer.set_len(length + 1).unwrap();
    assert!(decoder.check_certified().is_err());
    writer.set_len(length).unwrap();
    assert!(
        decoder.receive_certified_buffered().is_err(),
        "C2_PF1_BUFFERED_AUDIO_REJECTED"
    );
}
#[test]
fn s3_public_source_admission_checks_the_complete_digest() {
    let (_root, path, source) = bytes_fixture(&vec![17; CHUNK_BYTES + 123]);
    let identity = source.identity();
    let result = CertifiedSource::verify(
        File::open(path).unwrap(),
        [42; 32],
        identity.source_length,
        identity.table_sha256,
        Arc::clone(&source.0.chunks),
        Arc::new(()),
        Arc::new(|_| Ok(Arc::new(()))),
    );
    assert!(result.is_err(), "C2_PUBLIC_SOURCE_FULL_DIGEST");
}
