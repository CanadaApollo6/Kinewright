// Cases ported from Scarlet packages/mcp/src/dialogue-join.test.ts and dialogue-plan.test.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::*;
use crate::{Rational, TimeCode};
use proptest::prelude::*;

fn word(text: &str, start: u64, end: u64) -> Word {
    Word {
        text: text.into(),
        start_micros: start,
        end_micros: end,
    }
}
fn profile(hop: u32, duration: u64, voice: &[(u64, u64)], dips: &[(usize, i16)]) -> SoundProfile {
    SoundProfile::from_hops(
        hop,
        duration,
        (0..duration.div_ceil(u64::from(hop))).map(|i| {
            let at = i * u64::from(hop);
            let voiced = voice
                .iter()
                .any(|&(start, end)| start < at + u64::from(hop) && at < end);
            let level = dips
                .iter()
                .find(|&&(index, _)| u64::try_from(index).unwrap() == i)
                .map_or(if voiced { -1_800 } else { -6_000 }, |&(_, db)| db);
            Hop {
                peak_dbfs_hundredths: level,
                rms_dbfs_hundredths: level,
                voice: voiced,
            }
        }),
    )
    .unwrap()
}
fn fixture() -> Dialogue {
    Dialogue::new(
        profile(
            20_000,
            5_000_000,
            &[
                (980_000, 1_440_000),
                (1_800_000, 2_260_000),
                (3_400_000, 3_940_000),
            ],
            &[],
        ),
        vec![
            word("finish", 1_000_000, 1_380_000),
            word("first", 1_800_000, 2_200_000),
            word("next", 3_400_000, 3_860_000),
        ],
        Settings::default(),
    )
    .unwrap()
}
fn rounding_fixture() -> Dialogue {
    Dialogue::new(
        profile(10_000, 200_000, &[(0, 10_000), (100_000, 200_000)], &[]),
        vec![word("keep", 0, 10_000), word("remove", 100_000, 120_000)],
        Settings::default(),
    )
    .unwrap()
}
#[test]
fn dialogue_profile_has_one_validated_compact_door() {
    assert_eq!(
        SoundProfile::from_raw(0, 1, vec![], vec![], vec![]),
        Err(ProfileError::Hop)
    );
    assert_eq!(
        SoundProfile::from_raw(31_001, 1, vec![], vec![], vec![]),
        Err(ProfileError::Hop)
    );
    assert_eq!(
        SoundProfile::from_raw(10_000, 10_000, vec![-2_000], vec![], vec![true]),
        Err(ProfileError::Length)
    );
    assert_eq!(
        SoundProfile::from_raw(10_000, 10_001, vec![-2_000], vec![-2_000], vec![true]),
        Err(ProfileError::Length)
    );
    for (peak, rms) in [(1, 0), (-1_000, -999)] {
        assert_eq!(
            SoundProfile::from_raw(10_000, 10_000, vec![peak], vec![rms], vec![false]),
            Err(ProfileError::Level { index: 0 })
        );
    }
    let p =
        SoundProfile::from_raw(10_000, 7_001, vec![-32_000], vec![-32_000], vec![false]).unwrap();
    assert_eq!(p.hop(0).unwrap().rms_dbfs_hundredths, LEVEL_FLOOR);
    assert_eq!(p.span(0).end_micros, 7_001);
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<SoundProfile>(&json).unwrap(), p);
    assert!(
        serde_json::from_str::<SoundProfile>(&json.replace("\"voice\"", "\"unknown\"")).is_err()
    );
}
#[test]
fn dialogue_words_and_settings_are_validated_before_derivation() {
    let p = || profile(10_000, 100_000, &[], &[]);
    assert!(matches!(
        Dialogue::new(p(), vec![word("", 0, 1)], Settings::default()),
        Err(InputError::Word { index: 0 })
    ));
    assert!(matches!(
        Dialogue::new(
            p(),
            vec![word("a", 5, 6), word("b", 1, 4)],
            Settings::default()
        ),
        Err(InputError::Word { index: 1 })
    ));
    let settings = Settings {
        air_after_micros: 0,
        air_before_micros: 0,
        ..Settings::default()
    };
    assert!(matches!(
        Dialogue::new(p(), vec![], settings),
        Err(InputError::Settings)
    ));
}
#[test]
fn dialogue_padding_leaves_air_and_splits_short_runs() {
    // Scarlet: "pads an edge it places: air after kept speech, air before speech resumes, split when the pause is short".
    let settings = Settings::default();
    let span = Span::new(1_000_000, 2_000_000).unwrap();
    assert_eq!(
        pad_edge(span, AudibleSide::Out, &settings).unwrap(),
        1_150_000
    );
    assert_eq!(
        pad_edge(span, AudibleSide::In, &settings).unwrap(),
        1_900_000
    );
    let short = Span::new(1_000_000, 1_200_000).unwrap();
    for side in [AudibleSide::In, AudibleSide::Out] {
        assert_eq!(pad_edge(short, side, &settings).unwrap(), 1_120_000);
    }
    assert_eq!(
        pad_edge(Span::new(0, 20_000).unwrap(), AudibleSide::Out, &settings),
        Err(InputError::Span)
    );
}
#[test]
fn dialogue_word_range_uses_measured_neighbor_gaps() {
    // Scarlet: "resolves a word range to padded edges in the measured gaps beside it".
    let d = fixture();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap();
    assert_eq!(
        (cut.start.at_micros, cut.end.at_micros),
        (1_590_000, 3_300_000)
    );
    assert!(matches!(
        cut.start.placement,
        EdgePlacement::Quiet {
            air_after_micros: 150_000,
            shift: Shift {
                micros: 210_000,
                later: false
            },
            ..
        }
    ));
    assert!(matches!(
        cut.end.placement,
        EdgePlacement::Quiet {
            air_before_micros: 100_000,
            ..
        }
    ));
    assert_eq!(
        d.resolve(CutRequest::RemoveWords { from: 0, to: 0 })
            .unwrap()
            .start
            .placement,
        EdgePlacement::SourceBoundary { at_start: true }
    );
    assert_eq!(
        d.resolve(CutRequest::RemoveWords { from: 2, to: 2 })
            .unwrap()
            .end
            .at_micros,
        5_000_000
    );
}
#[test]
fn dialogue_internal_valley_does_not_replace_the_whole_pause() {
    // Scarlet: "puts an edge beside the kept word when a deep trough lies inside the quiet run".
    let d = Dialogue::new(
        profile(
            20_000,
            5_000_000,
            &[
                (980_000, 1_440_000),
                (1_800_000, 2_260_000),
                (3_400_000, 3_940_000),
            ],
            &(125..145).map(|i| (i, -9_000)).collect::<Vec<_>>(),
        ),
        fixture().words,
        Settings::default(),
    )
    .unwrap();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap();
    assert_eq!(
        (cut.start.at_micros, cut.end.at_micros),
        (1_590_000, 3_300_000)
    );
}
#[test]
fn dialogue_pause_leaves_requested_air_without_clipping_voice_tail() {
    // Scarlet dialogue-plan: "trims a pause named by the word it follows down to the beat, and to the length the agent asks".
    let d = fixture();
    for (leave, start, end) in [
        (250_000, 2_350_000, 3_300_000),
        (150_000, 2_290_000, 3_340_000),
        (600_000, 2_560_000, 3_160_000),
    ] {
        let cut = d
            .resolve(CutRequest::TrimPause {
                after_word: 1,
                leave_micros: leave,
            })
            .unwrap();
        assert_eq!((cut.start.at_micros, cut.end.at_micros), (start, end));
    }
    // Scarlet dialogue-plan: "refuses a pause reference that has nothing to trim, names no pause, or belongs to another kind".
    assert_eq!(
        d.resolve(CutRequest::TrimPause {
            after_word: 0,
            leave_micros: 500_000
        })
        .unwrap_err()
        .reason,
        RefusalReason::NothingToTrim
    );
    assert_eq!(
        d.resolve(CutRequest::TrimPause {
            after_word: 2,
            leave_micros: 0
        })
        .unwrap_err()
        .reason,
        RefusalReason::NoFollowingWord
    );
}
#[test]
fn dialogue_a_single_deep_dip_can_place_a_running_word_boundary() {
    // Scarlet dialogue-plan: "cuts a doubled word that follows the word before it without a pause, at the dip between the two".
    let words = || {
        vec![
            word("recognized", 1_000_000, 1_400_000),
            word("that", 1_400_000, 1_700_000),
            word("that", 2_500_000, 2_800_000),
            word("likely", 2_800_000, 3_200_000),
        ]
    };
    for level in [-6_000, -4_600, -1_800] {
        let d = Dialogue::new(
            profile(
                20_000,
                5_000_000,
                &[(980_000, 1_720_000), (2_500_000, 3_220_000)],
                &[(70, level)],
            ),
            words(),
            Settings::default(),
        )
        .unwrap();
        let cut = d.resolve(CutRequest::RemoveWords { from: 1, to: 1 });
        if level == -6_000 {
            let cut = cut.unwrap();
            assert_eq!(
                (cut.start.at_micros, cut.end.at_micros),
                (1_410_000, 2_400_000)
            );
            assert!(matches!(
                cut.start.placement,
                EdgePlacement::LevelDip {
                    rms_dbfs_hundredths: -6_000,
                    ..
                }
            ));
        } else {
            assert_eq!(cut.unwrap_err().reason, RefusalReason::NoMeasuredEdge);
        }
    }
}
#[test]
fn dialogue_two_dip_edges_refuse_a_word_from_running_speech() {
    // Scarlet: "holds a word cut from the middle of running speech, with a dip on both sides and a pause on neither".
    let d = Dialogue::new(
        profile(
            20_000,
            3_000_000,
            &[(1_000_000, 2_000_000)],
            &[(60, -6_000), (70, -6_000)],
        ),
        vec![
            word("at", 1_000_000, 1_200_000),
            word("the", 1_200_000, 1_400_000),
            word("the", 1_400_000, 1_600_000),
            word("base", 1_600_000, 2_000_000),
        ],
        Settings::default(),
    )
    .unwrap();
    assert!(
        matches!(
            d.resolve(CutRequest::RemoveWords { from: 1, to: 1 }),
            Err(Refusal {
                reason: RefusalReason::BothDipEdges,
                ..
            })
        ),
        "DQ2_TWO_DIPS_REFUSED"
    );
}
#[test]
fn dialogue_shared_gap_is_not_used_on_both_sides_of_a_short_word() {
    // Scarlet: "resolves a word range ...", stray "With" and "the" case. Missing evidence refuses rather than returning its timing fallback.
    let d = Dialogue::new(
        profile(
            20_000,
            11_000_000,
            &[(9_500_000, 9_720_000), (9_980_000, 10_600_000)],
            &[],
        ),
        vec![
            word("good", 9_500_000, 9_700_000),
            word("With", 10_000_000, 10_280_000),
            word("the", 10_280_000, 10_500_000),
        ],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        d.resolve(CutRequest::RemoveWords { from: 1, to: 1 })
            .unwrap_err()
            .reason,
        RefusalReason::NoMeasuredEdge
    );
}
#[test]
fn dialogue_long_recording_profile_reaches_both_kept_words() {
    // Scarlet: "places a word range beside the kept words when a pause is longer than the page read at its middle".
    let d = Dialogue::new(
        profile(
            20_000,
            21_000_000,
            &[
                (1_000_000, 1_400_000),
                (9_400_000, 9_800_000),
                (10_200_000, 10_600_000),
                (18_600_000, 19_000_000),
            ],
            &[],
        ),
        vec![
            word("one", 1_000_000, 1_400_000),
            word("two", 9_400_000, 9_800_000),
            word("three", 10_200_000, 10_600_000),
            word("four", 18_600_000, 19_000_000),
        ],
        Settings::default(),
    )
    .unwrap();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 2 })
        .unwrap();
    assert_eq!(
        (cut.start.at_micros, cut.end.at_micros),
        (1_550_000, 18_500_000)
    );
}
#[test]
fn dialogue_acoustics_override_stretched_word_times_and_bridged_vad() {
    // Scarlet: "trusts measured silence over a stretched word timing" and "preserves a repairable edge ... long [trough] is measured silence".
    let d = Dialogue::new(
        profile(
            10_000,
            1_000_000,
            &[(0, 1_000_000)],
            &(30..70).map(|i| (i, -6_000)).collect::<Vec<_>>(),
        ),
        vec![word("stretched", 100_000, 900_000)],
        Settings::default(),
    )
    .unwrap();
    assert!(d.quiet_runs().is_empty());
    assert_eq!(d.sound_at(500_000), Sound::Quiet);
    assert!(d.guard(500_000, AudibleSide::In).is_ok());
    let short = Dialogue::new(
        profile(
            10_000,
            1_000_000,
            &[],
            &(0..100)
                .map(|i| {
                    (
                        i,
                        if (30..38).contains(&i) {
                            -5_000
                        } else {
                            -1_800
                        },
                    )
                })
                .collect::<Vec<_>>(),
        ),
        vec![word("apple", 100_000, 900_000)],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(short.sound_at(340_000), Sound::Valley);
    assert!(matches!(
        short.guard(340_000, AudibleSide::In),
        Err(Refusal {
            reason: RefusalReason::SoundingWord { .. },
            ..
        })
    ));
}
#[test]
fn dialogue_guard_names_word_and_untranscribed_sound_with_quiet_neighbors() {
    // Scarlet "a clip edge in sound": "finds the piece of an unexplained sound ..." and "names a word split through its middle with no quiet in reach".
    let d = Dialogue::new(
        profile(
            20_000,
            5_000_000,
            &[
                (1_000_000, 1_360_000),
                (1_380_000, 1_600_000),
                (2_000_000, 2_180_000),
                (2_280_000, 2_900_000),
            ],
            &[],
        ),
        vec![
            word("logo", 1_000_000, 1_360_000),
            word("by", 1_360_000, 1_600_000),
            word("obviously", 2_280_000, 2_900_000),
        ],
        Settings::default(),
    )
    .unwrap();
    for side in [AudibleSide::In, AudibleSide::Out] {
        let err = d.guard(2_142_000, side).unwrap_err();
        assert_eq!(err.reason, RefusalReason::UntranscribedSound);
        assert_eq!(
            (err.quiet_before, err.quiet_after),
            (Some(1_970_000), Some(2_210_000))
        );
        assert!(d.guard(2_220_000, side).is_ok());
        assert!(d.guard(1_900_000, side).is_ok());
        assert_eq!(
            d.guard(2_600_000, side).unwrap_err().reason,
            RefusalReason::SoundingWord {
                index: 2,
                text: "obviously".into()
            }
        );
        assert!(d.guard(1_363_000, side).is_ok());
        assert_eq!(
            d.guard(1_410_000, side).unwrap_err().reason,
            RefusalReason::SoundingWord {
                index: 1,
                text: "by".into()
            }
        );
    }
    assert_eq!(
        d.guard(5_000_001, AudibleSide::Out).unwrap_err().reason,
        RefusalReason::Unmeasured
    );
}
#[test]
fn dialogue_untranscribed_continuous_sound_is_not_silently_accepted() {
    let d = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 1_000_000)], &[]),
        vec![],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        d.guard(500_000, AudibleSide::In).unwrap_err().reason,
        RefusalReason::UntranscribedSound
    );
}
#[test]
fn dialogue_frame_rounds_into_measured_support_not_nearest_outside() {
    let d = rounding_fixture();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap();
    assert_eq!(cut.start.at_micros, 64_000);
    let frame = d
        .frame_edge(cut.start, Rational::new(25, 1).unwrap())
        .unwrap();
    assert_eq!(frame.frame, TimeCode(1), "DQ2_FRAME_ROUNDS_INSIDE_RUN");
    assert!(
        d.guard_ratio(
            frame.instant_numerator,
            u128::from(frame.instant_denominator),
            AudibleSide::Out
        )
        .is_ok()
    );
    assert_eq!(
        d.frame_edge(cut.start, Rational::new(10, 1).unwrap())
            .unwrap_err()
            .reason,
        RefusalReason::NoFrameInRun
    );
}
#[test]
fn dialogue_frame_guard_checks_the_current_profile_and_exact_fraction() {
    // Astra: the frame is 66,733 1/3 us; trusted quiet ends at 66,733.
    let d = rounding_fixture();
    let edge = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap()
        .start;
    let other = Dialogue::new(
        SoundProfile::from_hops(
            1,
            200_000,
            (0..200_000).map(|i| Hop {
                peak_dbfs_hundredths: if i < 96_733 { -4_500 } else { -1_800 },
                rms_dbfs_hundredths: if i < 96_733 { -4_500 } else { -1_800 },
                voice: i >= 96_733,
            }),
        )
        .unwrap(),
        vec![],
        Settings::default(),
    )
    .unwrap();
    assert!(other.guard(66_733, AudibleSide::Out).is_ok());
    assert!(
        matches!(
            other.frame_edge(edge, Rational::new(30_000, 1_001).unwrap()),
            Err(Refusal {
                reason: RefusalReason::UntranscribedSound,
                ..
            })
        ),
        "DQ2_FRACTIONAL_GUARD_NOT_TRUNCATED"
    );
}

