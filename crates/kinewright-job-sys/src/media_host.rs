//! Audited host-side launch, observation and pidfd-only cleanup primitives.
#![allow(clippy::missing_errors_doc)]

use crate::{ProcessHandle, media_boundary, media_scope};
use rustix::{
    event::{PollFd, PollFlags, poll},
    process::{WaitId, WaitIdOptions, waitid},
};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    process::{Child, Command, Stdio},
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn bounded(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(8193).read_to_end(&mut data)?;
    if data.len() > 8192 {
        return Err(invalid("oversized native process observation"));
    }
    Ok(data)
}

pub fn readonly_source(file: &File) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(file)?;
    if flags & rustix::fs::OFlags::ACCMODE != rustix::fs::OFlags::RDONLY
        || !file.metadata()?.is_file()
    {
        return Err(invalid(
            "probe source must be a read-only regular descriptor",
        ));
    }
    Ok(())
}

pub struct BoundaryObservation {
    pub unobservable: Vec<&'static str>,
    /// Open before the dumpable drop, retaining observation without ptrace or
    /// any change to the host's security policy.
    pub smaps: Option<File>,
}

pub fn observe_boundary(
    shim: &ProcessHandle,
    payload: &ProcessHandle,
    source: &File,
) -> io::Result<BoundaryObservation> {
    use std::os::unix::fs::MetadataExt;
    media_scope::verify_child(shim.pid(), payload)?;
    let base = format!("/proc/{}", payload.pid());
    let mut observation = BoundaryObservation {
        unobservable: Vec::new(),
        smaps: None,
    };
    match bounded(format!("{base}/status")) {
        Ok(bytes) => {
            let status = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
            for (key, expected) in [
                ("NoNewPrivs:", "1"),
                ("Seccomp:", "2"),
                ("CapInh:", "0000000000000000"),
                ("CapPrm:", "0000000000000000"),
                ("CapEff:", "0000000000000000"),
                ("CapBnd:", "0000000000000000"),
                ("CapAmb:", "0000000000000000"),
            ] {
                if status
                    .lines()
                    .find_map(|line| line.strip_prefix(key))
                    .map(str::trim)
                    != Some(expected)
                {
                    return Err(invalid("observed media boundary is not installed"));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            observation.unobservable.push("status");
        }
        Err(error) => return Err(error),
    }
    for namespace in ["user", "mnt", "net", "pid"] {
        match fs::read_link(format!("{base}/ns/{namespace}")) {
            Ok(child) => {
                if child == fs::read_link(format!("/proc/self/ns/{namespace}"))? {
                    return Err(invalid("media namespace still equals host namespace"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                observation.unobservable.push(namespace);
            }
            Err(error) => return Err(error),
        }
    }
    match fs::read_dir(format!("{base}/fd")) {
        Ok(entries) => {
            let mut slots = entries
                .map(|entry| {
                    entry?
                        .file_name()
                        .to_str()
                        .and_then(|value| value.parse::<u32>().ok())
                        .ok_or_else(|| invalid("media fd inventory entry"))
                })
                .collect::<io::Result<Vec<_>>>()?;
            slots.sort_unstable();
            if slots != [3, 4, 5, 6] {
                return Err(invalid("media inherited extra descriptors"));
            }
            let actual = fs::metadata(format!("{base}/fd/6"))?;
            let expected = source.metadata()?;
            if (actual.dev(), actual.ino(), actual.len())
                != (expected.dev(), expected.ino(), expected.len())
            {
                return Err(invalid("media source descriptor identity differs"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            observation.unobservable.push("descriptors");
        }
        Err(error) => return Err(error),
    }
    match File::open(format!("{base}/smaps_rollup")) {
        Ok(file) => observation.smaps = Some(file),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            observation.unobservable.push("PSS");
        }
        Err(error) => return Err(error),
    }
    media_scope::verify_child(shim.pid(), payload)?;
    Ok(observation)
}

pub fn resident_bytes(process: &ProcessHandle) -> io::Result<u64> {
    if exited(process)? {
        return Ok(0);
    }
    let sample = live_resident_bytes(process);
    // Exit can race the status read, which then omits VmRSS. An exited pidfd
    // proves zero residency; other observation failures remain failures.
    if exited(process)? {
        return Ok(0);
    }
    sample
}
fn live_resident_bytes(process: &ProcessHandle) -> io::Result<u64> {
    if process.reaped()? {
        return Err(invalid("RSS identity disappeared"));
    }
    let bytes = bounded(format!("/proc/{}/status", process.pid()))?;
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let kb = text
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| invalid("RSS unavailable"))?;
    if process.reaped()? {
        return Err(invalid("RSS identity changed"));
    }
    kb.checked_mul(1024).ok_or_else(|| invalid("RSS overflow"))
}

/// Polling an existing pidfd never resolves a numeric PID again.
pub fn exited(process: &ProcessHandle) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(process, PollFlags::IN)];
    Ok(poll(&mut descriptors, Some(&rustix::event::Timespec::default()))? != 0)
}

/// Reap precisely this adopted child. ECHILD alone is not evidence of cleanup.
pub fn reap_adopted(process: &ProcessHandle) -> io::Result<bool> {
    match waitid(
        WaitId::PidFd(process.as_fd()),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
    ) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(rustix::io::Errno::CHILD) => process.reaped(),
        Err(error) => Err(error.into()),
    }
}

/// Find descendants in the shim's immutable session while its unreaped identity
/// reserves that SID. The final filter denies setsid/setpgid and process clone.
pub fn session_members(shim: &ProcessHandle) -> io::Result<Vec<ProcessHandle>> {
    if !exited(shim)? || shim.reaped()? {
        return Err(invalid(
            "session anchor must be exited and unreaped during descendant cleanup",
        ));
    }
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == shim.pid() {
            continue;
        }
        let data = match bounded(entry.path().join("stat")) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        // comm can contain non-UTF-8, ')' and newlines. Only numeric fields
        // following its final delimiter are interpreted.
        let fields = data
            .rsplit(|byte| *byte == b')')
            .next()
            .ok_or_else(|| invalid("process stat delimiter"))?;
        let session = fields
            .split(u8::is_ascii_whitespace)
            .filter(|part| !part.is_empty())
            .nth(3)
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<u32>().ok());
        if session != Some(shim.pid()) {
            continue;
        }
        match ProcessHandle::open(pid) {
            Ok(process) => {
                // Pinning must be followed by another membership observation:
                // the numeric directory may have been recycled since the scan.
                let current = bounded(entry.path().join("stat"))?;
                let fields = current
                    .rsplit(|byte| *byte == b')')
                    .next()
                    .ok_or_else(|| invalid("process stat delimiter"))?;
                let mut parts = fields
                    .split(u8::is_ascii_whitespace)
                    .filter(|part| !part.is_empty());
                let session = parts
                    .nth(3)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .and_then(|value| value.parse::<u32>().ok());
                let start = parts
                    .nth(15)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .and_then(|value| value.parse::<u64>().ok());
                if session == Some(shim.pid()) && start == Some(process.start()) {
                    members.push(process);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if members.len() > 8 {
            return Err(invalid("media session exceeds admitted task count"));
        }
    }
    if shim.reaped()? {
        return Err(invalid("session anchor changed during observation"));
    }
    Ok(members)
}

pub fn spawner_pidfd() -> io::Result<Option<OwnedFd>> {
    let tid = rustix::thread::gettid().as_raw_pid().cast_unsigned();
    match media_boundary::pidfd(tid, true) {
        Ok(fd) => Ok(Some(fd)),
        Err(error) if error.raw_os_error() == Some(libc::EINVAL) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn spawn_supervisor(
    executable: &Path,
    generation: u64,
    endpoint: &UnixStream,
) -> io::Result<Child> {
    if !executable.is_absolute() || generation == 0 {
        return Err(invalid("supervisor launch identity"));
    }
    let mut command = Command::new(executable);
    command
        .args([std::process::id().to_string(), generation.to_string()])
        .env_clear()
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            media_scope::session_bus_address(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: endpoint remains borrowed and live through spawn; the audited
    // wrapper duplicates it in the child using only async-signal-safe operations.
    unsafe {
        media_scope::inherit_socket(&mut command, endpoint.as_raw_fd());
    }
    command.spawn()
}

pub fn spawn_helper(
    executable: &Path,
    root: &Path,
    mode: media_boundary::Mode,
    address_space: u64,
    allocation: u64,
    fds: [BorrowedFd<'_>; 5],
    spawner: Option<BorrowedFd<'_>>,
) -> io::Result<Child> {
    if !executable.is_absolute() || !root.is_absolute() {
        return Err(invalid("helper launch paths"));
    }
    let mut command = Command::new(executable);
    command
        .arg(std::process::id().to_string())
        .arg(root)
        .arg(match mode {
            media_boundary::Mode::Scope => "scope",
            media_boundary::Mode::Degraded => "degraded",
        })
        .arg(address_space.to_string())
        .arg(allocation.to_string())
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: all six possible descriptors stay borrowed through spawn. The
    // audited wrapper remaps child-only copies; it never closes the parent's fds.
    unsafe {
        media_boundary::inherit_media_fds(
            &mut command,
            fds.map(|fd| fd.as_raw_fd()),
            spawner.map(|fd| fd.as_raw_fd()),
        );
    }
    command.spawn()
}

/// Kernel maximum RSS of any waited-for child, in bytes (not a sum).
pub fn children_peak_resident() -> io::Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage writes one complete rusage into valid writable storage;
    // RUSAGE_CHILDREN selects only this caller's waited-for children.
    if unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful getrusage initialized the entire output object.
    let usage = unsafe { usage.assume_init() };
    u64::try_from(usage.ru_maxrss)
        .map_err(io::Error::other)?
        .checked_mul(1024)
        .ok_or_else(|| invalid("child peak RSS overflow"))
}

/// One point-in-time smaps sample plus the kernel's status high-water counters.
/// All sizes are bytes; VmRSS/VmHWM are explicitly the approximate status values.
#[derive(Debug)]
pub struct MemoryProfile {
    pub rss: u64,
    pub pss: u64,
    pub private_clean: u64,
    pub private_dirty: u64,
    pub shared_clean: u64,
    pub shared_dirty: u64,
    pub vm_rss_status: u64,
    pub vm_hwm_status: u64,
    pub vm_peak: u64,
    pub vm_size: u64,
}
pub fn open_smaps(process: &ProcessHandle) -> io::Result<File> {
    if process.reaped()? {
        return Err(invalid("memory observation identity disappeared"));
    }
    let file = File::open(format!("/proc/{}/smaps_rollup", process.pid()))?;
    if process.reaped()? {
        return Err(invalid("memory observation identity changed"));
    }
    Ok(file)
}
pub fn memory_profile(process: &ProcessHandle, smaps: &mut File) -> io::Result<MemoryProfile> {
    use std::io::{Seek, SeekFrom};
    if process.reaped()? {
        return Err(invalid("memory observation identity disappeared"));
    }
    smaps.seek(SeekFrom::Start(0))?;
    let mut rollup = String::new();
    smaps.take(8193).read_to_string(&mut rollup)?;
    if rollup.len() > 8192 {
        return Err(invalid("oversized smaps observation"));
    }
    let status = bounded(format!("/proc/{}/status", process.pid()))?;
    let status = std::str::from_utf8(&status).map_err(io::Error::other)?;
    let field = |text: &str, key| -> io::Result<u64> {
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| invalid("missing native memory counter"))?;
        value
            .checked_mul(1024)
            .ok_or_else(|| invalid("memory counter overflow"))
    };
    let profile = MemoryProfile {
        rss: field(&rollup, "Rss:")?,
        pss: field(&rollup, "Pss:")?,
        private_clean: field(&rollup, "Private_Clean:")?,
        private_dirty: field(&rollup, "Private_Dirty:")?,
        shared_clean: field(&rollup, "Shared_Clean:")?,
        shared_dirty: field(&rollup, "Shared_Dirty:")?,
        vm_rss_status: field(status, "VmRSS:")?,
        vm_hwm_status: field(status, "VmHWM:")?,
        vm_peak: field(status, "VmPeak:")?,
        vm_size: field(status, "VmSize:")?,
    };
    if process.reaped()? {
        return Err(invalid("memory observation identity changed"));
    }
    Ok(profile)
}
