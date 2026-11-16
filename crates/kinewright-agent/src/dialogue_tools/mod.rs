//! Bound dialogue evidence and the (not yet served) measured dialogue tools.
mod budget;
mod check;
mod cut;
mod retention;
mod transcript;
pub(crate) use budget::bound as bound_check;
pub(crate) use check::{CheckArgs, Detail as CheckDetail, PlayedSpan, check, check_created, spans};
pub(crate) use cut::{CutArgs, Outcome, plan};
use kinewright_core::{
    MediaAsset,
    dialogue::{Dialogue, Settings, SoundProfile, Word},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Write as _,
    sync::{Arc, RwLock},
};
pub(crate) use transcript::{neighbours, word_frames};

/// Flip only when the Media adapter supplies real, stream-bound measurements.
pub const SERVE_DIALOGUE_TOOLS: bool = false;
pub(crate) const GUIDE: &str = include_str!("guide.md");
pub(crate) const TOOL_NAMES: &[&str] = &["cut_dialogue", "check_dialogue_cut"];
pub(crate) type ProviderSlot = Arc<RwLock<Option<Arc<dyn DialogueMeasurements>>>>;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaIdentity {
    pub sha256: String,
    pub byte_length: u64,
}
impl MediaIdentity {
    pub(crate) fn of(asset: &MediaAsset) -> Result<Self, Refusal> {
        let sha256 = asset
            .source_fingerprint
            .content_sha256
            .as_deref()
            .ok_or(Refusal::Identity)?;
        let byte_length = asset.source_fingerprint.byte_len.ok_or(Refusal::Identity)?;
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(Refusal::Identity);
        }
        Ok(Self {
            sha256: sha256.into(),
            byte_length,
        })
    }
}

