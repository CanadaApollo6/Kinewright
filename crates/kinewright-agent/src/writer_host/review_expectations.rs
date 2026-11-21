//! Derive expectations from a frozen document and its admitted source evidence.
//! Run derivation outside the writer lock.
mod overlays;
mod timing;
mod transcripts;

pub use transcripts::{SourceTranscripts, load_source_transcripts};

use kinewright_core::dialogue::Word;
use kinewright_core::output_review::{
    AbsenceProof, MAX_REVIEW_CLIPS, MAX_REVIEW_DERIVATION_WORK, MAX_REVIEW_PAUSES,
    MAX_REVIEW_TRACKS, ReviewAnnotations, ReviewCutIdentity, ReviewExpectations, ReviewInventory,
    ReviewSnapshotId, TranscriptIdentity,
};
use kinewright_core::{
    AudibleClockError, Clip, ClipId, Document, ExactMicros, ExportSettings, FrameRounding,
    MediaAsset, MediaError, OpError, Rational, TimeCode, TimeMappingError,
    map_frames_with_rounding,
};
use kinewright_project::operation_history::SnapshotIdentity;
use kinewright_project::owned_inputs::{OwnedInputEntry, OwnedInputManifest};
use serde::Serialize;
use std::{collections::BTreeMap, ops::Range};
use thiserror::Error;

const MAX_MANIFEST_ENTRIES: usize = 8_192;
const MAX_CODEC_NAME_BYTES: usize = 128;
const MAX_RASTER_EDGE: u32 = 16_384;
const MAX_TRANSFORM_EFFECTS: usize = 128;

pub struct ReviewExpectationInput<'a> {
    /// These must belong to the same frozen writer-host context.
    pub document: &'a Document,
    pub snapshot: SnapshotIdentity,
    pub input_manifest: &'a OwnedInputManifest,
    pub settings: &'a ExportSettings,
    pub transcripts: &'a [ReviewTranscript<'a>],
}

pub struct ReviewTranscript<'a> {
    /// Revision of the source-clock evidence selected by the host.
    pub identity: TranscriptIdentity,
    pub words: &'a [Word],
    /// Independently established processed range on this revision's source clock.
    /// Word timestamps alone do not establish recognition coverage.
    pub recognized_source: Option<kinewright_core::output_review::ReviewSpan>,
    /// Independently measured source events, on the recording's video and
    /// audio clocks relative to their pinned stream origins.
    pub landmarks: &'a [SourceAvLandmark],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceAvLandmark {
    pub id: u32,
    pub audio_at: ExactMicros,
    pub video_at: ExactMicros,
    /// Bound on the combined source measurement error of the two positions.
    pub uncertainty: ExactMicros,
}

