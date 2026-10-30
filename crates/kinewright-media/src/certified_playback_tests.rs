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

fn thumbnail_inputs(path: &Path, locator: &Path, source: CertifiedSource) -> Arc<PlaybackInputs> {
    let mut asset = crate::decode::probe_path(path, kinewright_core::AssetId(1)).unwrap();
    asset.color_description = fixture_interpretation(&asset.color_description);
    asset.path = locator.to_owned();
    Arc::new(
        PlaybackInputs::admit(
            Arc::new(crate::test_support::single_clip_document(asset)),
            [(kinewright_core::AssetId(1), source)].into(),
            Arc::new(crate::LutLibrary::default()),
            Arc::new(()),
        )
        .unwrap(),
    )
}

fn assert_red_thumbnail(image: &kinewright_core::RgbaImage) {
    let offset = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
    let pixel = &image.pixels[offset..offset + 4];
    assert!(
        pixel[0] > 200 && pixel[1] < 30 && pixel[2] < 30 && pixel[3] == 255,
        "C2_THUMBNAIL_CERTIFIED_PIXELS: {pixel:?}"
    );
}

struct ThumbnailEngine(Option<Arc<crate::engine::FfmpegMediaEngine>>);

impl std::ops::Deref for ThumbnailEngine {
    type Target = Arc<crate::engine::FfmpegMediaEngine>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl Drop for ThumbnailEngine {
    fn drop(&mut self) {
        let engine = self.0.take().unwrap();
        let finished = engine.finished();
        drop(engine);
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(10)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected),
            "thumbnail engine shuts down"
        );
    }
}

fn thumbnail_engine(root: &TempDirectory) -> ThumbnailEngine {
    let gpu = crate::gpu_test_support::fixture_gpu_or_skip().expect("thumbnail witness GPU");
    ThumbnailEngine(Some(Arc::new(
        crate::engine::FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu, root.path("engine"))
            .unwrap(),
    )))
}

#[test]
fn s3_certified_live_audio_retains_the_program_namespace_without_recue() {
    use kinewright_core::{LiveAudioChange, Playback};
    let (root, path, source) = media_fixture("red");
    let inputs = thumbnail_inputs(&path, &root.path("absent-locator"), source);
    let engine = thumbnail_engine(&root);
    engine.set_certified_document(Arc::clone(&inputs));
    engine.test_applied_scene().unwrap();
    let controls = engine.controls();
    let stamp = controls.stamp();
    controls.update_audio(LiveAudioChange::Both, Arc::clone(&inputs.document));
    let applied = engine.test_applied_scene().unwrap();
    assert_eq!(controls.stamp(), stamp, "C2_OWNED_LIVE_AUDIO_NO_RECUE");
    assert_eq!(
        applied["certified_sources"].as_array().unwrap().len(),
        1,
        "C2_OWNED_LIVE_AUDIO_KEEPS_CERTIFIED_SOURCE"
    );
    assert_eq!(
        applied["certified_sources"][0]["source_sha256"],
        serde_json::to_value(
            inputs.sources[&kinewright_core::AssetId(1)]
                .identity()
                .source_sha256
        )
        .unwrap()
    );
}

#[test]
fn s3_thumbnail_document_mismatch_refuses_locator_fallback() {
    use kinewright_core::{Analysis, MediaError};

    let (root_a, path_a, source_a) = media_fixture("red");
    let (_root_b, path_b, source_b) = media_fixture("blue");
    let locator = root_a.path("external.avi");
    fs::copy(&path_a, &locator).unwrap();
    let inputs_a = thumbnail_inputs(&path_a, &locator, source_a);
    let inputs_b = thumbnail_inputs(&path_b, &path_b, source_b);
    let engine = thumbnail_engine(&root_a);
    engine.set_certified_document(Arc::clone(&inputs_a));
    engine.test_applied_scene().unwrap();
    let origin = engine.certified_media(Arc::clone(&inputs_b));
    engine.set_certified_document(inputs_b);
    engine.test_applied_scene().unwrap();
    fs::copy(&path_b, &locator).unwrap();

    let result = origin.thumbnail_for_document(Arc::clone(&inputs_a.document), TimeCode::ZERO, 32);
    assert!(
        matches!(result, Err(MediaError::NotImplemented)),
        "C2_THUMBNAIL_MISMATCH_REFUSAL: {:?}",
        result.as_ref().map(|image| &image.pixels[..4])
    );
    let image = engine
        .thumbnail_for_inputs(inputs_a, TimeCode::ZERO, 32)
        .expect("C2_THUMBNAIL_CAPABILITY_HANDOFF");
    assert_red_thumbnail(&image);
}

