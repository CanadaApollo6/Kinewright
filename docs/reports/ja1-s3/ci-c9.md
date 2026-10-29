# C9 Linux CI fixtures

The Linux workspace jobs in [40964727628](https://github.com/CanadaApollo6/Kinewright/actions/runs/40964727628/job/123766182067) and [40955816162](https://github.com/CanadaApollo6/Kinewright/actions/runs/40955816162/job/123735515665) fail C9's clock assertion. Both logs report ALSA `cannot find card '0'` and `Unknown PCM default`. Frame zero already passed its exact snapshot check. The test requested a default audio device, so playback never started its audio clock. This differs from the earlier lease witness, whose clock advanced while late renders were dropped.

C9 now uses the existing paced test audio with a fixture-owned cache directory. It retains the exact frame-zero snapshot check, real audio-consumption clock advancement through frame 3, and host reaping. Terminal Studio errors fail immediately with position, stamp, playback statistics, Studio failure, and app status. Its retirement guard is declared before the owners it waits for, so panic cleanup drops those owners first.

The default GPU is cached in a process-wide `OnceLock`. Cloning its context also shares its allocation ledger. Four other tests therefore failed their zero-allocation teardown assertion while C9 still owned GPU resources. Each `in1_harness` engine now receives a test-only view with its own ledger and the same cached device, queue, and provenance. C9 uses that view too. `Retire` still asserts zero with no subtraction or allowance. A media unit test keeps allocations live in another view while checking isolated retirement.

The older run also reports `FIX1_ACK history_changed: retry the save`. The transcript reopen helper now drains its in-flight saved-note status reply before the next recording captures an upload generation. The status request can start transcription and publish metadata. The byte-preservation and stale-owner assertions remain unchanged. The exact CI interleaving was not traced. Receipt-aware helper retry, a controlled publication witness, stale-save retry UX, and the Linux missing-output disclosure remain in deferred.md.

Implemented on `bc0aed38` without changing that commit. Every Cargo gate sourced `scripts/setup-ffmpeg.sh` and ran in the foreground through `flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo ...`. `LOCALAPPDATA` and `TMPDIR` were under this worktree's `target/ci-c9-fix/`. The ordinary suites used `RUST_TEST_THREADS=4`. No second target directory was created. Disk had about 21 GiB free before the gates and 20 GiB afterward.

The gates passed with these results.

- `cargo fmt -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` both exited 0.
- `cargo test -p kinewright-app --features slow-tests` reported `959 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out`. This includes the transcript-preservation witness. The package's test target is a binary, not a library.
- The five original CI-failing Studio tests ran together once with `--test-threads=8`, `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`, and `LP_NUM_THREADS=8`. They reported `5 passed; 0 failed; 0 ignored; 0 measured; 958 filtered out`.
- `cargo test -p kinewright-media --lib` reported `1197 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out`. The isolated-ledger allocation test passed. No segfault or two-thread fallback occurred.
- `python3 scripts/slow_tests.py lint` passed with 118 manifest tests, 61 well-formed allowlist entries, and 27 equipped-session skip sites.

The raw test summaries were:

```text
test result: ok. 959 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 237.86s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 958 filtered out; finished in 6.15s
test result: ok. 1197 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 343.61s
```

Logs are retained, compressed, under `target/ci-c9-fix/`, beside a source hash manifest. Hosted CI, Windows MSVC compilation and execution, the workspace fast suite, and the media slow tier for this commit are NOT RUN. This commit is local and is not a stage closure.
