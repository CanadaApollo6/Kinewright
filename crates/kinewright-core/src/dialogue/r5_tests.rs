use super::*;
fn word(text: &str, start: u64) -> Word {
    Word {
        text: text.into(),
        start_micros: start,
        end_micros: start + 80_000,
    }
}
fn take(words: Vec<Word>, sounds: &[(u64, u64, i16, bool)]) -> Dialogue {
    let profile = SoundProfile::from_hops(
        10_000,
        3_000_000,
        (0..300).map(|i| {
            let t = i * 10_000;
            let &(.., db, voice) = sounds
                .iter()
                .rev()
                .find(|&&(a, b, _, _)| a <= t && t < b)
                .unwrap_or(&(0, 0, -8238, false));
            Hop {
                rms_dbfs_hundredths: db,
                peak_dbfs_hundredths: db,
                voice,
            }
        }),
    )
    .unwrap();
    Dialogue::new(profile, words, Settings::default()).unwrap()
}
fn removal(index: usize) -> CutRequest {
    CutRequest::RemoveWords {
        from: index,
        to: index,
        leave_micros: None,
    }
}
#[test]
fn r5_early_voiced_onset_is_bounded_by_the_neighbour_start() {
    let d = take(
        vec![
            word("kept", 200_000),
            word("um", 1_400_000),
            word("after", 2_200_000),
        ],
        &[
            (200_000, 350_000, -4400, true),
            (700_000, 1_500_000, -4400, true),
            (2_200_000, 2_400_000, -4400, true),
        ],
    );
    let cut = d.resolve(removal(1)).expect("R5_EARLY_VOICED_ONSET");
    assert!(cut.start.at_micros() < 700_000);
    assert!(cut.end.at_micros() > 1_500_000);
}
#[test]
fn r5_detached_transients_do_not_gain_the_early_voice_window() {
    for voiced in [false, true] {
        let d = take(
            vec![
                word("kept", 200_000),
                word("um", 1_400_000),
                word("after", 2_200_000),
            ],
            &[
                (200_000, 350_000, -4400, true),
                (700_000, 730_000, -4400, voiced),
                (1_350_000, 1_500_000, -4400, true),
                (2_200_000, 2_400_000, -4400, true),
            ],
        );
        let err = d
            .validate_cut(
                removal(1),
                [
                    crate::ExactMicros::integer(700_000),
                    crate::ExactMicros::integer(1_600_000),
                ],
            )
            .unwrap_err();
        assert!(matches!(
            err.reason,
            RefusalReason::Selection {
                issue: SelectionIssue::UnattributedSound { .. }
            }
        ));
    }
}
#[test]
fn r5_early_voice_does_not_cross_a_neighbour_or_extend_past_750_ms() {
    for (start, other) in [(1_460_000, None), (1_400_000, Some(900_000))] {
        let mut words = vec![word("kept", 200_000)];
        if let Some(at) = other {
            words.push(word("also_kept", at));
        }
        words.push(word("um", start));
        words.push(word("after", 2_200_000));
        let selected = words.len() - 2;
        let d = take(
            words,
            &[
                (200_000, 350_000, -4400, true),
                (700_000, 1_600_000, -4400, true),
                (2_200_000, 2_400_000, -4400, true),
            ],
        );
        assert!(
            d.validate_cut(
                removal(selected),
                [
                    crate::ExactMicros::integer(700_000),
                    crate::ExactMicros::integer(1_600_000)
                ]
            )
            .is_err()
        );
    }
}
#[test]
fn r5_unvoiced_start_containing_sound_does_not_get_the_extension() {
    let d = take(
        vec![
            word("kept", 200_000),
            word("um", 1_400_000),
            word("after", 2_200_000),
        ],
        &[
            (200_000, 350_000, -4400, true),
            (700_000, 1_500_000, -4400, false),
            (2_200_000, 2_400_000, -4400, true),
        ],
    );
    let r = d
        .validate_cut(
            removal(1),
            [
                crate::ExactMicros::integer(700_000),
                crate::ExactMicros::integer(1_600_000),
            ],
        )
        .unwrap_err();
    assert!(matches!(
        r.reason,
        RefusalReason::Selection {
            issue: SelectionIssue::UnattributedSound { .. }
        }
    ));
}
