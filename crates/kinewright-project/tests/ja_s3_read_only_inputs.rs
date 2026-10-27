//! Portable native-handle admission, independent of durable namespace writes.
#![cfg(any(unix, windows))]
#![allow(
    clippy::too_many_lines,
    reason = "positive controls and independent byte/identity oracles stay together"
)]
use kinewright_core::{
    AssetId, ColorDescription, Document, MediaAsset, MediaKind, MediaSourceFingerprint, Rational,
    TimeCode,
};
use kinewright_hash::Sha256;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    ReadOnlyInputBinding, ReadOnlyInputFiles, ReadOnlyInputs,
    bundle_records::{ObjectRef, RecordKind, SessionId},
    owned_inputs::{
        INPUT_CHUNK_BYTES, InputChunkTable, MediaInterpretation, OwnedInputEntry,
        OwnedInputManifest, OwnedInputRef, StreamInterpretation,
    },
    storage_budget::StorageLimits,
};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn reference(bytes: &[u8], record_kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind,
    }
}

fn record(path: &Path, value: &impl Serialize, kind: RecordKind) -> ObjectRef {
    let bytes = serde_json::to_vec(value).unwrap();
    std::fs::write(path, &bytes).unwrap();
    reference(&bytes, kind)
}

struct Fixture {
    root: TempDirectory,
    binding: ReadOnlyInputBinding,
    document: Document,
    manifest: OwnedInputManifest,
    table: InputChunkTable,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c0-read-only");
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend(9636_u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(48000_u32.to_le_bytes());
        bytes.extend(96000_u32.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(9600_u32.to_le_bytes());
        for _ in 0..4800 {
            bytes.extend(1234_i16.to_le_bytes());
        }
        let source = reference(&bytes, RecordKind::OwnedMedia);
        std::fs::write(root.path("media"), &bytes).unwrap();
        let mut hash = Sha256::new();
        hash.update(&bytes);
        let table = InputChunkTable {
            version: 1,
            algorithm: "sha256".into(),
            chunk_size: INPUT_CHUNK_BYTES,
            source_sha256: source.sha256.clone(),
            source_length: source.byte_length,
            chunk_sha256: vec![hash.finalize()],
        };
        let table_ref = record(&root.path("table"), &table, RecordKind::InputChunkTable);
        let interpretation = MediaInterpretation {
            version: 1,
            video: None,
            audio: Some(StreamInterpretation {
                index: 0,
                time_base: (1, 48000),
                timestamp_origin: 0,
            }),
            fps: Rational::default(),
            duration: TimeCode(3),
            kind: MediaKind::Audio,
            resolution: None,
            effective_color: ColorDescription::unknown(),
            probed_color: ColorDescription::unknown(),
            source_profile_assumption: None,
        };
        let profile = interpretation.canonical().unwrap();
        let entry = OwnedInputEntry {
            entry_id: format!(
                "media:1:{}:{}:{}",
                source.sha256,
                source.byte_length,
                kinewright_media::sha256_bytes(profile.as_bytes())
            ),
            object: source.clone(),
            dependencies: vec![table_ref],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: Some(profile),
        };
        let session_id = Some(SessionId::fresh().unwrap());
        let manifest = OwnedInputManifest {
            project_id: None,
            session_id: session_id.clone(),
            owner_epoch: 1,
            entries: vec![entry.clone()],
        };
        let manifest_ref = record(&root.path("manifest"), &manifest, RecordKind::InputManifest);
        let document = Document {
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: root.path("this-locator-does-not-exist.wav"),
                name: "provenance only".into(),
                duration: interpretation.duration,
                fps: interpretation.fps,
                kind: interpretation.kind,
                resolution: None,
                source_fingerprint: MediaSourceFingerprint {
                    content_sha256: Some(source.sha256),
                    byte_len: Some(source.byte_length),
                },
                color_description: interpretation.effective_color,
                assumed_from: None,
            }],
            ..Document::default()
        };
        let document_ref = record(&root.path("document"), &document, RecordKind::Document);
        Self {
            root,
            binding: ReadOnlyInputBinding {
                document_ref,
                input: OwnedInputRef {
                    manifest_ref,
                    entry,
                },
                project_id: None,
                session_id,
                owner_epoch: 1,
            },
            document,
            manifest,
            table,
            bytes,
        }
    }

    fn files(&self) -> ReadOnlyInputFiles {
        ReadOnlyInputFiles {
            document: File::open(self.root.path("document")).unwrap(),
            manifest: File::open(self.root.path("manifest")).unwrap(),
            media: File::open(self.root.path("media")).unwrap(),
            table: File::open(self.root.path("table")).unwrap(),
        }
    }

    fn audio_video() -> Self {
        use kinewright_media::{owned_io::CertifiedSource, test_support::GeneratedMedia};
        let generated = GeneratedMedia::ffmpeg(
            "c0-readonly-av",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=32x24:r=24:d=0.5",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.5",
                "-c:v",
                "libx264",
                "-qp",
                "0",
                "-preset",
                "ultrafast",
                "-x264-params",
                "colorprim=bt709:transfer=bt709:colormatrix=bt709",
                "-pix_fmt",
                "yuv420p",
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
            "mkv",
        );
        let mut fixture = Self::new();
        fixture.bytes = std::fs::read(generated.path()).unwrap();
        std::fs::write(fixture.root.path("media"), &fixture.bytes).unwrap();
        let object = reference(&fixture.bytes, RecordKind::OwnedMedia);
        let digest = |bytes: &[u8]| {
            let mut hash = Sha256::new();
            hash.update(bytes);
            hash.finalize()
        };
        fixture.table.source_sha256.clone_from(&object.sha256);
        fixture.table.source_length = object.byte_length;
        fixture.table.chunk_sha256 = fixture
            .bytes
            .chunks(usize::try_from(INPUT_CHUNK_BYTES).unwrap())
            .map(digest)
            .collect();
        let table_ref = record(
            &fixture.root.path("table"),
            &fixture.table,
            RecordKind::InputChunkTable,
        );
        let author = CertifiedSource::new(
            File::open(fixture.root.path("media")).unwrap(),
            digest(&fixture.bytes),
            object.byte_length,
            digest(&serde_json::to_vec(&fixture.table).unwrap()),
            Arc::from(fixture.table.chunk_sha256.as_slice()),
            Arc::new(()),
            Arc::new(|_| Ok(Arc::new(()))),
        )
        .unwrap();
        let probe = author.probe(cancel()).unwrap();
        let stream = |index| {
            let selected = probe
                .streams
                .iter()
                .find(|stream| stream.index == index)
                .unwrap();
            let base = Rational::new(
                u32::try_from(selected.time_base.0).unwrap(),
                u32::try_from(selected.time_base.1).unwrap(),
            )
            .unwrap();
            StreamInterpretation {
                index: u32::try_from(index).unwrap(),
                time_base: (i64::from(base.numerator()), i64::from(base.denominator())),
                timestamp_origin: selected.timestamp_origin,
            }
        };
        let asset = author
            .probe_asset(
                AssetId(1),
                &fixture.root.path("absent-video-provenance.mkv"),
                cancel(),
            )
            .unwrap();
        let interpretation = MediaInterpretation {
            version: 1,
            video: probe.video_stream.map(stream),
            audio: probe.audio_stream.map(stream),
            fps: probe.fps,
            duration: probe.duration,
            kind: probe.kind,
            resolution: probe.resolution,
            effective_color: probe.color_description.clone(),
            probed_color: probe.color_description,
            source_profile_assumption: kinewright_media::source_profile_assumption(
                &asset.color_description,
            ),
        };
        let profile = interpretation.canonical().unwrap();
        fixture.binding.input.entry.entry_id = format!(
            "media:1:{}:{}:{}",
            object.sha256,
            object.byte_length,
            kinewright_media::sha256_bytes(profile.as_bytes())
        );
        fixture.binding.input.entry.object = object;
        fixture.binding.input.entry.dependencies = vec![table_ref];
        fixture.binding.input.entry.profile = Some(profile);
        fixture.document.media_pool = vec![asset];
        fixture.update_document();
        fixture.update_manifest();
        fixture
    }

    fn admit(&self) -> Result<ReadOnlyInputs, kinewright_project::bundle::BundleError> {
        ReadOnlyInputs::admit_fixture(&self.binding, self.files(), &StorageLimits::default())
    }

    fn update_manifest(&mut self) {
        self.manifest.entries[0] = self.binding.input.entry.clone();
        self.binding.input.manifest_ref = record(
            &self.root.path("manifest"),
            &self.manifest,
            RecordKind::InputManifest,
        );
    }

    fn update_document(&mut self) {
        self.binding.document_ref = record(
            &self.root.path("document"),
            &self.document,
            RecordKind::Document,
        );
    }
}

