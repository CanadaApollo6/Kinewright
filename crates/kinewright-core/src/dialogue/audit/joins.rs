// Derived from Scarlet packages/mcp/src/timeline-perception.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::{
    AudibleSide, AuditSettings, Collector, Dialogue, Item, Piece, RecordingId, WordKey, measured,
};
use crate::dialogue::{DipIntrusion, RefusalReason};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Level {
    pub us: [u64; 2],
    /// Time-weighted mean of the per-hop dB readings, not an energy mix.
    pub rms_cdb: i16,
    pub peak_cdb: i16,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardIssue {
    SoundingWord,
    UntranscribedSound,
    Unmeasured,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EdgeInSound {
    pub reason: GuardIssue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word: Option<WordKey>,
    pub quiet_us: [Option<u64>; 2],
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Side {
    pub side: AudibleSide,
    /// Recording identity and source microsecond instant.
    pub source: (RecordingId, u64),
    pub level: Option<Level>,
    pub voice: bool,
    pub air_us: u64,
    pub low_air_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_in_sound: Option<EdgeInSound>,
    /// Independent warning: a measured dip inside the named word timing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dip_intrusion: Option<DipIntrusion>,
    /// ASR timing overlap when the guard's word margin leaves it unnamed.
    /// This is a candidate name, not a second guard classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timed_word: Option<WordKey>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub short_air: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word: Option<WordKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_air_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stretched_word: Option<WordKey>,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RoomTone {
    pub delta_cdb: i32,
    pub comparable: bool,
    pub flagged: bool,
}
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Join {
    pub at_us: u64,
    /// One side at the program head/tail, two at an interior source jump.
    pub sides: Vec<Side>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_tone: Option<RoomTone>,
}

fn sound_issue(
    recording: &Dialogue,
    id: RecordingId,
    at: u64,
    direction: AudibleSide,
) -> Option<EdgeInSound> {
    recording.guard(at, direction).err().map(|refusal| {
        let (reason, word) = match refusal.reason {
            RefusalReason::SoundingWord { index, .. } => {
                (GuardIssue::SoundingWord, Some(WordKey(id, index)))
            }
            RefusalReason::UntranscribedSound => (GuardIssue::UntranscribedSound, None),
            _ => (GuardIssue::Unmeasured, None),
        };
        EdgeInSound {
            reason,
            word,
            quiet_us: [refusal.quiet_before, refusal.quiet_after],
        }
    })
}

fn inspect_side(
    recording: &Dialogue,
    piece: Piece,
    outgoing: bool,
    settings: &AuditSettings,
) -> Side {
    let at = if outgoing {
        piece.source.end_micros
    } else {
        piece.source.start_micros
    };
    let direction = if outgoing {
        AudibleSide::Out
    } else {
        AudibleSide::In
    };
    let edge_in_sound = sound_issue(recording, piece.recording, at, direction);
    let span = |reach: u64| {
        if outgoing {
            [at.saturating_sub(reach).max(piece.source.start_micros), at]
        } else {
            [at, at.saturating_add(reach).min(piece.source.end_micros)]
        }
    };
    let window = span(settings.level_window_us);
    let voice = span(settings.voice_window_us);
    let air_us = measured::air(
        recording,
        piece.source.start_micros,
        piece.source.end_micros,
        outgoing,
        true,
        settings,
    );
    let low_air_us = measured::air(
        recording,
        piece.source.start_micros,
        piece.source.end_micros,
        outgoing,
        false,
        settings,
    );
    let words = recording.words();
    let timed_word = edge_in_sound
        .as_ref()
        .filter(|flag| flag.word.is_none())
        .and_then(|_| {
            words
                .iter()
                .position(|w| w.is_spoken() && w.start_micros < at && at < w.end_micros)
                .map(|i| WordKey(piece.recording, i))
        });
    let word = if outgoing {
        words.iter().enumerate().rev().find(|(_, w)| {
            w.is_spoken() && w.start_micros >= piece.source.start_micros && w.end_micros <= at
        })
    } else {
        words.iter().enumerate().find(|(_, w)| {
            w.is_spoken() && w.start_micros >= at && w.end_micros <= piece.source.end_micros
        })
    };
    let crossing = words.iter().enumerate().find(|(_, w)| {
        w.is_spoken()
            && w.start_micros.saturating_add(settings.word_margin_us) < at
            && at < w.end_micros.saturating_sub(settings.word_margin_us)
    });
    let word_air_us = word.and_then(|(_, w)| {
        if crossing.is_some() {
            None
        } else {
            Some(if outgoing {
                at - w.end_micros
            } else {
                w.start_micros - at
            })
        }
    });
    let stretched_word = crossing
        .filter(|_| edge_in_sound.is_none() && low_air_us >= settings.stretched_word_air_us)
        .map(|(i, _)| WordKey(piece.recording, i));
    Side {
        side: direction,
        source: (piece.recording, at),
        level: measured::level(recording, window[0], window[1]),
        voice: measured::samples(recording, voice[0], voice[1]).any(|s| s.hop.voice),
        air_us,
        low_air_us,
        edge_in_sound,
        dip_intrusion: recording.dip_intrusion(at),
        timed_word,
        short_air: air_us
            < if outgoing {
                settings.min_air_after_us
            } else {
                settings.min_air_before_us
            },
        word: word.map(|(i, _)| WordKey(piece.recording, i)),
        word_air_us,
        stretched_word,
    }
}

pub(super) fn inspect(
    recordings: &BTreeMap<RecordingId, Dialogue>,
    pieces: &[Piece],
    settings: &AuditSettings,
    collector: &mut Collector,
) {
    if pieces.is_empty() {
        return;
    }
    for i in 0..=pieces.len() {
        let mut sides = Vec::with_capacity(2);
        if i > 0 {
            let p = pieces[i - 1];
            sides.push(inspect_side(&recordings[&p.recording], p, true, settings));
        }
        if i < pieces.len() {
            let p = pieces[i];
            sides.push(inspect_side(&recordings[&p.recording], p, false, settings));
        }
        let room_tone = if sides.len() == 2 {
            sides[0].level.zip(sides[1].level).map(|(out, inn)| {
                let delta_cdb = i32::from(inn.rms_cdb) - i32::from(out.rms_cdb);
                let comparable = sides[0].air_us >= settings.level_window_us
                    && sides[1].air_us >= settings.level_window_us
                    && out.us[1] - out.us[0] == settings.level_window_us
                    && inn.us[1] - inn.us[0] == settings.level_window_us;
                let flagged = comparable
                    && delta_cdb.unsigned_abs() >= u32::from(settings.room_tone_jump_cdb)
                    && out.rms_cdb.max(inn.rms_cdb) > settings.audible_room_tone_cdb;
                RoomTone {
                    delta_cdb,
                    comparable,
                    flagged,
                }
            })
        } else {
            None
        };
        let at_us = if i < pieces.len() {
            pieces[i].play
        } else {
            let p = pieces[i - 1];
            p.play + (p.source.end_micros - p.source.start_micros)
        };
        collector.push(Item::Join(Join {
            at_us,
            sides,
            room_tone,
        }));
    }
}
