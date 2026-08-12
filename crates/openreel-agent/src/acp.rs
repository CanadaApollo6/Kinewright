//! Minimal Agent Client Protocol transport for local NDJSON agents.
//!
//! The transport owns the child process, keeps stdout on one reader thread,
//! routes JSON-RPC replies to their waiting callers, and exposes agent-to-
//! client requests plus notifications on a single channel. Provider-specific
//! session behavior belongs in the driver, not in this protocol layer.

use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader, Read, Write as _},
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use openreel_core::{AgentError, AgentEvent};
use serde_json::{Value, json};

type PendingReply = Result<Value, AgentError>;

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
    Fault(String),
}

pub(crate) struct AcpPendingRequest {
    id: u64,
    reply: Receiver<PendingReply>,
    pending: Arc<Mutex<HashMap<u64, Sender<PendingReply>>>>,
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
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, Sender<PendingReply>>>>,
    incoming_rx: Receiver<AcpIncoming>,
    next_id: AtomicU64,
    interrupted: Arc<AtomicBool>,
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
        let (incoming_tx, incoming_rx) = unbounded();
        let interrupted = Arc::new(AtomicBool::new(false));
        spawn_reader(
            stdout,
            stderr,
            Arc::clone(&pending),
            incoming_tx,
            Arc::clone(&interrupted),
            process_name,
        )?;

        Ok(Self {
            inner: Arc::new(AcpInner {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                pending,
                incoming_rx,
                next_id: AtomicU64::new(1),
                interrupted,
            }),
        })
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
        self.begin_request(method, params)?.wait(timeout)
    }

    pub(crate) fn begin_request(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<AcpPendingRequest, AgentError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply) = bounded(1);
        self.inner
            .pending
            .lock()
            .map_err(|_| AgentError::Harness("ACP pending-request lock was poisoned".to_owned()))?
            .insert(id, reply_tx);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&message) {
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
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub(crate) fn respond(&self, id: &Value, result: &Value) -> Result<(), AgentError> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    pub(crate) fn kill(&self) {
        self.inner.interrupted.store(true, Ordering::Release);
        if let Ok(mut child) = self.inner.child.lock() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }

    fn write_message(&self, message: &Value) -> Result<(), AgentError> {
        let mut stdin = self
            .inner
            .stdin
            .lock()
            .map_err(|_| AgentError::Harness("ACP stdin lock was poisoned".to_owned()))?;
        serde_json::to_writer(&mut *stdin, message)
            .map_err(|error| AgentError::Protocol(error.to_string()))?;
        stdin
            .write_all(b"\n")
            .and_then(|()| stdin.flush())
            .map_err(|error| AgentError::Harness(format!("could not write to ACP agent: {error}")))
    }
}

impl Drop for AcpInner {
    fn drop(&mut self) {
        self.interrupted.store(true, Ordering::Release);
        if let Ok(child) = self.child.get_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
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
) -> Result<(), AgentError> {
    let stderr_reader = thread::Builder::new()
        .name("openreel-acp-stderr".to_owned())
        .spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        })
        .map_err(|error| AgentError::Harness(error.to_string()))?;

    thread::Builder::new()
        .name("openreel-acp-reader".to_owned())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = incoming.send(AcpIncoming::Fault(format!(
                            "{process_name} stream error: {error}"
                        )));
                        break;
                    }
                };
                let message = match serde_json::from_str::<Value>(&line) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = incoming.send(AcpIncoming::Fault(format!(
                            "{process_name} sent invalid NDJSON: {error}"
                        )));
                        continue;
                    }
                };
                route_message(&message, &pending, &incoming);
            }

            let stderr_text = stderr_reader.join().unwrap_or_default();
            let detail = stderr_text.trim();
            let message = if detail.is_empty() {
                format!("{process_name} closed its ACP stream")
            } else {
                format!("{process_name} closed its ACP stream: {detail}")
            };
            fail_pending(&pending, &message);
            if !interrupted.load(Ordering::Acquire) {
                let _ = incoming.send(AcpIncoming::Fault(message));
            }
        })
        .map(|_| ())
        .map_err(|error| AgentError::Harness(error.to_string()))
}

fn route_message(
    message: &Value,
    pending: &Mutex<HashMap<u64, Sender<PendingReply>>>,
    incoming: &Sender<AcpIncoming>,
) {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = message.get("id") {
            let _ = incoming.send(AcpIncoming::Request {
                id: id.clone(),
                method: method.to_owned(),
                params,
            });
        } else {
            let _ = incoming.send(AcpIncoming::Notification {
                method: method.to_owned(),
                params,
            });
        }
        return;
    }

    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        let _ = incoming.send(AcpIncoming::Fault(
            "ACP message had neither a method nor a numeric reply id".to_owned(),
        ));
        return;
    };
    let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
    if let Some(sender) = sender {
        let reply = if let Some(error) = message.get("error") {
            Err(AgentError::Protocol(rpc_error_text(error)))
        } else {
            Ok(message.get("result").cloned().unwrap_or(Value::Null))
        };
        let _ = sender.send(reply);
    }
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

pub(crate) fn send_done(events: &Sender<AgentEvent>, done: &AtomicBool) {
    if !done.swap(true, Ordering::AcqRel) {
        let _ = events.send(AgentEvent::Done);
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut ProcessCommand) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_replies_requests_and_notifications_without_reader_contention() {
        let pending = Mutex::new(HashMap::new());
        let (reply_tx, reply_rx) = bounded(1);
        pending.lock().unwrap().insert(7, reply_tx);
        let (incoming_tx, incoming_rx) = unbounded();

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
}
