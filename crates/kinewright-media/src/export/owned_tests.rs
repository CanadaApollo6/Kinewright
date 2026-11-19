use super::super::{AUDIO_CHANNELS, LegacyRenderContext, export_document_with_luts};
use super::*;
use crate::LutLibrary;
use crate::{
    LutStore,
    cc1_fixtures::{fallback_gpu, simple_document},
    decode::probe_path,
    owned_io::{CHUNK_BYTES, CertifiedSource, ReadAuthority},
    test_support::{TempDirectory, run_ffmpeg},
};
use kinewright_core::{
    AssetId, ClipContent, ClipId, DeliveryEncodeDepth, DeliveryProfile, Effect, EffectId,
    ExportCancellation, ExportProgress, LoudnessTarget, LutAssetId, MediaAsset, MediaSourceOrigin,
    Operation, ParamValue, TimeCode, Title, Track, TrackId, TrackKind,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

const RESOLUTION: (u32, u32) = (320, 180);
const STAGING_BYTES: u64 = 1 << 20;
const IDENTITY_LUT: &str =
    "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
const BLACK_LUT: &str = "LUT_3D_SIZE 2\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n";

#[derive(Default)]
struct Ledger {
    acquires: AtomicU64,
    live: AtomicU64,
}

impl Ledger {
    fn reserve(self: &Arc<Self>, bytes: u64) -> LifetimeGuard {
        self.live.fetch_add(bytes, Relaxed);
        self.acquires.fetch_add(1, Relaxed);
        Arc::new(Charge(Arc::clone(self), bytes))
    }
}

struct Charge(Arc<Ledger>, u64);
impl Drop for Charge {
    fn drop(&mut self) {
        self.0.live.fetch_sub(self.1, Relaxed);
    }
}

fn pin(path: &Path, ledger: &Arc<Ledger>) -> CertifiedSource {
    use kinewright_hash::Sha256;
    fn hash(bytes: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(bytes);
        hash.finalize()
    }

    let bytes = fs::read(path).unwrap();
    let chunks: Vec<_> = bytes.chunks(CHUNK_BYTES).map(hash).collect();
    let table = chunks.as_flattened();
    let ledger = Arc::clone(ledger);
    CertifiedSource::verify(
        File::open(path).unwrap(),
        hash(&bytes),
        bytes.len() as u64,
        hash(table),
        chunks.into(),
        Arc::new(()),
        Arc::new(move |bytes| Ok(ledger.reserve(bytes))),
    )
    .unwrap()
}

fn generate_av(path: &Path, color: &str, frequency: &str) {
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=320x180:r=24:d=0.75"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={frequency}:sample_rate=48000:duration=0.75"),
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "-colorspace",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_primaries",
            "bt709",
        ],
        path,
    );
}

fn generate_still(path: &Path, color: &str) {
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=320x180"),
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-c:v",
            "png",
        ],
        path,
    );
}

fn probed(path: &Path, id: u64) -> MediaAsset {
    let mut asset = probe_path(path, AssetId(id)).unwrap();
    let Operation::SetAssetColorDescription {
        color_description, ..
    } = kinewright_core::assume_rec709_operation(asset.id, &asset.color_description)
    else {
        unreachable!()
    };
    asset.color_description = color_description;
    asset
}

struct Fixture {
    dir: TempDirectory,
    document: Arc<Document>,
    sources: BTreeMap<AssetId, CertifiedSource>,
    library: Arc<LutLibrary>,
    store: LutStore,
    lut_locator: PathBuf,
    av_reads: Arc<Ledger>,
    still_reads: Arc<Ledger>,
}

