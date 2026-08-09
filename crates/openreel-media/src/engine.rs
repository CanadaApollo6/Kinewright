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
use openreel_core::{
    AssetId, ClipId, Document, ExportSettings, FrameTexture, MediaAsset, MediaEngine, MediaError,
    MediaEvent, MediaKind, PlaybackState, ProgressSink, Rational, RgbaImage, TimeCode,
    TimelineTranscriptWord, TranscriptStatus,
};

use crate::{
    audio::AudioRuntime,
    clock::samples_to_frame,
    compositor::GpuContext,
    decode::{probe_path, thumbnail},
    render::FrameRenderer,
    timeline_source_at,
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
    next_asset_id: AtomicU64,
    gpu: GpuContext,
    export_document: Arc<RwLock<Arc<Document>>>,
    transcripts: TranscriptService,
}

impl FfmpegMediaEngine {
    pub fn new() -> Result<Self, MediaError> {
        Self::new_with_data_dir(default_data_dir())
    }

    pub fn new_with_data_dir(data_dir: PathBuf) -> Result<Self, MediaError> {
        static GPU: OnceLock<Result<GpuContext, MediaError>> = OnceLock::new();
        let gpu = GPU
            .get_or_init(|| {
                GpuContext::headless(false).or_else(|_| GpuContext::headless(true))
            })
            .clone()?;
        Self::new_with_gpu_and_data_dir(gpu, data_dir)
    }

    pub fn new_with_gpu(gpu: GpuContext) -> Result<Self, MediaError> {
        Self::new_with_gpu_and_data_dir(gpu, default_data_dir())
    }

    pub fn new_with_gpu_and_data_dir(
        gpu: GpuContext,
        data_dir: PathBuf,
    ) -> Result<Self, MediaError> {
        crate::initialize_ffmpeg()?;
        let (control_tx, control_rx) = unbounded();
        let (frames_tx, frames_rx) = bounded(2);
        let (events_tx, events_rx) = bounded(16);
        let clock = Arc::new(SharedClock::new());
        let worker_clock = Arc::clone(&clock);
        let frames_drop_rx = frames_rx.clone();
        let events_drop_rx = events_rx.clone();
        // Scrub positions use shared atomics so rapid mouse movement is coalesced
        // without an unbounded command backlog.
        let requested = Arc::new(RequestedPositions::default());
        let worker_requested = Arc::clone(&requested);
        let worker_gpu = gpu.clone();
        thread::Builder::new()
            .name("openreel-media".to_owned())
            .spawn(move || {
                Worker::new(
                    control_rx,
                    frames_tx,
                    frames_drop_rx,
                    events_tx,
                    events_drop_rx,
                    worker_clock,
                    worker_requested,
                    worker_gpu,
                )
                .run();
            })
            .map_err(|error| MediaError::Backend(error.to_string()))?;

        Ok(Self {
            control_tx,
            frames_rx,
            events_rx,
            requested,
            clock,
            next_asset_id: AtomicU64::new(1),
            gpu,
            export_document: Arc::new(RwLock::new(Arc::new(Document::default()))),
            transcripts: TranscriptService::new(data_dir)?,
        })
    }
}

#[derive(Default)]
struct RequestedPositions {
    frame: AtomicI64,
    frame_sequence: AtomicU64,
    seek: AtomicI64,
    seek_sequence: AtomicU64,
}

