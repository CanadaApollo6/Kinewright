#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
use super::*;
use crate::{
    Compositor, CompositorLayer, LayerMode, frame::WorkingFrame, timeline::TransitionRenderParams,
};
use kinewright_core::BlendMode;
fn config() -> DisplayConfig {
    DisplayConfig {
        premultiply: (0..=u16::MAX)
            .map(|i| ((f32::from(i & 255) * (f32::from(i >> 8) / 255.0)) + 0.5) as u8)
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .unwrap(),
        repaint: Arc::new(|| {}),
    }
}
fn gpu() -> GpuContext {
    let hardware = std::env::var("S3A_HARDWARE").ok().as_deref() == Some("1");
    let gpu = GpuContext::headless(!hardware).expect("a real device is required");
    println!("S3A adapter={:?}", gpu.monitor_proof_metadata());
    assert_eq!(
        gpu.monitor_proof_metadata().software_fallback,
        !hardware,
        "correctness lane identity"
    );
    if let Ok(expected) = std::env::var("S3A_EXPECT_ADAPTER") {
        assert!(
            gpu.monitor_proof_metadata().adapter.contains(&expected),
            "required adapter: {expected}"
        );
    }
    gpu
}
#[test]
fn i5_exact_self_check_requires_gpu() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (session, shared) = DisplayPool::session(&config());
    let pool = DisplayPool::new(gpu, &config(), shared);
    assert!(
        pool.is_ok(),
        "runtime GPU route: {:#?}",
        pool.as_ref().err()
    );
    assert_eq!(
        session.status(),
        DisplayStatus::Gpu,
        "CPU fallback is not parity evidence"
    );
    assert_eq!(
        pool.as_ref()
            .unwrap()
            .encoder
            .executions
            .load(Ordering::Relaxed),
        3
    );
    drop(pool);
    assert_eq!(ledger.live_bytes(), 0);
}
fn conservation(pool: &DisplayPool, session: &DisplaySession, reserved: &[usize]) {
    let state = pool.shared.lock();
    let mut ids = pool.free.clone();
    ids.extend(&state.returned);
    ids.extend(reserved);
    ids.extend(state.ready.as_ref().map(|f| f.allocation_id));
    ids.extend(session.bound.as_ref().map(|f| f.allocation_id));
    ids.extend(session.retiring.as_ref().map(|(_, f)| f.allocation_id));
    ids.sort_unstable();
    assert_eq!(ids, [0, 1, 2], "all three leases exist exactly once");
}
#[test]
fn i16_epochs_conserve_retained_leases_and_terminal() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let repaints = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&repaints);
    let mut cfg = config();
    cfg.repaint = Arc::new(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let (session, shared) = DisplayPool::session(&cfg);
    let mut pool = DisplayPool::new(gpu, &cfg, Arc::clone(&shared)).unwrap();
    let mut session = session; // disconnect before the pool on assertion unwind
    let stamp = FrameStamp::default();
    conservation(&pool, &session, &[]);
    let a = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    conservation(&pool, &session, &[a]);
    session.bind(1, pool.candidate(a, TimeCode(0), stamp));
    conservation(&pool, &session, &[]);
    let b = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    conservation(&pool, &session, &[b]);
    session.bind(1, pool.candidate(b, TimeCode(1), stamp));
    assert!(!session.can_bind(1));
    assert_eq!(
        repaints.load(Ordering::Relaxed),
        3,
        "initialization plus two binds repaint"
    );
    let c = pool.acquire((16, 8), Duration::ZERO, || false).unwrap();
    conservation(&pool, &session, &[c]);
    session.defer(pool.candidate(c, TimeCode(2), stamp));
    let retained = [
        pool.slots[a].as_ref().unwrap().texture.clone(),
        pool.slots[b].as_ref().unwrap().texture.clone(),
    ];
    conservation(&pool, &session, &[]);
    session.advance(1);
    assert_eq!(&*pool.slots[a].as_ref().unwrap().texture, &retained[0]);
    assert_eq!(&*pool.slots[b].as_ref().unwrap().texture, &retained[1]);
    assert!(
        session.retiring.is_some(),
        "same epoch cannot release the old binding"
    );
    conservation(&pool, &session, &[]);
    drop(retained);
    assert!(
        pool.acquire((8, 8), Duration::from_millis(2), || false)
            .is_none()
    );
    session.advance(2);
    let ready = session.reserve().unwrap();
    conservation(&pool, &session, &[ready.allocation_id]);
    session.bind(2, ready);
    for e in 3..103 {
        session.advance(e);
        conservation(&pool, &session, &[]);
    }
    let d = pool.acquire((32, 8), Duration::ZERO, || false).unwrap();
    pool.abandon(d);
    assert!([a, b].contains(&d));
    let retry = pool.acquire((32, 8), Duration::ZERO, || false).unwrap();
    assert_eq!(retry, d, "paused retry reuses the released identity");
    let reserved = pool.candidate(retry, TimeCode(4), stamp);
    session.terminal();
    session.defer(reserved);
    assert!(
        session.reserve().is_none(),
        "Terminal refuses deferred candidates"
    );
    session.terminal();
    conservation(&pool, &session, &[]);
    session.advance(1000);
    assert!(session.bound.is_none() && session.retiring.is_none());
    drop(pool);
    assert_eq!(
        ledger.live_bytes(),
        0,
        "post-release completion owns the charges"
    );
}
#[test]
fn i6_gpu_encode_refuses_sticky_nonfinite_flags() {
    let gpu = gpu();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), shared).unwrap();
    assert_eq!(session.status(), DisplayStatus::Gpu);
    let compositor = Compositor::new(gpu);
    for (source, backdrop, blend) in [
        (40000.0, 40000.0, BlendMode::Add),
        (256.0, 256.0, BlendMode::Multiply),
        (f32::NAN, 0.5, BlendMode::Add),
        (f32::INFINITY, 0.5, BlendMode::Screen),
    ] {
        let frame = |v| WorkingFrame {
            width: 1,
            height: 1,
            pixels: Arc::new(vec![v, v, v, 1.0].into_iter().map(f16::from_f32).collect()),
        };
        let (d, s, cover) = (frame(backdrop), frame(source), frame(0.5));
        let layer = |frame, blend| CompositorLayer {
            frame,
            effects: &[],
            transition: TransitionRenderParams::default(),
            mode: LayerMode {
                blend,
                role: crate::compositor::LayerRole::Pixels,
            },
        };
        let layers = [
            layer(&d, BlendMode::Normal),
            layer(&s, blend),
            layer(&cover, BlendMode::Normal),
        ];
        let id = pool.acquire((1, 1), Duration::ZERO, || false).unwrap();
        let (encoder, slot) = pool.slot(id);
        let result = compositor.render_display((1, 1), &layers, None, encoder, slot);
        pool.abandon(id);
        println!(
            "S3A_I6 gpu_executions={}",
            pool.encoder.executions.load(Ordering::Relaxed)
        );
        assert!(matches!(
            result,
            Err(MediaError::NonFiniteRender { layer: 1, .. })
        ));
        assert!(session.reserve().is_none());
    }
    assert_eq!(pool.encoder.executions.load(Ordering::Relaxed), 7);
    assert_eq!(compositor.full_frame_readbacks(), 0);
}

