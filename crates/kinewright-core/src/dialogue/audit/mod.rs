// Derived from Scarlet dialogue-leftovers.ts, gap-sound.ts and timeline-perception.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
//! Pure evidence about a speed-one playlist. Guard flags and quality warnings
//! are independent; no item is permission to apply an edit.
mod gaps;
mod joins;
mod leftovers;
mod measured;

use super::{AudibleSide, Dialogue, SourceRange};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Caller-bound recording identity, not a project or pathname identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordingId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptRange {
    pub recording: RecordingId,
    pub source: SourceRange,
}

/// The original recording and word index. Serialized as [recording, index].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct WordKey(pub RecordingId, pub usize);

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Text {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct NamedWord {
    pub key: WordKey,
    #[serde(flatten)]
    pub name: Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Location {
    pub recording: RecordingId,
    /// Half-open source and playback intervals, in microseconds.
    pub source_us: [u64; 2],
    pub play_us: [u64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditSettings {
    pub level_window_us: u64,
    pub voice_window_us: u64,
    pub min_air_after_us: u64,
    pub min_air_before_us: u64,
    pub room_tone_jump_cdb: u16,
    pub audible_room_tone_cdb: i16,
    pub stretched_word_air_us: u64,
    pub word_margin_us: u64,
    pub quiet_rms_cdb: i16,
    pub quiet_peak_cdb: i16,
    pub kept_pause_us: u64,
    pub repeat_within_us: u64,
    pub repeat_lookahead_us: u64,
    pub short_repeat_words: usize,
    pub gap_us: u64,
    pub speaking_percentile: u8,
    pub below_speech_cdb: u16,
    pub quietest_sound_cdb: i16,
    pub min_sound_run_us: u64,
    pub word_edge_us: u64,
    pub neighbor_us: u64,
    pub max_neighbor_gap_us: u64,
    pub max_phrase_us: u64,
    pub quiet_phrase_contrast_cdb: u16,
    pub max_items: usize,
    pub max_sound_runs: usize,
    pub max_word_bytes: usize,
    pub max_excerpt_bytes: usize,
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            level_window_us: 60_000,
            voice_window_us: 20_000,
            min_air_after_us: 60_000,
            min_air_before_us: 40_000,
            room_tone_jump_cdb: 600,
            audible_room_tone_cdb: -7_000,
            stretched_word_air_us: 100_000,
            word_margin_us: 30_000,
            quiet_rms_cdb: -4_001,
            quiet_peak_cdb: -2_603,
            kept_pause_us: 750_000,
            repeat_within_us: 3_000_000,
            repeat_lookahead_us: 25_000_000,
            short_repeat_words: 5,
            gap_us: 400_000,
            speaking_percentile: 85,
            below_speech_cdb: 1_200,
            quietest_sound_cdb: -4_500,
            min_sound_run_us: 100_000,
            word_edge_us: 50_000,
            neighbor_us: 2_000_000,
            max_neighbor_gap_us: 12_000_000,
            max_phrase_us: 8_000_000,
            quiet_phrase_contrast_cdb: 1_800,
            max_items: 64,
            max_sound_runs: 4,
            max_word_bytes: 96,
            max_excerpt_bytes: 256,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum AuditError {
    #[error("invalid audit thresholds or output caps")]
    Settings,
    #[error("kept range {index} names an unavailable recording")]
    Recording { index: usize },
    #[error("kept range {index} is empty, reversed or outside its recording")]
    Range { index: usize },
    #[error("playlist duration cannot be represented")]
    TimeOverflow,
}

pub use gaps::{Hole, QuietPhrase};
pub use joins::{EdgeInSound, GuardIssue, Join, Level, RoomTone, Side};
pub use leftovers::{Pause, Repeat};

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Join(Join),
    Pause(Pause),
    Repeat(Repeat),
    Hole(Hole),
    QuietPhrase(QuietPhrase),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub joins: u64,
    pub edge_in_sound: u64,
    pub short_air: u64,
    pub room_tone_jumps: u64,
    pub dip_intrusions: u64,
    pub pauses: u64,
    pub repeats: u64,
    pub holes: u64,
    pub sounding_holes: u64,
    pub quiet_phrases: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub duration_us: u64,
    /// Audit policy snapshot; each input Dialogue retains its own guard policy.
    pub settings: AuditSettings,
    pub words: Vec<NamedWord>,
    pub items: Vec<Item>,
    pub total: Counts,
    pub omitted: Counts,
}

#[derive(Debug, Clone, Copy)]
struct Piece {
    recording: RecordingId,
    source: SourceRange,
    play: u64,
}
impl Piece {
    fn location(self, start: u64, end: u64) -> Location {
        Location {
            recording: self.recording,
            source_us: [start, end],
            play_us: [
                self.play + (start - self.source.start_micros),
                self.play + (end - self.source.start_micros),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PlayedWord {
    key: WordKey,
    start: u64,
    end: u64,
    piece: usize,
    source: [u64; 2],
}

impl Counts {
    fn add(&mut self, item: &Item) {
        match item {
            Item::Join(j) => {
                self.joins += 1;
                self.edge_in_sound +=
                    j.sides.iter().filter(|s| s.edge_in_sound.is_some()).count() as u64;
                self.dip_intrusions +=
                    j.sides.iter().filter(|s| s.dip_intrusion.is_some()).count() as u64;
                self.short_air += j.sides.iter().filter(|s| s.short_air).count() as u64;
                self.room_tone_jumps += u64::from(j.room_tone.as_ref().is_some_and(|r| r.flagged));
            }
            Item::Pause(_) => self.pauses += 1,
            Item::Repeat(_) => self.repeats += 1,
            Item::Hole(h) => {
                self.holes += 1;
                self.sounding_holes += u64::from(h.sound_us > 0);
            }
            Item::QuietPhrase(_) => self.quiet_phrases += 1,
        }
    }
    fn minus(self, other: Self) -> Self {
        Self {
            joins: self.joins - other.joins,
            edge_in_sound: self.edge_in_sound - other.edge_in_sound,
            short_air: self.short_air - other.short_air,
            room_tone_jumps: self.room_tone_jumps - other.room_tone_jumps,
            dip_intrusions: self.dip_intrusions - other.dip_intrusions,
            pauses: self.pauses - other.pauses,
            repeats: self.repeats - other.repeats,
            holes: self.holes - other.holes,
            sounding_holes: self.sounding_holes - other.sounding_holes,
            quiet_phrases: self.quiet_phrases - other.quiet_phrases,
        }
    }
}

impl Item {
    fn order(&self) -> (u8, u64) {
        match self {
            Self::Join(j) => (
                if j.sides.iter().any(|s| s.edge_in_sound.is_some()) {
                    0
                } else if j
                    .sides
                    .iter()
                    .any(|s| s.short_air || s.dip_intrusion.is_some())
                    || j.room_tone.as_ref().is_some_and(|r| r.flagged)
                {
                    1
                } else {
                    2
                },
                j.at_us,
            ),
            Self::Pause(p) => (1, p.play_us[0]),
            Self::Repeat(r) => (1, r.play_us[0]),
            Self::Hole(h) => (if h.sound_us > 0 { 1 } else { 2 }, h.location.play_us[0]),
            Self::QuietPhrase(p) => (1, p.location.play_us[0]),
        }
    }
    fn word_keys(&self, keys: &mut BTreeSet<WordKey>) {
        match self {
            Self::Join(j) => {
                for s in &j.sides {
                    keys.extend(s.word);
                    keys.extend(s.dip_intrusion.map(|dip| WordKey(s.source.0, dip.word)));
                    keys.extend(s.timed_word);
                    keys.extend(s.stretched_word);
                    if let Some(flag) = &s.edge_in_sound {
                        keys.extend(flag.word);
                    }
                }
            }
            Self::Pause(p) => {
                keys.insert(p.after);
                keys.insert(p.before);
            }
            Self::Repeat(r) => {
                keys.insert(r.from_word);
                keys.insert(r.to_word);
                keys.insert(r.second_word);
            }
            Self::Hole(h) => {
                keys.extend(h.after);
                keys.extend(h.before);
            }
            Self::QuietPhrase(p) => {
                keys.insert(p.from_word);
                keys.insert(p.to_word);
            }
        }
    }
}

struct Collector {
    items: Vec<Item>,
    total: Counts,
    cap: usize,
}
impl Collector {
    fn push(&mut self, item: Item) {
        self.total.add(&item);
        let at = self
            .items
            .partition_point(|old| old.order() <= item.order());
        if at >= self.cap {
            return;
        }
        if self.items.len() == self.cap {
            self.items.pop();
        }
        self.items.insert(at, item);
    }
}

/// Minimal comparison rule, independent of any recogniser: Unicode lowercase
/// alphanumerics, ignoring punctuation and capitals.
#[must_use]
pub fn comparison_key(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn text_append(out: &mut String, text: &str, limit: usize) -> bool {
    let room = limit.saturating_sub(out.len());
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&text[..end]);
    end == text.len()
}
fn excerpt(text: &str, limit: usize) -> Text {
    let mut out = String::new();
    let complete = text_append(&mut out, text, limit);
    Text {
        text: out,
        truncated: !complete,
    }
}

fn validate(settings: &AuditSettings) -> Result<(), AuditError> {
    if [
        settings.quiet_rms_cdb,
        settings.quiet_peak_cdb,
        settings.audible_room_tone_cdb,
        settings.quietest_sound_cdb,
    ]
    .iter()
    .any(|&v| !(super::LEVEL_FLOOR..=0).contains(&v))
        || settings.quiet_rms_cdb > settings.quiet_peak_cdb
        || settings.level_window_us == 0
        || settings.voice_window_us == 0
        || settings.kept_pause_us == 0
        || settings.gap_us == 0
        || settings.min_sound_run_us == 0
        || settings.short_repeat_words == 0
        || settings.speaking_percentile > 100
        || settings.max_word_bytes == 0
        || settings.max_excerpt_bytes == 0
        || settings.max_sound_runs == 0
    {
        return Err(AuditError::Settings);
    }
    Ok(())
}

/// Audit a linear, speed-one cut without editing it. Source-contiguous splits
/// coalesce. Word and phrase text in this owned report is copied intentionally
/// so the caller may retain it after releasing the prepared recordings.
///
/// # Errors
/// Refuses invalid settings, missing recordings, out-of-range intervals and
/// an unrepresentable playback duration. No partial report guesses missing data.
pub fn audit(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    cut: &[KeptRange],
    settings: &AuditSettings,
    key: impl Fn(&str) -> String,
) -> Result<Report, AuditError> {
    validate(settings)?;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut duration = 0_u64;
    for (index, range) in cut.iter().enumerate() {
        let recording = recordings
            .get(&range.recording)
            .ok_or(AuditError::Recording { index })?;
        let r = range.source;
        if r.start_micros >= r.end_micros || r.end_micros > recording.profile().duration_micros() {
            return Err(AuditError::Range { index });
        }
        let next = duration
            .checked_add(r.end_micros - r.start_micros)
            .ok_or(AuditError::TimeOverflow)?;
        if let Some(last) = pieces
            .last_mut()
            .filter(|p| p.recording == range.recording && p.source.end_micros == r.start_micros)
        {
            last.source.end_micros = r.end_micros;
        } else {
            pieces.push(Piece {
                recording: range.recording,
                source: r,
                play: duration,
            });
        }
        duration = next;
    }
    let mut collector = Collector {
        items: Vec::new(),
        total: Counts::default(),
        cap: settings.max_items,
    };
    let used: BTreeSet<_> = pieces.iter().map(|p| p.recording).collect();
    let tokens: BTreeMap<_, Vec<_>> = used
        .iter()
        .map(|&id| {
            (
                id,
                recordings[&id]
                    .words()
                    .iter()
                    .map(|w| key(&w.text))
                    .collect(),
            )
        })
        .collect();
    let played = leftovers::played(recordings, &pieces, &tokens);
    joins::inspect(recordings, &pieces, settings, &mut collector);
    leftovers::inspect(recordings, &played, &tokens, settings, &mut collector);
    gaps::inspect(recordings, &pieces, &played, settings, &mut collector);
    let mut keys = BTreeSet::new();
    let mut listed = Counts::default();
    for item in &collector.items {
        item.word_keys(&mut keys);
        listed.add(item);
    }
    let words = keys
        .into_iter()
        .map(|key| NamedWord {
            key,
            name: excerpt(
                recordings[&key.0].words()[key.1].text.trim(),
                settings.max_word_bytes,
            ),
        })
        .collect();
    Ok(Report {
        duration_us: duration,
        settings: *settings,
        words,
        items: collector.items,
        total: collector.total,
        omitted: collector.total.minus(listed),
    })
}

#[cfg(test)]
mod tests;
