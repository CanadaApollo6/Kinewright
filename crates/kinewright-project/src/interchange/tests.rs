use std::{io::Cursor, sync::Mutex};

use crossbeam_channel::{Receiver, Sender, bounded};
use kinewright_core::{
    AssetId, ColorDescription, MediaError, MediaKind, MediaSourceFingerprint, ProbeEvent,
    ProbeObserver, TimeCode,
};
use serde_json::json;

use super::*;

fn otio(references: &[&str]) -> Vec<u8> {
    let children: Vec<_> = references.iter().map(|locator| json!({
        "OTIO_SCHEMA": "Clip.1",
        "source_range": {"start_time":{"value":0,"rate":24},"duration":{"value":24,"rate":24}},
        "media_reference": {"OTIO_SCHEMA":"ExternalReference.1","target_url":locator}
    })).collect();
    serde_json::to_vec(&json!({
        "OTIO_SCHEMA":"Timeline.1",
        "name":"Prepared edit",
        "global_start_time":{"value":0,"rate":24},
        "tracks":{"OTIO_SCHEMA":"Stack.1","children":[
            {"OTIO_SCHEMA":"Track.1","kind":"Video","children":children}
        ]}
    }))
    .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Open(PathBuf),
    Stat(PathBuf),
    Probe(PathBuf),
    ReadDir(PathBuf),
}

struct Block {
    names: Vec<&'static str>,
    phase: Option<ProbeEvent>,
    started: Sender<()>,
    release: Receiver<()>,
    stopped: Sender<bool>,
}

struct RecordingFs {
    main: Vec<u8>,
    calls: Mutex<Vec<Call>>,
    block: Option<Block>,
}

impl RecordingFs {
    fn new(references: &[&str]) -> Self {
        Self {
            main: otio(references),
            calls: Mutex::default(),
            block: None,
        }
    }

    fn blocking(&self, path: &Path, phase: Option<ProbeEvent>) -> Option<&Block> {
        self.block.as_ref().filter(|block| {
            phase == block.phase && block.names.iter().any(|name| path.ends_with(name))
        })
    }
}

impl LocatorFs for RecordingFs {
    type Metadata = EntryKind;
    type Probe = MediaAsset;
    type Reader = Cursor<Vec<u8>>;
    type Entry = DirectoryEntry;
    type Directory = std::iter::Empty<io::Result<DirectoryEntry>>;

    fn stat(&self, path: &Path) -> io::Result<EntryKind> {
        self.calls.lock().unwrap().push(Call::Stat(path.into()));
        if let Some(block) = self.blocking(path, None) {
            let _ = block.started.send(());
            let _ = block.release.recv();
            let _ = block.stopped.send(true);
        }
        Ok(EntryKind::File)
    }

    fn open(&self, path: &Path) -> io::Result<Self::Reader> {
        self.calls.lock().unwrap().push(Call::Open(path.into()));
        Ok(Cursor::new(self.main.clone()))
    }

    fn probe(&self, path: &Path, observer: &dyn ProbeObserver) -> Result<MediaAsset, MediaError> {
        self.calls.lock().unwrap().push(Call::Probe(path.into()));
        if !observer.observe(ProbeEvent::HashedBytes(64)) {
            return Err(MediaError::Cancelled);
        }
        if let Some(block) = self.blocking(path, Some(ProbeEvent::HashedBytes(128))) {
            let _ = block.started.send(());
            let _ = block.release.recv();
            let proceed = observer.observe(ProbeEvent::HashedBytes(128));
            let _ = block.stopped.send(!proceed);
            if !proceed {
                return Err(MediaError::Cancelled);
            }
        } else if !observer.observe(ProbeEvent::HashedBytes(128)) {
            return Err(MediaError::Cancelled);
        }
        if !observer.observe(ProbeEvent::MetadataStarted) {
            return Err(MediaError::Cancelled);
        }
        if let Some(block) = self.blocking(path, Some(ProbeEvent::MetadataStarted)) {
            let _ = block.started.send(());
            let _ = block.release.recv();
            let cancelled = observer.cancelled();
            let _ = block.stopped.send(cancelled);
            if cancelled {
                return Err(MediaError::Cancelled);
            }
        }
        Ok(MediaAsset {
            id: AssetId(1),
            path: path.into(),
            name: "fixture".into(),
            fps: Rational::new(24, 1).unwrap(),
            duration: TimeCode(24),
            resolution: Some((64, 36)),
            kind: MediaKind::Video,
            source_fingerprint: MediaSourceFingerprint {
                content_sha256: Some("a".repeat(64)),
                byte_len: Some(128),
            },
            color_description: ColorDescription::default(),
            assumed_from: None,
        })
    }

