//! R71 ordered teardown and Ready publication witnesses, adopted from the Opus probe.
use super::*;
use crate::{
    Compositor, CompositorLayer, LayerMode, frame::WorkingFrame, timeline::TransitionRenderParams,
};
use half::f16;
use std::{
    sync::{Barrier, mpsc},
    thread,
};
fn stamp(seq: u64) -> FrameStamp {
    FrameStamp { epoch: 1, seq }
}
/// C (deterministic): a pool dropped without Terminal (`Preview::drop` at
/// engine shutdown, or a replaced pool after a second `enable_display`)
/// takes Ready once; a reservation deferred after that take lands in Ready
/// forever, and the later Terminal does not transfer it.
#[test]
fn display_race_probe_c_pool_drop_vs_deferred_reservation() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), Arc::clone(&shared)).unwrap();
    let mut session = session;
    let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.publish(pool.candidate(id, TimeCode(0), stamp(1)));
    let reserved = session.reserve().expect("ready");
    let (reached, observed) = mpsc::channel();
    let barrier = Arc::new(Barrier::new(2));
    *shared.before_wait.lock().unwrap() = Some((reached, Arc::clone(&barrier)));
    let (done_tx, done) = mpsc::channel();
    thread::spawn(move || {
        drop(pool);
        let _ = done_tx.send(());
    });
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    barrier.wait();
    // The pool drop is now (or about to be) in its unbounded wait.
    thread::sleep(Duration::from_millis(20));
    session.defer(reserved);
    session.terminal();
    drop(session);
    let finished = done.recv_timeout(Duration::from_secs(3)).is_ok();
    let stuck_ready = shared.lock().ready.as_ref().map(|f| f.allocation_id);
    println!(
        "PROBE_C pool_drop_finished={finished} ready_still_holds={stuck_ready:?} ledger={}",
        ledger.live_bytes()
    );
    if !finished {
        // Rescue so the process can continue: drop the stranded Ready lease.
        let stranded = shared.lock().ready.take();
        drop(stranded);
        let rescued = done.recv_timeout(Duration::from_secs(3)).is_ok();
        println!(
            "PROBE_C rescued_by_taking_ready={rescued} ledger={}",
            ledger.live_bytes()
        );
    }
    assert!(
        finished,
        "pool drop hung on a lease deferred into Ready after its take"
    );
}

fn long_document() -> kinewright_core::Document {
    let mut clip = crate::mo2_fixtures::solid(1, [180, 90, 45], BlendMode::Normal, vec![]);
    clip.source_range = TimeCode(0)..TimeCode(3000);
    let document = kinewright_core::Document {
        resolution: (32, 32),
        duration: TimeCode(3000),
        tracks: vec![kinewright_core::Track {
            id: kinewright_core::TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![clip],
        }],
        ..kinewright_core::Document::default()
    };
    document.validate().unwrap();
    document
}
fn harness(gpu: &GpuContext) -> (crate::FfmpegMediaEngine, crate::test_support::TempDirectory) {
    use crate::audio::{AudioDiagnostics, simulated::SimulatedAudio};
    let temp = crate::test_support::TempDirectory::new("s3a-race");
    let engine = crate::FfmpegMediaEngine::new_for_harness(
        gpu.clone(),
        temp.root().into(),
        Some(SimulatedAudio::paced()),
        Arc::default(),
        Arc::new(AudioDiagnostics::default()),
    )
    .unwrap();
    (engine, temp)
}
/// K (deterministic): playback publishes every frame of a job with the
/// job's stamp; a deferred older reservation (same seq) replaces a newer
/// Ready frame published between reserve and defer, dropping the newer.
#[test]
fn display_race_probe_k_defer_equal_seq_replaces_newer_ready() {
    let gpu = gpu();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), shared).unwrap();
    let job = stamp(7);
    let a = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.publish(pool.candidate(a, TimeCode(10), job));
    let older = session.reserve().unwrap();
    let b = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.publish(pool.candidate(b, TimeCode(11), job));
    session.defer(older);
    let ready = session.reserve().map(|f| f.at);
    println!("PROBE_K ready_after_defer={ready:?} (newer was TimeCode(11))");
    drop((session, pool));
    assert_eq!(
        ready,
        Some(TimeCode(11)),
        "deferred older frame displaced the newer Ready frame"
    );
}

