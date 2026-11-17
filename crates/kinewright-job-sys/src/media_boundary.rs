//! Audited Linux media syscall boundary. Call only in the separately exec'd shim.
#![allow(clippy::missing_errors_doc)]
use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
};

mod filter;
pub use filter::{Mode, install_filter};

pub const COMMAND: RawFd = 3;
pub const RESPONSE: RawFd = 4;
pub const LIFETIME: RawFd = 5;
pub const SOURCE: RawFd = 6;

#[repr(C)]
struct CapabilityHeader {
    version: u32,
    pid: i32,
}
#[repr(C)]
struct CapabilityData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn control(option: i32, argument: u64) -> io::Result<()> {
    // SAFETY: prctl's variadic arguments have their full unsigned-long width;
    // callers supply only the reviewed process-local bootstrap controls.
    check(unsafe { libc::prctl(option, argument, 0_u64, 0_u64, 0_u64) })
}

pub fn arm_parent_death(expected: u32, parent: Option<&OwnedFd>) -> io::Result<()> {
    // SAFETY: prctl changes only this task's parent-death signal; getppid has no pointers.
    control(
        libc::PR_SET_PDEATHSIG,
        u64::from(libc::SIGKILL.cast_unsigned()),
    )?;
    // SAFETY: getppid has no preconditions. A namespace init's outer parent is PID 0.
    if unsafe { libc::getppid() }.cast_unsigned() != expected {
        return Err(io::Error::other("media parent changed"));
    }
    if let Some(parent) = parent {
        let mut fd = libc::pollfd {
            fd: parent.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd points to one valid, initialized pollfd for the duration of poll.
        if unsafe { libc::poll(&raw mut fd, 1, 0) } != 0 {
            return Err(io::Error::other("media parent pidfd not live"));
        }
    }
    Ok(())
}

pub fn pidfd(pid: u32, thread: bool) -> io::Result<OwnedFd> {
    // PIDFD_THREAD is O_EXCL on Linux >=6.9. Failure is fatal unless the caller
    // explicitly treats an old kernel's EINVAL as absence of the secondary check.
    // SAFETY: pidfd_open has scalar arguments and returns a new owned descriptor.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_pidfd_open,
            pid,
            if thread {
                libc::O_EXCL.cast_unsigned()
            } else {
                0
            },
        )
    };
    let fd = i32::try_from(fd).map_err(io::Error::other)?;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful pidfd_open returned a fresh descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn new_session() -> io::Result<()> {
    // SAFETY: setsid changes only the separately exec'd shim's session.
    check(unsafe { libc::setsid() })
}

pub fn enter_namespaces(app_pid: u32, spawner: Option<&OwnedFd>) -> io::Result<()> {
    if fs::read_dir("/proc/self/task")?.count() != 1 {
        return Err(io::Error::other("threaded media bootstrap"));
    }
    // SAFETY: getuid/getgid have no preconditions. Capture the caller before
    // NEWUSER, where an unmapped identity would instead read as the overflow UID.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    // SAFETY: the dedicated, single-threaded shim owns its descriptor table.
    check(unsafe { libc::unshare(libc::CLONE_NEWUSER) })?;
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    arm_parent_death(app_pid, spawner)?;
    // SAFETY: no shared descriptor table or threads exist; NEWPID applies to the next fork.
    check(unsafe { libc::unshare(libc::CLONE_NEWNS | libc::CLONE_NEWNET | libc::CLONE_NEWPID) })
}

/// Retain only the four fixed capabilities; even stdio is absent after this call.
pub fn close_inventory() -> io::Result<()> {
    let descriptors: Vec<_> = fs::read_dir("/proc/self/fd")?
        .map(|entry| {
            let entry = entry?;
            entry
                .file_name()
                .to_str()
                .and_then(|v| v.parse::<RawFd>().ok())
                .ok_or_else(|| io::Error::other("fd inventory"))
        })
        .collect::<io::Result<_>>()?;
    for fd in descriptors {
        if [COMMAND, RESPONSE, LIFETIME, SOURCE].contains(&fd) {
            continue;
        }
        // SAFETY: the single-threaded bootstrap owns every inventoried descriptor.
        // The read_dir descriptor may already be closed; EBADF for it is harmless.
        if unsafe { libc::close(fd) } < 0
            && io::Error::last_os_error().raw_os_error() != Some(libc::EBADF)
        {
            return Err(io::Error::last_os_error());
        }
    }
    let mut actual: Vec<_> = fs::read_dir("/proc/self/fd")?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse::<RawFd>().ok())
        .collect();
    // read_dir itself occupies the smallest available fd (0).
    actual.sort_unstable();
    if actual != [0, COMMAND, RESPONSE, LIFETIME, SOURCE] {
        return Err(io::Error::other("unexpected inherited media fd"));
    }
    for (fd, write) in [
        (COMMAND, false),
        (RESPONSE, true),
        (LIFETIME, false),
        (SOURCE, false),
    ] {
        // SAFETY: fcntl queries a retained descriptor without changing its ownership.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0
            || flags & libc::O_ACCMODE
                != if write {
                    libc::O_WRONLY
                } else {
                    libc::O_RDONLY
                }
        {
            return Err(io::Error::other("media descriptor access mode"));
        }
    }
    // SAFETY: fstat fills an initialized output object for the retained source fd.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: stat points to writable storage of the exact libc stat layout.
    check(unsafe { libc::fstat(SOURCE, &raw mut stat) })?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::other("media source is not regular"));
    }
    Ok(())
}

