// Cases ported from Scarlet packages/mcp/src/dialogue-leftovers.test.ts, gap-sound.test.ts and perception.test.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::*;
use crate::dialogue::{Hop, Settings, SoundProfile, Word};
use proptest::prelude::*;

fn word(text: &str, start: u64, end: u64) -> Word {
    Word {
        text: text.into(),
        start_micros: start,
        end_micros: end,
    }
}
fn said(entries: &[(&str, u64)]) -> Vec<Word> {
    entries
        .iter()
        .map(|&(text, start)| word(text, start, start + 300_000))
        .collect()
}
fn recording(duration: u64, words: Vec<Word>, read: impl Fn(u64) -> Hop) -> Dialogue {
    let n = duration.div_ceil(10_000);
    let profile =
        SoundProfile::from_hops(10_000, duration, (0..n).map(|i| read(i * 10_000 + 5_000)))
            .unwrap();
    Dialogue::new(profile, words, Settings::default()).unwrap()
}
fn hop(level: i16, voice: bool) -> Hop {
    Hop {
        rms_dbfs_hundredths: level,
        peak_dbfs_hundredths: (level + 600).min(0),
        voice,
    }
}
fn measured(duration: u64, words: Vec<Word>) -> Dialogue {
    let profile = SoundProfile::from_hops(
        10_000,
        duration,
        (0..duration.div_ceil(10_000)).map(|i| {
            let t = i * 10_000 + 5_000;
            let sound = words
                .iter()
                .any(|w| w.start_micros <= t && t < w.end_micros);
            hop(if sound { -2_000 } else { -6_000 }, sound)
        }),
    )
    .unwrap();
    Dialogue::new(profile, words, Settings::default()).unwrap()
}
fn kept(id: u64, start: u64, end: u64) -> KeptRange {
    KeptRange {
        recording: RecordingId(id),
        source: SourceRange {
            start_micros: start,
            end_micros: end,
        },
    }
}
fn report(recordings: &BTreeMap<RecordingId, Dialogue>, cut: &[KeptRange]) -> Report {
    audit(recordings, cut, &AuditSettings::default(), comparison_key).unwrap()
}
fn one(recording: Dialogue, cut: &[KeptRange]) -> Report {
    report(&BTreeMap::from([(RecordingId(1), recording)]), cut)
}
fn join_at(report: &Report, at: u64) -> &Join {
    report
        .items
        .iter()
        .find_map(|i| match i {
            Item::Join(j) if j.at_us == at => Some(j),
            _ => None,
        })
        .unwrap()
}
fn side(join: &Join, direction: AudibleSide) -> &Side {
    join.sides.iter().find(|s| s.side == direction).unwrap()
}

#[test]
fn dialogue_audit_clean_join_has_air_and_matching_room_tone() {
    // Scarlet: "calls a join with air on both sides and matching room tone clean".
    let d = measured(
        10_000_000,
        vec![
            word("kept", 3_000_000, 3_800_000),
            word("next", 6_120_000, 7_000_000),
        ],
    );
    let r = one(d, &[kept(1, 0, 4_000_000), kept(1, 6_000_000, 10_000_000)]);
    let j = join_at(&r, 4_000_000);
    assert_eq!(side(j, AudibleSide::Out).air_us, 200_000);
    assert_eq!(side(j, AudibleSide::In).air_us, 120_000);
    assert_eq!(j.room_tone.as_ref().unwrap().delta_cdb, 0);
    assert_eq!(
        (
            r.total.edge_in_sound,
            r.total.short_air,
            r.total.room_tone_jumps,
            r.total.repeats,
            r.total.pauses
        ),
        (0, 0, 0, 0, 0)
    );
    assert!(
        j.sides
            .iter()
            .all(|s| s.edge_in_sound.is_none() && !s.short_air)
    );
}