impl Fixture {
    #[allow(
        clippy::too_many_lines,
        reason = "Build one document spanning every owned input kind"
    )]
    fn new() -> Self {
        let dir = TempDirectory::new("s5-owned-export");
        let av = dir.path("av.mp4");
        let still = dir.path("still.png");
        generate_av(&av, "red", "440");
        generate_still(&still, "yellow");
        let av_reads = Arc::new(Ledger::default());
        let still_reads = Arc::new(Ledger::default());
        let sources = [
            (
                AssetId(1),
                pin(&av, &av_reads)
                    .with_selected_streams(Some(0), Some(1))
                    .unwrap(),
            ),
            (
                AssetId(2),
                pin(&still, &still_reads)
                    .with_selected_streams(Some(0), None)
                    .unwrap(),
            ),
        ]
        .into();

        let mut document = simple_document(probed(&av, 1), RESOLUTION);
        assert_eq!(document.duration, TimeCode(18));
        document.media_pool.push(probed(&still, 2));
        let store = LutStore::for_project(&dir.path("project.kinewright")).unwrap();
        let lut_locator = dir.path("look.cube");
        fs::write(&lut_locator, IDENTITY_LUT).unwrap();
        let asset = store
            .import_lut_asset(&lut_locator)
            .unwrap()
            .into_lut_asset(LutAssetId(1));
        document.lut_assets.push(asset);
        let (library, _) = LutLibrary::build(&document.lut_assets, Some(&store));
        let video = &mut document.tracks[0].clips[0];
        video.audio_gain_tenth_db = -25;
        video.audio_fade_in_frames = TimeCode(2);
        video.audio_fade_out_frames = TimeCode(3);
        video.effects.push(Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "creative_look".into(),
            parameters: BTreeMap::from([("lut_asset_id".into(), ParamValue::Integer(1))]),
            keyframes: BTreeMap::new(),
        });
        let mut still_clip = video.clone();
        still_clip.id = ClipId(2);
        still_clip.asset = AssetId(2);
        still_clip.effects.clear();
        still_clip.audio_gain_tenth_db = 0;
        still_clip.audio_fade_in_frames = TimeCode::ZERO;
        still_clip.audio_fade_out_frames = TimeCode::ZERO;
        still_clip.timeline_start = TimeCode(12);
        still_clip.source_range = TimeCode::ZERO..TimeCode(1);
        let mut title_clip = still_clip.clone();
        title_clip.id = ClipId(3);
        title_clip.asset = AssetId(0);
        title_clip.timeline_start = TimeCode::ZERO;
        title_clip.source_range = TimeCode::ZERO..document.duration;
        title_clip.content = ClipContent::Title(Title {
            text: "Pinned export".into(),
            ..Title::default()
        });
        for clip in [still_clip, title_clip] {
            document.tracks.push(Track {
                id: TrackId(clip.id.0),
                kind: TrackKind::Video,
                name: None,
                sync_lock: true,
                clips: vec![clip],
            });
        }
        document.audio_mix.master.effects.push(Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(2),
            name: "audio_true_peak_limiter".into(),
            parameters: BTreeMap::from([("lookahead_milliseconds".into(), ParamValue::Integer(5))]),
            keyframes: BTreeMap::new(),
        });
        document.validate().unwrap();
        Self {
            dir,
            document: Arc::new(document),
            sources,
            library: Arc::new(library),
            store,
            lut_locator,
            av_reads,
            still_reads,
        }
    }

    fn inputs(&self) -> PlaybackInputs {
        let bytes = kinewright_core::title_font_bytes();
        PlaybackInputs::admit(
            Arc::clone(&self.document),
            self.sources.clone(),
            Arc::clone(&self.library),
            Arc::new(()),
        )
        .unwrap()
        .with_manifest_fonts([(
            "font:Inter",
            crate::sha256_bytes(bytes).as_str(),
            bytes.len() as u64,
        )])
    }

    fn settings(&self) -> ExportSettings {
        let mut settings = DeliveryProfile::SourceMaster.export_settings(
            &self.document,
            DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );
        settings.loudness_normalization = Some(LoudnessTarget {
            integrated_lufs_hundredths: -1_600,
            true_peak_ceiling_dbtp_hundredths: -100,
            tolerance_lu_hundredths: 100,
            loudness_range_max_lu_hundredths: None,
        });
        settings
    }

    fn replace(&self, asset: usize, generate: impl FnOnce(&Path)) {
        let locator = &self.document.media_pool[asset].path;
        fs::rename(locator, locator.with_extension("pinned")).unwrap();
        generate(locator);
    }
}

