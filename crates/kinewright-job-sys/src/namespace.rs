use rustix::{mount, thread};
use std::os::unix::fs::MetadataExt;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Require the kernel's cgroup-namespace migration boundary on the actual mount.
///
/// # Errors
/// Refuses missing delegation rather than launching an unrestricted payload.
pub fn require_nsdelegate() -> io::Result<()> {
    let info = fs::read_to_string("/proc/self/mountinfo")?;
    verify_nsdelegate(&info)
}

/// Validate the mountinfo observation before authorizing native launch.
///
/// # Errors
/// Refuses an absent delegation boundary on the cgroup2 mount.
pub fn verify_nsdelegate(info: &str) -> io::Result<()> {
    if !info.lines().any(|line| {
        let Some((mount, superblock)) = line.split_once(" - ") else {
            return false;
        };
        let mount: Vec<_> = mount.split_whitespace().collect();
        let superblock: Vec<_> = superblock.split_whitespace().collect();
        mount.get(4) == Some(&"/sys/fs/cgroup")
            && superblock.first() == Some(&"cgroup2")
            && superblock
                .get(2)
                .is_some_and(|options| options.split(',').any(|o| o == "nsdelegate"))
    }) {
        return Err(io::Error::other("cgroup_nsdelegate_unavailable"));
    }
    Ok(())
}

/// Enter namespaces only after the blocked shim has joined its retained leaf.
/// No provider code runs in this process before this function succeeds.
///
/// # Errors
/// Refuses unavailable user/cgroup/mount/PID/IPC namespaces, identity maps or masks.
pub fn enter_payload_boundary() -> io::Result<()> {
    require_nsdelegate()?;
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    for flag in [
        thread::UnshareFlags::NEWUSER,
        thread::UnshareFlags::NEWCGROUP,
        thread::UnshareFlags::NEWNS,
        thread::UnshareFlags::NEWPID,
        thread::UnshareFlags::NEWIPC,
    ] {
        unshare_payload_namespace(flag)?;
    }
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    verify_cgroup_namespace()?;
    mask_runtime_directory(uid)?;
    mask_host_ipc_files()?;
    thread::set_no_new_privs(true)?;
    Ok(())
}

/// C5 fix 8, the unpaid dry policy only: give the payload its own network
/// namespace in which only loopback exists, is up and carries every route.
/// Nothing sent inside it can leave the machine. Call right after
/// [`enter_payload_boundary`], while the shim still holds its new user
/// namespace's capabilities and before it forks namespace init.
///
/// # Errors
/// Refuses an unavailable network namespace, a loopback that will not come
/// up, or any interface or route besides loopback.
pub fn isolate_payload_network() -> io::Result<()> {
    let before = fs::metadata("/proc/self/ns/net")?.ino();
    unshare_payload_namespace(thread::UnshareFlags::NEWNET)?;
    if fs::metadata("/proc/self/ns/net")?.ino() == before {
        return Err(io::Error::other("payload_network_namespace_unavailable"));
    }
    loopback_up()?;
    verify_loopback_only(
        &fs::read_to_string("/proc/self/net/dev")?,
        &fs::read_to_string("/proc/self/net/route")?,
        &fs::read_to_string("/proc/self/net/ipv6_route").unwrap_or_default(),
    )
}

