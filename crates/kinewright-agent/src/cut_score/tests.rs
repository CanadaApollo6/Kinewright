// Fixtures ported from Scarlet evals/test/metrics.test.ts and fixtures.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
use super::*;
use kinewright_core::{
    AssetId, Clip, ColorDescription, Document, MediaAsset, MediaKind, MediaSourceFingerprint,
    Rational, TimeCode, Track, TrackId, TrackKind,
};

fn range(start: u64, end: u64) -> SourceRange {
    SourceRange {
        start_micros: start,
        end_micros: end,
    }
}
fn word(text: &str, start: u64, end: u64) -> Word {
    Word {
        text: text.into(),
        start_micros: start,
        end_micros: end,
    }
}
fn candidate(kept: Vec<SourceRange>) -> Candidate {
    Candidate {
        recording_sha256: "a".repeat(64),
        kept,
    }
}
fn fixture() -> Case {
    let mut words = Vec::new();
    for n in 0..10 {
        words.push(word(
            "Line",
            2 * n * 1_000_000 + 500_000,
            (2 * n + 1) * 1_000_000,
        ));
        words.push(word(
            &format!("{n}."),
            (2 * n + 1) * 1_000_000,
            2 * n * 1_000_000 + 1_500_000,
        ));
    }
    Case {
        recording_sha256: "a".repeat(64),
        duration_micros: 20_000_000,
        request: "Remove lines 2 and 5.".into(),
        words,
        reference_kept: vec![
            range(0, 4_000_000),
            range(6_000_000, 10_000_000),
            range(12_000_000, 20_000_000),
        ],
        required_lines: None,
    }
}
fn measured(case: &Case, kept: Vec<SourceRange>) -> Score {
    score(case, &candidate(kept), &ScoreSettings::default()).unwrap()
}
fn asymmetric() -> Score {
    measured(
        &fixture(),
        vec![
            range(0, 4_000_000),
            range(6_000_000, 10_010_000),
            range(12_050_000, 14_000_000),
            range(16_000_000, 20_000_000),
        ],
    )
}