fn target(path: &Path) -> OwnedExportTarget<'_> {
    OwnedExportTarget {
        destination: path,
        staging_bytes: STAGING_BYTES,
        reserve_staging: &|_| Ok(Arc::new(())),
        reserve_working: &|_| Ok(Arc::new(())),
    }
}

fn ordinary(
    document: &Document,
    library: &Arc<LutLibrary>,
    settings: &ExportSettings,
    out: &Path,
    gpu: &GpuContext,
) -> ExportReport {
    let (progress, _) = crossbeam_channel::bounded(0);
    export_document_with_luts(
        document,
        out,
        settings,
        &progress,
        gpu.clone(),
        LegacyRenderContext::fixture(Arc::clone(library)),
    )
    .unwrap()
}

#[derive(Debug, PartialEq)]
struct Decoded {
    pictures: Vec<u8>,
    samples: Vec<f32>,
}

fn decoded(fixture: &Fixture, path: &Path) -> Decoded {
    let raw = fixture.dir.path("output.yuv");
    let args = [
        "-i".as_ref(),
        path.as_os_str(),
        "-an".as_ref(),
        "-pix_fmt".as_ref(),
        "yuv420p".as_ref(),
        "-c:v".as_ref(),
        "rawvideo".as_ref(),
        "-threads".as_ref(),
        "1".as_ref(),
        "-f".as_ref(),
        "rawvideo".as_ref(),
    ];
    run_ffmpeg(&args, &raw);
    let pictures = fs::read(raw).unwrap();
    let frame_bytes = (RESOLUTION.0 * RESOLUTION.1 * 3 / 2) as usize;
    assert_eq!(
        pictures.len(),
        usize::try_from(fixture.document.duration.0).unwrap() * frame_bytes
    );
    let samples = crate::decode_audio_range(
        path,
        fixture.document.fps,
        TimeCode::ZERO,
        fixture.document.duration,
        AUDIO_RATE,
        AUDIO_CHANNELS,
        &ExportCancellation::default(),
    )
    .unwrap();
    assert!(samples.iter().any(|sample| sample.abs() > 0.01));
    Decoded { pictures, samples }
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep the original encode and all replacements the same fixture"
)]
fn s5_owned_export_keeps_pinned_decodes_and_matches_ordinary_export() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let gpu = fallback_gpu().context();
    let out = fixture.dir.path("staged.mp4");
    let clean = fixture.dir.path("clean.mp4");
    let replaced = fixture.dir.path("replaced.mp4");
    let (progress, _) = crossbeam_channel::bounded(0);

    // One encode per process for paired peak-RSS measurements.
    match std::env::var("KINEWRIGHT_OWNED_EXPORT_RSS_LANE").as_deref() {
        Ok("ordinary") => {
            ordinary(&fixture.document, &fixture.library, &settings, &clean, &gpu);
            assert_eq!(crate::verify::presented_frame_count(&clean).unwrap(), 18);
            return;
        }
        Ok("owned") => {
            export_owned_document(
                &Arc::new(fixture.inputs()),
                target(&out),
                &settings,
                &progress,
                gpu,
            )
            .unwrap();
            assert_eq!(crate::verify::presented_frame_count(&out).unwrap(), 18);
            return;
        }
        _ => {}
    }

    let clean_report = ordinary(&fixture.document, &fixture.library, &settings, &clean, &gpu);
    assert!(clean_report.audio.as_ref().unwrap().limiter_passes > 0);
    let baseline = decoded(&fixture, &clean);
    let inputs = Arc::new(fixture.inputs());
    let assert_pinned = |case| {
        let before_av = fixture.av_reads.acquires.load(Relaxed);
        let before_still = fixture.still_reads.acquires.load(Relaxed);
        let receipt =
            export_owned_document(&inputs, target(&out), &settings, &progress, gpu.clone())
                .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(receipt.report, clean_report, "{case}: normalization");
        assert!(
            fixture.av_reads.acquires.load(Relaxed) >= before_av + 4,
            "{case}: certified video and audio opens"
        );
        assert!(
            fixture.still_reads.acquires.load(Relaxed) >= before_still + 2,
            "{case}: certified still open"
        );
        let actual = decoded(&fixture, &out);
        assert!(
            actual.pictures == baseline.pictures,
            "{case}: pinned frames differed"
        );
        assert!(
            actual.samples == baseline.samples,
            "{case}: pinned samples differed"
        );
        fs::remove_file(&out).unwrap();
    };
    assert_pinned("equivalence");
    fixture.replace(0, |path| generate_av(path, "blue", "880"));
    ordinary(
        &fixture.document,
        &fixture.library,
        &settings,
        &replaced,
        &gpu,
    );
    let alternate = decoded(&fixture, &replaced);
    assert!(baseline.pictures != alternate.pictures && baseline.samples != alternate.samples);
    assert_pinned("replaced AV");
    let av = &fixture.document.media_pool[0].path;
    fs::remove_file(av).unwrap();
    fs::rename(av.with_extension("pinned"), av).unwrap();

    fixture.replace(1, |path| generate_still(path, "blue"));
    ordinary(
        &fixture.document,
        &fixture.library,
        &settings,
        &replaced,
        &gpu,
    );
    assert!(
        baseline.pictures != decoded(&fixture, &replaced).pictures,
        "an ordinary export must see the replaced still",
    );
    assert_pinned("replaced still");
    let still = &fixture.document.media_pool[1].path;
    fs::remove_file(still).unwrap();
    fs::rename(still.with_extension("pinned"), still).unwrap();

    fs::write(&fixture.lut_locator, BLACK_LUT).unwrap();
    // Replace both the import locator and the store copy behind the original hash.
    fs::write(
        fixture
            .store
            .path_for(&fixture.document.lut_assets[0].sha256)
            .unwrap(),
        BLACK_LUT,
    )
    .unwrap();
    let alternative = fixture
        .store
        .import_lut_asset(&fixture.lut_locator)
        .unwrap()
        .into_lut_asset(LutAssetId(1));
    let mut replacement_doc = (*fixture.document).clone();
    replacement_doc.lut_assets[0] = alternative;
    let (library, _) = LutLibrary::build(&replacement_doc.lut_assets, Some(&fixture.store));
    ordinary(
        &replacement_doc,
        &Arc::new(library),
        &settings,
        &replaced,
        &gpu,
    );
    assert!(
        baseline.pictures != decoded(&fixture, &replaced).pictures,
        "an ordinary export must see the replacement LUT",
    );
    assert_pinned("replaced LUT");
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_owned_export_cancel_and_revocation_discard_partial_and_release_reservations() {
    let fixture = Fixture::new();
    let gpu = fallback_gpu().context();
    let origin = MediaSourceOrigin::new_project();
    for (at_retain, revoke, occupied) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, false, true),
    ] {
        let settings = fixture.settings();
        let out = fixture.dir.path("stage").with_extension("mp4");
        if occupied {
            fs::write(&out, b"earlier completed bytes").unwrap();
        }
        let (progress, events) = crossbeam_channel::unbounded::<ExportProgress>();
        let cancellation = settings.cancellation.clone();
        let finished = AtomicU64::new(0);
        let end_checks = AtomicU64::new(0);
        let last_frame = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&last_frame);
        let authority = ReadAuthority::new(
            origin.legacy_media().unwrap(),
            "owned fixture".into(),
            Arc::new(move || {
                for event in events.try_iter() {
                    observed.store(event.completed_frames, Relaxed);
                    if event.completed_frames == event.total_frames {
                        finished.fetch_add(1, Relaxed);
                    } else if !at_retain && event.completed_frames > 0 {
                        if revoke {
                            return Err(MediaError::MediaReadTemporarilyUnavailable);
                        }
                        cancellation.cancel();
                    }
                }
                // Two terminal progress events means the muxer wrote its trailer.
                // Cancel in the last check, after the intermediate check and sync.
                if at_retain && finished.load(Relaxed) == 2 && end_checks.fetch_add(1, Relaxed) == 1
                {
                    cancellation.cancel();
                }
                Ok(())
            }),
        );
        let staging = Arc::new(Ledger::default());
        let working = Arc::new(Ledger::default());
        let disk = Arc::clone(&staging);
        let ram = Arc::clone(&working);
        let reserve_staging = move |bytes| Ok(disk.reserve(bytes));
        let reserve_working = move |bytes| Ok(ram.reserve(bytes));
        let result = export_owned_document(
            &Arc::new(fixture.inputs().with_read_authority(authority).unwrap()),
            OwnedExportTarget {
                reserve_staging: &reserve_staging,
                reserve_working: &reserve_working,
                ..target(&out)
            },
            &settings,
            &progress,
            gpu.clone(),
        );
        let expected = if revoke {
            MediaError::MediaReadTemporarilyUnavailable
        } else {
            MediaError::Cancelled
        };
        assert_eq!(result.err(), Some(expected));
        if !at_retain {
            assert!(
                (1..u64::try_from(fixture.document.duration.0).unwrap())
                    .contains(&last_frame.load(Relaxed)),
                "cancellation or revocation must stop before the last submitted frame",
            );
        }
        for ledger in [&staging, &working, &fixture.av_reads, &fixture.still_reads] {
            assert_eq!(ledger.live.load(Relaxed), 0, "reservation outlived worker");
        }
        assert_eq!(staging.acquires.load(Relaxed), 1);
        assert_eq!(working.acquires.load(Relaxed), 1);
        if occupied {
            assert_eq!(fs::read(&out).unwrap(), b"earlier completed bytes");
            fs::remove_file(&out).unwrap();
        }
        assert!(!out.exists());
        assert_stage_clean(&fixture.dir);
    }
}

