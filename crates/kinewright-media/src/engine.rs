use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kinewright_core::{
    Analysis, AnalysisKind, AssetId, AssetTranscript, AudioLoudness, BeatStatus, Document, Export,
    ExportCancellation, ExportSettings, FrameTexture, LutAvailabilityKind, LutAvailabilityStatus,
    MediaAsset, MediaAvailabilityKind, MediaAvailabilityStatus, MediaCacheClearResult,
    MediaCacheFamily, MediaCacheFamilyStatus, MediaCacheInventory, MediaError, MediaEvent,
    MonitorProof, Playback, PlaybackState, ProgressSink, Rational, RgbaImage, SceneStatus,
    SilenceStatus, TimeCode, TimelineBeat, TimelineSceneChange, TimelineSilenceSpan,
    TimelineTranscriptWord, TranscriptStatus, VisualAssetResult, export_lut_preflight_with,
};

use crate::{
    analysis::VisualAssetService,
    audio::{AudioRuntime, MeterState, decode_audio_range},
    clock::samples_to_frame,
    compositor::GpuContext,
    decode::probe_path,
    derived::{DerivedAnalysisConfig, DerivedAnalysisService},
    derived_cache::CacheStats,
    loudness::measure_loudness,
    lut::CubeLut,
    lut_store::LutLibrary,
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
    sha256::source_fingerprint,
    transcript::{TranscriptService, default_data_dir},
};

const WORKER_TICK: Duration = Duration::from_millis(5);

struct SharedClock {
    position_samples: Arc<AtomicU64>,
    sample_rate: Arc<AtomicU32>,
    project_fps_num: AtomicU32,
    project_fps_den: AtomicU32,
    fallback_frame: AtomicI64,
}

impl SharedClock {
    fn new() -> Self {
        Self {
            position_samples: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
            project_fps_num: AtomicU32::new(30),
            project_fps_den: AtomicU32::new(1),
            fallback_frame: AtomicI64::new(0),
        }
    }

    fn set_fps(&self, fps: Rational) {
        self.project_fps_num
            .store(fps.numerator(), Ordering::Release);
        self.project_fps_den
            .store(fps.denominator(), Ordering::Release);
    }

    fn set_frame(&self, frame: TimeCode) {
        self.fallback_frame.store(frame.0.max(0), Ordering::Release);
        self.sample_rate.store(0, Ordering::Release);
    }

    fn position(&self) -> TimeCode {
        let sample_rate = self.sample_rate.load(Ordering::Acquire);
        if sample_rate == 0 {
            return TimeCode(self.fallback_frame.load(Ordering::Acquire));
        }
        let fps = Rational::new(
            self.project_fps_num.load(Ordering::Acquire),
            self.project_fps_den.load(Ordering::Acquire),
        )
        .unwrap_or_default();
        samples_to_frame(
            self.position_samples.load(Ordering::Acquire),
            sample_rate,
            fps,
        )
    }
}

/// How many verified lattices the engine's published table retains (CC4 2.4).
///
/// The table is content-addressed and shared by every open project, so it must
/// be bounded: nothing else would ever drop an entry belonging to a project
/// that has been closed. 512 entries is far more than any realistic set of
/// simultaneously open projects needs — the four built-ins never occupy a slot
/// at all — and eviction is never a correctness question, because a project
/// republishes its whole library whenever it is focused, saved, or edits its
/// asset table.
///
/// The retained bytes are already bounded by the process parse cache: an entry
/// here is an `Arc` clone of a lattice that cache owns, so the table adds a
/// hash key and a pointer per entry, not a second copy of the samples.
const PUBLISHED_LATTICE_LIMIT: usize = 512;

/// Every verified LUT lattice any open project has published, keyed by the
/// SHA-256 of its bytes (CC4 2.4).
///
/// This replaces a single published `LutLibrary`. A library is keyed by
/// `LutAssetId`, and ids restart at 1 in every project, so one library slot
/// meant whichever project published last answered every other project's
/// look requests: project A's export could resolve project B's lattice while
/// B held focus. The content hash cannot alias that way, so publication became
/// a *merge* into this table and every document-taking render path rebuilds a
/// document-local library from its own `Document.lut_assets`.
#[derive(Debug, Default)]
struct PublishedLattices {
    by_sha256: HashMap<String, Arc<CubeLut>>,
    /// Publication order, most recent first. This is what bounds `by_sha256`,
    /// and it is publication recency rather than lookup recency: the focused
    /// project republishes its whole library on every focus switch, save, and
    /// asset-table edit, so the looks actually in use keep returning to the
    /// front without a write lock on the render path.
    recent: VecDeque<String>,
}

impl PublishedLattices {
    /// Record one verified lattice, promoting it to most recently published.
    fn publish(&mut self, sha256: &str, lut: &Arc<CubeLut>) {
        if self
            .by_sha256
            .insert(sha256.to_owned(), Arc::clone(lut))
            .is_some()
            && let Some(index) = self.recent.iter().position(|key| key == sha256)
        {
            self.recent.remove(index);
        }
        self.recent.push_front(sha256.to_owned());
        while self.recent.len() > PUBLISHED_LATTICE_LIMIT {
            if let Some(evicted) = self.recent.pop_back() {
                self.by_sha256.remove(&evicted);
            }
        }
    }

    /// Merge every entry of one document's verified library into the table.
    fn merge(&mut self, library: &LutLibrary) {
        for (_, sha256, lut) in library.entries() {
            self.publish(sha256, lut);
        }
    }
}

/// Bind one document's LUT assets to already-published lattices (CC4 2.4).
///
/// Every render path that takes a `Document` goes through here, so a look is
/// resolved by the content hash the *document being rendered* records rather
/// than by an id some other project happens to share.
///
/// # Errors
///
/// Returns `missing_lut_asset:` naming each id and recorded hash when a node
/// that could evaluate on some frame references an asset the table does not
/// hold. Assets no evaluable node references, and nodes that are bypassed or
/// `mix = 0` on every frame, never block: CC4 2.3 blocks on the looks a frame
/// could actually need, not on the whole asset table.
fn bind_document_luts(
    document: &Document,
    published: &HashMap<String, Arc<CubeLut>>,
) -> Result<Arc<LutLibrary>, MediaError> {
    let (library, unbound) = LutLibrary::from_document_assets(&document.lut_assets, published);
    if unbound.is_empty() {
        return Ok(Arc::new(library));
    }
    let report = export_lut_preflight_with(document, &|asset| LutAvailabilityStatus {
        kind: if unbound.contains(&asset.id) {
            LutAvailabilityKind::Missing
        } else {
            LutAvailabilityKind::Verified
        },
        observed_sha256: None,
        reason: None,
        path: None,
    });
    if report.issues.is_empty() {
        return Ok(Arc::new(library));
    }
    let details = report
        .issues
        .iter()
        .map(|issue| format!("{} ({})", issue.lut_asset, issue.sha256))
        .collect::<Vec<_>>()
        .join(", ");
    Err(MediaError::Backend(format!(
        "missing_lut_asset: no published lattice matches LUT asset(s) {details}; restore or \
         re-import the asset and let the project republish its library before rendering"
    )))
}