#[test]
fn s3_read_only_fixture_native_handles_probe_playback_seek_reopen() {
    let fixture = Fixture::new();
    let before = ["document", "manifest", "media", "table"]
        .map(|name| std::fs::read(fixture.root.path(name)).unwrap());
    let admission = fixture.admit().unwrap();
    let source = admission.certify_media_input().unwrap();
    let probe = source.probe(cancel()).unwrap();
    assert_eq!(probe.audio_stream, Some(0));
    assert_eq!(probe.video_stream, None);
    assert_eq!(probe.duration, TimeCode(3));
    let mut decoder = source.open_audio(cancel()).unwrap();
    let frame = decoder.next_frame().unwrap().unwrap();
    assert_eq!(&frame.data(0)[..2], &1234_i16.to_le_bytes());
    decoder.seek(0).unwrap();
    assert!(decoder.next_frame().unwrap().is_some());
    let reopened = admission.certify_media_input().unwrap();
    assert!(
        reopened
            .open_audio(cancel())
            .unwrap()
            .next_frame()
            .unwrap()
            .is_some()
    );
    let mut cursor = source.cursor(cancel()).unwrap();
    let mut bytes = vec![0; fixture.bytes.len()];
    assert_eq!(cursor.read(&mut bytes).unwrap(), bytes.len());
    assert_eq!(bytes, fixture.bytes);
    assert_eq!(cursor.read(&mut [0; 1]).unwrap(), 0);
    assert!(admission.usage().unwrap().decoded_bytes > 0);
    drop(admission);
    drop(source);
    drop(reopened);
    drop(decoder);
    assert_eq!(&frame.data(0)[..2], &1234_i16.to_le_bytes());
    assert_eq!(cursor.seek(SeekFrom::Start(0)).unwrap(), 0);
    assert_eq!(cursor.read(&mut [0; 4]).unwrap(), 4);
    let after = ["document", "manifest", "media", "table"]
        .map(|name| std::fs::read(fixture.root.path(name)).unwrap());
    assert_eq!(
        before, after,
        "read-only admission made zero byte mutations"
    );
}

