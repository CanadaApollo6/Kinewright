//! Audited scope/bootstrap OS primitives. No D-Bus or media parsing lives here.
#![allow(clippy::missing_errors_doc)]
use crate::{Cgroup, ProcessHandle};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::{BorrowedFd, FromRawFd, RawFd},
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::Command,
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn bounded(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?.take(8193).read_to_string(&mut text)?;
    if text.len() > 8192 {
        return Err(invalid("oversized process/control observation"));
    }
    Ok(text)
}
fn stat(pid: u32) -> io::Result<(char, u32, u64)> {
    let text = bounded(&PathBuf::from(format!("/proc/{pid}/stat")))?;
    let (_, fields) = text
        .rsplit_once(')')
        .ok_or_else(|| invalid("process identity"))?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let state = fields
        .first()
        .and_then(|v| v.chars().next())
        .ok_or_else(|| invalid("process state"))?;
    let parent = fields
        .get(1)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("process parent"))?;
    let start = fields
        .get(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("process starttime"))?;
    Ok((state, parent, start))
}

pub fn child(app: u32, pid: u32) -> io::Result<ProcessHandle> {
    let process = ProcessHandle::open(pid)?;
    verify_child(app, &process)?;
    Ok(process)
}
pub fn verify_child(app: u32, process: &ProcessHandle) -> io::Result<()> {
    let (state, parent, start) = stat(process.pid())?;
    if matches!(state, 'Z' | 'X' | 'x') || parent != app || start != process.start() {
        return Err(invalid("scope PID is not the retained live app child"));
    }
    let mut fds = [rustix::event::PollFd::new(
        process,
        rustix::event::PollFlags::IN,
    )];
    if rustix::event::poll(&mut fds, Some(&rustix::event::Timespec::default()))? != 0 {
        return Err(invalid("scope pidfd exited"));
    }
    Ok(())
}
pub fn membership(pid: u32) -> io::Result<PathBuf> {
    let text = bounded(&PathBuf::from(format!("/proc/{pid}/cgroup")))?;
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| invalid("cgroup v2 required"))?;
    checked_group(path)
}
pub fn checked_group(relative: &str) -> io::Result<PathBuf> {
    let path = Path::new(relative);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        || relative.ends_with(" (deleted)")
    {
        return Err(invalid("invalid scope cgroup path"));
    }
    Ok(Path::new("/sys/fs/cgroup").join(path.strip_prefix("/").map_err(io::Error::other)?))
}
pub fn outside(app: u32, group: &Path) -> io::Result<()> {
    for pid in [app, std::process::id()] {
        if membership(pid)?.starts_with(group) {
            return Err(invalid("app/supervisor entered helper scope"));
        }
    }
    Ok(())
}
fn number(group: &Path, name: &str) -> io::Result<u64> {
    bounded(&group.join(name))?
        .trim()
        .parse()
        .map_err(io::Error::other)
}
pub fn verify_controls(group: &Cgroup, memory: u64) -> io::Result<()> {
    for (name, value) in [
        ("memory.max", memory),
        ("memory.swap.max", 0),
        ("pids.max", 8),
        ("memory.oom.group", 1),
    ] {
        if number(group.path(), name)? != value {
            return Err(invalid("scope controller not enforced"));
        }
    }
    let cpu = bounded(&group.path().join("cpu.max"))?;
    let values: Vec<_> = cpu.split_whitespace().collect();
    let quota = values.first().and_then(|v| v.parse::<u64>().ok());
    let period = values.get(1).and_then(|v| v.parse::<u64>().ok());
    if values.len() != 2 || quota.is_none() || quota != period || period == Some(0) {
        return Err(invalid("probe CPU quota not enforced"));
    }
    Ok(())
}

/// Inherit only the supervisor's private socket endpoint at fd 3.
/// # Safety
/// `fd` stays valid and owned by the caller until spawn completes.
pub unsafe fn inherit_socket(command: &mut Command, fd: RawFd) {
    // SAFETY: only async-signal-safe descriptor operations execute after fork.
    // The copied fd is independent of target 3, which may alias the original.
    unsafe {
        command.pre_exec(move || {
            let copy = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 32);
            if copy < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::dup2(copy, 3) < 0 {
                let error = io::Error::last_os_error();
                libc::close(copy);
                return Err(error);
            }
            libc::close(copy);
            Ok(())
        });
    }
}
/// Take the endpoint in the separately exec'd supervisor, never in a helper.
/// # Safety
/// The launcher installed a uniquely owned socket at fd 3 with no Rust owner.
pub unsafe fn inherited_socket(app: u32) -> io::Result<UnixStream> {
    // SAFETY: scalar query on the fixed inherited descriptor.
    let flags = unsafe { libc::fcntl(3, libc::F_GETFL) };
    if flags < 0 || flags & libc::O_ACCMODE != libc::O_RDWR {
        return Err(invalid("supervisor descriptor mode"));
    }
    // SAFETY: fd 3 is live after F_GETFL; the borrow ends before its unique
    // ownership is returned and no operation here closes or replaces it.
    let descriptor = unsafe { BorrowedFd::borrow_raw(3) };
    if rustix::net::sockopt::socket_type(descriptor)? != rustix::net::SocketType::STREAM
        || rustix::net::sockopt::socket_domain(descriptor)? != rustix::net::AddressFamily::UNIX
    {
        return Err(invalid(
            "supervisor endpoint is not an anonymous Unix stream",
        ));
    }
    let peer = rustix::net::sockopt::socket_peercred(descriptor)?;
    if peer.pid.as_raw_pid().cast_unsigned() != app {
        return Err(invalid("supervisor socket peer"));
    }
    let (_, parent, _) = stat(std::process::id())?;
    if parent != app {
        return Err(invalid("supervisor parent"));
    }
    rustix::io::fcntl_setfd(descriptor, rustix::io::FdFlags::CLOEXEC)?;
    // SAFETY: every socket/parent check passed; this is the sole Rust owner.
    let socket = unsafe { UnixStream::from_raw_fd(3) };
    if !socket.local_addr()?.is_unnamed() || !socket.peer_addr()?.is_unnamed() {
        return Err(invalid(
            "supervisor endpoint is not an anonymous socketpair",
        ));
    }
    Ok(socket)
}

/// Resolve the session bus before the supervisor's curated exec environment.
#[must_use]
pub fn session_bus_address() -> String {
    std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_else(|_| {
        format!(
            "unix:path=/run/user/{}/bus",
            rustix::process::geteuid().as_raw()
        )
    })
}
pub fn socketpair() -> io::Result<(UnixStream, UnixStream)> {
    let pair = UnixStream::pair()?;
    for endpoint in [&pair.0, &pair.1] {
        if !rustix::io::fcntl_getfd(endpoint)?.contains(rustix::io::FdFlags::CLOEXEC) {
            return Err(invalid("supervisor socket is inheritable"));
        }
    }
    Ok(pair)
}
