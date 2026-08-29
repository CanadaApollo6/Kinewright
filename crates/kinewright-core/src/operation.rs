use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, AudioBus, AudioBusId, AutomationCurve, BinId, COLOR_CONFIDENCE_MAX_BASIS_POINTS,
    CaptionPreset, Clip, ClipContent, ClipId, ColorContext, ColorDescription, ColorProvenance,
    Document, Effect, EffectId, FreezeFrame, KeyframeInterpolation, LinkId, LutAsset, LutAssetId,
    MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset, MediaBin, MediaSourceFingerprint,
    ParamValue, RelinkCandidate, StringOut, StringOutId, SyncGroup, SyncGroupId, ThreePointMode,
    TimeCode, TimeMappingError, Title, TitleParameterKind, TitlePosition, Track, TrackId,
    TrackKind, Transition, is_audio_effect, map_source_range_to_project,
    title_parameter_descriptor,
};

// The project colour context is intentionally kept inline in the operation so
// the generated schema exposes the complete atomic reset payload directly.
// Its size is larger than most timeline edits, but boxing it would make the
// public operation shape less inspectable and needlessly complicate serde.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Operation {
    AddAsset {
        asset: MediaAsset,
    },
    /// Relink one asset to a probed, content-identified path without changing
    /// the asset's stable id, name, colour metadata, or any timeline reference.
    /// The media layer owns filesystem access; Core validates only the supplied
    /// candidate data and the persisted source contract.
    RelinkAsset {
        asset: AssetId,
        candidate: RelinkCandidate,
        /// Legacy assets may have no persisted source fingerprint. Re-linking
        /// those assets requires an explicit user decision because matching
        /// technical metadata does not prove byte identity.
        allow_unverified_source: bool,
    },
    /// Replace one asset's interpreted source colour metadata with an explicit
    /// user override. Probe/container metadata enters through `AddAsset`, not
    /// through this mutation path.
    SetAssetColorDescription {
        asset: AssetId,
        color_description: ColorDescription,
    },
    /// Replace the project working, monitoring, and delivery colour context.
    /// This is an ordinary atomic project edit so explicit resets remain
    /// journaled, revision-gated, and undoable rather than being hidden load
    /// side effects.
    SetColorContext {
        color_context: ColorContext,
    },
    UpsertBin {
        bin: MediaBin,
    },
    RemoveBin {
        bin: BinId,
    },
    /// Move an asset to one bin, or back to the unfiled media-pool root.
    SetAssetBin {
        asset: AssetId,
        bin: Option<BinId>,
    },
    UpsertStringOut {
        string_out: StringOut,
    },
    RemoveStringOut {
        string_out: StringOutId,
    },
    UpsertSyncGroup {
        sync_group: SyncGroup,
    },
    RemoveSyncGroup {
        sync_group: SyncGroupId,
    },
    UpsertAudioBus {
        bus: AudioBus,
    },
    RemoveAudioBus {
        bus: AudioBusId,
    },
    AddTrack {
        track: Track,
    },
    RemoveTrack {
        track: TrackId,
    },
    /// Enable or disable cross-track ripple participation for one track.
    SetTrackSyncLock {
        track: TrackId,
        locked: bool,
    },
    AddClip {
        track: TrackId,
        asset: AssetId,
        at: TimeCode,
        source: std::ops::Range<TimeCode>,
    },
    AddTitle {
        track: TrackId,
        at: TimeCode,
        duration: TimeCode,
        title: Title,
    },
    SplitClip {
        clip: ClipId,
        /// Split position in project frames.
        at: TimeCode,
    },
    TrimClip {
        clip: ClipId,
        new_source: std::ops::Range<TimeCode>,
    },
    MoveClip {
        clip: ClipId,
        to_track: TrackId,
        to: TimeCode,
    },
    /// Perform a source-monitor edit from exactly three marked boundaries.
    /// The missing fourth boundary is derived using exact frame-rate mapping.
    ThreePointEdit {
        track: TrackId,
        asset: AssetId,
        source_in: Option<TimeCode>,
        source_out: Option<TimeCode>,
        timeline_in: Option<TimeCode>,
        timeline_out: Option<TimeCode>,
        mode: ThreePointMode,
    },
    /// Perform one source-monitor edit with explicit video and/or audio
    /// source-patch destinations. The missing fourth three-point boundary is
    /// derived once and the resulting source/timeline span is applied to every
    /// selected route. Unlike [`ThreePointEdit`], this operation can create a
    /// linked A/V pair while preserving one atomic insert ripple.
    PatchedThreePointEdit {
        asset: AssetId,
        source_in: Option<TimeCode>,
        source_out: Option<TimeCode>,
        timeline_in: Option<TimeCode>,
        timeline_out: Option<TimeCode>,
        mode: ThreePointMode,
        /// Explicit destination for the asset's video component, when routed.
        video_track: Option<TrackId>,
        /// Explicit destination for the asset's audio component, when routed.
        audio_track: Option<TrackId>,
    },
    /// Change the media under a fixed timeline slot while preserving duration.
    SlipClip {
        clip: ClipId,
        new_source_in: TimeCode,
    },
    /// Move the shared boundary between two butt-joined media clips.
    RollEdit {
        left_clip: ClipId,
        right_clip: ClipId,
        to: TimeCode,
    },
    /// Move a media clip between two butt-joined neighbors while preserving
    /// the sequence's outer boundaries.
    SlideClip {
        clip: ClipId,
        to: TimeCode,
    },
    /// Replace a media clip's source while preserving its exact timeline slot.
    ReplaceClip {
        clip: ClipId,
        asset: AssetId,
        source: std::ops::Range<TimeCode>,
    },
    /// Replace and retime source media to fill a clip's exact timeline slot.
    FitToFill {
        clip: ClipId,
        asset: AssetId,
        source: std::ops::Range<TimeCode>,
    },
    DeleteClip {
        clip: ClipId,
    },
    /// Delete one clip and close its duration on that clip's track and every
    /// other sync-locked track.
    ///
    /// The ripple point is the deleted clip's pre-edit end. On participating
    /// tracks, only clips whose start is at or after that point shift left.
    /// A clip that starts before the point is not shifted or trimmed, even
    /// when it straddles the point. Normal validation atomically rejects any
    /// overlap caused by shifting a later clip beside that unchanged clip.
    /// Project markers at or after the ripple point shift left regardless of
    /// track sync locks. A shifted marker is clamped to frame zero.
    RippleDeleteClip {
        clip: ClipId,
    },
    /// Insert empty time by shifting clips that start at or after `at` on the
    /// target track and every other sync-locked track. Clips that start before
    /// `at` remain unchanged. Project markers at or after `at` shift right
    /// regardless of track sync locks.
    RippleInsertGap {
        track: TrackId,
        at: TimeCode,
        duration: TimeCode,
    },
    /// Assign one fresh link id to the listed clips.
    ///
    /// Link-follow enforcement intentionally does not live in core apply:
    /// core operations stay independently pure, while the UI and agent build
    /// one `DoBatch` containing the corresponding edits for every member.
    LinkClips {
        clips: Vec<ClipId>,
    },
    /// Clear the link id on each listed clip. This is per-clip data mutation;
    /// remaining members of a group are not rewritten implicitly.
    UnlinkClips {
        clips: Vec<ClipId>,
    },
    AddMarker {
        marker: Marker,
    },
    RemoveMarker {
        marker: MarkerId,
    },
    MoveMarker {
        marker: MarkerId,
        to: TimeCode,
    },
    AddEffect {
        clip: ClipId,
        effect: Effect,
    },
    /// Insert one effect at an explicit position in `clip.effects` (CC4 §2.7).
    ///
    /// The positional sibling of [`Operation::AddEffect`], which appends. A
    /// stage-ordered colour stack must be able to place a `technical_lut`
    /// before an existing correction node without deleting it, so the index is
    /// part of the operation rather than a follow-up reorder. Validation is
    /// identical to `AddEffect`'s plus a bounds check: `index` may equal the
    /// current length, which appends.
    InsertEffect {
        clip: ClipId,
        index: usize,
        effect: Effect,
    },
    RemoveEffect {
        clip: ClipId,
        effect: EffectId,
    },
    SetEffectParam {
        clip: ClipId,
        effect: EffectId,
        name: String,
        value: ParamValue,
    },
    /// Replace one effect parameter's complete clip-local automation curve.
    SetEffectKeyframes {
        clip: ClipId,
        effect: EffectId,
        name: String,
        curve: AutomationCurve,
    },
    ClearEffectKeyframes {
        clip: ClipId,
        effect: EffectId,
        name: String,
    },
    /// Replace one legacy `look_lut` / `cube_lut` at its exact vector position
    /// with an equivalent managed `creative_look` node (CC4 §9).
    ///
    /// The only path from legacy to managed, and deliberately explicit: the
    /// converted node is *not* bit-identical to the legacy stage, because the
    /// legacy path clamped to `[0, 1]` in display space, mixed intensity in
    /// the encoded domain, and decoded with the non-invertible `decode_bt709`.
    /// The caller resolves a preset token or an external `.cube` path to a
    /// registered asset beforehand, so the visible batch is
    /// `[AddLutAsset, ConvertLegacyLook]`.
    ConvertLegacyLook {
        clip: ClipId,
        effect: EffectId,
        /// The already-registered asset the managed node binds to.
        lut_asset: LutAssetId,
        /// Look strength in basis points, `0..=10000`. A legacy
        /// `intensity_percent` converts as `percent * 100`.
        mix_basis_points: i64,
    },
    /// Register one project-owned LUT asset record (CC4 §2.7).
    ///
    /// Metadata only: no LUT sample byte ever enters the document, the
    /// journal, a branch, or a recovery record. The media layer parses,
    /// hashes, and stores the file first and passes the verified record here.
    AddLutAsset {
        asset: LutAsset,
    },
    /// Remove one project-owned LUT asset record (CC4 §2.7).
    ///
    /// Rejected while any effect on any clip references it — including a
    /// bypassed node and a `Hold` keyframe value. It never cascades, and it
    /// never deletes the content-addressed store file.
    RemoveLutAsset {
        lut_asset: LutAssetId,
    },
    SetTitleParam {
        clip: ClipId,
        name: String,
        value: ParamValue,
    },
    /// Replace all constant audio shaping values for one media clip.
    SetClipAudio {
        clip: ClipId,
        /// Integer tenths of a decibel, in the inclusive range -600..=120.
        gain_tenth_db: i32,
        /// Fade-in length in project frames.
        fade_in_frames: TimeCode,
        /// Fade-out length in project frames, anchored to the clip end.
        fade_out_frames: TimeCode,
    },
    AddTransition {
        clip: ClipId,
        transition: Transition,
    },
    RemoveTransition {
        clip: ClipId,
    },
    SetMarkerParam {
        marker: MarkerId,
        name: String,
        value: ParamValue,
    },
    AddFreezeFrame {
        track: TrackId,
        at: TimeCode,
        duration: TimeCode,
        asset: AssetId,
        source_frame: TimeCode,
    },
    /// Set a media clip's constant playback speed as an integer percentage.
    SetClipSpeed {
        clip: ClipId,
        /// Inclusive range 10..=1000; 100 is real time. Changing speed changes
        /// the clip's project duration; the operation fails if the new
        /// duration would overlap a later clip. Audio is muted at any speed
        /// other than 100.
        speed_percent: u32,
    },
}

pub trait ApplyOp {
    /// Validate and apply to `doc`. This method performs no I/O.
    ///
    /// # Errors
    ///
    /// Returns an operation error without mutating `doc` when validation fails.
    fn apply(&self, doc: &mut Document) -> Result<(), OpError>;
}

impl Operation {
    /// Validate and atomically apply this operation to a document.
    ///
    /// # Errors
    ///
    /// Returns an operation error without mutating `doc` when validation fails.
    pub fn apply(&self, doc: &mut Document) -> Result<(), OpError> {
        <Self as ApplyOp>::apply(self, doc)
    }

    /// Canonicalize compatibility aliases before history or journal capture.
    pub(crate) fn canonicalize_legacy_effect_names(&mut self) {
        if let Self::AddEffect { effect, .. } | Self::InsertEffect { effect, .. } = self {
            effect.canonicalize_legacy_name();
        }
    }
}

/// Validate and apply an ordered edit plan without exposing partial state.
///
/// The caller's document is replaced only after every operation succeeds. Each
/// operation is applied to the result of the previous one, so generated ids and
/// later validations have the same semantics as sequential `Operation::apply`
/// calls.
///
/// # Errors
///
/// Returns the failing operation index and error, leaving `doc` unchanged.
pub fn apply_batch(doc: &mut Document, operations: &[Operation]) -> Result<(), BatchError> {
    if operations.is_empty() {
        return Err(BatchError::Empty);
    }
    let mut candidate = doc.clone();
    for (index, operation) in operations.iter().enumerate() {
        operation
            .apply(&mut candidate)
            .map_err(|error| BatchError::OperationFailed {
                op_number: index + 1,
                error,
            })?;
    }
    *doc = candidate;
    Ok(())
}

