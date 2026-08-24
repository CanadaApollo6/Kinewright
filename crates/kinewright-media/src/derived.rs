use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use crossbeam_channel::{Sender, unbounded};
use kinewright_core::{
    AssetBeats, AssetSceneChanges, AssetSilences, BeatMarker, BeatStatus, Document,
    ExportCancellation, FrameRounding, FrameTexture, MediaAsset, MediaError, MediaKind, Rational,
    SceneChange, SceneStatus, SilenceSpan, SilenceStatus, TimeCode, TimelineBeat,
    TimelineSceneChange, TimelineSilenceSpan, map_frames_with_rounding,
    map_source_range_to_project,
};
use serde::{Deserialize, Serialize};

use crate::{
    audio::decode_audio_range,
    cache::FrameCache,
    decode::VideoDecoder,
    derived_cache::{
        CacheStats, CancellationRegistry, ContentHashes, JsonCache, StatusReporter, cache_root,
        clear_cache_root, inventory_cache_root, spawn_worker,
    },
};

const CACHE_VERSION: u32 = 1;
const ANALYSIS_SAMPLE_RATE: u32 = 48_000;
const SCENE_WINDOW_FRAMES: i64 = 32;
/// Speech-oriented silence threshold. -35 dBFS tracks perceived dialogue
/// onsets and offsets more closely than the previous -40 dBFS threshold.
pub const DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS: i32 = -3_500;
pub const DEFAULT_SILENCE_WINDOW_MILLISECONDS: u32 = 10;
pub const DEFAULT_MINIMUM_SILENCE_FRAMES: i64 = 6;
pub const DEFAULT_SCENE_PROXY_WIDTH: u32 = 320;
pub const DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;
pub const DEFAULT_BEAT_WINDOW_MILLISECONDS: u32 = 20;
pub const DEFAULT_BEAT_MINIMUM_INTERVAL_MILLISECONDS: u32 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SilenceDetectionConfig {
    pub threshold_dbfs_hundredths: i32,
    pub window_milliseconds: u32,
}