#[test]
fn s3_read_only_fixture_native_av_landmarks_seek_reopen() {
    let fixture = Fixture::audio_video();
    assert!(!fixture.document.media_pool[0].path.exists());
    let admission = fixture.admit().unwrap();
    let source = admission.certify_media_input().unwrap();
    let probe = source.probe(cancel()).unwrap();
    assert_eq!(probe.kind, MediaKind::AudioVideo);
    assert_eq!(probe.video_stream, Some(0));
    assert_eq!(probe.audio_stream, Some(1));
    let mut video = source.open_video(cancel()).unwrap();
    assert_eq!(video.next_frame().unwrap().unwrap().data(0)[0], 62);
    video.seek(0).unwrap();
    assert_eq!(video.next_frame().unwrap().unwrap().data(0)[0], 62);
    let mut audio = source.open_audio(cancel()).unwrap();
    let frame = audio.next_frame().unwrap().unwrap();
    let samples = frame
        .data(0)
        .as_chunks::<2>()
        .0
        .iter()
        .take(2048)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let crossings = samples
        .windows(2)
        .filter(|pair| pair[0] <= 0 && pair[1] > 0)
        .count();
    assert!(crossings.abs_diff(440 * samples.len() / 48000) <= 1);
    audio.seek(0).unwrap();
    assert!(audio.next_frame().unwrap().is_some());
    let reopened = admission.certify_media_input().unwrap();
    assert_eq!(
        reopened
            .open_video(cancel())
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap()
            .data(0)[0],
        62
    );
    assert!(
        reopened
            .open_audio(cancel())
            .unwrap()
            .next_frame()
            .unwrap()
            .is_some()
    );
}

