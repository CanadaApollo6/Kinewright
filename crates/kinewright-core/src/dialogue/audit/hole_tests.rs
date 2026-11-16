#[test]
fn dialogue_audit_word_end_changes_do_not_change_measured_holes() {
    let check = |end| {
        one(
            recording(1_000_000, vec![word("long", 100_000, end)], |t| {
                if (100_000..900_000).contains(&t) {
                    hop(-2_000, true)
                } else {
                    hop(-8_000, false)
                }
            }),
            &[kept(1, 0, 1_000_000)],
        )
    };
    let short = check(180_000);
    let long = check(900_000);
    let counts = |r: &Report| (r.total.holes, r.total.sounding_holes);
    println!(
        "DQ_MEASURED_HOLES stub={:?} longer={:?}",
        counts(&short),
        counts(&long)
    );
    assert_eq!(counts(&short), counts(&long), "DQ_HOLES_IGNORE_WORD_ENDS");
    assert_eq!(counts(&short), (0, 0));
}

#[test]
fn dialogue_audit_holes_count_relative_voice_not_background_or_silence() {
    let cases = [
        ("silence", None, (0, 0)),
        ("loud_background", Some((-2_000, false)), (0, 0)),
        ("soft_untranscribed_voice", Some((-5_500, true)), (1, 1)),
        ("quiet_untranscribed_voice", Some((-8_000, true)), (1, 0)),
    ];
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    for (name, extra, want) in cases {
        let d = recording(3_000_000, vec![word("known", 100_000, 180_000)], |t| {
            if (100_000..800_000).contains(&t) {
                hop(-5_000, true)
            } else if (2_000_000..2_400_000).contains(&t) {
                extra.map_or_else(|| hop(-9_000, false), |(db, v)| hop(db, v))
            } else {
                hop(-9_000, false)
            }
        });
        let r = one(d, &[kept(1, 0, 3_000_000)]);
        let found = (r.total.holes, r.total.sounding_holes);
        println!("DQ_MEASURED_HOLES {name}={found:?}");
        actual.push(found);
        expected.push(want);
    }
    assert_eq!(actual, expected, "DQ_HOLES_MEASURE_VOICE");
}

#[test]
fn dialogue_audit_hole_onset_window_matches_its_reported_policy() {
    let d = recording(2_000_000, vec![word("known", 100_000, 180_000)], |t| {
        let voice = (100_000..900_000).contains(&t);
        hop(if voice { -2_000 } else { -8_000 }, voice)
    });
    let recordings = BTreeMap::from([(RecordingId(1), d)]);
    let ranges = [kept(1, 0, 2_000_000)];
    let settings = AuditSettings {
        word_start_tolerance_us: 200_000,
        ..AuditSettings::default()
    };
    let r = audit(&recordings, &ranges, &settings, comparison_key).unwrap();
    assert_eq!(
        r.hole_basis,
        HoleBasis::MeasuredVoiceWithoutNearbyWordStart {
            after_word_start_us: 200_000
        }
    );
    let holes: Vec<_> = r
        .items
        .iter()
        .filter_map(|i| if let Item::Hole(h) = i { Some(h) } else { None })
        .collect();
    assert_eq!((r.total.holes, r.total.sounding_holes), (1, 1));
    assert_eq!(holes[0].location.source_us, [300_000, 900_000]);
    assert_eq!(holes[0].sound_us, 600_000);
    // Cropping the source cannot turn the known word's tail into an omission.
    let cropped = audit(
        &recordings,
        &[kept(1, 500_000, 900_000)],
        &AuditSettings::default(),
        comparison_key,
    )
    .unwrap();
    assert_eq!((cropped.total.holes, cropped.total.sounding_holes), (0, 0));
    for tolerance in [0, 30_000_001] {
        assert_eq!(
            audit(
                &recordings,
                &ranges,
                &AuditSettings {
                    word_start_tolerance_us: tolerance,
                    ..settings
                },
                comparison_key
            ),
            Err(AuditError::Settings)
        );
    }
}
