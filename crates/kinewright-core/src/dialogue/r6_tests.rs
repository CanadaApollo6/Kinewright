use super::*;
use crate::ExactMicros;
fn fixture(length: u64, other_start: Option<u64>) -> Dialogue {
    let mut words = vec![("kept", 200_000), ("um", 1_400_000), ("after", 2_200_000)];
    if let Some(at) = other_start {
        words.push(("unrequested", at));
    }
    words.sort_by_key(|w| w.1);
    let profile = SoundProfile::from_hops(
        10_000,
        3_000_000,
        (0..300).map(|i| {
            let t = i * 10_000;
            let sound = [
                (200_000, 400_000),
                (700_000, 700_000 + length),
                (1_350_000, 1_900_000),
                (2_200_000, 2_800_000),
            ]
            .into_iter()
            .any(|(a, b)| a <= t && t < b);
            Hop {
                rms_dbfs_hundredths: if sound { -4400 } else { -8238 },
                // RMS-quiet neighbours deliberately have both VAD and a high peak.
                peak_dbfs_hundredths: -1000,
                voice: sound || t == 690_000 || t == 700_000 + length,
            }
        }),
    )
    .unwrap();
    Dialogue::new(
        profile,
        words
            .into_iter()
            .map(|(text, start)| Word {
                text: text.into(),
                start_micros: start,
                end_micros: start + 80_000,
            })
            .collect(),
        Settings::default(),
    )
    .unwrap()
}
fn validate(d: &Dialogue, start: u64, end: u64) -> Result<(), Refusal> {
    let i = d.words().iter().position(|w| w.text == "um").unwrap();
    d.validate_cut(
        CutRequest::RemoveWords {
            from: i,
            to: i,
            leave_micros: None,
        },
        [ExactMicros::integer(start), ExactMicros::integer(end)],
    )
}
#[test]
fn r6_ten_and_twenty_ms_detached_blips_need_only_rms_quiet_neighbours() {
    for length in [10_000, 20_000] {
        validate(&fixture(length, None), 700_000, 1_900_000).expect("R6_RMS_QUIET_TRANSIENT");
    }
}
#[test]
fn r6_thirty_ms_detached_component_still_refuses() {
    let err = validate(&fixture(30_000, None), 700_000, 1_900_000).unwrap_err();
    assert!(
        matches!(
            err.reason,
            RefusalReason::Selection {
                issue: SelectionIssue::UnattributedSound { .. }
            }
        ),
        "R6_LONG_TRANSIENT_REFUSED: {err:?}"
    );
}
#[test]
fn r6_transient_never_waives_an_unrequested_asr_start() {
    let err = validate(&fixture(10_000, Some(705_000)), 700_000, 1_900_000).unwrap_err();
    assert!(
        matches!(
            err.reason,
            RefusalReason::Selection {
                issue: SelectionIssue::UnrequestedWordStart { word: 1 }
            }
        ),
        "R6_TRANSIENT_OTHER_START: {err:?}"
    );
}
#[test]
fn r6_transient_must_be_wholly_inside_the_cut() {
    assert!(
        validate(&fixture(20_000, None), 710_000, 1_900_000).is_err(),
        "R6_PARTIAL_TRANSIENT_REFUSED"
    );
}
#[test]
fn r6_transient_does_not_waive_a_plausible_kept_owner() {
    let err = validate(&fixture(10_000, Some(695_000)), 700_000, 1_900_000).unwrap_err();
    assert!(
        matches!(
            err.reason,
            RefusalReason::Selection {
                issue: SelectionIssue::UnrequestedSound { .. }
            }
        ),
        "R6_TRANSIENT_KEPT_OWNER: {err:?}"
    );
}
