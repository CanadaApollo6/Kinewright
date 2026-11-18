use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};

use eframe::egui;
use kinewright_core::{
    Analysis, AssetId, ColorDescription, Document, MediaAsset, MediaAvailabilityKind,
    MediaAvailabilityStatus, MediaError, MediaEvent, MediaKind, MediaSourceFingerprint, Playback,
    PreviewFrame, ProbeEvent, ProbeObserver, Rational, RefusedMedia, RgbaImage, SceneStatus,
    SilenceStatus, TimeCode, TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord,
    TranscriptStatus, VisualAssetResult, WorkingProof, export_media_preflight,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::interchange::{ImportOptions, PrepareOptions, PreparedImport, prepare};
use serde_json::json;

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Probe(PathBuf),
    Status(PathBuf),
    Playback(PathBuf),
    Thumbnail(PathBuf),
    Waveform(PathBuf),
}

pub(crate) struct ProbeHold {
    started: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    finished: mpsc::SyncSender<bool>,
}

impl ProbeHold {
    pub(crate) fn new(
        started: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        finished: mpsc::SyncSender<bool>,
    ) -> Self {
        Self {
            started,
            release: Mutex::new(release),
            finished,
        }
    }
}

#[derive(Default)]
pub(crate) struct RecordingMedia {
    calls: Mutex<Vec<Call>>,
    probe_hold: Option<ProbeHold>,
}

impl RecordingMedia {
    pub(crate) fn holding(hold: ProbeHold) -> Self {
        Self {
            calls: Mutex::default(),
            probe_hold: Some(hold),
        }
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
}

impl Playback for RecordingMedia {
    fn set_document(&self, document: Arc<Document>) {
        for asset in document.timeline_referenced_media_assets() {
            self.record(Call::Playback(asset.path.clone()));
        }
    }

    fn request_frame(&self, at: TimeCode) {
        RefusedMedia.request_frame(at);
    }
    fn frames(&self) -> crossbeam_channel::Receiver<PreviewFrame> {
        RefusedMedia.frames()
    }
    fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
        RefusedMedia.events()
    }
    fn play(&self, from: TimeCode) {
        RefusedMedia.play(from);
    }
    fn pause(&self) {
        RefusedMedia.pause();
    }
    fn seek(&self, to: TimeCode) {
        RefusedMedia.seek(to);
    }
    fn position(&self) -> TimeCode {
        RefusedMedia.position()
    }
    fn output_peaks(&self) -> [f32; 2] {
        RefusedMedia.output_peaks()
    }
}

impl Analysis for RecordingMedia {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError> {
        self.probe_observed(path, &())
    }

    fn probe_observed(
        &self,
        path: &Path,
        observer: &dyn ProbeObserver,
    ) -> Result<MediaAsset, MediaError> {
        self.record(Call::Probe(path.to_path_buf()));
        if !observer.observe(ProbeEvent::HashedBytes(64)) {
            return Err(MediaError::Cancelled);
        }
        if let Some(hold) = &self.probe_hold {
            hold.started.send(()).unwrap();
            hold.release.lock().unwrap().recv().unwrap();
            let stopped = !observer.observe(ProbeEvent::HashedBytes(128));
            hold.finished.send(stopped).unwrap();
            if stopped {
                return Err(MediaError::Cancelled);
            }
        }
        if !observer.observe(ProbeEvent::MetadataStarted) {
            return Err(MediaError::Cancelled);
        }
        Ok(MediaAsset {
            id: AssetId(1),
            path: path.to_path_buf(),
            remote_locator: None,
            name: "Probed fixture".into(),
            duration: TimeCode(480),
            fps: Rational::new(24, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((128, 72)),
            source_fingerprint: MediaSourceFingerprint {
                content_sha256: Some("a".repeat(64)),
                byte_len: Some(128),
            },
            color_description: ColorDescription::default(),
            assumed_from: None,
        })
    }

    fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
        self.record(Call::Status(asset.path.clone()));
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OfflineMissing,
            observed_fingerprint: None,
            reason: None,
        }
    }

    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError> {
        RefusedMedia.thumbnail_at(t, max_w)
    }

    fn thumbnail_for_document(
        &self,
        doc: Arc<Document>,
        t: TimeCode,
        max_w: u32,
    ) -> Result<RgbaImage, MediaError> {
        RefusedMedia.thumbnail_for_document(doc, t, max_w)
    }

    fn working_proof_for_document(
        &self,
        doc: Arc<Document>,
        t: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        RefusedMedia.working_proof_for_document(doc, t)
    }

    fn request_transcription(&self, _: MediaAsset) {}
    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus {
        RefusedMedia.transcript_status(asset)
    }
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        RefusedMedia.timeline_transcript(document, range)
    }

    fn request_silence_detection(&self, _: MediaAsset) {}
    fn silence_status(&self, asset: &MediaAsset) -> SilenceStatus {
        RefusedMedia.silence_status(asset)
    }
    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        shortest: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        RefusedMedia.timeline_silences(document, range, shortest)
    }

    fn request_scene_detection(&self, _: MediaAsset) {}
    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus {
        RefusedMedia.scene_status(asset)
    }
    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        threshold: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        RefusedMedia.timeline_scene_changes(document, range, threshold)
    }

    fn request_waveform(&self, asset: MediaAsset, _: u64) -> bool {
        self.record(Call::Waveform(asset.path));
        true
    }

    fn request_thumbnail(&self, asset: MediaAsset, _: TimeCode, _: u32, _: u64) -> bool {
        self.record(Call::Thumbnail(asset.path));
        true
    }

    fn visual_asset_results(&self) -> crossbeam_channel::Receiver<VisualAssetResult> {
        crossbeam_channel::never()
    }
}

