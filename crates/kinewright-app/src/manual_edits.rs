//! The person's latest desired value survives a temporarily full manual door.
use crate::joined_thread::JoinedThread;
use kinewright_core::{Command, CoreDisconnected, CoreMutationRefusal, Event, ManualPermit};
use std::collections::VecDeque;

pub(crate) const PENDING_ACTIONS: usize = 16;
pub(crate) const PENDING_WORDS: &str =
    "An edit is waiting to be applied. Its final value is kept here; it is not saved yet.";
const FULL_WORDS: &str = "Too many edits are waiting to be applied. Wait for them to finish, then try this edit again. Your earlier pending edits are still kept.";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ManualSendError {
    Disconnected,
    PendingCapacity,
}
impl std::fmt::Display for ManualSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Disconnected => "The Core actor has stopped.",
            Self::PendingCapacity => FULL_WORDS,
        })
    }
}
impl From<CoreDisconnected> for ManualSendError {
    fn from(_: CoreDisconnected) -> Self {
        Self::Disconnected
    }
}

struct PendingAction {
    // The identity stays here while Core owns a copy of the command.
    gesture: Option<String>,
    ended: bool,
    latest: Option<Command>,
    waiting_at: Option<u64>,
}
struct InFlight {
    command: Command,
    acknowledgement: u64,
    worker: JoinedThread<Result<Event, CoreDisconnected>>,
}
enum Notice {
    Refused(String),
    Full,
}
impl Notice {
    fn text(&self) -> &str {
        match self {
            Self::Refused(text) => text,
            Self::Full => FULL_WORDS,
        }
    }
}
#[derive(Default)]
pub(crate) struct ManualEdits {
    actions: VecDeque<PendingAction>,
    in_flight: Option<InFlight>,
    acknowledgement: u64,
    notice: Option<Notice>,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) before_send: Option<Box<dyn FnOnce() + Send>>,
}
impl ManualEdits {
    pub(crate) fn submit(
        &mut self,
        command: Command,
        permit: &ManualPermit,
    ) -> Result<(), ManualSendError> {
        if let Command::DoBatchCoalesced { coalesce_key, .. } = &command
            && let Some(action) = self
                .actions
                .iter_mut()
                .find(|a| a.gesture.as_ref() == Some(coalesce_key))
        {
            action.latest = Some(command);
            self.notice = None;
            self.start_next(permit);
            return Ok(());
        }
        if self.actions.len() >= PENDING_ACTIONS {
            self.notice = Some(Notice::Full);
            return Err(ManualSendError::PendingCapacity);
        }
        let gesture = match &command {
            Command::DoBatchCoalesced { coalesce_key, .. } => Some(coalesce_key.clone()),
            _ => None,
        };
        self.actions.push_back(PendingAction {
            ended: gesture.is_none(),
            gesture,
            latest: Some(command),
            waiting_at: None,
        });
        self.notice = None;
        self.start_next(permit);
        Ok(())
    }
    pub(crate) fn end_gestures(&mut self) {
        for action in &mut self.actions {
            action.ended = true;
        }
    }
    pub(crate) fn acknowledged(&mut self) {
        self.acknowledgement = self.acknowledgement.wrapping_add(1);
    }
    pub(crate) fn pending(&self) -> bool {
        self.in_flight.is_some() || self.actions.iter().any(|a| !a.ended || a.latest.is_some())
    }
    pub(crate) fn message(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .map(Notice::text)
            .or_else(|| self.pending().then_some(PENDING_WORDS))
    }
    pub(crate) fn attempt_running(&self) -> bool {
        self.in_flight.is_some()
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn action_count(&self) -> usize {
        self.actions.len()
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn waiting_for_ack(&self) -> bool {
        self.actions
            .front()
            .is_some_and(|a| a.waiting_at == Some(self.acknowledgement))
            && self.in_flight.is_none()
    }
    pub(crate) fn collect_ready(&mut self) {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|f| f.worker.is_finished())
        {
            let flight = self.in_flight.take().expect("finished manual request");
            let result = flight.worker.join();
            let action = self.actions.front_mut().expect("in-flight action retained");
            match result {
                Ok(Ok(Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualBackpressure,
                    ..
                })) => {
                    // Only acknowledgement can cure this refusal. A newer gesture value wins.
                    if action.latest.is_none() {
                        action.latest = Some(flight.command);
                    }
                    action.waiting_at = Some(flight.acknowledgement);
                }
                Ok(Ok(Event::DocumentChanged { .. })) => {
                    action.waiting_at = None;
                    if matches!(self.notice, Some(Notice::Refused(_))) {
                        self.notice = None;
                    }
                }
                other => {
                    self.notice = Some(Notice::Refused(match other {
                        Ok(Ok(Event::MutationRefused { reason, .. })) => reason.to_string(),
                        Ok(Ok(Event::OpRejected { error, .. })) => error.to_string(),
                        Ok(Ok(Event::BatchRejected { error, .. })) => error.to_string(),
                        Ok(Ok(Event::RevisionConflict { .. })) => "The document changed before this edit could be applied. Check its current value and try again.".into(),
                        Ok(Ok(Event::UndoBlockedBySave { .. })) => "Undo is waiting for the save to finish. Retry after the destination is reconciled.".into(),
                        _ => "The edit outcome could not be confirmed because its worker stopped. Check the document before trying again.".into(),
                    }));
                    // A final refusal belongs to the sent value. A newer value remains a new command.
                    action.waiting_at = None;
                    action.ended = true;
                }
            }
        }
    }
    pub(crate) fn start_next(&mut self, permit: &ManualPermit) {
        if self.in_flight.is_some() {
            return;
        }
        while self
            .actions
            .front()
            .is_some_and(|a| a.ended && a.latest.is_none())
        {
            self.actions.pop_front();
        }
        let Some(action) = self.actions.front_mut() else {
            return;
        };
        if action.waiting_at == Some(self.acknowledgement) {
            return;
        }
        let Some(command) = action.latest.take() else {
            return;
        };
        // Keep the exact command for a refused attempt; Core consumes its independent copy.
        let sent = command.clone();
        // This capability crosses to the acceptance worker; the UI keeps its original owner.
        let permit = permit.clone();
        #[cfg(all(test, target_os = "linux"))]
        let before_send = self.before_send.take();
        match std::thread::Builder::new()
            .name("manual-edit".into())
            .spawn(move || {
                #[cfg(all(test, target_os = "linux"))]
                if let Some(hook) = before_send {
                    hook();
                }
                permit.request(sent)
            }) {
            Ok(worker) => {
                self.in_flight = Some(InFlight {
                    command,
                    acknowledgement: self.acknowledgement,
                    worker: JoinedThread::new(worker),
                });
            }
            Err(error) => {
                action.ended = true;
                self.notice = Some(Notice::Refused(format!(
                    "This edit could not be sent: {error}. Check the document and try again."
                )));
            }
        }
    }
}