proptest! {
    #[test]
    fn dialogue_accepted_edges_never_bisect_sounding_words(at in 0_u64..5_000_001) {
        let d=fixture();
        let sounding_word=d.words().iter().any(|w|w.start_micros+20_000<at && at<w.end_micros-20_000);
        if d.guard(at,AudibleSide::In).is_ok() {prop_assert!(!sounding_word);}
    }
    #[test]
    fn dialogue_resolution_is_deterministic(from in 0_usize..3,to in 0_usize..3) {
        let d=fixture();let request=CutRequest::RemoveWords{from,to};
        prop_assert_eq!(d.resolve(request),d.resolve(request));
    }
    #[test]
    fn dialogue_every_returned_frame_passes_the_exact_guard(from in 0_usize..3,to in 0_usize..3,n in prop::sample::select(vec![24,25,30,24_000,30_000]),den in prop::sample::select(vec![1,1_001])) {
        let d=fixture();
        if let Ok(cut)=d.resolve(CutRequest::RemoveWords{from,to}) {for edge in [cut.start,cut.end] {
            if let Ok(frame)=d.frame_edge(edge,Rational::new(n,den).unwrap()) {
                let actual=frame.instant_numerator;let denominator=u128::from(frame.instant_denominator);
                prop_assert!(actual>=u128::from(edge.support.start)*denominator);
                if edge.support.inclusive_end {prop_assert!(actual<=u128::from(edge.support.end)*denominator);}
                else {prop_assert!(actual<u128::from(edge.support.end)*denominator);}
                prop_assert!(d.guard_ratio(actual,denominator,edge.side).is_ok());
            }
        }}
    }
    #[test]
    fn dialogue_no_quiet_anywhere_refuses_instead_of_guessing(level in -3_999_i16..=0,from in 0_usize..3,to in 0_usize..3,voice in any::<bool>()) {
        let p=SoundProfile::from_hops(10_000,1_000_000,(0..100).map(|_|Hop{peak_dbfs_hundredths:level,rms_dbfs_hundredths:level,voice})).unwrap();
        let d=Dialogue::new(p,vec![word("one",0,200_000),word("two",200_000,400_000),word("three",400_000,1_000_000)],Settings::default()).unwrap();
        if from<=to {prop_assert_eq!(d.resolve(CutRequest::RemoveWords{from,to}).unwrap_err().reason,RefusalReason::NoQuietEvidence);}
    }
}

