// Derived from Scarlet evals/src/intervals.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{Ratio, SourceRange};

pub(super) fn merge(ranges: &[SourceRange]) -> Vec<SourceRange> {
    let mut sorted = Vec::from(ranges);
    sorted.sort_by_key(|r| (r.start_micros, r.end_micros));
    let mut out: Vec<SourceRange> = Vec::new();
    for r in sorted {
        if let Some(last) = out
            .last_mut()
            .filter(|last| r.start_micros <= last.end_micros)
        {
            last.end_micros = last.end_micros.max(r.end_micros);
        } else {
            out.push(r);
        }
    }
    out
}

/// Adjacent source-contiguous clips are one playback piece, even when a
/// harmless split crosses a word. Order and repeated occurrences survive.
pub(super) fn pieces(ranges: &[SourceRange]) -> Vec<SourceRange> {
    let mut out: Vec<SourceRange> = Vec::new();
    for &r in ranges {
        if let Some(last) = out
            .last_mut()
            .filter(|last| last.end_micros == r.start_micros)
        {
            last.end_micros = r.end_micros;
        } else {
            out.push(r);
        }
    }
    out
}

pub(super) fn removed(kept: &[SourceRange], duration: u64) -> Vec<SourceRange> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in kept {
        if at < r.start_micros {
            out.push(SourceRange {
                start_micros: at,
                end_micros: r.start_micros,
            });
        }
        at = r.end_micros;
    }
    if at < duration {
        out.push(SourceRange {
            start_micros: at,
            end_micros: duration,
        });
    }
    out
}

pub(super) fn overlap(a: SourceRange, b: SourceRange) -> u64 {
    a.end_micros
        .min(b.end_micros)
        .saturating_sub(a.start_micros.max(b.start_micros))
}

pub(super) fn intersection(a: &[SourceRange], b: &[SourceRange]) -> u128 {
    let (mut i, mut j, mut total) = (0, 0, 0_u128);
    while i < a.len() && j < b.len() {
        total += u128::from(overlap(a[i], b[j]));
        if a[i].end_micros < b[j].end_micros {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
}

pub(super) fn total(ranges: &[SourceRange]) -> u128 {
    ranges
        .iter()
        .map(|r| u128::from(r.end_micros - r.start_micros))
        .sum()
}

/// One joined playback occurrence must retain at least half. Neither disjoint
/// slivers nor repeated small fragments can add up to a retained word.
pub(super) fn kept_word(start: u64, end: u64, kept: &[SourceRange]) -> bool {
    let r = SourceRange {
        start_micros: start,
        end_micros: end,
    };
    kept.iter()
        .any(|&k| u128::from(overlap(r, k)) * 2 >= u128::from(end - start))
}

pub(super) fn quantile(values: &[u64], numerator: u32, denominator: u32) -> Option<Ratio> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = Vec::from(values);
    sorted.sort_unstable();
    let position = (sorted.len() - 1) as u128 * u128::from(numerator);
    let d = u128::from(denominator);
    let lo = usize::try_from(position / d).expect("quantile index within values");
    let fraction = position % d;
    if fraction == 0 {
        return Some(Ratio::new(u128::from(sorted[lo]), 1));
    }
    let hi = (lo + 1).min(sorted.len() - 1);
    Some(Ratio::new(
        u128::from(sorted[lo]) * (d - fraction) + u128::from(sorted[hi]) * fraction,
        d,
    ))
}