#[test]
fn i16_release_terminal_and_disconnect_wake_real_waits() {
    for terminal in [false, true] {
        let gpu = gpu();
        let ledger = gpu.shared_ledger();
        let (session, shared) = DisplayPool::session(&config());
        let mut pool = DisplayPool::new(gpu, &config(), Arc::clone(&shared)).unwrap();
        let mut session = session;
        let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
        session.bind(1, pool.candidate(id, TimeCode(0), FrameStamp::default()));
        let (reached, observed) = std::sync::mpsc::channel();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        *shared.before_wait.lock().unwrap() = Some((reached, Arc::clone(&barrier)));
        let (done, completion) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            drop(pool);
            done.send(()).unwrap();
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(ledger.live_bytes() >= 192 * 1024 + 8 * 8 * 4);
        barrier.wait();
        if terminal {
            drop(session);
        } else {
            session.bound.take();
        }
        let woke = completion.recv_timeout(Duration::from_millis(500)).is_ok();
        shared.release.notify_all(); // drain a failed negative control before panicking
        thread.join().unwrap();
        assert!(woke, "a real waiter missed the release/Terminal wakeup");
        assert_eq!(ledger.live_bytes(), 0);
    }
}
#[test]
fn i16_device_loss_resolves_completion_before_uncharge() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), shared).unwrap();
    let mut session = session;
    for epoch in 0..2 {
        let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
        session.bind(
            epoch,
            pool.candidate(id, TimeCode(0), FrameStamp::default()),
        );
    }
    let writing = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    conservation(&pool, &session, &[writing]);
    let index = gpu.queue.submit([]);
    gpu.device.destroy();
    assert!(ledger.live_bytes() > 192 * 1024);
    complete(&gpu, index);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        session.terminal();
        pool.abandon(writing);
        drop(pool);
        tx.send(()).unwrap();
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "device-loss completion must drain before uncharge"
    );
    worker.join().unwrap();
    assert_eq!(ledger.live_bytes(), 0);
}

