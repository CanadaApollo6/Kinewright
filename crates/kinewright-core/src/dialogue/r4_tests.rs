use super::*;
fn token(text: &str, start: u64, end: u64) -> Word {
    Word {
        text: text.into(),
        start_micros: start,
        end_micros: end,
    }
}
fn measured_take(duration: u64, words: Vec<Word>, sound: &[(u64, u64, i16)]) -> Dialogue {
    let p = SoundProfile::from_hops(
        10_000,
        duration,
        (0..duration / 10_000).map(|i| {
            let t = i * 10_000;
            let active = sound.iter().find(|&&(a, b, _)| a <= t && t < b);
            let db = active.map_or(-8_238, |&(_, _, db)| db);
            Hop {
                rms_dbfs_hundredths: db,
                peak_dbfs_hundredths: db,
                voice: active.is_some(),
            }
        }),
    )
    .unwrap();
    Dialogue::new(p, words, Settings::default()).unwrap()
}
#[test]
fn r4_nearest_quiet_run_keeps_the_were_tail_after_its_stub_end() {
    let d = measured_take(
        3_000_000,
        vec![
            token("we", 200_000, 280_000),
            token("were", 520_000, 600_000),
            token("um", 1_480_000, 1_560_000),
            token("now", 2_200_000, 2_280_000),
        ],
        &[
            (200_000, 520_000, -4_468),
            (720_000, 820_000, -3_600),
            (1_480_000, 1_680_000, -4_468),
            (2_200_000, 2_400_000, -4_468),
        ],
    );
    let cut = d
        .resolve(CutRequest::RemoveWords {
            from: 2,
            to: 2,
            leave_micros: None,
        })
        .expect("R4_NEAREST_KEEPS_WERE");
    assert!(
        cut.start.at_micros() >= 820_000,
        "R4_NEAREST_KEEPS_WERE: {cut:?}"
    );
    assert!(cut.end.at_micros() <= 2_200_000);
    assert!(
        matches!(cut.start.placement(),EdgePlacement::Quiet{run,..} if run.start_micros()==820_000)
    );
    assert_eq!(cut.air.unwrap().actual_micros, 660_000);
}
#[test]
fn r4_a_pause_cannot_erase_an_unrequested_start_inside_quiet() {
    let d = measured_take(
        2_000_000,
        vec![
            token("so", 200_000, 280_000),
            token("that", 680_000, 760_000),
        ],
        &[(200_000, 300_000, -4_468), (1_220_000, 1_400_000, -4_468)],
    );
    match d.resolve(CutRequest::TrimPause {
        after_word: 0,
        leave_micros: 300_000,
    }) {
        Ok(cut) => assert!(
            !(cut.start.at_micros() <= 680_000 && 680_000 < cut.end.at_micros()),
            "R4_UNREQUESTED_START: {cut:?}"
        ),
        Err(e) => assert_eq!(
            serde_json::to_value(e.reason).unwrap()["violation"],
            "unrequested_word_start",
            "R4_UNREQUESTED_START_TYPED"
        ),
    }
}
#[test]
fn r4_an_unanchored_sound_between_requested_words_is_refused() {
    let d = measured_take(
        4_000_000,
        vec![
            token("keep", 200_000, 280_000),
            token("remove", 1_000_000, 1_080_000),
            token("these", 2_000_000, 2_080_000),
            token("after", 3_000_000, 3_080_000),
        ],
        &[
            (200_000, 300_000, -4_468),
            (1_000_000, 1_100_000, -4_468),
            (1_500_000, 1_600_000, -4_468),
            (2_000_000, 2_100_000, -4_468),
            (3_000_000, 3_100_000, -4_468),
        ],
    );
    let r = d.resolve(CutRequest::RemoveWords {
        from: 1,
        to: 2,
        leave_micros: None,
    });
    assert!(r.is_err(), "R4_UNANCHORED_SOUND: {r:?}");
    assert_eq!(
        serde_json::to_value(r.unwrap_err().reason).unwrap()["violation"],
        "unattributed_sound"
    );
}

#[test]
fn r4_a_quiet_take_uses_a_dip_above_the_old_floor_hugging_threshold() {
    let words = vec![
        token("keep", 700_000, 1_000_000),
        token("um", 1_010_000, 1_090_000),
        token("after", 1_700_000, 1_780_000),
    ];
    let p = SoundProfile::from_hops(
        10_000,
        2_000_000,
        (0..200).map(|i| {
            let t = i * 10_000;
            let voice = (700_000..1_300_000).contains(&t) || (1_700_000..1_900_000).contains(&t);
            let level = if (980_000..1_020_000).contains(&t) {
                -7_100
            } else if voice {
                -4_468
            } else {
                -8_238
            };
            Hop {
                rms_dbfs_hundredths: level,
                peak_dbfs_hundredths: level,
                voice,
            }
        }),
    )
    .unwrap();
    let d = Dialogue::new(p, words, Settings::default()).unwrap();
    let result = d.resolve(CutRequest::RemoveWords {
        from: 1,
        to: 1,
        leave_micros: None,
    });
    assert!(result.is_ok(), "R4_QUIET_TAKE_DIP: {result:?}");
    let cut = result.unwrap();
    assert!(matches!(
        cut.start.placement(),
        EdgePlacement::LevelDip { .. }
    ));
    assert!(matches!(cut.end.placement(), EdgePlacement::Quiet { .. }));
}

