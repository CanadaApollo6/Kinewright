//! Unit tests for the `MCP` server, split by tool family.

mod captions;
mod color_nodes;
mod color_proof;
mod look_assets;
mod mattes;
mod media_relink;
mod planning;
mod reframe_geometry_tests;
mod scope_tests;
mod source_program;
mod storyboards;
mod surface;
mod tracking;
mod tracking_tests;

use super::*;
use kinewright_core::{
    AssetBeats, AssetId, AssetSceneChanges, AssetTranscript, BeatMarker, Clip, ColorBitDepth,
    ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
    ColorWhitePoint, FrameTexture, Marker, MarkerId, MediaAsset, MediaAvailabilityKind,
    MediaAvailabilityStatus, MediaCacheClearResult, MediaCacheFamily, MediaCacheInventory,
    MediaError, MediaEvent, MediaKind, MediaSourceFingerprint, MonitorProof, MonitorProofMetadata,
    ParamValue, Rational, RgbaImage, SceneChange, SceneStatus, SilenceSpan, SilenceStatus,
    TimelineSceneChange, TimelineSilenceSpan, Title, Track, TrackId, TrackKind, TranscriptWord,
    VisualAssetResult,
};
use serde_json::json;
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
    time::Instant,
};

#[derive(Default)]
struct NoopMedia {
    probe_asset: Option<MediaAsset>,
    probe_paths: Mutex<Vec<PathBuf>>,
    cache_inventory: Option<MediaCacheInventory>,
    clear_cache_result: Option<MediaCacheClearResult>,
    availability_by_asset: BTreeMap<AssetId, MediaAvailabilityStatus>,
    availability_override: Option<Arc<Mutex<BTreeMap<AssetId, MediaAvailabilityStatus>>>>,
    transcript: Option<Arc<AssetTranscript>>,
    beat_statuses: BTreeMap<AssetId, BeatStatus>,
    scene_statuses: BTreeMap<AssetId, SceneStatus>,
    timeline_beats: Vec<TimelineBeat>,
    timeline_beat_error: Option<String>,
    beat_requests: Mutex<Vec<AssetId>>,
    scene_requests: Mutex<Vec<AssetId>>,
    thumbnail_frames: BTreeMap<TimeCode, RgbaImage>,
    candidate_thumbnail_frames: BTreeMap<TimeCode, RgbaImage>,
    candidate_effect_id: Option<EffectId>,
    /// Distinguish the candidate document by a stored primary parameter
    /// value rather than by node identity, so a proposal that corrects an
    /// existing node in place is still recognised as the candidate.
    candidate_primary_exposure_milli_stops: Option<i64>,
    render_error: Option<String>,
    proof_error: Option<MediaError>,
    /// Render a *different* frame whenever a node in the document carries
    /// `bypass = 1`, so a bypass path that is not actually lossless can be
    /// exercised (CC4 §8).
    bypass_leaks_pixel: Option<u8>,
    /// CC5 §4.1: the coverage raster this double answers a matte proof
    /// with. `None` keeps the trait's real `NotImplemented` default, which
    /// is what the production engine still returns, so both branches of
    /// every CC5 agent path are exercised.
    matte_coverage: Option<RgbaImage>,
}

impl Playback for NoopMedia {
    fn set_document(&self, _doc: Arc<Document>) {}

    fn request_frame(&self, _t: TimeCode) {}

    fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, FrameTexture)> {
        crossbeam_channel::never()
    }

    fn events(&self) -> crossbeam_channel::Receiver<MediaEvent> {
        crossbeam_channel::never()
    }

    fn play(&self, _from: TimeCode) {}

    fn pause(&self) {}

    fn seek(&self, _to: TimeCode) {}

    fn position(&self) -> TimeCode {
        TimeCode::ZERO
    }

    fn output_peaks(&self) -> [f32; 2] {
        [0.0; 2]
    }
}

impl Analysis for NoopMedia {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError> {
        self.probe_paths.lock().unwrap().push(path.to_path_buf());
        self.probe_asset
            .clone()
            .map_or(Err(MediaError::NotImplemented), |mut asset| {
                asset.path = path.to_path_buf();
                Ok(asset)
            })
    }

    fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
        if let Some(status) = self
            .availability_override
            .as_ref()
            .and_then(|statuses| statuses.lock().unwrap().get(&asset.id).cloned())
        {
            return status;
        }
        self.availability_by_asset
            .get(&asset.id)
            .cloned()
            .unwrap_or_else(|| MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OnlineUnverified,
                observed_fingerprint: None,
                reason: Some("test backend does not inspect filesystem state".to_owned()),
            })
    }

    fn cache_inventory(&self) -> MediaCacheInventory {
        self.cache_inventory.clone().unwrap_or(MediaCacheInventory {
            families: Vec::new(),
        })
    }

    fn clear_cache(&self, family: MediaCacheFamily) -> Result<MediaCacheClearResult, MediaError> {
        if family == MediaCacheFamily::GeneratedProxy {
            return Err(MediaError::NotImplemented);
        }
        self.clear_cache_result
            .clone()
            .ok_or(MediaError::NotImplemented)
    }

    fn request_transcription(&self, _asset: MediaAsset) {}

    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus {
        self.transcript
            .as_ref()
            .map_or(TranscriptStatus::NotRequested, |transcript| {
                if transcript.asset == asset.id {
                    TranscriptStatus::Ready(Arc::clone(transcript))
                } else {
                    TranscriptStatus::NotRequested
                }
            })
    }

    fn timeline_transcript(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        Ok(Vec::new())
    }

    fn request_silence_detection(&self, _asset: MediaAsset) {}

    fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
        SilenceStatus::NotRequested
    }

    fn timeline_silences(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        Ok(Vec::new())
    }

    fn request_scene_detection(&self, asset: MediaAsset) {
        self.scene_requests.lock().unwrap().push(asset.id);
    }

    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus {
        self.scene_statuses
            .get(&asset.id)
            .cloned()
            .unwrap_or(SceneStatus::NotRequested)
    }

    fn timeline_scene_changes(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        Ok(Vec::new())
    }

    fn request_beat_detection(&self, asset: MediaAsset) {
        self.beat_requests.lock().unwrap().push(asset.id);
    }

    fn beat_status(&self, asset: &MediaAsset) -> BeatStatus {
        self.beat_statuses
            .get(&asset.id)
            .cloned()
            .unwrap_or(BeatStatus::NotRequested)
    }

    fn timeline_beats(
        &self,
        _document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_strength_basis_points: u16,
    ) -> Result<Vec<TimelineBeat>, MediaError> {
        if let Some(error) = &self.timeline_beat_error {
            return Err(MediaError::Backend(error.clone()));
        }
        Ok(self
            .timeline_beats
            .iter()
            .copied()
            .filter(|beat| beat.strength_basis_points >= minimum_strength_basis_points)
            .filter(|beat| {
                range.as_ref().is_none_or(|range| {
                    beat.project_frame >= range.start && beat.project_frame < range.end
                })
            })
            .collect())
    }

    fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
        Err(MediaError::NotImplemented)
    }

    fn thumbnail_for_document(
        &self,
        document: Arc<Document>,
        t: TimeCode,
        _max_w: u32,
    ) -> Result<RgbaImage, MediaError> {
        if let Some(error) = &self.render_error {
            return Err(MediaError::Backend(error.clone()));
        }
        let candidate = document
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .any(|clip| {
                clip.effects.iter().any(|effect| {
                    effect.name == "primary_correction"
                        && self.candidate_effect_id.is_none_or(|id| effect.id == id)
                        && self
                            .candidate_primary_exposure_milli_stops
                            .is_none_or(|value| {
                                effect.parameters.get("exposure_milli_stops")
                                    == Some(&ParamValue::Integer(value))
                            })
                })
            });
        if candidate && let Some(image) = self.candidate_thumbnail_frames.get(&t) {
            return Ok(image.clone());
        }
        if let Some(pixel) = self.bypass_leaks_pixel
            && document
                .tracks
                .iter()
                .flat_map(|track| track.clips.iter())
                .any(|clip| {
                    clip.effects.iter().any(|effect| {
                        effect
                            .parameters
                            .get(kinewright_core::COLOR_NODE_BYPASS_PARAMETER)
                            == Some(&ParamValue::Integer(1))
                    })
                })
        {
            return Ok(RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![pixel; 16],
            });
        }
        if let Some(image) = self.thumbnail_frames.get(&t) {
            return Ok(image.clone());
        }
        Ok(RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0; 16],
        })
    }

    fn matte_proof_for_document(
        &self,
        _document: Arc<Document>,
        _at: TimeCode,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<kinewright_core::MatteProof, MediaError> {
        let Some(coverage) = self.matte_coverage.clone() else {
            return Err(MediaError::NotImplemented);
        };
        Ok(kinewright_core::MatteProof {
            metadata: kinewright_core::MatteProofMetadata {
                render: MonitorProofMetadata::test_double(),
                clip,
                effect,
                node_kind: "color_wheels".to_owned(),
                coverage_encoding: kinewright_core::MATTE_COVERAGE_ENCODING.to_owned(),
                coverage_scale: kinewright_core::MATTE_COVERAGE_SCALE,
                raster_aspect_millionths: 1_777_778,
                matte_enabled: true,
                window_count: 1,
                qualifier_enabled: false,
            },
            coverage,
        })
    }

    fn monitor_proof_for_document(
        &self,
        document: Arc<Document>,
        t: TimeCode,
    ) -> Result<MonitorProof, MediaError> {
        if let Some(error) = &self.proof_error {
            return Err(error.clone());
        }
        let image = self.thumbnail_for_document(Arc::clone(&document), t, u32::MAX)?;
        let (width, height) = document.resolution;
        if width == 0 || height == 0 {
            return Err(MediaError::Backend(
                "test proof document has an empty raster".to_owned(),
            ));
        }
        let proof_image = if image.width == width && image.height == height {
            image
        } else {
            let Some(source) = image::RgbaImage::from_raw(image.width, image.height, image.pixels)
            else {
                return Err(MediaError::Backend(
                    "test proof image has invalid RGBA dimensions".to_owned(),
                ));
            };
            let resized = image::imageops::resize(
                &source,
                width,
                height,
                image::imageops::FilterType::Nearest,
            );
            RgbaImage {
                width,
                height,
                pixels: resized.into_raw(),
            }
        };
        Ok(MonitorProof {
            image: proof_image,
            metadata: MonitorProofMetadata::test_double(),
        })
    }

    fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
        false
    }

    fn request_thumbnail(
        &self,
        _asset: MediaAsset,
        _source_at: TimeCode,
        _max_width: u32,
        _request_generation: u64,
    ) -> bool {
        false
    }

    fn visual_asset_results(&self) -> crossbeam_channel::Receiver<VisualAssetResult> {
        crossbeam_channel::never()
    }
}

