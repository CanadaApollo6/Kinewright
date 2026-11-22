//! JA-1 S6 C4: the `org.freedesktop.Notifications` client.
//!
//! Only the bus calls live here: `GetCapabilities`, `Notify`, and the wait
//! for `ActionInvoked` and `NotificationClosed`. The queue, the seal hook and
//! host status stay in `kinewright-agent`. This crate is Linux only and never
//! names the systemd manager; `scripts/ms2_client_boundary.py` checks both.
#![cfg(target_os = "linux")]

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use zbus::{
    MatchRule,
    blocking::{Connection, MessageIterator, Proxy, connection::Builder},
    message::Type,
    names::OwnedUniqueName,
    zvariant::Value,
};

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
/// The freedesktop key of a notification's default action (a click).
pub const DEFAULT_ACTION: &str = "default";
const MAX_SIGNALS_QUEUED: usize = 16;
const METHOD_TIMEOUT: Duration = Duration::from_secs(2);

/// A failed bus call or connection.
#[derive(Debug)]
pub struct Error(zbus::Error);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {}

impl From<zbus::Error> for Error {
    fn from(error: zbus::Error) -> Self {
        Self(error)
    }
}

/// How a notifier reaches a bus. Production opens the person's session bus;
/// tests inject a private one.
pub type Connect = Arc<dyn Fn() -> Result<Bus, Error> + Send + Sync>;

/// The person's session bus.
#[must_use]
pub fn session_bus() -> Connect {
    Arc::new(|| {
        Ok(Bus(Builder::session()?
            .method_timeout(METHOD_TIMEOUT)
            .build()?))
    })
}

/// One connection to a bus. Clones share it.
#[derive(Clone)]
pub struct Bus(Connection);

/// The unique bus name of the daemon that answered a `Notify`. Every later
/// signal about that notification must come from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonName(OwnedUniqueName);

/// A notification the daemon accepted.
#[derive(Debug)]
pub struct Sent {
    pub id: u32,
    pub daemon: DaemonName,
}

/// A notification signal.
#[derive(Debug)]
pub enum Signal {
    /// An action on notification `id`; [`DEFAULT_ACTION`] is a click.
    Action { id: u32, action: String },
    /// Notification `id` closed.
    Closed { id: u32 },
    /// Any other member, or a body of the wrong shape.
    Other,
}

/// A signal and who sent it.
#[derive(Debug)]
pub struct Received {
    pub signal: Signal,
    pub sender: Option<DaemonName>,
}

impl Bus {
    /// The daemon offers actions. A failed call counts as no.
    #[must_use]
    pub fn actions(&self) -> bool {
        Proxy::new(&self.0, DESTINATION, PATH, INTERFACE)
            .and_then(|proxy| proxy.call::<_, _, Vec<String>>("GetCapabilities", &()))
            .unwrap_or_default()
            .iter()
            .any(|capability| capability == "actions")
    }

    /// Send `Notify` with `title` as summary and application name. With
    /// `action`, a click sends [`DEFAULT_ACTION`] and the daemon may show the
    /// label.
    ///
    /// # Errors
    ///
    /// The call failed, or its reply had the wrong shape or no sender.
    pub fn notify(&self, title: &str, body: &str, action: Option<&str>) -> Result<Sent, Error> {
        let actions: Vec<&str> = action.map_or_else(Vec::new, |label| vec![DEFAULT_ACTION, label]);
        let hints: HashMap<&str, Value<'_>> = HashMap::new();
        let reply = Proxy::new(&self.0, DESTINATION, PATH, INTERFACE)?.call_method(
            "Notify",
            &(title, 0_u32, "", title, body, actions, hints, -1_i32),
        )?;
        let id: u32 = reply.body().deserialize()?;
        let daemon = reply
            .header()
            .sender()
            .map(|name| DaemonName(OwnedUniqueName::from(name.to_owned())))
            .ok_or_else(|| {
                zbus::Error::Failure("The notification reply named no sender.".into())
            })?;
        Ok(Sent { id, daemon })
    }

    /// The notification signals from now on. The match rule is in place when
    /// this returns, so a `Notify` sent after it cannot miss its click. The
    /// stream ends when the connection closes.
    ///
    /// # Errors
    ///
    /// The match rule could not be added.
    pub fn signals(&self) -> Result<Signals, Error> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(INTERFACE)?
            .path(PATH)?
            .build();
        Ok(Signals(MessageIterator::for_match_rule(
            rule,
            &self.0,
            Some(MAX_SIGNALS_QUEUED),
        )?))
    }

    /// Close the connection, which ends every [`Signals`] on it.
    pub fn close(self) {
        let _ = self.0.close();
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// See [`Bus::signals`].
pub struct Signals(MessageIterator);

impl Iterator for Signals {
    type Item = Result<Received, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let message = match self.0.next()? {
            Ok(message) => message,
            Err(error) => return Some(Err(Error(error))),
        };
        let header = message.header();
        let sender = header
            .sender()
            .map(|name| DaemonName(OwnedUniqueName::from(name.to_owned())));
        let signal = match header.member().map(zbus::names::MemberName::as_str) {
            Some("ActionInvoked") => message
                .body()
                .deserialize::<(u32, String)>()
                .map_or(Signal::Other, |(id, action)| Signal::Action { id, action }),
            Some("NotificationClosed") => message
                .body()
                .deserialize::<(u32, u32)>()
                .map_or(Signal::Other, |(id, _reason)| Signal::Closed { id }),
            _ => Signal::Other,
        };
        Some(Ok(Received { signal, sender }))
    }
}

#[cfg(feature = "test-bus")]
pub mod test_bus;
