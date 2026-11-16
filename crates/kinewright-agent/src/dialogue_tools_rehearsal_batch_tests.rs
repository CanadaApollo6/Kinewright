#[test]
fn dq_rehearsal_batch_applies_ready_members_and_names_conflicts() {
    let (s, v) = fixture();
    let before = s.snapshot().unwrap().1;
    let bad = json!({"kind":"words","first":address(&v,99),"last":address(&v,99)});
    let result = cut(
        &s,
        args(vec![words(&v, 1), bad, words(&v, 1), words(&v, 3)]),
        true,
    );
    assert_eq!(
        content(&result)["status"],
        "applied",
        "DQ_REHEARSAL_PARTIAL_BATCH: {result:?}"
    );
    let outcomes = content(&result)["removals"].as_array().unwrap();
    assert_eq!(outcomes.len(), 4);
    for (index, row) in outcomes.iter().enumerate() {
        assert_eq!(row["index"], index);
    }
    assert_eq!(outcomes[0]["outcome"], "ready");
    assert_eq!(outcomes[1]["code"], "word");
    assert_eq!(outcomes[2]["code"], "conflict");
    assert_eq!(outcomes[2]["other_index"], 0);
    assert_eq!(outcomes[2]["cause"], "overlap");
    assert_eq!(outcomes[3]["outcome"], "ready");
    assert_ne!(s.snapshot().unwrap().1, before);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(
        s.snapshot().unwrap().1,
        before,
        "DQ_REHEARSAL_ONE_BATCH_UNDO"
    );
}
#[test]
fn dq_rehearsal_atomic_batch_keeps_the_old_all_or_nothing_option() {
    let (s, v) = fixture();
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let mut request = args(vec![words(&v, 1), words(&v, 99)]);
    request["atomic"] = json!(true);
    let result = cut(&s, request, false);
    assert_eq!(content(&result)["status"], "refused");
    assert_eq!(content(&result)["removals"][1]["code"], "word");
    assert!(
        content(&result)["reason"]["code"] == "word",
        "DQ_REHEARSAL_ATOMIC_BEFORE_CONSENT"
    );
    assert_eq!(s.snapshot().unwrap(), before);
    assert_eq!(s.core.export_history_identified().unwrap(), history);
}
#[test]
fn dq_rehearsal_batch_coordinates_do_not_depend_on_request_order() {
    let (forward, v) = fixture();
    let (backward, _) = fixture();
    let before = backward.snapshot().unwrap().1;
    let a = cut(&forward, args(vec![words(&v, 1), words(&v, 3)]), true);
    let b = cut(&backward, args(vec![words(&v, 3), words(&v, 1)]), true);
    ok(&a);
    ok(&b);
    assert_eq!(
        forward.snapshot().unwrap().1,
        backward.snapshot().unwrap().1,
        "DQ_REHEARSAL_ORIGINAL_COORDINATES"
    );
    backward.core.request(Command::Undo).unwrap();
    assert_eq!(backward.snapshot().unwrap().1, before);
}
#[test]
fn dq_rehearsal_batch_does_not_erase_a_prior_members_address() {
    let (s, v) = fixture();
    let pause = json!({"kind":"pause","after":address(&v,1),"leave_micros":100_000,"overrule":[1_600_000,1_800_000]});
    let word = json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[1_100_000,1_400_000]});
    let result = cut(&s, args(vec![pause, word]), true);
    ok(&result);
    assert_eq!(content(&result)["removals"][1]["code"], "conflict");
    assert_eq!(content(&result)["removals"][1]["other_index"], 0);
    assert_eq!(content(&result)["removals"][1]["cause"], "address_removed");
    assert_eq!(s.snapshot().unwrap().1.duration, TimeCode(380));
}
#[test]
fn dq_rehearsal_a_refused_first_member_does_not_choose_the_batch_track() {
    let v = ordinary();
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = document(fps, fps, 0, 400);
    doc.tracks[0].sync_lock = false;
    // The second track owns its independent editable clip state.
    let mut second = doc.tracks[0].clone();
    second.id = TrackId(2);
    second.clips[0].id = ClipId(2);
    doc.tracks.push(second);
    let s = service(doc, Arc::clone(&v));
    let before = s.snapshot().unwrap().1;
    let address = json!({"clip":2,"index":1,"binding":v.binding()});
    let r = cut(
        &s,
        args(vec![
            words(&v, 99),
            json!({"kind":"words","first":address,"last":address,"leave_micros":250_000}),
        ]),
        true,
    );
    ok(&r);
    assert_eq!(content(&r)["removals"][0]["code"], "word");
    assert_eq!(content(&r)["removals"][1]["outcome"], "ready");
    assert_eq!(s.snapshot().unwrap().1.tracks[0], before.tracks[0]);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before);
}
#[test]
fn dq_rehearsal_refused_measurement_is_not_a_commit_dependency() {
    struct Withdrawn {
        a: Arc<BoundDialogue>,
        b: Arc<BoundDialogue>,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl DialogueMeasurements for Withdrawn {
        fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
            Ok(0)
        }
        fn get(&self, id: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
            if id == self.a.identity() {
                return Ok(Arc::clone(&self.a));
            }
            if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                return Err(Refusal::NotMeasured);
            }
            Ok(Arc::clone(&self.b))
        }
    }
    let a = ordinary();
    let b = Arc::new(
        BoundDialogue::new(
            MediaIdentity {
                sha256: "b".repeat(64),
                byte_length: 100,
            },
            0,
            "literal-v1".into(),
            SoundProfile::from_hops(
                10_000,
                4_000_000,
                (0..400).map(|i| {
                    level(
                        ![
                            (200_000, 500_000),
                            (1_100_000, 1_400_000),
                            (2_000_000, 2_300_000),
                            (3_000_000, 3_300_000),
                        ]
                        .iter()
                        .any(|&(a, b)| a <= i * 10_000 && i * 10_000 < b),
                    )
                }),
            )
            .unwrap(),
            said(),
            Settings::default(),
        )
        .unwrap(),
    );
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = document(fps, fps, 0, 400);
    let mut extra = document(fps, fps, 0, 400);
    let mut asset = extra.media_pool.pop().unwrap();
    asset.id = AssetId(2);
    asset.source_fingerprint.content_sha256 = Some("b".repeat(64));
    doc.media_pool.push(asset);
    let mut clip = extra.tracks.pop().unwrap().clips.pop().unwrap();
    clip.id = ClipId(2);
    clip.asset = AssetId(2);
    clip.timeline_start = TimeCode(400);
    doc.tracks[0].clips.push(clip);
    doc.duration = TimeCode(800);
    let bad = json!({"clip":2,"index":99,"binding":b.binding()});
    let s = service(doc, Arc::clone(&a));
    *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Withdrawn {
        a: Arc::clone(&a),
        b,
        reads: std::sync::atomic::AtomicUsize::new(0),
    }));
    let r = cut(
        &s,
        args(vec![
            json!({"kind":"words","first":bad,"last":bad}),
            words(&a, 1),
        ]),
        true,
    );
    assert_eq!(
        content(&r)["status"],
        "applied",
        "DQ_REHEARSAL_REFUSED_NOT_DEPENDENCY: {r:?}"
    );
    assert_eq!(content(&r)["removals"][0]["code"], "word");
    assert_eq!(content(&r)["removals"][1]["outcome"], "ready");
}
#[test]
fn dq_rehearsal_batch_air_uses_the_kept_quiet_in_final_geometry() {
    let (s, v) = fixture();
    let before = s.snapshot().unwrap().1;
    let mut filler = words(&v, 1);
    filler.as_object_mut().unwrap().remove("leave_micros");
    let pause = json!({"kind":"pause","after":address(&v,0),"leave_micros":200_000});
    let r = cut(&s, args(vec![filler, pause]), true);
    ok(&r);
    for row in content(&r)["removals"].as_array().unwrap() {
        assert_eq!(row["outcome"], "ready");
        assert_eq!(
            row["kept_air_micros"],
            json!([200_000, 1]),
            "DQ_REHEARSAL_FINAL_KEPT_AIR: {r:?}"
        );
    }
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before);
}