#[test]
fn dialogue_pause_cannot_cut_intervening_sound_when_leave_already_exceeds_pause() {
    let d = Dialogue::new(
        profile(
            10_000,
            3_000_000,
            &[
                (0, 1_000_000),
                (1_300_000, 1_800_000),
                (2_000_000, 3_000_000),
            ],
            &[],
        ),
        vec![
            word("before", 0, 1_000_000),
            word("after", 2_000_000, 3_000_000),
        ],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        d.resolve(CutRequest::TrimPause {
            after_word: 0,
            leave_micros: 2_000_000
        })
        .unwrap_err()
        .reason,
        RefusalReason::NothingToTrim
    );
}
#[test]
fn dialogue_zero_interior_margin_is_not_a_valid_setting() {
    assert!(matches!(
        Dialogue::new(
            profile(10_000, 10_000, &[], &[]),
            vec![],
            Settings {
                quiet_margin_micros: 0,
                ..Settings::default()
            }
        ),
        Err(InputError::Settings)
    ));
}

#[test]
fn dialogue_non_speech_labels_do_not_name_a_sounding_word() {
    let d = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 1_000_000)], &[]),
        vec![word("[NOISE]", 0, 1_000_000)],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        d.guard(500_000, AudibleSide::In).unwrap_err().reason,
        RefusalReason::UntranscribedSound
    );
}

