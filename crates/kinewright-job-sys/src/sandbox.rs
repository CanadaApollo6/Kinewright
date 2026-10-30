//! Filesystem and IPC isolation layered inside the retained PID/mount boundary.
use std::{
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};

const READ: u64 = (1 << 0) | (1 << 2) | (1 << 3);
const FILE_READ: u64 = (1 << 0) | (1 << 2);
const WRITE: u64 =
    READ | (1 << 1) | (1 << 4) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 12) | (1 << 13) | (1 << 14);
const HANDLED: u64 = (1 << 17) - 1;

#[repr(C)]
struct Ruleset {
    filesystem: u64,
    network: u64,
    scoped: u64,
}
#[repr(C, packed)]
struct Beneath {
    allowed: u64,
    parent: i32,
}

fn checked(result: libc::c_long) -> io::Result<libc::c_long> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

/// Pins the executable and installs an inherited, deny-by-default filesystem policy.
/// Runtime trees are read-only; only scratch and the dedicated credential store are writable.
/// TCP/UDP remain available. Pathname and abstract desktop sockets remain outside the domain.
///
/// # Errors
/// Refuses kernels older than Landlock ABI 9, replaced executables or invalid rules.
pub fn restrict_provider(
    executable: &Path,
    identity: [u64; 2],
    runtime: &[std::path::PathBuf],
    scratch: &Path,
    credentials: &Path,
) -> io::Result<File> {
    // SAFETY: VERSION takes no pointer; the remaining syscalls borrow live, correctly sized ABI structures.
    let abi = checked(unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1) })?;
    if abi < 9 {
        return Err(io::Error::other("provider_landlock_abi_unavailable"));
    }
    let pinned = File::open(executable)?;
    let metadata = pinned.metadata()?;
    if [metadata.dev(), metadata.ino()] != identity || !metadata.is_file() {
        return Err(io::Error::other("provider_executable_identity_changed"));
    }
    let attributes = Ruleset {
        filesystem: HANDLED,
        network: 0,
        scoped: 3,
    };
    // SAFETY: attributes has the kernel's ABI 6 layout. A successful syscall returns an owned descriptor.
    let fd = checked(unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attributes,
            size_of::<Ruleset>(),
            0,
        )
    })?;
    // SAFETY: The syscall returned a new valid descriptor and ownership is transferred exactly once.
    let rules = unsafe { File::from_raw_fd(i32::try_from(fd).map_err(io::Error::other)?) };
    for (path, access) in runtime.iter().map(|path| (path.as_path(), READ)).chain([
        (scratch, WRITE),
        (credentials, WRITE),
        (executable, FILE_READ),
    ]) {
        let file = File::open(path)?;
        let access = if file.metadata()?.is_dir() {
            access
        } else {
            access & (FILE_READ | (1 << 1) | (1 << 14))
        };
        let rule = Beneath {
            allowed: access,
            parent: file.as_raw_fd(),
        };
        // SAFETY: rules and file remain open; rule is the packed kernel path-beneath layout.
        checked(unsafe {
            libc::syscall(libc::SYS_landlock_add_rule, rules.as_raw_fd(), 1, &rule, 0)
        })?;
    }
    rustix::thread::set_no_new_privs(true)?;
    // SAFETY: rules is live, flags zero selects the supported restrict-self operation.
    checked(unsafe { libc::syscall(libc::SYS_landlock_restrict_self, rules.as_raw_fd(), 0) })?;
    Ok(pinned)
}