#[test]
fn s3_read_only_source_digest_and_length_refuse() {
    let mut fixture = Fixture::new();
    let mut bytes = fixture.bytes.clone();
    bytes[44] ^= 1;
    std::fs::write(fixture.root.path("media"), &bytes).unwrap();
    let mut hash = Sha256::new();
    hash.update(&bytes);
    fixture.table.chunk_sha256[0] = hash.finalize();
    fixture.binding.input.entry.dependencies[0] = record(
        &fixture.root.path("table"),
        &fixture.table,
        RecordKind::InputChunkTable,
    );
    fixture.update_manifest();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_SOURCE_DIGEST_REFUSED: same-length unverified source refused"
    );
    bytes.push(0);
    std::fs::write(fixture.root.path("media"), bytes).unwrap();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_SOURCE_LENGTH_REFUSED: wrong native descriptor length refused"
    );
}

#[test]
fn s3_read_only_manifest_digest_owner_and_membership_refuse() {
    let mut fixture = Fixture::new();
    fixture.binding.owner_epoch += 1;
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_MANIFEST_EPOCH_REFUSED"
    );
    fixture.binding.owner_epoch -= 1;
    fixture.binding.session_id = Some(SessionId::fresh().unwrap());
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_MANIFEST_OWNER_REFUSED"
    );
    fixture
        .binding
        .session_id
        .clone_from(&fixture.manifest.session_id);
    fixture.binding.input.entry.entry_id.push('x');
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_MANIFEST_MEMBERSHIP_REFUSED"
    );
    fixture.binding.input.entry = fixture.manifest.entries[0].clone();
    fixture.binding.input.entry.dependencies.push(ObjectRef {
        sha256: "0".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedReference,
    });
    assert!(
        fixture.admit().is_err(),
        "exact entry includes every dependency"
    );
    fixture.binding.input.entry = fixture.manifest.entries[0].clone();
    let mut grown = std::fs::read(fixture.root.path("manifest")).unwrap();
    grown.push(b' ');
    std::fs::write(fixture.root.path("manifest"), &grown).unwrap();
    assert!(
        fixture.admit().is_err(),
        "exact descriptor length includes metadata objects"
    );
    grown.pop();
    std::fs::write(fixture.root.path("manifest"), grown).unwrap();
    let mut bytes = std::fs::read(fixture.root.path("manifest")).unwrap();
    let slot = bytes.iter().position(|byte| *byte == b'1').unwrap();
    bytes[slot] = b'2';
    std::fs::write(fixture.root.path("manifest"), bytes).unwrap();
    assert!(
        fixture.admit().is_err(),
        "manifest bytes must match its exact ref"
    );
}

#[test]
fn s3_read_only_document_fingerprint_and_interpretation_refuse() {
    let mut fixture = Fixture::new();
    fixture.document.media_pool[0]
        .source_fingerprint
        .content_sha256 = Some("0".repeat(64));
    fixture.update_document();
    assert!(
        fixture.admit().is_err(),
        "hashed document with wrong digest refused"
    );
    fixture.document.media_pool[0]
        .source_fingerprint
        .content_sha256 = Some(fixture.binding.input.entry.object.sha256.clone());
    fixture.document.media_pool[0].source_fingerprint.byte_len = Some(1);
    fixture.update_document();
    assert!(
        fixture.admit().is_err(),
        "hashed document with wrong length refused"
    );
    fixture.document.media_pool[0].source_fingerprint.byte_len =
        Some(fixture.binding.input.entry.object.byte_length);
    fixture.document.media_pool[0].duration.0 += 1;
    fixture.update_document();
    assert!(
        fixture.admit().is_err(),
        "hashed document with wrong interpretation refused"
    );
    fixture.document.media_pool.clear();
    fixture.update_document();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_DOCUMENT_MEMBERSHIP_REFUSED: selected input must belong to the exact document"
    );
}

#[test]
fn s3_read_only_record_digest_and_canonical_refuse() {
    let mut fixture = Fixture::new();
    let original = std::fs::read(fixture.root.path("document")).unwrap();
    let mut bytes = original.clone();
    let offset = bytes
        .windows(b"provenance only".len())
        .position(|window| window == b"provenance only")
        .unwrap();
    bytes[offset] = b'q';
    std::fs::write(fixture.root.path("document"), &bytes).unwrap();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_RECORD_DIGEST_REFUSED: valid metadata must match its admitted digest"
    );
    bytes = original;
    bytes.insert(0, b' ');
    std::fs::write(fixture.root.path("document"), &bytes).unwrap();
    fixture.binding.document_ref = reference(&bytes, RecordKind::Document);
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_RECORD_CANONICAL_REFUSED: valid hashed noncanonical metadata refuses"
    );
}

