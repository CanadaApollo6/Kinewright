//! Frozen inputs to output review. Time intervals are half-open.
mod decisions;
pub use decisions::*;

use crate::{AssetId, ClipId, ExactMicros, Rational, TimeCode, TitlePixelBounds, TrackId};
use serde::{Deserialize, Serialize};
use std::ops::Range;

pub const MAX_REVIEW_TRACKS: usize = 256;
pub const MAX_REVIEW_CLIPS: usize = 8_192;
pub const MAX_REVIEW_ELEMENTS: usize = 1_024;
pub const MAX_REVIEW_SEGMENTS: usize = 16_384;
pub const MAX_REVIEW_TRANSCRIPTS: usize = 256;
pub const MAX_REVIEW_WORDS: usize = 65_536;
pub const MAX_REVIEW_WORD_BYTES: usize = 256;
pub const MAX_REVIEW_ELEMENT_TEXT_BYTES: usize = 2_048;
pub const MAX_REVIEW_BOUNDS_RUNS: usize = 65_536;
pub const MAX_REVIEW_PAUSES: usize = 1_024;
pub const MAX_REVIEW_LANDMARKS: usize = 4_096;
/// Aggregate units for timeline and overlay derivation.
pub const MAX_REVIEW_DERIVATION_WORK: usize = 2_000_000;

pub type ReviewSpan = Range<ExactMicros>;

