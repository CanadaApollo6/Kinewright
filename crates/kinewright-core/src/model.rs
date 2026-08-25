use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

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
id_type!(LutAssetId);

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
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

#[derive(Debug, Deserialize)]
struct EffectWire {
    id: EffectId,
    name: String,
    parameters: BTreeMap<String, ParamValue>,
    #[serde(default)]
    keyframes: BTreeMap<String, AutomationCurve>,
}

impl<'de> Deserialize<'de> for Effect {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut wire = EffectWire::deserialize(deserializer)?;
        canonicalize_legacy_color_grade_name(&mut wire.name);
        Ok(Self {
            id: wire.id,
            name: wire.name,
            parameters: wire.parameters,
            keyframes: wire.keyframes,
        })
    }
}

impl Effect {
    /// Rewrite legacy names whose persisted meaning now has one canonical
    /// representation before the effect enters live project state.
    pub(crate) fn canonicalize_legacy_name(&mut self) {
        canonicalize_legacy_color_grade_name(&mut self.name);
    }

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

fn canonicalize_legacy_color_grade_name(name: &mut String) {
    if name == "color_grade" {
        "primary_correction".clone_into(name);
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

/// Largest LUT asset id a project may allocate: `2^53 - 1`, so the id survives
/// every JSON consumer — including the agent's — without precision loss
/// (CC4 §2.1).
pub const LUT_ASSET_ID_MAX: u64 = 9_007_199_254_740_991;

/// Fewest lattice samples per edge a 3D LUT asset may declare (CC4 §2.1).
pub const LUT_SIZE_MIN: u32 = 2;

/// Most lattice samples per edge a 3D LUT asset may declare (CC4 §2.1).
///
/// 65 is the most common vendor export grid and `65 - 1` is a power of two,
/// which is what makes the CC4 §3.5 exactness claim hold.
pub const LUT_SIZE_MAX: u32 = 65;

/// The interchange form of one project-owned LUT asset (CC4 §2.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum LutAssetKind {
    /// A 3D `.cube` lattice, the only kind CC4 evaluates.
    #[serde(rename = "cube_3d")]
    Cube3d,
    /// A 1D shaper. Reserved so a future shaper needs no schema migration;
    /// rejected on import and by [`validate_lut_asset`] in CC4 (CC4 §1).
    #[serde(rename = "cube_1d")]
    Cube1d,
}

impl LutAssetKind {
    /// Stable serialized/manifest token for the kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cube3d => "cube_3d",
            Self::Cube1d => "cube_1d",
        }
    }
}

/// Where a LUT asset's hashed bytes came from (CC4 §2.1).
///
/// `source_path` is informational only: it is never opened by the renderer and
/// never resolved relative to anything. The store file name is the content
/// hash, so no user-supplied string ever reaches a path component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LutAssetSource {
    /// Imported from a file the operator chose.
    Imported { source_path: String },
    /// Generated in the binary from a pinned bake (CC4 §2.6).
    Builtin { name: String },
}

/// One project-owned, content-hashed LUT asset record (CC4 §2.1).
///
/// **The hashed bytes are the authority, not the record.** `size` and the
/// domain mirrors exist so a human or agent can read a project without
/// touching the store; a renderer must use the values parsed from the verified
/// bytes and must report `lut_asset_metadata_mismatch` when they disagree.
///
/// Availability (`verified` / `missing` / `changed` / `unreadable`) is runtime
/// state and is deliberately absent here: it is never serialized (CC4 §2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LutAsset {
    /// Stable project-local identity, allocated as `max(existing) + 1`.
    pub id: LutAssetId,
    /// The content identity of the LUT file bytes: 64 lowercase hex chars.
    pub sha256: String,
    /// The `.cube` `TITLE` when present, otherwise the file stem.
    /// Informational; never an identity.
    pub title: String,
    /// The interchange form. CC4 accepts [`LutAssetKind::Cube3d`] only.
    pub kind: LutAssetKind,
    /// Lattice edge length `S`, in [`LUT_SIZE_MIN`]`..=`[`LUT_SIZE_MAX`].
    pub size: u32,
    /// Diagnostic and cheap preflight, never proof by itself (M41's rule).
    pub byte_len: u64,
    /// Informational mirror of `DOMAIN_MIN`, rounded half away from zero.
    pub domain_min_millionths: [i64; 3],
    /// Informational mirror of `DOMAIN_MAX`, rounded half away from zero.
    pub domain_max_millionths: [i64; 3],
    /// Provenance.
    pub source: LutAssetSource,
}

/// Per-channel field names used by [`validate_lut_asset`]'s domain rejections.
const LUT_DOMAIN_FIELDS: [&str; 3] = [
    "domain_r_millionths",
    "domain_g_millionths",
    "domain_b_millionths",
];

