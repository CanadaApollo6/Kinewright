use std::{
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
    ExportCancellation, ExportSettings, FrameTexture, MediaAsset, MediaError, MediaEvent, Playback,
    PlaybackState, ProgressSink, Rational, RgbaImage, SceneStatus, SilenceStatus, TimeCode,
    TimelineBeat, TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord,
    TranscriptStatus, VisualAssetResult,
};

use crate::{
    analysis::VisualAssetService,
    audio::{AudioRuntime, MeterState, decode_audio_range},
    clock::samples_to_frame,
    compositor::GpuContext,
    decode::probe_path,
    derived::{DerivedAnalysisConfig, DerivedAnalysisService},
    loudness::measure_loudness,
    render::{DecodeStrategy, FrameRenderer, PREVIEW_MAX_WIDTH, RenderScale},
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

enum Control {
    SetDocument(Arc<Document>),
    Play(TimeCode),
    Pause,
    Thumbnail {
        document: Option<Arc<Document>>,
        at: TimeCode,
        max_width: u32,
        reply: Sender<Result<RgbaImage, MediaError>>,
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
    gpu: GpuContext,
    export_document: Arc<RwLock<Arc<Document>>>,
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
            gpu,
            export_document: Arc::new(RwLock::new(Arc::new(Document::default()))),
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

    fn request_waveform(&self, asset: MediaAsset) -> bool {
        self.visual_assets.request_waveform(asset)
    }

    fn request_thumbnail(&self, asset: MediaAsset, source_at: TimeCode, max_width: u32) -> bool {
        self.visual_assets
            .request_thumbnail(asset, source_at, max_width)
    }

    fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
        self.visual_assets.results()
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
        crate::export::export_document(&document, out, &settings, &progress, self.gpu.clone())
    }

    fn export_document(
        &self,
        document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        crate::export::export_document(&document, out, &settings, &progress, self.gpu.clone())
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
            Control::Play(from) => self.start_playback(from),
            Control::Pause => self.pause(),
            Control::Thumbnail {
                document,
                at,
                max_width,
                reply,
            } => {
                let scale = RenderScale::Proxy { max_width };
                let document = document.as_deref().unwrap_or(&self.document);
                let resolution = scale.output_resolution(document.resolution);
                let result = self
                    .renderer
                    .render(document, at, resolution, scale, DecodeStrategy::Seek)
                    .map(|frame| RgbaImage {
                        width: frame.width,
                        height: frame.height,
                        pixels: (*frame.rgba).clone(),
                    });
                let _ = reply.send(result);
            }
        }
    }

    fn set_document(&mut self, doc: &Document) {
        self.pause();
        self.document = Arc::new(doc.clone());
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