enum Control {
    SetDocument(Arc<Document>),
    /// CC4 2.4: the engine's content-addressed lattice table gained entries,
    /// so the playback worker rebinds its document-local library. The library
    /// itself never crosses this channel: it is rebuilt from the worker's own
    /// document, which is the only document the worker may resolve looks for.
    LutLatticesPublished,
    Play(TimeCode),
    Pause,
    Thumbnail {
        document: Option<Arc<Document>>,
        at: TimeCode,
        max_width: u32,
        reply: Sender<Result<RgbaImage, MediaError>>,
    },
    PreviewCacheStats {
        reply: Sender<CacheStats>,
    },
    ClearPreviewCache {
        reply: Sender<CacheStats>,
    },
}

pub struct FfmpegMediaEngine {
    control_tx: Sender<Control>,
    frames_rx: Receiver<(TimeCode, FrameTexture)>,
    events_rx: Receiver<MediaEvent>,
    requested: Arc<RequestedPositions>,
    clock: Arc<SharedClock>,
    meter: Arc<MeterState>,
    next_asset_id: AtomicU64,
    data_dir: PathBuf,
    gpu: GpuContext,
    export_document: Arc<RwLock<Arc<Document>>>,
    /// Every verified lattice any open project has published, by content hash.
    ///
    /// Shared with the playback worker, so the worker, a proof, and an export
    /// all resolve looks out of the same table — and each of them binds it to
    /// *its own* document's assets, so no project can be served another
    /// project's look (CC4 2.4).
    lut_lattices: Arc<RwLock<PublishedLattices>>,
    transcripts: TranscriptService,
    visual_assets: VisualAssetService,
    derived_analysis: DerivedAnalysisService,
}

impl FfmpegMediaEngine {
    /// Start the media engine with the default cache directory and GPU selection.
    ///
    /// # Errors
    ///
    /// Returns a media error when `FFmpeg`, GPU, audio, or worker initialization fails.
    pub fn new() -> Result<Self, MediaError> {
        Self::new_with_data_dir(default_data_dir())
    }

    /// Start the media engine with an explicit cache directory.
    ///
    /// # Errors
    ///
    /// Returns a media error when `FFmpeg`, GPU, audio, or worker initialization fails.
    pub fn new_with_data_dir(data_dir: PathBuf) -> Result<Self, MediaError> {
        static GPU: OnceLock<Result<GpuContext, MediaError>> = OnceLock::new();
        let gpu = GPU
            .get_or_init(|| GpuContext::headless(false).or_else(|_| GpuContext::headless(true)))
            .clone()?;
        Self::new_with_gpu_and_data_dir(gpu, data_dir)
    }

    /// Start the media engine with an existing GPU context and default cache directory.
    ///
    /// # Errors
    ///
    /// Returns a media error when `FFmpeg`, audio, or worker initialization fails.
    pub fn new_with_gpu(gpu: GpuContext) -> Result<Self, MediaError> {
        Self::new_with_gpu_and_data_dir(gpu, default_data_dir())
    }

    /// Start the media engine with an existing GPU context and explicit cache directory.
    ///
    /// # Errors
    ///
    /// Returns a media error when `FFmpeg`, audio, or worker initialization fails.
    pub fn new_with_gpu_and_data_dir(
        gpu: GpuContext,
        data_dir: PathBuf,
    ) -> Result<Self, MediaError> {
        Self::new_with_gpu_data_dir_and_analysis_config(
            gpu,
            data_dir,
            DerivedAnalysisConfig::default(),
        )
    }

    /// Start the media engine with explicit GPU, cache, and derived-analysis configuration.
    ///
    /// # Errors
    ///
    /// Returns a media error when `FFmpeg`, audio, or worker initialization fails.
    pub fn new_with_gpu_data_dir_and_analysis_config(
        gpu: GpuContext,
        data_dir: PathBuf,
        analysis_config: DerivedAnalysisConfig,
    ) -> Result<Self, MediaError> {
        crate::initialize_ffmpeg()?;
        let data_dir_for_self = data_dir.clone();
        let (control_tx, control_rx) = unbounded();
        let (frames_tx, frames_rx) = bounded(2);
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let worker_clock = Arc::clone(&clock);
        let meter = Arc::new(MeterState::default());
        let worker_meter = Arc::clone(&meter);
        let frames_drop_rx = frames_rx.clone();
        let events_drop_rx = events_rx.clone();
        // Scrub positions use shared atomics so rapid mouse movement is coalesced
        // without an unbounded command backlog.
        let requested = Arc::new(RequestedPositions::default());
        let worker_requested = Arc::clone(&requested);
        let worker_gpu = gpu.clone();
        let lut_lattices = Arc::new(RwLock::new(PublishedLattices::default()));
        let worker_lut_lattices = Arc::clone(&lut_lattices);
        thread::Builder::new()
            .name("kinewright-media".to_owned())
            .spawn(move || {
                Worker::new(
                    WorkerChannels {
                        control_rx,
                        frames_tx,
                        frames_drop_rx,
                        events_tx,
                        events_drop_rx,
                    },
                    worker_clock,
                    worker_meter,
                    worker_requested,
                    worker_gpu,
                    worker_lut_lattices,
                )
                .run();
            })
            .map_err(|error| MediaError::Backend(error.to_string()))?;

        let visual_assets = VisualAssetService::new(&data_dir)?;
        let derived_analysis = DerivedAnalysisService::new(&data_dir, analysis_config)?;
        Ok(Self {
            control_tx,
            frames_rx,
            events_rx,
            requested,
            clock,
            meter,
            next_asset_id: AtomicU64::new(1),
            data_dir: data_dir_for_self,
            gpu,
            export_document: Arc::new(RwLock::new(Arc::new(Document::default()))),
            lut_lattices,
            transcripts: TranscriptService::new(data_dir)?,
            visual_assets,
            derived_analysis,
        })
    }