#[test]
fn g6_broken_table_falls_back_and_completion_releases_scratch() {
    use crate::render::{DecodeStrategy, FrameRenderer, RenderScale};
    let gpu = gpu();
    let cfg = config();
    let doc = crate::mo2_fixtures::document(vec![crate::mo2_fixtures::solid(
        1,
        [180, 90, 45],
        BlendMode::Normal,
        vec![],
    )]);
    let mut renderer = FrameRenderer::new(gpu.clone());
    let render = |r: &mut FrameRenderer| {
        r.render_live(
            &doc,
            TimeCode(0),
            (8, 8),
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .unwrap()
    };
    let expected = render(&mut renderer);
    let baseline = gpu.ledger().live_bytes();
    let (session, shared) = DisplayPool::session(&cfg);
    let result = DisplayEncoder::with_table(gpu.clone(), &cfg, |bytes| bytes[0x3800] ^= 1);
    let pool = DisplayPool::initialized(result, shared);
    assert!(
        pool.is_err(),
        "broken RGB entry must fail exact opaque self-check"
    );
    assert_eq!(session.status(), DisplayStatus::Cpu);
    assert_eq!(
        gpu.ledger().live_bytes(),
        baseline,
        "failed self-check retires every scratch charge"
    );
    assert_eq!(
        render(&mut renderer).rgba,
        expected.rgba,
        "CPU bytes unchanged after fallback"
    );
    drop(renderer);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn i5_production_workloads_match_ten_frames_exactly() {
    use crate::{
        engine::monitor_max_width,
        perf_fixtures::Workload,
        render::{DecodeStrategy, FrameRenderer, RenderScale},
    };
    let gpu = gpu();
    let cfg = config();
    let (session, shared) = DisplayPool::session(&cfg);
    let mut pool = DisplayPool::new(gpu.clone(), &cfg, shared).unwrap();
    assert_eq!(
        session.status(),
        DisplayStatus::Gpu,
        "runtime GPU route required"
    );
    let workloads = crate::pf1_harness::play_workloads();
    assert_eq!(workloads.len(), 6, "I5 requires every original W workload");
    for (name, Workload(document, _media)) in workloads {
        let scale = RenderScale::Proxy {
            max_width: monitor_max_width(document.resolution),
        };
        let dims = scale.output_resolution(document.resolution);
        let mut renderer = FrameRenderer::new(gpu.clone());
        for i in 0..10 {
            let at = TimeCode(i * (document.duration.0 - 1) / 9);
            let cpu = renderer
                .render_live(&document, at, dims, scale, DecodeStrategy::Seek)
                .unwrap();
            let before = gpu.full_frame_readbacks();
            let id = pool.acquire(dims, Duration::ZERO, || false).unwrap();
            renderer
                .render_display(
                    &document,
                    at,
                    dims,
                    (scale, None, DecodeStrategy::Seek),
                    pool.slot(id),
                )
                .unwrap();
            assert_eq!(
                gpu.full_frame_readbacks(),
                before,
                "display route: {name} {at:?}"
            );
            let actual = read_texture(&gpu, &pool.slot(id).1.texture).unwrap();
            let expected: Vec<_> = cpu
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let pre = |v| cfg.premultiply[usize::from(p[3]) * 256 + usize::from(v)];
                    [pre(p[0]), pre(p[1]), pre(p[2]), p[3]]
                })
                .collect();
            pool.abandon(id);
            assert_eq!(actual.len(), expected.len());
            assert_eq!(
                actual.iter().zip(&expected).position(|(a, b)| a != b),
                None,
                "{name} frame {at:?}: first unequal byte"
            );
        }
        println!("S3A_I5 workload={name} frames=10 route=GPU exact=true");
    }
    drop(pool);
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn i16_post_release_submission_keeps_exact_charges_until_observed() {
    let gpu = gpu();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), shared).unwrap();
    let baseline = gpu.ledger().live_bytes();
    let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.abandon(id);
    let expected = baseline + 8 * 8 * 4 + 4;
    assert_eq!(gpu.ledger().live_bytes(), expected);
    drop(session);
    let polls = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&polls);
    let ledger = gpu.shared_ledger();
    let held = Arc::new(AtomicU64::new(expected));
    let minimum = Arc::clone(&held);
    crate::compositor::ledger_probes::with_hook(
        move |_, wait| {
            minimum.fetch_min(ledger.live_bytes(), Ordering::Relaxed);
            let named = matches!(
                wait,
                wgpu::PollType::Wait {
                    submission_index: Some(_),
                    timeout: None
                }
            );
            if !named {
                minimum.store(0, Ordering::Relaxed);
            }
            let first = observed.fetch_add(1, Ordering::Relaxed) == 0;
            first.then_some(Err(wgpu::PollError::Timeout))
        },
        || drop(pool),
    );
    assert_eq!(
        held.load(Ordering::Relaxed),
        expected,
        "release token owns every charge until observed"
    );
    assert!(
        polls.load(Ordering::Relaxed) >= 2,
        "poll error cannot authorize release"
    );
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn i16_resize_keeps_old_allocation_charged_through_release_completion() {
    let gpu = gpu();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), shared).unwrap();
    let baseline = gpu.ledger().live_bytes();
    let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.abandon(id);
    let expected = baseline + 8 * 8 * 4 + 4;
    let polls = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&polls);
    let held = Arc::new(AtomicBool::new(true));
    let retained = Arc::clone(&held);
    let ledger = gpu.shared_ledger();
    let resized = crate::compositor::ledger_probes::with_hook(
        move |_, wait| {
            if ledger.live_bytes() != expected
                || !matches!(
                    wait,
                    wgpu::PollType::Wait {
                        submission_index: Some(_),
                        timeout: None
                    }
                )
            {
                retained.store(false, Ordering::Relaxed);
            }
            (observed.fetch_add(1, Ordering::Relaxed) == 0).then_some(Err(wgpu::PollError::Timeout))
        },
        || pool.acquire((16, 8), Duration::ZERO, || false).unwrap(),
    );
    pool.abandon(resized);
    assert_eq!(resized, id, "resize preserves the lease identity");
    assert!(
        held.load(Ordering::Relaxed),
        "old allocation survives every release poll"
    );
    assert!(polls.load(Ordering::Relaxed) >= 2);
    assert_eq!(gpu.ledger().live_bytes(), baseline + 16 * 8 * 4 + 4);
    drop((session, pool));
    assert_eq!(gpu.ledger().live_bytes(), 0);
}

