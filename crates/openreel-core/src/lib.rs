//! Pure project state, edit operations, history, and subsystem contracts.

mod actor;
mod agent;
mod effect;
mod journal;
mod media;
mod model;
mod operation;
mod time;

pub use actor::{Command, Core, CoreDisconnected, Event, Query, QueryResult};
pub use agent::{
    AgentDriver, AgentError, AgentEvent, AgentSession, AuthenticationStatus, HarnessId,
    HarnessInfo, SessionConfig,
};
pub use effect::{
    EFFECT_DESCRIPTORS, EffectDescriptor, EffectParameterDescriptor, EffectUniform,
    effect_descriptor,
};
pub use journal::JournalCommand;
pub use media::{
    Analysis, AssetSceneChanges, AssetSilences, AssetTranscript, Export, ExportCancellation,
    ExportProgress, ExportSettings, FrameTexture, MediaError, MediaEvent, Playback, PlaybackState,
    ProgressSink, RgbaImage, SceneChange, SceneStatus, SilenceSpan, SilenceStatus, ThumbnailFrame,
    ThumbnailKey, TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord,
    TranscriptStatus, TranscriptWord, VisualAssetResult, VisualRequestKind, WaveformData,
    WaveformPeak,
};
pub use model::{
    AssetId, Clip, ClipId, Document, Effect, EffectId, LinkId, MARKER_COLOR_TOKEN_COUNT, Marker,
    MarkerId, MediaAsset, MediaKind, ParamValue, Track, TrackId, TrackKind, Transition,
};
pub use operation::{ApplyOp, BatchError, OpError, Operation, apply_batch};
pub use time::{
    FrameRounding, Rational, TimeCode, TimeMappingError, map_frames, map_frames_with_rounding,
    map_source_range_to_project,
};
