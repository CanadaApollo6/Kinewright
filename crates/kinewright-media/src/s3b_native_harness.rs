// Shared harness-only R75 comparison; included inside pf1_harness on both trees.
fn s4_device(gpu: &GpuContext) -> serde_json::Value {
    let info = gpu.selected_adapter();
    serde_json::json!({
        "selected": {"backend": format!("{:?}", info.backend), "name": info.name,
            "vendor_id": info.vendor, "device_id": info.device, "driver": info.driver},
        "driver_info": info.driver_info,
        "software": info.device_type == wgpu::DeviceType::Cpu
    })
}

fn s4_publish(gpu: &GpuContext, measured: Option<(bool, f64)>) {
    s4_publish_metrics(
        gpu,
        serde_json::json!({"name": "native-display", "poll_ms": 5,
            "warmup_ms": 2000, "duration_s": 60}),
        measured.map(|(valid, fps)| (valid, serde_json::json!({"fps": fps}))),
    );
}

fn s4_publish_metrics(
    gpu: &GpuContext,
    protocol: serde_json::Value,
    measured: Option<(bool, serde_json::Value)>,
) {
    if let Ok(path) = std::env::var("PF1_RECORD") {
        let mut record = s4_device(gpu);
        S4_INPUT.with_borrow(|slot| {
            if let Some(consumed) = slot {
                record["consumed"] = consumed.clone();
            }
        });
        record["token"] = std::env::var("PF1_TOKEN").expect("collector token").into();
        record["protocol"] = protocol;
        record["timestamp_s"] = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            .into();
        if let Some((valid, metrics)) = measured {
            if record["protocol"]["name"] != "P-rss" {
                record["valid"] = valid.into();
            }
            record["metrics"] = metrics;
        }
        let temporary = format!("{path}.tmp");
        std::fs::write(&temporary, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::rename(temporary, path).unwrap();
    }
}

#[test]
fn s4_selected_device_readback() {
    let gpu = GpuContext::headless(true).expect("software correctness adapter");
    let record = s4_device(&gpu);
    let info = gpu.selected_adapter();
    assert_eq!(record["selected"]["name"], info.name);
    let backend = record["selected"]["backend"].as_str().unwrap();
    assert!(["Vulkan", "Dx12", "Metal", "Gl"].contains(&backend));
    assert_eq!(record["selected"]["driver"], info.driver);
    assert_eq!(record["selected"]["vendor_id"], info.vendor);
    assert_eq!(record["driver_info"], info.driver_info);
    assert_eq!(
        record["software"],
        info.device_type == wgpu::DeviceType::Cpu
    );
}

fn s3b_native_consume(
    native: &mut crate::DisplaySession,
    session: &Session,
    epoch: &mut u64,
    dimensions: (u32, u32),
) -> Option<(bool, i64, i64)> {
    native.advance(*epoch);
    let result = native.reserve().and_then(|frame| {
        assert_eq!(
            frame.dimensions, dimensions,
            "canonical P-1 native preview raster"
        );
        let position = session.engine.position().0;
        let stamp = session.engine.stamp();
        if frame.stamp.epoch != stamp.epoch {
            return None;
        }
        if frame.at.0 > position {
            native.defer(frame);
            return None;
        }
        // Match the live consumer's current-frame fence; early-only and stale do not count.
        if frame.at.0 != position {
            return None;
        }
        if !native.can_bind(*epoch) {
            native.defer(frame);
            return None;
        }
        let (at, stamp) = (frame.at, frame.stamp);
        native.bind(*epoch, frame);
        assert_eq!(
            native.status(),
            crate::DisplayStatus::Gpu,
            "bound native route"
        );
        // Match PaintMarker: a clock advance between bind and paint is late.
        let painted_position = session.engine.position().0;
        if session.engine.stamp().epoch != stamp.epoch {
            return None;
        }
        let expired = painted_position > at.0;
        let presented = Instant::now();
        session.engine.ack_presented(stamp, at, presented, expired);
        Some((expired, at.0, painted_position))
    });
    *epoch += 1;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "one native timing lifecycle shared by both sides"
)]
fn s3b_native_run(document: &Document) -> (PlayMetrics, f64, String) {
    if std::env::var_os("PF1_RECORD").is_some() {
        assert!((document.duration.0 as f64 * frame_ms(document) - 60_000.0).abs() < 1e-5);
    }
    let audio = SimulatedAudio::paced();
    let session = Session::new(Some(audio.clone()), Arc::default(), lane().0);
    s4_publish(&session.gpu, None);
    let metadata = session.gpu.monitor_proof_metadata();
    assert_eq!(
        metadata.software_fallback,
        !lane().0,
        "the requested LL/LH adapter route"
    );
    if lane().0 {
        assert!(
            metadata.adapter.contains("RTX 3090"),
            "LH uses the desktop 3090"
        );
    }
    let dimensions = crate::render::RenderScale::Proxy {
        max_width: crate::engine::monitor_max_width(document.resolution),
    }
    .output_resolution(document.resolution);
    println!(
        "S3B_ADAPTER lane={} metadata={metadata:?} dimensions={dimensions:?}",
        lane().1
    );
    let mut native = session
        .engine
        .enable_display(crate::display::tests::config());
    session.engine.set_document(Arc::new(document.clone()));
    let until = Instant::now() + Duration::from_secs(120);
    let mut epoch = 0;
    while s3b_native_consume(&mut native, &session, &mut epoch, dimensions).is_none() {
        assert!(Instant::now() < until, "native initialization completed");
        session.assert_no_error("initialize");
        thread::sleep(SAMPLE);
    }
    assert_eq!(native.status(), crate::DisplayStatus::Gpu);
    session.events.try_iter().for_each(drop);
    session.engine.play(TimeCode::ZERO);
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(2) {
        s3b_native_consume(&mut native, &session, &mut epoch, dimensions);
        drop(native.timings());
        thread::sleep(SAMPLE);
    }
    session.engine.pause();
    let pause_until = Instant::now() + Duration::from_secs(30);
    loop {
        let event = session
            .events
            .recv_deadline(pause_until)
            .expect("warm-up paused");
        assert!(event.error().is_none(), "warm-up error: {event:?}");
        if event == MediaEvent::PlaybackStateChanged(PlaybackState::Paused) {
            break;
        }
    }
    let until = Instant::now() + Duration::from_secs(120);
    while s3b_native_consume(&mut native, &session, &mut epoch, dimensions).is_none() {
        assert!(Instant::now() < until, "paused warm-up render finished");
        thread::sleep(SAMPLE);
    }
    drop(native.timings());
    let before = session.gpu.full_frame_readbacks();
    let missed = audio.missed_periods();
    let (duration, frame) = (document.duration.0, frame_ms(document));
    let nominal = duration as f64 * frame;
    let upper = 1.02 * nominal + 500.0;
    let work_before = pf1_clock::snapshot();
    let start = Instant::now();
    session.engine.play(TimeCode::ZERO);
    let mut trace = Trace::default();
    let mut acknowledged = BTreeSet::new();
    let mut received_acks = BTreeSet::new();
    let mut expired_acks = BTreeSet::new();
    let mut next = start;
    loop {
        let position = session.engine.position().0;
        let t = ms(start);
        trace.samples.push((t, position));
        if let Some(ack) = s3b_native_consume(&mut native, &session, &mut epoch, dimensions) {
            s3b_record_ack(
                ack,
                ms(start),
                duration,
                &mut trace,
                &mut received_acks,
                &mut acknowledged,
                &mut expired_acks,
            );
        }
        drop(native.timings());
        session.assert_no_error("native P-play");
        if trace.end.is_none() && position >= duration {
            trace.end = Some(t);
        }
        if trace.end.is_some() || t > upper + 5_000.0 {
            break;
        }
        next += SAMPLE;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let elapsed = trace.end.unwrap_or(f64::NAN);
    let missed = audio.missed_periods() - missed;
    let mut metrics = s3b_native_metrics(&trace, duration, frame, &expired_acks);
    metrics.valid = q2_valid(elapsed, nominal, missed) && trace.end.is_some();
    let fps = acknowledged.len() as f64 / (elapsed / 1000.0);
    let stats = session.engine.stats();
    let raw = pf1_clock::snapshot();
    let counters =
        std::array::from_fn::<_, { pf1_clock::LEN }, _>(|index| raw[index] - work_before[index]);
    let line = format!(
        "route=native poll_ms=5 valid={} elapsed_s={:.6} missed_callbacks={} due={} acknowledged={} expired_acks={} fps={:.6} on_time={} late={} early={} dropped={} present_p95_ms={:.6} scheduled_conversions_t={} scheduled_conversions_w={} decoded={} {}",
        metrics.valid,
        elapsed / 1000.0,
        missed,
        duration,
        acknowledged.len(),
        expired_acks.len(),
        fps,
        metrics.on_time,
        metrics.late,
        metrics.early,
        metrics.dropped,
        metrics.present[1],
        counters[pf1_clock::CONVERT_T],
        counters[pf1_clock::CONVERT_W],
        counters[pf1_clock::DECODED],
        engine_fields(&stats)
    );
    native.terminal();
    let finished = session.engine.finished();
    let gpu = session.gpu.clone();
    drop(session);
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(30)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected),
        "bounded native teardown"
    );
    assert_eq!(
        gpu.full_frame_readbacks(),
        before,
        "no CPU monitor fallback/readback"
    );
    drop(native);
    assert_eq!(gpu.ledger().live_bytes(), 0, "every charge returned");
    s4_publish(&gpu, Some((metrics.valid, fps)));
    (metrics, fps, line)
}