#[test]
fn i6_transport_gpu_failure_keeps_the_issued_stamp() {
    use crate::{
        FfmpegMediaEngine,
        mo2_fixtures::{document, effect, solid},
        test_support::TempDirectory,
    };
    use kinewright_core::{MediaEvent, Playback};
    let gpu = gpu();
    let temp = TempDirectory::new("s3a-refusal");
    let engine =
        FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu.clone(), temp.root().into()).unwrap();
    let session = engine.enable_display(config());
    let boosts = (1..=4)
        .map(|id| effect(id, "primary_correction", &[("exposure_milli_stops", 5000)]))
        .collect();
    let doc = document(vec![solid(1, [255; 3], BlendMode::Normal, boosts)]);
    let events = engine.events();
    engine.set_document(Arc::new(doc));
    engine.seek(TimeCode(3));
    let issued = engine.stamp();
    let deadline = Instant::now() + Duration::from_secs(60);
    let (stamp, error) = loop {
        assert!(Instant::now() < deadline, "stamped refusal deadline");
        assert!(
            session.reserve().is_none(),
            "non-finite frame was published"
        );
        if let Ok(MediaEvent::StampedError(stamp, error)) =
            events.recv_timeout(Duration::from_millis(20))
            && stamp.epoch == issued.epoch
        {
            break (stamp, error);
        }
    };
    assert_eq!(
        session.status(),
        DisplayStatus::Gpu,
        "observed production GPU route"
    );
    assert_eq!(
        gpu.full_frame_readbacks(),
        0,
        "refusal executed on the GPU route"
    );
    assert_eq!(stamp, issued);
    assert_eq!(
        error,
        MediaError::NonFiniteRender {
            layer: 0,
            clip: Some(kinewright_core::ClipId(1)),
            at: Some(TimeCode(3))
        }
    );
    assert!(session.reserve().is_none());
    drop(session);
    let finished = engine.finished();
    drop(engine);
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(30)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    assert_eq!(gpu.ledger().live_bytes(), 0);
}
#[test]
fn i16_receiver_disconnect_retires_display_and_continues_cpu_preview() {
    use crate::{FfmpegMediaEngine, test_support::TempDirectory};
    use kinewright_core::{Document, Playback};
    let gpu = gpu();
    let temp = TempDirectory::new("s3a-disconnect");
    let engine =
        FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu.clone(), temp.root().into()).unwrap();
    let mut session = engine.enable_display(config());
    let document = Document {
        resolution: (64, 64),
        ..Document::default()
    };
    engine.set_document(Arc::new(document));
    let deadline = Instant::now() + Duration::from_secs(60);
    let frame = loop {
        if let Some(frame) = session.reserve() {
            break frame;
        }
        assert!(Instant::now() < deadline, "production display deadline");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(session.status(), DisplayStatus::Gpu);
    session.bind(0, frame);
    drop(session); // no on_exit; the engine remains alive
    let frames = engine.frames();
    engine.seek(TimeCode(1));
    let issued = engine.stamp();
    let cpu = frames.recv_timeout(Duration::from_secs(10));
    let finished = engine.finished();
    drop(engine);
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(30)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    assert_eq!(gpu.ledger().live_bytes(), 0);
    assert_eq!(
        cpu.expect("receiver disconnection must resume CPU preview")
            .stamp,
        issued
    );
}

