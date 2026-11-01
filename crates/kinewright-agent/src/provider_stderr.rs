//! What Kinewright keeps from a provider's stderr: only which known errors
//! appeared. Provider-written bytes are never retained, shown or stored, on
//! any adapter path. A person sees a fixed sentence, the exit status and, at
//! most, one fixed hint chosen by matching a known error.

use std::{
    io::{self, Read},
    process::ExitStatus,
    thread::{self, JoinHandle},
};

/// Provider errors the host recognizes. Only the fixed text is ever shown or
/// stored; the matched provider bytes are not kept.
const KNOWN_PROVIDER_ERRORS: [(&[u8], &str); 1] = [(
    b"Codex executable path is not configured",
    "Codex could not resolve its own executable path",
)];

/// Which known error, if any, appeared on a provider's stderr.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StderrSummary {
    known: Option<&'static str>,
}

/// Drain provider stderr on its own thread and match it against the fixed
/// allowlist. Draining is required: an unread pipe blocks the child once it
/// fills. Memory is bounded to one read buffer plus the longest pattern.
pub(crate) fn stderr_summary(
    mut reader: impl Read + Send + 'static,
    thread_name: &str,
) -> io::Result<JoinHandle<StderrSummary>> {
    thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let longest = KNOWN_PROVIDER_ERRORS
                .iter()
                .map(|(pattern, _)| pattern.len())
                .max()
                .unwrap_or(1);
            let mut summary = StderrSummary::default();
            let mut window = Vec::with_capacity(8192 + longest);
            let mut chunk = [0_u8; 8192];
            loop {
                let read = match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                window.extend_from_slice(&chunk[..read]);
                if summary.known.is_none() {
                    summary.known = KNOWN_PROVIDER_ERRORS
                        .iter()
                        .find(|(pattern, _)| {
                            window.windows(pattern.len()).any(|part| part == *pattern)
                        })
                        .map(|(_, text)| *text);
                }
                let keep = longest.saturating_sub(1).min(window.len());
                window.drain(..window.len() - keep);
            }
            summary
        })
}

/// Join a summary thread. A panicked or missing reader yields no hint.
pub(crate) fn join_summary(reader: JoinHandle<StderrSummary>) -> StderrSummary {
    reader.join().unwrap_or_default()
}

/// Append the fixed hint for a matched known error, if any.
pub(crate) fn with_hint(base: String, summary: StderrSummary) -> String {
    match summary.known {
        Some(text) => format!("{base}: {text}"),
        None => base,
    }
}

/// The person-facing message for a provider process that exited unsuccessfully
/// before its turn finished. `provider` is a fixed adapter name.
pub(crate) fn exit_message(provider: &str, status: ExitStatus, summary: StderrSummary) -> String {
    let base = match status.code() {
        Some(code) => format!("{provider} exited with status {code} before replying"),
        // The text is the standard library's rendering of the signal number.
        None => format!("{provider} stopped before replying ({status})"),
    };
    with_hint(base, summary)
}

/// C5 fix 5 witness: each direct adapter's reader runs against a real child
/// that writes a synthetic credential-looking value to stderr and exits 1.
/// Returns every error-carrying output each adapter produced.
#[cfg(any(test, feature = "eval-harness"))]
pub mod witness {
    use kinewright_core::AgentEvent;
    use std::{
        collections::HashMap,
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex, atomic::AtomicBool},
    };

    /// The synthetic value the fixture child prints. Never a real credential.
    pub const ADAPTER_STDERR_SECRET: &str = "refresh_token=synthetic-secret";

    fn failing_child() -> Child {
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sh");
            command.arg("-c").arg(format!(
                "printf '%s\\n' 'fixture: {ADAPTER_STDERR_SECRET}' >&2; exit 1"
            ));
            command
        };
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt as _;
            let mut command = Command::new("cmd");
            command.raw_arg(format!(
                "/C echo fixture: {ADAPTER_STDERR_SECRET} 1>&2 & exit /b 1"
            ));
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the platform shell runs the fixture child")
    }

    /// `(adapter, outputs)` for the Codex, Copilot and ACP readers.
    ///
    /// # Panics
    ///
    /// When the platform shell cannot run the fixture child or a reader
    /// thread cannot start. This is a test witness, not a product path.
    #[must_use]
    pub fn adapter_stderr_witness() -> Vec<(&'static str, Vec<AgentEvent>)> {
        let mut out = Vec::new();

        let mut child = failing_child();
        let (stdout, stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let (sender, receiver) = crate::protocol::limits::event_channel();
        crate::drivers::spawn_codex_reader(
            stdout,
            stderr,
            Arc::new(Mutex::new(Some((1, child)))),
            sender,
            Arc::new(AtomicBool::new(false)),
            1,
        )
        .unwrap()
        .join()
        .unwrap();
        out.push(("codex", receiver.try_iter().collect()));

        let mut child = failing_child();
        let (stdout, stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let (sender, receiver) = crossbeam_channel::unbounded();
        crate::copilot::spawn_copilot_reader(
            stdout,
            stderr,
            Arc::new(Mutex::new(Some(child))),
            sender,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        // The reader drops its sender when the child's stdout ends.
        out.push(("copilot", receiver.iter().collect()));

        let mut child = failing_child();
        let (stdout, stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let (reply, replies) = crossbeam_channel::bounded(1);
        let pending = Arc::new(Mutex::new(HashMap::from([(7_u64, reply)])));
        let (incoming, faults) = crossbeam_channel::unbounded();
        crate::acp::spawn_reader(
            stdout,
            stderr,
            pending,
            incoming,
            Arc::new(AtomicBool::new(false)),
            "Fixture ACP agent",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let mut acp = Vec::new();
        // The session turns a fault into an error event (`acp_session.rs`),
        // and a pending request receives it as its `AgentError`. Both
        // senders drop when the reader thread ends with the child's stdout.
        for message in &faults {
            if let crate::acp::AcpIncoming::Fault(message) = message {
                acp.push(AgentEvent::Error(message));
            }
        }
        if let Ok(Err(error)) = replies.recv() {
            acp.push(AgentEvent::Error(error.to_string()));
        }
        let _ = child.wait();
        out.push(("acp", acp));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic stderr a provider might print before dying. It carries a
    /// credential-looking value that must never reach an event or the UI.
    const SYNTHETIC_SECRET: &str = "refresh_token=synthetic-secret";

    #[test]
    fn s4b_c5_fix5_summary_keeps_no_provider_bytes() {
        let mut stderr = format!("error: auth failed {SYNTHETIC_SECRET}\n").into_bytes();
        stderr.extend(std::iter::repeat_n(b'z', 100_000));
        stderr.extend_from_slice(b"Codex executable path is not configured\n");
        let summary = join_summary(stderr_summary(io::Cursor::new(stderr), "fix5-test").unwrap());
        assert_eq!(
            summary.known,
            Some("Codex could not resolve its own executable path")
        );
        let plain = join_summary(
            stderr_summary(io::Cursor::new(SYNTHETIC_SECRET.as_bytes()), "fix5-test").unwrap(),
        );
        assert_eq!(plain, StderrSummary::default());
        assert_eq!(
            with_hint("Codex closed its stream".to_owned(), summary),
            "Codex closed its stream: Codex could not resolve its own executable path"
        );
    }
}