    /// Register a trusted transcript for this engine session after verifying
    /// that its content identity, frame rate, asset id, and word ranges match
    /// the referenced media. This is the ingestion seam for reproducible
    /// speaker-labelled sidecars; it does not rewrite the media or silently
    /// persist third-party annotations as Whisper output.
    ///
    /// # Errors
    ///
    /// Returns a media error when the transcript does not describe `asset` or
    /// contains invalid, unsorted source-frame ranges.
    pub fn register_transcript(
        &self,
        asset: &MediaAsset,
        transcript: AssetTranscript,
    ) -> Result<(), MediaError> {
        if transcript.asset != asset.id {
            return Err(MediaError::Backend(format!(
                "transcript asset {} does not match media asset {}",
                transcript.asset, asset.id
            )));
        }
        if transcript.source_fps != asset.fps {
            return Err(MediaError::Backend(format!(
                "transcript frame rate {}/{} does not match media frame rate {}/{}",
                transcript.source_fps.numerator(),
                transcript.source_fps.denominator(),
                asset.fps.numerator(),
                asset.fps.denominator()
            )));
        }
        let content_sha256 = crate::sha256_file(&asset.path)?;
        if transcript.content_sha256 != content_sha256 {
            return Err(MediaError::Backend(format!(
                "transcript content hash {} does not match media hash {content_sha256}",
                transcript.content_sha256
            )));
        }
        let mut previous_start = TimeCode::ZERO;
        for (index, word) in transcript.words.iter().enumerate() {
            if word.source_start < TimeCode::ZERO
                || word.source_end <= word.source_start
                || word.source_end > asset.duration
                || (index > 0 && word.source_start < previous_start)
            {
                return Err(MediaError::Backend(format!(
                    "transcript word {index} has invalid or unsorted source range {}..{} for media duration {}",
                    word.source_start.0, word.source_end.0, asset.duration.0
                )));
            }
            previous_start = word.source_start;
        }
        self.transcripts.register(&asset.path, transcript);
        Ok(())
    }

    /// Publish one project's verified CC4 lattices for preview, proof, and
    /// export.
    ///
    /// This is the media-side counterpart of `Playback::set_document`: the
    /// layer that owns a project's LUT store builds the library from that
    /// project's `Document.lut_assets` and publishes it here, and the renderer
    /// never opens a LUT file itself. It is an inherent method rather than a
    /// `Playback` trait method because `LutLibrary` holds verified media-crate
    /// sample data that Core, which is I/O-free, cannot name.
    ///
    /// Publication **merges by content hash** rather than replacing a single
    /// slot. One engine serves every open project, and `LutAssetId`s restart
    /// at 1 in each of them, so a single slot meant a proof or a queued export
    /// belonging to project A could resolve `LutAssetId(1)` to whatever
    /// project B had published while B held focus. Merging into a
    /// content-addressed table removes the shared name: each render binds the
    /// table to the hashes its *own* document records
    /// ([`LutLibrary::from_document_assets`]).
    ///
    /// A merge therefore never invalidates another project's looks, and
    /// publishing the same library twice is idempotent. The table is bounded
    /// at [`PUBLISHED_LATTICE_LIMIT`] entries in publication order; an evicted
    /// lattice is republished by the project that owns it, and until it is, an
    /// active node referencing it fails the render with `missing_lut_asset`
    /// rather than rendering without the look.
    ///
    /// Built-in looks need no publication at all: they are generated in this
    /// binary and resolve straight from the pinned bake table.
    // The `Arc` is taken by value rather than by reference because this is the
    // application's publication seam and the caller hands over a clone it has
    // no further use for; narrowing it to `&Arc` would only move the clone to
    // every call site.
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_lut_library(&self, library: Arc<LutLibrary>) {
        if let Ok(mut published) = self.lut_lattices.write() {
            published.merge(&library);
        }
        let _ = self.control_tx.send(Control::LutLatticesPublished);
    }

    /// Bind one document's LUT assets to the published table, for a
    /// caller-thread renderer.
    ///
    /// # Errors
    ///
    /// Returns `missing_lut_asset:` when a node that could evaluate references
    /// an asset no project has published, and a backend error when the table
    /// lock is poisoned.
    fn document_lut_library(&self, document: &Document) -> Result<Arc<LutLibrary>, MediaError> {
        let published = self.lut_lattices.read().map_err(|_| {
            MediaError::Backend("the published LUT lattice table lock was poisoned".to_owned())
        })?;
        bind_document_luts(document, &published.by_sha256)
    }

    fn preview_cache_command(&self, clear: bool) -> Result<CacheStats, MediaError> {
        let (reply, response) = bounded(1);
        let control = if clear {
            Control::ClearPreviewCache { reply }
        } else {
            Control::PreviewCacheStats { reply }
        };
        self.control_tx
            .send(control)
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?;
        response
            .recv()
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))
    }

    fn cache_root(&self, family: &str) -> PathBuf {
        self.data_dir.join(family).join("v1")
    }

    fn cache_family_status(
        family: MediaCacheFamily,
        root: Option<PathBuf>,
        supported: bool,
        may_repopulate: bool,
        stats: Result<CacheStats, MediaError>,
        note: Option<String>,
    ) -> MediaCacheFamilyStatus {
        match stats {
            Ok(stats) => MediaCacheFamilyStatus {
                family,
                supported,
                root,
                file_count: stats.file_count,
                bytes: stats.bytes,
                may_repopulate,
                note,
            },
            Err(error) => MediaCacheFamilyStatus {
                family,
                supported,
                root,
                file_count: 0,
                bytes: 0,
                may_repopulate,
                note: Some(format!(
                    "cache inventory unavailable: {error}{}",
                    note.map_or_else(String::new, |note| format!("; {note}"))
                )),
            },
        }
    }
}

#[derive(Default)]
struct RequestedPositions {
    frame: AtomicI64,
    frame_sequence: AtomicU64,
    seek: AtomicI64,
    seek_sequence: AtomicU64,
}

impl Playback for FfmpegMediaEngine {
    fn set_document(&self, doc: Arc<Document>) {
        self.clock.set_fps(doc.fps);
        let next_id = doc
            .media_pool
            .iter()
            .map(|asset| asset.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.next_asset_id.fetch_max(next_id, Ordering::Relaxed);
        if let Ok(mut export_document) = self.export_document.write() {
            *export_document = Arc::clone(&doc);
        }
        let _ = self.control_tx.send(Control::SetDocument(doc));
    }

    fn request_frame(&self, at: TimeCode) {
        self.requested.frame.store(at.0.max(0), Ordering::Relaxed);
        self.requested
            .frame_sequence
            .fetch_add(1, Ordering::Release);
    }

    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
        self.frames_rx.clone()
    }

    fn events(&self) -> Receiver<MediaEvent> {
        self.events_rx.clone()
    }

    fn play(&self, from: TimeCode) {
        self.meter.clear();
        self.clock.set_frame(from);
        let _ = self.control_tx.send(Control::Play(from));
    }