#[test]
fn cut_score_ported_reference_words_time_edges_and_required_line() {
    // Scarlet: "measures removals, edge placement and lost required lines against an editor cut".
    let s = asymmetric();
    assert_eq!(
        (
            s.removed_words.true_positive,
            s.removed_words.false_positive,
            s.removed_words.false_negative
        ),
        (4, 2, 0)
    );
    assert_eq!(s.removed_words.precision.display.as_deref(), Some("0.667"));
    assert_eq!(s.removed_words.recall.display.as_deref(), Some("1.000"));
    assert_eq!(s.removed_words.f1.display.as_deref(), Some("0.800"));
    assert_eq!(
        (
            s.candidate_removed_micros,
            s.reference_removed_micros,
            s.overlap_removed_micros
        ),
        (6_040_000, 4_000_000, 3_990_000)
    );
    assert_eq!(s.removed_time.recall.display.as_deref(), Some("0.998"));
    assert_eq!(
        (
            s.edges.candidate.len(),
            s.edges.reference.len(),
            s.edges.matches.len()
        ),
        (6, 4, 4)
    );
    assert_eq!(
        s.edges
            .within
            .iter()
            .map(|t| t.reference_share.numerator)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!(
        s.edges.median_abs_error_micros.as_ref().unwrap(),
        &Ratio::new(10_000, 2)
    );
    assert_eq!(
        s.edges.p90_abs_error_micros.as_ref().unwrap(),
        &Ratio::new(380_000, 10)
    );
    assert_eq!(s.required_lines.required, 8);
    assert_eq!(s.required_lines.missing.len(), 1);
    assert_eq!(s.required_lines.missing[0].text, "Line 7.");
    assert_eq!(s.required_lines.missing_micros, 1_000_000);
    assert_eq!(s.time_to_first_change_micros, None);
}

#[test]
fn cut_score_precision_recall_are_not_swapped() {
    let s = asymmetric();
    assert_eq!(
        (
            s.removed_words.precision.numerator,
            s.removed_words.precision.denominator,
            s.removed_words.recall.numerator,
            s.removed_words.recall.denominator
        ),
        (4, 6, 4, 4),
        "DQ4_PRECISION_RECALL_NOT_SWAPPED"
    );
}

#[test]
fn cut_score_identical_cut_is_perfect_including_harmless_splits() {
    // Scarlet: "an identical cut scores perfectly".
    let case = fixture();
    let s = measured(
        &case,
        vec![
            range(0, 2_000_000),
            range(2_000_000, 4_000_000),
            range(6_000_000, 10_000_000),
            range(12_000_000, 20_000_000),
        ],
    );
    assert!(s.reference_equivalent);
    for ratio in [
        &s.removed_words.f1,
        &s.removed_time.f1,
        &s.edges.within[0].reference_share,
    ] {
        assert_eq!(ratio.display.as_deref(), Some("1.000"));
    }
    assert!(s.required_lines.missing.is_empty());
}

#[test]
fn cut_score_nothing_and_everything_removed_are_defined_honestly() {
    // Scarlet: "handles empty removals".
    let case = fixture();
    let none = measured(&case, vec![range(0, case.duration_micros)]);
    assert_eq!(
        (
            none.removed_words.true_positive,
            none.removed_words.false_positive,
            none.removed_words.false_negative
        ),
        (0, 0, 4)
    );
    assert_eq!(none.removed_words.precision.display, None);
    assert_eq!(none.removed_words.recall.display.as_deref(), Some("0.000"));
    let all = measured(&case, vec![]);
    assert_eq!(
        (
            all.removed_words.true_positive,
            all.removed_words.false_positive,
            all.removed_words.false_negative
        ),
        (4, 16, 0)
    );
    assert_eq!(all.candidate_removed_micros, 20_000_000);
    assert_eq!(all.required_lines.missing.len(), 8);
    assert_eq!(all.pacing.played_words, 0);
    let mut case = fixture();
    case.reference_kept = vec![range(0, case.duration_micros)];
    let unchanged = measured(&case, vec![range(0, case.duration_micros)]);
    assert!(unchanged.reference_equivalent);
    assert_eq!(unchanged.removed_words.f1.display, None);
    assert_eq!(Ratio::new(1, 2).display.as_deref(), Some("0.500"));
    assert_eq!(Ratio::new(1, 6).display.as_deref(), Some("0.167"));
}

#[test]
fn cut_score_edge_tolerances_distinguish_19_21_99_101_ms() {
    let mut case = fixture();
    case.reference_kept = vec![range(0, 4_000_000)];
    for (error, at20, at100) in [
        (19_000, 1, 1),
        (21_000, 0, 1),
        (99_000, 0, 1),
        (101_000, 0, 0),
        (1_000_001, 0, 0),
    ] {
        let s = measured(&case, vec![range(0, 4_000_000 + error)]);
        assert_eq!(s.edges.within[0].reference_share.numerator, at20);
        assert_eq!(s.edges.within[1].reference_share.numerator, at100);
        assert_eq!(s.edges.matches.len(), usize::from(error <= 1_000_000));
    }
}

#[test]
fn cut_score_single_reference_chooses_nearest_on_same_side() {
    // Scarlet: "matches each edge once, nearest first, on the same side".
    use super::metrics::{Edge, EdgeSide, match_edges};
    let c = [
        Edge {
            at_micros: 1_000_000,
            side: EdgeSide::Out,
        },
        Edge {
            at_micros: 1_050_000,
            side: EdgeSide::Out,
        },
    ];
    let r = [
        Edge {
            at_micros: 1_040_000,
            side: EdgeSide::Out,
        },
        Edge {
            at_micros: 1_040_000,
            side: EdgeSide::In,
        },
    ];
    let m = match_edges(&c, &r, 100_000);
    assert_eq!(m.len(), 1);
    assert_eq!(
        (
            m[0].candidate_index,
            m[0].reference_index,
            m[0].error_micros
        ),
        (1, 0, 10_000)
    );
}

#[test]
fn cut_score_source_coverage_does_not_hide_playback_duplicates() {
    let mut case = fixture();
    case.reference_kept = vec![range(0, 20_000_000)];
    let s = measured(&case, vec![range(0, 20_000_000), range(0, 20_000_000)]);
    assert_eq!(s.candidate_removed_micros, 0);
    assert!(!s.reference_equivalent);
    assert_eq!(s.pacing.played_words, 40);
    assert_eq!(s.edges.candidate.len(), 2);
}

#[test]
fn cut_score_interval_fixture_and_half_word_boundary() {
    // Scarlet: "merges, subtracts and intersects" / labelWords's half-word rule.
    assert_eq!(
        intervals::merge(&[range(3, 4), range(0, 1), range(1, 2)]),
        vec![range(0, 2), range(3, 4)]
    );
    assert_eq!(
        intervals::removed(&[range(2, 3), range(5, 6)], 10),
        vec![range(0, 2), range(3, 5), range(6, 10)]
    );
    assert_eq!(intervals::intersection(&[range(0, 5)], &[range(4, 8)]), 1);
    assert!(intervals::kept_word(0, 10, &[range(5, 10)]));
    assert!(!intervals::kept_word(0, 10, &[range(6, 10)]));
    assert_eq!(intervals::merge(&[range(0, 1), range(2, 3)]).len(), 2); // no float epsilon merges a 1-us gap
}

#[test]
fn cut_score_sentences_and_adjustable_required_shares() {
    // Scarlet: "splits sentences at punctuation and long gaps".
    let mut case = fixture();
    case.words = vec![
        word("One", 0, 300_000),
        word("two.", 300_000, 600_000),
        word("Three", 700_000, 1_000_000),
        word("four", 3_000_000, 3_400_000),
    ];
    case.reference_kept = vec![range(0, 20_000_000)];
    let s = measured(&case, vec![]);
    assert_eq!(
        s.required_lines
            .missing
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        vec!["One two.", "Three", "four"]
    );
    case.required_lines = Some(vec![RequiredLine {
        from_word: 0,
        to_word: 1,
    }]);
    case.reference_kept.clear(); // Explicit required lines do not depend on reference omissions.
    assert_eq!(measured(&case, vec![]).required_lines.required, 1);
    let settings = ScoreSettings {
        missing_cut_share: Share {
            numerator: 3,
            denominator: 4,
        },
        ..ScoreSettings::default()
    };
    let c = candidate(vec![range(300_000, 600_000)]);
    assert!(
        score(&case, &c, &settings)
            .unwrap()
            .required_lines
            .missing
            .is_empty()
    );
}

#[test]
fn cut_score_pacing_gap_fixture() {
    // Scarlet: "counts gaps inside kept footage and across joins".
    let words = vec![
        word("So", 0, 300_000),
        word("here", 400_000, 700_000),
        word("we", 1_450_000, 1_700_000),
        word("go.", 1_800_000, 2_000_000),
        word("[BLANK_AUDIO]", 2_100_000, 2_500_000),
        word("Next", 3_650_000, 4_000_000),
        word("up", 4_100_000, 4_300_000),
        word("this", 5_550_000, 5_900_000),
    ];
    let p = pacing::measure(
        &words,
        &[range(0, 2_600_000), range(3_600_000, 6_000_000)],
        &ScoreSettings::default(),
    )
    .unwrap();
    assert_eq!(
        (
            p.played_words,
            p.joins,
            p.gaps_inside,
            p.gaps_at_joins,
            p.long_gaps,
            p.excess_gap_micros
        ),
        (7, 1, 2, 1, 1, 1_900_000)
    );
    assert_eq!(p.median_join_gap_micros, Some(Ratio::new(650_000, 1)));
    assert_eq!(
        p.gaps.iter().map(|g| g.duration_micros).collect::<Vec<_>>(),
        vec![750_000, 650_000, 1_250_000]
    );
}

#[test]
fn cut_score_played_words_follow_piece_and_overlap_semantics() {
    // Scarlet: "places a version's words on its timeline, one piece per unbroken run".
    let case = fixture();
    let s = measured(
        &case,
        vec![range(0, 4_000_000), range(6_000_000, 20_000_000)],
    );
    assert_eq!(s.pacing.played_words, 18);
    assert_eq!(s.pacing.joins, 1);
    let mut case = fixture();
    case.words = vec![word("crosses", 900_000, 1_100_000)];
    let unsplit = measured(&case, vec![range(0, 2_000_000)]);
    let split = measured(
        &case,
        vec![range(0, 1_000_000), range(1_000_000, 2_000_000)],
    );
    assert_eq!(unsplit.pacing, split.pacing);
}

#[test]
fn cut_score_pacing_repeated_word_and_phrase_fixture() {
    // Scarlet: "finds a word or a run of words said twice in a row, and the ones a join made".
    let said = [
        "Now", "now", "that", "there", "are,", "that", "there", "are", "very,", "very", "good",
        "it.", "It", "works", "so", "…", "so",
    ];
    let mut words = Vec::new();
    let mut at = 0;
    for text in said {
        if text == "…" {
            at += 4_000_000;
            continue;
        }
        words.push(word(text, at, at + 200_000));
        at += 300_000;
    }
    // The first source jump makes the "Now now" join without changing played word spacing.
    words[0].start_micros = 0;
    let p = pacing::measure(
        &words,
        &[range(0, 250_000), range(300_000, at)],
        &ScoreSettings::default(),
    )
    .unwrap();
    assert_eq!(
        p.repeats
            .iter()
            .map(|r| (r.text.as_str(), r.words, r.join))
            .collect::<Vec<_>>(),
        vec![
            ("Now now", 1, true),
            ("that there are, that there are", 3, false),
            ("very, very", 1, false)
        ]
    );
    assert_eq!(
        (
            p.repeated_single_words,
            p.repeated_phrases,
            p.repeats_at_joins
        ),
        (2, 1, 1)
    );
}

#[test]
fn cut_score_whole_repeated_opening_and_optional_phrase_bound() {
    // Scarlet: "counts an entire repeated opening with a two-word name and distinguishes the clean take".
    let said = [
        "Our", "next", "guest", "is", "Robin", "Green", "Our", "next", "guest", "is", "Robin",
        "Green", "from", "Boston.",
    ];
    let words: Vec<_> = said
        .iter()
        .zip(0_u64..)
        .map(|(s, i)| word(s, i * 300_000, i * 300_000 + 200_000))
        .collect();
    let all = [range(0, 4_200_000)];
    let p = pacing::measure(&words, &all, &ScoreSettings::default()).unwrap();
    assert_eq!(p.repeats.len(), 1);
    assert_eq!(p.repeats[0].words, 6);
    assert!(
        pacing::measure(
            &words,
            &[range(1_800_000, 4_200_000)],
            &ScoreSettings::default()
        )
        .unwrap()
        .repeats
        .is_empty()
    );
    assert!(
        pacing::measure(
            &words,
            &all,
            &ScoreSettings {
                repeat_words: Some(5),
                ..ScoreSettings::default()
            }
        )
        .unwrap()
        .repeats
        .is_empty()
    );
}

fn document() -> Document {
    let mut d = Document::default();
    d.media_pool.push(MediaAsset {
        id: AssetId(1),
        path: "recording.wav".into(),
        name: "recording".into(),
        duration: TimeCode(600),
        fps: Rational::default(),
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(100),
        },
        color_description: ColorDescription::default(),
        assumed_from: None,
    });
    let clip:Clip=serde_json::from_value(serde_json::json!({"id":1,"asset":1,"source_range":{"start":0,"end":600},"timeline_start":0,"effects":[],"transition_in":null})).unwrap();
    d.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![clip],
    });
    d.duration = TimeCode(600);
    d
}

