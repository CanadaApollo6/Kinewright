use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use crossbeam_channel::{Sender, unbounded};
use openreel_core::{
    AssetId, AssetSceneChanges, AssetSilences, ClipContent, Document, ExportCancellation,
    FrameRounding, MediaAsset, MediaError, MediaKind, Rational, SceneChange, SceneStatus,
    SilenceSpan, SilenceStatus, TimeCode, TimelineSceneChange, TimelineSilenceSpan,
    map_frames_with_rounding, map_source_range_to_project,
};
use serde::{Deserialize, Serialize};

use crate::{
    audio::decode_audio_range,
    cache::FrameCache,
    decode::VideoDecoder,
    derived_cache::{ContentHashes, JsonCache, StatusReporter, cache_root, spawn_worker},
};

const CACHE_VERSION: u32 = 1;
const ANALYSIS_SAMPLE_RATE: u32 = 48_000;
const SCENE_WINDOW_FRAMES: i64 = 32;
pub const DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS: i32 = -4_000;
pub const DEFAULT_SILENCE_WINDOW_MILLISECONDS: u32 = 10;
pub const DEFAULT_MINIMUM_SILENCE_FRAMES: i64 = 6;
pub const DEFAULT_SCENE_PROXY_WIDTH: u32 = 320;
pub const DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DerivedAnalysisConfig {
    pub silence: SilenceDetectionConfig,
    pub scenes: SceneDetectionConfig,
}

pub(crate) struct DerivedAnalysisService {
    jobs: Sender<Job>,
    silence_states: StatusReporter<SilenceStatus>,
    scene_states: StatusReporter<SceneStatus>,
    config: DerivedAnalysisConfig,
}

impl DerivedAnalysisService {
    pub(crate) fn new(data_dir: &Path, config: DerivedAnalysisConfig) -> Result<Self, MediaError> {
        let (jobs, jobs_rx) = unbounded();
        let silence_states = StatusReporter::new();
        let scene_states = StatusReporter::new();
        let mut worker = DerivedAnalysisWorker::new(
            cache_root(data_dir, "derived-analysis", CACHE_VERSION),
            silence_states.clone(),
            scene_states.clone(),
        );
        spawn_worker(
            "openreel-derived-analysis",
            "derived analysis",
            jobs_rx,
            move |job| worker.handle(job),
        )?;
        Ok(Self {
            jobs,
            silence_states,
            scene_states,
            config,
        })
    }

