//! C0 production input identity, graph retention and refusal witnesses.
#![cfg(all(feature = "test-util", target_os = "linux"))]
#![allow(
    clippy::too_many_lines,
    reason = "fault controls and independent identity oracles stay together"
)]
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::LoadedBundle,
    bundle_copy::{CopyStatus, SaveAsOutcome},
    bundle_records::{ObjectRef, RecordKind},
    c0_fixtures::build_c0_fixture_pair,
    editing_session::Ja1EditingSession,
    io_observer,
    owned_inputs::{InputChunkTable, OwnedInputRef},
    storage_budget::StorageLimits,
};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[test]
fn s3_owned_import_refuses_dynamic_mpegps_before_probe() {
    use kinewright_core::{Core, Document};
    use kinewright_media::{owned_io::observe_stream_info_probes, test_support::run_ffmpeg};
    use kinewright_project::bundle::{BundleError, BundleWriteAdmission};
    let temp = temp("c0-mpegps-import-refusal");
    let wav = temp.path("control.wav");
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=1",
            "-c:a",
            "pcm_s16le",
            "-threads",
            "1",
        ],
        &wav,
    );
    let mpeg = temp.path("dynamic.mpg");
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=25:duration=1",
            "-c:v",
            "mpeg2video",
            "-g",
            "1",
            "-threads",
            "1",
            "-f",
            "mpeg",
        ],
        &mpeg,
    );
    let core = Core::spawn(Document::default()).unwrap();
    let root = temp.path("refusal.kinewright");
    let mut edit =
        Ja1EditingSession::activate(&root, BundleWriteAdmission::new_document(), &core, None)
            .unwrap();
    let (control, probes) = observe_stream_info_probes(|| edit.import_owned_media(&wav));
    control.expect("supported production WAV import control");
    assert!(probes > 0, "observer sees the successful production probe");
    let history = core.export_history().unwrap();
    let inputs = edit.current_inputs_ref().clone();
    let root_bytes = std::fs::read(&root).unwrap();
    let (result, probes) = observe_stream_info_probes(|| edit.import_owned_media(&mpeg));
    assert_eq!(probes, 0, "C0_PRODUCTION_INCOMPLETE_PROBE_NOT_ENTERED");
    assert!(
        matches!(result, Err(BundleError::Corrupt(ref reason))
            if reason == "certified decoder: incomplete or dynamic stream table unsupported"),
        "C0_PRODUCTION_MPEGPS_GUARD_REFUSAL: {result:?}",
    );
    assert_eq!(
        core.export_history().unwrap(),
        history,
        "MPEG-PS refusal preserves the document"
    );
    assert_eq!(
        edit.current_inputs_ref(),
        &inputs,
        "MPEG-PS refusal preserves the manifest"
    );
    assert_eq!(
        std::fs::read(&root).unwrap(),
        root_bytes,
        "MPEG-PS refusal preserves the durable root"
    );
    assert_eq!(
        edit.store().unwrap().usage().unwrap().reserved_bytes,
        0,
        "MPEG-PS refusal releases staging reservations"
    );
}

