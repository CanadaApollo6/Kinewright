use super::*;
fn take(speech: i16, floor: i16, extra: impl Fn(u64) -> Option<(i16, bool)>) -> SoundProfile {
    SoundProfile::from_hops(
        10_000,
        2_000_000,
        (0..200).map(|i| {
            let at = i * 10_000;
            let (level, voice) = extra(at).unwrap_or_else(|| {
                if (300_000..600_000).contains(&at) {
                    (speech, true)
                } else {
                    (floor, false)
                }
            });
            Hop {
                rms_dbfs_hundredths: level,
                peak_dbfs_hundredths: level,
                voice,
            }
        }),
    )
    .unwrap()
}
fn words() -> Vec<Word> {
    vec![Word {
        text: "spoken".into(),
        start_micros: 300_000,
        end_micros: 600_000,
    }]
}
#[test]
fn rehearsal_recording_levels_are_stored_and_validate_the_cache() {
    let p = take(-5_000, -9_000, |_| None);
    let value = serde_json::to_value(&p).unwrap();
    assert_eq!(
        value["levels"]["speech_rms_hundredths"], -5_000,
        "DQ_REHEARSAL_STORED_SPEECH"
    );
    assert_eq!(value["levels"]["noise_floor_hundredths"], -9_000);
    assert_eq!(serde_json::from_value::<SoundProfile>(value).unwrap(), p);
}
#[test]
fn rehearsal_quiet_take_does_not_call_soft_sound_quiet() {
    let p = take(-5_000, -9_000, |at| {
        (1_000_000..1_400_000)
            .contains(&at)
            .then_some((-6_500, false))
    });
    let d = Dialogue::new(p, words(), Settings::default()).unwrap();
    assert!(
        d.guard(1_200_000, AudibleSide::Out).is_err(),
        "DQ_REHEARSAL_RELATIVE_QUIET"
    );
}
#[test]
fn rehearsal_soft_voice_is_never_a_valley_gap() {
    let p = take(-2_000, -9_000, |at| {
        (1_000_000..1_700_000)
            .contains(&at)
            .then_some((-6_000, true))
    });
    let d = Dialogue::new(
        p,
        vec![Word {
            text: "soft".into(),
            start_micros: 1_000_000,
            end_micros: 1_700_000,
        }],
        Settings::default(),
    )
    .unwrap();
    assert!(
        d.guard(1_300_000, AudibleSide::Out).is_err(),
        "DQ_REHEARSAL_VOICE_VALLEY"
    );
}
#[test]
fn rehearsal_noise_floor_refuses_instead_of_guessing() {
    let p = take(-2_000, -4_200, |_| None);
    let result = Dialogue::new(p, words(), Settings::default());
    assert!(result.is_err(), "DQ_REHEARSAL_NOISE_SEPARATION");
}
fn pauses(split: bool) -> Dialogue {
    let words = [
        ("before", 200_000, 400_000),
        ("uh", 900_000, 1_100_000),
        ("after", 2_100_000, 2_300_000),
    ]
    .into_iter()
    .map(|(text, start_micros, end_micros)| Word {
        text: text.into(),
        start_micros,
        end_micros,
    })
    .collect();
    let p = SoundProfile::from_hops(
        10_000,
        3_000_000,
        (0..300).map(|i| {
            let t = i * 10_000;
            let voice = [
                (200_000, 400_000),
                (900_000, 1_100_000),
                (2_100_000, 2_300_000),
            ]
            .iter()
            .any(|&(a, b)| a <= t && t < b)
                || split && (1_400_000..1_500_000).contains(&t);
            let db = if voice { -2_000 } else { -9_000 };
            Hop {
                peak_dbfs_hundredths: db,
                rms_dbfs_hundredths: db,
                voice,
            }
        }),
    )
    .unwrap();
    Dialogue::new(p, words, Settings::default()).unwrap()
}
#[test]
fn rehearsal_filler_preserves_the_longer_pause_air() {
    let d = pauses(false);
    let cut = d
        .resolve(CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: None,
        })
        .unwrap();
    let kept_air = (cut.start.at_micros() - 400_000) + (2_100_000 - cut.end.at_micros());
    assert_eq!(kept_air, 1_000_000, "DQ_REHEARSAL_NATURAL_PAUSE");
    assert_eq!(
        (cut.start.at_micros(), cut.end.at_micros()),
        (430_000, 1_130_000)
    );
    // A mirrored take must preserve the longer outgoing flank instead.
    let mirrored = SoundProfile::from_hops(
        10_000,
        3_000_000,
        (0..d.profile().len())
            .rev()
            .map(|i| d.profile().hop(i).unwrap()),
    )
    .unwrap();
    let labels = ["after", "uh", "before"];
    let words = d
        .words()
        .iter()
        .rev()
        .enumerate()
        .map(|(i, w)| Word {
            text: labels[i].into(),
            start_micros: 3_000_000 - w.end_micros,
            end_micros: 3_000_000 - w.start_micros,
        })
        .collect();
    let reversed = Dialogue::new(mirrored, words, Settings::default()).unwrap();
    let cut = reversed
        .resolve(CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: None,
        })
        .unwrap();
    assert_eq!(
        (cut.start.at_micros(), cut.end.at_micros()),
        (1_870_000, 2_570_000)
    );
    assert_eq!(cut.air.unwrap().actual_micros, 1_000_000);
}
#[test]
fn rehearsal_pause_chooses_the_longest_of_two_runs() {
    let d = pauses(true);
    let cut = d
        .resolve(CutRequest::TrimPause {
            after_word: 1,
            leave_micros: 200_000,
        })
        .expect("DQ_REHEARSAL_LONGEST_PAUSE");
    assert_eq!(
        (cut.start.at_micros(), cut.end.at_micros()),
        (1_620_000, 2_020_000)
    );
    assert_eq!(cut.other_pauses_micros, vec![300_000]);
    assert_eq!(cut.air.unwrap().actual_micros, 200_000);
    assert_eq!(
        d.resolve(CutRequest::TrimPause {
            after_word: 1,
            leave_micros: 600_000
        })
        .unwrap_err()
        .reason,
        RefusalReason::NothingToTrim
    );
}

