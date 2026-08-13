use thiserror::Error;

use crate::{
    CaptionPreset, Document, Operation, Rational, TimeCode, TimelineTranscriptWord, Track, TrackId,
    TrackKind,
};

const MAX_CAPTION_CHARACTERS: usize = 42;

/// One half-open caption interval in project frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionCue {
    pub start: TimeCode,
    pub end: TimeCode,
    pub text: String,
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
    #[error("track id space is exhausted")]
    TrackIdExhausted,
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
    let mut operations = Vec::with_capacity(cues.len().saturating_add(1));
    operations.push(Operation::AddTrack {
        track: Track {
            id: track_id,
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    });
    for cue in cues {
        let duration = cue
            .end
            .checked_sub(cue.start)
            .filter(|duration| *duration > TimeCode::ZERO)
            .ok_or(CaptionPlanError::InvalidCueDuration)?;
        operations.push(Operation::AddTitle {
            track: track_id,
            at: cue.start,
            duration,
            title: preset.title(cue.text.clone()),
        });
    }
    Ok(operations)
}

/// Build readable caption cues from ordered, de-duplicated timeline words.
#[must_use]
pub fn caption_cues(words: &[TimelineTranscriptWord], project_fps: Rational) -> Vec<CaptionCue> {
    let hold = half_second_frames(project_fps);
    let mut cues = Vec::new();
    let mut current_words = Vec::new();
    let mut current_start = TimeCode::ZERO;
    let mut current_end = TimeCode::ZERO;
    let mut previous_end = None;

    for word in words {
        let text = word.text.trim();
        if text.is_empty() || word.project_end <= word.project_start {
            continue;
        }

        let gap_break = previous_end
            .is_some_and(|end: TimeCode| word.project_start.0.saturating_sub(end.0) > hold.0);
        let length_break = !current_words.is_empty()
            && current_words
                .iter()
                .map(|word: &&str| word.chars().count())
                .sum::<usize>()
                .saturating_add(current_words.len())
                .saturating_add(text.chars().count())
                > MAX_CAPTION_CHARACTERS;

        if !current_words.is_empty() && (gap_break || length_break) {
            push_cue(&mut cues, &mut current_words, current_start, current_end);
        }
        if current_words.is_empty() {
            current_start = word.project_start;
        }
        current_end = word.project_end;
        current_words.push(text);
        previous_end = Some(word.project_end);

        if ends_sentence(text) {
            push_cue(&mut cues, &mut current_words, current_start, current_end);
        }
    }
    if !current_words.is_empty() {
        push_cue(&mut cues, &mut current_words, current_start, current_end);
    }

    for index in 0..cues.len() {
        let held_end = TimeCode(cues[index].end.0.saturating_add(hold.0));
        cues[index].end = cues
            .get(index + 1)
            .map_or(held_end, |next| held_end.min(next.start));
    }
    cues.retain(|cue| cue.end > cue.start);
    cues
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

fn push_cue(cues: &mut Vec<CaptionCue>, words: &mut Vec<&str>, start: TimeCode, end: TimeCode) {
    if end > start {
        cues.push(CaptionCue {
            start,
            end,
            text: words.join(" "),
        });
    }
    words.clear();
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
    fn forty_two_character_limit_breaks_before_the_overflowing_word() {
        let fps = Rational::new(30, 1).unwrap();
        let words = [
            word("12345678901234567890", 0, 2),
            word("123456789012345678901", 3, 5),
            word("x", 6, 8),
        ];
        assert_eq!(
            caption_cues(&words, fps),
            vec![
                cue(0, 6, "12345678901234567890 123456789012345678901"),
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
}