#[test]
fn s3_owned_input_identity() {
    let temp = temp("c0-two-snapshots");
    let pair = build_c0_fixture_pair(temp.root()).unwrap();
    assert_ne!(pair[0].media.entry.object, pair[1].media.entry.object);
    assert_ne!(pair[0].lut_ref, pair[1].lut_ref);
    for fixture in &pair {
        println!(
            "C0_FIXTURE {}",
            serde_json::json!({
                "source": fixture.media.entry.object, "manifest": fixture.media.manifest_ref,
                "table": fixture.table_ref, "lut": fixture.lut_ref,
                "history_sha256": kinewright_media::sha256_bytes(&serde_json::to_vec(&fixture.history).unwrap()),
                "video_luma": fixture.expected_video_plane0, "audio_hz": fixture.audio_frequency
            })
        );
        let loaded = LoadedBundle::read(&fixture.path).unwrap();
        let mut owner = loaded.into_writer().unwrap();
        let manifest = owner
            .store()
            .read_input_manifest(&fixture.media.manifest_ref)
            .unwrap();
        assert!(manifest.entries.contains(&fixture.media.entry));
        let lut_entry = manifest
            .entries
            .iter()
            .find(|entry| entry.object == fixture.lut_ref)
            .unwrap()
            .clone();
        let resource = owner
            .store()
            .certify_resource_input(&OwnedInputRef {
                manifest_ref: fixture.media.manifest_ref.clone(),
                entry: lut_entry,
            })
            .unwrap();
        assert_eq!(
            kinewright_media::sha256_bytes(resource.bytes()),
            fixture.lut_ref.sha256
        );
        drop(resource);

        let table = owner
            .store()
            .read_record::<InputChunkTable>(&fixture.table_ref)
            .unwrap();
        table
            .validate_binding(&fixture.media.entry.object, owner.store().limits())
            .unwrap();
        let source = owner.store().certify_media_input(&fixture.media).unwrap();
        let probe = source.probe(cancel()).unwrap();
        assert_eq!(probe.video_stream, Some(0));
        assert_eq!(probe.audio_stream, Some(1));
        let mut video = source.open_video(cancel()).unwrap();
        let frame = video.next_frame().unwrap().unwrap();
        assert_eq!(frame.data(0)[0], fixture.expected_video_plane0);
        video.seek(0).unwrap();
        assert!(video.next_frame().unwrap().is_some());
        let mut audio = source.open_audio(cancel()).unwrap();
        let audio_frame = audio.next_frame().unwrap().unwrap();
        let samples = audio_frame
            .data(0)
            .as_chunks::<2>()
            .0
            .iter()
            .take(2048)
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect::<Vec<_>>();
        let crossings = samples.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count();
        let expected = fixture.audio_frequency as usize * samples.len() / 48000;
        assert!(
            crossings.abs_diff(expected) <= 1,
            "independent audio landmark {crossings} versus {expected}"
        );
        audio.seek(0).unwrap();
        assert!(audio.next_frame().unwrap().is_some());
        let mut lease = owner.store().lease_input(&fixture.media).unwrap();
        lease.certify_decoder_open().unwrap();
        let mut bytes = vec![0; usize::try_from(fixture.media.entry.object.byte_length).unwrap()];
        lease.read_exact(&mut bytes).unwrap();
        assert_eq!(
            kinewright_media::sha256_bytes(&bytes),
            fixture.media.entry.object.sha256
        );
        assert_eq!(lease.read(&mut [0; 1]).unwrap(), 0);
        lease.seek(SeekFrom::Start(0)).unwrap();
        let source_again = owner.store().certify_media_input(&fixture.media).unwrap();
        assert!(
            source_again
                .open_video(cancel())
                .unwrap()
                .next_frame()
                .unwrap()
                .is_some()
        );
        drop(lease);
        drop(audio);
        drop(video);
        drop(source);
        drop(source_again);
        drop(table);
        drop(manifest);
        let copy_path = temp.path(&format!("copy-{}.kinewright", fixture.audio_frequency));
        let SaveAsOutcome::Copied(copy) = owner.save_as(&copy_path).unwrap() else {
            panic!("new copy")
        };
        let rebound = copy.rebound_input(&fixture.media).unwrap();
        assert_eq!(rebound.entry.chunk_table_ref().unwrap(), &fixture.table_ref);
        assert!(LoadedBundle::read(&copy_path).is_ok());
        let Ok(destination) = copy.into_destination() else {
            panic!("copy committed");
        };
        let held = destination
            .store()
            .closure(std::slice::from_ref(&rebound.manifest_ref))
            .unwrap();
        assert!(
            held.contains(&fixture.table_ref),
            "C0_MANIFEST_TABLE_EDGE_PRESERVED"
        );
        assert!(held.contains(&fixture.lut_ref));
    }
}

