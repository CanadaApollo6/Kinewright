use super::{COMMAND, LIFETIME, RESPONSE, SOURCE};
use seccompiler::{
    BpfProgram, SeccompAction as Action, SeccompCmpArgLen as Len, SeccompCmpOp as Op,
    SeccompCondition as Condition, SeccompFilter as Filter, SeccompRule as Rule,
};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Scope,
    Degraded,
}
fn eq(index: u8, value: u64) -> io::Result<Condition> {
    Condition::new(index, Len::Qword, Op::Eq, value).map_err(io::Error::other)
}
fn masked(index: u8, mask: u64, value: u64) -> io::Result<Condition> {
    Condition::new(index, Len::Qword, Op::MaskedEq(mask), value).map_err(io::Error::other)
}
fn rule(conditions: Vec<Condition>) -> io::Result<Rule> {
    Rule::new(conditions).map_err(io::Error::other)
}
fn fds(descriptors: &[i32]) -> io::Result<Vec<Rule>> {
    descriptors
        .iter()
        .map(|fd| rule(vec![eq(0, (*fd).cast_unsigned().into())?]))
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "the audited syscall table stays contiguous"
)]
fn install(
    mode: Mode,
    default: Action,
    apply: fn(seccompiler::BpfProgramRef<'_>) -> seccompiler::Result<()>,
) -> io::Result<()> {
    let arch = std::env::consts::ARCH
        .try_into()
        .map_err(io::Error::other)?;
    let fallback: BpfProgram = Filter::new(
        [(libc::SYS_clone3, vec![])].into(),
        Action::Allow,
        Action::Errno(libc::ENOSYS.cast_unsigned()),
        arch,
    )
    .map_err(io::Error::other)?
    .try_into()
    .map_err(io::Error::other)?;
    let mut rules = BTreeMap::new();
    for syscall in [
        libc::SYS_munmap,
        libc::SYS_brk,
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_sched_yield,
        libc::SYS_poll,
        libc::SYS_ppoll,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_getppid,
        libc::SYS_getrandom,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_clone3,
    ] {
        rules.insert(syscall, vec![]);
    }
    rules.insert(libc::SYS_read, fds(&[SOURCE, COMMAND, LIFETIME])?);
    rules.insert(libc::SYS_pread64, fds(&[SOURCE, COMMAND])?);
    rules.insert(libc::SYS_lseek, fds(&[SOURCE, COMMAND])?);
    rules.insert(libc::SYS_write, fds(&[RESPONSE])?);
    rules.insert(libc::SYS_writev, fds(&[RESPONSE])?);
    rules.insert(
        libc::SYS_close,
        fds(&[SOURCE, COMMAND, RESPONSE, LIFETIME])?,
    );
    rules.insert(libc::SYS_fstat, fds(&[SOURCE, COMMAND, RESPONSE])?);
    // libc may zero-extend its signed 32-bit fd argument on this syscall ABI.
    // Both exact representations mean -1; no other high bits are accepted.
    rules.insert(
        libc::SYS_mmap,
        [u64::MAX, u64::from(u32::MAX)]
            .into_iter()
            .map(|fd| {
                rule(vec![
                    masked(
                        2,
                        !u64::from((libc::PROT_READ | libc::PROT_WRITE).cast_unsigned()),
                        0,
                    )?,
                    masked(
                        3,
                        u64::from(libc::MAP_ANONYMOUS.cast_unsigned()),
                        u64::from(libc::MAP_ANONYMOUS.cast_unsigned()),
                    )?,
                    eq(4, fd)?,
                ])
            })
            .collect::<io::Result<_>>()?,
    );
    for syscall in [libc::SYS_mprotect, libc::SYS_pkey_mprotect] {
        rules.insert(
            syscall,
            vec![rule(vec![masked(
                2,
                !u64::from((libc::PROT_READ | libc::PROT_WRITE).cast_unsigned()),
                0,
            )?])?],
        );
    }
    // Ruling 33: libc realloc may resize/move its own mapping. Reject mapping
    // duplication (old_size == 0), FIXED, DONTUNMAP and all high-bit aliases.
    rules.insert(
        libc::SYS_mremap,
        [0, u64::from(libc::MREMAP_MAYMOVE.cast_unsigned())]
            .into_iter()
            .map(|flags| {
                rule(vec![
                    Condition::new(1, Len::Qword, Op::Gt, 0).map_err(io::Error::other)?,
                    eq(3, flags)?,
                ])
            })
            .collect::<io::Result<_>>()?,
    );
    rules.insert(
        libc::SYS_madvise,
        [
            libc::MADV_NORMAL,
            libc::MADV_DONTNEED,
            libc::MADV_FREE,
            102,
            103,
        ]
        .into_iter()
        .map(|value| rule(vec![eq(2, value.cast_unsigned().into())?]))
        .collect::<io::Result<_>>()?,
    );
    let mut futex = Vec::new();
    for op in [0, 1, 128, 129] {
        futex.push(rule(vec![eq(1, op)?])?);
    }
    for op in [9, 137, 265, 393] {
        futex.push(rule(vec![eq(1, op)?, eq(5, u64::from(u32::MAX))?])?);
    }
    rules.insert(libc::SYS_futex, futex);
    rules.insert(libc::SYS_sched_getaffinity, vec![rule(vec![eq(0, 0)?])?]);
    // SAFETY: getpid has no preconditions and captures the helper's own namespace tgid.
    rules.insert(
        libc::SYS_tgkill,
        vec![rule(vec![eq(
            0,
            unsafe { libc::getpid() }.cast_unsigned().into(),
        )?])?],
    );
    if mode == Mode::Scope {
        let flags = libc::CLONE_VM
            | libc::CLONE_FS
            | libc::CLONE_FILES
            | libc::CLONE_SIGHAND
            | libc::CLONE_THREAD
            | libc::CLONE_SYSVSEM
            | libc::CLONE_SETTLS
            | libc::CLONE_PARENT_SETTID
            | libc::CLONE_CHILD_CLEARTID;
        rules.insert(
            libc::SYS_clone,
            vec![rule(vec![eq(0, flags.cast_unsigned().into())?])?],
        );
    }
    rules.insert(
        libc::SYS_prctl,
        vec![rule(vec![
            eq(0, libc::PR_SET_DUMPABLE.cast_unsigned().into())?,
            eq(1, 0)?,
        ])?],
    );
    let policy: BpfProgram = Filter::new(rules, default, Action::Allow, arch)
        .map_err(io::Error::other)?
        .try_into()
        .map_err(io::Error::other)?;
    apply(&fallback).map_err(io::Error::other)?;
    apply(&policy).map_err(io::Error::other)
}

pub fn install_filter(mode: Mode) -> io::Result<()> {
    install(
        mode,
        Action::KillProcess,
        seccompiler::apply_filter_all_threads,
    )
}
#[cfg(test)]
pub(super) fn install_diagnostic(mode: Mode) -> io::Result<()> {
    install(mode, Action::Trap, seccompiler::apply_filter_all_threads)
}
#[cfg(test)]
pub(super) fn install_pthread_fixture(mode: Mode, diagnostic: bool) -> io::Result<()> {
    let action = if diagnostic {
        Action::Trap
    } else {
        Action::KillProcess
    };
    // New pthreads inherit both filters from the fixture thread. Libtest's
    // existing runner thread stays outside this pthread creation witness.
    install(mode, action, seccompiler::apply_filter)
}
