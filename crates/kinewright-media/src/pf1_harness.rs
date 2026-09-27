//! PF1 S0: the playback measurement harness on today's APIs (design §2, V-4).
//!
//! Lanes (Q-1): the deterministic tests (the metrics and the controls'
//! logic, the V-5 driver, `process_memory`) run in ordinary CI. The timing
//! lanes are `--ignored` release runs: LL (lavapipe, the default), LH
//! (`PF1_HARDWARE=1`, the RTX 3090) and, by hand, the WARP VM.
//! `PF1_ONLY=a,b` narrows the workloads (`controls` names the Q-3 runs);
//! `PF1_DEVICE=1` plays one run per workload on the real device (V-5's
//! cross-check). Every result line starts with `PF1 `.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::Receiver;
use kinewright_core::{Document, FrameTexture, MediaEvent, Playback, PlaybackState, TimeCode};

use crate::{
    FfmpegMediaEngine,
    audio::{
        DEVICE_LATENCY_MICROS, DEVICE_UNDERRUN_FRAMES,
        simulated::{CALLBACK_FRAMES, SimulatedAudio},
    },
    compositor::GpuContext,
    engine::Faults,
    gpu_test_support::fixture_gpu_or_skip,
    perf_fixtures::{self, MIB, Workload},
    test_support::TempDirectory,
};

const SAMPLE: Duration = Duration::from_millis(5);

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1e3
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

/// Nearest-rank percentile; sorts `values`.
fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let rank = (values.len() as f64 * p).ceil() as usize;
    values
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------- P-rss

/// P-rss: this process's resident set, its peak and its thread count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcessMemory {
    pub(crate) rss: u64,
    pub(crate) peak: u64,
    pub(crate) threads: u64,
}

/// Linux: `VmRSS`/`VmHWM` from `/proc/self/status`, threads from
/// `/proc/self/task`.
#[cfg(target_os = "linux")]
pub(crate) fn process_memory() -> Option<ProcessMemory> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let threads = std::fs::read_dir("/proc/self/task").ok()?.count() as u64;
    parse_status(&status, threads)
}

#[cfg(target_os = "linux")]
fn parse_status(status: &str, threads: u64) -> Option<ProcessMemory> {
    let kib = |key: &str| {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.trim().strip_suffix(" kB")?;
            value.trim().parse::<u64>().ok()
        })
    };
    Some(ProcessMemory {
        rss: kib("VmRSS:")? * 1_024,
        peak: kib("VmHWM:")? * 1_024,
        threads,
    })
}

/// Linux restarts `VmHWM` (`clear_refs` 5), so a run's peak is its own.
#[cfg(target_os = "linux")]
fn reset_peak() -> bool {
    std::fs::write("/proc/self/clear_refs", "5").is_ok()
}

