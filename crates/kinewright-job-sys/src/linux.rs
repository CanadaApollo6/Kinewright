use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags, openat},
    process::{Pid, PidfdFlags, Signal, WaitOptions},
};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::ChildStdin,
};

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

/// Refuse a lifetime writer that could survive an exec in another child.
///
/// # Errors
/// Returns an error if the descriptor is inheritable or cannot be inspected.
pub fn check_lifetime_writer(writer: &ChildStdin) -> io::Result<()> {
    if !rustix::io::fcntl_getfd(writer)?.contains(rustix::io::FdFlags::CLOEXEC) {
        return Err(invalid("inheritable lifetime writer"));
    }
    Ok(())
}

/// Become the orphan-descendant reaper before creating any payload child.
///
/// # Errors
/// Returns an error if the kernel cannot install or verify the subreaper.
pub fn install_subreaper() -> io::Result<()> {
    let enabled = Pid::from_raw(1).ok_or_else(|| invalid("subreaper flag"))?;
    rustix::process::set_child_subreaper(Some(enabled))?;
    if rustix::process::child_subreaper()?.is_none() {
        return Err(invalid("subreaper verification failed"));
    }
    Ok(())
}

/// Reap only children of this dedicated guardian, including adopted orphans.
///
/// # Errors
/// Returns unexpected wait errors. A live child returns false.
pub fn reap_all() -> io::Result<bool> {
    loop {
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(false),
            Err(rustix::io::Errno::CHILD) => return Ok(true),
            Err(error) => return Err(error.into()),
        }
    }
}

/// A pinned process identity. Signalling never resolves a numeric PID again.
pub struct ProcessHandle {
    fd: OwnedFd,
    pid: u32,
    start: u64,
}

impl ProcessHandle {
    /// # Errors
    /// Refuses a missing/replaced process or unavailable pidfd support.
    pub fn open(pid: u32) -> io::Result<Self> {
        let before = start_time(pid)?;
        let native_pid = Pid::from_raw(i32::try_from(pid).map_err(io::Error::other)?)
            .ok_or_else(|| invalid("invalid PID"))?;
        let fd = rustix::process::pidfd_open(native_pid, PidfdFlags::NONBLOCK)?;
        if start_time(pid)? != before {
            return Err(invalid("process changed while pinning identity"));
        }
        Ok(Self {
            fd,
            pid,
            start: before,
        })
    }

    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }
    #[must_use]
    pub fn start(&self) -> u64 {
        self.start
    }

    /// # Errors
    /// Returns a kernel signalling failure, except an already exited identity.
    pub fn kill(&self) -> io::Result<()> {
        match rustix::process::pidfd_send_signal(&self.fd, Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Exit alone is insufficient. A zombie must also disappear or be replaced.
    ///
    /// # Errors
    /// Returns an error if the process identity cannot be inspected reliably.
    pub fn reaped(&self) -> io::Result<bool> {
        match start_time(self.pid) {
            Ok(start) => Ok(start != self.start),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }
}

/// Observe the original PID/start identity after reopening, without signalling it.
/// # Errors
/// Refuses unreadable or malformed kernel identity data.
pub fn process_identity_reaped(pid: u32, start: u64) -> io::Result<bool> {
    match start_time(pid) {
        Ok(current) => Ok(current != start),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

/// Prove a systemd cgroup name has disappeared from the original cgroup-v2 mount.
/// Includes zombies in deleted descendants. The caller must also prove the
/// original guardian was reaped. This cannot signal
/// a replacement group or accept a regular-directory substitute.
/// # Errors
/// Refuses an existing name, symlink, changed mount, or failed kernel observation.
pub fn cgroup_removed(path: &Path, device: u64) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let normalized: PathBuf = path.components().collect();
    if normalized.as_os_str() != path.as_os_str() {
        return Err(invalid("noncanonical cgroup name"));
    }
    if !path.starts_with("/sys/fs/cgroup")
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(invalid("foreign cgroup path"));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut parent = path.parent().ok_or_else(|| invalid("cgroup parent"))?;
    while !parent.try_exists()? {
        parent = parent.parent().ok_or_else(|| invalid("cgroup mount"))?;
    }
    let file = File::open(parent)?;
    if fs::canonicalize(parent)? != parent
        || file.metadata()?.dev() != device
        || rustix::fs::fstatfs(&file)?.f_type != 0x6367_7270
    {
        return Err(invalid("cgroup mount changed"));
    }
    scan_cgroup_processes(path, None)
}

fn scan_cgroup_processes(path: &Path, excluded: Option<u32>) -> io::Result<bool> {
    let relative = path
        .strip_prefix("/sys/fs/cgroup")
        .map_err(io::Error::other)?;
    let expected = format!("0::/{}", relative.display());
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let pid = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok());
        if pid.is_none() || pid == excluded {
            continue;
        }
        match fs::read_to_string(entry.path().join("cgroup")) {
            Ok(text)
                if text.lines().any(|line| {
                    line == expected
                        || line
                            .strip_prefix(&expected)
                            .is_some_and(|suffix| suffix.starts_with('/') || suffix == " (deleted)")
                }) =>
            {
                return Ok(false);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io::Error::other(format!(
                    "cgroup scan {}: {error}",
                    entry.path().display()
                )));
            }
        }
    }
    Ok(true)
}

