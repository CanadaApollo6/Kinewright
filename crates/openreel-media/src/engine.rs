use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use openreel_core::{
    AssetId, Document, ExportSettings, FrameRounding, FrameTexture, MediaAsset, MediaEngine,
    MediaError, MediaEvent, MediaKind, PlaybackState, ProgressSink, Rational, RgbaImage, TimeCode,
    map_frames_with_rounding,
};

use crate::{
    audio::AudioRuntime,
    cache::FrameCache,
    clock::samples_to_frame,
    decode::{VideoDecoder, probe_path, thumbnail},
};

const FRAME_CACHE_CAPACITY: usize = 36;
const PREFETCH_FRAMES: i64 = 16;
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
}

impl FfmpegMediaEngine {
    pub fn new() -> Result<Self, MediaError> {
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
        _out: &Path,
        _settings: ExportSettings,
        _progress: ProgressSink,
    ) -> Result<(), MediaError> {
        Err(MediaError::NotImplemented)
    }
}

#[derive(Clone)]
struct ActiveMedia {
    path: PathBuf,
    source_fps: Rational,
    project_fps: Rational,
    source_duration: TimeCode,
    project_duration: TimeCode,
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
    active: Option<ActiveMedia>,
    video: Option<VideoDecoder>,
    cache: FrameCache,
    audio: Option<AudioRuntime>,
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
            active: None,
            video: None,
            cache: FrameCache::new(FRAME_CACHE_CAPACITY),
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
                at,
                max_width,
                reply,
            } => {
                let result = self.active.as_ref().ok_or_else(|| {
                    MediaError::Backend("no media asset is loaded".to_owned())
                }).and_then(|active| {
                    let source = project_to_source(at, active).unwrap_or(TimeCode::ZERO);
                    thumbnail(&active.path, active.source_fps, source, max_width)
                });
                let _ = reply.send(result);
            }
        }
    }

    fn set_document(&mut self, doc: &Document) {
        self.pause();
        self.video = None;
        self.cache.clear();
        self.active = doc.media_pool.last().map(|asset| {
            let project_duration = map_frames_with_rounding(
                asset.duration,
                asset.fps,
                doc.fps,
                FrameRounding::Ceil,
            )
            .unwrap_or(asset.duration);
            ActiveMedia {
                path: asset.path.clone(),
                source_fps: asset.fps,
                project_fps: doc.fps,
                source_duration: asset.duration,
                project_duration,
                kind: asset.kind,
            }
        });
        self.clock.set_fps(doc.fps);
        self.clock.set_frame(TimeCode::ZERO);
        self.last_position = None;
        if self.active.is_some() {
            self.present(TimeCode::ZERO);
        }
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
        let Some(active) = self.active.clone() else {
            self.fail(MediaError::Backend("no media asset is loaded".to_owned()));
            return;
        };
        if !matches!(active.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            self.fail(MediaError::Backend(
                "the loaded media has no audio master stream".to_owned(),
            ));
            return;
        }
        let from = TimeCode(from.0.clamp(0, active.project_duration.0.saturating_sub(1)));
        self.clock.set_frame(from);
        match AudioRuntime::open(
            &active.path,
            active.project_fps,
            from,
            Arc::clone(&self.clock.position_samples),
            Arc::clone(&self.clock.sample_rate),
        )
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
        if self
            .active
            .as_ref()
            .is_some_and(|active| position >= active.project_duration)
        {
            let end = self
                .active
                .as_ref()
                .map_or(TimeCode::ZERO, |active| active.project_duration);
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
        let Some(active) = self.active.clone() else {
            return;
        };
        if !matches!(active.kind, MediaKind::Video | MediaKind::AudioVideo) {
            return;
        }
        let project_at = TimeCode(
            project_at
                .0
                .clamp(0, active.project_duration.0.saturating_sub(1)),
        );
        let Ok(source_at) = project_to_source(project_at, &active) else {
            return;
        };
        if !self.cache.contains(source_at) {
            if self.video.is_none() {
                match VideoDecoder::open(&active.path, active.source_fps) {
                    Ok(decoder) => self.video = Some(decoder),
                    Err(error) => {
                        self.fail(error);
                        return;
                    }
                }
            }
            let end = TimeCode(
                source_at
                    .0
                    .saturating_add(PREFETCH_FRAMES)
                    .min(active.source_duration.0.saturating_sub(1)),
            );
            if let Some(video) = &mut self.video {
                if let Err(error) = video.decode_window(source_at, end, &mut self.cache) {
                    self.fail(error);
                    return;
                }
            }
        }
        if let Some(frame) = self.cache.frame_at_or_before(source_at) {
            send_latest(
                &self.frames_tx,
                &self.frames_drop_rx,
                (project_at, frame),
            );
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

fn project_to_source(at: TimeCode, active: &ActiveMedia) -> Result<TimeCode, MediaError> {
    map_frames_with_rounding(
        at,
        active.project_fps,
        active.source_fps,
        FrameRounding::Floor,
    )
    .map_err(|error| MediaError::Backend(error.to_string()))
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