fn loopback_up() -> io::Result<()> {
    // SAFETY: socket takes no pointers; ownership of the new descriptor is
    // taken at once.
    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if socket < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket is a fresh, valid descriptor owned only here.
    let socket = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(socket) };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&socket);
    // SAFETY: ifreq is plain data; all-zero is a valid value.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    request.ifr_name[0] = b'l'.cast_signed();
    request.ifr_name[1] = b'o'.cast_signed();
    let flags = |request: &mut libc::ifreq| -> io::Result<libc::c_short> {
        // SAFETY: request is a live ifreq naming "lo"; GIFFLAGS fills ifru_flags.
        if unsafe { libc::ioctl(raw, libc::SIOCGIFFLAGS, std::ptr::from_mut(request)) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GIFFLAGS initialised the flags member of the union.
        Ok(unsafe { request.ifr_ifru.ifru_flags })
    };
    let up = libc::c_short::try_from(libc::IFF_UP).map_err(io::Error::other)?;
    request.ifr_ifru.ifru_flags = flags(&mut request)? | up;
    // SAFETY: request names "lo" and carries the flags to set.
    if unsafe { libc::ioctl(raw, libc::SIOCSIFFLAGS, std::ptr::from_mut(&mut request)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags(&mut request)? & up == 0 {
        return Err(io::Error::other("payload_loopback_unavailable"));
    }
    Ok(())
}

/// Only `lo` exists, the IPv4 main table is empty and every IPv6 route
/// leaves through `lo`.
///
/// # Errors
/// Refuses any other interface or route.
pub fn verify_loopback_only(dev: &str, route: &str, ipv6_route: &str) -> io::Result<()> {
    let interfaces = dev
        .lines()
        .skip(2)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim())
        .collect::<Vec<_>>();
    let routes = route.lines().skip(1).filter(|line| !line.trim().is_empty());
    if interfaces != ["lo"]
        || routes.count() != 0
        || ipv6_route
            .lines()
            .any(|line| line.split_whitespace().last() != Some("lo"))
    {
        return Err(io::Error::other("payload_network_not_loopback_only"));
    }
    Ok(())
}

fn mask_host_ipc_files() -> io::Result<()> {
    for path in ["/dev/shm", "/dev/mqueue"] {
        let previous = fs::metadata(path)?;
        mount::mount(
            "kinewright-ipc-mask",
            path,
            "tmpfs",
            mount::MountFlags::NOSUID | mount::MountFlags::NODEV | mount::MountFlags::NOEXEC,
            c"mode=0700,size=4096",
        )?;
        if previous.dev() == fs::metadata(path)?.dev() || fs::read_dir(path)?.next().is_some() {
            return Err(io::Error::other("host_ipc_mask_unavailable"));
        }
    }
    Ok(())
}

fn unshare_payload_namespace(flag: thread::UnshareFlags) -> io::Result<()> {
    // SAFETY: These flags never unshare the descriptor table. The caller keeps
    // its descriptors valid; the kernel also refuses NEWUSER in a threaded task.
    unsafe { thread::unshare_unsafe(flag) }.map_err(|error| {
        io::Error::other(format!(
            "payload_namespace_unavailable: unshare {flag:?}: {error} (errno={})",
            error.raw_os_error()
        ))
    })
}

/// Complete the boundary in the first child after NEWPID, before readiness.
///
/// # Errors
/// Refuses a missing PID namespace or unavailable fresh procfs.
pub fn mount_payload_proc() -> io::Result<()> {
    verify_payload_pid_namespace(
        rustix::process::getpid()
            .as_raw_nonzero()
            .get()
            .cast_unsigned(),
    )?;
    let previous = fs::metadata("/proc")?;
    mount::mount(
        "proc",
        "/proc",
        "proc",
        mount::MountFlags::NOSUID | mount::MountFlags::NODEV | mount::MountFlags::NOEXEC,
        None::<&std::ffi::CStr>,
    )
    .map_err(|e| io::Error::other(format!("payload_proc_mount_unavailable: {e}")))?;
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc")? {
        if let Some(pid) = entry?
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        {
            pids.push(pid);
        }
    }
    if previous.dev() == fs::metadata("/proc")?.dev() || pids != [1] {
        return Err(io::Error::other("payload_proc_mount_unavailable"));
    }
    verify_cgroup_namespace()?;
    verify_runtime_mask(&runtime_directory(rustix::process::getuid().as_raw()))?;
    Ok(())
}

/// Make namespace init non-dumpable before it starts the provider. Its /proc
/// entries then belong to root, so the same-UID provider cannot open them
/// whatever the host's Yama policy. The provider's exec resets its own state.
///
/// # Errors
/// Refuses if the kernel does not keep the setting.
pub fn make_payload_init_nondumpable() -> io::Result<()> {
    use rustix::process::{DumpableBehavior, dumpable_behavior, set_dumpable_behavior};
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(io::Error::other("payload_init_dumpable"));
    }
    Ok(())
}

/// Close the readiness writer before starting the provider.
///
/// # Errors
/// Refuses a failed descriptor replacement.
pub fn close_payload_readiness() -> io::Result<()> {
    rustix::stdio::dup2_stdout(fs::File::options().write(true).open("/dev/null")?)?;
    Ok(())
}

