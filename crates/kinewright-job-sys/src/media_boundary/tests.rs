use super::*;
use crate::deadline;
use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[test]
fn namespace_setup_separates_policy_denials_from_resource_failures() {
    for errno in [libc::EPERM, libc::EACCES] {
        assert!(
            matches!(
                namespace_setup_error(io::Error::from_raw_os_error(errno)),
                NamespaceError::PolicyDenied(_)
            ),
            "errno {errno}"
        );
        assert!(matches!(
            second_unshare_error(io::Error::from_raw_os_error(errno)),
            NamespaceError::PolicyDenied(_)
        ));
    }
    for errno in [libc::ENOSPC, libc::ENOMEM, libc::EAGAIN, libc::EUSERS] {
        assert!(
            matches!(
                namespace_setup_error(io::Error::from_raw_os_error(errno)),
                NamespaceError::Setup(_)
            ),
            "errno {errno}"
        );
        assert!(matches!(
            second_unshare_error(io::Error::from_raw_os_error(errno)),
            NamespaceError::Setup(_)
        ));
    }
    for errno in [libc::EINVAL, libc::ENOSYS] {
        assert!(matches!(
            namespace_setup_error(io::Error::from_raw_os_error(errno)),
            NamespaceError::Setup(_)
        ));
        assert!(
            matches!(
                second_unshare_error(io::Error::from_raw_os_error(errno)),
                NamespaceError::RequiredNamespacesUnavailable(_)
            ),
            "errno {errno}"
        );
    }
}

