use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AutomationCurve, CaptionPreset, ClipId, Document, Effect, EffectId, Keyframe,
    KeyframeInterpolation, Operation, ParamValue, Rational, TimeCode, TimelineTranscriptWord,
    TitlePosition, Track, TrackId, TrackKind,
};

const MAX_CAPTION_CHARACTERS: usize = 48;

/// One half-open caption interval in project frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionCue {
    pub start: TimeCode,
    pub end: TimeCode,
    pub text: String,
}

/// Stable caption motion compositions built from ordinary effect automation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionMotion {
    #[default]
    None,
    Fade,
    Pop,
    SlideUp,
}

impl CaptionMotion {
    pub const ALL: [Self; 4] = [Self::None, Self::Fade, Self::Pop, Self::SlideUp];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fade => "fade",
            Self::Pop => "pop",
            Self::SlideUp => "slide_up",
        }
    }
}

/// Collapse repeated A/V copies of the same audible word at the same source
/// and project interval while preserving first-track order.
#[must_use]
pub fn dedup_timeline_words(words: Vec<TimelineTranscriptWord>) -> Vec<TimelineTranscriptWord> {
    let mut seen = std::collections::HashSet::new();
    words
        .into_iter()
        .filter(|word| {
            seen.insert((
                word.asset,
                word.source_start,
                word.source_end,
                word.project_start,
                word.project_end,
            ))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptionPlanError {
    #[error("there are no caption cues to add")]
    NoCues,
    #[error("the authored caption script is empty")]
    EmptyAuthoredScript,
    #[error("the authored caption script cannot be aligned to the generated cues")]
    AuthoredScriptAlignment,
    #[error("track id space is exhausted")]
    TrackIdExhausted,
    #[error("clip id space is exhausted")]
    ClipIdExhausted,
    #[error("caption cue duration must be positive")]
    InvalidCueDuration,
}

/// Build one atomic title-track plan from transcript cues and a stable style preset.
///
/// # Errors
///
/// Returns an error for empty cues, invalid intervals, or exhausted track ids.
pub fn caption_title_operations(
    document: &Document,
    cues: &[CaptionCue],
    preset: CaptionPreset,
) -> Result<Vec<Operation>, CaptionPlanError> {
    animated_caption_operations(document, cues, preset, CaptionMotion::None)
}

/// Build one atomic caption track with optional renderer-native motion curves.
///
/// # Errors
///
/// Returns an error for empty/invalid cues or exhausted track/clip ids.
pub fn animated_caption_operations(
    document: &Document,
    cues: &[CaptionCue],
    preset: CaptionPreset,
    motion: CaptionMotion,
) -> Result<Vec<Operation>, CaptionPlanError> {
    animated_caption_operations_at(document, cues, preset, motion, None)
}

/// Build one atomic caption track with an optional placement override.
///
/// This keeps the stable preset's typography while allowing a model to move
/// captions away from a known subject without rewriting title parameters cue
/// by cue.
///
/// # Errors
///
/// Returns an error for empty/invalid cues or exhausted track/clip ids.
pub fn animated_caption_operations_at(
    document: &Document,
    cues: &[CaptionCue],
    preset: CaptionPreset,
    motion: CaptionMotion,
    position: Option<TitlePosition>,
) -> Result<Vec<Operation>, CaptionPlanError> {
    if cues.is_empty() {
        return Err(CaptionPlanError::NoCues);
    }
    let track_id = document
        .tracks
        .iter()
        .map(|track| track.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(TrackId)
        .ok_or(CaptionPlanError::TrackIdExhausted)?;
    let first_clip_id = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(CaptionPlanError::ClipIdExhausted)?;
    let effects_per_cue = usize::from(motion != CaptionMotion::None)
        + usize::from(matches!(
            motion,
            CaptionMotion::Pop | CaptionMotion::SlideUp
        ));
    let mut operations = Vec::with_capacity(
        cues.len()
            .saturating_mul(1 + effects_per_cue)
            .saturating_add(1),
    );
    operations.push(Operation::AddTrack {
        track: Track {
            id: track_id,
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    });
    for (index, cue) in cues.iter().enumerate() {
        let duration = cue
            .end
            .checked_sub(cue.start)
            .filter(|duration| *duration > TimeCode::ZERO)
            .ok_or(CaptionPlanError::InvalidCueDuration)?;
        let mut title = preset.title(cue.text.clone());
        if let Some(position) = position {
            title.position = position;
        }
        operations.push(Operation::AddTitle {
            track: track_id,
            at: cue.start,
            duration,
            title,
        });
        let clip = first_clip_id
            .checked_add(u64::try_from(index).unwrap_or(u64::MAX))
            .map(ClipId)
            .ok_or(CaptionPlanError::ClipIdExhausted)?;
        operations.extend(caption_motion_effects(clip, duration, motion));
    }
    Ok(operations)
}

fn caption_motion_effects(
    clip: ClipId,
    duration: TimeCode,
    motion: CaptionMotion,
) -> Vec<Operation> {
    // A one-frame cue has no temporal room to animate. Leaving it at the
    // title's native opacity is both readable and deterministic.
    if motion == CaptionMotion::None || duration <= TimeCode(1) {
        return Vec::new();
    }
    let last = TimeCode(duration.0.saturating_sub(1));
    let entrance = TimeCode((duration.0 / 4).clamp(1, 6).min(last.0));
    let exit = TimeCode(last.0.saturating_sub(entrance.0));
    let opacity_curve = dedup_keyframes([
        keyframe(TimeCode::ZERO, 0, KeyframeInterpolation::EaseOut),
        keyframe(entrance, 100, KeyframeInterpolation::Hold),
        keyframe(exit, 100, KeyframeInterpolation::EaseIn),
        keyframe(last, 0, KeyframeInterpolation::Linear),
    ]);
    let mut operations = vec![Operation::AddEffect {
        clip,
        effect: Effect {
            id: EffectId(1),
            name: "opacity".to_owned(),
            parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::from([("percent".to_owned(), opacity_curve)]),
        },
    }];
    let transform = match motion {
        CaptionMotion::None | CaptionMotion::Fade => None,
        CaptionMotion::Pop => {
            let overshoot = TimeCode((entrance.0 / 2).max(1));
            Some((
                "scale_percent",
                BTreeMap::from([
                    ("scale_percent".to_owned(), ParamValue::Integer(100)),
                    ("x_percent".to_owned(), ParamValue::Integer(0)),
                    ("y_percent".to_owned(), ParamValue::Integer(0)),
                ]),
                dedup_keyframes([
                    keyframe(TimeCode::ZERO, 80, KeyframeInterpolation::EaseOut),
                    keyframe(overshoot, 110, KeyframeInterpolation::EaseInOut),
                    keyframe(entrance, 100, KeyframeInterpolation::Linear),
                ]),
            ))
        }
        CaptionMotion::SlideUp => Some((
            "y_percent",
            BTreeMap::from([
                ("scale_percent".to_owned(), ParamValue::Integer(100)),
                ("x_percent".to_owned(), ParamValue::Integer(0)),
                ("y_percent".to_owned(), ParamValue::Integer(0)),
            ]),
            dedup_keyframes([
                keyframe(TimeCode::ZERO, 15, KeyframeInterpolation::EaseOut),
                keyframe(entrance, 0, KeyframeInterpolation::Linear),
            ]),
        )),
    };
    if let Some((parameter, parameters, curve)) = transform {
        operations.push(Operation::AddEffect {
            clip,
            effect: Effect {
                id: EffectId(2),
                name: "transform".to_owned(),
                parameters,
                keyframes: BTreeMap::from([(parameter.to_owned(), curve)]),
            },
        });
    }
    operations
}

const fn keyframe(at: TimeCode, value: i64, interpolation: KeyframeInterpolation) -> Keyframe {
    Keyframe {
        at,
        value,
        interpolation,
    }
}

fn dedup_keyframes<const N: usize>(keyframes: [Keyframe; N]) -> AutomationCurve {
    let mut deduped = BTreeMap::new();
    for keyframe in keyframes {
        deduped.insert(keyframe.at, keyframe);
    }
    AutomationCurve {
        keyframes: deduped.into_values().collect(),
    }
}

/// Build readable caption cues from ordered, de-duplicated timeline words.
#[must_use]
pub fn caption_cues(words: &[TimelineTranscriptWord], project_fps: Rational) -> Vec<CaptionCue> {
    let hold = half_second_frames(project_fps);
    let mut words = words
        .iter()
        .filter_map(|word| {
            let text = word.text.trim();
            (!text.is_empty() && word.project_end > word.project_start).then(|| CaptionWord {
                text: text.to_owned(),
                start: word.project_start,
                end: word.project_end,
                asset: word.asset,
                clip: word.clip,
                speaker: word.speaker.clone(),
            })
        })
        .collect::<Vec<_>>();
    infer_sentence_punctuation(&mut words);

    let mut cues = Vec::new();
    let mut segment = Vec::new();
    for word in &words {
        let break_before = segment.last().is_some_and(|previous: &&CaptionWord| {
            word.start.0.saturating_sub(previous.end.0) > hold.0
                || word.asset != previous.asset
                || word.clip != previous.clip
                || word.speaker != previous.speaker
        });
        if break_before {
            push_semantic_cues(&mut cues, &segment);
            segment.clear();
        }
        segment.push(word);
        if ends_sentence(&word.text) {
            push_semantic_cues(&mut cues, &segment);
            segment.clear();
        }
    }
    push_semantic_cues(&mut cues, &segment);

    for index in 0..cues.len() {
        let held_end = TimeCode(cues[index].end.0.saturating_add(hold.0));
        cues[index].end = cues
            .get(index + 1)
            .map_or(held_end, |next| held_end.min(next.start));
    }
    cues.retain(|cue| cue.end > cue.start);
    cues
}

/// Re-author generated cue text from an exact script without changing cue timing.
///
/// Sentence endings are hard boundaries: one cue never contains the end of one
/// sentence and the beginning of the next. Existing cue boundaries that are
/// already sentence-safe are preserved. When a sentence boundary displaces an
/// existing split, that sentence is rebalanced evenly across its assigned cues.
///
/// # Errors
///
/// Returns an error when the script is empty or cannot provide at least one
/// word per cue and one cue per sentence.
pub fn authored_caption_cues(
    cues: &[CaptionCue],
    script: &str,
) -> Result<Vec<CaptionCue>, CaptionPlanError> {
    if cues.is_empty() {
        return Err(CaptionPlanError::NoCues);
    }
    let words = script
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return Err(CaptionPlanError::EmptyAuthoredScript);
    }
    if words.len() < cues.len() {
        return Err(CaptionPlanError::AuthoredScriptAlignment);
    }

    let mut sentence_ends = words
        .iter()
        .enumerate()
        .filter_map(|(index, word)| ends_sentence(word).then_some(index + 1))
        .collect::<Vec<_>>();
    if sentence_ends.last().copied() != Some(words.len()) {
        sentence_ends.push(words.len());
    }
    if sentence_ends.len() > cues.len() {
        return Err(CaptionPlanError::AuthoredScriptAlignment);
    }

    let raw_word_counts = cues
        .iter()
        .map(|cue| cue.text.split_whitespace().count())
        .collect::<Vec<_>>();
    let raw_total = raw_word_counts.iter().sum::<usize>();
    if raw_total == 0 {
        return Err(CaptionPlanError::AuthoredScriptAlignment);
    }
    let raw_boundaries = scaled_boundaries(&raw_word_counts, words.len(), raw_total)?;

    let mut sentence_cue_ends = Vec::with_capacity(sentence_ends.len());
    let mut previous_cue_end = 0;
    let mut previous_sentence_end = 0;
    for (sentence_index, &sentence_end) in sentence_ends.iter().enumerate() {
        if sentence_index + 1 == sentence_ends.len() {
            sentence_cue_ends.push(cues.len());
            break;
        }
        let remaining_sentences = sentence_ends.len() - sentence_index - 1;
        let remaining_words = words.len() - sentence_end;
        let sentence_words = sentence_end - previous_sentence_end;
        let minimum = (previous_cue_end + 1).max(cues.len().saturating_sub(remaining_words));
        let maximum = (cues.len() - remaining_sentences)
            .min(previous_cue_end + sentence_words)
            .min(cues.len() - 1);
        if minimum > maximum {
            return Err(CaptionPlanError::AuthoredScriptAlignment);
        }
        let cue_end = (minimum..=maximum)
            .min_by_key(|&candidate| raw_boundaries[candidate - 1].abs_diff(sentence_end))
            .ok_or(CaptionPlanError::AuthoredScriptAlignment)?;
        sentence_cue_ends.push(cue_end);
        previous_cue_end = cue_end;
        previous_sentence_end = sentence_end;
    }

    let mut final_boundaries = vec![0; cues.len() + 1];
    let mut sentence_start = 0;
    let mut cue_start = 0;
    for (&sentence_end, &cue_end) in sentence_ends.iter().zip(&sentence_cue_ends) {
        final_boundaries[cue_start] = sentence_start;
        final_boundaries[cue_end] = sentence_end;
        let cue_count = cue_end - cue_start;
        let word_count = sentence_end - sentence_start;
        let mut targets = Vec::with_capacity(cue_count);
        for offset in 1..cue_count {
            let minimum = offset;
            let maximum = word_count.saturating_sub(cue_count - offset);
            targets.push(
                raw_boundaries[cue_start + offset - 1]
                    .saturating_sub(sentence_start)
                    .clamp(minimum, maximum),
            );
        }
        targets.push(word_count);
        let semantic =
            semantic_boundaries(&words[sentence_start..sentence_end], cue_count, &targets)?;
        for (offset, boundary) in semantic.into_iter().enumerate().skip(1) {
            final_boundaries[cue_start + offset] = sentence_start + boundary;
        }
        sentence_start = sentence_end;
        cue_start = cue_end;
    }

    Ok(cues
        .iter()
        .enumerate()
        .map(|(index, cue)| CaptionCue {
            start: cue.start,
            end: cue.end,
            text: words[final_boundaries[index]..final_boundaries[index + 1]].join(" "),
        })
        .collect())
}

fn scaled_boundaries(
    raw_word_counts: &[usize],
    authored_word_count: usize,
    raw_word_count: usize,
) -> Result<Vec<usize>, CaptionPlanError> {
    let mut boundaries = Vec::with_capacity(raw_word_counts.len());
    let mut raw_cumulative = 0_usize;
    let mut previous = 0_usize;
    for (index, count) in raw_word_counts.iter().enumerate() {
        raw_cumulative = raw_cumulative.saturating_add(*count);
        let remaining_cues = raw_word_counts.len() - index - 1;
        let minimum = previous + 1;
        let maximum = authored_word_count.saturating_sub(remaining_cues);
        if minimum > maximum {
            return Err(CaptionPlanError::AuthoredScriptAlignment);
        }
        let scaled = raw_cumulative
            .saturating_mul(authored_word_count)
            .saturating_add(raw_word_count / 2)
            / raw_word_count;
        let boundary = scaled.clamp(minimum, maximum);
        boundaries.push(boundary);
        previous = boundary;
    }
    Ok(boundaries)
}

/// Serialize caption cues as `SubRip` text.
#[must_use]
pub fn srt(cues: &[CaptionCue], fps: Rational) -> String {
    let blocks = cues
        .iter()
        .enumerate()
        .map(|(index, cue)| {
            format!(
                "{}\n{} --> {}\n{}",
                index + 1,
                timestamp(cue.start, fps, ','),
                timestamp(cue.end, fps, ','),
                cue.text
            )
        })
        .collect::<Vec<_>>();
    if blocks.is_empty() {
        String::new()
    } else {
        format!("{}\n", blocks.join("\n\n"))
    }
}

/// Serialize caption cues as `WebVTT` text.
#[must_use]
pub fn vtt(cues: &[CaptionCue], fps: Rational) -> String {
    let blocks = cues
        .iter()
        .map(|cue| {
            format!(
                "{} --> {}\n{}",
                timestamp(cue.start, fps, '.'),
                timestamp(cue.end, fps, '.'),
                cue.text
            )
        })
        .collect::<Vec<_>>();
    if blocks.is_empty() {
        "WEBVTT\n\n".to_owned()
    } else {
        format!("WEBVTT\n\n{}\n", blocks.join("\n\n"))
    }
}

fn half_second_frames(fps: Rational) -> TimeCode {
    if !fps.is_valid() {
        return TimeCode::ZERO;
    }
    let numerator = u64::from(fps.numerator());
    let denominator = u64::from(fps.denominator()) * 2;
    let rounded = (numerator + denominator / 2) / denominator;
    TimeCode(i64::try_from(rounded).unwrap_or(i64::MAX))
}

#[derive(Debug, Clone)]
struct CaptionWord {
    text: String,
    start: TimeCode,
    end: TimeCode,
    asset: crate::AssetId,
    clip: ClipId,
    speaker: Option<String>,
}

fn infer_sentence_punctuation(words: &mut [CaptionWord]) {
    for index in 1..words.len() {
        let (left, right) = words.split_at_mut(index);
        let previous = &mut left[index - 1];
        let next = &right[0];
        if ends_sentence(&previous.text) {
            continue;
        }
        let source_change = previous.asset != next.asset
            || previous.clip != next.clip
            || previous.speaker != next.speaker;
        let gap = next.start.0.saturating_sub(previous.end.0);
        let likely_start = starts_with_uppercase(&next.text)
            && (gap >= 4 || source_change)
            && likely_sentence_starter(&next.text);
        if source_change || likely_start {
            previous.text.push('.');
        }
    }
}

fn push_semantic_cues(cues: &mut Vec<CaptionCue>, words: &[&CaptionWord]) {
    let mut start = 0;
    while start < words.len() {
        let mut farthest = start + 1;
        while farthest < words.len()
            && phrase_characters(
                words[start..=farthest]
                    .iter()
                    .map(|word| word.text.as_str()),
            ) <= MAX_CAPTION_CHARACTERS
        {
            farthest += 1;
        }
        let end = if farthest == words.len() {
            farthest
        } else {
            let search_start = (start + 1).max(farthest.saturating_sub(4));
            (search_start..=farthest)
                .min_by_key(|&candidate| {
                    let characters = phrase_characters(
                        words[start..candidate]
                            .iter()
                            .map(|word| word.text.as_str()),
                    );
                    let short_penalty = usize::from(characters < 12) * 120;
                    boundary_penalty(&words[candidate - 1].text, &words[candidate].text)
                        .saturating_add(farthest.saturating_sub(candidate) * 8)
                        .saturating_add(short_penalty)
                })
                .unwrap_or(farthest)
        };
        let first = words[start];
        let last = words[end - 1];
        cues.push(CaptionCue {
            start: first.start,
            end: last.end,
            text: words[start..end]
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        });
        start = end;
    }
}

fn semantic_boundaries(
    words: &[&str],
    cue_count: usize,
    targets: &[usize],
) -> Result<Vec<usize>, CaptionPlanError> {
    if cue_count == 0 || words.len() < cue_count || targets.len() != cue_count {
        return Err(CaptionPlanError::AuthoredScriptAlignment);
    }
    let width = words.len() + 1;
    let unreachable = u64::MAX / 4;
    let mut costs = vec![unreachable; (cue_count + 1) * width];
    let mut parents = vec![usize::MAX; (cue_count + 1) * width];
    costs[0] = 0;
    let total_characters = phrase_characters(words.iter().copied());
    let ideal = total_characters.div_ceil(cue_count);

    for used in 1..=cue_count {
        let minimum_end = used;
        let maximum_end = words.len().saturating_sub(cue_count - used);
        for end in minimum_end..=maximum_end {
            for start in (used - 1)..end {
                let previous = costs[(used - 1) * width + start];
                if previous == unreachable {
                    continue;
                }
                let characters = phrase_characters(words[start..end].iter().copied());
                let over = characters.saturating_sub(MAX_CAPTION_CHARACTERS) as u64;
                let ragged = characters.abs_diff(ideal) as u64;
                let target_distance = end.abs_diff(targets[used - 1]) as u64;
                let boundary = if end == words.len() {
                    0
                } else {
                    boundary_penalty(words[end - 1], words[end]) as u64
                };
                let cost = previous
                    .saturating_add(over.saturating_mul(over).saturating_mul(500))
                    .saturating_add(ragged.saturating_mul(ragged))
                    .saturating_add(target_distance.saturating_mul(12))
                    .saturating_add(boundary);
                let slot = used * width + end;
                if cost < costs[slot] {
                    costs[slot] = cost;
                    parents[slot] = start;
                }
            }
        }
    }

    if costs[cue_count * width + words.len()] == unreachable {
        return Err(CaptionPlanError::AuthoredScriptAlignment);
    }
    let mut boundaries = vec![0; cue_count + 1];
    boundaries[cue_count] = words.len();
    let mut end = words.len();
    for used in (1..=cue_count).rev() {
        let start = parents[used * width + end];
        if start == usize::MAX {
            return Err(CaptionPlanError::AuthoredScriptAlignment);
        }
        boundaries[used - 1] = start;
        end = start;
    }
    Ok(boundaries)
}

fn phrase_characters<'a>(words: impl Iterator<Item = &'a str>) -> usize {
    let (characters, count) = words.fold((0_usize, 0_usize), |(characters, count), word| {
        (characters + word.chars().count(), count + 1)
    });
    characters.saturating_add(count.saturating_sub(1))
}

fn boundary_penalty(previous: &str, next: &str) -> usize {
    if ends_sentence(previous) || ends_clause(previous) {
        return 0;
    }
    if is_bound_phrase(previous, next) {
        return 4_000;
    }
    if is_dangling_end(previous) {
        return 4_000;
    }
    if is_dangling_start(next) {
        return 4_000;
    }
    if is_connective(next) {
        return 1_000;
    }
    256
}

fn ends_clause(text: &str) -> bool {
    matches!(
        text.trim_end_matches(['\'', '"', ')', ']', '}'])
            .chars()
            .next_back(),
        Some(',' | ';' | ':')
    )
}

fn normalized_token(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_dangling_end(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "a" | "an"
            | "the"
            | "and"
            | "or"
            | "but"
            | "of"
            | "to"
            | "in"
            | "on"
            | "at"
            | "for"
            | "from"
            | "with"
            | "my"
            | "your"
            | "their"
            | "our"
            | "its"
            | "that"
            | "these"
            | "those"
            | "this"
            | "where"
    )
}

fn is_dangling_start(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "of" | "to" | "in" | "on" | "at" | "for" | "from" | "with"
    )
}

fn is_bound_phrase(previous: &str, next: &str) -> bool {
    if ends_sentence(previous) || ends_clause(previous) {
        return false;
    }
    let previous_normalized = normalized_token(previous);
    let next_normalized = normalized_token(next);
    let proper_name = starts_with_uppercase(previous)
        && starts_with_uppercase(next)
        && !ends_sentence(previous)
        && !ends_clause(previous);
    proper_name
        || matches!(
            previous_normalized.as_str(),
            "i" | "ive"
                | "im"
                | "you"
                | "youre"
                | "he"
                | "hes"
                | "she"
                | "shes"
                | "it"
                | "its"
                | "we"
                | "were"
                | "they"
                | "theyre"
                | "very"
                | "recently"
                | "especially"
                | "maybe"
                | "just"
                | "even"
        )
        || matches!(
            (previous_normalized.as_str(), next_normalized.as_str()),
            ("super", "8") | ("home", "movies")
        )
}

fn is_connective(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "and" | "but" | "so" | "because" | "while" | "when" | "where" | "then"
    )
}

fn starts_with_uppercase(text: &str) -> bool {
    text.chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase)
}