/// M (engine level, ordered): a reservation held across engine shutdown is
/// deferred after `Preview::drop`'s pool drop took Ready; Terminal does not
/// transfer Ready, so the worker never finishes.
#[test]
fn display_race_probe_m_engine_shutdown_then_defer() {
    use kinewright_core::Playback;
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (engine, _temp) = harness(&gpu);
    let mut session = engine.enable_display(config());
    engine.set_document(Arc::new(long_document()));
    engine.play(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut epoch = 1;
    let held = loop {
        assert!(Instant::now() < deadline, "no same-epoch candidate");
        session.advance(epoch);
        if let Some(f) = session.reserve() {
            if session.can_bind(epoch) {
                session.bind(epoch, f);
                if !session.can_bind(epoch) {
                    // wait for the next frame in this same epoch
                    let until = Instant::now() + Duration::from_millis(200);
                    let mut next = None;
                    while next.is_none() && Instant::now() < until {
                        next = session.reserve();
                        thread::sleep(Duration::from_micros(200));
                    }
                    if let Some(next) = next {
                        break next;
                    }
                }
            } else {
                drop(f);
            }
        }
        epoch += 1;
        thread::sleep(Duration::from_millis(2));
    };
    let shared = Arc::clone(&session.shared);
    let finished = engine.finished();
    drop(engine);
    thread::sleep(Duration::from_millis(300));
    session.defer(held); // the same-epoch deferral, after the shutdown began
    session.terminal();
    drop(session);
    let fin = finished.recv_timeout(Duration::from_secs(10));
    let stranded = shared.lock().ready.as_ref().map(|f| f.allocation_id);
    println!(
        "PROBE_M finished={fin:?} stranded_ready={stranded:?} ledger={}",
        ledger.live_bytes()
    );
    if fin.is_ok() || fin == Err(crossbeam_channel::RecvTimeoutError::Timeout) {
        let taken = shared.lock().ready.take();
        drop(taken);
        let rescued = finished.recv_timeout(Duration::from_secs(10));
        println!(
            "PROBE_M after taking Ready: finished={rescued:?} ledger={}",
            ledger.live_bytes()
        );
    }
    assert_eq!(
        fin,
        Err(crossbeam_channel::RecvTimeoutError::Disconnected),
        "worker hung in Preview::drop"
    );
}

#[test]
fn r71_terminal_refuses_reserve_and_bind() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (mut session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu, &config(), shared).unwrap();
    let a = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    let reserved = pool.candidate(a, TimeCode(0), stamp(1));
    let b = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    pool.publish(pool.candidate(b, TimeCode(1), stamp(2)));
    session.terminal();
    let after = session.reserve();
    let refused = after.is_none();
    drop(after);
    session.bind(1, reserved);
    let bound = session.bound.is_some();
    session.terminal();
    drop((session, pool));
    println!(
        "R71_F7 reserve_refused={refused} bound_after_terminal={bound} ledger={}",
        ledger.live_bytes()
    );
    assert!(
        refused && !bound,
        "Terminal must refuse both reserve and bind"
    );
    assert_eq!(ledger.live_bytes(), 0);
}
fn writing_unwind(step: &str) -> (bool, u64) {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu, &config(), Arc::clone(&shared)).unwrap();
    assert_eq!(session.status(), DisplayStatus::Gpu);
    let id = 2; // Fresh real pool: the first Free lease popped is 2.
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if step == "allocation" {
            *shared.writing_panic.lock().unwrap() = Some("allocation");
        }
        let acquired = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
        assert_eq!(acquired, id);
        if step == "candidate" {
            *shared.writing_panic.lock().unwrap() = Some("candidate");
            drop(pool.candidate(id, TimeCode(0), stamp(1)));
        } else {
            let _writing = pool.slot(id);
            panic!("R71 Writing panic");
        }
    }));
    assert!(panic.is_err());
    drop(session);
    let (tx, rx) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(pool);
        tx.send(()).unwrap();
    });
    let finished = rx.recv_timeout(Duration::from_secs(2)).is_ok();
    if !finished {
        shared.give(id);
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    dropper.join().unwrap();
    println!(
        "R71_F6 step={step} writing_unwind_finished={finished} ledger={}",
        ledger.live_bytes()
    );
    (finished, ledger.live_bytes())
}
#[test]
fn r71_writing_unwind_returns_exact_charges() {
    assert_eq!(
        writing_unwind("render"),
        (true, 0),
        "Writing unwind stranded a lease"
    );
}
#[test]
fn r71_writing_construction_unwind_returns_exact_charges() {
    let allocation = writing_unwind("allocation");
    let candidate = writing_unwind("candidate");
    assert_eq!(
        (allocation, candidate),
        ((true, 0), (true, 0)),
        "allocation/candidate unwind stranded a lease"
    );
}
#[test]
fn r71_paused_terminal_is_scheduler_work() {
    use kinewright_core::{Document, Playback};
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let (engine, _temp) = harness(&gpu);
    engine.set_document(Arc::new(Document {
        resolution: (8, 8),
        ..Document::default()
    }));
    engine
        .frames()
        .recv_timeout(Duration::from_secs(15))
        .unwrap();
    let baseline = ledger.live_bytes();
    let mut session = engine.enable_display(config());
    engine.seek(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(15);
    let frame = loop {
        if let Some(f) = session.reserve() {
            break f;
        }
        assert!(Instant::now() < deadline, "paused frame");
        thread::sleep(Duration::from_millis(2));
    };
    session.bind(1, frame);
    assert_eq!(session.status(), DisplayStatus::Gpu);
    drop(session);
    let deadline = Instant::now() + Duration::from_secs(2);
    while ledger.live_bytes() != baseline && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let live = ledger.live_bytes();
    let finished = engine.finished();
    drop(engine);
    assert_eq!(
        finished.recv_timeout(Duration::from_secs(10)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    println!(
        "R71_F2 paused_terminal_live={live} baseline={baseline} final={}",
        ledger.live_bytes()
    );
    assert_eq!(
        live, baseline,
        "idle Terminal must retire without a seek or request"
    );
}

/// G: device destroyed from a third thread while the preview thread
/// encodes real frames (Writing) and the UI thread holds Bound/Retiring.
#[test]
fn display_race_probe_g_device_destroyed_mid_encode() {
    let n = std::env::var("PROBE_G_ITERS").map_or(1, |v| v.parse().unwrap());
    let (mut hangs, mut panics, mut errors) = (0, 0, 0u64);
    for it in 0..n {
        let gpu = gpu();
        let ledger = gpu.shared_ledger();
        let (session, shared) = DisplayPool::session(&config());
        let pool = DisplayPool::new(gpu.clone(), &config(), Arc::clone(&shared)).unwrap();
        let compositor = Compositor::new(gpu.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let preview = thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut pool = pool;
                let frame = WorkingFrame {
                    width: 32,
                    height: 32,
                    pixels: Arc::new(vec![f16::from_f32(0.5); 32 * 32 * 4]),
                };
                let mut seq = 0;
                let mut errs = 0u64;
                while !stop.load(Ordering::Acquire) {
                    let Some(id) = pool.acquire((32, 32), Duration::from_millis(5), || false)
                    else {
                        if pool.terminal() {
                            break;
                        }
                        continue;
                    };
                    let catch = std::env::var("PROBE_G_CATCH").is_ok();
                    let (encoder, slot) = pool.slot(id);
                    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        compositor.render_display(
                            (32, 32),
                            &[CompositorLayer {
                                frame: &frame,
                                effects: &[],
                                transition: TransitionRenderParams::default(),
                                mode: LayerMode {
                                    blend: BlendMode::Normal,
                                    role: crate::compositor::LayerRole::Pixels,
                                },
                            }],
                            None,
                            encoder,
                            slot,
                        )
                    }));
                    let result = match attempt {
                        Ok(result) => result,
                        Err(_panic) if catch => {
                            println!("PROBE_G caught encode panic; abandoning the Writing lease");
                            pool.abandon(id);
                            break;
                        }
                        Err(panic) => std::panic::resume_unwind(panic),
                    };
                    pool.collect_timing();
                    if result.is_ok() {
                        seq += 1;
                        pool.publish(pool.candidate(id, TimeCode(seq), stamp(seq as u64)));
                    } else {
                        errs += 1;
                        pool.abandon(id);
                    }
                }
                (pool, compositor, errs)
            }
        });
        let ui_stop = Arc::new(AtomicBool::new(false));
        let ui = thread::spawn({
            let ui_stop = Arc::clone(&ui_stop);
            move || {
                let mut session = session;
                let mut epoch = 1;
                while !ui_stop.load(Ordering::Acquire) {
                    session.advance(epoch);
                    if let Some(f) = session.reserve() {
                        if session.can_bind(epoch) {
                            session.bind(epoch, f);
                        } else {
                            session.defer(f);
                        }
                    }
                    epoch += 1;
                    thread::sleep(Duration::from_micros(500));
                }
                session
            }
        });
        thread::sleep(Duration::from_millis(10));
        gpu.device.destroy();
        thread::sleep(Duration::from_millis(30));
        ui_stop.store(true, Ordering::Release);
        let session = ui.join().unwrap();
        stop.store(true, Ordering::Release);
        let wait_finished = |h: &thread::JoinHandle<_>, limit: Duration| {
            let deadline = Instant::now() + limit;
            while !h.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            h.is_finished()
        };
        let mut session = session;
        if !wait_finished(&preview, Duration::from_secs(10)) {
            // Is the preview blocked in an unwinding pool drop on app-held leases?
            session.terminal();
            let after = wait_finished(&preview, Duration::from_secs(10));
            hangs += 1;
            println!(
                "PROBE_G it={it} preview blocked until app Terminal; finished_after_terminal={after}"
            );
            if !after {
                println!("PROBE_G it={it} preview HANG even after Terminal");
                continue;
            }
        }
        let Ok((pool, compositor, errs)) = preview.join() else {
            panics += 1;
            println!("PROBE_G it={it} preview PANIC");
            drop(session);
            let deadline = Instant::now() + Duration::from_secs(2);
            while ledger.live_bytes() != 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            println!("PROBE_G it={it} ledger after panic={}", ledger.live_bytes());
            continue;
        };
        errors += errs;
        session.terminal();
        match within(Duration::from_secs(10), move || {
            drop(pool);
            drop(compositor);
        }) {
            Outcome::Done(()) => {}
            Outcome::Hang => {
                hangs += 1;
                println!("PROBE_G it={it} pool drop HANG after device loss");
                continue;
            }
            Outcome::Panicked => {
                panics += 1;
                println!("PROBE_G it={it} pool drop PANIC");
                continue;
            }
        }
        drop(session);
        drop(gpu);
        let deadline = Instant::now() + Duration::from_secs(2);
        while ledger.live_bytes() != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        if ledger.live_bytes() != 0 {
            println!("PROBE_G it={it} LEAK live={}", ledger.live_bytes());
        }
        assert_eq!(
            ledger.live_bytes(),
            0,
            "iteration {it}: exact charges after loss"
        );
    }
    println!(
        "PROBE_G iterations={n} hangs={hangs} panics={panics} post_loss_render_errors={errors}"
    );
    assert_eq!((hangs, panics), (0, 0));
}

