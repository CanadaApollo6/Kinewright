//! Child-process plumbing shared by every harness driver.
//!
//! All ten drivers spawn CLIs the same way: hide the console window on
//! Windows, read a short `--version`/status command's output (preferring
//! stdout and falling back to stderr, because several CLIs print their
//! version banner on stderr), allocate an empty scratch working directory,
//! and drain a running child's stderr. The drain is not cosmetic: a piped
//! stderr nobody reads blocks the child once the pipe buffer fills (~64 KiB
//! on Linux, ~4 KiB on Windows), which wedges the turn with no diagnostic.

use std::{
    env, fs,
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command as ProcessCommand, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use kinewright_core::AgentError;

/// How long one detection or version probe may take before it is killed and
/// reported as no answer. `Command::output()` has no timeout of its own, and
/// several of these probes are network round trips (`devin auth status`,
/// `cursor-agent status`): without a bound, one unreachable endpoint or
/// captive-portal Wi-Fi hangs the caller for ever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// One counter for every harness: scratch names only need to be unique
/// within this process, and the pid and timestamp separate processes.
static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
pub(crate) fn hide_console_window(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn hide_console_window(_command: &mut ProcessCommand) {}

/// A finished command's text: stdout when it wrote any, else stderr.
pub(crate) fn command_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    if stdout.trim().is_empty() {
        String::from_utf8_lossy(stderr).into_owned()
    } else {
        stdout.into_owned()
    }
}

/// Run one short CLI command to completion and return its text. `None` when
/// the command could not be spawned or exited non-zero.
pub(crate) fn process_output(command: ProcessCommand, arguments: &[&str]) -> Option<String> {
    let output = run_to_completion(command, arguments)?;
    output
        .status
        .success()
        .then(|| command_output_text(&output.stdout, &output.stderr))
}

/// As [`process_output`], but keeping the text of a failing run: some CLIs
/// report "not logged in" on a non-zero exit and the message is the answer.
pub(crate) fn process_output_unchecked(
    command: ProcessCommand,
    arguments: &[&str],
) -> Option<String> {
    run_to_completion(command, arguments)
        .map(|output| command_output_text(&output.stdout, &output.stderr))
}

/// `Command::output()` with [`PROBE_TIMEOUT`] applied. Both pipes are drained
/// on their own threads, as `output()` does, so a chatty probe cannot fill one
/// and block; a probe that outlives the budget is killed and reported as no
/// answer, which every caller already treats as "unknown".
fn run_to_completion(mut command: ProcessCommand, arguments: &[&str]) -> Option<Output> {
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(&mut command);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take().and_then(spawn_pipe_reader);
    let stderr = child.stderr.take().and_then(spawn_pipe_reader);
    let status = wait_with_timeout(&mut child, PROBE_TIMEOUT)?;
    Some(Output {
        status,
        stdout: stdout
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default(),
        stderr: stderr
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default(),
    })
}

/// Poll for the child with a growing backoff, so a fast probe costs about a
/// millisecond and a wedged one is killed at the deadline.
fn wait_with_timeout(child: &mut Child, budget: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + budget;
    let mut nap = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(20));
    }
}

fn spawn_pipe_reader(pipe: impl Read + Send + 'static) -> Option<JoinHandle<Vec<u8>>> {
    thread::Builder::new()
        .name("kinewright-probe-pipe".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(pipe).read_to_end(&mut bytes);
            bytes
        })
        .ok()
}

/// Create a fresh empty directory for one harness session to run in.
/// `infix` names the harness in both the directory name and the error.
pub(crate) fn create_scratch_directory(infix: &str) -> Result<PathBuf, AgentError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..16 {
        let counter = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "kinewright-{infix}-{}-{now}-{counter}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(AgentError::Harness(format!(
                    "could not create the {infix} scratch directory: {error}"
                )));
            }
        }
    }
    Err(AgentError::Harness(format!(
        "could not allocate a unique {infix} scratch directory"
    )))
}

/// Drain a running child's stderr on its own thread and keep the text for
/// the failure message. Joining the handle waits for the child's stderr to
/// close, so join it only after its stdout has ended.
pub(crate) fn spawn_stderr_capture(
    stderr: impl Read + Send + 'static,
    thread_name: &str,
) -> Result<JoinHandle<String>, AgentError> {
    thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        })
        .map_err(|error| AgentError::Harness(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_text_prefers_stdout_and_falls_back_to_stderr() {
        assert_eq!(
            command_output_text(b"  \n", b"version 1.2.3"),
            "version 1.2.3"
        );
        assert_eq!(
            command_output_text(b"version 4.5.6", b"version 1.2.3"),
            "version 4.5.6"
        );
        assert_eq!(command_output_text(b"", b""), "");
    }

    #[test]
    fn scratch_directories_are_unique_and_empty() {
        let first = create_scratch_directory("unit-test").unwrap();
        let second = create_scratch_directory("unit-test").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_dir(&first).unwrap().count(), 0);
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("kinewright-unit-test-")
        );
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }
}
