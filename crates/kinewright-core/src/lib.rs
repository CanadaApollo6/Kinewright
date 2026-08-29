//! Pure project state, edit operations, history, and subsystem contracts.

mod actor;
mod agent;
mod automation;
mod captions;
pub mod cc7_scenarios;
pub mod cc8_hdr;
mod color;
mod color_qc;
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
pub use cc7_scenarios::{
    CC7_A_OPERATIONS, CC7_B1_OPERATIONS, CC7_B2_OPERATIONS, CC7_C_OPERATIONS, CC7_D_OPERATIONS,
    CC7_D2_OPERATIONS, CC7_E_OPERATIONS, CC7_F_OPERATIONS, CC7_SCENARIO_SPECS, CC7_SCENARIOS,
    Cc7Camera, Cc7CameraTransform, Cc7Clip, Cc7MatchProposal, Cc7Operation, Cc7Patch,
    Cc7PersonPath, Cc7PixelRect, Cc7Scenario, Cc7ScenarioSpec, Cc7Source,
    cc7_analytic_square_centre_basis_points, cc7_analytic_square_top_left,
    cc7_b1_canonical_operations, cc7_camera_code, cc7_camera_patch_codes, cc7_camera_transform,
    cc7_canonical_operations, cc7_d2_canonical_operations, cc7_decode_display709, cc7_encode_bt709,
    cc7_grade709_decode, cc7_log_encode_code, cc7_log_inverse_display,
    cc7_lut_backed_canonical_operations, cc7_spec, cc7_tracking_sample_frames,
};
pub use cc8_hdr::{
    CC8_BT709_PRIMARIES_TEN_THOUSANDTHS, CC8_BT709_TO_REC2020, CC8_BT2020_CB_DENOMINATOR,
    CC8_BT2020_CR_DENOMINATOR, CC8_BT2020_KB, CC8_BT2020_KG, CC8_BT2020_KR, CC8_BT2020_LUMA_F32,
    CC8_D65_TEN_THOUSANDTHS, CC8_GATE_MEASUREMENT_STEP, CC8_GATES, CC8_HDR_DELIVERY_ALLOWED,
    CC8_HDR_DEPTH_ALLOWED, CC8_HDR_MATRIX_ALLOWED, CC8_HDR_MAX_INTEGER_DEPTH_BITS,
    CC8_HDR_MIN_INTEGER_DEPTH_BITS, CC8_HDR_PRIMARIES_ALLOWED, CC8_HDR_RANGE_ALLOWED,
    CC8_HDR_RECOVERY_ACTION, CC8_HDR_WHITE_POINT_ALLOWED, CC8_HLG_A, CC8_HLG_B, CC8_HLG_C,
    CC8_HLG_NOMINAL_PEAK_NITS, CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT, CC8_HLG_SCENE_BREAKPOINT,
    CC8_HLG_SIGNAL_BREAKPOINT, CC8_HLG_SYSTEM_GAMMA_THOUSANDTHS, CC8_PQ_C1, CC8_PQ_C2, CC8_PQ_C3,
    CC8_PQ_M1, CC8_PQ_M2, CC8_PQ_PEAK_NITS, CC8_REC2020_PRIMARIES_TEN_THOUSANDTHS,
    CC8_REC2020_TO_BT709, CC8_REFERENCE_WHITE_NITS, CC8_REJECTED_HDR_ADJACENT, CC8_SOURCE_PROFILES,
    Cc8ChromaticityTenThousandths, Cc8Gate, Cc8GateShape, Cc8GateValue, Cc8RejectedHdrTuple,
    Cc8SourceProfile, cc8_apply_matrix, cc8_bt2020_luma, cc8_hlg_decode_working_linear,
    cc8_hlg_encode_working_linear, cc8_hlg_inverse_oetf, cc8_hlg_inverse_ootf,
    cc8_hlg_inverse_ootf_nominal, cc8_hlg_nominal_peak_nits, cc8_hlg_oetf, cc8_hlg_ootf_nits,
    cc8_hlg_ootf_nits_nominal, cc8_hlg_system_gamma, cc8_is_hdr_source_pair,
    cc8_nits_to_working_linear, cc8_pq_decode_working_linear, cc8_pq_encode_working_linear,
    cc8_pq_eotf_nits, cc8_pq_inverse_eotf, cc8_sign, cc8_source_profile_by_id,
    cc8_source_profile_for_primaries_and_transfer, cc8_working_linear_to_nits,
};
pub use color::{
    COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorContext, ColorDescription, ColorMatrix,
    ColorPipelineState, ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError,
    ColorSourceProfile, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint,
    classify_source, classify_source_with_assumption, color_description_is_cc8_hdr,
};
pub use color_qc::{
    BT709_CB_DENOMINATOR, BT709_CR_DENOMINATOR, BT709_KB, BT709_KR, COLOR_QC_ENGINE,
    ChannelRangeExcursion, ColorGamutReport, ColorNodeQcContribution, ColorQcCheck, ColorQcError,
    ColorQcException, ColorQcNodeContributions, ColorQcProvenance, ColorQcRegion, ColorQcReport,
    ColorQcRequest, ColorRangeReport, GAMUT_DEFINITION, MAX_QC_NODE_CONTRIBUTIONS,
    MatteRegionScope, NODE_ATTRIBUTION_REMOVED, PlaneLegalExcursion,
    QC_GAMUT_EXCEPTION_BASIS_POINTS, QC_RANGE_EXCEPTION_BASIS_POINTS,
    SKIN_BAND_CENTER_CENTIDEGREES, SKIN_BAND_EXCEPTION_BASIS_POINTS,
    SKIN_BAND_HALF_WIDTH_CENTIDEGREES, SKIN_DIAGNOSTIC_BOUNDARY, SKIN_MAX_SPREAD_CENTIDEGREES,
    SKIN_MIN_CHROMA_MILLIONTHS, SKIN_PATCH_HUE_CENTIDEGREES, SkinDiagnostics,
    YCBCR_CHROMA_LEGAL_HIGH, YCBCR_CHROMA_OFFSET, YCBCR_CHROMA_SPAN, YCBCR_LUMA_LEGAL_HIGH,
    YCBCR_LUMA_OFFSET, YCBCR_LUMA_SPAN, YCbCrLegalReport, YCbCrLegalSource,
    attach_node_contributions, bt709_limited_ycbcr, encode_bt709_delivery, measure_color_qc, nodes,
    validate_node_budget,
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
    DECODED_RANGE_EXCEPTION_BASIS_POINTS, DELIVERY_BIT_DEPTH_ALLOWED, DELIVERY_LUMA_MAX_CODE_8BIT,
    DELIVERY_LUMA_MAX_CODE_10BIT, DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
    DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS, DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
    DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS, DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT,
    DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT, DELIVERY_RGB_EXTREMES_NOTE,
    DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS, DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
    DELIVERY_VERIFICATION_FRAME_COUNT, DELIVERY_VERIFICATION_MAX_FRAMES, DeliveryAspect,
    DeliveryBudgets, DeliveryChannelDifference, DeliveryColorError, DeliveryColorMismatch,
    DeliveryComparison, DeliveryConformanceReport, DeliveryEncodeDepth, DeliveryProfile,
    DeliveryTagCheck, DeliveryTagNotRepresentable, DeliveryTagSource, DeliveryVariant,
    DeliveryVariantError, DeliveryVerification, DeliveryVerificationError,
    DeliveryVerificationRequest, H264_WHITE_POINT_NOT_REPRESENTABLE_REASON,
    HDR_SOURCE_ON_SDR_DELIVERY, delivery_color_for_depth, delivery_color_mismatch,
    delivery_color_mismatches, delivery_conformance, delivery_tag_check,
    document_for_delivery_profile, document_for_delivery_variant,
};
pub use editorial::ThreePointMode;
pub use effect::{
    COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_PARAMETER_COUNT, COLOR_CURVE_WHITE_BASIS_POINTS,
    COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT, COLOR_CURVES_PARAMETER_COUNT,
    COLOR_NODE_BYPASS_PARAMETER, COLOR_NODE_LIMIT_PER_LAYER, COLOR_WHEEL_GAIN_MAX_THOUSANDTHS,
    COLOR_WHEEL_GAIN_MIN_THOUSANDTHS, COLOR_WHEEL_GAMMA_MAX_THOUSANDTHS,
    COLOR_WHEEL_GAMMA_MIN_THOUSANDTHS, COLOR_WHEEL_LIFT_MAX_BASIS_POINTS,
    COLOR_WHEEL_LIFT_MIN_BASIS_POINTS, COLOR_WHEEL_UNITY_THOUSANDTHS,
    COLOR_WHEELS_DESCRIPTOR_PARAMETER_COUNT, CREATIVE_LOOK_DESCRIPTOR_PARAMETER_COUNT,
    ColorCurveChannel, ColorCurveOrderViolation, ColorNodeInactiveReason, ColorNodeKind,
    ColorStage, ColorStageOrderViolation, ColorWheelChannel, ColorWheelControl,
    ColorWheelControlSet, ColorWheelsParams, CurvePoints, EFFECT_DESCRIPTORS,
    EffectCompatibilityStage, EffectDescriptor, EffectParameterDescriptor, EffectUniform,
    LEGACY_DISPLAY_EFFECT_NAMES, LUT_ASSET_ID_PARAMETER, LUT_INPUT_ENCODING_PARAMETER,
    LUT_INPUT_ENCODING_TOKEN_MAX, LUT_MIX_BASIS_POINTS_MAX, LUT_MIX_PARAMETER,
    LUT_NODE_LIMIT_PER_LAYER, LutNodeParams, MANAGED_COLOR_NODE_NAMES,
    MATTE_CONTROL_PARAMETER_COUNT, MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES, MATTE_LUMA_BAND,
    MATTE_MIX_BASIS_POINTS_MAX, MATTE_PARAMETER_COUNT, MATTE_SATURATION_BAND,
    MATTE_WINDOW_CENTER_MAX_BASIS_POINTS, MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
    MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS, MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
    MATTE_WINDOW_LIMIT, MATTE_WINDOW_PARAMETER_COUNT, MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES,
    MatteParams, MatteQualifierParams, MatteWindowParams, POST_PRIMARY_LUT_EFFECT_NAMES,
    PRIMARY_CORRECTION_DESCRIPTOR_PARAMETER_COUNT, ResolvedCurves, active_color_nodes,
    classify_color_node, color_curve_order_violation, color_curve_parameter_names,
    color_node_inactive_reason, color_stage_order_violation, effect_compatibility_stage,
    effect_descriptor, is_audio_effect, is_hold_only_matte_parameter, is_legacy_display_effect,
    is_lut_color_node, is_managed_color_node, is_matte_capable_color_node, is_matte_parameter,
    lut_node_count, lut_node_may_be_active, managed_color_node_count, matte_capable,
    matte_parameter_names, matte_parameters, matte_window_parameter_names, matte_window_parameters,
};
pub use journal::JournalCommand;
pub use media::{
    Analysis, AnalysisJobStatus, AnalysisKind, AnalysisPhase, AssetBeats, AssetSceneChanges,
    AssetSilences, AssetTranscript, AudioLoudness, BeatMarker, BeatStatus, Export,
    ExportCancellation, ExportLutPreflightIssue, ExportLutPreflightReport,
    ExportMediaPreflightIssue, ExportMediaPreflightReport, ExportProgress, ExportSettings,
    FrameTexture, LinearRgbaImage, LutAvailabilityKind, LutAvailabilityStatus,
    MATTE_COVERAGE_ENCODING, MATTE_COVERAGE_HISTOGRAM_BUCKETS, MATTE_COVERAGE_SCALE,
    MatteCoverageError, MatteCoverageStatistics, MatteProof, MatteProofError, MatteProofMetadata,
    MediaAvailabilityKind, MediaAvailabilityStatus, MediaCacheClearResult, MediaCacheFamily,
    MediaCacheFamilyStatus, MediaCacheInventory, MediaError, MediaEvent, MonitorProof,
    MonitorProofMetadata, MonitorProofRenderKind, Playback, PlaybackState, ProgressSink, RgbaImage,
    SceneChange, SceneStatus, SilenceSpan, SilenceStatus, ThumbnailFrame, ThumbnailKey,
    TimelineBeat, TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord,
    TranscriptStatus, TranscriptWord, VisualAssetResult, VisualRequestKind, WORKING_PROOF_ENCODING,
    WORKING_PROOF_STAGE, WaveformData, WaveformPeak, WorkingProof, WorkingProofMetadata,
    export_lut_preflight_with, export_media_preflight, matte_coverage_statistics,
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
    plan_subject_reframe_basis_points_with_containment, stabilize_tracked_centres_basis_points,
};
pub use operation::{ApplyOp, BatchError, OpError, Operation, apply_batch};
pub use qa::{QaIssue, QaReport, QaSeverity, qa_document};
pub use scopes::{
    ChannelStatistics, ChannelStatisticsDelta, ClippingBasisPoints, ClippingDelta, LumaWaveform,
    MATTE_SCOPE_THRESHOLD, MatteRegionDescription, NormalizedRoi, NormalizedRoiError,
    ParadeChannel, PixelRoi, RgbParade, RgbParadeDelta, SCOPE_BASIS_POINTS, SCOPE_HIGH_CLIP_CODE,
    SCOPE_LOW_CLIP_CODE, SCOPE_MAX_HISTOGRAM_BINS, SCOPE_MAX_TEMPORAL_FRAMES,
    SCOPE_MAX_VECTORSCOPE_SIZE, SCOPE_MAX_WAVEFORM_COLUMNS, SCOPE_MAX_WAVEFORM_ROWS,
    SCOPE_MEAN_SCALE, SCOPE_SAMPLE_SCALE, ScopeClipping, ScopeClippingDelta, ScopeComparison,
    ScopeComparisonError, ScopeError, ScopeEvidence, ScopeFrame, ScopeFrameInput, ScopeGridDelta,
    ScopeHistogramDelta, ScopeHistograms, ScopeMeasurementMetadata, ScopePipelineStage,
    ScopeRasterResolution, ScopeRequest, ScopeResolution, ScopeStage, ScopeStatistics,
    ScopeStatisticsDelta, SignedDelta, VectorscopeDensity, compare_scope_evidence, compare_scopes,
    matte_scoped_frame, measure_scope, measure_scopes,
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