/// Windows: the same counters (`GetProcessMemoryInfo`'s working set and its
/// peak, the thread snapshot) through `Get-Process`: the workspace forbids
/// `unsafe_code`, so the FFI calls cannot be made directly.
#[cfg(windows)]
pub(crate) fn process_memory() -> Option<ProcessMemory> {
    let script = format!(
        "$p = Get-Process -Id {}; \"$($p.WorkingSet64) $($p.PeakWorkingSet64) $($p.Threads.Count)\"",
        std::process::id()
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let fields: Vec<u64> = text
        .split_whitespace()
        .filter_map(|f| f.parse().ok())
        .collect();
    let [rss, peak, threads] = fields[..] else {
        return None;
    };
    Some(ProcessMemory { rss, peak, threads })
}

/// Windows keeps the process-lifetime peak.
#[cfg(windows)]
fn reset_peak() -> bool {
    false
}

// ---------------------------------------------------------------- V-4 metrics

/// V-4: one P-play run as the consumer saw it, in ms since `play`.
#[derive(Default)]
struct Trace {
    /// `position()` every 5 ms.
    samples: Vec<(f64, i64)>,
    /// `frames()` arrivals: (time, frame, `position()` at receipt).
    arrivals: Vec<(f64, i64, i64)>,
    /// When the clock reached the duration.
    end: Option<f64>,
}

#[derive(Debug, Default)]
struct PlayMetrics {
    due: usize,
    on_time: usize,
    late: usize,
    dropped: usize,
    /// Present intervals: p50, p95, max.
    present: [f64; 3],
    held_max: f64,
    av_offset_max: f64,
    clock_stall_max: f64,
}

impl Trace {
    fn metrics(&self, due: i64, frame_ms: f64) -> PlayMetrics {
        let end = self.end.unwrap_or(f64::INFINITY);
        let samples: Vec<_> = self.samples.iter().filter(|(t, _)| *t <= end).collect();
        // A frame is due at the first sample whose position reached it.
        let (mut due_at, mut next) = (vec![None; due as usize], 0);
        for &&(t, position) in &samples {
            while next < due && next <= position {
                due_at[next as usize] = Some(t);
                next += 1;
            }
        }
        let mut first = BTreeMap::new();
        for &(t, at, _) in &self.arrivals {
            first.entry(at).or_insert(t);
        }
        let mut m = PlayMetrics {
            due: due as usize,
            ..PlayMetrics::default()
        };
        for (frame, due_t) in due_at.iter().enumerate() {
            match (due_t, first.get(&(frame as i64))) {
                (Some(due_t), Some(t)) if *t <= due_t + frame_ms => m.on_time += 1,
                (Some(_), Some(_)) => m.late += 1,
                _ => m.dropped += 1,
            }
        }
        let mut intervals: Vec<f64> = self.arrivals.windows(2).map(|w| w[1].0 - w[0].0).collect();
        m.present = [0.5, 0.95, 1.0].map(|p| percentile(&mut intervals, p));
        // Held age: time since the last arrival of a newer frame.
        let (mut shown, mut since, mut arrivals) = (-1, 0.0, self.arrivals.iter().peekable());
        let (mut value, mut changed) = (i64::MIN, 0.0);
        for &&(t, position) in &samples {
            while let Some(&(arrived, at, _)) = arrivals.next_if(|a| a.0 <= t) {
                if at > shown {
                    (shown, since) = (at, arrived);
                }
            }
            m.held_max = m.held_max.max(t - since);
            m.clock_stall_max = m.clock_stall_max.max(t - changed);
            if position != value {
                (value, changed) = (position, t);
            }
        }
        m.av_offset_max = (self.arrivals.iter())
            .map(|(_, at, position)| (position - at).abs() as f64 * frame_ms)
            .fold(0.0, f64::max);
        m
    }
}

impl PlayMetrics {
    /// G1: ≤ 1% late/held/dropped and p95 present interval ≤ 50 ms.
    fn g1(&self) -> bool {
        (self.late + self.dropped) as f64 <= 0.01 * self.due as f64 && self.present[1] <= 50.0
    }

    /// G14: max held age ≤ 100 ms.
    fn g14(&self) -> bool {
        self.held_max <= 100.0
    }

    /// G16: max clock stall ≤ 100 ms.
    fn g16(&self) -> bool {
        self.clock_stall_max <= 100.0
    }
}

// ---------------------------------------------------------------- sessions

fn lane() -> (bool, &'static str) {
    let hardware = std::env::var_os("PF1_HARDWARE").is_some();
    (hardware, if hardware { "LH" } else { "LL" })
}

fn wanted(key: &str) -> bool {
    std::env::var("PF1_ONLY").map_or(true, |only| only.split(',').any(|k| k == key))
}

/// A named workload builder.
type Builder = (&'static str, fn() -> Workload);

/// Timing lanes are release evidence (as R28's).
fn assert_release() {
    let release = !cfg!(debug_assertions);
    assert!(
        release,
        "PF1 timing is release evidence: cargo test --release"
    );
}

fn frame_ms(document: &Document) -> f64 {
    1e3 * f64::from(document.fps.denominator()) / f64::from(document.fps.numerator())
}

/// Q-2 P-play's workloads: W-1 plus MO2's two, unchanged builders at 60 s.
fn play_workloads() -> Vec<(&'static str, Workload)> {
    let all: [Builder; 6] = [
        ("typical_1080p", typical_60s),
        ("blend_heavy_1080p", || perf_fixtures::blend_heavy(1_800)),
        ("explainer_16x9", perf_fixtures::explainer_16x9),
        ("reel_9x16", || perf_fixtures::social((1080, 1920))),
        ("feed_4x5", || perf_fixtures::social((1080, 1350))),
        ("talk_recut", perf_fixtures::talk_recut),
    ];
    let wanted = all.into_iter().filter(|(key, _)| wanted(key));
    wanted.map(|(key, make)| (key, make())).collect()
}

fn typical_60s() -> Workload {
    perf_fixtures::cuts((1920, 1080), 600, 3, 360, 15)
}

struct Session {
    engine: FfmpegMediaEngine,
    frames: Receiver<(TimeCode, FrameTexture)>,
    events: Receiver<MediaEvent>,
    gpu: GpuContext,
    _data: TempDirectory,
}

impl Session {
    fn new(audio: Option<SimulatedAudio>, faults: Arc<Faults>, hardware: bool) -> Self {
        let gpu = GpuContext::headless(!hardware).expect("the lane's adapter");
        let data = TempDirectory::new("pf1-harness");
        let root = data.root().to_path_buf();
        let engine = FfmpegMediaEngine::new_for_harness(gpu.clone(), root, audio, faults)
            .expect("the harness engine starts");
        let (frames, events) = (engine.frames(), engine.events());
        Self {
            engine,
            frames,
            events,
            gpu,
            _data: data,
        }
    }

    fn load(&self, document: &Document) {
        self.engine.set_document(Arc::new(document.clone()));
        let first = self.wait_frame(0, Instant::now(), Duration::from_secs(120));
        first.expect("the first paused frame renders");
    }

    /// The first `target` frame received after `from`, in ms since `from`.
    fn wait_frame(&self, target: i64, from: Instant, limit: Duration) -> Option<f64> {
        loop {
            match self.frames.recv_deadline(from + limit) {
                Ok((at, _)) if at.0 == target => return Some(ms(from)),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }

    fn drain(&self) {
        self.frames.try_iter().for_each(drop);
    }
}

// ---------------------------------------------------------------- P-play

/// Q-3 controls: each injects one fault into an otherwise normal run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    None,
    /// +50 ms per preview render (fails G1).
    Slowdown,
    /// Publication stopped 1 s with the clock running (fails G14).
    Freeze,
    /// The clock stopped 1 s while playing (fails G16).
    ClockFreeze,
    /// A 2 s `fill_audio` stall (registers `underrun_frames` > 0).
    Stall,
}

/// Q-2 P-play: a 2 s warm-up, then the whole timeline from 0 on the paced
/// simulated driver (or the device), sampled every 5 ms; faults fire at 20 s.
fn play_run(document: &Document, control: Control, device: bool) -> (PlayMetrics, u64, String) {
    let audio = (!device).then(SimulatedAudio::paced);
    let faults = Arc::new(Faults::default());
    let session = Session::new(audio.clone(), Arc::clone(&faults), lane().0);
    session.load(document);
    session.engine.play(TimeCode::ZERO);
    thread::sleep(Duration::from_secs(2));
    session.engine.pause();
    let paused = Instant::now() + Duration::from_secs(60);
    let paused_event = MediaEvent::PlaybackStateChanged(PlaybackState::Paused);
    while session
        .events
        .recv_deadline(paused)
        .is_ok_and(|e| e != paused_event)
    {}
    session.drain();
    let underruns = || {
        audio.as_ref().map_or_else(
            || DEVICE_UNDERRUN_FRAMES.load(Ordering::Relaxed),
            SimulatedAudio::underrun_frames,
        )
    };
    let underruns_before = underruns();
    let peak_reset = reset_peak();
    let slowdown = if control == Control::Slowdown { 50 } else { 0 };
    faults.render_delay_ms.store(slowdown, Ordering::Relaxed);
    let (duration, frame) = (document.duration.0, frame_ms(document));
    let limit = 1.02 * duration as f64 * frame + 500.0;
    let (mut trace, mut armed) = (Trace::default(), true);
    let start = Instant::now();
    session.engine.play(TimeCode::ZERO);
    let mut next = start;
    loop {
        next += SAMPLE;
        while let Ok((at, _)) = session.frames.recv_deadline(next) {
            let position = session.engine.position().0;
            trace.arrivals.push((ms(start), at.0, position));
        }
        let (t, position) = (ms(start), session.engine.position().0);
        trace.samples.push((t, position));
        if armed && t >= 20_000.0 {
            armed = false;
            match control {
                Control::Freeze => {
                    let until = Instant::now() + Duration::from_secs(1);
                    *faults.unpublished_until.lock().expect("fault state") = Some(until);
                }
                Control::ClockFreeze => audio
                    .as_ref()
                    .expect("paced")
                    .freeze(Duration::from_secs(1)),
                Control::Stall => faults.fill_stall_ms.store(2_000, Ordering::Relaxed),
                Control::None | Control::Slowdown => {}
            }
        }
        for event in session.events.try_iter() {
            assert!(
                !matches!(event, MediaEvent::Error(_)),
                "P-play run failed: {event:?}"
            );
        }
        if trace.end.is_none() && position >= duration {
            trace.end = Some(t);
        }
        if trace.end.is_some_and(|end| t > end + 250.0) || t > limit + 5_000.0 {
            break;
        }
    }
    session.engine.pause();
    let m = trace.metrics(duration, frame);
    let underrun = underruns() - underruns_before;
    let peak = process_memory().map_or(0, |memory| memory.peak);
    let latency = if device {
        let micros = DEVICE_LATENCY_MICROS.load(Ordering::Relaxed);
        format!(" device_latency_ms={:.1}", micros as f64 / 1e3)
    } else {
        String::new()
    };
    let line = format!(
        "valid={} elapsed_s={:.2} due={} on_time={} late={} dropped={} present_p50_ms={:.1} \
         present_p95_ms={:.1} present_max_ms={:.1} held_max_ms={:.1} av_offset_max_ms={:.1} \
         clock_stall_max_ms={:.1} underrun_frames={underrun} peak_rss_mib={:.1}{} \
         ledger_peak_mib={:.1}{latency}",
        trace.end.is_some_and(|end| end <= limit),
        trace.end.unwrap_or(f64::NAN) / 1e3,
        m.due,
        m.on_time,
        m.late,
        m.dropped,
        m.present[0],
        m.present[1],
        m.present[2],
        m.held_max,
        m.av_offset_max,
        m.clock_stall_max,
        mib(peak),
        if peak_reset { "" } else { "(lifetime)" },
        mib(session.gpu.ledger().peak_bytes()),
    );
    (m, underrun, line)
}

#[test]
#[ignore = "PF1 P-play lane: cargo test --release -p kinewright-media --lib pf1_play_baseline -- --ignored --nocapture --test-threads=1"]
fn pf1_play_baseline() {
    assert_release();
    let (hardware, lane) = lane();
    let device = std::env::var_os("PF1_DEVICE").is_some();
    let adapter = GpuContext::headless(!hardware).expect("an adapter");
    let adapter = adapter.monitor_proof_metadata().adapter;
    let output = if device { "device" } else { "simulated" };
    for (key, Workload(document, _media)) in play_workloads() {
        for run in 0..if device { 1 } else { 3 } {
            let (_, _, line) = play_run(&document, Control::None, device);
            println!(
                "PF1 play lane={lane} adapter={adapter} output={output} workload={key} run={run} {line}"
            );
        }
    }
    if device || !wanted("controls") {
        return;
    }
    let Workload(document, _media) = typical_60s();
    let mut passed = Vec::new();
    for (control, metric) in [
        (Control::Slowdown, "G1"),
        (Control::Freeze, "G14"),
        (Control::ClockFreeze, "G16"),
        (Control::Stall, "underrun_frames"),
    ] {
        let (m, underrun, line) = play_run(&document, control, false);
        let fails = match control {
            Control::Slowdown => !m.g1(),
            Control::Freeze => !m.g14(),
            Control::ClockFreeze => !m.g16(),
            Control::Stall | Control::None => underrun > 0,
        };
        println!(
            "PF1 control={control:?} lane={lane} workload=typical_1080p metric={metric} fails={fails} {line}"
        );
        if !fails {
            passed.push(control);
        }
    }
    assert!(
        passed.is_empty(),
        "every Q-3 control must fail its metric: {passed:?}"
    );
}

// ---------------------------------------------------------------- P-seek

/// Q-2 P-seek (paused): 200 random seeks, 200 forward steps (+1…+12), 200
/// backward steps (−1…−12), each as the app's `seek_to`; then a 5 s
/// `request_frame` drag at 30 Hz and its release `seek` (L-6).
fn seek_run(document: &Document, seed: u64) -> String {
    let session = Session::new(None, Arc::default(), lane().0);
    session.load(document);
    let n = document.duration.0;
    let mut state = seed;
    let mut next = |bound: i64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as i64
    };
    let op = |target: i64| {
        session.drain();
        let from = Instant::now();
        session.engine.seek(TimeCode(target));
        session.engine.request_frame(TimeCode(target));
        let latency = session.wait_frame(target, from, Duration::from_secs(10));
        latency.unwrap_or(f64::INFINITY)
    };
    let mut random: Vec<f64> = (0..200).map(|_| op(next(n))).collect();
    let (mut at, mut forward, mut plus_one, mut backward) = (0, Vec::new(), Vec::new(), Vec::new());
    for _ in 0..200 {
        if at + 12 >= n {
            at = next(n - 13);
            op(at);
        }
        let step = 1 + next(12);
        at += step;
        let latency = op(at);
        forward.push(latency);
        if step == 1 {
            plus_one.push(latency);
        }
    }
    for _ in 0..200 {
        if at < 12 {
            at = 12 + next(n - 12);
            op(at);
        }
        at -= 1 + next(12);
        backward.push(op(at));
    }
    // The drag: calls at 30 Hz; call i is answered by the first frame at or
    // past its target (targets rise, so a newer call's frame answers it).
    let mut target = next(n - 800);
    op(target);
    let (start, mut calls, mut arrivals) = (Instant::now(), Vec::new(), Vec::new());
    for i in 0..150_u32 {
        let due = start + Duration::from_secs(1) * i / 30;
        while let Ok((frame, _)) = session.frames.recv_deadline(due) {
            arrivals.push((ms(start), frame.0));
        }
        target += 1 + next(4);
        calls.push((ms(start), target));
        session.engine.request_frame(TimeCode(target));
    }
    let settle = start + Duration::from_secs(15);
    while arrivals.last().is_none_or(|&(_, frame)| frame != target) {
        let Ok((frame, _)) = session.frames.recv_deadline(settle) else {
            break;
        };
        arrivals.push((ms(start), frame.0));
    }
    let mut drag: Vec<f64> = (calls.iter())
        .map(|&(t, goal)| {
            let answer = arrivals.iter().find(|&&(a, frame)| a >= t && frame >= goal);
            answer.map_or(f64::INFINITY, |&(a, _)| a - t)
        })
        .collect();
    let distinct: BTreeSet<i64> = (arrivals.iter())
        .filter(|(t, _)| *t <= 5_000.0)
        .map(|&(_, frame)| frame)
        .collect();
    let from = Instant::now();
    session.engine.seek(TimeCode(target));
    let release = session.wait_frame(target, from, Duration::from_secs(10));
    let timeouts = [&random, &forward, &backward, &drag]
        .iter()
        .flat_map(|v| v.iter())
        .filter(|l| l.is_infinite())
        .count();
    format!(
        "random_p95_ms={:.1} random_max_ms={:.1} forward_p95_ms={:.1} plus1_p95_ms={:.1} \
         backward_p95_ms={:.1} drag_p95_ms={:.1} drag_distinct_fps={:.1} release_shown={} \
         release_ms={:.1} timeouts={timeouts}",
        percentile(&mut random, 0.95),
        percentile(&mut random, 1.0),
        percentile(&mut forward, 0.95),
        percentile(&mut plus_one, 0.95),
        percentile(&mut backward, 0.95),
        percentile(&mut drag, 0.95),
        distinct.len() as f64 / 5.0,
        release.is_some(),
        release.unwrap_or(f64::NAN),
    )
}

#[test]
#[ignore = "PF1 P-seek lane: cargo test --release -p kinewright-media --lib pf1_seek_baseline -- --ignored --nocapture --test-threads=1"]
fn pf1_seek_baseline() {
    assert_release();
    let lane = lane().1;
    let all: [Builder; 3] = [
        ("seek_gop60", perf_fixtures::seek_gop60),
        ("talk_recut", perf_fixtures::talk_recut),
        ("explainer_16x9", perf_fixtures::explainer_16x9),
    ];
    for (key, make) in all.into_iter().filter(|(key, _)| wanted(key)) {
        let Workload(document, _media) = make();
        for run in 0..3 {
            let line = seek_run(&document, 0x5EED_0000 + run);
            println!("PF1 seek lane={lane} workload={key} run={run} {line}");
        }
    }
}

/// P-rss: a fresh process per measurement (the child below).
#[test]
#[ignore = "PF1 P-rss lane (LL pinned): cargo test --release -p kinewright-media --lib pf1_rss_baseline -- --ignored --nocapture --test-threads=1"]
fn pf1_rss_baseline() {
    let lane = lane().1;
    let mut all = play_workloads();
    all.push(("title_only", perf_fixtures::title_only()));
    for (key, Workload(document, _media)) in all {
        let temp = TempDirectory::new("pf1-rss");
        let path = temp.path("document.json");
        std::fs::write(&path, serde_json::to_vec(&document).expect("serialises")).expect("writes");
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "pf1_harness::pf1_rss_child",
                "--ignored",
                "--nocapture",
            ])
            .args(["--test-threads=1"])
            .env("PF1_RSS_DOCUMENT", &path)
            .output()
            .expect("the P-rss child runs");
        let stdout = String::from_utf8_lossy(&child.stdout);
        // libtest prints `test … ... ` on the same line before the output.
        let line = (stdout.lines()).find_map(|line| Some(&line[line.find("PF1 rss ")?..]));
        let stderr = String::from_utf8_lossy(&child.stderr);
        let line = line.unwrap_or_else(|| panic!("the P-rss child failed: {stderr}"));
        println!("{line} lane={lane} workload={key}");
    }
}