    fn pause(&self) {
        self.meter.clear();
        let _ = self.control_tx.send(Control::Pause);
    }

    fn seek(&self, to: TimeCode) {
        self.clock.set_frame(to);
        self.requested.seek.store(to.0.max(0), Ordering::Relaxed);
        self.requested.seek_sequence.fetch_add(1, Ordering::Release);
        self.request_frame(to);
    }

    fn position(&self) -> TimeCode {
        self.clock.position()
    }

    fn output_peaks(&self) -> [f32; 2] {
        self.meter.peaks()
    }
}

impl Analysis for FfmpegMediaEngine {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError> {
        let id = AssetId(self.next_asset_id.fetch_add(1, Ordering::Relaxed));
        probe_path(path, id)
    }

    fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
        media_availability(asset)
    }

    fn request_transcription(&self, asset: MediaAsset) {
        self.transcripts.request(asset, None);
    }

    fn request_transcription_with_language(&self, asset: MediaAsset, language: Option<&str>) {
        self.transcripts.request(asset, language.map(str::to_owned));
    }

    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus {
        self.transcripts.status(&asset.path)
    }

    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        self.transcripts.timeline_words(document, range)
    }

    fn request_silence_detection(&self, asset: MediaAsset) {
        self.derived_analysis.request_silences(asset);
    }

    fn silence_status(&self, asset: &MediaAsset) -> SilenceStatus {
        self.derived_analysis.silence_status(&asset.path)
    }

    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        self.derived_analysis
            .timeline_silences(document, range, minimum_source_frames)
    }

    fn request_scene_detection(&self, asset: MediaAsset) {
        self.derived_analysis.request_scenes(asset);
    }

    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus {
        self.derived_analysis.scene_status(&asset.path)
    }

    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        self.derived_analysis
            .timeline_scenes(document, range, minimum_confidence_basis_points)
    }

    fn asset_loudness(&self, asset: &MediaAsset) -> Result<AudioLoudness, MediaError> {
        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            48_000,
            2,
            &ExportCancellation::default(),
        )?;
        measure_loudness(&samples, 48_000, 2)
    }

    fn timeline_loudness(&self, document: &Document) -> Result<AudioLoudness, MediaError> {
        let settings = ExportSettings {
            fps: document.fps,
            resolution: document.resolution,
            delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 1,
            audio_bitrate: 1,
            cancellation: ExportCancellation::default(),
        };
        let samples = crate::export::mix_audio(document, &settings)?;
        measure_loudness(&samples, 48_000, 2)
    }

    fn request_beat_detection(&self, asset: MediaAsset) {
        self.derived_analysis.request_beats(asset);
    }

    fn beat_status(&self, asset: &MediaAsset) -> BeatStatus {
        self.derived_analysis.beat_status(&asset.path)
    }

    fn timeline_beats(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_strength_basis_points: u16,
    ) -> Result<Vec<TimelineBeat>, MediaError> {
        self.derived_analysis
            .timeline_beats(document, range, minimum_strength_basis_points)
    }

    fn cancel_analysis(&self, asset: &MediaAsset, kind: AnalysisKind) -> bool {
        if kind == AnalysisKind::Transcript {
            self.transcripts.cancel(&asset.path)
        } else {
            self.derived_analysis.cancel(&asset.path, kind)
        }
    }

    fn thumbnail_at(&self, at: TimeCode, max_width: u32) -> Result<RgbaImage, MediaError> {
        let (reply, response) = bounded(1);
        self.control_tx
            .send(Control::Thumbnail {
                document: None,
                at,
                max_width,
                reply,
            })
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?;
        response
            .recv()
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?
    }

    fn thumbnail_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        max_width: u32,
    ) -> Result<RgbaImage, MediaError> {
        let (reply, response) = bounded(1);
        self.control_tx
            .send(Control::Thumbnail {
                document: Some(document),
                at,
                max_width,
                reply,
            })
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?;
        response
            .recv()
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?
    }

    fn monitor_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
    ) -> Result<MonitorProof, MediaError> {
        // Proof rendering is deliberately isolated from the playback worker:
        // a revision-bound before/after pair must not evict or reuse its
        // proxy cache, alter transport state, or let one branch's asset ids
        // collide with another branch.
        let mut renderer = FrameRenderer::new(self.gpu.clone());
        // CC4 2.4: bound to THIS document's asset hashes, not to whatever
        // library was published most recently, so a branch server's proof
        // cannot resolve the focused project's looks.
        renderer.set_lut_library(self.document_lut_library(&document)?);
        let resolution = document.resolution;
        // Bind the scale once so the render and the claim it produces cannot
        // drift apart.
        let scale = RenderScale::FullResolution;
        let frame = renderer.render(&document, at, resolution, scale, DecodeStrategy::Seek)?;
        Ok(MonitorProof {
            image: RgbaImage {
                width: frame.width,
                height: frame.height,
                pixels: (*frame.rgba).clone(),
            },
            // CC1 5: a proof may only claim the full raster when it was
            // requested at full scale AND came back at the document raster.
            metadata: self.gpu.monitor_proof_metadata_for(
                scale,
                (frame.width, frame.height),
                resolution,
            ),
        })
    }

    fn request_waveform(&self, asset: MediaAsset, request_generation: u64) -> bool {
        self.visual_assets
            .request_waveform(asset, request_generation)
    }

    fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
    ) -> bool {
        self.visual_assets
            .request_thumbnail(asset, source_at, max_width, request_generation)
    }

    fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
        self.visual_assets.results()
    }

    fn cache_inventory(&self) -> MediaCacheInventory {
        let preview_note = Some(
            "preview_memory is an ephemeral in-memory decode cache; it is not a disk proxy"
                .to_owned(),
        );
        let visual_root = self.cache_root("visual-assets");
        let derived_root = self.cache_root("derived-analysis");
        let proxy_root = self.cache_root("generated-proxy");
        MediaCacheInventory {
            families: vec![
                Self::cache_family_status(
                    MediaCacheFamily::PreviewMemory,
                    None,
                    true,
                    true,
                    self.preview_cache_command(false),
                    preview_note,
                ),
                Self::cache_family_status(
                    MediaCacheFamily::VisualAssets,
                    Some(visual_root),
                    true,
                    true,
                    self.visual_assets.cache_stats(),
                    Some("background visual workers may repopulate this family".to_owned()),
                ),
                Self::cache_family_status(
                    MediaCacheFamily::DerivedAnalysis,
                    Some(derived_root),
                    true,
                    true,
                    self.derived_analysis.cache_stats(),
                    Some("background analysis workers may repopulate this family".to_owned()),
                ),
                Self::cache_family_status(
                    MediaCacheFamily::Transcripts,
                    Some(self.transcripts.cache_root().to_path_buf()),
                    true,
                    true,
                    self.transcripts.cache_stats(),
                    Some("background transcription workers may repopulate this family".to_owned()),
                ),
                Self::cache_family_status(
                    MediaCacheFamily::GeneratedProxy,
                    Some(proxy_root),
                    false,
                    false,
                    crate::derived_cache::inventory_cache_root(&self.cache_root("generated-proxy")),
                    Some("generated disk proxies are not supported in M41".to_owned()),
                ),
            ],
        }
    }

    fn clear_cache(&self, family: MediaCacheFamily) -> Result<MediaCacheClearResult, MediaError> {
        let (supported, may_repopulate, stats, note) = match family {
            MediaCacheFamily::PreviewMemory => (
                true,
                true,
                self.preview_cache_command(true)?,
                Some(
                    "preview playback or scrubbing can repopulate this in-memory cache".to_owned(),
                ),
            ),
            MediaCacheFamily::VisualAssets => (
                true,
                true,
                self.visual_assets.clear_cache()?,
                Some("queued visual work may repopulate this family".to_owned()),
            ),
            MediaCacheFamily::DerivedAnalysis => (
                true,
                true,
                self.derived_analysis.clear_cache()?,
                Some("queued analysis work may repopulate this family".to_owned()),
            ),
            MediaCacheFamily::Transcripts => (
                true,
                true,
                self.transcripts.clear_cache()?,
                Some("queued transcription work may repopulate this family".to_owned()),
            ),
            MediaCacheFamily::GeneratedProxy => (
                false,
                false,
                CacheStats::default(),
                Some("generated disk proxies are not supported in M41".to_owned()),
            ),
        };
        Ok(MediaCacheClearResult {
            family,
            supported,
            removed_file_count: stats.file_count,
            removed_bytes: stats.bytes,
            may_repopulate,
            note,
        })
    }
}