fn fixture() -> (Core, Arc<dyn Playback>, Arc<dyn Analysis>) {
    let asset = MediaAsset {
        id: AssetId(1),
        path: PathBuf::from("fixture.mp4"),
        name: "fixture".to_owned(),
        duration: TimeCode(60),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..TimeCode(60),
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
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(60),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
    };
    let media = Arc::new(NoopMedia::default());
    (Core::spawn(document).unwrap(), media.clone(), media)
}

fn verified_source_analysis() -> Arc<dyn Analysis> {
    Arc::new(NoopMedia {
        availability_by_asset: BTreeMap::from([(
            AssetId(1),
            MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OnlineVerified,
                observed_fingerprint: None,
                reason: Some("verified source fixture".to_owned()),
            },
        )]),
        ..NoopMedia::default()
    })
}

fn source_program_service_with_second_video_track() -> KinewrightMcp {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    document.tracks.push(Track {
        id: TrackId(9),
        kind: TrackKind::Video,
        sync_lock: false,
        // Keep a lower id after the overwrite so Core's post-clear id
        // allocator reuses the removed highest id (99). An id-only
        // before/after diff would mistake the valid replacement for the
        // old clip and fail to report the routed result.
        clips: vec![
            Clip {
                id: ClipId(99),
                asset: AssetId(1),
                source_range: TimeCode::ZERO..TimeCode(20),
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode(10),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            },
            Clip {
                id: ClipId(98),
                asset: AssetId(1),
                source_range: TimeCode(20)..TimeCode(30),
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode(40),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            },
        ],
    });
    KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        playback,
        verified_source_analysis(),
        ConfirmationBroker::default(),
    )
}

fn source_program_av_service() -> KinewrightMcp {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    document.media_pool[0].kind = MediaKind::AudioVideo;
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: Vec::new(),
    });
    KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        playback,
        verified_source_analysis(),
        ConfirmationBroker::default(),
    )
}

fn fingerprint(byte_len: u64, nibble: char) -> MediaSourceFingerprint {
    MediaSourceFingerprint {
        content_sha256: Some(std::iter::repeat_n(nibble, 64).collect()),
        byte_len: Some(byte_len),
    }
}

