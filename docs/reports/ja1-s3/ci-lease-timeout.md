# Lease timeout witness on Linux CI

The failure was in the test setup. It started 30 fps playback before waiting for its positive frame. The default 1920×1080 document uses a 1280×720 monitor preview. Under CPU contention, lavapipe finishes each preview after its clock target. PF1 R-3 intentionally drops those stale frames. Waiting longer does not deliver the frames already dropped.

[CI run 40948815278](https://github.com/CanadaApollo6/Kinewright/actions/runs/40948815278) fails before the host owner is held or the lease is suspended. Its panic is `FIX1_PLAYBACK_POSITIVE Timeout`, with positions through frame 284. The production preview scheduler already checks `upload_available()`.

Local evidence under `target/ja1-s3-ci-lease/`:

- Unmodified base `b9ed8e8d` passes with lavapipe alone and on one CPU alone.
- With the Vulkan ICD pinned to lavapipe, `LP_NUM_THREADS=8`, affinity CPU 0, and four competing owned CPU processes, the original test reproduces the CI panic through frame 289. `repro-lvp-load.log` exits 101 with `0 passed; 1 failed`.
- The instrumented baseline selects Vulkan `llvmpipe (LLVM 23.1.3, 256 bits)`. `trace-lvp-load.log` records a completed render targeting frame 2 at clock frame 7, followed by `CI_LEASE_LATE target=2 clock=7`. All 45 completed attempts enter the late-frame discard branch. They take 181.5–296.2 ms in this stressed run. At the panic there are 290 due frames and 290 dropped, with zero underruns and zero slot starvation. The tracing code is not included in the commit.
- The corrected test passes the identical contention setup. `fixed-lvp-load.log` exits 0 with `1 passed; 0 failed`. The runner terminates and waits for all four owned competitors on both paths.

The test now obtains and checks frame zero while paused, then waits for the paced playback clock to advance before holding the owner mutex. It retains the same document, raster, 10-second positive-frame deadline, actual status timeout, recovery, exact paused target, Pause cancellation and explicit revocation assertions. Failure diagnostics include `PlaybackStats`. No production behavior changes.

`cargo test -p kinewright-agent --features slow-tests` passes all 22 target summaries, including doc tests: 1,133 passed, 0 failed, 22 declared ignores. Workspace all-target clippy with `-D warnings`, fmt, and slow-test manifest lint pass. The Windows MSVC check stops before changed Rust in native FFmpeg, Whisper and aws-lc dependencies. There is no Windows compile credit. Provider and equipment acceptance are not qualified by this local run.

The CI run was read with `gh run list --branch ja1/s3` and `gh run view 40948815278 --log-failed`. No push was performed. CI for the new commit and native Windows execution are NOT RUN. This local result is ready for the lead's cherry-pick and CI check.