#[test]
fn dialogue_audit_quiet_twelve_db_jump_warns_with_both_guards_passing() {
    // Accepted DQ3 counterexample; Scarlet: "reports ... a room-tone jump".
    let a = recording(2_000_000, vec![], |_| hop(-6_000, false));
    let b = recording(2_000_000, vec![], |_| hop(-4_800, false));
    assert_eq!(a.guard(1_000_000, AudibleSide::Out), Ok(()));
    assert_eq!(b.guard(1_000_000, AudibleSide::In), Ok(()));
    let r = report(
        &BTreeMap::from([(RecordingId(1), a), (RecordingId(2), b)]),
        &[kept(1, 0, 1_000_000), kept(2, 1_000_000, 2_000_000)],
    );
    let j = join_at(&r, 1_000_000);
    let room = j.room_tone.as_ref().unwrap();
    assert_eq!(room.delta_cdb, 1_200, "DQ3_ROOM_TONE_USES_BOTH_SIDES");
    assert!(
        room.flagged && room.comparable,
        "DQ3_ROOM_TONE_WARNING_INDEPENDENT"
    );
    assert!(
        j.sides
            .iter()
            .all(|s| s.edge_in_sound.is_none() && !s.short_air)
    );
}

#[test]
fn dialogue_audit_inaudible_floors_do_not_warn() {
    // Scarlet: "Two inaudible floors (a gated mic) differ without anyone hearing it".
    let a = recording(2_000_000, vec![], |_| hop(-8_600, false));
    let b = recording(2_000_000, vec![], |_| hop(-7_400, false));
    let r = report(
        &BTreeMap::from([(RecordingId(1), a), (RecordingId(2), b)]),
        &[kept(1, 0, 1_000_000), kept(2, 1_000_000, 2_000_000)],
    );
    let room = join_at(&r, 1_000_000).room_tone.as_ref().unwrap();
    assert_eq!(room.delta_cdb, 1_200);
    assert!(room.comparable && !room.flagged);
}

#[test]
fn dialogue_audit_one_hop_inside_a_word_names_it_and_keeps_flags_independent() {
    // Scarlet: "reports a clipped word, an edge on speech, tight air and a room-tone jump".
    let d = measured(
        4_000_000,
        vec![
            word("really", 1_000_000, 1_800_000),
            word("next", 3_000_000, 3_400_000),
        ],
    );
    let recordings = BTreeMap::from([(RecordingId(1), d)]);
    for at in [1_010_000, 1_040_000] {
        let r = audit(
            &recordings,
            &[kept(1, 0, at), kept(1, 2_800_000, 4_000_000)],
            &AuditSettings::default(),
            comparison_key,
        )
        .unwrap();
        let out = side(join_at(&r, at), AudibleSide::Out);
        let flag = out.edge_in_sound.as_ref().unwrap();
        // Stage A intentionally leaves the first 20 ms outside its named-word
        // interior. Preserve that classification and supply the timed candidate.
        if at == 1_010_000 {
            assert_eq!(flag.reason, GuardIssue::UntranscribedSound);
            assert_eq!(out.timed_word, Some(WordKey(RecordingId(1), 0)));
        } else {
            assert_eq!(flag.reason, GuardIssue::SoundingWord);
            assert_eq!(flag.word, Some(WordKey(RecordingId(1), 0)));
        }
        assert!(out.short_air && out.voice);
        assert_eq!(out.air_us, 0);
        assert!(
            r.words
                .iter()
                .any(|w| w.key == WordKey(RecordingId(1), 0) && w.name.text == "really")
        );
    }
}

#[test]
fn dialogue_audit_short_air_is_not_a_guard_refusal() {
    let d = measured(
        4_000_000,
        vec![
            word("kept", 500_000, 1_000_000),
            word("next", 2_200_000, 2_600_000),
        ],
    );
    let r = one(d, &[kept(1, 0, 1_050_000), kept(1, 2_000_000, 4_000_000)]);
    let s = side(join_at(&r, 1_050_000), AudibleSide::Out);
    assert!(s.short_air && s.edge_in_sound.is_none());
    assert_eq!(s.air_us, 50_000);
}

