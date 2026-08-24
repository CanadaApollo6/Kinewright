use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AutomationCurve, ColorContext, ColorDescription, OpError, Rational, TimeCode, Title,
    map_source_range_to_project,
};

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
id_type!(BinId);
id_type!(StringOutId);
id_type!(SyncGroupId);
id_type!(AudioBusId);

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

/// A content identity captured for one source file.
///
/// A fingerprint is either completely unknown (both fields are `None`) or a
/// verified pair containing the canonical lowercase SHA-256 and the source
/// byte length.  Validation lives at the operation/document boundary so this
/// pure model remains usable while a media worker is still hashing a file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaSourceFingerprint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub content_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub byte_len: Option<u64>,
}

impl MediaSourceFingerprint {
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            content_sha256: None,
            byte_len: None,
        }
    }

    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        self.content_sha256.is_none() && self.byte_len.is_none()
    }

    /// Return whether both identity components are present. Callers that need
    /// to trust the values must still validate their hash spelling and length.
    #[must_use]
    pub const fn is_verified(&self) -> bool {
        self.content_sha256.is_some() && self.byte_len.is_some()
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
    /// Content identity from a completed source hash. Missing in pre-M41
    /// project files and therefore defaults to an explicit unknown identity.
    #[serde(default, skip_serializing_if = "MediaSourceFingerprint::is_unknown")]
    #[schemars(default)]
    pub source_fingerprint: MediaSourceFingerprint,
    /// Source colour metadata from probing or an explicit user override.
    ///
    /// Missing in pre-CC0 project files and therefore defaults to an explicit
    /// unknown description. Unknown must not be treated as Rec.709 by
    /// consumers without an explicit decision.
    #[serde(default)]
    #[schemars(default)]
    pub color_description: ColorDescription,
}

/// Probed replacement metadata supplied to the filesystem-owning media layer
/// before a relink enters the pure Core operation path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelinkCandidate {
    pub path: PathBuf,
    pub fingerprint: MediaSourceFingerprint,
    pub kind: MediaKind,
    pub fps: Rational,
    pub duration: TimeCode,
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
    /// Clip-local automation keyed by registered parameter name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(default)]
    pub keyframes: BTreeMap<String, AutomationCurve>,
}

impl Effect {
    /// Resolve a parameter at one clip-local frame, falling back to its static value.
    #[must_use]
    pub fn integer_parameter_at(&self, name: &str, at: TimeCode) -> Option<i64> {
        self.keyframes
            .get(name)
            .and_then(|curve| curve.value_at(at))
            .or_else(|| match self.parameters.get(name) {
                Some(ParamValue::Integer(value)) => Some(*value),
                Some(ParamValue::Boolean(_) | ParamValue::Text(_)) | None => None,
            })
    }

    /// Produce an ephemeral static effect for one rendered frame.
    #[must_use]
    pub fn evaluated_at(&self, at: TimeCode) -> Self {
        let mut evaluated = self.clone();
        for (name, curve) in &self.keyframes {
            if let Some(value) = curve.value_at(at) {
                evaluated
                    .parameters
                    .insert(name.clone(), ParamValue::Integer(value));
            }
        }
        evaluated.keyframes.clear();
        evaluated
    }
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
    /// Constant playback speed for a media clip as an integer percentage.
    /// The validated range is 10..=1000; 100 is real time. Speed scales the
    /// clip's effective source frame rate, so 50 doubles the project duration
    /// (slow motion) and 200 halves it. Title and freeze clips are always 100.
    /// Audio for clips at any speed other than 100 is muted.
    #[serde(
        default = "default_clip_speed",
        skip_serializing_if = "speed_is_real_time"
    )]
    #[schemars(default)]
    pub speed_percent: u32,
}

const fn default_clip_speed() -> u32 {
    100
}

/// Return the frame rate at which a clip consumes its source, honoring its
/// playback speed. Every source-to-project mapping for a clip must go through
/// this — never scale fps at a call site.
///
/// # Errors
///
/// Returns [`crate::TimeMappingError`] when the scaled rate is invalid.
pub fn clip_effective_fps(
    asset_fps: crate::Rational,
    clip: &Clip,
) -> Result<crate::Rational, crate::TimeMappingError> {
    crate::speed_scaled_fps(asset_fps, clip.speed_percent)
}

// Serde's `skip_serializing_if` callbacks receive references to the fields.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn speed_is_real_time(speed_percent: &u32) -> bool {
    *speed_percent == 100
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

/// One hierarchical media-pool bin. An asset belongs to at most one bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaBin {
    pub id: BinId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub parent: Option<BinId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub assets: Vec<AssetId>,
}

/// One labeled source range inside an ordered string-out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceSelect {
    pub asset: AssetId,
    pub source: std::ops::Range<TimeCode>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schemars(default)]
    pub label: String,
}

/// Ordered source selects used to review, compare, and build a rough cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StringOut {
    pub id: StringOutId,
    pub name: String,
    pub selects: Vec<SourceSelect>,
}

/// One synchronized source angle. `offset` is relative to the sync group's
/// zero point and may be negative when this source starts early.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncGroupMember {
    pub asset: AssetId,
    pub offset: TimeCode,
    pub angle_name: String,
}

/// Reusable synchronization metadata and the stable foundation for multicam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncGroup {
    pub id: SyncGroupId,
    pub name: String,
    pub members: Vec<SyncGroupMember>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaCatalog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub bins: Vec<MediaBin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub string_outs: Vec<StringOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub sync_groups: Vec<SyncGroup>,
}

/// One deterministic mix bus. Tracks may route to at most one bus. Bus effects
/// use the registered `audio_*` descriptors, and ducking sidechains reference
/// the pre-bus signal from the listed tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioBus {
    pub id: AudioBusId,
    pub name: String,
    pub tracks: Vec<TrackId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub ducking_sidechain_tracks: Vec<TrackId>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioMix {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub buses: Vec<AudioBus>,
}

impl AudioMix {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buses.is_empty()
    }
}

impl MediaCatalog {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bins.is_empty() && self.string_outs.is_empty() && self.sync_groups.is_empty()
    }
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
    /// Branchable, undoable media organization and multicam foundation.
    #[serde(default, skip_serializing_if = "MediaCatalog::is_empty")]
    #[schemars(default)]
    pub catalog: MediaCatalog,
    /// Branchable, undoable audio routing and processing graph.
    #[serde(default, skip_serializing_if = "AudioMix::is_empty")]
    #[schemars(default)]
    pub audio_mix: AudioMix,
    /// Distinct project working, monitoring, and delivery colour descriptions.
    /// Missing in pre-CC0 files and defaulted to the current SDR Rec.709
    /// application context.
    #[serde(default)]
    #[schemars(default)]
    pub color_context: ColorContext,
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
            catalog: MediaCatalog::default(),
            audio_mix: AudioMix::default(),
            color_context: ColorContext::default(),
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
        let effective = clip_effective_fps(asset.fps, clip).map_err(OpError::TimeMapping)?;
        map_source_range_to_project(clip.source_range.clone(), effective, self.fps)
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