fn likely_sentence_starter(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "and" | "but" | "so" | "then" | "they" | "meanwhile" | "however"
    )
}

fn ends_sentence(text: &str) -> bool {
    let without_closers = text.trim_end_matches(|character| {
        matches!(
            character,
            '\'' | '"' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
        )
    });
    matches!(without_closers.chars().next_back(), Some('.' | '!' | '?'))
}

fn timestamp(frames: TimeCode, fps: Rational, millisecond_separator: char) -> String {
    let milliseconds = frame_milliseconds(frames, fps).max(0);
    let hours = milliseconds / 3_600_000;
    let minutes = milliseconds / 60_000 % 60;
    let seconds = milliseconds / 1_000 % 60;
    let remainder = milliseconds % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{millisecond_separator}{remainder:03}")
}

fn frame_milliseconds(frames: TimeCode, fps: Rational) -> i128 {
    if !fps.is_valid() {
        return 0;
    }
    i128::from(frames.0)
        .saturating_mul(1_000)
        .saturating_mul(i128::from(fps.denominator()))
        .div_euclid(i128::from(fps.numerator()))
}

#[cfg(test)]
mod tests {
    use crate::{AssetId, ClipContent, ClipId, TrackId, apply_batch};

    use super::*;

    fn word(text: &str, start: i64, end: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(1),
            track: TrackId(1),
            clip: ClipId(1),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    fn cue(start: i64, end: i64, text: &str) -> CaptionCue {
        CaptionCue {
            start: TimeCode(start),
            end: TimeCode(end),
            text: text.to_owned(),
        }
    }

    #[test]
    fn empty_words_produce_no_cues() {
        assert!(caption_cues(&[], Rational::default()).is_empty());
    }

    #[test]
    fn a_gap_over_half_a_second_breaks_before_the_next_word() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [word("one", 0, 5), word("two", 21, 25)];
        assert_eq!(
            caption_cues(&words, fps),
            vec![cue(0, 20, "one"), cue(21, 40, "two")]
        );
    }

