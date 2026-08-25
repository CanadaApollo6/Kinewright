//! Pure project state, edit operations, history, and subsystem contracts.

mod actor;
mod agent;
mod automation;
mod captions;
mod color;
mod creator;
mod delivery;
mod editorial;
mod effect;
mod journal;
mod media;
mod model;
mod multicam;
mod operation;
mod qa;
mod scopes;
mod time;
mod title;
mod transcript_edit;
mod transition;

pub use actor::{Command, Core, CoreDisconnected, Event, Query, QueryResult, TimelineRevision};
pub use agent::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
pub use automation::{AutomationCurve, AutomationCurveError, Keyframe, KeyframeInterpolation};
pub use captions::{
    CaptionCue, CaptionMotion, CaptionPlanError, animated_caption_operations,
    animated_caption_operations_at, authored_caption_cues, caption_cues, caption_title_operations,
    dedup_timeline_words, srt, vtt,
};
pub use color::{
    COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorContext, ColorDescription, ColorMatrix,
    ColorPipelineState, ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError,
    ColorSourceProfile, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint,
    classify_source, classify_source_with_assumption,
};
pub use creator::{
    BeatMontageAnchorRepair, BeatMontageCadenceContract, BeatMontageCadenceSummary,
    BeatMontageCutAnchor, BeatMontagePlan, BeatMontageSelect, BeatMontageShot, BeatPacingPlan,
    BeatPacingPoint, CreatorPlanError, MUSIC_STRUCTURE_DEFAULT_METER_BEATS,
    MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS, MUSIC_STRUCTURE_MAX_METER_BEATS,
    MUSIC_STRUCTURE_MAX_PHRASE_BARS, MusicBeatAnchor, MusicDurationFit, MusicEndAnchor,
    MusicEndAnchorEvidence, MusicEndBeatAlignment, MusicFitPlan, MusicFitStrategy,
    MusicPlaybackMode, MusicRepeatMode, MusicStructureAnalysis, MusicStructureCandidate,
    MusicStructureParameters, MusicStructureRole, TimelineBeatAnalysisState, beat_montage_plan,
    beat_montage_plan_near_anchors, beat_montage_plan_near_anchors_with_report,
    beat_montage_plan_with_anchors, beat_pacing_plan, music_fit_plan,
    music_fit_plan_with_end_anchor, music_structure_analysis, validate_beat_montage_cadence,
    validate_beat_montage_plan_cadence,
};
pub use delivery::{
    DeliveryAspect, DeliveryConformanceReport, DeliveryProfile, DeliveryVariant,
    DeliveryVariantError, delivery_conformance, document_for_delivery_profile,
    document_for_delivery_variant,
};
pub use editorial::ThreePointMode;
pub use effect::{
    COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_PARAMETER_COUNT, COLOR_CURVE_WHITE_BASIS_POINTS,
    COLOR_CURVES_PARAMETER_COUNT, COLOR_NODE_BYPASS_PARAMETER, COLOR_NODE_LIMIT_PER_LAYER,
    COLOR_WHEEL_GAIN_MAX_THOUSANDTHS, COLOR_WHEEL_GAIN_MIN_THOUSANDTHS,
    COLOR_WHEEL_GAMMA_MAX_THOUSANDTHS, COLOR_WHEEL_GAMMA_MIN_THOUSANDTHS,
    COLOR_WHEEL_LIFT_MAX_BASIS_POINTS, COLOR_WHEEL_LIFT_MIN_BASIS_POINTS,
    COLOR_WHEEL_UNITY_THOUSANDTHS, ColorCurveChannel, ColorCurveOrderViolation,
    ColorNodeInactiveReason, ColorNodeKind, ColorStage, ColorStageOrderViolation,
    ColorWheelChannel, ColorWheelControl, ColorWheelControlSet, ColorWheelsParams, CurvePoints,
    EFFECT_DESCRIPTORS, EffectCompatibilityStage, EffectDescriptor, EffectParameterDescriptor,
    EffectUniform, LEGACY_DISPLAY_EFFECT_NAMES, LUT_ASSET_ID_PARAMETER,
    LUT_INPUT_ENCODING_PARAMETER, LUT_INPUT_ENCODING_TOKEN_MAX, LUT_MIX_BASIS_POINTS_MAX,
    LUT_MIX_PARAMETER, LUT_NODE_LIMIT_PER_LAYER, LutNodeParams, MANAGED_COLOR_NODE_NAMES,
    POST_PRIMARY_LUT_EFFECT_NAMES, ResolvedCurves, active_color_nodes, classify_color_node,
    color_curve_order_violation, color_curve_parameter_names, color_node_inactive_reason,
    color_stage_order_violation, effect_compatibility_stage, effect_descriptor, is_audio_effect,
    is_legacy_display_effect, is_lut_color_node, is_managed_color_node, lut_node_count,
    lut_node_may_be_active, managed_color_node_count,
};
pub use journal::JournalCommand;
pub use media::{
    Analysis, AnalysisJobStatus, AnalysisKind, AnalysisPhase, AssetBeats, AssetSceneChanges,
    AssetSilences, AssetTranscript, AudioLoudness, BeatMarker, BeatStatus, Export,
    ExportCancellation, ExportLutPreflightIssue, ExportLutPreflightReport,
    ExportMediaPreflightIssue, ExportMediaPreflightReport, ExportProgress, ExportSettings,
    FrameTexture, LutAvailabilityKind, LutAvailabilityStatus, MediaAvailabilityKind,
    MediaAvailabilityStatus, MediaCacheClearResult, MediaCacheFamily, MediaCacheFamilyStatus,
    MediaCacheInventory, MediaError, MediaEvent, MonitorProof, MonitorProofMetadata,
    MonitorProofRenderKind, Playback, PlaybackState, ProgressSink, RgbaImage, SceneChange,
    SceneStatus, SilenceSpan, SilenceStatus, ThumbnailFrame, ThumbnailKey, TimelineBeat,
    TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord, TranscriptStatus,
    TranscriptWord, VisualAssetResult, VisualRequestKind, WaveformData, WaveformPeak,
    export_lut_preflight_with, export_media_preflight,
};
pub use model::{
    AssetId, AudioBus, AudioBusId, AudioMix, BinId, Clip, ClipContent, ClipId, Document, Effect,
    EffectId, FreezeFrame, LUT_ASSET_ID_MAX, LUT_SIZE_MAX, LUT_SIZE_MIN, LinkId, LutAsset,
    LutAssetId, LutAssetKind, LutAssetSource, MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId,
    MediaAsset, MediaBin, MediaCatalog, MediaKind, MediaSourceFingerprint, ParamValue,
    RelinkCandidate, SourceSelect, StringOut, StringOutId, SyncGroup, SyncGroupId, SyncGroupMember,
    Track, TrackId, TrackKind, Transition, clip_effective_fps, validate_lut_asset,
};
pub use multicam::{
    ReframeFocusBounds, SpeakerAngleAssignment, SpeakerMulticamCut, SpeakerMulticamError,
    SpeakerMulticamPlan, SpeakerMulticamSettings, SubjectCenterBasisPointSample,
    SubjectCenterSample, SubjectFocusBasisPointConstraint, SubjectReframeBasisPointPlan,
    SubjectReframeError, SubjectReframePlan, SubjectReframeSettings, plan_speaker_multicam,
    plan_subject_reframe, plan_subject_reframe_basis_points,
    plan_subject_reframe_basis_points_with_containment,
};
pub use operation::{ApplyOp, BatchError, OpError, Operation, apply_batch};
pub use qa::{QaIssue, QaReport, QaSeverity, qa_document};
pub use scopes::{
    ChannelStatistics, ChannelStatisticsDelta, ClippingBasisPoints, ClippingDelta, LumaWaveform,
    NormalizedRoi, NormalizedRoiError, ParadeChannel, PixelRoi, RgbParade, RgbParadeDelta,
    SCOPE_BASIS_POINTS, SCOPE_HIGH_CLIP_CODE, SCOPE_LOW_CLIP_CODE, SCOPE_MAX_HISTOGRAM_BINS,
    SCOPE_MAX_TEMPORAL_FRAMES, SCOPE_MAX_VECTORSCOPE_SIZE, SCOPE_MAX_WAVEFORM_COLUMNS,
    SCOPE_MAX_WAVEFORM_ROWS, SCOPE_MEAN_SCALE, SCOPE_SAMPLE_SCALE, ScopeClipping,
    ScopeClippingDelta, ScopeComparison, ScopeComparisonError, ScopeError, ScopeEvidence,
    ScopeFrame, ScopeFrameInput, ScopeGridDelta, ScopeHistogramDelta, ScopeHistograms,
    ScopeMeasurementMetadata, ScopePipelineStage, ScopeRasterResolution, ScopeRequest,
    ScopeResolution, ScopeStage, ScopeStatistics, ScopeStatisticsDelta, SignedDelta,
    VectorscopeDensity, compare_scope_evidence, compare_scopes, measure_scope, measure_scopes,
};
pub use time::{
    FrameRounding, Rational, TimeCode, TimeMappingError, map_frames, map_frames_with_rounding,
    map_source_range_to_project, speed_scaled_fps,
};
pub use title::{
    CaptionPreset, TITLE_COLORS, TITLE_FONT_SIZES, TITLE_PARAMETER_DESCRIPTORS, Title,
    TitleColorDescriptor, TitleFontSizeDescriptor, TitleLayout, TitleParameterDescriptor,
    TitleParameterKind, TitlePixelBounds, TitlePosition, title_color, title_font_bytes,
    title_font_size, title_layout, title_parameter_descriptor, title_parameter_value,
};
pub use transcript_edit::{
    TranscriptCutRange, is_filler_word, silence_cut_margin_frames, transcript_cut_ranges,
    transcript_cut_ranges_for_indices,
};
pub use transition::{
    TRANSITION_DESCRIPTORS, TransitionDescriptor, TransitionShading, transition_descriptor,
};