enum Outcome<T> {
    Done(T),
    Hang,
    Panicked,
}
fn within<T: Send + 'static>(
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Outcome<T> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(v) => Outcome::Done(v),
        Err(mpsc::RecvTimeoutError::Timeout) => Outcome::Hang,
        Err(mpsc::RecvTimeoutError::Disconnected) => Outcome::Panicked,
    }
}

#[test]
fn r71_consumer_submission_retains_slot_until_release_observed() {
    let gpu = gpu();
    let ledger = gpu.shared_ledger();
    let baseline = ledger.live_bytes();
    let (mut session, shared) = DisplayPool::session(&config());
    let mut pool = DisplayPool::new(gpu.clone(), &config(), Arc::clone(&shared)).unwrap();
    let id = pool.acquire((8, 8), Duration::ZERO, || false).unwrap();
    let frame = WorkingFrame {
        width: 8,
        height: 8,
        pixels: Arc::new(vec![f16::from_f32(0.5); 8 * 8 * 4]),
    };
    let compositor = Compositor::new(gpu.clone());
    let (encoder, slot) = pool.slot(id);
    // Record the actual encode submission polled by render_display's flags map.
    let encode_fence = Arc::new(Mutex::new(None));
    let recorded_fence = Arc::clone(&encode_fence);
    crate::compositor::ledger_probes::with_hook(
        move |_, wait| {
            if let wgpu::PollType::Wait {
                submission_index: Some(index),
                ..
            } = wait
            {
                *recorded_fence.lock().unwrap() = Some(index.clone());
            }
            None
        },
        || {
            compositor
                .render_display(
                    (8, 8),
                    &[CompositorLayer {
                        frame: &frame,
                        effects: &[],
                        transition: TransitionRenderParams::default(),
                        mode: LayerMode::NORMAL,
                    }],
                    None,
                    encoder,
                    slot,
                )
                .unwrap();
        },
    );
    let encode = encode_fence
        .lock()
        .unwrap()
        .take()
        .expect("encode fence observed");
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(encode.clone()),
            timeout: None,
        })
        .unwrap();
    // Encoding and its flags map have completed. This later submission reads the slot as a texture.
    let output = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("R71 consumer"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl("@group(0) @binding(0) var image: texture_2d<f32>; @group(0) @binding(1) var<storage, read_write> value: u32; @compute @workgroup_size(1) fn main() { value = u32(round(textureLoad(image, vec2<i32>(0), 0).r * 255.0)); }".into()) });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let expected_sample = read_texture(&gpu, &pool.slot(id).1.texture).unwrap()[0];
    let candidate = pool.candidate(id, TimeCode(0), stamp(1));
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&candidate.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    session.bind(1, candidate);
    let mut commands = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = commands.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    commands.copy_buffer_to_buffer(&output, 0, &readback, 0, 4);
    let consumer = gpu.queue.submit([commands.finish()]);
    let identity = pool.slots[id].as_ref().unwrap().texture.clone();
    drop(compositor);
    let expected = ledger.live_bytes();
    let consumer_identity = format!("{consumer:?}");
    let encode_identity = format!("{encode:?}");
    assert_ne!(
        encode_identity, consumer_identity,
        "consumer follows encode"
    );
    *shared.release_probe.lock().unwrap() = Some(Box::new(move |pool, release| {
        use crate::compositor::ledger_probes::{with_completion_hold, with_hook};
        let gpu = pool.encoder.gpu.clone();
        let release_identity = format!("{release:?}");
        assert_ne!(
            release_identity, encode_identity,
            "release must not use encode fence"
        );
        assert_ne!(
            release_identity, consumer_identity,
            "release fence follows consumer"
        );
        let (held_tx, held_rx) = mpsc::sync_channel(1);
        let (polled_tx, polled_rx) = mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = mpsc::sync_channel(1);
        let (pending_tx, pending_rx) = mpsc::sync_channel(1);
        let completed = Arc::new(AtomicBool::new(false));
        let waiter_done = Arc::clone(&completed);
        let waiter = thread::spawn(move || {
            with_completion_hold(held_tx, || {
                let mut first = true;
                with_hook(
                    move |device, wait| {
                        if !first {
                            // The first successful poll returned to production;
                            // the held callback must keep its completion loop alive.
                            pending_tx.send(()).unwrap();
                            resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                            return None;
                        }
                        first = false;
                        let wgpu::PollType::Wait {
                            submission_index: Some(index),
                            ..
                        } = wait
                        else {
                            panic!("release must poll its submission");
                        };
                        let polled_identity = format!("{index:?}");
                        let result = device.poll(wait.clone());
                        polled_tx.send((polled_identity, result.is_ok())).unwrap();
                        Some(result)
                    },
                    || complete(&gpu, release),
                );
            });
            waiter_done.store(true, Ordering::Release);
        });
        // A real successful poll delivered this callback, but its production
        // done flag is still false. The waiter cannot return until we run it.
        let callback = held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (polled_identity, poll_succeeded) =
            polled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let wait_reentered = pending_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let completion_withheld = !completed.load(Ordering::Acquire);
        let retained = pool.slots[id]
            .as_ref()
            .is_some_and(|slot| *slot.texture == identity);
        let charged = pool.encoder.gpu.ledger().live_bytes();
        println!(
            "R71_F4 encode={encode_identity} consumer={consumer_identity} release={release_identity} polled={polled_identity} poll_succeeded={poll_succeeded} completion_wait_reentered={wait_reentered} consumer_completion_withheld={completion_withheld} identity_retained={retained} charges={charged} expected={expected}"
        );
        callback();
        let _ = resume_tx.send(());
        waiter.join().unwrap();
        assert!(completed.load(Ordering::Acquire));
        assert_eq!(
            polled_identity, release_identity,
            "observed the post-consumer fence"
        );
        assert!(poll_succeeded && wait_reentered && completion_withheld);
        assert!(
            retained,
            "original allocation must remain while consumer completion is withheld"
        );
        assert_eq!(
            charged, expected,
            "exact charges while consumer completion is withheld"
        );
        assert_eq!(*pool.slots[id].as_ref().unwrap().texture, identity);
        assert_eq!(pool.encoder.gpu.ledger().live_bytes(), expected);
    }));
    session.terminal();
    drop(pool);
    let bytes = mapped_bytes(&gpu, &readback, consumer).unwrap();
    assert_eq!(
        u32::from_le_bytes(bytes.try_into().unwrap()),
        u32::from(expected_sample),
        "the later consumer sampled the encoded slot"
    );
    assert_eq!(ledger.live_bytes(), baseline);
    println!(
        "R71_F4 consumer_completed=true allocation_released=true charges={} baseline={baseline}",
        ledger.live_bytes()
    );
}
