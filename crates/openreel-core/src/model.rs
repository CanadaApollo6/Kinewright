use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{OpError, Rational, TimeCode, map_source_range_to_project};

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
    /// One of `brightness`, `contrast`, `saturation`, `opacity`, or `transform`.
    pub name: String,
    /// Integer-only fixed-point parameters. Missing parameters use their neutral defaults:
    ///
    /// - brightness/contrast/saturation: `percent` in -100..=100, default 0
    /// - opacity: `percent` in 0..=100, default 100
    /// - transform: `scale_percent` in 1..=400 (default 100), plus
    ///   `x_percent` and `y_percent` in -100..=100 (default 0). Offsets are
    ///   percentages of the project width/height, positive right/down.
    pub parameters: BTreeMap<String, ParamValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// Currently `crossfade`. It fades the clip from the already-composited
    /// lower layers during the first `duration` project frames.
    pub name: String,
    /// Transition length in project frames.
    pub duration: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    /// In/out within the source, in source frames.
    pub source_range: std::ops::Range<TimeCode>,
    /// Position on the track, in project frames.
    pub timeline_start: TimeCode,
    pub effects: Vec<Effect>,
    pub transition_in: Option<Transition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    /// Video and audio tracks, ordered z-bottom to top.
    pub tracks: Vec<Track>,
    pub media_pool: Vec<MediaAsset>,
    pub fps: Rational,
    pub resolution: (u32, u32),
    pub duration: TimeCode,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            media_pool: Vec::new(),
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

    pub fn validate(&self) -> Result<(), OpError> {
        crate::operation::validate_document(self)
    }

    pub(crate) fn clip_duration(&self, clip: &Clip) -> Result<TimeCode, OpError> {
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