struct Digest(kinewright_hash::Sha256);
impl std::io::Write for Digest {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An immutable cache value: word indices, profile and recogniser provenance
/// belong to this one content/stream. The server shares it, never copies PCM or hops.
pub struct BoundDialogue {
    identity: MediaIdentity,
    stream: u32,
    recogniser_key: String,
    dialogue: Dialogue,
    binding: String,
}
impl BoundDialogue {
    /// # Errors
    /// Refuses missing provenance, a non-10-ms profile or invalid words/settings.
    pub fn new(
        identity: MediaIdentity,
        stream: u32,
        recogniser_key: String,
        profile: SoundProfile,
        words: Vec<Word>,
        settings: Settings,
    ) -> Result<Self, Refusal> {
        if identity.sha256.len() != 64
            || !identity
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || recogniser_key.is_empty()
            || recogniser_key.len() > 256
            || profile.hop_micros() != 10_000
        {
            return Err(Refusal::NotMeasured);
        }
        // Hash the actual value, not just its model label: remeasurement or a
        // changed recogniser result invalidates addresses even at one doc revision.
        let mut digest = Digest(kinewright_hash::Sha256::new());
        serde_json::to_writer(
            &mut digest,
            &(
                &identity,
                stream,
                &recogniser_key,
                &profile,
                &words,
                &settings,
            ),
        )
        .map_err(|_| Refusal::NotMeasured)?;
        let mut binding = String::with_capacity(64);
        for b in digest.0.finalize() {
            write!(binding, "{b:02x}").expect("String write");
        }
        let dialogue = Dialogue::new(profile, words, settings).map_err(|error| match error {
            kinewright_core::dialogue::InputError::NoiseSeparation {
                speech_rms_hundredths,
                noise_floor_hundredths,
                quiet_rms_hundredths,
            } => Refusal::NoiseSeparation {
                speech_rms_hundredths,
                noise_floor_hundredths,
                quiet_rms_hundredths,
            },
            kinewright_core::dialogue::InputError::SpeechLevelUnavailable => {
                Refusal::SpeechLevelUnavailable
            }
            _ => Refusal::NotMeasured,
        })?;
        Ok(Self {
            identity,
            stream,
            recogniser_key,
            dialogue,
            binding,
        })
    }
    #[must_use]
    pub fn identity(&self) -> &MediaIdentity {
        &self.identity
    }
    #[must_use]
    pub fn stream(&self) -> u32 {
        self.stream
    }
    #[must_use]
    pub fn recogniser_key(&self) -> &str {
        &self.recogniser_key
    }
    #[must_use]
    pub fn binding(&self) -> &str {
        &self.binding
    }
    #[must_use]
    pub fn dialogue(&self) -> &Dialogue {
        &self.dialogue
    }
}

/// The adapter chooses the same audio stream the clip plays. It returns one
/// immutable, content-bound recogniser/profile cache value, on source time zero.
/// No IO, media paths, or provider commands are performed by the tools themselves.
pub trait DialogueMeasurements: Send + Sync {
    /// # Errors
    /// Returns a typed absence if the asset has no selected/measured audio stream.
    fn audio_stream(&self, identity: &MediaIdentity) -> Result<u32, Refusal>;
    /// # Errors
    /// Returns typed absence while measurement is unavailable. Never fabricate quiet.
    fn get(&self, identity: &MediaIdentity, stream: u32) -> Result<Arc<BoundDialogue>, Refusal>;
}

#[derive(Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Refusal {
    #[error("member conflicts with earlier member {other_index}: {cause}")]
    Conflict {
        other_index: usize,
        cause: ConflictCause,
    },
    #[error("recording has no measured speech level; no edge was guessed")]
    SpeechLevelUnavailable,
    #[error(
        "speech level {speech_rms_hundredths} and floor {noise_floor_hundredths} centidBFS are too close to quiet threshold {quiet_rms_hundredths}; use a cleaner recording"
    )]
    NoiseSeparation {
        speech_rms_hundredths: i16,
        noise_floor_hundredths: i16,
        quiet_rms_hundredths: i16,
    },
    #[error("media content identity is missing or differs from the bound measurement")]
    Identity,
    #[error("audio stream differs from the bound measurement")]
    Stream,
    #[error("dialogue has not been measured")]
    NotMeasured,
    #[error("clip {clip} is retimed; only unit-speed dialogue is checked")]
    Retimed { clip: u64 },
    #[error("clip {clip} is unavailable or not audible dialogue")]
    Clip { clip: u64 },
    #[error("clip {clip} has an unproven enabled-run origin; renderer conformance is required")]
    UnprovenSegment { clip: u64 },
    #[error("clip {clip}: {why}; {lost_micros} microseconds would be lost")]
    UnselectedAudio {
        clip: u64,
        lost_micros: u64,
        why: String,
    },
    #[error(
        "clip {clip}: {why}; {changed_micros} microseconds outside the requested cut would change"
    )]
    UnmeasuredExtent {
        clip: u64,
        changed_micros: u64,
        why: String,
    },
    #[error(
        "clip {clip}: {why}; {changed_micros} microseconds outside the requested cut would change"
    )]
    UnprovenExtent {
        clip: u64,
        changed_micros: u64,
        why: String,
    },
    #[error(
        "heard removal is {actual_micros} microseconds; at least {minimum_micros} are required"
    )]
    MinimumCut {
        actual_micros: u64,
        minimum_micros: u64,
    },
    #[error("word address is outside the current bound recording")]
    Word,
    #[error("the recording's bound words/profile changed; read its transcript again")]
    BindingChanged,
    #[error("timeline revision changed before resolving dialogue")]
    Revision,
    #[error("invalid dialogue request: {why:.128}")]
    Request {
        #[serde(serialize_with = "serialize_reason")]
        why: String,
    },
    #[error("acoustic guard refused at {at_us} microseconds: {reason}")]
    Acoustic {
        reason: String,
        at_us: u64,
        quiet_us: [Option<u64>; 2],
    },
}

