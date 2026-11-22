//! JA-1 S6 C4: a desktop notification on every seal.
//!
//! The host hands each fresh seal to [`Notifier::notify`] under its lock;
//! nothing here takes that lock or blocks it. One short-lived sender thread
//! at a time drains a small queue and calls `org.freedesktop.Notifications`
//! on the session bus. With a GUI attached, the notification carries a
//! default action, and a listener thread waits for this host's own
//! notification IDs: a click puts a one-shot selection in host status, which
//! the GUI acknowledges by nonce. The listener's wait is the bus's message
//! stream; shutdown ends it by closing the connection.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use kinewright_core::output_review::SealedReviewOutcome;
use kinewright_project::{review_job::delivery::seal_notice_text, studio_records::SnapshotKey};
use zbus::{
    MatchRule,
    blocking::{Connection, MessageIterator, Proxy, connection::Builder},
    message::Type,
    names::OwnedUniqueName,
    zvariant::Value,
};

use super::protocol::NotificationSelect;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
/// The notification's title and application name.
const TITLE: &str = "Kinewright";
/// The freedesktop key of a notification's default action (a click).
const DEFAULT_ACTION: &str = "default";
/// Seals waiting while one send is in flight; the oldest is dropped beyond it.
pub(super) const MAX_QUEUED: usize = 4;
/// This host's notifications that a click may still select.
const MAX_OUTSTANDING: usize = 16;
const MAX_SIGNALS_QUEUED: usize = 16;
const METHOD_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ERROR: usize = 240;

/// How the notifier reaches a bus. Production opens the person's session
/// bus; tests inject a private one.
pub(super) type Connect = Arc<dyn Fn() -> zbus::Result<Connection> + Send + Sync>;

/// The person's session bus.
pub(super) fn session_bus() -> Connect {
    Arc::new(|| Builder::session()?.method_timeout(METHOD_TIMEOUT).build())
}

/// What one seal says, and the version a click selects when a GUI is
/// attached. The text carries the version's label and nothing else of the
/// person's.
pub(super) struct SealNotice {
    body: String,
    select: Option<Target>,
    /// The attached GUIs at seal time. The action is offered only if one is
    /// still alive, and the host not detached, when the notice is sent.
    gui: Vec<Arc<AtomicBool>>,
}

struct Target {
    version: SnapshotKey,
    key_sha256: String,
}

impl SealNotice {
    /// `select` is the sealed receipt's version and key digest; `gui` holds
    /// the liveness of each GUI attached at seal time.
    pub(super) fn new(
        label: &str,
        outcome: &SealedReviewOutcome,
        select: Option<(SnapshotKey, String)>,
        gui: Vec<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            body: seal_notice_text(label, outcome),
            select: select.map(|(version, key_sha256)| Target {
                version,
                key_sha256,
            }),
            gui,
        }
    }
}

#[derive(Clone)]
struct Bus {
    connection: Connection,
    actions: bool,
}

struct Listener {
    thread: JoinHandle<()>,
    // Closing it ends the listener's wait.
    connection: Connection,
}

/// A notification this host sent that a click may select.
struct Outstanding {
    daemon: OwnedUniqueName,
    target: Target,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent sender, listener and host lifecycle flags"
)]
struct Shared {
    queue: VecDeque<SealNotice>,
    dropped: u64,
    error: Option<String>,
    // A sender thread is draining the queue.
    sending: bool,
    // A listener thread is waiting for signals.
    listening: bool,
    cancelled: bool,
    // The sender has taken a notice off the queue and not finished it.
    in_flight: bool,
    // JA-1 S6 C4 fix 1: the host detached; nothing sent from now on can be
    // clicked.
    detached: bool,
    bus: Option<Bus>,
    listener: Option<Listener>,
    outstanding: BTreeMap<u32, Outstanding>,
    select: Option<NotificationSelect>,
    next_nonce: u64,
}

