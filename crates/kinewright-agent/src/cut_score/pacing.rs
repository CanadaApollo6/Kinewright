// Derived from Scarlet evals/src/metrics.ts and timeline.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{CannotScore, Ratio, ScoreSettings, SourceRange, intervals, metrics::ends_sentence};
use kinewright_core::dialogue::Word;
use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PacingGap {
    pub at_micros: u64,
    pub duration_micros: u64,
    pub after_word: usize,
    pub before_word: usize,
    pub join: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PacingRepeat {
    pub at_micros: u64,
    pub text: String,
    pub words: usize,
    pub join: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Pacing {
    pub played_words: usize,
    pub joins: usize,
    pub gaps_inside: usize,
    pub gaps_at_joins: usize,
    pub long_gaps: usize,
    pub excess_gap_micros: u128,
    pub median_join_gap_micros: Option<Ratio>,
    pub repeated_single_words: usize,
    pub repeated_phrases: usize,
    pub repeats_at_joins: usize,
    pub gaps: Vec<PacingGap>,
    pub repeats: Vec<PacingRepeat>,
}

#[derive(Debug, Clone, Copy)]
struct PlayedWord {
    word: usize,
    start: u64,
    end: u64,
    piece: usize,
}

fn token(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn played_words(words: &[Word], pieces: &[SourceRange]) -> Result<Vec<PlayedWord>, CannotScore> {
    let mut played = Vec::new();
    let mut offset = 0_u64;
    for (piece, range) in pieces.iter().enumerate() {
        for (word, w) in words.iter().enumerate() {
            let middle = u64::midpoint(w.start_micros, w.end_micros);
            if range.start_micros <= middle && middle < range.end_micros {
                played.push(PlayedWord {
                    word,
                    piece,
                    start: offset
                        .checked_add(w.start_micros.max(range.start_micros) - range.start_micros)
                        .ok_or(CannotScore::TimeOverflow)?,
                    end: offset
                        .checked_add(w.end_micros.min(range.end_micros) - range.start_micros)
                        .ok_or(CannotScore::TimeOverflow)?,
                });
            }
        }
        offset = offset
            .checked_add(range.end_micros - range.start_micros)
            .ok_or(CannotScore::TimeOverflow)?;
    }
    played.sort_by_key(|w| w.start);
    Ok(played)
}

pub(super) fn measure(
    words: &[Word],
    pieces: &[SourceRange],
    settings: &ScoreSettings,
) -> Result<Pacing, CannotScore> {
    let tokens: Vec<_> = words.iter().map(|w| token(&w.text)).collect();
    let stream: Vec<_> = played_words(words, pieces)?
        .into_iter()
        .filter(|w| {
            let text = words[w.word].text.trim();
            !(tokens[w.word].is_empty() || text.starts_with('[') && text.ends_with(']'))
        })
        .collect();
    let mut gaps = Vec::new();
    let mut joins = 0;
    let mut join_gaps = Vec::new();
    if let Some(mut left) = stream.first().copied() {
        for &right in stream.iter().skip(1) {
            let gap = right.start.saturating_sub(left.end);
            let join = left.piece != right.piece;
            if join {
                joins += 1;
                join_gaps.push(gap);
            }
            if gap >= settings.gap_micros {
                gaps.push(PacingGap {
                    at_micros: left.end,
                    duration_micros: gap,
                    after_word: left.word,
                    before_word: right.word,
                    join,
                });
            }
            if join || right.end >= left.end {
                left = right;
            }
        }
    }
    let mut repeats = Vec::new();
    let mut i = 0;
    let mut last_restart = 0;
    while i + 1 < stream.len() {
        while last_restart + 1 < stream.len()
            && stream[last_restart + 1]
                .start
                .saturating_sub(stream[i].start)
                <= settings.repeat_lookahead_micros
        {
            last_restart += 1;
        }
        let max_words = settings
            .repeat_words
            .unwrap_or_else(|| 5.max(last_restart.saturating_sub(i)));
        let mut found = 0;
        for n in (1..=max_words.min((stream.len() - i) / 2)).rev() {
            if tokens[stream[i].word] != tokens[stream[i + n].word]
                || i128::from(stream[i + n].start) - i128::from(stream[i + n - 1].end)
                    >= i128::from(settings.repeat_within_micros)
                || n == 1 && ends_sentence(&words[stream[i].word].text)
            {
                continue;
            }
            if (1..n).all(|j| tokens[stream[i + j].word] == tokens[stream[i + n + j].word]) {
                found = n;
                break;
            }
        }
        if found == 0 {
            i += 1;
            continue;
        }
        let said = &stream[i..i + 2 * found];
        repeats.push(PacingRepeat {
            at_micros: said[0].start,
            text: said
                .iter()
                .map(|w| words[w.word].text.trim())
                .collect::<Vec<_>>()
                .join(" "),
            words: found,
            join: said.iter().any(|w| w.piece != said[0].piece),
        });
        i += found;
    }
    Ok(Pacing {
        played_words: stream.len(),
        joins,
        gaps_inside: gaps.iter().filter(|g| !g.join).count(),
        gaps_at_joins: gaps.iter().filter(|g| g.join).count(),
        long_gaps: gaps
            .iter()
            .filter(|g| g.duration_micros >= settings.long_gap_micros)
            .count(),
        excess_gap_micros: gaps
            .iter()
            .map(|g| u128::from(g.duration_micros - settings.beat_micros))
            .sum(),
        median_join_gap_micros: intervals::quantile(&join_gaps, 1, 2),
        repeated_single_words: repeats.iter().filter(|r| r.words == 1).count(),
        repeated_phrases: repeats.iter().filter(|r| r.words > 1).count(),
        repeats_at_joins: repeats.iter().filter(|r| r.join).count(),
        gaps,
        repeats,
    })
}
