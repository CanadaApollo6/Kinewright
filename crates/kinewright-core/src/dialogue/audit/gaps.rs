// Derived from Scarlet packages/mcp/src/gap-sound.ts and dialogue-leftovers.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{
    AuditSettings, Collector, Dialogue, Item, Level, Location, Piece, RecordingId, Text, WordKey,
    measured, text_append,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, Serialize)]
/// A measured voice span outside the configured word-onset support windows.
pub struct Hole {
    pub location: Location,
    /// One continuous voice interval may have several source portions.
    #[serde(skip)]
    pub gap_play_us: [u64; 2],
    /// Index into `Report.hole_groups`. Shared extent is serialized only once.
    pub group: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Nearest spoken source onsets, not word-end boundaries.
    pub after: Option<WordKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<WordKey>,
    /// JSON is `[RMS centidB, peak centidB]`; its window is `location.source_us`.
    #[serde(serialize_with = "hole_level")]
    pub level: Level,
    /// Stored median RMS of voice-flagged hops in this recording.
    pub speaking_cdb: i16,
    /// Strict RMS threshold, shifted by the recording's stored speech level.
    pub threshold_cdb: i16,
    #[serde(skip_serializing_if = "super::zero")]
    pub sound_us: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sound_runs_us: Vec<[u64; 2]>,
    #[serde(skip_serializing_if = "super::zero")]
    pub omitted_runs: u64,
}
fn hole_level<S: serde::Serializer>(level: &Level, serializer: S) -> Result<S::Ok, S::Error> {
    (level.rms_cdb, level.peak_cdb).serialize(serializer)
}

#[derive(Default)]
struct SoundPortion {
    total: u64,
    runs: Vec<[u64; 2]>,
    omitted: u64,
}

// A hole is one playback stretch. Threshold each source against its own
// speaking level, keep an open run across joins, then attribute surviving
// portions back to the source. The minimum never restarts at a source cut.
fn sound_portions(
    recordings: &BTreeMap<RecordingId, &Dialogue>,
    gaps: &[Gap],
    settings: &AuditSettings,
) -> Vec<SoundPortion> {
    let mut result: Vec<_> = (0..gaps.len()).map(|_| SoundPortion::default()).collect();
    let mut open: Vec<(usize, [u64; 2])> = Vec::new();
    let mut duration = 0_u64;
    let mut until = None;
    let close =
        |open: &mut Vec<(usize, [u64; 2])>, duration: &mut u64, result: &mut [SoundPortion]| {
            if *duration >= settings.min_sound_run_us {
                for &(i, span) in open.iter() {
                    let out = &mut result[i];
                    out.total += span[1] - span[0];
                    if out.runs.len() < settings.max_sound_runs {
                        out.runs.push(span);
                    } else {
                        out.omitted += 1;
                    }
                }
            }
            open.clear();
            *duration = 0;
        };
    for (i, gap) in gaps.iter().enumerate() {
        let location = gap.location;
        let recording = &recordings[&location.recording];
        let threshold = recording.thresholds().quiet_rms_max;
        for sample in measured::samples(recording, location.source_us[0], location.source_us[1]) {
            let play = location.play_us[0] + sample.start - location.source_us[0];
            if until != Some(play) {
                close(&mut open, &mut duration, &mut result);
            }
            if sample.hop.voice && sample.hop.rms_dbfs_hundredths > threshold {
                duration += sample.end - sample.start;
                if let Some((_, span)) = open
                    .last_mut()
                    .filter(|(last, span)| *last == i && span[1] == sample.start)
                {
                    span[1] = sample.end;
                } else {
                    open.push((i, [sample.start, sample.end]));
                }
            } else {
                close(&mut open, &mut duration, &mut result);
            }
            until = Some(play + sample.end - sample.start);
        }
    }
    close(&mut open, &mut duration, &mut result);
    result
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

fn voice_gaps(
    recording: &Dialogue,
    p: Piece,
    range: [u64; 2],
    after: Option<WordKey>,
    before: Option<WordKey>,
    out: &mut Vec<Gap>,
) {
    let mut start = None;
    for sample in measured::samples(recording, range[0], range[1]) {
        if sample.hop.voice {
            start.get_or_insert(sample.start);
        } else if let Some(a) = start.take() {
            out.push(Gap {
                location: p.location(a, sample.start),
                after,
                before,
            });
        }
    }
    if let Some(a) = start {
        out.push(Gap {
            location: p.location(a, range[1]),
            after,
            before,
        });
    }
}

fn gaps(
    recordings: &BTreeMap<RecordingId, &Dialogue>,
    pieces: &[Piece],
    tolerance: u64,
) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for &p in pieces {
        let recording = recordings[&p.recording];
        let mut at = p.source.start_micros;
        let mut after = None;
        // Include earlier onsets whose support reaches this kept source range.
        // A source cut never makes an ASR end into a measured speech boundary.
        for (i, w) in recording
            .words()
            .iter()
            .enumerate()
            .filter(|(_, w)| w.is_spoken())
        {
            if w.start_micros >= p.source.end_micros {
                break;
            }
            let until = w.start_micros.min(p.source.end_micros);
            if at < until {
                voice_gaps(
                    recording,
                    p,
                    [at, until],
                    after,
                    Some(WordKey(p.recording, i)),
                    &mut gaps,
                );
            }
            at = at.max(
                w.start_micros
                    .saturating_add(tolerance)
                    .min(p.source.end_micros),
            );
            after = Some(WordKey(p.recording, i));
        }
        if at < p.source.end_micros {
            voice_gaps(
                recording,
                p,
                [at, p.source.end_micros],
                after,
                None,
                &mut gaps,
            );
        }
    }
    gaps
}

fn holes(
    recordings: &BTreeMap<RecordingId, &Dialogue>,
    pieces: &[Piece],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    let gaps = gaps(recordings, pieces, settings.word_start_tolerance_us);
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
        if play[1] - play[0] >= settings.min_sound_run_us {
            let after = gaps[first].after;
            let before = gaps[end - 1].before;
            let sounds = sound_portions(recordings, &gaps[first..end], settings);
            for (gap, sound) in gaps[first..end].iter().zip(sounds) {
                let location = gap.location;
                let recording = &recordings[&location.recording];
                let speech = recording
                    .profile()
                    .levels()
                    .speech_rms_hundredths
                    .expect("prepared dialogue has a measured speech level");
                let threshold = recording.thresholds().quiet_rms_max;
                collector.push(Item::Hole(Hole {
                    location,
                    gap_play_us: play,
                    group: 0,
                    after,
                    before,
                    level: measured::level(recording, location.source_us[0], location.source_us[1])
                        .expect("nonempty hole"),
                    speaking_cdb: speech,
                    threshold_cdb: threshold,
                    sound_us: sound.total,
                    sound_runs_us: sound.runs,
                    omitted_runs: sound.omitted,
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
    recordings: &BTreeMap<RecordingId, &Dialogue>,
    pieces: &[Piece],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    let used: std::collections::BTreeSet<_> = pieces.iter().map(|p| p.recording).collect();
    for id in used {
        let recording = recordings[&id];
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
    recordings: &BTreeMap<RecordingId, &Dialogue>,
    pieces: &[Piece],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    holes(recordings, pieces, settings, collector);
    quiet_phrases(recordings, pieces, settings, collector);
}