#[test]
fn rehearsal_explicit_word_air_uses_the_ratio_and_the_guard_margins() {
    let d = pauses(false);
    let cut = d
        .resolve(CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: Some(300_000),
        })
        .unwrap();
    assert_eq!(
        (cut.start.at_micros(), cut.end.at_micros()),
        (580_000, 1_980_000)
    );
    assert_eq!(cut.air.unwrap().actual_micros, 300_000);
    assert_eq!(
        d.resolve(CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: Some(10_000)
        })
        .unwrap_err()
        .reason,
        RefusalReason::AirBelowGuardMargin {
            requested_micros: 10_000,
            minimum_micros: 60_000
        }
    );
}
#[test]
fn rehearsal_calibration_cannot_be_omitted_or_forged() {
    let p = take(-5_000, -9_000, |_| None);
    let mut json = serde_json::to_value(&p).unwrap();
    json["levels"]["speech_rms_hundredths"] = serde_json::json!(-2_000);
    assert!(serde_json::from_value::<SoundProfile>(json.clone()).is_err());
    json.as_object_mut().unwrap().remove("levels");
    assert!(serde_json::from_value::<SoundProfile>(json).is_err());
    let p = SoundProfile::from_hops(
        10_000,
        100_000,
        (0..10).map(|_| Hop {
            peak_dbfs_hundredths: -9_000,
            rms_dbfs_hundredths: -9_000,
            voice: false,
        }),
    )
    .unwrap();
    assert!(matches!(
        Dialogue::new(p, vec![], Settings::default()),
        Err(InputError::SpeechLevelUnavailable)
    ));
    assert!(matches!(
        Dialogue::new(take(-2_000, -4_200, |_| None), words(), Settings::default()),
        Err(InputError::NoiseSeparation {
            speech_rms_hundredths: -2_000,
            noise_floor_hundredths: -4_200,
            quiet_rms_hundredths: -4_001
        })
    ));
}

