use openreel_core::{Rational, SilenceSpan, TimeCode};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: i64, end: i64) -> SilenceSpan {
        SilenceSpan {
            source_start: TimeCode(start),
            source_end: TimeCode(end),
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
}