/// What host status reports about notifications.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct NotifyStatus {
    pub select: Option<NotificationSelect>,
    pub error: Option<String>,
    pub dropped: u64,
}

/// The host's notifier. With no bus it does nothing at all.
pub(super) struct Notifier {
    connect: Option<Connect>,
    shared: Arc<Mutex<Shared>>,
    sender: Option<JoinHandle<()>>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new(None)
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn bounded(error: &zbus::Error) -> String {
    error.to_string().chars().take(MAX_ERROR).collect()
}

impl Notifier {
    pub(super) fn new(connect: Option<Connect>) -> Self {
        Self {
            connect,
            shared: Arc::new(Mutex::new(Shared {
                next_nonce: 1,
                ..Shared::default()
            })),
            sender: None,
        }
    }

    /// Queue one seal's notification and make sure a sender runs. Never
    /// waits on the bus.
    pub(super) fn notify(&mut self, notice: SealNotice) {
        let Some(connect) = &self.connect else {
            return;
        };
        let mut shared = lock(&self.shared);
        if shared.cancelled {
            return;
        }
        if shared.queue.len() >= MAX_QUEUED {
            shared.queue.pop_front();
            shared.dropped = shared.dropped.saturating_add(1);
        }
        shared.queue.push_back(notice);
        if shared.sending {
            return;
        }
        shared.sending = true;
        drop(shared);
        // A finished sender only returns; joining it does not wait on the bus.
        if let Some(previous) = self.sender.take() {
            let _ = previous.join();
        }
        let shared = Arc::clone(&self.shared);
        let connect = Arc::clone(connect);
        match thread::Builder::new()
            .name("kinewright-notify".into())
            .spawn(move || send_queued(&shared, &connect))
        {
            Ok(thread) => self.sender = Some(thread),
            Err(error) => {
                let mut shared = lock(&self.shared);
                shared.sending = false;
                shared.queue.clear();
                shared.error = Some(format!("The notification thread did not start: {error}"));
            }
        }
    }

    /// A seal whose notification could not be prepared.
    pub(super) fn record_error(&self, error: &str) {
        if self.connect.is_some() {
            lock(&self.shared).error = Some(error.chars().take(MAX_ERROR).collect());
        }
    }

    pub(super) fn status(&self) -> NotifyStatus {
        let shared = lock(&self.shared);
        NotifyStatus {
            select: shared.select.clone(),
            error: shared.error.clone(),
            dropped: shared.dropped,
        }
    }

    /// The GUI handled the selection with this nonce: it is never reported
    /// again. Another nonce leaves a newer selection in place.
    pub(super) fn acknowledge(&self, nonce: u64) {
        let mut shared = lock(&self.shared);
        if shared
            .select
            .as_ref()
            .is_some_and(|select| select.nonce == nonce)
        {
            shared.select = None;
        }
    }

    /// The host is stopping: take no new seals, finish only the one in
    /// flight (or, before the sender takes it, the oldest queued), and drop
    /// and count the rest, so a hung bus holds the exit for one send. Called
    /// under the host lock; waits for nothing.
    pub(super) fn cancel(&self) {
        let mut shared = lock(&self.shared);
        shared.cancelled = true;
        let keep = usize::from(!shared.in_flight);
        let excess = shared.queue.len().saturating_sub(keep);
        shared.queue.truncate(keep);
        shared.dropped = shared
            .dropped
            .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    }

    /// JA-1 S6 C4 fix 1: the host detached from its GUI, or a GUI attached
    /// again. Read when each notice is sent.
    pub(super) fn set_detached(&self, detached: bool) {
        lock(&self.shared).detached = detached;
    }

    /// A handle that outlives the notifier, for [`NotifierProbe::joined`].
    #[cfg(test)]
    pub(super) fn probe(&self) -> NotifierProbe {
        NotifierProbe(Arc::clone(&self.shared))
    }

