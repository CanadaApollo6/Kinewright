use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock,
        atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kinewright_core::{
    Analysis, AnalysisKind, AssetId, AssetTranscript, AudioLoudness, AudioQcReport, AudioQcRequest,
    AudioRepairReport, AudioRepairRequest, BeatStatus, ClipContent, ClipId,
    DeliveryAudioVerification, DeliveryVerification, DeliveryVerificationRequest, Document,
    EffectId, Export, ExportCancellation, ExportReport, ExportSettings, FrameStamp,
    LiveAudioChange, LoudnessSnapshot, LoudnessTarget, LutAvailabilityKind, LutAvailabilityStatus,
    MATTE_COVERAGE_ENCODING, MATTE_COVERAGE_SCALE, MatteParams, MatteProof, MatteProofError,
    MatteProofMetadata, MediaAsset, MediaAvailabilityKind, MediaAvailabilityStatus,
    MediaCacheClearResult, MediaCacheFamily, MediaCacheFamilyStatus, MediaCacheInventory,
    MediaError, MediaEvent, MediaKind, MixLevelReport, MixLevelRequest, MixNoiseProfileRequest,
    MixPeaks, MixSpectrumReport, MixSpectrumRequest, MixWindowLevelReport, MixWindowRequest,
    MonitorProof, NoiseProfileReport, Playback, PlaybackState, PlaybackStats, PreviewFrame,
    ProgressSink, Rational, RgbaImage, SceneStatus, SilenceStatus, TimeCode, TimelineBeat,
    TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord, TranscriptStatus,
    VisualAssetResult, WORKING_PROOF_ENCODING, WORKING_PROOF_STAGE, WorkingProof,
    WorkingProofMetadata, audio_qc_technical_pass, delivery_audio_exceptions,
    export_lut_preflight_with,
};

use crate::{
    analysis::VisualAssetService,
    audio::{
        AudioDecoder, AudioDiagnostics, AudioRuntime, MeterState, MixMeters, OutputDevice,
        decode_audio_range,
    },
    clock::{frame_to_samples, samples_to_frame},
    compositor::GpuContext,
    decode::probe_path,
    derived::{DerivedAnalysisConfig, DerivedAnalysisService},
    derived_cache::CacheStats,
    loudness::{LiveLoudnessMeter, LoudnessMeter},
    lut::CubeLut,
    lut_store::LutLibrary,
    preview::{
        AgentJob, AgentWork, CancelOnDrop, JobKind, Lane, Preview, Scene, TransportJob, Wakeup,
        worker_stopped,
    },
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
    sha256::source_fingerprint,
    stats::Ack,
    transcript::{TranscriptService, default_data_dir},
};

const WORKER_TICK: Duration = Duration::from_millis(5);

/// How often a waiting agent request checks its caller's `AgentCancel`.
const AGENT_CANCEL_POLL: Duration = Duration::from_millis(10);

/// Test support (review B F2): the preview thread held by an agent job;
/// dropping it releases the thread.
#[cfg(any(test, feature = "test-util"))]
pub struct AgentLaneHold {
    lane: Arc<Lane>,
    _release: Sender<()>,
}

#[cfg(any(test, feature = "test-util"))]
impl AgentLaneHold {
    /// Agent jobs waiting behind the hold, and how many are cancelled.
    #[must_use]
    pub fn waiting(&self) -> (usize, usize) {
        self.lane.waiting()
    }
}

/// AU3 §5.3: the delivery audio measurement lane. Every loudness figure this
/// engine publishes is measured at 48 kHz stereo, whatever the file carries.
const AUDIO_MEASUREMENT_RATE: u32 = 48_000;
const AUDIO_MEASUREMENT_CHANNELS: u16 = 2;

pub(crate) struct SharedClock {
    position_samples: Arc<AtomicU64>,
    sample_rate: Arc<AtomicU32>,
    project_fps_num: AtomicU32,
    project_fps_den: AtomicU32,
    fallback_frame: AtomicI64,
}

