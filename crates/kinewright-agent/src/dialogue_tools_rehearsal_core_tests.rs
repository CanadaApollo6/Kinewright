#[test]
fn dq_rehearsal_transcript_reports_recording_levels_once() {
    let (s, v) = fixture();
    s.core
        .request(Command::Do(Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(200),
        }))
        .unwrap();
    let result = invoke(&s, "get_timeline_transcript", json!({}));
    ok(&result);
    let levels = content(&result)["recording_levels"].as_object().unwrap();
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[v.binding()], json!([-1800, -6000]));
    assert_eq!(content(&result)["segments"].as_array().unwrap().len(), 2);
    assert_eq!(
        content(&result)["recording_level_columns"],
        json!(["speech_rms_cdb", "noise_floor_cdb"])
    );
}
#[test]
fn dq_rehearsal_word_cut_reports_the_air_that_is_heard() {
    let (s, v) = fixture();
    let mut natural = words(&v, 1);
    natural.as_object_mut().unwrap().remove("leave_micros");
    let result = cut(&s, args(vec![natural]), true);
    ok(&result);
    assert_eq!(
        content(&result)["removals"][0]["kept_air_micros"],
        json!([600_000, 1])
    );
    let (s, v) = fixture();
    let mut explicit = words(&v, 1);
    explicit["leave_micros"] = json!(300_000);
    let result = cut(&s, args(vec![explicit]), true);
    ok(&result);
    assert_eq!(
        content(&result)["removals"][0]["kept_air_micros"],
        json!([300_000, 1])
    );
}
#[test]
fn dq_rehearsal_pause_reply_lists_the_other_measured_pause() {
    let v = value(said(), 4_000_000, |t| {
        level(
            ![
                (200_000, 500_000),
                (1_100_000, 1_400_000),
                (1_600_000, 1_700_000),
                (2_000_000, 2_300_000),
                (3_000_000, 3_300_000),
            ]
            .iter()
            .any(|&(a, b)| a <= t && t < b),
        )
    });
    let s = service(
        document(
            Rational::new(100, 1).unwrap(),
            Rational::new(100, 1).unwrap(),
            0,
            400,
        ),
        Arc::clone(&v),
    );
    let before = s.snapshot().unwrap().1;
    let result = cut(
        &s,
        args(vec![
            json!({"kind":"pause","after":address(&v,1),"leave_micros":200_000}),
        ]),
        true,
    );
    ok(&result);
    assert_eq!(
        content(&result)["removals"][0]["other_pauses_micros"],
        json!([200_000])
    );
    assert_eq!(
        content(&result)["removals"][0]["kept_air_micros"],
        json!([200_000, 1])
    );
    let after_first = s.snapshot().unwrap().1;
    let left = after_first.tracks[0].clips[0].id;
    let next = cut(
        &s,
        json!({"expected_revision":content(&result)["revision"],"removals":[{"kind":"pause","after":{"clip":left.0,"index":1,"binding":v.binding()},"leave_micros":100_000}]}),
        true,
    );
    assert_eq!(
        next.is_error,
        Some(false),
        "DQ_REHEARSAL_NEXT_PAUSE: {next:?}"
    );
    assert_eq!(
        content(&next)["removals"][0]["kept_air_micros"],
        json!([100_000, 1])
    );
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, after_first);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before);
}
#[test]
fn dq_rehearsal_noisy_measurement_has_a_typed_refusal_with_both_levels() {
    let profile = SoundProfile::from_hops(
        10_000,
        1_000_000,
        (0..100).map(|i| {
            let voice = i < 50;
            let db = if voice { -2000 } else { -4200 };
            Hop {
                rms_dbfs_hundredths: db,
                peak_dbfs_hundredths: db,
                voice,
            }
        }),
    )
    .unwrap();
    let result = BoundDialogue::new(
        MediaIdentity {
            sha256: "a".repeat(64),
            byte_length: 100,
        },
        0,
        "literal-v1".into(),
        profile,
        vec![],
        Settings::default(),
    );
    let Err(reason) = result else {
        panic!("DQ_REHEARSAL_TYPED_NOISE");
    };
    assert_eq!(
        serde_json::to_value(reason).unwrap(),
        json!({"code":"noise_separation","speech_rms_hundredths":-2000,"noise_floor_hundredths":-4200,"quiet_rms_hundredths":-4001})
    );
}

#[test]
fn dq_rehearsal_first_visible_word_reaches_the_clip_boundary() {
    for (leave, end, air) in [(None, 190, 100_000), (Some(200_000), 180, 200_000)] {
        let v = ordinary();
        let fps = Rational::new(100, 1).unwrap();
        let s = service(document(fps, fps, 80, 270), Arc::clone(&v));
        let before = s.snapshot().unwrap().1;
        let r = cut(
            &s,
            args(vec![
                json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"leave_micros":leave}),
            ]),
            true,
        );
        ok(&r);
        let after = s.snapshot().unwrap().1;
        assert_eq!(after.tracks[0].clips.len(), 1);
        assert_eq!(
            after.tracks[0].clips[0].source_range,
            TimeCode(end)..TimeCode(270)
        );
        assert_eq!(after.tracks[0].clips[0].timeline_start, TimeCode(0));
        assert_eq!(
            content(&r)["removals"][0]["kept_air_micros"],
            json!([air, 1])
        );
        s.core.request(Command::Undo).unwrap();
        assert_eq!(s.snapshot().unwrap().1, before);
    }
}
#[test]
fn dq_rehearsal_last_visible_word_reaches_the_clip_boundary() {
    for (leave, start, air) in [(None, 155, 150_000), (Some(200_000), 160, 200_000)] {
        let v = ordinary();
        let fps = Rational::new(100, 1).unwrap();
        let s = service(document(fps, fps, 80, 270), Arc::clone(&v));
        let before = s.snapshot().unwrap().1;
        let r = cut(
            &s,
            args(vec![
                json!({"kind":"words","first":address(&v,2),"last":address(&v,2),"leave_micros":leave}),
            ]),
            true,
        );
        ok(&r);
        let after = s.snapshot().unwrap().1;
        assert_eq!(after.tracks[0].clips.len(), 1);
        assert_eq!(
            after.tracks[0].clips[0].source_range,
            TimeCode(80)..TimeCode(start)
        );
        assert_eq!(
            content(&r)["removals"][0]["kept_air_micros"],
            json!([air, 1])
        );
        s.core.request(Command::Undo).unwrap();
        assert_eq!(s.snapshot().unwrap().1, before);
    }
}