#[test]
fn dialogue_audit_untranscribed_sound_and_stretched_words() {
    // Scarlet: "Whisper stretched ... over a measured pause: the recording decides".
    let d = recording(
        4_000_000,
        vec![word("stretched", 500_000, 1_900_000)],
        |t| {
            hop(
                if (900_000..2_000_000).contains(&t) {
                    -6_000
                } else {
                    -2_000
                },
                !(900_000..2_000_000).contains(&t),
            )
        },
    );
    let r = one(d, &[kept(1, 0, 1_500_000), kept(1, 2_300_000, 4_000_000)]);
    let out = side(join_at(&r, 1_500_000), AudibleSide::Out);
    assert!(out.edge_in_sound.is_none());
    assert_eq!(out.stretched_word, Some(WordKey(RecordingId(1), 0)));
    let incoming = side(join_at(&r, 1_500_000), AudibleSide::In);
    assert_eq!(
        incoming.edge_in_sound.as_ref().unwrap().reason,
        GuardIssue::UntranscribedSound
    );
}

#[test]
fn dialogue_audit_voice_bridge_preserves_level_only_air_and_word_distance() {
    // Scarlet: "measures the kept word ... when voice activity bridges its quiet tail".
    // The flat playlist is speed one with no crossfade handles.
    let d = recording(5_000_000, vec![word("kept", 3_300_000, 3_700_000)], |t| {
        hop(if t < 3_700_000 { -2_000 } else { -6_000 }, t < 4_100_000)
    });
    let r = one(d, &[kept(1, 0, 4_000_000)]);
    let out = side(join_at(&r, 4_000_000), AudibleSide::Out);
    assert_eq!(
        (out.air_us, out.low_air_us, out.word_air_us),
        (0, 300_000, Some(300_000))
    );
    assert!(out.edge_in_sound.is_none() && out.short_air);
}

#[test]
fn dialogue_audit_pauses_use_playback_and_name_only_untouched_ones() {
    // Scarlet: "lists a pause by how long it plays with the cuts out, and names the word that trims an untouched one".
    let d = measured(
        7_000_000,
        said(&[
            ("one", 0),
            ("two", 400_000),
            ("three", 2_000_000),
            ("four", 5_300_000),
            ("five", 5_700_000),
            ("six", 6_600_000),
        ]),
    );
    let r = one(d, &[kept(1, 0, 2_800_000), kept(1, 4_800_000, 7_000_000)]);
    let pauses: Vec<_> = r
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Pause(p) = i {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(pauses.len(), 2);
    assert_eq!(
        pauses
            .iter()
            .map(|p| p.play_us[1] - p.play_us[0])
            .collect::<Vec<_>>(),
        vec![1_300_000, 1_000_000]
    );
    assert_eq!(pauses[0].after_word, Some(WordKey(RecordingId(1), 1)));
    assert_eq!(pauses[1].after_word, None);
}

#[test]
fn dialogue_audit_lists_point_eight_second_pause_but_not_point_seven() {
    let r = one(
        measured(
            3_000_000,
            said(&[("one", 100_000), ("two", 1_200_000), ("three", 2_200_000)]),
        ),
        &[kept(1, 0, 3_000_000)],
    );
    let pauses: Vec<_> = r
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Pause(p) = i {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].play_us, [400_000, 1_200_000]);
}