/// One P-rss measurement: before the engine, constructed (no document), first
/// render, settled idle after 6 s, and 10 s of playback (rss MiB/threads).
#[test]
#[ignore = "PF1 P-rss child: spawned by pf1_rss_baseline"]
fn pf1_rss_child() {
    let Some(path) = std::env::var_os("PF1_RSS_DOCUMENT") else {
        return;
    };
    let document: Document =
        serde_json::from_slice(&std::fs::read(path).expect("reads")).expect("a document");
    let memory = || process_memory().expect("process memory");
    let show = |m: ProcessMemory| format!("{:.1}/{}", mib(m.rss), m.threads);
    let before = memory();
    let session = Session::new(Some(SimulatedAudio::paced()), Arc::default(), lane().0);
    thread::sleep(Duration::from_millis(500));
    let constructed = memory();
    session.load(&document);
    let first = memory();
    thread::sleep(Duration::from_secs(6));
    let settled = memory();
    let peak_reset = reset_peak();
    session.engine.play(TimeCode::ZERO);
    thread::sleep(Duration::from_secs(10));
    let playing = memory();
    session.engine.pause();
    println!(
        "PF1 rss before={} constructed={} first_render={} settled_idle={} playing={} \
         playing_peak_mib={:.1}{}",
        show(before),
        show(constructed),
        show(first),
        show(settled),
        show(playing),
        mib(playing.peak),
        if peak_reset { "" } else { "(lifetime)" }
    );
}