fn media_availability(asset: &MediaAsset) -> MediaAvailabilityStatus {
    let metadata = match fs::metadata(&asset.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OfflineMissing,
                observed_fingerprint: None,
                reason: Some(format!("media path is missing: {}", asset.path.display())),
            };
        }
        Err(error) => {
            return MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::Unreadable,
                observed_fingerprint: None,
                reason: Some(format!(
                    "could not inspect media path {}: {error}",
                    asset.path.display()
                )),
            };
        }
    };
    if !metadata.is_file() {
        return MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OfflineMissing,
            observed_fingerprint: None,
            reason: Some(format!(
                "media path is missing or not a regular file: {}",
                asset.path.display()
            )),
        };
    }
    let observed_fingerprint = match source_fingerprint(&asset.path) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            return MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::Unreadable,
                observed_fingerprint: None,
                reason: Some(error.to_string()),
            };
        }
    };
    if !asset.source_fingerprint.is_verified() {
        return MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OnlineUnverified,
            observed_fingerprint: Some(observed_fingerprint),
            reason: Some("source identity is not persisted for this asset".to_owned()),
        };
    }
    if asset.source_fingerprint == observed_fingerprint {
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OnlineVerified,
            observed_fingerprint: Some(observed_fingerprint),
            reason: None,
        }
    } else {
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::Changed,
            observed_fingerprint: Some(observed_fingerprint),
            reason: Some(
                "the file at this path no longer matches the imported source fingerprint"
                    .to_owned(),
            ),
        }
    }
}

impl Export for FfmpegMediaEngine {
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        let document = self
            .export_document
            .read()
            .map_err(|_| MediaError::Backend("export document lock was poisoned".to_owned()))?
            .clone();
        let library = self.document_lut_library(&document)?;
        crate::export::export_document_with_luts(
            &document,
            out,
            &settings,
            &progress,
            self.gpu.clone(),
            library,
        )
    }

    fn export_document(
        &self,
        document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        // CC4 2.4: an export queue outlives focus, so the library is bound to
        // the immutable document being encoded rather than to whichever
        // project published last.
        let library = self.document_lut_library(&document)?;
        crate::export::export_document_with_luts(
            &document,
            out,
            &settings,
            &progress,
            self.gpu.clone(),
            library,
        )
    }
}

struct Worker {
    control_rx: Receiver<Control>,
    frames_tx: Sender<(TimeCode, FrameTexture)>,
    frames_drop_rx: Receiver<(TimeCode, FrameTexture)>,
    events_tx: Sender<MediaEvent>,
    events_drop_rx: Receiver<MediaEvent>,
    clock: Arc<SharedClock>,
    meter: Arc<MeterState>,
    requested: Arc<RequestedPositions>,
    handled_frame_sequence: u64,
    handled_seek_sequence: u64,
    document: Arc<Document>,
    renderer: FrameRenderer,
    /// The engine's content-addressed lattice table, shared with the
    /// caller-thread proof and export paths (CC4 2.4).
    lut_lattices: Arc<RwLock<PublishedLattices>>,
    /// The document-local library bound from [`Worker::document`]. Rebuilt
    /// whenever the document changes or the table gains entries, never
    /// received ready-made, so the worker cannot resolve a look belonging to a
    /// project it is not previewing.
    lut_library: Arc<LutLibrary>,
    audio: Option<AudioRuntime>,
    playing: bool,
    last_position: Option<TimeCode>,
}

struct WorkerChannels {
    control_rx: Receiver<Control>,
    frames_tx: Sender<(TimeCode, FrameTexture)>,
    frames_drop_rx: Receiver<(TimeCode, FrameTexture)>,
    events_tx: Sender<MediaEvent>,
    events_drop_rx: Receiver<MediaEvent>,
}

impl Worker {
    fn new(
        channels: WorkerChannels,
        clock: Arc<SharedClock>,
        meter: Arc<MeterState>,
        requested: Arc<RequestedPositions>,
        gpu: GpuContext,
        lut_lattices: Arc<RwLock<PublishedLattices>>,
    ) -> Self {
        Self {
            control_rx: channels.control_rx,
            frames_tx: channels.frames_tx,
            frames_drop_rx: channels.frames_drop_rx,
            events_tx: channels.events_tx,
            events_drop_rx: channels.events_drop_rx,
            clock,
            meter,
            requested,
            handled_frame_sequence: 0,
            handled_seek_sequence: 0,
            document: Arc::new(Document::default()),
            renderer: FrameRenderer::new(gpu),
            lut_lattices,
            lut_library: Arc::new(LutLibrary::default()),
            audio: None,
            playing: false,
            last_position: None,
        }
    }

