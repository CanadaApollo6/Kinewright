# MS2 slice 1a
Status: IN PROGRESS; Linux IX admission remains in process until the later wiring step.
Commits: `4a55b8d4` plan; `79d93705` bounded IPC; `90741cf4` prior blocker evidence.
Step 2 (this commit): confined helper, shared owned_io input crate, audited syscall boundary and native witnesses; detailed gates in `ms2-s1a-evidence/helper-gates.json`.
FFmpeg: base setup script unchanged; libavcodec 62.11.100 SHA256 `daf171e4d0648d5483f94a0d1fb97fd42c1b99c5f8ce990865235f6e0b8831fa`; MS1 is independent.
App and helper use the same extracted owned_io opener; later MS1 policy changes reach both there.
Ruling 32: exact madvise 0/4/8/102/103; native guard install/remove succeeds; tested other advice values and high-bit aliases die with SIGSYS, controls survive.
Ruling 33: sched_getaffinity arg0 exactly 0; positive self-query passes, explicit PID and high-bit aliases die, controls survive; cited design row added.
The production filter joins a running worker, creates/joins a C pthread, permits libc anonymous mmap, and returns clone3 ENOSYS; isolated forbidden-operation controls pass.
Native helper: MP4/PNG/UHD AV1 first-frame/close and WAV/MP3 admission pass; native open cancellation and duplicated-fd offset preservation pass (7 tests).
Debug fd teardown uses an audited close wrapper, preserving the fcntl denial; no fd syscall was added.
The pre-created Rust watchdog finishes runtime initialization before TSYNC; it never supplies the non-cooperative kill guarantee.
IPC: strict framing, typed bootstrap/state, pre-allocation traffic charging and decoded-text limits pass (7 tests). No libFuzzer run yet.
Gates: 1,289 tests pass, 32 existing hardware/performance/data opt-ins ignored; touched all-target clippy -D warnings, fmt, slow lint and 44 lint self-tests pass.
Isolated Landlock: canary read denied EACCES, control reads, canary unchanged. x32/foreign architecture and degraded valid-pthread attempts die with SIGSYS; controls survive.
Probe PSS/private/shared RSS/VmPeak/cgroup measurements: NOT RUN; probe MemoryMax/RLIMIT_AS remain unfrozen.
Supervisor, spawner, complete degraded supervision, admission rewiring, full-stack escape/kill-chain and fuzz qualification remain pending.
No transient scopes created; current user-unit inventory is empty. Target is 30 GB. No desktop policy changes or hardware GPU use.
Windows helper/IPC/job-sys cross-check passed; shared FFmpeg/media Windows check, final app build, Ubuntu availability/missing-profile qualification: NOT RUN.
1b: native Windows LPAC spike, SID/install ACLs, storage denial/quota, Job/CPU/mitigation and handle/IPC canaries, then probe/admission integration; Windows stays in process.
