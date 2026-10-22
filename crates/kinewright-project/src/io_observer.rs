//! Scoped, thread-local observations of existing project writes. No durability
//! or ordering behavior is supplied by this observer.
use std::{cell::RefCell, path::PathBuf};

use serde::Serialize;

type Hook = (&'static str, Box<dyn FnMut()>);
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
/// Schedule a real Core command at a production save barrier on this thread.
/// # Panics
/// Refuses nesting; the callback and operation may propagate their own panic.
pub fn at_barrier<T>(
    point: &'static str,
    callback: impl FnMut() + 'static,
    run: impl FnOnce() -> T,
) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOOK.with_borrow_mut(|hook| *hook = None);
        }
    }
    HOOK.with_borrow_mut(|hook| {
        assert!(hook.is_none());
        *hook = Some((point, Box::new(callback)));
    });
    let _reset = Reset;
    run()
}
pub(crate) fn at_test_hook(point: &str) {
    HOOK.with_borrow_mut(|hook| {
        if let Some((expected, callback)) = hook
            && point == *expected
        {
            callback();
        }
    });
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncProfile {
    pub calls: u64,
    pub nanoseconds: u128,
}
thread_local! { static HISTORY_BOUND: RefCell<SyncProfile> = const { RefCell::new(SyncProfile { calls: 0, nanoseconds: 0 }) }; }
pub(crate) fn note_history_bound(duration: std::time::Duration) {
    HISTORY_BOUND.with_borrow_mut(|p| {
        p.calls += 1;
        p.nanoseconds += duration.as_nanos();
    });
}
#[must_use]
pub fn history_bound_profile() -> SyncProfile {
    HISTORY_BOUND.with_borrow(Clone::clone)
}
thread_local! { static HISTORY_DECODES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
pub(crate) fn note_history_decode() {
    HISTORY_DECODES.with(|count| {
        count.set(
            count
                .get()
                .checked_add(1)
                .expect("test decode counter overflow"),
        );
    });
}
/// Number of full history decodes on this thread, separate from mutation observations.
#[must_use]
pub fn history_decode_count() -> u64 {
    HISTORY_DECODES.with(std::cell::Cell::get)
}
thread_local! {static SYNC_PROFILE: RefCell<Option<SyncProfile>>=const{RefCell::new(None)};}
#[cfg(target_os = "linux")]
pub(crate) fn measure_sync<T, E>(run: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    let started = std::time::Instant::now();
    let result = run();
    SYNC_PROFILE.with_borrow_mut(|s| {
        if let Some(s) = s {
            s.calls += 1;
            s.nanoseconds += started.elapsed().as_nanos();
        }
    });
    result
}
/// Snapshot the active thread's counters without nesting a profiling scope.
#[must_use]
pub fn sync_profile() -> SyncProfile {
    SYNC_PROFILE.with_borrow(|s| s.clone().unwrap_or_default())
}

/// Time the actual sync calls on this thread; independent from object/hash counters.
/// # Panics
/// Refuses nested profiling scopes.
pub fn profile_sync<T>(run: impl FnOnce() -> T) -> (T, SyncProfile) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            SYNC_PROFILE.with_borrow_mut(|s| *s = None);
        }
    }
    SYNC_PROFILE.with_borrow_mut(|s| {
        assert!(s.is_none());
        *s = Some(SyncProfile::default());
    });
    let _reset = Reset;
    let result = run();
    let profile = SYNC_PROFILE
        .with_borrow_mut(Option::take)
        .expect("profile installed");
    (result, profile)
}

#[derive(Debug, Clone, Serialize)]
pub struct IoObservation {
    pub operation: &'static str,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempted_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempted_length: Option<usize>,
}

#[derive(Debug, Default, Serialize)]
pub struct IoTrace {
    pub events: Vec<IoObservation>,
    pub overflowed: bool,
}

thread_local! {
    static TRACE: RefCell<Option<IoTrace>> = const { RefCell::new(None) };
}

pub(crate) fn note(operation: &'static str, path: &std::path::Path) {
    note_bytes(operation, path, None);
}

pub(crate) fn note_bytes(operation: &'static str, path: &std::path::Path, bytes: Option<&[u8]>) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace {
            if trace.events.len() == 4096 {
                trace.overflowed = true;
            } else {
                trace.events.push(IoObservation {
                    operation,
                    path: path.to_path_buf(),
                    attempted_sha256: bytes.map(kinewright_media::sha256_bytes),
                    attempted_length: bytes.map(<[u8]>::len),
                });
            }
        }
    });
}

pub(crate) fn note_digest(
    operation: &'static str,
    path: &std::path::Path,
    sha256: String,
    length: usize,
) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace {
            if trace.events.len() == 4096 {
                trace.overflowed = true;
            } else {
                trace.events.push(IoObservation {
                    operation,
                    path: path.to_path_buf(),
                    attempted_sha256: Some(sha256),
                    attempted_length: Some(length),
                });
            }
        }
    });
}
/// Test observation at a caller's actual mutation entry, including app-private journal work.
pub fn mutation_attempt(operation: &'static str, path: &std::path::Path) {
    note(operation, path);
}

/// Capture only calls made by this thread in `run`. Nested observations refuse.
/// # Panics
/// Panics on a nested observation. Unwinding still clears the observer.
pub fn observe<T>(run: impl FnOnce() -> T) -> (T, IoTrace) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACE.with_borrow_mut(|trace| *trace = None);
        }
    }
    TRACE.with_borrow_mut(|trace| {
        assert!(trace.is_none(), "nested IO observer");
        *trace = Some(IoTrace::default());
    });
    let _reset = Reset;
    let result = run();
    let trace = TRACE
        .with_borrow_mut(Option::take)
        .expect("observer installed");
    (result, trace)
}

thread_local! { static FAILURE: RefCell<Option<&'static str>> = const {RefCell::new(None)}; }
thread_local! { static PATH_FAILURE: RefCell<Option<(&'static str, PathBuf)>> = const {RefCell::new(None)}; }
/// Keep one file undeletable for the entire scope, including every retry.
/// # Panics
/// Refuses nested persistent fault scopes.
pub fn fail_path<T>(operation: &'static str, path: &std::path::Path, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PATH_FAILURE.with_borrow_mut(|f| *f = None);
        }
    }
    PATH_FAILURE.with_borrow_mut(|f| {
        assert!(f.is_none());
        *f = Some((operation, path.to_path_buf()));
    });
    let _reset = Reset;
    run()
}
/// Inject one operation error in a fixture-owned temporary namespace.
/// # Panics
/// Refuses nested fault scopes. Unwinding clears the scope.
pub fn fail_once<T>(operation: &'static str, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAILURE.with_borrow_mut(|failure| *failure = None);
        }
    }
    FAILURE.with_borrow_mut(|failure| {
        assert!(failure.is_none(), "nested failure scope");
        *failure = Some(operation);
    });
    let _reset = Reset;
    run()
}
pub(crate) fn check_failure(
    operation: &'static str,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let fail = FAILURE.with_borrow_mut(|failure| {
        if *failure == Some(operation) {
            *failure = None;
            true
        } else {
            false
        }
    });
    let persistent = PATH_FAILURE.with_borrow(|failure| {
        failure
            .as_ref()
            .is_some_and(|(op, p)| *op == operation && p == path)
    });
    if fail || persistent {
        note("injected_failure", path);
        Err(std::io::Error::other(format!("injected {operation}")))
    } else {
        Ok(())
    }
}