    fn run(mut self) {
        loop {
            match self.control_rx.recv_timeout(WORKER_TICK) {
                Ok(control) => self.handle_control(control),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
            while let Ok(control) = self.control_rx.try_recv() {
                self.handle_control(control);
            }
            self.handle_coalesced_requests();
            self.tick();
        }
    }

    fn handle_control(&mut self, control: Control) {
        match control {
            Control::SetDocument(doc) => self.set_document(&doc),
            Control::LutLatticesPublished => self.rebind_lut_library(),
            Control::Play(from) => self.start_playback(from),
            Control::Pause => self.pause(),
            Control::Thumbnail {
                document,
                at,
                max_width,
                reply,
            } => {
                let scale = RenderScale::Proxy { max_width };
                // A thumbnail may be requested for a document the worker is
                // not previewing - a branch, a media-bin entry - so its looks
                // are bound from that document's own asset hashes and the
                // preview binding is restored afterwards (CC4 2.4).
                let requested = document.unwrap_or_else(|| Arc::clone(&self.document));
                self.renderer
                    .set_lut_library(self.bound_lut_library(&requested));
                let resolution = scale.output_resolution(requested.resolution);
                let result = self
                    .renderer
                    .render(&requested, at, resolution, scale, DecodeStrategy::Seek)
                    .map(|frame| RgbaImage {
                        width: frame.width,
                        height: frame.height,
                        pixels: (*frame.rgba).clone(),
                    });
                self.renderer.set_lut_library(Arc::clone(&self.lut_library));
                let _ = reply.send(result);
            }
            Control::PreviewCacheStats { reply } => {
                let _ = reply.send(self.renderer.cache_stats());
            }
            Control::ClearPreviewCache { reply } => {
                let _ = reply.send(self.renderer.clear());
            }
        }
    }

    /// Bind one document's LUT assets to the published table.
    ///
    /// The preview path reports a missing look through the render itself -
    /// the compositor fails with `missing_lut_asset` naming the node - so an
    /// unbound asset is simply absent here rather than an error the worker has
    /// nowhere to send.
    fn bound_lut_library(&self, document: &Document) -> Arc<LutLibrary> {
        let Ok(published) = self.lut_lattices.read() else {
            return Arc::new(LutLibrary::default());
        };
        let (library, _unbound) =
            LutLibrary::from_document_assets(&document.lut_assets, &published.by_sha256);
        Arc::new(library)
    }

    /// Rebuild the preview library from the worker's own document.
    ///
    /// Rebinding hands the compositor the same `Arc<CubeLut>` values the table
    /// holds, so an unchanged look keeps its atlas-cache identity and steady
    /// playback does not re-upload the atlas.
    fn rebind_lut_library(&mut self) {
        let document = Arc::clone(&self.document);
        self.lut_library = self.bound_lut_library(&document);
        self.renderer.set_lut_library(Arc::clone(&self.lut_library));
    }

    fn set_document(&mut self, doc: &Document) {
        self.pause();
        self.document = Arc::new(doc.clone());
        // CC4 2.4: the incoming document may belong to a different project, so
        // its looks are rebound before the first frame is presented.
        self.rebind_lut_library();
        self.renderer.clear();
        self.clock.set_fps(doc.fps);
        self.clock.set_frame(TimeCode::ZERO);
        self.last_position = None;
        self.present(TimeCode::ZERO);
    }

    fn handle_coalesced_requests(&mut self) {
        let seek_sequence = self.requested.seek_sequence.load(Ordering::Acquire);
        if seek_sequence != self.handled_seek_sequence {
            self.handled_seek_sequence = seek_sequence;
            let at = TimeCode(self.requested.seek.load(Ordering::Relaxed));
            if self.playing {
                self.start_playback(at);
            } else {
                self.clock.set_frame(at);
                self.emit(MediaEvent::Position(at));
            }
        }

        let frame_sequence = self.requested.frame_sequence.load(Ordering::Acquire);
        if frame_sequence != self.handled_frame_sequence {
            self.handled_frame_sequence = frame_sequence;
            self.present(TimeCode(self.requested.frame.load(Ordering::Relaxed)));
        }
    }

    fn start_playback(&mut self, from: TimeCode) {
        self.audio = None;
        self.meter.clear();
        if self.document.duration <= TimeCode::ZERO {
            self.fail(MediaError::Backend("the timeline is empty".to_owned()));
            return;
        }
        let from = TimeCode(from.0.clamp(0, self.document.duration.0.saturating_sub(1)));
        self.clock.set_frame(from);
        match self.audio_for_position(from).and_then(|runtime| {
            runtime.play()?;
            Ok(runtime)
        }) {
            Ok(runtime) => {
                self.audio = Some(runtime);
                self.playing = true;
                self.emit(MediaEvent::PlaybackStateChanged(PlaybackState::Playing));
                self.present(from);
            }
            Err(error) => self.fail(error),
        }
    }

    fn pause(&mut self) {
        if let Some(audio) = &self.audio
            && let Err(error) = audio.pause()
        {
            self.emit(MediaEvent::Error(error));
        }
        let position = self.clock.position();
        self.clock
            .fallback_frame
            .store(position.0, Ordering::Release);
        self.audio = None;
        self.meter.clear();
        self.clock.sample_rate.store(0, Ordering::Release);
        if self.playing {
            self.playing = false;
            self.emit(MediaEvent::PlaybackStateChanged(PlaybackState::Paused));
        }
    }

    fn tick(&mut self) {
        if !self.playing {
            return;
        }
        let audio_error = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.error_flag.swap(false, Ordering::AcqRel));
        if audio_error {
            self.fail(MediaError::Backend("audio output stream failed".to_owned()));
            return;
        }
        if let Some(audio) = &mut self.audio
            && let Err(error) = audio.fill()
        {
            self.fail(error);
            return;
        }
        let position = self.clock.position();
        if position >= self.document.duration {
            let end = self.document.duration;
            self.clock.fallback_frame.store(end.0, Ordering::Release);
            self.pause();
            self.emit(MediaEvent::Position(end));
            return;
        }
        if self.last_position != Some(position) {
            self.last_position = Some(position);
            self.emit(MediaEvent::Position(position));
            self.present(position);
        }
    }

