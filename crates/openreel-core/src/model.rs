use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{OpError, Rational, TimeCode, Title, map_source_range_to_project};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug,
            Default,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

id_type!(AssetId);
id_type!(TrackId);
id_type!(ClipId);
id_type!(EffectId);
id_type!(LinkId);
id_type!(MarkerId);

/// Number of presentation-token choices available to project markers.
///
/// The stable index-to-token mapping is documented in `docs/DESIGN.md`.
pub const MARKER_COLOR_TOKEN_COUNT: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum MediaKind {
    Video,
    Audio,
    AudioVideo,
}

impl MediaKind {
    #[must_use]
    pub const fn supports(self, track: TrackKind) -> bool {
        matches!(
            (self, track),
            (Self::Video | Self::AudioVideo, TrackKind::Video)
                | (Self::Audio | Self::AudioVideo, TrackKind::Audio)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    pub name: String,
    /// Duration in source frames.
    pub duration: TimeCode,
    /// Exact source frame rate.
    pub fps: Rational,
    pub kind: MediaKind,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ParamValue {
    Integer(i64),
    Boolean(bool),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Effect {
    pub id: EffectId,
    /// A registered effect name from `EFFECT_DESCRIPTORS`.
    pub name: String,
    /// Integer-only fixed-point parameters. Their ranges and neutral defaults
    /// are defined by `EFFECT_DESCRIPTORS`.
    pub parameters: BTreeMap<String, ParamValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// A registered transition name from `TRANSITION_DESCRIPTORS`.
    /// Its compositor semantics are defined by the descriptor table.
    pub name: String,
    /// Transition length in project frames.
    pub duration: TimeCode,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipContent {
    #[default]
    Media,
    Title(Title),
    Freeze(FreezeFrame),
}

/// A project-local clip that repeatedly displays one frame from a real asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FreezeFrame {
    /// The held frame in the referenced asset's source-frame time base.
    pub source_frame: TimeCode,
}

impl ClipContent {
    #[must_use]
    pub const fn is_media(&self) -> bool {
        matches!(self, Self::Media)
    }

    #[must_use]
    pub const fn title(&self) -> Option<&Title> {
        match self {
            Self::Media | Self::Freeze(_) => None,
            Self::Title(title) => Some(title),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    pub id: ClipId,
    /// Media asset id. Title clips use the default id and ignore this field;
    /// freeze clips reference the real asset whose frame is held. Keeping the
    /// field on every clip preserves the pre-M14 serialized media-clip shape.
    pub asset: AssetId,
    /// Media in/out in source frames, or a title/freeze-local duration span in
    /// project frames.
    pub source_range: std::ops::Range<TimeCode>,
    /// Missing on pre-M14 clips, which are media by definition.
    #[serde(default, skip_serializing_if = "ClipContent::is_media")]
    #[schemars(default)]
    pub content: ClipContent,
    /// Position on the track, in project frames.
    pub timeline_start: TimeCode,
    pub effects: Vec<Effect>,
    pub transition_in: Option<Transition>,
    /// Optional A/V edit group. Core operations remain deliberately per-clip;
    /// UI and agent orchestration apply linked edits as atomic batches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub link: Option<LinkId>,
    /// Constant gain for this clip's audio contribution, in integer tenths of
    /// a decibel. The validated range is -600..=120 (-60.0 dB..=+12.0 dB).
    #[serde(default, skip_serializing_if = "i32_is_zero")]
    #[schemars(default)]
    pub audio_gain_tenth_db: i32,
    /// Linear fade-in length for this clip's audio contribution, in project frames.
    /// The value is non-negative and composes with any transition audio ramp.
    #[serde(default, skip_serializing_if = "time_code_is_zero")]
    #[schemars(default)]
    pub audio_fade_in_frames: TimeCode,
    /// Linear fade-out length for this clip's audio contribution, in project frames.
    /// The value is non-negative and the fade window anchors to the clip's project end.
    #[serde(default, skip_serializing_if = "time_code_is_zero")]
    #[schemars(default)]
    pub audio_fade_out_frames: TimeCode,
}

// Serde's `skip_serializing_if` callbacks receive references to the fields.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn i32_is_zero(value: &i32) -> bool {
    *value == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn time_code_is_zero(value: &TimeCode) -> bool {
    value.0 == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    /// Whether ripple edits initiated on other tracks shift this track to
    /// preserve cross-track synchronization. The edited track always ripples
    /// itself even when this flag is false. Defaults to true when omitted.
    #[serde(
        default = "default_track_sync_lock",
        skip_serializing_if = "track_sync_lock_is_default"
    )]
    #[schemars(extend("default" = true))]
    pub sync_lock: bool,
    pub clips: Vec<Clip>,
}

const fn default_track_sync_lock() -> bool {
    true
}

// Serde's `skip_serializing_if` callback receives a reference to the field.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn track_sync_lock_is_default(sync_lock: &bool) -> bool {
    *sync_lock
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Marker {
    pub id: MarkerId,
    /// Position on the project timeline, in project frames.
    pub position: TimeCode,
    pub label: String,
    /// Stable index into the marker token mapping in `docs/DESIGN.md`.
    pub color_token: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    /// Video and audio tracks, ordered z-bottom to top.
    pub tracks: Vec<Track>,
    pub media_pool: Vec<MediaAsset>,
    /// Project-level editorial notes. Missing in pre-M13 files and therefore
    /// defaulted for backward-compatible project loading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub markers: Vec<Marker>,
    pub fps: Rational,
    pub resolution: (u32, u32),
    pub duration: TimeCode,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            media_pool: Vec::new(),
            markers: Vec::new(),
            fps: Rational::default(),
            resolution: (1_920, 1_080),
            duration: TimeCode::ZERO,
        }
    }
}

impl Document {
    #[must_use]
    pub fn asset(&self, id: AssetId) -> Option<&MediaAsset> {
        self.media_pool.iter().find(|asset| asset.id == id)
    }

    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .find(|clip| clip.id == id)
    }

    #[must_use]
    pub fn marker(&self, id: MarkerId) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.id == id)
    }

    /// Validate every cross-reference and timeline invariant in the document.
    ///
    /// # Errors
    ///
    /// Returns the first violated document invariant.
    pub fn validate(&self) -> Result<(), OpError> {
        crate::operation::validate_document(self)
    }

    /// Return a clip's duration on the project frame grid.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing media asset or an unrepresentable frame-rate mapping.
    pub fn clip_duration(&self, clip: &Clip) -> Result<TimeCode, OpError> {
        if matches!(clip.content, ClipContent::Title(_) | ClipContent::Freeze(_)) {
            return clip
                .source_range
                .end
                .checked_sub(clip.source_range.start)
                .ok_or(OpError::TimeOverflow);
        }
        let asset = self
            .asset(clip.asset)
            .ok_or(OpError::MissingAsset(clip.asset))?;
        map_source_range_to_project(clip.source_range.clone(), asset.fps, self.fps)
            .map_err(OpError::TimeMapping)
    }

    pub(crate) fn clip_end(&self, clip: &Clip) -> Result<TimeCode, OpError> {
        clip.timeline_start
            .checked_add(self.clip_duration(clip)?)
            .ok_or(OpError::TimeOverflow)
    }

    pub(crate) fn recompute_duration(&mut self) -> Result<(), OpError> {
        let mut duration = TimeCode::ZERO;
        for clip in self.tracks.iter().flat_map(|track| &track.clips) {
            duration = duration.max(self.clip_end(clip)?);
        }
        self.duration = duration;
        Ok(())
    }
}
