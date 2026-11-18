//! Which resource mode a native test may expect. Primary needs a systemd user
//! manager on the session bus the runtime would use
//! (`DBUS_SESSION_BUS_ADDRESS`, else the user's runtime directory); a pass that
//! requested primary never accepts degraded while one answers.
use std::process::{Command, Stdio};

/// Whether a systemd user manager answers on that session bus.
pub fn user_manager() -> bool {
    Command::new("busctl")
        .args([
            "--user",
            "get-property",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "Version",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
