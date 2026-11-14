// Derived from Scarlet packages/mcp/src/gap-sound.ts and dialogue-leftovers.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{
    AuditSettings, Collector, Dialogue, Item, Level, Location, Piece, PlayedWord, RecordingId,
    Text, WordKey, measured, text_append,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Hole {
    pub location: Location,
    /// One wordless playback interval may have several source portions.
    pub gap_play_us: [u64; 2],
    pub after: Option<WordKey>,
    pub before: Option<WordKey>,
    pub level: Level,
    pub speaking_cdb: i16,
    pub threshold_cdb: i16,
    pub sound_us: u64,
    pub sound_runs_us: Vec<[u64; 2]>,
    pub omitted_runs: u64,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct QuietPhrase {
    pub location: Location,
    pub from_word: WordKey,
    pub to_word: WordKey,
    pub level_cdb: i16,
    /// Original-source neighbor deliveries, not a rendered mix comparison.
    pub neighbors_cdb: [i16; 2],
    #[serde(flatten)]
    pub said: Text,
}

struct Gap {
    location: Location,
    after: Option<WordKey>,
    before: Option<WordKey>,
}

fn gaps(recordings: &BTreeMap<RecordingId, Dialogue>, pieces: &[Piece]) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for &p in pieces {
        let mut at = p.source.start_micros;
        let mut after = None;
        for (i, w) in recordings[&p.recording]
            .words()
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.is_spoken()
                    && w.end_micros > p.source.start_micros
                    && w.start_micros < p.source.end_micros
            })
        {
            let until = w.start_micros.min(p.source.end_micros);
            if at < until {
                gaps.push(Gap {
                    location: p.location(at, until),
                    after,
                    before: Some(WordKey(p.recording, i)),
                });
            }
            at = at.max(w.end_micros.min(p.source.end_micros));
            after = Some(WordKey(p.recording, i));
        }
        if at < p.source.end_micros {
            gaps.push(Gap {
                location: p.location(at, p.source.end_micros),
                after,
                before: None,
            });
        }
    }
    gaps
}

fn holes(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    pieces: &[Piece],
    heard: &[PlayedWord],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    let gaps = gaps(recordings, pieces);
    let speech: BTreeMap<_, _> = pieces
        .iter()
        .map(|p| p.recording)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|id| (id, measured::speaking_level(&recordings[&id], settings)))
        .collect();
    let mut first = 0;
    while first < gaps.len() {
        let mut end = first + 1;
        while end < gaps.len() && gaps[end - 1].location.play_us[1] == gaps[end].location.play_us[0]
        {
            end += 1;
        }
        let play = [
            gaps[first].location.play_us[0],
            gaps[end - 1].location.play_us[1],
        ];
        if play[1] - play[0] >= settings.gap_us {
            let left = heard.partition_point(|w| w.end <= play[0]);
            let right = heard.partition_point(|w| w.start < play[1]);
            let after = gaps[first]
                .after
                .or_else(|| left.checked_sub(1).map(|i| heard[i].key));
            let before = gaps[end - 1]
                .before
                .or_else(|| heard.get(right).map(|w| w.key));
            for gap in &gaps[first..end] {
                let location = gap.location;
                let recording = &recordings[&location.recording];
                let speech = speech[&location.recording];
                let threshold = i32::from(speech)
                    .saturating_sub(i32::from(settings.below_speech_cdb))
                    .max(i32::from(settings.quietest_sound_cdb));
                let threshold =
                    i16::try_from(threshold).expect("threshold is bounded by valid levels");
                // A source cut is not a word boundary. Pad only the real word
                // ends so small portions of a cross-join hole are still read.
                let pad_after = gap.after.is_some_and(|k| {
                    recordings[&k.0].words()[k.1].end_micros == location.source_us[0]
                });
                let pad_before = gap.before.is_some_and(|k| {
                    recordings[&k.0].words()[k.1].start_micros == location.source_us[1]
                });
                let a = location.source_us[0]
                    .saturating_add(if pad_after { settings.word_edge_us } else { 0 })
                    .min(location.source_us[1]);
                let b = location.source_us[1]
                    .saturating_sub(if pad_before { settings.word_edge_us } else { 0 })
                    .max(a);
                let (sound_us, sound_runs_us, omitted_runs) =
                    measured::sound_runs(recording, a, b, threshold, settings);
                collector.push(Item::Hole(Hole {
                    location,
                    gap_play_us: play,
                    after,
                    before,
                    level: measured::level(recording, location.source_us[0], location.source_us[1])
                        .expect("nonempty hole"),
                    speaking_cdb: speech,
                    threshold_cdb: threshold,
                    sound_us,
                    sound_runs_us,
                    omitted_runs,
                }));
            }
        }
        first = end;
    }
}