impl ApplyOp for Operation {
    fn apply(&self, doc: &mut Document) -> Result<(), OpError> {
        // Applying to a clone makes rejection atomic: the caller's document is
        // unchanged if either the operation or final invariant check fails.
        validate_document(doc)?;
        let mut candidate = doc.clone();
        apply_unchecked(self, &mut candidate)?;
        candidate.recompute_duration()?;
        validate_document(&candidate)?;
        *doc = candidate;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpError {
    #[error("asset {0} already exists")]
    DuplicateAsset(AssetId),
    #[error("bin {0} occurs more than once")]
    DuplicateBin(BinId),
    #[error("bin {0} does not exist")]
    MissingBin(BinId),
    #[error("bin {0} must have a non-empty name")]
    EmptyBinName(BinId),
    #[error("bin {0} cannot be its own parent")]
    BinSelfParent(BinId),
    #[error("bin hierarchy contains a cycle through bin {0}")]
    BinCycle(BinId),
    #[error("bin {0} has child bins and cannot be removed")]
    BinHasChildren(BinId),
    #[error("asset {asset} occurs more than once in bin {bin}")]
    DuplicateBinAsset { bin: BinId, asset: AssetId },
    #[error("asset {asset} belongs to both bin {first} and bin {second}")]
    AssetInMultipleBins {
        asset: AssetId,
        first: BinId,
        second: BinId,
    },
    #[error("string-out {0} occurs more than once")]
    DuplicateStringOut(StringOutId),
    #[error("string-out {0} does not exist")]
    MissingStringOut(StringOutId),
    #[error("string-out {0} must have a name and at least one source select")]
    InvalidStringOut(StringOutId),
    #[error("sync group {0} occurs more than once")]
    DuplicateSyncGroup(SyncGroupId),
    #[error("sync group {0} does not exist")]
    MissingSyncGroup(SyncGroupId),
    #[error("sync group {0} must have a name and at least two members")]
    InvalidSyncGroup(SyncGroupId),
    #[error("asset {asset} occurs more than once in sync group {group}")]
    DuplicateSyncGroupAsset { group: SyncGroupId, asset: AssetId },
    #[error("sync group {group} has an empty angle name for asset {asset}")]
    EmptySyncAngle { group: SyncGroupId, asset: AssetId },
    #[error("audio bus {0} occurs more than once")]
    DuplicateAudioBus(AudioBusId),
    #[error("audio bus {0} does not exist")]
    MissingAudioBus(AudioBusId),
    #[error("audio bus {0} must have a non-empty name and at least one routed track")]
    InvalidAudioBus(AudioBusId),
    #[error("track {track} is routed to both audio bus {first} and {second}")]
    TrackInMultipleAudioBuses {
        track: TrackId,
        first: AudioBusId,
        second: AudioBusId,
    },
    #[error("audio bus {bus} references missing track {track}")]
    AudioBusMissingTrack { bus: AudioBusId, track: TrackId },
    #[error("audio bus {bus} cannot use visual effect {effect:?}")]
    VisualEffectOnAudioBus { bus: AudioBusId, effect: String },
    #[error("audio effect {effect:?} must be placed on an audio bus, not clip {clip}")]
    AudioEffectOnClip { clip: ClipId, effect: String },
    #[error("audio bus {bus} has duplicate effect id {effect}")]
    DuplicateAudioBusEffect { bus: AudioBusId, effect: EffectId },
    #[error("audio bus {0} uses audio_ducking without any sidechain tracks")]
    AudioBusDuckingWithoutSidechain(AudioBusId),
    #[error(
        "automation keyframe {at} for audio bus {bus} effect {effect} parameter {name:?} is outside project range 0..{duration}"
    )]
    AudioBusKeyframeOutsideProject {
        bus: AudioBusId,
        effect: EffectId,
        name: String,
        at: TimeCode,
        duration: TimeCode,
    },
    #[error("track {0} occurs more than once")]
    DuplicateTrack(TrackId),
    #[error("new track {0} must be empty")]
    NewTrackNotEmpty(TrackId),
    #[error("clip {0} occurs more than once")]
    DuplicateClip(ClipId),
    #[error("asset {0} does not exist")]
    MissingAsset(AssetId),
    #[error("track {0} does not exist")]
    MissingTrack(TrackId),
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("asset {asset} cannot be placed on {track} track {track_id}")]
    IncompatibleTrack {
        asset: AssetId,
        track: &'static str,
        track_id: TrackId,
    },
    #[error("title clips can only be placed on video track {0}")]
    TitleOnAudioTrack(TrackId),
    #[error("freeze clips can only be placed on video track {0}")]
    FreezeOnAudioTrack(TrackId),
    #[error("source range must be non-empty and non-negative: {start}..{end}")]
    InvalidSourceRange { start: i64, end: i64 },
    #[error("source range ends at {end}, beyond asset {asset}'s duration {duration}")]
    SourceOutOfBounds {
        asset: AssetId,
        end: TimeCode,
        duration: TimeCode,
    },
    #[error("timeline positions must be non-negative: {0}")]
    NegativeTimelinePosition(TimeCode),
    #[error("clip {0} maps to zero project frames")]
    ZeroProjectDuration(ClipId),
    #[error("clip {clip} overlaps clip {with} on track {track}")]
    ClipOverlap {
        track: TrackId,
        clip: ClipId,
        with: ClipId,
    },
    #[error("clips on track {track} are not sorted: {previous} appears before {next}")]
    ClipsUnsorted {
        track: TrackId,
        previous: ClipId,
        next: ClipId,
    },
    #[error("split at project frame {at} is outside clip {clip}")]
    SplitOutsideClip { clip: ClipId, at: TimeCode },
    #[error("project frame {at} inside clip {clip} is not an integer source-frame boundary")]
    UnrepresentableSplit { clip: ClipId, at: TimeCode },
    #[error("editorial operation requires media clip {0}")]
    EditorialRequiresMedia(ClipId),
    #[error("clips {left} and {right} must be butt-joined neighbors on the same track")]
    ClipsNotAdjacent { left: ClipId, right: ClipId },
    #[error("clip {clip} must have butt-joined media neighbors on both sides")]
    SlideRequiresNeighbors { clip: ClipId },
    #[error("project frame {at} cannot be represented as a source boundary for clip {clip}")]
    UnrepresentableEditBoundary { clip: ClipId, at: TimeCode },
    #[error("exactly three of source_in, source_out, timeline_in, and timeline_out must be marked")]
    InvalidThreePointSelection,
    #[error("patched three-point edit must target at least one route")]
    EmptySourcePatch,
    #[error("patched three-point edit targets track {0} more than once")]
    DuplicateSourcePatchTrack(TrackId),
    #[error(
        "patched three-point {component} route requires a {expected:?} track, got {actual:?} track {track_id}"
    )]
    InvalidSourcePatchRouteKind {
        component: &'static str,
        expected: TrackKind,
        actual: TrackKind,
        track_id: TrackId,
    },
    #[error("three-point edit produced an invalid source range {start}..{end}")]
    InvalidThreePointSource { start: TimeCode, end: TimeCode },
    #[error("three-point edit produced an invalid timeline range {start}..{end}")]
    InvalidThreePointTimeline { start: TimeCode, end: TimeCode },
    #[error(
        "replacement source maps to {actual} project frames, but clip {clip} occupies {required}"
    )]
    ReplacementDurationMismatch {
        clip: ClipId,
        required: TimeCode,
        actual: TimeCode,
    },
    #[error(
        "source range cannot be fit exactly into clip {clip} with an integer speed from 10% through 1000%"
    )]
    FitToFillUnrepresentable { clip: ClipId },
    #[error("document frame rate is invalid")]
    InvalidProjectRate,
    #[error("asset {0} has an invalid frame rate")]
    InvalidAssetRate(AssetId),
    #[error("asset {0} has a non-positive duration")]
    InvalidAssetDuration(AssetId),
    #[error(
        "asset {asset} source fingerprint must include both SHA-256 and byte length, or neither"
    )]
    SourceFingerprintIncomplete { asset: AssetId },
    #[error(
        "asset {asset} source fingerprint SHA-256 must be exactly 64 lowercase hexadecimal characters"
    )]
    InvalidSourceFingerprintHash { asset: AssetId },
    #[error("asset {asset} source fingerprint byte length must be positive")]
    InvalidSourceFingerprintByteLength { asset: AssetId },
    #[error("relink candidate path for asset {asset} must be non-empty")]
    EmptyRelinkCandidatePath { asset: AssetId },
    #[error("relink candidate for asset {asset} must have a verified source fingerprint")]
    UnverifiedRelinkCandidate { asset: AssetId },
    #[error(
        "relink candidate for asset {asset} has incompatible {field}: expected {expected}, got {actual}"
    )]
    RelinkMetadataMismatch {
        asset: AssetId,
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("relink candidate for asset {asset} does not match its persisted source fingerprint")]
    RelinkFingerprintMismatch { asset: AssetId },
    #[error(
        "asset {asset} has no persisted source fingerprint; relink requires explicit allow_unverified_source"
    )]
    RelinkRequiresExplicitUnverifiedSource { asset: AssetId },
    #[error("color confidence is {actual}, outside the inclusive range 0..=10000 basis points")]
    ColorConfidenceOutOfRange { actual: u16 },
    /// CC8 §7 item 3: an HDR delivery description that is not §5.1's lane.
    ///
    /// It carries §5.3's own three facts — `field` / `observed` / `allowed` —
    /// plus the recovery action, so the refusal is data rather than one opaque
    /// sentence, exactly as `DeliveryColorError` is at the export gate. The two
    /// read the same allowed phrases from the authority module, so a document
    /// cannot be refused here for one reason and there for another.
    #[error(
        "unsupported CC8 HDR delivery description: field={field}, observed={observed}, allowed={allowed}. {recovery}"
    )]
    UnsupportedHdrDeliveryDescription {
        field: String,
        observed: String,
        allowed: String,
        recovery: &'static str,
    },
    #[error("asset {asset} color override must have positive confidence")]
    ZeroConfidenceColorOverride { asset: AssetId },
    #[error("asset {asset} color override requires user_override provenance, got {actual:?}")]
    InvalidColorOverrideProvenance {
        asset: AssetId,
        actual: ColorProvenance,
    },
    #[error("document resolution must be non-zero")]
    InvalidResolution,
    #[error("document duration {actual:?} does not match calculated duration {expected:?}")]
    IncorrectDocumentDuration {
        expected: TimeCode,
        actual: TimeCode,
    },
    #[error("clip id space is exhausted")]
    ClipIdExhausted,
    #[error("link id space is exhausted")]
    LinkIdExhausted,
    #[error("link operation requires at least {minimum} clip(s)")]
    TooFewLinkedClips { minimum: usize },
    #[error("clip {0} occurs more than once in the link operation")]
    DuplicateClipSelection(ClipId),
    #[error("marker {0} already exists")]
    DuplicateMarker(MarkerId),
    #[error("marker {0} does not exist")]
    MissingMarker(MarkerId),
    #[error("markers are not sorted: {previous} appears before {next}")]
    MarkersUnsorted { previous: MarkerId, next: MarkerId },
    #[error("marker positions must be non-negative: {0}")]
    NegativeMarkerPosition(TimeCode),
    #[error("marker color token {actual} is outside the supported range 0..{maximum_exclusive}")]
    InvalidMarkerColor { actual: u8, maximum_exclusive: u8 },
    #[error("ripple gap duration must be positive: {0}")]
    InvalidRippleDuration(TimeCode),
    #[error("title duration must be positive: {0}")]
    InvalidTitleDuration(TimeCode),
    #[error("freeze duration must be positive: {0}")]
    InvalidFreezeDuration(TimeCode),
    #[error("freeze source frame {source_frame} is outside asset {asset}'s range 0..{duration}")]
    FreezeSourceFrameOutOfRange {
        asset: AssetId,
        source_frame: TimeCode,
        duration: TimeCode,
    },
    #[error("clip {0} is not a title")]
    NotTitleClip(ClipId),
    #[error("unknown title parameter {0:?}")]
    UnknownTitleParam(String),
    #[error("title parameter {name:?} has the wrong value type")]
    InvalidTitleParamType { name: String },
    #[error("title parameter {name:?} is {actual}, outside the inclusive range {min}..={max}")]
    TitleParamOutOfRange {
        name: String,
        min: i64,
        max: i64,
        actual: i64,
    },
    #[error("title text exceeds the maximum length of {maximum} characters")]
    TitleTextTooLong { maximum: usize },
    #[error("title clip {clip} does not match its {preset:?} caption preset fields")]
    CaptionPresetMismatch { clip: ClipId, preset: CaptionPreset },
    #[error("title fade {name:?} ({frames} frames) exceeds clip {clip} duration {duration}")]
    TitleFadeTooLong {
        clip: ClipId,
        name: &'static str,
        frames: TimeCode,
        duration: TimeCode,
    },
    #[error("marker {marker} has no parameter {name:?}")]
    UnknownMarkerParam { marker: MarkerId, name: String },
    #[error("marker {marker} parameter {name:?} has the wrong value type")]
    InvalidMarkerParamType { marker: MarkerId, name: String },
    #[error("effect {effect} already exists on clip {clip}")]
    DuplicateEffect { clip: ClipId, effect: EffectId },
    #[error("effect {effect} does not exist on clip {clip}")]
    MissingEffect { clip: ClipId, effect: EffectId },
    #[error("unknown effect name {0:?}")]
    UnknownEffect(String),
    #[error("effect {effect:?} has no parameter {name:?}")]
    UnknownEffectParam { effect: String, name: String },
    #[error("effect {effect:?} parameter {name:?} requires an integer")]
    InvalidEffectParamType { effect: String, name: String },
    #[error("cube_lut requires a non-empty text path parameter")]
    MissingCubeLutPath,
    #[error(
        "effect {effect:?} parameter {name:?} is {actual}, outside the inclusive range {min}..={max}"
    )]
    EffectParamOutOfRange {
        effect: String,
        name: String,
        min: i64,
        max: i64,
        actual: i64,
    },
    #[error(
        "effect {effect:?} curve {curve:?} point {index} has x {x}, which is not greater than the previous point's x {previous_x}; x must be strictly increasing over the active prefix"
    )]
    InvalidCurvePoints {
        effect: String,
        curve: String,
        index: usize,
        previous_x: i64,
        x: i64,
    },
    #[error("effect {effect:?} parameter {name:?} accepts only hold keyframes")]
    NonHoldKeyframeParameter { effect: String, name: String },
    #[error(
        "effect {effect:?} curve {curve:?} cannot keyframe point coordinates while its point_count has more than one keyframe"
    )]
    CurvePointCountAnimatedWithPoints { effect: String, curve: String },
    #[error(
        "clip {clip} would carry {actual} managed colour nodes, exceeding the limit of {limit}"
    )]
    TooManyColorNodes {
        clip: ClipId,
        limit: usize,
        actual: usize,
    },
    #[error("clip {clip} would carry {actual} LUT nodes, exceeding the limit of {limit}")]
    TooManyLutNodes {
        clip: ClipId,
        limit: usize,
        actual: usize,
    },
    #[error("LUT asset {0} already exists")]
    DuplicateLutAsset(LutAssetId),
    #[error("LUT asset {lut_asset} SHA-256 {observed:?} must be {allowed}")]
    InvalidLutAssetHash {
        lut_asset: LutAssetId,
        observed: String,
        allowed: &'static str,
    },
    #[error("LUT asset {field} is {observed:?}, outside the allowed {allowed}")]
    InvalidLutAssetMetadata {
        field: &'static str,
        observed: String,
        allowed: &'static str,
    },
    #[error("LUT asset {lut_asset} is still referenced by effect {effect} on clip {clip}")]
    LutAssetInUse {
        lut_asset: LutAssetId,
        clip: ClipId,
        effect: EffectId,
    },
    #[error(
        "effect {effect} on clip {clip} references LUT asset {lut_asset}, which does not exist"
    )]
    MissingLutAsset {
        clip: ClipId,
        effect: EffectId,
        lut_asset: LutAssetId,
    },
    #[error("LUT asset id space is exhausted")]
    LutAssetIdExhausted,
    #[error("LUT asset {0} does not exist")]
    UnknownLutAsset(LutAssetId),
    #[error("effect index {index} is outside clip {clip}'s effect vector of length {len}")]
    EffectIndexOutOfRange {
        clip: ClipId,
        index: usize,
        len: usize,
    },
    #[error("effect {effect} on clip {clip} is {name:?}, not a legacy look_lut or cube_lut")]
    NotALegacyLook {
        clip: ClipId,
        effect: EffectId,
        name: String,
    },
    #[error(
        "colour node {effect} ({kind:?}, stage {color_stage_rank}) on clip {clip} is placed after node {previous_effect} ({previous_kind:?}, stage {previous_color_stage_rank}); managed nodes must have non-decreasing stage rank"
    )]
    ColorStageOrderViolation {
        clip: ClipId,
        effect: EffectId,
        kind: String,
        color_stage_rank: u8,
        previous_effect: EffectId,
        previous_kind: String,
        previous_color_stage_rank: u8,
    },
    #[error("effect {effect:?} parameter {name:?} has an invalid automation curve: {reason}")]
    InvalidEffectAutomation {
        effect: String,
        name: String,
        reason: String,
    },
    #[error(
        "automation keyframe {at} for effect {effect} parameter {name:?} is outside clip {clip}'s local range 0..{duration}"
    )]
    EffectKeyframeOutsideClip {
        clip: ClipId,
        effect: EffectId,
        name: String,
        at: TimeCode,
        duration: TimeCode,
    },
    #[error("clip {0} already has a transition_in")]
    DuplicateTransition(ClipId),
    #[error("clip {0} has no transition_in")]
    MissingTransition(ClipId),
    #[error("unknown transition name {0:?}")]
    UnknownTransition(String),
    #[error("transition duration on clip {clip} must be positive, got {duration}")]
    InvalidTransitionDuration { clip: ClipId, duration: TimeCode },
    #[error("transition duration {duration} exceeds clip {clip} duration {clip_duration}")]
    TransitionTooLong {
        clip: ClipId,
        duration: TimeCode,
        clip_duration: TimeCode,
    },
    #[error(
        "audio gain on clip {clip} is {gain_tenth_db} tenth-dB, outside the inclusive range -600..=120"
    )]
    AudioGainOutOfRange { clip: ClipId, gain_tenth_db: i32 },
    #[error("audio {name} on clip {clip} must be non-negative, got {frames} frames")]
    NegativeAudioFade {
        clip: ClipId,
        name: &'static str,
        frames: TimeCode,
    },
    #[error(
        "audio fades on clip {clip} total {fade_total} frames (in {fade_in_frames} + out {fade_out_frames}), exceeding clip duration {clip_duration}"
    )]
    AudioFadesTooLong {
        clip: ClipId,
        fade_in_frames: TimeCode,
        fade_out_frames: TimeCode,
        fade_total: TimeCode,
        clip_duration: TimeCode,
    },
    #[error("title clip {0} has no audio contribution; SetClipAudio accepts media clips only")]
    TitleClipHasNoAudio(ClipId),
    #[error("freeze clip {0} has no audio contribution; SetClipAudio accepts media clips only")]
    FreezeClipHasNoAudio(ClipId),
    #[error("clip speed must be an integer percentage in 10..=1000, got {0}")]
    ClipSpeedOutOfRange(u32),
    #[error("clip {0} is not a media clip; only media clips have playback speed")]
    SpeedOnNonMediaClip(ClipId),
    #[error("time calculation overflowed")]
    TimeOverflow,
    #[error(transparent)]
    TimeMapping(#[from] TimeMappingError),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchError {
    #[error("edit plan must contain at least one operation")]
    Empty,
    #[error("edit plan operation {op_number} failed: {error}")]
    OperationFailed {
        /// One-based operation number as presented to users and agents.
        op_number: usize,
        #[source]
        error: OpError,
    },
}