#[test]
fn cut_score_document_identity_playback_order_and_speed_refusal() {
    let mut d = document();
    let c = candidate_from_document(&d, AssetId(1), TrackId(1)).unwrap();
    assert_eq!(c, candidate(vec![range(0, 20_000_000)]));
    d.tracks[0].clips[0].speed_percent = 200;
    d.duration = TimeCode(300);
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::Speed { clip: 1 })
    );
}

#[test]
fn cut_score_document_refuses_unverified_and_unrepresentable_shapes() {
    let mut d = document();
    d.media_pool[0].source_fingerprint.content_sha256 = None;
    d.media_pool[0].source_fingerprint.byte_len = None;
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::UnverifiedSource)
    );
    let mut d = document();
    d.tracks[0].clips[0].timeline_start = TimeCode(1);
    d.duration = TimeCode(601);
    assert!(matches!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape { .. })
    ));
    let mut d = document();
    d.tracks[0].clips[0].enabled = false;
    assert!(matches!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape { .. })
    ));
}

#[test]
fn cut_score_document_preserves_source_occurrence_order_and_exact_grid() {
    let mut d = document();
    d.tracks[0].clips[0].source_range = TimeCode(300)..TimeCode(600);
    let second: Clip = serde_json::from_value(serde_json::json!({
        "id":2,"asset":1,"source_range":{"start":0,"end":300},
        "timeline_start":300,"effects":[],"transition_in":null
    }))
    .unwrap();
    d.tracks[0].clips.push(second);
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1))
            .unwrap()
            .kept,
        vec![range(10_000_000, 20_000_000), range(0, 10_000_000)]
    );
    let mut d = document();
    d.fps = Rational::new(24, 1).unwrap();
    d.tracks[0].clips[0].source_range.end = TimeCode(1);
    d.duration = TimeCode(1);
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape {
            detail: "source span is not exact on the project's frame grid"
        })
    );
}