impl Refusal {
    fn extent_change_micros(&self) -> Option<u64> {
        match self {
            Self::UnselectedAudio { lost_micros, .. } => Some(*lost_micros),
            Self::UnmeasuredExtent { changed_micros, .. }
            | Self::UnprovenExtent { changed_micros, .. } => Some(*changed_micros),
            _ => None,
        }
    }
}

impl From<kinewright_core::dialogue::Refusal> for Refusal {
    fn from(refusal: kinewright_core::dialogue::Refusal) -> Self {
        let reason = short_reason(refusal.reason.to_string());
        Self::Acoustic {
            reason,
            at_us: refusal.at_micros,
            quiet_us: [refusal.quiet_before, refusal.quiet_after],
        }
    }
}

pub(crate) fn bound(
    provider: &dyn DialogueMeasurements,
    asset: &MediaAsset,
) -> Result<Arc<BoundDialogue>, Refusal> {
    let identity = MediaIdentity::of(asset)?;
    let stream = provider.audio_stream(&identity)?;
    let value = provider.get(&identity, stream)?;
    if value.identity != identity {
        return Err(Refusal::Identity);
    }
    if value.stream != stream {
        return Err(Refusal::Stream);
    }
    Ok(value)
}
pub(crate) fn request(why: impl Into<String>) -> Refusal {
    Refusal::Request {
        why: short_reason(why.into()),
    }
}
pub(crate) fn clip_end(
    doc: &kinewright_core::Document,
    clip: &kinewright_core::Clip,
) -> Result<kinewright_core::TimeCode, Refusal> {
    let duration = doc
        .clip_duration(clip)
        .map_err(|e| request(e.to_string()))?;
    clip.timeline_start
        .checked_add(duration)
        .ok_or_else(|| request("timeline overflow"))
}

// Pin one immutable value per content within a call, even if two asset IDs
// name it or the recogniser cache publishes a replacement during resolution.
pub(crate) fn cached<K: Ord>(
    provider: &dyn DialogueMeasurements,
    asset: &MediaAsset,
    values: &std::collections::BTreeMap<K, Arc<BoundDialogue>>,
) -> Result<Arc<BoundDialogue>, Refusal> {
    let identity = MediaIdentity::of(asset)?;
    if let Some(value) = values.values().find(|v| v.identity == identity) {
        return Ok(Arc::clone(value));
    }
    bound(provider, asset)
}

fn reason_cut(text: &str) -> usize {
    if text.len() <= 128 {
        return text.len();
    }
    let mut end = 125;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}
fn short_reason(mut text: String) -> String {
    if text.len() > 128 {
        text.truncate(reason_cut(&text));
        text.push_str("...");
    }
    text
}
fn serialize_reason<S: serde::Serializer>(text: &str, serializer: S) -> Result<S::Ok, S::Error> {
    if text.len() <= 128 {
        return serializer.serialize_str(text);
    }
    // Only the bounded wire prefix is copied, not an arbitrary provider error.
    let mut short = String::from(&text[..reason_cut(text)]);
    short.push_str("...");
    serializer.serialize_str(&short)
}

#[cfg(test)]
pub(crate) mod cost {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static TRIALS: AtomicU64 = AtomicU64::new(0);
    static OPERATIONS: AtomicU64 = AtomicU64::new(0);
    pub(crate) fn begin() {
        assert!(!ACTIVE.swap(true, Ordering::Relaxed));
        TRIALS.store(0, Ordering::Relaxed);
        OPERATIONS.store(0, Ordering::Relaxed);
    }
    pub(crate) fn trial() {
        if ACTIVE.load(Ordering::Relaxed) {
            TRIALS.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn operation() {
        if ACTIVE.load(Ordering::Relaxed) {
            OPERATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn end() -> [u64; 2] {
        ACTIVE.store(false, Ordering::Relaxed);
        [
            TRIALS.load(Ordering::Relaxed),
            OPERATIONS.load(Ordering::Relaxed),
        ]
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ConflictCause {
    #[error("overlapping removals")]
    Overlap,
    #[error("a member word address would be removed")]
    AddressRemoved,
}