#[test]
fn dialogue_hop_builder_refuses_before_unbounded_consumption() {
    let never = std::iter::from_fn(|| panic!("DQ2_METADATA_BEFORE_HOPS"));
    assert_eq!(SoundProfile::from_hops(0, 1, never), Err(ProfileError::Hop));
    let mut reads = 0;
    let extra = std::iter::from_fn(|| {
        reads += 1;
        assert!(reads <= 2, "DQ2_HOP_BUILDER_READ_BOUND");
        Some(Hop {
            peak_dbfs_hundredths: -6_000,
            rms_dbfs_hundredths: -6_000,
            voice: false,
        })
    });
    assert_eq!(
        SoundProfile::from_hops(10_000, 10_000, extra),
        Err(ProfileError::Length)
    );
    assert_eq!(reads, 2);
}

#[test]
fn dialogue_frame_can_use_the_safe_dip_beside_the_minimum_hop() {
    let d = Dialogue::new(
        profile(
            10_000,
            3_000_000,
            &[(1_000_000, 2_000_000), (2_600_000, 3_000_000)],
            &[(149, -6_000), (150, -6_000), (151, -6_500)],
        ),
        vec![
            word("one", 1_000_000, 1_500_000),
            word("two", 1_500_000, 2_000_000),
            word("three", 2_600_000, 3_000_000),
        ],
        Settings::default(),
    )
    .unwrap();
    let edge = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap()
        .start;
    assert_eq!(edge.at_micros(), 1_515_000);
    let frame = d
        .frame_edge(edge, Rational::new(30, 1).unwrap())
        .expect("DQ2_FRAME_INSIDE_CONTIGUOUS_DIP");
    assert_eq!(frame.frame, TimeCode(45));
    assert_eq!(frame.measured_hop.unwrap().rms_dbfs_hundredths, -6_000);
    assert_eq!(frame.rounding_shift_numerator, -450_000);
}