    fn read_dir(&self, path: &Path) -> io::Result<Self::Directory> {
        self.calls.lock().unwrap().push(Call::ReadDir(path.into()));
        Ok(std::iter::empty())
    }
}

#[test]
fn remote_source_roots_and_media_never_reach_the_locator_seam() {
    let references = [
        "file://evil.example/s/a.mov",
        r"\\host\share\a.mov",
        r"\\?\UNC\host\share\a.mov",
        r"\\.\device",
        "//host/a.mov",
        "file://localhost//host/share/a.mov",
        "file:///local/%2fa.mov",
        "file:///local/%5Ca.mov",
        "https://host/a.mov",
    ];
    let fs = Arc::new(RecordingFs::new(&references));
    for locator in references {
        let refusal = prepare_with(
            Arc::clone(&fs),
            locator,
            ImportOptions::default(),
            Preparation::Probe,
        )
        .unwrap_err();
        assert_eq!(refusal.code, RefusalCode::RemotePath, "{locator}");
        let options = ImportOptions {
            media_roots: vec![locator.into()],
            ..ImportOptions::default()
        };
        let refusal = prepare_with(
            Arc::clone(&fs),
            "timeline.otio",
            options,
            Preparation::Probe,
        )
        .unwrap_err();
        assert_eq!(refusal.code, RefusalCode::RemotePath, "{locator}");
    }
    assert!(fs.calls.lock().unwrap().is_empty());
    let (prepared, _) = prepare_with(
        Arc::clone(&fs),
        "timeline.otio",
        ImportOptions::default(),
        Preparation::Probe,
    )
    .unwrap();
    assert_eq!(prepared.record().media.remote, 9);
    assert!(
        prepared
            .media()
            .iter()
            .all(|entry| entry.status == MediaStatus::Remote)
    );
    assert_eq!(
        *fs.calls.lock().unwrap(),
        [Call::Open("timeline.otio".into())]
    );
}

#[test]
fn inspection_stats_and_never_probes_or_hashes_media() {
    let fs = Arc::new(RecordingFs::new(&["media.mov"]));
    let options = ImportOptions::default();
    let control = options.control.clone();
    let (prepared, _) = prepare_with(
        Arc::clone(&fs),
        "timeline.otio",
        options,
        Preparation::Inspect(None),
    )
    .unwrap();
    assert_eq!(prepared.media()[0].status, MediaStatus::Unprobed);
    assert_eq!(prepared.record().media.online, 1);
    assert_eq!(control.hashed_bytes(), 0);
    let calls = fs.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(matches!(calls[0], Call::Open(_)));
    assert!(matches!(calls[1], Call::Stat(_)));
}

#[test]
fn abandoned_stats_are_replaced_and_offline_ids_are_last() {
    let slow = ["a.mov", "b.mov", "c.mov", "d.mov"];
    let (started, did_start) = bounded(4);
    let (release, held) = bounded(4);
    let (stopped, did_stop) = bounded(4);
    let mut fs = RecordingFs::new(&["a.mov", "b.mov", "c.mov", "d.mov", "live.mov"]);
    fs.block = Some(Block {
        names: slow.into(),
        phase: None,
        started,
        release: held,
        stopped,
    });
    let options = ImportOptions {
        deadlines: MediaDeadlines {
            stat: Duration::from_millis(150),
            ..MediaDeadlines::default()
        },
        ..ImportOptions::default()
    };
    let (prepared, _) =
        prepare_with(Arc::new(fs), "timeline.otio", options, Preparation::Probe).unwrap();
    for _ in slow {
        did_start.try_recv().unwrap();
        release.send(()).unwrap();
    }
    for _ in slow {
        assert!(did_stop.recv().unwrap());
    }
    assert_eq!(prepared.record().media.timed_out, 4);
    assert_eq!(prepared.record().media.online, 1);
    assert!(!prepared.record().estimated_asset(AssetId(1)));
    assert!(prepared.record().estimated_asset(AssetId(2)));
    assert!(prepared.record().estimated_asset(AssetId(5)));
    assert_eq!(prepared.media()[4].status, MediaStatus::Online);
}

