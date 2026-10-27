//! Bounded production readers, queues and transient reservations.
//! Original Kinewright ingress implementation. H1 owns durable publication.
#![allow(clippy::missing_errors_doc)]
#![allow(dead_code, reason = "some admission ports await the H1 owner")]

#[path = "usage.rs"]
pub mod usage;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use kinewright_core::{AgentError, AgentEvent};
use kinewright_project::orchestration_accounting::{AccountingRefusal, encode};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

pub const FRAME_BYTES: usize = 64 * 1024;
pub const PIPE_BYTES: usize = 64 * 1024;
pub const EVENTS: usize = 32;
pub const COMMANDS: usize = 64;
pub const COMMAND_BYTES: usize = 256 * 1024;
pub const CONTROL_BYTES: u64 = 64 * 1024 * 1024;

/// Never calls `read_line` or `read_until`. a missing delimiter cannot grow a buffer.
pub struct Frames<R> {
    reader: BufReader<R>,
    stopped: bool,
}
impl<R: Read> Frames<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::with_capacity(PIPE_BYTES, reader),
            stopped: false,
        }
    }
}
impl<R: Read> Iterator for Frames<R> {
    type Item = io::Result<String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped {
            return None;
        }
        let mut frame = Vec::new();
        loop {
            let available = match self.reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.stopped = true;
                    return Some(Err(error));
                }
            };
            if available.is_empty() {
                self.stopped = true;
                return if frame.is_empty() {
                    None
                } else {
                    Some(
                        String::from_utf8(frame)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                    )
                };
            }
            let end = available.iter().position(|b| *b == b'\n');
            let take = end.unwrap_or(available.len());
            if take > FRAME_BYTES.saturating_sub(frame.len()) {
                self.stopped = true;
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ingress: frame_capacity",
                )));
            }
            frame.reserve_exact(take);
            frame.extend_from_slice(&available[..take]);
            self.reader.consume(take + usize::from(end.is_some()));
            if end.is_some() {
                if frame.last() == Some(&b'\r') {
                    frame.pop();
                }
                return Some(
                    String::from_utf8(frame)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                );
            }
        }
    }
}

pub fn encoded_frame(value: &impl serde::Serialize) -> Result<Vec<u8>, AgentError> {
    encode(value, FRAME_BYTES)
        .map_err(|_| AgentError::Protocol("ingress: encoding_capacity".into()))
}

pub fn write_frame(
    writer: &mut impl Write,
    value: &impl serde::Serialize,
) -> Result<(), AgentError> {
    let bytes = encoded_frame(value)?;
    writer
        .write_all(&bytes)
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|e| AgentError::Harness(e.to_string()))
}

/// Drain forever with a fixed ring and a bounded retained diagnostic prefix.
pub fn stderr_capture(
    mut reader: impl Read + Send + 'static,
    name: &str,
) -> Result<JoinHandle<String>, AgentError> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut ring = vec![0; PIPE_BYTES];
            let mut retained = Vec::new();
            loop {
                match reader.read(&mut ring) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let keep = n.min(FRAME_BYTES.saturating_sub(retained.len()));
                        retained.reserve_exact(keep);
                        retained.extend_from_slice(&ring[..keep]);
                    }
                }
            }
            String::from_utf8(retained).unwrap_or_else(|_| "ingress: stderr_invalid_utf8".into())
        })
        .map_err(|e| AgentError::Harness(e.to_string()))
}

/// Two slots are reserved for an overflow reason and terminal observation.
/// A full queue interrupts admission without losing previously accepted events.
#[derive(Clone)]
pub struct EventSender {
    sender: Sender<AgentEvent>,
    failed: Arc<Mutex<bool>>,
}
pub fn event_channel() -> (EventSender, Receiver<AgentEvent>) {
    let (sender, receiver) = bounded(EVENTS);
    (
        EventSender {
            sender,
            failed: Arc::new(Mutex::new(false)),
        },
        receiver,
    )
}
impl EventSender {
    pub fn send(&self, event: AgentEvent) -> Result<(), crossbeam_channel::SendError<AgentEvent>> {
        let mut failed = self
            .failed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *failed {
            return Err(crossbeam_channel::SendError(event));
        }
        if (self.sender.len() >= EVENTS - 2 && !matches!(event, AgentEvent::Done))
            || self.sender.len() >= EVENTS
            || event_bytes(&event) > FRAME_BYTES
        {
            *failed = true;
            let _ = self.sender.try_send(AgentEvent::Error(
                "ingress: event_capacity; attempt interrupted".into(),
            ));
            let _ = self.sender.try_send(AgentEvent::Done);
            return Err(crossbeam_channel::SendError(event));
        }
        self.sender.try_send(event).map_err(|e| match e {
            TrySendError::Full(e) | TrySendError::Disconnected(e) => {
                crossbeam_channel::SendError(e)
            }
        })
    }
    #[must_use]
    pub fn failed(&self) -> bool {
        *self
            .failed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn close_admission(&self) {
        *self
            .failed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    }
    pub fn interrupt(&self, reason: &str) {
        let mut failed = self
            .failed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*failed {
            *failed = true;
            let _ = self.sender.try_send(AgentEvent::Error(reason.into()));
            let _ = self.sender.try_send(AgentEvent::Done);
        }
    }
}

pub trait EventSink {
    fn emit(&self, event: AgentEvent);
}
impl EventSink for EventSender {
    fn emit(&self, event: AgentEvent) {
        let _ = self.send(event);
    }
}
impl EventSink for Sender<AgentEvent> {
    fn emit(&self, event: AgentEvent) {
        let _ = self.try_send(event);
    }
}

fn event_bytes(event: &AgentEvent) -> usize {
    match event {
        AgentEvent::Text(text) | AgentEvent::Error(text) => text.len(),
        AgentEvent::ToolCall { name, arguments } => name.len().saturating_add(arguments.len()),
        AgentEvent::ToolResult { name, result } => name.len().saturating_add(result.len()),
        _ => std::mem::size_of::<AgentEvent>(),
    }
}

/// The existing project's physical ledger owns transient command charges.
pub struct AdmittedCommand {
    pub bytes: Vec<u8>,
    _reservation: kinewright_project::storage_budget::StorageReservation,
    _pin: kinewright_project::store::StorePin,
}
pub struct CommandQueue {
    sender: Sender<AdmittedCommand>,
    pub receiver: Receiver<AdmittedCommand>,
}
impl CommandQueue {
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = bounded(COMMANDS);
        Self { sender, receiver }
    }
    pub fn admit(
        &self,
        store: &kinewright_project::store::ProjectStore,
        value: &impl serde::Serialize,
        dependencies: &[kinewright_project::bundle_records::ObjectRef],
    ) -> Result<(), AccountingRefusal> {
        let reservation = store
            .reserve_copy(COMMAND_BYTES as u64)
            .map_err(|_| AccountingRefusal::Capacity)?;
        let pin = store
            .pin(dependencies)
            .map_err(|_| AccountingRefusal::Capacity)?;
        let bytes = encode(value, COMMAND_BYTES)?;
        self.sender
            .try_send(AdmittedCommand {
                bytes,
                _reservation: reservation,
                _pin: pin,
            })
            .map_err(|_| AccountingRefusal::Capacity)
    }
}
impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}