#[test]
fn dialogue_boundary_dip_reports_the_kept_word_intrusion() {
    // Astra closure witness, with the lead's explicit 50-ms timing tolerance.
    let d = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 500_000)], &[(25, -6_000)]),
        vec![word("cat", 0, 300_000), word("next", 300_000, 500_000)],
        Settings::default(),
    )
    .unwrap();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap();
    assert_eq!(cut.start.at_micros(), 255_000);
    let expected = DipIntrusion {
        word: 0,
        boundary: WordBoundary::End,
        micros: 45_000,
    };
    assert_eq!(d.dip_intrusion(255_000), Some(expected));
    assert!(
        matches!(cut.start.placement(),EdgePlacement::LevelDip{intrusion:Some(i),..}if i==expected)
    );
    assert_eq!(
        d.frame_edge(cut.start, Rational::new(60_000, 1_001).unwrap())
            .unwrap()
            .frame,
        TimeCode(15)
    );
}

#[test]
fn dialogue_dip_inside_word_beyond_reach_is_refused() {
    let basis = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 500_000)], &[(15, -6_000)]),
        vec![word("short", 0, 200_000), word("next", 200_000, 500_000)],
        Settings::default(),
    )
    .unwrap();
    let edge = basis
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap()
        .start;
    let d = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 500_000)], &[(15, -6_000)]),
        vec![word("long", 0, 400_000), word("next", 400_000, 500_000)],
        Settings::default(),
    )
    .unwrap();
    assert!(
        matches!(
            d.guard(150_000, AudibleSide::Out),
            Err(Refusal {
                reason: RefusalReason::SoundingWord { index: 0, .. },
                ..
            })
        ),
        "DQ2_DIP_INTRUSION_BOUND"
    );
    assert_eq!(
        d.resolve(CutRequest::RemoveWords { from: 1, to: 1 })
            .unwrap_err()
            .reason,
        RefusalReason::NoMeasuredEdge
    );
    assert!(matches!(
        d.frame_edge(edge, Rational::new(1_000, 1).unwrap()),
        Err(Refusal {
            reason: RefusalReason::SoundingWord { index: 0, .. },
            ..
        })
    ));
    assert_eq!(
        d.dip_intrusion(155_000),
        Some(DipIntrusion {
            word: 0,
            boundary: WordBoundary::Start,
            micros: 155_000
        })
    );
}