#[allow(clippy::too_many_lines)]
fn apply_unchecked(operation: &Operation, doc: &mut Document) -> Result<(), OpError> {
    match operation {
        Operation::AddAsset { asset } => add_asset(doc, asset.clone()),
        Operation::RelinkAsset {
            asset,
            candidate,
            allow_unverified_source,
        } => relink_asset(doc, *asset, candidate, *allow_unverified_source),
        Operation::SetAssetColorDescription {
            asset,
            color_description,
        } => set_asset_color_description(doc, *asset, color_description.clone()),
        Operation::SetColorContext { color_context } => {
            set_color_context(doc, color_context.clone())
        }
        Operation::UpsertBin { bin } => {
            upsert_bin(doc, bin.clone());
            Ok(())
        }
        Operation::RemoveBin { bin } => remove_bin(doc, *bin),
        Operation::SetAssetBin { asset, bin } => set_asset_bin(doc, *asset, *bin),
        Operation::UpsertStringOut { string_out } => {
            upsert_string_out(doc, string_out.clone());
            Ok(())
        }
        Operation::RemoveStringOut { string_out } => remove_string_out(doc, *string_out),
        Operation::UpsertSyncGroup { sync_group } => {
            upsert_sync_group(doc, sync_group.clone());
            Ok(())
        }
        Operation::RemoveSyncGroup { sync_group } => remove_sync_group(doc, *sync_group),
        Operation::UpsertAudioBus { bus } => upsert_audio_bus(doc, bus.clone()),
        Operation::RemoveAudioBus { bus } => remove_audio_bus(doc, *bus),
        Operation::AddTrack { track } => add_track(doc, track.clone()),
        Operation::RemoveTrack { track } => remove_track(doc, *track),
        Operation::SetTrackSyncLock { track, locked } => set_track_sync_lock(doc, *track, *locked),
        Operation::AddClip {
            track,
            asset,
            at,
            source,
        } => add_clip(doc, *track, *asset, *at, source.clone()),
        Operation::AddTitle {
            track,
            at,
            duration,
            title,
        } => add_title(doc, *track, *at, *duration, title.clone()),
        Operation::AddFreezeFrame {
            track,
            at,
            duration,
            asset,
            source_frame,
        } => add_freeze_frame(doc, *track, *at, *duration, *asset, *source_frame),
        Operation::SplitClip { clip, at } => split_clip(doc, *clip, *at),
        Operation::TrimClip { clip, new_source } => trim_clip(doc, *clip, new_source.clone()),
        Operation::MoveClip { clip, to_track, to } => move_clip(doc, *clip, *to_track, *to),
        Operation::ThreePointEdit {
            track,
            asset,
            source_in,
            source_out,
            timeline_in,
            timeline_out,
            mode,
        } => three_point_edit(
            doc,
            *track,
            *asset,
            *source_in,
            *source_out,
            *timeline_in,
            *timeline_out,
            *mode,
        ),
        Operation::PatchedThreePointEdit {
            asset,
            source_in,
            source_out,
            timeline_in,
            timeline_out,
            mode,
            video_track,
            audio_track,
        } => patched_three_point_edit(
            doc,
            *asset,
            *source_in,
            *source_out,
            *timeline_in,
            *timeline_out,
            *mode,
            *video_track,
            *audio_track,
        ),
        Operation::SlipClip {
            clip,
            new_source_in,
        } => slip_clip(doc, *clip, *new_source_in),
        Operation::RollEdit {
            left_clip,
            right_clip,
            to,
        } => roll_edit(doc, *left_clip, *right_clip, *to),
        Operation::SlideClip { clip, to } => slide_clip(doc, *clip, *to),
        Operation::ReplaceClip {
            clip,
            asset,
            source,
        } => replace_clip(doc, *clip, *asset, source.clone(), false),
        Operation::FitToFill {
            clip,
            asset,
            source,
        } => replace_clip(doc, *clip, *asset, source.clone(), true),
        Operation::DeleteClip { clip } => delete_clip(doc, *clip),
        Operation::RippleDeleteClip { clip } => ripple_delete_clip(doc, *clip),
        Operation::RippleInsertGap {
            track,
            at,
            duration,
        } => ripple_insert_gap(doc, *track, *at, *duration),
        Operation::LinkClips { clips } => link_clips(doc, clips),
        Operation::UnlinkClips { clips } => unlink_clips(doc, clips),
        Operation::AddMarker { marker } => add_marker(doc, marker.clone()),
        Operation::RemoveMarker { marker } => remove_marker(doc, *marker),
        Operation::MoveMarker { marker, to } => move_marker(doc, *marker, *to),
        Operation::InsertEffect {
            clip,
            index,
            effect,
        } => insert_effect(doc, *clip, Some(*index), effect.clone()),
        Operation::ConvertLegacyLook {
            clip,
            effect,
            lut_asset,
            mix_basis_points,
        } => convert_legacy_look(doc, *clip, *effect, *lut_asset, *mix_basis_points),
        Operation::AddLutAsset { asset } => add_lut_asset(doc, asset.clone()),
        Operation::RemoveLutAsset { lut_asset } => remove_lut_asset(doc, *lut_asset),
        Operation::AddEffect { clip, effect } => {
            let mut effect = effect.clone();
            effect.canonicalize_legacy_name();
            add_effect(doc, *clip, effect)
        }
        Operation::RemoveEffect { clip, effect } => remove_effect(doc, *clip, *effect),
        Operation::SetEffectParam {
            clip,
            effect,
            name,
            value,
        } => set_effect_param(doc, *clip, *effect, name, value.clone()),
        Operation::SetEffectKeyframes {
            clip,
            effect,
            name,
            curve,
        } => set_effect_keyframes(doc, *clip, *effect, name, curve.clone()),
        Operation::ClearEffectKeyframes { clip, effect, name } => {
            clear_effect_keyframes(doc, *clip, *effect, name)
        }
        Operation::SetTitleParam { clip, name, value } => {
            set_title_param(doc, *clip, name, value.clone())
        }
        Operation::SetClipAudio {
            clip,
            gain_tenth_db,
            fade_in_frames,
            fade_out_frames,
        } => set_clip_audio(
            doc,
            *clip,
            *gain_tenth_db,
            *fade_in_frames,
            *fade_out_frames,
        ),
        Operation::AddTransition { clip, transition } => {
            add_transition(doc, *clip, transition.clone())
        }
        Operation::RemoveTransition { clip } => remove_transition(doc, *clip),
        Operation::SetMarkerParam {
            marker,
            name,
            value,
        } => set_marker_param(doc, *marker, name, value.clone()),
        Operation::SetClipSpeed {
            clip,
            speed_percent,
        } => set_clip_speed(doc, *clip, *speed_percent),
    }
}

fn add_track(doc: &mut Document, track: Track) -> Result<(), OpError> {
    if doc.tracks.iter().any(|existing| existing.id == track.id) {
        return Err(OpError::DuplicateTrack(track.id));
    }
    if !track.clips.is_empty() {
        return Err(OpError::NewTrackNotEmpty(track.id));
    }
    doc.tracks.push(track);
    Ok(())
}

fn remove_track(doc: &mut Document, track_id: TrackId) -> Result<(), OpError> {
    let index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    doc.tracks.remove(index);
    for bus in &mut doc.audio_mix.buses {
        bus.tracks.retain(|track| *track != track_id);
        bus.ducking_sidechain_tracks
            .retain(|track| *track != track_id);
    }
    doc.audio_mix.buses.retain(|bus| !bus.tracks.is_empty());
    Ok(())
}

fn set_track_sync_lock(doc: &mut Document, track_id: TrackId, locked: bool) -> Result<(), OpError> {
    let track = doc
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    track.sync_lock = locked;
    Ok(())
}

fn add_asset(doc: &mut Document, asset: MediaAsset) -> Result<(), OpError> {
    if doc.asset(asset.id).is_some() {
        return Err(OpError::DuplicateAsset(asset.id));
    }
    validate_asset(&asset)?;
    doc.media_pool.push(asset);
    Ok(())
}

fn relink_asset(
    doc: &mut Document,
    asset_id: AssetId,
    candidate: &RelinkCandidate,
    allow_unverified_source: bool,
) -> Result<(), OpError> {
    let index = doc
        .media_pool
        .iter()
        .position(|asset| asset.id == asset_id)
        .ok_or(OpError::MissingAsset(asset_id))?;
    if candidate.path.as_os_str().is_empty() {
        return Err(OpError::EmptyRelinkCandidatePath { asset: asset_id });
    }
    validate_source_fingerprint(asset_id, &candidate.fingerprint)?;
    if !candidate.fingerprint.is_verified() {
        return Err(OpError::UnverifiedRelinkCandidate { asset: asset_id });
    }

    let current = &doc.media_pool[index];
    if current.kind != candidate.kind {
        return Err(OpError::RelinkMetadataMismatch {
            asset: asset_id,
            field: "kind",
            expected: format!("{:?}", current.kind),
            actual: format!("{:?}", candidate.kind),
        });
    }
    if current.fps != candidate.fps {
        return Err(OpError::RelinkMetadataMismatch {
            asset: asset_id,
            field: "fps",
            expected: format!("{}/{}", current.fps.numerator(), current.fps.denominator()),
            actual: format!(
                "{}/{}",
                candidate.fps.numerator(),
                candidate.fps.denominator()
            ),
        });
    }
    if current.duration != candidate.duration {
        return Err(OpError::RelinkMetadataMismatch {
            asset: asset_id,
            field: "duration",
            expected: current.duration.to_string(),
            actual: candidate.duration.to_string(),
        });
    }
    if current.resolution != candidate.resolution {
        return Err(OpError::RelinkMetadataMismatch {
            asset: asset_id,
            field: "resolution",
            expected: format!("{:?}", current.resolution),
            actual: format!("{:?}", candidate.resolution),
        });
    }

    if current.source_fingerprint.is_verified() {
        if current.source_fingerprint != candidate.fingerprint {
            return Err(OpError::RelinkFingerprintMismatch { asset: asset_id });
        }
    } else if !allow_unverified_source {
        return Err(OpError::RelinkRequiresExplicitUnverifiedSource { asset: asset_id });
    }

    doc.media_pool[index].path.clone_from(&candidate.path);
    doc.media_pool[index]
        .source_fingerprint
        .clone_from(&candidate.fingerprint);
    Ok(())
}

fn set_asset_color_description(
    doc: &mut Document,
    asset_id: AssetId,
    color_description: ColorDescription,
) -> Result<(), OpError> {
    let index = doc
        .media_pool
        .iter()
        .position(|asset| asset.id == asset_id)
        .ok_or(OpError::MissingAsset(asset_id))?;
    validate_color_description(&color_description)?;
    if color_description.confidence_basis_points == 0 {
        return Err(OpError::ZeroConfidenceColorOverride { asset: asset_id });
    }
    if color_description.provenance != ColorProvenance::UserOverride {
        return Err(OpError::InvalidColorOverrideProvenance {
            asset: asset_id,
            actual: color_description.provenance,
        });
    }
    doc.media_pool[index].color_description = color_description;
    Ok(())
}

fn set_color_context(doc: &mut Document, color_context: ColorContext) -> Result<(), OpError> {
    for description in [
        &color_context.working,
        &color_context.monitoring,
        &color_context.delivery,
    ] {
        validate_color_description(description)?;
    }
    validate_hdr_delivery_description(&color_context.delivery)?;
    doc.color_context = color_context;
    Ok(())
}

/// CC8 §7 item 3: an HDR delivery description is validated against §5.1's
/// table when it is **set**, not only when it is exported.
///
/// §7 item 3, verbatim: "Setting an HDR delivery description is an ordinary
/// undoable, revision-gated, journalled operation, validated against §5.1's
/// table." [`Operation::SetColorContext`] is already that operation — ordinary,
/// undoable, revision-gated and journalled like every other — so what CC8 adds
/// is the clause after the comma, and it adds it here rather than inventing a
/// second operation for one field.
///
/// Only an **HDR-shaped** description is checked, and it is checked against
/// §5.1's lane alone. An SDR description takes the path it always took: the
/// project's delivery contract is not a delivery *gate*, and CC6 deliberately
/// lets a document hold a description the export refuses so that
/// `unsupported_delivery_color` can report it. What §7 item 3 changes is that
/// an HDR description cannot be stored half-formed — `bt2020` + `arib_std_b67`
/// with a BT.709 matrix would look like §5.1's lane in the colour status and
/// would only fail much later, at the encoder.
///
/// §11's PQ deferral reaches the document through the same check: `bt2020` +
/// `smpte2084` is an HDR pair, is not §5.1's lane, and is refused with §5.3's
/// own three facts and the deferral named.
fn validate_hdr_delivery_description(delivery: &ColorDescription) -> Result<(), OpError> {
    if !crate::color_description_is_cc8_hdr(delivery) {
        return Ok(());
    }
    match crate::delivery_color_mismatches_for_lane(delivery, crate::DeliveryLane::HdrHlgRec2020)
        .into_iter()
        .next()
    {
        None => Ok(()),
        Some(mismatch) => Err(OpError::UnsupportedHdrDeliveryDescription {
            field: mismatch.field.clone(),
            observed: mismatch.observed.clone(),
            allowed: mismatch.allowed.clone(),
            recovery: crate::delivery_field_recovery_action(&mismatch),
        }),
    }
}

fn upsert_bin(doc: &mut Document, bin: MediaBin) {
    if let Some(index) = doc
        .catalog
        .bins
        .iter()
        .position(|existing| existing.id == bin.id)
    {
        doc.catalog.bins[index] = bin;
    } else {
        doc.catalog.bins.push(bin);
    }
    doc.catalog.bins.sort_by_key(|bin| bin.id);
}

fn remove_bin(doc: &mut Document, bin_id: BinId) -> Result<(), OpError> {
    if doc
        .catalog
        .bins
        .iter()
        .any(|bin| bin.parent == Some(bin_id))
    {
        return Err(OpError::BinHasChildren(bin_id));
    }
    let index = doc
        .catalog
        .bins
        .iter()
        .position(|bin| bin.id == bin_id)
        .ok_or(OpError::MissingBin(bin_id))?;
    doc.catalog.bins.remove(index);
    Ok(())
}

fn set_asset_bin(
    doc: &mut Document,
    asset_id: AssetId,
    destination: Option<BinId>,
) -> Result<(), OpError> {
    if doc.asset(asset_id).is_none() {
        return Err(OpError::MissingAsset(asset_id));
    }
    if destination.is_some_and(|id| !doc.catalog.bins.iter().any(|bin| bin.id == id)) {
        return Err(OpError::MissingBin(destination.expect("checked as some")));
    }
    for bin in &mut doc.catalog.bins {
        bin.assets.retain(|asset| *asset != asset_id);
        if Some(bin.id) == destination {
            bin.assets.push(asset_id);
            bin.assets.sort_unstable();
        }
    }
    Ok(())
}

fn upsert_string_out(doc: &mut Document, string_out: StringOut) {
    if let Some(index) = doc
        .catalog
        .string_outs
        .iter()
        .position(|existing| existing.id == string_out.id)
    {
        doc.catalog.string_outs[index] = string_out;
    } else {
        doc.catalog.string_outs.push(string_out);
    }
    doc.catalog
        .string_outs
        .sort_by_key(|string_out| string_out.id);
}

fn remove_string_out(doc: &mut Document, id: StringOutId) -> Result<(), OpError> {
    let index = doc
        .catalog
        .string_outs
        .iter()
        .position(|string_out| string_out.id == id)
        .ok_or(OpError::MissingStringOut(id))?;
    doc.catalog.string_outs.remove(index);
    Ok(())
}

fn upsert_sync_group(doc: &mut Document, sync_group: SyncGroup) {
    if let Some(index) = doc
        .catalog
        .sync_groups
        .iter()
        .position(|existing| existing.id == sync_group.id)
    {
        doc.catalog.sync_groups[index] = sync_group;
    } else {
        doc.catalog.sync_groups.push(sync_group);
    }
    doc.catalog.sync_groups.sort_by_key(|group| group.id);
}

fn remove_sync_group(doc: &mut Document, id: SyncGroupId) -> Result<(), OpError> {
    let index = doc
        .catalog
        .sync_groups
        .iter()
        .position(|group| group.id == id)
        .ok_or(OpError::MissingSyncGroup(id))?;
    doc.catalog.sync_groups.remove(index);
    Ok(())
}