#[test]
fn rehearsal_first_word_boundary_keeps_requested_air_on_the_kept_side() {
    let d = pauses(false);
    let natural = d
        .resolve(CutRequest::RemoveWords {
            from: 0,
            to: 0,
            leave_micros: None,
        })
        .unwrap();
    assert_eq!(natural.start.at_micros(), 0);
    assert_eq!(natural.air.unwrap().actual_micros, 100_000);
    let explicit = d
        .resolve(CutRequest::RemoveWords {
            from: 0,
            to: 0,
            leave_micros: Some(200_000),
        })
        .unwrap();
    assert_eq!(
        (explicit.start.at_micros(), explicit.end.at_micros()),
        (0, 700_000)
    );
    assert_eq!(explicit.air.unwrap().actual_micros, 200_000);
    let bounded = d
        .resolve(CutRequest::RemoveWords {
            from: 0,
            to: 0,
            leave_micros: Some(9_000_000),
        })
        .unwrap();
    assert_eq!(bounded.air.unwrap().actual_micros, 470_000);
    assert!(d.guard(bounded.start.at_micros(), AudibleSide::Out).is_ok());
    assert!(d.guard(bounded.end.at_micros(), AudibleSide::In).is_ok());
}
#[test]
fn rehearsal_last_word_boundary_keeps_requested_air_on_the_kept_side() {
    let d = pauses(false);
    let natural = d
        .resolve(CutRequest::RemoveWords {
            from: 2,
            to: 2,
            leave_micros: None,
        })
        .unwrap();
    assert_eq!(natural.end.at_micros(), 3_000_000);
    assert_eq!(natural.air.unwrap().actual_micros, 150_000);
    let explicit = d
        .resolve(CutRequest::RemoveWords {
            from: 2,
            to: 2,
            leave_micros: Some(200_000),
        })
        .unwrap();
    assert_eq!(
        (explicit.start.at_micros(), explicit.end.at_micros()),
        (1_300_000, 3_000_000)
    );
    assert_eq!(explicit.air.unwrap().actual_micros, 200_000);
    let bounded = d
        .resolve(CutRequest::RemoveWords {
            from: 2,
            to: 2,
            leave_micros: Some(9_000_000),
        })
        .unwrap();
    assert_eq!(bounded.air.unwrap().actual_micros, 970_000);
    assert!(d.guard(bounded.start.at_micros(), AudibleSide::Out).is_ok());
    assert!(d.guard(bounded.end.at_micros(), AudibleSide::In).is_ok());
}

#[test]
fn rehearsal_one_measured_flank_keeps_standard_air() {
    let words = [
        ("recognized", 1_000_000, 1_400_000),
        ("that", 1_400_000, 1_700_000),
        ("next", 2_500_000, 2_800_000),
    ]
    .into_iter()
    .map(|(text, start_micros, end_micros)| Word {
        text: text.into(),
        start_micros,
        end_micros,
    })
    .collect();
    let profile = SoundProfile::from_hops(
        10_000,
        3_000_000,
        (0..300).map(|i| {
            let t = i * 10_000;
            let voice = (980_000..1_720_000).contains(&t) || (2_500_000..2_820_000).contains(&t);
            let db = if i == 140 {
                -6_000
            } else if voice {
                -2_000
            } else {
                -9_000
            };
            Hop {
                rms_dbfs_hundredths: db,
                peak_dbfs_hundredths: db,
                voice,
            }
        }),
    )
    .unwrap();
    let d = Dialogue::new(profile, words, Settings::default()).unwrap();
    let cut = d
        .resolve(CutRequest::RemoveWords {
            from: 1,
            to: 1,
            leave_micros: None,
        })
        .unwrap();
    assert_eq!(
        cut.air.unwrap().actual_micros,
        100_000,
        "DQ_REHEARSAL_SINGLE_FLANK_STANDARD_AIR"
    );
    assert_eq!(cut.end.at_micros(), 2_400_000);
    assert!(d.guard(cut.start.at_micros(), AudibleSide::Out).is_ok());
    assert!(d.guard(cut.end.at_micros(), AudibleSide::In).is_ok());
}
#[test]
fn rehearsal_level_shift_covers_all_thresholds_and_quantiles_are_exact() {
    let d = Dialogue::new(take(-5_000, -9_000, |_| None), words(), Settings::default()).unwrap();
    let s = d.thresholds();
    assert_eq!(
        [
            s.quiet_rms_max,
            s.quiet_peak_max,
            s.valley_rms_max,
            s.valley_peak_max
        ],
        [-7_001, -5_603, -8_398, -6_842]
    );
    assert_eq!(s.dip_dbfs_hundredths, Some(-7_001));
    let p = SoundProfile::from_hops(
        10_000,
        120_000,
        [
            -9_000, -8_000, -7_000, -6_000, -5_000, -4_000, -3_000, -2_000, -1_000, 0,
        ]
        .into_iter()
        .map(|db| Hop {
            rms_dbfs_hundredths: db,
            peak_dbfs_hundredths: db,
            voice: false,
        })
        .chain([-2_101, -1_900].into_iter().map(|db| Hop {
            rms_dbfs_hundredths: db,
            peak_dbfs_hundredths: db,
            voice: true,
        })),
    )
    .unwrap();
    assert_eq!(
        p.levels(),
        RecordingLevels {
            speech_rms_hundredths: Some(-2_001),
            noise_floor_hundredths: Some(-9_000)
        }
    );
}