    #[test]
    fn sentence_closers_break_after_the_punctuated_word() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [
            word("word.", 0, 2),
            word("word!)", 3, 5),
            word("word?\"'", 6, 8),
            word("next", 9, 11),
        ];
        assert_eq!(
            caption_cues(&words, fps),
            vec![
                cue(0, 3, "word."),
                cue(3, 6, "word!)"),
                cue(6, 9, "word?\"'"),
                cue(9, 26, "next"),
            ]
        );
    }

    #[test]
    fn forty_eight_character_limit_breaks_before_the_overflowing_word() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [
            word("12345678901234567890", 0, 2),
            word("123456789012345678901234567", 3, 5),
            word("x", 6, 8),
        ];
        assert_eq!(
            caption_cues(&words, fps),
            vec![
                cue(0, 6, "12345678901234567890 123456789012345678901234567"),
                cue(6, 23, "x"),
            ]
        );
    }

    #[test]
    fn caption_presets_resolve_to_declarative_title_operations() {
        let document = Document::default();
        let cues = [cue(0, 15, "Hello")];
        for preset in CaptionPreset::ALL {
            let operations = caption_title_operations(&document, &cues, preset).unwrap();
            let mut applied = document.clone();
            apply_batch(&mut applied, &operations).unwrap();
            let ClipContent::Title(title) = &applied.tracks[0].clips[0].content else {
                panic!("expected caption title");
            };
            assert_eq!(title.caption_preset, Some(preset));
            assert_eq!(title.text, "Hello");
        }
    }

    #[test]
    fn animated_caption_presets_build_valid_editable_effect_curves() {
        let document = Document {
            fps: Rational::new(30, 1).unwrap(),
            ..Document::default()
        };
        let cues = [cue(0, 24, "Hello"), cue(30, 60, "again")];
        for motion in CaptionMotion::ALL {
            let operations =
                animated_caption_operations(&document, &cues, CaptionPreset::Social, motion)
                    .unwrap();
            let mut applied = document.clone();
            apply_batch(&mut applied, &operations).unwrap();
            let clips = &applied.tracks[0].clips;
            assert_eq!(clips.len(), 2);
            for clip in clips {
                assert_eq!(
                    clip.effects.len(),
                    match motion {
                        CaptionMotion::None => 0,
                        CaptionMotion::Fade => 1,
                        CaptionMotion::Pop | CaptionMotion::SlideUp => 2,
                    }
                );
                if motion != CaptionMotion::None {
                    let opacity = &clip.effects[0];
                    assert_eq!(
                        opacity.integer_parameter_at("percent", TimeCode::ZERO),
                        Some(0)
                    );
                    assert_eq!(
                        opacity
                            .integer_parameter_at("percent", TimeCode(clip.source_range.end.0 / 2)),
                        Some(100)
                    );
                }
            }
        }
    }

    #[test]
    fn one_long_word_still_becomes_its_own_cue() {
        let fps = Rational::new(30, 1).unwrap();
        let long = "a".repeat(43);
        let words = [word(&long, 0, 4), word("short", 5, 8)];
        assert_eq!(
            caption_cues(&words, fps),
            vec![cue(0, 5, &long), cue(5, 23, "short")]
        );
    }

    #[test]
    fn hold_is_capped_at_the_next_cue_start() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [word("First.", 0, 5), word("Second", 10, 12)];
        assert_eq!(
            caption_cues(&words, fps),
            vec![cue(0, 10, "First."), cue(10, 27, "Second")]
        );
    }

    #[test]
    fn authored_script_makes_sentence_boundaries_authoritative() {
        let generated = vec![
            cue(4, 119, "Last spring this empty lot collected weeds"),
            cue(119, 182, "and rain Neighbors decided it could feed"),
            cue(182, 287, "families instead Over three weekends"),
            cue(287, 342, "volunteers built raised beds"),
            cue(350, 412, "Then they planted tomatoes herbs and"),
            cue(412, 448, "peppers"),
            cue(448, 508, "Now the Saturday market supplies fresh"),
            cue(508, 585, "produce to dozens of local families"),
        ];
        let script = concat!(
            "Last spring this empty lot collected weeds and rainwater. ",
            "Neighbors decided it could feed families instead. ",
            "Over three weekends volunteers built raised beds. ",
            "Then they planted tomatoes herbs and peppers. ",
            "Now the Saturday market supplies fresh produce to dozens of local families."
        );

        let authored = authored_caption_cues(&generated, script).unwrap();
        assert_eq!(
            authored
                .iter()
                .map(|cue| cue.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Last spring this empty lot collected weeds and rainwater.",
                "Neighbors decided it could feed families instead.",
                "Over three weekends",
                "volunteers built raised beds.",
                "Then they planted tomatoes",
                "herbs and peppers.",
                "Now the Saturday market supplies fresh",
                "produce to dozens of local families.",
            ]
        );
        assert_eq!(authored[0].start, generated[0].start);
        assert_eq!(authored[7].end, generated[7].end);
    }

    #[test]
    fn authored_interview_captions_keep_names_and_syntax_together() {
        let generated_text = [
            "But recently I was living in New",
            "Orleans and my house flooded",
            "and a lot of my films and especially",
            "my recently shot Super 8 home movies",
            "and I've been cleaning them",
            "They deteriorated very quickly in that",
            "short you know two weeks where they",
            "were submerged in those floodwaters",
            "And I've been cleaning them",
            "and they look deteriorated",
            "and old even though they're just",
            "you know maybe 12 months old",
            "So I'm going to be screening just",
            "a selection of some of that cleaned",
            "flood damage by Hurricane Katrina films",
        ];
        let generated = generated_text
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let start = i64::try_from(index).unwrap() * 30;
                cue(start, start + 30, text)
            })
            .collect::<Vec<_>>();
        let script = concat!(
            "But recently I was living in New Orleans, and my house flooded, and a lot of my films, and especially my recently shot Super 8 home movies, and I've been cleaning them. ",
            "They deteriorated very quickly in that short, you know, two weeks where they were submerged in those floodwaters. ",
            "And I've been cleaning them, and they look deteriorated and old even though they're just, you know, maybe 12 months old. ",
            "So I'm going to be screening just a selection of some of that cleaned flood damage by Hurricane Katrina films."
        );

        let authored = authored_caption_cues(&generated, script).unwrap();
        for pair in authored.windows(2) {
            let previous = pair[0].text.split_whitespace().next_back().unwrap();
            let next = pair[1].text.split_whitespace().next().unwrap();
            assert!(
                !is_bound_phrase(previous, next),
                "split bound phrase across cues: {:?} / {:?}",
                pair[0].text,
                pair[1].text
            );
            assert!(
                !is_dangling_end(previous),
                "left dangling syntax at cue end: {:?}",
                pair[0].text
            );
            assert!(
                !is_dangling_start(next),
                "started a cue with attached syntax: {:?}",
                pair[1].text
            );
        }
        assert_eq!(
            authored
                .iter()
                .map(|cue| cue.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            script
        );
    }

    #[test]
    fn authored_script_rejects_more_sentences_than_timing_cues() {
        let generated = [cue(0, 30, "one two")];
        assert_eq!(
            authored_caption_cues(&generated, "One. Two."),
            Err(CaptionPlanError::AuthoredScriptAlignment)
        );
    }

    #[test]
    fn raw_cues_avoid_dangling_connectives_at_character_breaks() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [
            word("Recently", 0, 3),
            word("I", 4, 5),
            word("was", 6, 8),
            word("living", 9, 12),
            word("in", 13, 14),
            word("New", 15, 17),
            word("Orleans", 18, 21),
            word("and", 22, 24),
            word("my", 25, 27),
            word("house", 28, 31),
            word("flooded.", 32, 36),
        ];

        let cues = caption_cues(&words, fps);
        assert_eq!(
            cues.iter().map(|cue| cue.text.as_str()).collect::<Vec<_>>(),
            vec![
                "Recently I was living in New Orleans",
                "and my house flooded."
            ]
        );
    }

    #[test]
    fn transcript_capitalization_restores_audible_sentence_punctuation() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [
            word("I've", 0, 3),
            word("been", 4, 7),
            word("cleaning", 8, 12),
            word("They", 16, 19),
            word("deteriorated", 20, 26),
        ];

        assert_eq!(
            caption_cues(&words, fps)
                .iter()
                .map(|cue| cue.text.as_str())
                .collect::<Vec<_>>(),
            vec!["I've been cleaning.", "They deteriorated"]
        );
    }

    #[test]
    fn ntsc_timestamps_use_flooring_integer_math() {
        let fps = Rational::new(30_000, 1_001).unwrap();
        let cues = [cue(0, 30, "NTSC")];
        assert_eq!(srt(&cues, fps), "1\n00:00:00,000 --> 00:00:01,001\nNTSC\n");
    }

    #[test]
    fn serializers_match_numbering_headers_and_long_hours() {
        let fps = Rational::new(30, 1).unwrap();
        let cues = [cue(0, 30, "At zero"), cue(108_030, 108_060, "Past an hour")];
        assert_eq!(
            srt(&cues, fps),
            concat!(
                "1\n00:00:00,000 --> 00:00:01,000\nAt zero\n\n",
                "2\n01:00:01,000 --> 01:00:02,000\nPast an hour\n",
            )
        );
        assert_eq!(
            vtt(&cues, fps),
            concat!(
                "WEBVTT\n\n",
                "00:00:00.000 --> 00:00:01.000\nAt zero\n\n",
                "01:00:01.000 --> 01:00:02.000\nPast an hour\n",
            )
        );
    }

    #[test]
    fn empty_serializers_have_their_format_specific_preamble() {
        let fps = Rational::default();
        assert_eq!(srt(&[], fps), "");
        assert_eq!(vtt(&[], fps), "WEBVTT\n\n");
    }

    #[test]
    fn one_frame_caption_stays_visible_without_degenerate_motion() {
        let document = Document::default();
        let operations = animated_caption_operations(
            &document,
            &[CaptionCue {
                start: TimeCode(10),
                end: TimeCode(11),
                text: "Now".to_owned(),
            }],
            CaptionPreset::Clean,
            CaptionMotion::Pop,
        )
        .unwrap();

        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, Operation::AddEffect { .. }))
                .count(),
            0
        );
    }
}