/// Whether a string is exactly 64 lowercase hexadecimal characters.
///
/// The same spelling M41 requires of [`MediaSourceFingerprint::content_sha256`].
fn is_canonical_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Validate one [`LutAsset`] record against the CC4 §2.1 table.
///
/// This is the single source of truth for `AddLutAsset` and for the document
/// invariant, so a record can never enter a project through one path that the
/// other would reject.
///
/// # Errors
///
/// Returns [`OpError::InvalidLutAssetHash`] for a malformed content hash and
/// [`OpError::InvalidLutAssetMetadata`] naming `field`, `observed`, and
/// `allowed` for every other rejection.
pub fn validate_lut_asset(asset: &LutAsset) -> Result<(), OpError> {
    if asset.id.0 == 0 || asset.id.0 > LUT_ASSET_ID_MAX {
        return Err(OpError::InvalidLutAssetMetadata {
            field: "id",
            observed: asset.id.0.to_string(),
            allowed: "1..=9007199254740991",
        });
    }
    if !is_canonical_sha256(&asset.sha256) {
        return Err(OpError::InvalidLutAssetHash {
            lut_asset: asset.id,
            observed: asset.sha256.clone(),
            allowed: "exactly 64 lowercase hexadecimal characters",
        });
    }
    if asset.title.is_empty() {
        return Err(OpError::InvalidLutAssetMetadata {
            field: "title",
            observed: String::new(),
            allowed: "a non-empty title",
        });
    }
    if !matches!(asset.kind, LutAssetKind::Cube3d) {
        return Err(OpError::InvalidLutAssetMetadata {
            field: "kind",
            observed: asset.kind.as_str().to_owned(),
            allowed: "cube_3d",
        });
    }
    if asset.size < LUT_SIZE_MIN || asset.size > LUT_SIZE_MAX {
        return Err(OpError::InvalidLutAssetMetadata {
            field: "size",
            observed: asset.size.to_string(),
            allowed: "2..=65",
        });
    }
    if asset.byte_len == 0 {
        return Err(OpError::InvalidLutAssetMetadata {
            field: "byte_len",
            observed: "0".to_owned(),
            allowed: "a positive byte length",
        });
    }
    let domains = asset
        .domain_min_millionths
        .into_iter()
        .zip(asset.domain_max_millionths)
        .zip(LUT_DOMAIN_FIELDS);
    for ((min, max), field) in domains {
        if min >= max {
            return Err(OpError::InvalidLutAssetMetadata {
                field,
                observed: format!("{min}..{max}"),
                allowed: "domain_min_millionths < domain_max_millionths",
            });
        }
    }
    Ok(())
}