/// Pivot to a private, bounded empty tmpfs; never mount proc, devices or host paths.
pub fn empty_root(root: &Path) -> io::Result<()> {
    use rustix::mount::{self, MountFlags, MountPropagationFlags, UnmountFlags};
    mount::mount_change(
        "/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )?;
    mount::mount(
        "kinewright-media-root",
        root,
        "tmpfs",
        MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC,
        c"size=4096,mode=0700",
    )?;
    fs::create_dir(root.join("old"))?;
    rustix::process::chdir(root)?;
    rustix::process::pivot_root(".", "old")?;
    rustix::process::chdir("/")?;
    mount::unmount("/old", UnmountFlags::DETACH)?;
    fs::remove_dir("/old")?;
    for capability in 0..64 {
        // SAFETY: prctl drops this task's capability bounding set monotonically.
        let result = control(libc::PR_CAPBSET_DROP, capability);
        if let Err(error) = result {
            if error.raw_os_error() == Some(libc::EINVAL) {
                break;
            }
            return Err(error);
        }
    }
    control(
        libc::PR_CAP_AMBIENT,
        u64::from(libc::PR_CAP_AMBIENT_CLEAR_ALL.cast_unsigned()),
    )?;
    // Remove all capability sets. No parser runs while this task is namespace root.
    let header = CapabilityHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let data = [
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];
    // SAFETY: capset reads two version-3 capability words with the kernel ABI layout.
    check(unsafe {
        i32::try_from(libc::syscall(
            libc::SYS_capset,
            &raw const header,
            data.as_ptr(),
        ))
        .map_err(io::Error::other)?
    })?;
    Ok(())
}

pub fn limits(address_space: u64) -> io::Result<()> {
    for (resource, limit) in [
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_FSIZE, 0),
        (libc::RLIMIT_NOFILE, 16),
        (libc::RLIMIT_STACK, 8 * 1024 * 1024),
        (libc::RLIMIT_AS, address_space),
        (libc::RLIMIT_CPU, 300),
    ] {
        let value = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        // SAFETY: value is a valid rlimit; only the helper's own limits are lowered.
        check(unsafe { libc::setrlimit(resource, &raw const value) })?;
    }
    // SAFETY: PR_SET_NO_NEW_PRIVS only removes authority from this process.
    control(libc::PR_SET_NO_NEW_PRIVS, 1)
}

pub fn drop_dumpable() -> io::Result<()> {
    // SAFETY: this is the monotonic final dumpability drop allowed by the filter.
    control(libc::PR_SET_DUMPABLE, 0)
}

/// Optional defence in depth, applied before the watchdog inherits this policy.
/// Failure never authorizes bypass of the namespace or seccomp boundary.
#[must_use]
pub fn landlock() -> bool {
    use landlock::{ABI, Access, AccessFs, AccessNet, Ruleset, RulesetAttr, Scope};
    // SAFETY: a null ruleset pointer and VERSION flag query the supported ABI only.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0,
            1,
        )
    };
    if version < 3 {
        return false;
    }
    let selected = ABI::from(i32::try_from(version).unwrap_or(9));
    let policy = (|| {
        let mut rules = Ruleset::default().handle_access(AccessFs::from_all(selected))?;
        if version >= 4 {
            rules = rules.handle_access(AccessNet::from_all(selected))?;
        }
        if version >= 6 {
            rules = rules.scope(Scope::from_all(selected))?;
        }
        rules.create()?.restrict_self()
    })();
    policy.is_ok_and(|status| {
        status.no_new_privs && status.ruleset == landlock::RulesetStatus::FullyEnforced
    })
}

/// Map the exact host capabilities across exec. The caller supplies descriptors
/// that remain owned until `Command::spawn` completes. No user closure runs after fork.
/// # Safety
/// Every supplied descriptor must remain valid until `Command::spawn` returns.
pub unsafe fn inherit_media_fds(
    command: &mut std::process::Command,
    fds: [RawFd; 5],
    spawner: Option<RawFd>,
) {
    use std::os::unix::process::CommandExt;
    // SAFETY: this pre_exec closure calls only async-signal-safe fcntl/dup2/close.
    // Original descriptors remain alive in the spawning thread until exec completes.
    unsafe {
        command.pre_exec(move || {
            let mut copies = [-1; 6];
            for (index, fd) in fds.into_iter().chain(spawner).enumerate() {
                copies[index] = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 32);
                if copies[index] < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            for (index, fd) in copies.into_iter().enumerate() {
                if fd < 0 {
                    continue;
                }
                let target = i32::try_from(index).map_err(io::Error::other)? + COMMAND;
                if libc::dup2(fd, target) < 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::close(fd);
            }
            Ok(())
        });
    }
}