impl MediaEngine for FfmpegMediaEngine {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError> {
        let id = AssetId(self.next_asset_id.fetch_add(1, Ordering::Relaxed));
        probe_path(path, id)
    }

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
        self.clock.set_frame(from);
        let _ = self.control_tx.send(Control::Play(from));
    }

    fn pause(&self) {
        let _ = self.control_tx.send(Control::Pause);
    }

    fn seek(&self, to: TimeCode) {
        self.clock.set_frame(to);
        self.requested.seek.store(to.0.max(0), Ordering::Relaxed);
        self.requested
            .seek_sequence
            .fetch_add(1, Ordering::Release);
        self.request_frame(to);
    }

    fn position(&self) -> TimeCode {
        self.clock.position()
    }

    fn request_transcription(&self, asset: MediaAsset) {
        self.transcripts.request(asset);
    }

    fn transcript_status(&self, asset: AssetId) -> TranscriptStatus {
        self.transcripts.status(asset)
    }

    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        self.transcripts.timeline_words(document, range)
    }

    fn thumbnail_at(&self, at: TimeCode, max_width: u32) -> Result<RgbaImage, MediaError> {
        let (reply, response) = bounded(1);
        self.control_tx
            .send(Control::Thumbnail {
                at,
                max_width,
                reply,
            })
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?;
        response
            .recv()
            .map_err(|_| MediaError::Backend("media worker stopped".to_owned()))?
    }

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
        crate::export::export_document(&document, out, settings, progress, self.gpu.clone())
    }
}

#[derive(Clone)]
struct ActiveMedia {
    clip: ClipId,
    path: PathBuf,
    source_fps: Rational,
    project_fps: Rational,
    source_at: TimeCode,
    source_end: TimeCode,
    kind: MediaKind,
}

struct Worker {
    control_rx: Receiver<Control>,
    frames_tx: Sender<(TimeCode, FrameTexture)>,
    frames_drop_rx: Receiver<(TimeCode, FrameTexture)>,
    events_tx: Sender<MediaEvent>,
    events_drop_rx: Receiver<MediaEvent>,
    clock: Arc<SharedClock>,
    requested: Arc<RequestedPositions>,
    handled_frame_sequence: u64,
    handled_seek_sequence: u64,
    document: Arc<Document>,
    renderer: FrameRenderer,
    audio: Option<AudioRuntime>,
    audio_clip: Option<ClipId>,
    playing: bool,
    last_position: Option<TimeCode>,
}