#[test]
fn each_probe_deadline_abandons_and_stops_at_the_next_observation() {
    for phase in [ProbeEvent::HashedBytes(128), ProbeEvent::MetadataStarted] {
        let (started, did_start) = bounded(1);
        let (release, held) = bounded(1);
        let (stopped, did_stop) = bounded(1);
        let mut fs = RecordingFs::new(&["held.mov"]);
        fs.block = Some(Block {
            names: vec!["held.mov"],
            phase: Some(phase),
            started,
            release: held,
            stopped,
        });
        let options = ImportOptions {
            deadlines: MediaDeadlines {
                hash_stall: Duration::from_millis(150),
                metadata: Duration::from_millis(150),
                ..MediaDeadlines::default()
            },
            ..ImportOptions::default()
        };
        let (prepared, _) =
            prepare_with(Arc::new(fs), "timeline.otio", options, Preparation::Probe).unwrap();
        did_start.try_recv().unwrap();
        release.send(()).unwrap();
        assert!(did_stop.recv().unwrap());
        assert_eq!(prepared.record().media.timed_out, 1);
        assert!(
            prepared
                .record()
                .rows
                .iter()
                .any(|row| row.code == LossCode::MediaTimeout)
        );
    }
}

#[test]
fn cancel_discards_preparation_and_prevents_publication() {
    let options = ImportOptions::default();
    let control = options.control.clone();
    let fs = Arc::new(RecordingFs::new(&["media.mov"]));
    let (prepared, _) =
        prepare_with(Arc::clone(&fs), "edit.otio", options, Preparation::Probe).unwrap();
    assert_eq!(control.hashed_bytes(), 128);
    control.cancel();
    assert!(matches!(
        commit::<(), ()>(prepared, |_, _, _| panic!(
            "cancelled import must not publish"
        )),
        Err(CommitImportError::Cancelled)
    ));
    let opts = ImportOptions {
        control,
        ..ImportOptions::default()
    };
    assert_eq!(
        prepare_with(fs, "edit.otio", opts, Preparation::Probe)
            .unwrap_err()
            .code,
        RefusalCode::Cancelled
    );
}

#[test]
fn estimated_relink_requires_consent_and_covers_each_source_range() {
    use kinewright_core::{OpError, Operation, RelinkCandidate};

    let fs = RecordingFs::new(&["https://example.invalid/source.mov"]);
    let (mut prepared, _) = prepare_with(
        Arc::new(fs),
        "edit.otio",
        ImportOptions::default(),
        Preparation::Probe,
    )
    .unwrap();
    let id = AssetId(1);
    let mut candidate = RelinkCandidate {
        path: "new/source.mov".into(),
        fingerprint: MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(64),
        },
        kind: MediaKind::AudioVideo,
        fps: Rational::new(24, 1).unwrap(),
        duration: TimeCode(96),
        resolution: Some((256, 144)),
    };
    let relink = |document: &mut Document, candidate: &RelinkCandidate, allow_unverified_source| {
        Operation::RelinkAsset {
            asset: id,
            candidate: candidate.clone(),
            allow_unverified_source,
        }
        .apply(document)
    };
    assert_eq!(
        relink(&mut prepared.document, &candidate, false),
        Err(OpError::RelinkRequiresExplicitUnverifiedSource { asset: id })
    );
    candidate.duration = TimeCode(23);
    assert_eq!(
        relink(&mut prepared.document, &candidate, true),
        Err(OpError::SourceOutOfBounds {
            asset: id,
            end: TimeCode(24),
            duration: TimeCode(23)
        })
    );
    candidate.duration = TimeCode(96);
    candidate.kind = MediaKind::Audio;
    assert!(matches!(
        relink(&mut prepared.document, &candidate, true),
        Err(OpError::RelinkMetadataMismatch { field: "kind", .. })
    ));
    candidate.kind = MediaKind::AudioVideo;
    relink(&mut prepared.document, &candidate, true).unwrap();
    let asset = prepared.document.asset(id).unwrap();
    assert_eq!(asset.path, candidate.path);
    assert_eq!(asset.source_fingerprint, candidate.fingerprint);
    assert_eq!(asset.kind, candidate.kind);
    assert_eq!(asset.resolution, candidate.resolution);
    assert_eq!(asset.duration, candidate.duration);
    assert_eq!(prepared.document.duration, TimeCode(24));
    candidate.resolution = Some((512, 288));
    assert!(matches!(
        relink(&mut prepared.document, &candidate, true),
        Err(OpError::RelinkMetadataMismatch {
            field: "resolution",
            ..
        })
    ));
}

