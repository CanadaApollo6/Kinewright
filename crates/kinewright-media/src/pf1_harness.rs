//! PF1 S0: the playback measurement harness on today's APIs (design §2, V-4).
//!
//! Lanes (Q-1): the deterministic tests (the metrics and the controls'
//! logic, the V-5 driver, `process_memory`) run in ordinary CI. The timing
//! lanes are `--ignored` release runs: LL (lavapipe, the default), LH
//! (`PF1_HARDWARE=1`, the RTX 3090) and, by hand, the WARP VM.
//! `PF1_ONLY=a,b` narrows the workloads (`controls` names the Q-3 runs);
//! `PF1_RUNS=n` overrides P-play's three runs (a spot check); `PF1_DEVICE=1`
//! plays one run per workload on the real device (V-5's cross-check). Every
//! result line starts with `PF1 `.

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
use kinewright_core::{
    Document, FrameStamp, MediaEvent, Playback, PlaybackState, PlaybackStats, PreviewFrame,
    TimeCode,
};

use crate::{
    FfmpegMediaEngine,
    audio::{
        AudioDiagnostics,
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
pub(crate) fn process_memory() -> Result<ProcessMemory, String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
    let tasks = std::fs::read_dir("/proc/self/task").map_err(|e| e.to_string())?;
    parse_status(&status, tasks.count() as u64).ok_or_else(|| format!("unparsed: {status}"))
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
/// `unsafe_code`, so the FFI calls cannot be made directly. Bounded by a
/// 30 s deadline; the exit status and exactly three integers are required.
#[cfg(windows)]
pub(crate) fn process_memory() -> Result<ProcessMemory, String> {
    use std::{io::Read, process::Stdio};
    let script = format!(
        "$p = Get-Process -Id {}; \"$($p.WorkingSet64) $($p.PeakWorkingSet64) $($p.Threads.Count)\"",
        std::process::id()
    );
    let mut child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("powershell.exe: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() <= deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let cleanup = abandon(&mut child);
                return Err(format!("Get-Process timed out after 30 s; {cleanup}"));
            }
            Err(error) => {
                let cleanup = abandon(&mut child);
                return Err(format!("waiting on Get-Process failed: {error}; {cleanup}"));
            }
        }
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let pipes = (child.stdout.take(), child.stderr.take());
    if let (Some(mut out), Some(mut err)) = pipes {
        out.read_to_string(&mut stdout).map_err(|e| e.to_string())?;
        err.read_to_string(&mut stderr).map_err(|e| e.to_string())?;
    }
    if !status.success() {
        return Err(format!("Get-Process failed ({status}): {stderr}"));
    }
    let fields: Result<Vec<u64>, _> = stdout.split_whitespace().map(str::parse).collect();
    match fields.as_deref() {
        Ok(&[rss, peak, threads]) => Ok(ProcessMemory { rss, peak, threads }),
        _ => Err(format!(
            "unexpected Get-Process output {stdout:?}: {stderr}"
        )),
    }
}

/// Kills an abandoned `powershell.exe` and reaps it, both bounded (5 s):
/// returns what each step did, for the error.
#[cfg(windows)]
fn abandon(child: &mut std::process::Child) -> String {
    let kill = child.kill();
    let deadline = Instant::now() + Duration::from_secs(5);
    let reap = loop {
        match child.try_wait() {
            Ok(Some(status)) => break format!("reaped ({status})"),
            Ok(None) if Instant::now() <= deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => break "not reaped within 5 s".to_owned(),
            Err(error) => break format!("reap failed: {error}"),
        }
    };
    match kill {
        Ok(()) => format!("killed, {reap}"),
        Err(error) => format!("kill failed: {error}, {reap}"),
    }
}

/// Windows keeps the process-lifetime peak.
#[cfg(windows)]
fn reset_peak() -> bool {
    false
}

/// A memory reading for a result line; a failed probe is never a zero.
fn peak_field(peak_reset: bool) -> String {
    match process_memory() {
        Ok(memory) if peak_reset => format!("{:.1}", mib(memory.peak)),
        Ok(memory) => format!("{:.1}(lifetime)", mib(memory.peak)),
        Err(error) => format!("unavailable({})", error.replace(' ', "_")),
    }
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
    /// Received only before the clock reached them (R-5's lower bound).
    early: usize,
    dropped: usize,
    /// Intervals between presentations of newer frames: p50, p95, max.
    present: [f64; 3],
    held_max: f64,
    av_offset_max: f64,
    clock_stall_max: f64,
    /// Q-2 validity, set by the run; every gate requires it.
    valid: bool,
}

impl Trace {
    /// Due-frame outcomes (R-5), present intervals, held age, A/V offset and
    /// clock stall, all over the measured window (up to `end`). A receipt is
    /// eligible only once the clock has reached its frame (its receipt
    /// position); presentations are eligible receipts of newer frames, so
    /// repeated or early images improve neither intervals nor held age.
    fn metrics(&self, due: i64, frame_ms: f64) -> PlayMetrics {
        let end = self.end.unwrap_or(f64::INFINITY);
        let samples: Vec<_> = self.samples.iter().filter(|(t, _)| *t <= end).collect();
        let arrivals: Vec<_> = self.arrivals.iter().filter(|(t, ..)| *t <= end).collect();
        // A frame is due at the first sample whose position reached it.
        let (mut due_at, mut next) = (vec![None; due as usize], 0);
        for &&(t, position) in &samples {
            while next < due && next <= position {
                due_at[next as usize] = Some(t);
                next += 1;
            }
        }
        let (mut eligible, mut received) = (BTreeMap::new(), BTreeSet::new());
        let mut shown: Vec<(f64, i64)> = Vec::new();
        for &&(t, at, position) in &arrivals {
            received.insert(at);
            if position >= at {
                eligible.entry(at).or_insert(t);
                if shown.last().is_none_or(|&(_, last)| at > last) {
                    shown.push((t, at));
                }
            }
        }
        let mut m = PlayMetrics {
            due: due as usize,
            ..PlayMetrics::default()
        };
        for (frame, due_t) in due_at.iter().enumerate() {
            let frame = frame as i64;
            match (due_t, eligible.get(&frame)) {
                (Some(due_t), Some(t)) if *t <= due_t + frame_ms => m.on_time += 1,
                (Some(_), Some(_)) => m.late += 1,
                // Q-2: a frame the clock never passed is dropped.
                (Some(_), None) if received.contains(&frame) => m.early += 1,
                _ => m.dropped += 1,
            }
        }
        let mut intervals: Vec<f64> = shown.windows(2).map(|w| w[1].0 - w[0].0).collect();
        m.present = [0.5, 0.95, 1.0].map(|p| percentile(&mut intervals, p));
        // Held age: time since the latest presentation of a newer frame.
        let (mut since, mut shown) = (0.0, shown.iter().peekable());
        let (mut value, mut changed) = (i64::MIN, 0.0);
        for &&(t, position) in &samples {
            while let Some(&(presented, _)) = shown.next_if(|s| s.0 <= t) {
                since = presented;
            }
            m.held_max = m.held_max.max(t - since);
            m.clock_stall_max = m.clock_stall_max.max(t - changed);
            if position != value {
                (value, changed) = (position, t);
            }
        }
        m.av_offset_max = (arrivals.iter())
            .map(|(_, at, position)| (position - at).abs() as f64 * frame_ms)
            .fold(0.0, f64::max);
        m
    }
}

impl PlayMetrics {
    /// G1's metric: ≤ 1% late/early/dropped and p95 present interval ≤ 50 ms.
    fn g1_metric(&self) -> bool {
        let missed = self.late + self.early + self.dropped;
        missed as f64 <= 0.01 * self.due as f64 && self.present[1] <= 50.0
    }

    /// G14's metric: max held age ≤ 100 ms.
    fn g14_metric(&self) -> bool {
        self.held_max <= 100.0
    }

    /// G16's metric: max clock stall ≤ 100 ms.
    fn g16_metric(&self) -> bool {
        self.clock_stall_max <= 100.0
    }

    /// The gates: a run passes only if it is valid and meets every metric.
    fn passes(&self) -> bool {
        self.valid && self.g1_metric() && self.g14_metric() && self.g16_metric()
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

/// Timing and memory lanes are release evidence (as R28's).
fn assert_release() {
    let release = !cfg!(debug_assertions);
    assert!(
        release,
        "PF1 timing and memory lanes are release evidence: cargo test --release"
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
    frames: Receiver<PreviewFrame>,
    events: Receiver<MediaEvent>,
    gpu: GpuContext,
    /// This engine's audio diagnostics (R23).
    diagnostics: Arc<AudioDiagnostics>,
    _data: TempDirectory,
}

impl Session {
    fn new(audio: Option<SimulatedAudio>, faults: Arc<Faults>, hardware: bool) -> Self {
        let gpu = GpuContext::headless(!hardware).expect("the lane's adapter");
        let data = TempDirectory::new("pf1-harness");
        let root = data.root().to_path_buf();
        let diagnostics = Arc::new(AudioDiagnostics::default());
        let shared = Arc::clone(&diagnostics);
        let engine = FfmpegMediaEngine::new_for_harness(gpu.clone(), root, audio, faults, shared)
            .expect("the harness engine starts");
        let (frames, events) = (engine.frames(), engine.events());
        Self {
            engine,
            frames,
            events,
            gpu,
            diagnostics,
            _data: data,
        }
    }

    fn load(&self, document: &Document) {
        self.engine.set_document(Arc::new(document.clone()));
        let issued = self.engine.stamp();
        let first = self.wait_frame(0, issued, Instant::now(), Duration::from_secs(120));
        first.expect("the first paused frame renders");
    }

    /// The first `target` frame received after `from` that answers the
    /// call stamped `issued` (R-2: its epoch, not older), in ms since
    /// `from`; an older in-flight image of the same target does not count.
    fn wait_frame(
        &self,
        target: i64,
        issued: FrameStamp,
        from: Instant,
        limit: Duration,
    ) -> Option<f64> {
        wait_answer(&self.frames, target, issued, from, limit)
    }

    fn drain(&self) {
        self.frames.try_iter().for_each(drop);
    }

    fn assert_no_error(&self, context: &str) {
        for event in self.events.try_iter() {
            assert!(event.error().is_none(), "{context} failed: {event:?}");
        }
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

/// Fires a Q-3 control's fault (at 20 s; the slowdown runs from the start).
fn fire(control: Control, faults: &Faults, audio: Option<&SimulatedAudio>) {
    match control {
        Control::Freeze => {
            let until = Instant::now() + Duration::from_secs(1);
            *faults.unpublished_until.lock().expect("fault state") = Some(until);
        }
        Control::ClockFreeze => audio.expect("paced").freeze(Duration::from_secs(1)),
        Control::Stall => faults.fill_stall_ms.store(2_000, Ordering::Relaxed),
        Control::None | Control::Slowdown => {}
    }
}

/// Q-2 validity: the clock reached the duration within 0.98–1.02 of nominal
/// (± 0.5 s) and the paced driver missed no callback deadline (A-F2, R25/D1).
pub(crate) fn q2_valid(elapsed_ms: f64, nominal_ms: f64, missed_callbacks: u64) -> bool {
    let (lower, upper) = (0.98 * nominal_ms - 500.0, 1.02 * nominal_ms + 500.0);
    (lower..=upper).contains(&elapsed_ms) && missed_callbacks == 0
}

/// Q-2 P-play: a 2 s warm-up, then the whole timeline from 0 on the paced
/// simulated driver (or the device), sampled every 5 ms; faults fire at 20 s.
/// R31 (a): the callbacks classify their own underruns: before the
/// programme's last sample is in the ring (`underrun_frames`, G11's) or
/// after it (`post_end_underrun_frames`, the straddle at the end). Missed
/// deadlines invalidate the run wherever they fall, drain included. After
/// the run the session is torn down and `teardown` records whether the
/// preview thread finished, what the engine still holds, and the process's
/// threads and current RSS (review B: RSS attribution).
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
    let counters = || {
        let missed = audio.as_ref().map_or(0, SimulatedAudio::missed_periods);
        let [_, frames, _, post_end] = session.diagnostics.underruns();
        (frames, post_end, missed)
    };
    let before = counters();
    let peak_reset = reset_peak();
    let slowdown = if control == Control::Slowdown { 50 } else { 0 };
    faults.render_delay_ms.store(slowdown, Ordering::Relaxed);
    let (duration, frame) = (document.duration.0, frame_ms(document));
    let nominal = duration as f64 * frame;
    let upper = 1.02 * nominal + 500.0;
    let (mut trace, mut armed, mut rejected) = (Trace::default(), true, 0);
    let start = Instant::now();
    session.engine.play(TimeCode::ZERO);
    let mut next = start;
    loop {
        next += SAMPLE;
        while let Ok(PreviewFrame { at, stamp, .. }) = session.frames.recv_deadline(next) {
            let (received, position) = (Instant::now(), session.engine.position().0);
            trace.arrivals.push((ms(start), at.0, position));
            // R-2 at the final consumer (here, at receipt): the current
            // epoch and the clock's own frame; then the R-5 ack, with the
            // receipt standing for the paint.
            if stamp.epoch == session.engine.stamp().epoch && at.0 == position {
                session.engine.ack_presented(stamp, at, received, false);
            } else {
                rejected += 1;
            }
        }
        let (t, position) = (ms(start), session.engine.position().0);
        trace.samples.push((t, position));
        if armed && t >= 20_000.0 {
            armed = false;
            fire(control, &faults, audio.as_ref());
        }
        session.assert_no_error("P-play run");
        if trace.end.is_none() && position >= duration {
            trace.end = Some(t);
        }
        if trace.end.is_some_and(|end| t > end + 250.0) || t > upper + 5_000.0 {
            break;
        }
    }
    let peak = peak_field(peak_reset);
    let stats = session.engine.stats();
    session.engine.pause();
    let (final_underruns, final_post_end, final_missed) = counters();
    let underrun = final_underruns - before.0;
    let post_end = final_post_end - before.1;
    let missed = final_missed - before.2;
    let mut m = trace.metrics(duration, frame);
    let elapsed = trace.end.unwrap_or(f64::NAN);
    m.valid = q2_valid(elapsed, nominal, missed);
    let latency = if device {
        let micros = session.diagnostics.latency_micros();
        format!(" device_latency_ms={:.1}", micros as f64 / 1e3)
    } else {
        String::new()
    };
    let line = format!(
        "valid={} elapsed_s={:.2} missed_callbacks={missed} due={} on_time={} late={} early={} \
         dropped={} present_p50_ms={:.1} present_p95_ms={:.1} present_max_ms={:.1} \
         held_max_ms={:.1} receipt_offset_max_ms={:.1} clock_stall_max_ms={:.1} \
         underrun_frames={underrun} post_end_underrun_frames={post_end} peak_rss_mib={peak} \
         ledger_peak_mib={:.1} table_live_kib={} passes={}{latency} {} \
         consumer_rejected={rejected}",
        m.valid,
        elapsed / 1e3,
        m.due,
        m.on_time,
        m.late,
        m.early,
        m.dropped,
        m.present[0],
        m.present[1],
        m.present[2],
        m.held_max,
        m.av_offset_max,
        m.clock_stall_max,
        mib(session.gpu.ledger().peak_bytes()),
        crate::conversion::live_table_bytes() / 1024,
        m.passes(),
        engine_fields(&stats),
    );
    let line = format!("{line} {}", teardown(session));
    (m, underrun, line)
}

/// `Session::wait_frame`: the first `target` frame from `frames` that
/// answers the call stamped `issued`, in ms since `from`.
fn wait_answer(
    frames: &Receiver<PreviewFrame>,
    target: i64,
    issued: FrameStamp,
    from: Instant,
    limit: Duration,
) -> Option<f64> {
    loop {
        match frames.recv_deadline(from + limit) {
            Ok(PreviewFrame { at, stamp, .. }) if at.0 == target && stamp.is_current(issued) => {
                return Some(ms(from));
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Review B (RSS attribution), re-review B D6: keep only telemetry
/// handles (the ledger, the decoder gauge), drop the rest of the session,
/// wait (≤ 30 s) for the engine's worker thread to finish its whole
/// teardown (`FfmpegMediaEngine::finished`: preview joined, worker and
/// audio dropped), drop the GPU context, and only then record what
/// outlives it: ledger charges, decoders, conversion tables, and the
/// process's threads and current RSS.
fn teardown(session: Session) -> String {
    let Session {
        engine,
        frames,
        events,
        gpu,
        diagnostics,
        _data: data,
    } = session;
    let (decoders, finished, ledger) = (
        engine.decoder_gauge(),
        engine.finished(),
        gpu.shared_ledger(),
    );
    drop((engine, frames, events, diagnostics));
    let deadline = Instant::now() + Duration::from_secs(30);
    let complete = matches!(
        finished.recv_deadline(deadline),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    drop((gpu, data));
    let memory = process_memory().map_or_else(
        |error| format!("unavailable({})", error.replace(' ', "_")),
        |memory| format!("{:.1} threads={}", mib(memory.rss), memory.threads),
    );
    format!(
        "teardown_complete={complete} teardown_ledger_live_kib={} teardown_decoders={} \
         teardown_table_live_kib={} teardown_rss_mib={memory}",
        ledger.live_bytes() / 1024,
        decoders.open(),
        crate::conversion::live_table_bytes() / 1024,
    )
}

/// R-5: the engine's own `stats()` for the run, beside the consumer's.
fn engine_fields(stats: &PlaybackStats) -> String {
    format!(
        "engine_due={} engine_on_time={} engine_late={} engine_dropped={} \
         engine_dropped_agent={} engine_held_max_ms={:.1} engine_av_offset_max_ms={:.1} \
         engine_clock_stall_max_ms={:.1} engine_underrun_events={} engine_underrun_frames={} \
         engine_post_end_underrun_frames={} engine_sync_decoders={} engine_table_live_kib={} \
         engine_stale_errors={} engine_acks_overflowed={} engine_acks_unmatched={} \
         engine_sync_fallback_frames={} engine_lookahead_starved={}",
        stats.due_frames,
        stats.on_time,
        stats.late,
        stats.dropped,
        stats.dropped_agent,
        stats.max_held_ms,
        stats.max_av_offset_ms,
        stats.max_clock_stall_ms,
        stats.underrun_events,
        stats.underrun_frames,
        stats.post_end_underrun_frames,
        stats.sync_decoders,
        stats.live_table_bytes / 1024,
        stats.stale_errors,
        stats.acks_overflowed,
        stats.acks_unmatched,
        stats.sync_fallback_frames,
        stats.lookahead_starved,
    )
}

#[test]
#[ignore = "PF1 P-play lane: cargo test --release -p kinewright-media --lib pf1_play_baseline -- --ignored --nocapture --test-threads=1"]
fn pf1_play_baseline() {
    assert_release();
    let (hardware, lane) = lane();
    let device = std::env::var_os("PF1_DEVICE").is_some();
    let runs = std::env::var("PF1_RUNS").map_or(if device { 1 } else { 3 }, |runs| {
        runs.parse().expect("PF1_RUNS is a count")
    });
    let adapter = GpuContext::headless(!hardware).expect("an adapter");
    let adapter = adapter.monitor_proof_metadata().adapter;
    let output = if device { "device" } else { "simulated" };
    for (key, Workload(document, _media)) in play_workloads() {
        for run in 0..runs {
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
            Control::Slowdown => !m.g1_metric(),
            Control::Freeze => !m.g14_metric(),
            Control::ClockFreeze => !m.g16_metric(),
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
/// `request_frame` drag at 30 Hz, released by a `seek` at the 5 s boundary
/// (L-6) while drag renders may still be pending. Backward steps mix frame
/// cache hits and refills: reported combined (L-4a/L-4b are not separated).
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
        let issued = session.engine.stamp();
        let latency = session.wait_frame(target, issued, from, Duration::from_secs(10));
        latency.unwrap_or(f64::INFINITY)
    };
    let mut random: Vec<f64> = (0..200).map(|_| op(next(n))).collect();
    // Steps start from where the transport actually is.
    let mut at = next(n - 13);
    op(at);
    let (mut forward, mut plus_one, mut backward) = (Vec::new(), Vec::new(), Vec::new());
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
    let target = next(n - 800);
    op(target);
    let (drag, release_shown) = drag_and_release(&session, target, || 1 + next(4));
    let timeouts = [&random, &forward, &backward]
        .iter()
        .flat_map(|v| v.iter())
        .filter(|l| l.is_infinite())
        .count()
        + usize::from(!release_shown);
    format!(
        "random_p95_ms={:.1} random_max_ms={:.1} forward_p95_ms={:.1} plus1_p95_ms={:.1} \
         plus1_n={} backward_combined_p95_ms={:.1} {drag} timeouts={timeouts}",
        percentile(&mut random, 0.95),
        percentile(&mut random, 1.0),
        percentile(&mut forward, 0.95),
        percentile(&mut plus_one, 0.95),
        plus_one.len(),
        percentile(&mut backward, 0.95),
    )
}

/// The drag from `target` (already shown): 150 `request_frame` calls at
/// 30 Hz with rising targets, each answered by the first drag frame at or
/// past it; then the release `seek` at the 5 s boundary, unsettled, to a
/// frame the drag never requested, so its arrival is attributable to it.
/// `stale_frames_after_release` counts drag renders delivered after the
/// release call; `frames_over_release` those delivered after the release
/// frame itself (within 500 ms), which would replace it on screen.
fn drag_and_release(
    session: &Session,
    mut target: i64,
    mut step: impl FnMut() -> i64,
) -> (String, bool) {
    let (start, mut calls, mut arrivals) = (Instant::now(), Vec::new(), Vec::new());
    let collect = |until: Instant, arrivals: &mut Vec<(f64, i64, FrameStamp)>| {
        while let Ok(PreviewFrame { at, stamp, .. }) = session.frames.recv_deadline(until) {
            arrivals.push((ms(start), at.0, stamp));
        }
    };
    for i in 0..150_u32 {
        collect(start + Duration::from_secs(1) * i / 30, &mut arrivals);
        target += step();
        session.engine.request_frame(TimeCode(target));
        calls.push((ms(start), target, session.engine.stamp()));
    }
    collect(start + Duration::from_secs(5), &mut arrivals);
    // Review B: an answer carries the call's stamp or a newer one of its
    // epoch, so an older in-flight image never answers a newer call.
    let answered = |arrivals: &[(f64, i64, FrameStamp)],
                    (t, goal, issued): (f64, i64, FrameStamp)| {
        let answer = (arrivals.iter()).find(|&&(a, frame, stamp)| {
            a >= t && (goal..=target).contains(&frame) && stamp.is_current(issued)
        });
        answer.map(|&(a, ..)| a - t)
    };
    let pending = (calls.iter())
        .filter(|&&call| answered(&arrivals, call).is_none())
        .count();
    let release_target = target + 1;
    let (release_at, from) = (ms(start), Instant::now());
    session.engine.seek(TimeCode(release_target));
    let deadline = from + Duration::from_secs(10);
    let mut release = None;
    let latest = session.engine.stamp();
    while let Ok(PreviewFrame { at, stamp, .. }) = session.frames.recv_deadline(deadline) {
        arrivals.push((ms(start), at.0, stamp));
        // L-6 (R-2): the release target with the release's own epoch.
        if at.0 == release_target && stamp.epoch == latest.epoch {
            release = Some(ms(from));
            break;
        }
    }
    // Drag answers stop at the release receipt (R25/D4); later arrivals only
    // feed the overwrite diagnostic.
    let answerable = arrivals.len();
    // A stale drag render landing after the release frame would replace it.
    let release_arrived = (arrivals.last())
        .filter(|_| release.is_some())
        .map_or(f64::INFINITY, |&(t, ..)| t);
    // Frames after it that R-2 would bind in its place (current epoch).
    let (until, mut valid_over) = (Instant::now() + Duration::from_millis(500), 0);
    while let Ok(PreviewFrame { at, stamp, .. }) = session.frames.recv_deadline(until) {
        arrivals.push((ms(start), at.0, stamp));
        valid_over += usize::from(stamp.epoch == latest.epoch && at.0 != release_target);
    }
    let overwrote = (arrivals.iter())
        .filter(|&&(t, frame, _)| t > release_arrived && frame != release_target)
        .count();
    let mut drag: Vec<f64> = (calls.iter())
        .map(|&call| answered(&arrivals[..answerable], call).unwrap_or(f64::INFINITY))
        .collect();
    let mut answered: Vec<f64> = drag.iter().copied().filter(|l| l.is_finite()).collect();
    let unanswered = drag.len() - answered.len();
    let distinct: BTreeSet<i64> = (arrivals.iter())
        .filter(|&&(t, frame, _)| t <= 5_000.0 && frame <= target)
        .map(|&(_, frame, _)| frame)
        .collect();
    let after_release = (arrivals.iter())
        .filter(|&&(t, frame, _)| t >= release_at && frame != release_target)
        .count();
    let line = format!(
        "drag_p95_ms={:.1} drag_answered_p95_ms={:.1} drag_unanswered={unanswered} \
         drag_distinct_fps={:.1} \
         release_pending_drag_calls={pending} release_shown={} release_ms={:.1} \
         stale_frames_after_release={after_release} frames_over_release={overwrote} \
         valid_frames_over_release={valid_over}",
        percentile(&mut drag, 0.95),
        percentile(&mut answered, 0.95),
        distinct.len() as f64 / 5.0,
        release.is_some(),
        release.unwrap_or(f64::NAN),
    );
    (line, release.is_some())
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
    assert_release();
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
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(
            child.status.success(),
            "the P-rss child failed ({}): {stdout}{stderr}",
            child.status
        );
        // libtest prints `test … ... ` on the same line before the output.
        let line = (stdout.lines()).find_map(|line| Some(&line[line.find("PF1 rss ")?..]));
        let line = line.unwrap_or_else(|| panic!("no P-rss result: {stdout}{stderr}"));
        println!("{line} lane={lane} workload={key}");
    }
}

/// One P-rss measurement: before the engine, constructed (no document), first
/// render, settled idle after 6 s, and 10 s of playback (rss MiB/threads),
/// which must have advanced at least 5 s without an engine error; then the
/// engine's teardown record (`teardown`), before the process exits.
#[test]
#[ignore = "PF1 P-rss child: spawned by pf1_rss_baseline"]
fn pf1_rss_child() {
    let Some(path) = std::env::var_os("PF1_RSS_DOCUMENT") else {
        return;
    };
    assert_release();
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
    let played = session.engine.position().0 as f64 * frame_ms(&document);
    session.assert_no_error("P-rss playback");
    assert!(
        played >= 5_000.0,
        "P-rss playback advanced only {played} ms"
    );
    session.engine.pause();
    // E12.10 D3(a): wait for the engine's teardown record, as P-play does,
    // so the process never exits while the worker is still destroying the
    // GPU device.
    let teardown = teardown(session);
    println!(
        "PF1 rss before={} constructed={} first_render={} settled_idle={} playing={} \
         playing_peak_mib={:.1}{} played_s={:.1} {teardown}",
        show(before),
        show(constructed),
        show(first),
        show(settled),
        show(playing),
        mib(playing.peak),
        if peak_reset { "" } else { "(lifetime)" },
        played / 1e3,
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
    let mut clean = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0)).metrics(90, frame);
    assert_eq!(
        (clean.on_time, clean.late, clean.early, clean.dropped),
        (90, 0, 0, 0),
        "{clean:?}"
    );
    assert!(!clean.passes(), "an invalid run passes no gate");
    clean.valid = true;
    assert!(clean.passes(), "{clean:?}");
    let slowdown = synthetic(90, 60.0, (0.0, 0.0), (0.0, 0.0)).metrics(90, frame);
    assert!(
        // The last frame's delayed arrival is past the endpoint: dropped.
        !slowdown.g1_metric() && (slowdown.late, slowdown.dropped) == (89, 1),
        "slowdown: {slowdown:?}"
    );
    let freeze = synthetic(90, 10.0, (0.0, 0.0), (1_000.0, 1_000.0)).metrics(90, frame);
    assert!(
        !freeze.g14_metric() && freeze.held_max >= 1_000.0,
        "freeze: {freeze:?}"
    );
    let clock = synthetic(90, 10.0, (1_000.0, 1_000.0), (0.0, 0.0)).metrics(90, frame);
    assert!(
        !clock.g16_metric() && clock.clock_stall_max >= 1_000.0,
        "clock freeze: {clock:?}"
    );
    // A frame the clock never passed is dropped, however the run ends.
    let mut short = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0));
    short.samples.retain(|s| s.1 < 60);
    short.end = None;
    assert_eq!(short.metrics(90, frame).dropped, 30);
}

/// A-F1: frames received before the clock reaches them are never on time,
/// and repeated images improve neither the present intervals nor held age.
#[test]
// Repeated arrivals must leave the statistics bit-identical.
#[allow(clippy::float_cmp)]
fn pf1_metrics_reject_early_frames_and_repeated_images() {
    let frame = 1e3 / 30.0;
    let mut early = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0));
    let arrivals: Vec<_> = (0..90).map(|k| (0.0, k, 0)).collect();
    early.arrivals = arrivals;
    let m = early.metrics(90, frame);
    assert_eq!((m.on_time, m.early), (1, 89), "{m:?}");
    assert!(!m.g1_metric() && !m.g14_metric(), "{m:?}");
    let clean = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0));
    let mut repeated = synthetic(90, 10.0, (0.0, 0.0), (0.0, 0.0));
    for &(t, at, position) in &clean.arrivals {
        repeated.arrivals.push((t + 1.0, at, position));
    }
    repeated.arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (clean, repeated) = (clean.metrics(90, frame), repeated.metrics(90, frame));
    assert_eq!(repeated.present, clean.present, "{repeated:?}");
    assert_eq!(repeated.held_max, clean.held_max);
    // A frozen image repeated every 5 ms is still a held image.
    let mut frozen = synthetic(90, 10.0, (0.0, 0.0), (1_000.0, 1_000.0));
    let last_before = (frozen.arrivals.iter()).rfind(|a| a.0 < 1_000.0).copied();
    let (_, at, position) = last_before.expect("an image before the freeze");
    for step in 0..200 {
        frozen
            .arrivals
            .push((1_000.0 + f64::from(step) * 5.0, at, position));
    }
    frozen.arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let frozen = frozen.metrics(90, frame);
    assert!(frozen.held_max >= 1_000.0, "{frozen:?}");
}

/// Re-review B nit: P-seek repeats a target, and the earlier seek's image
/// of it may still be in flight. Only the frame stamped by the newer call
/// answers it: the stale one of the same target and a current one of
/// another target are passed over, and stale answers alone time out.
#[test]
fn pf1_a_repeated_seek_target_is_answered_only_by_its_own_call() {
    let frame = |at: i64, epoch: u64, seq: u64| PreviewFrame {
        at: TimeCode(at),
        stamp: FrameStamp { epoch, seq },
        texture: kinewright_core::FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 4]),
        },
    };
    let (sender, frames) = crossbeam_channel::unbounded();
    let issued = FrameStamp { epoch: 5, seq: 9 };
    for queued in [
        frame(10, 4, 7),
        frame(11, 5, 9),
        frame(10, 5, 9),
        frame(10, 5, 10),
    ] {
        sender.send(queued).unwrap();
    }
    let limit = Duration::from_millis(50);
    let answer = wait_answer(&frames, 10, issued, Instant::now(), limit);
    assert!(answer.is_some());
    assert_eq!(frames.len(), 1, "answered by the third: its own stamp");
    sender.send(frame(10, 4, 8)).unwrap();
    let later = FrameStamp { epoch: 6, seq: 11 };
    assert_eq!(wait_answer(&frames, 10, later, Instant::now(), limit), None);
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

/// V-3: the worker keeps filling while the preview thread is stuck in a 5 s
/// render: 3.2 s of stepped callbacks, 20 ms apart, never underrun (a worker
/// that rendered would starve the 1 s cushion within about a second).
#[test]
fn pf1_the_fill_runs_while_the_preview_renders() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let (audio, data) = (SimulatedAudio::stepped(), TempDirectory::new("pf1-v3"));
    let (faults, diagnostics) = (Arc::new(Faults::default()), Arc::default());
    faults.render_delay_ms.store(5_000, Ordering::Relaxed);
    let root = data.root().to_path_buf();
    let engine = FfmpegMediaEngine::new_for_harness(
        gpu,
        root,
        Some(audio.clone()),
        faults,
        Arc::clone(&diagnostics),
    )
    .expect("the engine starts");
    engine.set_document(Arc::new(perf_fixtures::title_card((64, 64), 900)));
    engine.play(TimeCode::ZERO);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !audio.advance(0) {
        assert!(Instant::now() < deadline, "the simulated stream opens");
        thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..150 {
        assert!(audio.advance(CALLBACK_FRAMES));
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(diagnostics.underrun_frames(), 0, "the worker kept filling");
    assert!(engine.position() >= TimeCode(95), "{:?}", engine.position());
    engine.pause();
}

/// V-5: the engine's clock moves only when the stepped driver runs a callback.
#[test]
fn pf1_engine_clock_follows_the_stepped_simulated_driver() {
    let Some(gpu) = fixture_gpu_or_skip() else {
        return;
    };
    let (audio, data) = (SimulatedAudio::stepped(), TempDirectory::new("pf1-stepped"));
    let root = data.root().to_path_buf();
    let diagnostics = Arc::new(AudioDiagnostics::default());
    let engine = FfmpegMediaEngine::new_for_harness(
        gpu,
        root,
        Some(audio.clone()),
        Arc::default(),
        Arc::clone(&diagnostics),
    )
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