    /// The half of a stop that waits, for running with the host lock
    /// released: the one send [`Notifier::cancel`] kept finishes, then the
    /// connection closes, which ends the listener's wait, and both threads
    /// are joined.
    pub(super) fn take_stop(&mut self) -> NotifierStop {
        self.cancel();
        NotifierStop {
            shared: Arc::clone(&self.shared),
            sender: self.sender.take(),
        }
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        self.take_stop().finish();
    }
}

#[cfg(test)]
pub(super) struct NotifierProbe(Arc<Mutex<Shared>>);

#[cfg(test)]
impl NotifierProbe {
    /// The notifier was stopped and dropped: its sender and listener threads
    /// hold no share of it, and it sends nothing more.
    pub(super) fn joined(&self) -> bool {
        let shared = lock(&self.0);
        Arc::strong_count(&self.0) == 1
            && shared.cancelled
            && !shared.sending
            && !shared.listening
            && shared.listener.is_none()
            && shared.bus.is_none()
    }
}

/// See [`Notifier::take_stop`].
pub(super) struct NotifierStop {
    shared: Arc<Mutex<Shared>>,
    sender: Option<JoinHandle<()>>,
}

impl NotifierStop {
    pub(super) fn finish(self) {
        if let Some(sender) = self.sender {
            let _ = sender.join();
        }
        let (bus, listener) = {
            let mut shared = lock(&self.shared);
            (shared.bus.take(), shared.listener.take())
        };
        if let Some(bus) = bus {
            let _ = bus.connection.close();
        }
        if let Some(listener) = listener {
            let _ = listener.connection.close();
            let _ = listener.thread.join();
        }
    }
}

fn open(connect: &Connect) -> zbus::Result<Bus> {
    let connection = connect()?;
    let capabilities: Vec<String> = Proxy::new(&connection, DESTINATION, PATH, INTERFACE)?
        .call("GetCapabilities", &())
        .unwrap_or_default();
    Ok(Bus {
        connection,
        actions: capabilities
            .iter()
            .any(|capability| capability == "actions"),
    })
}

fn signals(connection: &Connection) -> zbus::Result<MessageIterator> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(INTERFACE)?
        .path(PATH)?
        .build();
    MessageIterator::for_match_rule(rule, connection, Some(MAX_SIGNALS_QUEUED))
}

/// Send `Notify`, returning the notification's ID and the daemon's unique
/// name, which every later signal about it must come from.
fn send(bus: &Bus, body: &str, actionable: bool) -> zbus::Result<(u32, OwnedUniqueName)> {
    let actions: Vec<&str> = if actionable {
        vec![DEFAULT_ACTION, "Show"]
    } else {
        Vec::new()
    };
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let reply = Proxy::new(&bus.connection, DESTINATION, PATH, INTERFACE)?.call_method(
        "Notify",
        &(TITLE, 0_u32, "", TITLE, body, actions, hints, -1_i32),
    )?;
    let id: u32 = reply.body().deserialize()?;
    let daemon = reply
        .header()
        .sender()
        .map(|name| OwnedUniqueName::from(name.to_owned()))
        .ok_or_else(|| zbus::Error::Failure("The notification reply named no sender.".into()))?;
    Ok((id, daemon))
}

