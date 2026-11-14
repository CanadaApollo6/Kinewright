// Derived from Scarlet packages/mcp/src/dialogue-leftovers.ts and packages/shared/src/adjacentRepeats.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{
    AuditSettings, Collector, Dialogue, Item, Piece, PlayedWord, RecordingId, Text, WordKey,
    text_append,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Pause {
    pub source: [(RecordingId, u64); 2],
    pub play_us: [u64; 2],
    pub after: WordKey,
    pub before: WordKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_word: Option<WordKey>,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Repeat {
    pub source: (RecordingId, [u64; 2]),
    pub second_source: (RecordingId, u64),
    pub play_us: [u64; 2],
    pub from_word: WordKey,
    pub to_word: WordKey,
    pub second_word: WordKey,
    pub words: usize,
    pub at_join: bool,
    #[serde(flatten)]
    pub said: Text,
}

pub(super) fn played(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    pieces: &[Piece],
    tokens: &BTreeMap<RecordingId, Vec<String>>,
) -> Vec<PlayedWord> {
    let mut heard = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let words = recordings[&piece.recording].words();
        let from = words.partition_point(|w| w.end_micros <= piece.source.start_micros);
        for (i, w) in words
            .iter()
            .enumerate()
            .skip(from)
            .take_while(|(_, w)| w.start_micros < piece.source.end_micros)
        {
            let covered = w
                .end_micros
                .min(piece.source.end_micros)
                .saturating_sub(w.start_micros.max(piece.source.start_micros));
            if !w.is_spoken()
                || tokens[&piece.recording][i].is_empty()
                || u128::from(covered) * 2 < u128::from(w.end_micros - w.start_micros)
            {
                continue;
            }
            let a = w.start_micros.max(piece.source.start_micros);
            let b = w.end_micros.min(piece.source.end_micros);
            heard.push(PlayedWord {
                key: WordKey(piece.recording, i),
                start: piece.play + (a - piece.source.start_micros),
                end: piece.play + (b - piece.source.start_micros),
                piece: index,
                source: [a, b],
            });
        }
    }
    heard
}

fn ends_sentence(text: &str) -> bool {
    text.trim()
        .trim_end_matches(['"', '\'', '”', '’', ')', ']'])
        .ends_with(['.', '?', '!'])
}

pub(super) fn said_text(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    said: &[PlayedWord],
    limit: usize,
) -> Text {
    let mut text = String::new();
    let mut complete = true;
    for (i, w) in said.iter().enumerate() {
        if i > 0 && !text_append(&mut text, " ", limit) {
            complete = false;
            break;
        }
        if !text_append(
            &mut text,
            recordings[&w.key.0].words()[w.key.1].text.trim(),
            limit,
        ) {
            complete = false;
            break;
        }
    }
    Text {
        text,
        truncated: !complete,
    }
}

pub(super) fn inspect(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    heard: &[PlayedWord],
    tokens: &BTreeMap<RecordingId, Vec<String>>,
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    for pair in heard.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        if right.start.saturating_sub(left.end) < settings.kept_pause_us {
            continue;
        }
        let original = &recordings[&left.key.0].words()[left.key.1];
        let after_word = (left.piece == right.piece
            && left.key.0 == right.key.0
            && right.key.1 == left.key.1 + 1
            && left.source[0] == original.start_micros
            && left.source[1] == original.end_micros)
            .then_some(left.key);
        collector.push(Item::Pause(Pause {
            source: [(left.key.0, left.source[1]), (right.key.0, right.source[0])],
            play_us: [left.end, right.start],
            after: left.key,
            before: right.key,
            after_word,
        }));
    }
    let mut i = 0;
    while i + 1 < heard.len() {
        let mut found = 0;
        let horizon = heard.partition_point(|w| {
            w.start <= heard[i].start.saturating_add(settings.repeat_lookahead_us)
        });
        let available = settings
            .short_repeat_words
            .max(horizon.saturating_sub(i + 1))
            .min((heard.len() - i) / 2);
        if available > settings.max_repeat_words
            && settings
                .playback_us
                .is_none_or(|r| heard[i].end >= r[0] && heard[i].start < r[1])
        {
            collector.repeat_search_limited += 1;
        }
        let max_words = available.min(settings.max_repeat_words);
        for n in (1..=max_words).rev() {
            if n > settings.short_repeat_words
                && heard[i + n].start.saturating_sub(heard[i].start) > settings.repeat_lookahead_us
            {
                continue;
            }
            if tokens[&heard[i].key.0][heard[i].key.1]
                != tokens[&heard[i + n].key.0][heard[i + n].key.1]
            {
                continue;
            }
            let saying = &heard[i..i + 2 * n];
            if saying[..n].iter().any(|w| w.key.0 != heard[i].key.0)
                || heard[i + n].start.saturating_sub(heard[i + n - 1].end)
                    >= settings.repeat_within_us
                || n == 1
                    && ends_sentence(&recordings[&heard[i].key.0].words()[heard[i].key.1].text)
                || (0..n).any(|k| heard[i + k].key.1 != heard[i].key.1 + k)
                || (0..n).any(|k| {
                    tokens[&heard[i + k].key.0][heard[i + k].key.1]
                        != tokens[&heard[i + n + k].key.0][heard[i + n + k].key.1]
                })
            {
                continue;
            }
            found = n;
            break;
        }
        if found == 0 {
            i += 1;
            continue;
        }
        let saying = &heard[i..i + 2 * found];
        let across_join = saying.iter().any(|w| w.piece != saying[0].piece);
        collector.push(Item::Repeat(Repeat {
            source: (
                heard[i].key.0,
                [heard[i].source[0], heard[i + found - 1].source[1]],
            ),
            second_source: (heard[i + found].key.0, heard[i + found].source[0]),
            play_us: [heard[i].start, heard[i + 2 * found - 1].end],
            from_word: heard[i].key,
            to_word: heard[i + found - 1].key,
            second_word: heard[i + found].key,
            words: found,
            at_join: across_join,
            said: said_text(recordings, saying, settings.max_excerpt_bytes),
        }));
        i += found;
    }
}