#[test]
#[ignore = "S3b R75 native P-play timing lane; isolated release with recorded load"]
fn s3b_native_play_lane() {
    assert_release();
    let (hardware, lane) = lane();
    let runs: usize = std::env::var("PF1_RUNS").map_or(3, |s| s.parse().unwrap());
    let offset: usize = std::env::var("S3B_RUN_OFFSET").map_or(0, |s| s.parse().unwrap());
    assert!(
        (1..=3).contains(&runs) && offset + runs <= 3,
        "bounded R75 run indices"
    );
    let reference = std::env::var("S3B_SIDE").ok().as_deref() == Some("reference");
    let collecting = std::env::var_os("PF1_RECORD").is_some();
    let workloads = play_workloads();
    assert_eq!(workloads.len(), 1, "one workload per bounded lane");
    let mut failures = Vec::new();
    for (key, Workload(document, _media)) in workloads {
        for run in offset..offset + runs {
            pf1_clock::start_run();
            let (metrics, fps, line) = s3b_native_run(&document);
            println!("S3B_NATIVE lane={lane} workload={key} run={run} {line}");
            let floor = match key {
                "typical_1080p" | "explainer_16x9" => 24.0,
                "blend_heavy_1080p" | "reel_9x16" => 15.0,
                _ => f64::NAN,
            };
            if !s4_measurement_only(collecting, reference, hardware, &metrics, fps, floor) {
                failures.push(format!("{key}/{run}: invalid Q2 or absolute gate miss"));
            }
        }
    }
    assert!(failures.is_empty(), "R75 native gates: {failures:?}");
}