#[test]
fn s3_table_fingerprint_binding_refuses_a_valid_hashed_conflict() {
    let temp = temp("c0-table-conflict");
    let [fixture, _] = build_c0_fixture_pair(temp.root()).unwrap();
    let mut owner = LoadedBundle::read(&fixture.path)
        .unwrap()
        .into_writer()
        .unwrap();
    let original = owner
        .store()
        .read_record::<InputChunkTable>(&fixture.table_ref)
        .unwrap();
    let mut conflicting = (*original).clone();
    conflicting.source_sha256 = "0".repeat(64);
    drop(original);
    let bytes = serde_json::to_vec(&conflicting).unwrap();
    let table = ObjectRef {
        sha256: kinewright_media::sha256_bytes(&bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::InputChunkTable,
    };
    let admitted = owner
        .store_mut()
        .unwrap()
        .admit_chunk_table(&conflicting)
        .unwrap();
    assert_eq!(admitted.reference, table);
    let before = std::fs::read(&fixture.path).unwrap();
    let mut manifest = (*owner
        .store()
        .read_input_manifest(&fixture.media.manifest_ref)
        .unwrap())
    .clone();
    manifest
        .entries
        .iter_mut()
        .find(|entry| entry.object.record_kind == RecordKind::OwnedMedia)
        .unwrap()
        .dependencies = vec![table];
    assert!(owner.admit_inputs(&manifest).is_err());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn s3_unsaved_frozen_import_and_first_save_retains_table() {
    let temp = temp("c0-unsaved-import");
    let [fixture, _] = build_c0_fixture_pair(temp.root()).unwrap();
    let source_path = temp.path("external.mkv");
    std::fs::copy(
        temp.path(&format!(
            "c0-red.kinewright-assets/studio/objects/{}",
            fixture.media.entry.object.sha256
        )),
        &source_path,
    )
    .unwrap();
    let local_core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
    let mut local =
        Ja1EditingSession::new(temp.root(), &local_core, StorageLimits::default()).unwrap();
    local.import_owned_media(&source_path).unwrap();
    let first = temp.path("first-save.kinewright");
    assert!(matches!(
        local.first_save(&first, StorageLimits::default()).unwrap(),
        CopyStatus::Committed(_)
    ));
    assert!(LoadedBundle::read(&first).is_ok());
    let first_inputs = local
        .store()
        .unwrap()
        .read_input_manifest(local.current_inputs_ref())
        .unwrap();
    assert!(
        first_inputs
            .entries
            .iter()
            .any(|entry| entry.chunk_table_ref().is_ok())
    );
    drop(first_inputs);
    drop(local);
    let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let launch = session.prepare_host_attachment().unwrap();
    assert!(launch.source.is_some());
    let (asset, report) = session.import_owned_media(&source_path).unwrap();
    assert!(report.is_none());
    assert_eq!(
        session.core().export_history().unwrap().document.media_pool,
        vec![asset]
    );
    let refreshed = session.refresh_unsaved_host_source().unwrap();
    assert_eq!(&refreshed.inputs, session.current_inputs_ref());
    assert!(
        refreshed
            .source
            .unwrap()
            .objects()
            .any(|object| object.record_kind == RecordKind::InputChunkTable)
    );
}

#[test]
fn s3_rescue_preserves_owned_table_and_lut() {
    let temp = temp("c0-rescue-graph");
    let drive = temp.path("drive");
    std::fs::create_dir(&drive).unwrap();
    let [fixture, _] = build_c0_fixture_pair(&drive).unwrap();
    let mut session = Ja1EditingSession::open(&fixture.path).unwrap();
    assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
    std::fs::rename(&drive, temp.path("hidden-drive")).unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let selected = session
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selected.consent();
    let path = temp.path("rescued.kinewright");
    let destination = session.inspect_rescue_destination(&path).unwrap();
    let outcome = session.rescue(selected, &consent, &destination).unwrap();
    assert!(outcome.activated);
    assert!(matches!(outcome.status, CopyStatus::Committed(_)));
    let loaded = LoadedBundle::read(&path).unwrap();
    let root = loaded.root().unwrap().clone();
    drop(loaded);
    let inputs = session
        .store()
        .unwrap()
        .read_input_manifest(&root.input_manifest_ref)
        .unwrap();
    let entry = inputs
        .entries
        .iter()
        .find(|entry| entry.object == fixture.media.entry.object)
        .unwrap();
    assert_eq!(entry.chunk_table_ref().unwrap(), &fixture.table_ref);
    assert!(
        session
            .store()
            .unwrap()
            .closure(std::slice::from_ref(&root.input_manifest_ref))
            .unwrap()
            .contains(&fixture.lut_ref)
    );
    let source = session
        .store()
        .unwrap()
        .certify_media_input(&OwnedInputRef {
            manifest_ref: root.input_manifest_ref,
            entry: entry.clone(),
        })
        .unwrap();
    assert!(
        source
            .open_video(cancel())
            .unwrap()
            .next_frame()
            .unwrap()
            .is_some()
    );
}

#[test]
fn s3_certified_input_through_use() {
    use kinewright_core::{ColorContext, ColorProvenance, Core, Document};
    use kinewright_media::test_support::{GeneratedMedia, ffprobe_executable};
    use kinewright_project::bundle::BundleWriteAdmission;
    use std::{io::Write, process::Command, sync::Barrier};
    let temp = temp("c0-production-uncached-seek");
    let input = GeneratedMedia::ffmpeg(
        "c0-uncached-av",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:r=5:d=12",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=12",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
        "avi",
    );
    assert!(input.path().metadata().unwrap().len() > 4 * 1_048_576);
    let core = Core::spawn(Document::default()).unwrap();
    let path = temp.path("long.kinewright");
    let mut session =
        Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &core, None)
            .unwrap();
    let mut color = ColorContext::sdr_rec709().delivery;
    color.provenance = ColorProvenance::UserOverride;
    let (asset, report) = session
        .import_owned_media_with_color(input.path(), Some(color))
        .unwrap();
    assert!(report.is_some());
    let manifest = session
        .store()
        .unwrap()
        .read_input_manifest(session.current_inputs_ref())
        .unwrap();
    let entry = manifest
        .entries
        .iter()
        .find(|entry| entry.matches_media_asset(&asset))
        .unwrap()
        .clone();
    let reference = OwnedInputRef {
        manifest_ref: session.current_inputs_ref().clone(),
        entry,
    };
    let owned_path = temp.path(&format!(
        "long.kinewright-assets/studio/objects/{}",
        reference.entry.object.sha256
    ));
    let mut preexisting_writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(owned_path)
        .unwrap();
    let source = session
        .store()
        .unwrap()
        .certify_media_input(&reference)
        .unwrap();
    let mut video = source.open_video(cancel()).unwrap();
    let mut audio = source.open_audio(cancel()).unwrap();
    assert_eq!(video.next_frame().unwrap().unwrap().data(0)[0], 81);
    assert!(audio.next_frame().unwrap().is_some());
    let video_cached = video.cached_chunk_indices();
    let audio_cached = audio.cached_chunk_indices();
    let oracle = Command::new(ffprobe_executable())
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_entries",
            "packet=stream_index,pts_time,pos,size",
            "-of",
            "json",
        ])
        .arg(input.path())
        .output()
        .unwrap();
    assert!(oracle.status.success());
    let packets: serde_json::Value = serde_json::from_slice(&oracle.stdout).unwrap();
    let target = |index: u32| {
        packets["packets"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|packet| {
                if packet["stream_index"].as_u64()? != u64::from(index)
                    || packet["pts_time"].as_str()?.parse::<f64>().ok()? < 8.0
                {
                    return None;
                }
                let position = packet["pos"].as_str()?.parse::<u64>().ok()?;
                let size = packet["size"].as_str()?.parse::<u64>().ok()?;
                let byte = position + size.min(4096) / 2;
                let chunk = byte / 1_048_576;
                if video_cached.contains(&Some(chunk)) || audio_cached.contains(&Some(chunk)) {
                    return None;
                }
                Some((byte, chunk))
            })
            .expect("independent oracle found an uncached payload packet")
    };
    let video_target = target(source.interpretation().video.as_ref().unwrap().index);
    let audio_target = target(source.interpretation().audio.as_ref().unwrap().index);
    assert!(!video_cached.contains(&Some(video_target.1)));
    assert!(!audio_cached.contains(&Some(audio_target.1)));
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        worker_barrier.wait();
        for (offset, _) in [video_target, audio_target] {
            preexisting_writer.seek(SeekFrom::Start(offset)).unwrap();
            let mut byte = [0];
            preexisting_writer.read_exact(&mut byte).unwrap();
            preexisting_writer.seek(SeekFrom::Start(offset)).unwrap();
            preexisting_writer.write_all(&[byte[0] ^ 0xff]).unwrap();
        }
        preexisting_writer.flush().unwrap();
        worker_barrier.wait();
    });
    barrier.wait();
    barrier.wait();
    writer.join().unwrap();
    assert!(
        video
            .seek(8_000_000)
            .and_then(|()| video.next_frame().map(|_| ()))
            .is_err(),
        "no accepted video output after uncached payload tamper"
    );
    assert!(
        audio
            .seek(8_000_000)
            .and_then(|()| audio.next_frame().map(|_| ()))
            .is_err(),
        "no accepted audio output after uncached payload tamper"
    );
    assert!(video.next_frame().is_err());
    assert!(audio.next_frame().is_err());
    assert!(
        !input.path().exists()
            || input.path().metadata().unwrap().len() == reference.entry.object.byte_length
    );
}