/// PID 1 reaps all adopted children until its direct provider exits.
///
/// # Errors
/// Refuses unexpected wait failures or disappearance of the provider.
pub fn reap_payload_until_exit(provider: u32) -> io::Result<i32> {
    loop {
        match rustix::process::wait(rustix::process::WaitOptions::empty()) {
            Ok(Some((pid, status))) if pid.as_raw_nonzero().get().cast_unsigned() == provider => {
                return Ok(status
                    .exit_status()
                    .unwrap_or_else(|| 128 + status.terminating_signal().unwrap_or(0)));
            }
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn mask_runtime_directory(uid: u32) -> io::Result<()> {
    let runtime = runtime_directory(uid);
    let previous = fs::metadata(&runtime)?;
    mount::mount_change(
        "/",
        mount::MountPropagationFlags::PRIVATE | mount::MountPropagationFlags::REC,
    )?;
    mount::mount(
        "kinewright-runtime-mask",
        &runtime,
        "tmpfs",
        mount::MountFlags::NOSUID | mount::MountFlags::NODEV | mount::MountFlags::NOEXEC,
        c"mode=0700,size=4096",
    )?;
    if previous.dev() == fs::metadata(&runtime)?.dev() {
        return Err(io::Error::other("runtime_directory_mask_unavailable"));
    }
    verify_runtime_mask(&runtime)?;
    Ok(())
}

fn runtime_directory(uid: u32) -> PathBuf {
    PathBuf::from(format!("/run/user/{uid}"))
}

fn verify_cgroup_namespace() -> io::Result<()> {
    if fs::read_to_string("/proc/self/cgroup")?.trim() != "0::/" {
        return Err(io::Error::other(
            "payload_cgroup_namespace_root_unavailable",
        ));
    }
    Ok(())
}

fn verify_runtime_mask(runtime: &Path) -> io::Result<()> {
    let info = fs::read_to_string("/proc/self/mountinfo")?;
    let expected = runtime.to_string_lossy();
    let masked =
        info.lines()
            .filter_map(|line| line.split_once(" - "))
            .any(|(mount, superblock)| {
                let fields: Vec<_> = mount.split_whitespace().collect();
                fields.get(4).copied() == Some(expected.as_ref())
                    && superblock.starts_with("tmpfs kinewright-runtime-mask ")
                    && !fields
                        .iter()
                        .any(|field| field.starts_with("shared:") || field.starts_with("master:"))
            });
    if !masked || fs::read_dir(runtime)?.next().is_some() {
        return Err(io::Error::other("runtime_directory_mask_unavailable"));
    }
    Ok(())
}

/// Fork the dedicated, single-threaded shim without losing namespace capabilities.
///
/// # Errors
/// Refuses a threaded caller or a failed fork. The parent receives the host PID.
pub fn fork_payload_init() -> io::Result<Option<u32>> {
    if fs::read_dir("/proc/self/task")?.count() != 1 {
        return Err(io::Error::other("payload_init_requires_single_thread"));
    }
    // SAFETY: The separately exec'd shim has exactly one thread, no held locks
    // or shared mappings. libc fork runs its runtime hooks; descriptors remain owned.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(None),
        pid => Ok(Some(pid.cast_unsigned())),
    }
}

/// Reap the namespace init from the outer shim using its host PID.
///
/// # Errors
/// Refuses wait failures or a missing child.
pub fn wait_payload_init(pid: u32) -> io::Result<i32> {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).map_err(io::Error::other)?)
        .ok_or_else(|| io::Error::other("invalid init PID"))?;
    loop {
        match rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty()) {
            Ok(Some((_, status))) => {
                return Ok(status
                    .exit_status()
                    .unwrap_or_else(|| 128 + status.terminating_signal().unwrap_or(0)));
            }
            Err(rustix::io::Errno::INTR) | Ok(None) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Verify that the first child entered the requested PID namespace.
///
/// # Errors
/// Refuses a missing namespace before procfs or provider execution.
pub fn verify_payload_pid_namespace(pid: u32) -> io::Result<()> {
    if pid != 1 {
        return Err(io::Error::other("payload_pid_namespace_unavailable"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn s4b_c5_fix8_network_must_be_loopback_only() {
        let header = "Inter-|   Receive\n face |bytes\n";
        let lo = format!("{header}    lo: 0 0\n");
        let v6 = "00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000001 00000000 80200001       lo\n";
        super::verify_loopback_only(&lo, "Iface\tDestination\n", v6).unwrap();
        let eth = format!("{lo}  eth0: 0 0\n");
        assert!(super::verify_loopback_only(&eth, "Iface\n", "").is_err());
        assert!(super::verify_loopback_only(&lo, "Iface\neth0\t00000000\n", "").is_err());
        let v6eth = v6.replace("       lo", "     eth0");
        assert!(super::verify_loopback_only(&lo, "Iface\n", &v6eth).is_err());
    }
    #[test]
    fn s4_unshare_failure_names_flag_and_errno() {
        let error = std::thread::spawn(|| {
            super::unshare_payload_namespace(rustix::thread::UnshareFlags::NEWUSER).unwrap_err()
        })
        .join()
        .unwrap();
        let reason = error.to_string();
        assert!(reason.contains("NEWUSER"), "failing flag missing: {reason}");
        assert!(reason.contains("errno="), "errno missing: {reason}");
    }
}
