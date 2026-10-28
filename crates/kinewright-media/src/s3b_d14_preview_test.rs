#[test]
fn s3b_d14_superseded_real_refill_delivers_target_without_optional_redecode() {
    let workload = crate::perf_fixtures::one_source((160, 90), (160, 90), 60, 10);
    let document = Arc::new(workload.0.clone());
    let lane = Arc::new(Lane::default());
    let (frames, worker) = threaded_preview(&lane);
    lane.post(Some(job(
        &document,
        JobKind::Paused(TimeCode(20)),
        stamp(1, 1),
    )));
    let first = frames.recv_timeout(Duration::from_secs(10));
    if first.is_err() {
        let failures = lane.take_failures();
        lane.shut_down();
        join_within(worker);
        panic!("D14 setup frame missing: {failures:?}");
    }
    assert_eq!(first.unwrap().at, TimeCode(20));
    wait_until(&lane, |state| !decoding(state));
    lane.decodes.lock().unwrap().clear();
    let (release, held) = bounded::<()>(0);
    *lane.hold_at.lock().unwrap() = Some((19, held));
    lane.post(Some(job(
        &document,
        JobKind::Paused(TimeCode(19)),
        stamp(1, 2),
    )));
    wait_until(&lane, |state| decoding_at(state, 19));
    let refill_version = lane.lock().readers.version();
    lane.post(Some(job(
        &document,
        JobKind::Paused(TimeCode(8)),
        stamp(1, 3),
    )));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut state = lane.lock();
    while !(state.readers.version() > refill_version
        && state.readers.slots.iter().any(|slot| {
            slot.plan.version == state.readers.version() && slot.plan.required.contains(&8)
        }))
    {
        assert!(
            Instant::now() < deadline,
            "new plan reached the reader before real refill completion"
        );
        // A post wakes before the preview applies the plan; observe that later
        // state with bounded condvar waits, without supplying any completion.
        state = state.wait_timeout(&lane.ready, Duration::from_millis(10));
    }
    drop(state);
    drop(release);
    let shown = shown_at(&frames, 8);
    wait_until(&lane, |state| !decoding(state));
    let records = lane.decodes.lock().unwrap().clone();
    lane.shut_down();
    join_within(worker);
    assert!(lane.take_failures().is_empty());
    let refill = records
        .iter()
        .find(|record| record.at == 19)
        .expect("actual superseded refill");
    assert_eq!(
        refill.kept_times, 9,
        "GOP-10 refill kept exactly 10 through 18"
    );
    let redundant: Vec<_> = records
        .iter()
        .filter(|record| (4..8).contains(&record.at))
        .collect();
    let converted: u64 = redundant.iter().map(|record| record.converted).sum();
    println!(
        "S3B_D14 target=8 scheduled_optional={} optional_conversions={} target_received={} target_converted={}",
        redundant.len(),
        converted,
        records.iter().find(|r| r.at == 8).unwrap().received,
        records.iter().find(|r| r.at == 8).unwrap().converted
    );
    assert!(
        redundant.is_empty(),
        "unavailable optional requests eliminated: {records:?}"
    );
    assert_eq!(
        converted, 0,
        "necessary GOP pre-roll is separately recorded by received"
    );
    let mut reference = FrameRenderer::legacy_preview_fixture(fallback_gpu().context());
    let expected = reference
        .render_live(
            &document,
            TimeCode(8),
            (160, 90),
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .unwrap();
    assert_eq!(*shown.texture.rgba, *expected.rgba, "target byte parity");
    assert_eq!(
        lane.lock().readers.live().0,
        0,
        "all real CPU charge holders returned"
    );
}
