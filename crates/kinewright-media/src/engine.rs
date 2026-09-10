use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kinewright_core::{
    Analysis, AnalysisKind, AssetId, AssetTranscript, AudioLoudness, AudioQcReport, AudioQcRequest,
    AudioRepairReport, AudioRepairRequest, BeatStatus, ClipId, DeliveryAudioVerification,
    DeliveryVerification, DeliveryVerificationRequest, Document, EffectId, Export,
    ExportCancellation, ExportReport, ExportSettings, FrameTexture, LiveAudioChange,
    LoudnessSnapshot, LoudnessTarget, LutAvailabilityKind, LutAvailabilityStatus,
    MATTE_COVERAGE_ENCODING, MATTE_COVERAGE_SCALE, MatteParams, MatteProof, MatteProofError,
    MatteProofMetadata, MediaAsset, MediaAvailabilityKind, MediaAvailabilityStatus,
    MediaCacheClearResult, MediaCacheFamily, MediaCacheFamilyStatus, MediaCacheInventory,
    MediaError, MediaEvent, MediaKind, MixLevelReport, MixLevelRequest, MixNoiseProfileRequest,
    MixPeaks, MixSpectrumReport, MixSpectrumRequest, MixWindowLevelReport, MixWindowRequest,
    MonitorProof, NoiseProfileReport, Playback, PlaybackState, ProgressSink, Rational, RgbaImage,
    SceneStatus, SilenceStatus, TimeCode, TimelineBeat, TimelineSceneChange, TimelineSilenceSpan,
    TimelineTranscriptWord, TranscriptStatus, VisualAssetResult, WORKING_PROOF_ENCODING,
    WORKING_PROOF_STAGE, WorkingProof, WorkingProofMetadata, audio_qc_technical_pass,
    delivery_audio_exceptions, export_lut_preflight_with,
};

use crate::{
    analysis::VisualAssetService,
    audio::{AudioDecoder, AudioRuntime, MeterState, MixMeters, decode_audio_range},
    clock::{frame_to_samples, samples_to_frame},
    compositor::GpuContext,
    decode::probe_path,
    derived::{DerivedAnalysisConfig, DerivedAnalysisService},
    derived_cache::CacheStats,
    loudness::{LiveLoudnessMeter, LoudnessMeter},
    lut::CubeLut,
    lut_store::LutLibrary,
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
    sha256::source_fingerprint,
    transcript::{TranscriptService, default_data_dir},
};

const WORKER_TICK: Duration = Duration::from_millis(5);

/// AU3 §5.3: the delivery audio measurement lane. Every loudness figure this
/// engine publishes is measured at 48 kHz stereo, whatever the file carries.
const AUDIO_MEASUREMENT_RATE: u32 = 48_000;
const AUDIO_MEASUREMENT_CHANNELS: u16 = 2;

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

/// Locate one clip's colour node, returning the clip's timeline start
/// alongside the *stored* effect.
///
/// The stored effect is returned rather than an evaluated one because the
/// caller needs the clip's timeline start to evaluate it at the requested
/// frame, and evaluating twice at two different frames is exactly the drift
/// CC5 3.2 forbids.
fn locate_color_node(
    document: &Document,
    clip: ClipId,
    effect: EffectId,
) -> Result<(TimeCode, &kinewright_core::Effect), MatteProofError> {
    let target = document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|candidate| candidate.id == clip)
        .ok_or(MatteProofError::EffectNotFound { clip, effect })?;
    let node = target
        .effects
        .iter()
        .find(|candidate| candidate.id == effect)
        .ok_or(MatteProofError::EffectNotFound { clip, effect })?;
    Ok((target.timeline_start, node))
}

/// Reduce a document to the target clip's track and clip.
///
/// CC5 4.1: a matte proof renders the coverage of one node on one clip, so no
/// other layer may composite over it. Removing every other track and clip is
/// stronger than trusting z-order, and it also keeps the proof honest when the
/// target sits under an opaque layer. `lut_assets` is retained so the surviving
/// clip's LUT nodes still bind (CC4 2.4).
fn matte_proof_scratch_document(
    document: &Document,
    clip: ClipId,
    effect: EffectId,
) -> Result<Document, MatteProofError> {
    let mut scratch = document.clone();
    scratch
        .tracks
        .retain(|track| track.clips.iter().any(|candidate| candidate.id == clip));
    for track in &mut scratch.tracks {
        track.clips.retain(|candidate| candidate.id == clip);
    }
    if scratch.tracks.is_empty() {
        return Err(MatteProofError::EffectNotFound { clip, effect });
    }
    Ok(scratch)
}

