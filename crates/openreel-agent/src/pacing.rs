use openreel_core::{
    TimeCode, TimelineSilenceSpan, TimelineTranscriptWord, TrackId, is_filler_word,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MergedSilence {
    track: TrackId,
    start: TimeCode,
    end: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DialoguePacingGap {
    pub(crate) previous_word: String,
    pub(crate) next_word: String,
    pub(crate) previous_end: TimeCode,
    pub(crate) next_start: TimeCode,
    /// Preferred observed pause. Uses energy-detected silence when available,
    /// otherwise falls back to the transcript boundary distance.
    pub(crate) pause_frames: TimeCode,
    pub(crate) transcript_pause_frames: TimeCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) acoustic_start: Option<TimeCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) acoustic_end: Option<TimeCode>,
    pub(crate) measurement: &'static str,
    pub(crate) status: &'static str,
    pub(crate) reason: String,
}

pub(crate) fn dialogue_pacing_gaps(
    words: &[TimelineTranscriptWord],
    silences: &[TimelineSilenceSpan],
    minimum_pause_frames: TimeCode,
    maximum_pause_frames: TimeCode,
    capitalization_boundary_minimum_frames: TimeCode,
) -> Vec<DialoguePacingGap> {
    let audible = words
        .iter()
        .filter(|word| !is_filler_word(&word.text))
        .collect::<Vec<_>>();
    let silences = merge_timeline_silences(silences);
    audible
        .windows(2)
        .filter_map(|pair| {
            let previous = pair[0];
            let next = pair[1];
            let transcript_pause_frames =
                TimeCode(next.project_start.0.saturating_sub(previous.project_end.0));
            let mut reasons = Vec::new();
            if previous.asset != next.asset {
                reasons.push("asset_change");
            }
            if previous.speaker.is_some()
                && next.speaker.is_some()
                && previous.speaker != next.speaker
            {
                reasons.push("speaker_change");
            }
            if word_ends_sentence(&previous.text) {
                reasons.push("terminal_punctuation");
            }
            if transcript_pause_frames >= capitalization_boundary_minimum_frames
                && word_starts_uppercase(&next.text)
            {
                reasons.push("pause_backed_capitalization");
            }
            if reasons.is_empty() {
                return None;
            }
            let acoustic = acoustic_pause_between(previous, next, &silences);
            let pause_frames = acoustic.map_or(transcript_pause_frames, |silence| {
                TimeCode(silence.end.0.saturating_sub(silence.start.0))
            });
            let status = if pause_frames < minimum_pause_frames {
                "short"
            } else if pause_frames > maximum_pause_frames {
                "long"
            } else {
                "target"
            };
            Some(DialoguePacingGap {
                previous_word: previous.text.clone(),
                next_word: next.text.clone(),
                previous_end: previous.project_end,
                next_start: next.project_start,
                pause_frames,
                transcript_pause_frames,
                acoustic_start: acoustic.map(|silence| silence.start),
                acoustic_end: acoustic.map(|silence| silence.end),
                measurement: if acoustic.is_some() {
                    "acoustic_silence"
                } else {
                    "transcript_bounds"
                },
                status,
                reason: reasons.join("+"),
            })
        })
        .collect()
}

fn merge_timeline_silences(silences: &[TimelineSilenceSpan]) -> Vec<MergedSilence> {
    let mut ranges = silences
        .iter()
        .filter(|silence| silence.project_end > silence.project_start)
        .map(|silence| MergedSilence {
            track: silence.track,
            start: silence.project_start,
            end: silence.project_end,
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|silence| (silence.track, silence.start, silence.end));
    let mut merged = Vec::<MergedSilence>::new();
    for silence in ranges {
        if let Some(previous) = merged.last_mut()
            && previous.track == silence.track
            && silence.start.0 <= previous.end.0.saturating_add(1)
        {
            previous.end = previous.end.max(silence.end);
        } else {
            merged.push(silence);
        }
    }
    merged
}

fn acoustic_pause_between(
    previous: &TimelineTranscriptWord,
    next: &TimelineTranscriptWord,
    silences: &[MergedSilence],
) -> Option<MergedSilence> {
    silences
        .iter()
        .filter(|silence| silence.track == previous.track || silence.track == next.track)
        // Whisper can extend a word timestamp well into real silence. Search
        // between word onsets so that energy evidence, not that late endpoint,
        // owns the measured pause.
        .filter(|silence| {
            silence.start < next.project_start && silence.end > previous.project_start
        })
        .max_by_key(|silence| silence.end.0.saturating_sub(silence.start.0))
        .copied()
}

fn word_ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', ')', ']', '}'])
        .ends_with(['.', '!', '?'])
}

