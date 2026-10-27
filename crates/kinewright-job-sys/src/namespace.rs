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
/// Refuses unavailable user/cgroup/mount/PID namespaces, identity maps or masks.
pub fn enter_payload_boundary() -> io::Result<()> {
    require_nsdelegate()?;
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    // SAFETY: These flags never unshare the descriptor table. The caller keeps
    // its descriptors valid; the kernel also refuses NEWUSER in a threaded task.
    unsafe {
        thread::unshare_unsafe(
            thread::UnshareFlags::NEWUSER
                | thread::UnshareFlags::NEWCGROUP
                | thread::UnshareFlags::NEWNS
                | thread::UnshareFlags::NEWPID,
        )
    }
    .map_err(|e| io::Error::other(format!("payload_pid_namespace_unavailable: {e}")))?;
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    verify_cgroup_namespace()?;
    mask_runtime_directory(uid)?;
    thread::set_no_new_privs(true)?;
    Ok(())
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
