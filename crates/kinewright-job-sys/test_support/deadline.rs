//! Test-only watchdog for native calls (including joins and child I/O) that
//! cannot return an error to the test thread once they stall.
use std::{
    io::Write as _,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    time::Duration,
};

#[must_use]
pub fn start(reason: &'static str, timeout: Duration) -> Sender<()> {
    let test = std::thread::current();
    let (done, wait) = mpsc::channel();
    std::thread::Builder::new()
        .name("ms2-test-deadline".into())
        .spawn(move || {
            if wait.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                // Bypass libtest's output capture: the blocked test thread
                // never gets a chance to report its captured output.
                let test = test.name().unwrap_or("unnamed");
                let _ = writeln!(
                    std::io::stderr(),
                    "MS2_TEST_DEADLINE {reason} test={test} ({timeout:?})"
                );
                std::process::exit(124);
            }
        })
        .expect("start MS2 test deadline");
    done
}

#[must_use]
pub fn test() -> Sender<()> {
    start("native fixture", Duration::from_secs(120))
}
