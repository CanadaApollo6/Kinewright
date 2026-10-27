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
/// Refuses unavailable user/cgroup/mount namespaces, identity maps or masks.
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
                | thread::UnshareFlags::NEWNS,
        )
    }
    .map_err(|e| io::Error::other(format!("payload_namespace_unavailable: {e}")))?;
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    verify_cgroup_namespace()?;
    mask_runtime_directory(uid)?;
    thread::set_no_new_privs(true)?;
    Ok(())
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
