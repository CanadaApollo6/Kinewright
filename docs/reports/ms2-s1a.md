# MS2 slice 1a
Status: IN PROGRESS; spawner/kill chain qualified; Linux IX admission remains in process until rewiring.
Commits: `4a55b8d4` plan; `79d93705` IPC; `90741cf4`/`ac1c60ce` historical stops; `2d926a59` helper; `7afcde61` supervisor; `ef8195b2` polling; `6a11af0f` CI pin; `9edbcf98` AU6; spawner in this commit.
FFmpeg: base setup unchanged; libavcodec 62.11.100 SHA256 `daf171e4d0648d5483f94a0d1fb97fd42c1b99c5f8ce990865235f6e0b8831fa`; MS1 is independent and untouched.
App/helper share the extracted owned_io opener, so later MS1 policy reaches both there.
Ruling 32: exact madvise 0/4/8/102/103; native guard install/remove succeeds; other advice/high-bit aliases die with SIGSYS and controls survive.
Ruling 33: sched_getaffinity arg0 exactly 0; self-query succeeds; explicit PID/high-bit aliases die and controls survive; cited table row added.
The final filter joins a running worker, creates/joins a C pthread, permits libc anonymous mmap, and returns clone3 ENOSYS; forbidden-operation controls pass.
Native helper: MP4/PNG/UHD AV1 first-frame/close, WAV/MP3 admission, native open cancellation and duplicated-fd offset preservation pass, 7 tests.
Audited direct-close ownership preserves fcntl denial; the watchdog initializes before TSYNC and supplies no non-cooperative kill guarantee.
IPC strict framing, typed state, pre-allocation/text limits and exact explicit-stream selection pass, 8 tests; libFuzzer NOT RUN.
Landlock denies canary reads with EACCES, control reads succeed and the canary is unchanged; x32/foreign-arch/degraded pthread attempts die with SIGSYS.
Lifecycle: 17 watchdog-disabled cases cover app/supervisor loss before ready/during decode, spawner abort before arming/during decode, PID-0 pre-arm loss, and cancel/deadline/revocation discarding partial metadata.
Four kernel/guard removals fail at their intended assertions and restored controls pass; a fifth removal shows stream replacement/widening is refused only with the host binding check.
Every test scope was stopped; `systemctl --user list-units 'kinewright-*' --all --no-pager` reports zero units.
Ruling 34 CI gate: 7 self-tests plus a real-app direct-zbus mutation fail/restore/pass; no app feature edits; one locked zbus 5.19.0, accessibility only in the app.
Evidence: `ms2-s1a-evidence/spawner-gates.json` and raw-log archive; supervisor catalog retains all 99 packages; helper has no zbus, supervisor has no FFmpeg.
Spawner gates: 43 touched-crate slow-feature tests pass, zero skips; default/slow all-target clippy -D warnings, fmt, 129-test manifest, all 45 parser self-tests, boundary gate and actionlint pass.
Probe PSS/private/shared RSS/VmPeak/cgroup measurements: NOT RUN; probe MemoryMax/RLIMIT_AS remain unfrozen.
Accepted deviations: inherited shim pidfd closes the PID-0 race; systemd PIDFDs prevent delayed reuse; ruling 34 preserves app accessibility. No new helper syscall allowance.
Remaining: degraded RSS supervision/overshoot, measurements, admission rewiring, full-stack escape witnesses, fuzz and Ubuntu packaging/CI.
All-target MSVC cross-check of IPC/job-sys/helper/supervisor/client passed; shared FFmpeg/media native Windows, final app build and native Ubuntu qualification: NOT RUN.
Target remains below 40 GB. No desktop policy changes, hardware GPU use or push.
1b: LPAC spike, SID/install ACLs, storage denial/quota, Job/CPU/mitigation and handle/IPC canaries, then Windows probe/admission integration; Windows stays in process.
CI: dependency-free proofs isolate Cargo/Clippy 1.100 changes; 6 C8 cases pass on 1.99.1. AU6 iterator passes Linux workspace clippy 1.100 and all 1,267 media slow-feature tests; 32 existing opt-ins ignored.