// The syscall operands deliberately have no paths, listeners or real process IDs.
// Full-stack canaries are separate from these isolated, native seccomp predicates.
#[allow(clippy::too_many_lines, reason = "isolated syscall attack table")]
fn attack(name: &str) {
    #[cfg(target_arch = "x86_64")]
    if name == "i386_getpid" {
        // SAFETY: the compat getpid syscall has no pointer operands or side
        // effects. C caller-saved registers cover the interrupt ABI's clobbers.
        unsafe {
            std::arch::asm!("int 0x80", inlateout("eax") 20_u32 => _, clobber_abi("C"));
        }
        return;
    }
    if let Some(value) = name.strip_prefix("madvise_value:") {
        let advice = value.parse::<u64>().unwrap();
        // SAFETY: zero length and a null address cannot alter any mapping.
        unsafe {
            libc::syscall(libc::SYS_madvise, 0_u64, 0_u64, advice);
        }
        return;
    }
    let (syscall, args): (i64, [u64; 6]) = match name {
        "x32_getpid" => (libc::SYS_getpid | 0x4000_0000, [0; 6]),
        "affinity_other" => (libc::SYS_sched_getaffinity, [u64::MAX, 0, 0, 0, 0, 0]),
        "affinity_high" => (libc::SYS_sched_getaffinity, [1_u64 << 32, 0, 0, 0, 0, 0]),
        "open" => (libc::SYS_open, [0; 6]),
        "openat" => (libc::SYS_openat, [0; 6]),
        "openat2" => (libc::SYS_openat2, [0; 6]),
        "newfstatat" => (libc::SYS_newfstatat, [0; 6]),
        "statx" => (libc::SYS_statx, [0; 6]),
        "dup" => (libc::SYS_dup, [0; 6]),
        "dup2" => (libc::SYS_dup2, [0; 6]),
        "dup3" => (libc::SYS_dup3, [0; 6]),
        "fcntl" => (libc::SYS_fcntl, [0; 6]),
        "pidfd_getfd" => (libc::SYS_pidfd_getfd, [u64::MAX, 0, 0, 0, 0, 0]),
        "memfd_create" => (libc::SYS_memfd_create, [0; 6]),
        "userfaultfd" => (libc::SYS_userfaultfd, [0; 6]),
        "personality" => (libc::SYS_personality, [u32::MAX.into(), 0, 0, 0, 0, 0]),
        "ioctl" => (libc::SYS_ioctl, [u64::MAX, 0, 0, 0, 0, 0]),
        "mremap_duplicate" => (libc::SYS_mremap, [0, 0, 4096, 1, 0, 0]),
        "mremap_fixed" => (libc::SYS_mremap, [0, 4096, 8192, 3, 0, 0]),
        "mremap_dontunmap" => (libc::SYS_mremap, [0, 4096, 4096, 5, 0, 0]),
        "mremap_high" => (libc::SYS_mremap, [0, 4096, 8192, (1_u64 << 32) | 1, 0, 0]),
        "pkey_alloc" => (libc::SYS_pkey_alloc, [u64::MAX, 0, 0, 0, 0, 0]),
        "execve" => (libc::SYS_execve, [0; 6]),
        "execveat" => (libc::SYS_execveat, [0; 6]),
        "unshare" => (libc::SYS_unshare, [u64::MAX, 0, 0, 0, 0, 0]),
        "setns" => (libc::SYS_setns, [u64::MAX, 0, 0, 0, 0, 0]),
        "seccomp" => (libc::SYS_seccomp, [u64::MAX, 0, 0, 0, 0, 0]),
        "socket" => (
            libc::SYS_socket,
            [libc::AF_INET.cast_unsigned().into(), 1, 0, 0, 0, 0],
        ),
        "socketpair" => (
            libc::SYS_socketpair,
            [libc::AF_UNIX.cast_unsigned().into(), 1, 0, 0, 0, 0],
        ),
        "socket_netlink" => (
            libc::SYS_socket,
            [libc::AF_NETLINK.cast_unsigned().into(), 3, 0, 0, 0, 0],
        ),
        "socket_vsock" => (
            libc::SYS_socket,
            [libc::AF_VSOCK.cast_unsigned().into(), 1, 0, 0, 0, 0],
        ),
        "connect" => (libc::SYS_connect, [u64::MAX, 0, 0, 0, 0, 0]),
        "bind" => (libc::SYS_bind, [u64::MAX, 0, 0, 0, 0, 0]),
        "listen" => (libc::SYS_listen, [u64::MAX, 0, 0, 0, 0, 0]),
        "accept" => (libc::SYS_accept, [u64::MAX, 0, 0, 0, 0, 0]),
        "accept4" => (libc::SYS_accept4, [u64::MAX, 0, 0, 0, 0, 0]),
        "sendmsg" => (libc::SYS_sendmsg, [u64::MAX, 0, 0, 0, 0, 0]),
        "recvmsg" => (libc::SYS_recvmsg, [u64::MAX, 0, 0, 0, 0, 0]),
        "process_vm_readv" => (libc::SYS_process_vm_readv, [u64::MAX, 0, 0, 0, 0, 0]),
        "process_vm_writev" => (libc::SYS_process_vm_writev, [u64::MAX, 0, 0, 0, 0, 0]),
        "mount" => (libc::SYS_mount, [0; 6]),
        "bpf" => (libc::SYS_bpf, [0; 6]),
        "keyctl" => (libc::SYS_keyctl, [u64::MAX, 0, 0, 0, 0, 0]),
        "perf_event_open" => (libc::SYS_perf_event_open, [0; 6]),
        "io_uring_setup" => (libc::SYS_io_uring_setup, [0; 6]),
        "io_uring_enter" => (libc::SYS_io_uring_enter, [u64::MAX, 0, 0, 0, 0, 0]),
        "io_uring_register" => (libc::SYS_io_uring_register, [u64::MAX, 0, 0, 0, 0, 0]),
        "kill" => (libc::SYS_kill, [u64::MAX, 0, 0, 0, 0, 0]),
        "tkill" => (libc::SYS_tkill, [u64::MAX, 0, 0, 0, 0, 0]),
        "pidfd_send_signal" => (libc::SYS_pidfd_send_signal, [u64::MAX, 0, 0, 0, 0, 0]),
        "rt_sigqueueinfo" => (libc::SYS_rt_sigqueueinfo, [u64::MAX, 0, 0, 0, 0, 0]),
        "rt_tgsigqueueinfo" => (libc::SYS_rt_tgsigqueueinfo, [u64::MAX, 0, 0, 0, 0, 0]),
        "read_response" => (
            libc::SYS_read,
            [RESPONSE.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "read_high" => (
            libc::SYS_read,
            [(1 << 32) | u64::from(SOURCE.cast_unsigned()), 0, 0, 0, 0, 0],
        ),
        "pread_lifetime" => (
            libc::SYS_pread64,
            [LIFETIME.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "seek_response" => (
            libc::SYS_lseek,
            [RESPONSE.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "write_source" => (
            libc::SYS_write,
            [SOURCE.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "writev_command" => (
            libc::SYS_writev,
            [COMMAND.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "close_stderr" => (libc::SYS_close, [2, 0, 0, 0, 0, 0]),
        "fstat_lifetime" => (
            libc::SYS_fstat,
            [LIFETIME.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "mmap_source" => (
            libc::SYS_mmap,
            [0, 4096, 1, 2, SOURCE.cast_unsigned().into(), 0],
        ),
        "mmap_high_fd" => (
            libc::SYS_mmap,
            [0, 4096, 3, 34, (1_u64 << 32) | u64::from(u32::MAX), 0],
        ),
        "mmap_anon_source" => (
            libc::SYS_mmap,
            [0, 4096, 3, 34, SOURCE.cast_unsigned().into(), 0],
        ),
        "mmap_exec" => (libc::SYS_mmap, [0, 4096, 7, 34, u64::MAX, 0]),
        "mprotect_exec" => (libc::SYS_mprotect, [0, 4096, 7, 0, 0, 0]),
        "pkey_mprotect_exec" => (libc::SYS_pkey_mprotect, [0, 4096, 7, 0, 0, 0]),
        "madvise_other" => (
            libc::SYS_madvise,
            [0, 4096, libc::MADV_DONTFORK.cast_unsigned().into(), 0, 0, 0],
        ),
        "futex_requeue" => (libc::SYS_futex, [0, 3, 0, 0, 0, 0]),
        "futex_wake_bitset" => (libc::SYS_futex, [0, 10, 0, 0, 0, 0]),
        "futex_bad_mask" => (libc::SYS_futex, [0, 9, 0, 0, 0, 1]),
        "futex_high_op" => (libc::SYS_futex, [0, 1 << 32, 0, 0, 0, 0]),
        "tgkill_other" => (libc::SYS_tgkill, [u64::MAX, 0, 0, 0, 0, 0]),
        "clone_flags" => (
            libc::SYS_clone,
            [libc::CLONE_SIGHAND.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        "prctl_dumpable" => (
            libc::SYS_prctl,
            [libc::PR_SET_DUMPABLE.cast_unsigned().into(), 1, 0, 0, 0, 0],
        ),
        "prctl_other" => (
            libc::SYS_prctl,
            [libc::PR_GET_DUMPABLE.cast_unsigned().into(), 0, 0, 0, 0, 0],
        ),
        _ => std::process::exit(9),
    };
    // SAFETY: negative cases use null buffers, invalid descriptors, zero lengths,
    // or self-contained socket creation. No arbitrary process is signalled or inspected.
    unsafe {
        libc::syscall(
            syscall, args[0], args[1], args[2], args[3], args[4], args[5],
        );
    }
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "self-exec native boundary fixture dispatch"
)]
fn fixture() {
    let Ok(case) = std::env::var("KW_MEDIA_ATTACK") else {
        return;
    };
    let core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: this fixture only lowers its own core-dump limit before installing the filter.
    assert_eq!(
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const core) },
        0
    );
    if case == "landlock" {
        let path = std::env::var_os("KW_MEDIA_CANARY").unwrap();
        let control = std::env::var_os("KW_MEDIA_CONTROL").is_some();
        if !control && !landlock() {
            std::process::exit(77);
        }
        let result = std::fs::read(path);
        if control {
            assert_eq!(result.unwrap(), b"synthetic-canary");
        } else {
            assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EACCES));
        }
    } else if case == "join" {
        let running = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let state = Arc::clone(&running);
        let gate = Arc::clone(&release);
        let worker = std::thread::spawn(move || {
            state.store(true, Ordering::Release);
            while !gate.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(50));
        });
        while !running.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        install_filter(Mode::Scope).unwrap();
        release.store(true, Ordering::Release);
        worker.join().unwrap();
    } else if matches!(
        case.as_str(),
        "pthread" | "pthread_diag" | "degraded_pthread"
    ) {
        if std::env::var_os("KW_MEDIA_CONTROL").is_none() {
            if case.ends_with("_diag") {
                diagnostic();
                super::filter::install_diagnostic(Mode::Scope).unwrap();
            } else {
                install_filter(if case == "degraded_pthread" {
                    Mode::Degraded
                } else {
                    Mode::Scope
                })
                .unwrap();
            }
        }
        let mut thread = std::mem::MaybeUninit::uninit();
        // SAFETY: pthread_create initializes the pthread identifier on success;
        // the worker receives no borrowed Rust data and is joined before exit.
        unsafe {
            assert_eq!(
                libc::pthread_create(
                    thread.as_mut_ptr(),
                    std::ptr::null(),
                    worker,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_eq!(
                libc::pthread_join(thread.assume_init(), std::ptr::null_mut()),
                0
            );
        }
    } else if case == "primary_clone" || case == "primary_clone_diag" {
        if case.ends_with("_diag") {
            diagnostic();
            super::filter::install_diagnostic(Mode::Scope).unwrap();
        } else {
            install_filter(Mode::Scope).unwrap();
        }
        let worker = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(10)));
        worker.join().unwrap();
    } else if case == "guard_advice" {
        install_filter(Mode::Degraded).unwrap();
        // SAFETY: the fixture owns one anonymous page. Guard installation is
        // followed by removal before the page is accessed or unmapped.
        unsafe {
            let page = libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(page, libc::MAP_FAILED);
            let installed = libc::madvise(page, 4096, 102);
            if installed != 0 {
                // Older Ubuntu kernels may lack guard advice; errno is a native
                // refusal, rather than a SIGSYS policy denial.
                assert!(matches!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EINVAL | libc::ENOSYS)
                ));
            } else {
                assert_eq!(libc::madvise(page, 4096, 103), 0);
                page.cast::<u8>().write_volatile(71);
                assert_eq!(page.cast::<u8>().read_volatile(), 71);
            }
            assert_eq!(libc::munmap(page, 4096), 0);
        }
    } else if case == "affinity_self" {
        install_filter(Mode::Degraded).unwrap();
        // SAFETY: sched_getaffinity writes only this initialized, caller-owned
        // CPU bitmap; pid 0 queries the calling thread and cannot name a peer.
        unsafe {
            let mut cpus: libc::cpu_set_t = std::mem::zeroed();
            assert_eq!(
                libc::sched_getaffinity(0, std::mem::size_of_val(&cpus), &raw mut cpus),
                0
            );
            assert!(libc::CPU_COUNT(&cpus) > 0);
        }
    } else if case == "resize_mapping" {
        install_filter(Mode::Degraded).unwrap();
        resize_mapping();
    } else if case == "anonymous_mmap" {
        install_filter(Mode::Degraded).unwrap();
        // SAFETY: this fixture owns its anonymous page, never exposes a reference,
        // and unmaps exactly the successful allocation before exiting.
        unsafe {
            let page = libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(page, libc::MAP_FAILED);
            assert_eq!(libc::munmap(page, 4096), 0);
        }
    } else if case == "clone3" {
        let control = std::env::var_os("KW_MEDIA_CONTROL").is_some();
        if !control {
            install_filter(Mode::Degraded).unwrap();
        }
        // SAFETY: a null clone_args and zero length cannot create a process.
        let result = unsafe { libc::syscall(libc::SYS_clone3, 0, 0) };
        let errno = io::Error::last_os_error().raw_os_error();
        if result != -1
            || if control {
                errno == Some(libc::ENOSYS)
            } else {
                errno != Some(libc::ENOSYS)
            }
        {
            std::process::exit(8);
        }
    } else {
        if std::env::var_os("KW_MEDIA_CONTROL").is_none() {
            install_filter(Mode::Scope).unwrap();
        }
        attack(&case);
    }
    std::process::exit(0);
}
fn resize_mapping() {
    // SAFETY: the fixture exclusively owns the anonymous mapping. It checks
    // every resize before replacing its pointer and unmaps the final extent.
    unsafe {
        let page = libc::mmap(
            std::ptr::null_mut(),
            8192,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        assert_ne!(page, libc::MAP_FAILED);
        page.cast::<u8>().write_volatile(71);
        let smaller = libc::mremap(page, 8192, 4096, 0);
        assert_ne!(smaller, libc::MAP_FAILED);
        let larger = libc::mremap(smaller, 4096, 16384, libc::MREMAP_MAYMOVE);
        assert_ne!(larger, libc::MAP_FAILED);
        assert_eq!(larger.cast::<u8>().read_volatile(), 71);
        larger.cast::<u8>().add(16383).write_volatile(19);
        assert_eq!(libc::munmap(larger, 16384), 0);
    }
}
extern "C" fn worker(_: *mut libc::c_void) -> *mut libc::c_void {
    let delay = libc::timespec {
        tv_sec: 0,
        tv_nsec: 10_000_000,
    };
    // SAFETY: delay is initialized and nanosleep writes no output here.
    unsafe {
        libc::nanosleep(&raw const delay, std::ptr::null_mut());
    }
    std::ptr::null_mut()
}

fn child(case: &str, control: bool) -> std::process::Output {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "media_boundary::tests::fixture", "--nocapture"])
        .env("KW_MEDIA_ATTACK", case)
        .stdin(Stdio::null());
    if control {
        child.env("KW_MEDIA_CONTROL", "1");
    } else {
        child.env_remove("KW_MEDIA_CONTROL");
    }
    child.output().unwrap()
}
#[test]
fn final_filter_joins_running_worker_and_clone3_falls_back() {
    let _deadline = deadline::test();
    let control = child("clone3", true);
    assert!(
        control.status.success(),
        "clone3 negative control {control:?}"
    );
    for case in [
        "join",
        "pthread",
        "anonymous_mmap",
        "resize_mapping",
        "guard_advice",
        "affinity_self",
        "clone3",
    ] {
        let result = child(case, false);
        assert!(result.status.success(), "{case}: {result:?}");
    }
}
#[test]
#[cfg(target_arch = "x86_64")]
fn foreign_audit_architecture_and_x32_are_killed() {
    let _deadline = deadline::test();
    for case in ["i386_getpid", "x32_getpid"] {
        let denied = child(case, false);
        assert_eq!(
            denied.status.signal(),
            Some(libc::SIGSYS),
            "{case}: {denied:?}"
        );
        let control = child(case, true);
        assert!(control.status.success(), "{case}: {control:?}");
    }
}

#[test]
fn degraded_filter_kills_even_valid_pthread_creation() {
    let _deadline = deadline::test();
    let denied = child("degraded_pthread", false);
    assert_eq!(denied.status.signal(), Some(libc::SIGSYS), "{denied:?}");
    let control = child("degraded_pthread", true);
    assert!(control.status.success(), "{control:?}");
}

#[test]
fn each_isolated_denial_is_sigsys_and_changes_without_filter() {
    let _deadline = deadline::test();
    for case in [
        "affinity_other",
        "affinity_high",
        "open",
        "openat",
        "openat2",
        "newfstatat",
        "statx",
        "dup",
        "dup2",
        "dup3",
        "fcntl",
        "pidfd_getfd",
        "memfd_create",
        "userfaultfd",
        "personality",
        "ioctl",
        "mremap_duplicate",
        "mremap_fixed",
        "mremap_dontunmap",
        "mremap_high",
        "pkey_alloc",
        "execve",
        "execveat",
        "unshare",
        "setns",
        "seccomp",
        "socket",
        "socketpair",
        "socket_netlink",
        "socket_vsock",
        "connect",
        "bind",
        "listen",
        "accept",
        "accept4",
        "sendmsg",
        "recvmsg",
        "process_vm_readv",
        "process_vm_writev",
        "mount",
        "bpf",
        "keyctl",
        "perf_event_open",
        "io_uring_setup",
        "io_uring_enter",
        "io_uring_register",
        "kill",
        "tkill",
        "pidfd_send_signal",
        "rt_sigqueueinfo",
        "rt_tgsigqueueinfo",
        "read_response",
        "read_high",
        "pread_lifetime",
        "seek_response",
        "write_source",
        "writev_command",
        "close_stderr",
        "fstat_lifetime",
        "mmap_source",
        "mmap_exec",
        "mmap_high_fd",
        "mmap_anon_source",
        "mprotect_exec",
        "pkey_mprotect_exec",
        "madvise_other",
        "futex_requeue",
        "futex_wake_bitset",
        "futex_bad_mask",
        "futex_high_op",
        "tgkill_other",
        "clone_flags",
        "prctl_dumpable",
        "prctl_other",
    ] {
        let denied = child(case, false);
        assert_eq!(
            denied.status.signal(),
            Some(libc::SIGSYS),
            "{case}: {denied:?}"
        );
        let control = child(case, true);
        assert!(
            control.status.success(),
            "negative control {case}: {control:?}"
        );
    }
}

#[test]
fn landlock_alone_refuses_synthetic_canary_and_control_can_read() {
    let _deadline = deadline::test();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ms2-s1a");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("landlock-{}", std::process::id()));
    let mut canary = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut canary, b"synthetic-canary").unwrap();
    let run = |control: bool| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "media_boundary::tests::fixture", "--nocapture"])
            .env("KW_MEDIA_ATTACK", "landlock")
            .env("KW_MEDIA_CANARY", &path)
            .stdin(Stdio::null());
        if control {
            command.env("KW_MEDIA_CONTROL", "1");
        } else {
            command.env_remove("KW_MEDIA_CONTROL");
        }
        command.output().unwrap()
    };
    let control = run(true);
    let restricted = run(false);
    std::fs::remove_file(&path).unwrap();
    assert!(control.status.success(), "{control:?}");
    if restricted.status.code() == Some(77) {
        eprintln!("MS2_LANDLOCK_UNAVAILABLE: isolated filesystem witness not run");
    } else {
        assert!(restricted.status.success(), "{restricted:?}");
    }
}

#[cfg(target_arch = "x86_64")]
fn diagnostic() {
    #[repr(C)]
    struct Sigsys {
        signal: i32,
        errno: i32,
        code: i32,
        address: *mut libc::c_void,
        number: i32,
        arch: u32,
    }
    extern "C" fn handler(_: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
        // SAFETY: SA_SIGINFO passes the initialized SIGSYS layout and ucontext
        // from this x86_64 kernel. Only fixed-width scalar registers are inspected.
        unsafe {
            let info = &*info.cast::<Sigsys>();
            let registers = &(*context.cast::<libc::ucontext_t>()).uc_mcontext.gregs;
            let mut bytes = [0_u8; 56];
            bytes[..4].copy_from_slice(b"SYSC");
            bytes[4..8].copy_from_slice(&info.number.to_le_bytes());
            for (index, register) in [
                libc::REG_RDI,
                libc::REG_RSI,
                libc::REG_RDX,
                libc::REG_R10,
                libc::REG_R8,
                libc::REG_R9,
            ]
            .into_iter()
            .enumerate()
            {
                bytes[8 + index * 8..16 + index * 8].copy_from_slice(
                    &registers[usize::try_from(register).unwrap_or(0)].to_le_bytes(),
                );
            }
            libc::write(RESPONSE, bytes.as_ptr().cast(), bytes.len());
            libc::_exit(99);
        }
    }
    // SAFETY: the fixture owns its descriptor table. Refuse to replace any live
    // fd 4; dup2 gives the handler the response-only write capability in the table.
    unsafe {
        assert_eq!(libc::fcntl(RESPONSE, libc::F_GETFD), -1);
        assert_eq!(libc::dup2(1, RESPONSE), RESPONSE);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_flags = libc::SA_SIGINFO;
        action.sa_sigaction = handler as *const () as usize;
        libc::sigemptyset(&raw mut action.sa_mask);
        assert_eq!(
            libc::sigaction(libc::SIGSYS, &raw const action, std::ptr::null_mut()),
            0
        );
    }
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn all_other_madvise_values_and_high_bits_are_killed() {
    let _deadline = deadline::test();
    for advice in (0_u64..=255).chain([u64::MAX, 1_u64 << 32, (1_u64 << 32) | 0x66]) {
        if [0, 4, 8, 102, 103].contains(&advice) {
            continue;
        }
        let case = format!("madvise_value:{advice}");
        let denied = child(&case, false);
        assert_eq!(
            denied.status.signal(),
            Some(libc::SIGSYS),
            "{case}: {denied:?}"
        );
        let control = child(&case, true);
        assert!(control.status.success(), "{case}: {control:?}");
    }
}
