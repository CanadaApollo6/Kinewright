//! The person's latest desired value survives a temporarily full manual door.
use crate::joined_thread::JoinedThread;
use kinewright_core::{
    Command, CoreDisconnected, CoreMutationRefusal, Event, ManualPermit, TimelineRevision,
};
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
    latest: Option<PendingValue>,
    /// Keep the accepted request while its gesture is open: a widget frame may
    /// not have received the corresponding document event yet. Moved, not copied.
    accepted: Option<Command>,
    waiting_at: Option<u64>,
}
struct InFlight {
    command: PendingValue,
    acknowledgement: u64,
    worker: JoinedThread<Result<Event, CoreDisconnected>>,
    disposition: Disposition,
}
struct PendingValue {
    command: Command,
    /// The complete desired value equals the widget's accepted document at this revision.
    unchanged_at: Option<TimelineRevision>,
}
enum Disposition {
    Active,
    /// Keep the worker owned until it finishes, but never retry its old capability.
    Abandoned {
        count_if_refused: bool,
    },
}
enum Notice {
    Refused(String),
    Unapplied { count: usize, text: String },
    Full,
}
impl Notice {
    fn text(&self) -> &str {
        match self {
            Self::Refused(text) | Self::Unapplied { text, .. } => text,
            Self::Full => FULL_WORDS,
        }
    }
}
#[derive(Default)]
pub(crate) struct ManualEdits {
    actions: VecDeque<PendingAction>,
    in_flight: Option<InFlight>,
    acknowledgement: u64,
    accepted_revision: Option<TimelineRevision>,
    notice: Option<Notice>,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) before_send: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) after_send: Option<Box<dyn FnOnce() + Send>>,
}
impl ManualEdits {
    pub(crate) fn submit(
        &mut self,
        command: Command,
        permit: &ManualPermit,
    ) -> Result<(), ManualSendError> {
        self.submit_value(command, None, permit)
    }
    pub(crate) fn submit_value(
        &mut self,
        command: Command,
        unchanged_at: Option<TimelineRevision>,
        permit: &ManualPermit,
    ) -> Result<(), ManualSendError> {
        let gesture = match &command {
            Command::DoBatchCoalesced { coalesce_key, .. } => Some(coalesce_key.clone()),
            _ => None,
        };
        let command = PendingValue {
            command,
            unchanged_at,
        };
        if let Some(coalesce_key) = &gesture
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
        self.actions.push_back(PendingAction {
            ended: gesture.is_none(),
            gesture,
            latest: Some(command),
            accepted: None,
            waiting_at: None,
        });
        self.notice = None;
        self.start_next(permit);
        Ok(())
    }
    /// The opaque egui colour picker compares its input with its output. Feed it
    /// this control's last requested value, including an unresolved flight.
    pub(crate) fn desired_solid_color(
        &self,
        clip: kinewright_core::ClipId,
    ) -> Option<kinewright_core::SolidColor> {
        fn color(
            command: &Command,
            clip: kinewright_core::ClipId,
        ) -> Option<kinewright_core::SolidColor> {
            fn operation(
                op: &kinewright_core::Operation,
                clip: kinewright_core::ClipId,
            ) -> Option<kinewright_core::SolidColor> {
                match op {
                    kinewright_core::Operation::SetSolidColor {
                        clip: target,
                        color,
                    } if *target == clip => Some(*color),
                    _ => None,
                }
            }
            match command {
                Command::Do(op) | Command::DoIfRevision { operation: op, .. } => {
                    operation(op, clip)
                }
                Command::DoBatch(ops)
                | Command::DoBatchCoalesced {
                    operations: ops, ..
                }
                | Command::DoBatchIfRevision {
                    operations: ops, ..
                } => ops.iter().rev().find_map(|op| operation(op, clip)),
                _ => None,
            }
        }
        for (index, action) in self.actions.iter().enumerate().rev() {
            if let Some(found) = action
                .latest
                .as_ref()
                .and_then(|value| color(&value.command, clip))
            {
                return Some(found);
            }
            if index == 0
                && let Some(found) = self
                    .in_flight
                    .as_ref()
                    .filter(|flight| matches!(flight.disposition, Disposition::Active))
                    .and_then(|flight| color(&flight.command.command, clip))
            {
                return Some(found);
            }
            if let Some(found) = action
                .accepted
                .as_ref()
                .and_then(|command| color(command, clip))
            {
                return Some(found);
            }
        }
        None
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
        self.attempt_running() || self.actions.iter().any(|a| !a.ended || a.latest.is_some())
    }
    pub(crate) fn message(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .map(Notice::text)
            .or_else(|| self.waiting().then_some(PENDING_WORDS))
    }
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(Notice::text)
    }
    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }
    pub(crate) fn record_refusal(&mut self, reason: CoreMutationRefusal) {
        self.notice = Some(Notice::Refused(reason.to_string()));
    }
    fn waiting(&self) -> bool {
        self.actions.iter().enumerate().any(|(index, action)| {
            action.waiting_at.is_some()
                || (action.ended
                    && (action.latest.is_some() || (index == 0 && self.attempt_running())))
        })
    }
    pub(crate) fn attempt_running(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|f| matches!(f.disposition, Disposition::Active))
    }
    /// These values never entered Core. Accepted history remains owned by its permit and cut.
    pub(crate) fn abandon_unaccepted(&mut self) {
        self.collect_ready();
        let count = self.actions.iter().filter(|a| a.latest.is_some()).count();
        if let Some(flight) = &mut self.in_flight
            && matches!(flight.disposition, Disposition::Active)
        {
            flight.disposition = Disposition::Abandoned {
                count_if_refused: self.actions.front().is_none_or(|a| a.latest.is_none()),
            };
        }
        self.actions.clear();
        self.accepted_revision = None;
        if count > 0 {
            self.note_unapplied(count);
        }
    }
    fn note_unapplied(&mut self, count: usize) {
        let count = count
            + match &self.notice {
                Some(Notice::Unapplied { count, .. }) => *count,
                _ => 0,
            };
        let edit = if count == 1 { "edit was" } else { "edits were" };
        self.notice = Some(Notice::Unapplied {
            count,
            text: format!(
                "{count} hand {edit} not applied. Check the document and repeat {} after recovery.",
                if count == 1 { "it" } else { "them" }
            ),
        });
    }
    fn note_unconfirmed(&mut self, reason: &str) {
        if let Some(Notice::Unapplied { text, .. }) = &mut self.notice {
            text.push(' ');
            text.push_str(reason);
        } else {
            self.notice = Some(Notice::Refused(reason.into()));
        }
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn action_count(&self) -> usize {
        self.actions.len()
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn worker_owned(&self) -> bool {
        self.in_flight.is_some()
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
            if let Disposition::Abandoned { count_if_refused } = flight.disposition {
                match result {
                    Ok(Ok(Event::DocumentChanged { revision, .. })) => {
                        self.accepted_revision = Some(revision);
                    }
                    Ok(Ok(_)) if count_if_refused => self.note_unapplied(1),
                    Ok(Err(CoreDisconnected)) => self.note_unconfirmed(
                        "The Core actor stopped. Check the document; this edit's outcome could not be confirmed."
                    ),
                    Err(_) => self.note_unconfirmed(
                        "The edit worker panicked. Check the document; this edit's outcome could not be confirmed."
                    ),
                    Ok(Ok(_)) => {}
                }
                return;
            }
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
                Ok(Ok(Event::DocumentChanged { revision, .. })) => {
                    self.accepted_revision = Some(revision);
                    action.accepted = Some(flight.command.command);
                    action.waiting_at = None;
                }
                other => {
                    self.notice = Some(Notice::Refused(match other {
                        Ok(Ok(Event::MutationRefused { reason, .. })) => reason.to_string(),
                        Ok(Ok(Event::OpRejected { error, .. })) => error.to_string(),
                        Ok(Ok(Event::BatchRejected { error, .. })) => error.to_string(),
                        Ok(Ok(Event::RevisionConflict { .. })) => "The document changed before this edit could be applied. Check its current value and try again.".into(),
                        Ok(Ok(Event::UndoBlockedBySave { .. })) => "Undo is waiting for the save to finish. Retry after the destination is reconciled.".into(),
                        Ok(Err(CoreDisconnected)) => "The Core actor stopped. Check the document; this edit's outcome could not be confirmed.".into(),
                        Err(_) => "The edit worker panicked. Check the document; this edit's outcome could not be confirmed.".into(),
                        _ => "The Core actor returned an unexpected reply. Check the document before trying this edit again.".into(),
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
        while let Some(action) = self.actions.front_mut() {
            // An older flight must finish before this hint can be trusted. Its
            // accepted revision invalidates a comparison against a stale UI frame.
            if action
                .latest
                .as_ref()
                .and_then(|v| v.unchanged_at)
                .is_some_and(|revision| {
                    self.accepted_revision
                        .is_none_or(|accepted| accepted <= revision)
                })
            {
                action.latest = None;
                action.waiting_at = None;
            }
            if !action.ended || action.latest.is_some() {
                break;
            }
            self.actions.pop_front();
        }
        if self.actions.len() < PENDING_ACTIONS && matches!(self.notice, Some(Notice::Full)) {
            self.notice = None;
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
        let sent = command.command.clone();
        // This capability crosses to the acceptance worker; the UI keeps its original owner.
        let permit = permit.clone();
        #[cfg(all(test, target_os = "linux"))]
        let before_send = self.before_send.take();
        #[cfg(all(test, target_os = "linux"))]
        let after_send = self.after_send.take();
        match std::thread::Builder::new()
            .name("manual-edit".into())
            .spawn(move || {
                #[cfg(all(test, target_os = "linux"))]
                if let Some(hook) = before_send {
                    hook();
                }
                let result = permit.request(sent);
                #[cfg(all(test, target_os = "linux"))]
                if let Some(hook) = after_send {
                    hook();
                }
                result
            }) {
            Ok(worker) => {
                self.in_flight = Some(InFlight {
                    command,
                    acknowledgement: self.acknowledgement,
                    worker: JoinedThread::new(worker),
                    disposition: Disposition::Active,
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

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/unit/manual_edits.rs"]
pub(crate) mod tests;
