use std::{collections::HashSet, ops::RangeInclusive};

use crate::{ClipId, Document, Rational, TimeCode, TimelineTranscriptWord, TrackId};

/// Return whether transcript text is one of the deliberately narrow filler sounds.
///
/// These are unambiguous hesitation sounds. Context-dependent words such as
/// "like", "you know", "ah", "oh", and "so" are excluded because a
/// one-click destructive action should prefer misses over false positives.
#[must_use]
pub fn is_filler_word(text: &str) -> bool {
    let normalized = text
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | ',' | '!' | '?' | ';' | ':' | '\'' | '"' | '(' | ')' | '-' | '…'
                )
        })
        .to_lowercase();
    matches!(
        normalized.as_str(),
        "um" | "uh" | "erm" | "hmm" | "mm" | "mhm"
    )
}

/// One source-frame range selected for removal from a timeline clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptCutRange {
    pub track: TrackId,
    pub clip: ClipId,
    pub source_range: std::ops::Range<TimeCode>,
}

/// Return the 100 ms speech-safety margin in exact source frames.
#[must_use]
pub fn silence_cut_margin_frames(source_fps: Rational) -> TimeCode {
    if !source_fps.is_valid() {
        return TimeCode::ZERO;
    }

    let numerator = u64::from(source_fps.numerator());
    let denominator = u64::from(source_fps.denominator()) * 10;
    let rounded = (numerator + denominator / 2) / denominator;
    TimeCode(i64::try_from(rounded).unwrap_or(i64::MAX))
}

/// Convert a contiguous timeline-word selection into source-frame cut ranges.
///
/// Selected runs are evaluated independently per clip. The gap after a run is
/// included up to the next retained word's safety margin. A run at the end of
/// a clip receives only one margin after its final word.
#[must_use]
pub fn transcript_cut_ranges(
    document: &Document,
    words: &[TimelineTranscriptWord],
    selected: RangeInclusive<usize>,
) -> Vec<TranscriptCutRange> {
    let selected_start = *selected.start();
    let selected_end = *selected.end();
    if selected_start > selected_end || selected_end >= words.len() {
        return Vec::new();
    }

    let selected_indices = (selected_start..=selected_end).collect::<Vec<_>>();
    transcript_cut_ranges_for_indices(document, words, &selected_indices)
}

/// Convert arbitrary timeline-word indices into source-frame cut ranges.
///
/// Duplicate and out-of-range indices are ignored. Within each clip, selected
/// words that are consecutive in source-sorted word order form one run. Every
/// run uses the same trailing-gap and retained-neighbor safety semantics as a
/// contiguous transcript selection.
#[must_use]
pub fn transcript_cut_ranges_for_indices(
    document: &Document,
    words: &[TimelineTranscriptWord],
    selected_indices: &[usize],
) -> Vec<TranscriptCutRange> {
    let selected = selected_indices
        .iter()
        .copied()
        .filter(|index| *index < words.len())
        .collect::<HashSet<_>>();
    if selected.is_empty() {
        return Vec::new();
    }

    let mut selected_clips = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if selected.contains(&index) && !selected_clips.contains(&word.clip) {
            selected_clips.push(word.clip);
        }
    }

    let mut cuts = Vec::new();
    for clip_id in selected_clips {
        let Some(clip) = document.clip(clip_id) else {
            continue;
        };
        let Some(asset) = document.asset(clip.asset) else {
            continue;
        };
        let margin = silence_cut_margin_frames(asset.fps).0;
        let mut clip_words = words
            .iter()
            .enumerate()
            .filter(|(_, word)| word.clip == clip_id)
            .collect::<Vec<_>>();
        clip_words.sort_by_key(|(index, word)| (word.source_start, word.source_end, *index));

        let mut cursor = 0;
        while cursor < clip_words.len() {
            let is_selected = |index: usize| selected.contains(&index);
            if !is_selected(clip_words[cursor].0) {
                cursor += 1;
                continue;
            }

            let run_start = cursor;
            while cursor + 1 < clip_words.len() && is_selected(clip_words[cursor + 1].0) {
                cursor += 1;
            }
            let run_end = cursor;
            let first = clip_words[run_start].1;
            let last = clip_words[run_end].1;
            let previous_retained_end = run_start
                .checked_sub(1)
                .map_or(clip.source_range.start, |index| {
                    clip_words[index].1.source_end
                });
            let mut cut_start = first
                .source_start
                .max(previous_retained_end)
                .clamp(clip.source_range.start, clip.source_range.end);
            let mut cut_end = if let Some((_, next)) = clip_words.get(run_end + 1) {
                TimeCode(
                    last.source_end
                        .0
                        .max(next.source_start.0.saturating_sub(margin)),
                )
                .min(next.source_start)
            } else {
                TimeCode(last.source_end.0.saturating_add(margin))
            };
            cut_end = cut_end.clamp(clip.source_range.start, clip.source_range.end);
            cut_start = cut_start.min(clip.source_range.end);

            if cut_end > cut_start {
                cuts.push(TranscriptCutRange {
                    track: first.track,
                    clip: clip_id,
                    source_range: cut_start..cut_end,
                });
            }
            cursor += 1;
        }
    }

    debug_assert!(retained_words_are_clear(words, &selected, &cuts));
    cuts
}