#[test]
fn cut_score_refuses_overflowing_playback_without_allocating_a_trace() {
    let mut case = fixture();
    case.duration_micros = u64::MAX;
    let c = candidate(vec![range(0, u64::MAX), range(0, 1)]);
    assert_eq!(
        score(&case, &c, &ScoreSettings::default()),
        Err(CannotScore::TimeOverflow)
    );
}

#[test]
fn cut_score_document_refuses_a_span_below_the_integer_microsecond_grid() {
    let mut d = document();
    d.fps = Rational::new(3_000_000, 1).unwrap();
    d.media_pool[0].fps = d.fps;
    d.tracks[0].clips[0].source_range.end = TimeCode(1);
    d.duration = TimeCode(1);
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::Range {
            input: "document",
            index: 0
        })
    );
}

#[test]
fn cut_score_validates_all_inputs_and_never_silently_clips_ranges() {
    let case = fixture();
    let mut c = candidate(vec![range(0, 20_000_001)]);
    assert_eq!(
        score(&case, &c, &ScoreSettings::default()),
        Err(CannotScore::Range {
            input: "candidate",
            index: 0
        })
    );
    c.kept = vec![range(1, 1)];
    assert!(matches!(
        score(&case, &c, &ScoreSettings::default()),
        Err(CannotScore::Range { .. })
    ));
    c.kept.clear();
    c.recording_sha256 = "b".repeat(64);
    assert_eq!(
        score(&case, &c, &ScoreSettings::default()),
        Err(CannotScore::RecordingMismatch)
    );
    c.recording_sha256 = "A".repeat(64);
    assert_eq!(
        score(&case, &c, &ScoreSettings::default()),
        Err(CannotScore::InvalidHash)
    );
    let mut case = fixture();
    case.words[0].end_micros = case.words[0].start_micros;
    assert_eq!(
        score(&case, &candidate(vec![]), &ScoreSettings::default()),
        Err(CannotScore::Word { index: 0 })
    );
    let mut case = fixture();
    case.required_lines = Some(vec![RequiredLine {
        from_word: 0,
        to_word: 20,
    }]);
    assert_eq!(
        score(&case, &candidate(vec![]), &ScoreSettings::default()),
        Err(CannotScore::RequiredLine { index: 0 })
    );
    let mut settings = ScoreSettings::default();
    settings.required_keep_share.denominator = 0;
    assert_eq!(
        score(&fixture(), &candidate(vec![]), &settings),
        Err(CannotScore::Settings)
    );
}