impl SharedClock {
    pub(crate) fn new() -> Self {
        Self {
            position_samples: Arc::new(AtomicU64::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
            project_fps_num: AtomicU32::new(30),
            project_fps_den: AtomicU32::new(1),
            fallback_frame: AtomicI64::new(0),
        }
    }

    pub(crate) fn set_fps(&self, fps: Rational) {
        self.project_fps_num
            .store(fps.numerator(), Ordering::Release);
        self.project_fps_den
            .store(fps.denominator(), Ordering::Release);
    }

    pub(crate) fn set_frame(&self, frame: TimeCode) {
        self.fallback_frame.store(frame.0.max(0), Ordering::Release);
        self.sample_rate.store(0, Ordering::Release);
    }

    pub(crate) fn position(&self) -> TimeCode {
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

/// Project a document onto what the proof may see, keeping it valid.
///
/// CC5 4.1: a matte proof renders the coverage of one node on one clip, so no
/// other layer may composite over it. Every other clip is disabled rather
/// than removed, so the duration, track tables and references stay exactly
/// the original's (a removal broke shorter targets, MO2 B1 fix G5). An
/// adjustment keeps the strictly lower tracks it grades (MO2 R18, G6);
/// nothing that could occlude the proof survives. `lut_assets` is retained
/// so the surviving clips' LUT nodes still bind (CC4 2.4).
fn matte_proof_scratch_document(
    document: &Document,
    clip: ClipId,
    effect: EffectId,
) -> Result<Document, MatteProofError> {
    let mut scratch = document.clone();
    let (target_track, adjustment) = scratch
        .tracks
        .iter()
        .enumerate()
        .find_map(|(index, track)| {
            let found = track.clips.iter().find(|candidate| candidate.id == clip);
            found.map(|target| (index, matches!(target.content, ClipContent::Adjustment)))
        })
        .ok_or(MatteProofError::EffectNotFound { clip, effect })?;
    for (index, track) in scratch.tracks.iter_mut().enumerate() {
        let below = adjustment && index < target_track;
        for candidate in track.clips.iter_mut().filter(|c| c.id != clip && !below) {
            candidate.enabled = false;
            candidate.enabled_curve = None;
        }
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

/// PF1 R-1: a stamped control's stamp and its issuance index, both taken
/// under the coalesced lock. The worker applies stamped controls in index
/// order, so concurrent callers whose sends cross cannot reverse them
/// (review A/B F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Issued {
    stamp: FrameStamp,
    index: u64,
}

enum Control {
    SetEventWakeup(Wakeup),
    /// PF1 R-1: the transport controls carry the stamp they were issued with.
    SetDocument(Arc<Document>, Issued),
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
    Play(TimeCode, Issued),
    Pause(Issued),
    /// AU3 §3.9: restart the integrated, range, and true-peak measurement at
    /// the position the meter is being fed.
    ResetLoudness,
    /// PF1 R-4: the agent lane's jobs; the worker only resolves and pushes.
    Thumbnail {
        document: Option<Arc<Document>>,
        at: TimeCode,
        max_width: u32,
        reply: Sender<Result<RgbaImage, MediaError>>,
        cancel: Arc<AtomicBool>,
    },
    PreviewCache {
        clear: bool,
        reply: Sender<Result<CacheStats, MediaError>>,
        cancel: Arc<AtomicBool>,
    },
}

impl Control {
    /// A transport control's issuance, applied in index order (R-1).
    const fn issued(&self) -> Option<Issued> {
        match self {
            Self::SetDocument(_, issued) | Self::Play(_, issued) | Self::Pause(issued) => {
                Some(*issued)
            }
            _ => None,
        }
    }
}

pub struct FfmpegMediaEngine {
    control_tx: Sender<Control>,
    frames_rx: Receiver<PreviewFrame>,
    events_rx: Receiver<MediaEvent>,
    coalesced: Arc<Mutex<Coalesced>>,
    /// PF1 R-4: the agent lane the `thumbnail_*` guards cancel through.
    lane: Arc<Lane>,
    /// PF1 R-5 `sync_decoders`: decoders open in this engine's renderers.
    decoders: crate::render::DecoderGauge,
    /// PF1 R-5: this engine's output-callback underrun counters.
    diagnostics: Arc<AudioDiagnostics>,
    clock: Arc<SharedClock>,
    meter: Arc<MeterState>,
    /// AU1 §4.1: the peak table the worker installs while it is playing, read
    /// by `Playback::mix_peaks`.
    mix_meters: Arc<RwLock<Arc<MixMeters>>>,
    /// AU3 §3.9: the live loudness the worker publishes by audible position,
    /// read by `Playback::loudness`.
    loudness: Arc<LiveLoudness>,
    /// AU6 §13: post-master monitor gain the callback applies. Shared with the
    /// worker so a hold is heard without a control-channel round trip.
    monitor_gain_tenth_db: Arc<AtomicI32>,
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
    /// Re-review B D6: disconnects once the worker thread has finished.
    #[cfg(test)]
    finished: Receiver<()>,
}

impl FfmpegMediaEngine {
    /// Wake an event-driven consumer after publishing a preview frame or event.
    ///
    /// Install this before requesting work. The callback runs on the media
    /// worker and must return promptly; it should schedule UI work, not do it.
    pub fn set_event_wakeup(&self, wakeup: impl Fn() + Send + Sync + 'static) {
        let _ = self
            .control_tx
            .send(Control::SetEventWakeup(Arc::new(wakeup)));
    }

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
        Self::start(gpu, data_dir, analysis_config, EngineOptions::default())
    }

    /// PF1 S0: an engine on the V-5 simulated output (`None` keeps the
    /// device) with the Q-3 faults armed by the harness, which shares the
    /// engine's audio `diagnostics` (R23). Test builds only.
    #[cfg(test)]
    pub(crate) fn new_for_harness(
        gpu: GpuContext,
        data_dir: PathBuf,
        audio: Option<crate::audio::simulated::SimulatedAudio>,
        faults: Arc<Faults>,
        diagnostics: Arc<AudioDiagnostics>,
    ) -> Result<Self, MediaError> {
        let config = DerivedAnalysisConfig::default();
        let options = EngineOptions {
            output_device: audio.map_or(OutputDevice::Default, OutputDevice::Simulated),
            faults,
            diagnostics,
        };
        Self::start(gpu, data_dir, config, options)
    }

    // The engine's threads and their shared state are wired in one place
    // (the test-only teardown signal takes it past the limit).
    #[allow(clippy::too_many_lines)]
    fn start(
        gpu: GpuContext,
        data_dir: PathBuf,
        analysis_config: DerivedAnalysisConfig,
        options: EngineOptions,
    ) -> Result<Self, MediaError> {
        crate::initialize_ffmpeg()?;
        let data_dir_for_self = data_dir.clone();
        let (control_tx, control_rx) = unbounded();
        let (frames_tx, frames_rx) = bounded(2);
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let worker_clock = Arc::clone(&clock);
        let lane = Arc::new(Lane::default());
        let decoders = crate::render::DecoderGauge::default();
        let preview = {
            let (lane, clock, gpu) = (Arc::clone(&lane), Arc::clone(&clock), gpu.clone());
            let gauge = decoders.clone();
            let frames = (frames_tx, frames_rx.clone());
            #[cfg(test)]
            let faults = Arc::clone(&options.faults);
            spawn_preview(move || {
                let renderer = gauge.scope(|| FrameRenderer::new_preview(gpu));
                #[allow(unused_mut)]
                let mut preview = Preview::new(lane, renderer, clock, frames);
                #[cfg(test)]
                {
                    preview.faults = faults;
                }
                preview
            })?
        };
        let meter = Arc::new(MeterState::default());
        let worker_meter = Arc::clone(&meter);
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let worker_mix_meters = Arc::clone(&mix_meters);
        let loudness = Arc::new(LiveLoudness::default());
        let worker_loudness = Arc::clone(&loudness);
        let monitor_gain_tenth_db = Arc::new(AtomicI32::new(0));
        let worker_monitor_gain = Arc::clone(&monitor_gain_tenth_db);
        let events_drop_rx = events_rx.clone();
        let coalesced = Arc::new(Mutex::new(Coalesced::default()));
        let worker_coalesced = Arc::clone(&coalesced);
        let worker_lane = Arc::clone(&lane);
        let lut_lattices = Arc::new(RwLock::new(PublishedLattices::default()));
        let worker_lut_lattices = Arc::clone(&lut_lattices);
        let diagnostics = Arc::clone(&options.diagnostics);
        #[cfg(test)]
        let (finished_tx, finished) = bounded::<()>(0);
        let spawned = thread::Builder::new()
            .name("kinewright-media".to_owned())
            .spawn(move || {
                let mut worker = Worker::new(
                    WorkerChannels {
                        control_rx,
                        events_tx,
                        events_drop_rx,
                        lane: worker_lane,
                    },
                    worker_clock,
                    worker_meter,
                    worker_mix_meters,
                    worker_loudness,
                    worker_coalesced,
                    worker_lut_lattices,
                    worker_monitor_gain,
                );
                worker.output_device = options.output_device;
                worker.audio_diagnostics = options.diagnostics;
                #[cfg(test)]
                {
                    worker.faults = options.faults;
                }
                worker.preview = Some(preview);
                worker.run();
                #[cfg(test)]
                drop(finished_tx);
            });
        if let Err(error) = spawned {
            // The worker never started: stop the preview it would have joined.
            drop(lane.shut_down());
            return Err(MediaError::Backend(error.to_string()));
        }

        let visual_assets = VisualAssetService::new(&data_dir)?;
        let derived_analysis = DerivedAnalysisService::new(&data_dir, analysis_config)?;
        Ok(Self {
            control_tx,
            frames_rx,
            events_rx,
            coalesced,
            lane,
            decoders,
            diagnostics,
            clock,
            meter,
            mix_meters,
            loudness,
            monitor_gain_tenth_db,
            next_asset_id: AtomicU64::new(1),
            data_dir: data_dir_for_self,
            gpu,
            export_document: Arc::new(RwLock::new(Arc::new(Document::default()))),
            lut_lattices,
            transcripts: TranscriptService::new(data_dir)?,
            visual_assets,
            derived_analysis,
            #[cfg(test)]
            finished,
        })
    }

    /// Re-review B D6 (harness teardown): a receiver that disconnects once
    /// the worker thread has finished everything the engine's drop starts:
    /// its lane shut down, the preview thread joined (its renderer and
    /// decoders dropped), the worker and its audio runtime dropped.
    #[cfg(test)]
    pub(crate) fn finished(&self) -> Receiver<()> {
        self.finished.clone()
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
        self.agent_request(|reply, cancel| Control::PreviewCache {
            clear,
            reply,
            cancel,
        })
    }

    /// The harness's handle on this engine's decoder gauge (teardown).
    #[cfg(test)]
    pub(crate) fn decoder_gauge(&self) -> crate::render::DecoderGauge {
        self.decoders.clone()
    }

    /// PF1 R-4: send one agent-lane control and wait for its one reply,
    /// holding the guard that cancels it if this caller goes away.
    fn agent_request<T>(
        &self,
        control: impl FnOnce(Sender<Result<T, MediaError>>, Arc<AtomicBool>) -> Control,
    ) -> Result<T, MediaError> {
        let (reply, response) = bounded(1);
        let guard = CancelOnDrop::new(&self.lane);
        self.control_tx
            .send(control(reply, guard.flag()))
            .map_err(|_| worker_stopped())?;
        // R-4 (review B F2): a caller's `AgentCancel` (the MCP server's, when
        // a client cancels the tool call) stops the wait; dropping the guard
        // then discards the queued job unanswered.
        let result = match kinewright_core::AgentCancel::current() {
            None => response.recv().map_err(|_| worker_stopped())?,
            Some(token) => loop {
                match response.recv_timeout(AGENT_CANCEL_POLL) {
                    Ok(result) => break result,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Err(worker_stopped());
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) if token.is_cancelled() => {
                        drop(guard);
                        let cancelled = "preview-thread: agent request cancelled";
                        return Err(MediaError::Backend(cancelled.to_owned()));
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }
            },
        };
        drop(guard);
        result
    }

    /// Test support (review B F2): occupy the preview thread with an agent
    /// job until the returned hold drops, so later agent requests wait.
    ///
    /// # Panics
    ///
    /// If the agent lane refuses the job or the preview thread is gone.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn hold_agent_lane(&self) -> AgentLaneHold {
        let (started, begun) = bounded(1);
        let (release_tx, release) = bounded::<()>(0);
        let job = AgentJob {
            work: AgentWork::Hold { started, release },
            cancel: Arc::default(),
        };
        assert!(self.lane.try_push(job), "the agent lane took the hold");
        begun.recv().expect("the preview thread runs the hold");
        AgentLaneHold {
            lane: Arc::clone(&self.lane),
            _release: release_tx,
        }
    }

    fn coalesced(&self) -> MutexGuard<'_, Coalesced> {
        self.coalesced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// PF1 R-1: send a control issued under the coalesced lock, after
    /// unlock (H-4). The worker applies it in issuance order, whenever the
    /// send lands.
    fn send_issued(&self, control: Control) {
        #[cfg(test)]
        if let Some(hook) = BETWEEN_ISSUE_AND_SEND.with(std::cell::RefCell::take) {
            hook();
        }
        let _ = self.control_tx.send(control);
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

/// How `start` configures the worker and the preview.
struct EngineOptions {
    /// PF1 V-5: the default device, or the harness's simulated output.
    output_device: OutputDevice,
    /// PF1 R23: this engine's output-callback diagnostics.
    diagnostics: Arc<AudioDiagnostics>,
    #[cfg(test)]
    faults: Arc<Faults>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            output_device: OutputDevice::Default,
            diagnostics: Arc::default(),
            #[cfg(test)]
            faults: Arc::default(),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// R-1 witness: runs once on this thread between a control's issue and
    /// its send, so a test can reverse two senders on the wire.
    pub(crate) static BETWEEN_ISSUE_AND_SEND: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// E-2 witness: the next preview spawn on this thread fails.
    pub(crate) static FAIL_PREVIEW_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// PF1 H-1: start the preview thread, which builds its own renderer. E-2: a
/// spawn failure is new, so it is prefixed `preview-thread:`.
fn spawn_preview(
    make: impl FnOnce() -> Preview + Send + 'static,
) -> Result<JoinHandle<()>, MediaError> {
    let spawned = thread::Builder::new().name("kinewright-preview".to_owned());
    #[cfg(test)]
    if FAIL_PREVIEW_SPAWN.with(std::cell::Cell::take) {
        return Err(MediaError::Backend(
            "preview-thread: spawn failed: injected".to_owned(),
        ));
    }
    spawned
        .spawn(move || make().run())
        .map_err(|error| MediaError::Backend(format!("preview-thread: spawn failed: {error}")))
}

/// PF1 R-1: one coalesced `(target, stamp)` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Request {
    at: TimeCode,
    stamp: FrameStamp,
    /// Stamped controls issued before it; the worker posts it only once it
    /// has applied that many.
    controls_before: u64,
}

/// PF1 R-1: the stamp source and the coalesced seek/frame slots, one leaf
/// lock (H-4) replacing today's atomics.
#[derive(Debug, Default)]
struct Coalesced {
    /// The newest stamp issued.
    latest: FrameStamp,
    /// Stamped controls issued.
    controls: u64,
    seek: Option<Request>,
    frame: Option<Request>,
    /// PF1 V-2 (review B F1): bumped by the worker's terminal stop before it
    /// publishes `Paused`, so a seek can tell whether it saw that stop.
    eos_generation: u64,
    /// The `eos_generation` the latest seek observed when it was published.
    seek_eos_generation: u64,
    /// Every call issued, with its stamp (tests).
    #[cfg(test)]
    log: Vec<(FrameStamp, Call)>,
}

impl Coalesced {
    fn issue(&mut self, epoch: bool) -> FrameStamp {
        self.latest.seq += 1;
        self.latest.epoch += u64::from(epoch);
        self.latest
    }

    /// `set_document`, `play`, `pause`: a new epoch, sent as a control.
    fn control(&mut self) -> Issued {
        self.controls += 1;
        let stamp = self.issue(true);
        Issued {
            stamp,
            index: self.controls,
        }
    }

    // PF1 R-1: each transport call is issued here, under the coalesced
    // lock: its stamp, its index and its caller-side clock effect together,
    // so `tick` can tell a clock a call moved from the playback the worker
    // has applied (review B F3). A control is sent after unlock.

    fn set_document(&mut self, doc: Arc<Document>, clock: &SharedClock) -> Control {
        clock.set_fps(doc.fps);
        let issued = self.control();
        #[cfg(test)]
        self.log
            .push((issued.stamp, Call::Document(doc.duration.0)));
        Control::SetDocument(doc, issued)
    }

    fn play(&mut self, from: TimeCode, clock: &SharedClock) -> Control {
        clock.set_frame(from);
        let issued = self.control();
        #[cfg(test)]
        self.log.push((issued.stamp, Call::Play(from)));
        Control::Play(from, issued)
    }

    fn pause(&mut self) -> Control {
        let issued = self.control();
        #[cfg(test)]
        self.log.push((issued.stamp, Call::Pause));
        Control::Pause(issued)
    }

    fn request(&mut self, at: TimeCode, epoch: bool) -> Request {
        Request {
            at: TimeCode(at.0.max(0)),
            stamp: self.issue(epoch),
            controls_before: self.controls,
        }
    }

    fn request_frame(&mut self, at: TimeCode) {
        let request = self.request(at, false);
        #[cfg(test)]
        self.log.push((request.stamp, Call::Frame(request.at)));
        self.frame = Some(request);
    }

    /// A seek supersedes any pending frame request and records the
    /// terminal-stop generation it saw.
    fn seek(&mut self, to: TimeCode, clock: &SharedClock) {
        clock.set_frame(to);
        let request = self.request(to, true);
        #[cfg(test)]
        self.log.push((request.stamp, Call::Seek(request.at)));
        self.seek = Some(request);
        self.frame = None;
        self.seek_eos_generation = self.eos_generation;
    }
}

/// PF1 I8: one issued transport call, as issued (the tests' oracle).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Document(i64),
    Play(TimeCode),
    Pause,
    Seek(TimeCode),
    Frame(TimeCode),
}

/// `Playback::ack_presented` (R34): the ack is queued for the worker,
/// which alone settles it against the due frames it registered. It reads
/// no clock and registers nothing.
fn acknowledge(lane: &Lane, stamp: FrameStamp, at: TimeCode, painted: Instant, expired: bool) {
    let (epoch, at) = (stamp.epoch, at.0);
    (lane.counters()).ack(Ack {
        epoch,
        at,
        painted,
        expired,
    });
}

impl Playback for FfmpegMediaEngine {
    fn set_document(&self, doc: Arc<Document>) {
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
        let control = self.coalesced().set_document(doc, &self.clock);
        self.send_issued(control);
    }

    fn request_frame(&self, at: TimeCode) {
        self.coalesced().request_frame(at);
    }

    fn frames(&self) -> Receiver<PreviewFrame> {
        self.frames_rx.clone()
    }

    fn events(&self) -> Receiver<MediaEvent> {
        self.events_rx.clone()
    }

    fn play(&self, from: TimeCode) {
        self.clear_mix_meters();
        let control = self.coalesced().play(from, &self.clock);
        self.send_issued(control);
    }

    fn pause(&self) {
        self.clear_mix_meters();
        let control = self.coalesced().pause();
        self.send_issued(control);
    }

    fn seek(&self, to: TimeCode) {
        self.coalesced().seek(to, &self.clock);
    }

    fn stamp(&self) -> FrameStamp {
        self.coalesced().latest
    }

    fn stats(&self) -> PlaybackStats {
        let (mut stats, base, playing) = {
            let counters = self.lane.counters();
            (counters.stats, counters.underrun_base, counters.playing())
        };
        let underruns = self.diagnostics.underruns();
        let since = |index: usize| underruns[index].saturating_sub(base[index]);
        stats.underrun_events = since(0);
        stats.underrun_frames = since(1);
        stats.post_end_underrun_events = since(2);
        stats.post_end_underrun_frames = since(3);
        stats.max_clock_stall_ms = self.diagnostics.max_stall_ms(playing);
        stats.sync_decoders = self.decoders.open();
        stats.live_table_bytes =
            u64::try_from(crate::conversion::live_table_bytes()).unwrap_or(u64::MAX);
        stats
    }

    /// R-5: an ack of the image of `at` in `stamp`'s epoch, painted at
    /// `painted`. Only a due frame of that epoch counts (review A F5: an
    /// image painted before a stop still counts when acked after it).
    fn ack_presented(&self, stamp: FrameStamp, at: TimeCode, painted: Instant, expired: bool) {
        acknowledge(&self.lane, stamp, at, painted, expired);
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

    fn set_monitor_gain_tenth_db(&self, gain_tenth_db: i32) {
        self.monitor_gain_tenth_db
            .store(gain_tenth_db, Ordering::Relaxed);
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
        self.agent_request(|reply, cancel| Control::Thumbnail {
            document: None,
            at,
            max_width,
            reply,
            cancel,
        })
    }

    fn thumbnail_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        max_width: u32,
    ) -> Result<RgbaImage, MediaError> {
        self.agent_request(|reply, cancel| Control::Thumbnail {
            document: Some(document),
            at,
            max_width,
            reply,
            cancel,
        })
    }

    fn monitor_proof_for_document(
        &self,
        document: Arc<Document>,
        at: TimeCode,
    ) -> Result<MonitorProof, MediaError> {
        let mut renderer = self.decoders.scope(|| FrameRenderer::new(self.gpu.clone()));
        renderer.set_lut_library(self.document_lut_library(&document)?);
        let resolution = document.resolution;
        let scale = RenderScale::FullResolution;
        let frame = renderer.render(&document, at, resolution, scale, DecodeStrategy::Seek)?;
        Ok(MonitorProof {
            image: RgbaImage {
                width: frame.width,
                height: frame.height,
                pixels: (*frame.rgba).clone(),
            },
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
        let (timeline_start, target) = locate_color_node(&document, clip, effect)?;
        let node_kind = target.name.clone();
        let local_at = at.checked_sub(timeline_start).unwrap_or(TimeCode::ZERO);
        // MO1 R4: a disabled node renders nothing, so it has no coverage to
        // prove — `NodeInactive`, not `EffectNotFound`, because the recovery
        // (re-enable) differs from a wrong node id (CC5 §4.1).
        if !target.is_enabled_at(local_at) {
            return Err(MatteProofError::NodeInactive {
                reason: "disabled".to_owned(),
            }
            .into());
        }
        let matte = MatteParams::from_effect(&target.evaluated_at(local_at));

        let scratch = matte_proof_scratch_document(&document, clip, effect)?;
        let mut renderer = self.decoders.scope(|| FrameRenderer::new(self.gpu.clone()));
        renderer.set_lut_library(self.document_lut_library(&scratch)?);
        let resolution = scratch.resolution;
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
            pixels.extend_from_slice(&[*coverage, *coverage, *coverage, u8::MAX]);
        }
        Ok(MatteProof {
            coverage: RgbaImage {
                width: raster.width,
                height: raster.height,
                pixels,
            },
            metadata: MatteProofMetadata {
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
        let mut renderer = self.decoders.scope(|| FrameRenderer::new(self.gpu.clone()));
        renderer.set_lut_library(self.document_lut_library(&document)?);
        let resolution = document.resolution;
        let scale = RenderScale::FullResolution;
        let image =
            renderer.render_working(&document, at, resolution, scale, DecodeStrategy::Seek)?;
        let raster_aspect_millionths = raster_aspect_millionths(image.width, image.height);
        Ok(WorkingProof {
            metadata: WorkingProofMetadata {
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
        self.decoders
            .scope(|| {
                crate::export::export_document_with_luts(
                    &document,
                    out,
                    &settings,
                    &progress,
                    self.gpu.clone(),
                    library,
                )
            })
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
        let library = self.document_lut_library(&document)?;
        self.decoders.scope(|| {
            crate::export::export_document_with_luts(
                &document,
                out,
                &settings,
                &progress,
                self.gpu.clone(),
                library,
            )
        })
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
    event_wakeup: Option<Wakeup>,
    events_tx: Sender<MediaEvent>,
    events_drop_rx: Receiver<MediaEvent>,
    clock: Arc<SharedClock>,
    meter: Arc<MeterState>,
    /// AU1 §4.1: shared with the engine; holds `MixMeters::empty` whenever the
    /// worker is not playing.
    mix_meters: Arc<RwLock<Arc<MixMeters>>>,
    coalesced: Arc<Mutex<Coalesced>>,
    /// PF1 R-1: stamped controls applied (they apply in issuance order).
    applied_controls: u64,
    /// Stamped controls that arrived ahead of an earlier one, by index.
    stashed: BTreeMap<u64, Control>,
    /// The newest stamp applied or handled. Only a job the worker starts on
    /// its own (a live-audio re-cue, which no transport call stamps) carries
    /// it; every other job carries its own control's or request's stamp.
    handled: FrameStamp,
    /// PF1 H-1: the preview thread's jobs and agent lane.
    lane: Arc<Lane>,
    /// Joined at shutdown (H-6); `None` for a worker a test drives.
    preview: Option<JoinHandle<()>>,
    /// Bumped by every `set_document`, so the preview clears its caches.
    generation: u64,
    document: Arc<Document>,
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
    monitor_gain_tenth_db: Arc<AtomicI32>,
    playing: bool,
    /// PF1 V-2 (review B F1): the transport stopped at the end on its own,
    /// not by a pause; a seek published before that stop resumes playing.
    resume_after_eos: bool,
    last_position: Option<TimeCode>,
    /// PF1 V-5: the default device, or the harness's simulated output.
    output_device: OutputDevice,
    /// PF1 R23: this engine's output-callback diagnostics.
    audio_diagnostics: Arc<AudioDiagnostics>,
    /// PF1 R-2: stamped failures suppressed as superseded or old-epoch.
    #[cfg(test)]
    faults: Arc<Faults>,
}

/// PF1 Q-3: the faults the harness's controls inject into the worker.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Faults {
    /// Slowdown: added to every preview render.
    pub(crate) render_delay_ms: AtomicU64,
    /// Freeze: renders finishing before this instant are not published.
    pub(crate) unpublished_until: std::sync::Mutex<Option<std::time::Instant>>,
    /// Stall: the next playing tick sleeps this long before it fills (once).
    pub(crate) fill_stall_ms: AtomicU64,
    /// PF1 S2a model tests: the preview skips the GPU and publishes 1×1.
    pub(crate) fake_render: AtomicBool,
    /// The next preview render fails (once).
    pub(crate) fail_render: AtomicBool,
    /// A playback hold checks once and returns `Pending` (stepped models).
    pub(crate) step_hold: AtomicBool,
    /// Signalled once when a playback hold begins.
    pub(crate) on_hold: std::sync::Mutex<Option<Sender<()>>>,
    /// Runs once inside the next agent job (a stepped model's clock moves
    /// while it renders).
    pub(crate) on_agent: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// PF1 I8 cheap model: playback opens no audio runtime; the model moves
    /// the clock itself.
    fake_audio: AtomicBool,
    /// Coalesced requests a later control superseded (I8 coverage).
    superseded: AtomicU64,
    /// Stamped controls in the order the worker applied them.
    applied: std::sync::Mutex<Vec<Issued>>,
}

#[cfg(test)]
impl Faults {
    pub(crate) fn publish_after_render(&self) -> bool {
        thread::sleep(Duration::from_millis(
            self.render_delay_ms.load(Ordering::Relaxed),
        ));
        let until = *self.unpublished_until.lock().expect("fault state");
        until.is_none_or(|until| std::time::Instant::now() >= until)
    }

    fn stall_fill(&self) {
        thread::sleep(Duration::from_millis(
            self.fill_stall_ms.swap(0, Ordering::Relaxed),
        ));
    }
}

struct WorkerChannels {
    control_rx: Receiver<Control>,
    events_tx: Sender<MediaEvent>,
    events_drop_rx: Receiver<MediaEvent>,
    lane: Arc<Lane>,
}

impl Worker {
    #[allow(clippy::too_many_arguments)]
    fn new(
        channels: WorkerChannels,
        clock: Arc<SharedClock>,
        meter: Arc<MeterState>,
        mix_meters: Arc<RwLock<Arc<MixMeters>>>,
        loudness: Arc<LiveLoudness>,
        coalesced: Arc<Mutex<Coalesced>>,
        lut_lattices: Arc<RwLock<PublishedLattices>>,
        monitor_gain_tenth_db: Arc<AtomicI32>,
    ) -> Self {
        Self {
            control_rx: channels.control_rx,
            event_wakeup: None,
            events_tx: channels.events_tx,
            events_drop_rx: channels.events_drop_rx,
            clock,
            meter,
            mix_meters,
            coalesced,
            applied_controls: 0,
            stashed: BTreeMap::new(),
            handled: FrameStamp::default(),
            lane: channels.lane,
            preview: None,
            generation: 0,
            document: Arc::new(Document::default()),
            lut_lattices,
            lut_library: Arc::new(LutLibrary::default()),
            audio: None,
            loudness: WorkerLoudness::new(loudness),
            monitor_gain_tenth_db,
            playing: false,
            resume_after_eos: false,
            last_position: None,
            output_device: OutputDevice::Default,
            audio_diagnostics: Arc::default(),
            #[cfg(test)]
            faults: Arc::default(),
        }
    }

    fn run(mut self) {
        loop {
            match self.control_rx.recv_timeout(WORKER_TICK) {
                Ok(control) => {
                    self.handle_control(control);
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
            self.handle_preview_failures();
            self.tick();
        }
        self.shut_down();
    }

    /// PF1 H-6: the control channel disconnected.
    fn shut_down(&mut self) {
        let (moved, transport) = self.lane.shut_down();
        drop(transport);
        for job in moved {
            job.reply_error(worker_stopped());
        }
        self.audio = None;
        if let Some(preview) = self.preview.take() {
            let _ = preview.join();
        }
    }

    /// PF1 R-1 (reviews A/B F1): transport calls take effect in issuance
    /// order. A stamped control whose send overtook an earlier control's
    /// waits for it; every other control applies at once.
    fn handle_control(&mut self, control: Control) {
        if let Some(issued) = control.issued()
            && issued.index != self.applied_controls + 1
        {
            debug_assert!(issued.index > self.applied_controls, "{issued:?}");
            self.stashed.insert(issued.index, control);
            return;
        }
        self.apply(control);
        while let Some(next) = self.stashed.remove(&(self.applied_controls + 1)) {
            self.apply(next);
        }
    }

    /// PF1 R-1: the coalesced requests issued before stamped control
    /// `issued`, taken as it applies. The control supersedes them (they
    /// post nothing, never relabelled), except that a seek issued before a
    /// pause decides where that pause rests.
    fn take_requests_before(&mut self, issued: Issued) -> Option<Request> {
        let mut coalesced = self.lock_coalesced();
        let before = |slot: &mut Option<Request>| {
            slot.take_if(|request| request.controls_before < issued.index)
        };
        let (seek, frame) = (before(&mut coalesced.seek), before(&mut coalesced.frame));
        drop(coalesced);
        for request in seek.iter().chain(&frame) {
            self.handled = self.handled.max(request.stamp);
            #[cfg(test)]
            self.faults.superseded.fetch_add(1, Ordering::Relaxed);
        }
        seek
    }

    fn apply(&mut self, control: Control) {
        match control {
            Control::SetEventWakeup(wakeup) => {
                self.lane.set_wakeup(Arc::clone(&wakeup));
                self.event_wakeup = Some(wakeup);
            }
            Control::SetDocument(doc, issued) => {
                self.take_requests_before(issued);
                self.apply_control(issued);
                self.set_document(&doc, issued.stamp);
            }
            Control::LutLatticesPublished => self.rebind_lut_library(),
            Control::UpdateAudio(kind, doc) => self.update_audio(kind, doc),
            Control::Play(from, issued) => {
                self.take_requests_before(issued);
                self.apply_control(issued);
                let underruns = self.audio_diagnostics.underruns();
                self.lane.counters().clear(underruns);
                self.audio_diagnostics.reset_stall();
                self.start_playback(from, issued.stamp);
                if !self.playing {
                    self.post_resting(from, issued.stamp);
                }
            }
            Control::Pause(issued) => {
                // A seek issued before the pause is where it rests: it is
                // applied paused, and the pause's own image shows it.
                let seek = self.take_requests_before(issued);
                self.apply_control(issued);
                self.pause_or_stop_at_end(seek.is_some());
                let at = seek.map_or_else(
                    || self.clock.position(),
                    |seek| {
                        self.clock.set_frame(seek.at);
                        self.emit(MediaEvent::Position(seek.at));
                        seek.at
                    },
                );
                self.post_resting(at, issued.stamp);
            }
            Control::ResetLoudness => self.reset_loudness(),
            Control::Thumbnail {
                document,
                at,
                max_width,
                reply,
                cancel,
            } => {
                let document = document.unwrap_or_else(|| Arc::clone(&self.document));
                let lut = self.bound_lut_library(&document);
                let work = AgentWork::Thumbnail {
                    document,
                    lut,
                    at,
                    max_width,
                    reply,
                };
                self.push_agent(AgentJob { work, cancel });
            }
            Control::PreviewCache {
                clear,
                reply,
                cancel,
            } => {
                let work = AgentWork::CacheStats { clear, reply };
                self.push_agent(AgentJob { work, cancel });
            }
        }
    }

    /// PF1 R-4: push without waiting; a full queue replies at once.
    fn push_agent(&self, job: AgentJob) {
        self.lane.try_push(job);
    }

    fn apply_control(&mut self, issued: Issued) {
        self.applied_controls = issued.index;
        self.handled = self.handled.max(issued.stamp);
        #[cfg(test)]
        self.faults
            .applied
            .lock()
            .expect("fault state")
            .push(issued);
    }

    /// R34 (review B F3, re-review 2 D1): the playback the worker applied,
    /// from its own runtime. A caller's seek or play moves the shared clock
    /// when it is issued, and a worker transition writes it too; neither
    /// touches the samples the runtime's callback consumed, so R-5 registers
    /// only playback that happened.
    fn runtime_position(&self) -> Option<TimeCode> {
        let audio = self.audio.as_ref()?;
        let samples = self.clock.position_samples.load(Ordering::Acquire);
        Some(samples_to_frame(
            samples,
            audio.sample_rate(),
            self.document.fps,
        ))
    }

    /// The runtime's position; without one (the I8 model's fake audio,
    /// whose clock the model advances), the clock if every issued call is
    /// applied.
    fn applied_position(&self) -> Option<TimeCode> {
        if self.audio.is_some() {
            return self.runtime_position();
        }
        let coalesced = self.lock_coalesced();
        let settled = coalesced.seek.is_none() && coalesced.controls == self.applied_controls;
        settled.then(|| self.clock.position())
    }

    fn lock_coalesced(&self) -> MutexGuard<'_, Coalesced> {
        self.coalesced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn scene(&self) -> Scene {
        Scene {
            document: Arc::clone(&self.document),
            lut: Arc::clone(&self.lut_library),
            generation: self.generation,
        }
    }

    /// R-1: a job carries the stamp of the control or request that posts
    /// it, bound to the scene current at that stamp.
    fn post(&self, kind: JobKind, stamp: FrameStamp) {
        let job = TransportJob {
            kind,
            stamp,
            scene: self.scene(),
        };
        self.lane.post(Some(job));
    }

    /// S-1: the paused slot's newest target, exact.
    fn post_paused(&self, at: TimeCode, stamp: FrameStamp) {
        self.post(JobKind::Paused(at), stamp);
    }

    /// PF1 R-2: a `play` or `pause` epoch gets its own image of the frame
    /// the transport rests on, clamped into the programme.
    fn post_resting(&self, at: TimeCode, stamp: FrameStamp) {
        let last = self.document.duration.0.saturating_sub(1);
        self.post_paused(TimeCode(at.0.min(last).max(0)), stamp);
    }

    /// PF1 R-2: a stamped preview failure stops playback only while it is
    /// current and not superseded; anything older is only counted.
    fn handle_preview_failures(&mut self) {
        for (stamp, error) in self.lane.take_failures() {
            if stamp.is_current(self.lock_coalesced().latest) {
                self.pause();
                self.emit(MediaEvent::StampedError(stamp, error));
            } else {
                self.lane.counters().stats.stale_errors += 1;
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
        self.lane.rebind(self.generation, &self.lut_library);
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

    /// AU2 §5.8: "a pause and re-cue", not a rewind. No transport call
    /// stamped it, so its image carries the newest stamp handled (R-1).
    fn recue_audio(&mut self, doc: &Arc<Document>) {
        let at = self.clock.position();
        let stamp = self.handled;
        self.set_document(doc, stamp);
        let at = TimeCode(at.0.clamp(0, self.document.duration.0.saturating_sub(1)));
        self.clock.set_frame(at);
        self.emit(MediaEvent::Position(at));
        self.post_paused(at, stamp);
    }

    fn update_audio_mix(&mut self, doc: Arc<Document>) -> bool {
        if !can_retarget_audio_mix(&self.document, &doc) {
            self.recue_audio(&doc);
            return false;
        }
        self.document = doc;
        if self.audio.is_none() {
            return true;
        }
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

    fn set_document(&mut self, doc: &Document, stamp: FrameStamp) {
        self.pause();
        // AU3 §3.9: a new document is a new programme.
        self.loudness.reset(None, doc.fps);
        self.document = Arc::new(doc.clone());
        self.generation += 1;
        self.rebind_lut_library();
        self.clock.set_fps(doc.fps);
        self.clock.set_frame(TimeCode::ZERO);
        self.last_position = None;
        self.post_paused(TimeCode::ZERO, stamp);
    }

    /// PF1 R-1/S-1: the coalesced seek and frame requests. A request waits
    /// until every control issued before it is applied (else a loop); the
    /// next control resolves every request issued before it
    /// (`take_requests_before`), so a request taken here is current: it
    /// posts its own job, with its own stamp, bound to its epoch's scene.
    fn handle_coalesced_requests(&mut self) {
        let applied = self.applied_controls;
        let (seek, frame, seek_predates_eos) = {
            let mut coalesced = self.lock_coalesced();
            let issued_before = |slot: &mut Option<Request>| {
                slot.take_if(|request| request.controls_before <= applied)
            };
            let seek = issued_before(&mut coalesced.seek);
            let frame = issued_before(&mut coalesced.frame);
            let predates = coalesced.seek_eos_generation != coalesced.eos_generation;
            (seek, frame, predates)
        };
        for request in seek.iter().chain(&frame) {
            debug_assert_eq!(request.controls_before, applied, "{request:?}");
            self.handled = self.handled.max(request.stamp);
        }
        // A seek clears the frame slot, so a frame request beside a seek is
        // the newer of the two.
        if let Some(seek) = seek {
            let at = seek.at;
            // PF1 V-2 (review B F1): a seek published while playing, which
            // raced the terminal stop, keeps playing.
            let raced_eos = self.resume_after_eos && seek_predates_eos;
            if self.playing || raced_eos {
                self.start_playback(at, seek.stamp);
            } else {
                self.clock.set_frame(at);
                self.emit(MediaEvent::Position(at));
                if frame.is_none() {
                    self.post_paused(at, seek.stamp);
                }
            }
        }
        if let Some(frame) = frame {
            if self.playing {
                // While playing, the request supersedes the playback job
                // with its own, from the clock, under its own stamp.
                let from = self.clock.position();
                self.post(JobKind::Playback { from }, frame.stamp);
            } else {
                self.post_paused(frame.at, frame.stamp);
            }
        }
    }

    fn start_playback(&mut self, from: TimeCode, stamp: FrameStamp) {
        self.resume_after_eos = false;
        // R34 (re-review 2 D2): the outgoing epoch is due through where its
        // runtime reached, so a paint of it acked later still matches.
        let outgoing = self.runtime_position().map(|at| at.0);
        (self.lane.counters()).end(Instant::now(), outgoing);
        self.audio = None;
        self.meter.clear();
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
        match self.open_audio(from, &mix_meters) {
            Ok(runtime) => {
                self.install_mix_meters(mix_meters);
                self.audio = runtime;
                self.playing = true;
                let frame_ms = crate::preview::frame_ms(self.document.fps);
                let end = self.document.duration.0;
                let now = Instant::now();
                (self.lane.counters()).begin(now, from.0, frame_ms, end, stamp.epoch);
                self.emit(MediaEvent::PlaybackStateChanged(PlaybackState::Playing));
                self.post(JobKind::Playback { from }, stamp);
            }
            Err(error) => self.fail(error),
        }
    }

    /// The running audio for `from`; `None` only under the cheap I8 model's
    /// fake audio, whose clock the model advances itself.
    fn open_audio(
        &mut self,
        from: TimeCode,
        mix_meters: &Arc<MixMeters>,
    ) -> Result<Option<AudioRuntime>, MediaError> {
        #[cfg(test)]
        if self.faults.fake_audio.load(Ordering::Acquire) {
            return Ok(None);
        }
        let fps = self.document.fps;
        let mut runtime = self.audio_for_position(from, Arc::clone(mix_meters))?;
        let meter = (self.loudness).begin(from, runtime.sample_rate(), runtime.channels(), fps)?;
        runtime.fill(meter)?;
        runtime.play()?;
        Ok(Some(runtime))
    }

    /// PF1 V-2 (review B F2): a pause that arrives once the programme has
    /// played out, before `tick` saw it, is the terminal stop: the clock
    /// trails the end by up to a frame, so an ordinary pause would stop
    /// short of the duration.
    ///
    /// Re-review D1: "played out" is the current runtime's ring drained,
    /// counted by the samples the callback consumed, never `position()`,
    /// which a caller's `seek` has already moved to the requested frame.
    /// A pending seek decides the position itself, so it also keeps the
    /// ordinary pause (one `Position`, from the seek).
    fn pause_or_stop_at_end(&mut self, seek_before: bool) {
        let seek_pending = seek_before || self.lock_coalesced().seek.is_some();
        if self.playing && self.ring_drained() && !seek_pending {
            self.stop_at_end();
            self.resume_after_eos = false;
        } else if seek_before {
            // The seek moved the clock when it was issued: playback ran
            // through where the runtime's own samples reached (R34).
            self.pause_through(self.runtime_position());
        } else {
            self.pause();
        }
    }

    /// The current runtime's programme has left the ring: the mixer
    /// exhausted, all of it pushed and popped (V-2's drained predicate).
    fn ring_drained(&self) -> bool {
        let samples = self.clock.position_samples.load(Ordering::Acquire);
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.drained(samples))
    }

    /// The programme has played out: the ring drained, or the clock reached
    /// the duration (today's check).
    fn programme_ended(&self) -> bool {
        self.ring_drained() || self.clock.position() >= self.document.duration
    }

    fn pause(&mut self) {
        let through = self.applied_position();
        self.pause_through(through);
    }

    /// Pause; R-5's due frames run through `through`, the applied playback
    /// position, if the clock still shows it (review B F3).
    fn pause_through(&mut self, through: Option<TimeCode>) {
        self.resume_after_eos = false;
        self.lane.post(None);
        if let Some(audio) = &self.audio
            && let Err(error) = audio.pause()
        {
            self.emit(MediaEvent::Error(error));
        }
        let position = self.clock.position();
        self.clock
            .fallback_frame
            .store(position.0, Ordering::Release);
        if self.audio.is_some() {
            self.loudness.pause_at(position, self.document.fps);
        }
        self.audio = None;
        self.meter.clear();
        self.install_mix_meters(Arc::new(MixMeters::empty(Arc::clone(&self.meter))));
        self.clock.sample_rate.store(0, Ordering::Release);
        let now = Instant::now();
        (self.lane.counters()).end(now, through.map(|at| at.0));
        if self.playing {
            self.playing = false;
            self.emit(MediaEvent::PlaybackStateChanged(PlaybackState::Paused));
        }
    }

    /// PF1 V-2: the terminal stop. Unlike `pause()` it never reads the clock,
    /// which trails the end by up to a frame when the programme drains
    /// (a one-frame 30000/1001 timeline ends at sample 1601, clock frame 0).
    fn stop_at_end(&mut self) {
        // Review B F1: a seek stamped before this bump raced the stop.
        self.lock_coalesced().eos_generation += 1;
        self.resume_after_eos = true;
        // Worker-initiated: no stamp, the playback job just ends (R-1).
        self.lane.post(None);
        if let Some(audio) = &self.audio
            && let Err(error) = audio.pause()
        {
            self.emit(MediaEvent::Error(error));
        }
        let end = self.document.duration;
        if self.audio.is_some() {
            self.loudness.pause_at(end, self.document.fps);
        }
        self.audio = None;
        self.meter.clear();
        self.install_mix_meters(Arc::new(MixMeters::empty(Arc::clone(&self.meter))));
        self.clock.fallback_frame.store(end.0, Ordering::Release);
        self.clock.sample_rate.store(0, Ordering::Release);
        self.playing = false;
        // Review A F5: the frames through the last are due.
        self.lane.counters().end(Instant::now(), Some(end.0));
        self.emit(MediaEvent::PlaybackStateChanged(PlaybackState::Paused));
        self.emit(MediaEvent::Position(end));
    }

    fn tick(&mut self) {
        if !self.playing {
            // R34: acks queued after a stop are settled all the same.
            self.lane.counters().sample(Instant::now(), None);
            return;
        }
        #[cfg(test)]
        self.faults.stall_fill();
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
        let applied = self.applied_position().map(|at| at.0);
        (self.lane.counters()).sample(Instant::now(), applied);
        let position = self.clock.position();
        // Review B F1: a seek pending since `handle_coalesced_requests` is
        // applied, still playing, on the next pass instead of the stop.
        let seek_pending = self.lock_coalesced().seek.is_some();
        if !seek_pending && self.programme_ended() {
            self.stop_at_end();
            return;
        }
        // PF1 H-1: the preview thread renders; the tick only reports.
        if self.last_position != Some(position) {
            self.last_position = Some(position);
            self.emit(MediaEvent::Position(position));
        }
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
            Arc::clone(&self.monitor_gain_tenth_db),
            &self.output_device,
            &self.audio_diagnostics,
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
        self.wake_consumer();
    }

    fn wake_consumer(&self) {
        if let Some(wakeup) = &self.event_wakeup {
            wakeup();
        }
    }
}

/// PF1 P-1: the monitor caps the long edge at `PREVIEW_MAX_WIDTH` too, so a
/// 9:16 document previews at 720×1280 and 4:5 at 1024×1280. Only the
/// preview's transport renders use it; every other proxy keeps
/// `PREVIEW_MAX_WIDTH`.
pub(crate) fn monitor_max_width((width, height): (u32, u32)) -> u32 {
    let capped = u64::from(PREVIEW_MAX_WIDTH) * u64::from(width) / u64::from(height.max(1));
    u32::try_from(capped).map_or(PREVIEW_MAX_WIDTH, |capped| capped.min(PREVIEW_MAX_WIDTH))
}

pub(crate) fn send_latest<T: Send>(sender: &Sender<T>, drop_receiver: &Receiver<T>, value: T) {
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
    use std::{
        fs,
        sync::atomic::{AtomicI32, AtomicUsize},
    };

    use kinewright_core::{
        AutomationCurve, Clip, ClipContent, Effect, Keyframe, KeyframeInterpolation, LutAsset,
        LutAssetId, LutAssetKind, LutAssetSource, MediaKind, MediaSourceFingerprint, ParamValue,
        Title, Track, TrackId, TrackKind,
    };

    use super::*;
    use crate::{
        audio::simulated::{CALLBACK_FRAMES, SimulatedAudio},
        cc1_fixtures::fallback_gpu,
        initialize_ffmpeg,
        lut::parse_cube_lut,
        sha256::{sha256_bytes, source_fingerprint},
        test_support::{GeneratedMedia, TempDirectory, single_clip_document},
    };

    #[test]
    fn preview_frames_and_paused_seek_events_wake_a_sleeping_consumer() {
        let temp = TempDirectory::new("media-wakeup");
        let engine = FfmpegMediaEngine::new_with_gpu_and_data_dir(
            fallback_gpu().context(),
            temp.root().to_path_buf(),
        )
        .unwrap();
        let frames = engine.frames();
        let events = engine.events();
        let (wake_tx, wake_rx) = unbounded();
        engine.set_event_wakeup(move || {
            // Consume only in response to the wake, just as a sleeping event
            // loop would. Publication must precede the callback.
            let _ = wake_tx.send((frames.try_recv().ok(), events.try_recv().ok()));
        });

        engine.set_document(Arc::new(Document {
            resolution: (64, 64),
            ..Document::default()
        }));
        let (frame, event) = wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(frame.unwrap().at, TimeCode::ZERO);
        assert!(event.is_none());

        // No playback clock or UI polling is running. A seek must wake for
        // both the position event and the asynchronously rendered frame.
        engine.seek(TimeCode(10));
        let (frame, event) = wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(frame.is_none());
        assert!(matches!(event, Some(MediaEvent::Position(TimeCode(10)))));
        let (frame, event) = wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(frame.unwrap().at, TimeCode(10));
        assert!(event.is_none());

        // Errors must also wake the consumer instead of waiting for input.
        engine.play(TimeCode::ZERO);
        let (frame, event) = wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(frame.is_none());
        assert!(matches!(event, Some(MediaEvent::Error(_))));
    }

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
            assumed_from: None,
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
            enabled: true,
            enabled_curve: None,
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
                    enabled: true,
                    enabled_curve: None,
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
                    blend_mode: kinewright_core::BlendMode::Normal,
                }],
            }],
            lut_assets: vec![asset],
            ..Document::default()
        }
    }

    #[test]
    fn two_projects_sharing_one_asset_id_bind_to_their_own_lattices() {
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
        let mut table = PublishedLattices::default();
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
            enabled: true,
            enabled_curve: None,
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
            // `IN1b` §3.9 rule 36: the refusal is carried typed, and its
            // rendered text still begins with the code token.
            let MediaError::MatteProof(error) = error else {
                panic!("a matte proof refusal is a typed matte-proof error");
            };
            error.to_string()
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
        engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(9),
            )
            .expect("the matte-carrying node on the same clip still proves");
    }

    /// MO1 R4: a disabled node has no coverage to prove. The refusal is
    /// `NodeInactive` with the `disabled` reason — not `EffectNotFound`,
    /// because the recovery is "re-enable", not "fix the effect id" — and
    /// the enabled sibling on the same clip still proves.
    #[test]
    fn mo1_matte_proof_refuses_a_disabled_node_as_inactive() {
        initialize_ffmpeg().expect("FFmpeg should initialize for the matte proof fixture");
        let gpu = fallback_gpu().context();
        let media = matte_source("matte-proof-disabled-node");
        let mut off = wheels_node(7, CENTERED_RECT);
        off.enabled = false;
        let document = matte_document(&media, vec![off, wheels_node(9, CENTERED_RECT)]);
        let engine = FfmpegMediaEngine::new_with_gpu(gpu)
            .expect("media engine should start for the matte proof fixture");

        let error = engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(7),
            )
            .expect_err("a disabled node must not render a coverage frame");
        let MediaError::MatteProof(error) = error else {
            panic!("a matte proof refusal is a typed matte-proof error");
        };
        let message = error.to_string();
        assert!(
            message.starts_with("matte_proof_node_inactive:"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("disabled"),
            "the reason token must be reported: {message}"
        );
        engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(9),
            )
            .expect("the enabled sibling on the same clip still proves");
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
        let MediaError::MatteProof(error) = error else {
            panic!("a matte proof refusal is a typed matte-proof error");
        };
        let message = error.to_string();
        assert!(
            message.starts_with("matte_proof_clip_not_visible:"),
            "unexpected message: {message}"
        );
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
        let absent = engine
            .matte_proof_for_document(
                Arc::clone(&document),
                TimeCode::ZERO,
                ClipId(1),
                EffectId(99),
            )
            .expect_err("an absent node still refuses");
        let MediaError::MatteProof(absent) = absent else {
            panic!("a matte proof refusal is a typed matte-proof error");
        };
        let absent = absent.to_string();
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
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(20),
                        value: 7_500,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
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
                enabled: true,
                enabled_curve: None,
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
            enabled: true,
            enabled_curve: None,
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
            enabled: true,
            enabled_curve: None,
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
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let meter = Arc::new(MeterState::default());
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let mut worker = Worker::new(
            WorkerChannels {
                control_rx,
                events_tx,
                events_drop_rx: events_rx.clone(),
                lane: Arc::default(),
            },
            Arc::clone(&clock),
            Arc::clone(&meter),
            Arc::clone(&mix_meters),
            Arc::new(LiveLoudness::default()),
            Arc::default(),
            Arc::new(RwLock::new(PublishedLattices::default())),
            Arc::new(AtomicI32::new(0)),
        );

        let limiter = |milliseconds: i64| Effect {
            enabled: true,
            enabled_curve: None,
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

        let mut faded = (*deeper).clone();
        faded.audio_mix.buses[0].gain_tenth_db = -60;
        worker.update_audio_mix(Arc::new(faded));
        assert_eq!(worker.clock.position(), TimeCode(10));
        assert_eq!(worker.document.audio_mix.buses[0].gain_tenth_db, -60);
    }

    /// PF1 V-2 (I9): a worker on the stepped simulated output, playing a
    /// title-only (silent) timeline from frame 0; `tick` runs only when a test calls it.
    fn stepped_worker(
        fps: Rational,
        duration: TimeCode,
    ) -> (Worker, SimulatedAudio, Receiver<MediaEvent>) {
        let (mut worker, events_rx) = test_worker();
        let audio = SimulatedAudio::stepped();
        worker.output_device = OutputDevice::Simulated(audio.clone());
        let mut document = crate::perf_fixtures::title_card((64, 64), duration.0);
        document.fps = fps;
        worker.document = Arc::new(document);
        worker.clock.set_fps(fps);
        worker.start_playback(TimeCode::ZERO, FrameStamp::default());
        assert!(
            worker.playing,
            "{:?}",
            events_rx.try_iter().collect::<Vec<_>>()
        );
        (worker, audio, events_rx)
    }

    /// A worker driven from the test thread (no `run` loop).
    fn test_worker() -> (Worker, Receiver<MediaEvent>) {
        let (_control_tx, control_rx) = unbounded::<Control>();
        let (events_tx, events_rx) = bounded(16);
        let meter = Arc::new(MeterState::default());
        let worker = Worker::new(
            WorkerChannels {
                control_rx,
                events_tx,
                events_drop_rx: events_rx.clone(),
                lane: Arc::default(),
            },
            Arc::new(SharedClock::new()),
            Arc::clone(&meter),
            Arc::new(RwLock::new(Arc::new(MixMeters::empty(meter)))),
            Arc::new(LiveLoudness::default()),
            Arc::default(),
            Arc::new(RwLock::new(PublishedLattices::default())),
            Arc::new(AtomicI32::new(0)),
        );
        (worker, events_rx)
    }

    /// E-2: a preview spawn failure is new, so it is prefixed.
    #[test]
    fn a_preview_spawn_failure_is_prefixed() {
        let temp = TempDirectory::new("pf1-preview-spawn");
        FAIL_PREVIEW_SPAWN.with(|fail| fail.set(true));
        let gpu = fallback_gpu().context();
        let started = FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu, temp.root().into());
        let Err(MediaError::Backend(message)) = started else {
            panic!("the engine started without its preview thread");
        };
        assert!(message.starts_with("preview-thread: "), "{message}");
    }

    /// H-6 kill test: dropping the engine disconnects the worker, which
    /// stops the lane and joins the preview; the preview's frame sender then
    /// goes away, so a held receiver disconnects. Re-review B D6: the
    /// harness's teardown signal (`finished`) follows all of that.
    #[test]
    fn dropping_the_engine_stops_the_preview_thread() {
        let temp = TempDirectory::new("pf1-preview-drop");
        let gpu = fallback_gpu().context();
        let engine = FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu, temp.root().into()).unwrap();
        let frames = engine.frames();
        engine.set_document(Arc::new(crate::perf_fixtures::title_card((64, 64), 3)));
        assert!(frames.recv_timeout(Duration::from_secs(60)).is_ok());
        let thumbnail = engine.thumbnail_at(TimeCode(1), 64).expect("a thumbnail");
        assert_eq!(thumbnail.width, 64);
        let finished = engine.finished();
        drop(engine);
        let done = finished.recv_timeout(Duration::from_secs(30));
        assert_eq!(done, Err(crossbeam_channel::RecvTimeoutError::Disconnected));
        // Re-review B D6: the worker thread finishes after joining the
        // preview, so its frame sender is already gone.
        let rest = loop {
            if let Err(error) = frames.try_recv() {
                break error;
            }
        };
        assert_eq!(
            rest,
            crossbeam_channel::TryRecvError::Disconnected,
            "the preview thread outlived the engine"
        );
    }

    /// R-4/E-2: the worker never waits on the agent lane; a full queue gets
    /// the prefixed refusal at once, as the reply.
    #[test]
    fn a_full_agent_queue_is_refused_without_waiting() {
        let (mut worker, _events) = test_worker();
        let mut responses = Vec::new();
        for _ in 0..=crate::preview::AGENT_QUEUE_LIMIT {
            let (reply, response) = bounded(1);
            let cancel = Arc::default();
            worker.handle_control(Control::PreviewCache {
                clear: false,
                reply,
                cancel,
            });
            responses.push(response);
        }
        let refused = responses
            .pop()
            .unwrap()
            .try_recv()
            .expect("an immediate reply");
        let error = MediaError::Backend("preview-thread: agent queue full".to_owned());
        assert_eq!(refused, Err(error));
        assert!(responses.iter().all(Receiver::is_empty));
        worker.shut_down();
        for response in responses {
            assert_eq!(response.try_recv().unwrap(), Err(worker_stopped()));
        }
    }

    fn transport_job(worker: &Worker) -> Option<(JobKind, FrameStamp)> {
        let state = worker.lane.lock();
        state.transport.as_ref().map(|job| (job.kind, job.stamp))
    }

    /// R-1: a request waits for every control issued before it; the job it
    /// posts carries its own stamp and that epoch's document.
    #[test]
    fn a_request_waits_for_the_controls_issued_before_it() {
        let (mut worker, _events) = test_worker();
        let document = Arc::new(crate::perf_fixtures::title_card((64, 64), 9));
        let control = worker.lock_coalesced().control();
        worker.lock_coalesced().request_frame(TimeCode(4));
        worker.handle_coalesced_requests();
        assert_eq!(transport_job(&worker), None, "waits a loop");
        worker.handle_control(Control::SetDocument(Arc::clone(&document), control));
        worker.handle_coalesced_requests();
        let request = FrameStamp { epoch: 1, seq: 2 };
        assert_eq!(
            transport_job(&worker),
            Some((JobKind::Paused(TimeCode(4)), request))
        );
        let scene = worker.lane.lock().transport.as_ref().unwrap().scene.clone();
        assert_eq!(scene.document.duration, document.duration);
    }

    /// R-2: a stamped failure stops playback only while current and not
    /// superseded; anything else counts `stale_errors`.
    #[test]
    fn only_a_current_stamped_failure_stops_playback() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, _audio, events) = stepped_worker(fps, TimeCode(50));
        let issued = worker.lock_coalesced().control().stamp;
        let error = || MediaError::Backend("render".to_owned());
        let old_epoch = FrameStamp {
            epoch: issued.epoch - 1,
            seq: issued.seq,
        };
        worker.lock_coalesced().request_frame(TimeCode(3));
        let superseded = issued;
        worker.lane.lock().failures = vec![(old_epoch, error()), (superseded, error())];
        let _ = events.try_iter().count();
        worker.handle_preview_failures();
        assert!(worker.playing && events.is_empty());
        assert_eq!(worker.lane.counters().stats.stale_errors, 2);
        let current = worker.lock_coalesced().latest;
        worker.lane.lock().failures = vec![(current, error())];
        worker.handle_preview_failures();
        assert!(!worker.playing);
        let events: Vec<_> = events.try_iter().collect();
        assert!(
            events.contains(&MediaEvent::StampedError(current, error())),
            "{events:?}"
        );
    }

    /// Reviews A/B F1: `request_frame(80)` on document A, then
    /// `set_document(B)`, both before the worker runs. B supersedes the
    /// request: the only job is B's own first image, with B's stamp and
    /// scene; the old target is never rendered under B's stamp.
    #[test]
    fn a_request_issued_before_a_new_document_is_superseded_not_relabelled() {
        let (mut worker, _events) = test_worker();
        let a = Arc::new(crate::perf_fixtures::title_card((64, 64), 100));
        let control = worker.lock_coalesced().set_document(a, &worker.clock);
        worker.handle_control(control);
        worker.handle_coalesced_requests();
        worker.lock_coalesced().request_frame(TimeCode(80));
        let b = Arc::new(crate::perf_fixtures::title_card((64, 64), 50));
        let control = worker
            .lock_coalesced()
            .set_document(Arc::clone(&b), &worker.clock);
        let issued = control.issued().unwrap();
        worker.handle_control(control);
        worker.handle_coalesced_requests();
        let state = worker.lane.lock();
        let job = state.transport.as_ref().expect("B's first image");
        assert_eq!(
            (job.kind, job.stamp),
            (JobKind::Paused(TimeCode::ZERO), issued.stamp)
        );
        assert_eq!(job.scene.document.duration, b.duration);
        drop(state);
        assert_eq!(worker.faults.superseded.load(Ordering::Relaxed), 1);
    }

    /// Reviews A/B F1: two stamped controls whose sends reverse on the wire
    /// apply in issuance order: the later-issued document wins.
    #[test]
    fn controls_apply_in_issuance_order_whatever_order_they_arrive() {
        let (mut worker, _events) = test_worker();
        let document = |frames| Arc::new(crate::perf_fixtures::title_card((64, 64), frames));
        let first = worker
            .lock_coalesced()
            .set_document(document(30), &worker.clock);
        let second = worker
            .lock_coalesced()
            .set_document(document(60), &worker.clock);
        worker.handle_control(second);
        assert_eq!(worker.applied_controls, 0, "the second waits for the first");
        worker.handle_control(first);
        assert_eq!(worker.document.duration, TimeCode(60));
        let applied: Vec<_> = worker
            .faults
            .applied
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.index)
            .collect();
        assert_eq!(applied, [1, 2]);
    }

    fn fake_engine(temp: &TempDirectory) -> (FfmpegMediaEngine, Arc<Faults>) {
        let faults = Arc::new(Faults::default());
        faults.fake_render.store(true, Ordering::Release);
        let gpu = fallback_gpu().context();
        let root = temp.root().into();
        let engine = FfmpegMediaEngine::new_for_harness(
            gpu,
            root,
            None,
            Arc::clone(&faults),
            Arc::default(),
        );
        (engine.expect("the engine starts"), faults)
    }

    /// Review B F2: an agent request made inside a caller's `AgentCancel`
    /// stops waiting when the token is cancelled, answers the prefixed E-2
    /// error, and its queued job is cancelled (discarded unanswered). An
    /// unscoped request still waits for its answer.
    #[test]
    fn a_cancelled_caller_stops_waiting_for_its_agent_job() {
        let temp = TempDirectory::new("pf1-agent-cancel");
        let (engine, _faults) = fake_engine(&temp);
        let engine = Arc::new(engine);
        let hold = engine.hold_agent_lane();
        let token = kinewright_core::AgentCancel::default();
        let caller = {
            let (engine, token) = (Arc::clone(&engine), token.clone());
            std::thread::spawn(move || token.scope(|| engine.preview_cache_command(false)))
        };
        let until = |wanted: (usize, usize)| {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while hold.waiting() != wanted {
                assert!(std::time::Instant::now() < deadline, "{:?}", hold.waiting());
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        until((1, 0));
        token.cancel();
        let error = caller.join().unwrap().expect_err("the caller gave up");
        let expected = "preview-thread: agent request cancelled";
        assert_eq!(error, MediaError::Backend(expected.to_owned()));
        assert_eq!(hold.waiting(), (1, 1), "the queued job is cancelled");
        let waiting = {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || engine.preview_cache_command(false))
        };
        until((2, 1));
        drop(hold);
        assert!(
            waiting.join().unwrap().is_ok(),
            "an unscoped caller is answered"
        );
    }

    /// The newest stamp's frame, collecting every frame before it.
    fn frames_until(frames: &Receiver<PreviewFrame>, latest: FrameStamp) -> Vec<PreviewFrame> {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut received = Vec::new();
        while received
            .last()
            .is_none_or(|frame: &PreviewFrame| frame.stamp != latest)
        {
            received.push(frames.recv_deadline(deadline).expect("the newest frame"));
        }
        received
    }

    /// Reviews A/B F1 on real threads: caller 1 issues `set_document(30)`
    /// and is held between issue and send while caller 2 issues and sends
    /// `set_document(60)`. The worker still applies 30 then 60, and the
    /// newest frame renders 60.
    #[test]
    fn two_concurrent_senders_reversed_on_the_wire_apply_in_issuance_order() {
        let temp = TempDirectory::new("pf1-reversed-senders");
        let (engine, faults) = fake_engine(&temp);
        let engine = Arc::new(engine);
        let frames = engine.frames();
        let document = |frames| Arc::new(crate::perf_fixtures::title_card((64, 64), frames));
        let (issued_tx, issued) = bounded::<()>(0);
        let (sent_tx, sent) = bounded::<()>(0);
        let first = {
            let engine = Arc::clone(&engine);
            let a = document(30);
            thread::spawn(move || {
                let hook = Box::new(move || {
                    issued_tx.send(()).unwrap();
                    sent.recv().unwrap();
                });
                BETWEEN_ISSUE_AND_SEND.with(|between| *between.borrow_mut() = Some(hook));
                engine.set_document(a);
            })
        };
        issued.recv().unwrap();
        engine.set_document(document(60));
        sent_tx.send(()).unwrap();
        first.join().unwrap();
        let latest = engine.stamp();
        let newest = frames_until(&frames, latest).pop().unwrap();
        assert_eq!(rendered_duration(&newest), 60, "the later-issued document");
        let applied: Vec<_> = faults
            .applied
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.index)
            .collect();
        assert_eq!(applied, [1, 2]);
    }

    /// Review B (I8/M5): four callers issue documents, seeks, frame
    /// requests and pauses concurrently on a real engine. Every frame the
    /// preview publishes answers an issued call (the engine's issue log):
    /// its target, and the document current at its stamp; controls apply
    /// in issuance order; the final seek ends on screen.
    #[test]
    fn concurrent_callers_never_relabel_a_request() {
        let temp = TempDirectory::new("pf1-concurrent-callers");
        let (engine, faults) = fake_engine(&temp);
        let engine = Arc::new(engine);
        let frames = engine.frames();
        engine.set_document(Arc::new(crate::perf_fixtures::title_card((64, 64), 10)));
        let callers: Vec<_> = (0..4_i64)
            .map(|caller| {
                let engine = Arc::clone(&engine);
                thread::spawn(move || {
                    let mut rng = Seeded(u64::try_from(caller).unwrap() * 7_919 + 1);
                    for step in 0..50 {
                        match rng.below(4) {
                            0 => engine.request_frame(rng.frame(40)),
                            1 => engine.seek(rng.frame(40)),
                            2 => engine.pause(),
                            _ => engine.set_document(Arc::new(crate::perf_fixtures::title_card(
                                (64, 64),
                                11 + caller * 100 + step,
                            ))),
                        }
                    }
                })
            })
            .collect();
        let mut received = Vec::new();
        while callers.iter().any(|caller| !caller.is_finished()) {
            received.extend(frames.recv_timeout(Duration::from_millis(5)).ok());
        }
        for caller in callers {
            caller.join().unwrap();
        }
        let target = TimeCode(7);
        engine.seek(target);
        let latest = engine.stamp();
        received.extend(frames_until(&frames, latest));
        let log: BTreeMap<_, _> = engine.coalesced().log.iter().copied().collect();
        let document_at = |stamp| {
            (log.range(..=stamp).rev())
                .find_map(|(_, call)| match call {
                    Call::Document(frames) => Some(*frames),
                    _ => None,
                })
                .unwrap()
        };
        for frame in &received {
            let call = log[&frame.stamp];
            let document = document_at(frame.stamp);
            assert_eq!(rendered_duration(frame), document, "{call:?}");
            match call {
                Call::Document(_) => assert_eq!(frame.at, TimeCode::ZERO),
                Call::Seek(to) | Call::Frame(to) => assert_eq!(frame.at, to, "{call:?}"),
                Call::Pause => assert!(frame.at.0 < document),
                Call::Play(_) => panic!("never played"),
            }
        }
        let last = received.last().unwrap();
        assert_eq!(
            (last.at, rendered_duration(last)),
            (target, document_at(latest))
        );
        let applied: Vec<_> = faults
            .applied
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.index)
            .collect();
        assert_eq!(applied, (1..=applied.len() as u64).collect::<Vec<_>>());
        assert!(applied.len() > 50, "{}", applied.len());
    }

    /// Review B F3: at frame 10 a caller's `seek(900)` lands after the
    /// worker's request pass and before its tick. The clock shows 900, but
    /// playback never passed 11..=899: the tick samples nothing, and the
    /// applied seek's playback starts with its own first frame.
    #[test]
    fn a_seek_racing_the_tick_manufactures_no_due_frames() {
        let fps = Rational::new(30, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(1_000));
        while worker.clock.position() < TimeCode(10) {
            audio.advance(CALLBACK_FRAMES);
            worker.tick();
        }
        worker.handle_coalesced_requests();
        let due = worker.lane.counters().stats.due_frames;
        worker.lock_coalesced().seek(TimeCode(900), &worker.clock);
        worker.tick();
        assert_eq!(worker.lane.counters().stats.due_frames, due, "no jump");
        worker.handle_coalesced_requests();
        assert!(worker.playing);
        assert_eq!(
            worker.lane.counters().stats.due_frames,
            due + 1,
            "frame 900"
        );
    }

    /// R33 (re-review A D3 / B D1): frame 10 of the playing epoch is
    /// painted, a caller issues `seek(900)`, and the paint is acknowledged
    /// through the production ack path before the worker applies the seek.
    /// No frames 11..=900 become due and the offset stays within a frame
    /// (the R32 ack sampled the caller-side clock: 890 frames due, 29.7 s
    /// off); the applied seek then registers only its own first frame.
    /// R34: the ack is queued, and the worker's next tick settles it.
    #[test]
    fn an_ack_racing_an_unapplied_seek_manufactures_no_due_frames() {
        let fps = Rational::new(30, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(1_000));
        while worker.clock.position() < TimeCode(10) {
            audio.advance(CALLBACK_FRAMES);
            worker.tick();
        }
        let at = worker.clock.position();
        let painted = Instant::now();
        let due = worker.lane.counters().stats.due_frames;
        worker.lock_coalesced().seek(TimeCode(900), &worker.clock);
        acknowledge(&worker.lane, FrameStamp::default(), at, painted, false);
        worker.tick();
        let stats = worker.lane.counters().stats;
        assert_eq!(stats.due_frames, due, "no jump: {stats:?}");
        assert_eq!(stats.on_time + stats.late, 1, "the paint counts");
        assert!(stats.max_av_offset_ms < 34.0, "{stats:?}");
        worker.handle_coalesced_requests();
        assert!(worker.playing);
        assert_eq!(worker.lane.counters().stats.due_frames, due + 1);
    }

    /// Plays to frame 10, then lets the runtime reach a frame the worker's
    /// tick has not registered yet, and returns it.
    fn painted_ahead_of_the_tick(worker: &mut Worker, audio: &SimulatedAudio) -> TimeCode {
        while worker.clock.position() < TimeCode(10) {
            audio.advance(CALLBACK_FRAMES);
            worker.tick();
        }
        let sampled = worker.clock.position();
        while worker.clock.position() == sampled {
            audio.advance(CALLBACK_FRAMES);
        }
        worker.clock.position()
    }

    /// R33 (E11.11.4): an image painted before the worker's tick reached its
    /// frame (the clock moved; no call was issued) is acked through the
    /// production path: its frame is due and on time, with no offset, and
    /// the next tick does not make it dropped. The settle-only ack found no
    /// record, and the tick then registered the frame unacknowledged: 124
    /// on time of 1,800 against the harness's 574 on the first P-play LL
    /// rerun. R34: the ack waits in the queue until the tick registers it.
    #[test]
    fn an_ack_ahead_of_the_workers_tick_counts_on_time() {
        let fps = Rational::new(30, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(1_000));
        let at = painted_ahead_of_the_tick(&mut worker, &audio);
        acknowledge(
            &worker.lane,
            FrameStamp::default(),
            at,
            Instant::now(),
            false,
        );
        worker.tick();
        let stats = worker.lane.counters().stats;
        assert_eq!(stats.on_time, 1, "{stats:?}");
        assert!(stats.max_av_offset_ms < 0.1, "{stats:?}");
    }

    /// R34 (re-review 2 D2): a frame painted ahead of the worker's tick,
    /// whose ack arrives after a caller issued a pause, a seek or a new
    /// document and before the worker applied it, counts on time: the
    /// applied transition ends the epoch through its runtime's position,
    /// which registers the frame, and the held ack settles it. (R33's ack
    /// saw the newer issuance, sampled nothing and was lost; the frame was
    /// then registered unacknowledged.)
    #[test]
    fn an_ack_ahead_of_the_tick_counts_across_a_pause_seek_or_document() {
        let fps = Rational::new(30, 1).unwrap();
        for transition in ["pause", "seek", "document"] {
            let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(1_000));
            let at = painted_ahead_of_the_tick(&mut worker, &audio);
            let painted = Instant::now();
            let due = worker.lane.counters().stats.due_frames;
            let control = match transition {
                "pause" => Some(worker.lock_coalesced().pause()),
                "seek" => {
                    worker.lock_coalesced().seek(TimeCode(900), &worker.clock);
                    None
                }
                _ => {
                    let document = Arc::clone(&worker.document);
                    Some(
                        worker
                            .lock_coalesced()
                            .set_document(document, &worker.clock),
                    )
                }
            };
            acknowledge(&worker.lane, FrameStamp::default(), at, painted, false);
            match control {
                Some(control) => worker.handle_control(control),
                None => worker.handle_coalesced_requests(),
            }
            let stats = worker.lane.counters().stats;
            let seeked = u64::from(transition == "seek");
            assert_eq!(
                stats.due_frames,
                due + 1 + seeked,
                "{transition}: {stats:?}"
            );
            let outcomes = (stats.on_time, stats.acks_unmatched);
            assert_eq!(outcomes, (1, 0), "{transition}: {stats:?}");
            assert!(stats.max_av_offset_ms < 0.1, "{transition}: {stats:?}");
        }
    }

    /// Review A F5: the starting frame is due at `play`, and a one-frame
    /// programme drained before its first tick, stopped by a pause, still
    /// counts it.
    #[test]
    fn the_first_frame_is_due_at_play_even_when_drained_before_a_tick() {
        let fps = Rational::new(30_000, 1_001).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(1));
        assert_eq!(worker.lane.counters().stats.due_frames, 1, "due at play");
        audio.advance(CALLBACK_FRAMES);
        audio.advance(CALLBACK_FRAMES);
        let pause = worker.lock_coalesced().pause();
        worker.handle_control(pause);
        assert_stopped_at_end(&worker, &events, TimeCode(1));
        let stats = worker.lane.counters().stats;
        assert_eq!((stats.due_frames, stats.dropped), (1, 1));
    }