#[test]
fn s3_thumbnail_document_only_edit_keeps_certified_inputs() {
    use kinewright_core::Analysis;

    let (root, path, certified) = media_fixture("red");
    let (_replacement_root, replacement, _) = media_fixture("blue");
    let locator = root.path("external.avi");
    fs::copy(&path, &locator).unwrap();
    let inputs = thumbnail_inputs(&path, &locator, certified);
    let mut edited = inputs.document.as_ref().clone();
    edited.duration.0 -= 1;
    edited.tracks[0].clips[0].source_range.end = edited.duration;
    let engine = thumbnail_engine(&root);
    let origin = engine.certified_media(Arc::clone(&inputs));
    engine.set_certified_document(inputs);
    engine.test_applied_scene().unwrap();
    fs::copy(replacement, locator).unwrap();

    let image = origin
        .thumbnail_for_document(Arc::new(edited), TimeCode::ZERO, 32)
        .unwrap();
    assert_red_thumbnail(&image);
}

#[test]
fn s3_thumbnail_capability_survives_queued_scene_change() {
    use kinewright_core::Playback;

    let (root, path, certified) = media_fixture("red");
    let (_replacement_root, replacement, _) = media_fixture("blue");
    let locator = root.path("external.avi");
    fs::copy(&replacement, &locator).unwrap();
    let inputs = thumbnail_inputs(&path, &locator, certified);
    let retained = Arc::downgrade(&inputs);
    let engine = thumbnail_engine(&root);
    let hold = engine.hold_agent_lane();
    let caller_engine = Arc::clone(&engine);
    let caller =
        thread::spawn(move || caller_engine.thumbnail_for_inputs(inputs, TimeCode::ZERO, 32));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while engine.s4_lane().waiting().0 == 0 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let queued = engine.s4_lane().waiting().0 == 1;
    engine
        .unadmitted_media(Arc::new(kinewright_core::Document::default()))
        .set_document(Arc::new(kinewright_core::Document::default()));
    engine.test_applied_scene().unwrap();
    let pinned = retained.upgrade().is_some();
    drop(hold);
    let result = caller.join().unwrap();
    assert!(queued, "C2_THUMBNAIL_QUEUED");
    assert!(pinned, "C2_THUMBNAIL_LEASE_RETAINED");
    assert_red_thumbnail(&result.expect("C2_THUMBNAIL_CAPABILITY_HANDOFF"));
    drop(engine);
    assert!(retained.upgrade().is_none(), "C2_THUMBNAIL_LEASE_RELEASED");
}

#[test]
fn s3_thumbnail_damaged_capability_refuses_valid_external_locator() {
    let (root, path, certified) = media_fixture("red");
    let (_replacement_root, replacement, _) = media_fixture("blue");
    let locator = root.path("external.avi");
    fs::copy(replacement, &locator).unwrap();
    let length = certified.identity().source_length;
    let inputs = thumbnail_inputs(&path, &locator, certified);
    let writer = OpenOptions::new().write(true).open(path).unwrap();
    writer.set_len(length + 1).unwrap();
    let engine = thumbnail_engine(&root);
    assert!(
        engine
            .thumbnail_for_inputs(inputs, TimeCode::ZERO, 32)
            .is_err(),
        "C2_THUMBNAIL_DAMAGED_INPUT_REFUSAL"
    );
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