fn s4_measurement_only(
    collecting: bool,
    reference: bool,
    hardware: bool,
    metrics: &PlayMetrics,
    fps: f64,
    floor: f64,
) -> bool {
    collecting
        || (metrics.valid
            && (reference
                || if hardware {
                    metrics.g1_metric()
                } else {
                    fps.is_finite() && fps >= floor
                }))
}

#[test]
fn s4_shared_misses_reach_the_paired_decision() {
    let mut metrics = s3b_native_metrics(&Trace::default(), 300, 33.333, &BTreeSet::new());
    metrics.valid = true;
    for reference in [false, true] {
        assert!(s4_measurement_only(
            true, reference, false, &metrics, 10.0, 24.0
        ));
        assert_eq!(
            s4_measurement_only(false, reference, false, &metrics, 10.0, 24.0),
            reference
        );
    }
    metrics.valid = false;
    assert!(!s4_measurement_only(
        false, true, false, &metrics, 30.0, 24.0
    ));
}

// R57's unchanged P-seek operation/seed protocol, split only to interleave pairs.
#[test]
#[ignore = "S3b D12 R57 working-step paired lane; isolated release with recorded load"]
fn s3b_d12_working_step_lane() {
    assert_release();
    let offset: u64 = std::env::var("S3B_RUN_OFFSET").unwrap().parse().unwrap();
    assert!(offset < 3);
    let Workload(document, _media) = perf_fixtures::seek_gop60();
    let line = seek_run(&document, 0x5EED_0000 + offset);
    println!(
        "PF1 seek lane={} workload=seek_gop60 run={offset} {line}",
        lane().1
    );
    assert_idle_discard(&line);
    let timeouts = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("timeouts="))
        .unwrap();
    assert_eq!(timeouts, "0", "every setup/step/reposition must finish");
    // Work counters must exist and include real conversions on both sides.
    assert!(line.contains("conv_t_n=") && line.contains("conv_w_n="));
}

#[test]
#[ignore = "S3b I4 paired R28 30+300 frame lane; isolated release with recorded load"]
fn s3b_i4_lane() {
    assert_release();
    let offset: u64 = std::env::var("S3B_RUN_OFFSET").unwrap().parse().unwrap();
    assert!(offset < 3);
    let hardware = lane().0;
    let Workload(document, _media) = perf_fixtures::cuts((1920, 1080), 600, 3, 24, 15);
    let gpu = GpuContext::headless(!hardware).expect("the I4 adapter");
    let metadata = gpu.monitor_proof_metadata();
    assert_eq!(metadata.software_fallback, !hardware);
    if hardware {
        assert!(metadata.adapter.contains("RTX 3090"));
    }
    let run = crate::mo2_bench::run(&gpu, &document, Duration::ZERO, false);
    let peak = gpu.ledger().peak_bytes();
    println!(
        "S3B_I4 lane={} run={offset} adapter={:?} resident=false workload=typical_1080p warm_frames=30 measured_frames=300 dims={:?} mean_ms={:.6} p95_ms={:.6} ledger_peak_bytes={peak}",
        lane().1,
        metadata.adapter,
        run.dims,
        run.mean_ms,
        run.p95_ms
    );
    assert!(peak <= 384 * MIB, "R28 ledger ceiling");
    assert_eq!(
        gpu.ledger().live_bytes(),
        0,
        "R28 teardown exact ledger baseline"
    );
    let pinned = if hardware { 494.39 } else { 497.86 };
    assert!(
        run.mean_ms / pinned <= 1.05,
        "I4 unchanged pre-MO2 criterion"
    );
}