pub(crate) fn hostile_locator() -> String {
    let (head, tail) = ("file://unapproved.invalid/source/\n", "\u{202e}tail.mov");
    let bytes = 8 * 1024 * 1024;
    format!(
        "{head}{}{tail}",
        "x".repeat(bytes - head.len() - tail.len())
    )
}

pub(crate) fn assert_locator_layout(output: &egui::FullOutput, limit: usize) {
    let painted = crate::theme::painted_text(output);
    let longest = painted.iter().map(|text| text.chars().count()).max();
    assert!(
        longest.is_some_and(|n| n <= limit),
        "longest galley {longest:?}, limit {limit}"
    );
    assert!(
        painted
            .iter()
            .any(|text| text.contains(r"\u{202e}tail") && text.contains('…')),
        "the bounded locator reached the layout"
    );
    assert!(
        painted
            .iter()
            .all(|text| !text.contains(['\n', '\u{202e}']))
    );
}

pub(crate) fn layout_text_position(
    output: &egui::FullOutput,
    text_matches: impl Fn(&str) -> bool,
) -> egui::Pos2 {
    fn find(shape: &egui::epaint::Shape, matches: &impl Fn(&str) -> bool) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(text) if matches(text.galley.text()) => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| find(shape, matches))
            }
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, &text_matches))
        .expect("the text was laid out")
}

pub(crate) fn click_text(
    ctx: &egui::Context,
    at: egui::Pos2,
    button: egui::PointerButton,
    mut draw: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let input = |pressed| egui::RawInput {
        events: vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        ..Default::default()
    };
    let _ = ctx.run_ui(input(true), &mut draw);
    ctx.run_ui(input(false), draw)
}

pub(crate) fn assert_locator_tooltip(
    ctx: &egui::Context,
    at: egui::Pos2,
    mut draw: impl FnMut(&mut egui::Ui),
) {
    ctx.global_style_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let input = egui::RawInput {
        events: vec![egui::Event::PointerMoved(at)],
        ..Default::default()
    };
    let _ = ctx.run_ui(input, &mut draw);
    let output = ctx.run_ui(egui::RawInput::default(), draw);
    assert_locator_layout(&output, 1024);
    assert!(
        crate::theme::painted_text(&output)
            .iter()
            .any(|text| { text.chars().count() > 160 && text.contains(r"\u{202e}tail") }),
        "the tooltip laid out the longer text"
    );
}

pub(crate) fn write_edit(path: &Path, reference: &str) {
    let timeline = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": "Reel Two",
        "global_start_time": {"value":0,"rate":24},
        "tracks": {"OTIO_SCHEMA":"Stack.1","children":[
            {"OTIO_SCHEMA":"Track.1","name":"Camera","kind":"Video","children":[
                {"OTIO_SCHEMA":"Clip.1","name":"First take",
                 "source_range":{"start_time":{"value":0,"rate":24},"duration":{"value":24,"rate":24}},
                 "media_reference":{"OTIO_SCHEMA":"ExternalReference.1","target_url":reference}}
            ]}
        ]}
    });
    std::fs::write(path, serde_json::to_vec(&timeline).unwrap()).unwrap();
}

pub(crate) fn prepare_edit(path: &Path, analysis: Arc<dyn Analysis>) -> PreparedImport {
    prepare(
        path.to_str().unwrap(),
        PrepareOptions {
            import: ImportOptions::default(),
            allowed_shares: std::collections::BTreeSet::new(),
            analysis,
        },
    )
    .unwrap()
}

