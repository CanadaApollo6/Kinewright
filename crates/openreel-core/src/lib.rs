//! Pure project state, edit operations, history, and subsystem contracts.

mod actor;
mod agent;
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
pub use journal::JournalCommand;
pub use media::{
    AssetTranscript, ExportCancellation, ExportProgress, ExportSettings, FrameTexture, MediaEngine,
    MediaError, MediaEvent, PlaybackState, ProgressSink, RgbaImage, TimelineTranscriptWord,
    TranscriptStatus, TranscriptWord,
};
pub use model::{
    AssetId, Clip, ClipId, Document, Effect, EffectId, MediaAsset, MediaKind, ParamValue, Track,
    TrackId, TrackKind, Transition,
};
pub use operation::{ApplyOp, OpError, Operation};
pub use time::{
    FrameRounding, Rational, TimeCode, TimeMappingError, map_frames,
    map_frames_with_rounding, map_source_range_to_project,
};