impl Worker {
    fn new(
        control_rx: Receiver<Control>,
        frames_tx: Sender<(TimeCode, FrameTexture)>,
        frames_drop_rx: Receiver<(TimeCode, FrameTexture)>,
        events_tx: Sender<MediaEvent>,
        events_drop_rx: Receiver<MediaEvent>,
        clock: Arc<SharedClock>,
        requested: Arc<RequestedPositions>,
        gpu: GpuContext,
    ) -> Self {
        Self {
            control_rx,
            frames_tx,
            frames_drop_rx,
            events_tx,
            events_drop_rx,
            clock,
            requested,
            handled_frame_sequence: 0,
            handled_seek_sequence: 0,
            document: Arc::new(Document::default()),
            renderer: FrameRenderer::new(gpu),
            audio: None,
            audio_clip: None,
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
                at,
                max_width,
                reply,
            } => {
                let result = active_media_at(&self.document, at).and_then(|active| {
                    if let Some(active) = active {
                        thumbnail(&active.path, active.source_fps, active.source_at, max_width)
                    } else {
                        Ok(black_image(self.document.resolution, max_width))
                    }
                });
                let _ = reply.send(result);
            }
        }
    }

    fn set_document(&mut self, doc: &Document) {
        self.pause();
        self.document = Arc::new(doc.clone());
        self.renderer.clear();
        self.audio_clip = None;
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
        if self.document.duration <= TimeCode::ZERO {
            self.fail(MediaError::Backend("the timeline is empty".to_owned()));
            return;
        }
        let from = TimeCode(
            from.0
                .clamp(0, self.document.duration.0.saturating_sub(1)),
        );
        self.clock.set_frame(from);
        match self
            .audio_for_position(from)
        .and_then(|runtime| {
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
        if let Some(audio) = &self.audio {
            if let Err(error) = audio.pause() {
                self.emit(MediaEvent::Error(error));
            }
        }
        let position = self.clock.position();
        self.clock.fallback_frame.store(position.0, Ordering::Release);
        self.audio = None;
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
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.fill() {
                self.fail(error);
                return;
            }
        }
        let position = self.clock.position();
        if position >= self.document.duration {
            let end = self.document.duration;
            self.clock.fallback_frame.store(end.0, Ordering::Release);
            self.pause();
            self.emit(MediaEvent::Position(end));
            return;
        }
        let clip = match active_media_at(&self.document, position) {
            Ok(active) => active.map(|active| active.clip),
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        if clip != self.audio_clip {
            self.audio = None;
            match self.audio_for_position(position).and_then(|runtime| {
                runtime.play()?;
                Ok(runtime)
            }) {
                Ok(runtime) => self.audio = Some(runtime),
                Err(error) => {
                    self.fail(error);
                    return;
                }
            }
        }
        if self.last_position != Some(position) {
            self.last_position = Some(position);
            self.emit(MediaEvent::Position(position));
            self.present(position);
        }
    }

    fn present(&mut self, project_at: TimeCode) {
        let document = Arc::clone(&self.document);
        let frame = match self
            .renderer
            .render(&document, project_at, document.resolution)
        {
            Ok(frame) => frame,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        send_latest(
            &self.frames_tx,
            &self.frames_drop_rx,
            (project_at, frame),
        );
    }

    fn audio_for_position(&mut self, project_at: TimeCode) -> Result<AudioRuntime, MediaError> {
        let active = active_media_at(&self.document, project_at)?;
        self.audio_clip = active.as_ref().map(|active| active.clip);
        if let Some(active) = active.filter(|active| {
            matches!(active.kind, MediaKind::Audio | MediaKind::AudioVideo)
        }) {
            AudioRuntime::open(
                &active.path,
                active.source_fps,
                active.project_fps,
                active.source_at,
                active.source_end,
                project_at,
                Arc::clone(&self.clock.position_samples),
                Arc::clone(&self.clock.sample_rate),
            )
        } else {
            AudioRuntime::open_silence(
                self.document.fps,
                project_at,
                Arc::clone(&self.clock.position_samples),
                Arc::clone(&self.clock.sample_rate),
            )
        }
    }

    fn fail(&mut self, error: MediaError) {
        self.pause();
        self.emit(MediaEvent::Error(error));
    }

    fn emit(&self, event: MediaEvent) {
        send_latest(&self.events_tx, &self.events_drop_rx, event);
    }
}

fn active_media_at(
    document: &Document,
    project_at: TimeCode,
) -> Result<Option<ActiveMedia>, MediaError> {
    let Some(source) = timeline_source_at(document, project_at)? else {
        return Ok(None);
    };
    let asset = document.asset(source.asset).ok_or_else(|| {
        MediaError::Backend(format!("timeline asset {} disappeared", source.asset))
    })?;
    Ok(Some(ActiveMedia {
        clip: source.clip,
        path: asset.path.clone(),
        source_fps: asset.fps,
        project_fps: document.fps,
        source_at: source.source_at,
        source_end: source.source_end,
        kind: asset.kind,
    }))
}

fn black_image(resolution: (u32, u32), max_width: u32) -> RgbaImage {
    let width = resolution.0.min(max_width.max(1)).max(1);
    let height = u32::try_from(
        u64::from(resolution.1)
            .saturating_mul(u64::from(width))
            / u64::from(resolution.0.max(1)),
    )
    .unwrap_or(resolution.1)
    .max(1);
    let pixel_count = usize::try_from(width)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(height).unwrap_or_default())
        .saturating_mul(4);
    let mut pixels = vec![0; pixel_count];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn send_latest<T: Send>(sender: &Sender<T>, drop_receiver: &Receiver<T>, value: T) {
    match sender.try_send(value) {
        Ok(()) => {}
        Err(crossbeam_channel::TrySendError::Full(value)) => {
            let _ = drop_receiver.try_recv();
            let _ = sender.try_send(value);
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
    }
}