fn word_starts_uppercase(text: &str) -> bool {
    text.chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use openreel_core::{AssetId, ClipId, TimelineSilenceSpan, TrackId};

    use super::*;

    fn word(text: &str, asset: u64, start: i64, end: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(asset),
            track: TrackId(1),
            clip: ClipId(asset),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    fn silence(clip: u64, start: i64, end: i64) -> TimelineSilenceSpan {
        TimelineSilenceSpan {
            asset: AssetId(clip),
            track: TrackId(1),
            clip: ClipId(clip),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    #[test]
    fn acoustic_silence_overrides_late_transcript_endpoints_across_cuts() {
        let words = vec![word("rain", 1, 128, 141), word("Neighbors", 1, 153, 165)];
        let silences = vec![silence(1, 108, 147), silence(2, 147, 153)];

        let gaps = dialogue_pacing_gaps(&words, &silences, TimeCode(9), TimeCode(15), TimeCode(4));

        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].transcript_pause_frames, TimeCode(12));
        assert_eq!(gaps[0].pause_frames, TimeCode(45));
        assert_eq!(gaps[0].measurement, "acoustic_silence");
        assert_eq!(gaps[0].status, "long");
        assert_eq!(gaps[0].acoustic_start, Some(TimeCode(108)));
        assert_eq!(gaps[0].acoustic_end, Some(TimeCode(153)));
    }

    #[test]
    fn acoustic_silence_exposes_a_short_cross_asset_boundary() {
        let words = vec![word("instead", 1, 214, 227), word("Over", 2, 239, 247)];
        let silences = vec![
            silence(1, 230, 233),
            silence(2, 233, 236),
            silence(3, 236, 239),
        ];

        let gaps = dialogue_pacing_gaps(&words, &silences, TimeCode(12), TimeCode(24), TimeCode(4));

        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].pause_frames, TimeCode(9));
        assert_eq!(gaps[0].measurement, "acoustic_silence");
        assert_eq!(gaps[0].status, "short");
    }

    #[test]
    fn reviewed_opening_fails_without_rejecting_the_later_natural_pauses() {
        let words = vec![
            word("rain", 1, 128, 141),
            word("Neighbors", 1, 153, 165),
            word("instead", 1, 214, 227),
            word("Over", 3, 239, 247),
            word("beds", 3, 352, 366),
            word("Then", 3, 378, 382),
            word("peppers.", 3, 440, 461),
            word("Now", 4, 473, 476),
        ];
        let silences = vec![
            silence(1, 107, 147),
            silence(2, 147, 154),
            silence(2, 230, 233),
            silence(3, 233, 236),
            silence(4, 236, 239),
            silence(5, 346, 374),
            silence(6, 374, 377),
            silence(6, 461, 467),
            silence(7, 467, 470),
            silence(8, 470, 474),
        ];

        let gaps = dialogue_pacing_gaps(&words, &silences, TimeCode(10), TimeCode(40), TimeCode(4));

        assert_eq!(gaps.len(), 4);
        assert_eq!(
            gaps.iter().map(|gap| gap.pause_frames).collect::<Vec<_>>(),
            [TimeCode(47), TimeCode(9), TimeCode(31), TimeCode(13)]
        );
        assert_eq!(
            gaps.iter().map(|gap| gap.status).collect::<Vec<_>>(),
            ["long", "short", "target", "target"]
        );
    }
}