#[test]
fn s3_duplicate_publication_full_digest_refuses_late_corruption_with_warm_memo() {
    use kinewright_core::{ColorContext, ColorProvenance, Core, Document};
    use kinewright_media::test_support::GeneratedMedia;
    use kinewright_project::bundle::{BundleError, BundleWriteAdmission};
    use std::io::Write;

    let temp = temp("c0-duplicate-full-digest");
    let input = GeneratedMedia::ffmpeg(
        "c0-duplicate-long",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:r=5:d=12",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
        ],
        "avi",
    );
    let core = Core::spawn(Document::default()).unwrap();
    let mut edit = Ja1EditingSession::activate(
        &temp.path("duplicate.kinewright"),
        BundleWriteAdmission::new_document(),
        &core,
        None,
    )
    .unwrap();
    let mut color = ColorContext::sdr_rec709().delivery;
    color.provenance = ColorProvenance::UserOverride;
    let (original, _) = edit
        .import_owned_media_with_color(input.path(), Some(color.clone()))
        .unwrap();
    let (duplicate, _) = edit
        .import_owned_media_with_color(input.path(), Some(color.clone()))
        .unwrap();
    assert_eq!(
        original.source_fingerprint, duplicate.source_fingerprint,
        "C0_DUPLICATE_POSITIVE_IDENTITY"
    );
    let inputs = edit.current_inputs_ref().clone();
    let manifest = edit.store().unwrap().read_input_manifest(&inputs).unwrap();
    let entry = manifest
        .entries
        .iter()
        .find(|entry| entry.matches_media_asset(&original))
        .unwrap()
        .clone();
    let reference = OwnedInputRef {
        manifest_ref: inputs.clone(),
        entry,
    };
    let certificate = edit
        .store()
        .unwrap()
        .certify_media_input(&reference)
        .unwrap();
    let mut video = certificate.open_video(cancel()).unwrap();
    assert!(video.next_frame().unwrap().is_some());
    let cached = video.cached_chunk_indices();
    let chunk_count = reference.entry.object.byte_length.div_ceil(1_048_576);
    assert!(chunk_count > 4);
    let late_chunk = (4..chunk_count)
        .find(|index| !cached.contains(&Some(*index)))
        .unwrap();
    let offset = late_chunk * 1_048_576 + 17;
    assert!(
        offset < reference.entry.object.byte_length,
        "late uncached payload exists"
    );
    let owned = temp.path(&format!(
        "duplicate.kinewright-assets/studio/objects/{}",
        reference.entry.object.sha256
    ));
    let mut writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&owned)
        .unwrap();
    writer.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0];
    writer.read_exact(&mut byte).unwrap();
    writer.seek(SeekFrom::Start(offset)).unwrap();
    writer.write_all(&[byte[0] ^ 0xff]).unwrap();
    writer.flush().unwrap();
    drop(video);
    drop(certificate);
    let history = core.export_history().unwrap();
    let root_before = std::fs::read(temp.path("duplicate.kinewright")).unwrap();
    let Err(error) = edit.import_owned_media_with_color(input.path(), Some(color)) else {
        panic!("C0_DUPLICATE_FULL_DIGEST_REFUSED: corrupted duplicate was admitted");
    };
    assert!(
        matches!(error, BundleError::Corrupt(ref reason) if reason == "existing source object digest conflict"),
        "C0_DUPLICATE_FULL_DIGEST_REFUSED: {error:?}"
    );
    assert_eq!(
        core.export_history().unwrap(),
        history,
        "C0_DUPLICATE_DOCUMENT_PRESERVED"
    );
    assert_eq!(
        edit.current_inputs_ref(),
        &inputs,
        "C0_DUPLICATE_MANIFEST_PRESERVED"
    );
    assert_eq!(
        std::fs::read(temp.path("duplicate.kinewright")).unwrap(),
        root_before,
        "C0_DUPLICATE_DURABLE_ROOT_PRESERVED"
    );
    assert_eq!(
        edit.store().unwrap().usage().unwrap().reserved_bytes,
        0,
        "C0_DUPLICATE_NO_STAGING_LEAK"
    );
}