/// The sender thread: send every queued seal, in order, then exit.
fn send_queued(shared: &Arc<Mutex<Shared>>, connect: &Connect) {
    loop {
        let (notice, bus) = {
            let mut state = lock(shared);
            let Some(notice) = state.queue.pop_front() else {
                state.sending = false;
                return;
            };
            state.in_flight = true;
            (notice, state.bus.clone())
        };
        let bus = match bus {
            Some(bus) => bus,
            None => match open(connect) {
                Ok(bus) => {
                    lock(shared).bus = Some(bus.clone());
                    bus
                }
                Err(error) => {
                    let mut state = lock(shared);
                    state.error = Some(bounded(&error));
                    state.in_flight = false;
                    continue;
                }
            },
        };
        // The GUI is read now, not when the seal was queued: a notice sent
        // after a detach, or after its GUI is gone, offers no action.
        let attached = {
            let state = lock(shared);
            !state.detached && notice.gui.iter().any(|alive| alive.load(Ordering::SeqCst))
        };
        let actionable =
            notice.select.is_some() && attached && bus.actions && ensure_listener(shared, &bus);
        match send(&bus, &notice.body, actionable) {
            Ok((id, daemon)) => {
                let mut state = lock(shared);
                state.error = None;
                state.in_flight = false;
                if actionable && let Some(target) = notice.select {
                    if state.outstanding.len() >= MAX_OUTSTANDING {
                        state.outstanding.pop_first();
                    }
                    state.outstanding.insert(id, Outstanding { daemon, target });
                }
            }
            Err(error) => {
                let mut state = lock(shared);
                state.error = Some(bounded(&error));
                state.in_flight = false;
                // A failed bus is opened again for the next seal.
                if bus.connection.is_closed() {
                    state.bus = None;
                }
            }
        }
    }
}

/// Make sure a listener waits for signals before a notification that can be
/// clicked is sent. While this sender runs, a live listener does not exit.
fn ensure_listener(shared: &Arc<Mutex<Shared>>, bus: &Bus) -> bool {
    {
        let state = lock(shared);
        if state.cancelled {
            return false;
        }
        if state.listening {
            return true;
        }
    }
    // The match rule is in place before `Notify` is sent.
    let iterator = match signals(&bus.connection) {
        Ok(iterator) => iterator,
        Err(error) => {
            lock(shared).error = Some(bounded(&error));
            return false;
        }
    };
    let mut state = lock(shared);
    if let Some(previous) = state.listener.take() {
        // It set `listening` false as its last step.
        let _ = previous.thread.join();
    }
    let listening = Arc::clone(shared);
    match thread::Builder::new()
        .name("kinewright-notify-listen".into())
        .spawn(move || listen(iterator, &listening))
    {
        Ok(thread) => {
            state.listening = true;
            state.listener = Some(Listener {
                thread,
                connection: bus.connection.clone(),
            });
            true
        }
        Err(error) => {
            state.error = Some(format!("The notification listener did not start: {error}"));
            false
        }
    }
}

/// The listener thread: turn a click on one of this host's notifications
/// into one selection. It ends when the stream ends (the connection
/// closed), when the host stops, or when no notification of this host can
/// still be clicked and no send is in flight.
fn listen(iterator: MessageIterator, shared: &Arc<Mutex<Shared>>) {
    for message in iterator {
        let Ok(message) = message else {
            break;
        };
        let header = message.header();
        let member = header.member().map(|member| member.as_str().to_owned());
        let sender = header.sender().map(zbus::names::UniqueName::to_owned);
        let mut state = lock(shared);
        let ours = |state: &Shared, id: u32| {
            state.outstanding.get(&id).is_some_and(|outstanding| {
                sender
                    .as_ref()
                    .is_some_and(|sender| *sender == *outstanding.daemon)
            })
        };
        match member.as_deref() {
            Some("ActionInvoked") => {
                if let Ok((id, action)) = message.body().deserialize::<(u32, String)>()
                    && action == DEFAULT_ACTION
                    && ours(&state, id)
                    && let Some(clicked) = state.outstanding.remove(&id)
                {
                    let nonce = state.next_nonce;
                    state.next_nonce = nonce.saturating_add(1);
                    state.select = Some(NotificationSelect {
                        nonce,
                        version: clicked.target.version,
                        key_sha256: clicked.target.key_sha256,
                    });
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>()
                    && ours(&state, id)
                {
                    state.outstanding.remove(&id);
                }
            }
            _ => {}
        }
        if state.cancelled || (!state.sending && state.outstanding.is_empty()) {
            state.listening = false;
            return;
        }
    }
    lock(shared).listening = false;
}

#[cfg(test)]
pub(super) mod test_bus;
#[cfg(test)]
mod tests;