fn start_time(pid: u32) -> io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid("invalid /proc process identity"))
}

/// Retained cgroup-v2 identity. No PID or cgroup name authorizes a kill.
pub struct Cgroup {
    directory: File,
    events: File,
    kill: File,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Cgroup {
    /// Create a payload leaf under this independently managed service.
    ///
    /// # Errors
    /// Refuses missing cgroup-v2 delegation, native handles, or wrong service.
    pub fn create(unit: &str) -> io::Result<Self> {
        let own = fs::read_to_string("/proc/self/cgroup")?;
        let relative = own
            .lines()
            .find_map(|line| line.strip_prefix("0::/"))
            .ok_or_else(|| invalid("cgroup-v2 required"))?;
        let service = Path::new("/sys/fs/cgroup").join(relative);
        if service.file_name().and_then(|name| name.to_str()) != Some(unit) {
            return Err(invalid(
                "guardian is not the independent service main process",
            ));
        }
        let parent = File::from(rustix::fs::open(
            &service,
            OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        if rustix::fs::fstatfs(&parent)?.f_type != 0x6367_7270 {
            return Err(invalid("cgroup-v2 filesystem required"));
        }
        rustix::fs::mkdirat(&parent, "payload", Mode::RWXU)?;
        Self::open(&service.join("payload"))
    }

    /// Open a retained leaf after checking its filesystem and file identity.
    ///
    /// # Errors
    /// Refuses symlinks, other filesystems, or absent native kill/events handles.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = File::from(rustix::fs::open(
            path,
            OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        if rustix::fs::fstatfs(&directory)?.f_type != 0x6367_7270 {
            return Err(invalid("cgroup-v2 filesystem required"));
        }
        let stat = rustix::fs::fstat(&directory)?;
        let events = File::from(openat(
            &directory,
            "cgroup.events",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        let kill = File::from(openat(
            &directory,
            "cgroup.kill",
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        Ok(Self {
            directory,
            events,
            kill,
            path: path.to_owned(),
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    /// # Errors
    /// Returns a native whole-leaf termination error.
    pub fn kill(&mut self) -> io::Result<()> {
        self.kill.write_all(b"1")
    }

    /// # Errors
    /// Missing/malformed observation is never emptiness.
    pub fn empty(&mut self) -> io::Result<bool> {
        if let Err(error) = self.events.seek(SeekFrom::Start(0)) {
            if error.raw_os_error() == Some(19) {
                return self.kernel_deleted();
            }
            return Err(error);
        }
        let mut text = String::new();
        match (&self.events).take(1024).read_to_string(&mut text) {
            Ok(_) => {}
            Err(error) if error.raw_os_error() == Some(19) => return self.kernel_deleted(),
            Err(error) => return Err(error),
        }
        match text
            .lines()
            .find_map(|line| line.strip_prefix("populated "))
        {
            Some("0") => Ok(true),
            Some("1") => Ok(false),
            _ => Err(invalid("missing cgroup populated observation")),
        }
    }

    /// # Errors
    /// Returns an error if membership cannot be read or exceeds the profile cap.
    pub fn members(&self) -> io::Result<Vec<ProcessHandle>> {
        let file = File::from(openat(
            &self.directory,
            "cgroup.procs",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        let mut text = String::new();
        file.take(8193).read_to_string(&mut text)?;
        if text.len() > 8192 {
            return Err(invalid("cgroup membership cap"));
        }
        text.lines()
            .map(|line| ProcessHandle::open(line.parse().map_err(io::Error::other)?))
            .collect()
    }

    /// # Errors
    /// Returns an error if a child cannot join this retained leaf.
    pub fn enter(&self) -> io::Result<()> {
        let mut procs = File::from(openat(
            &self.directory,
            "cgroup.procs",
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        write!(procs, "{}", std::process::id())
    }
}

impl Cgroup {
    /// Certify the retained-inode deletion signal on this hierarchy and kernel.
    ///
    /// # Errors
    /// Missing zero-link evidence refuses launch, with no alternate proof route.
    pub fn require_deletion_signal(&self) -> io::Result<()> {
        let sample = (|| {
            let leaf = self.create_child("kernel-deletion-probe")?;
            let live = leaf.link_count()?;
            leaf.try_remove()?;
            let removed = leaf.link_count()?;
            if live == 0 || removed != 0 || !leaf.kernel_deleted()? {
                return Err(invalid(&format!(
                    "live_nlink={live}, deleted_nlink={removed}"
                )));
            }
            Ok(())
        })();
        sample.map_err(|error: io::Error| {
            invalid(&format!("kernel_deletion_signal_unavailable: {error}"))
        })
    }

    /// The retained cgroup inode is the only source of deletion evidence.
    /// Linux forbids live members/children at rmdir and joining a removed group.
    ///
    /// # Errors
    /// Any failed stat or changed native identity refuses proof.
    pub fn kernel_deleted(&self) -> io::Result<bool> {
        let stat = rustix::fs::fstat(&self.directory)?;
        if (stat.st_dev, stat.st_ino) != (self.device, self.inode) {
            return Err(invalid("retained cgroup identity changed"));
        }
        Ok(stat.st_nlink == 0)
    }

    /// # Errors
    /// Returns a failed native stat, never an inferred link count.
    pub fn link_count(&self) -> io::Result<u64> {
        Ok(rustix::fs::fstat(&self.directory)?.st_nlink)
    }

    /// Create a native witness leaf relative to a retained cgroup directory.
    ///
    /// # Errors
    /// Refuses invalid names or unavailable delegation.
    pub fn create_child(&self, name: &str) -> io::Result<Self> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(invalid("invalid cgroup leaf name"));
        }
        rustix::fs::mkdirat(&self.directory, name, Mode::RWXU)?;
        Self::open(&self.path.join(name))
    }

    /// Attempt a real rmdir while retaining the verified inode descriptor.
    ///
    /// # Errors
    /// A nonempty cgroup must refuse removal.
    pub fn try_remove(&self) -> io::Result<()> {
        fs::remove_dir(&self.path)
    }

    /// Include zombie identities that cgroup.events and cgroup.procs omit.
    ///
    /// # Errors
    /// Any unreadable potentially owned process prevents cleanup proof.
    pub fn no_processes(&self) -> io::Result<bool> {
        self.no_processes_except(None)
    }

    /// Inspect the whole retained subtree, optionally excluding the supervisor.
    ///
    /// # Errors
    /// Every error except an exited per-PID entry invalidates the sample.
    pub fn no_processes_except(&self, excluded: Option<u32>) -> io::Result<bool> {
        scan_cgroup_processes(&self.path, excluded)
    }
}

/// Check the actual lifetime pipe immediately before native release.
///
/// # Errors
/// Returns an error if the kernel cannot observe the input pipe.
pub fn lifetime_alive() -> io::Result<bool> {
    let stdin = io::stdin();
    let mut descriptors = [rustix::event::PollFd::new(
        &stdin,
        rustix::event::PollFlags::HUP | rustix::event::PollFlags::ERR,
    )];
    rustix::event::poll(
        &mut descriptors,
        Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    )?;
    Ok(!descriptors[0]
        .revents()
        .intersects(rustix::event::PollFlags::HUP | rustix::event::PollFlags::ERR))
}