fn upsert_audio_bus(doc: &mut Document, bus: AudioBus) -> Result<(), OpError> {
    validate_audio_bus(doc, &bus)?;
    if let Some(index) = doc
        .audio_mix
        .buses
        .iter()
        .position(|existing| existing.id == bus.id)
    {
        doc.audio_mix.buses[index] = bus;
    } else {
        doc.audio_mix.buses.push(bus);
    }
    doc.audio_mix.buses.sort_by_key(|bus| bus.id);
    Ok(())
}

fn remove_audio_bus(doc: &mut Document, id: AudioBusId) -> Result<(), OpError> {
    let index = doc
        .audio_mix
        .buses
        .iter()
        .position(|bus| bus.id == id)
        .ok_or(OpError::MissingAudioBus(id))?;
    doc.audio_mix.buses.remove(index);
    Ok(())
}

fn add_clip(
    doc: &mut Document,
    track_id: TrackId,
    asset_id: AssetId,
    at: TimeCode,
    source: std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_source_range(asset, &source)?;
    let track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    validate_track_compatibility(asset, &doc.tracks[track_index])?;

    let clip = Clip {
        id: next_clip_id(doc)?,
        asset: asset_id,
        source_range: source,
        content: ClipContent::Media,
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    };
    doc.tracks[track_index].clips.push(clip);
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn add_title(
    doc: &mut Document,
    track_id: TrackId,
    at: TimeCode,
    duration: TimeCode,
    title: Title,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    if duration <= TimeCode::ZERO {
        return Err(OpError::InvalidTitleDuration(duration));
    }
    let track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    if doc.tracks[track_index].kind != TrackKind::Video {
        return Err(OpError::TitleOnAudioTrack(track_id));
    }
    let clip_id = next_clip_id(doc)?;
    validate_title(clip_id, &title, duration)?;
    doc.tracks[track_index].clips.push(Clip {
        id: clip_id,
        asset: AssetId::default(),
        source_range: TimeCode::ZERO..duration,
        content: ClipContent::Title(title),
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    });
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn add_freeze_frame(
    doc: &mut Document,
    track_id: TrackId,
    at: TimeCode,
    duration: TimeCode,
    asset_id: AssetId,
    source_frame: TimeCode,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    if duration <= TimeCode::ZERO {
        return Err(OpError::InvalidFreezeDuration(duration));
    }
    let track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    if doc.tracks[track_index].kind != TrackKind::Video {
        return Err(OpError::FreezeOnAudioTrack(track_id));
    }
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_track_compatibility(asset, &doc.tracks[track_index])?;
    validate_freeze_source_frame(asset, source_frame)?;
    let clip_id = next_clip_id(doc)?;
    doc.tracks[track_index].clips.push(Clip {
        id: clip_id,
        asset: asset_id,
        source_range: TimeCode::ZERO..duration,
        content: ClipContent::Freeze(FreezeFrame { source_frame }),
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    });
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn split_clip(doc: &mut Document, clip_id: ClipId, at: TimeCode) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let original = doc.tracks[track_index].clips[clip_index].clone();
    let end = doc.clip_end(&original)?;
    if at <= original.timeline_start || at >= end {
        return Err(OpError::SplitOutsideClip { clip: clip_id, at });
    }

    let offset = at
        .checked_sub(original.timeline_start)
        .ok_or(OpError::TimeOverflow)?;
    let source_split = match &original.content {
        ClipContent::Media => {
            let asset = doc
                .asset(original.asset)
                .ok_or(OpError::MissingAsset(original.asset))?;
            let effective =
                crate::clip_effective_fps(asset.fps, &original).map_err(OpError::TimeMapping)?;
            find_source_boundary(original.source_range.clone(), offset, effective, doc.fps)
                .ok_or(OpError::UnrepresentableSplit { clip: clip_id, at })?
        }
        ClipContent::Title(_) | ClipContent::Freeze(_) => original
            .source_range
            .start
            .checked_add(offset)
            .ok_or(OpError::TimeOverflow)?,
    };
    let new_id = next_clip_id(doc)?;

    doc.tracks[track_index].clips[clip_index].source_range.end = source_split;
    let mut right = original;
    right.id = new_id;
    right.source_range.start = source_split;
    right.timeline_start = at;
    right.transition_in = None;
    if let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content {
        title.fade_in_frames = title.fade_in_frames.min(offset);
        title.fade_out_frames = TimeCode::ZERO;
    }
    if let ClipContent::Title(title) = &mut right.content {
        title.fade_in_frames = TimeCode::ZERO;
        let right_duration = end.checked_sub(at).ok_or(OpError::TimeOverflow)?;
        title.fade_out_frames = title.fade_out_frames.min(right_duration);
    }
    doc.tracks[track_index].clips.push(right);
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn trim_clip(
    doc: &mut Document,
    clip_id: ClipId,
    new_source: std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let original = doc.tracks[track_index].clips[clip_index].clone();
    let source_fps = match &original.content {
        ClipContent::Media => {
            let asset = doc
                .asset(original.asset)
                .ok_or(OpError::MissingAsset(original.asset))?;
            validate_source_range(asset, &new_source)?;
            crate::clip_effective_fps(asset.fps, &original).map_err(OpError::TimeMapping)?
        }
        ClipContent::Title(_) | ClipContent::Freeze(_) => {
            validate_title_range(&new_source)?;
            doc.fps
        }
    };
    let shifted_start = match new_source.start.cmp(&original.source_range.start) {
        std::cmp::Ordering::Greater => {
            let offset = map_source_range_to_project(
                original.source_range.start..new_source.start,
                source_fps,
                doc.fps,
            )?;
            original
                .timeline_start
                .checked_add(offset)
                .ok_or(OpError::TimeOverflow)?
        }
        std::cmp::Ordering::Less => {
            let offset = map_source_range_to_project(
                new_source.start..original.source_range.start,
                source_fps,
                doc.fps,
            )?;
            original
                .timeline_start
                .checked_sub(offset)
                .ok_or(OpError::TimeOverflow)?
        }
        std::cmp::Ordering::Equal => original.timeline_start,
    };
    if shifted_start < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(shifted_start));
    }
    let duration = new_source
        .end
        .checked_sub(new_source.start)
        .ok_or(OpError::TimeOverflow)?;
    doc.tracks[track_index].clips[clip_index].timeline_start = shifted_start;
    doc.tracks[track_index].clips[clip_index].source_range = new_source;
    if let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content {
        title.fade_in_frames = title.fade_in_frames.min(duration);
        title.fade_out_frames = title.fade_out_frames.min(duration);
    }
    Ok(())
}

fn move_clip(
    doc: &mut Document,
    clip_id: ClipId,
    target_track_id: TrackId,
    to: TimeCode,
) -> Result<(), OpError> {
    if to < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(to));
    }
    let (source_track_index, clip_index) = find_clip(doc, clip_id)?;
    let target_track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == target_track_id)
        .ok_or(OpError::MissingTrack(target_track_id))?;
    match &doc.tracks[source_track_index].clips[clip_index].content {
        ClipContent::Media => {
            let asset_id = doc.tracks[source_track_index].clips[clip_index].asset;
            let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
            validate_track_compatibility(asset, &doc.tracks[target_track_index])?;
        }
        ClipContent::Title(_) => {
            if doc.tracks[target_track_index].kind != TrackKind::Video {
                return Err(OpError::TitleOnAudioTrack(target_track_id));
            }
        }
        ClipContent::Freeze(_) => {
            if doc.tracks[target_track_index].kind != TrackKind::Video {
                return Err(OpError::FreezeOnAudioTrack(target_track_id));
            }
        }
    }

    let mut clip = doc.tracks[source_track_index].clips.remove(clip_index);
    clip.timeline_start = to;
    doc.tracks[target_track_index].clips.push(clip);
    doc.tracks[target_track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn three_point_edit(
    doc: &mut Document,
    track_id: TrackId,
    asset_id: AssetId,
    source_in: Option<TimeCode>,
    source_out: Option<TimeCode>,
    timeline_in: Option<TimeCode>,
    timeline_out: Option<TimeCode>,
    mode: ThreePointMode,
) -> Result<(), OpError> {
    validate_three_point_selection(source_in, source_out, timeline_in, timeline_out)?;
    let asset = doc
        .asset(asset_id)
        .ok_or(OpError::MissingAsset(asset_id))?
        .clone();
    let track = doc
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    validate_track_compatibility(&asset, track)?;

    let (source, timeline) = derive_three_point_ranges(
        doc,
        &asset,
        source_in,
        source_out,
        timeline_in,
        timeline_out,
    )?;
    let duration = timeline_duration(&timeline)?;

    match mode {
        ThreePointMode::Insert => {
            let straddling = doc
                .tracks
                .iter()
                .filter(|track| track.id == track_id || track.sync_lock)
                .filter_map(|track| {
                    track.clips.iter().find_map(|clip| {
                        let end = doc.clip_end(clip).ok()?;
                        (clip.timeline_start < timeline.start && timeline.start < end)
                            .then_some(clip.id)
                    })
                })
                .collect::<Vec<_>>();
            for clip in straddling {
                split_clip(doc, clip, timeline.start)?;
            }
            ripple_insert_gap(doc, track_id, timeline.start, duration)?;
        }
        ThreePointMode::Overwrite => {
            clear_track_range(doc, track_id, timeline.clone())?;
        }
    }
    add_clip(doc, track_id, asset_id, timeline.start, source)
}

fn validate_three_point_selection(
    source_in: Option<TimeCode>,
    source_out: Option<TimeCode>,
    timeline_in: Option<TimeCode>,
    timeline_out: Option<TimeCode>,
) -> Result<(), OpError> {
    let marks = [source_in, source_out, timeline_in, timeline_out]
        .into_iter()
        .flatten()
        .count();
    if marks == 3 {
        Ok(())
    } else {
        Err(OpError::InvalidThreePointSelection)
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_three_point_ranges(
    doc: &Document,
    asset: &MediaAsset,
    source_in: Option<TimeCode>,
    source_out: Option<TimeCode>,
    timeline_in: Option<TimeCode>,
    timeline_out: Option<TimeCode>,
) -> Result<(std::ops::Range<TimeCode>, std::ops::Range<TimeCode>), OpError> {
    let (source, timeline) = match (source_in, source_out, timeline_in, timeline_out) {
        (Some(source_in), Some(source_out), Some(timeline_in), None) => {
            let source = source_in..source_out;
            validate_source_range(asset, &source)?;
            let duration = map_source_range_to_project(source.clone(), asset.fps, doc.fps)?;
            let timeline_out = timeline_in
                .checked_add(duration)
                .ok_or(OpError::TimeOverflow)?;
            (source, timeline_in..timeline_out)
        }
        (Some(source_in), Some(source_out), None, Some(timeline_out)) => {
            let source = source_in..source_out;
            validate_source_range(asset, &source)?;
            let duration = map_source_range_to_project(source.clone(), asset.fps, doc.fps)?;
            let timeline_in = timeline_out
                .checked_sub(duration)
                .ok_or(OpError::TimeOverflow)?;
            (source, timeline_in..timeline_out)
        }
        (Some(source_in), None, Some(timeline_in), Some(timeline_out)) => {
            validate_timeline_range(timeline_in, timeline_out)?;
            let duration = timeline_out
                .checked_sub(timeline_in)
                .ok_or(OpError::TimeOverflow)?;
            let source_out = source_end_for_project_duration(
                source_in,
                asset.duration,
                asset.fps,
                doc.fps,
                duration,
            )
            .ok_or(OpError::InvalidThreePointSource {
                start: source_in,
                end: asset.duration,
            })?;
            (source_in..source_out, timeline_in..timeline_out)
        }
        (None, Some(source_out), Some(timeline_in), Some(timeline_out)) => {
            validate_timeline_range(timeline_in, timeline_out)?;
            let duration = timeline_out
                .checked_sub(timeline_in)
                .ok_or(OpError::TimeOverflow)?;
            let source_in = source_start_for_project_duration(
                TimeCode::ZERO,
                source_out,
                asset.fps,
                doc.fps,
                duration,
            )
            .ok_or(OpError::InvalidThreePointSource {
                start: TimeCode::ZERO,
                end: source_out,
            })?;
            (source_in..source_out, timeline_in..timeline_out)
        }
        _ => return Err(OpError::InvalidThreePointSelection),
    };
    validate_source_range(asset, &source)?;
    validate_timeline_range(timeline.start, timeline.end)?;
    let duration = timeline_duration(&timeline)?;
    let mapped = map_source_range_to_project(source.clone(), asset.fps, doc.fps)?;
    if mapped != duration {
        return Err(OpError::InvalidThreePointSource {
            start: source.start,
            end: source.end,
        });
    }
    Ok((source, timeline))
}

fn timeline_duration(range: &std::ops::Range<TimeCode>) -> Result<TimeCode, OpError> {
    range
        .end
        .checked_sub(range.start)
        .ok_or(OpError::TimeOverflow)
}

#[allow(clippy::too_many_arguments)]
fn patched_three_point_edit(
    doc: &mut Document,
    asset_id: AssetId,
    source_in: Option<TimeCode>,
    source_out: Option<TimeCode>,
    timeline_in: Option<TimeCode>,
    timeline_out: Option<TimeCode>,
    mode: ThreePointMode,
    video_track: Option<TrackId>,
    audio_track: Option<TrackId>,
) -> Result<(), OpError> {
    validate_three_point_selection(source_in, source_out, timeline_in, timeline_out)?;
    let asset = doc
        .asset(asset_id)
        .ok_or(OpError::MissingAsset(asset_id))?
        .clone();
    let target_tracks = validate_source_patch_routes(doc, &asset, video_track, audio_track)?;
    let (source, timeline) = derive_three_point_ranges(
        doc,
        &asset,
        source_in,
        source_out,
        timeline_in,
        timeline_out,
    )?;
    let duration = timeline_duration(&timeline)?;

    match mode {
        ThreePointMode::Insert => {
            let target_set = target_tracks.iter().copied().collect::<HashSet<_>>();
            let straddling = doc
                .tracks
                .iter()
                .filter(|track| target_set.contains(&track.id) || track.sync_lock)
                .filter_map(|track| {
                    track.clips.iter().find_map(|clip| {
                        let end = doc.clip_end(clip).ok()?;
                        (clip.timeline_start < timeline.start && timeline.start < end)
                            .then_some(clip.id)
                    })
                })
                .collect::<Vec<_>>();
            for clip in straddling {
                split_clip(doc, clip, timeline.start)?;
            }
            ripple_insert_gap_for_tracks(doc, &target_tracks, timeline.start, duration)?;
        }
        ThreePointMode::Overwrite => {
            for track_id in &target_tracks {
                clear_track_range(doc, *track_id, timeline.clone())?;
            }
        }
    }

    let mut clip_ids = Vec::with_capacity(target_tracks.len());
    for track_id in target_tracks {
        let clip_id = next_clip_id(doc)?;
        add_clip(doc, track_id, asset_id, timeline.start, source.clone())?;
        clip_ids.push(clip_id);
    }
    if clip_ids.len() == 2 {
        link_clips(doc, &clip_ids)?;
    }
    Ok(())
}

fn validate_source_patch_routes(
    doc: &Document,
    asset: &MediaAsset,
    video_track: Option<TrackId>,
    audio_track: Option<TrackId>,
) -> Result<Vec<TrackId>, OpError> {
    let mut routes = Vec::with_capacity(2);
    let mut seen = HashSet::new();
    for (component, expected, track_id) in [
        ("video", TrackKind::Video, video_track),
        ("audio", TrackKind::Audio, audio_track),
    ]
    .into_iter()
    .filter_map(|(component, expected, track_id)| {
        track_id.map(|track_id| (component, expected, track_id))
    }) {
        if !seen.insert(track_id) {
            return Err(OpError::DuplicateSourcePatchTrack(track_id));
        }
        let track = doc
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .ok_or(OpError::MissingTrack(track_id))?;
        if track.kind != expected {
            return Err(OpError::InvalidSourcePatchRouteKind {
                component,
                expected,
                actual: track.kind,
                track_id,
            });
        }
        validate_track_compatibility(asset, track)?;
        routes.push(track_id);
    }
    if routes.is_empty() {
        return Err(OpError::EmptySourcePatch);
    }
    Ok(routes)
}

fn validate_timeline_range(start: TimeCode, end: TimeCode) -> Result<(), OpError> {
    if start < TimeCode::ZERO || end <= start {
        return Err(OpError::InvalidThreePointTimeline { start, end });
    }
    Ok(())
}

fn clear_track_range(
    doc: &mut Document,
    track_id: TrackId,
    range: std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    for boundary in [range.end, range.start] {
        let candidate = doc
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .ok_or(OpError::MissingTrack(track_id))?
            .clips
            .iter()
            .find_map(|clip| {
                let end = doc.clip_end(clip).ok()?;
                (clip.timeline_start < boundary && boundary < end).then_some(clip.id)
            });
        if let Some(clip) = candidate {
            split_clip(doc, clip, boundary)?;
        }
    }

    let remove = doc
        .tracks
        .iter()
        .find(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?
        .clips
        .iter()
        .filter_map(|clip| {
            let end = doc.clip_end(clip).ok()?;
            (clip.timeline_start >= range.start && end <= range.end).then_some(clip.id)
        })
        .collect::<Vec<_>>();
    for clip in remove {
        delete_clip(doc, clip)?;
    }
    Ok(())
}

fn slip_clip(doc: &mut Document, clip_id: ClipId, new_source_in: TimeCode) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    if !clip.content.is_media() {
        return Err(OpError::EditorialRequiresMedia(clip_id));
    }
    let span = clip
        .source_range
        .end
        .checked_sub(clip.source_range.start)
        .ok_or(OpError::TimeOverflow)?;
    let new_source_out = new_source_in
        .checked_add(span)
        .ok_or(OpError::TimeOverflow)?;
    let asset = doc
        .asset(clip.asset)
        .ok_or(OpError::MissingAsset(clip.asset))?;
    validate_source_range(asset, &(new_source_in..new_source_out))?;
    doc.tracks[track_index].clips[clip_index].source_range = new_source_in..new_source_out;
    Ok(())
}

fn roll_edit(
    doc: &mut Document,
    left_id: ClipId,
    right_id: ClipId,
    to: TimeCode,
) -> Result<(), OpError> {
    let (track_index, left_index) = find_clip(doc, left_id)?;
    let (right_track_index, right_index) = find_clip(doc, right_id)?;
    if track_index != right_track_index || right_index != left_index + 1 {
        return Err(OpError::ClipsNotAdjacent {
            left: left_id,
            right: right_id,
        });
    }
    let left = doc.tracks[track_index].clips[left_index].clone();
    let right = doc.tracks[track_index].clips[right_index].clone();
    require_media(&left)?;
    require_media(&right)?;
    let left_end = doc.clip_end(&left)?;
    let right_end = doc.clip_end(&right)?;
    if left_end != right.timeline_start || to <= left.timeline_start || to >= right_end {
        return Err(OpError::ClipsNotAdjacent {
            left: left_id,
            right: right_id,
        });
    }

    let left_asset = doc
        .asset(left.asset)
        .ok_or(OpError::MissingAsset(left.asset))?;
    let left_fps = crate::clip_effective_fps(left_asset.fps, &left)?;
    let left_duration = to
        .checked_sub(left.timeline_start)
        .ok_or(OpError::TimeOverflow)?;
    let left_source_out = source_end_for_project_duration(
        left.source_range.start,
        left_asset.duration,
        left_fps,
        doc.fps,
        left_duration,
    )
    .ok_or(OpError::UnrepresentableEditBoundary {
        clip: left_id,
        at: to,
    })?;

    let right_asset = doc
        .asset(right.asset)
        .ok_or(OpError::MissingAsset(right.asset))?;
    let right_fps = crate::clip_effective_fps(right_asset.fps, &right)?;
    let right_duration = right_end.checked_sub(to).ok_or(OpError::TimeOverflow)?;
    let right_source_in = source_start_for_project_duration(
        TimeCode::ZERO,
        right.source_range.end,
        right_fps,
        doc.fps,
        right_duration,
    )
    .ok_or(OpError::UnrepresentableEditBoundary {
        clip: right_id,
        at: to,
    })?;

    doc.tracks[track_index].clips[left_index].source_range.end = left_source_out;
    doc.tracks[track_index].clips[right_index]
        .source_range
        .start = right_source_in;
    doc.tracks[track_index].clips[right_index].timeline_start = to;
    Ok(())
}

fn slide_clip(doc: &mut Document, clip_id: ClipId, to: TimeCode) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let track_len = doc.tracks[track_index].clips.len();
    if clip_index == 0 || clip_index + 1 >= track_len {
        return Err(OpError::SlideRequiresNeighbors { clip: clip_id });
    }
    let left = doc.tracks[track_index].clips[clip_index - 1].clone();
    let middle = doc.tracks[track_index].clips[clip_index].clone();
    let right = doc.tracks[track_index].clips[clip_index + 1].clone();
    require_media(&left)?;
    require_media(&middle)?;
    require_media(&right)?;
    let left_end = doc.clip_end(&left)?;
    let middle_duration = doc.clip_duration(&middle)?;
    let middle_end = doc.clip_end(&middle)?;
    let right_end = doc.clip_end(&right)?;
    let new_end = to
        .checked_add(middle_duration)
        .ok_or(OpError::TimeOverflow)?;
    if left_end != middle.timeline_start
        || middle_end != right.timeline_start
        || to <= left.timeline_start
        || new_end >= right_end
    {
        return Err(OpError::SlideRequiresNeighbors { clip: clip_id });
    }

    let left_asset = doc
        .asset(left.asset)
        .ok_or(OpError::MissingAsset(left.asset))?;
    let left_fps = crate::clip_effective_fps(left_asset.fps, &left)?;
    let left_duration = to
        .checked_sub(left.timeline_start)
        .ok_or(OpError::TimeOverflow)?;
    let left_source_out = source_end_for_project_duration(
        left.source_range.start,
        left_asset.duration,
        left_fps,
        doc.fps,
        left_duration,
    )
    .ok_or(OpError::UnrepresentableEditBoundary {
        clip: left.id,
        at: to,
    })?;

    let right_asset = doc
        .asset(right.asset)
        .ok_or(OpError::MissingAsset(right.asset))?;
    let right_fps = crate::clip_effective_fps(right_asset.fps, &right)?;
    let right_duration = right_end
        .checked_sub(new_end)
        .ok_or(OpError::TimeOverflow)?;
    let right_source_in = source_start_for_project_duration(
        TimeCode::ZERO,
        right.source_range.end,
        right_fps,
        doc.fps,
        right_duration,
    )
    .ok_or(OpError::UnrepresentableEditBoundary {
        clip: right.id,
        at: new_end,
    })?;

    doc.tracks[track_index].clips[clip_index - 1]
        .source_range
        .end = left_source_out;
    doc.tracks[track_index].clips[clip_index].timeline_start = to;
    doc.tracks[track_index].clips[clip_index + 1]
        .source_range
        .start = right_source_in;
    doc.tracks[track_index].clips[clip_index + 1].timeline_start = new_end;
    Ok(())
}

fn replace_clip(
    doc: &mut Document,
    clip_id: ClipId,
    asset_id: AssetId,
    source: std::ops::Range<TimeCode>,
    fit_to_fill: bool,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let original = doc.tracks[track_index].clips[clip_index].clone();
    require_media(&original)?;
    let required = doc.clip_duration(&original)?;
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_source_range(asset, &source)?;
    validate_track_compatibility(asset, &doc.tracks[track_index])?;

    let speed_percent = if fit_to_fill {
        let real_time_duration = map_source_range_to_project(source.clone(), asset.fps, doc.fps)?.0;
        let ideal_speed = real_time_duration
            .checked_mul(100)
            .and_then(|value| value.checked_add(required.0 / 2))
            .and_then(|value| value.checked_div(required.0))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(OpError::TimeOverflow)?;
        (CLIP_SPEED_MIN_PERCENT..=CLIP_SPEED_MAX_PERCENT)
            .filter_map(|speed| {
                let effective = crate::speed_scaled_fps(asset.fps, speed).ok()?;
                let duration =
                    map_source_range_to_project(source.clone(), effective, doc.fps).ok()?;
                (duration == required).then_some(speed)
            })
            .min_by_key(|speed| speed.abs_diff(ideal_speed))
            .ok_or(OpError::FitToFillUnrepresentable { clip: clip_id })?
    } else {
        let actual = map_source_range_to_project(source.clone(), asset.fps, doc.fps)?;
        if actual != required {
            return Err(OpError::ReplacementDurationMismatch {
                clip: clip_id,
                required,
                actual,
            });
        }
        100
    };

    let clip = &mut doc.tracks[track_index].clips[clip_index];
    clip.asset = asset_id;
    clip.source_range = source;
    clip.content = ClipContent::Media;
    clip.speed_percent = speed_percent;
    Ok(())
}

fn require_media(clip: &Clip) -> Result<(), OpError> {
    if clip.content.is_media() {
        Ok(())
    } else {
        Err(OpError::EditorialRequiresMedia(clip.id))
    }
}

fn source_end_for_project_duration(
    source_start: TimeCode,
    maximum_end: TimeCode,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Option<TimeCode> {
    let mut low = source_start.0.checked_add(1)?;
    let mut high = maximum_end.0;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = TimeCode(middle);
        let mapped =
            map_source_range_to_project(source_start..candidate, source_fps, project_fps).ok()?;
        match mapped.cmp(&project_duration) {
            std::cmp::Ordering::Less => low = middle.checked_add(1)?,
            std::cmp::Ordering::Greater => high = middle.checked_sub(1)?,
            std::cmp::Ordering::Equal => return Some(candidate),
        }
    }
    None
}

fn source_start_for_project_duration(
    minimum_start: TimeCode,
    source_end: TimeCode,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Option<TimeCode> {
    let mut low = minimum_start.0;
    let mut high = source_end.0.checked_sub(1)?;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = TimeCode(middle);
        let mapped =
            map_source_range_to_project(candidate..source_end, source_fps, project_fps).ok()?;
        match mapped.cmp(&project_duration) {
            std::cmp::Ordering::Greater => low = middle.checked_add(1)?,
            std::cmp::Ordering::Less => high = middle.checked_sub(1)?,
            std::cmp::Ordering::Equal => return Some(candidate),
        }
    }
    None
}

fn delete_clip(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    doc.tracks[track_index].clips.remove(clip_index);
    Ok(())
}

fn ripple_delete_clip(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let removed = doc.tracks[track_index].clips[clip_index].clone();
    let duration = doc.clip_duration(&removed)?;
    let ripple_point = removed
        .timeline_start
        .checked_add(duration)
        .ok_or(OpError::TimeOverflow)?;
    let source_track = doc.tracks[track_index].id;
    doc.tracks[track_index].clips.remove(clip_index);
    for track in &mut doc.tracks {
        if track.id != source_track && !track.sync_lock {
            continue;
        }
        for clip in track
            .clips
            .iter_mut()
            .filter(|clip| clip.timeline_start >= ripple_point)
        {
            clip.timeline_start = clip
                .timeline_start
                .checked_sub(duration)
                .ok_or(OpError::TimeOverflow)?;
        }
    }
    shift_markers_left(&mut doc.markers, ripple_point, duration)?;
    Ok(())
}

fn ripple_insert_gap(
    doc: &mut Document,
    track_id: TrackId,
    at: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    ripple_insert_gap_for_tracks(doc, &[track_id], at, duration)
}

fn ripple_insert_gap_for_tracks(
    doc: &mut Document,
    target_track_ids: &[TrackId],
    at: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    if duration <= TimeCode::ZERO {
        return Err(OpError::InvalidRippleDuration(duration));
    }
    let target_tracks = target_track_ids.iter().copied().collect::<HashSet<_>>();
    for track_id in &target_tracks {
        if !doc.tracks.iter().any(|track| track.id == *track_id) {
            return Err(OpError::MissingTrack(*track_id));
        }
    }
    for track in &mut doc.tracks {
        if !target_tracks.contains(&track.id) && !track.sync_lock {
            continue;
        }
        for clip in track
            .clips
            .iter_mut()
            .filter(|clip| clip.timeline_start >= at)
        {
            clip.timeline_start = clip
                .timeline_start
                .checked_add(duration)
                .ok_or(OpError::TimeOverflow)?;
        }
    }
    shift_markers_right(&mut doc.markers, at, duration)?;
    Ok(())
}

fn shift_markers_left(
    markers: &mut [Marker],
    boundary: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    for marker in markers
        .iter_mut()
        .filter(|marker| marker.position >= boundary)
    {
        marker.position = marker
            .position
            .checked_sub(duration)
            .ok_or(OpError::TimeOverflow)?
            .max(TimeCode::ZERO);
    }
    markers.sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn shift_markers_right(
    markers: &mut [Marker],
    boundary: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    for marker in markers
        .iter_mut()
        .filter(|marker| marker.position >= boundary)
    {
        marker.position = marker
            .position
            .checked_add(duration)
            .ok_or(OpError::TimeOverflow)?;
    }
    markers.sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn validate_clip_selection(
    doc: &Document,
    clips: &[ClipId],
    minimum: usize,
) -> Result<(), OpError> {
    if clips.len() < minimum {
        return Err(OpError::TooFewLinkedClips { minimum });
    }
    let mut selected = HashSet::new();
    for clip in clips {
        if !selected.insert(*clip) {
            return Err(OpError::DuplicateClipSelection(*clip));
        }
        find_clip(doc, *clip)?;
    }
    Ok(())
}

fn link_clips(doc: &mut Document, clips: &[ClipId]) -> Result<(), OpError> {
    validate_clip_selection(doc, clips, 2)?;
    let link = next_link_id(doc)?;
    let selected = clips.iter().copied().collect::<HashSet<_>>();
    for clip in doc.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if selected.contains(&clip.id) {
            clip.link = Some(link);
        }
    }
    Ok(())
}

fn unlink_clips(doc: &mut Document, clips: &[ClipId]) -> Result<(), OpError> {
    validate_clip_selection(doc, clips, 1)?;
    let selected = clips.iter().copied().collect::<HashSet<_>>();
    for clip in doc.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if selected.contains(&clip.id) {
            clip.link = None;
        }
    }
    Ok(())
}

fn add_marker(doc: &mut Document, marker: Marker) -> Result<(), OpError> {
    if doc.marker(marker.id).is_some() {
        return Err(OpError::DuplicateMarker(marker.id));
    }
    validate_marker(&marker)?;
    doc.markers.push(marker);
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn remove_marker(doc: &mut Document, marker_id: MarkerId) -> Result<(), OpError> {
    let index = doc
        .markers
        .iter()
        .position(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    doc.markers.remove(index);
    Ok(())
}

fn move_marker(doc: &mut Document, marker_id: MarkerId, to: TimeCode) -> Result<(), OpError> {
    if to < TimeCode::ZERO {
        return Err(OpError::NegativeMarkerPosition(to));
    }
    let marker = doc
        .markers
        .iter_mut()
        .find(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    marker.position = to;
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn set_marker_param(
    doc: &mut Document,
    marker_id: MarkerId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let marker = doc
        .markers
        .iter_mut()
        .find(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    match (name, value) {
        ("label", ParamValue::Text(label)) => marker.label = label,
        ("color_token", ParamValue::Integer(token)) => {
            marker.color_token = u8::try_from(token).map_err(|_| OpError::InvalidMarkerColor {
                actual: u8::MAX,
                maximum_exclusive: MARKER_COLOR_TOKEN_COUNT,
            })?;
        }
        ("position", ParamValue::Integer(position)) => marker.position = TimeCode(position),
        ("label" | "color_token" | "position", _) => {
            return Err(OpError::InvalidMarkerParamType {
                marker: marker_id,
                name: name.to_owned(),
            });
        }
        _ => {
            return Err(OpError::UnknownMarkerParam {
                marker: marker_id,
                name: name.to_owned(),
            });
        }
    }
    validate_marker(marker)?;
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn add_effect(doc: &mut Document, clip_id: ClipId, effect: Effect) -> Result<(), OpError> {
    insert_effect(doc, clip_id, None, effect)
}

/// Place one effect on a clip, appending when `index` is `None` (CC4 §2.7).
///
/// `AddEffect` and `InsertEffect` share every check so a stack that one path
/// accepts can never be one the other rejects; the only difference is where
/// the effect lands and the additional bounds error.
fn insert_effect(
    doc: &mut Document,
    clip_id: ClipId,
    index: Option<usize>,
    effect: Effect,
) -> Result<(), OpError> {
    validate_effect(&effect)?;
    if is_audio_effect(&effect.name) {
        return Err(OpError::AudioEffectOnClip {
            clip: clip_id,
            effect: effect.name,
        });
    }
    validate_color_curve_points(&effect)?;
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip_duration = doc.clip_duration(&doc.tracks[track_index].clips[clip_index])?;
    validate_effect_automation(clip_id, clip_duration, &effect)?;
    // CC4 §3.3: a LUT node must resolve to a registered asset before it can be
    // stored, so `lut_assets` and the node stack can never disagree.
    validate_lut_node_references(doc, clip_id, &effect)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    if clip.effects.iter().any(|existing| existing.id == effect.id) {
        return Err(OpError::DuplicateEffect {
            clip: clip_id,
            effect: effect.id,
        });
    }
    let position = match index {
        None => clip.effects.len(),
        Some(index) => {
            if index > clip.effects.len() {
                return Err(OpError::EffectIndexOutOfRange {
                    clip: clip_id,
                    index,
                    len: clip.effects.len(),
                });
            }
            index
        }
    };
    // CC3 §3.1: a layer carries at most sixteen managed colour nodes. A
    // bypassed node keeps its slot, so the count is of stored nodes rather
    // than of active ones, and the seventeenth is a typed error instead of a
    // silent truncation.
    if crate::is_managed_color_node(&effect.name) {
        let existing = crate::managed_color_node_count(&clip.effects);
        if existing >= crate::COLOR_NODE_LIMIT_PER_LAYER {
            return Err(OpError::TooManyColorNodes {
                clip: clip_id,
                limit: crate::COLOR_NODE_LIMIT_PER_LAYER,
                actual: existing + 1,
            });
        }
    }
    // CC4 §3.1: LUT nodes carry a tighter limit than the managed stack,
    // because each one needs a texture atlas slot.
    if crate::is_lut_color_node(&effect.name) {
        let existing = crate::lut_node_count(&clip.effects);
        if existing >= crate::LUT_NODE_LIMIT_PER_LAYER {
            return Err(OpError::TooManyLutNodes {
                clip: clip_id,
                limit: crate::LUT_NODE_LIMIT_PER_LAYER,
                actual: existing + 1,
            });
        }
    }
    // CC4 §3.2: a vector order that contradicts the stage order is rejected,
    // never silently reordered, so the stored order stays the execution order.
    if let Some(violation) = crate::effect::color_stage_order_violation_over(
        prospective_color_nodes(&clip.effects, position, Some(&effect)),
    ) {
        return Err(stage_order_error(clip_id, &violation));
    }
    doc.tracks[track_index].clips[clip_index]
        .effects
        .insert(position, effect);
    Ok(())
}

/// The managed-node subsequence a clip would carry once `replacement` is
/// placed at `position`.
///
/// `replacement` is `Some` for an insertion and for a conversion that swaps
/// one effect for another; the existing effect at `position` is displaced by
/// an insertion, which the caller expresses by passing the untouched slice.
fn prospective_color_nodes<'a>(
    effects: &'a [Effect],
    position: usize,
    replacement: Option<&'a Effect>,
) -> Vec<(usize, EffectId, crate::ColorNodeKind)> {
    let mut nodes = Vec::new();
    let mut push = |index: usize, effect: &Effect| {
        if let Some(kind) = crate::ColorNodeKind::from_effect_name(&effect.name) {
            nodes.push((index, effect.id, kind));
        }
    };
    for (index, effect) in effects.iter().enumerate() {
        if index == position
            && let Some(replacement) = replacement
        {
            push(position, replacement);
        }
        push(if index < position { index } else { index + 1 }, effect);
    }
    if position == effects.len()
        && let Some(replacement) = replacement
    {
        push(position, replacement);
    }
    nodes
}

/// Convert a CC4 §3.2 violation into the typed rejection, which names both
/// nodes and both stages so a hand-written plan is diagnosable.
fn stage_order_error(clip: ClipId, violation: &crate::ColorStageOrderViolation) -> OpError {
    OpError::ColorStageOrderViolation {
        clip,
        effect: violation.effect,
        kind: violation.kind.effect_name().to_owned(),
        color_stage_rank: violation.color_stage.rank(),
        previous_effect: violation.previous_effect,
        previous_kind: violation.previous_kind.effect_name().to_owned(),
        previous_color_stage_rank: violation.previous_color_stage.rank(),
    }
}

/// Reject a LUT node whose stored or `Hold`-keyframed `lut_asset_id` does not
/// name a registered asset (CC4 §2.7, §3.3).
///
/// `0` is rejected with the rest: it is the unbound sentinel that keeps a
/// resolved node from indexing a missing asset, and a valid document never
/// stores it. An omitted `lut_asset_id` resolves to `0` and is rejected here,
/// which is why a LUT node's reset batch must exclude the parameter.
fn validate_lut_node_references(
    doc: &Document,
    clip: ClipId,
    effect: &Effect,
) -> Result<(), OpError> {
    if !crate::is_lut_color_node(&effect.name) {
        return Ok(());
    }
    let referenced = |value: i64| -> Result<(), OpError> {
        let lut_asset = LutAssetId(u64::try_from(value).unwrap_or_default());
        if lut_asset.0 == 0 || doc.lut_asset(lut_asset).is_none() {
            return Err(OpError::MissingLutAsset {
                clip,
                effect: effect.id,
                lut_asset,
            });
        }
        Ok(())
    };
    let stored = crate::LutNodeParams::from_effect(effect);
    referenced(i64::try_from(stored.lut_asset_id.0).unwrap_or_default())?;
    if let Some(curve) = effect.keyframes.get(crate::LUT_ASSET_ID_PARAMETER) {
        for keyframe in &curve.keyframes {
            referenced(keyframe.value)?;
        }
    }
    Ok(())
}

fn add_lut_asset(doc: &mut Document, asset: LutAsset) -> Result<(), OpError> {
    crate::validate_lut_asset(&asset)?;
    if doc
        .lut_assets
        .iter()
        .any(|existing| existing.id == asset.id)
    {
        return Err(OpError::DuplicateLutAsset(asset.id));
    }
    doc.lut_assets.push(asset);
    Ok(())
}

fn remove_lut_asset(doc: &mut Document, lut_asset: LutAssetId) -> Result<(), OpError> {
    // CC4 §2.7: removal never cascades. A bypassed node and a `Hold` keyframe
    // value both count as references, because both still resolve to the asset
    // on some frame or after one undo.
    if let Some(&(clip, effect)) = doc.lut_asset_references(lut_asset).first() {
        return Err(OpError::LutAssetInUse {
            lut_asset,
            clip,
            effect,
        });
    }
    let index = doc
        .lut_assets
        .iter()
        .position(|asset| asset.id == lut_asset)
        .ok_or(OpError::UnknownLutAsset(lut_asset))?;
    doc.lut_assets.remove(index);
    Ok(())
}

/// Replace one legacy LUT stage with a managed `creative_look` (CC4 §9).
fn convert_legacy_look(
    doc: &mut Document,
    clip_id: ClipId,
    effect_id: EffectId,
    lut_asset: LutAssetId,
    mix_basis_points: i64,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    let position = clip
        .effects
        .iter()
        .position(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    let legacy = &clip.effects[position];
    if !crate::POST_PRIMARY_LUT_EFFECT_NAMES.contains(&legacy.name.as_str()) {
        return Err(OpError::NotALegacyLook {
            clip: clip_id,
            effect: effect_id,
            name: legacy.name.clone(),
        });
    }
    if doc.lut_asset(lut_asset).is_none() {
        return Err(OpError::MissingLutAsset {
            clip: clip_id,
            effect: effect_id,
            lut_asset,
        });
    }
    let converted = Effect {
        id: effect_id,
        name: crate::ColorNodeKind::CreativeLook.effect_name().to_owned(),
        parameters: BTreeMap::from([
            (
                crate::LUT_ASSET_ID_PARAMETER.to_owned(),
                ParamValue::Integer(i64::try_from(lut_asset.0).unwrap_or(i64::MAX)),
            ),
            (
                crate::LUT_MIX_PARAMETER.to_owned(),
                ParamValue::Integer(mix_basis_points),
            ),
        ]),
        keyframes: BTreeMap::new(),
    };
    validate_effect(&converted)?;
    // The legacy stage is not a managed node, so the conversion adds one
    // managed node and one LUT node to the clip's budgets.
    let managed = crate::managed_color_node_count(&clip.effects);
    if managed >= crate::COLOR_NODE_LIMIT_PER_LAYER {
        return Err(OpError::TooManyColorNodes {
            clip: clip_id,
            limit: crate::COLOR_NODE_LIMIT_PER_LAYER,
            actual: managed + 1,
        });
    }
    let luts = crate::lut_node_count(&clip.effects);
    if luts >= crate::LUT_NODE_LIMIT_PER_LAYER {
        return Err(OpError::TooManyLutNodes {
            clip: clip_id,
            limit: crate::LUT_NODE_LIMIT_PER_LAYER,
            actual: luts + 1,
        });
    }
    let mut prospective: Vec<(usize, EffectId, crate::ColorNodeKind)> = clip
        .effects
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .filter_map(|(index, effect)| {
            crate::ColorNodeKind::from_effect_name(&effect.name)
                .map(|kind| (index, effect.id, kind))
        })
        .collect();
    let insert_at = prospective
        .iter()
        .position(|(index, _, _)| *index > position)
        .unwrap_or(prospective.len());
    prospective.insert(
        insert_at,
        (position, effect_id, crate::ColorNodeKind::CreativeLook),
    );
    if let Some(violation) = crate::effect::color_stage_order_violation_over(prospective) {
        return Err(stage_order_error(clip_id, &violation));
    }
    doc.tracks[track_index].clips[clip_index].effects[position] = converted;
    Ok(())
}

fn remove_effect(doc: &mut Document, clip_id: ClipId, effect_id: EffectId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &mut doc.tracks[track_index].clips[clip_index];
    let effect_index = clip
        .effects
        .iter()
        .position(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    clip.effects.remove(effect_index);
    Ok(())
}

fn set_effect_param(
    doc: &mut Document,
    clip_id: ClipId,
    effect_id: EffectId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    let effect_index = clip
        .effects
        .iter()
        .position(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    let effect = &clip.effects[effect_index];
    validate_effect_parameter(&effect.name, name, &value)?;
    // CC3 §2.3: strict `x` ordering is checked against the parameter map the
    // change would produce, so a rejected edit leaves the document untouched.
    if crate::classify_color_node(effect) == Some(crate::ColorNodeKind::Curves) {
        let mut prospective = effect.clone();
        prospective
            .parameters
            .insert(name.to_owned(), value.clone());
        validate_color_curve_points(&prospective)?;
    }
    // CC4 §6: a `SetEffectParam` that would unbind a node, or retarget it at
    // an asset the project does not own, is rejected rather than producing a
    // document that only `validate_document` would catch. This is why a LUT
    // node's reset batch must exclude `lut_asset_id`.
    if crate::is_lut_color_node(&effect.name) && name == crate::LUT_ASSET_ID_PARAMETER {
        let lut_asset = match value {
            ParamValue::Integer(stored) => LutAssetId(u64::try_from(stored).unwrap_or_default()),
            ParamValue::Boolean(_) | ParamValue::Text(_) => LutAssetId(0),
        };
        if lut_asset.0 == 0 || doc.lut_asset(lut_asset).is_none() {
            return Err(OpError::MissingLutAsset {
                clip: clip_id,
                effect: effect_id,
                lut_asset,
            });
        }
    }
    doc.tracks[track_index].clips[clip_index].effects[effect_index]
        .parameters
        .insert(name.to_owned(), value);
    Ok(())
}

fn set_effect_keyframes(
    doc: &mut Document,
    clip_id: ClipId,
    effect_id: EffectId,
    name: &str,
    curve: AutomationCurve,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip_duration = doc.clip_duration(&doc.tracks[track_index].clips[clip_index])?;
    let effect = doc.tracks[track_index].clips[clip_index]
        .effects
        .iter_mut()
        .find(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    let descriptor = crate::effect_descriptor(&effect.name)
        .and_then(|descriptor| descriptor.parameter(name))
        .ok_or_else(|| OpError::UnknownEffectParam {
            effect: effect.name.clone(),
            name: name.to_owned(),
        })?;
    validate_curve(
        clip_id,
        clip_duration,
        effect_id,
        &effect.name,
        descriptor,
        name,
        &curve,
    )?;
    // CC3 §6: the two legal curve keyframing policies are checked against the
    // automation map the change would produce, so either side of the pair -
    // an animated `point_count` or an animated coordinate - is rejected
    // atomically whichever one arrives second.
    if crate::classify_color_node(effect) == Some(crate::ColorNodeKind::Curves) {
        let mut prospective = effect.keyframes.clone();
        prospective.insert(name.to_owned(), curve.clone());
        validate_curve_keyframe_policy(&effect.name, &prospective)?;
    }
    effect.keyframes.insert(name.to_owned(), curve);
    Ok(())
}

fn clear_effect_keyframes(
    doc: &mut Document,
    clip_id: ClipId,
    effect_id: EffectId,
    name: &str,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let effect = doc.tracks[track_index].clips[clip_index]
        .effects
        .iter_mut()
        .find(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    if crate::effect_descriptor(&effect.name)
        .and_then(|descriptor| descriptor.parameter(name))
        .is_none()
    {
        return Err(OpError::UnknownEffectParam {
            effect: effect.name.clone(),
            name: name.to_owned(),
        });
    }
    effect.keyframes.remove(name);
    Ok(())
}

fn set_title_param(
    doc: &mut Document,
    clip_id: ClipId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let duration = doc.clip_duration(&doc.tracks[track_index].clips[clip_index])?;
    let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content else {
        return Err(OpError::NotTitleClip(clip_id));
    };
    validate_title_parameter(name, &value)?;
    match (name, value) {
        ("text", ParamValue::Text(value)) => title.text = value,
        ("font_size_token", ParamValue::Integer(value)) => {
            title.font_size_token =
                u8::try_from(value).map_err(|_| OpError::TitleParamOutOfRange {
                    name: name.to_owned(),
                    min: 0,
                    max: i64::from(u8::MAX),
                    actual: value,
                })?;
            title.caption_preset = None;
        }
        ("color_token", ParamValue::Integer(value)) => {
            title.color_token = u8::try_from(value).map_err(|_| OpError::TitleParamOutOfRange {
                name: name.to_owned(),
                min: 0,
                max: i64::from(u8::MAX),
                actual: value,
            })?;
            title.caption_preset = None;
        }
        ("position", ParamValue::Text(value)) => {
            title.position = value.parse().map_err(|()| OpError::InvalidTitleParamType {
                name: name.to_owned(),
            })?;
            title.caption_preset = None;
        }
        ("caption_preset", ParamValue::Text(value)) => {
            if value == "none" {
                title.caption_preset = None;
            } else {
                let preset = value.parse::<CaptionPreset>().map_err(|()| {
                    OpError::InvalidTitleParamType {
                        name: name.to_owned(),
                    }
                })?;
                let resolved = preset.title(title.text.clone());
                title.font_size_token = resolved.font_size_token;
                title.color_token = resolved.color_token;
                title.position = resolved.position;
                title.background_scrim = resolved.background_scrim;
                title.caption_preset = Some(preset);
            }
        }
        ("background_scrim", ParamValue::Boolean(value)) => {
            title.background_scrim = value;
            title.caption_preset = None;
        }
        ("fade_in_frames", ParamValue::Integer(value)) => {
            title.fade_in_frames = TimeCode(value);
        }
        ("fade_out_frames", ParamValue::Integer(value)) => {
            title.fade_out_frames = TimeCode(value);
        }
        _ => {
            return Err(OpError::InvalidTitleParamType {
                name: name.to_owned(),
            });
        }
    }
    validate_title(clip_id, title, duration)
}

fn add_transition(
    doc: &mut Document,
    clip_id: ClipId,
    transition: Transition,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    if clip.transition_in.is_some() {
        return Err(OpError::DuplicateTransition(clip_id));
    }
    validate_transition(doc, clip, &transition)?;
    doc.tracks[track_index].clips[clip_index].transition_in = Some(transition);
    Ok(())
}

fn remove_transition(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let transition = doc.tracks[track_index].clips[clip_index]
        .transition_in
        .take();
    if transition.is_none() {
        return Err(OpError::MissingTransition(clip_id));
    }
    Ok(())
}

/// Bounds for [`crate::Clip::speed_percent`]: 0.1x through 10x real time.
pub const CLIP_SPEED_MIN_PERCENT: u32 = 10;
pub const CLIP_SPEED_MAX_PERCENT: u32 = 1000;

fn set_clip_speed(doc: &mut Document, clip_id: ClipId, speed_percent: u32) -> Result<(), OpError> {
    validate_clip_speed(speed_percent)?;
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    if !doc.tracks[track_index].clips[clip_index].content.is_media() {
        return Err(OpError::SpeedOnNonMediaClip(clip_id));
    }
    doc.tracks[track_index].clips[clip_index].speed_percent = speed_percent;
    Ok(())
}

fn validate_clip_speed(speed_percent: u32) -> Result<(), OpError> {
    if !(CLIP_SPEED_MIN_PERCENT..=CLIP_SPEED_MAX_PERCENT).contains(&speed_percent) {
        return Err(OpError::ClipSpeedOutOfRange(speed_percent));
    }
    Ok(())
}

fn set_clip_audio(
    doc: &mut Document,
    clip_id: ClipId,
    gain_tenth_db: i32,
    fade_in_frames: TimeCode,
    fade_out_frames: TimeCode,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    match clip.content {
        ClipContent::Title(_) => return Err(OpError::TitleClipHasNoAudio(clip_id)),
        ClipContent::Freeze(_) => return Err(OpError::FreezeClipHasNoAudio(clip_id)),
        ClipContent::Media => {}
    }
    let clip_duration = doc.clip_duration(clip)?;
    validate_clip_audio_values(
        clip_id,
        gain_tenth_db,
        fade_in_frames,
        fade_out_frames,
        clip_duration,
    )?;
    let clip = &mut doc.tracks[track_index].clips[clip_index];
    clip.audio_gain_tenth_db = gain_tenth_db;
    clip.audio_fade_in_frames = fade_in_frames;
    clip.audio_fade_out_frames = fade_out_frames;
    Ok(())
}

fn find_clip(doc: &Document, id: ClipId) -> Result<(usize, usize), OpError> {
    doc.tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| {
            track
                .clips
                .iter()
                .position(|clip| clip.id == id)
                .map(|clip_index| (track_index, clip_index))
        })
        .ok_or(OpError::MissingClip(id))
}

fn next_clip_id(doc: &Document) -> Result<ClipId, OpError> {
    doc.tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(ClipId)
        .ok_or(OpError::ClipIdExhausted)
}

fn next_link_id(doc: &Document) -> Result<LinkId, OpError> {
    doc.tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|clip| clip.link)
        .map(|link| link.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(LinkId)
        .ok_or(OpError::LinkIdExhausted)
}

fn find_source_boundary(
    source: std::ops::Range<TimeCode>,
    project_offset: TimeCode,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
) -> Option<TimeCode> {
    let mut low = source.start.0.checked_add(1)?;
    let mut high = source.end.0.checked_sub(1)?;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = TimeCode(middle);
        let mapped =
            map_source_range_to_project(source.start..candidate, source_fps, project_fps).ok()?;
        match mapped.cmp(&project_offset) {
            std::cmp::Ordering::Less => low = middle.checked_add(1)?,
            std::cmp::Ordering::Greater => high = middle.checked_sub(1)?,
            std::cmp::Ordering::Equal => return Some(candidate),
        }
    }
    None
}

fn validate_asset(asset: &MediaAsset) -> Result<(), OpError> {
    if !asset.fps.is_valid() {
        return Err(OpError::InvalidAssetRate(asset.id));
    }
    if asset.duration <= TimeCode::ZERO {
        return Err(OpError::InvalidAssetDuration(asset.id));
    }
    if asset
        .resolution
        .is_some_and(|(width, height)| width == 0 || height == 0)
    {
        return Err(OpError::InvalidResolution);
    }
    validate_source_fingerprint(asset.id, &asset.source_fingerprint)?;
    validate_color_description(&asset.color_description)?;
    Ok(())
}

fn validate_source_fingerprint(
    asset: AssetId,
    fingerprint: &MediaSourceFingerprint,
) -> Result<(), OpError> {
    match (&fingerprint.content_sha256, fingerprint.byte_len) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(OpError::SourceFingerprintIncomplete { asset }),
        (Some(hash), Some(byte_len)) => {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(OpError::InvalidSourceFingerprintHash { asset });
            }
            if byte_len == 0 {
                return Err(OpError::InvalidSourceFingerprintByteLength { asset });
            }
            Ok(())
        }
    }
}

fn validate_color_description(color_description: &ColorDescription) -> Result<(), OpError> {
    if color_description.confidence_basis_points > COLOR_CONFIDENCE_MAX_BASIS_POINTS {
        return Err(OpError::ColorConfidenceOutOfRange {
            actual: color_description.confidence_basis_points,
        });
    }
    Ok(())
}

fn validate_freeze_source_frame(asset: &MediaAsset, source_frame: TimeCode) -> Result<(), OpError> {
    if source_frame < TimeCode::ZERO || source_frame >= asset.duration {
        return Err(OpError::FreezeSourceFrameOutOfRange {
            asset: asset.id,
            source_frame,
            duration: asset.duration,
        });
    }
    Ok(())
}

fn validate_effect(effect: &Effect) -> Result<(), OpError> {
    let Some(descriptor) = crate::effect_descriptor(&effect.name) else {
        return Err(OpError::UnknownEffect(effect.name.clone()));
    };
    if effect.name == "cube_lut"
        && !matches!(effect.parameters.get("path"), Some(ParamValue::Text(path)) if !path.trim().is_empty())
    {
        return Err(OpError::MissingCubeLutPath);
    }
    for (name, value) in &effect.parameters {
        if effect.name == "cube_lut" && name == "path" {
            continue;
        }
        validate_described_effect_parameter(descriptor, name, value)?;
    }
    for name in effect.keyframes.keys() {
        if descriptor.parameter(name).is_none() {
            return Err(OpError::UnknownEffectParam {
                effect: effect.name.clone(),
                name: name.clone(),
            });
        }
    }
    Ok(())
}

fn validate_effect_automation(
    clip: ClipId,
    clip_duration: TimeCode,
    effect: &Effect,
) -> Result<(), OpError> {
    let descriptor = crate::effect_descriptor(&effect.name)
        .ok_or_else(|| OpError::UnknownEffect(effect.name.clone()))?;
    for (name, curve) in &effect.keyframes {
        let parameter = descriptor
            .parameter(name)
            .ok_or_else(|| OpError::UnknownEffectParam {
                effect: effect.name.clone(),
                name: name.clone(),
            })?;
        validate_curve(
            clip,
            clip_duration,
            effect.id,
            &effect.name,
            parameter,
            name,
            curve,
        )?;
    }
    validate_curve_keyframe_policy(&effect.name, &effect.keyframes)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_curve(
    clip: ClipId,
    clip_duration: TimeCode,
    effect_id: EffectId,
    effect_name: &str,
    descriptor: &crate::EffectParameterDescriptor,
    name: &str,
    curve: &AutomationCurve,
) -> Result<(), OpError> {
    curve
        .validate()
        .map_err(|error| OpError::InvalidEffectAutomation {
            effect: effect_name.to_owned(),
            name: name.to_owned(),
            reason: error.to_string(),
        })?;
    // CC3 §6 policy 1: `{curve}_point_count` switches the whole curve
    // discontinuously, so only `Hold` keyframes are legal. Any other
    // interpolation would resolve intermediate point counts that no author
    // ever authored. CC4 §6 and CC5 §5.1 extend the same rule to LUT and
    // matte tokens and counts.
    if is_hold_only_parameter(effect_name, name) {
        for keyframe in &curve.keyframes {
            if keyframe.interpolation != KeyframeInterpolation::Hold {
                return Err(OpError::NonHoldKeyframeParameter {
                    effect: effect_name.to_owned(),
                    name: name.to_owned(),
                });
            }
        }
    }
    for keyframe in &curve.keyframes {
        if keyframe.at >= clip_duration {
            return Err(OpError::EffectKeyframeOutsideClip {
                clip,
                effect: effect_id,
                name: name.to_owned(),
                at: keyframe.at,
                duration: clip_duration,
            });
        }
        validate_described_effect_parameter(
            crate::effect_descriptor(effect_name).expect("registered effect"),
            name,
            &ParamValue::Integer(keyframe.value),
        )?;
        debug_assert!((descriptor.min..=descriptor.max).contains(&keyframe.value));
    }
    Ok(())
}

/// Whether one effect parameter accepts `Hold` keyframes only.
///
/// CC3 §6 policy 1 covers `{curve}_point_count`, which switches a whole curve
/// discontinuously. CC4 §6 adds a LUT node's `lut_asset_id` and
/// `input_encoding_token`: interpolating between two asset ids or two transfer
/// functions is meaningless, so the same typed rejection applies. CC5 §5.1
/// generalizes the same rule to a matte's tokens and counts — `matte_enabled`,
/// `matte_window_count`, `matte_combine_token`, `matte_invert`, and each
/// window's `shape_token` and `invert` — on the four matte-capable kinds.
/// Every other matte control, including the mix, the window geometry, and
/// every qualifier scalar, keeps every interpolation.
fn is_hold_only_parameter(effect_name: &str, name: &str) -> bool {
    let Some(kind) = crate::ColorNodeKind::from_effect_name(effect_name) else {
        return false;
    };
    if kind == crate::ColorNodeKind::Curves
        && crate::ColorCurveChannel::ALL
            .into_iter()
            .any(|curve| curve.point_count_parameter() == name)
    {
        return true;
    }
    if kind.is_lut()
        && (name == crate::LUT_ASSET_ID_PARAMETER || name == crate::LUT_INPUT_ENCODING_PARAMETER)
    {
        return true;
    }
    kind.supports_matte() && crate::is_hold_only_matte_parameter(name)
}

/// Reject a `color_curves` node whose stored static points are not strictly
/// increasing in `x` over a curve's active prefix (CC3 §2.3).
///
/// Points at index `>= point_count` are ignored, so their colliding
/// `(10000, 10000)` neutrals stay legal.
fn validate_color_curve_points(effect: &Effect) -> Result<(), OpError> {
    if let Some(violation) = crate::color_curve_order_violation(effect) {
        return Err(OpError::InvalidCurvePoints {
            effect: effect.name.clone(),
            curve: violation.curve.name().to_owned(),
            index: violation.index,
            previous_x: violation.previous_x,
            x: violation.x,
        });
    }
    Ok(())
}

/// Enforce the CC3 §6 curve keyframing policies over a complete automation map.
///
/// Whole-curve steps keyframe `{curve}_point_count`; point-wise interpolation
/// keyframes coordinates at a constant point count. Mixing them - a
/// `point_count` with more than one keyframe while any coordinate of the same
/// curve is animated - would resolve point lists nobody authored.
fn validate_curve_keyframe_policy(
    effect_name: &str,
    keyframes: &BTreeMap<String, AutomationCurve>,
) -> Result<(), OpError> {
    if crate::ColorNodeKind::from_effect_name(effect_name) != Some(crate::ColorNodeKind::Curves) {
        return Ok(());
    }
    for curve in crate::ColorCurveChannel::ALL {
        let animated_point_count = keyframes
            .get(curve.point_count_parameter())
            .is_some_and(|automation| automation.keyframes.len() > 1);
        if !animated_point_count {
            continue;
        }
        let animated_coordinate = curve.parameter_names().iter().skip(1).any(|name| {
            keyframes
                .get(*name)
                .is_some_and(|automation| !automation.keyframes.is_empty())
        });
        if animated_coordinate {
            return Err(OpError::CurvePointCountAnimatedWithPoints {
                effect: effect_name.to_owned(),
                curve: curve.name().to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_effect_parameter(effect: &str, name: &str, value: &ParamValue) -> Result<(), OpError> {
    if effect == "cube_lut" && name == "path" {
        return match value {
            ParamValue::Text(path) if !path.trim().is_empty() => Ok(()),
            ParamValue::Text(_) | ParamValue::Integer(_) | ParamValue::Boolean(_) => {
                Err(OpError::MissingCubeLutPath)
            }
        };
    }
    let Some(descriptor) = crate::effect_descriptor(effect) else {
        return Err(OpError::UnknownEffect(effect.to_owned()));
    };
    validate_described_effect_parameter(descriptor, name, value)
}

fn validate_described_effect_parameter(
    effect: crate::EffectDescriptor,
    name: &str,
    value: &ParamValue,
) -> Result<(), OpError> {
    let Some(parameter) = effect.parameter(name) else {
        return Err(OpError::UnknownEffectParam {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
        });
    };
    let ParamValue::Integer(actual) = value else {
        return Err(OpError::InvalidEffectParamType {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
        });
    };
    if !(parameter.min..=parameter.max).contains(actual) {
        return Err(OpError::EffectParamOutOfRange {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
            min: parameter.min,
            max: parameter.max,
            actual: *actual,
        });
    }
    Ok(())
}

fn validate_transition(
    doc: &Document,
    clip: &Clip,
    transition: &Transition,
) -> Result<(), OpError> {
    if crate::transition_descriptor(&transition.name).is_none() {
        return Err(OpError::UnknownTransition(transition.name.clone()));
    }
    if transition.duration <= TimeCode::ZERO {
        return Err(OpError::InvalidTransitionDuration {
            clip: clip.id,
            duration: transition.duration,
        });
    }
    let clip_duration = doc.clip_duration(clip)?;
    if transition.duration > clip_duration {
        return Err(OpError::TransitionTooLong {
            clip: clip.id,
            duration: transition.duration,
            clip_duration,
        });
    }
    Ok(())
}

fn validate_clip_audio(doc: &Document, clip: &Clip) -> Result<(), OpError> {
    if !clip.content.is_media()
        && (clip.audio_gain_tenth_db != 0
            || clip.audio_fade_in_frames != TimeCode::ZERO
            || clip.audio_fade_out_frames != TimeCode::ZERO)
    {
        return match clip.content {
            ClipContent::Title(_) => Err(OpError::TitleClipHasNoAudio(clip.id)),
            ClipContent::Freeze(_) => Err(OpError::FreezeClipHasNoAudio(clip.id)),
            ClipContent::Media => Ok(()),
        };
    }
    validate_clip_audio_values(
        clip.id,
        clip.audio_gain_tenth_db,
        clip.audio_fade_in_frames,
        clip.audio_fade_out_frames,
        doc.clip_duration(clip)?,
    )
}

fn validate_clip_audio_values(
    clip: ClipId,
    gain_tenth_db: i32,
    fade_in_frames: TimeCode,
    fade_out_frames: TimeCode,
    clip_duration: TimeCode,
) -> Result<(), OpError> {
    if !(-600..=120).contains(&gain_tenth_db) {
        return Err(OpError::AudioGainOutOfRange {
            clip,
            gain_tenth_db,
        });
    }
    for (name, frames) in [("fade-in", fade_in_frames), ("fade-out", fade_out_frames)] {
        if frames < TimeCode::ZERO {
            return Err(OpError::NegativeAudioFade { clip, name, frames });
        }
    }
    let fade_total = fade_in_frames
        .checked_add(fade_out_frames)
        .ok_or(OpError::TimeOverflow)?;
    if fade_total > clip_duration {
        return Err(OpError::AudioFadesTooLong {
            clip,
            fade_in_frames,
            fade_out_frames,
            fade_total,
            clip_duration,
        });
    }
    Ok(())
}

fn validate_marker(marker: &Marker) -> Result<(), OpError> {
    if marker.position < TimeCode::ZERO {
        return Err(OpError::NegativeMarkerPosition(marker.position));
    }
    if marker.color_token >= MARKER_COLOR_TOKEN_COUNT {
        return Err(OpError::InvalidMarkerColor {
            actual: marker.color_token,
            maximum_exclusive: MARKER_COLOR_TOKEN_COUNT,
        });
    }
    Ok(())
}

fn validate_title_parameter(name: &str, value: &ParamValue) -> Result<(), OpError> {
    let descriptor = title_parameter_descriptor(name)
        .ok_or_else(|| OpError::UnknownTitleParam(name.to_owned()))?;
    match (descriptor.kind, value) {
        (TitleParameterKind::Text { maximum_characters }, ParamValue::Text(text)) => {
            if text.chars().count() > maximum_characters {
                return Err(OpError::TitleTextTooLong {
                    maximum: maximum_characters,
                });
            }
        }
        (TitleParameterKind::Integer { min, max }, ParamValue::Integer(actual)) => {
            if !(min..=max).contains(actual) {
                return Err(OpError::TitleParamOutOfRange {
                    name: name.to_owned(),
                    min,
                    max,
                    actual: *actual,
                });
            }
        }
        (TitleParameterKind::Boolean, ParamValue::Boolean(_)) => {}
        (TitleParameterKind::Position, ParamValue::Text(position)) => {
            if position.parse::<TitlePosition>().is_err() {
                return Err(OpError::InvalidTitleParamType {
                    name: name.to_owned(),
                });
            }
        }
        (TitleParameterKind::CaptionPreset, ParamValue::Text(preset)) => {
            if preset != "none" && preset.parse::<CaptionPreset>().is_err() {
                return Err(OpError::InvalidTitleParamType {
                    name: name.to_owned(),
                });
            }
        }
        _ => {
            return Err(OpError::InvalidTitleParamType {
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_title(clip: ClipId, title: &Title, duration: TimeCode) -> Result<(), OpError> {
    for descriptor in crate::TITLE_PARAMETER_DESCRIPTORS {
        let value = crate::title_parameter_value(title, descriptor.name)
            .expect("every title descriptor has a typed value");
        validate_title_parameter(descriptor.name, &value)?;
    }
    if let Some(preset) = title.caption_preset {
        let resolved = preset.title("");
        if title.font_size_token != resolved.font_size_token
            || title.color_token != resolved.color_token
            || title.position != resolved.position
            || title.background_scrim != resolved.background_scrim
        {
            return Err(OpError::CaptionPresetMismatch { clip, preset });
        }
    }
    for (name, frames) in [
        ("fade_in_frames", title.fade_in_frames),
        ("fade_out_frames", title.fade_out_frames),
    ] {
        if frames > duration {
            return Err(OpError::TitleFadeTooLong {
                clip,
                name,
                frames,
                duration,
            });
        }
    }
    Ok(())
}

fn validate_title_range(source: &std::ops::Range<TimeCode>) -> Result<(), OpError> {
    if source.start < TimeCode::ZERO || source.end <= source.start {
        return Err(OpError::InvalidSourceRange {
            start: source.start.0,
            end: source.end.0,
        });
    }
    Ok(())
}

fn validate_source_range(
    asset: &MediaAsset,
    source: &std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    if source.start < TimeCode::ZERO || source.end <= source.start {
        return Err(OpError::InvalidSourceRange {
            start: source.start.0,
            end: source.end.0,
        });
    }
    if source.end > asset.duration {
        return Err(OpError::SourceOutOfBounds {
            asset: asset.id,
            end: source.end,
            duration: asset.duration,
        });
    }
    Ok(())
}

fn validate_track_compatibility(asset: &MediaAsset, track: &crate::Track) -> Result<(), OpError> {
    if asset.kind.supports(track.kind) {
        Ok(())
    } else {
        Err(OpError::IncompatibleTrack {
            asset: asset.id,
            track: match track.kind {
                crate::TrackKind::Video => "video",
                crate::TrackKind::Audio => "audio",
            },
            track_id: track.id,
        })
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn validate_document(doc: &Document) -> Result<(), OpError> {
    if !doc.fps.is_valid() {
        return Err(OpError::InvalidProjectRate);
    }
    if doc.resolution.0 == 0 || doc.resolution.1 == 0 {
        return Err(OpError::InvalidResolution);
    }
    for color_description in [
        &doc.color_context.working,
        &doc.color_context.monitoring,
        &doc.color_context.delivery,
    ] {
        validate_color_description(color_description)?;
    }

    let mut asset_ids = HashSet::new();
    for asset in &doc.media_pool {
        if !asset_ids.insert(asset.id) {
            return Err(OpError::DuplicateAsset(asset.id));
        }
        validate_asset(asset)?;
    }

    // CC4 §2.7: `lut_assets` ids are unique, and every record satisfies the
    // §2.1 table. Both are checked here so a hand-edited project cannot load
    // with a record `AddLutAsset` would have rejected.
    let mut lut_asset_ids = HashSet::new();
    for asset in &doc.lut_assets {
        if !lut_asset_ids.insert(asset.id) {
            return Err(OpError::DuplicateLutAsset(asset.id));
        }
        crate::validate_lut_asset(asset)?;
    }

    validate_catalog(doc)?;

    let mut track_ids = HashSet::new();
    let mut clip_ids = HashSet::new();
    let mut expected_duration = TimeCode::ZERO;
    for track in &doc.tracks {
        if !track_ids.insert(track.id) {
            return Err(OpError::DuplicateTrack(track.id));
        }
        let mut previous: Option<(&Clip, TimeCode)> = None;
        for clip in &track.clips {
            if !clip_ids.insert(clip.id) {
                return Err(OpError::DuplicateClip(clip.id));
            }
            if clip.timeline_start < TimeCode::ZERO {
                return Err(OpError::NegativeTimelinePosition(clip.timeline_start));
            }
            match &clip.content {
                ClipContent::Media => {
                    let asset = doc
                        .asset(clip.asset)
                        .ok_or(OpError::MissingAsset(clip.asset))?;
                    validate_track_compatibility(asset, track)?;
                    validate_source_range(asset, &clip.source_range)?;
                    validate_clip_speed(clip.speed_percent)?;
                }
                ClipContent::Title(title) => {
                    if track.kind != TrackKind::Video {
                        return Err(OpError::TitleOnAudioTrack(track.id));
                    }
                    validate_title_range(&clip.source_range)?;
                    let duration = doc.clip_duration(clip)?;
                    validate_title(clip.id, title, duration)?;
                }
                ClipContent::Freeze(freeze) => {
                    if track.kind != TrackKind::Video {
                        return Err(OpError::FreezeOnAudioTrack(track.id));
                    }
                    let asset = doc
                        .asset(clip.asset)
                        .ok_or(OpError::MissingAsset(clip.asset))?;
                    validate_track_compatibility(asset, track)?;
                    validate_freeze_source_frame(asset, freeze.source_frame)?;
                    validate_title_range(&clip.source_range)?;
                }
            }
            if !clip.content.is_media() && clip.speed_percent != 100 {
                return Err(OpError::SpeedOnNonMediaClip(clip.id));
            }
            let clip_duration = doc.clip_duration(clip)?;
            if clip_duration <= TimeCode::ZERO {
                return Err(OpError::ZeroProjectDuration(clip.id));
            }
            let clip_end = clip
                .timeline_start
                .checked_add(clip_duration)
                .ok_or(OpError::TimeOverflow)?;
            let mut effect_ids = HashSet::new();
            for effect in &clip.effects {
                if !effect_ids.insert(effect.id) {
                    return Err(OpError::DuplicateEffect {
                        clip: clip.id,
                        effect: effect.id,
                    });
                }
                if is_audio_effect(&effect.name) {
                    return Err(OpError::AudioEffectOnClip {
                        clip: clip.id,
                        effect: effect.name.clone(),
                    });
                }
                validate_effect(effect)?;
                // CC4 §2.7: every `lut_asset_id` a node references, static or
                // `Hold`-keyframed, must name a registered asset. A dangling
                // reference can only arrive by hand and must not load
                // silently, because a resolved node would then index an asset
                // the project does not own.
                validate_lut_node_references(doc, clip.id, effect)?;
                // CC3 §2.3: the strictly-increasing-`x` rule is a document
                // invariant, not just an operation precondition. Without it a
                // hand-edited project that `AddEffect`/`SetEffectParam` would
                // reject loads with `Ok` and then locks its own colour node,
                // because every later `SetEffectParam` re-validates the whole
                // effect and fails on parameters the user never touched.
                validate_color_curve_points(effect)?;
                validate_effect_automation(clip.id, clip_duration, effect)?;
            }
            // CC3 §3.1: the sixteen-managed-node limit is likewise an
            // invariant. `AddEffect` enforces it, so a document that exceeds it
            // can only have arrived by hand and must not load silently.
            let color_nodes = crate::managed_color_node_count(&clip.effects);
            if color_nodes > crate::COLOR_NODE_LIMIT_PER_LAYER {
                return Err(OpError::TooManyColorNodes {
                    clip: clip.id,
                    limit: crate::COLOR_NODE_LIMIT_PER_LAYER,
                    actual: color_nodes,
                });
            }
            // CC4 §3.1: the four-LUT-node atlas budget is an invariant for the
            // same reason.
            let lut_nodes = crate::lut_node_count(&clip.effects);
            if lut_nodes > crate::LUT_NODE_LIMIT_PER_LAYER {
                return Err(OpError::TooManyLutNodes {
                    clip: clip.id,
                    limit: crate::LUT_NODE_LIMIT_PER_LAYER,
                    actual: lut_nodes,
                });
            }
            // CC4 §3.2: the managed subsequence must have non-decreasing stage
            // rank. Every pre-CC4 project satisfies this trivially, because
            // all of its managed nodes are corrections at rank 1.
            if let Some(violation) = crate::color_stage_order_violation(&clip.effects) {
                return Err(stage_order_error(clip.id, &violation));
            }
            if let Some(transition) = &clip.transition_in {
                validate_transition(doc, clip, transition)?;
            }
            validate_clip_audio(doc, clip)?;
            if let Some((previous_clip, previous_end)) = previous {
                if clip.timeline_start < previous_clip.timeline_start {
                    return Err(OpError::ClipsUnsorted {
                        track: track.id,
                        previous: previous_clip.id,
                        next: clip.id,
                    });
                }
                if clip.timeline_start < previous_end {
                    return Err(OpError::ClipOverlap {
                        track: track.id,
                        clip: previous_clip.id,
                        with: clip.id,
                    });
                }
            }
            previous = Some((clip, clip_end));
            expected_duration = expected_duration.max(clip_end);
        }
    }

    let mut marker_ids = HashSet::new();
    let mut previous_marker: Option<&Marker> = None;
    for marker in &doc.markers {
        if !marker_ids.insert(marker.id) {
            return Err(OpError::DuplicateMarker(marker.id));
        }
        validate_marker(marker)?;
        if let Some(previous) = previous_marker
            && marker.position < previous.position
        {
            return Err(OpError::MarkersUnsorted {
                previous: previous.id,
                next: marker.id,
            });
        }
        previous_marker = Some(marker);
    }

    if doc.duration != expected_duration {
        return Err(OpError::IncorrectDocumentDuration {
            expected: expected_duration,
            actual: doc.duration,
        });
    }
    validate_audio_mix(doc)?;
    Ok(())
}

fn validate_audio_mix(doc: &Document) -> Result<(), OpError> {
    let mut bus_ids = HashSet::new();
    let mut routed_tracks = std::collections::HashMap::new();
    for bus in &doc.audio_mix.buses {
        if !bus_ids.insert(bus.id) {
            return Err(OpError::DuplicateAudioBus(bus.id));
        }
        validate_audio_bus(doc, bus)?;
        for track in &bus.tracks {
            if let Some(first) = routed_tracks.insert(*track, bus.id) {
                return Err(OpError::TrackInMultipleAudioBuses {
                    track: *track,
                    first,
                    second: bus.id,
                });
            }
        }
    }
    Ok(())
}

fn validate_audio_bus(doc: &Document, bus: &AudioBus) -> Result<(), OpError> {
    if bus.name.trim().is_empty() || bus.tracks.is_empty() {
        return Err(OpError::InvalidAudioBus(bus.id));
    }
    let mut tracks = HashSet::new();
    for track in &bus.tracks {
        if !tracks.insert(*track) {
            return Err(OpError::TrackInMultipleAudioBuses {
                track: *track,
                first: bus.id,
                second: bus.id,
            });
        }
        if !doc.tracks.iter().any(|candidate| candidate.id == *track) {
            return Err(OpError::AudioBusMissingTrack {
                bus: bus.id,
                track: *track,
            });
        }
    }
    let mut sidechains = HashSet::new();
    for track in &bus.ducking_sidechain_tracks {
        if !sidechains.insert(*track) || !doc.tracks.iter().any(|candidate| candidate.id == *track)
        {
            return Err(OpError::AudioBusMissingTrack {
                bus: bus.id,
                track: *track,
            });
        }
    }
    let mut effect_ids = HashSet::new();
    for effect in &bus.effects {
        if !effect_ids.insert(effect.id) {
            return Err(OpError::DuplicateAudioBusEffect {
                bus: bus.id,
                effect: effect.id,
            });
        }
        if !is_audio_effect(&effect.name) {
            return Err(OpError::VisualEffectOnAudioBus {
                bus: bus.id,
                effect: effect.name.clone(),
            });
        }
        validate_effect(effect)?;
        if effect.name == "audio_ducking" && bus.ducking_sidechain_tracks.is_empty() {
            return Err(OpError::AudioBusDuckingWithoutSidechain(bus.id));
        }
        let descriptor = crate::effect_descriptor(&effect.name).expect("registered effect");
        for (name, curve) in &effect.keyframes {
            curve
                .validate()
                .map_err(|error| OpError::InvalidEffectAutomation {
                    effect: effect.name.clone(),
                    name: name.clone(),
                    reason: error.to_string(),
                })?;
            for keyframe in &curve.keyframes {
                if keyframe.at >= doc.duration {
                    return Err(OpError::AudioBusKeyframeOutsideProject {
                        bus: bus.id,
                        effect: effect.id,
                        name: name.clone(),
                        at: keyframe.at,
                        duration: doc.duration,
                    });
                }
                validate_described_effect_parameter(
                    descriptor,
                    name,
                    &ParamValue::Integer(keyframe.value),
                )?;
            }
        }
    }
    Ok(())
}

fn validate_catalog(doc: &Document) -> Result<(), OpError> {
    let mut bin_ids = HashSet::new();
    let mut asset_bins = std::collections::HashMap::new();
    for bin in &doc.catalog.bins {
        if !bin_ids.insert(bin.id) {
            return Err(OpError::DuplicateBin(bin.id));
        }
        if bin.name.trim().is_empty() {
            return Err(OpError::EmptyBinName(bin.id));
        }
        if bin.parent == Some(bin.id) {
            return Err(OpError::BinSelfParent(bin.id));
        }
        let mut assets = HashSet::new();
        for asset in &bin.assets {
            if doc.asset(*asset).is_none() {
                return Err(OpError::MissingAsset(*asset));
            }
            if !assets.insert(*asset) {
                return Err(OpError::DuplicateBinAsset {
                    bin: bin.id,
                    asset: *asset,
                });
            }
            if let Some(first) = asset_bins.insert(*asset, bin.id) {
                return Err(OpError::AssetInMultipleBins {
                    asset: *asset,
                    first,
                    second: bin.id,
                });
            }
        }
    }
    for bin in &doc.catalog.bins {
        if let Some(parent) = bin.parent
            && !bin_ids.contains(&parent)
        {
            return Err(OpError::MissingBin(parent));
        }
        let mut seen = HashSet::new();
        let mut cursor = Some(bin.id);
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(OpError::BinCycle(id));
            }
            cursor = doc
                .catalog
                .bins
                .iter()
                .find(|candidate| candidate.id == id)
                .and_then(|candidate| candidate.parent);
        }
    }

    let mut string_out_ids = HashSet::new();
    for string_out in &doc.catalog.string_outs {
        if !string_out_ids.insert(string_out.id) {
            return Err(OpError::DuplicateStringOut(string_out.id));
        }
        if string_out.name.trim().is_empty() || string_out.selects.is_empty() {
            return Err(OpError::InvalidStringOut(string_out.id));
        }
        for select in &string_out.selects {
            let asset = doc
                .asset(select.asset)
                .ok_or(OpError::MissingAsset(select.asset))?;
            validate_source_range(asset, &select.source)?;
        }
    }

    let mut sync_group_ids = HashSet::new();
    for group in &doc.catalog.sync_groups {
        if !sync_group_ids.insert(group.id) {
            return Err(OpError::DuplicateSyncGroup(group.id));
        }
        if group.name.trim().is_empty() || group.members.len() < 2 {
            return Err(OpError::InvalidSyncGroup(group.id));
        }
        let mut assets = HashSet::new();
        for member in &group.members {
            if doc.asset(member.asset).is_none() {
                return Err(OpError::MissingAsset(member.asset));
            }
            if !assets.insert(member.asset) {
                return Err(OpError::DuplicateSyncGroupAsset {
                    group: group.id,
                    asset: member.asset,
                });
            }
            if member.angle_name.trim().is_empty() {
                return Err(OpError::EmptySyncAngle {
                    group: group.id,
                    asset: member.asset,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_shift_left_clamps_negative_results_to_zero() {
        let mut markers = [Marker {
            id: MarkerId(1),
            position: TimeCode(5),
            label: String::new(),
            color_token: 0,
        }];

        shift_markers_left(&mut markers, TimeCode(5), TimeCode(10)).unwrap();

        assert_eq!(markers[0].position, TimeCode::ZERO);
    }
}