fn retained_words_are_clear(
    words: &[TimelineTranscriptWord],
    selected: &HashSet<usize>,
    cuts: &[TranscriptCutRange],
) -> bool {
    words.iter().enumerate().all(|(index, word)| {
        selected.contains(&index)
            || cuts.iter().filter(|cut| cut.clip == word.clip).all(|cut| {
                cut.source_range.end <= word.source_start
                    || cut.source_range.start >= word.source_end
            })
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use crate::{AssetId, Clip, ClipContent, MediaAsset, MediaKind, Track, TrackKind};

    use super::*;

    fn fixture(clip_ranges: &[(u64, std::ops::Range<i64>)]) -> Document {
        let fps = Rational::new(30, 1).unwrap();
        let mut clips = Vec::new();
        let mut assets = Vec::new();
        let mut timeline_start = 0;
        for (id, source) in clip_ranges {
            let asset = MediaAsset {
                id: AssetId(*id),
                path: PathBuf::from(format!("asset-{id}.wav")),
                name: format!("asset-{id}.wav"),
                duration: TimeCode(source.end.max(1)),
                fps,
                kind: MediaKind::Audio,
                resolution: None,
            };
            clips.push(Clip {
                id: ClipId(*id),
                asset: asset.id,
                source_range: TimeCode(source.start)..TimeCode(source.end),
                content: ClipContent::Media,
                timeline_start: TimeCode(timeline_start),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
            });
            timeline_start += source.end - source.start;
            assets.push(asset);
        }
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips,
            }],
            media_pool: assets,
            markers: Vec::new(),
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(timeline_start),
        }
    }

    fn word(clip: u64, start: i64, end: i64) -> TimelineTranscriptWord {
        TimelineTranscriptWord {
            text: format!("word-{start}"),
            asset: AssetId(clip),
            track: TrackId(1),
            clip: ClipId(clip),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        }
    }

    fn cut(clip: u64, start: i64, end: i64) -> TranscriptCutRange {
        TranscriptCutRange {
            track: TrackId(1),
            clip: ClipId(clip),
            source_range: TimeCode(start)..TimeCode(end),
        }
    }

    #[test]
    fn filler_word_normalization_is_deliberately_conservative() {
        for text in ["Um,", "UH", "hmm...", "(erm)", " mm ", "\"mhm\"", "uh…"] {
            assert!(is_filler_word(text), "{text:?} should match");
        }
        for text in ["umbrella", "drum", "like", "ah", "oh", "so", "you know"] {
            assert!(!is_filler_word(text), "{text:?} should not match");
        }
    }

    #[test]
    fn single_word_takes_the_trailing_gap_and_keeps_the_next_margin() {
        let document = fixture(&[(1, 0..100)]);
        let words = [word(1, 0, 5), word(1, 10, 15), word(1, 30, 35)];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 1..=1),
            vec![cut(1, 10, 27)]
        );
    }

    #[test]
    fn run_of_words_becomes_one_range() {
        let document = fixture(&[(1, 0..100)]);
        let words = [word(1, 10, 15), word(1, 18, 23), word(1, 40, 45)];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 0..=1),
            vec![cut(1, 10, 37)]
        );
    }

    #[test]
    fn tightly_packed_speech_clamps_to_the_selected_word_end() {
        let document = fixture(&[(1, 0..100)]);
        let words = [word(1, 10, 29), word(1, 30, 40)];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 0..=0),
            vec![cut(1, 10, 29)]
        );
    }

    #[test]
    fn selections_at_clip_start_and_end_stay_inside_the_clip() {
        let document = fixture(&[(1, 10..100)]);
        let words = [word(1, 10, 20), word(1, 40, 50), word(1, 95, 100)];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 0..=0),
            vec![cut(1, 10, 37)]
        );
        assert_eq!(
            transcript_cut_ranges(&document, &words, 2..=2),
            vec![cut(1, 95, 100)]
        );
    }

    #[test]
    fn last_word_margin_is_capped_at_the_clip_end() {
        let document = fixture(&[(1, 0..100)]);
        let words = [word(1, 90, 99)];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 0..=0),
            vec![cut(1, 90, 100)]
        );
    }

    #[test]
    fn multi_clip_selection_yields_one_source_range_per_clip() {
        let document = fixture(&[(1, 0..100), (2, 20..80)]);
        let words = [
            word(1, 10, 20),
            word(1, 30, 40),
            word(2, 20, 30),
            word(2, 40, 50),
        ];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 1..=2),
            vec![cut(1, 30, 43), cut(2, 20, 37)]
        );
    }

    #[test]
    fn non_contiguous_indices_produce_multiple_ranges_in_one_call() {
        let document = fixture(&[(1, 0..100)]);
        let words = [
            word(1, 0, 5),
            word(1, 10, 15),
            word(1, 20, 25),
            word(1, 30, 35),
            word(1, 40, 45),
        ];
        assert_eq!(
            transcript_cut_ranges_for_indices(&document, &words, &[0, 2, 4]),
            vec![cut(1, 0, 7), cut(1, 20, 27), cut(1, 40, 48)]
        );
    }

    #[test]
    fn separated_selected_words_keep_the_retained_word_and_both_margins() {
        let document = fixture(&[(1, 0..100)]);
        let words = [
            word(1, 0, 5),
            word(1, 10, 15),
            word(1, 20, 25),
            word(1, 30, 35),
            word(1, 40, 45),
        ];
        assert_eq!(
            transcript_cut_ranges_for_indices(&document, &words, &[1, 3]),
            vec![cut(1, 10, 17), cut(1, 30, 37)]
        );
    }

    #[test]
    fn contiguous_wrapper_matches_the_index_set_api() {
        let document = fixture(&[(1, 0..100)]);
        let words = [
            word(1, 0, 5),
            word(1, 10, 15),
            word(1, 20, 25),
            word(1, 30, 35),
            word(1, 40, 45),
        ];
        assert_eq!(
            transcript_cut_ranges(&document, &words, 1..=3),
            transcript_cut_ranges_for_indices(&document, &words, &[1, 2, 3])
        );
    }

    #[test]
    fn empty_or_inverted_ranges_are_dropped() {
        let document = fixture(&[(1, 0..100)]);
        let words = [word(1, 100, 100)];
        assert!(transcript_cut_ranges(&document, &words, 0..=0).is_empty());
        let inverted = RangeInclusive::new(words.len(), words.len().saturating_sub(1));
        assert!(transcript_cut_ranges(&document, &words, inverted).is_empty());
        assert!(transcript_cut_ranges(&document, &words, 2..=2).is_empty());
    }

    proptest! {
        #[test]
        fn retained_word_integrity_property(
            gaps in prop::collection::vec(0_i64..8, 2..12),
            lengths in prop::collection::vec(1_i64..8, 2..12),
            selected_start in 0_usize..12,
            selected_len in 1_usize..12,
        ) {
            let count = gaps.len().min(lengths.len());
            let mut cursor = 0_i64;
            let mut words = Vec::with_capacity(count);
            for index in 0..count {
                cursor += gaps[index];
                let end = cursor + lengths[index];
                words.push(word(1, cursor, end));
                cursor = end;
            }
            let document = fixture(&[(1, 0..cursor.saturating_add(30))]);
            let start = selected_start.min(count - 1);
            let end = start.saturating_add(selected_len - 1).min(count - 1);
            let cuts = transcript_cut_ranges(&document, &words, start..=end);
            for (index, retained) in words.iter().enumerate() {
                if (start..=end).contains(&index) {
                    continue;
                }
                for cut in cuts.iter().filter(|cut| cut.clip == retained.clip) {
                    prop_assert!(
                        cut.source_range.end <= retained.source_start
                            || cut.source_range.start >= retained.source_end
                    );
                }
            }
        }


        #[test]
        fn retained_word_integrity_for_index_sets(
            gaps in prop::collection::vec(0_i64..8, 2..12),
            lengths in prop::collection::vec(1_i64..8, 2..12),
            selected_flags in prop::collection::vec(any::<bool>(), 2..12),
        ) {
            let count = gaps.len().min(lengths.len()).min(selected_flags.len());
            let mut cursor = 0_i64;
            let mut words = Vec::with_capacity(count);
            for index in 0..count {
                cursor += gaps[index];
                let end = cursor + lengths[index];
                words.push(word(1, cursor, end));
                cursor = end;
            }
            let selected = selected_flags
                .iter()
                .take(count)
                .enumerate()
                .filter_map(|(index, selected)| selected.then_some(index))
                .collect::<Vec<_>>();
            let document = fixture(&[(1, 0..cursor.saturating_add(30))]);
            let cuts = transcript_cut_ranges_for_indices(&document, &words, &selected);
            for (index, retained) in words.iter().enumerate() {
                if selected.contains(&index) {
                    continue;
                }
                for cut in cuts.iter().filter(|cut| cut.clip == retained.clip) {
                    prop_assert!(
                        cut.source_range.end <= retained.source_start
                            || cut.source_range.start >= retained.source_end
                    );
                }
            }
        }
    }
}