/// The complete selected snapshot identity, including its owner state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewSnapshotId(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCutIdentity {
    pub snapshot: ReviewSnapshotId,
    pub input_manifest_sha256: [u8; 32],
    pub export_settings_sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewAnnotations {
    pub snapshot: ReviewSnapshotId,
    /// Clip identities of authored captions, titles or cards.
    pub sourced_elements: Vec<ClipId>,
    /// Plain title clips whose authored text is presented as a card.
    pub text_cards: Vec<ClipId>,
    /// Author-declared holds in project frames. Ordinary clip gaps carry no such declaration.
    pub intended_pauses: Vec<Range<TimeCode>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptIdentity {
    pub asset: AssetId,
    pub source_sha256: [u8; 32],
    pub source_byte_length: u64,
    pub audio_stream: u32,
    pub revision_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReviewLane {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReviewSegmentId {
    pub clip: ClipId,
    pub lane: ReviewLane,
    /// Project start of the enabled run. A clip may contribute several runs.
    pub project_start: TimeCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentSampling {
    /// Audio placement uses the export mix's 48 kHz sample grid. Padding after
    /// the source ends is outside the mapped interval.
    Audio,
    /// Output frames sample the floored project frame. That frame's offset
    /// from the clip start selects the floored source frame at the effective rate,
    /// clamped to the trim's last frame. Source below is the continuous edit span.
    Video {
        clip_start: TimeCode,
        source_in: TimeCode,
        source_end: TimeCode,
        source_fps: Rational,
        effective_fps: Rational,
    },
    /// All output frames in the interval sample this one source frame.
    VideoHold {
        frame: TimeCode,
        source_fps: Rational,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSegment {
    pub id: ReviewSegmentId,
    pub track: TrackId,
    pub asset: AssetId,
    pub source: ReviewSpan,
    pub project: Range<TimeCode>,
    pub output: ReviewSpan,
    pub sampling: SegmentSampling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptWord {
    /// Original word ordinal within this transcript revision, including any
    /// non-speech tokens in the original ordinals.
    pub transcript: TranscriptIdentity,
    pub word_index: u32,
    /// The occurrence identity is the original word plus this placement.
    pub segment: ReviewSegmentId,
    pub text: String,
    pub output: ReviewSpan,
    /// Part of the recognizer interval lies outside the kept source segment.
    pub clipped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvLandmark {
    pub asset: AssetId,
    pub source_landmark: u32,
    pub audio_segment: ReviewSegmentId,
    pub video_segment: ReviewSegmentId,
    pub audio_at: ExactMicros,
    pub video_at: ExactMicros,
    /// Combined source measurement and export-clock uncertainty.
    pub uncertainty: ExactMicros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbsenceProof {
    NoAudibleSourceSegments,
    NoRecognizedSpeechInKeptRanges,
    NoDocumentElementsOnOutput,
    NoAuthorPauseDeclarations,
    NoPairedAudioVideo,
}

/// A resolved inventory is nonempty. A partial inventory cannot establish
/// absence or complete coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewInventory<T> {
    NoneProven(AbsenceProof),
    NotDetermined { known: Vec<T> },
    Present { first: T, rest: Vec<T> },
}

impl<T> ReviewInventory<T> {
    pub fn known(&self) -> impl Iterator<Item = &T> {
        let (first, rest) = match self {
            Self::NoneProven(_) => (None, &[][..]),
            Self::NotDetermined { known } => (None, known.as_slice()),
            Self::Present { first, rest } => (Some(first), rest.as_slice()),
        };
        first.into_iter().chain(rest)
    }

    #[must_use]
    pub const fn is_determined(&self) -> bool {
        !matches!(self, Self::NotDetermined { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayKind {
    Caption,
    Title,
    Card,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayUnknown {
    OpaqueCard,
    ImageNotPinned,
    TextLayoutUnavailable,
    GeometryUnavailable,
    UpperLayerGeometry,
    NotVisible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayText {
    Expected(String),
    Unknown(OverlayUnknown),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayPixels {
    RenderedText,
    PinnedImage { sha256: [u8; 32], byte_length: u64 },
    Unknown(OverlayUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayBoundsRun {
    pub output: ReviewSpan,
    /// A conservative half-open rectangle in the export raster.
    pub bounds: Result<TitlePixelBounds, OverlayUnknown>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOverlay {
    pub element: ClipId,
    pub kind: OverlayKind,
    /// Half-open authored cue, before visibility trims. Caption agreement uses this range.
    pub cue: ReviewSpan,
    /// Expected visible intervals with constant output-pixel bounds.
    /// An unresolved geometry run retains its interval and the refusal reason.
    pub visible: Vec<OverlayBoundsRun>,
    pub text: OverlayText,
    pub pixels: OverlayPixels,
    /// False means the author has not declared the element sourced, with the
    /// editorial audit row providing the other control.
    pub sourced: bool,
}

/// Author declarations about a clip with no visible output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffOutputAnnotation {
    pub element: ClipId,
    pub sourced: bool,
    pub text_card: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayExpectations {
    pub annotated_off_output: Vec<OffOutputAnnotation>,
    pub captions: ReviewInventory<ReviewOverlay>,
    pub titles: ReviewInventory<ReviewOverlay>,
    pub cards: ReviewInventory<ReviewOverlay>,
}

impl OverlayExpectations {
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        [&self.captions, &self.titles, &self.cards].into_iter().all(|inventory| {
            inventory.is_determined() && inventory.known().all(|element| {
                !element.visible.is_empty()
                    && match element.pixels {
                        OverlayPixels::RenderedText => matches!(&element.text, OverlayText::Expected(text) if !text.trim().is_empty()),
                        OverlayPixels::PinnedImage { .. } => true,
                        OverlayPixels::Unknown(_) => false,
                    }
                    && element.visible.iter().all(|run| run.bounds.is_ok())
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewExpectations {
    pub identity: ReviewCutIdentity,
    pub project_fps: Rational,
    pub output_fps: Rational,
    pub raster: (u32, u32),
    pub output_frames: TimeCode,
    pub transcripts: Vec<TranscriptIdentity>,
    pub segments: Vec<ReviewSegment>,
    pub kept_words: ReviewInventory<KeptWord>,
    /// Union of known kept-word intervals, with the same completeness as the words.
    pub speech_windows: ReviewInventory<ReviewSpan>,
    pub uncovered_speech_segments: Vec<ReviewSegmentId>,
    pub intended_pauses: ReviewInventory<ReviewSpan>,
    pub av_landmarks: ReviewInventory<AvLandmark>,
    pub overlays: OverlayExpectations,
}