// ---------------------------------------------------------------- CI lane

/// A 30 fps trace of `n` frames, each arriving `delay` ms after it is due,
/// with the clock stopped over `clock_gap` and publication over `mute`.
fn synthetic(n: i64, delay: f64, clock_gap: (f64, f64), mute: (f64, f64)) -> Trace {
    let frame = 1e3 / 30.0;
    let clock = |t: f64| {
        let (at, len) = clock_gap;
        ((if t < at { t } else { (t - len).max(at) }) / frame) as i64
    };
    let mut trace = Trace::default();
    let mut t = 0.0;
    while clock(t) < n {
        trace.samples.push((t, clock(t)));
        t += 5.0;
    }
    trace.samples.push((t, n));
    trace.end = Some(t);
    for k in 0..n {
        let due = trace.samples.iter().find(|s| s.1 >= k).expect("due").0;
        let arrived = due + delay;
        if !(mute.0..mute.0 + mute.1).contains(&arrived) {
            trace.arrivals.push((arrived, k, clock(arrived)));
        }
    }
    trace
}

#[test]
fn pf1_metrics_pass_a_clean_trace_and_every_control_fails_its_metric() {
    let frame = 1e3 / 30.0;
    let clean = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0)).metrics(90, frame);
    assert_eq!(
        (clean.on_time, clean.late, clean.dropped),
        (90, 0, 0),
        "{clean:?}"
    );
    assert!(clean.g1() && clean.g14() && clean.g16(), "{clean:?}");
    let slowdown = synthetic(90, 60.0, (0.0, 0.0), (0.0, 0.0)).metrics(90, frame);
    assert!(
        !slowdown.g1() && slowdown.late == 90,
        "slowdown: {slowdown:?}"
    );
    let freeze = synthetic(90, 10.0, (0.0, 0.0), (1_000.0, 1_000.0)).metrics(90, frame);
    assert!(
        !freeze.g14() && freeze.held_max >= 1_000.0,
        "freeze: {freeze:?}"
    );
    let clock = synthetic(90, 10.0, (1_000.0, 1_000.0), (0.0, 0.0)).metrics(90, frame);
    assert!(
        !clock.g16() && clock.clock_stall_max >= 1_000.0,
        "clock freeze: {clock:?}"
    );
    // A frame the clock never passed is dropped, however the run ends.
    let mut short = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0));
    short.samples.retain(|s| s.1 < 60);
    short.end = None;
    assert_eq!(short.metrics(90, frame).dropped, 30);
}

