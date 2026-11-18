use std::ops::Range;

use kinewright_core::{
    Fidelity, InterchangeFormat, LossBound, LossCode, MediaKind, Rational, TrackKind,
};

use crate::{MAX_TEXT_BYTES, Refusal, Time, shorten};

#[derive(Debug, Default, Clone, Copy)]
pub struct ParseOptions<'a> {
    pub timeline: Option<&'a str>,
    pub project_fps: Option<Rational>,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub format: InterchangeFormat,
    pub version: String,
    pub name: String,
    pub sha256: String,
}

impl Source {
    #[must_use]
    pub fn new(format: InterchangeFormat, version: &str, name: &str, bytes: &[u8]) -> Self {
        let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
        Self {
            format,
            version: shorten(version, 16).into(),
            name: match name {
                "" | "." | ".." => match format {
                    InterchangeFormat::Otio => "timeline.otio",
                    InterchangeFormat::Fcpxml => "timeline.fcpxml",
                },
                name => shorten(name, 128),
            }
            .into(),
            sha256: kinewright_core::ImportRecord::hash_source(bytes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub source: Source,
    pub name: String,
    pub start: Time,
    pub rate_hint: Rational,
    pub resolution_hint: Option<(u32, u32)>,
    pub root: Stack,
    pub media: Vec<Media>,
    pub notes: Vec<Loss>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stack {
    pub tracks: Vec<Track>,
    pub window: Option<Range<Time>>,
    pub time: ContainerTime,
    pub decorations: Decorations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub kind: TrackKind,
    pub name: Option<String>,
    pub items: Vec<Placed>,
    pub decorations: Decorations,
}

impl Track {
    #[must_use]
    pub const fn new(kind: TrackKind) -> Self {
        Self {
            kind,
            name: None,
            items: Vec::new(),
            decorations: Decorations::empty(),
        }
    }

    #[must_use]
    pub fn population(&self) -> u32 {
        self.decorations.population() + self.items.iter().map(Placed::population).sum::<u32>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub at: Time,
    pub item: Item,
    pub site: String,
    pub notes: Vec<Loss>,
    pub decorations: Decorations,
    /// A separate A/V part can share the original source item's accounting site.
    pub counts_item: bool,
    /// FCPXML connected clips, with placements in this track's coordinate system.
    pub anchors: Vec<Anchor>,
}

impl Placed {
    #[must_use]
    pub fn new(at: Time, item: Item, site: impl Into<String>) -> Self {
        Self {
            at,
            item,
            site: site.into(),
            notes: Vec::new(),
            decorations: Decorations::empty(),
            counts_item: true,
            anchors: Vec::new(),
        }
    }

    #[must_use]
    pub fn population(&self) -> u32 {
        if !self.counts_item {
            return 0;
        }
        self.item.population()
            + self.decorations.population()
            + self
                .anchors
                .iter()
                .map(|anchor| anchor.track.population())
                .sum::<u32>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub lane: i64,
    pub track: Track,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Clip(Clip),
    Gap {
        duration: Time,
    },
    Transition {
        kind: String,
        before: Time,
        after: Time,
    },
    Nested {
        stack: Stack,
        duration: Time,
    },
    Title {
        text: String,
        duration: Time,
        decorations: Decorations,
    },
    Unsupported {
        what: String,
        duration: Time,
    },
    /// Source items removed during parsing, retained as a report contribution.
    Discarded {
        count: u32,
        loss: Loss,
    },
}

impl Item {
    #[must_use]
    pub fn span(&self) -> Time {
        match self {
            Self::Clip(clip) => clip.span,
            Self::Gap { duration }
            | Self::Nested { duration, .. }
            | Self::Title { duration, .. }
            | Self::Unsupported { duration, .. } => *duration,
            Self::Transition { .. } | Self::Discarded { .. } => Time::ZERO,
        }
    }

    #[must_use]
    pub fn population(&self) -> u32 {
        match self {
            Self::Discarded { count, .. } => *count,
            Self::Clip(clip) => 1 + clip.decorations.population(),
            Self::Nested { stack, .. } => 1 + stack.population(),
            Self::Title { decorations, .. } => 1 + decorations.population(),
            _ => 1,
        }
    }
}

impl Stack {
    #[must_use]
    pub fn population(&self) -> u32 {
        self.decorations.population() + self.tracks.iter().map(Track::population).sum::<u32>()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub locator: String,
    pub name: String,
    pub kind: MediaKind,
    pub fps: Option<Rational>,
    pub start: Option<Time>,
    pub duration: Option<Time>,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub media: MediaId,
    /// Media origin is still included in this source time.
    pub source_start: Time,
    pub span: Time,
    pub time: Playback,
    pub gain_tenth_db: i64,
    pub gain_keys: Vec<GainKey>,
    pub fade_in: Time,
    pub fade_out: Time,
    pub enabled: bool,
    pub role: Option<String>,
    pub decorations: Decorations,
    pub name: Option<String>,
    pub av_group: Option<u32>,
}

impl Clip {
    #[must_use]
    pub const fn new(media: MediaId, source_start: Time, span: Time) -> Self {
        Self {
            media,
            source_start,
            span,
            time: Playback::Normal,
            gain_tenth_db: 0,
            gain_keys: Vec::new(),
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            enabled: true,
            role: None,
            decorations: Decorations::empty(),
            name: None,
            av_group: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainKey {
    pub at: Time,
    pub tenth_db: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Playback {
    #[default]
    Normal,
    Speed(Time),
    Reverse(Time),
    Freeze(Time),
    Variable(Vec<TimePoint>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimePoint {
    pub at: Time,
    pub value: Time,
}

impl Playback {
    /// # Errors
    /// Refuses a time map whose average cannot be represented.
    pub fn average(&self) -> Result<Time, Refusal> {
        match self {
            Self::Normal => Ok(Time::ONE),
            Self::Freeze(_) => Ok(Time::ZERO),
            Self::Speed(ratio) => Ok(*ratio),
            Self::Reverse(ratio) => Time::ZERO.checked_sub(*ratio),
            Self::Variable(points) => {
                let (Some(first), Some(last)) = (points.first(), points.last()) else {
                    return Ok(Time::ONE);
                };
                last.value
                    .checked_sub(first.value)?
                    .checked_div(last.at.checked_sub(first.at)?)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerTime {
    pub offset: Time,
    pub in_point: Time,
    pub speed: Time,
}

impl Default for ContainerTime {
    fn default() -> Self {
        Self {
            offset: Time::ZERO,
            in_point: Time::ZERO,
            speed: Time::ONE,
        }
    }
}

impl ContainerTime {
    /// # Errors
    /// Refuses a zero speed or an unrepresentable mapped time.
    pub fn to_parent(self, local: Time) -> Result<Time, Refusal> {
        self.offset
            .checked_add(local.checked_sub(self.in_point)?.checked_div(self.speed)?)
    }

    /// # Errors
    /// Refuses a mapping outside the exact time representation.
    pub fn in_local(self, parent: Time) -> Result<Time, Refusal> {
        self.in_point
            .checked_add(parent.checked_sub(self.offset)?.checked_mul(self.speed)?)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Decorations {
    pub markers: Vec<Marker>,
    pub effects: Vec<NamedEffect>,
    pub removed: Vec<Discarded>,
}

impl Decorations {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            markers: Vec::new(),
            effects: Vec::new(),
            removed: Vec::new(),
        }
    }

    #[must_use]
    pub fn population(&self) -> u32 {
        u32::try_from(self.markers.len() + self.effects.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.removed.iter().map(|discard| discard.count).sum())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discarded {
    pub count: u32,
    pub loss: Loss,
    pub site: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub at: Time,
    pub duration: Time,
    pub label: String,
    pub color: Option<String>,
    pub site: String,
    pub kind: MarkerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Plain,
    Chapter,
    Todo,
    KeywordOrRating,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedEffect {
    pub name: String,
    pub site: String,
    pub code: LossCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    pub code: LossCode,
    pub fidelity: Fidelity,
    pub name: Option<String>,
    pub bound: Option<LossBound>,
}

impl Loss {
    #[must_use]
    pub const fn approximation(code: LossCode) -> Self {
        Self {
            code,
            fidelity: Fidelity::Approximated,
            name: None,
            bound: None,
        }
    }

    #[must_use]
    pub const fn dropped(code: LossCode) -> Self {
        Self {
            code,
            fidelity: Fidelity::Dropped,
            name: None,
            bound: None,
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(shorten(name, MAX_TEXT_BYTES).into());
        self
    }

    #[must_use]
    pub const fn bounded(mut self, bound: LossBound) -> Self {
        self.bound = Some(bound);
        self
    }
}