#[test]
fn cut_score_json_is_strict_and_reports_integer_exact_pairs() {
    let encoded = serde_json::to_value(fixture()).unwrap();
    let mut bad = encoded;
    bad["extra"] = true.into();
    assert!(serde_json::from_value::<Case>(bad).is_err());
    let encoded = serde_json::to_value(asymmetric()).unwrap();
    assert_eq!(encoded["removed_words"]["precision"]["numerator"], 4);
    assert_eq!(encoded["removed_words"]["precision"]["denominator"], 6);
    assert_eq!(encoded["removed_words"]["precision"]["display"], "0.667");
    assert!(encoded["time_to_first_change_micros"].is_null());
}

#[test]
fn cut_score_competing_neighbors_maximize_matches_before_error() {
    // Astra: the closer candidate used to lose a match to the greedy choice.
    let mut case = fixture();
    case.duration_micros = 2_000_000;
    case.words = vec![
        word("finish", 800_000, 1_000_000),
        word("it", 1_110_000, 1_150_000),
        word("now", 1_400_000, 1_800_000),
    ];
    case.reference_kept = vec![
        range(0, 1_000_000),
        range(1_110_000, 1_150_000),
        range(1_400_000, 2_000_000),
    ];
    let a = measured(
        &case,
        vec![
            range(0, 1_090_000),
            range(1_110_000, 1_240_000),
            range(1_400_000, 2_000_000),
        ],
    );
    let b = measured(
        &case,
        vec![
            range(0, 905_000),
            range(1_110_000, 1_245_000),
            range(1_400_000, 2_000_000),
        ],
    );
    assert_eq!(
        a.edges.within[1].reference_share.numerator, 4,
        "DQ4_EDGE_ASSIGNMENT_MAX_CARDINALITY"
    );
    assert_eq!(b.edges.within[1].reference_share.numerator, 4);
    assert_eq!(a.edges.matches.len(), 4);
    assert_eq!(
        a.edges
            .p90_abs_error_micros
            .as_ref()
            .unwrap()
            .display
            .as_deref(),
        Some("90000.000")
    );
    assert_eq!(
        b.edges
            .p90_abs_error_micros
            .as_ref()
            .unwrap()
            .display
            .as_deref(),
        Some("95000.000")
    );
    for r in [&a, &b] {
        assert!(
            r.edges
                .within
                .windows(2)
                .all(|w| w[0].removal.true_positive <= w[1].removal.true_positive)
        );
        let t = &r.edges.within[1];
        assert_eq!(
            (
                t.removal.true_positive,
                t.removal.false_positive,
                t.removal.false_negative
            ),
            (4, 0, 0)
        );
    }
}

