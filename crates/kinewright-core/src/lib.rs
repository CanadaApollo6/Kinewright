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
    ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer, ColorWhitePoint,
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
    EFFECT_DESCRIPTORS, EffectDescriptor, EffectParameterDescriptor, EffectUniform,
    effect_descriptor, is_audio_effect,
};
pub use journal::JournalCommand;
pub use media::{
    Analysis, AnalysisJobStatus, AnalysisKind, AnalysisPhase, AssetBeats, AssetSceneChanges,
    AssetSilences, AssetTranscript, AudioLoudness, BeatMarker, BeatStatus, Export,
    ExportCancellation, ExportProgress, ExportSettings, FrameTexture, MediaError, MediaEvent,
    Playback, PlaybackState, ProgressSink, RgbaImage, SceneChange, SceneStatus, SilenceSpan,
    SilenceStatus, ThumbnailFrame, ThumbnailKey, TimelineBeat, TimelineSceneChange,
    TimelineSilenceSpan, TimelineTranscriptWord, TranscriptStatus, TranscriptWord,
    VisualAssetResult, VisualRequestKind, WaveformData, WaveformPeak,
};
pub use model::{
    AssetId, AudioBus, AudioBusId, AudioMix, BinId, Clip, ClipContent, ClipId, Document, Effect,
    EffectId, FreezeFrame, LinkId, MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset,
    MediaBin, MediaCatalog, MediaKind, ParamValue, SourceSelect, StringOut, StringOutId, SyncGroup,
    SyncGroupId, SyncGroupMember, Track, TrackId, TrackKind, Transition, clip_effective_fps,
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
