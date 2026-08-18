use kinewright_core::{Rational, SilenceSpan, TimeCode, TranscriptWord};

pub use kinewright_core::silence_cut_margin_frames;

/// Shrink a raw detector span on both sides for safe cutting.
///
/// The detector result remains unchanged. Spans no longer containing any
/// frames after applying both margins are omitted.
#[must_use]
pub fn shrink_silence_span_for_cutting(
    span: SilenceSpan,
    source_fps: Rational,
) -> Option<SilenceSpan> {
    let margin = silence_cut_margin_frames(source_fps);
    let duration = span.source_end.0.saturating_sub(span.source_start.0);
    if span.source_start < TimeCode::ZERO || duration <= margin.0.saturating_mul(2) {
        return None;
    }

    Some(SilenceSpan {
        source_start: TimeCode(span.source_start.0.saturating_add(margin.0)),
        source_end: TimeCode(span.source_end.0.saturating_sub(margin.0)),
    })
}

/// Clamp one raw detector span against cached transcript words for safe cutting.
///
/// Each word interval is protected plus the same fps-aware speech margin on
/// both sides. A detector span containing a word can therefore produce two
/// cuttable spans. Without a transcript, this preserves the original fixed-
/// margin behavior exactly.
#[must_use]
pub fn shrink_silence_span_for_cutting_with_transcript(
    span: SilenceSpan,
    source_fps: Rational,
    transcript_words: Option<&[TranscriptWord]>,
) -> Vec<SilenceSpan> {
    let Some(words) = transcript_words else {
        return shrink_silence_span_for_cutting(span, source_fps)
            .into_iter()
            .collect();
    };
    if span.source_start < TimeCode::ZERO || span.source_end <= span.source_start {
        return Vec::new();
    }

    let margin = silence_cut_margin_frames(source_fps).0;
    let mut cuttable = vec![span];
    for word in words
        .iter()
        .filter(|word| word.source_start >= TimeCode::ZERO && word.source_end > word.source_start)
    {
        let protected_start = TimeCode(word.source_start.0.saturating_sub(margin));
        let protected_end = TimeCode(word.source_end.0.saturating_add(margin));
        let mut next = Vec::with_capacity(cuttable.len().saturating_add(1));
        for candidate in cuttable {
            if protected_end <= candidate.source_start || protected_start >= candidate.source_end {
                next.push(candidate);
                continue;
            }
            if candidate.source_start < protected_start {
                next.push(SilenceSpan {
                    source_start: candidate.source_start,
                    source_end: protected_start.min(candidate.source_end),
                });
            }
            if protected_end < candidate.source_end {
                next.push(SilenceSpan {
                    source_start: protected_end.max(candidate.source_start),
                    source_end: candidate.source_end,
                });
            }
        }
        cuttable = next;
        if cuttable.is_empty() {
            break;
        }
    }
    cuttable.sort_by_key(|candidate| (candidate.source_start, candidate.source_end));
    cuttable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: i64, end: i64) -> SilenceSpan {
        SilenceSpan {
            source_start: TimeCode(start),
            source_end: TimeCode(end),
        }
    }

    fn word(start: i64, end: i64) -> TranscriptWord {
        TranscriptWord {
            text: "word".to_owned(),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            speaker: None,
        }
    }

    #[test]
    fn normal_span_is_shrunk_by_one_hundred_milliseconds_per_side() {
        assert_eq!(
            shrink_silence_span_for_cutting(span(10, 40), Rational::new(30, 1).unwrap()),
            Some(span(13, 37))
        );
    }

    #[test]
    fn span_that_vanishes_is_omitted() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(shrink_silence_span_for_cutting(span(10, 16), fps), None);
        assert_eq!(shrink_silence_span_for_cutting(span(10, 15), fps), None);
    }

    #[test]
    fn span_at_source_zero_keeps_a_valid_half_open_range() {
        assert_eq!(
            shrink_silence_span_for_cutting(span(0, 20), Rational::new(30, 1).unwrap()),
            Some(span(3, 17))
        );
    }

    #[test]
    fn margin_rounds_in_each_assets_source_time_base() {
        for (fps, expected) in [
            (Rational::new(24, 1).unwrap(), 2),
            (Rational::new(25, 1).unwrap(), 3),
            (Rational::new(30_000, 1_001).unwrap(), 3),
            (Rational::new(60_000, 1_001).unwrap(), 6),
        ] {
            assert_eq!(silence_cut_margin_frames(fps), TimeCode(expected));
            assert_eq!(
                shrink_silence_span_for_cutting(span(0, 100), fps),
                Some(span(expected, 100 - expected))
            );
        }
    }

    #[test]
    fn transcript_clamp_removes_word_overlaps_and_protects_both_boundaries() {
        let words = [word(0, 12), word(38, 50)];
        assert_eq!(
            shrink_silence_span_for_cutting_with_transcript(
                span(8, 42),
                Rational::new(30, 1).unwrap(),
                Some(&words),
            ),
            vec![span(15, 35)]
        );
    }

    #[test]
    fn transcript_clamp_retreats_from_adjacent_words() {
        let words = [word(0, 10), word(40, 50)];
        assert_eq!(
            shrink_silence_span_for_cutting_with_transcript(
                span(10, 40),
                Rational::new(30, 1).unwrap(),
                Some(&words),
            ),
            vec![span(13, 37)]
        );
    }

    #[test]
    fn transcript_clamp_keeps_the_cuttable_parts_between_and_around_words() {
        let words = [word(20, 25), word(40, 45)];
        assert_eq!(
            shrink_silence_span_for_cutting_with_transcript(
                span(10, 55),
                Rational::new(30, 1).unwrap(),
                Some(&words),
            ),
            vec![span(10, 17), span(28, 37), span(48, 55)]
        );
    }

    #[test]
    fn no_transcript_uses_the_fixed_margin_unchanged() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(
            shrink_silence_span_for_cutting_with_transcript(span(10, 40), fps, None),
            shrink_silence_span_for_cutting(span(10, 40), fps)
                .into_iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn transcript_clamp_uses_each_assets_fps_margin() {
        let words = [word(0, 10), word(90, 100)];
        for (fps, expected) in [
            (Rational::new(24, 1).unwrap(), span(12, 88)),
            (Rational::new(25, 1).unwrap(), span(13, 87)),
            (Rational::new(30_000, 1_001).unwrap(), span(13, 87)),
            (Rational::new(60_000, 1_001).unwrap(), span(16, 84)),
        ] {
            assert_eq!(
                shrink_silence_span_for_cutting_with_transcript(span(10, 90), fps, Some(&words),),
                vec![expected]
            );
        }
    }

    #[test]
    fn transcript_clamp_omits_zero_length_results() {
        let fps = Rational::new(30, 1).unwrap();
        assert!(
            shrink_silence_span_for_cutting_with_transcript(
                span(10, 20),
                fps,
                Some(&[word(13, 17)]),
            )
            .is_empty()
        );
        assert!(
            shrink_silence_span_for_cutting_with_transcript(span(10, 10), fps, Some(&[]),)
                .is_empty()
        );
    }
}
