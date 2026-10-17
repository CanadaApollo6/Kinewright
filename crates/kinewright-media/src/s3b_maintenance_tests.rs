fn s3b_paused(preview: &mut Preview, document: &Arc<Document>, seq: u64) {
    preview.lane.post(Some(job(
        document,
        JobKind::Paused(TimeCode::ZERO),
        stamp(1, seq),
    )));
    let work = preview.next_work(false).expect("paused work selected");
    preview.execute(work);
    assert!(
        preview.lane.take_failures().is_empty(),
        "actual render succeeds"
    );
}

#[test]
fn s3b_maintenance_paused_expiry_without_another_request() {
    let (start_tx, start_rx) = bounded(1);
    let (tx, rx) = bounded(1);
    let worker = std::thread::spawn(move || {
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        s3b_paused(&mut preview, &Arc::new(title_card((64, 64), 4)), 1);
        frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(preview.renderer.s3b_residents().0, 1);
        let gpu = preview.renderer.display_gpu();
        start_tx.send(preview.lane.clone()).unwrap();
        while let Some(work) = preview.next_work(true) {
            preview.execute(work);
            if preview.renderer.s3b_residents().0 == 0 {
                tx.send((
                    preview.renderer.s3b_residents().1,
                    gpu.ledger().live_bytes(),
                ))
                .unwrap();
                return;
            }
        }
    });
    let lane = start_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let expired = rx.recv_timeout(Duration::from_secs(5));
    drop(lane.shut_down()); // rescue the old idle behaviour before asserting
    worker.join().unwrap();
    let (resident_bytes, live) = expired.expect("idle worker expires residency without a request");
    assert_eq!(resident_bytes, 0, "no resident or retiring allocation");
    assert!(
        live > 0,
        "recycling retains the single original texture charge"
    );
}

#[test]
fn s3b_maintenance_parked_upload_serves_agent_then_retries() {
    let (start_tx, start_rx) = bounded(1);
    let (tx, rx) = bounded(1);
    let worker = std::thread::spawn(move || {
        let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
        let callbacks = preview.renderer.s3b_park_maps();
        let gpu = preview.renderer.display_gpu();
        assert!(
            !preview.renderer.upload_available(),
            "two real maps pending"
        );
        let document = Arc::new(title_card((64, 64), 4));
        let lane = preview.lane.clone();
        lane.post(Some(job(
            &document,
            JobKind::Paused(TimeCode::ZERO),
            stamp(1, 1),
        )));
        let (agent, answer, _) = stats_job();
        assert!(lane.try_push(agent));
        let work = preview
            .next_work(false)
            .expect("eligible agent while uploads parked");
        assert!(
            matches!(work, Work::Agent(_)),
            "transport cannot consume a pending map"
        );
        preview.execute(work);
        assert!(answer.recv_timeout(Duration::from_secs(1)).unwrap().is_ok());
        assert!(
            preview.next_work(false).is_none(),
            "paused request stays parked"
        );
        start_tx.send((lane, gpu, frames, callbacks)).unwrap();
        preview.run();
        tx.send(()).unwrap();
    });
    let (lane, gpu, frames, callbacks) = start_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    for callback in callbacks {
        callback.deliver();
    }
    let frame = frames.recv_timeout(Duration::from_secs(5));
    drop(lane.shut_down());
    rx.recv_timeout(Duration::from_secs(5))
        .expect("bounded teardown");
    worker.join().unwrap();
    assert_eq!(
        gpu.ledger().live_bytes(),
        0,
        "every actual charge returned once"
    );
    assert_eq!(
        frame
            .expect("parked request retried after late delivery")
            .at,
        TimeCode::ZERO
    );
}

#[test]
fn s3b_maintenance_paused_decoded_video_uploads_zero_bytes() {
    let workload = crate::perf_fixtures::cuts((160, 90), 12, 1, 1, 8);
    let document = Arc::new(workload.0.clone());
    let (mut preview, frames) = test_preview(Arc::new(SharedClock::new()));
    let ((), first) =
        crate::compositor::s3b_tests::uploaded_bytes(|| s3b_paused(&mut preview, &document, 1));
    let original = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    let ((), second) =
        crate::compositor::s3b_tests::uploaded_bytes(|| s3b_paused(&mut preview, &document, 2));
    let repeated = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(*original.texture.rgba, *repeated.texture.rgba);
    assert_eq!(first, 160 * 90 * 8, "actual decoded video uploaded");
    assert_eq!(second, 0, "same live reader allocation is resident");
    assert_eq!(
        preview.renderer.cache_stats().file_count,
        0,
        "reader supplied the decoded input"
    );
    let gpu = preview.renderer.display_gpu();
    drop(preview);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn s3b_maintenance_engine_before_session_drains_pending_maps() {
    use kinewright_core::Playback;
    let gpu = fallback_gpu().context();
    let ledger = gpu.shared_ledger();
    let temp = TempDirectory::new("s3b-map-engine");
    let faults = Arc::new(crate::engine::Faults::default());
    let (tx, rx) = bounded(1);
    *faults.on_agent.lock().unwrap() = Some(Box::new(move || {
        tx.send(crate::compositor::s3b_tests::delay_next_map())
            .unwrap();
    }));
    let engine =
        FfmpegMediaEngine::new_for_harness(gpu, temp.root().into(), None, faults, Arc::default())
            .unwrap();
    kinewright_core::Analysis::cache_inventory(&engine);
    let map = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut session = engine.enable_display(crate::display::tests::config());
    engine.set_document(Arc::new(title_card((64, 64), 4)));
    let deadline = Instant::now() + Duration::from_secs(10);
    let frame = loop {
        if let Some(frame) = session.reserve() {
            break frame;
        }
        assert!(Instant::now() < deadline, "actual native render");
        std::thread::sleep(Duration::from_millis(5));
    };
    session.bind(1, frame);
    let delivery = map
        .recv_timeout(Duration::from_secs(5))
        .expect("registered real map outstanding");
    let finished = engine.finished();
    drop(engine);
    drop(session); // engine went first, its consumer hands back its bound lease
    let premature = finished.recv_timeout(Duration::from_millis(100));
    let held = ledger.live_bytes();
    delivery(); // rescue before assertions, also on a regression
    let done = finished.recv_timeout(Duration::from_secs(5));
    assert_eq!(
        premature,
        Err(crossbeam_channel::RecvTimeoutError::Timeout),
        "registered map must settle before teardown"
    );
    assert!(held >= 64 * 64 * 8, "real pending staging charge retained");
    assert_eq!(done, Err(crossbeam_channel::RecvTimeoutError::Disconnected));
    assert_eq!(ledger.live_bytes(), 0, "exact charges returned");
}