#[test]
fn s3_read_only_table_binding_and_probe_interpretation_refuse() {
    let mut fixture = Fixture::new();
    let original = fixture.table.source_sha256.clone();
    fixture.table.source_sha256 = "0".repeat(64);
    fixture.binding.input.entry.dependencies[0] = record(
        &fixture.root.path("table"),
        &fixture.table,
        RecordKind::InputChunkTable,
    );
    fixture.update_manifest();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_TABLE_BINDING_REFUSED: valid hashed fingerprint-conflicting table refused"
    );
    fixture.table.source_sha256 = original;
    fixture.binding.input.entry.dependencies[0] = record(
        &fixture.root.path("table"),
        &fixture.table,
        RecordKind::InputChunkTable,
    );
    let mut interpretation = fixture.binding.input.entry.media_interpretation().unwrap();
    interpretation.audio.as_mut().unwrap().timestamp_origin = 1;
    let profile = interpretation.canonical().unwrap();
    fixture.binding.input.entry.entry_id = format!(
        "media:1:{}:{}:{}",
        fixture.binding.input.entry.object.sha256,
        fixture.binding.input.entry.object.byte_length,
        kinewright_media::sha256_bytes(profile.as_bytes())
    );
    fixture.binding.input.entry.profile = Some(profile);
    fixture.update_manifest();
    assert!(
        fixture.admit().is_err(),
        "C0_READONLY_PROBE_INTERPRETATION_REFUSED: probe timing must match admitted interpretation"
    );
}

#[test]
fn s3_read_only_tamper_after_certification_and_cache_hit_size_refuse() {
    let fixture = Fixture::new();
    let admission = fixture.admit().unwrap();
    let source = admission.certify_media_input().unwrap();
    let mut cursor = source.cursor(cancel()).unwrap();
    let mut writer = OpenOptions::new()
        .write(true)
        .open(fixture.root.path("media"))
        .unwrap();
    writer.seek(SeekFrom::Start(44)).unwrap();
    writer.write_all(&[0]).unwrap();
    writer.flush().unwrap();
    assert!(
        cursor.read(&mut [0; 64]).is_err(),
        "uncached altered chunk refused"
    );
    assert!(
        admission.certify_media_input().is_err(),
        "fresh certification sees tamper"
    );
    writer.seek(SeekFrom::Start(44)).unwrap();
    writer.write_all(&fixture.bytes[44..45]).unwrap();
    writer.flush().unwrap();
    let source = admission.certify_media_input().unwrap();
    let mut cursor = source.cursor(cancel()).unwrap();
    assert_eq!(cursor.read(&mut [0; 4]).unwrap(), 4);
    writer.set_len(fixture.bytes.len() as u64 + 1).unwrap();
    assert!(
        cursor.read(&mut [0; 4]).is_err(),
        "size change rejects a cached read"
    );
}

#[test]
fn s3_read_only_budget_refuses_before_allocation() {
    let fixture = Fixture::new();
    let limits = StorageLimits {
        metadata_bytes: 1,
        ..StorageLimits::default()
    };
    assert!(ReadOnlyInputs::admit_fixture(&fixture.binding, fixture.files(), &limits).is_err());
    let limits = StorageLimits {
        decoder_bytes: kinewright_media::owned_io::CURSOR_RESERVATION_BYTES - 1,
        ..StorageLimits::default()
    };
    assert!(ReadOnlyInputs::admit_fixture(&fixture.binding, fixture.files(), &limits).is_err());
    let limits = StorageLimits {
        decoded_record_bytes: 128,
        ..StorageLimits::default()
    };
    assert!(ReadOnlyInputs::admit_fixture(&fixture.binding, fixture.files(), &limits).is_err());
}

