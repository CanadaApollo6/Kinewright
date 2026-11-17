# MS2 slice 1a

Status: BLOCKED during native seccomp qualification. Linux IX admission is not confined by this branch yet.
Commits: `4a55b8d4` records the plan; `79d93705` adds the IPC crate.
The unqualified step-2 work is parked in `.review/arch/ms2-s1a-helper-wip.patch`; `git apply --check` passes.
The active tree retains the gated IPC code. No failed implementation step was committed.
The plan records the lead's correction: use base FFmpeg unchanged and share the `owned_io` opener with the helper. MS1 is independent.

Blocker: on Arch with glibc 2.44, C `pthread_create` under the production filter terminates with SIGSYS, return code -31.
A test-only TRAP diagnostic identifies syscall 28, `madvise`, length 4096, advice 102 (`MADV_GUARD_INSTALL`). Rust thread creation produces the same call.
The approved table at `docs/MS2-MEDIA-SANDBOX.md:504` allows only NORMAL, DONTNEED, and FREE. That conflicts with this libc's primary thread-creation path.
The pinned `libavcodec.so` imports `pthread_create`. No additional syscall or advice was allowed to work around this conflict.
Evidence and the exact tested binary SHA-256 are in `docs/reports/ms2-s1a-evidence/`. A design ruling is required before continuing.

Witnesses: the isolated listed seccomp denials return SIGSYS and their filter-free controls return successfully.
The final filter joins a pre-created running worker; primary thread creation remains failing. The unfiltered clone3 control passed; the filtered fallback case was not reached.
The Landlock canary test passed, but its optional-unavailable branch was not separately excluded; do not count it as full Landlock qualification.
IPC at `79d93705`: 5 tests passed, clippy passed, formatting passed, Windows all-target check passed, and slow-test lint passed. No libFuzzer run.
Probe PSS, private/shared RSS, VmPeak, and cgroup measurements: NOT RUN. No production probe MemoryMax or RLIMIT_AS was frozen.
Helper probes, complete escape/kill-chain witnesses, supervisor, spawner, degraded RSS supervision, admission rewiring, and Ubuntu qualification remain incomplete.
The parked code compiled before later edits; its clippy gate failed and its first four native probe tests failed before fork. It is not review-ready implementation.
No transient scopes were created. Final `systemctl --user list-units 'kinewright-*' --all` shows zero units. Target remained 28 GB.
Final app build and full touched-crate slow suites: NOT RUN because the required native policy gate failed.
1b still requires the native Windows LPAC spike, SID/install ACLs, storage denial or quota, Job/CPU/mitigation checks, handle/IPC canaries, and probe/admission integration. Windows remains in process.