struct Phrase {
    from: usize,
    to: usize,
    start: u64,
    end: u64,
}

fn phrase_text(words: &[crate::dialogue::Word], limit: usize) -> Text {
    let mut text = String::new();
    let mut complete = true;
    for (count, w) in words.iter().filter(|w| w.is_spoken()).enumerate() {
        if count > 0 && !text_append(&mut text, " ", limit) {
            complete = false;
            break;
        }
        if !text_append(&mut text, w.text.trim(), limit) {
            complete = false;
            break;
        }
    }
    Text {
        text,
        truncated: !complete,
    }
}

fn quiet_phrases(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    pieces: &[Piece],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    let used: std::collections::BTreeSet<_> = pieces.iter().map(|p| p.recording).collect();
    for id in used {
        let recording = &recordings[&id];
        let words = recording.words();
        let mut phrases: Vec<Phrase> = Vec::new();
        for (i, w) in words.iter().enumerate().filter(|(_, w)| w.is_spoken()) {
            if let Some(last) = phrases
                .last_mut()
                .filter(|p| w.start_micros.saturating_sub(p.end) < settings.gap_us)
            {
                last.to = i;
                last.end = last.end.max(w.end_micros);
            } else {
                phrases.push(Phrase {
                    from: i,
                    to: i,
                    start: w.start_micros,
                    end: w.end_micros,
                });
            }
        }
        for window in phrases.windows(3) {
            let (before, p, after) = (&window[0], &window[1], &window[2]);
            if p.end - p.start > settings.max_phrase_us
                || p.start.saturating_sub(before.end) > settings.max_neighbor_gap_us
                || after.start.saturating_sub(p.end) > settings.max_neighbor_gap_us
            {
                continue;
            }
            let Some(level) =
                measured::phrase_level(recording, p.from, p.to, p.start, p.end, settings)
            else {
                continue;
            };
            if level > settings.quietest_sound_cdb {
                continue;
            }
            let Some(left) = measured::phrase_level(
                recording,
                before.from,
                before.to,
                before
                    .end
                    .saturating_sub(settings.neighbor_us)
                    .max(before.start),
                before.end,
                settings,
            ) else {
                continue;
            };
            let Some(right) = measured::phrase_level(
                recording,
                after.from,
                after.to,
                after.start,
                after
                    .start
                    .saturating_add(settings.neighbor_us)
                    .min(after.end),
                settings,
            ) else {
                continue;
            };
            if i32::from(left.min(right)) - i32::from(level)
                < i32::from(settings.quiet_phrase_contrast_cdb)
            {
                continue;
            }
            for &piece in pieces.iter().filter(|piece| {
                piece.recording == id
                    && piece.source.start_micros <= p.start
                    && piece.source.end_micros >= p.end
            }) {
                collector.push(Item::QuietPhrase(QuietPhrase {
                    location: piece.location(p.start, p.end),
                    from_word: WordKey(id, p.from),
                    to_word: WordKey(id, p.to),
                    level_cdb: level,
                    neighbors_cdb: [left, right],
                    said: phrase_text(&words[p.from..=p.to], settings.max_excerpt_bytes),
                }));
            }
        }
    }
}

pub(super) fn inspect(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    pieces: &[Piece],
    heard: &[PlayedWord],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    holes(recordings, pieces, heard, settings, collector);
    quiet_phrases(recordings, pieces, settings, collector);
}