fn assert_stage_clean(dir: &TempDirectory) {
    assert!(!fs::read_dir(dir.root()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".kinewright-owned-")
    }));
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn s5_owned_export_refuses_an_unrenderable_manifest_font_and_enforces_stage_limits() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let gpu = fallback_gpu().context();
    let out = fixture.dir.path("stage.mp4");
    let (progress, _) = crossbeam_channel::bounded(0);
    let inputs = Arc::new(fixture.inputs().with_manifest_fonts([(
        "font:Example",
        "custom-digest",
        16,
    )]));
    let result = export_owned_document(&inputs, target(&out), &settings, &progress, gpu.clone());
    assert_eq!(
        result.err(),
        Some(MediaError::OwnedExportInputUnavailable {
            input: "font:Example (sha256 custom-digest)".into(),
        })
    );
    let inputs = Arc::new(fixture.inputs());
    let staging = Arc::new(Ledger::default());
    let disk = Arc::clone(&staging);
    let reserve_staging = move |bytes| Ok(disk.reserve(bytes));
    let receipt = export_owned_document(
        &inputs,
        OwnedExportTarget {
            reserve_staging: &reserve_staging,
            ..target(&out)
        },
        &settings,
        &progress,
        gpu.clone(),
    )
    .unwrap();
    assert_eq!(staging.live.load(Relaxed), STAGING_BYTES);
    let complete_len = fs::metadata(&out).unwrap().len();
    fs::remove_file(&out).unwrap();
    drop(receipt);
    assert_eq!(staging.live.load(Relaxed), 0);
    let payload_refusal = export_owned_document(
        &inputs,
        OwnedExportTarget {
            staging_bytes: 2_048,
            ..target(&out)
        },
        &settings,
        &progress,
        gpu.clone(),
    );
    assert!(
        matches!(payload_refusal, Err(MediaError::Backend(message)) if message.contains("staging limit"))
    );
    // The packet payload fits here. The complete MP4, including its trailer, does not.
    let disk_refusal = export_owned_document(
        &inputs,
        OwnedExportTarget {
            staging_bytes: complete_len - 1,
            ..target(&out)
        },
        &settings,
        &progress,
        gpu.clone(),
    );
    assert!(
        matches!(disk_refusal, Err(MediaError::Backend(message)) if !message.contains("coded payload"))
    );
    assert!(!out.exists());
    assert_stage_clean(&fixture.dir);
    fs::write(&out, b"keep me").unwrap();
    let collision = export_owned_document(&inputs, target(&out), &settings, &progress, gpu);
    assert!(collision.is_err());
    assert_eq!(fs::read(&out).unwrap(), b"keep me");
    assert_stage_clean(&fixture.dir);
}