#[test]
fn refusing_hand_edited_import_record_uses_the_project_load_path() {
    use kinewright_media::test_support::TempDirectory;
    let fs = RecordingFs::new(&["one.mov"]);
    let (prepared, _) = prepare_with(
        Arc::new(fs),
        "edit.otio",
        ImportOptions::default(),
        Preparation::Inspect(None),
    )
    .unwrap();
    let directory = TempDirectory::new("ix1-edited-record");
    let path = directory.path("project.kinewright");
    let value = serde_json::to_value(prepared.document()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(&crate::load_document(&path).unwrap().0, prepared.document());

    let mut inconsistent = value.clone();
    inconsistent["imported_from"][0]["items"] = json!(9);
    std::fs::write(&path, serde_json::to_vec(&inconsistent).unwrap()).unwrap();
    assert!(
        crate::load_document(&path)
            .unwrap_err()
            .contains("inconsistent item totals")
    );

    let mut oversized = value;
    oversized["imported_from"][0]["rows"] = json!([]);
    oversized["imported_from"][0]["omitted_codes"] = json!([]);
    let mut rows: Vec<_> = [
        "cut_snapped",
        "clip_too_short",
        "effect_dropped",
        "transition_at_nest_cut",
        "source_clamped",
        "source_origin_assumed",
        "subframe_rounded",
        "before_start_trimmed",
    ]
    .into_iter()
    .map(|code| kinewright_core::LossRow {
        code: serde_json::from_value(json!(code)).unwrap(),
        fidelity: kinewright_core::Fidelity::Dropped,
        count: 1,
        bound: None,
        unverified: false,
        names: vec!["\0".repeat(24), "\u{1}".repeat(24)],
        at: vec![
            kinewright_core::LossSite {
                source: "\0".repeat(24),
                frame: i64::MAX,
                clip: Some(kinewright_core::ClipId(u64::MAX)),
            },
            kinewright_core::LossSite {
                source: "\u{1}".repeat(24),
                frame: i64::MAX,
                clip: Some(kinewright_core::ClipId(u64::MAX)),
            },
        ],
    })
    .collect();
    rows.sort_by_key(kinewright_core::LossRow::order);
    oversized["imported_from"][0]["rows"] = json!(rows);
    std::fs::write(&path, serde_json::to_vec(&oversized).unwrap()).unwrap();
    assert!(
        crate::load_document(&path)
            .unwrap_err()
            .contains("import record exceeds 4 KiB")
    );
}

struct LavfiFs(NativeLocatorFs);

impl LocatorFs for LavfiFs {
    type Metadata = EntryKind;
    type Probe = MediaAsset;
    type Reader = std::fs::File;
    type Entry = DirectoryEntry;
    type Directory = files::NativeEntries;

    fn stat(&self, path: &Path) -> io::Result<EntryKind> {
        self.0.stat(path)
    }

    fn open(&self, path: &Path) -> io::Result<Self::Reader> {
        self.0.open(path)
    }

    fn probe(&self, path: &Path, observer: &dyn ProbeObserver) -> Result<MediaAsset, MediaError> {
        kinewright_media::probe_fixture_observed(path, observer)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Self::Directory> {
        self.0.read_dir(path)
    }
}

#[test]
fn lavfi_probe_overrides_estimated_media_and_preserves_av_sync() {
    use kinewright_media::test_support::{TempDirectory, run_ffmpeg};

    let dir = TempDirectory::new("ix1-lavfi");
    let media = dir.path("reference.mp4");
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x36:r=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=400:sample_rate=48000",
            "-t",
            "0.5",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
        ],
        &media,
    );
    let main = dir.path("edit.otio");
    let mut source: serde_json::Value = serde_json::from_slice(&otio(&["reference.mp4"])).unwrap();
    let mut audio_track = source["tracks"]["children"][0].clone();
    audio_track["kind"] = json!("Audio");
    source["tracks"]["children"]
        .as_array_mut()
        .unwrap()
        .push(audio_track);
    for (has_origin, code) in [
        (false, LossCode::SourceOriginAssumed),
        (true, LossCode::SourceClamped),
    ] {
        if has_origin {
            for track in source["tracks"]["children"].as_array_mut().unwrap() {
                track["children"][0]["media_reference"]["available_range"] = json!({
                    "start_time":{"rate":24,"value":0}, "duration":{"rate":24,"value":24},
                });
            }
        }
        std::fs::write(&main, serde_json::to_vec(&source).unwrap()).unwrap();
        let online = prepare_with(
            Arc::new(LavfiFs(NativeLocatorFs { analysis: None })),
            main.to_str().unwrap(),
            ImportOptions::default(),
            Preparation::Probe,
        )
        .unwrap()
        .0;
        let inspected = inspect(main.to_str().unwrap(), ImportOptions::default(), None).unwrap();
        assert!(
            online.record().rows.iter().any(|row| row.code == code),
            "{:?}",
            online.record()
        );
        assert!(!inspected.record.rows.iter().any(|row| row.code == code));
        let document = online.document();
        assert_eq!(document.media_pool.len(), 1);
        let asset = &document.media_pool[0];
        assert_eq!(asset.kind, MediaKind::AudioVideo);
        assert_eq!(asset.fps, Rational::new(24, 1).unwrap());
        assert_eq!(asset.resolution, Some((64, 36)));
        assert_eq!(asset.duration, TimeCode(12));
        assert_eq!(asset.path, media);
        assert_eq!(
            asset.source_fingerprint.content_sha256.as_deref(),
            Some(ImportRecord::hash_source(&std::fs::read(&media).unwrap()).as_str()),
        );
        let clips: Vec<_> = document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .collect();
        assert_eq!(clips.len(), 2);
        assert!(clips[0].link.is_some());
        assert_eq!(clips[0].link, clips[1].link);
        assert_eq!(clips[0].timeline_start, clips[1].timeline_start);
        assert_eq!(clips[0].source_range, TimeCode::ZERO..TimeCode(12));
        assert_eq!(clips[0].source_range, clips[1].source_range);
    }
}