fn s3b_record_ack(
    (expired, at, position): (bool, i64, i64),
    time: f64,
    duration: i64,
    trace: &mut Trace,
    received: &mut BTreeSet<i64>,
    acknowledged: &mut BTreeSet<i64>,
    expired_acks: &mut BTreeSet<i64>,
) {
    if trace.end.is_none() && (0..duration).contains(&at) && received.insert(at) {
        trace.arrivals.push((time, at, position));
        if expired {
            expired_acks.insert(at);
        } else if position == at {
            acknowledged.insert(at);
        }
    }
}

#[test]
fn s3b_native_ack_count_excludes_expired_early_repeated_and_outside_window() {
    let mut trace = Trace::default();
    let mut received = BTreeSet::new();
    let mut acknowledged = BTreeSet::new();
    let mut expired = BTreeSet::new();
    for (time, ack) in [
        (1.0, (false, 5, 5)),
        (2.0, (false, 5, 5)),
        (3.0, (true, 6, 7)),
        (4.0, (false, 7, 6)),
        (5.0, (true, 4, 8)),
        (6.0, (false, 10, 10)),
    ] {
        s3b_record_ack(
            ack,
            time,
            10,
            &mut trace,
            &mut received,
            &mut acknowledged,
            &mut expired,
        );
    }
    assert_eq!(acknowledged, BTreeSet::from([5]));
    assert_eq!(expired, BTreeSet::from([4, 6]));
    assert_eq!(
        trace.arrivals.len(),
        4,
        "late acknowledgements remain in G2's fixed-denominator trace"
    );
    trace.end = Some(6.0);
    s3b_record_ack(
        (false, 8, 8),
        7.0,
        10,
        &mut trace,
        &mut received,
        &mut acknowledged,
        &mut expired,
    );
    assert_eq!(acknowledged, BTreeSet::from([5]));
    assert_eq!(
        trace.arrivals.len(),
        4,
        "no presentation after the measured window"
    );
    println!(
        "S3B_NATIVE_ACK eligible={} expired={} trace={}",
        acknowledged.len(),
        expired.len(),
        trace.arrivals.len()
    );
}

fn s3b_native_metrics(
    trace: &Trace,
    due: i64,
    frame_ms: f64,
    expired: &BTreeSet<i64>,
) -> PlayMetrics {
    let mut metrics = trace.metrics(due, frame_ms);
    let end = trace.end.unwrap_or(f64::INFINITY);
    for at in expired {
        let due_at = trace
            .samples
            .iter()
            .find(|(time, position)| *time <= end && position >= at);
        let ack = trace
            .arrivals
            .iter()
            .find(|(time, frame, _)| *time <= end && frame == at);
        if let (Some((due_at, _)), Some((painted, _, _))) = (due_at, ack)
            && painted <= &(due_at + frame_ms)
        {
            // R33: the final paint fence overrides the arrival timing tolerance.
            metrics.on_time -= 1;
            metrics.late += 1;
        }
    }
    metrics
}

#[test]
fn s3b_native_expired_paint_is_late_even_inside_timing_tolerance() {
    let mut trace = Trace {
        samples: vec![(0.0, 0), (5.0, 1)],
        arrivals: vec![(5.0, 0, 1)],
        end: Some(5.0),
    };
    let expired = BTreeSet::from([0]);
    let plain = trace.metrics(1, 33.0);
    assert_eq!(
        plain.on_time, 1,
        "the generic arrival metric's timing tolerance"
    );
    let metrics = s3b_native_metrics(&trace, 1, 33.0, &expired);
    assert_eq!(
        (
            metrics.due,
            metrics.on_time,
            metrics.late,
            metrics.early,
            metrics.dropped
        ),
        (1, 0, 1, 0, 0),
        "R33 expired paint is always late, fixed denominator unchanged"
    );
    trace.arrivals[0].0 = 40.0;
    trace.end = Some(50.0);
    let metrics = s3b_native_metrics(&trace, 1, 33.0, &expired);
    assert_eq!(
        (metrics.due, metrics.on_time, metrics.late),
        (1, 0, 1),
        "a frame already late is not counted twice"
    );
    println!(
        "S3B_NATIVE_EXPIRED due={} on_time={} late={} early={} dropped={}",
        metrics.due, metrics.on_time, metrics.late, metrics.early, metrics.dropped
    );
}