#[derive(Debug, Error)]
pub enum ReviewExpectationError {
    #[error("{field} exceeds its limit of {maximum}")]
    Bound { field: &'static str, maximum: usize },
    #[error("review annotations belong to a different snapshot")]
    AnnotationSnapshot,
    #[error("review annotation names missing overlay {0}")]
    AnnotationElement(ClipId),
    #[error("text card annotation requires a plain title clip, got {0}")]
    AnnotationTextCard(ClipId),
    #[error("intended pause must be nonempty and contained in the project")]
    AnnotationPause,
    #[error("the export raster must have positive dimensions within the review limit")]
    Raster,
    #[error("transcript for asset {0} does not match one admitted source and selected stream")]
    TranscriptBinding(kinewright_core::AssetId),
    #[error("source evidence for asset {0} has an invalid range, word or landmark")]
    TranscriptTiming(kinewright_core::AssetId),
    #[error(transparent)]
    Document(#[from] OpError),
    #[error(transparent)]
    Clock(#[from] AudibleClockError),
    #[error(transparent)]
    FrameMapping(#[from] TimeMappingError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

/// Derive the record without reading files, decoding, recognising, or retaining it.
///
/// # Errors
/// Refuses invalid document or time geometry, mismatched annotations or source
/// evidence, and inputs or results above the review limits.
pub fn derive_review_expectations(
    input: &ReviewExpectationInput<'_>,
    annotations: &ReviewAnnotations,
) -> Result<ReviewExpectations, ReviewExpectationError> {
    let snapshot = ReviewSnapshotId(input.snapshot.digest());
    if annotations.snapshot != snapshot {
        return Err(ReviewExpectationError::AnnotationSnapshot);
    }
    let mut work = validate_limits(input, annotations)?;
    let document = input.document;
    document.validate()?;
    let settings = input.settings;
    let duration = document.duration;
    let output_frames =
        map_frames_with_rounding(duration, document.fps, settings.fps, FrameRounding::Ceil)?;
    let evidence = timing::derive(input, &mut work)?;
    let overlays = overlays::derive(input, annotations, &mut work)?;
    let intended_pauses = intended_pauses(document, annotations)?;
    Ok(ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot,
            input_manifest_sha256: digest(input.input_manifest)?,
            export_settings_sha256: digest(settings)?,
        },
        project_fps: document.fps,
        output_fps: settings.fps,
        raster: settings.resolution,
        output_frames,
        transcripts: evidence.transcripts,
        segments: evidence.segments,
        kept_words: evidence.kept_words,
        speech_windows: evidence.speech_windows,
        uncovered_speech_segments: evidence.uncovered_speech_segments,
        intended_pauses,
        av_landmarks: evidence.av_landmarks,
        overlays,
    })
}

fn validate_limits(
    input: &ReviewExpectationInput<'_>,
    annotations: &ReviewAnnotations,
) -> Result<DerivationWork, ReviewExpectationError> {
    use kinewright_core::output_review::{
        MAX_REVIEW_ELEMENT_TEXT_BYTES, MAX_REVIEW_ELEMENTS, MAX_REVIEW_TRANSCRIPTS,
    };
    bound("tracks", input.document.tracks.len(), MAX_REVIEW_TRACKS)?;
    bound(
        "input_manifest_entries",
        input.input_manifest.entries.len(),
        MAX_MANIFEST_ENTRIES,
    )?;
    bound(
        "transcripts",
        input.transcripts.len(),
        MAX_REVIEW_TRANSCRIPTS,
    )?;
    bound(
        "sourced_elements",
        annotations.sourced_elements.len(),
        MAX_REVIEW_ELEMENTS,
    )?;
    bound(
        "text_cards",
        annotations.text_cards.len(),
        MAX_REVIEW_ELEMENTS,
    )?;
    bound(
        "intended_pauses",
        annotations.intended_pauses.len(),
        MAX_REVIEW_PAUSES,
    )?;
    let settings = input.settings;
    bound(
        "video_codec_bytes",
        settings.video_codec.len(),
        MAX_CODEC_NAME_BYTES,
    )?;
    bound(
        "audio_codec_bytes",
        settings.audio_codec.len(),
        MAX_CODEC_NAME_BYTES,
    )?;
    let (width, height) = settings.resolution;
    if width == 0 || height == 0 || width > MAX_RASTER_EDGE || height > MAX_RASTER_EDGE {
        return Err(ReviewExpectationError::Raster);
    }

    let mut clips = 0;
    let mut elements = 0;
    let mut work = DerivationWork::default();
    for track in &input.document.tracks {
        clips += track.clips.len();
        bound("clips", clips, MAX_REVIEW_CLIPS)?;
        for clip in &track.clips {
            if overlays::element_kind(input.document, clip).is_some() {
                elements += 1;
                bound("elements", elements, MAX_REVIEW_ELEMENTS)?;
                bound("overlay_effects", clip.effects.len(), MAX_TRANSFORM_EFFECTS)?;
            }
            if let kinewright_core::ClipContent::Title(title) = &clip.content {
                bound(
                    "element_text_bytes",
                    title.text.len(),
                    MAX_REVIEW_ELEMENT_TEXT_BYTES,
                )?;
            }
            work.register(input.document, clip)?;
        }
    }
    Ok(work)
}

fn bound(field: &'static str, actual: usize, maximum: usize) -> Result<(), ReviewExpectationError> {
    if actual > maximum {
        return Err(ReviewExpectationError::Bound { field, maximum });
    }
    Ok(())
}

#[derive(Default)]
struct DerivationWork {
    used: usize,
    frame_costs: BTreeMap<ClipId, usize>,
}

impl DerivationWork {
    fn charge(&mut self, units: usize) -> Result<(), ReviewExpectationError> {
        self.used = self.used.saturating_add(units);
        bound("derivation_work", self.used, MAX_REVIEW_DERIVATION_WORK)
    }

    fn frame_cost(&self, clip: &Clip) -> usize {
        self.frame_costs[&clip.id]
    }

    fn register(&mut self, document: &Document, clip: &Clip) -> Result<(), ReviewExpectationError> {
        let enable_cost = 1 + clip
            .enabled_curve
            .as_ref()
            .map_or(0, |curve| curve.keyframes.len());
        let mut units = enable_cost;
        for effect in &clip.effects {
            let descriptor = kinewright_core::effect_descriptor(&effect.name);
            let keys = effect.keyframes.values().map(|curve| curve.keyframes.len());
            let enable_keys = effect
                .enabled_curve
                .as_ref()
                .map_or(0, |curve| curve.keyframes.len());
            let fixed = 1 + effect.parameters.len() + descriptor.map_or(0, |d| d.parameters.len());
            units = keys
                .chain([units, fixed, enable_keys])
                .fold(0, usize::saturating_add);
            bound("derivation_work", units, MAX_REVIEW_DERIVATION_WORK)?;
        }
        self.charge(units)?;
        if clip.enabled_curve.is_some() {
            // Audio and video may each scan the clip's project clock.
            let frames = usize::try_from(document.clip_duration(clip)?.0).unwrap_or(usize::MAX);
            self.charge(frames.saturating_mul(enable_cost).saturating_mul(2))?;
        }
        self.frame_costs.insert(clip.id, units);
        Ok(())
    }
}

fn inventory<T>(values: Vec<T>, complete: bool, empty: AbsenceProof) -> ReviewInventory<T> {
    if !complete {
        return ReviewInventory::NotDetermined { known: values };
    }
    let mut values = values.into_iter();
    match values.next() {
        Some(first) => ReviewInventory::Present {
            first,
            rest: values.collect(),
        },
        None => ReviewInventory::NoneProven(empty),
    }
}

fn clip_end(document: &Document, clip: &Clip) -> Result<TimeCode, ReviewExpectationError> {
    clip.timeline_start
        .checked_add(document.clip_duration(clip)?)
        .ok_or(OpError::TimeOverflow.into())
}

fn output_frames(
    frames: Range<TimeCode>,
    project: Rational,
    output: Rational,
) -> Result<Range<TimeCode>, TimeMappingError> {
    let to_output = |at| map_frames_with_rounding(at, project, output, FrameRounding::Ceil);
    Ok(to_output(frames.start)?..to_output(frames.end)?)
}

fn times(frames: Range<TimeCode>, rate: Rational) -> Result<Range<ExactMicros>, AudibleClockError> {
    Ok(ExactMicros::frames(frames.start, rate)?..ExactMicros::frames(frames.end, rate)?)
}

fn intended_pauses(
    document: &Document,
    annotations: &ReviewAnnotations,
) -> Result<ReviewInventory<Range<ExactMicros>>, ReviewExpectationError> {
    let mut pauses = Vec::with_capacity(annotations.intended_pauses.len());
    // The audio mix shares the project's time origin. Pause edges are author
    // times, not rounded to video frames or inferred from silence.
    for pause in &annotations.intended_pauses {
        if pause.start < TimeCode::ZERO || pause.start >= pause.end || pause.end > document.duration
        {
            return Err(ReviewExpectationError::AnnotationPause);
        }
        pauses.push(times(pause.clone(), document.fps)?);
    }
    Ok(inventory(
        union_windows(pauses),
        true,
        AbsenceProof::NoAuthorPauseDeclarations,
    ))
}

fn union_windows(mut windows: Vec<Range<ExactMicros>>) -> Vec<Range<ExactMicros>> {
    windows.sort_unstable_by_key(|window| (window.start, window.end));
    let mut merged: Vec<Range<ExactMicros>> = Vec::new();
    for window in windows {
        if let Some(last) = merged.last_mut().filter(|last| window.start <= last.end) {
            last.end = last.end.max(window.end);
        } else {
            merged.push(window);
        }
    }
    merged
}

fn pin_for<'a>(
    manifest: &'a OwnedInputManifest,
    asset: &MediaAsset,
) -> Option<&'a OwnedInputEntry> {
    let mut matches = manifest
        .entries
        .iter()
        .filter(|entry| entry.matches_media_asset(asset));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn parse_digest(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

fn digest(data: &impl Serialize) -> Result<[u8; 32], serde_json::Error> {
    struct HashWriter(kinewright_hash::Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(kinewright_hash::Sha256::new());
    serde_json::to_writer(&mut writer, data)?;
    Ok(writer.0.finalize())
}

#[cfg(test)]
mod tests;