#[test]
fn bundles_and_roots_resolve_local_media_in_place() {
    use kinewright_media::test_support::TempDirectory;
    let dir = TempDirectory::new("ix1-bundles");
    let otio_bundle = dir.path("edit.otiod");
    let bundled_media = otio_bundle.join("media/one.mov");
    std::fs::create_dir_all(bundled_media.parent().unwrap()).unwrap();
    std::fs::write(&bundled_media, []).unwrap();
    std::fs::write(
        otio_bundle.join("content.otio"),
        otio(&["media/one.mov", "gone/two.mov", "gone/duplicate.mov"]),
    )
    .unwrap();
    let root = dir.path("roots");
    for parent in ["A", "B"] {
        std::fs::create_dir_all(root.join(parent)).unwrap();
        std::fs::write(root.join(parent).join("duplicate.mov"), []).unwrap();
    }
    let found = root.join("A/two.mov");
    std::fs::write(&found, []).unwrap();

    let inspected = inspect(
        otio_bundle.to_str().unwrap(),
        ImportOptions {
            media_roots: vec![root.to_str().unwrap().to_owned()],
            ..ImportOptions::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        inspected.media[0].resolved_path.as_ref(),
        Some(&bundled_media)
    );
    assert_eq!(inspected.media[1].resolved_path.as_ref(), Some(&found));
    assert!(inspected.media[1].relinked_by_name);
    assert_eq!(inspected.media[2].status, MediaStatus::Ambiguous);
    assert_eq!(inspected.media[2].matches.as_deref().unwrap().len(), 2);
    assert!(
        inspected
            .record
            .rows
            .iter()
            .any(|row| row.code == LossCode::RelinkedByName)
    );

    let fcpxml_bundle = dir.path("edit.fcpxmld");
    std::fs::create_dir(&fcpxml_bundle).unwrap();
    let main = fcpxml_bundle.join("Info.fcpxml");
    std::fs::write(&main, r#"<fcpxml version="1.13"><resources>
        <format id="f" frameDuration="1/24s" width="64" height="36"/>
        <asset id="a" format="f" start="0s" duration="1s" hasVideo="1"><media-rep kind="original-media" src="shot.mov"/></asset>
        </resources><library><event><project name="Test"><sequence format="f"><spine>
        <asset-clip ref="a" duration="1s"/>
        </spine></sequence></project></event></library></fcpxml>"#).unwrap();
    std::fs::write(fcpxml_bundle.join("TrackData"), b"opaque").unwrap();
    std::fs::write(fcpxml_bundle.join("shot.mov"), []).unwrap();
    let inspected = inspect(
        fcpxml_bundle.to_str().unwrap(),
        ImportOptions::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        inspected.media[0].resolved_path.as_ref(),
        Some(&fcpxml_bundle.join("shot.mov"))
    );
    let row = inspected
        .record
        .rows
        .iter()
        .find(|row| row.code == LossCode::MetadataDropped)
        .unwrap();
    assert!(row.names.iter().any(|name| name == "TrackData"));
    assert_eq!(
        inspect(main.to_str().unwrap(), ImportOptions::default(), None)
            .unwrap()
            .record,
        inspected.record
    );
}

#[test]
fn active_hash_cancellation_reaches_the_worker() {
    let (started, did_start) = bounded(1);
    let (release, held) = bounded(1);
    let (stopped, did_stop) = bounded(1);
    let options = ImportOptions::default();
    let control = options.control.clone();
    let mut fs = RecordingFs::new(&["held.mov"]);
    fs.block = Some(Block {
        names: vec!["held.mov"],
        phase: Some(ProbeEvent::HashedBytes(128)),
        started,
        release: held,
        stopped,
    });
    let import =
        std::thread::spawn(|| prepare_with(Arc::new(fs), "edit.otio", options, Preparation::Probe));
    did_start.recv().unwrap();
    assert_eq!(control.hashed_bytes(), 64);
    control.cancel();
    release.send(()).unwrap();
    assert!(did_stop.recv().unwrap());
    assert_eq!(
        import.join().unwrap().unwrap_err().code,
        RefusalCode::Cancelled
    );
}

#[test]
fn abandoning_the_worker_budget_reports_undispatched_locators_as_timeouts() {
    let slow = [
        "1.mov", "2.mov", "3.mov", "4.mov", "5.mov", "6.mov", "7.mov", "8.mov",
    ];
    let (started, did_start) = bounded(8);
    let (release, held) = bounded(8);
    let (stopped, did_stop) = bounded(8);
    let mut fs = RecordingFs::new(&[
        "1.mov", "2.mov", "3.mov", "4.mov", "5.mov", "6.mov", "7.mov", "8.mov", "9.mov",
    ]);
    fs.block = Some(Block {
        names: slow.into(),
        phase: None,
        started,
        release: held,
        stopped,
    });
    let fs = Arc::new(fs);
    let options = ImportOptions {
        deadlines: MediaDeadlines {
            stat: Duration::from_millis(150),
            ..MediaDeadlines::default()
        },
        ..ImportOptions::default()
    };
    let (prepared, _) = prepare_with(
        Arc::clone(&fs),
        "timeline.otio",
        options,
        Preparation::Probe,
    )
    .unwrap();
    for _ in slow {
        did_start.try_recv().unwrap();
        release.send(()).unwrap();
    }
    for _ in slow {
        assert!(did_stop.recv().unwrap());
    }
    let calls = fs.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::Stat(_)))
            .count(),
        8
    );
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::Stat(path) if path.ends_with("9.mov")))
    );
    assert_eq!(prepared.record().media.timed_out, 9);
    assert!(
        prepared
            .media()
            .iter()
            .all(|entry| entry.status == MediaStatus::TimedOut)
    );
}

#[cfg(unix)]
#[test]
fn media_root_walk_does_not_follow_symlinks() {
    use kinewright_media::test_support::TempDirectory;

    let dir = TempDirectory::new("ix1-media-root-link");
    let main = dir.path("timeline.otio");
    std::fs::write(&main, otio(&["lost/linked.mov"])).unwrap();
    let outside = dir.path("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("linked.mov"), []).unwrap();
    let root = dir.path("media_root");
    std::fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("shortcut")).unwrap();
    let inspected = inspect(
        main.to_str().unwrap(),
        ImportOptions {
            media_roots: vec![root.to_str().unwrap().into()],
            ..ImportOptions::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(inspected.media[0].status, MediaStatus::Missing);
    assert!(inspected.media[0].matches.is_none());
}