    fn present(&mut self, project_at: TimeCode) {
        let document = Arc::clone(&self.document);
        let scale = RenderScale::Proxy {
            max_width: PREVIEW_MAX_WIDTH,
        };
        let resolution = scale.output_resolution(document.resolution);
        let strategy = if self.playing {
            DecodeStrategy::Sequential
        } else {
            DecodeStrategy::Seek
        };
        let frame = match self
            .renderer
            .render(&document, project_at, resolution, scale, strategy)
        {
            Ok(frame) => frame,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        send_latest(&self.frames_tx, &self.frames_drop_rx, (project_at, frame));
    }

    fn audio_for_position(&self, project_at: TimeCode) -> Result<AudioRuntime, MediaError> {
        AudioRuntime::open(
            &self.document,
            project_at,
            &self.clock.position_samples,
            &self.clock.sample_rate,
            Arc::clone(&self.meter),
        )
    }

    fn fail(&mut self, error: MediaError) {
        self.pause();
        self.emit(MediaEvent::Error(error));
    }

    fn emit(&self, event: MediaEvent) {
        send_latest(&self.events_tx, &self.events_drop_rx, event);
    }
}

fn send_latest<T: Send>(sender: &Sender<T>, drop_receiver: &Receiver<T>, value: T) {
    match sender.try_send(value) {
        Ok(()) | Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
        Err(crossbeam_channel::TrySendError::Full(value)) => {
            let _ = drop_receiver.try_recv();
            let _ = sender.try_send(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kinewright_core::{
        Clip, ClipContent, ClipId, Effect, EffectId, LutAsset, LutAssetId, LutAssetKind,
        LutAssetSource, MediaKind, MediaSourceFingerprint, ParamValue, Title, Track, TrackId,
        TrackKind,
    };

    use super::*;
    use crate::{
        lut::parse_cube_lut,
        sha256::{sha256_bytes, source_fingerprint},
        test_support::TempDirectory,
    };

    fn asset(path: PathBuf, fingerprint: MediaSourceFingerprint) -> MediaAsset {
        MediaAsset {
            id: AssetId(1),
            path,
            name: "fixture".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
            source_fingerprint: fingerprint,
            color_description: kinewright_core::ColorDescription::default(),
        }
    }

    /// An `S = 2` `.cube` document whose green channel is scaled, so two of
    /// them are different *looks* rather than two spellings of one.
    fn scaled_cube_text(green: f32) -> String {
        use std::fmt::Write as _;
        let mut text = String::from("LUT_3D_SIZE 2\n");
        for blue in [0.0_f32, 1.0] {
            for g in [0.0_f32, 1.0] {
                for red in [0.0_f32, 1.0] {
                    let _ = writeln!(text, "{red:.6} {:.6} {blue:.6}", g * green);
                }
            }
        }
        text
    }

    /// The parsed lattice and the record a project would hold for it, under an
    /// id the caller chooses. Nothing here touches a store: the point is that
    /// binding happens on the hash, so the bytes only ever need to be hashed.
    fn published_pair(green: f32, id: u64) -> (String, Arc<CubeLut>, LutAsset) {
        let text = scaled_cube_text(green);
        let sha256 = sha256_bytes(text.as_bytes());
        let lut = Arc::new(parse_cube_lut(&text).expect("the fixture lattice parses"));
        let (domain_min_millionths, domain_max_millionths) = lut.domain_millionths();
        let asset = LutAsset {
            id: LutAssetId(id),
            sha256: sha256.clone(),
            title: format!("Green {green}"),
            kind: LutAssetKind::Cube3d,
            size: lut.size,
            byte_len: text.len() as u64,
            domain_min_millionths,
            domain_max_millionths,
            source: LutAssetSource::Imported {
                source_path: format!("/fixtures/green-{green}.cube"),
            },
        };
        (sha256, lut, asset)
    }

    /// A one-clip title timeline carrying one active `creative_look` bound to
    /// `asset`, plus that asset in the project table.
    fn look_document(asset: LutAsset) -> Document {
        let mut look = Effect {
            id: EffectId(1),
            name: "creative_look".to_owned(),
            parameters: std::collections::BTreeMap::new(),
            keyframes: std::collections::BTreeMap::new(),
        };
        look.parameters.insert(
            "lut_asset_id".to_owned(),
            ParamValue::Integer(i64::try_from(asset.id.0).unwrap()),
        );
        Document {
            resolution: (64, 36),
            duration: TimeCode(4),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId::default(),
                    source_range: TimeCode(0)..TimeCode(4),
                    content: ClipContent::Title(Title {
                        text: "CC4".to_owned(),
                        ..Title::default()
                    }),
                    timeline_start: TimeCode::ZERO,
                    effects: vec![look],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            lut_assets: vec![asset],
            ..Document::default()
        }
    }

    #[test]
    fn two_projects_sharing_one_asset_id_bind_to_their_own_lattices() {
        // CC4 2.4.  `LutAssetId(1)` names a different look in every project,
        // and one engine serves them all.  Publication merges by content hash
        // and each document rebinds from its own records, so publishing B
        // after A cannot make A's node resolve to B's lattice - which is
        // exactly what a single published-library slot did.
        let (alpha_sha, alpha_lut, alpha_asset) = published_pair(0.25, 1);
        let (beta_sha, beta_lut, beta_asset) = published_pair(0.75, 1);
        assert_eq!(alpha_asset.id, beta_asset.id, "the ids collide on purpose");
        assert_ne!(alpha_sha, beta_sha, "the looks are genuinely different");

        let mut table = PublishedLattices::default();
        table.publish(&alpha_sha, &alpha_lut);
        table.publish(&beta_sha, &beta_lut);

        let alpha_document = look_document(alpha_asset);
        let beta_document = look_document(beta_asset);

        let alpha_library = bind_document_luts(&alpha_document, &table.by_sha256)
            .expect("project A's look is published");
        let beta_library = bind_document_luts(&beta_document, &table.by_sha256)
            .expect("project B's look is published");

        let bound_alpha = alpha_library.get(LutAssetId(1)).expect("A binds its look");
        let bound_beta = beta_library.get(LutAssetId(1)).expect("B binds its look");
        assert!(Arc::ptr_eq(bound_alpha, &alpha_lut));
        assert!(Arc::ptr_eq(bound_beta, &beta_lut));
        assert_ne!(
            bound_alpha.rgba, bound_beta.rgba,
            "the two projects must not resolve to the same samples"
        );

        // Order of publication is irrelevant, which is what makes focus
        // switching unable to alias: republishing A last changes nothing
        // about B.
        table.publish(&alpha_sha, &alpha_lut);
        let beta_again = bind_document_luts(&beta_document, &table.by_sha256)
            .expect("republishing A leaves B bound");
        assert!(Arc::ptr_eq(
            beta_again.get(LutAssetId(1)).expect("B still binds"),
            &beta_lut
        ));
    }

    #[test]
    fn an_unpublished_look_blocks_the_render_with_a_typed_failure() {
        // CC4 2.3: a look a frame could need and the engine cannot resolve
        // fails the render, naming the id and the hash that was looked for.
        let (sha256, _lut, asset) = published_pair(0.5, 1);
        let document = look_document(asset);

        let error =
            bind_document_luts(&document, &HashMap::new()).expect_err("nothing was ever published");
        let MediaError::Backend(message) = error else {
            panic!("LUT failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("missing_lut_asset: "),
            "message should lead with the code: {message}"
        );
        assert!(
            message.contains(&sha256),
            "message should name the hash: {message}"
        );
        assert!(
            message.contains('1'),
            "message should name the id: {message}"
        );
    }

    #[test]
    fn an_asset_no_evaluable_node_needs_never_blocks() {
        // CC4 2.3 blocks on the looks a frame could actually need.  An asset
        // in the project table that no node references is not one of them, so
        // an unpublished spare must not fail an otherwise deliverable export.
        let (_sha, lut, bound) = published_pair(0.25, 1);
        let (_spare_sha, _spare_lut, spare) = published_pair(0.75, 2);
        let mut document = look_document(bound.clone());
        document.lut_assets.push(spare);

        let mut table = PublishedLattices::default();
        table.publish(&bound.sha256, &lut);

        let library = bind_document_luts(&document, &table.by_sha256)
            .expect("an unreferenced spare does not block");
        assert!(library.get(LutAssetId(1)).is_some());
        assert!(
            library.get(LutAssetId(2)).is_none(),
            "the spare is still withheld, it simply blocks nothing"
        );
    }

    #[test]
    fn a_hand_edited_record_is_withheld_even_when_its_lattice_is_published() {
        // The table is keyed by hash, so a second project can publish the very
        // bytes a first project misdescribes.  The record still loses: the
        // bytes are the authority (CC4 2.1).
        let (sha256, lut, mut asset) = published_pair(0.5, 1);
        asset.size = lut.size + 1;
        let document = look_document(asset);

        let mut table = PublishedLattices::default();
        table.publish(&sha256, &lut);

        let error = bind_document_luts(&document, &table.by_sha256)
            .expect_err("a record that disagrees with the bytes resolves to nothing");
        let MediaError::Backend(message) = error else {
            panic!("LUT failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("missing_lut_asset: "),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn the_published_table_is_bounded_in_publication_order() {
        // The table outlives the projects that filled it - nothing else would
        // ever drop an entry for a closed project - so it is bounded, most
        // recently published first.
        let mut table = PublishedLattices::default();
        // A green scale no later fixture rounds onto, so the first entry is
        // genuinely evicted rather than accidentally republished.
        let first = published_pair(0.000_5, 1);
        table.publish(&first.0, &first.1);

        let mut later = Vec::new();
        for index in 0..PUBLISHED_LATTICE_LIMIT {
            #[allow(clippy::cast_precision_loss)]
            let entry = published_pair(0.001 * (index as f32 + 1.0), 1);
            assert_ne!(entry.0, first.0, "fixture hashes must stay distinct");
            table.publish(&entry.0, &entry.1);
            later.push(entry);
        }
        assert_eq!(table.by_sha256.len(), PUBLISHED_LATTICE_LIMIT);
        assert_eq!(table.recent.len(), PUBLISHED_LATTICE_LIMIT);
        assert!(
            !table.by_sha256.contains_key(&first.0),
            "the oldest publication is the one evicted"
        );
        assert!(
            table.by_sha256.contains_key(&later[later.len() - 1].0),
            "the newest publication survives"
        );

        // Republishing is idempotent and promotes rather than duplicating.
        let head = &later[later.len() - 1];
        table.publish(&head.0, &head.1);
        assert_eq!(table.by_sha256.len(), PUBLISHED_LATTICE_LIMIT);
        assert_eq!(table.recent.len(), PUBLISHED_LATTICE_LIMIT);
        assert_eq!(
            table.recent.front().map(String::as_str),
            Some(head.0.as_str())
        );
    }

    #[test]
    fn a_built_in_look_resolves_without_ever_being_published() {
        // Built-ins are generated in this binary, so they come from the pinned
        // bake table and need no publication at all - and a recorded hash this
        // build does not bake is withheld rather than silently re-baked.
        let look = crate::builtin_looks::BuiltinLook::Warm;
        let baked = look.cached_bake();
        let (domain_min_millionths, domain_max_millionths) = baked.domain_millionths();
        let asset = LutAsset {
            id: LutAssetId(1),
            sha256: look.sha256().to_owned(),
            title: "Warm".to_owned(),
            kind: LutAssetKind::Cube3d,
            size: baked.size,
            byte_len: 0,
            domain_min_millionths,
            domain_max_millionths,
            source: LutAssetSource::Builtin {
                name: look.name().to_owned(),
            },
        };
        let document = look_document(asset.clone());
        let library = bind_document_luts(&document, &HashMap::new())
            .expect("a built-in needs no publication");
        assert!(Arc::ptr_eq(
            library.get(LutAssetId(1)).expect("the built-in binds"),
            &baked
        ));

        let mut stale = asset;
        stale.sha256 = "0".repeat(64);
        let error = bind_document_luts(&look_document(stale), &HashMap::new())
            .expect_err("a hash this build does not bake is withheld");
        let MediaError::Backend(message) = error else {
            panic!("LUT failures cross as MediaError::Backend");
        };
        assert!(
            message.starts_with("missing_lut_asset: "),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn availability_distinguishes_verified_unverified_changed_missing_and_non_regular() {
        let directory = TempDirectory::new("availability");
        let path = directory.path("source.bin");
        fs::write(&path, b"original").unwrap();
        let fingerprint = source_fingerprint(&path).unwrap();

        assert_eq!(
            media_availability(&asset(path.clone(), fingerprint.clone())).kind,
            MediaAvailabilityKind::OnlineVerified
        );
        assert_eq!(
            media_availability(&asset(path.clone(), MediaSourceFingerprint::unknown())).kind,
            MediaAvailabilityKind::OnlineUnverified
        );

        fs::write(&path, b"changed source").unwrap();
        let changed = media_availability(&asset(path.clone(), fingerprint));
        assert_eq!(changed.kind, MediaAvailabilityKind::Changed);
        assert!(changed.observed_fingerprint.is_some());

        let missing = asset(
            directory.path("missing.bin"),
            MediaSourceFingerprint::unknown(),
        );
        assert_eq!(
            media_availability(&missing).kind,
            MediaAvailabilityKind::OfflineMissing
        );

        let directory_asset = asset(
            directory.root().to_path_buf(),
            MediaSourceFingerprint::unknown(),
        );
        assert_eq!(
            media_availability(&directory_asset).kind,
            MediaAvailabilityKind::OfflineMissing
        );
    }
}
