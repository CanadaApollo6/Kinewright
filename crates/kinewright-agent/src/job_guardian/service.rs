#[cfg(target_os = "linux")]
use super::linux::Tree;
use super::protocol::{
    self, AttemptIdentity, GuardianMessage, HostMessage, Program, RecoveryIdentity,
};
#[cfg(windows)]
use super::windows::Tree;
#[cfg(unix)]
use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

pub(super) fn atomic_record(
    path: &Path,
    value: &impl serde::Serialize,
    cap: usize,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > cap {
        return Err(io::Error::other("private record cap"));
    }
    let staging = path.with_extension("partial");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(staging, path)?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("record parent"))?,
    )?
    .sync_all()?;
    Ok(())
}

fn micros(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}
fn send(message: &GuardianMessage) -> io::Result<()> {
    protocol::write_frame(&mut io::stdout().lock(), message)
}
fn commands() -> Receiver<io::Result<HostMessage>> {
    let (sender, receiver) = mpsc::sync_channel(2);
    thread::spawn(move || {
        #[cfg(windows)]
        let mut stdin = kinewright_job_sys::control_input();
        #[cfg(not(windows))]
        let mut stdin = io::stdin().lock();
        loop {
            let message = protocol::read_frame(&mut stdin);
            let terminal = message.is_err();
            if sender.send(message).is_err() || terminal {
                break;
            }
        }
    });
    receiver
}
fn receive(
    receiver: &Receiver<io::Result<HostMessage>>,
    deadline: Instant,
) -> io::Result<HostMessage> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::other("guardian startup deadline"));
        }
        match receiver.try_recv() {
            Ok(message) => return message,
            Err(TryRecvError::Disconnected) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Err(TryRecvError::Empty) => {}
        }
        thread::sleep(Duration::from_millis(1));
    }
}
fn expected(
    receiver: &Receiver<io::Result<HostMessage>>,
    identity: &AttemptIdentity,
    commit: bool,
) -> io::Result<()> {
    let message = receive(receiver, Instant::now() + protocol::STARTUP)?;
    let matches = match message {
        HostMessage::Contain { nonce } if !commit => nonce == identity.nonce,
        HostMessage::Commit { nonce } if commit => nonce == identity.nonce,
        _ => false,
    };
    if !matches {
        return Err(io::Error::other("guardian handshake order/identity"));
    }
    Ok(())
}

/// Run the shared native executable. No provider or project writer is loaded.
///
/// # Errors
/// Refuses unsupported supervision or any failed handshake/cleanup prerequisite.
#[cfg(any(target_os = "linux", windows))]
pub fn main_entry() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .ok_or_else(|| io::Error::other("guardian mode required"))?;
    #[cfg(target_os = "linux")]
    if mode == "--shim" {
        return super::linux::shim(args);
    }
    if mode != "--serve" {
        return Err(io::Error::other("unknown guardian mode"));
    }
    let root = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("private root required"))?,
    );
    let unit = args
        .next()
        .and_then(|v| v.into_string().ok())
        .ok_or_else(|| io::Error::other("unit required"))?;
    if args.next().is_some() || !root.is_absolute() {
        return Err(io::Error::other("invalid guardian arguments"));
    }
    let receiver = commands();
    let started = Instant::now();
    let HostMessage::Prepare { identity, program } =
        receive(&receiver, started + protocol::STARTUP)?
    else {
        return Err(io::Error::other("PREPARE required"));
    };
    identity.validate()?;
    program.validate()?;
    if unit != identity.unit() {
        return Err(io::Error::other("unit identity mismatch"));
    }
    let mut tree = Tree::new(&unit)
        .map_err(|error| io::Error::other(format!("external_supervision_unavailable: {error}")))?;
    let process = kinewright_job_sys::ProcessHandle::open(std::process::id())?;
    let (containment, device, inode) = tree.identity();
    let recovery = RecoveryIdentity {
        version: protocol::VERSION,
        attempt: identity.clone(),
        guardian_pid: process.pid(),
        guardian_start: process.start(),
        containment,
        device,
        inode,
    };
    recovery.validate()?;
    atomic_record(
        &root.join("native-recovery.json"),
        &recovery,
        protocol::RECOVERY_CAP,
    )?;
    let execution = execute(
        &mut tree, &receiver, &root, &identity, &program, &recovery, started,
    );
    cleanup(&mut tree, &root, &identity, started)?;
    execution
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn main_entry() -> io::Result<()> {
    Err(io::Error::other("external_supervision_unavailable"))
}

