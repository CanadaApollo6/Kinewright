# MS2 slice 1a
Status: IN PROGRESS; confinement/supervision/measurements qualified; Linux IX admission awaits rewiring.
Commits: `4a55b8d4` plan; `79d93705` IPC; `2d926a59` helper; `7afcde61` supervisor; `ef8195b2` polling; `6a11af0f` CI pin; `9edbcf98` AU6; `7954fac6` spawner; `dacf0be6` degraded; measurements in this commit; `90741cf4`/`ac1c60ce` historical stops.
FFmpeg: base setup unchanged; libavcodec 62.11.100 SHA256 `daf171e4d0648d5483f94a0d1fb97fd42c1b99c5f8ce990865235f6e0b8831fa`; MS1 untouched; app/helper share owned_io for later MS1 policy.
Ruling 32: exact madvise 0/4/8/102/103; native guards succeed; other values/high-bit aliases die with SIGSYS, controls survive.
Ruling 33: affinity arg0 exactly0; self-query positive, explicit PID/highbits killed. Added mremap old_size>0 flags exactly0/1: native shrink/grow preserves bytes; FIXED/DONTUNMAP/duplication/highbits killed; cited design rows.
Native final filter: running-worker/C-pthread join, anonymous mmap, clone3 ENOSYS, decoder close/cancel, AVC/PNG/JPEG/UHD AV1/WAV/MP3 and unchanged source OFD offset pass; arbitrary colour tags refused, finite existing identifiers retained.
IPC strict framing/typed state/limits/explicit-stream binding pass; libFuzzer NOT RUN. Landlock read canary EACCES with readable control; x32/foreign-arch/degraded pthread attempts SIGSYS.
Lifecycle: 17 watchdog-disabled app/supervisor/spawner/PID-0 race/cancel/deadline/revocation cases; four kernel/guard removals plus stream-binding removal fail intended assertions, restored controls pass.
Degraded: five release control/burst pairs with watchdog on; 128 MiB test budget overshoots by at least 182.8–200.4 MiB (largest-child peak); not a hard RSS guarantee. RSS-kill/quarantine removals fail; restored controls pass.
Ruling 34: CI gate + 7 self-tests and real-app direct-zbus red; one zbus 5.19.0; app accessibility unchanged. Supervisor separation/limits/native-empty witnesses pass.
## Probe memory measurements
Five interleaved release runs/fixture before limits, then five each at selected limits; 40 successes, zero errors; live frames/decoder retained through sample. MiB maxima below; RSS/private/shared sum shim+payload (shared double-counted).
| Fixture | PSS | RSS | Private | Shared | Payload VmPeak | Largest-child peak RSS | cgroup current / peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1080p H.264 8-bit | 44.34 | 60.08 | 33.05 | 27.12 | 295.66 | 37.84 | 18.95 / 18.95 |
| UHD AV1 10-bit | 54.20 | 69.74 | 43.09 | 26.67 | 295.66 | 47.67 | 29.61 / 29.75 |
| UHD RGB48 PNG | 74.57 | 90.05 | 63.35 | 26.71 | 295.66 | 69.04 | 51.08 / 52.28 |
| UHD JPEG | 50.33 | 65.81 | 39.11 | 26.77 | 295.66 | 44.43 | 26.57 / 27.23 |
MemoryMax=1,024 MiB, RLIMIT_AS=1,536 MiB, allocation=896 MiB: initial unknown-format UHD AV1 DPB guard needs836 MiB; 448 MiB trial refused ResourceLimit; 128 MiB runtime headroom. Provisional 128 MiB probe target replaced; not every codec/DPB qualified.
All cgroup oom/oom_kill counters zero. PSS differs from cgroup charge (pre-join pages); status RSS/HWM approximate, child peak is not a tree sum. Raw counters/hashes/commands/failures: `ms2-s1a-evidence/probe-measurements.json` and archive.
Step6 gates: 47 touched slow-feature tests, zero failures/skips; default/slow all-target clippy -D warnings, fmt, manifest130/parser45/boundary7 and MSVC cross-check pass. Final app/native Ubuntu/native Windows NOT RUN.
Accepted deviations: inherited shim pidfd closes PID-0 race; systemd PIDFDs prevent delayed reuse; ruling34 retains accessibility; measured probe reservation above. Remaining: admission, full-stack escape, fuzz, Ubuntu packaging/CI.
Every started scope stopped; native inventory zero. Target <40 GB; no desktop policy changes, hardware GPU or push. Earlier evidence: spawner/degraded/supervisor gate JSON and archives.
1b: LPAC spike, SID/install ACLs, storage denial/quota, Job/CPU/mitigations, handle/IPC canaries, then Windows admission; stays in process. CI regressions: Cargo/Clippy1.100 isolated; 6 C8 pass1.99.1; Linux workspace clippy1.100 and 1,267 media slow tests pass, 32 existing opt-ins ignored.
