//! PF1 S2c Amendment R55 (Rec): process-wide test-build counters for P-seek's
//! backward phase. They measure; they change nothing.
//!
//! - frames the decoders received;
//! - colour conversions, split into t's (and every other) and window frames'
//!   (`convert_retained`), each timed in its two steps: the filter graph
//!   (YUV to RGBA64, swscale) and the working-frame step; plus the calling
//!   thread's CPU time inside the graph (about equal to the graph's wall
//!   time when it runs on that thread alone);
//! - paused jobs: the time from the post to every required frame ready (the
//!   wait), and the render after it; jobs whose required frames were all
//!   resident at the post (a hit served pre-converted);
//! - S-3 refills: started, the decoded bytes they keep, and windows that end
//!   fully converted (a refill that keeps nothing counts as full).
//!
//! Amendment R59: the preview's admission waits (K-2's `Admission::Wait`
//! until the set is admitted: count, total and longest), and the invariant
//! "a reader idle while holding discard charges" (0 in every harness run).
//!
//! Amendment R56 (trace): while a harness has it on, a timeline of events
//! (ns since the first, a kind, a source time, the reader's id or −1): the
//! harness's steps, the preview's posts, frames ready and renders done, and
//! the readers' refills, window frames kept, conversions, other decodes and
//! waits.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(crate) const DECODED: usize = 0;
pub(crate) const CONVERT_T: usize = 1; // count, graph ns, work ns, graph cpu ns
pub(crate) const CONVERT_W: usize = 5;
pub(crate) const JOBS: usize = 9; // count, wait ns, render ns
pub(crate) const PRECONVERTED: usize = 12;
pub(crate) const REFILLS: usize = 13;
pub(crate) const KEPT_BYTES: usize = 14;
pub(crate) const WINDOWS_FULL: usize = 15;
pub(crate) const ADMIT_WAITS: usize = 16; // count, total ns, longest ns
pub(crate) const IDLE_DISCARD: usize = 19;
pub(crate) const LEN: usize = 20;

static COUNTS: [AtomicU64; LEN] = [const { AtomicU64::new(0) }; LEN];

pub(crate) fn add(index: usize, value: u64) {
    COUNTS[index].fetch_add(value, Ordering::Relaxed);
}

pub(crate) fn snapshot() -> [u64; LEN] {
    std::array::from_fn(|i| COUNTS[i].load(Ordering::Relaxed))
}

fn nanos(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// One conversion: `window` for a kept window frame.
pub(crate) fn converted(
    window: bool,
    graph: std::time::Duration,
    work: std::time::Duration,
    graph_cpu: u64,
) {
    let base = if window { CONVERT_W } else { CONVERT_T };
    add(base, 1);
    add(base + 1, nanos(graph));
    add(base + 2, nanos(work));
    add(base + 3, graph_cpu);
}

/// Amendment R59: the preview's admission is waiting (`true`) or has
/// admitted (`false`); a wait is timed from its first `Admission::Wait`.
pub(crate) fn admission(waiting: bool) {
    static SINCE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut since = SINCE.lock().expect("admission");
    match (waiting, *since) {
        (true, None) => {
            *since = Some(std::time::Instant::now());
            add(ADMIT_WAITS, 1);
            trace(Event::AdmitWait, -1);
        }
        (false, Some(start)) => {
            *since = None;
            let waited = nanos(start.elapsed());
            add(ADMIT_WAITS + 1, waited);
            COUNTS[ADMIT_WAITS + 2].fetch_max(waited, Ordering::Relaxed);
            trace(Event::Admitted, -1);
        }
        _ => {}
    }
}

/// Amendment R59: a harness run starts: the longest admission wait is
/// reset; returns the counters to take deltas from.
pub(crate) fn start_run() -> [u64; LEN] {
    COUNTS[ADMIT_WAITS + 2].store(0, Ordering::Relaxed);
    snapshot()
}

/// Amendment R59: reader `id` dropped its discarded kept frames above
/// `bound` (a discard-only job).
pub(crate) fn discarded(id: u64, bound: i64) {
    traced(Event::Discard, bound, i64::try_from(id).unwrap_or(-1));
}

/// Amendment R59: reader `id` went idle holding discard charges (never, by
/// the invariant).
pub(crate) fn idle_with_discards(id: u64) {
    add(IDLE_DISCARD, 1);
    traced(Event::IdleDiscard, -1, i64::try_from(id).unwrap_or(-1));
}

/// One paused job's wait for its frames and its render.
pub(crate) fn job(wait: std::time::Duration, render: std::time::Duration) {
    add(JOBS, 1);
    add(JOBS + 1, nanos(wait));
    add(JOBS + 2, nanos(render));
}

/// The calling thread's CPU time so far, in ns (`/proc/thread-self/schedstat`;
/// 0 where it cannot be read).
pub(crate) fn thread_cpu_ns() -> u64 {
    let stat = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let first = stat.split_whitespace().next();
    first.and_then(|ns| ns.parse().ok()).unwrap_or(0)
}

/// The process's CPU time so far (user + system), in ms (`/proc/self/stat`,
/// clock ticks of 10 ms; 0 where it cannot be read).
pub(crate) fn process_cpu_ms() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after the command's closing parenthesis: utime is the 12th, stime the 13th.
    let rest = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
    let fields: Vec<u64> = (rest.split_whitespace())
        .skip(11)
        .take(2)
        .filter_map(|field| field.parse().ok())
        .collect();
    fields.iter().sum::<u64>() * 10
}