    pub(crate) fn request_silences(&self, asset: MediaAsset) {
        let asset_id = asset.id;
        if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
            self.silence_states.update(asset.id, SilenceStatus::NoAudio);
            return;
        }
        let should_queue = self.silence_states.should_queue(asset.id, |status| {
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
        self.silence_states.update(asset.id, SilenceStatus::Queued);
        if self
            .jobs
            .send(Job::Silences(asset, self.config.silence))
            .is_err()
        {
            self.silence_states.update(
                asset_id,
                SilenceStatus::Failed("derived analysis worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn request_scenes(&self, asset: MediaAsset) {
        let asset_id = asset.id;
        if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
            self.scene_states.update(asset.id, SceneStatus::NoVideo);
            return;
        }
        let should_queue = self.scene_states.should_queue(asset.id, |status| {
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
        self.scene_states.update(asset.id, SceneStatus::Queued);
        if self
            .jobs
            .send(Job::Scenes(asset, self.config.scenes))
            .is_err()
        {
            self.scene_states.update(
                asset_id,
                SceneStatus::Failed("derived analysis worker stopped".to_owned()),
            );
        }
    }

    pub(crate) fn silence_status(&self, asset: AssetId) -> SilenceStatus {
        self.silence_states
            .get_or(asset, SilenceStatus::NotRequested)
    }

    pub(crate) fn scene_status(&self, asset: AssetId) -> SceneStatus {
        self.scene_states.get_or(asset, SceneStatus::NotRequested)
    }

    pub(crate) fn timeline_silences(
        &self,
        document: &Document,
        range: Option<Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        map_timeline_silences(document, range, minimum_source_frames, |asset| {
            match self.silence_status(asset) {
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
            match self.scene_status(asset) {
                SceneStatus::Ready(scenes) => Some(scenes),
                _ => None,
            }
        })
    }
}

enum Job {
    Silences(MediaAsset, SilenceDetectionConfig),
    Scenes(MediaAsset, SceneDetectionConfig),
}

struct DerivedAnalysisWorker {
    root: PathBuf,
    silence_states: StatusReporter<SilenceStatus>,
    scene_states: StatusReporter<SceneStatus>,
    hashes: ContentHashes,
}

impl DerivedAnalysisWorker {
    fn new(
        root: PathBuf,
        silence_states: StatusReporter<SilenceStatus>,
        scene_states: StatusReporter<SceneStatus>,
    ) -> Self {
        Self {
            root,
            silence_states,
            scene_states,
            hashes: ContentHashes::default(),
        }
    }

    fn handle(&mut self, job: Job) -> bool {
        match job {
            Job::Silences(asset, config) => {
                if let Err(error) = self.analyze_silences(&asset, config) {
                    self.silence_states
                        .update(asset.id, SilenceStatus::Failed(error.to_string()));
                }
            }
            Job::Scenes(asset, config) => {
                if let Err(error) = self.analyze_scenes(&asset, config) {
                    self.scene_states
                        .update(asset.id, SceneStatus::Failed(error.to_string()));
                }
            }
        }
        true
    }

    fn analyze_silences(
        &mut self,
        asset: &MediaAsset,
        config: SilenceDetectionConfig,
    ) -> Result<(), MediaError> {
        self.silence_states.update(asset.id, SilenceStatus::Hashing);
        let hash = self.content_hash(&asset.path)?;
        let store = SilenceStore::new(self.root.join("silences"), config);
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            cached.asset = asset.id;
            self.silence_states
                .update(asset.id, SilenceStatus::Ready(Arc::new(cached)));
            return Ok(());
        }
        self.silence_states
            .update(asset.id, SilenceStatus::Analyzing);
        let samples = decode_audio_range(
            &asset.path,
            asset.fps,
            TimeCode::ZERO,
            asset.duration,
            ANALYSIS_SAMPLE_RATE,
            1,
            &ExportCancellation::default(),
        )?;
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
        store.save(&result)?;
        self.silence_states
            .update(asset.id, SilenceStatus::Ready(Arc::new(result)));
        Ok(())
    }

    fn analyze_scenes(
        &mut self,
        asset: &MediaAsset,
        config: SceneDetectionConfig,
    ) -> Result<(), MediaError> {
        self.scene_states.update(asset.id, SceneStatus::Hashing);
        let hash = self.content_hash(&asset.path)?;
        let store = SceneStore::new(self.root.join("scenes"), config);
        if let Some(mut cached) = store.load(&hash, asset.fps, asset.duration)? {
            cached.asset = asset.id;
            self.scene_states
                .update(asset.id, SceneStatus::Ready(Arc::new(cached)));
            return Ok(());
        }
        self.scene_states.update(asset.id, SceneStatus::Analyzing);
        let changes = detect_scene_changes(&asset.path, asset.fps, asset.duration, config)?;
        let result = AssetSceneChanges {
            asset: asset.id,
            content_sha256: hash,
            source_fps: asset.fps,
            source_frames: asset.duration,
            proxy_width: config.proxy_width,
            changes,
        };
        store.save(&result)?;
        self.scene_states
            .update(asset.id, SceneStatus::Ready(Arc::new(result)));
        Ok(())
    }

    fn content_hash(&mut self, path: &Path) -> Result<String, MediaError> {
        self.hashes.get(path)
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

fn detect_scene_changes(
    path: &Path,
    fps: Rational,
    duration: TimeCode,
    config: SceneDetectionConfig,
) -> Result<Vec<SceneChange>, MediaError> {
    let proxy_width = config.proxy_width.clamp(32, 512);
    let mut decoder = VideoDecoder::open_scaled(path, fps, Some(proxy_width))?;
    let mut cache = FrameCache::new(usize::try_from(SCENE_WINDOW_FRAMES + 1).unwrap_or(33));
    let mut previous_pixels: Option<Arc<Vec<u8>>> = None;
    let mut previous_difference: Option<f64> = None;
    let mut changes = Vec::new();
    let mut start = 0_i64;
    while start < duration.0 {
        let end = start
            .saturating_add(SCENE_WINDOW_FRAMES - 1)
            .min(duration.0.saturating_sub(1));
        if start == 0 {
            decoder.decode_window(TimeCode(start), TimeCode(end), &mut cache)?;
        } else {
            decoder.decode_window_sequential(TimeCode(start), TimeCode(end), &mut cache)?;
        }
        for frame_index in start..=end {
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
        .chunks_exact(4)
        .zip(current.chunks_exact(4))
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
    F: FnMut(AssetId) -> Option<Arc<AssetSilences>>,
{
    let requested = validated_range(document, range, "timeline silence")?;
    let minimum = minimum_source_frames.0.max(1);
    let mut analyses = BTreeMap::new();
    let mut mapped = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if matches!(clip.content, ClipContent::Title(_)) {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let cached_silences = analyses
                .entry(asset.id)
                .or_insert_with(|| silences_for(asset.id));
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
    F: FnMut(AssetId) -> Option<Arc<AssetSceneChanges>>,
{
    let requested = validated_range(document, range, "timeline scene")?;
    let mut analyses = BTreeMap::new();
    let mut mapped = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if matches!(clip.content, ClipContent::Title(_)) {
                continue;
            }
            let Some(asset) = document.asset(clip.asset) else {
                continue;
            };
            let cached_scenes = analyses
                .entry(asset.id)
                .or_insert_with(|| scenes_for(asset.id));
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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use openreel_core::{Clip, ClipId, MediaAsset, Track, TrackId, TrackKind};

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
    fn silence_and_scene_caches_round_trip_and_ignore_corruption() {
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
        };
        let duration = map_source_range_to_project(
            TimeCode(10)..TimeCode(40),
            asset.fps,
            Rational::new(30, 1).unwrap(),
        )
        .unwrap();
        Document {
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
