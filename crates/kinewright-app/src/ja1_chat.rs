//! Non-owning chat client. Only the writer host executes agent sessions.
use kinewright_agent::writer_host::{
    HostClient, HostCommand, HostError, HostReply, HostSessionConfig, HostSessionStatus,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct HostChatError {
    pub(crate) retention_lost: bool,
    pub(crate) message: String,
}
impl HostChatError {
    fn unconfirmed(message: impl Into<String>) -> Self {
        Self {
            retention_lost: false,
            message: message.into(),
        }
    }

    fn lost(message: impl Into<String>) -> Self {
        Self {
            retention_lost: true,
            message: message.into(),
        }
    }
}

struct ChatCompletion {
    session: Option<u64>,
    reply: Result<HostReply, HostError>,
}

pub(crate) struct HostChat {
    client: HostClient,
    id: Option<u64>,
    pending: Option<JoinHandle<ChatCompletion>>,
    last_poll: Instant,
    pub(crate) running: bool,
    cancelled: Arc<AtomicBool>,
}
impl HostChat {
    pub(crate) fn start(
        client: HostClient,
        harness: String,
        config: HostSessionConfig,
        message: String,
    ) -> Self {
        let request_client = client.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let requested_cancel = Arc::clone(&cancelled);
        let pending = Some(std::thread::spawn(move || {
            let mut session_id = None;
            let reply = (|| {
                let mut started =
                    request_client.request(HostCommand::StartSession { harness, config })?;
                if started.error.is_some() {
                    return Ok(started);
                }
                let session = started
                    .session
                    .as_ref()
                    .ok_or_else(|| HostError::Protocol("host omitted its session identity".into()))?
                    .id;
                session_id = Some(session);
                let sent = request_client.request(HostCommand::SendMessage { session, message })?;
                if sent.error.is_some() {
                    return Ok(sent);
                }
                if requested_cancel.load(Ordering::Acquire) {
                    let _ = request_client.request(HostCommand::InterruptSession { session });
                }
                let mut reply = request_client.request(HostCommand::SessionStatus { session })?;
                if let (Some(started), Some(current)) = (&mut started.session, &mut reply.session) {
                    let mut events = std::mem::take(&mut started.events);
                    events.append(&mut current.events);
                    current.events = events;
                }
                Ok(reply)
            })();
            ChatCompletion {
                session: session_id,
                reply,
            }
        }));
        Self {
            client,
            id: None,
            pending,
            last_poll: Instant::now(),
            running: true,
            cancelled,
        }
    }

    pub(crate) fn send(&mut self, message: String) -> Result<(), String> {
        let session = self.id.ok_or("The host session is still starting")?;
        if self.pending.is_some() {
            return Err("The host is still checking this session".into());
        }
        let client = self.client.clone();
        let requested_cancel = Arc::clone(&self.cancelled);
        self.pending = Some(std::thread::spawn(move || {
            let reply = (|| {
                let sent = client.request(HostCommand::SendMessage { session, message })?;
                if sent.error.is_some() {
                    return Ok(sent);
                }
                if requested_cancel.load(Ordering::Acquire) {
                    let _ = client.request(HostCommand::InterruptSession { session });
                }
                client.request(HostCommand::SessionStatus { session })
            })();
            ChatCompletion {
                session: Some(session),
                reply,
            }
        }));
        self.running = true;
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<Option<HostSessionStatus>, HostChatError> {
        if self.pending.as_ref().is_some_and(JoinHandle::is_finished) {
            let completed = self
                .pending
                .take()
                .expect("completed chat request")
                .join()
                .map_err(|_| HostChatError::unconfirmed("The host chat request stopped unexpectedly. Message delivery is unconfirmed; it will not be resent."))?;
            self.id = completed.session.or(self.id);
            self.last_poll = Instant::now();
            let reply = completed.reply.map_err(|error| {
                self.running = self.id.is_some();
                HostChatError::unconfirmed(format!(
                    "The host chat reply was not confirmed. Known session status will be checked again; an uncertain message will not be resent. {error}"
                ))
            })?;
            if let Some(error) = reply.error {
                return Err(HostChatError {
                    retention_lost: error.code == "retention_lost",
                    message: error.to_string(),
                });
            }
            if !reply.status.retention_proven {
                return Err(HostChatError::lost(
                    "Source retention was not re-established",
                ));
            }
            let status = reply
                .session
                .ok_or_else(|| HostChatError::unconfirmed("The host omitted session status"))?;
            self.id = Some(status.id);
            self.running = status.running;
            self.last_poll = Instant::now();
            return Ok(Some(status));
        }
        if self.pending.is_none()
            && self.running
            && self.last_poll.elapsed() >= Duration::from_millis(50)
            && let Some(session) = self.id
        {
            let client = self.client.clone();
            self.pending = Some(std::thread::spawn(move || ChatCompletion {
                session: Some(session),
                reply: client.request(HostCommand::SessionStatus { session }),
            }));
        }
        Ok(None)
    }

    /// Explicit Stop only. Dropping/detaching this client never sends cancellation.
    pub(crate) fn interrupt(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(session) = self.id {
            let client = self.client.clone();
            let _ = std::thread::spawn(move || {
                client.request(HostCommand::InterruptSession { session })
            });
        }
    }
}

#[cfg(test)]
#[path = "ja1_chat_tests.rs"]
mod tests;
