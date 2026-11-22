//! JA-1 S6 C4 test support: a private session bus with a fake notification
//! daemon on it. Nothing here reaches the person's session bus: the daemon
//! runs with an empty environment and its own configuration, which listens on
//! a fresh abstract socket and activates no services.
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use zbus::{
    blocking::{Connection, connection::Builder},
    names::BusName,
    zvariant::OwnedValue,
};

use super::{Connect, DEFAULT_ACTION, DESTINATION, INTERFACE, PATH};

/// One `Notify` call the fake daemon answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::writer_host) struct NotifyCall {
    pub id: u32,
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: Vec<String>,
    pub expire_timeout: i32,
}

/// How the fake daemon behaves.
#[derive(Default)]
pub(in crate::writer_host) struct Daemon {
    pub capabilities: Vec<String>,
    pub calls: Mutex<Vec<NotifyCall>>,
    /// `Notify` answers with an error.
    pub refuse: AtomicBool,
    /// `Notify` waits here, once entered, until released.
    pub hold: AtomicBool,
    pub entered: AtomicBool,
    next_id: AtomicU64,
}

struct Fake(Arc<Daemon>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Fake {
    fn get_capabilities(&self) -> Vec<String> {
        self.0.capabilities.clone()
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the freedesktop Notify signature"
    )]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        self.0.entered.store(true, Ordering::SeqCst);
        wait_for(|| !self.0.hold.load(Ordering::SeqCst), "S6C4_DAEMON_HELD");
        if self.0.refuse.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::Failed("refused by the test".into()));
        }
        let id = u32::try_from(self.0.next_id.fetch_add(1, Ordering::SeqCst) + 1).unwrap();
        self.0.calls.lock().unwrap().push(NotifyCall {
            id,
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints: hints.into_keys().collect(),
            expire_timeout,
        });
        Ok(id)
    }
}

/// Poll `ready` for up to 30 s; no channel or timed receive (PITFALLS).
pub(in crate::writer_host) fn wait_for(ready: impl Fn() -> bool, marker: &str) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(30), "{marker}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A private `dbus-daemon`, killed and reaped on drop.
pub(in crate::writer_host) struct PrivateBus {
    child: Child,
    address: String,
    config: std::path::PathBuf,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.config);
    }
}

impl PrivateBus {
    pub(in crate::writer_host) fn start() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "kinewright-s6c4-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        // TMPDIR is inside the worktree's target for every test run.
        let config = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&config).unwrap();
        let file = config.join("bus.conf");
        std::fs::write(
            &file,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:abstract={name}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#
            ),
        )
        .unwrap();
        let mut child = Command::new("/usr/bin/dbus-daemon")
            .arg("--nofork")
            .arg("--print-address=1")
            .arg(format!("--config-file={}", file.display()))
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("S6C4_DBUS_DAEMON");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap().take(1024))
            .read_line(&mut address)
            .unwrap();
        assert!(address.ends_with('\n'), "S6C4_BUS_ADDRESS: {address:?}");
        Self {
            child,
            address: address.trim_end().to_owned(),
            config,
        }
    }

    /// The notifier's way onto this bus.
    pub(in crate::writer_host) fn connect(&self) -> Connect {
        let address = self.address.clone();
        Arc::new(move || {
            Builder::address(address.as_str())?
                .method_timeout(Duration::from_secs(5))
                .build()
        })
    }

    /// A way onto this bus that always fails, as a missing bus does.
    pub(in crate::writer_host) fn refused() -> Connect {
        Arc::new(|| Err(zbus::Error::Address("no session bus in this test".into())))
    }

    /// A fake daemon that owns the notification name on this bus.
    pub(in crate::writer_host) fn daemon(&self, actions: bool) -> (Arc<Daemon>, Connection) {
        let daemon = Arc::new(Daemon {
            capabilities: if actions {
                vec!["actions".into(), "body".into()]
            } else {
                vec!["body".into()]
            },
            ..Daemon::default()
        });
        let connection = Builder::address(self.address.as_str())
            .unwrap()
            .name(DESTINATION)
            .unwrap()
            .serve_at(PATH, Fake(Arc::clone(&daemon)))
            .unwrap()
            .build()
            .unwrap();
        (daemon, connection)
    }

    /// Another client on this bus, which does not own the daemon's name.
    pub(in crate::writer_host) fn stranger(&self) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .unwrap()
    }
}

/// Emit a notification signal from `from`.
pub(in crate::writer_host) fn emit<B>(from: &Connection, member: &str, body: &B)
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    from.emit_signal(None::<BusName<'_>>, PATH, INTERFACE, member, body)
        .unwrap();
}

/// A click on notification `id`.
pub(in crate::writer_host) fn click(from: &Connection, id: u32) {
    emit(from, "ActionInvoked", &(id, DEFAULT_ACTION));
}