#[test]
fn r4_a_reported_long_pause_names_the_clip_boundary_that_limited_it() {
    let d = measured_take(
        5_000_000,
        vec![
            token("it", 1_000_000, 1_080_000),
            token("and", 4_000_000, 4_080_000),
        ],
        &[
            (1_000_000, 1_100_000, -4_468),
            (4_000_000, 4_100_000, -4_468),
        ],
    );
    assert!(d.gap_runs().iter().any(|r| r.duration() > 700_000));
    let result = d.resolve_pause_in(0, 300_000, Span::new(0, 1_350_000).unwrap());
    assert!(result.is_err());
    let reason = serde_json::to_value(result.unwrap_err().reason).unwrap();
    assert_eq!(
        reason["violation"], "pause_limited",
        "R4_PAUSE_CLIP_MEASURE: {reason}"
    );
    assert_eq!(reason["full_micros"], 2_900_000);
    assert_eq!(reason["usable_micros"], 250_000);
}

#[test]
fn r4_an_accessible_reported_pause_trims_with_the_original_margins() {
    let d = measured_take(
        3_000_000,
        vec![
            token("it", 200_000, 280_000),
            token("is", 2_000_000, 2_080_000),
        ],
        &[(200_000, 300_000, -4_468), (2_000_000, 2_200_000, -4_468)],
    );
    let available = Span::new(0, 3_000_000).unwrap();
    let result = d.resolve_pause_in(0, 300_000, available).unwrap();
    assert_eq!(result.air.unwrap().actual_micros, 300_000);
    assert_eq!(result.end.at_micros() - result.start.at_micros(), 1_400_000);
    assert!(d.guard(result.start.at_micros(), AudibleSide::Out).is_ok());
    assert!(d.guard(result.end.at_micros(), AudibleSide::In).is_ok());
}

#[test]
fn r4_exact_selection_checks_kept_sound_even_when_its_start_is_outside() {
    let d = measured_take(
        3_000_000,
        vec![
            token("were", 520_000, 600_000),
            token("um", 1_480_000, 1_560_000),
        ],
        &[(720_000, 820_000, -4_468), (1_480_000, 1_680_000, -4_468)],
    );
    let result = d.validate_cut(
        CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: None,
        },
        [
            crate::ExactMicros::integer(600_000),
            crate::ExactMicros::integer(1_800_000),
        ],
    );
    assert!(matches!(
        result.unwrap_err().reason,
        RefusalReason::Selection {
            issue: SelectionIssue::UnrequestedSound { word: 0, .. }
        }
    ));
}
#[test]
fn r4_selection_checks_fractional_frame_extents_and_half_open_starts() {
    let d = measured_take(
        2_000_000,
        vec![
            token("remove", 100_000, 180_000),
            token("keep", 600_000, 680_000),
        ],
        &[(1_400_000, 1_500_000, -4_468)],
    );
    let request = CutRequest::RemoveWords {
        from: 0,
        to: 0,
        leave_micros: None,
    };
    assert!(
        d.validate_cut(
            request,
            [
                crate::ExactMicros::integer(50_000),
                crate::ExactMicros::integer(600_000)
            ]
        )
        .is_ok()
    );
    let error = d
        .validate_cut(
            request,
            [
                crate::ExactMicros::integer(50_000),
                crate::ExactMicros::new(1_800_001, 3).unwrap(),
            ],
        )
        .unwrap_err();
    assert!(matches!(
        error.reason,
        RefusalReason::Selection {
            issue: SelectionIssue::UnrequestedWordStart { word: 1 }
        }
    ));
}
#[test]
fn r4_sound_ownership_does_not_use_supplied_word_ends() {
    let make = |end| {
        measured_take(
            2_000_000,
            vec![token("remove", 500_000, end)],
            &[(500_000, 900_000, -4_468)],
        )
    };
    let a = make(580_000);
    let b = make(1_200_000);
    let request = CutRequest::RemoveWords {
        from: 0,
        to: 0,
        leave_micros: None,
    };
    let extent = [
        crate::ExactMicros::integer(400_000),
        crate::ExactMicros::integer(1_000_000),
    ];
    assert_eq!(
        a.validate_cut(request, extent),
        b.validate_cut(request, extent)
    );
    assert!(a.validate_cut(request, extent).is_ok());
}
#[test]
fn r4_a_plausible_kept_sound_owner_vetoes_the_removal() {
    let d = measured_take(
        2_000_000,
        vec![
            token("keep", 800_000, 880_000),
            token("remove", 1_200_000, 1_280_000),
        ],
        &[(1_000_000, 1_100_000, -4_468)],
    );
    let result = d.validate_cut(
        CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: None,
        },
        [
            crate::ExactMicros::integer(950_000),
            crate::ExactMicros::integer(1_400_000),
        ],
    );
    assert!(matches!(
        result.unwrap_err().reason,
        RefusalReason::Selection {
            issue: SelectionIssue::UnrequestedSound { word: 0, .. }
        }
    ));
}