#[test]
fn i16_paused_starvation_retries_after_a_real_lease_release() {
    use crate::{FfmpegMediaEngine, test_support::TempDirectory};
    use kinewright_core::Playback;
    let gpu = gpu();
    let temp = TempDirectory::new("s3a-starvation");
    let engine =
        FfmpegMediaEngine::new_with_gpu_and_data_dir(gpu.clone(), temp.root().into()).unwrap();
    let mut session = engine.enable_display(config());
    let mut document = crate::mo2_fixtures::document(vec![crate::mo2_fixtures::solid(
        1,
        [180, 90, 45],
        BlendMode::Normal,
        vec![],
    )]);
    document.resolution = (8, 8);
    engine.set_document(Arc::new(document));
    let events = engine.events();
    let next = |session: &DisplaySession, at: TimeCode| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(frame) = session.reserve() {
                println!(
                    "S3A_PAUSED candidate={:?}/{:?} latest={:?}",
                    frame.at,
                    frame.stamp,
                    engine.stamp()
                );
                if frame.at == at && frame.stamp == engine.stamp() {
                    return frame;
                }
            }
            for event in events.try_iter() {
                assert!(event.error().is_none(), "paused display error: {event:?}");
            }
            assert!(
                Instant::now() < deadline,
                "paused display deadline at {at:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    session.bind(0, next(&session, TimeCode(0)));
    engine.seek(TimeCode(1));
    session.bind(0, next(&session, TimeCode(1)));
    engine.seek(TimeCode(2));
    let reserved = next(&session, TimeCode(2));
    assert_eq!(session.status(), DisplayStatus::Gpu);
    engine.seek(TimeCode(3));
    let issued = engine.stamp();
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.stats().slot_starved == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let starved = engine.stats().slot_starved;
    assert!(
        session.reserve().is_none(),
        "every lease is held during starvation"
    );
    drop(reserved);
    let retried = next(&session, TimeCode(3));
    assert_eq!(
        retried.stamp, issued,
        "paused retry preserves its issued identity"
    );
    drop(retried);
    session.terminal();
    let finished = engine.finished();
    drop((session, engine));
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(30)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    assert_eq!(gpu.ledger().live_bytes(), 0);
    assert!(
        starved > 0,
        "the real preview exhausted its two-interval wait"
    );
}