#[test]
fn rehearsal_missing_noise_floor_does_not_invent_dips() {
    let p = SoundProfile::from_hops(
        10_000,
        1_000_000,
        (0..100).map(|_| Hop {
            rms_dbfs_hundredths: LEVEL_FLOOR,
            peak_dbfs_hundredths: LEVEL_FLOOR,
            voice: true,
        }),
    )
    .unwrap();
    let d = Dialogue::new(
        p,
        vec![Word {
            text: "floor".into(),
            start_micros: 100_000,
            end_micros: 900_000,
        }],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(d.thresholds().dip_dbfs_hundredths, None);
    assert!(d.guard(130_000, AudibleSide::Out).is_err());
}
#[test]
fn rehearsal_quiet_pcm_uses_the_recordings_measured_level() {
    let mut builder = measure::ProfileBuilder::new(1_000, 1).unwrap();
    let samples: Vec<_> = (0..2_000)
        .map(|i| {
            if (300..600).contains(&i) {
                0.003_162_28
            } else if (1_000..1_400).contains(&i) {
                0.000_562_341
            } else {
                0.000_031_622_8
            }
        })
        .collect();
    builder.push(&samples).unwrap();
    let profile = builder
        .finish(Some(&[SourceRange {
            start_micros: 300_000,
            end_micros: 600_000,
        }]))
        .unwrap();
    assert_eq!(
        profile.levels(),
        RecordingLevels {
            speech_rms_hundredths: Some(-5000),
            noise_floor_hundredths: Some(-9000)
        }
    );
    let d = Dialogue::new(profile, words(), Settings::default()).unwrap();
    assert!(d.guard(1_200_000, AudibleSide::Out).is_err());
}
#[test]
fn rehearsal_voice_flag_excludes_a_valley_at_the_reference_speech_level() {
    let profile = take(-2_000, -9_000, |at| {
        if at < 1_000_000 {
            Some((-2_000, true))
        } else if (1_100_000..1_300_000).contains(&at) {
            Some((-6_000, true))
        } else {
            None
        }
    });
    let d = Dialogue::new(
        profile,
        vec![Word {
            text: "soft".into(),
            start_micros: 1_100_000,
            end_micros: 1_300_000,
        }],
        Settings::default(),
    )
    .unwrap();
    assert_eq!(d.profile().levels().speech_rms_hundredths, Some(-2_000));
    assert!(
        matches!(
            d.guard(1_200_000, AudibleSide::Out).unwrap_err().reason,
            RefusalReason::SoundingWord { index: 0, .. }
        ),
        "DQ_REHEARSAL_VOICE_VALLEY_INDEPENDENT"
    );
}
