fn r6_fixture(crossing: bool) -> (KinewrightMcp, Arc<BoundDialogue>) {
    let v = value(
        [("before", 200_000), ("um", 1_200_000), ("after", 2_500_000)]
            .into_iter()
            .map(|(text, start)| Word {
                text: text.into(),
                start_micros: start,
                end_micros: start + 80_000,
            })
            .collect(),
        4_000_000,
        |t| {
            let sounds = if crossing {
                vec![
                    (200_000, 400_000),
                    (1_100_000, 1_400_000),
                    (2_200_000, 2_300_000),
                    (3_000_000, 3_300_000),
                ]
            } else {
                vec![
                    (200_000, 400_000),
                    (1_100_000, 1_400_000),
                    (2_500_000, 2_800_000),
                ]
            };
            level(!sounds.into_iter().any(|(a, b)| a <= t && t < b))
        },
    );
    let fps = Rational::new(100, 1).unwrap();
    (service(document(fps, fps, 0, 400), Arc::clone(&v)), v)
}
fn r6_batch(v: &BoundDialogue, end: u64) -> Value {
    args(vec![
        json!({"kind":"words","first":address(v,1),"last":address(v,1),"overrule":[1_100_000,end]}),
        json!({"kind":"pause","after":address(v,1),"leave_micros":300_000}),
    ])
}
#[test]
fn dq_r6_removed_filler_anchor_reanchors_and_recalculates_retained_air() {
    let (s, v) = r6_fixture(false);
    let before = s.snapshot().unwrap().1;
    let result = cut(&s, r6_batch(&v, 1_500_000), true);
    ok(&result);
    let rows = &content(&result)["removals"];
    assert_eq!(
        rows[1]["outcome"], "ready",
        "R6_PAUSE_REANCHORED: {result:?}"
    );
    assert_eq!(rows[1]["reanchored_after"], json!({"from":1,"to":0}));
    assert_eq!(
        rows[1]["edges"][0][2],
        json!([1_680_000, 1]),
        "R6_PAUSE_AIR_RECALCULATED"
    );
    assert_eq!(s.snapshot().unwrap().1.duration, TimeCode(290));
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before, "R6_REANCHOR_SINGLE_UNDO");
}
#[test]
fn dq_r6_reanchor_never_crosses_an_unrequested_start() {
    let (s, v) = r6_fixture(true);
    let result = cut(&s, r6_batch(&v, 2_150_000), true);
    ok(&result);
    assert_eq!(content(&result)["removals"][0]["outcome"], "ready");
    assert_eq!(
        content(&result)["removals"][1]["violation"],
        "unrequested_word_start",
        "R6_REANCHOR_OTHER_START: {result:?}"
    );
}
#[test]
fn dq_r6_nonfiller_address_conflict_is_unchanged() {
    let (s, v) = r6_fixture(false);
    let request = args(vec![
        json!({"kind":"words","first":address(&v,0),"last":address(&v,1),"overrule":[0,1_500_000]}),
        json!({"kind":"pause","after":address(&v,1),"leave_micros":300_000}),
    ]);
    // The earlier word is not a filler: its removal must not gain the exception.
    let result = cut(&s, request, true);
    ok(&result);
    assert_eq!(
        content(&result)["removals"][1]["code"],
        "conflict",
        "R6_NONFILLER_BATCH_UNCHANGED"
    );
}

#[test]
fn dq_r6_missing_same_side_anchor_is_a_typed_refusal() {
    let (_, v) = r6_fixture(false);
    let fps = Rational::new(100, 1).unwrap();
    let s = service(document(fps, fps, 100, 400), Arc::clone(&v));
    let request = args(vec![
        json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[1_000_000,1_500_000]}),
        json!({"kind":"pause","after":address(&v,1),"leave_micros":300_000}),
    ]);
    let result = cut(&s, request, true);
    ok(&result);
    assert_eq!(
        content(&result)["removals"][1]["code"],
        "pause_reanchor_unavailable",
        "R6_REANCHOR_TYPED: {result:?}"
    );
    assert_eq!(
        content(&result)["removals"][1]["cause"],
        "no_same_side_word"
    );
}