/// Closes mutation and descriptor escape operations not mediated by Landlock.
///
/// # Errors
/// Refuses unsupported architectures or a failed seccomp installation.
#[cfg(target_arch = "x86_64")]
pub fn restrict_provider_syscalls() -> io::Result<()> {
    fn statement(code: u16, value: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k: value,
        }
    }
    fn equal(value: u32, yes: u8, no: u8) -> libc::sock_filter {
        libc::sock_filter {
            code: 0x15,
            jt: yes,
            jf: no,
            k: value,
        }
    }
    let mut filter = vec![
        statement(0x20, 4),
        equal(0xc000_003e, 1, 0),
        statement(0x06, 0x8000_0000),
        statement(0x20, 0),
        // Reject the alternate x32 syscall table as well as foreign audit architectures.
        libc::sock_filter {
            code: 0x45,
            jt: 0,
            jf: 1,
            k: 0x4000_0000,
        },
        statement(0x06, 0x8000_0000),
    ];
    for syscall in [
        libc::SYS_chmod,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        libc::SYS_fchmodat2,
        libc::SYS_chown,
        libc::SYS_fchown,
        libc::SYS_lchown,
        libc::SYS_fchownat,
        libc::SYS_utime,
        libc::SYS_utimes,
        libc::SYS_futimesat,
        libc::SYS_utimensat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_fsetxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        libc::SYS_fremovexattr,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_mount_setattr,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
        libc::SYS_io_uring_setup,
        libc::SYS_open_by_handle_at,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
    ] {
        filter.push(equal(
            u32::try_from(syscall).map_err(io::Error::other)?,
            0,
            1,
        ));
        filter.push(statement(0x06, 0x0005_0000 | libc::EPERM.cast_unsigned()));
    }
    filter.push(statement(0x06, 0x7fff_0000));
    let program = libc::sock_fprog {
        len: u16::try_from(filter.len()).map_err(io::Error::other)?,
        filter: filter.as_mut_ptr(),
    };
    rustix::thread::set_no_new_privs(true)?;
    // SAFETY: program points to a live, bounded BPF array; the kernel copies it synchronously.
    checked(unsafe { libc::syscall(libc::SYS_seccomp, 1, 0, &program) })?;
    Ok(())
}

#[cfg(not(target_arch = "x86_64"))]
pub fn restrict_provider_syscalls() -> io::Result<()> {
    Err(io::Error::other(
        "provider_seccomp_architecture_unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::{
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        process::Command,
    };

    #[test]
    fn s4b_c2_sandbox_child() {
        let Some(root) = std::env::var_os("KINEWRIGHT_S4B_SANDBOX_CHILD") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let executable = std::env::current_exe().unwrap();
        let metadata = fs::metadata(&executable).unwrap();
        let runtime = ["/usr", "/lib", "/lib64", "/dev/null"].map(Into::into);
        let _pinned = restrict_provider(
            &executable,
            [metadata.dev(), metadata.ino()],
            &runtime,
            &root.join("scratch"),
            &root.join("credentials"),
        )
        .unwrap();
        restrict_provider_syscalls().unwrap();
        let case: u8 = std::env::var("KINEWRIGHT_S4B_CASE")
            .unwrap()
            .parse()
            .unwrap();
        if case == 0 {
            assert!(
                fs::read(root.join("outside")).is_err(),
                "S4B_C2_HOME_DENIED"
            );
        }
        if case == 1 {
            assert!(
                fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).is_err(),
                "S4B_C2_PROJECT_DENIED"
            );
        }
        if case == 2 {
            assert!(
                UnixStream::connect(root.join("desktop")).is_err(),
                "S4B_C2_DESKTOP_DENIED"
            );
        }
        if case == 3 {
            assert!(
                fs::write(root.join("outside"), b"changed").is_err(),
                "S4B_C2_CREDENTIAL_BOUNDARY"
            );
        }
        if case == 4 {
            assert!(
                fs::set_permissions(root.join("outside"), fs::Permissions::from_mode(0o777))
                    .is_err(),
                "S4B_C2_METADATA_DENIED"
            );
        }
        fs::write(root.join("credentials/auth.tmp-1"), b"refreshed").unwrap();
        fs::rename(
            root.join("credentials/auth.tmp-1"),
            root.join("credentials/auth.json"),
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("credentials/auth.json")).unwrap(),
            b"refreshed"
        );
        fs::write(root.join("scratch/result"), b"private").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let connection = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let _accepted = listener.accept().unwrap();
        drop(connection);
    }

    fn sandbox_case(case: u8) {
        let root =
            std::env::temp_dir().join(format!("s4b-c2-sandbox-{}-{case}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("scratch")).unwrap();
        fs::create_dir(root.join("credentials")).unwrap();
        fs::write(root.join("outside"), b"outside secret").unwrap();
        let socket = UnixListener::bind(root.join("desktop")).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sandbox::tests::s4b_c2_sandbox_child",
                "--nocapture",
            ])
            .env("KINEWRIGHT_S4B_SANDBOX_CHILD", &root)
            .env("KINEWRIGHT_S4B_CASE", case.to_string())
            .output()
            .unwrap();
        drop(socket);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(root.join("outside")).unwrap(), b"outside secret");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn s4b_c2_home_denial() {
        sandbox_case(0);
    }
    #[test]
    fn s4b_c2_project_denial() {
        sandbox_case(1);
    }
    #[test]
    fn s4b_c2_desktop_denial() {
        sandbox_case(2);
    }
    #[test]
    fn s4b_c2_credential_boundary() {
        sandbox_case(3);
    }
    #[test]
    fn s4b_c2_metadata_denial() {
        sandbox_case(4);
    }
}