#[test]
fn unapproved_remote_import_reaches_editor_media_ports_only_with_an_empty_path() {
    use crate::{media_access::MediaBackend, project::ProjectSession, visual_cache::VisualCache};
    use kinewright_core::PROJECT_FORMAT_VERSION;
    use kinewright_project::{SidecarMode, bundle::BundleWriteAdmission, interchange::commit};

    let directory = TempDirectory::new("ix1-remote");
    let input = directory.path("edit.otio");
    let locator = "file://unapproved.invalid/Camera/first.mov";
    write_edit(&input, locator);
    let ports = Arc::new(RecordingMedia::default());

    let prepared = prepare_edit(&input, ports.clone());
    assert!(
        ports.calls.lock().unwrap().is_empty(),
        "prepare never probes this host"
    );
    let session = commit(
        prepared,
        |name, document, admission: BundleWriteAdmission| {
            ProjectSession::create(
                1,
                name,
                document,
                admission,
                None,
                MediaBackend::Stub {
                    playback: ports.clone(),
                    analysis: ports.clone(),
                    exporter: Arc::new(RefusedMedia),
                },
                &SidecarMode::None,
                None,
                PROJECT_FORMAT_VERSION,
                None,
            )
        },
    )
    .unwrap();
    let asset = &session.document.media_pool[0];
    assert_eq!(asset.display_location(), locator);
    assert_eq!(asset.path, Path::new(""));
    assert!(std::fs::File::open(&asset.path).is_err());
    assert!(!asset.path.is_file());

    let playback = session.media().playback;
    playback.set_document(Arc::clone(&session.document));
    playback.request_frame(TimeCode::ZERO);
    let analysis = session.analysis();
    let (responses, receive) = mpsc::channel();
    crate::media_workflow::request_media_status(
        Arc::clone(&analysis),
        asset.clone(),
        session.id,
        1,
        responses,
    )
    .unwrap()
    .join()
    .unwrap();
    let status = receive.try_recv().unwrap();
    assert_eq!(status.path, Path::new(""));
    assert_eq!(status.status.kind, MediaAvailabilityKind::OfflineMissing);
    let mut visuals = VisualCache::new(analysis.visual_asset_results());
    assert!(
        visuals
            .thumbnail(analysis.as_ref(), asset, TimeCode::ZERO, 96)
            .is_none()
    );
    assert!(visuals.waveform(analysis.as_ref(), asset).is_none());
    let report = export_media_preflight(&session.document, analysis.as_ref());
    assert_eq!(report.checked_assets, [asset.id]);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(
        *ports.calls.lock().unwrap(),
        [
            Call::Playback(PathBuf::new()),
            Call::Status(PathBuf::new()),
            Call::Thumbnail(PathBuf::new()),
            Call::Waveform(PathBuf::new()),
            Call::Status(PathBuf::new()),
        ]
    );
}

#[test]
fn estimated_import_uses_the_editors_confirmed_relink_with_new_metadata() {
    use crate::media_workflow::{RelinkPreflight, build_relink_candidate, preflight_relink};
    use kinewright_core::{OpError, Operation};

    let directory = TempDirectory::new("ix1-ui-relink");
    let input = directory.path("edit.otio");
    write_edit(&input, "file://unapproved.invalid/Camera/first.mov");
    let analysis = Arc::new(RecordingMedia::default());
    let prepared = prepare_edit(&input, analysis.clone());
    let mut document =
        kinewright_project::interchange::commit(prepared, |_, document, _| Ok::<_, ()>(document))
            .unwrap();
    let asset = &document.media_pool[0];
    let id = asset.id;
    let replacement = directory.path("replacement.mov");
    let probed = analysis.probe(&replacement).unwrap();
    let candidate = build_relink_candidate(replacement, &probed);
    assert_ne!(asset.duration, candidate.duration);
    assert_ne!(asset.resolution, candidate.resolution);
    assert_eq!(
        preflight_relink(asset, &candidate),
        Ok(RelinkPreflight::NeedsLegacyConfirmation)
    );
    let without_consent = Operation::RelinkAsset {
        asset: id,
        candidate: candidate.clone(),
        allow_unverified_source: false,
    };
    assert_eq!(
        without_consent.apply(&mut document),
        Err(OpError::RelinkRequiresExplicitUnverifiedSource { asset: id }),
    );
    assert!(document.asset(id).unwrap().remote_locator.is_some());
    Operation::RelinkAsset {
        asset: id,
        candidate,
        allow_unverified_source: true,
    }
    .apply(&mut document)
    .unwrap();
    let relinked = document.asset(id).unwrap();
    assert_eq!(relinked.path, probed.path);
    assert_eq!(relinked.duration, probed.duration);
    assert_eq!(relinked.resolution, probed.resolution);
    assert!(relinked.remote_locator.is_none());
    document.validate().unwrap();
}