#[test]
fn dialogue_audit_repeats_cross_a_join_and_preserve_source_indices() {
    // Scarlet: "finds words said twice in a row, a restart the part left and a word its own join doubled".
    let words = said(&[
        ("here's", 0),
        ("the", 350_000),
        ("plate", 700_000),
        ("here's", 1_200_000),
        ("the", 1_550_000),
        ("plate.", 1_900_000),
        ("It", 2_400_000),
        ("is", 2_800_000),
        ("uh", 3_200_000),
        ("is", 3_600_000),
        ("good.", 4_000_000),
        ("Good", 4_500_000),
        ("then.", 4_900_000),
    ]);
    let r = one(
        measured(5_500_000, words),
        &[kept(1, 0, 3_150_000), kept(1, 3_550_000, 5_500_000)],
    );
    let repeats: Vec<_> = r
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Repeat(r) = i {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(repeats.len(), 2, "DQ3_REPEAT_ACROSS_JOIN_IS_LISTED");
    assert_eq!(
        repeats
            .iter()
            .map(|r| (r.said.text.as_str(), r.at_join))
            .collect::<Vec<_>>(),
        vec![
            ("here's the plate here's the plate.", false),
            ("is is", true)
        ]
    );
    assert_eq!(repeats[1].from_word, WordKey(RecordingId(1), 7));
}

#[test]
fn dialogue_audit_long_restart_follows_the_played_clock() {
    // Scarlet: "reports a long phrase doubled by a ripple on the clock of the saved part".
    let phrase = ["listen", "for", "the", "bell", "at", "noon"];
    let mut words = Vec::new();
    for start in [1_000_000, 63_000_000] {
        for (i, text) in phrase.iter().enumerate() {
            let t = start + i as u64 * 400_000;
            words.push(word(text, t, t + 300_000));
        }
    }
    words.push(word("tomorrow.", 65_400_000, 65_700_000));
    let r = one(
        measured(66_000_000, words),
        &[kept(1, 0, 3_500_000), kept(1, 62_200_000, 66_000_000)],
    );
    let repeat = r
        .items
        .iter()
        .find_map(|i| {
            if let Item::Repeat(r) = i {
                Some(r)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(repeat.words, 6);
    assert!(repeat.at_join);
    assert_eq!(repeat.from_word, WordKey(RecordingId(1), 0));
}

#[test]
fn dialogue_audit_the_the_at_a_join_but_not_ten_words_apart() {
    let r = one(
        measured(
            3_000_000,
            said(&[("the", 100_000), ("unused", 700_000), ("the", 1_500_000)]),
        ),
        &[kept(1, 0, 500_000), kept(1, 1_400_000, 3_000_000)],
    );
    assert_eq!(r.total.repeats, 1);
    assert!(
        r.items
            .iter()
            .any(|i| matches!(i,Item::Repeat(r)if r.at_join))
    );
    let mut words = said(&[("the", 0)]);
    for n in 1..=10 {
        words.push(word(
            &format!("word{n}"),
            n * 400_000,
            n * 400_000 + 300_000,
        ));
    }
    words.push(word("the", 4_400_000, 4_700_000));
    let r = one(measured(5_000_000, words), &[kept(1, 0, 5_000_000)]);
    assert_eq!(r.total.repeats, 0);
}

fn gap_fixture(sound: impl Fn(u64) -> Option<i16>) -> Dialogue {
    let words = vec![
        word("You can", 0, 2_900_000),
        word("see", 4_600_000, 9_000_000),
    ];
    recording(10_000_000, words, |t| {
        let level = sound(t).unwrap_or(if t < 2_900_000 || (4_600_000..9_000_000).contains(&t) {
            -2_000
        } else {
            -6_000
        });
        hop(level, level >= -3_000)
    })
}
fn middle_hole(r: &Report) -> &Hole {
    r.items
        .iter()
        .find_map(|i| match i {
            Item::Hole(h) if h.location.source_us == [2_900_000, 4_600_000] => Some(h),
            _ => None,
        })
        .unwrap()
}

#[test]
fn dialogue_audit_holes_find_untranscribed_sound_without_calling_it_speech() {
    // Scarlet: "finds speech the transcript left out of a gap, and calls an empty gap quiet".
    let r = one(
        gap_fixture(|t| {
            ((3_100_000..3_500_000).contains(&t) || (4_200_000..4_500_000).contains(&t))
                .then_some(-2_194)
        }),
        &[kept(1, 0, 10_000_000)],
    );
    let h = middle_hole(&r);
    assert_eq!(h.speaking_cdb, -2_000);
    assert_eq!(h.sound_us, 700_000);
    assert_eq!(
        h.sound_runs_us,
        vec![[3_100_000, 3_500_000], [4_200_000, 4_500_000]]
    );
    let r = one(gap_fixture(|_| None), &[kept(1, 0, 10_000_000)]);
    assert_eq!(middle_hole(&r).sound_us, 0);
}

#[test]
fn dialogue_audit_breath_hiss_click_and_word_tails_are_below_the_speaking_run_test() {
    // Scarlet: "leaves out a sibilant's hiss, a click and each word's own edge".
    let r = one(
        gap_fixture(|t| {
            if (2_900_000..2_940_000).contains(&t) || (4_560_000..4_600_000).contains(&t) {
                Some(-2_000)
            } else if (3_700_000..3_740_000).contains(&t) {
                Some(-1_400)
            } else if (2_940_000..4_560_000).contains(&t) {
                Some(-4_440)
            } else {
                None
            }
        }),
        &[kept(1, 0, 10_000_000)],
    );
    let h = middle_hole(&r);
    assert_eq!(h.sound_us, 0);
    assert!(h.sound_runs_us.is_empty());
    assert!(h.level.rms_cdb < -4_000);
}

fn quiet_fixture(before: i16, phrase: i16, after: i16) -> Dialogue {
    // The original zero-duration "it" has a positive 1-us span to obey Stage A's word contract.
    let words = vec![
        word("Watch", 800_000, 1_200_000),
        word("here.", 1_200_000, 1_600_000),
        word("move", 2_400_000, 2_650_000),
        word("it", 2_650_000, 2_650_001),
        word("over", 2_700_000, 3_000_000),
        word("Ready.", 5_000_000, 5_500_000),
    ];
    let profile = SoundProfile::from_hops(
        10_000,
        10_000_000,
        (0..1000).map(|i| {
            let t = i * 10_000 + 5_000;
            let level = if words
                .iter()
                .any(|w| w.start_micros <= t && t < w.end_micros)
            {
                if t < 2_000_000 {
                    before
                } else if t < 4_000_000 {
                    phrase
                } else {
                    after
                }
            } else {
                -12_000
            };
            hop(level, false)
        }),
    )
    .unwrap();
    Dialogue::new(profile, words, Settings::default()).unwrap()
}

#[test]
fn dialogue_audit_quiet_phrase_has_local_levels_text_and_stable_words() {
    // Scarlet: "reports a soft phrase with both local levels, its whole text and stable word references".
    let r = one(
        quiet_fixture(-2_000, -4_800, -2_200),
        &[kept(1, 0, 10_000_000)],
    );
    let p = r
        .items
        .iter()
        .find_map(|i| {
            if let Item::QuietPhrase(p) = i {
                Some(p)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(p.said.text, "move it over");
    assert_eq!(p.level_cdb, -4_800);
    assert_eq!(p.neighbors_cdb, [-2_000, -2_200]);
    assert_eq!((p.from_word.1, p.to_word.1), (2, 4));
    for (a, b, c) in [
        (-4_440, -4_800, -4_600),
        (-2_000, -4_800, -4_200),
        (-2_000, -2_300, -2_200),
    ] {
        let r = one(quiet_fixture(a, b, c), &[kept(1, 0, 10_000_000)]);
        assert_eq!(r.total.quiet_phrases, 0);
    }
}

#[test]
fn dialogue_audit_quiet_phrase_requires_both_contexts_and_full_retention() {
    // Scarlet: "requires measured speech on both sides of a candidate within the available reading".
    let d = measured(
        4_000_000,
        vec![
            word("soft", 1_000_000, 1_500_000),
            word("next", 3_000_000, 3_500_000),
        ],
    );
    let r = one(d, &[kept(1, 0, 4_000_000)]);
    assert_eq!(r.total.quiet_phrases, 0);
    let r = one(
        quiet_fixture(-2_000, -4_800, -2_200),
        &[kept(1, 2_500_000, 2_900_000)],
    );
    assert_eq!(r.total.quiet_phrases, 0);
}

#[test]
fn dialogue_audit_two_recordings_keep_holes_and_edges_attributed() {
    let a = measured(2_000_000, vec![word("the plan", 1_000_000, 1_800_000)]);
    let b = measured(2_000_000, vec![word("next", 200_000, 900_000)]);
    let r = report(
        &BTreeMap::from([(RecordingId(1), a), (RecordingId(2), b)]),
        &[kept(1, 0, 2_000_000), kept(2, 0, 2_000_000)],
    );
    let parts: Vec<_> = r
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Hole(h) if h.gap_play_us == [1_800_000, 2_200_000] => Some(h),
            _ => None,
        })
        .collect();
    assert_eq!(parts.len(), 2);
    assert_ne!(parts[0].location.recording, parts[1].location.recording);
    assert!(
        parts
            .iter()
            .all(|h| h.after == Some(WordKey(RecordingId(1), 0))
                && h.before == Some(WordKey(RecordingId(2), 0)))
    );
    let j = join_at(&r, 2_000_000);
    assert_eq!(j.sides[0].source.0, RecordingId(1));
    assert_eq!(j.sides[1].source.0, RecordingId(2));
}

#[test]
fn dialogue_audit_never_combines_different_voices_into_a_text_repeat() {
    let a = measured(1_000_000, said(&[("the", 100_000)]));
    let b = measured(1_000_000, said(&[("the", 100_000)]));
    let r = report(
        &BTreeMap::from([(RecordingId(1), a), (RecordingId(2), b)]),
        &[kept(1, 0, 1_000_000), kept(2, 0, 1_000_000)],
    );
    assert_eq!(r.total.repeats, 0);
}

#[test]
fn dialogue_audit_caps_items_runs_and_utf8_text_with_honest_counts() {
    let d = recording(
        10_000_000,
        vec![
            word("été".repeat(100).as_str(), 0, 1_000_000),
            word("last", 9_000_000, 9_500_000),
        ],
        |t| {
            hop(
                if (1_000_000..9_000_000).contains(&t) && t / 200_000 % 2 == 0 {
                    -2_000
                } else {
                    -6_000
                },
                false,
            )
        },
    );
    let settings = AuditSettings {
        max_items: 1,
        max_sound_runs: 2,
        max_word_bytes: 5,
        ..AuditSettings::default()
    };
    let recordings = BTreeMap::from([(RecordingId(1), d)]);
    let r = audit(
        &recordings,
        &[kept(1, 0, 10_000_000)],
        &settings,
        comparison_key,
    )
    .unwrap();
    assert_eq!(r.items.len(), 1);
    assert!(r.total.holes > r.omitted.holes || r.total.pauses > r.omitted.pauses);
    assert!(r.words.iter().any(|w| w.name.truncated));
    assert!(r.words.iter().all(|w| w.name.text.len() <= 5));
    let Item::Pause(_) = &r.items[0] else {
        panic!("first warning follows playback order")
    };
    let r = audit(
        &recordings,
        &[kept(1, 0, 10_000_000)],
        &AuditSettings {
            max_sound_runs: 2,
            ..AuditSettings::default()
        },
        comparison_key,
    )
    .unwrap();
    let h = r
        .items
        .iter()
        .find_map(|i| if let Item::Hole(h) = i { Some(h) } else { None })
        .unwrap();
    assert_eq!(h.sound_runs_us.len(), 2);
    assert!(h.omitted_runs > 0);
    assert!(h.sound_us > 400_000);
}

#[test]
fn dialogue_audit_rejects_bad_inputs_before_measurement_and_coalesces_splits() {
    let recordings = BTreeMap::from([(
        RecordingId(1),
        measured(2_000_000, said(&[("one", 100_000), ("two", 700_000)])),
    )]);
    assert_eq!(
        audit(
            &recordings,
            &[kept(2, 0, 1)],
            &AuditSettings::default(),
            comparison_key
        ),
        Err(AuditError::Recording { index: 0 })
    );
    for r in [kept(1, 1, 1), kept(1, 0, 2_000_001)] {
        assert_eq!(
            audit(&recordings, &[r], &AuditSettings::default(), comparison_key),
            Err(AuditError::Range { index: 0 })
        );
    }
    let a = audit(
        &recordings,
        &[kept(1, 0, 2_000_000)],
        &AuditSettings::default(),
        comparison_key,
    )
    .unwrap();
    let b = audit(
        &recordings,
        &[kept(1, 0, 200_000), kept(1, 200_000, 2_000_000)],
        &AuditSettings::default(),
        comparison_key,
    )
    .unwrap();
    assert_eq!(a, b);
    assert!(
        audit(
            &recordings,
            &[],
            &AuditSettings {
                level_window_us: 0,
                ..AuditSettings::default()
            },
            comparison_key
        )
        .is_err()
    );
}

#[test]
fn dialogue_audit_ten_minute_sixty_join_report_is_under_32_kib() {
    let mut words = Vec::new();
    let mut cut = Vec::new();
    for i in 0..=60 {
        let start = i * 9_800_000;
        words.push(word(
            &format!("passage{i}"),
            start + 200_000,
            start + 8_800_000,
        ));
        cut.push(kept(1, start, start + 9_000_000));
    }
    let r = one(measured(600_000_000, words), &cut);
    assert_eq!(r.total.joins, 62);
    assert_eq!(r.total.edge_in_sound, 0);
    let json = serde_json::to_vec(&r).unwrap();
    println!(
        "DQ3_REPORT_SIZE bytes={} listed={} omitted_joins={} omitted_holes={}",
        json.len(),
        r.items.len(),
        r.omitted.joins,
        r.omitted.holes
    );
    assert!(
        json.len() < 32 * 1024,
        "DQ3_REPORT_BYTE_BUDGET bytes={}",
        json.len()
    );
    assert!(r.items.len() <= 64);
    assert!(!String::from_utf8(json).unwrap().contains("peaks"));
}

proptest! {
    #[test]
    fn dialogue_audit_sound_flags_equal_the_stage_a_guard(out in 1_u64..3_999_999,inn in 1_u64..3_999_999,level in -8_000_i16..=-1_000,voice in any::<bool>()){
        let words=vec![word("spoken",500_000,3_500_000)];
        let d=recording(4_000_000,words,|t|hop(if (1_000_000..2_000_000).contains(&t){-6_000}else{level},voice));
        let recordings=BTreeMap::from([(RecordingId(1),d)]);
        let r=audit(&recordings,&[kept(1,0,out),kept(1,inn,4_000_000)],&AuditSettings{max_items:100,..AuditSettings::default()},comparison_key).unwrap();
        for item in &r.items {if let Item::Join(j)=item{for s in &j.sides{
            prop_assert_eq!(s.edge_in_sound.is_some(),recordings[&s.source.0].guard(s.source.1,s.side).is_err());
        }}}
    }
}

#[test]
fn dialogue_audit_boundary_dip_is_a_fourth_independent_warning() {
    use crate::dialogue::{DipIntrusion, WordBoundary};
    let d = recording(1_000_000, vec![word("cat", 0, 300_000)], |t| {
        hop(
            if (250_000..260_000).contains(&t) {
                -5_600
            } else {
                -2_000
            },
            true,
        )
    });
    assert!(d.guard(255_000, AudibleSide::Out).is_ok());
    let r = one(d, &[kept(1, 0, 255_000)]);
    let s = side(join_at(&r, 255_000), AudibleSide::Out);
    assert!(s.edge_in_sound.is_none());
    assert_eq!(
        s.dip_intrusion,
        Some(DipIntrusion {
            word: 0,
            boundary: WordBoundary::End,
            micros: 45_000
        })
    );
    assert_eq!(r.total.dip_intrusions, 1);
    assert!(
        r.words
            .iter()
            .any(|w| w.key == WordKey(RecordingId(1), 0) && w.name.text == "cat")
    );
}

#[test]
fn dialogue_audit_deep_dip_warns_and_cannot_hide_the_sound_flag() {
    let d = recording(1_000_000, vec![word("long", 0, 400_000)], |t| {
        hop(
            if (150_000..160_000).contains(&t) {
                -5_600
            } else {
                -2_000
            },
            true,
        )
    });
    let r = one(d, &[kept(1, 0, 155_000)]);
    let s = side(join_at(&r, 155_000), AudibleSide::Out);
    assert_eq!(
        s.edge_in_sound.as_ref().unwrap().word,
        Some(WordKey(RecordingId(1), 0))
    );
    assert_eq!(s.dip_intrusion.unwrap().micros, 155_000);
    assert_eq!(r.total.edge_in_sound, 1);
    assert_eq!(r.total.dip_intrusions, 1);
}

#[test]
fn dialogue_audit_half_word_ownership_is_per_joined_occurrence() {
    for (a, b, repeats) in [
        (0, 500_000, 1),
        (500_000, 1_000_000, 1),
        (450_000, 550_000, 0),
    ] {
        let r = one(
            measured(1_000_000, vec![word("the", 0, 1_000_000)]),
            &[kept(1, a, b), kept(1, a, b)],
        );
        assert_eq!(r.total.repeats, repeats);
    }
}
