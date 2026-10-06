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

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const DECODED: usize = 0;
pub(crate) const CONVERT_T: usize = 1; // count, graph ns, work ns, graph cpu ns
pub(crate) const CONVERT_W: usize = 5;
pub(crate) const JOBS: usize = 9; // count, wait ns, render ns
pub(crate) const PRECONVERTED: usize = 12;
pub(crate) const REFILLS: usize = 13;
pub(crate) const KEPT_BYTES: usize = 14;
pub(crate) const WINDOWS_FULL: usize = 15;
pub(crate) const LEN: usize = 16;

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
