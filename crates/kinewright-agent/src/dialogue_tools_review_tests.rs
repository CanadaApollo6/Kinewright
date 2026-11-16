fn repeated_document(
    rate: Rational,
    project: Rational,
    start: i64,
    end: i64,
    clips: usize,
) -> Document {
    let mut doc = document(rate, project, start, end);
    let prototype = doc.tracks[0].clips.pop().unwrap();
    let duration = doc.clip_duration(&prototype).unwrap().0;
    for n in 0..clips {
        // Each fixture clip owns its editable attributes; this is a new project.
        let mut c = prototype.clone();
        c.id = ClipId(u64::try_from(n).unwrap() + 1);
        c.timeline_start = TimeCode(i64::try_from(n).unwrap() * duration);
        doc.tracks[0].clips.push(c);
    }
    doc.duration = TimeCode(i64::try_from(clips).unwrap() * duration);
    doc.validate().unwrap();
    doc
}
fn repeated_removals(v: &BoundDialogue, count: usize) -> Value {
    args(
        (1..=count)
            .map(|clip| {
                let address = json!({"clip":clip,"index":1,"binding":v.binding()});
                json!({"kind":"words","first":address,"last":address})
            })
            .collect(),
    )
}

#[test]
#[ignore = "on-demand: dialogue scratch cost at 3000 clips and 512 failed trials"]
fn dq_tools_review_scratch_cost() {
    let mut times = Vec::new();
    for round in 0..5 {
        let v = ordinary();
        let fps = Rational::new(100, 1).unwrap();
        let s = service(repeated_document(fps, fps, 0, 400, 3000), Arc::clone(&v));
        let args = repeated_removals(&v, 10);
        dq::cost::begin();
        let started = Instant::now();
        let result = cut(&s, args, true);
        let elapsed = started.elapsed();
        let counts = dq::cost::end();
        ok(&result);
        times.push(elapsed);
        println!(
            "DQ_SCRATCH_TEN round={round} wall_us={} trials={} operations={}",
            elapsed.as_micros(),
            counts[0],
            counts[1]
        );
    }
    times.sort();
    println!("DQ_SCRATCH_TEN_MEDIAN_US {}", times[2].as_micros());
    let v = clock_value();
    let s = service(
        repeated_document(
            Rational::new(25, 1).unwrap(),
            Rational::new(24, 1).unwrap(),
            14,
            64,
            3000,
        ),
        Arc::clone(&v),
    );
    let args = repeated_removals(&v, 32);
    dq::cost::begin();
    let started = Instant::now();
    let result = cut(&s, args, true);
    let elapsed = started.elapsed();
    let counts = dq::cost::end();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(counts[0], 512, "DQ_SCRATCH_WORST_ALL_TRIALS");
    println!(
        "DQ_SCRATCH_WORST wall_us={} trials={} operations={}",
        elapsed.as_micros(),
        counts[0],
        counts[1]
    );
}