#[test]
fn cut_score_tolerance_boundaries_include_20_100_and_1000_ms() {
    let mut case = fixture();
    case.reference_kept = vec![range(0, 4_000_000)];
    for (offset, n20, n100) in [(20_000, 1, 1), (100_000, 0, 1), (1_000_000, 0, 0)] {
        let s = measured(&case, vec![range(0, 4_000_000 + offset)]);
        assert_eq!(
            (
                s.edges.within[0].reference_share.numerator,
                s.edges.within[1].reference_share.numerator
            ),
            (n20, n100),
            "DQ4_TOLERANCES_ARE_INCLUSIVE"
        );
        assert_eq!(s.edges.matches.len(), 1, "DQ4_WIDEST_WINDOW_IS_INCLUSIVE");
    }
    let s = asymmetric();
    assert_eq!(
        (
            s.edges.within[0].removal.true_positive,
            s.edges.within[0].removal.false_positive,
            s.edges.within[0].removal.false_negative
        ),
        (3, 3, 1)
    );
    assert_eq!(
        (
            s.edges.within[1].removal.true_positive,
            s.edges.within[1].removal.false_positive,
            s.edges.within[1].removal.false_negative
        ),
        (4, 2, 0)
    );
}

#[test]
fn cut_score_half_word_occurrences_agree_with_pacing() {
    let mut case = fixture();
    case.words = vec![word("hello", 0, 1_000_000)];
    case.reference_kept = vec![range(0, 1_000_000)];
    for kept in [
        vec![range(0, 500_000)],
        vec![range(500_000, 1_000_000)],
        vec![range(0, 250_000), range(250_000, 500_000)],
    ] {
        let s = measured(&case, kept);
        assert_eq!(s.removed_words.false_positive, 0);
        assert_eq!(s.pacing.played_words, 1);
    }
    for kept in [
        vec![range(450_000, 550_000)],
        vec![range(0, 250_000), range(750_000, 1_000_000)],
        vec![range(450_000, 550_000), range(450_000, 550_000)],
    ] {
        let s = measured(&case, kept);
        assert_eq!(s.removed_words.false_positive, 1);
        assert_eq!(s.pacing.played_words, 0);
    }
    let repeated = measured(&case, vec![range(0, 500_000), range(0, 500_000)]);
    assert_eq!(repeated.pacing.played_words, 2);
    assert_eq!(repeated.removed_words.false_positive, 0);
}

