//! Pure project state, edit operations, history, and subsystem contracts.

mod actor;
mod agent;
mod captions;
mod effect;
mod journal;
mod media;
mod model;
mod operation;
mod time;
mod title;
mod transcript_edit;
mod transition;

pub use actor::{Command, Core, CoreDisconnected, Event, Query, QueryResult, TimelineRevision};
pub use agent::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
pub use captions::{CaptionCue, caption_cues, srt, vtt};
pub use effect::{
    EFFECT_DESCRIPTORS, EffectDescriptor, EffectParameterDescriptor, EffectUniform,
    effect_descriptor,
};
pub use journal::JournalCommand;
pub use media::{
    Analysis, AnalysisJobStatus, AnalysisKind, AnalysisPhase, AssetBeats, AssetSceneChanges,
    AssetSilences, AssetTranscript, BeatMarker, BeatStatus, Export, ExportCancellation,
    ExportProgress, ExportSettings, FrameTexture, MediaError, MediaEvent, Playback, PlaybackState,
    ProgressSink, RgbaImage, SceneChange, SceneStatus, SilenceSpan, SilenceStatus, ThumbnailFrame,
    ThumbnailKey, TimelineBeat, TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord,
    TranscriptStatus, TranscriptWord, VisualAssetResult, VisualRequestKind, WaveformData,
    WaveformPeak,
};
pub use model::{
    AssetId, Clip, ClipContent, ClipId, Document, Effect, EffectId, FreezeFrame, LinkId,
    MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset, MediaKind, ParamValue, Track, TrackId,
    TrackKind, Transition, clip_effective_fps,
};
pub use operation::{ApplyOp, BatchError, OpError, Operation, apply_batch};
pub use time::{
    FrameRounding, Rational, TimeCode, TimeMappingError, map_frames, map_frames_with_rounding,
    map_source_range_to_project, speed_scaled_fps,
};
pub use title::{
    TITLE_COLORS, TITLE_FONT_SIZES, TITLE_PARAMETER_DESCRIPTORS, Title, TitleColorDescriptor,
    TitleFontSizeDescriptor, TitleParameterDescriptor, TitleParameterKind, TitlePosition,
    title_color, title_font_size, title_parameter_descriptor, title_parameter_value,
};
pub use transcript_edit::{
    TranscriptCutRange, is_filler_word, silence_cut_margin_frames, transcript_cut_ranges,
    transcript_cut_ranges_for_indices,
};
pub use transition::{
    TRANSITION_DESCRIPTORS, TransitionDescriptor, TransitionShading, transition_descriptor,
};