/// An inventoried helper descriptor with syscall-only teardown. Rust's debug
/// `OwnedFd` destructor first calls `F_GETFD`, which is deliberately forbidden after
/// lockdown. Ownership stays unique and close remains checked by seccomp.
pub struct MediaFile(std::mem::ManuallyDrop<fs::File>);
impl io::Read for &MediaFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(output)
    }
}
impl io::Read for MediaFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(output)
    }
}
impl io::Write for MediaFile {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        (&*self.0).write(input)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}
impl MediaFile {
    /// Read the retained source without changing a duplicated open-file offset.
    /// The final filter permits pread only on source/command descriptors.
    pub fn read_at(&self, output: &mut [u8], offset: u64) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        self.0.read_at(output, offset)
    }
}
impl Drop for MediaFile {
    fn drop(&mut self) {
        // SAFETY: this uniquely owned descriptor is never closed or transferred
        // elsewhere. ManuallyDrop prevents a second close or a post-filter fcntl.
        // Do not retry close on EINTR, since the kernel may already have closed it.
        unsafe {
            libc::close(self.0.as_raw_fd());
        }
    }
}

/// Take one fixed, inherited descriptor in the separately exec'd helper.
///
/// # Safety
/// The descriptor is valid, owned by this process, and has no Rust owner.
#[must_use]
pub unsafe fn inherited_file(fd: RawFd) -> MediaFile {
    // SAFETY: only the dedicated helper calls this once per inventoried descriptor,
    // after bootstrap has finished all raw descriptor manipulation.
    MediaFile(std::mem::ManuallyDrop::new(unsafe {
        fs::File::from_raw_fd(fd)
    }))
}

pub fn close_shim_fds() -> io::Result<()> {
    for fd in [COMMAND, LIFETIME, SOURCE] {
        // SAFETY: the shim has created no Rust owners for these inherited descriptors.
        check(unsafe { libc::close(fd) })?;
    }
    Ok(())
}

/// Take ownership of the optional inherited spawner pidfd before any fork.
/// # Safety
/// When fd 8 exists it is the inherited spawner pidfd and has no Rust owner.
#[must_use]
pub unsafe fn inherited_spawner() -> Option<OwnedFd> {
    // SAFETY: fcntl queries fd 8 without borrowing or modifying other descriptors.
    if unsafe { libc::fcntl(8, libc::F_GETFD) } < 0 {
        return None;
    }
    // SAFETY: descriptor 8 is reserved by inherit_media_fds and has no other owner.
    Some(unsafe { OwnedFd::from_raw_fd(8) })
}

pub fn source_length() -> io::Result<u64> {
    // SAFETY: fstat fills an initialized object using the fixed source descriptor.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: stat points to an initialized, writable libc stat object.
    check(unsafe { libc::fstat(SOURCE, &raw mut stat) })?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::other("nonregular media source"));
    }
    u64::try_from(stat.st_size).map_err(io::Error::other)
}

#[cfg(test)]
mod tests;

/// The pre-created trusted watchdog uses this after TSYNC. It never supplies
/// app-loss containment; both parent-death edges remain mandatory.
pub fn watchdog_poll(command_active: bool) -> io::Result<bool> {
    let mut fds = [
        libc::pollfd {
            fd: LIFETIME,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: if command_active { COMMAND } else { -1 },
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: fds is an initialized, writable array of two pollfd objects.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), 2, 100) };
    if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
        return Err(io::Error::last_os_error());
    }
    if fds[0].revents != 0 || fds[1].revents & (libc::POLLHUP | libc::POLLNVAL | libc::POLLERR) != 0
    {
        exit_helper(125);
    }
    Ok(fds[1].revents & libc::POLLIN != 0)
}
pub fn exit_helper(status: i32) -> ! {
    // SAFETY: exit_group terminates this helper only, including namespace init.
    unsafe {
        libc::syscall(libc::SYS_exit_group, status);
    }
    std::process::abort()
}

pub fn wait_readable(
    fd: std::os::fd::BorrowedFd<'_>,
    timeout: std::time::Duration,
) -> io::Result<()> {
    use rustix::event::{PollFd, PollFlags, poll};
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let limit = rustix::event::Timespec::try_from(timeout).map_err(io::Error::other)?;
    if poll(&mut fds, Some(&limit))? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "media pipe deadline",
        ));
    }
    Ok(())
}
