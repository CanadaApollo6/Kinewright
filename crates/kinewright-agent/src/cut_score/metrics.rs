// Derived from Scarlet evals/src/metrics.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{
    Candidate, CannotScore, Case, Pacing, PrecisionRecall, Ratio, RequiredLine, ScoreSettings,
    SourceRange, intervals, pacing, validate,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSide {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub at_micros: u64,
    pub side: EdgeSide,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EdgeMatch {
    pub candidate_index: usize,
    pub reference_index: usize,
    pub error_micros: i128,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EdgeTolerance {
    pub tolerance_micros: u64,
    pub reference_share: Ratio,
    pub removal: PrecisionRecall,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EdgeComparison {
    pub candidate: Vec<Edge>,
    pub reference: Vec<Edge>,
    pub within: Vec<EdgeTolerance>,
    pub matches: Vec<EdgeMatch>,
    pub median_abs_error_micros: Option<Ratio>,
    pub p90_abs_error_micros: Option<Ratio>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct MissingLine {
    pub from_word: usize,
    pub to_word: usize,
    pub source: SourceRange,
    pub text: String,
    pub cut_share: Ratio,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RequiredLines {
    pub required: usize,
    pub missing: Vec<MissingLine>,
    /// Union duration: overlapping explicit lines are not charged twice.
    pub missing_micros: u128,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Score {
    /// Same source sequence after only source-contiguous splits are coalesced.
    pub reference_equivalent: bool,
    pub words_in_scope: usize,
    pub removed_words: PrecisionRecall,
    pub candidate_removed_micros: u128,
    pub reference_removed_micros: u128,
    pub overlap_removed_micros: u128,
    pub removed_time: PrecisionRecall,
    pub edges: EdgeComparison,
    pub required_lines: RequiredLines,
    pub pacing: Pacing,
    /// Deliberately absent without a run trace; file scoring cannot infer it.
    pub time_to_first_change_micros: Option<u64>,
}

fn edges(kept: &[SourceRange], duration: u64) -> Vec<Edge> {
    let mut out = Vec::new();
    for (i, r) in kept.iter().enumerate() {
        if i > 0 || r.start_micros != 0 {
            out.push(Edge {
                at_micros: r.start_micros,
                side: EdgeSide::In,
            });
        }
        if i + 1 < kept.len() || r.end_micros != duration {
            out.push(Edge {
                at_micros: r.end_micros,
                side: EdgeSide::Out,
            });
        }
    }
    out
}

pub(super) fn match_edges(candidate: &[Edge], reference: &[Edge], window: u64) -> Vec<EdgeMatch> {
    let mut possible = Vec::new();
    for (i, c) in candidate.iter().enumerate() {
        for (j, r) in reference.iter().enumerate() {
            if c.side == r.side && c.at_micros.abs_diff(r.at_micros) <= window {
                possible.push((c.at_micros.abs_diff(r.at_micros), i, j));
            }
        }
    }
    possible.sort_unstable();
    let mut used_candidate = vec![false; candidate.len()];
    let mut used_reference = vec![false; reference.len()];
    let mut out = Vec::new();
    for (_, i, j) in possible {
        if used_candidate[i] || used_reference[j] {
            continue;
        }
        used_candidate[i] = true;
        used_reference[j] = true;
        out.push(EdgeMatch {
            candidate_index: i,
            reference_index: j,
            error_micros: i128::from(candidate[i].at_micros) - i128::from(reference[j].at_micros),
        });
    }
    out
}

fn compare_edges(
    candidate: &[SourceRange],
    reference: &[SourceRange],
    duration: u64,
    settings: &ScoreSettings,
) -> EdgeComparison {
    let candidate = edges(candidate, duration);
    let reference = edges(reference, duration);
    let within = settings
        .edge_tolerances_micros
        .iter()
        .map(|&tolerance| {
            let matched = match_edges(&candidate, &reference, tolerance).len() as u128;
            EdgeTolerance {
                tolerance_micros: tolerance,
                reference_share: Ratio::new(matched, reference.len() as u128),
                removal: PrecisionRecall::new(
                    matched,
                    candidate.len() as u128 - matched,
                    reference.len() as u128 - matched,
                ),
            }
        })
        .collect();
    let matches = match_edges(&candidate, &reference, settings.edge_match_window_micros);
    let errors: Vec<_> = matches
        .iter()
        .map(|m| {
            u64::try_from(m.error_micros.unsigned_abs())
                .expect("difference between u64 times fits u64")
        })
        .collect();
    EdgeComparison {
        within,
        matches,
        median_abs_error_micros: intervals::quantile(&errors, 1, 2),
        p90_abs_error_micros: intervals::quantile(&errors, 9, 10),
        candidate,
        reference,
    }
}

pub(super) fn ends_sentence(text: &str) -> bool {
    text.trim()
        .trim_end_matches(['"', '\'', '”', '’', ')', ']'])
        .ends_with(['.', '?', '!'])
}

fn sentences(case: &Case, gap: u64) -> Vec<RequiredLine> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, w) in case.words.iter().enumerate() {
        if i > from && w.start_micros.saturating_sub(case.words[i - 1].end_micros) > gap {
            out.push(RequiredLine {
                from_word: from,
                to_word: i - 1,
            });
            from = i;
        }
        if ends_sentence(&w.text) {
            out.push(RequiredLine {
                from_word: from,
                to_word: i,
            });
            from = i + 1;
        }
    }
    if from < case.words.len() {
        out.push(RequiredLine {
            from_word: from,
            to_word: case.words.len() - 1,
        });
    }
    out
}

fn required_lines(
    case: &Case,
    candidate_kept: &[bool],
    reference_kept: &[bool],
    settings: &ScoreSettings,
) -> RequiredLines {
    let inferred;
    let lines = if let Some(lines) = &case.required_lines {
        lines.as_slice()
    } else {
        inferred = sentences(case, settings.sentence_gap_micros);
        &inferred
    };
    let mut required = 0;
    let mut missing = Vec::new();
    for line in lines {
        let indices = line.from_word..=line.to_word;
        let count = line.to_word - line.from_word + 1;
        let reference_count = indices.clone().filter(|&i| reference_kept[i]).count();
        if case.required_lines.is_none()
            && !settings.required_keep_share.reached(reference_count, count)
        {
            continue;
        }
        required += 1;
        let cut = indices.filter(|&i| !candidate_kept[i]).count();
        if settings.missing_cut_share.reached(cut, count) {
            missing.push(MissingLine {
                from_word: line.from_word,
                to_word: line.to_word,
                source: SourceRange {
                    start_micros: case.words[line.from_word].start_micros,
                    end_micros: case.words[line.to_word].end_micros,
                },
                text: case.words[line.from_word..=line.to_word]
                    .iter()
                    .map(|w| w.text.trim())
                    .collect::<Vec<_>>()
                    .join(" "),
                cut_share: Ratio::new(cut as u128, count as u128),
            });
        }
    }
    let missing_ranges: Vec<_> = missing.iter().map(|line| line.source).collect();
    let missing_micros = intervals::total(&intervals::merge(&missing_ranges));
    RequiredLines {
        required,
        missing,
        missing_micros,
    }
}

/// Compare one candidate with its reference. The base is the entire recording.
/// No weighted aggregate invents a trade-off between lost words and dead air.
///
/// # Errors
/// Refuses malformed cases, mismatched recording contents and invalid settings.
pub fn score(
    case: &Case,
    candidate: &Candidate,
    settings: &ScoreSettings,
) -> Result<Score, CannotScore> {
    validate(case, candidate, settings)?;
    let candidate_coverage = intervals::merge(&candidate.kept);
    let reference_coverage = intervals::merge(&case.reference_kept);
    let candidate_pieces = intervals::pieces(&candidate.kept);
    let reference_pieces = intervals::pieces(&case.reference_kept);
    let candidate_removed = intervals::removed(&candidate_coverage, case.duration_micros);
    let reference_removed = intervals::removed(&reference_coverage, case.duration_micros);
    let candidate_removed_micros = intervals::total(&candidate_removed);
    let reference_removed_micros = intervals::total(&reference_removed);
    let overlap_removed_micros = intervals::intersection(&candidate_removed, &reference_removed);
    let labels = |kept: &[SourceRange]| -> Vec<_> {
        case.words
            .iter()
            .map(|w| intervals::kept_word(w.start_micros, w.end_micros, kept))
            .collect()
    };
    let candidate_labels = labels(&candidate_coverage);
    let reference_labels = labels(&reference_coverage);
    let (mut tp, mut fp, mut missed) = (0_u128, 0_u128, 0_u128);
    for (&c, &r) in candidate_labels.iter().zip(&reference_labels) {
        match (c, r) {
            (false, false) => tp += 1,
            (false, true) => fp += 1,
            (true, false) => missed += 1,
            (true, true) => {}
        }
    }
    Ok(Score {
        reference_equivalent: candidate_pieces == reference_pieces,
        words_in_scope: case.words.len(),
        removed_words: PrecisionRecall::new(tp, fp, missed),
        candidate_removed_micros,
        reference_removed_micros,
        overlap_removed_micros,
        removed_time: PrecisionRecall::new(
            overlap_removed_micros,
            candidate_removed_micros - overlap_removed_micros,
            reference_removed_micros - overlap_removed_micros,
        ),
        edges: compare_edges(
            &candidate_pieces,
            &reference_pieces,
            case.duration_micros,
            settings,
        ),
        required_lines: required_lines(case, &candidate_labels, &reference_labels, settings),
        pacing: pacing::measure(&case.words, &candidate_pieces, settings)?,
        time_to_first_change_micros: None,
    })
}
