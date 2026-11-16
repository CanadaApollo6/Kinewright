#[test]
fn dq_rehearsal_transcript_word_range_preserves_addresses() {
    let (s, v) = fixture();
    let full = invoke(&s, "get_timeline_transcript", json!({}));
    ok(&full);
    let part = invoke(
        &s,
        "get_timeline_transcript",
        json!({"word_range":{"clip":1,"first":1,"end":3}}),
    );
    ok(&part);
    let rows = content(&part)["segments"].as_array().unwrap();
    let count: usize = rows
        .iter()
        .map(|row| row["words"].as_array().unwrap().len())
        .sum();
    assert_eq!(count, 2, "DQ_REHEARSAL_TRANSCRIPT_RANGE");
    assert_eq!(rows[0]["first"], 1);
    assert_eq!(rows[0]["words"][0][0], "remove");
    assert_eq!(rows[0]["binding"], v.binding());
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    let part_bytes = serde_json::to_vec(&part).unwrap().len();
    println!("DQ_REHEARSAL_TRANSCRIPT_BYTES full={full_bytes} two_words={part_bytes}");
    assert!(part_bytes < full_bytes);
    let project = invoke(
        &s,
        "get_timeline_transcript",
        json!({"range":{"start":100,"end":260}}),
    );
    ok(&project);
    assert_eq!(content(&project)["segments"][0]["first"], 1);
    let levels = content(&full)["recording_levels"].as_object().unwrap();
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[v.binding()], json!([-1800, -6000]));
}
#[test]
fn dq_rehearsal_neighbours_are_current_addresses_for_the_next_cut() {
    let entries: Vec<_> = (0..10)
        .map(|i| Word {
            text: format!("w{i}"),
            start_micros: 200_000 + i * 800_000,
            end_micros: 500_000 + i * 800_000,
        })
        .collect();
    let sound: Vec<_> = entries
        .iter()
        .map(|w| (w.start_micros, w.end_micros))
        .collect();
    let v = value(entries, 10_000_000, |t| {
        level(!sound.iter().any(|&(a, b)| a <= t && t < b))
    });
    let s = service(
        document(
            Rational::new(100, 1).unwrap(),
            Rational::new(100, 1).unwrap(),
            0,
            1000,
        ),
        Arc::clone(&v),
    );
    let first = cut(&s, args(vec![words(&v, 4)]), true);
    ok(&first);
    let n = &content(&first)["neighbours"]["joins"][0];
    assert!(n.is_object(), "DQ_REHEARSAL_KEPT_NEIGHBOURS: {first:?}");
    let indices = |side: &str| {
        n[side]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a[1].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(indices("left"), vec![1, 2, 3]);
    assert_eq!(indices("right"), vec![5, 6, 7]);
    assert!(n["not_measured"].as_array().unwrap().is_empty());
    let after_first = s.snapshot().unwrap().1;
    let next = &n["right"][0];
    let binding = &content(&first)["neighbours"]["bindings"][next[0].as_u64().unwrap().to_string()];
    assert_eq!(binding, v.binding());
    let address = json!({"clip":next[0],"index":next[1],"binding":binding});
    let result = cut(
        &s,
        json!({"expected_revision":content(&first)["revision"],"removals":[{"kind":"words","first":address,"last":address,"leave_micros":250_000}]}),
        true,
    );
    ok(&result);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, after_first);
}
#[test]
fn dq_rehearsal_transcript_ranges_refuse_invalid_addresses() {
    let (s, _) = fixture();
    for word_range in [
        json!({"clip":1,"first":1,"end":99}),
        json!({"clip":1,"first":1,"end":1}),
    ] {
        let r = invoke(
            &s,
            "get_timeline_transcript",
            json!({"word_range":word_range}),
        );
        assert_eq!(r.is_error, Some(true));
        assert_eq!(content(&r)["reason"]["code"], "word");
    }
    let missing = invoke(
        &s,
        "get_timeline_transcript",
        json!({"word_range":{"clip":99,"first":0,"end":1}}),
    );
    assert_eq!(missing.is_error, Some(true));
    assert_eq!(
        content(&missing)["reason"],
        json!({"code":"clip","clip":99})
    );
    let r = invoke(
        &s,
        "get_timeline_transcript",
        json!({"word_range":{"clip":1,"first":0,"end":1},"range":{"start":0,"end":10}}),
    );
    assert_eq!(r.is_error, Some(true));
    assert_eq!(content(&r)["reason"]["code"], "request");
}
#[test]
fn dq_rehearsal_neighbours_cross_clip_boundaries_without_a_full_read() {
    let v = ordinary();
    let fps = Rational::new(100, 1).unwrap();
    let doc = repeated_document(fps, fps, 0, 100, 5);
    let s = service(doc, Arc::clone(&v));
    let address = json!({"clip":3,"index":0,"binding":v.binding()});
    let r = cut(
        &s,
        args(vec![json!({"kind":"words","first":address,"last":address})]),
        true,
    );
    ok(&r);
    let rows = &content(&r)["neighbours"]["joins"][0];
    assert_eq!(rows["left"], json!([[1, 0], [2, 0]]));
    assert_eq!(rows["right"], json!([[4, 0], [5, 0]]));
    assert!(rows["not_measured"].as_array().unwrap().is_empty());
    assert_eq!(
        content(&r)["neighbours"]["bindings"]
            .as_object()
            .unwrap()
            .len(),
        4
    );
}