#[test]
fn r4_dip_limit_is_between_floor_and_speech_with_six_db_floor_margin() {
    for speech in [-4_468, -2_000] {
        for separation in [2_601, 3_770, 6_000] {
            let floor = speech - separation;
            let profile = SoundProfile::from_hops(
                10_000,
                20_000,
                [
                    Hop {
                        rms_dbfs_hundredths: floor,
                        peak_dbfs_hundredths: floor,
                        voice: false,
                    },
                    Hop {
                        rms_dbfs_hundredths: speech,
                        peak_dbfs_hundredths: speech,
                        voice: true,
                    },
                ],
            )
            .unwrap();
            let d = Dialogue::new(profile, Vec::new(), Settings::default()).unwrap();
            let limit = d.thresholds().dip_dbfs_hundredths.unwrap();
            assert!(
                limit >= floor + 600 && limit < speech && limit <= d.thresholds().quiet_rms_max
            );
        }
    }
}

#[test]
fn r4_recorded_nine_unrequested_starts_are_refused_even_in_quiet() {
    // R3's old removed intervals, re-expressed on the source 30 fps grid.
    // These are invariant probes, not a replay of the recording's measurements.
    for (word, at, a, b) in [
        ("that's", 9_680_000_u64, 283_u64, 303_u64),
        ("in", 40_480_000_u64, 1177_u64, 1218_u64),
        ("your", 40_560_000_u64, 1177_u64, 1218_u64),
        ("from", 136_960_000_u64, 4092_u64, 4109_u64),
        ("I", 288_080_000_u64, 8625_u64, 8651_u64),
        ("see", 288_320_000_u64, 8625_u64, 8651_u64),
        ("now", 292_720_000_u64, 8737_u64, 8782_u64),
        ("it", 304_080_000_u64, 9121_u64, 9151_u64),
        ("okay", 365_360_000_u64, 10_916_u64, 10_961_u64),
    ] {
        let d = measured_take(
            (b * 1_000_000 / 30 + 100_000).next_multiple_of(10_000),
            vec![
                token("before", 10_000, 20_000),
                token(word, at, at + 80_000),
            ],
            &[(0, 10_000, -4_468)],
        );
        let cut = [
            crate::ExactMicros::new(u128::from(a) * 1_000_000, 30).unwrap(),
            crate::ExactMicros::new(u128::from(b) * 1_000_000, 30).unwrap(),
        ];
        let reason = d
            .validate_cut(
                CutRequest::TrimPause {
                    after_word: 0,
                    leave_micros: 300_000,
                },
                cut,
            )
            .unwrap_err()
            .reason;
        assert!(
            matches!(
                reason,
                RefusalReason::Selection {
                    issue: SelectionIssue::UnrequestedWordStart { word: 1 }
                }
            ),
            "R4_R3_START_{word}: {reason:?}"
        );
    }
}

#[test]
fn r4_a_requested_word_can_own_several_measured_syllables() {
    let d = measured_take(
        2_000_000,
        vec![token("remove", 500_000, 580_000)],
        &[(500_000, 600_000, -4_468), (700_000, 800_000, -4_468)],
    );
    assert!(
        d.validate_cut(
            CutRequest::RemoveWords {
                from: 0,
                to: 0,
                leave_micros: None
            },
            [
                crate::ExactMicros::integer(400_000),
                crate::ExactMicros::integer(900_000)
            ]
        )
        .is_ok()
    );
}

#[test]
fn r4_calibrated_dips_cannot_bypass_measured_quiet_margins() {
    let d = measured_take(1_000_000, Vec::new(), &[(500_000, 800_000, -4_468)]);
    for at in [10_000, 490_000] {
        assert!(
            d.guard(at, AudibleSide::Out).is_err(),
            "R4_QUIET_MARGIN_NOT_A_DIP: {at}"
        );
    }
    for at in [30_000, 470_000] {
        assert!(d.guard(at, AudibleSide::Out).is_ok());
    }
}