#[cfg(all(feature = "test-util", target_os = "linux"))]
#[test]
fn s3_read_only_production_fixture_and_store_identity_ledger() {
    use kinewright_project::{bundle::LoadedBundle, c0_fixtures::build_c0_fixture_pair};
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c0-readonly-real");
    for fixture in build_c0_fixture_pair(temp.root()).unwrap() {
        let loaded = LoadedBundle::read(&fixture.path).unwrap();
        let document_ref = loaded.manual_checkpoint().unwrap().document_ref.clone();
        let owner = loaded.into_writer().unwrap();
        let manifest = owner
            .store()
            .read_input_manifest(&fixture.media.manifest_ref)
            .unwrap();
        let binding = ReadOnlyInputBinding {
            document_ref,
            input: fixture.media.clone(),
            project_id: manifest.project_id.clone(),
            session_id: manifest.session_id.clone(),
            owner_epoch: manifest.owner_epoch,
        };
        drop(manifest);
        let objects = fixture
            .path
            .with_extension("kinewright-assets")
            .join("studio/objects");
        let files = || ReadOnlyInputFiles {
            document: File::open(objects.join(&binding.document_ref.sha256)).unwrap(),
            manifest: File::open(objects.join(&binding.input.manifest_ref.sha256)).unwrap(),
            media: File::open(objects.join(&binding.input.entry.object.sha256)).unwrap(),
            table: File::open(objects.join(&fixture.table_ref.sha256)).unwrap(),
        };
        let standalone =
            ReadOnlyInputs::admit_fixture(&binding, files(), &StorageLimits::default()).unwrap();
        let certified = standalone.certify_media_input().unwrap();
        let mut video = certified.open_video(cancel()).unwrap();
        assert_eq!(
            video.next_frame().unwrap().unwrap().data(0)[0],
            fixture.expected_video_plane0
        );
        video.seek(0).unwrap();
        assert!(video.next_frame().unwrap().is_some());
        let mut audio = certified.open_audio(cancel()).unwrap();
        assert!(audio.next_frame().unwrap().is_some());
        audio.seek(0).unwrap();
        assert!(audio.next_frame().unwrap().is_some());
        assert!(standalone.certify_media_input().is_ok());

        let baseline = owner.store().usage().unwrap().decoded_bytes;
        let admitted = owner
            .store()
            .admit_read_only_inputs(&binding, files())
            .unwrap();
        let certified = admitted.certify_media_input().unwrap();
        assert_eq!(
            admitted.usage().unwrap().decoded_bytes,
            owner.store().usage().unwrap().decoded_bytes
        );
        assert!(owner.store().usage().unwrap().decoded_bytes > baseline);
        let mut video = certified.open_video(cancel()).unwrap();
        let frame = video.next_frame().unwrap().unwrap();
        drop(video);
        drop(certified);
        drop(admitted);
        assert_eq!(frame.data(0)[0], fixture.expected_video_plane0);
        assert!(
            owner.store().usage().unwrap().decoded_bytes > baseline,
            "last frame retains owner charges"
        );
        drop(frame);
        assert_eq!(owner.store().usage().unwrap().decoded_bytes, baseline);

        let copy = temp.path("same-bytes-different-native-object");
        std::fs::copy(objects.join(&binding.input.entry.object.sha256), &copy).unwrap();
        let mut wrong = files();
        wrong.media = File::open(copy).unwrap();
        assert!(
            owner
                .store()
                .admit_read_only_inputs(&binding, wrong)
                .is_err(),
            "C0_READONLY_NATIVE_IDENTITY_REFUSED: equal bytes cannot substitute another native object"
        );
        let mut wrong_owner = binding.clone();
        wrong_owner.owner_epoch += 1;
        assert!(
            owner
                .store()
                .admit_read_only_inputs(&wrong_owner, files())
                .is_err()
        );
    }
}

#[cfg(windows)]
#[test]
fn s3_read_only_windows_certification_keeps_write_refusal_zero_mutation() {
    use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};
    let fixture = Fixture::new();
    let admission = fixture.admit().unwrap();
    assert!(
        admission
            .certify_media_input()
            .unwrap()
            .open_audio(cancel())
            .is_ok()
    );
    let destination = fixture.root.path("must-remain-absent.kinewright");
    let before = std::fs::read_dir(fixture.root.root()).unwrap().count();
    assert!(ProjectBundle::activate(&destination, BundleWriteAdmission::new_document()).is_err());
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read_dir(fixture.root.root()).unwrap().count(),
        before
    );
}