/// `round(1e6 * W / H)` for the rendered raster.
///
/// The proof records the aspect it rendered at because CC5 2.3's window
/// geometry is aspect-corrected: a coverage image without its aspect cannot be
/// checked against the CPU reference.
#[allow(clippy::cast_possible_truncation)]
fn raster_aspect_millionths(width: u32, height: u32) -> i64 {
    if height == 0 {
        return 0;
    }
    (f64::from(width) * 1_000_000.0 / f64::from(height)).round() as i64
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
    /// AU1 §5.3 / AU4 §3.8 rule 75: a document the running engine can absorb
    /// without pausing, reseeking, or touching video state, carrying **which**
    /// halves to apply beside it.
    ///
    /// One control, not two: the worker's single arm applies mix then shaping
    /// from the **same** `Arc<Document>`, so the two halves cannot interleave
    /// with a re-cue or with each other. Draining two controls in send order
    /// gives nothing observable that this does not.
    UpdateAudio(LiveAudioChange, Arc<Document>),
    Play(TimeCode),
    Pause,
    /// AU3 §3.9: restart the integrated, range, and true-peak measurement at
    /// the position the meter is being fed.
    ResetLoudness,
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
    /// AU1 §4.1: the peak table the worker installs while it is playing, read
    /// by `Playback::mix_peaks`.
    mix_meters: Arc<RwLock<Arc<MixMeters>>>,
    /// AU3 §3.9: the live loudness the worker publishes by audible position,
    /// read by `Playback::loudness`.
    loudness: Arc<LiveLoudness>,
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
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let worker_mix_meters = Arc::clone(&mix_meters);
        let loudness = Arc::new(LiveLoudness::default());
        let worker_loudness = Arc::clone(&loudness);
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
                    worker_mix_meters,
                    worker_loudness,
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
            mix_meters,
            loudness,
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

    /// AU1 §4.1: clear whatever peak table is installed, master included.
    fn clear_mix_meters(&self) {
        if let Ok(meters) = self.mix_meters.read() {
            meters.clear();
        } else {
            self.meter.clear();
        }
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

impl FfmpegMediaEngine {
    /// AU4 §3.8 rule 75: refresh the export document and send the **one**
    /// `Control::UpdateAudio`, so both live-audio halves cross the channel
    /// together with the document they were computed from.
    fn publish_live_audio(&self, kind: LiveAudioChange, doc: Arc<Document>) {
        if let Ok(mut export_document) = self.export_document.write() {
            *export_document = Arc::clone(&doc);
        }
        let _ = self.control_tx.send(Control::UpdateAudio(kind, doc));
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
        self.clear_mix_meters();
        self.clock.set_frame(from);
        let _ = self.control_tx.send(Control::Play(from));
    }

    fn pause(&self) {
        self.clear_mix_meters();
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

    fn mix_peaks(&self) -> MixPeaks {
        self.mix_meters
            .read()
            .map_or_else(|_| MixPeaks::default(), |meters| meters.peaks())
    }

    /// AU2 §5.8: publish a document that differs only in `audio_mix`.
    ///
    /// The export document is refreshed exactly as `set_document` does; the
    /// clock, the next asset id, and the worker's video state are untouched.
    fn update_audio_mix(&self, doc: Arc<Document>) {
        self.publish_live_audio(LiveAudioChange::Mix, doc);
    }

    /// AU4 §3.8 rule 75: publish a document that differs only in clip audio
    /// shaping, through the same one control.
    fn update_clip_shaping(&self, doc: Arc<Document>) {
        self.publish_live_audio(LiveAudioChange::ClipShaping, doc);
    }

    /// AU4 §3.8 rule 75 / §4.4 rule 89: **one** control carries both halves.
    ///
    /// The default would send two, and a `Both` batch would then be two
    /// controls the worker could drain around a re-cue. This override collapses
    /// every kind — `Both` included — into the single
    /// `Control::UpdateAudio(kind, document)` the worker's one arm branches on,
    /// which is what makes "mix then shaping from the **same**
    /// `Arc<Document>`" a guarantee rather than an ordering convention.
    fn update_audio(&self, change: LiveAudioChange, doc: Arc<Document>) {
        self.publish_live_audio(change, doc);
    }

    /// AU3 §3.9: the snapshot the worker last published by audible position.
    fn loudness(&self) -> LoudnessSnapshot {
        self.loudness.load()
    }

    /// AU3 §3.9: restart the measurement (`Control::ResetLoudness`).
    fn reset_loudness(&self) {
        let _ = self.control_tx.send(Control::ResetLoudness);
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
        LoudnessMeter::measure(&samples, 48_000, 2)
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
            loudness_normalization: None,
            cancellation: ExportCancellation::default(),
        };
        let samples = crate::export::mix_audio(document, &settings)?;
        LoudnessMeter::measure(&samples, 48_000, 2)
    }

    fn mix_levels(
        &self,
        document: &Document,
        request: &MixLevelRequest,
    ) -> Result<MixLevelReport, MediaError> {
        crate::export::measure_mix_levels(document, request)
    }

    /// AU2 §5.9: measured through the real mix path, at 48 kHz stereo.
    fn mix_spectrum(
        &self,
        document: &Document,
        request: &MixSpectrumRequest,
    ) -> Result<MixSpectrumReport, MediaError> {
        crate::export::measure_mix_spectrum(document, request)
    }

    /// AU3 §3.10: the post-clamp master over a project range, streamed
    /// through `QcObserver` and judged by core.
    fn audio_qc(
        &self,
        document: &Document,
        request: &AudioQcRequest,
    ) -> Result<AudioQcReport, MediaError> {
        crate::export::measure_audio_qc(document, request)
    }

    /// AU5 §3.7: learned synchronously through the real mix path at 48 kHz, so
    /// the profile describes what the node sees.
    fn mix_noise_profile(
        &self,
        document: &Document,
        request: &MixNoiseProfileRequest,
    ) -> Result<NoiseProfileReport, MediaError> {
        crate::export::measure_mix_noise_profile(document, request)
    }

    /// AU5 §3.8: the short-window RMS accessor AU4's `plan_clip_fades` wanted.
    fn mix_window_levels(
        &self,
        document: &Document,
        request: &MixWindowRequest,
    ) -> Result<MixWindowLevelReport, MediaError> {
        crate::export::measure_mix_window_levels(document, request)
    }

    /// AU5 §3.9: percentile SNR, mains hum excess and click density, over one
    /// render, judged by core's `audio_repair_exceptions`.
    fn audio_repair(
        &self,
        document: &Document,
        request: &AudioRepairRequest,
    ) -> Result<AudioRepairReport, MediaError> {
        crate::export::measure_audio_repair(document, request)
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

    fn matte_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<MatteProof, MediaError> {
        // Resolve the node against the document the caller named, before any
        // rendering, so an absent clip or effect is a typed answer rather than
        // a compositor-shaped one, and so the metadata below describes the
        // node that was actually asked about.
        let (timeline_start, target) = locate_color_node(&document, clip, effect)?;
        let node_kind = target.name.clone();
        // CC5 3.2: matte identity is resolved at the requested frame, after
        // keyframe evaluation, exactly as the renderer resolves the active
        // index it proves.
        let local_at = at.checked_sub(timeline_start).unwrap_or(TimeCode::ZERO);
        let matte = MatteParams::from_effect(&target.evaluated_at(local_at));

        // Proof rendering is isolated from the playback worker for the same
        // reasons as the monitor proof, and the document is additionally
        // reduced to the target clip's track and clip so no other layer can
        // composite over the coverage (CC5 4.1).
        let scratch = matte_proof_scratch_document(&document, clip, effect)?;
        let mut renderer = FrameRenderer::new(self.gpu.clone());
        // CC4 2.4: bind THIS document's lattices; the reduction keeps
        // `lut_assets`, so the target clip's LUT nodes still resolve.
        renderer.set_lut_library(self.document_lut_library(&scratch)?);
        let resolution = scratch.resolution;
        // Bind the scale once so the render and the claim it produces cannot
        // drift apart.
        let scale = RenderScale::FullResolution;
        let raster = renderer.render_matte(
            &scratch,
            at,
            resolution,
            scale,
            DecodeStrategy::Seek,
            clip,
            effect,
        )?;

        let pixel_count = usize::try_from(raster.width)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(raster.height).unwrap_or(usize::MAX));
        if raster.coverage.len() != pixel_count {
            return Err(MediaError::Backend(format!(
                "matte_proof_coverage_size_mismatch: {} coverage bytes for a {}x{} raster",
                raster.coverage.len(),
                raster.width,
                raster.height
            )));
        }
        let mut pixels = Vec::with_capacity(pixel_count.saturating_mul(4));
        for coverage in &raster.coverage {
            // R = G = B = round(255 * m) with an opaque alpha: CC5 writes no
            // alpha, so the proof states full opacity rather than reporting a
            // coverage byte a compositor could mistake for one.
            pixels.extend_from_slice(&[*coverage, *coverage, *coverage, u8::MAX]);
        }
        Ok(MatteProof {
            coverage: RgbaImage {
                width: raster.width,
                height: raster.height,
                pixels,
            },
            metadata: MatteProofMetadata {
                // CC1 5: the full-raster claim is derived from the render that
                // actually happened, not asserted by the caller.
                render: self.gpu.monitor_proof_metadata_for(
                    scale,
                    (raster.width, raster.height),
                    resolution,
                ),
                clip,
                effect,
                node_kind,
                coverage_encoding: MATTE_COVERAGE_ENCODING.to_owned(),
                coverage_scale: MATTE_COVERAGE_SCALE,
                raster_aspect_millionths: raster_aspect_millionths(raster.width, raster.height),
                matte_enabled: matte.is_enabled(),
                window_count: u8::try_from(matte.window_count).unwrap_or(u8::MAX),
                qualifier_enabled: matte.qualifier.is_enabled(),
            },
        })
    }

    fn working_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        // Proof rendering is deliberately isolated from the playback worker,
        // for `monitor_proof_for_document`'s reasons verbatim: a
        // revision-bound before/after pair must not evict or reuse its proxy
        // cache, alter transport state, or let one branch's asset ids collide
        // with another branch.
        let mut renderer = FrameRenderer::new(self.gpu.clone());
        // CC4 2.4: bound to THIS document's asset hashes, not to whatever
        // library was published most recently.
        renderer.set_lut_library(self.document_lut_library(&document)?);
        let resolution = document.resolution;
        // Bind the scale once so the render and the claim it produces cannot
        // drift apart. CC6 2.2: there is no proxy working proof, because this
        // method takes no scale.
        let scale = RenderScale::FullResolution;
        let image =
            renderer.render_working(&document, at, resolution, scale, DecodeStrategy::Seek)?;
        let raster_aspect_millionths = raster_aspect_millionths(image.width, image.height);
        Ok(WorkingProof {
            metadata: WorkingProofMetadata {
                // CC1 5: a proof may only claim the full raster when it was
                // requested at full scale AND came back at the document
                // raster. Derived, never asserted.
                render: self.gpu.monitor_proof_metadata_for(
                    scale,
                    (image.width, image.height),
                    resolution,
                ),
                stage: WORKING_PROOF_STAGE.to_owned(),
                encoding: WORKING_PROOF_ENCODING.to_owned(),
                raster_aspect_millionths,
            },
            image,
        })
    }

    fn verify_delivery_output(
        &self,
        document: Arc<Document>,
        path: &Path,
        settings: &ExportSettings,
        request: DeliveryVerificationRequest,
    ) -> Result<DeliveryVerification, MediaError> {
        crate::verify::verify_delivery_output(
            &self.gpu,
            self.document_lut_library(&document)?,
            &document,
            path,
            settings,
            &request,
        )
    }

    fn verify_delivery_audio(
        &self,
        path: &Path,
        target: Option<LoudnessTarget>,
    ) -> Result<DeliveryAudioVerification, MediaError> {
        // AU3 §5.7: the same bare-path probe `verify_delivery_output` uses,
        // so the written file is described by the production prober rather
        // than by a document that may not name it.
        let asset = probe_path(path, AssetId(0))?;
        if asset.kind == MediaKind::Video {
            return Err(MediaError::Backend(format!(
                "{} has no audio stream to verify",
                path.display()
            )));
        }
        let end_sample = frame_to_samples(asset.duration, AUDIO_MEASUREMENT_RATE, asset.fps);
        let mut decoder = AudioDecoder::open(
            path,
            AUDIO_MEASUREMENT_RATE,
            AUDIO_MEASUREMENT_CHANNELS,
            0,
            end_sample,
        )?;
        // Streamed: `decode_audio_range` would materialise the whole delivery
        // as one `Vec`, and a delivery is as long as the programme.
        let mut meter = LoudnessMeter::new(AUDIO_MEASUREMENT_RATE, AUDIO_MEASUREMENT_CHANNELS)?;
        while let Some(chunk) = decoder.next_chunk()? {
            meter.push(&chunk)?;
        }
        let measured = meter.finish()?;
        let exceptions = delivery_audio_exceptions(&measured, target);
        let technical_pass = audio_qc_technical_pass(&exceptions);
        Ok(DeliveryAudioVerification {
            output_path: path.to_path_buf(),
            measured,
            sample_rate: AUDIO_MEASUREMENT_RATE,
            channels: AUDIO_MEASUREMENT_CHANNELS,
            sample_frames: measured.sample_frames,
            target,
            exceptions,
            technical_pass,
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
        .map(|_| ())
    }

    fn export_document(
        &self,
        document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        self.export_document_reporting(document, out, settings, progress)
            .map(|_| ())
    }

    /// AU3 §5.2: the same export, returning what the loudness normalization
    /// step did. `export_document` is this method with the report discarded,
    /// so the two cannot describe different encodes.
    fn export_document_reporting(
        &self,
        document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<ExportReport, MediaError> {
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

/// AU3 §3.9: the live loudness, written once per publish on the worker
/// thread (Release) and read by `Playback::loudness` (Acquire).
///
/// `i32::MIN` is "none"; `energy_loudness(0)` maps to it. A torn read pairs
/// fields from different publishes; each field is individually current or
/// newer and none is stale, which §3.9 accepts (F15) because the six values
/// are displayed, never combined.
pub(crate) struct LiveLoudness {
    momentary: AtomicI32,
    short_term: AtomicI32,
    integrated: AtomicI32,
    loudness_range: AtomicI32,
    true_peak: AtomicI32,
    programme_seconds: AtomicU32,
}

/// The atomic sentinel for an unmeasured field.
const LIVE_LOUDNESS_NONE: i32 = i32::MIN;

impl Default for LiveLoudness {
    fn default() -> Self {
        Self {
            momentary: AtomicI32::new(LIVE_LOUDNESS_NONE),
            short_term: AtomicI32::new(LIVE_LOUDNESS_NONE),
            integrated: AtomicI32::new(LIVE_LOUDNESS_NONE),
            loudness_range: AtomicI32::new(LIVE_LOUDNESS_NONE),
            true_peak: AtomicI32::new(LIVE_LOUDNESS_NONE),
            programme_seconds: AtomicU32::new(0),
        }
    }
}

impl LiveLoudness {
    /// Store all six fields, Release.
    pub(crate) fn publish(&self, snapshot: &LoudnessSnapshot) {
        let store = |atomic: &AtomicI32, value: Option<i32>| {
            atomic.store(value.unwrap_or(LIVE_LOUDNESS_NONE), Ordering::Release);
        };
        store(&self.momentary, snapshot.momentary_lufs_hundredths);
        store(&self.short_term, snapshot.short_term_lufs_hundredths);
        store(&self.integrated, snapshot.integrated_lufs_hundredths);
        store(&self.loudness_range, snapshot.loudness_range_lu_hundredths);
        store(&self.true_peak, snapshot.true_peak_dbtp_hundredths);
        self.programme_seconds
            .store(snapshot.programme_seconds, Ordering::Release);
    }

    /// Load all six fields, Acquire, sentinels mapped to `None`.
    pub(crate) fn load(&self) -> LoudnessSnapshot {
        let load = |atomic: &AtomicI32| {
            let value = atomic.load(Ordering::Acquire);
            (value != LIVE_LOUDNESS_NONE).then_some(value)
        };
        LoudnessSnapshot {
            momentary_lufs_hundredths: load(&self.momentary),
            short_term_lufs_hundredths: load(&self.short_term),
            integrated_lufs_hundredths: load(&self.integrated),
            loudness_range_lu_hundredths: load(&self.loudness_range),
            true_peak_dbtp_hundredths: load(&self.true_peak),
            programme_seconds: self.programme_seconds.load(Ordering::Acquire),
        }
    }

    pub(crate) fn clear(&self) {
        self.publish(&LoudnessSnapshot::default());
    }
}

/// AU3 §3.9: the worker's side of the live meter — the device-rate
/// [`LiveLoudnessMeter`] (which outlives the mixer and the runtime), the
/// origin the ring keys count from, `paused_at`, and the publish step.
///
/// Keys are device sample frames since the last reset: the clock's absolute
/// project-sample position minus `reset_sample`. The fill pushes the meter
/// up to a second ahead of the loudspeaker; `publish_at` stores the ring
/// entry at or behind the audible position, so nothing is published as
/// current until the sound has been heard.
pub(crate) struct WorkerLoudness {
    shared: Arc<LiveLoudness>,
    meter: Option<LiveLoudnessMeter>,
    reset_sample: u64,
    paused_at: Option<TimeCode>,
    published_key: Option<u64>,
}

impl WorkerLoudness {
    pub(crate) const fn new(shared: Arc<LiveLoudness>) -> Self {
        Self {
            shared,
            meter: None,
            reset_sample: 0,
            paused_at: None,
            published_key: None,
        }
    }

    /// The clock position at the last `pause`, if playback has not restarted.
    #[cfg(test)]
    pub(crate) const fn paused_at(&self) -> Option<TimeCode> {
        self.paused_at
    }

    /// The ring key (frames since reset) of the last published entry.
    #[cfg(test)]
    pub(crate) const fn published_key(&self) -> Option<u64> {
        self.published_key
    }

    /// The meter's own head, for the lag pin.
    #[cfg(test)]
    pub(crate) fn fed_block_end(&self) -> Option<u64> {
        self.meter
            .as_ref()
            .and_then(|meter| meter.meter().last_block_end())
    }

    pub(crate) const fn meter_mut(&mut self) -> Option<&mut LiveLoudnessMeter> {
        self.meter.as_mut()
    }

    /// §3.9 continue and reset, at `start_playback`: the meter **continues**
    /// when `from == paused_at` on an unchanged device format and **resets**
    /// otherwise (a seek, `play(from)` elsewhere, a first play, or a device
    /// rate change). Returns the meter the initial fill threads.
    ///
    /// # Errors
    ///
    /// As [`LiveLoudnessMeter::new`].
    pub(crate) fn begin(
        &mut self,
        from: TimeCode,
        sample_rate: u32,
        output_channels: u16,
        fps: Rational,
    ) -> Result<&mut LiveLoudnessMeter, MediaError> {
        let same_format = self.meter.as_ref().is_some_and(|meter| {
            meter.sample_rate() == sample_rate && meter.channels() == output_channels.clamp(1, 2)
        });
        let continues = same_format && self.paused_at == Some(from);
        if !same_format {
            self.meter = Some(LiveLoudnessMeter::new(sample_rate, output_channels)?);
        }
        if !continues {
            self.reset_to(frame_to_samples(from, sample_rate, fps));
        }
        self.paused_at = None;
        Ok(self.meter.as_mut().expect("the meter was just ensured"))
    }

    /// The tick: convert the clock's absolute position to frames since reset
    /// and store the ring entry at or behind it, once per new entry.
    pub(crate) fn publish_at(&mut self, position_samples: u64) {
        let Some(meter) = &self.meter else {
            return;
        };
        let audible = position_samples.saturating_sub(self.reset_sample);
        if let Some((key, snapshot)) = meter.published(audible)
            && self.published_key != Some(key)
        {
            self.shared.publish(&snapshot);
            self.published_key = Some(key);
        }
    }

    /// §3.9 pause: `paused_at` is the clock position at the pause; the
    /// integration is truncated to blocks ending at or before it and the
    /// paused snapshot (momentary and short-term `None`) is published.
    pub(crate) fn pause_at(&mut self, position: TimeCode, fps: Rational) {
        self.paused_at = Some(position);
        let reset_sample = self.reset_sample;
        let Some(meter) = &mut self.meter else {
            return;
        };
        let paused_sample = frame_to_samples(position, meter.sample_rate(), fps);
        if paused_sample < reset_sample {
            // A `reset_loudness` mid-play put the origin at the fed position
            // and the sound never reached it: nothing measured was heard, so
            // the origin moves back to the pause and a continue lines up.
            self.reset_to(paused_sample);
            return;
        }
        let audible = paused_sample - reset_sample;
        meter.truncate_to(audible);
        self.shared.publish(&meter.paused_snapshot());
        self.published_key = Some(audible);
    }

    /// Reset everything (meter, ring, atomics) with the ring origin at
    /// `fed_position_samples` — the mixer's fed *output* position while
    /// playing (`cursor_sample − latency`, AU2 §3.7), so keys stay aligned
    /// with the audio actually rendered; the paused position while
    /// paused, so a continue at `paused_at` lines up; zero when stopped.
    pub(crate) fn reset(&mut self, fed_position_samples: Option<u64>, fps: Rational) {
        let origin = fed_position_samples.unwrap_or_else(|| match (self.paused_at, &self.meter) {
            (Some(paused_at), Some(meter)) => frame_to_samples(paused_at, meter.sample_rate(), fps),
            _ => 0,
        });
        self.reset_to(origin);
    }

    fn reset_to(&mut self, origin: u64) {
        if let Some(meter) = &mut self.meter {
            meter.reset();
        }
        self.reset_sample = origin;
        self.published_key = None;
        self.shared.clear();
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
    /// AU1 §4.1: shared with the engine; holds `MixMeters::empty` whenever the
    /// worker is not playing.
    mix_meters: Arc<RwLock<Arc<MixMeters>>>,
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
    /// AU3 §3.9: the live meter, ring, and publish state.
    loudness: WorkerLoudness,
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
    // The worker's shared handles are constructed once, in the engine's
    // constructor; a struct of eight `Arc`s would only move the count.
    #[allow(clippy::too_many_arguments)]
    fn new(
        channels: WorkerChannels,
        clock: Arc<SharedClock>,
        meter: Arc<MeterState>,
        mix_meters: Arc<RwLock<Arc<MixMeters>>>,
        loudness: Arc<LiveLoudness>,
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
            mix_meters,
            requested,
            handled_frame_sequence: 0,
            handled_seek_sequence: 0,
            document: Arc::new(Document::default()),
            renderer: FrameRenderer::new(gpu),
            lut_lattices,
            lut_library: Arc::new(LutLibrary::default()),
            audio: None,
            loudness: WorkerLoudness::new(loudness),
            playing: false,
            last_position: None,
        }
    }

    fn run(mut self) {
        loop {
            match self.control_rx.recv_timeout(WORKER_TICK) {
                Ok(control) => {
                    self.handle_control(control);
                    // AU1 §5.3: a burst of controls cannot starve the ring for
                    // longer than one decode.
                    self.fill_audio();
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
            while let Ok(control) = self.control_rx.try_recv() {
                self.handle_control(control);
                self.fill_audio();
            }
            self.handle_coalesced_requests();
            self.tick();
        }
    }

    fn handle_control(&mut self, control: Control) {
        match control {
            Control::SetDocument(doc) => self.set_document(&doc),
            Control::LutLatticesPublished => self.rebind_lut_library(),
            Control::UpdateAudio(kind, doc) => self.update_audio(kind, doc),
            Control::Play(from) => self.start_playback(from),
            Control::Pause => self.pause(),
            Control::ResetLoudness => self.reset_loudness(),
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

    /// AU2 §5.8: the document differs only in `audio_mix`, which no video path
    /// reads, so nothing is paused, reseeked, or rebound. The app's
    /// `live_audio_change` predicate (AU4 §4.4 rule 88) is the contract; the
    /// worker does not verify it, with one exception.
    ///
    /// **The latency guard (A34).** `L` is fixed for a processor's life, so a
    /// document whose declared lookahead differs cannot be applied to a running
    /// one: the worker falls back to `set_document` and re-cues to the position
    /// it was at, rather than retargeting. Defensive, because the app predicate
    /// already refuses the live path in that case.
    /// AU4 §3.8 rule 75 / §4.4 rule 89: the single live-audio arm.
    ///
    /// `Mix` retargets the processor, `ClipShaping` rebuilds the running
    /// mixer's per-source shaping, `Both` does **both, mix then shaping, from
    /// the same `Arc<Document>`** — the real guarantee is the single document
    /// applied twice inside one control, so the two halves cannot interleave
    /// with each other or with a re-cue. `None` re-cues.
    fn update_audio(&mut self, kind: LiveAudioChange, doc: Arc<Document>) {
        match kind {
            LiveAudioChange::None => self.recue_audio(&doc),
            LiveAudioChange::Mix => {
                self.update_audio_mix(doc);
            }
            LiveAudioChange::ClipShaping => self.update_clip_shaping(doc),
            LiveAudioChange::Both => {
                if self.update_audio_mix(doc.clone()) {
                    self.update_clip_shaping(doc);
                }
            }
        }
    }

    /// AU4 §3.8 rule 73: rebuild the running mixer's clip shaping in place, or
    /// fall back to the same pause-and-re-cue branch the chain path uses.
    fn update_clip_shaping(&mut self, doc: Arc<Document>) {
        if !can_retarget_audio_mix(&self.document, &doc) {
            // AU2 §5.8: "a pause and re-cue", not a rewind. `set_document`
            // lands the transport on frame 0, so the position is captured
            // first and restored afterwards exactly as the paused branch of
            // `handle_coalesced_requests` does — the same place the app's own
            // non-live path leaves it.
            self.recue_audio(&doc);
            return;
        }
        let applied = self
            .audio
            .as_mut()
            .is_some_and(|audio| audio.update_clip_shaping(&doc));
        if applied || self.audio.is_none() {
            self.document = doc;
            return;
        }
        self.recue_audio(&doc);
    }

    /// AU2 §5.8: "a pause and re-cue", not a rewind.
    fn recue_audio(&mut self, doc: &Arc<Document>) {
        let at = self.clock.position();
        self.set_document(doc);
        let at = TimeCode(at.0.clamp(0, self.document.duration.0.saturating_sub(1)));
        self.clock.set_frame(at);
        self.emit(MediaEvent::Position(at));
        self.present(at);
    }

    fn update_audio_mix(&mut self, doc: Arc<Document>) -> bool {
        if !can_retarget_audio_mix(&self.document, &doc) {
            // AU2 §5.8: "a pause and re-cue", not a rewind. `set_document`
            // lands the transport on frame 0, so the position is captured
            // first and restored afterwards exactly as the paused branch of
            // `handle_coalesced_requests` does — the same place the app's own
            // non-live path leaves it.
            self.recue_audio(&doc);
            return false;
        }
        self.document = doc;
        if self.audio.is_none() {
            return true;
        }
        // AU2 §5.8/R8: the peak table is rebuilt only when its key set stops
        // describing the document, so a fader drag does not zero every meter on
        // every frame, and the rebuild attaches and publishes in one call.
        let rebuilt =
            self.mix_meters.read().ok().and_then(|installed| {
                mix_meters_for_update(&installed, &self.document, &self.meter)
            });
        if let Some(audio) = &mut self.audio {
            audio.update_audio_mix(&self.document);
            if let Some(meters) = &rebuilt {
                audio.attach_mix_meters(Arc::clone(meters));
            }
        }
        if let Some(meters) = rebuilt {
            self.install_mix_meters(meters);
        }
        true
    }

    fn set_document(&mut self, doc: &Document) {
        self.pause();
        // AU3 §3.9: a new document is a new programme.
        self.loudness.reset(None, doc.fps);
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
        // AU1 §4.1: a seek during playback re-enters here without passing
        // through `Playback::play`, so the installed slots are cleared too.
        if let Ok(meters) = self.mix_meters.read() {
            meters.clear();
        }
        if self.document.duration <= TimeCode::ZERO {
            self.fail(MediaError::Backend("the timeline is empty".to_owned()));
            return;
        }
        let from = TimeCode(from.0.clamp(0, self.document.duration.0.saturating_sub(1)));
        self.clock.set_frame(from);
        let mix_meters = Arc::new(MixMeters::for_document(
            &self.document,
            Arc::clone(&self.meter),
        ));
        let fps = self.document.fps;
        let opened = self.audio_for_position(from, Arc::clone(&mix_meters));
        let loudness = &mut self.loudness;
        match opened.and_then(|mut runtime| {
            // AU3 §3.9: continue at `paused_at`, reset elsewhere; the meter
            // is threaded through the initial fill before the stream starts.
            let meter = loudness.begin(from, runtime.sample_rate(), runtime.channels(), fps)?;
            runtime.fill(meter)?;
            runtime.play()?;
            Ok(runtime)
        }) {
            Ok(runtime) => {
                // AU1 §4.1: installed only once the runtime exists, so
                // `mix_peaks()` stays empty when playback fails to start.
                self.install_mix_meters(mix_meters);
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
        // AU3 §3.9: `paused_at` is this same position; the integration is
        // truncated to what was heard and the paused snapshot published.
        if self.audio.is_some() {
            self.loudness.pause_at(position, self.document.fps);
        }
        self.audio = None;
        self.meter.clear();
        self.install_mix_meters(Arc::new(MixMeters::empty(Arc::clone(&self.meter))));
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
        if let (Some(audio), Some(meter)) = (&mut self.audio, self.loudness.meter_mut())
            && let Err(error) = audio.fill(meter)
        {
            self.fail(error);
            return;
        }
        // AU3 §3.9: publish by the device clock's audible position.
        self.loudness
            .publish_at(self.clock.position_samples.load(Ordering::Acquire));
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

    fn audio_for_position(
        &self,
        project_at: TimeCode,
        mix_meters: Arc<MixMeters>,
    ) -> Result<AudioRuntime, MediaError> {
        AudioRuntime::open(
            &self.document,
            project_at,
            &self.clock.position_samples,
            &self.clock.sample_rate,
            Arc::clone(&self.meter),
            mix_meters,
        )
    }

    /// AU1 §4.1: publish the table `Playback::mix_peaks` reads.
    fn install_mix_meters(&self, meters: Arc<MixMeters>) {
        if let Ok(mut installed) = self.mix_meters.write() {
            *installed = meters;
        }
    }

    /// AU1 §5.3: top the output ring back up to the live fill target.
    fn fill_audio(&mut self) {
        if !self.playing {
            return;
        }
        if let (Some(audio), Some(meter)) = (&mut self.audio, self.loudness.meter_mut())
            && let Err(error) = audio.fill(meter)
        {
            self.fail(error);
        }
    }

    /// AU3 §3.9 / `Control::ResetLoudness`: restart the measurement. While
    /// playing the ring origin is the mixer's fed position — the audio between
    /// the loudspeaker and the fill is already rendered and cannot be
    /// re-measured — so the atomics read `default()` until the sound reaches
    /// it; while paused it is `paused_at`, so a continue lines up.
    fn reset_loudness(&mut self) {
        let fed = self
            .audio
            .as_ref()
            .filter(|_| self.playing)
            .map(AudioRuntime::fed_position_samples);
        self.loudness.reset(fed, self.document.fps);
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

/// AU2 §5.8/A34: whether a live update can be applied to a running processor.
///
/// `L` is derived from the document and fixed for a processor's life, so a
/// document whose declared lookahead differs cannot be retargeted onto a
/// running one — the worker pauses and re-cues instead. Defensive: the app
/// predicate already refuses the live path in that case (OPEN-3).
fn can_retarget_audio_mix(current: &Document, incoming: &Document) -> bool {
    current.audio_mix.lookahead_milliseconds() == incoming.audio_mix.lookahead_milliseconds()
}

/// AU2 §5.8/R8: the peak table a live update needs, or `None` to keep the one
/// already installed.
///
/// `MixMeters::for_document` allocates fresh zeroed slots and the mixer UI only
/// raises its meters while playing, so rebuilding on every update would zero
/// every meter on every frame of a fader drag. The table is rebuilt only when
/// its `(track id, bus id, chain-and-effect-id)` key set stops describing the
/// document (A33).
fn mix_meters_for_update(
    installed: &MixMeters,
    document: &Document,
    master: &Arc<MeterState>,
) -> Option<Arc<MixMeters>> {
    (!installed.matches_document(document))
        .then(|| Arc::new(MixMeters::for_document(document, Arc::clone(master))))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::atomic::AtomicUsize};

    use kinewright_core::{
        AutomationCurve, Clip, ClipContent, Effect, Keyframe, KeyframeInterpolation, LutAsset,
        LutAssetId, LutAssetKind, LutAssetSource, MediaKind, MediaSourceFingerprint, ParamValue,
        Title, Track, TrackId, TrackKind,
    };

    use super::*;
    use crate::{
        cc1_fixtures::fallback_gpu,
        initialize_ffmpeg,
        lut::parse_cube_lut,
        sha256::{sha256_bytes, source_fingerprint},
        test_support::{GeneratedMedia, TempDirectory, single_clip_document},
    };

    /// AU3 §7 item A12 (§3.9): `Worker::reset_loudness`'s origin selection —
    /// `Some(AudioRuntime::fed_position_samples())` while playing, `None`
    /// otherwise — exercised device-free on `WorkerLoudness` itself.
    ///
    /// While playing the origin is the fed position, so the ≤ 1 s already
    /// rendered publishes nothing until the loudspeaker reaches it; while
    /// paused it is `paused_at`, so a continue lines up; stopped it is zero.
    #[test]
    fn a_loudness_reset_keys_from_the_fed_position_only_while_playing() {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        fn stereo_tone(frames: usize) -> Vec<f32> {
            (0..frames)
                .flat_map(|frame| {
                    let sample = (0.1
                        * (2.0 * std::f64::consts::PI * 1_000.0 * frame as f64 / 48_000.0).sin())
                        as f32;
                    [sample, sample]
                })
                .collect()
        }

        let fps = Rational::new(10, 1).unwrap();
        let shared = Arc::new(LiveLoudness::default());
        let mut loudness = WorkerLoudness::new(Arc::clone(&shared));
        loudness.begin(TimeCode::ZERO, 48_000, 2, fps).unwrap();

        // Playing: the fill has metered 2 s and the loudspeaker has heard 1 s.
        // (Each later push is one second — ten sub-blocks — so the whole run
        // stays inside the 16-entry ring.)
        let two_seconds = stereo_tone(96_000);
        let one_second = &two_seconds[..96_000];
        loudness
            .meter_mut()
            .unwrap()
            .push_chunk(&two_seconds, 2)
            .unwrap();
        loudness.publish_at(48_000);
        assert_eq!(loudness.published_key(), Some(48_000));
        assert_ne!(shared.load(), LoudnessSnapshot::default());

        // `Some(fed)`: the origin is the fed position, 96 000, so nothing is
        // published until the loudspeaker has passed it by a whole sub-block.
        loudness.reset(Some(96_000), fps);
        assert_eq!(loudness.published_key(), None);
        assert_eq!(shared.load(), LoudnessSnapshot::default());
        loudness
            .meter_mut()
            .unwrap()
            .push_chunk(one_second, 2)
            .unwrap();
        loudness.publish_at(100_799);
        assert_eq!(
            loudness.published_key(),
            None,
            "one frame short of the first sub-block past the fed origin"
        );
        loudness.publish_at(100_800);
        assert_eq!(loudness.published_key(), Some(4_800));

        // `None` while paused: the origin is `paused_at`. Pausing at frame 12
        // (1.2 s at 10 fps) leaves the ring keyed from 57 600.
        loudness.pause_at(TimeCode(12), fps);
        assert_eq!(loudness.paused_at(), Some(TimeCode(12)));
        loudness.reset(None, fps);
        assert_eq!(loudness.published_key(), None);
        loudness
            .meter_mut()
            .unwrap()
            .push_chunk(one_second, 2)
            .unwrap();
        loudness.publish_at(57_600 + 4_799);
        assert_eq!(loudness.published_key(), None);
        loudness.publish_at(57_600 + 4_800);
        assert_eq!(
            loudness.published_key(),
            Some(4_800),
            "a paused reset keys from `paused_at`, so a continue lines up"
        );

        // `None` with no `paused_at` (stopped, or reset before a first play):
        // the origin is zero.
        loudness.begin(TimeCode(30), 48_000, 2, fps).unwrap();
        assert_eq!(loudness.paused_at(), None);
        loudness.reset(None, fps);
        loudness
            .meter_mut()
            .unwrap()
            .push_chunk(one_second, 2)
            .unwrap();
        loudness.publish_at(4_799);
        assert_eq!(loudness.published_key(), None);
        loudness.publish_at(4_800);
        assert_eq!(loudness.published_key(), Some(4_800), "keyed from zero");
    }

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
                    audio_gain_curve: None,
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

    /// A 64x36 solid source: the matte proof asserts *geometry*, so the
    /// picture only has to decode, and the raster has to be small enough to
    /// state a pixel-exact expectation.
    fn matte_source(label: &str) -> GeneratedMedia {
        GeneratedMedia::ffmpeg(
            label,
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=gray:size=64x36:rate=30:duration=1",
                "-frames:v",
                "30",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "yuv444p",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ],
            "mkv",
        )
    }

    /// A `color_wheels` node whose gain is off neutral, so the node is active
    /// and the proof is about the matte rather than about CC3 3.3.
    fn wheels_node(id: u64, parameters: &[(&str, i64)]) -> Effect {
        let mut stored = vec![("gain_master_thousandths", 1_500_i64)];
        stored.extend_from_slice(parameters);
        Effect {
            id: EffectId(id),
            name: "color_wheels".to_owned(),
            parameters: stored
                .into_iter()
                .map(|(name, value)| (name.to_owned(), ParamValue::Integer(value)))
                .collect(),
            keyframes: std::collections::BTreeMap::new(),
        }
    }

    /// One centred 2500/2500 rect window with no feather: on a 64x36 raster
    /// its pixel centres are `x in 16..48`, `y in 9..27`.
    const CENTERED_RECT: &[(&str, i64)] = &[
        ("matte_enabled", 1),
        ("matte_window_count", 1),
        ("matte_window0_shape_token", 1),
        ("matte_window0_center_x_basis_points", 5_000),
        ("matte_window0_center_y_basis_points", 5_000),
        ("matte_window0_half_width_basis_points", 2_500),
        ("matte_window0_half_height_basis_points", 2_500),
        ("matte_window0_feather_basis_points", 0),
    ];

    /// Stage a single-clip 64x36 document carrying one effect stack.
    fn matte_document(media: &GeneratedMedia, effects: Vec<Effect>) -> Arc<Document> {
        let mut asset =
            probe_path(media.path(), AssetId(1)).expect("the matte source should probe");
        assert_eq!(asset.resolution, Some((64, 36)));
        // The proof is about matte geometry, so the source colour is stated
        // explicitly rather than inferred: an unknown-primaries source would
        // fail the managed decode before any coverage existed.
        asset.color_description = kinewright_core::ColorDescription {
            primaries: kinewright_core::ColorPrimaries::Bt709,
            transfer: kinewright_core::ColorTransfer::Bt709,
            matrix: kinewright_core::ColorMatrix::Bt709,
            range: kinewright_core::ColorRange::Limited,
            white_point: kinewright_core::ColorWhitePoint::D65,
            bit_depth: kinewright_core::ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::UserOverride,
        };
        let mut document = single_clip_document(asset);
        document.tracks[0].clips[0].effects = effects;
        assert_eq!(document.resolution, (64, 36));
        Arc::new(document)
    }

    /// The set of pixel indices the proof reports as covered, asserting the
    /// coverage encoding itself on every pixel: opaque, grey, and bi-level for
    /// a `feather = 0` window.
    fn covered_indices(proof: &MatteProof) -> std::collections::BTreeSet<usize> {
        let mut covered = std::collections::BTreeSet::new();
        for (index, pixel) in proof.coverage.pixels.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(pixel[3], u8::MAX, "pixel {index} is not opaque");
            assert_eq!(pixel[0], pixel[1], "pixel {index} is not grey");
            assert_eq!(pixel[1], pixel[2], "pixel {index} is not grey");
            assert!(
                pixel[0] == 0 || pixel[0] == 255,
                "a feather-free window has no partial coverage; pixel {index} is {}",
                pixel[0]
            );
            if pixel[0] == 255 {
                covered.insert(index);
            }
        }
        covered
    }

    /// CC5 4.1: the coverage of a centred, feather-free rect window is exact,
    /// opaque, and carries the resolved matte identity as metadata.
    #[test]
    fn cc5_matte_proof_reports_exact_window_coverage_and_metadata() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-coverage");
        let document = matte_document(&media, vec![wheels_node(7, CENTERED_RECT)]);
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");

        let proof = engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(7),
            )
            .expect("the matte proof should render");
        assert_eq!((proof.coverage.width, proof.coverage.height), (64, 36));
        let covered = covered_indices(&proof);
        let expected = (0..64 * 36)
            .filter(|index| {
                let (x, y) = (index % 64, index / 64);
                (16..48).contains(&x) && (9..27).contains(&y)
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(expected.len(), 576);
        assert_eq!(
            covered, expected,
            "the covered set is not the 2500/2500 rect"
        );
        assert_eq!(64 * 36 - covered.len(), 1_728);

        assert_eq!(proof.metadata.clip, ClipId(1));
        assert_eq!(proof.metadata.effect, EffectId(7));
        assert_eq!(proof.metadata.node_kind, "color_wheels");
        assert_eq!(proof.metadata.coverage_encoding, MATTE_COVERAGE_ENCODING);
        assert_eq!(proof.metadata.coverage_scale, 255);
        // round(1e6 * 64 / 36)
        assert_eq!(proof.metadata.raster_aspect_millionths, 1_777_778);
        assert!(proof.metadata.matte_enabled);
        assert_eq!(proof.metadata.window_count, 1);
        assert!(!proof.metadata.qualifier_enabled);
        assert!(
            proof.metadata.render.full_resolution,
            "an isolated proof renders the document raster at full scale"
        );
    }

    /// CC5 4.1: a proof never returns a blank frame. Each refusal is typed and
    /// carries its stable code.
    #[test]
    fn cc5_matte_proof_fails_typed_instead_of_returning_a_blank_frame() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-typed-failures");
        let mut bypassed = CENTERED_RECT.to_vec();
        bypassed.push(("bypass", 1));
        let mut disabled = CENTERED_RECT.to_vec();
        disabled[0] = ("matte_enabled", 0);
        let document = matte_document(
            &media,
            vec![
                wheels_node(7, &bypassed),
                wheels_node(8, &disabled),
                wheels_node(9, CENTERED_RECT),
            ],
        );
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");
        let refusal = |effect: u64| {
            let error = engine
                .matte_proof_for_document(
                    Arc::clone(&document),
                    TimeCode::ZERO,
                    ClipId(1),
                    EffectId(effect),
                )
                .expect_err("a refusing node must not render a frame");
            let MediaError::Backend(message) = error else {
                panic!("a matte proof refusal is a backend error");
            };
            message
        };

        let inactive = refusal(7);
        assert!(
            inactive.starts_with("matte_proof_node_inactive:"),
            "unexpected message: {inactive}"
        );
        assert!(
            inactive.contains("bypassed"),
            "the reason token must be reported: {inactive}"
        );
        let no_matte = refusal(8);
        assert!(
            no_matte.starts_with("matte_proof_no_matte:"),
            "unexpected message: {no_matte}"
        );
        let absent = refusal(99);
        assert!(
            absent.starts_with("matte_proof_effect_not_found:"),
            "unexpected message: {absent}"
        );
        // The clip itself is still provable, so the refusals above are about
        // the named node rather than about a broken document.
        engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(9),
            )
            .expect("the matte-carrying node on the same clip still proves");
    }

    /// CC5 4.1: a clip that exists but is not an active visual layer at the
    /// proved frame is its **own** refusal.
    ///
    /// This used to report `matte_proof_effect_not_found`, which named a node
    /// that was never missing and sent the caller hunting for the wrong thing.
    /// The two failures have different recoveries — "fix the effect id" versus
    /// "prove a frame the clip is on screen at" — so they carry different
    /// codes.
    #[test]
    fn cc5_matte_proof_refuses_a_clip_that_is_not_visible_at_the_frame() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-clip-not-visible");
        let document = matte_document(&media, vec![wheels_node(7, CENTERED_RECT)]);
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");

        // The claim is only worth making if the same clip and node prove
        // cleanly at a frame the clip *is* on screen at, so state that first.
        let clip = &document.tracks[0].clips[0];
        let past_the_end = TimeCode(
            clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0 + 10,
        );
        assert!(past_the_end > clip.timeline_start);
        engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(7),
            )
            .expect("the clip proves at a frame it is visible at");

        let error = engine
            .matte_proof_for_document(Arc::clone(&document), past_the_end, ClipId(1), EffectId(7))
            .expect_err("a clip that is off screen must not render a coverage frame");
        let MediaError::Backend(message) = error else {
            panic!("a matte proof refusal is a backend error");
        };
        assert!(
            message.starts_with("matte_proof_clip_not_visible:"),
            "unexpected message: {message}"
        );
        // The refusal names the clip and the frame it was asked about, which
        // is the whole difference from the effect-not-found code.
        assert!(
            message.contains(&format!("{}", ClipId(1))),
            "the refusal must name the clip: {message}"
        );
        assert!(
            message.contains(&format!("{past_the_end}")),
            "the refusal must name the frame: {message}"
        );
        assert_eq!(
            kinewright_core::MatteProofError::ClipNotVisible {
                clip: ClipId(1),
                at: past_the_end,
            }
            .code(),
            "matte_proof_clip_not_visible"
        );
        // And the node itself is still findable, so this is not the absent-id
        // failure wearing a new name.
        let absent = engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(99),
            )
            .expect_err("an absent node still refuses");
        let MediaError::Backend(absent) = absent else {
            panic!("a matte proof refusal is a backend error");
        };
        assert!(
            absent.starts_with("matte_proof_effect_not_found:"),
            "unexpected message: {absent}"
        );
    }

    /// CC5 4.1: the scratch document is reduced to the target clip's track and
    /// clip, so an opaque layer above it cannot composite over the coverage.
    #[test]
    fn cc5_matte_proof_ignores_a_layer_above_the_target_clip() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-isolation");
        let document = matte_document(&media, vec![wheels_node(7, CENTERED_RECT)]);
        let mut covered_document = (*document).clone();
        let mut covering = covered_document.tracks[0].clips[0].clone();
        covering.id = ClipId(2);
        covering.effects = Vec::new();
        covered_document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![covering],
        });
        let covered_document = Arc::new(covered_document);
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");

        // The isolation claim is only worth making if the covering layer would
        // otherwise be visible, so state that first.
        let plain = engine
            .monitor_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
            .expect("the single-layer monitor proof should render");
        let obscured = engine
            .monitor_proof_for_document(Arc::clone(&covered_document), TimeCode::ZERO)
            .expect("the covered monitor proof should render");
        assert_ne!(
            plain.image.pixels, obscured.image.pixels,
            "the second layer must really composite over the target"
        );

        let alone = engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(7),
            )
            .expect("the single-layer proof should render");
        let under_a_layer = engine
            .matte_proof_for_document(
                Arc::clone(&covered_document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(7),
            )
            .expect("the covered proof should render");
        assert_eq!(
            covered_indices(&under_a_layer),
            covered_indices(&alone),
            "an opaque layer above the target changed its coverage"
        );
    }

    /// CC5 3.2: the matte is resolved at the requested frame, after keyframe
    /// evaluation, so a tracked window moves with its curve.
    #[test]
    fn cc5_matte_proof_follows_a_keyframed_window_center() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-keyframes");
        // A full-height window, so only the keyframed x centre selects.
        let mut node = wheels_node(
            7,
            &[
                ("matte_enabled", 1),
                ("matte_window_count", 1),
                ("matte_window0_shape_token", 1),
                ("matte_window0_center_x_basis_points", 2_500),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 10_000),
                ("matte_window0_feather_basis_points", 0),
            ],
        );
        node.keyframes.insert(
            "matte_window0_center_x_basis_points".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 2_500,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(20),
                        value: 7_500,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let document = matte_document(&media, vec![node]);
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");

        let at_start = covered_indices(
            &engine
                .matte_proof_for_document(
                    Arc::clone(&document),
                    TimeCode::ZERO,
                    ClipId(1),
                    EffectId(7),
                )
                .expect("frame 0 proof"),
        );
        let at_end = covered_indices(
            &engine
                .matte_proof_for_document(
                    Arc::clone(&document),
                    TimeCode(20),
                    ClipId(1),
                    EffectId(7),
                )
                .expect("frame 20 proof"),
        );
        assert!(!at_start.is_empty() && !at_end.is_empty());
        assert!(
            at_start.is_disjoint(&at_end),
            "a window that travelled a full width must not overlap itself"
        );
        assert!(
            at_start.iter().all(|index| index % 64 < 32),
            "at frame 0 the window sits on the left half"
        );
        assert!(
            at_end.iter().all(|index| index % 64 >= 32),
            "at frame 20 the window sits on the right half"
        );
        assert_eq!(at_start.len(), 32 * 36);
        assert_eq!(at_end.len(), 32 * 36);
    }

    /// AU2 §7 item B12 / R8: ten consecutive live updates that change only a
    /// gain keep the installed peak table — `Arc::ptr_eq` on the very table
    /// `Playback::mix_peaks` reads — and a structural edit replaces it.
    #[test]
    fn ten_gain_only_updates_keep_the_installed_meter_table() {
        use kinewright_core::{AudioBus, AudioBusId, ParamValue};

        let audio_track = |id: u64| Track {
            id: TrackId(id),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        };
        let mut document = Document {
            fps: Rational::new(10, 1).unwrap(),
            duration: TimeCode(30),
            tracks: vec![audio_track(1), audio_track(2)],
            ..Document::default()
        };
        document.audio_mix.buses = vec![AudioBus {
            id: AudioBusId(1),
            name: "Bed".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects: vec![Effect {
                id: EffectId(1),
                name: "audio_compressor".to_owned(),
                parameters: std::collections::BTreeMap::new(),
                keyframes: std::collections::BTreeMap::new(),
            }],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        }];

        let master = Arc::new(MeterState::default());
        let installed = Arc::new(MixMeters::for_document(&document, Arc::clone(&master)));
        let mut current = Arc::clone(&installed);
        for step in 1..=10_i32 {
            document.audio_mix.buses[0].gain_tenth_db = -step;
            document.audio_mix.master.gain_tenth_db = step;
            if let Some(rebuilt) = mix_meters_for_update(&current, &document, &master) {
                current = rebuilt;
            }
            assert!(
                Arc::ptr_eq(&current, &installed),
                "update {step} replaced the installed table"
            );
        }

        // A master node adds a gain-reduction key, so the table must change.
        document.audio_mix.master.effects = vec![Effect {
            id: EffectId(9),
            name: "audio_true_peak_limiter".to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "ceiling_tenth_db".to_owned(),
                ParamValue::Integer(-10),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        }];
        let rebuilt = mix_meters_for_update(&current, &document, &master)
            .expect("a structural edit must rebuild the table");
        assert!(!Arc::ptr_eq(&rebuilt, &installed));
        assert!(rebuilt.matches_document(&document));
        assert!(mix_meters_for_update(&rebuilt, &document, &master).is_none());
    }

    /// AU2 §7 item B11 / A34: the worker's latency guard. A gain-only edit and
    /// a second bus that declares no more than the standing maximum stay on the
    /// live path; anything that moves `L` re-cues.
    #[test]
    fn the_latency_guard_re_cues_only_when_the_declared_lookahead_moves() {
        use kinewright_core::{AudioBus, AudioBusId, ParamValue};

        let lookahead_node = |id: u64, name: &str, milliseconds: i64| Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "lookahead_milliseconds".to_owned(),
                ParamValue::Integer(milliseconds),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        };
        let bus = |id: u64, effects: Vec<Effect>| AudioBus {
            id: AudioBusId(id),
            name: format!("Bus {id}"),
            tracks: Vec::new(),
            gain_tenth_db: 0,
            effects,
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        };
        let mut document = Document {
            fps: Rational::new(10, 1).unwrap(),
            duration: TimeCode(30),
            ..Document::default()
        };
        document.audio_mix.buses = vec![bus(
            1,
            vec![lookahead_node(1, "audio_true_peak_limiter", 10)],
        )];

        let mut faded = document.clone();
        faded.audio_mix.buses[0].gain_tenth_db = -60;
        faded.audio_mix.master.gain_tenth_db = -30;
        assert!(can_retarget_audio_mix(&document, &faded));

        // `L_bus` is the maximum over buses, so a second 10 ms chain does not
        // move it.
        let mut second = document.clone();
        second
            .audio_mix
            .buses
            .push(bus(2, vec![lookahead_node(2, "audio_compressor", 10)]));
        assert!(can_retarget_audio_mix(&document, &second));

        // A master limiter adds a whole new stage.
        let mut mastered = document.clone();
        mastered.audio_mix.master.effects = vec![lookahead_node(3, "audio_true_peak_limiter", 5)];
        assert!(!can_retarget_audio_mix(&document, &mastered));

        // And so does raising the bus stage.
        let mut deeper = document.clone();
        deeper.audio_mix.buses[0].effects = vec![lookahead_node(1, "audio_true_peak_limiter", 4)];
        assert!(!can_retarget_audio_mix(&document, &deeper));
    }

    /// AU2 §7 item B11 / §5.8 (A34): the latency guard is a pause and **re-cue**,
    /// not a rewind. A live update whose declared lookahead differs, applied
    /// while the transport sits at frame 10, leaves it at frame 10 and paused —
    /// the same place the app's own non-live path leaves it — instead of
    /// snapping to frame 0 the way `set_document` alone would.
    #[test]
    fn the_latency_guard_fallback_re_cues_at_the_current_position() {
        use kinewright_core::{AudioBus, AudioBusId, ParamValue};

        let (_control_tx, control_rx) = unbounded::<Control>();
        let (frames_tx, frames_rx) = bounded(2);
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let meter = Arc::new(MeterState::default());
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let mut worker = Worker::new(
            WorkerChannels {
                control_rx,
                frames_tx,
                frames_drop_rx: frames_rx.clone(),
                events_tx,
                events_drop_rx: events_rx.clone(),
            },
            Arc::clone(&clock),
            Arc::clone(&meter),
            Arc::clone(&mix_meters),
            Arc::new(LiveLoudness::default()),
            Arc::new(RequestedPositions::default()),
            fallback_gpu().context(),
            Arc::new(RwLock::new(PublishedLattices::default())),
        );

        let limiter = |milliseconds: i64| Effect {
            id: EffectId(1),
            name: "audio_true_peak_limiter".to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "lookahead_milliseconds".to_owned(),
                ParamValue::Integer(milliseconds),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        };
        let document = |milliseconds: i64| {
            let mut document = Document {
                fps: Rational::new(10, 1).unwrap(),
                duration: TimeCode(30),
                resolution: (64, 64),
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                }],
                ..Document::default()
            };
            document.audio_mix.buses = vec![AudioBus {
                id: AudioBusId(1),
                name: "Bed".to_owned(),
                tracks: vec![TrackId(1)],
                gain_tenth_db: 0,
                effects: vec![limiter(milliseconds)],
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
            }];
            Arc::new(document)
        };

        // The transport is playing at frame 10. No audio device is opened here:
        // the guard runs before anything touches `self.audio`.
        worker.document = document(10);
        worker.playing = true;
        worker.clock.set_fps(worker.document.fps);
        worker.clock.set_frame(TimeCode(10));
        assert_eq!(worker.clock.position(), TimeCode(10));

        let deeper = document(4);
        assert!(!can_retarget_audio_mix(&worker.document, &deeper));
        worker.update_audio_mix(Arc::clone(&deeper));

        assert_eq!(
            worker.clock.position(),
            TimeCode(10),
            "the defensive fallback must re-cue, not rewind to frame 0"
        );
        assert!(!worker.playing, "the fallback pauses the transport");
        assert_eq!(worker.document.audio_mix, deeper.audio_mix);

        // A gain-only update on the same lookahead stays on the live path and
        // leaves the position alone too.
        let mut faded = (*deeper).clone();
        faded.audio_mix.buses[0].gain_tenth_db = -60;
        worker.update_audio_mix(Arc::new(faded));
        assert_eq!(worker.clock.position(), TimeCode(10));
        assert_eq!(worker.document.audio_mix.buses[0].gain_tenth_db, -60);
    }

    /// AU4 §7 item A15 (§4.4 rule 89): the defaulted `Playback::update_audio`
    /// dispatches every kind, so a test double that overrides neither half
    /// still behaves and `Both` reaches both.
    #[test]
    fn the_default_update_audio_dispatches_every_live_audio_change() {
        #[derive(Default)]
        struct PlainDouble(AtomicUsize);
        impl Playback for PlainDouble {
            fn set_document(&self, _doc: Arc<Document>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            fn request_frame(&self, _t: TimeCode) {}
            fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
                unbounded().1
            }
            fn events(&self) -> Receiver<MediaEvent> {
                unbounded().1
            }
            fn play(&self, _from: TimeCode) {}
            fn pause(&self) {}
            fn seek(&self, _to: TimeCode) {}
            fn position(&self) -> TimeCode {
                TimeCode::ZERO
            }
            fn output_peaks(&self) -> [f32; 2] {
                [0.0, 0.0]
            }
        }

        #[derive(Default)]
        struct CountingPlayback {
            documents: AtomicUsize,
            mixes: AtomicUsize,
            shapings: AtomicUsize,
        }

        impl Playback for CountingPlayback {
            fn set_document(&self, _doc: Arc<Document>) {
                self.documents.fetch_add(1, Ordering::Relaxed);
            }
            fn request_frame(&self, _t: TimeCode) {}
            fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
                unbounded().1
            }
            fn events(&self) -> Receiver<MediaEvent> {
                unbounded().1
            }
            fn play(&self, _from: TimeCode) {}
            fn pause(&self) {}
            fn seek(&self, _to: TimeCode) {}
            fn position(&self) -> TimeCode {
                TimeCode::ZERO
            }
            fn output_peaks(&self) -> [f32; 2] {
                [0.0, 0.0]
            }
            fn update_audio_mix(&self, _doc: Arc<Document>) {
                self.mixes.fetch_add(1, Ordering::Relaxed);
            }
            fn update_clip_shaping(&self, _doc: Arc<Document>) {
                self.shapings.fetch_add(1, Ordering::Relaxed);
            }
        }

        let double = CountingPlayback::default();
        let doc = || Arc::new(Document::default());
        double.update_audio(LiveAudioChange::None, doc());
        assert_eq!(double.documents.load(Ordering::Relaxed), 1);
        double.update_audio(LiveAudioChange::Mix, doc());
        assert_eq!(double.mixes.load(Ordering::Relaxed), 1);
        double.update_audio(LiveAudioChange::ClipShaping, doc());
        assert_eq!(double.shapings.load(Ordering::Relaxed), 1);
        double.update_audio(LiveAudioChange::Both, doc());
        assert_eq!(double.mixes.load(Ordering::Relaxed), 2);
        assert_eq!(double.shapings.load(Ordering::Relaxed), 2);
        assert_eq!(
            double.documents.load(Ordering::Relaxed),
            1,
            "no live kind re-cues"
        );

        // And a double that overrides neither half falls all the way back to
        // `set_document`, so nothing in the workspace has to change.
        let plain = PlainDouble::default();
        plain.update_audio(LiveAudioChange::Both, doc());
        assert_eq!(plain.0.load(Ordering::Relaxed), 2);
    }

    /// AU4 §7 item A15 (§3.8 rule 75): **one** `Control::UpdateAudio(kind,
    /// document)` carries both halves, and the worker's single arm applies mix
    /// then shaping from the same `Arc<Document>`. A `Both` batch that fails
    /// the latency guard re-cues once and does not then apply half of itself.
    #[test]
    fn one_update_audio_control_carries_both_live_audio_halves() {
        use kinewright_core::{AudioBus, AudioBusId, ParamValue};

        let (control_tx, control_rx) = unbounded::<Control>();
        let (frames_tx, frames_rx) = bounded(2);
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let meter = Arc::new(MeterState::default());
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let mut worker = Worker::new(
            WorkerChannels {
                control_rx: control_rx.clone(),
                frames_tx,
                frames_drop_rx: frames_rx.clone(),
                events_tx,
                events_drop_rx: events_rx.clone(),
            },
            Arc::clone(&clock),
            Arc::clone(&meter),
            Arc::clone(&mix_meters),
            Arc::new(LiveLoudness::default()),
            Arc::new(RequestedPositions::default()),
            fallback_gpu().context(),
            Arc::new(RwLock::new(PublishedLattices::default())),
        );

        let limiter = |milliseconds: i64| Effect {
            id: EffectId(1),
            name: "audio_true_peak_limiter".to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "lookahead_milliseconds".to_owned(),
                ParamValue::Integer(milliseconds),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        };
        let document = |milliseconds: i64, gain: i32| {
            let mut document = Document {
                fps: Rational::new(10, 1).unwrap(),
                duration: TimeCode(30),
                resolution: (64, 64),
                tracks: vec![Track {
                    id: TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                }],
                ..Document::default()
            };
            document.audio_mix.buses = vec![AudioBus {
                id: AudioBusId(1),
                name: "Bed".to_owned(),
                tracks: vec![TrackId(1)],
                gain_tenth_db: gain,
                effects: vec![limiter(milliseconds)],
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
            }];
            Arc::new(document)
        };

        worker.document = document(10, 0);
        worker.playing = true;
        worker.clock.set_fps(worker.document.fps);
        worker.clock.set_frame(TimeCode(10));

        // One send, one control drained: a `Both` batch never crosses the
        // channel as two.
        let both = document(10, -60);
        control_tx
            .send(Control::UpdateAudio(
                LiveAudioChange::Both,
                Arc::clone(&both),
            ))
            .unwrap();
        let drained = control_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(drained.len(), 1, "a Both batch is one control");
        for control in drained {
            worker.handle_control(control);
        }
        assert_eq!(worker.document.audio_mix, both.audio_mix);
        assert_eq!(
            worker.clock.position(),
            TimeCode(10),
            "a live kind must not re-cue"
        );
        assert!(worker.playing);

        // The latency guard still wins inside `Both`: the mix half re-cues and
        // the shaping half is not applied on top of a re-cued transport.
        let deeper = document(4, -60);
        worker.handle_control(Control::UpdateAudio(
            LiveAudioChange::Both,
            Arc::clone(&deeper),
        ));
        assert_eq!(worker.clock.position(), TimeCode(10));
        assert!(!worker.playing, "the fallback pauses the transport");
        assert_eq!(worker.document.audio_mix, deeper.audio_mix);

        // And `None` is the existing stop-and-re-cue.
        worker.playing = true;
        worker.clock.set_frame(TimeCode(7));
        let plain = document(4, 0);
        worker.handle_control(Control::UpdateAudio(
            LiveAudioChange::None,
            Arc::clone(&plain),
        ));
        assert_eq!(worker.clock.position(), TimeCode(7));
        assert!(!worker.playing);
        assert_eq!(worker.document.audio_mix, plain.audio_mix);
    }
}