fn relink_probe_asset(source_fingerprint: MediaSourceFingerprint) -> MediaAsset {
    MediaAsset {
        id: AssetId(99),
        path: PathBuf::from("probe-placeholder.mp4"),
        name: "replacement".to_owned(),
        duration: TimeCode(60),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((320, 180)),
        source_fingerprint,
        color_description: kinewright_core::ColorDescription::default(),
    }
}

fn relink_service(
    current_fingerprint: MediaSourceFingerprint,
    candidate_fingerprint: MediaSourceFingerprint,
) -> (KinewrightMcp, Core, Arc<NoopMedia>) {
    relink_service_with_probe(
        current_fingerprint,
        relink_probe_asset(candidate_fingerprint),
    )
}

fn relink_service_with_probe(
    current_fingerprint: MediaSourceFingerprint,
    probe_asset: MediaAsset,
) -> (KinewrightMcp, Core, Arc<NoopMedia>) {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    document.media_pool[0].source_fingerprint = current_fingerprint;
    let core = Core::spawn(document).unwrap();
    let media = Arc::new(NoopMedia {
        probe_asset: Some(probe_asset),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        media.clone(),
        ConfirmationBroker::default(),
    );
    (service, core, media)
}

fn relink_request(
    expected_revision: u64,
    asset_id: u64,
    path: &str,
    allow_unverified_source: bool,
) -> CallToolRequestParams {
    CallToolRequestParams::new("relink_media").with_arguments(
        json!({
            "expected_revision": expected_revision,
            "asset_id": asset_id,
            "path": path,
            "allow_unverified_source": allow_unverified_source,
        })
        .as_object()
        .unwrap()
        .clone(),
    )
}

fn montage_analysis(status: BeatStatus) -> Arc<NoopMedia> {
    Arc::new(NoopMedia {
        beat_statuses: BTreeMap::from([(AssetId(9), status)]),
        timeline_beats: vec![TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 9_000,
            estimated_bpm_milli: 120_000,
        }],
        ..NoopMedia::default()
    })
}