#[cfg(target_os = "linux")]
#[test]
fn pf1_process_memory_parses_proc_status() {
    let status = "Name:\tx\nVmHWM:\t  2048 kB\nVmRSS:\t   1024 kB\nThreads:\t3\n";
    let memory = parse_status(status, 3).expect("parses");
    assert_eq!(
        (memory.rss, memory.peak, memory.threads),
        (1 << 20, 2 << 20, 3)
    );
    assert!(
        parse_status("VmRSS:\t1 kB\n", 1).is_none(),
        "no peak, no reading"
    );
}

#[test]
fn pf1_process_memory_reads_this_process() {
    let memory = process_memory().expect("process memory");
    assert!(
        memory.rss > 0 && memory.peak >= memory.rss && memory.threads >= 1,
        "{memory:?}"
    );
}

/// V-5: the engine's clock moves only when the stepped driver runs a callback.
#[test]
fn pf1_engine_clock_follows_the_stepped_simulated_driver() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let (audio, data) = (SimulatedAudio::stepped(), TempDirectory::new("pf1-stepped"));
    let root = data.root().to_path_buf();
    let engine = FfmpegMediaEngine::new_for_harness(gpu, root, Some(audio.clone()), Arc::default())
        .expect("the engine starts");
    engine.set_document(Arc::new(perf_fixtures::title_card((64, 64), 90)));
    engine.play(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(60);
    let (receiver, mut events) = (engine.events(), Vec::new());
    while !audio.advance(0) {
        events.extend(receiver.try_iter());
        assert!(
            Instant::now() < deadline,
            "the simulated stream opens: {events:?}"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(engine.position(), TimeCode::ZERO, "no callback, no clock");
    for _ in 0..47 {
        assert!(audio.advance(CALLBACK_FRAMES));
    }
    // 47 × 1,024 = 48,128 frames at 48 kHz: frame 30 at 30 fps.
    assert_eq!(engine.position(), TimeCode(30));
    engine.pause();
}