/// Whether one effect is a CC4 LUT node bound to `id`, statically or through
/// any value in its `Hold` keyframe curve.
fn effect_references_lut_asset(effect: &Effect, id: LutAssetId) -> bool {
    if !crate::is_lut_color_node(&effect.name) {
        return false;
    }
    let referenced = |value: i64| u64::try_from(value).is_ok_and(|value| value == id.0);
    let static_reference = matches!(
        effect.parameters.get(crate::LUT_ASSET_ID_PARAMETER),
        Some(ParamValue::Integer(value)) if referenced(*value)
    );
    static_reference
        || effect
            .keyframes
            .get(crate::LUT_ASSET_ID_PARAMETER)
            .is_some_and(|curve| {
                curve
                    .keyframes
                    .iter()
                    .any(|keyframe| referenced(keyframe.value))
            })
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
    /// Project-owned, content-hashed LUT asset records (CC4 §2.1).
    ///
    /// Absent in every pre-CC4 project, so those projects load byte-unchanged
    /// and re-save without the field until a look is added. The samples
    /// themselves never enter the document: only the 64-character digest does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub lut_assets: Vec<LutAsset>,
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
            lut_assets: Vec::new(),
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

    /// Return the unique source assets that are actually referenced by clips
    /// on the timeline, in media-pool order.
    ///
    /// Title clips intentionally do not refer to a source asset. Both normal
    /// media clips and freeze clips do: a freeze is still decoded from its
    /// source and must therefore participate in source availability and
    /// export preflight checks. This distinction is central to keeping stale,
    /// unused media-bin entries from blocking a delivery.
    #[must_use]
    pub fn timeline_referenced_media_assets(&self) -> Vec<&MediaAsset> {
        let referenced_ids: BTreeSet<_> = self
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_)))
            .map(|clip| clip.asset)
            .collect();
        self.media_pool
            .iter()
            .filter(|asset| referenced_ids.contains(&asset.id))
            .collect()
    }

    #[must_use]
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .find(|clip| clip.id == id)
    }

    /// Look up one project-owned LUT asset record.
    #[must_use]
    pub fn lut_asset(&self, id: LutAssetId) -> Option<&LutAsset> {
        self.lut_assets.iter().find(|asset| asset.id == id)
    }

    /// Allocate the next LUT asset id as `max(existing) + 1` (CC4 §2.1).
    ///
    /// # Errors
    ///
    /// Returns [`OpError::LutAssetIdExhausted`] when the highest existing id
    /// already occupies [`LUT_ASSET_ID_MAX`].
    pub fn next_lut_asset_id(&self) -> Result<LutAssetId, OpError> {
        let highest = self
            .lut_assets
            .iter()
            .map(|asset| asset.id.0)
            .max()
            .unwrap_or(0);
        let next = highest.saturating_add(1);
        if next > LUT_ASSET_ID_MAX {
            return Err(OpError::LutAssetIdExhausted);
        }
        Ok(LutAssetId(next))
    }

    /// Every `(clip, effect)` pair whose LUT node references one asset, in
    /// document order (CC4 §2.7, §6).
    ///
    /// A reference counts whether the node is active, bypassed, or neutral,
    /// and whether the id is the node's stored static value or any value
    /// appearing in its `Hold` keyframe curve. `RemoveLutAsset` and
    /// availability reporting both read this, so a look can never be removed
    /// out from under a frame that still resolves to it.
    #[must_use]
    pub fn lut_asset_references(&self, id: LutAssetId) -> Vec<(ClipId, EffectId)> {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .flat_map(|clip| {
                clip.effects
                    .iter()
                    .filter(move |effect| effect_references_lut_asset(effect, id))
                    .map(move |effect| (clip.id, effect.id))
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        AutomationCurve, JournalCommand, Keyframe, KeyframeInterpolation, Operation, TrackKind,
    };

    fn effect(name: &str) -> Effect {
        Effect {
            id: EffectId(7),
            name: name.to_owned(),
            parameters: BTreeMap::from([
                ("exposure_milli_stops".to_owned(), ParamValue::Integer(750)),
                ("tint_percent".to_owned(), ParamValue::Integer(-12)),
                (
                    "label".to_owned(),
                    ParamValue::Text("preserve me".to_owned()),
                ),
            ]),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode(3),
                        value: 1_250,
                        interpolation: KeyframeInterpolation::EaseIn,
                    }],
                },
            )]),
        }
    }

    fn document_with_effects(effects: Vec<Effect>) -> Document {
        let mut document = Document::default();
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: AssetId(1),
                source_range: TimeCode::ZERO..TimeCode(10),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects,
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        document
    }

    #[test]
    fn legacy_color_grade_wire_name_migrates_without_losing_effect_data() {
        let original = effect("color_grade");
        let wire = serde_json::to_value(&original).expect("effect should serialize");
        assert_eq!(wire["name"], "color_grade");

        let decoded: Effect = serde_json::from_value(wire).expect("legacy effect should decode");

        assert_eq!(decoded.name, "primary_correction");
        assert_eq!(decoded.id, original.id);
        assert_eq!(decoded.parameters, original.parameters);
        assert_eq!(decoded.keyframes, original.keyframes);
        assert_eq!(
            serde_json::to_value(decoded).expect("migrated effect should serialize")["name"],
            "primary_correction"
        );
    }

    #[test]
    fn raw_in_memory_legacy_effect_keeps_its_wire_name_until_the_core_boundary() {
        let legacy = effect("color_grade");
        let canonical = effect("primary_correction");

        assert_eq!(
            serde_json::to_value(legacy).expect("legacy effect should serialize")["name"],
            "color_grade"
        );
        assert_eq!(
            serde_json::to_value(canonical).expect("canonical effect should serialize")["name"],
            "primary_correction"
        );
    }

    #[test]
    fn legacy_effect_migration_preserves_project_vector_position() {
        let original = document_with_effects(vec![
            effect("brightness"),
            effect("color_grade"),
            effect("saturation"),
        ]);
        let wire = serde_json::to_value(&original).expect("document should serialize");
        assert_eq!(
            wire["tracks"][0]["clips"][0]["effects"][1]["name"],
            "color_grade"
        );

        let decoded: Document = serde_json::from_value(wire).expect("document should decode");
        let effects = &decoded.tracks[0].clips[0].effects;
        assert_eq!(
            effects
                .iter()
                .map(|effect| effect.name.as_str())
                .collect::<Vec<_>>(),
            vec!["brightness", "primary_correction", "saturation"]
        );
        assert_eq!(
            effects[1].parameters,
            original.tracks[0].clips[0].effects[1].parameters
        );
        assert_eq!(
            effects[1].keyframes,
            original.tracks[0].clips[0].effects[1].keyframes
        );
    }

    #[test]
    fn legacy_effect_inside_journal_operation_migrates_on_decode() {
        let command = JournalCommand::Do(Operation::AddEffect {
            clip: ClipId(1),
            effect: effect("color_grade"),
        });
        let wire = serde_json::to_value(command).expect("journal command should serialize");
        let decoded: JournalCommand =
            serde_json::from_value(wire).expect("legacy journal operation should decode");

        let JournalCommand::Do(Operation::AddEffect { effect, .. }) = decoded else {
            panic!("expected an AddEffect journal command");
        };
        assert_eq!(effect.name, "primary_correction");
        assert_eq!(effect.id, EffectId(7));
    }

    #[test]
    fn unknown_effect_names_round_trip_without_renaming() {
        let mut wire =
            serde_json::to_value(effect("future_effect")).expect("future effect should serialize");
        wire["name"] = json!("future_effect_v2");

        let decoded: Effect = serde_json::from_value(wire).expect("future effect should decode");
        assert_eq!(decoded.name, "future_effect_v2");
        let round_trip = serde_json::to_value(decoded).expect("future effect should serialize");
        assert_eq!(round_trip["name"], "future_effect_v2");
    }
}