fn tail_value(ntsc: bool, sounding_tail: bool) -> Arc<BoundDialogue> {
    let ranges = if ntsc {
        vec![
            ("kept", 200_000, 500_000),
            ("remove", 16_100_000, 16_400_000),
            ("next", 17_000_000, 17_300_000),
            ("last", 19_000_000, 19_300_000),
        ]
    } else {
        vec![
            ("kept", 200_000, 500_000),
            ("remove", 1_100_000, 1_300_000),
            ("next", 1_600_000, 2_000_000),
            ("last", 3_700_000, 4_000_000),
        ]
    };
    let words = ranges
        .iter()
        .map(|&(text, start_micros, end_micros)| Word {
            text: text.into(),
            start_micros,
            end_micros,
        })
        .collect();
    value(words, if ntsc { 20_020_000 } else { 4_000_000 }, |t| {
        let sound = ranges
            .iter()
            .any(|&(word, a, b)| a <= t && t < b && (word != "last" || sounding_tail || ntsc))
            || ntsc && sounding_tail && t >= 19_700_000;
        level(!sound)
    })
}
fn assert_refused_unchanged(
    s: &KinewrightMcp,
    before: &(TimelineRevision, Arc<Document>),
    history: &(
        kinewright_core::HistoryChangeId,
        kinewright_core::HistorySnapshot,
    ),
    r: &CallToolResult,
    marker: &str,
) {
    assert_eq!(r.is_error, Some(true), "{marker}: {r:?}");
    assert_eq!(&s.snapshot().unwrap(), before, "{marker}");
    assert_eq!(
        &s.core.export_history_identified().unwrap(),
        history,
        "{marker}"
    );
}
#[test]
fn dq_tools_review_unselected_sounding_tail_is_preserved() {
    let v = tail_value(false, true);
    for copies in [1, 2] {
        let doc = repeated_document(
            Rational::new(25, 1).unwrap(),
            Rational::new(24, 1).unwrap(),
            0,
            100,
            copies,
        );
        let s = service(doc, Arc::clone(&v));
        let before = s.snapshot().unwrap();
        let history = s.core.export_history_identified().unwrap();
        let r = cut(&s, args(vec![words(&v, 1)]), true);
        println!("DQ_RETAINED_TAIL copies={copies} {}", content(&r));
        if r.is_error == Some(true) {
            assert_refused_unchanged(
                &s,
                &before,
                &history,
                &r,
                "DQ_TOOLS_UNSELECTED_TAIL_PROTECTED",
            );
            assert!(
                content(&r).to_string().contains("frame rates differ"),
                "DQ_TOOLS_TAIL_CAUSE: {r:?}"
            );
        } else {
            let after = s.snapshot().unwrap().1;
            let next = after.clip(ClipId(2)).map(|c| c.timeline_start);
            let end = dq::spans(&after)
                .unwrap()
                .into_iter()
                .rfind(|s| next.is_none_or(|at| s.project.start < at))
                .unwrap()
                .source
                .end;
            assert_eq!(
                end,
                kinewright_core::ExactMicros::integer(4_000_000),
                "DQ_TOOLS_UNSELECTED_TAIL_PROTECTED"
            );
            let check = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
            assert!(
                !content(&check).to_string().contains("\"edge_in_sound\":"),
                "DQ_TOOLS_NO_UNREPORTED_TAIL_SOUND"
            );
            assert!(content(&r)["retained_edges"].is_array());
        }
    }
}
#[test]
fn dq_tools_review_ntsc_quiet_loss_is_reported_and_sound_loss_refuses() {
    for sounding in [false, true] {
        let v = tail_value(true, sounding);
        let s = service(
            document(
                Rational::new(30_000, 1_001).unwrap(),
                Rational::new(30, 1).unwrap(),
                0,
                600,
            ),
            Arc::clone(&v),
        );
        let before = s.snapshot().unwrap();
        let history = s.core.export_history_identified().unwrap();
        let r = cut(&s, args(vec![words(&v, 1)]), true);
        println!("DQ_NTSC_TAIL sound={sounding} {}", content(&r));
        if sounding {
            assert_refused_unchanged(
                &s,
                &before,
                &history,
                &r,
                "DQ_TOOLS_NTSC_SOUNDING_LOSS_REFUSED",
            );
            assert!(content(&r).to_string().contains("frame rates differ"));
        } else {
            ok(&r);
            assert!(
                content(&r)["extent_changes"]["measured"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["kind"] == "quiet_loss" && c["micros"].as_u64().unwrap() > 0),
                "DQ_TOOLS_NTSC_QUIET_LOSS_REPORTED"
            );
            s.core.request(Command::Undo).unwrap();
            assert_eq!(s.snapshot().unwrap().1, before.1);
        }
    }
}
#[test]
fn dq_tools_review_unproven_enabled_origins_are_not_guard_evidence() {
    let v = value(vec![], 4_480_000, |t| {
        level(!(1_420_000..1_600_000).contains(&t))
    });
    let mut doc = document(
        Rational::new(25, 1).unwrap(),
        Rational::new(24, 1).unwrap(),
        12,
        112,
    );
    doc.tracks[0].clips[0].enabled_curve = Some(kinewright_core::AutomationCurve {
        keyframes: [(0, 0), (23, 1)]
            .into_iter()
            .map(|(at, value)| kinewright_core::Keyframe {
                at: TimeCode(at),
                value,
                interpolation: kinewright_core::KeyframeInterpolation::Hold,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    });
    let s = service(doc, v);
    let r = invoke(
        &s,
        "check_dialogue_cut",
        json!({"track_id":1,"detail":"all"}),
    );
    ok(&r);
    assert!(
        content(&r)["check"]["segments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        content(&r)["check"]["not_checked"][0]["code"],
        "unproven_segment"
    );
    assert_eq!(content(&r)["check"]["not_checked"][0]["range"][0], 23);
}
#[test]
fn dq_tools_review_global_bound_prioritizes_sound_and_filters_unavailable_range() {
    let fps = Rational::new(100, 1).unwrap();
    let v = ordinary();
    let mut doc = repeated_document(fps, fps, 0, 400, 3000);
    for (i, c) in doc.tracks[0].clips.iter_mut().enumerate() {
        c.speed_percent = 200;
        c.timeline_start = TimeCode(i64::try_from(i).unwrap() * 200);
    }
    let mut clip = doc.tracks[0].clips[0].clone();
    clip.id = ClipId(3001);
    clip.speed_percent = 100;
    clip.source_range = TimeCode(20)..TimeCode(380);
    clip.timeline_start = TimeCode(600_000);
    doc.tracks[0].clips.push(clip);
    doc.duration = TimeCode(600_360);
    let s = service(doc, v);
    let r = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
    ok(&r);
    let bytes = serde_json::to_vec(&r).unwrap().len();
    println!("DQ_CHECK_GLOBAL_BYTES {bytes}");
    assert!(bytes <= 4096, "DQ_CHECK_GLOBAL_WIRE_BOUND");
    let c = &content(&r)["check"];
    assert!(
        c["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["report"]["items"].as_array().unwrap())
            .any(|i| i["sides"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| !s["edge_in_sound"].is_null()))),
        "DQ_CHECK_ACOUSTIC_FIRST"
    );
    assert_eq!(
        c["omitted"]["not_checked"].as_u64().unwrap()
            + u64::try_from(c["not_checked"].as_array().unwrap().len()).unwrap(),
        3000
    );
    assert!(c["more"].as_str().unwrap().contains("range"));
    let r = invoke(
        &s,
        "check_dialogue_cut",
        json!({"track_id":1,"range":[1000,1400]}),
    );
    ok(&r);
    assert_eq!(
        content(&r)["check"]["not_checked"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for row in content(&r)["check"]["not_checked"].as_array().unwrap() {
        assert!(
            row["range"][0].as_i64().unwrap() >= 1000 && row["range"][1].as_i64().unwrap() <= 1400
        );
    }
}
#[test]
fn dq_tools_review_request_text_is_bounded_at_construction_and_on_wire() {
    for text in ["x".repeat(1000), "界".repeat(1000)] {
        let direct = Refusal::Request { why: text.clone() };
        assert!(direct.to_string().chars().count() <= 128 + "invalid dialogue request: ".len());
        assert!(
            serde_json::to_value(direct).unwrap()["why"]
                .as_str()
                .unwrap()
                .len()
                <= 128
        );
        let made = dq::request(text);
        let Refusal::Request { why } = made else {
            unreachable!()
        };
        assert!(why.len() <= 128 && why.ends_with("..."));
    }
}

#[test]
fn dq_tools_review_one_frame_is_structurally_refused() {
    let v = ordinary();
    let fps = Rational::new(10, 1).unwrap();
    let s = service(document(fps, fps, 0, 40), Arc::clone(&v));
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let r = cut(
        &s,
        args(vec![
            json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[1_200_000,1_300_000]}),
        ]),
        true,
    );
    assert_refused_unchanged(&s, &before, &history, &r, "DQ_TOOLS_TWO_PROJECT_FRAMES");
    assert!(content(&r).to_string().contains("two project frames"));
}
#[test]
fn dq_tools_review_sync_tracks_and_atomic_undo() {
    let v = ordinary();
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = document(fps, fps, 0, 400);
    // Independent tracks intentionally own independent clip attributes.
    let mut parallel = doc.tracks[0].clone();
    parallel.id = TrackId(2);
    parallel.clips[0].id = ClipId(2);
    doc.tracks.push(parallel);
    doc.validate().unwrap();
    let s = service(doc, Arc::clone(&v));
    let before = s.snapshot().unwrap().1;
    let r = cut(&s, args(vec![words(&v, 1), words(&v, 3)]), true);
    ok(&r);
    let after = s.snapshot().unwrap().1;
    let geometry = |track: usize| {
        after.tracks[track]
            .clips
            .iter()
            .map(|c| (c.source_range.clone(), c.timeline_start))
            .collect::<Vec<_>>()
    };
    assert_eq!(geometry(0), geometry(1), "DQ_TOOLS_SYNC_EXACT_GEOMETRY");
    assert_eq!(
        geometry(0),
        vec![
            (TimeCode(0)..TimeCode(65), TimeCode(0)),
            (TimeCode(190)..TimeCode(245), TimeCode(65))
        ]
    );
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before, "DQ_TOOLS_SYNC_ONE_UNDO");
}
#[test]
fn dq_tools_review_binding_changes_during_confirmation() {
    let (s, v) = fixture();
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let changed = value(said(), 4_000_000, |_| level(true));
    thread::scope(|scope| {
        let worker = scope.spawn(|| invoke(&s, "cut_dialogue", args(vec![words(&v, 1)])));
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let pending = s.confirmations.pending_requests();
            if let Some(request) = pending.first() {
                *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Fake {
                    value: changed,
                    stream: 0,
                }));
                assert!(s.confirmations.approve(request.id));
                break;
            }
            assert!(!worker.is_finished(), "DQ_TOOLS_CONFIRMATION_REQUIRED");
            assert!(Instant::now() < until, "DQ_TOOL_REQUEST_BOUNDED");
            thread::yield_now();
        }
        let r = worker.join().unwrap();
        assert_refused_unchanged(
            &s,
            &before,
            &history,
            &r,
            "DQ_TOOLS_BINDING_RECHECK_AFTER_CONSENT",
        );
        assert_eq!(content(&r)["reason"], json!({"code":"binding_changed"}));
    });
}
#[test]
fn dq_tools_review_stale_before_confirmation_never_prompts() {
    struct Changes {
        first: Arc<BoundDialogue>,
        second: Arc<BoundDialogue>,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl DialogueMeasurements for Changes {
        fn audio_stream(&self, _: &MediaIdentity) -> Result<u32, Refusal> {
            Ok(0)
        }
        fn get(&self, _: &MediaIdentity, _: u32) -> Result<Arc<BoundDialogue>, Refusal> {
            Ok(Arc::clone(
                if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    &self.first
                } else {
                    &self.second
                },
            ))
        }
    }
    let (s, v) = fixture();
    *s.dialogue_measurements.write().unwrap() = Some(Arc::new(Changes {
        first: Arc::clone(&v),
        second: value(said(), 4_000_000, |_| level(true)),
        reads: std::sync::atomic::AtomicUsize::new(0),
    }));
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    thread::scope(|scope| {
        let worker = scope.spawn(|| invoke(&s, "cut_dialogue", args(vec![words(&v, 1)])));
        let mut prompted = false;
        let until = Instant::now() + Duration::from_secs(60);
        while !worker.is_finished() {
            for request in s.confirmations.pending_requests() {
                prompted = true;
                s.confirmations
                    .reject(request.id, "stale plan must not prompt");
            }
            assert!(Instant::now() < until, "DQ_TOOL_REQUEST_BOUNDED");
            thread::yield_now();
        }
        let r = worker.join().unwrap();
        assert!(!prompted, "DQ_TOOLS_STALE_BEFORE_PROMPT");
        assert_refused_unchanged(&s, &before, &history, &r, "DQ_TOOLS_STALE_NOT_CONSUMED");
        assert_eq!(content(&r)["reason"], json!({"code":"binding_changed"}));
    });
}
#[test]
fn dq_tools_review_last_word_overrule_keeps_the_requested_end() {
    let (s, v) = fixture();
    let r = cut(
        &s,
        args(vec![
            json!({"kind":"words","first":address(&v,3),"last":address(&v,3),"overrule":[3_100_000,3_300_000]}),
        ]),
        true,
    );
    ok(&r);
    assert_eq!(
        content(&r)["removals"][0]["cut"],
        json!([310, 330]),
        "DQ_TOOLS_TAIL_OVERRULE_INSTANT"
    );
    assert_eq!(
        s.snapshot().unwrap().1.tracks[0]
            .clips
            .last()
            .unwrap()
            .source_range
            .end,
        TimeCode(400)
    );
}
#[test]
fn dq_tools_review_half_speed_is_not_checked() {
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = document(fps, fps, 0, 400);
    doc.tracks[0].clips[0].speed_percent = 50;
    doc.duration = TimeCode(800);
    let s = service(doc, ordinary());
    let r = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
    ok(&r);
    assert_eq!(
        content(&r)["check"]["not_checked"][0]["code"],
        "retimed",
        "DQ_TOOLS_HALF_SPEED_NOT_CHECKED"
    );
    assert!(
        content(&r)["check"]["segments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn dq_tools_review_removal_count_boundary() {
    for count in [32, 33] {
        let v = ordinary();
        let fps = Rational::new(100, 1).unwrap();
        let s = service(repeated_document(fps, fps, 0, 400, count), Arc::clone(&v));
        let r = cut(&s, repeated_removals(&v, count), true);
        if count == 32 {
            ok(&r);
            assert_eq!(s.snapshot().unwrap().0, TimelineRevision(1));
        } else {
            assert_eq!(r.is_error, Some(true), "DQ_TOOLS_33_REFUSED");
            assert!(content(&r).to_string().contains("1 to 32"));
            assert_eq!(s.snapshot().unwrap().0, TimelineRevision(0));
        }
    }
}
#[test]
fn dq_tools_review_transcript_real_enabled_gap_starts_new_group() {
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = document(fps, fps, 0, 400);
    doc.tracks[0].clips[0].enabled_curve = Some(kinewright_core::AutomationCurve {
        keyframes: [(0, 1), (100, 0), (200, 1)]
            .into_iter()
            .map(|(at, value)| kinewright_core::Keyframe {
                at: TimeCode(at),
                value,
                interpolation: kinewright_core::KeyframeInterpolation::Hold,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    });
    let s = service(doc, ordinary());
    let r = invoke(&s, "get_timeline_transcript", json!({}));
    ok(&r);
    let groups = content(&r)["segments"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "DQ_TOOLS_TRANSCRIPT_GAP_GROUPS");
    assert_eq!(groups[0]["first"], 0);
    assert_eq!(groups[1]["first"], 2);
    assert!(groups[0]["words"].to_string().contains("Hello"));
    assert!(
        !groups
            .iter()
            .any(|g| g["words"].to_string().contains("remove"))
    );
}
#[test]
fn dq_tools_review_minimum_uses_source_time_once() {
    let v = value(said(), 4_000_000, |_| level(true));
    for width in [100_000, 66_666] {
        let s = service(
            document(
                Rational::new(30, 1).unwrap(),
                Rational::new(24, 1).unwrap(),
                0,
                120,
            ),
            Arc::clone(&v),
        );
        let r = cut(
            &s,
            args(vec![
                json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[2_000_000,2_000_000+width]}),
            ]),
            true,
        );
        println!("DQ_MIN_SOURCE width={width} {}", content(&r));
        if width == 100_000 {
            ok(&r);
            assert_eq!(
                content(&r)["removals"][0]["cut"],
                json!([48, 50]),
                "DQ_TOOLS_SOURCE_MINIMUM_ACCEPTED"
            );
        } else {
            assert_eq!(r.is_error, Some(true), "DQ_TOOLS_SOURCE_MINIMUM_REFUSED");
            assert!(content(&r).to_string().contains("100000"));
        }
    }
}
#[test]
fn dq_tools_review_whole_multiple_cuts_preserve_outside_extent() {
    let measurement = value(said(), 5_000_000, |_| level(true));
    let mut seed = 81_u64;
    for (source, project, frames) in [(30, 30, 120), (60, 30, 240), (60_000, 30_000, 240)] {
        let den = if source == 60_000 { 1001 } else { 1 };
        let source = Rational::new(source, den).unwrap();
        let project = Rational::new(project, den).unwrap();
        for _ in 0..24 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let start = 10 + i64::try_from(seed % 45).unwrap();
            let end = start + 15;
            let start_us = kinewright_core::ExactMicros::frames(TimeCode(start), project)
                .unwrap()
                .ceil();
            let end_us = kinewright_core::ExactMicros::frames(TimeCode(end), project)
                .unwrap()
                .floor();
            let service = service(
                document(source, project, 0, frames),
                Arc::clone(&measurement),
            );
            let original = dq::spans(&service.snapshot().unwrap().1).unwrap().remove(0);
            let reply = cut(
                &service,
                args(vec![
                    json!({"kind":"words","first":address(&measurement,1),"last":address(&measurement,1),"overrule":[start_us,end_us]}),
                ]),
                true,
            );
            ok(&reply);
            let after = dq::spans(&service.snapshot().unwrap().1).unwrap();
            assert_eq!(after.len(), 2);
            let prefix = &after[0].source;
            let suffix = &after[1].source;
            assert_eq!(
                prefix.start, original.source.start,
                "DQ_TOOLS_MULTIPLE_PREFIX_UNCHANGED"
            );
            assert_eq!(
                suffix.end, original.source.end,
                "DQ_TOOLS_MULTIPLE_TAIL_UNCHANGED"
            );
            assert_eq!(
                suffix.padding, original.source.padding,
                "DQ_TOOLS_MULTIPLE_OUTER_PADDING_UNCHANGED"
            );
            assert!(prefix.end >= kinewright_core::ExactMicros::integer(start_us));
            assert!(suffix.start <= kinewright_core::ExactMicros::integer(end_us));
            assert!(
                content(&reply)["extent_changes"]["measured"].is_null(),
                "DQ_TOOLS_WHOLE_MULTIPLE_OUTSIDE_UNCHANGED: {reply:?}"
            );
        }
    }
}
#[test]
fn dq_tools_review_adjacent_overrule_does_not_cover_the_backed_off_edge() {
    let ws = [
        ("kept", 700_000, 1_560_000),
        ("remove", 1_550_000, 2_300_000),
        ("next", 2_700_000, 3_000_000),
    ]
    .into_iter()
    .map(|(text, start_micros, end_micros)| Word {
        text: text.into(),
        start_micros,
        end_micros,
    })
    .collect();
    let v = value(ws, 3_200_000, |t| {
        level(
            (1_490_000..1_550_000).contains(&t)
                || (2_410_000..2_470_000).contains(&t)
                || (2_490_000..2_550_000).contains(&t)
                || t >= 3_000_000,
        )
    });
    let s = service(
        document(
            Rational::new(25, 1).unwrap(),
            Rational::new(24, 1).unwrap(),
            12,
            80,
        ),
        Arc::clone(&v),
    );
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let r = cut(
        &s,
        args(vec![
            words(&v, 1),
            json!({"kind":"words","first":address(&v,2),"last":address(&v,2),"overrule":[2_480_000,3_000_000]}),
        ]),
        true,
    );
    println!("DQ_SCOPED_OVERRULE {}", content(&r));
    if r.is_error == Some(true) {
        assert_refused_unchanged(&s, &before, &history, &r, "DQ_TOOLS_SCOPED_OVERRULE");
    } else {
        let outcome = &content(&r)["removals"][0];
        for edge in outcome["edges"].as_array().unwrap() {
            let exact = kinewright_core::ExactMicros::new(
                u128::from(edge[2][0].as_u64().unwrap()),
                edge[2][1].as_u64().unwrap(),
            )
            .unwrap();
            assert!(
                v.dialogue()
                    .guard_exact(
                        exact,
                        if edge == &outcome["edges"][0] {
                            AudibleSide::Out
                        } else {
                            AudibleSide::In
                        }
                    )
                    .is_ok(),
                "DQ_TOOLS_SCOPED_OVERRULE"
            );
        }
        // The sliver's other boundary is still owned by A, although B starts here.
        let edge = &content(&r)["removals"][1]["edges"][0];
        let exact = kinewright_core::ExactMicros::new(
            edge[2][0].as_u64().unwrap().into(),
            edge[2][1].as_u64().unwrap(),
        )
        .unwrap();
        assert!(
            v.dialogue().guard_exact(exact, AudibleSide::Out).is_ok(),
            "DQ_TOOLS_SCOPED_OVERRULE"
        );
    }
}
#[test]
fn dq_tools_review_two_cuts_and_unmeasured_sync_extents() {
    let v = ordinary();
    let s = service(
        document(
            Rational::new(25, 1).unwrap(),
            Rational::new(24, 1).unwrap(),
            0,
            100,
        ),
        Arc::clone(&v),
    );
    let before = s.snapshot().unwrap().1;
    let r = cut(&s, args(vec![words(&v, 1), words(&v, 3)]), true);
    ok(&r);
    assert_eq!(content(&r)["removals"].as_array().unwrap().len(), 2);
    s.core.request(Command::Undo).unwrap();
    assert_eq!(s.snapshot().unwrap().1, before);
    let mut doc = document(
        Rational::new(24, 1).unwrap(),
        Rational::new(24, 1).unwrap(),
        0,
        96,
    );
    let mut music = doc.media_pool[0].clone();
    music.id = AssetId(2);
    music.fps = Rational::new(25, 1).unwrap();
    music.duration = TimeCode(100);
    music.source_fingerprint.content_sha256 = Some("b".repeat(64));
    doc.media_pool.push(music);
    let mut bed = doc.tracks[0].clone();
    bed.id = TrackId(2);
    bed.clips[0].id = ClipId(2);
    bed.clips[0].asset = AssetId(2);
    bed.clips[0].source_range = TimeCode(0)..TimeCode(100);
    doc.tracks.push(bed);
    doc.validate().unwrap();
    let s = service(doc, Arc::clone(&v));
    let before = s.snapshot().unwrap();
    let history = s.core.export_history_identified().unwrap();
    let r = cut(&s, args(vec![words(&v, 1)]), true);
    if r.is_error == Some(true) {
        assert_refused_unchanged(
            &s,
            &before,
            &history,
            &r,
            "DQ_TOOLS_SYNC_UNMEASURED_PRESERVED",
        );
        assert_eq!(content(&r)["removals"][0]["code"], "unmeasured_extent");
    } else {
        item12_assert_sync_zero_change(&before.1, &s.snapshot().unwrap().1, &r);
    }
}
#[test]
fn dq_tools_review_recording_dictionary_shares_the_wire_budget() {
    let fps = Rational::new(100, 1).unwrap();
    let mut doc = repeated_document(fps, fps, 0, 400, 250);
    let template = doc.media_pool.pop().unwrap();
    for (n, clip) in doc.tracks[0].clips.iter_mut().enumerate() {
        // New fixture assets with identical authenticated recording content.
        let mut asset = template.clone();
        asset.id = AssetId(u64::try_from(n).unwrap() + 1);
        clip.asset = asset.id;
        doc.media_pool.push(asset);
    }
    doc.validate().unwrap();
    let s = service(doc, ordinary());
    let r = invoke(&s, "check_dialogue_cut", json!({"track_id":1}));
    ok(&r);
    let bytes = serde_json::to_vec(&r).unwrap().len();
    assert!(bytes <= 4096, "DQ_TOOLS_RECORDINGS_GLOBAL_BUDGET: {bytes}");
    let check = &content(&r)["check"];
    assert_eq!(
        check["omitted"]["recordings"].as_u64().unwrap()
            + u64::try_from(check["recordings"].as_object().unwrap().len()).unwrap(),
        250,
        "DQ_TOOLS_OMITTED_RECORDING_COUNT"
    );
    println!("DQ_CHECK_RECORDINGS_BYTES {bytes}");
}
#[test]
fn dq_tools_review_growth_is_reported_and_its_edge_is_guarded_even_with_overrule() {
    for sounding in [false, true] {
        let v = value(said(), 5_000_000, |t| {
            level(
                !((sounding && (4_000_000..4_010_000).contains(&t))
                    || [
                        (200_000, 500_000),
                        (1_100_000, 1_400_000),
                        (2_000_000, 2_300_000),
                        (3_000_000, 3_300_000),
                    ]
                    .into_iter()
                    .any(|(a, b)| a <= t && t < b)),
            )
        });
        let s = service(
            document(
                Rational::new(30_000, 1001).unwrap(),
                Rational::new(30, 1).unwrap(),
                0,
                120,
            ),
            Arc::clone(&v),
        );
        let before = s.snapshot().unwrap();
        let history = s.core.export_history_identified().unwrap();
        assert_eq!(
            dq::spans(&before.1).unwrap()[0].source.end,
            kinewright_core::ExactMicros::integer(4_000_000)
        );
        let r = cut(
            &s,
            args(vec![
                json!({"kind":"words","first":address(&v,1),"last":address(&v,1),"overrule":[650_000,1_900_000]}),
            ]),
            true,
        );
        println!("DQ_GROWTH sound={sounding} {}", content(&r));
        if sounding {
            assert_refused_unchanged(&s, &before, &history, &r, "DQ_TOOLS_GROWTH_NOT_OVERRULED");
            assert!(
                content(&r)
                    .to_string()
                    .contains("sounding untranscribed audio")
            );
        } else {
            ok(&r);
            assert!(
                content(&r)["extent_changes"]["measured"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["kind"] == "growth" && c["micros"].as_u64().unwrap() > 0),
                "DQ_TOOLS_GROWTH_REPORTED"
            );
        }
    }
}
#[test]
fn dq_tools_review_quiet_edge_cannot_hide_sound_later_in_the_lost_stretch() {
    let tokens = [
        ("kept", 200_000, 500_000),
        ("remove", 1_100_000, 1_300_000),
        ("next", 1_600_000, 2_000_000),
        ("last", 3_700_000, 4_000_000),
    ]
    .into_iter()
    .map(|(text, start_micros, end_micros)| Word {
        text: text.into(),
        start_micros,
        end_micros,
    })
    .collect();
    let measurement = value(tokens, 4_000_000, |t| {
        level(
            ![
                (200_000, 500_000),
                (1_100_000, 1_300_000),
                (1_600_000, 2_000_000),
                (3_990_000, 4_000_000),
            ]
            .into_iter()
            .any(|(a, b)| a <= t && t < b),
        )
    });
    assert!(
        measurement
            .dialogue()
            .guard(3_985_000, AudibleSide::Out)
            .is_ok(),
        "DQ_TOOLS_NEW_EDGE_ALONE_IS_QUIET"
    );
    let server = service(
        document(
            Rational::new(25, 1).unwrap(),
            Rational::new(24, 1).unwrap(),
            0,
            100,
        ),
        Arc::clone(&measurement),
    );
    let before = server.snapshot().unwrap();
    let history = server.core.export_history_identified().unwrap();
    let reply = cut(&server, args(vec![words(&measurement, 1)]), true);
    assert_refused_unchanged(
        &server,
        &before,
        &history,
        &reply,
        "DQ_TOOLS_ENTIRE_LOSS_IS_QUIET",
    );
    assert_eq!(content(&reply)["removals"][0]["code"], "unselected_audio");
}
