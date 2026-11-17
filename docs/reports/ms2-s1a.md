# MS2 slice 1a
Status: IN PROGRESS; ruling 34 applied, supervisor qualified; Linux IX admission remains in process until rewiring.
Commits: `4a55b8d4` plan; `79d93705` IPC; `90741cf4`/`ac1c60ce` historical stops; `2d926a59` helper; `7afcde61` supervisor; `ef8195b2` polling; CI toolchain pin in this commit.
FFmpeg: base setup unchanged; libavcodec 62.11.100 SHA256 `daf171e4d0648d5483f94a0d1fb97fd42c1b99c5f8ce990865235f6e0b8831fa`; MS1 is independent and untouched.
App/helper share the extracted owned_io opener, so later MS1 policy reaches both there.
Ruling 32: exact madvise 0/4/8/102/103; native guard install/remove succeeds; other advice/high-bit aliases die with SIGSYS and controls survive.
Ruling 33: sched_getaffinity arg0 exactly 0; self-query succeeds; explicit PID/high-bit aliases die and controls survive; cited table row added.
The final filter joins a running worker, creates/joins a C pthread, permits libc anonymous mmap, and returns clone3 ENOSYS; forbidden-operation controls pass.
Native helper: MP4/PNG/UHD AV1 first-frame/close, WAV/MP3 admission, native open cancellation and duplicated-fd offset preservation pass, 7 tests.
Audited direct-close ownership preserves fcntl denial; the watchdog initializes before TSYNC and supplies no non-cooperative kill guarantee.
IPC strict framing, typed bootstrap/state, pre-allocation charging and decoded-text limits pass, 7 tests; libFuzzer NOT RUN.
Landlock denies canary reads with EACCES, control reads succeed and the canary is unchanged; x32/foreign-arch/degraded pthread attempts die with SIGSYS.
Helper gates: 1,289 tests pass, 32 existing opt-ins ignored; touched all-target clippy -D warnings, fmt, slow lint and 44 lint self-tests pass; details in `ms2-s1a-evidence/helper-gates.json`.
Supervisor: 29 touched-crate slow-feature tests and all-target clippy pass; scope join/limits/separation, bounded records, socket-EOF cleanup, stale/non-lifecycle refusal and no-manager/private-bus cases pass.
Every test scope was stopped; `systemctl --user list-units 'kinewright-*' --all --no-pager` reports zero units.
Ruling 34 CI gate: 7 self-tests plus a real-app direct-zbus mutation fail/restore/pass; no app feature edits; one locked zbus 5.19.0, accessibility only in the app.
Evidence: `ms2-s1a-evidence/supervisor-gates.json`, full normal/build trees and the 99-package license/checksum catalog; helper has no zbus, supervisor has no FFmpeg.
Slow manifest/lint agrees on 123 tests; all 45 lint self-tests pass; unavailable primary user-manager witnesses are counted NOT RUN, never qualified silently.
Probe PSS/private/shared RSS/VmPeak/cgroup measurements: NOT RUN; probe MemoryMax/RLIMIT_AS remain unfrozen.
Accepted deviations: systemd PIDFDs prevent delayed PID reuse; ruling 34 preserves existing app accessibility. No new helper syscall allowance in this step.
Remaining: spawner/kill chain, complete degraded supervision, measurements, admission rewiring, full-stack escape/kill witnesses, fuzz and Ubuntu packaging/CI.
Windows supervisor/helper/IPC/job-sys MSVC cross-check passed; shared FFmpeg/media Windows check, final app build and native Ubuntu qualification: NOT RUN.
Target remains below 40 GB. No desktop policy changes, hardware GPU use or push.
1b: LPAC spike, SID/install ACLs, storage denial/quota, Job/CPU/mitigation and handle/IPC canaries, then Windows probe/admission integration; Windows stays in process.
CI: 1.100 changed test paths and field-index linting; dependency-free red/green proofs recorded. All 6 C8 cases and Linux workspace clippy pass on pinned 1.99.1; AU6 iterator fix follows.
