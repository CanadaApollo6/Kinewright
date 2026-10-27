//! Minimal Agent Client Protocol transport for local NDJSON agents.
//!
//! The transport owns the child process, keeps stdout on one reader thread,
//! routes JSON-RPC replies to their waiting callers, and exposes agent-to-
//! client requests plus notifications on a single channel. Provider-specific
//! session behavior belongs in the driver, not in this protocol layer.

use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::protocol::limits::{EVENTS, EventSink, Frames, encoded_frame, stderr_capture};
#[cfg(test)]
use crossbeam_channel::unbounded;
use crossbeam_channel::{Receiver, Sender, bounded};
use kinewright_core::{AgentError, AgentEvent};
use serde_json::{Value, json};

use crate::child_process::hide_console_window;

type PendingReply = Result<Value, AgentError>;
const WRITE_BUDGET: Duration = Duration::from_secs(2);

struct PipeWrite {
    bytes: Vec<u8>,
    finished: Sender<Result<(), AgentError>>,
}

#[derive(Debug, Clone)]
pub(crate) enum AcpIncoming {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// One line could not be understood. The stream is still alive: the
    /// reader skipped it and keeps going, so a consumer must report it and
    /// carry on. A banner or a deprecation warning on stdout arrives here.
    Malformed(String),
    /// The stream is gone. Every pending request has already been failed;
    /// the consumer's loop ends here.
    Fault(String),
}

pub(crate) struct AcpPendingRequest {
    id: u64,
    reply: Receiver<PendingReply>,
    pending: Arc<Mutex<HashMap<u64, Sender<PendingReply>>>>,
}

/// A pending request the caller drops without waiting (a fire-and-forget
/// `turn/cancel`) must not leave its reply slot in the map for the life of
/// the session.
impl Drop for AcpPendingRequest {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

impl AcpPendingRequest {
    pub(crate) fn wait(self, timeout: Duration) -> Result<Value, AgentError> {
        match self.reply.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&self.id);
                }
                Err(AgentError::Protocol(format!(
                    "ACP request {} timed out after {} seconds",
                    self.id,
                    timeout.as_secs()
                )))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(AgentError::Protocol(
                "ACP reply channel closed unexpectedly".to_owned(),
            )),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AcpClient {
    inner: Arc<AcpInner>,
}

struct AcpInner {
    child: Mutex<Child>,
    writes: Sender<PipeWrite>,
    #[cfg(all(test, target_os = "linux"))]
    writing: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<u64, Sender<PendingReply>>>>,
    incoming_rx: Receiver<AcpIncoming>,
    next_id: AtomicU64,
    interrupted: Arc<AtomicBool>,
    ingress_failed: Arc<AtomicBool>,
}

impl AcpClient {
    pub(crate) fn spawn(
        mut command: ProcessCommand,
        process_name: &'static str,
    ) -> Result<Self, AgentError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);