#[test]
fn cut_score_only_selected_track_mix_blocks_and_generated_content_refuses() {
    use kinewright_core::{AudioBus, AudioBusId, ClipContent, SolidColor, TrackMix};
    let mut d = document();
    d.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![],
    });
    d.audio_mix.buses.push(AudioBus {
        id: AudioBusId(1),
        name: "music".into(),
        tracks: vec![TrackId(2)],
        gain_tenth_db: -30,
        gain_curve: None,
        effects: vec![],
        ducking_sidechain_tracks: vec![],
    });
    assert!(candidate_from_document(&d, AssetId(1), TrackId(1)).is_ok());
    d.audio_mix.tracks.push(TrackMix {
        gain_tenth_db: -20,
        ..TrackMix::neutral(TrackId(2))
    });
    assert!(candidate_from_document(&d, AssetId(1), TrackId(1)).is_ok());
    d.audio_mix.tracks.insert(
        0,
        TrackMix {
            gain_tenth_db: -20,
            ..TrackMix::neutral(TrackId(1))
        },
    );
    assert!(matches!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape { .. })
    ));
    d.audio_mix.tracks.remove(0);
    d.audio_mix.tracks[0].solo = true;
    assert!(matches!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape { .. })
    ));
    d.audio_mix.tracks[0].solo = false;
    d.audio_mix.buses[0].tracks = vec![TrackId(1)];
    assert!(matches!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape { .. })
    ));
    let mut d = document();
    d.tracks[0].kind = TrackKind::Video;
    d.tracks[0].clips[0].content = ClipContent::Solid(SolidColor::default());
    assert_eq!(
        candidate_from_document(&d, AssetId(1), TrackId(1)),
        Err(CannotScore::TrackShape {
            detail: "dialogue track contains another source or generated content"
        })
    );
}

#[test]
fn cut_score_sentence_closers_keep_quoted_lines_separate() {
    let mut case = fixture();
    case.words = vec![
        word("One.\"", 0, 100_000),
        word("Two?]", 200_000, 300_000),
        word("Three!)", 400_000, 500_000),
    ];
    case.reference_kept = vec![range(0, 1_000_000)];
    let s = measured(&case, vec![]);
    assert_eq!(s.required_lines.required, 3);
    assert_eq!(s.required_lines.missing.len(), 3);
    assert_eq!(s.required_lines.missing[0].text, "One.\"");
}

#[test]
fn cut_score_ordered_assignment_agrees_with_exhaustive_small_cases() {
    use super::metrics::{Edge, EdgeSide, match_edges};
    fn brute(c: &[u64], r: &[u64], window: u64) -> (usize, u128) {
        if c.is_empty() || r.is_empty() {
            return (0, 0);
        }
        let a = brute(&c[1..], r, window);
        let b = brute(c, &r[1..], window);
        let better = |a: (usize, u128), b: (usize, u128)| {
            if a.0 > b.0 || a.0 == b.0 && a.1 < b.1 {
                a
            } else {
                b
            }
        };
        let mut best = better(a, b);
        if c[0].abs_diff(r[0]) <= window {
            let tail = brute(&c[1..], &r[1..], window);
            best = better(best, (tail.0 + 1, tail.1 + u128::from(c[0].abs_diff(r[0]))));
        }
        best
    }
    for seed in 0..128_u64 {
        let mut c = vec![
            (seed % 7) * 100_000,
            ((seed / 3) % 7) * 100_000,
            ((seed / 11) % 7) * 100_000,
        ];
        let mut r = vec![
            ((seed / 5) % 7) * 100_000,
            ((seed / 13) % 7) * 100_000,
            ((seed / 17) % 7) * 100_000,
        ];
        let edges = |v: &[u64]| {
            v.iter()
                .map(|&t| Edge {
                    at_micros: t,
                    side: EdgeSide::Out,
                })
                .collect::<Vec<_>>()
        };
        for window in [0, 100_000, 300_000] {
            let matches = match_edges(&edges(&c), &edges(&r), window);
            let actual = (
                matches.len(),
                matches
                    .iter()
                    .map(|m| m.error_micros.unsigned_abs())
                    .sum::<u128>(),
            );
            c.sort_unstable();
            r.sort_unstable();
            assert_eq!(actual, brute(&c, &r, window));
            assert_eq!(
                super::matching::match_count(&edges(&c), &edges(&r), window),
                actual.0
            );
        }
    }
}
