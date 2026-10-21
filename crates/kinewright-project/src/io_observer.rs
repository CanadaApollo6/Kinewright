//! Scoped, thread-local observations of existing project writes. No durability
//! or ordering behavior is supplied by this observer.
use std::{cell::RefCell, path::PathBuf};

use serde::Serialize;

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
            if trace.events.len() == 128 {
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