        let mut child = command.spawn().map_err(|error| {
            AgentError::Harness(format!("could not start {process_name}: {error}"))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AgentError::Harness(format!("{process_name} stdin was not available"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AgentError::Harness(format!("{process_name} stdout was not available"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AgentError::Harness(format!("{process_name} stderr was not available"))
        })?;

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (incoming_tx, incoming_rx) = bounded(EVENTS);
        let interrupted = Arc::new(AtomicBool::new(false));
        let ingress_failed = Arc::new(AtomicBool::new(false));
        let (writes, queued) = bounded::<PipeWrite>(1);
        let writing = Arc::new(AtomicBool::new(false));
        let writer_busy = Arc::clone(&writing);
        let writer_interrupted = Arc::clone(&interrupted);
        let mut stdin = stdin;
        if let Err(error) = thread::Builder::new()
            .name("kinewright-acp-writer".into())
            .spawn(move || {
                for write in queued {
                    if writer_interrupted.load(Ordering::Acquire) {
                        break;
                    }
                    writer_busy.store(true, Ordering::Release);
                    let result = stdin
                        .write_all(&write.bytes)
                        .and_then(|()| stdin.flush())
                        .map_err(|error| AgentError::Harness(error.to_string()));
                    writer_busy.store(false, Ordering::Release);
                    let failed = result.is_err();
                    let _ = write.finished.try_send(result);
                    if failed {
                        break;
                    }
                }
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AgentError::Harness(error.to_string()));
        }
        spawn_reader(
            stdout,
            stderr,
            Arc::clone(&pending),
            incoming_tx,
            Arc::clone(&interrupted),
            process_name,
            Arc::clone(&ingress_failed),
        )?;

        Ok(Self {
            inner: Arc::new(AcpInner {
                child: Mutex::new(child),
                writes,
                #[cfg(all(test, target_os = "linux"))]
                writing,
                pending,
                incoming_rx,
                next_id: AtomicU64::new(1),
                interrupted,
                ingress_failed,
            }),
        })
    }

    pub(crate) fn ingress_failed(&self) -> bool {
        self.inner.ingress_failed.load(Ordering::Acquire)
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn inner_writer_busy_for_test(&self) -> bool {
        self.inner.writing.load(Ordering::Acquire)
    }
    #[cfg(all(test, unix))]
    pub(crate) fn child_reaped(&self) -> bool {
        self.inner
            .child
            .lock()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_some()
    }

    pub(crate) fn incoming(&self) -> Receiver<AcpIncoming> {
        self.inner.incoming_rx.clone()
    }

    pub(crate) fn request(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, AgentError> {
        let started = Instant::now();
        self.begin_request_with_budget(method, params, timeout.min(WRITE_BUDGET))?
            .wait(timeout.saturating_sub(started.elapsed()))
    }

    pub(crate) fn begin_request(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<AcpPendingRequest, AgentError> {
        self.begin_request_with_budget(method, params, WRITE_BUDGET)
    }

    fn begin_request_with_budget(
        &self,
        method: &str,
        params: &Value,
        budget: Duration,
    ) -> Result<AcpPendingRequest, AgentError> {
        if self.inner.interrupted.load(Ordering::Acquire)
            || (self.ingress_failed() && !restore_or_cancel(method))
        {
            return Err(AgentError::Protocol("ingress: interrupted".into()));
        }
        if method.len() > 256 {
            return Err(AgentError::Protocol("ingress: method_capacity".into()));
        }
        encoded_frame(params)?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply) = bounded(1);
        reserve_reply(&self.inner.pending, id, reply_tx)?;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message_with_budget(&message, budget) {
            if let Ok(mut pending) = self.inner.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        Ok(AcpPendingRequest {
            id,
            reply,
            pending: Arc::clone(&self.inner.pending),
        })
    }

    pub(crate) fn notify(&self, method: &str, params: &Value) -> Result<(), AgentError> {
        encoded_frame(params)?;
        if method.len() > 256 {
            return Err(AgentError::Protocol("ingress: method_capacity".into()));
        }
        // `params` is optional in JSON-RPC: a null params member is not the
        // same as an absent one (MSP's `initialized` rejects the former).
        let mut message = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params.clone();
        }
        self.write_message(&message)
    }

    pub(crate) fn respond(&self, id: &Value, result: &Value) -> Result<(), AgentError> {
        encoded_frame(id)?;
        encoded_frame(result)?;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    pub(crate) fn kill(&self) {
        self.inner.interrupted.store(true, Ordering::Release);
        self.stop_child();
    }

    fn stop_child(&self) {
        if let Ok(mut child) = self.inner.child.lock() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }

    fn write_message(&self, message: &Value) -> Result<(), AgentError> {
        self.write_message_with_budget(message, WRITE_BUDGET)
    }

    fn write_message_with_budget(
        &self,
        message: &Value,
        budget: Duration,
    ) -> Result<(), AgentError> {
        let restore = message
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(restore_or_cancel);
        if self.inner.interrupted.load(Ordering::Acquire) || (self.ingress_failed() && !restore) {
            return Err(AgentError::Protocol("ingress: interrupted".into()));
        }
        let mut bytes = encoded_frame(message)?;
        bytes.push(b'\n');
        let (finished, completion) = bounded(1);
        if self
            .inner
            .writes
            .try_send(PipeWrite { bytes, finished })
            .is_err()
        {
            return self.write_failed("write_capacity");
        }
        self.wait_write(&completion, budget, restore)
    }

    fn wait_write(
        &self,
        completion: &Receiver<Result<(), AgentError>>,
        budget: Duration,
        restore: bool,
    ) -> Result<(), AgentError> {
        let deadline = Instant::now() + budget;
        loop {
            if self.inner.interrupted.load(Ordering::Acquire) || (self.ingress_failed() && !restore)
            {
                return Err(AgentError::Protocol("ingress: interrupted".into()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.write_failed("write_timeout");
            }
            match completion.recv_timeout(remaining.min(Duration::from_millis(10))) {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(_)) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return self.write_failed("write_closed");
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn write_failed(&self, reason: &str) -> Result<(), AgentError> {
        self.inner.ingress_failed.store(true, Ordering::Release);
        // Reaping uses only the process lock. The blocked writer owns its pipe.
        // Leave the reader free to publish Fault after the child closes stdout.
        self.stop_child();
        let message = format!("ingress: {reason}");
        fail_pending(&self.inner.pending, &message);
        Err(AgentError::Protocol(message))
    }
}

impl Drop for AcpInner {
    /// Kill, never wait.
    ///
    /// This runs on whichever thread happens to hold the last handle, and
    /// that is not always a worker: a session's own clone can outlive its
    /// teardown thread, and then the last drop lands on the caller — the
    /// egui frame. `waitpid` there is an unbounded wait on somebody else's
    /// process. Reaping belongs to [`AcpClient::kill`], which every teardown
    /// worker and every catalog probe calls; by the time this runs the child
    /// is normally already reaped, and the `try_wait` below collects it.
    fn drop(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        if let Ok(child) = self.child.get_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            // One non-blocking reap for the usual case where the signal has
            // already landed. Anything slower is the worker's to collect.
            let _ = child.try_wait();
        }
    }
}

fn spawn_reader(
    stdout: impl Read + Send + 'static,
    stderr: impl Read + Send + 'static,
    pending: Arc<Mutex<HashMap<u64, Sender<PendingReply>>>>,
    incoming: Sender<AcpIncoming>,
    interrupted: Arc<AtomicBool>,
    process_name: &'static str,
    ingress_failed: Arc<AtomicBool>,
) -> Result<(), AgentError> {
    let stderr_reader = stderr_capture(stderr, "kinewright-acp-stderr")?;

    thread::Builder::new()
        .name("kinewright-acp-reader".to_owned())
        .spawn(move || {
            let mut failure = None;
            for line in Frames::new(stdout) {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        failure = Some(format!("{process_name} stream error: {error}"));
                        break;
                    }
                };
                let message = match serde_json::from_str::<Value>(&line) {
                    Ok(message) => message,
                    Err(error) => {
                        if !offer_incoming(
                            &incoming,
                            AcpIncoming::Malformed(format!(
                                "{process_name} sent invalid NDJSON: {error}"
                            )),
                        ) {
                            failure = Some("ingress: event_capacity; attempt interrupted".into());
                            break;
                        }
                        continue;
                    }
                };
                if !route_message(&message, &pending, &incoming) {
                    failure = Some("ingress: event_capacity; attempt interrupted".into());
                    break;
                }
            }
            // Read interruption first so cleanup cannot hide an earlier write fault.
            // Freeze this decision before exposing the raw-reader failure latch.
            let notify_fault =
                !interrupted.load(Ordering::Acquire) || ingress_failed.load(Ordering::Acquire);
            ingress_failed.store(true, Ordering::Release);
            let stderr_text = if stderr_reader.is_finished() {
                stderr_reader.join().unwrap_or_default()
            } else {
                String::new()
            };
            let detail = stderr_text.trim();
            let message = failure.unwrap_or_else(|| {
                if detail.is_empty() {
                    format!("{process_name} closed its ACP stream")
                } else {
                    format!("{process_name} closed its ACP stream: {detail}")
                }
            });
            fail_pending(&pending, &message);
            if notify_fault {
                let _ = incoming.try_send(AcpIncoming::Fault(message));
            }
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

fn route_message(
    message: &Value,
    pending: &Mutex<HashMap<u64, Sender<PendingReply>>>,
    incoming: &Sender<AcpIncoming>,
) -> bool {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = message.get("id") {
            return offer_incoming(
                incoming,
                AcpIncoming::Request {
                    id: id.clone(),
                    method: method.to_owned(),
                    params,
                },
            );
        }
        return offer_incoming(
            incoming,
            AcpIncoming::Notification {
                method: method.to_owned(),
                params,
            },
        );
    }

    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return offer_incoming(
            incoming,
            AcpIncoming::Malformed(
                "ACP message had neither a method nor a numeric reply id".to_owned(),
            ),
        );
    };
    let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
    if let Some(sender) = sender {
        let reply = if let Some(error) = message.get("error") {
            Err(AgentError::Protocol(rpc_error_text(error)))
        } else {
            Ok(message.get("result").cloned().unwrap_or(Value::Null))
        };
        let _ = sender.try_send(reply);
    }
    true
}

fn reserve_reply(
    pending: &Mutex<HashMap<u64, Sender<PendingReply>>>,
    id: u64,
    reply: Sender<PendingReply>,
) -> Result<(), AgentError> {
    let mut pending = pending
        .lock()
        .map_err(|_| AgentError::Harness("ACP pending-request lock was poisoned".into()))?;
    if pending.len() >= EVENTS {
        return Err(AgentError::Protocol("ingress: reply_capacity".into()));
    }
    pending.insert(id, reply);
    Ok(())
}

fn restore_or_cancel(method: &str) -> bool {
    matches!(
        method,
        "session/set_config_option" | "session/cancel" | "turn/cancel"
    )
}

fn offer_incoming(incoming: &Sender<AcpIncoming>, value: AcpIncoming) -> bool {
    if incoming.len() >= EVENTS - 1 {
        return false;
    }
    incoming.try_send(value).is_ok()
}

fn rpc_error_text(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("ACP request failed");
    match error.get("data") {
        Some(data) if !data.is_null() => format!("{message}: {data}"),
        _ => message.to_owned(),
    }
}

fn fail_pending(pending: &Mutex<HashMap<u64, Sender<PendingReply>>>, message: &str) {
    let senders = pending
        .lock()
        .map(|mut map| map.drain().map(|(_, sender)| sender).collect::<Vec<_>>())
        .unwrap_or_default();
    for sender in senders {
        let _ = sender.send(Err(AgentError::Harness(message.to_owned())));
    }
}

pub(crate) fn send_done(events: &impl EventSink, done: &AtomicBool) {
    if !done.swap(true, Ordering::AcqRel) {
        events.emit(AgentEvent::Done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4_prior_write_fault_survives_cleanup_before_eof() {
        let (sender, receiver) = bounded(EVENTS);
        spawn_reader(
            std::io::empty(),
            std::io::empty(),
            Arc::new(Mutex::new(HashMap::new())),
            sender,
            Arc::new(AtomicBool::new(true)),
            "write fault before EOF fixture",
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        assert!(
            matches!(
                receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
                AcpIncoming::Fault(_)
            ),
            "an already latched write fault must retain its terminal signal"
        );
    }

    #[test]
    fn s4_raw_fault_is_retained_when_cleanup_races_publication() {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let publication_barrier = pending.lock().unwrap();
        let failed = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = bounded(EVENTS);
        spawn_reader(
            std::io::Cursor::new(vec![b'x'; crate::protocol::limits::FRAME_BYTES + 1]),
            std::io::empty(),
            Arc::clone(&pending),
            sender,
            Arc::clone(&interrupted),
            "publication race fixture",
            Arc::clone(&failed),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !failed.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "raw failure must precede publication"
            );
            thread::sleep(Duration::from_millis(1));
        }
        interrupted.store(true, Ordering::Release);
        drop(publication_barrier);
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), AcpIncoming::Fault(reason) if reason.contains("frame_capacity")),
            "cleanup cannot suppress the retained fault"
        );
    }

    #[test]
    fn s4_acp_pending_reply_map_is_bounded() {
        let pending = Mutex::new(HashMap::new());
        let mut receivers = vec![];
        for n in 0..EVENTS {
            let (sender, receiver) = bounded(1);
            reserve_reply(&pending, n as u64, sender).unwrap();
            receivers.push(receiver);
        }
        assert!(reserve_reply(&pending, 99, bounded(1).0).is_err());
        assert_eq!(
            pending
                .lock()
                .unwrap()
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            (0..EVENTS as u64).collect()
        );
        fail_pending(&pending, "retained interruption");
        for receiver in receivers {
            assert!(
                matches!(receiver.try_recv().unwrap(),Err(AgentError::Harness(reason)) if reason=="retained interruption")
            );
        }
    }

    #[test]
    fn s4_acp_reader_capacity_retains_accepted_and_fails_waiters() {
        use std::fmt::Write as _;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (reply, waiter) = bounded(1);
        pending.lock().unwrap().insert(7, reply);
        let (sender, receiver) = bounded(EVENTS);
        let mut bytes = String::new();
        for n in 0..EVENTS + 4 {
            writeln!(bytes, "{{\"method\":\"event-{n}\",\"params\":{{}}}}").unwrap();
        }
        spawn_reader(
            std::io::Cursor::new(bytes.into_bytes()),
            std::io::empty(),
            Arc::clone(&pending),
            sender,
            Arc::new(AtomicBool::new(false)),
            "fixture",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(
            waiter
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        for n in 0..EVENTS - 1 {
            assert!(
                matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(),AcpIncoming::Notification {method,..} if method==format!("event-{n}"))
            );
        }
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(),AcpIncoming::Fault(reason) if reason.contains("event_capacity"))
        );
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn s4_acp_oversized_malformed_frame_is_interrupted_before_json() {
        let (sender, receiver) = bounded(EVENTS);
        spawn_reader(
            std::io::Cursor::new(vec![b'x'; 2 * crate::protocol::limits::FRAME_BYTES]),
            std::io::empty(),
            Arc::new(Mutex::new(HashMap::new())),
            sender,
            Arc::new(AtomicBool::new(false)),
            "fixture",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(),AcpIncoming::Fault(reason) if reason.contains("frame_capacity"))
        );
    }

    #[test]
    fn routes_replies_requests_and_notifications_without_reader_contention() {
        let pending = Mutex::new(HashMap::new());
        let (reply_tx, reply_rx) = bounded(1);
        pending.lock().unwrap().insert(7, reply_tx);
        let (incoming_tx, incoming_rx) = bounded(EVENTS);

        route_message(
            &json!({"jsonrpc":"2.0","id":7,"result":{"sessionId":"s1"}}),
            &pending,
            &incoming_tx,
        );
        assert_eq!(reply_rx.recv().unwrap().unwrap()["sessionId"], "s1");

        route_message(
            &json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1"}}),
            &pending,
            &incoming_tx,
        );
        assert!(matches!(
            incoming_rx.recv().unwrap(),
            AcpIncoming::Notification { method, .. } if method == "session/update"
        ));

        route_message(
            &json!({"jsonrpc":"2.0","id":"permission-1","method":"session/request_permission","params":{}}),
            &pending,
            &incoming_tx,
        );
        assert!(matches!(
            incoming_rx.recv().unwrap(),
            AcpIncoming::Request { id, method, .. }
                if id == "permission-1" && method == "session/request_permission"
        ));
    }

    #[test]
    fn rpc_errors_reach_the_matching_waiter() {
        let pending = Mutex::new(HashMap::new());
        let (reply_tx, reply_rx) = bounded(1);
        pending.lock().unwrap().insert(3, reply_tx);
        let (incoming_tx, _incoming_rx) = unbounded();
        route_message(
            &json!({"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"no session"}}),
            &pending,
            &incoming_tx,
        );
        assert_eq!(
            reply_rx.recv().unwrap().unwrap_err(),
            AgentError::Protocol("no session".to_owned())
        );
    }

    /// A banner, a progress line or a reply with a string id is one bad line,
    /// not a dead stream. The reader must skip it, say so once, and keep
    /// delivering — the whole session used to go deaf on the first one.
    /// Portable: `spawn_reader` takes two `impl Read`, so no child is needed.
    #[test]
    fn an_unparseable_line_is_malformed_and_the_stream_survives_it() {
        let stdout = concat!(
            "OpenCode v1.2.3 starting…\n",
            r#"{"jsonrpc":"2.0","id":"not-a-number","result":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#,
            "\n",
        );
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (reply_tx, reply_rx) = bounded(1);
        pending.lock().unwrap().insert(1, reply_tx);
        let (incoming_tx, incoming_rx) = bounded(EVENTS);
        spawn_reader(
            std::io::Cursor::new(stdout.as_bytes().to_vec()),
            std::io::empty(),
            Arc::clone(&pending),
            incoming_tx,
            Arc::new(AtomicBool::new(false)),
            "Fake ACP",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        let first = incoming_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(&first, AcpIncoming::Malformed(message) if message.contains("invalid NDJSON")),
            "{first:?}"
        );
        let second = incoming_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(&second, AcpIncoming::Malformed(message) if message.contains("reply id")),
            "{second:?}"
        );
        // The valid notification after both bad lines still arrives...
        assert!(matches!(
            incoming_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AcpIncoming::Notification { method, .. } if method == "session/update"
        ));
        // ...and so does the reply the caller is waiting on.
        assert_eq!(
            reply_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()["sessionId"],
            "s1"
        );
        // Only the end of the stream is a `Fault`.
        assert!(matches!(
            incoming_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AcpIncoming::Fault(message) if message.contains("closed its ACP stream")
        ));
    }
}