#[test]
fn s3_default_budget_hd_import_probe_decode_and_reopen() {
    use kinewright_core::{ColorContext, ColorProvenance, Core, Document};
    use kinewright_media::test_support::GeneratedMedia;
    use kinewright_project::bundle::BundleWriteAdmission;

    let temp = temp("c0-default-hd");
    for (width, height) in [(1280, 720), (1920, 1080)] {
        let input = GeneratedMedia::ffmpeg(
            "c0-default-hd",
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("color=c=red:s={width}x{height}:r=4:d=0.5"),
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.5",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-threads",
                "1",
                "-c:a",
                "pcm_s16le",
                "-shortest",
            ],
            "mkv",
        );
        let path = temp.path(&format!("hd-{height}.kinewright"));
        let core = Core::spawn(Document::default()).unwrap();
        let mut edit =
            Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &core, None)
                .unwrap();
        let mut color = ColorContext::sdr_rec709().delivery;
        color.provenance = ColorProvenance::UserOverride;
        let (asset, report) = edit
            .import_owned_media_with_color(input.path(), Some(color))
            .unwrap();
        assert!(report.is_some(), "C0_HD_PRODUCTION_IMPORT_SAVED");
        assert_eq!(asset.resolution, Some((width, height)));
        let manifest = edit
            .store()
            .unwrap()
            .read_input_manifest(edit.current_inputs_ref())
            .unwrap();
        let reference = OwnedInputRef {
            manifest_ref: edit.current_inputs_ref().clone(),
            entry: manifest
                .entries
                .iter()
                .find(|entry| entry.matches_media_asset(&asset))
                .unwrap()
                .clone(),
        };
        let source = edit
            .store()
            .unwrap()
            .certify_media_input(&reference)
            .unwrap();
        assert_eq!(
            source.probe(cancel()).unwrap().resolution,
            Some((width, height)),
            "C0_HD_PRODUCTION_PROBE"
        );
        let mut video = source.open_video(cancel()).unwrap();
        let mut audio = source.open_audio(cancel()).unwrap();
        assert!(video.next_frame().unwrap().is_some(), "C0_HD_VIDEO_DECODE");
        assert!(audio.next_frame().unwrap().is_some(), "C0_HD_AUDIO_DECODE");
        let usage = edit.store().unwrap().usage().unwrap();
        assert!(
            usage.decoder_bytes > StorageLimits::default().metadata_bytes,
            "C0_HD_DECODER_SEPARATE_FROM_METADATA"
        );
        assert!(
            usage.decoder_bytes <= StorageLimits::default().decoder_bytes,
            "C0_HD_SIMULTANEOUS_AV_BOUNDED"
        );
        assert!(usage.decoded_bytes >= usage.decoder_bytes);
        drop(video);
        drop(audio);
        let mut reopened = source.open_video(cancel()).unwrap();
        assert!(
            reopened.next_frame().unwrap().is_some(),
            "C0_HD_DECODER_REOPEN"
        );
        drop(reopened);
        drop(source);
        assert_eq!(
            edit.store().unwrap().usage().unwrap().decoder_bytes,
            0,
            "C0_HD_DECODER_RELEASED"
        );
        drop(manifest);
        drop(edit);
        let owner = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
        let reopened = owner.store().certify_media_input(&reference).unwrap();
        assert!(
            reopened
                .open_video(cancel())
                .unwrap()
                .next_frame()
                .unwrap()
                .is_some(),
            "C0_HD_DURABLE_REOPEN"
        );
    }
}