#[test]
fn dialogue_dip_reach_accepts_50_ms_but_refuses_51_and_checks_overlaps() {
    let d = Dialogue::new(
        profile(
            10_000,
            1_000_000,
            &[(0, 500_000)],
            &[(34, -6_000), (35, -6_000)],
        ),
        vec![word("long", 0, 400_000), word("next", 400_000, 500_000)],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(d.guard(350_000, AudibleSide::Out), Ok(()));
    assert!(matches!(
        d.guard(349_000, AudibleSide::Out),
        Err(Refusal {
            reason: RefusalReason::SoundingWord { index: 0, .. },
            ..
        })
    ));
    let d = Dialogue::new(
        profile(10_000, 600_000, &[(0, 500_000)], &[(25, -6_000)]),
        vec![word("first", 0, 300_000), word("overlap", 100_000, 400_000)],
        Settings::default(),
    )
    .unwrap();
    assert!(matches!(
        d.guard(255_000, AudibleSide::Out),
        Err(Refusal {
            reason: RefusalReason::SoundingWord { index: 1, .. },
            ..
        })
    ));
}

#[test]
fn dialogue_exact_dip_support_end_is_excluded() {
    // Astra: an alternate quiet profile must not make the exclusive endpoint eligible.
    let d = Dialogue::new(
        profile(
            10_000,
            3_000_000,
            &[(1_000_000, 2_000_000), (2_600_000, 3_000_000)],
            &[(149, -6_000), (150, -6_000), (151, -6_500)],
        ),
        vec![
            word("one", 1_000_000, 1_500_000),
            word("two", 1_500_000, 2_000_000),
            word("three", 2_600_000, 3_000_000),
        ],
        Settings::default(),
    )
    .unwrap();
    let edge = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap()
        .start;
    assert_eq!(
        (
            edge.support.start,
            edge.support.end,
            edge.support.inclusive_end
        ),
        (1_490_000, 1_520_000, false)
    );
    let other = Dialogue::new(
        profile(10_000, 3_000_000, &[], &[]),
        vec![],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        other
            .frame_edge(edge, Rational::new(25, 1).unwrap())
            .expect_err("DQ2_DIP_SUPPORT_END_IS_EXCLUSIVE")
            .reason,
        RefusalReason::NoFrameInRun,
        "DQ2_DIP_SUPPORT_END_IS_EXCLUSIVE"
    );
}

#[test]
fn dialogue_frame_ties_keep_material_on_each_side() {
    let d = Dialogue::new(
        profile(10_000, 200_000, &[], &[]),
        vec![],
        Settings::default(),
    )
    .unwrap();
    for side in [AudibleSide::Out, AudibleSide::In] {
        let edge = ResolvedEdge {
            at_micros: 50_050,
            side,
            placement: EdgePlacement::Quiet {
                run: Span::new(0, 200_000).unwrap(),
                air_after_micros: 50_050,
                air_before_micros: 149_950,
                shift: Shift {
                    micros: 0,
                    later: false,
                },
                snapped: false,
            },
            support: super::resolve::Support {
                start: 30_000,
                end: 170_000,
                inclusive_end: true,
            },
        };
        let frame = d
            .frame_edge(edge, Rational::new(30_000, 1_001).unwrap())
            .unwrap();
        assert_eq!(
            frame.frame,
            TimeCode(if side == AudibleSide::Out { 2 } else { 1 })
        );
    }
}

#[test]
fn dialogue_word_cut_floor_is_typed_and_boundary_is_inclusive() {
    for duration in [99_999, 100_000] {
        let d = Dialogue::new(
            profile(10_000, duration, &[], &[]),
            vec![word("short", 10_000, 20_000)],
            Settings::default(),
        )
        .unwrap();
        let result = d.resolve(CutRequest::RemoveWords { from: 0, to: 0 });
        if duration < 100_000 {
            assert_eq!(
                result.unwrap_err().reason,
                RefusalReason::WordCutTooShort {
                    actual_micros: 99_999,
                    minimum_micros: 100_000
                }
            );
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn dialogue_quantized_quiet_threshold_is_on_the_conservative_side() {
    assert_eq!(Settings::default().quiet_rms_max, -4_001);
    for (level, quiet) in [(-4_000, false), (-4_001, true)] {
        let d = Dialogue::new(
            SoundProfile::from_hops(
                10_000,
                200_000,
                (0..20).map(|_| Hop {
                    peak_dbfs_hundredths: level,
                    rms_dbfs_hundredths: level,
                    voice: false,
                }),
            )
            .unwrap(),
            vec![],
            Settings::default(),
        )
        .unwrap();
        assert_eq!(d.guard(100_000, AudibleSide::In).is_ok(), quiet);
    }
}

#[test]
fn dialogue_start_side_kept_and_removed_slack_are_not_interchangeable() {
    let d = Dialogue::new(
        profile(
            10_000,
            3_000_000,
            &[(0, 200_000), (800_000, 1_200_000), (2_000_000, 2_300_000)],
            &[(99, -6_000)],
        ),
        vec![
            word("keep", 0, 1_000_000),
            word("remove", 1_000_000, 1_200_000),
            word("next", 2_000_000, 2_300_000),
        ],
        Settings::default(),
    )
    .unwrap();
    let cut = d
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap();
    assert_eq!(cut.start.at_micros(), 995_000);
    let levels: Vec<_> = (100..120).map(|i| (i, -3_000)).collect();
    let d = Dialogue::new(
        profile(
            10_000,
            3_000_000,
            &[
                (0, 1_200_000),
                (1_400_000, 1_600_000),
                (2_000_000, 2_300_000),
            ],
            &levels,
        ),
        vec![
            word("keep", 0, 1_000_000),
            word("remove", 1_000_000, 1_600_000),
            word("next", 2_000_000, 2_300_000),
        ],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(
        d.resolve(CutRequest::RemoveWords { from: 1, to: 1 })
            .unwrap()
            .start
            .at_micros(),
        1_320_000
    );
}

proptest! {
    #[test]
    fn dialogue_generated_dips_obey_reach_and_returned_edges_pass_guard(index in 0_usize..50) {
        let d=Dialogue::new(profile(10_000,1_000_000,&[(0,500_000)],&[(index,-6_000)]),vec![word("keep",0,400_000),word("remove",400_000,500_000)],Settings::default()).unwrap();
        let at=index as u64*10_000+5_000;
        if 50_000<at&&at<350_000 {prop_assert!(d.guard(at,AudibleSide::Out).is_err());}
        if let Ok(cut)=d.resolve(CutRequest::RemoveWords{from:1,to:1}) {for edge in [cut.start,cut.end] {prop_assert!(d.guard(edge.at_micros(),edge.side()).is_ok());}}
    }
}

// Regression supplied by the independent DQ3 review.
#[test]
fn dialogue_fractional_dip_reach_refuses_the_extra_third_microsecond() {
    let d = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 500_000)], &[(6, -6_000)]),
        vec![word("word", 16_733, 400_000)],
        Settings::default(),
    )
    .unwrap();
    assert!(d.guard(66_733, AudibleSide::Out).is_ok());
    let basis = Dialogue::new(
        profile(10_000, 1_000_000, &[(0, 500_000)], &[(6, -6_000)]),
        vec![word("short", 0, 110_000), word("next", 110_000, 500_000)],
        Settings::default(),
    )
    .unwrap();
    let e = basis
        .resolve(CutRequest::RemoveWords { from: 1, to: 1 })
        .unwrap()
        .start;
    assert_eq!(e.at_micros(), 65_000);
    let f = d.frame_edge(e, Rational::new(30_000, 1_001).unwrap());
    assert!(
        matches!(
            f,
            Err(Refusal {
                reason: RefusalReason::SoundingWord { index: 0, .. },
                ..
            })
        ),
        "DQ3_FRACTIONAL_REACH: {f:?}"
    );
}
