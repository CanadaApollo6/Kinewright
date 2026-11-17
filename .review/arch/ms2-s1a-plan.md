# MS2 slice 1a execution plan

Base: `9aef3952b76095334e144ebf7ff7513e52d34c3c`, branch `ms2`.
Done means Linux IX video/still admission crosses a source-bound confined
probe, with measured limits and passing native escape/lifecycle witnesses.
Windows retains its explicitly cfg-gated in-process probe until slice 1b.

1. IPC crate. Fixed bounded framing, strict typed probe/lifecycle messages,
   duplicate/depth rejection, private validated metadata, state/identity checks.
   Test malformed frames, forged metadata, sequencing, floods and fuzz corpus.
2. Helper and job-sys boundary. Minimal FFmpeg-only probe code, descriptor AVIO,
   first frame/still inspection, allocation sub-budget, blocked shim, namespace
   bootstrap, fd inventory, empty root, exact seccomp table, optional Landlock.
   Test each syscall predicate alone, final-filter join/close/cancel, and attacks.
3. Supervisor binary. Blocking zbus only here, private typed socketpair,
   PID/starttime/parent checks, bounded scope records, verified transient controls.
   Test scope limits, blocked join, supervisor separation and native emptiness.
4. Spawner and kill chain. Dedicated non-returning abort-guarded thread,
   parent-death edges plus pidfd race checks, reservations, bounded queue,
   deadlines/cancellation/revocation and supervisor-loss recovery.
   Test watchdog-disabled app/supervisor/thread loss before ready and in decode.
5. Degraded mode. Fixed watchdog, TSYNC, no later clone, single decode thread,
   address-space/CPU limits, RSS polling and retained cleanup ownership.
   Repeat lifecycle/escape tests and measure RSS overshoot.
6. Measure probe PSS, private/shared RSS, peak address space and cgroup metrics
   for 1080p, UHD and stills; record fixtures/method before freezing limits.
7. Admission rewiring. CertifiedSource and all Linux import/project probes;
   host retains byte identity, pins, authority and checked domain construction.
   Test IX video/still success, malformed refusal, no host decode/fallback.
8. Qualification. Full-stack escape/kill-chain and parser/state fuzz witnesses,
   packaged AppArmor profile and Ubuntu CI installation/missing-profile gate.
   Record dependency graph and remaining native Ubuntu/Windows boundaries.

Each implementation step is one local commit after touched-file formatting,
touched-crate all-target clippy (-D warnings), full touched-crate slow tests,
and slow-test manifest lint. New markers also run slow_tests_selftest.py.
Cargo uses setup-ffmpeg.sh and the prescribed flock/jobs/nice command.
No GPU tests, sudo, host policy changes, credentials or pushes.
Final gate builds kinewright-app. Stop and report evidence for an unbuildable
design point rather than silently altering the approved boundary.
Report: docs/reports/ms2-s1a.md (at most 25 lines).
