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
const NETWORK: u64 = 3;
const HANDLED: u64 = (1 << 17) - 1;

#[repr(C)]
struct Ruleset {
    filesystem: u64,
    network: u64,
    scoped: u64,
}
#[repr(C, packed)]
struct Port {
    allowed: u64,
    port: u64,
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

/// Probe the required Landlock ABI without restricting the calling host.
///
/// # Errors
/// Refuses unavailable or older Landlock kernels.
pub fn require_provider_landlock() -> io::Result<()> {
    // SAFETY: VERSION takes no pointer and changes no process policy.
    let abi = checked(unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1) })?;
    if abi < 9 {
        return Err(io::Error::other("provider_landlock_abi_unavailable"));
    }
    Ok(())
}

/// Pins the executable and installs an inherited, deny-by-default filesystem policy.
/// Runtime trees are read-only; only scratch and the dedicated credential store are writable.
/// TCP connects are limited to the explicit port set; TCP bind is denied.
///
/// # Errors
/// Refuses kernels older than Landlock ABI 9, replaced executables or invalid rules.
pub fn restrict_provider(
    executable: &Path,
    identity: [u64; 2],
    runtime: &[std::path::PathBuf],
    scratch: &Path,
    credentials: &Path,
    connect_tcp: &[u16],
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
        network: NETWORK,
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
    for port in connect_tcp.iter().filter(|_| NETWORK != 0) {
        if *port == 0 {
            return Err(io::Error::other("invalid provider connect port"));
        }
        let rule = Port {
            allowed: 2,
            port: u64::from(*port),
        };
        // SAFETY: rule is the live kernel net-port layout; type 2 selects it.
        checked(unsafe {
            libc::syscall(libc::SYS_landlock_add_rule, rules.as_raw_fd(), 2, &rule, 0)
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
#[allow(
    clippy::too_many_lines,
    reason = "the architecture check and complete syscall denylist are audited together"
)]
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
        libc::SYS_getxattr,
        libc::SYS_lgetxattr,
        libc::SYS_fgetxattr,
        libc::SYS_listxattr,
        libc::SYS_llistxattr,
        libc::SYS_flistxattr,
        // x86_64 kernel ABI: set/get/list/removexattrat. The installed kernel
        // headers expose these before the pinned libc crate does.
        463,
        464,
        465,
        466,
        libc::SYS_shmget,
        libc::SYS_shmat,
        libc::SYS_shmdt,
        libc::SYS_shmctl,
        libc::SYS_msgget,
        libc::SYS_msgsnd,
        libc::SYS_msgrcv,
        libc::SYS_msgctl,
        libc::SYS_semget,
        libc::SYS_semop,
        libc::SYS_semtimedop,
        libc::SYS_semctl,
        libc::SYS_mq_open,
        libc::SYS_mq_unlink,
        libc::SYS_mq_timedsend,
        libc::SYS_mq_timedreceive,
        libc::SYS_mq_notify,
        libc::SYS_mq_getsetattr,
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
    #[allow(
        clippy::too_many_lines,
        reason = "one isolated subprocess exercises every confinement probe"
    )]
    fn s4b_c2_sandbox_child() {
        let Some(root) = std::env::var_os("KINEWRIGHT_S4B_SANDBOX_CHILD") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let case: u8 = std::env::var("KINEWRIGHT_S4B_CASE")
            .unwrap()
            .parse()
            .unwrap();
        let outside = (case == 5).then(|| File::open(root.join("outside")).unwrap());
        let executable = std::env::current_exe().unwrap();
        let metadata = fs::metadata(&executable).unwrap();
        let runtime: Vec<std::path::PathBuf> = ["/usr", "/lib", "/lib64", "/dev/null"]
            .map(Into::into)
            .into_iter()
            .chain([root.join("runtime")])
            .collect();
        let _pinned = restrict_provider(
            &executable,
            [metadata.dev(), metadata.ino()],
            &runtime,
            &root.join("scratch"),
            &root.join("credentials"),
            &[
                443,
                std::env::var("KINEWRIGHT_S4B_TCP")
                    .unwrap()
                    .parse()
                    .unwrap(),
            ],
        )
        .unwrap();
        restrict_provider_syscalls().unwrap();
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
        if case == 5 {
            let path = std::ffi::CString::new(root.join("outside").as_os_str().as_encoded_bytes())
                .unwrap();
            let file = outside.as_ref().unwrap();
            // SAFETY: All path/name pointers and the retained descriptor are live.
            // Null output buffers request lengths only. The *at tests use invalid
            // arguments intentionally: EPERM must precede argument validation.
            let results = unsafe {
                [
                    libc::getxattr(
                        path.as_ptr(),
                        c"user.kinewright".as_ptr(),
                        std::ptr::null_mut(),
                        0,
                    ),
                    libc::lgetxattr(
                        path.as_ptr(),
                        c"user.kinewright".as_ptr(),
                        std::ptr::null_mut(),
                        0,
                    ),
                    libc::fgetxattr(
                        file.as_raw_fd(),
                        c"user.kinewright".as_ptr(),
                        std::ptr::null_mut(),
                        0,
                    ),
                    libc::listxattr(path.as_ptr(), std::ptr::null_mut(), 0),
                    libc::llistxattr(path.as_ptr(), std::ptr::null_mut(), 0),
                    libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0),
                ]
            };
            assert!(
                results.iter().all(|result| *result == -1),
                "S4B_C2_FIX1_XATTR_READS_DENIED"
            );
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::EPERM),
                "S4B_C2_FIX1_XATTR_READS_DENIED"
            );
            #[cfg(target_arch = "x86_64")]
            for syscall in [463, 464, 465, 466] {
                // SAFETY: No argument may be inspected before seccomp denies this call.
                let result = unsafe {
                    libc::syscall(
                        syscall,
                        -1,
                        std::ptr::null::<libc::c_char>(),
                        0,
                        std::ptr::null::<u8>(),
                        0,
                    )
                };
                assert_eq!(result, -1, "S4B_C2_FIX1_XATTR_AT_DENIED");
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM),
                    "S4B_C2_FIX1_XATTR_AT_DENIED"
                );
            }
        }
        if case == 6 {
            // SAFETY: Invalid IDs cannot name a host object. These probes check
            // seccomp's EPERM, independently of IPC namespace lookup failures.
            unsafe {
                assert_eq!(libc::shmat(-1, std::ptr::null(), 0) as isize, -1);
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM),
                    "S4B_C2_FIX1_IPC_SYSCALLS_DENIED"
                );
                assert_eq!(libc::msgctl(-1, libc::IPC_RMID, std::ptr::null_mut()), -1);
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM),
                    "S4B_C2_FIX1_IPC_SYSCALLS_DENIED"
                );
                assert_eq!(libc::semctl(-1, 0, libc::GETVAL), -1);
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM),
                    "S4B_C2_FIX1_IPC_SYSCALLS_DENIED"
                );
                assert_eq!(
                    libc::mq_open(c"/kinewright-invalid-probe".as_ptr(), libc::O_RDONLY),
                    -1
                );
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM),
                    "S4B_C2_FIX1_IPC_SYSCALLS_DENIED"
                );
            }
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
        assert_eq!(
            fs::read(root.join("runtime/dependency")).unwrap(),
            b"runtime"
        );
        assert!(
            fs::write(root.join("runtime/dependency"), b"changed").is_err(),
            "C5_USER_RUNTIME_READ_ONLY"
        );
        fs::write(root.join("scratch/result"), b"private").unwrap();
        let allowed: u16 = std::env::var("KINEWRIGHT_S4B_TCP")
            .unwrap()
            .parse()
            .unwrap();
        let denied: u16 = std::env::var("KINEWRIGHT_S4B_DENIED_TCP")
            .unwrap()
            .parse()
            .unwrap();
        std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, allowed)).unwrap();
        assert!(
            matches!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, denied)), Err(error) if error.kind() == io::ErrorKind::PermissionDenied),
            "S4B_C5_CONNECT_PORT_DENIED"
        );
        assert!(
            matches!(std::net::TcpListener::bind("127.0.0.1:0"), Err(error) if error.kind() == io::ErrorKind::PermissionDenied),
            "S4B_C5_BIND_DENIED"
        );
    }

    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c5_network_ports() {
        sandbox_case(7);
    }

    fn sandbox_case(case: u8) {
        let root =
            std::env::temp_dir().join(format!("s4b-c2-sandbox-{}-{case}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("runtime")).unwrap();
        fs::write(root.join("runtime/dependency"), b"runtime").unwrap();
        fs::create_dir(root.join("scratch")).unwrap();
        fs::create_dir(root.join("credentials")).unwrap();
        fs::write(root.join("outside"), b"outside secret").unwrap();
        if case == 5 {
            let path = std::ffi::CString::new(root.join("outside").as_os_str().as_encoded_bytes())
                .unwrap();
            // SAFETY: The fixture owns this path and both byte slices remain live.
            assert_eq!(
                unsafe {
                    libc::setxattr(
                        path.as_ptr(),
                        c"user.kinewright".as_ptr(),
                        b"private-xattr".as_ptr().cast(),
                        13,
                        0,
                    )
                },
                0
            );
        }
        let socket = UnixListener::bind(root.join("desktop")).unwrap();
        let allowed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let denied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sandbox::tests::s4b_c2_sandbox_child",
                "--nocapture",
            ])
            .env("KINEWRIGHT_S4B_SANDBOX_CHILD", &root)
            .env("KINEWRIGHT_S4B_CASE", case.to_string())
            .env(
                "KINEWRIGHT_S4B_TCP",
                allowed.local_addr().unwrap().port().to_string(),
            )
            .env(
                "KINEWRIGHT_S4B_DENIED_TCP",
                denied.local_addr().unwrap().port().to_string(),
            )
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
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_home_denial() {
        sandbox_case(0);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_project_denial() {
        sandbox_case(1);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_desktop_denial() {
        sandbox_case(2);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_credential_boundary() {
        sandbox_case(3);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_metadata_denial() {
        sandbox_case(4);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_fix1_xattr_reads_denied() {
        sandbox_case(5);
    }
    #[test]
    #[ignore = "on-demand: Landlock ABI 9 native qualification"]
    fn s4b_c2_fix1_ipc_syscalls_denied() {
        sandbox_case(6);
    }
}