#[cfg(any(target_os = "linux", windows))]
fn execute(
    tree: &mut Tree,
    receiver: &Receiver<io::Result<HostMessage>>,
    root: &Path,
    identity: &AttemptIdentity,
    program: &Program,
    recovery: &RecoveryIdentity,
    started: Instant,
) -> io::Result<()> {
    send(&GuardianMessage::Owned {
        recovery: recovery.clone(),
        at_us: micros(started),
    })?;
    expected(receiver, identity, false)?;
    let payload = tree.contain(root, program)?;
    let deadline = Instant::now() + protocol::STARTUP;
    while !tree.membership(payload.pid())? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("containment deadline"));
        }
        match receiver.try_recv() {
            Err(TryRecvError::Empty) => {}
            _ => return Err(io::Error::other("lifetime lost during containment")),
        }
        thread::sleep(Duration::from_millis(1));
    }
    send(&GuardianMessage::Contained {
        nonce: identity.nonce.clone(),
        pid: payload.pid(),
        start: payload.start(),
        at_us: micros(started),
    })?;
    expected(receiver, identity, true)?;
    // A queued cancel, malformed frame, or EOF vetoes release even after COMMIT.
    if !matches!(receiver.try_recv(), Err(TryRecvError::Empty)) {
        return Err(io::Error::other("lifetime/cancel before native release"));
    }
    if !kinewright_job_sys::lifetime_alive()? {
        return Err(io::Error::other("lifetime EOF before native release"));
    }
    tree.resume()?;
    send(&GuardianMessage::Executing {
        nonce: identity.nonce.clone(),
        at_us: micros(started),
    })?;
    loop {
        match receiver.try_recv() {
            Ok(Ok(HostMessage::Cancel { nonce })) if nonce == identity.nonce => {
                let grace = Instant::now() + protocol::GRACE;
                while Instant::now() < grace {
                    // Parent death during grace skips its remainder.
                    if !matches!(receiver.try_recv(), Err(TryRecvError::Empty)) {
                        break;
                    }
                    if tree.empty()? && tree.reaped()? {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                return Ok(());
            }
            Err(TryRecvError::Empty) => {}
            _ => return Err(io::Error::other("guardian lifetime/protocol lost")),
        }
        if tree.exited()? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(any(target_os = "linux", windows))]
fn cleanup(
    tree: &mut Tree,
    root: &Path,
    identity: &AttemptIdentity,
    started: Instant,
) -> io::Result<()> {
    let kill_decision = Instant::now();
    let killed_at_us = micros(started);
    let deadline = kill_decision + protocol::CLEANUP;
    let kill = tree.kill();
    let mut empty_at_us = 0;
    let mut reaped_at_us = 0;
    loop {
        if Instant::now() < deadline && tree.empty().unwrap_or(false) && empty_at_us == 0 {
            empty_at_us = micros(started);
        }
        if Instant::now() < deadline && tree.reaped().unwrap_or(false) && reaped_at_us == 0 {
            reaped_at_us = micros(started);
        }
        if kill.is_ok() && empty_at_us != 0 && reaped_at_us != 0 && Instant::now() < deadline {
            let message = GuardianMessage::Cleaned {
                nonce: identity.nonce.clone(),
                killed_at_us,
                empty_at_us,
                reaped_at_us,
            };
            atomic_record(
                &root.join("native-completion.json"),
                &message,
                protocol::RECOVERY_CAP,
            )?;
            let _ = send(&message);
            return Ok(());
        }
        if Instant::now() >= deadline {
            let message = GuardianMessage::CleanupBlocked {
                nonce: identity.nonce.clone(),
                reason: "cleanup_blocked".into(),
            };
            atomic_record(
                &root.join("native-completion.json"),
                &message,
                protocol::RECOVERY_CAP,
            )?;
            let _ = send(&message);
            // Remain the cleanup owner. The host keeps reservations and denies replacements.
            loop {
                thread::sleep(Duration::from_millis(100));
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::{HostMessage, receive};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn queued_commit_cannot_extend_an_expired_startup() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(HostMessage::Commit {
                nonce: "late".into(),
            }))
            .unwrap();
        assert!(
            receive(
                &receiver,
                Instant::now().checked_sub(Duration::from_secs(1)).unwrap()
            )
            .is_err()
        );
    }
}