impl Default for SilenceDetectionConfig {
    fn default() -> Self {
        Self {
            threshold_dbfs_hundredths: DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS,
            window_milliseconds: DEFAULT_SILENCE_WINDOW_MILLISECONDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneDetectionConfig {
    pub proxy_width: u32,
}

impl Default for SceneDetectionConfig {
    fn default() -> Self {
        Self {
            proxy_width: DEFAULT_SCENE_PROXY_WIDTH,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeatDetectionConfig {
    pub window_milliseconds: u32,
    pub minimum_interval_milliseconds: u32,
}

impl Default for BeatDetectionConfig {
    fn default() -> Self {
        Self {
            window_milliseconds: DEFAULT_BEAT_WINDOW_MILLISECONDS,
            minimum_interval_milliseconds: DEFAULT_BEAT_MINIMUM_INTERVAL_MILLISECONDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DerivedAnalysisConfig {
    pub silence: SilenceDetectionConfig,
    pub scenes: SceneDetectionConfig,
    pub beats: BeatDetectionConfig,
}

pub(crate) struct DerivedAnalysisService {
    jobs: Sender<Job>,
    silence_states: StatusReporter<SilenceStatus>,
    scene_states: StatusReporter<SceneStatus>,
    beat_states: StatusReporter<BeatStatus>,
    silence_cancellations: CancellationRegistry,
    scene_cancellations: CancellationRegistry,
    beat_cancellations: CancellationRegistry,
    config: DerivedAnalysisConfig,
    root: PathBuf,
}

impl DerivedAnalysisService {
    pub(crate) fn new(data_dir: &Path, config: DerivedAnalysisConfig) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = unbounded();
        let silence_states = StatusReporter::new();
        let scene_states = StatusReporter::new();
        let beat_states = StatusReporter::new();
        let silence_cancellations = CancellationRegistry::default();
        let scene_cancellations = CancellationRegistry::default();
        let beat_cancellations = CancellationRegistry::default();
        let root = cache_root(data_dir, "derived-analysis", CACHE_VERSION);
        let mut worker = DerivedAnalysisWorker::new(
            root.clone(),
            silence_states.clone(),
            scene_states.clone(),
            beat_states.clone(),
            silence_cancellations.clone(),
            scene_cancellations.clone(),
            beat_cancellations.clone(),
        );
        spawn_worker(
            "kinewright-derived-analysis",
            "derived analysis",
            jobs_rx,
            move |job| worker.handle(job),
        )?;
        Ok(Self {
            jobs,
            silence_states,
            scene_states,
            beat_states,
            silence_cancellations,
            scene_cancellations,
            beat_cancellations,
            config,
            root,
        })
    }

    pub(crate) fn request_silences(&self, asset: MediaAsset) {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            self.silence_states
                .update(&asset.path, SilenceStatus::NoAudio);
            return;
        }
        let should_queue = self.silence_states.should_queue(&asset.path, |status| {
            matches!(
                status,
                SilenceStatus::Queued
                    | SilenceStatus::Hashing
                    | SilenceStatus::Analyzing
                    | SilenceStatus::Ready(_)
                    | SilenceStatus::NoAudio
            )
        });
        if !should_queue {
            return;
        }
        let Some(cancellation) = self.silence_cancellations.start(&asset.path) else {
            return;
        };
        self.silence_states
            .update(&asset.path, SilenceStatus::Queued);
        let path = asset.path.clone();
        if self
            .jobs
            .send(Job::Silences(
                asset,
                self.config.silence,
                cancellation.clone(),
            ))
            .is_err()
        {
            self.silence_cancellations.finish(&path, &cancellation);
            self.silence_states.update(
                &path,
                SilenceStatus::Failed("derived analysis worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn request_scenes(&self, asset: MediaAsset) {
        if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
            self.scene_states.update(&asset.path, SceneStatus::NoVideo);
            return;
        }
        let should_queue = self.scene_states.should_queue(&asset.path, |status| {
            matches!(
                status,
                SceneStatus::Queued
                    | SceneStatus::Hashing
                    | SceneStatus::Analyzing
                    | SceneStatus::Ready(_)
                    | SceneStatus::NoVideo
            )
        });
        if !should_queue {
            return;
        }
        let Some(cancellation) = self.scene_cancellations.start(&asset.path) else {
            return;
        };
        self.scene_states.update(&asset.path, SceneStatus::Queued);
        let path = asset.path.clone();
        if self
            .jobs
            .send(Job::Scenes(asset, self.config.scenes, cancellation.clone()))
            .is_err()
        {
            self.scene_cancellations.finish(&path, &cancellation);
            self.scene_states.update(
                &path,
                SceneStatus::Failed("derived analysis worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn silence_status(&self, path: &Path) -> SilenceStatus {
        self.silence_states
            .get_or(path, SilenceStatus::NotRequested)
    }

    pub(crate) fn scene_status(&self, path: &Path) -> SceneStatus {
        self.scene_states.get_or(path, SceneStatus::NotRequested)
    }

    pub(crate) fn request_beats(&self, asset: MediaAsset) {
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            self.beat_states.update(&asset.path, BeatStatus::NoAudio);
            return;
        }
        let should_queue = self.beat_states.should_queue(&asset.path, |status| {
            matches!(
                status,
                BeatStatus::Queued
                    | BeatStatus::Hashing
                    | BeatStatus::Analyzing { .. }
                    | BeatStatus::Ready(_)
                    | BeatStatus::NoAudio
            )
        });
        if !should_queue {
            return;
        }
        let Some(cancellation) = self.beat_cancellations.start(&asset.path) else {
            return;
        };
        self.beat_states.update(&asset.path, BeatStatus::Queued);
        let path = asset.path.clone();
        if self
            .jobs
            .send(Job::Beats(asset, self.config.beats, cancellation.clone()))
            .is_err()
        {
            self.beat_cancellations.finish(&path, &cancellation);
            self.beat_states.update(
                &path,
                BeatStatus::Failed("derived analysis worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn beat_status(&self, path: &Path) -> BeatStatus {
        self.beat_states.get_or(path, BeatStatus::NotRequested)
    }

    pub(crate) fn cache_stats(&self) -> Result<CacheStats, MediaError> {
        inventory_cache_root(&self.root)
    }

    pub(crate) fn clear_cache(&self) -> Result<CacheStats, MediaError> {
        clear_cache_root(&self.root)
    }

    pub(crate) fn cancel(&self, path: &Path, kind: kinewright_core::AnalysisKind) -> bool {
        match kind {
            kinewright_core::AnalysisKind::Transcript => false,
            kinewright_core::AnalysisKind::Silence => {
                let cancelled = self.silence_cancellations.cancel(path);
                if cancelled {
                    self.silence_states.update(path, SilenceStatus::Cancelled);
                }
                cancelled
            }
            kinewright_core::AnalysisKind::Scene => {
                let cancelled = self.scene_cancellations.cancel(path);
                if cancelled {
                    self.scene_states.update(path, SceneStatus::Cancelled);
                }
                cancelled
            }
            kinewright_core::AnalysisKind::Beat => {
                let cancelled = self.beat_cancellations.cancel(path);
                if cancelled {
                    self.beat_states.update(path, BeatStatus::Cancelled);
                }
                cancelled
            }
        }
    }

    pub(crate) fn timeline_silences(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        map_timeline_silences(document, range, minimum_source_frames, |asset| {
            match self.silence_status(&asset.path) {
                SilenceStatus::Ready(silences) => Some(silences),
                _ => None,
            }
        })
    }

    pub(crate) fn timeline_scenes(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        map_timeline_scene_changes(document, range, minimum_confidence_basis_points, |asset| {
            match self.scene_status(&asset.path) {
                SceneStatus::Ready(scenes) => Some(scenes),
                _ => None,
            }
        })
    }

    pub(crate) fn timeline_beats(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        minimum_strength_basis_points: u16,
    ) -> Result<Vec<TimelineBeat>, MediaError> {
        map_timeline_beats(
            document,
            range,
            minimum_strength_basis_points,
            |asset| match self.beat_status(&asset.path) {
                BeatStatus::Ready(beats) => Some(beats),
                _ => None,
            },
        )
    }
}

enum Job {
    Silences(MediaAsset, SilenceDetectionConfig, ExportCancellation),
    Scenes(MediaAsset, SceneDetectionConfig, ExportCancellation),
    Beats(MediaAsset, BeatDetectionConfig, ExportCancellation),
}

struct DerivedAnalysisWorker {
    root: PathBuf,
    silence_states: StatusReporter<SilenceStatus>,
    scene_states: StatusReporter<SceneStatus>,
    beat_states: StatusReporter<BeatStatus>,
    silence_cancellations: CancellationRegistry,
    scene_cancellations: CancellationRegistry,
    beat_cancellations: CancellationRegistry,
    hashes: ContentHashes,
}

impl DerivedAnalysisWorker {
    fn new(
        root: PathBuf,
        silence_states: StatusReporter<SilenceStatus>,
        scene_states: StatusReporter<SceneStatus>,
        beat_states: StatusReporter<BeatStatus>,
        silence_cancellations: CancellationRegistry,
        scene_cancellations: CancellationRegistry,
        beat_cancellations: CancellationRegistry,
    ) -> Self {
        Self {
            root,
            silence_states,
            scene_states,
            beat_states,
            silence_cancellations,
            scene_cancellations,
            beat_cancellations,
            hashes: ContentHashes,
        }
    }

    fn handle(&mut self, job: Job) -> bool {
        match job {
            Job::Silences(asset, config, cancellation) => {
                if let Err(error) = self.analyze_silences(&asset, config, &cancellation) {
                    let status = if error == MediaError::Cancelled {
                        SilenceStatus::Cancelled
                    } else {
                        SilenceStatus::Failed(error.to_string())
                    };
                    self.silence_states.update(&asset.path, status);
                }
                self.silence_cancellations
                    .finish(&asset.path, &cancellation);
            }
            Job::Scenes(asset, config, cancellation) => {
                if let Err(error) = self.analyze_scenes(&asset, config, &cancellation) {
                    let status = if error == MediaError::Cancelled {
                        SceneStatus::Cancelled
                    } else {
                        SceneStatus::Failed(error.to_string())
                    };
                    self.scene_states.update(&asset.path, status);
                }
                self.scene_cancellations.finish(&asset.path, &cancellation);
            }
            Job::Beats(asset, config, cancellation) => {
                if let Err(error) = self.analyze_beats(&asset, config, &cancellation) {
                    let status = if error == MediaError::Cancelled {
                        BeatStatus::Cancelled
                    } else {
                        BeatStatus::Failed(error.to_string())
                    };
                    self.beat_states.update(&asset.path, status);
                }
                self.beat_cancellations.finish(&asset.path, &cancellation);
            }
        }
        true
    }

    fn analyze_silences(
        &mut self,
        asset: &MediaAsset,
        config: SilenceDetectionConfig,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        self.silence_states
            .update(&asset.path, SilenceStatus::Hashing);
        let hash = self.content_hash(&asset.path)?;
        check_cancelled(cancellation)?;
        let store = SilenceStore::new(self.root.join("silences"), config);
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            check_cancelled(cancellation)?;
            cached.asset = asset.id;
            self.silence_states
                .update(&asset.path, SilenceStatus::Ready(Arc::new(cached)));
            return Ok(());
        }
        self.silence_states
            .update(&asset.path, SilenceStatus::Analyzing);
        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            ANALYSIS_SAMPLE_RATE,
            1,
            cancellation,
        )?;
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let spans = detect_silences(
            &samples,
            ANALYSIS_SAMPLE_RATE,
            asset.fps,
            asset.duration,
            config,
            TimeCode(1),
        )?;
        let result = AssetSilences {
            asset: asset.id,
            content_sha256: hash,
            source_fps: asset.fps,
            source_frames: asset.duration,
            threshold_dbfs_hundredths: config.threshold_dbfs_hundredths,
            window_milliseconds: config.window_milliseconds,
            spans,
        };
        check_cancelled(cancellation)?;
        store.save(&result)?;
        check_cancelled(cancellation)?;
        self.silence_states
            .update(&asset.path, SilenceStatus::Ready(Arc::new(result)));
        Ok(())
    }

    fn analyze_scenes(
        &mut self,
        asset: &MediaAsset,
        config: SceneDetectionConfig,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        self.scene_states.update(&asset.path, SceneStatus::Hashing);
        let hash = self.content_hash(&asset.path)?;
        check_cancelled(cancellation)?;
        let store = SceneStore::new(self.root.join("scenes"), config);
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            check_cancelled(cancellation)?;
            cached.asset = asset.id;
            self.scene_states
                .update(&asset.path, SceneStatus::Ready(Arc::new(cached)));
            return Ok(());
        }
        self.scene_states
            .update(&asset.path, SceneStatus::Analyzing);
        let changes =
            detect_scene_changes(&asset.path, asset.fps, asset.duration, config, cancellation)?;
        let result = AssetSceneChanges {
            asset: asset.id,
            content_sha256: hash,
            source_fps: asset.fps,
            source_frames: asset.duration,
            proxy_width: config.proxy_width,
            changes,
        };
        check_cancelled(cancellation)?;
        store.save(&result)?;
        check_cancelled(cancellation)?;
        self.scene_states
            .update(&asset.path, SceneStatus::Ready(Arc::new(result)));
        Ok(())
    }

    fn analyze_beats(
        &mut self,
        asset: &MediaAsset,
        config: BeatDetectionConfig,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        self.beat_states.update(&asset.path, BeatStatus::Hashing);
        let hash = self.content_hash(&asset.path)?;
        check_cancelled(cancellation)?;
        let store = BeatStore::new(self.root.join("beats"), config);
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            check_cancelled(cancellation)?;
            cached.asset = asset.id;
            self.beat_states
                .update(&asset.path, BeatStatus::Ready(Arc::new(cached)));
            return Ok(());
        }
        self.beat_states.update(
            &asset.path,
            BeatStatus::Analyzing {
                progress_percent: Some(0),
            },
        );
        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            ANALYSIS_SAMPLE_RATE,
            1,
            cancellation,
        )?;
        check_cancelled(cancellation)?;
        self.beat_states.update(
            &asset.path,
            BeatStatus::Analyzing {
                progress_percent: Some(75),
            },
        );
        let (beats, estimated_bpm_milli) = detect_beats(
            &samples,
            ANALYSIS_SAMPLE_RATE,
            asset.fps,
            asset.duration,
            config,
            cancellation,
        )?;
        let result = AssetBeats {
            asset: asset.id,
            content_sha256: hash,
            source_fps: asset.fps,
            source_frames: asset.duration,
            estimated_bpm_milli,
            beats,
        };
        check_cancelled(cancellation)?;
        store.save(&result)?;
        check_cancelled(cancellation)?;
        self.beat_states
            .update(&asset.path, BeatStatus::Ready(Arc::new(result)));
        Ok(())
    }

    fn content_hash(&mut self, path: &Path) -> Result<String, MediaError> {
        self.hashes.get(path)
    }
}

fn check_cancelled(cancellation: &ExportCancellation) -> Result<(), MediaError> {
    if cancellation.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

// Window lengths are memory-bounded; f64 normalization is stable for every realizable buffer.
#[allow(clippy::cast_precision_loss)]
fn detect_silences(
    samples: &[f32],
    sample_rate: u32,
    source_fps: Rational,
    source_frames: TimeCode,
    config: SilenceDetectionConfig,
    minimum_source_frames: TimeCode,
) -> Result<Vec<SilenceSpan>, MediaError> {
    if config.window_milliseconds == 0 {
        return Err(MediaError::Backend(
            "silence analysis window must be positive".to_owned(),
        ));
    }
    let window_samples = usize::try_from(
        u64::from(sample_rate)
            .saturating_mul(u64::from(config.window_milliseconds))
            .div_ceil(1_000),
    )
    .unwrap_or(usize::MAX)
    .max(1);
    let threshold = 10.0_f64.powf(f64::from(config.threshold_dbfs_hundredths) / 2_000.0);
    let mut spans = Vec::new();
    let mut silent_start = None;
    let sample_fps =
        Rational::new(sample_rate, 1).map_err(|error| MediaError::Backend(error.to_string()))?;
    for (window_index, window) in samples.chunks(window_samples).enumerate() {
        let square_sum = window
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>();
        let rms = (square_sum / window.len().max(1) as f64).sqrt();
        let start_sample = window_index.saturating_mul(window_samples);
        if rms <= threshold {
            silent_start.get_or_insert(start_sample);
        } else if let Some(start) = silent_start.take() {
            push_silence_span(
                &mut spans,
                start,
                start_sample,
                sample_fps,
                source_fps,
                source_frames,
                minimum_source_frames,
            )?;
        }
    }
    if let Some(start) = silent_start {
        push_silence_span(
            &mut spans,
            start,
            samples.len(),
            sample_fps,
            source_fps,
            source_frames,
            minimum_source_frames,
        )?;
    }
    Ok(spans)
}

#[allow(clippy::too_many_arguments)]
fn push_silence_span(
    spans: &mut Vec<SilenceSpan>,
    start_sample: usize,
    end_sample: usize,
    sample_fps: Rational,
    source_fps: Rational,
    source_frames: TimeCode,
    minimum_source_frames: TimeCode,
) -> Result<(), MediaError> {
    let start_sample = TimeCode(i64::try_from(start_sample).unwrap_or(i64::MAX));
    let end_sample = TimeCode(i64::try_from(end_sample).unwrap_or(i64::MAX));
    let start =
        map_frames_with_rounding(start_sample, sample_fps, source_fps, FrameRounding::Floor)
            .map_err(|error| MediaError::Backend(error.to_string()))?;
    let end = map_frames_with_rounding(end_sample, sample_fps, source_fps, FrameRounding::Ceil)
        .map_err(|error| MediaError::Backend(error.to_string()))?;
    let start = TimeCode(start.0.clamp(0, source_frames.0));
    let end = TimeCode(end.0.clamp(start.0, source_frames.0));
    if end.0.saturating_sub(start.0) >= minimum_source_frames.0.max(1) {
        spans.push(SilenceSpan {
            source_start: start,
            source_end: end,
        });
    }
    Ok(())
}

// Energy windows and normalization are intentionally f64 so the same PCM input
// produces stable markers across supported sample rates.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
fn detect_beats(
    samples: &[f32],
    sample_rate: u32,
    source_fps: Rational,
    source_frames: TimeCode,
    config: BeatDetectionConfig,
    cancellation: &ExportCancellation,
) -> Result<(Vec<BeatMarker>, u32), MediaError> {
    const BASELINE_WINDOWS: usize = 8;
    if config.window_milliseconds == 0 || config.minimum_interval_milliseconds == 0 {
        return Err(MediaError::Backend(
            "beat analysis windows and minimum interval must be positive".to_owned(),
        ));
    }
    let window_samples = usize::try_from(
        u64::from(sample_rate)
            .saturating_mul(u64::from(config.window_milliseconds))
            .div_ceil(1_000),
    )
    .unwrap_or(usize::MAX)
    .max(1);
    let mut energies = Vec::with_capacity(samples.len().div_ceil(window_samples));
    for (index, window) in samples.chunks(window_samples).enumerate() {
        if index % 64 == 0 && cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let square_sum = window
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>();
        energies.push((square_sum / window.len().max(1) as f64).sqrt());
    }
    if energies.len() < 3 {
        return Ok((Vec::new(), 0));
    }

    let novelty = energies
        .iter()
        .enumerate()
        .map(|(index, energy)| {
            let start = index.saturating_sub(BASELINE_WINDOWS);
            let history = &energies[start..index];
            let baseline = if history.is_empty() {
                0.0
            } else {
                history.iter().sum::<f64>() / history.len() as f64
            };
            (energy - baseline).max(0.0)
        })
        .collect::<Vec<_>>();
    let maximum = novelty.iter().copied().fold(0.0_f64, f64::max);
    if maximum <= f64::EPSILON {
        return Ok((Vec::new(), 0));
    }
    let threshold = (maximum * 0.12).max(0.002);
    let minimum_windows = usize::try_from(
        u64::from(config.minimum_interval_milliseconds)
            .div_ceil(u64::from(config.window_milliseconds)),
    )
    .unwrap_or(usize::MAX)
    .max(1);
    let mut selected: Vec<usize> = Vec::new();
    for index in 1..novelty.len().saturating_sub(1) {
        let strength = novelty[index];
        if strength < threshold || strength < novelty[index - 1] || strength < novelty[index + 1] {
            continue;
        }
        if let Some(previous) = selected.last_mut()
            && index.saturating_sub(*previous) < minimum_windows
        {
            if strength > novelty[*previous] {
                *previous = index;
            }
            continue;
        }
        selected.push(index);
    }

    let sample_fps =
        Rational::new(sample_rate, 1).map_err(|error| MediaError::Backend(error.to_string()))?;
    let mut beats: Vec<BeatMarker> = Vec::with_capacity(selected.len());
    for window_index in &selected {
        let center_sample = window_index
            .saturating_mul(window_samples)
            .saturating_add(window_samples / 2);
        let source_frame = map_frames_with_rounding(
            TimeCode(i64::try_from(center_sample).unwrap_or(i64::MAX)),
            sample_fps,
            source_fps,
            FrameRounding::Nearest,
        )
        .map_err(|error| MediaError::Backend(error.to_string()))?;
        let source_frame = TimeCode(source_frame.0.clamp(0, source_frames.0.saturating_sub(1)));
        let strength_basis_points =
            u16::try_from(((novelty[*window_index] / maximum) * 10_000.0).round() as u64)
                .unwrap_or(10_000)
                .min(10_000);
        if let Some(previous) = beats.last_mut()
            && previous.source_frame == source_frame
        {
            previous.strength_basis_points =
                previous.strength_basis_points.max(strength_basis_points);
            continue;
        }
        beats.push(BeatMarker {
            source_frame,
            strength_basis_points,
        });
    }
    Ok((
        beats,
        estimate_tempo_milli(&selected, window_samples, sample_rate),
    ))
}

fn estimate_tempo_milli(selected: &[usize], window_samples: usize, sample_rate: u32) -> u32 {
    let mut intervals = selected
        .windows(2)
        .filter_map(|pair| pair[1].checked_sub(pair[0]))
        .map(|windows| windows.saturating_mul(window_samples))
        .filter(|samples| *samples > 0)
        .collect::<Vec<_>>();
    if intervals.is_empty() {
        return 0;
    }
    intervals.sort_unstable();
    let median = intervals[intervals.len() / 2];
    let numerator = u64::from(sample_rate).saturating_mul(60_000);
    let mut bpm_milli = numerator.saturating_div(u64::try_from(median).unwrap_or(u64::MAX));
    while bpm_milli > 0 && bpm_milli < 40_000 {
        bpm_milli = bpm_milli.saturating_mul(2);
    }
    while bpm_milli > 240_000 {
        bpm_milli = bpm_milli.saturating_div(2);
    }
    u32::try_from(bpm_milli).unwrap_or(u32::MAX)
}

fn detect_scene_changes(
    path: &Path,
    fps: Rational,
    duration: TimeCode,
    config: SceneDetectionConfig,
    cancellation: &ExportCancellation,
) -> Result<Vec<SceneChange>, MediaError> {
    let proxy_width = config.proxy_width.clamp(32, 512);
    let mut decoder = VideoDecoder::open_scaled(path, fps, Some(proxy_width))?;
    let mut cache: FrameCache<FrameTexture> =
        FrameCache::new(usize::try_from(SCENE_WINDOW_FRAMES + 1).unwrap_or(33));
    let mut previous_pixels: Option<Arc<Vec<u8>>> = None;
    let mut previous_difference: Option<f64> = None;
    let mut changes = Vec::new();
    let mut start = 0_i64;
    while start < duration.0 {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let end = start
            .saturating_add(SCENE_WINDOW_FRAMES - 1)
            .min(duration.0.saturating_sub(1));
        if start == 0 {
            decoder.decode_window(TimeCode(start), TimeCode(end), &mut cache)?;
        } else {
            decoder.decode_window_sequential(TimeCode(start), TimeCode(end), &mut cache)?;
        }
        for frame_index in start..=end {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let frame = cache
                .frame_at_or_before(TimeCode(frame_index))
                .ok_or_else(|| {
                    MediaError::Backend(format!(
                        "scene analysis did not decode source frame {frame_index}"
                    ))
                })?;
            if let Some(previous) = previous_pixels.as_deref() {
                let difference = frame_difference(previous, &frame.rgba);
                if let Some(previous_difference) = previous_difference {
                    // This mirrors scdet's temporal-spike behavior: persistent
                    // motion is suppressed while a one-frame discontinuity is
                    // retained. Histogram distance makes hard palette cuts
                    // robust; per-pixel SAD catches composition changes.
                    let confidence = difference
                        .min((difference - previous_difference).abs())
                        .clamp(0.0, 1.0);
                    let basis_points = confidence_basis_points(confidence);
                    if basis_points > 0 {
                        changes.push(SceneChange {
                            source_frame: TimeCode(frame_index),
                            confidence_basis_points: basis_points,
                        });
                    }
                }
                previous_difference = Some(difference);
            }
            previous_pixels = Some(Arc::clone(&frame.rgba));
        }
        start = end.saturating_add(1);
    }
    Ok(changes)
}

// Aggregate pixel counts are intentionally normalized in f64 for scene scoring.
#[allow(clippy::cast_precision_loss)]
fn frame_difference(previous: &[u8], current: &[u8]) -> f64 {
    let pixel_count = previous.len().min(current.len()) / 4;
    if pixel_count == 0 {
        return 0.0;
    }
    let mut previous_histogram = [0_u64; 64];
    let mut current_histogram = [0_u64; 64];
    let mut sad = 0_u64;
    for (before, after) in previous
        .as_chunks::<4>()
        .0
        .iter()
        .zip(current.as_chunks::<4>().0.iter())
        .take(pixel_count)
    {
        let before_luma =
            (u32::from(before[0]) * 77 + u32::from(before[1]) * 150 + u32::from(before[2]) * 29)
                >> 8;
        let after_luma =
            (u32::from(after[0]) * 77 + u32::from(after[1]) * 150 + u32::from(after[2]) * 29) >> 8;
        sad = sad.saturating_add(u64::from(before_luma.abs_diff(after_luma)));
        previous_histogram[usize::try_from(before_luma >> 2).unwrap_or(63).min(63)] += 1;
        current_histogram[usize::try_from(after_luma >> 2).unwrap_or(63).min(63)] += 1;
    }
    let normalized_sad = sad as f64 / (pixel_count as f64 * 255.0);
    let histogram_l1 = previous_histogram
        .iter()
        .zip(current_histogram)
        .map(|(before, after)| before.abs_diff(after))
        .sum::<u64>() as f64
        / (pixel_count as f64 * 2.0);
    histogram_l1.max(normalized_sad * 1.5).clamp(0.0, 1.0)
}

// Confidence is clamped to 0..=1 before this rounded conversion to basis points.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn confidence_basis_points(confidence: f64) -> u16 {
    (confidence * 10_000.0).round() as u16
}

pub(crate) fn map_timeline_silences<F>(
    document: &Document,
    range: Option<Range<TimeCode>>,
    minimum_source_frames: TimeCode,
    mut silences_for: F,
) -> Result<Vec<TimelineSilenceSpan>, MediaError>
where
    F: FnMut(&MediaAsset) -> Option<Arc<AssetSilences>>,
{
    let requested = validated_range(document, range, "timeline silence")?;
    let minimum = minimum_source_frames.0.max(1);
    let mut analyses = BTreeMap::new();
    let mut mapped = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if !clip.content.is_media() {
                continue;
            }
            // Derived source timestamps no longer align project-linearly on a
            // speed-changed clip; remapping them is deferred, so skip for now.
            if clip.speed_percent != 100 {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let cached_silences = analyses
                .entry(asset.id)
                .or_insert_with(|| silences_for(asset));
            let Some(silences) = cached_silences else {
                continue;
            };
            let clip_duration =
                map_source_range_to_project(clip.source_range.clone(), asset.fps, document.fps)
                    .map_err(|error| MediaError::Backend(error.to_string()))?;
            let clip_end = clip
                .timeline_start
                .checked_add(clip_duration)
                .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?;
            for span in &silences.spans {
                let source_start = span.source_start.max(clip.source_range.start);
                let source_end = span.source_end.min(clip.source_range.end);
                if source_end.0.saturating_sub(source_start.0) < minimum {
                    continue;
                }
                let start_offset = source_start
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let end_offset = source_end
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let project_start = clip
                    .timeline_start
                    .checked_add(
                        map_frames_with_rounding(
                            start_offset,
                            asset.fps,
                            document.fps,
                            FrameRounding::Floor,
                        )
                        .map_err(|error| MediaError::Backend(error.to_string()))?,
                    )
                    .ok_or_else(|| {
                        MediaError::Backend("timeline position overflowed".to_owned())
                    })?;
                let project_end = clip
                    .timeline_start
                    .checked_add(
                        map_frames_with_rounding(
                            end_offset,
                            asset.fps,
                            document.fps,
                            FrameRounding::Ceil,
                        )
                        .map_err(|error| MediaError::Backend(error.to_string()))?,
                    )
                    .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?
                    .min(clip_end);
                if project_end <= requested.start
                    || project_start >= requested.end
                    || project_end <= project_start
                {
                    continue;
                }
                mapped.push(TimelineSilenceSpan {
                    asset: asset.id,
                    track: track.id,
                    clip: clip.id,
                    source_start,
                    source_end,
                    project_start,
                    project_end,
                });
            }
        }
    }
    mapped.sort_by_key(|span| (span.project_start, span.track, span.clip, span.source_start));
    Ok(mapped)
}

pub(crate) fn map_timeline_scene_changes<F>(
    document: &Document,
    range: Option<Range<TimeCode>>,
    minimum_confidence_basis_points: u16,
    mut scenes_for: F,
) -> Result<Vec<TimelineSceneChange>, MediaError>
where
    F: FnMut(&MediaAsset) -> Option<Arc<AssetSceneChanges>>,
{
    let requested = validated_range(document, range, "timeline scene")?;
    let mut analyses = BTreeMap::new();
    let mut mapped = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if !clip.content.is_media() {
                continue;
            }
            // Derived source timestamps no longer align project-linearly on a
            // speed-changed clip; remapping them is deferred, so skip for now.
            if clip.speed_percent != 100 {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let cached_scenes = analyses
                .entry(asset.id)
                .or_insert_with(|| scenes_for(asset));
            let Some(scenes) = cached_scenes else {
                continue;
            };
            for change in &scenes.changes {
                if change.confidence_basis_points < minimum_confidence_basis_points
                    || change.source_frame < clip.source_range.start
                    || change.source_frame >= clip.source_range.end
                {
                    continue;
                }
                let offset = change
                    .source_frame
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let project_frame = clip
                    .timeline_start
                    .checked_add(
                        map_frames_with_rounding(
                            offset,
                            asset.fps,
                            document.fps,
                            FrameRounding::Nearest,
                        )
                        .map_err(|error| MediaError::Backend(error.to_string()))?,
                    )
                    .ok_or_else(|| {
                        MediaError::Backend("timeline position overflowed".to_owned())
                    })?;
                if project_frame < requested.start || project_frame >= requested.end {
                    continue;
                }
                mapped.push(TimelineSceneChange {
                    asset: asset.id,
                    track: track.id,
                    clip: clip.id,
                    source_frame: change.source_frame,
                    project_frame,
                    confidence_basis_points: change.confidence_basis_points,
                });
            }
        }
    }
    mapped.sort_by_key(|change| {
        (
            change.project_frame,
            change.track,
            change.clip,
            change.source_frame,
        )
    });
    Ok(mapped)
}

pub(crate) fn map_timeline_beats<F>(
    document: &Document,
    range: Option<Range<TimeCode>>,
    minimum_strength_basis_points: u16,
    mut beats_for: F,
) -> Result<Vec<TimelineBeat>, MediaError>
where
    F: FnMut(&MediaAsset) -> Option<Arc<AssetBeats>>,
{
    let requested = validated_range(document, range, "timeline beat")?;
    let mut analyses = BTreeMap::new();
    let mut mapped = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if !clip.content.is_media() || clip.speed_percent != 100 {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let cached_beats = analyses.entry(asset.id).or_insert_with(|| beats_for(asset));
            let Some(beats) = cached_beats else {
                continue;
            };
            for beat in &beats.beats {
                if beat.strength_basis_points < minimum_strength_basis_points
                    || beat.source_frame < clip.source_range.start
                    || beat.source_frame >= clip.source_range.end
                {
                    continue;
                }
                let offset = beat
                    .source_frame
                    .checked_sub(clip.source_range.start)
                    .ok_or_else(|| MediaError::Backend("source position underflowed".to_owned()))?;
                let project_frame = clip
                    .timeline_start
                    .checked_add(
                        map_frames_with_rounding(
                            offset,
                            asset.fps,
                            document.fps,
                            FrameRounding::Nearest,
                        )
                        .map_err(|error| MediaError::Backend(error.to_string()))?,
                    )
                    .ok_or_else(|| {
                        MediaError::Backend("timeline position overflowed".to_owned())
                    })?;
                if project_frame < requested.start || project_frame >= requested.end {
                    continue;
                }
                mapped.push(TimelineBeat {
                    asset: asset.id,
                    track: track.id,
                    clip: clip.id,
                    source_frame: beat.source_frame,
                    project_frame,
                    strength_basis_points: beat.strength_basis_points,
                    estimated_bpm_milli: beats.estimated_bpm_milli,
                });
            }
        }
    }
    mapped.sort_by_key(|beat| (beat.project_frame, beat.track, beat.clip, beat.source_frame));
    Ok(mapped)
}

fn validated_range(
    document: &Document,
    range: Option<Range<TimeCode>>,
    label: &str,
) -> Result<Range<TimeCode>, MediaError> {
    let requested = range.unwrap_or(TimeCode::ZERO..document.duration);
    if requested.start < TimeCode::ZERO || requested.end <= requested.start {
        return Err(MediaError::Backend(format!(
            "{label} range must be non-empty and non-negative: {}..{}",
            requested.start.0, requested.end.0
        )));
    }
    Ok(requested)
}

struct SilenceStore {
    cache: JsonCache,
    config: SilenceDetectionConfig,
}

impl SilenceStore {
    fn new(root: PathBuf, config: SilenceDetectionConfig) -> Self {
        Self {
            cache: JsonCache::new(root, CACHE_VERSION, "silence"),
            config,
        }
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.cache.path_for(
            hash,
            &format!(
                "-t{}-w{}",
                self.config.threshold_dbfs_hundredths, self.config.window_milliseconds
            ),
        )
    }

    fn load(
        &self,
        hash: &str,
        fps: Rational,
        frames: TimeCode,
    ) -> Result<Option<AssetSilences>, MediaError> {
        self.cache
            .load(&self.path_for(hash), |stored: StoredSilences| {
                let silences = stored.silences;
                (silences.content_sha256 == hash
                    && silences.source_fps == fps
                    && silences.source_frames == frames
                    && silences.threshold_dbfs_hundredths == self.config.threshold_dbfs_hundredths
                    && silences.window_milliseconds == self.config.window_milliseconds
                    && !silences.spans.iter().any(|span| {
                        span.source_start < TimeCode::ZERO
                            || span.source_end <= span.source_start
                            || span.source_end > frames
                    }))
                .then_some(silences)
            })
    }

    fn save(&self, silences: &AssetSilences) -> Result<(), MediaError> {
        self.cache.save(
            &self.path_for(&silences.content_sha256),
            &StoredSilences {
                silences: silences.clone(),
            },
        )
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSilences {
    silences: AssetSilences,
}

struct SceneStore {
    cache: JsonCache,
    config: SceneDetectionConfig,
}

impl SceneStore {
    fn new(root: PathBuf, config: SceneDetectionConfig) -> Self {
        Self {
            cache: JsonCache::new(root, CACHE_VERSION, "scene"),
            config,
        }
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.cache
            .path_for(hash, &format!("-w{}", self.config.proxy_width))
    }

    fn load(
        &self,
        hash: &str,
        fps: Rational,
        frames: TimeCode,
    ) -> Result<Option<AssetSceneChanges>, MediaError> {
        self.cache
            .load(&self.path_for(hash), |stored: StoredScenes| {
                let scenes = stored.scenes;
                (scenes.content_sha256 == hash
                    && scenes.source_fps == fps
                    && scenes.source_frames == frames
                    && scenes.proxy_width == self.config.proxy_width
                    && !scenes.changes.iter().any(|change| {
                        change.source_frame <= TimeCode::ZERO || change.source_frame >= frames
                    }))
                .then_some(scenes)
            })
    }

    fn save(&self, scenes: &AssetSceneChanges) -> Result<(), MediaError> {
        self.cache.save(
            &self.path_for(&scenes.content_sha256),
            &StoredScenes {
                scenes: scenes.clone(),
            },
        )
    }
}

#[derive(Serialize, Deserialize)]
struct StoredScenes {
    scenes: AssetSceneChanges,
}

struct BeatStore {
    cache: JsonCache,
    config: BeatDetectionConfig,
}

impl BeatStore {
    fn new(root: PathBuf, config: BeatDetectionConfig) -> Self {
        Self {
            cache: JsonCache::new(root, CACHE_VERSION, "beat"),
            config,
        }
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.cache.path_for(
            hash,
            &format!(
                "-w{}-i{}",
                self.config.window_milliseconds, self.config.minimum_interval_milliseconds
            ),
        )
    }

    fn load(
        &self,
        hash: &str,
        fps: Rational,
        frames: TimeCode,
    ) -> Result<Option<AssetBeats>, MediaError> {
        self.cache
            .load(&self.path_for(hash), |stored: StoredBeats| {
                let beats = stored.beats;
                (beats.content_sha256 == hash
                    && beats.source_fps == fps
                    && beats.source_frames == frames
                    && !beats.beats.iter().any(|beat| {
                        beat.source_frame < TimeCode::ZERO
                            || beat.source_frame >= frames
                            || beat.strength_basis_points > 10_000
                    }))
                .then_some(beats)
            })
    }

    fn save(&self, beats: &AssetBeats) -> Result<(), MediaError> {
        self.cache.save(
            &self.path_for(&beats.content_sha256),
            &StoredBeats {
                beats: beats.clone(),
            },
        )
    }
}

#[derive(Serialize, Deserialize)]
struct StoredBeats {
    beats: AssetBeats,
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use kinewright_core::{
        AssetId, Clip, ClipContent, ClipId, MediaAsset, Track, TrackId, TrackKind,
    };

    use super::*;
    use crate::test_support::{TempDirectory, ffmpeg_executable};

    // The generated fixture intentionally narrows its analytic f64 sine wave to PCM f32.
    #[allow(clippy::cast_possible_truncation)]
    fn tone_silence_tone(sample_rate: u32) -> Vec<f32> {
        let seconds = 3_u32;
        (0..sample_rate * seconds)
            .map(|sample| {
                if (sample_rate..sample_rate * 2).contains(&sample) {
                    0.0
                } else {
                    let phase =
                        std::f64::consts::TAU * 440.0 * f64::from(sample) / f64::from(sample_rate);
                    (phase.sin() * 0.5) as f32
                }
            })
            .collect()
    }

    #[allow(clippy::cast_possible_truncation)]
    fn click_track(sample_rate: u32, bpm: u32, seconds: u32) -> Vec<f32> {
        let interval = sample_rate.saturating_mul(60).saturating_div(bpm);
        let click_length = sample_rate / 100;
        (0..sample_rate.saturating_mul(seconds))
            .map(|sample| {
                if sample % interval < click_length {
                    let phase = std::f64::consts::TAU * 1_000.0 * f64::from(sample)
                        / f64::from(sample_rate);
                    (phase.sin() * 0.9) as f32
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn silence_spans_are_exact_across_multiple_sample_rates() {
        let fps = Rational::new(30, 1).unwrap();
        for sample_rate in [22_050, 44_100, 48_000] {
            let spans = detect_silences(
                &tone_silence_tone(sample_rate),
                sample_rate,
                fps,
                TimeCode(90),
                SilenceDetectionConfig::default(),
                TimeCode(1),
            )
            .unwrap();
            assert_eq!(spans.len(), 1, "sample rate {sample_rate}");
            assert!(
                spans[0].source_start.0.abs_diff(30) <= 1,
                "sample rate {sample_rate}: {spans:?}"
            );
            assert!(
                spans[0].source_end.0.abs_diff(60) <= 1,
                "sample rate {sample_rate}: {spans:?}"
            );
        }
    }

    #[test]
    fn beat_detector_finds_a_stable_one_hundred_twenty_bpm_click_track() {
        let fps = Rational::new(30, 1).unwrap();
        for sample_rate in [44_100, 48_000] {
            let (beats, bpm_milli) = detect_beats(
                &click_track(sample_rate, 120, 4),
                sample_rate,
                fps,
                TimeCode(120),
                BeatDetectionConfig::default(),
                &ExportCancellation::default(),
            )
            .unwrap();
            assert!(bpm_milli.abs_diff(120_000) <= 2_500, "{bpm_milli}");
            assert!(beats.len() >= 6, "sample rate {sample_rate}: {beats:?}");
            for (index, beat) in beats.iter().take(6).enumerate() {
                let expected = i64::try_from(index + 1).unwrap() * 15;
                assert!(
                    beat.source_frame.0.abs_diff(expected) <= 1,
                    "sample rate {sample_rate}: {beats:?}"
                );
            }
        }
    }

    #[test]
    fn beat_detector_honors_pre_cancelled_work() {
        let cancellation = ExportCancellation::default();
        cancellation.cancel();
        assert_eq!(
            detect_beats(
                &click_track(48_000, 120, 2),
                48_000,
                Rational::new(30, 1).unwrap(),
                TimeCode(60),
                BeatDetectionConfig::default(),
                &cancellation,
            ),
            Err(MediaError::Cancelled)
        );
    }

    #[test]
    fn decoded_pcm_audio_preserves_known_silence_gaps() {
        crate::initialize_ffmpeg().unwrap();
        let directory = TempDirectory::new("decoded-silence");
        let fps = Rational::new(30, 1).unwrap();
        for sample_rate in [22_050, 48_000] {
            let path = directory.path(&format!("tone-gap-{sample_rate}.wav"));
            write_pcm16_mono(&path, sample_rate, &tone_silence_tone(sample_rate));
            let decoded = decode_audio_range(
                &path,
                fps,
                TimeCode::ZERO,
                TimeCode(90),
                ANALYSIS_SAMPLE_RATE,
                1,
                &ExportCancellation::default(),
            )
            .unwrap();
            let spans = detect_silences(
                &decoded,
                ANALYSIS_SAMPLE_RATE,
                fps,
                TimeCode(90),
                SilenceDetectionConfig::default(),
                TimeCode(1),
            )
            .unwrap();
            assert_eq!(spans.len(), 1, "sample rate {sample_rate}: {spans:?}");
            assert!(
                spans[0].source_start.0.abs_diff(30) <= 1,
                "sample rate {sample_rate}: {spans:?}"
            );
            assert!(
                spans[0].source_end.0.abs_diff(60) <= 1,
                "sample rate {sample_rate}: {spans:?}"
            );
        }
    }

    #[test]
    fn minimum_duration_filters_short_energy_gaps() {
        let sample_rate = 1_000;
        let mut samples = vec![0.5; 1_000];
        samples[400..500].fill(0.0);
        let config = SilenceDetectionConfig {
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 10,
        };
        assert_eq!(
            detect_silences(
                &samples,
                sample_rate,
                Rational::new(100, 1).unwrap(),
                TimeCode(100),
                config,
                TimeCode(11),
            )
            .unwrap(),
            Vec::new()
        );
        assert_eq!(
            detect_silences(
                &samples,
                sample_rate,
                Rational::new(100, 1).unwrap(),
                TimeCode(100),
                config,
                TimeCode(10),
            )
            .unwrap(),
            vec![SilenceSpan {
                source_start: TimeCode(40),
                source_end: TimeCode(50),
            }]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn derived_caches_round_trip_and_ignore_corruption() {
        let directory = TempDirectory::new("derived-cache");
        let silence_store =
            SilenceStore::new(directory.path("silence"), SilenceDetectionConfig::default());
        let silence = AssetSilences {
            asset: AssetId(1),
            content_sha256: "a".repeat(64),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(90),
            threshold_dbfs_hundredths: DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS,
            window_milliseconds: DEFAULT_SILENCE_WINDOW_MILLISECONDS,
            spans: vec![SilenceSpan {
                source_start: TimeCode(30),
                source_end: TimeCode(60),
            }],
        };
        silence_store.save(&silence).unwrap();
        let mut loaded = silence_store
            .load(
                &silence.content_sha256,
                silence.source_fps,
                silence.source_frames,
            )
            .unwrap()
            .unwrap();
        loaded.asset = AssetId(99);
        assert_eq!(loaded.spans, silence.spans);
        fs::write(silence_store.path_for(&silence.content_sha256), b"broken").unwrap();
        assert!(
            silence_store
                .load(
                    &silence.content_sha256,
                    silence.source_fps,
                    silence.source_frames
                )
                .unwrap()
                .is_none()
        );

        let scene_store = SceneStore::new(directory.path("scene"), SceneDetectionConfig::default());
        let scenes = AssetSceneChanges {
            asset: AssetId(2),
            content_sha256: "b".repeat(64),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(90),
            proxy_width: DEFAULT_SCENE_PROXY_WIDTH,
            changes: vec![SceneChange {
                source_frame: TimeCode(30),
                confidence_basis_points: 8_000,
            }],
        };
        scene_store.save(&scenes).unwrap();
        assert_eq!(
            scene_store
                .load(
                    &scenes.content_sha256,
                    scenes.source_fps,
                    scenes.source_frames
                )
                .unwrap()
                .unwrap()
                .changes,
            scenes.changes
        );
        fs::write(scene_store.path_for(&scenes.content_sha256), b"broken").unwrap();
        assert!(
            scene_store
                .load(
                    &scenes.content_sha256,
                    scenes.source_fps,
                    scenes.source_frames
                )
                .unwrap()
                .is_none()
        );

        let beat_store = BeatStore::new(directory.path("beat"), BeatDetectionConfig::default());
        let beats = AssetBeats {
            asset: AssetId(3),
            content_sha256: "c".repeat(64),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(90),
            estimated_bpm_milli: 120_000,
            beats: vec![BeatMarker {
                source_frame: TimeCode(15),
                strength_basis_points: 9_000,
            }],
        };
        beat_store.save(&beats).unwrap();
        assert_eq!(
            beat_store
                .load(&beats.content_sha256, beats.source_fps, beats.source_frames)
                .unwrap()
                .unwrap()
                .beats,
            beats.beats
        );
        fs::write(beat_store.path_for(&beats.content_sha256), b"broken").unwrap();
        assert!(
            beat_store
                .load(&beats.content_sha256, beats.source_fps, beats.source_frames)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn generated_hard_cuts_are_detected_without_continuous_false_positives() {
        crate::initialize_ffmpeg().unwrap();
        let directory = TempDirectory::new("scene-detection");
        let hard_cuts = directory.path("hard-cuts.mp4");
        let continuous = directory.path("continuous.mp4");
        let ffmpeg = ffmpeg_executable();
        let hard_status = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=160x90:rate=10:duration=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:size=160x90:rate=10:duration=1",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10:duration=1",
                "-filter_complex",
                "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]",
                "-map",
                "[v]",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&hard_cuts)
            .status()
            .unwrap();
        assert!(hard_status.success());
        let continuous_status = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10:duration=3",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&continuous)
            .status()
            .unwrap();
        assert!(continuous_status.success());

        let changes = detect_scene_changes(
            &hard_cuts,
            Rational::new(10, 1).unwrap(),
            TimeCode(30),
            SceneDetectionConfig::default(),
            &ExportCancellation::default(),
        )
        .unwrap();
        let strong = changes
            .iter()
            .filter(|change| {
                change.confidence_basis_points >= DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS
            })
            .collect::<Vec<_>>();
        assert_eq!(strong.len(), 2, "all candidates: {changes:?}");
        assert!(strong[0].source_frame.0.abs_diff(10) <= 1);
        assert!(strong[1].source_frame.0.abs_diff(20) <= 1);
        assert!(strong[0].confidence_basis_points > 2_000);
        assert!(strong[1].confidence_basis_points > 2_000);

        let continuous_changes = detect_scene_changes(
            &continuous,
            Rational::new(10, 1).unwrap(),
            TimeCode(30),
            SceneDetectionConfig::default(),
            &ExportCancellation::default(),
        )
        .unwrap();
        assert!(
            continuous_changes.iter().all(|change| {
                change.confidence_basis_points < DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS
            }),
            "continuous candidates: {continuous_changes:?}"
        );
    }

    #[test]
    fn timeline_mapping_clips_and_orders_derived_data() {
        let document = fixture_document();
        let silences = Arc::new(AssetSilences {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(24, 1).unwrap(),
            source_frames: TimeCode(100),
            threshold_dbfs_hundredths: DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS,
            window_milliseconds: DEFAULT_SILENCE_WINDOW_MILLISECONDS,
            spans: vec![SilenceSpan {
                source_start: TimeCode(9),
                source_end: TimeCode(20),
            }],
        });
        let mapped = map_timeline_silences(&document, None, TimeCode(1), |_| {
            Some(Arc::clone(&silences))
        })
        .unwrap();
        assert_eq!(mapped[0].source_start, TimeCode(10));
        assert_eq!(mapped[0].project_start, TimeCode(100));

        let scenes = Arc::new(AssetSceneChanges {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(24, 1).unwrap(),
            source_frames: TimeCode(100),
            proxy_width: DEFAULT_SCENE_PROXY_WIDTH,
            changes: vec![SceneChange {
                source_frame: TimeCode(18),
                confidence_basis_points: 8_000,
            }],
        });
        let mapped =
            map_timeline_scene_changes(&document, None, 1_000, |_| Some(Arc::clone(&scenes)))
                .unwrap();
        assert_eq!(mapped[0].project_frame, TimeCode(110));

        let beats = Arc::new(AssetBeats {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(24, 1).unwrap(),
            source_frames: TimeCode(100),
            estimated_bpm_milli: 120_000,
            beats: vec![BeatMarker {
                source_frame: TimeCode(18),
                strength_basis_points: 9_000,
            }],
        });
        let mapped =
            map_timeline_beats(&document, None, 5_000, |_| Some(Arc::clone(&beats))).unwrap();
        assert_eq!(mapped[0].project_frame, TimeCode(110));
        assert_eq!(mapped[0].estimated_bpm_milli, 120_000);
    }

    fn fixture_document() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(100),
            fps: Rational::new(24, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((160, 90)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let duration = map_source_range_to_project(
            TimeCode(10)..TimeCode(40),
            asset.fps,
            Rational::new(30, 1).unwrap(),
        )
        .unwrap();
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(10)..TimeCode(40),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(100),
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
            resolution: (160, 90),
            duration: TimeCode(100 + duration.0),
        }
    }

    // The clamped, rounded fixture samples are intentionally quantized to PCM i16.
    #[allow(clippy::cast_possible_truncation)]
    fn write_pcm16_mono(path: &Path, sample_rate: u32, samples: &[f32]) {
        let data_bytes = u32::try_from(samples.len().saturating_mul(2)).unwrap();
        let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in samples {
            let pcm = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            bytes.extend_from_slice(&pcm.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }
}