fn montage_fixture(status: BeatStatus) -> (Core, Arc<NoopMedia>) {
    let fps = Rational::new(30, 1).unwrap();
    let video_asset = |id| MediaAsset {
        id: AssetId(id),
        path: PathBuf::from(format!("montage-{id}.mp4")),
        name: format!("montage-{id}"),
        duration: TimeCode(180),
        fps,
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let music = MediaAsset {
        id: AssetId(9),
        path: PathBuf::from("montage-music.mp4"),
        name: "montage music".to_owned(),
        duration: TimeCode(180),
        fps,
        kind: MediaKind::AudioVideo,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let document = Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(90),
                    asset: music.id,
                    source_range: TimeCode::ZERO..TimeCode(120),
                    content: ClipContent::Media,
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
        media_pool: vec![video_asset(1), video_asset(2), music],
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode(120),
        ..Document::default()
    };
    let analysis = montage_analysis(status);
    (Core::spawn(document).unwrap(), analysis)
}

fn music_structure_fixture(
    status: BeatStatus,
    timeline_beats: Vec<TimelineBeat>,
) -> (Core, Arc<NoopMedia>) {
    let fps = Rational::new(30, 1).unwrap();
    let video = MediaAsset {
        id: AssetId(1),
        path: PathBuf::from("music-structure-video.mp4"),
        name: "music structure video".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let music = MediaAsset {
        id: AssetId(9),
        path: PathBuf::from("music-structure-audio.wav"),
        name: "music structure audio".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let media_clip = |id, asset, track_start| Clip {
        id: ClipId(id),
        asset: AssetId(asset),
        source_range: TimeCode::ZERO..TimeCode(120),
        content: ClipContent::Media,
        timeline_start: TimeCode(track_start),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    };
    let document = Document {
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![media_clip(1, 1, 0)],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![media_clip(90, 9, 0)],
            },
        ],
        media_pool: vec![video, music],
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode(120),
        ..Document::default()
    };
    let analysis = Arc::new(NoopMedia {
        beat_statuses: BTreeMap::from([(AssetId(9), status)]),
        timeline_beats,
        ..NoopMedia::default()
    });
    (Core::spawn(document).unwrap(), analysis)
}

fn end_anchored_music_fit_fixture() -> (Core, Arc<NoopMedia>) {
    let source_fps = Rational::new(30, 1).unwrap();
    let music = MediaAsset {
        id: AssetId(9),
        path: PathBuf::from("end-anchored-music.wav"),
        name: "end anchored music".to_owned(),
        duration: TimeCode(6_170),
        fps: source_fps,
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let document = Document {
        tracks: vec![Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: vec![music],
        fps: Rational::new(25, 1).unwrap(),
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
        color_context: kinewright_core::ColorContext::default(),
        ..Document::default()
    };
    let analysis = Arc::new(NoopMedia {
        beat_statuses: BTreeMap::from([(
            AssetId(9),
            BeatStatus::Ready(Arc::new(AssetBeats {
                asset: AssetId(9),
                content_sha256: "end-anchored-music-test".to_owned(),
                source_fps,
                source_frames: TimeCode(6_170),
                estimated_bpm_milli: 120_000,
                beats: vec![
                    BeatMarker {
                        source_frame: TimeCode(5_160),
                        strength_basis_points: 5_638,
                    },
                    BeatMarker {
                        source_frame: TimeCode(5_161),
                        strength_basis_points: 10_000,
                    },
                ],
            })),
        )]),
        ..NoopMedia::default()
    });
    (Core::spawn(document).unwrap(), analysis)
}

fn ready_music_structure_status() -> BeatStatus {
    BeatStatus::Ready(Arc::new(AssetBeats {
        asset: AssetId(9),
        content_sha256: "music-structure-test".to_owned(),
        source_fps: Rational::new(30, 1).unwrap(),
        source_frames: TimeCode(120),
        estimated_bpm_milli: 120_000,
        beats: vec![
            BeatMarker {
                source_frame: TimeCode::ZERO,
                strength_basis_points: 9_000,
            },
            BeatMarker {
                source_frame: TimeCode(30),
                strength_basis_points: 5_000,
            },
            BeatMarker {
                source_frame: TimeCode(60),
                strength_basis_points: 8_000,
            },
        ],
    }))
}

fn ready_montage_status() -> BeatStatus {
    BeatStatus::Ready(Arc::new(AssetBeats {
        asset: AssetId(9),
        content_sha256: "montage-test".to_owned(),
        source_fps: Rational::new(30, 1).unwrap(),
        source_frames: TimeCode(180),
        estimated_bpm_milli: 120_000,
        beats: vec![BeatMarker {
            source_frame: TimeCode(30),
            strength_basis_points: 9_000,
        }],
    }))
}

fn montage_plan_args() -> BeatMontagePlanArgs {
    BeatMontagePlanArgs {
        target_track_id: TrackId(1),
        music_asset_id: AssetId(9),
        timeline_range: TranscriptRangeArgs {
            start: TimeCode::ZERO,
            end: TimeCode(60),
        },
        selects: vec![
            BeatMontageSelectArgs {
                asset_id: AssetId(1),
                source_range: TranscriptRangeArgs {
                    start: TimeCode(10),
                    end: TimeCode(100),
                },
            },
            BeatMontageSelectArgs {
                asset_id: AssetId(2),
                source_range: TranscriptRangeArgs {
                    start: TimeCode(20),
                    end: TimeCode(110),
                },
            },
        ],
        cut_anchor_frames: None,
        anchor_repair: None,
        min_strength: None,
        minimum_shot_frames: None,
        maximum_shot_frames: None,
        cadence: None,
        mode: ThreePointMode::Overwrite,
    }
}

fn commit_prepared_plan(
    service: &KinewrightMcp,
    plan: &serde_json::Value,
    revision: TimelineRevision,
) -> CallToolResult {
    service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                serde_json::json!({
                    "plan_id": plan["prepared_edit_plan"]["plan_id"],
                    "expected_revision": revision,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap()
}

fn delete_request() -> CallToolRequestParams {
    CallToolRequestParams::new("delete_clip").with_arguments(
        json!({"expected_revision": 0, "clip": 1})
            .as_object()
            .unwrap()
            .clone(),
    )
}

fn plan_request(operations: serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams::new("apply_edit_plan").with_arguments(serde_json::Map::from_iter([
        ("expected_revision".to_owned(), json!(0)),
        ("operations".to_owned(), operations),
    ]))
}

fn wait_for_request(broker: &ConfirmationBroker) -> ConfirmationRequest {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(request) = broker.pending_requests().into_iter().next() {
            return request;
        }
        assert!(
            Instant::now() < deadline,
            "confirmation request was not published"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn invoke_in_background(
    service: KinewrightMcp,
    request: CallToolRequestParams,
) -> crossbeam_channel::Receiver<Result<CallToolResult, McpError>> {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    thread::spawn(move || {
        let _ = sender.send(service.call_blocking(request));
    });
    receiver
}