    /// Review A F5: the last frame, painted before the terminal stop and
    /// acknowledged after it (the next root epoch comes after EOS), counts;
    /// the stop registered every frame through the last.
    #[test]
    fn an_ack_after_the_terminal_stop_counts() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(5));
        let painted = std::time::Instant::now();
        assert!(play_out(&mut worker, &audio, 400) < 400);
        assert!(!worker.playing);
        assert_eq!(worker.lane.counters().stats.due_frames, 5);
        acknowledge(
            &worker.lane,
            FrameStamp::default(),
            TimeCode(4),
            painted,
            false,
        );
        worker.tick();
        let stats = worker.lane.counters().stats;
        assert_eq!(stats.on_time + stats.late, 1, "{stats:?}");
        assert_eq!(stats.dropped, 4);
    }

    /// R31 (a): underruns before the programme's end and the straddle
    /// after it are counted apart: a clean play-out has none before the
    /// end; a fill stall mid-programme does.
    #[test]
    fn underruns_before_the_programme_end_are_reported_apart() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(50));
        assert!(play_out(&mut worker, &audio, 400) < 400);
        let [events, frames, post_events, post_frames] = worker.audio_diagnostics.underruns();
        assert_eq!((events, frames), (0, 0), "none before the end");
        assert!(post_events > 0 && post_frames > 0, "the straddle, apart");

        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(50));
        assert_eq!(play_out(&mut worker, &audio, 40), 40);
        for _ in 0..94 {
            audio.advance(CALLBACK_FRAMES);
        }
        let [events, frames, ..] = worker.audio_diagnostics.underruns();
        assert!(
            events > 0 && frames > 0,
            "the stall underran in the programme"
        );
    }

    /// R31 (b): the callback records the clock's progress. A worker that
    /// does not run while the ring still plays is no stall; a ring run dry
    /// is, however long the worker then takes to notice.
    #[test]
    fn the_callback_records_clock_stalls_the_worker_cannot_see() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(500));
        let diagnostics = Arc::clone(&worker.audio_diagnostics);
        audio.advance(CALLBACK_FRAMES);
        diagnostics.reset_stall();
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(5));
            audio.advance(CALLBACK_FRAMES);
        }
        let smooth = diagnostics.max_stall_ms(true);
        assert!(smooth < 60.0, "the clock kept moving: {smooth} ms");
        for _ in 0..60 {
            audio.advance(CALLBACK_FRAMES);
        }
        thread::sleep(Duration::from_millis(150));
        assert!(diagnostics.max_stall_ms(true) >= 150.0, "stalled now");
        worker.tick();
        audio.advance(CALLBACK_FRAMES);
        let stalled = diagnostics.max_stall_ms(false);
        assert!(
            stalled >= 150.0,
            "the dry ring stalled the clock: {stalled} ms"
        );
    }

    /// R33 (re-review A D5 / B D4): the whole programme is in the ring (the
    /// producer is done) but the callbacks freeze with its tail queued: the
    /// ongoing stall is measured until they consume it (the old measurement
    /// stopped at the producer's last push and reported none). Once the
    /// tail is consumed, the idle time before EOS is no stall.
    #[test]
    fn a_freeze_with_the_tail_queued_is_a_stall() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(5));
        worker.tick();
        let queued = worker.audio.as_ref().is_some_and(AudioRuntime::pushed_all);
        assert!(queued, "the producer pushed the whole programme");
        let diagnostics = Arc::clone(&worker.audio_diagnostics);
        audio.advance(CALLBACK_FRAMES);
        diagnostics.reset_stall();
        thread::sleep(Duration::from_millis(100));
        let frozen = diagnostics.max_stall_ms(true);
        assert!(frozen >= 100.0, "frozen with the tail queued: {frozen} ms");
        for _ in 0..30 {
            audio.advance(CALLBACK_FRAMES);
        }
        let [.., post_events, _] = diagnostics.underruns();
        assert!(post_events > 0, "consumed through the end");
        thread::sleep(Duration::from_millis(300));
        let after = diagnostics.max_stall_ms(true);
        assert!(
            (100.0..250.0).contains(&after),
            "the freeze counts; the idle after the end does not: {after} ms"
        );
    }

    /// Review B F4: `sync_decoders` counts the decoders this engine's
    /// renderers hold: open after a render, closed by a cache clear, and
    /// none after teardown. `live_table_bytes` is nonzero after an SDR
    /// render and sane (no underflow) after teardown.
    #[test]
    fn sync_decoders_counts_open_decoders_until_teardown() {
        let temp = TempDirectory::new("pf1-decoders");
        let crate::perf_fixtures::Workload(document, _media) =
            crate::perf_fixtures::cuts((160, 90), 30, 1, 2, 10);
        let gpu = fallback_gpu().context();
        let engine = FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu, temp.root().into()).unwrap();
        let gauge = engine.decoder_gauge();
        let frames = engine.frames();
        engine.set_document(Arc::new(document));
        frames
            .recv_timeout(Duration::from_secs(60))
            .expect("frame 0");
        let stats = engine.stats();
        assert!(stats.sync_decoders >= 1, "{stats:?}");
        // Re-review B nit: the production table field, not only the
        // gauge: an SDR source's RGB input table is live (128 KiB each).
        let table = 1 << 17;
        assert!(stats.live_table_bytes >= table, "{stats:?}");
        assert_eq!(stats.live_table_bytes % table, 0, "{stats:?}");
        engine.preview_cache_command(true).unwrap();
        assert_eq!(engine.stats().sync_decoders, 0, "cleared");
        engine.thumbnail_at(TimeCode(15), 64).unwrap();
        assert!(gauge.open() >= 1, "the thumbnail opened one");
        drop(engine);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while frames.recv_deadline(deadline).is_ok() {}
        assert_eq!(gauge.open(), 0, "teardown closed every decoder");
        // What outlives it is whole tables the registry keeps (process
        // wide), never a count driven below zero by the teardown.
        let after = crate::conversion::live_table_bytes() as u64;
        assert!(after.is_multiple_of(table) && after < 1 << 30, "{after}");
    }

    /// A tiny deterministic generator for the seeded models.
    struct Seeded(u64);

    impl Seeded {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % bound
        }

        fn frame(&mut self, bound: u64) -> TimeCode {
            TimeCode(i64::try_from(self.below(bound)).unwrap())
        }
    }

    /// Re-review A D6: the consumer's view of the transport, as the App
    /// keeps it: the state events received, never `Worker::playing`. Returns
    /// the stamped errors among them.
    fn observe(events: &Receiver<MediaEvent>, playing: &mut bool) -> Vec<FrameStamp> {
        let mut errors = Vec::new();
        for event in events.try_iter() {
            match event {
                MediaEvent::PlaybackStateChanged(state) => {
                    *playing = state == PlaybackState::Playing;
                }
                MediaEvent::StampedError(stamp, _) => errors.push(stamp),
                _ => {}
            }
        }
        errors
    }

    /// R-2's error gate on one worker pass: only current, unsuperseded
    /// failures stop playback; the rest count `stale_errors`. Returns the
    /// (current, stale) failures it saw.
    fn error_gate(
        worker: &mut Worker,
        events: &Receiver<MediaEvent>,
        observed: &mut bool,
    ) -> (u64, u64) {
        let latest = worker.lock_coalesced().latest;
        let stamps: Vec<_> = worker.lane.lock().failures.iter().map(|f| f.0).collect();
        let current = stamps.iter().any(|stamp| stamp.is_current(latest));
        let (playing, stale) = (worker.playing, worker.lane.counters().stats.stale_errors);
        observe(events, observed);
        worker.handle_preview_failures();
        let stopped = observe(events, observed);
        assert!(stopped.iter().all(|stamp| stamp.is_current(latest)));
        if current {
            assert!(!worker.playing && !stopped.is_empty());
            (1, 0)
        } else {
            assert_eq!((worker.playing, stopped.len()), (playing, 0));
            let now = worker.lane.counters().stats.stale_errors;
            assert_eq!(now, stale + stamps.len() as u64);
            (0, stamps.len() as u64)
        }
    }

    /// I8 coverage over a shard: every count must be reached.
    #[derive(Debug, Default)]
    struct Coverage {
        /// A control sent while one issued before it was still unsent.
        reversed_sends: u64,
        /// A control the worker stashed until its predecessor arrived.
        stashed: u64,
        /// Requests a later control superseded (they rendered nothing).
        superseded: u64,
        /// Published frames the consumer rejected as not current (R-2).
        stale_rejected: u64,
        /// Re-review A D6: current playback frames the consumer rejected
        /// because the clock had passed them by consumption.
        expired_rejected: u64,
        playback_published: u64,
        paused_published: u64,
        /// A taken job executed after other steps ran in between.
        interleaved_takes: u64,
        current_failures: u64,
        stale_failures: u64,
        releases: u64,
    }

    /// Bounds a model's quiescence loop (review B: a mutation that keeps a
    /// job alive must fail, not hang).
    const QUIESCENCE_STEPS: usize = 64;

    /// PF1 I8: the worker and a preview driven step by step, with every
    /// boundary split: a call is issued (stamp, clock effect) apart from
    /// its send; sends land in any order; a job is taken apart from its
    /// render; a frame is published apart from its consumption. `calls` is
    /// the model's own record of what it issued: the oracle every published
    /// frame is checked against (its stamp's call, target and document).
    struct Model<'a> {
        worker: Worker,
        events: Receiver<MediaEvent>,
        audio: Option<SimulatedAudio>,
        preview: &'a mut Preview,
        frames: &'a Receiver<PreviewFrame>,
        calls: BTreeMap<FrameStamp, Call>,
        pending: Vec<Control>,
        taken: Option<crate::preview::Work>,
        /// Steps run since the job in `taken` was taken.
        since_take: u32,
        published: VecDeque<PreviewFrame>,
        shown: Option<PreviewFrame>,
        /// The transport as the consumer tracks it: from the state events.
        playing: bool,
        documents: i64,
        coverage: &'a mut Coverage,
    }

    fn rendered_duration(frame: &PreviewFrame) -> i64 {
        let bytes: [u8; 4] = frame.texture.rgba[..4].try_into().unwrap();
        i64::from(u32::from_le_bytes(bytes))
    }

    impl Model<'_> {
        fn issue(&mut self, control: Control, call: Call) {
            let issued = control.issued().expect("a stamped control");
            self.calls.insert(issued.stamp, call);
            self.pending.push(control);
        }

        fn set_document(&mut self) {
            self.documents += 1;
            let frames = 10 + self.documents;
            let document = Arc::new(crate::perf_fixtures::title_card((64, 64), frames));
            let clock = &self.worker.clock;
            let control = self.worker.lock_coalesced().set_document(document, clock);
            self.issue(control, Call::Document(frames));
        }

        fn play(&mut self, from: TimeCode) {
            let control = (self.worker.lock_coalesced()).play(from, &self.worker.clock);
            self.issue(control, Call::Play(from));
        }

        fn pause(&mut self) {
            let control = self.worker.lock_coalesced().pause();
            self.issue(control, Call::Pause);
        }

        fn seek(&mut self, to: TimeCode) {
            let stamp = {
                let mut coalesced = self.worker.lock_coalesced();
                coalesced.seek(to, &self.worker.clock);
                coalesced.latest
            };
            self.calls.insert(stamp, Call::Seek(to));
        }

        fn request_frame(&mut self, at: TimeCode) {
            let stamp = {
                let mut coalesced = self.worker.lock_coalesced();
                coalesced.request_frame(at);
                coalesced.latest
            };
            self.calls.insert(stamp, Call::Frame(at));
        }

        /// Deliver one issued control, any of them: sends reverse.
        fn send(&mut self, rng: &mut Seeded) -> bool {
            if self.pending.is_empty() {
                return false;
            }
            let index = usize::try_from(rng.below(self.pending.len() as u64)).unwrap();
            self.coverage.reversed_sends += u64::from(index > 0);
            let stashed = self.worker.stashed.len();
            self.worker.handle_control(self.pending.remove(index));
            self.coverage.stashed += u64::from(self.worker.stashed.len() > stashed);
            self.worker.fill_audio();
            self.observe();
            true
        }

        /// Receive the worker's events: the consumer's transport view, which
        /// the state events keep equal to the worker's.
        fn observe(&mut self) {
            observe(&self.events, &mut self.playing);
            assert_eq!(self.playing, self.worker.playing, "the state events");
        }

        fn worker_pass(&mut self) {
            self.worker.handle_coalesced_requests();
            let events = (&mut self.worker, &self.events);
            let (current, stale) = error_gate(events.0, events.1, &mut self.playing);
            self.coverage.current_failures += current;
            self.coverage.stale_failures += stale;
            self.observe();
        }

        fn take(&mut self) {
            if self.taken.is_none() {
                self.taken = self.preview.next_work(false);
                self.since_take = 0;
            }
        }

        /// Render the taken job; its frames are checked as they publish.
        fn execute(&mut self) {
            let Some(work) = self.taken.take() else {
                return;
            };
            self.coverage.interleaved_takes += u64::from(self.since_take > 0);
            let playback = matches!(work, crate::preview::Work::Playback(..));
            self.preview.execute(work);
            let frames: Vec<_> = self.frames.try_iter().collect();
            for frame in frames {
                self.check(&frame, playback);
                if playback {
                    self.coverage.playback_published += 1;
                } else {
                    self.coverage.paused_published += 1;
                }
                self.published.push_back(frame);
            }
        }

        /// The oracle: a frame carries a stamp the model issued, renders
        /// the document current at that stamp (never a later epoch's), and
        /// shows its call's target; a playback frame shows the clock's own
        /// frame, never early or expired.
        fn check(&self, frame: &PreviewFrame, playback: bool) {
            let stamp = frame.stamp;
            let call = *(self.calls.get(&stamp)).unwrap_or_else(|| panic!("{stamp:?} not issued"));
            let document = (self.calls.range(..=stamp).rev())
                .find_map(|(_, call)| match call {
                    Call::Document(frames) => Some(*frames),
                    _ => None,
                })
                .expect("a document before it");
            let rendered = rendered_duration(frame);
            assert_eq!(
                rendered, document,
                "{call:?} {stamp:?}: another epoch's document"
            );
            if playback {
                assert!(!matches!(call, Call::Document(_) | Call::Pause), "{call:?}");
                assert_eq!(frame.at, self.worker.clock.position(), "early or expired");
            } else {
                match call {
                    Call::Document(_) => assert_eq!(frame.at, TimeCode::ZERO, "{call:?}"),
                    Call::Seek(to) | Call::Frame(to) => assert_eq!(frame.at, to, "{call:?}"),
                    Call::Pause | Call::Play(_) => assert!(frame.at.0 < document, "resting"),
                }
            }
        }

        /// R-2 at the consumer: only a current frame is shown and, while
        /// the consumer's own transport view says playing, only the clock's
        /// frame (re-review A D6: one published at frame 10 and consumed
        /// after the clock reached 11 is expired). A current frame is never
        /// ahead of the clock: it published at the clock's frame.
        fn consume(&mut self) -> bool {
            let Some(frame) = self.published.pop_front() else {
                return false;
            };
            let latest = self.worker.lock_coalesced().latest;
            let position = self.worker.clock.position();
            if !frame.stamp.is_current(latest) {
                self.coverage.stale_rejected += 1;
            } else if self.playing && frame.at != position {
                assert!(frame.at < position, "{frame:?} ahead of {position:?}");
                self.coverage.expired_rejected += 1;
            } else {
                self.shown = Some(frame);
            }
            true
        }

        /// One output callback (or, with fake audio, one frame of clock).
        fn advance(&mut self) {
            if let Some(audio) = &self.audio {
                audio.advance(CALLBACK_FRAMES);
            } else if self.worker.playing {
                let next = self.worker.clock.position().0 + 1;
                self.worker.clock.set_frame(TimeCode(next));
            }
            self.worker.tick();
            self.observe();
        }

        fn idle(&self) -> bool {
            let coalesced = self.worker.lock_coalesced();
            let requests = coalesced.seek.is_none() && coalesced.frame.is_none();
            drop(coalesced);
            let transport = self.worker.lane.lock().transport.is_none();
            let queued = self.pending.is_empty() && self.published.is_empty();
            requests && transport && queued && self.taken.is_none()
        }

        /// Everything issued lands and renders, within a step budget.
        fn quiesce(&mut self, rng: &mut Seeded) {
            for _ in 0..QUIESCENCE_STEPS {
                while self.send(rng) {}
                self.worker_pass();
                self.take();
                self.execute();
                while self.consume() {}
                if self.idle() {
                    return;
                }
            }
            panic!("no quiescence within {QUIESCENCE_STEPS} steps");
        }
    }

    /// One I8 sequence: 32 random steps, a scripted play (so every seed
    /// publishes due frames), then the release (L-6): a pause and the final
    /// seek, which must end on screen with the newest stamp and document.
    fn i8_sequence(
        seed: u64,
        (preview, frames, faults): (&mut Preview, &Receiver<PreviewFrame>, &Arc<Faults>),
        (lane, clock): (&Arc<Lane>, &Arc<SharedClock>),
        audio: Option<SimulatedAudio>,
        coverage: &mut Coverage,
    ) {
        let mut rng = Seeded(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut worker, events) = test_worker();
        (worker.lane, worker.clock) = (Arc::clone(lane), Arc::clone(clock));
        worker.faults = Arc::clone(faults);
        faults.fake_audio.store(audio.is_none(), Ordering::Release);
        faults.fail_render.store(false, Ordering::Release);
        let superseded = faults.superseded.load(Ordering::Relaxed);
        lane.post(None);
        drop(lane.take_failures());
        preview.reset_cursor();
        clock.set_frame(TimeCode::ZERO);
        if let Some(audio) = &audio {
            worker.output_device = OutputDevice::Simulated(audio.clone());
        }
        let mut model = Model {
            worker,
            events,
            audio,
            preview,
            frames,
            calls: BTreeMap::new(),
            pending: Vec::new(),
            taken: None,
            since_take: 0,
            published: VecDeque::new(),
            shown: None,
            playing: false,
            documents: 0,
            coverage,
        };
        model.set_document();
        for _ in 0..32 {
            let step = rng.below(15);
            match step {
                0 => model.request_frame(rng.frame(40)),
                1 => model.seek(rng.frame(40)),
                2 => model.play(rng.frame(30)),
                3 => model.pause(),
                4 => model.set_document(),
                5 | 6 => drop(model.send(&mut rng)),
                7 => while model.send(&mut rng) {},
                8 | 9 => model.worker_pass(),
                10 => model.take(),
                11 => model.execute(),
                12 => drop(model.consume()),
                13 => model.advance(),
                _ => faults.fail_render.store(true, Ordering::Release),
            }
            if !matches!(step, 10 | 11) {
                model.since_take += 1;
            }
        }
        faults.fail_render.store(false, Ordering::Release);
        model.play(rng.frame(30));
        while model.send(&mut rng) {}
        model.worker_pass();
        for _ in 0..12 {
            model.take();
            model.execute();
            // Re-review A D6: the clock can pass a frame between its
            // publication and its consumption.
            if rng.below(3) == 0 {
                model.advance();
            }
            while model.consume() {}
            model.advance();
        }
        // A current failure (of the playing job, or of a resting image)
        // reaches the error gate in every third seed.
        if seed.is_multiple_of(3) {
            model.request_frame(TimeCode(1));
            model.worker_pass();
            faults.fail_render.store(true, Ordering::Release);
            model.take();
            model.execute();
            model.worker_pass();
            faults.fail_render.store(false, Ordering::Release);
        }
        model.pause();
        let target = rng.frame(40);
        model.seek(target);
        model.quiesce(&mut rng);
        let latest = model.worker.lock_coalesced().latest;
        let shown =
            (model.shown.as_ref()).map(|frame| (frame.at, frame.stamp, rendered_duration(frame)));
        let document = 10 + model.documents;
        assert_eq!(shown, Some((target, latest, document)), "seed {seed}: L-6");
        model.coverage.superseded += faults.superseded.load(Ordering::Relaxed) - superseded;
        model.coverage.releases += 1;
    }

    /// I8 (media path): seeded sequences over the split boundaries, checked
    /// against the model's oracle at publish, at the consumer and at the
    /// release, with the shard's coverage asserted. `real_audio` drives the
    /// stepped simulated output; otherwise playback opens no audio runtime
    /// and the model moves the clock (cheap: no mixer, no ring fill).
    fn seeded_interleavings(seeds: std::ops::Range<u64>, real_audio: bool) -> Coverage {
        let (lane, clock) = (Arc::<Lane>::default(), Arc::new(SharedClock::new()));
        let (mut preview, frames) =
            crate::preview::tests::test_preview_on(Arc::clone(&lane), Arc::clone(&clock));
        preview.faults.fake_render.store(true, Ordering::Release);
        preview.faults.step_hold.store(true, Ordering::Release);
        let faults = Arc::clone(&preview.faults);
        let mut coverage = Coverage::default();
        let count = seeds.end - seeds.start;
        for seed in seeds {
            let audio = real_audio.then(SimulatedAudio::stepped);
            let shared = (&mut preview, &frames, &faults);
            i8_sequence(seed, shared, (&lane, &clock), audio, &mut coverage);
        }
        assert_eq!(coverage.releases, count);
        coverage
    }

    /// Every coverage count reached at least `floor` times.
    fn assert_covered(coverage: &Coverage, floor: u64) {
        let counts = [
            coverage.reversed_sends,
            coverage.stashed,
            coverage.superseded,
            coverage.stale_rejected,
            coverage.expired_rejected,
            coverage.playback_published,
            coverage.paused_published,
            coverage.interleaved_takes,
            coverage.current_failures,
            coverage.stale_failures,
        ];
        assert!(counts.iter().all(|&count| count >= floor), "{coverage:?}");
    }

    macro_rules! i8_shards {
        ($($name:ident: $shard:literal),*) => {$(
            #[test]
            fn $name() {
                let coverage = seeded_interleavings($shard * 250..($shard + 1) * 250, false);
                println!("I8 shard {} coverage {coverage:?}", $shard);
                assert_covered(&coverage, 10);
            }
        )*};
    }

    i8_shards!(i8_media_0: 0, i8_media_1: 1, i8_media_2: 2, i8_media_3: 3);

    /// I8's smaller real-audio set: the same sequences on the stepped
    /// simulated output (each play opens and fills the audio runtime).
    #[test]
    fn i8_media_real_audio() {
        let coverage = seeded_interleavings(10_000..10_024, true);
        println!("I8 real-audio coverage {coverage:?}");
        assert_covered(&coverage, 1);
    }

    /// G10 (stepped): a 2 s fill stall (94 callbacks with no worker pass, as
    /// the Q-3 stall blocks it) starves the ring and stops the clock; the
    /// preview never waits on the fill (V-3), every frame it publishes is
    /// within 33 ms of the clock, and publication is back within 5 s.
    #[test]
    fn g10_video_tracks_the_clock_within_five_seconds_of_a_fill_stall() {
        let fps = Rational::new(30, 1).unwrap();
        let (mut worker, audio, _events) = stepped_worker(fps, TimeCode(900));
        let (lane, clock) = (Arc::clone(&worker.lane), Arc::clone(&worker.clock));
        let (mut preview, frames) = crate::preview::tests::test_preview_on(lane, clock);
        preview.faults.fake_render.store(true, Ordering::Release);
        preview.faults.step_hold.store(true, Ordering::Release);
        let (stall, recovered, end) = (47..141, 141 + 235, 141 + 235 + 47);
        let (mut frozen, mut first_after, mut tail) = (0, None, 0);
        for callback in 0..end {
            let before = worker.clock.position();
            assert!(audio.advance(CALLBACK_FRAMES));
            if !stall.contains(&callback) {
                worker.tick();
            }
            frozen += usize::from(worker.clock.position() == before);
            let work = preview.next_work(false).expect("the playback job stays");
            preview.execute(work);
            for frame in frames.try_iter() {
                // One frame is 33.3 ms: ≤ 33 ms means the clock's own frame.
                let offset = worker.clock.position().0 - frame.at.0;
                assert_eq!(offset, 0, "callback {callback}: offset {offset} frames");
                if callback >= stall.end {
                    first_after.get_or_insert(callback);
                }
                tail += usize::from(callback >= recovered);
            }
        }
        assert!(
            worker.audio_diagnostics.underrun_frames() > 0,
            "the stall starved"
        );
        assert!(frozen >= 40, "the clock stopped: {frozen}");
        assert!(
            first_after.is_some_and(|at| at < recovered),
            "{first_after:?}"
        );
        assert!(tail >= 15, "still publishing: {tail}");
    }

    /// Callbacks with a tick after each until the worker stops (or `limit`).
    fn play_out(worker: &mut Worker, audio: &SimulatedAudio, limit: usize) -> usize {
        (0..limit)
            .take_while(|_| {
                audio.advance(CALLBACK_FRAMES);
                worker.tick();
                worker.playing
            })
            .count()
    }

    fn assert_stopped_at_end(worker: &Worker, events: &Receiver<MediaEvent>, end: TimeCode) {
        assert!(!worker.playing && worker.audio.is_none());
        assert_eq!(worker.clock.position(), end);
        assert_eq!(worker.clock.sample_rate.load(Ordering::Acquire), 0);
        assert_eq!(
            worker.loudness.paused_at(),
            Some(end),
            "truncated at the end"
        );
        let events: Vec<_> = events.try_iter().collect();
        assert!(
            matches!(
                events.as_slice(),
                [
                    ..,
                    MediaEvent::PlaybackStateChanged(PlaybackState::Paused),
                    MediaEvent::Position(at)
                ] if *at == end
            ),
            "{events:?}"
        );
    }

    /// PF1 V-2: a one-frame 30000/1001 timeline ends at sample 1,601, where
    /// the clock still reads frame 0; the drained stop lands on the duration.
    #[test]
    fn a_drained_one_frame_timeline_stops_at_its_duration() {
        let fps = Rational::new(30_000, 1_001).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(1));
        assert_eq!(play_out(&mut worker, &audio, 8), 1, "stops on the 2nd tick");
        assert_stopped_at_end(&worker, &events, TimeCode(1));
    }

    /// PF1 V-2 (review B F2): a pause handled after the last callback
    /// drained the one-frame programme, before `tick`, is the terminal stop.
    #[test]
    fn a_pause_after_the_programme_drained_stops_at_the_duration() {
        let fps = Rational::new(30_000, 1_001).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(1));
        audio.advance(CALLBACK_FRAMES);
        audio.advance(CALLBACK_FRAMES);
        assert_eq!(worker.clock.position(), TimeCode::ZERO, "the clock trails");
        let pause = worker.lock_coalesced().control();
        worker.handle_control(Control::Pause(pause));
        assert_stopped_at_end(&worker, &events, TimeCode(1));
        assert!(!worker.resume_after_eos, "a pause never resumes");
    }

    /// A long programme played until its ring has drained, the terminal
    /// `tick` not yet run.
    fn drained_before_the_tick() -> (Worker, SimulatedAudio, Receiver<MediaEvent>) {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(50));
        for _ in 0..400 {
            audio.advance(CALLBACK_FRAMES);
            let samples = worker.clock.position_samples.load(Ordering::Acquire);
            if worker.audio.as_ref().unwrap().drained(samples) {
                return (worker, audio, events);
            }
            worker.tick();
            assert!(worker.playing);
        }
        panic!("the programme never drained");
    }

    fn seek(worker: &Worker, to: TimeCode) {
        worker.lock_coalesced().seek(to, &worker.clock);
    }

    /// PF1 V-2 (review B F1): a seek published while playing, which races
    /// the terminal stop, keeps playing: whether it lands before the tick's
    /// check (the stop waits for it) or between that check and the stop
    /// (its generation predates the stop's).
    #[test]
    fn a_playing_seek_that_races_the_terminal_stop_keeps_playing() {
        // Published after `handle_coalesced_requests`, before `tick`.
        let (mut worker, _audio, _events) = drained_before_the_tick();
        seek(&worker, TimeCode(10));
        worker.tick();
        assert!(worker.playing, "the stop waits for the pending seek");
        worker.handle_coalesced_requests();
        assert!(worker.playing && worker.audio.is_some());
        assert_eq!(worker.clock.position(), TimeCode(10));

        // Published after the tick's check, before the stop's generation.
        let (mut worker, _audio, events) = drained_before_the_tick();
        seek(&worker, TimeCode(10));
        worker.stop_at_end();
        assert!(!worker.playing);
        let _ = events.try_iter().count();
        worker.handle_coalesced_requests();
        assert!(worker.playing && worker.audio.is_some(), "resumed");
        assert_eq!(worker.clock.position(), TimeCode(10));
        let events: Vec<_> = events.try_iter().collect();
        let resumed = MediaEvent::PlaybackStateChanged(PlaybackState::Playing);
        assert!(events.contains(&resumed), "{events:?}");
    }

    /// R34 (re-review 2 D1): the R29 terminal-race ordering. `seek(10)` is
    /// issued before the 50-frame programme's terminal stop, which writes
    /// clock 50; the ack of an old paint (frame 48, the played epoch) is
    /// delayed across the stop and the resumed seek. The worker registered
    /// only playback that ran, 0..=49 by the stop and then the seek's own
    /// first frame, and the ack settles frame 48 and registers nothing.
    /// (R33's ack sampled the seek's issued epoch with clock 50, and once
    /// the seek was applied registered 11..=49 of it.)
    #[test]
    fn an_old_paints_ack_delayed_across_a_raced_stop_registers_nothing() {
        let (mut worker, _audio, _events) = drained_before_the_tick();
        let painted = Instant::now();
        seek(&worker, TimeCode(10));
        worker.stop_at_end();
        assert_eq!(worker.lane.counters().stats.due_frames, 50);
        worker.handle_coalesced_requests();
        assert!(worker.playing, "resumed");
        acknowledge(
            &worker.lane,
            FrameStamp::default(),
            TimeCode(48),
            painted,
            false,
        );
        worker.tick();
        let stats = worker.lane.counters().stats;
        assert_eq!(stats.due_frames, 51, "0..=49, then 10: {stats:?}");
        assert_eq!(stats.on_time + stats.late, 1, "{stats:?}");
        assert_eq!(stats.acks_unmatched, 0, "{stats:?}");
    }

    /// PF1 V-2 (review B F1, the controls): a seek after the published stop,
    /// or one followed by a pause, stays paused.
    #[test]
    fn a_seek_after_the_stop_or_before_a_pause_stays_paused() {
        let (mut worker, _audio, _events) = drained_before_the_tick();
        worker.tick();
        assert!(!worker.playing, "the terminal stop");
        seek(&worker, TimeCode(10));
        worker.handle_coalesced_requests();
        assert!(!worker.playing && worker.audio.is_none());
        assert_eq!(worker.clock.position(), TimeCode(10));

        let (mut worker, _audio, _events) = drained_before_the_tick();
        seek(&worker, TimeCode(10));
        worker.stop_at_end();
        let pause = worker.lock_coalesced().control();
        worker.handle_control(Control::Pause(pause));
        worker.handle_coalesced_requests();
        assert!(!worker.playing && worker.audio.is_none(), "the pause wins");
        assert_eq!(worker.clock.position(), TimeCode(10));
    }

    /// PF1 V-2 (re-review D1): `seek(duration)` then `Pause`, both before
    /// the worker runs, mid-programme: the requested position is not
    /// completion. The pause is ordinary (no terminal stop) and the seek
    /// alone reports the duration, once. The same holds once the ring has
    /// drained: the pending seek decides the position.
    #[test]
    fn a_pause_behind_a_seek_to_the_end_is_not_terminal() {
        let exactly_one_pause_and_position = |worker: &mut Worker, events: &Receiver<_>| {
            let end = worker.document.duration;
            let _ = events.try_iter().count();
            let eos = worker.lock_coalesced().eos_generation;
            seek(worker, end);
            let pause = worker.lock_coalesced().control();
            worker.handle_control(Control::Pause(pause));
            worker.handle_coalesced_requests();
            let paused = MediaEvent::PlaybackStateChanged(PlaybackState::Paused);
            let events: Vec<_> = events.try_iter().collect();
            assert_eq!(events, [paused, MediaEvent::Position(end)]);
            assert_eq!(worker.lock_coalesced().eos_generation, eos);
            assert!(!worker.playing && !worker.resume_after_eos);
            assert_eq!(worker.clock.position(), end);
        };

        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(50));
        assert_eq!(play_out(&mut worker, &audio, 10), 10, "mid-programme");
        let samples = worker.clock.position_samples.load(Ordering::Acquire);
        assert!(
            !worker.audio.as_ref().unwrap().drained(samples),
            "undrained"
        );
        exactly_one_pause_and_position(&mut worker, &events);

        let (mut worker, _audio, events) = drained_before_the_tick();
        exactly_one_pause_and_position(&mut worker, &events);
    }

    /// PF1 V-2: a long programme stops at its duration; a 2 s fill stall
    /// underruns without advancing the clock, so it does not complete early.
    #[test]
    fn a_long_programme_stops_at_its_duration_and_a_stall_does_not_complete() {
        let fps = Rational::new(10, 1).unwrap();
        let (mut worker, audio, events) = stepped_worker(fps, TimeCode(50));
        assert_eq!(play_out(&mut worker, &audio, 40), 40);
        for _ in 0..94 {
            assert!(audio.advance(CALLBACK_FRAMES), "a stall: no tick, no fill");
        }
        let stalled = worker.clock.position();
        assert!(stalled < TimeCode(50), "{stalled:?}");
        assert!(worker.playing);
        let samples = worker.clock.position_samples.load(Ordering::Acquire);
        assert!(!worker.audio.as_ref().unwrap().drained(samples));
        assert!(play_out(&mut worker, &audio, 400) < 400);
        assert_stopped_at_end(&worker, &events, TimeCode(50));
    }

    /// PF1 P-1/G12: the monitor raster's long edge is capped at 1,280; a
    /// 1080×1920 (`reel_9x16`) document presents 720×1280.
    #[test]
    fn the_monitor_caps_the_long_edge_at_1280() {
        for (document, monitor) in [
            ((1_080, 1_920), (720, 1_280)),
            ((1_080, 1_350), (1_024, 1_280)),
            ((1_920, 1_080), (1_280, 720)),
            ((640, 360), (640, 360)),
        ] {
            let proxy = RenderScale::Proxy {
                max_width: monitor_max_width(document),
            };
            assert_eq!(proxy.output_resolution(document), monitor, "{document:?}");
        }
        let temp = TempDirectory::new("pf1-monitor-raster");
        let root = temp.root().to_path_buf();
        let engine =
            FfmpegMediaEngine::new_with_gpu_and_data_dir(fallback_gpu().context(), root).unwrap();
        let frames = engine.frames();
        engine.set_document(Arc::new(crate::perf_fixtures::title_card(
            (1_080, 1_920),
            3,
        )));
        let frame = frames
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .texture;
        assert_eq!((frame.width, frame.height), (720, 1_280));
        assert_eq!(frame.rgba.len(), 720 * 1_280 * 4);
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
            fn frames(&self) -> Receiver<PreviewFrame> {
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
            fn frames(&self) -> Receiver<PreviewFrame> {
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
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let meter = Arc::new(MeterState::default());
        let mix_meters = Arc::new(RwLock::new(Arc::new(MixMeters::empty(Arc::clone(&meter)))));
        let mut worker = Worker::new(
            WorkerChannels {
                control_rx: control_rx.clone(),
                events_tx,
                events_drop_rx: events_rx.clone(),
                lane: Arc::default(),
            },
            Arc::clone(&clock),
            Arc::clone(&meter),
            Arc::clone(&mix_meters),
            Arc::new(LiveLoudness::default()),
            Arc::default(),
            Arc::new(RwLock::new(PublishedLattices::default())),
            Arc::new(AtomicI32::new(0)),
        );

        let limiter = |milliseconds: i64| Effect {
            enabled: true,
            enabled_curve: None,
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