#[test]
fn s3_colour_override_save_undo_save_and_relink_preserve_exact_source() {
    use kinewright_core::{ColorProvenance, Command, Core, Event, Operation, RelinkCandidate};
    let temp = temp("c0-colour-undo");
    let [fixture, _] = build_c0_fixture_pair(temp.root()).unwrap();
    let loaded = LoadedBundle::read(&fixture.path).unwrap();
    let core = Core::spawn((*fixture.history.baseline).clone()).unwrap();
    loaded.restore_manual(&core).unwrap();
    let bundle = loaded.into_writer().unwrap();
    let mut edit =
        Ja1EditingSession::from_bundle(bundle, &core, fixture.media.manifest_ref.clone()).unwrap();
    let original = core.export_history().unwrap().document.media_pool[0].clone();
    let mut changed = original.clone();
    changed.color_description.provenance = ColorProvenance::UserOverride;
    changed.color_description.confidence_basis_points = 9999;
    edit.apply_owned_media_interpretation(&changed).unwrap();
    edit.save_active().expect("C0_OVERRIDE_SAVE_SUCCEEDS");
    let admitted = edit.current_inputs_ref().clone();
    let manifest = edit
        .store()
        .unwrap()
        .read_input_manifest(&admitted)
        .unwrap();
    let original_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.matches_media_asset(&original))
        .unwrap();
    let changed_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.matches_media_asset(&changed))
        .unwrap();
    assert_ne!(
        original_entry.entry_id, changed_entry.entry_id,
        "C0_OVERRIDE_ADDS_INTERPRETATION"
    );
    assert_eq!(
        original_entry.object, changed_entry.object,
        "C0_OVERRIDE_SOURCE_UNCHANGED"
    );
    assert_eq!(
        original_entry.dependencies, changed_entry.dependencies,
        "C0_OVERRIDE_TABLE_UNCHANGED"
    );
    assert_eq!(
        original_entry.media_interpretation().unwrap().probed_color,
        changed_entry.media_interpretation().unwrap().probed_color,
        "C0_OVERRIDE_PROBED_COLOUR_RETAINED"
    );
    assert!(matches!(
        core.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    edit.save_active().expect("C0_OVERRIDE_UNDO_SAVE_SUCCEEDS");
    assert_eq!(
        core.export_history().unwrap().document.media_pool[0],
        original,
        "C0_OVERRIDE_UNDO_RESTORES_ORIGINAL"
    );
    let candidate = RelinkCandidate {
        path: temp.path("display-only-new-location.mkv"),
        fingerprint: original.source_fingerprint.clone(),
        kind: original.kind,
        fps: original.fps,
        duration: original.duration,
        resolution: original.resolution,
    };
    assert!(matches!(
        core.request(Command::Do(Operation::RelinkAsset {
            asset: original.id,
            candidate,
            allow_unverified_source: false,
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    edit.save_active()
        .expect("C0_SAME_FINGERPRINT_RELINK_SAVES");
    let mut relabelled = core.export_history().unwrap().document.media_pool[0].clone();
    relabelled.name = "new display name".into();
    let neutral = edit.prepare_media_interpretation(&relabelled).unwrap();
    assert_eq!(
        neutral.inputs_ref, admitted,
        "C0_LOCATOR_AND_NAME_IDENTITY_NEUTRAL"
    );
    drop(neutral);
    let before = core.export_history().unwrap();
    let inputs = edit.current_inputs_ref().clone();
    let mut wrong = relabelled;
    wrong.source_fingerprint.content_sha256 = Some("0".repeat(64));
    assert!(
        edit.prepare_media_interpretation(&wrong).is_err(),
        "C0_INTERPRETATION_DIFFERENT_FINGERPRINT_REFUSED"
    );
    let candidate = RelinkCandidate {
        path: wrong.path.clone(),
        fingerprint: wrong.source_fingerprint.clone(),
        kind: wrong.kind,
        fps: wrong.fps,
        duration: wrong.duration,
        resolution: wrong.resolution,
    };
    assert!(
        !matches!(
            core.request(Command::Do(Operation::RelinkAsset {
                asset: wrong.id,
                candidate,
                allow_unverified_source: true,
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ),
        "C0_RELINK_DIFFERENT_FINGERPRINT_REFUSED"
    );
    assert_eq!(
        core.export_history().unwrap(),
        before,
        "C0_RELINK_DOCUMENT_PRESERVED"
    );
    assert_eq!(
        edit.current_inputs_ref(),
        &inputs,
        "C0_RELINK_MANIFEST_PRESERVED"
    );
}

#[test]
fn s3_committed_import_retains_inputs_when_following_save_fails() {
    use kinewright_core::{Core, Document};
    use kinewright_media::test_support::GeneratedMedia;
    use kinewright_project::bundle::BundleWriteAdmission;
    let temp = temp("c0-import-save-failure");
    let input = GeneratedMedia::ffmpeg(
        "c0-import-save-failure",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.2",
            "-c:a",
            "pcm_s16le",
        ],
        "wav",
    );
    let core = Core::spawn(Document::default()).unwrap();
    let mut edit = Ja1EditingSession::activate(
        &temp.path("failure.kinewright"),
        BundleWriteAdmission::new_document(),
        &core,
        None,
    )
    .unwrap();
    let previous = edit.current_inputs_ref().clone();
    let failed = io_observer::fail_nth("root_before_publish", 3, || {
        edit.import_owned_media(input.path())
    });
    assert!(failed.is_err(), "C0_IMPORT_FOLLOWING_SAVE_FAULT_REACHED");
    let history = core.export_history().unwrap();
    assert_eq!(
        history.document.media_pool.len(),
        1,
        "C0_IMPORT_COMMAND_ALREADY_COMMITTED"
    );
    assert_ne!(
        edit.current_inputs_ref(),
        &previous,
        "C0_IMPORT_COMMITTED_INPUTS_RETAINED"
    );
    let manifest = edit
        .store()
        .unwrap()
        .read_input_manifest(edit.current_inputs_ref())
        .unwrap();
    let asset = &history.document.media_pool[0];
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.matches_media_asset(asset)),
        "C0_IMPORT_COMMITTED_MANIFEST_BOUND"
    );
    drop(manifest);
    edit.reconcile_active().unwrap();
    edit.save_active()
        .expect("C0_IMPORT_RECOVERY_SAVE_SUCCEEDS");
    assert_eq!(
        core.export_history().unwrap(),
        history,
        "C0_IMPORT_RECOVERY_DOES_NOT_DUPLICATE"
    );
}