/// Amendment R56: the trace's event kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The harness issues a step to `at` (before `seek`).
    Step,
    /// The harness saw the step's frame (`at`).
    Seen,
    /// The preview posts a paused job requiring `at`.
    Post,
    /// The job's frames are ready (render starts).
    Ready,
    /// The job's render is done.
    Rendered,
    /// A reader starts a refill to t = `at`, and ends it.
    RefillStart,
    RefillEnd,
    /// A refill kept the decoded frame at `at` (the window's queue).
    Kept,
    /// A reader starts converting the kept frame at `at`, and ends.
    ConvertStart,
    ConvertEnd,
    /// A reader starts any other decode of `at`, and ends it.
    DecodeStart,
    DecodeEnd,
    /// A reader goes idle (`Next::Wait`).
    Wait,
    /// Amendment R59: the preview's admission waits, and admits.
    AdmitWait,
    Admitted,
    /// Amendment R59: a reader drops its discarded kept frames (`at`: the
    /// bound) with no decode; a reader idles holding discard charges.
    Discard,
    IdleDiscard,
}

static TRACING: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Vec<(u64, Event, i64, i64)>> = Mutex::new(Vec::new());
static BASE: OnceLock<std::time::Instant> = OnceLock::new();

/// Amendment R56: record `event` at `at` if a harness traces.
pub(crate) fn trace(event: Event, at: i64) {
    traced(event, at, -1);
}

/// Amendment R56: record reader `id`'s `event` at `at` if a harness traces.
fn traced(event: Event, at: i64, id: i64) {
    if !TRACING.load(Ordering::Relaxed) {
        return;
    }
    let base = BASE.get_or_init(std::time::Instant::now);
    let ns = nanos(base.elapsed());
    TRACE.lock().expect("trace").push((ns, event, at, id));
}

/// Amendment R56: start a trace (an empty one).
pub(crate) fn trace_on() {
    TRACE.lock().expect("trace").clear();
    TRACING.store(true, Ordering::Relaxed);
}

/// Amendment R61: whether a harness traces now.
pub(crate) fn tracing() -> bool {
    TRACING.load(Ordering::Relaxed)
}

/// Amendment R56: stop tracing and take the events.
pub(crate) fn trace_off() -> Vec<(u64, Event, i64, i64)> {
    TRACING.store(false, Ordering::Relaxed);
    std::mem::take(&mut *TRACE.lock().expect("trace"))
}

/// Amendment R56: a reader starts decoding `at`: a refill (`from`, paused),
/// a kept frame's conversion or another decode. Returns its end event.
pub(crate) fn decode_start(
    (id, decoder): (u64, Option<&crate::decode::VideoDecoder>),
    at: i64,
    from: Option<i64>,
    paused: bool,
) -> Event {
    if !TRACING.load(Ordering::Relaxed) {
        return Event::DecodeEnd;
    }
    let kept = decoder.is_some_and(|decoder| decoder.kept_times().contains(&at));
    let (start, end) = match (from.filter(|_| paused), kept) {
        (Some(_), _) => (Event::RefillStart, Event::RefillEnd),
        (None, true) => (Event::ConvertStart, Event::ConvertEnd),
        (None, false) => (Event::DecodeStart, Event::DecodeEnd),
    };
    traced(start, at, i64::try_from(id).unwrap_or(-1));
    end
}

/// Amendment R56: the decode of `at` ended (`end`); a refill's kept
/// frames follow it, ascending.
pub(crate) fn decode_end(
    (id, decoder): (u64, Option<&crate::decode::VideoDecoder>),
    at: i64,
    end: Event,
) {
    let id = i64::try_from(id).unwrap_or(-1);
    traced(end, at, id);
    if end == Event::RefillEnd && TRACING.load(Ordering::Relaxed) {
        let kept = decoder.map(crate::decode::VideoDecoder::kept_times);
        for at in kept.unwrap_or_default() {
            traced(Event::Kept, at, id);
        }
    }
}

/// Amendment R56: reader `id` goes idle (its next decode ends the wait).
pub(crate) fn waits(id: u64) {
    traced(Event::Wait, -1, i64::try_from(id).unwrap_or(-1));
}
