# TE1 safety

Fixes 1 and 2 certify frozen run inputs outside the owner mutex for StartRun,
Revise cut, and native recovery. The final locked bind validates the run scope
and rechecks the project, document hash and reference, manifest, owner epoch, and
certified descriptor metadata. A fork can have a new head while retaining the
exact same certified input data. A changed source refuses execution. Decoder
cursors still verify every used chunk.

Shared-owner direct callers use `execute_attached_shared`, so certification runs
without their mutex guard. The unshared direct API remains available. It refuses
new run preparation when called through a registered shared owner. Revise replays
do not certify footage again.

Task calls check live authority while waiting for the initial owner lock and
the final lock after a media read. Reply metadata, refusal accounting, and size
accounting do not wait behind a held owner lock after Stop. Edit-consent waits
retain their existing 350 ms ceiling and also check stopped authority.

## Measurements

The same generated RF64 file contains 6,528,000,114 bytes. Each measured path
clears the media admission memo first, so these are cold certificate
measurements with OS-cached footage after import. A sampler tries the owner
mutex every 100 microseconds. The numbers are the longest observed blocked
intervals, including scheduling delay,
not exact mutex instrumentation or an isolated-machine speedup claim.

| Path | Before | After |
| --- | ---: | ---: |
| StartRun owner hold | 10.917966 s | 0.364930 s |
| restore_native_run owner hold | 9.755663 s | 0.005151 s |
| Complete media reply | 11.085252 s | 7.627047 s |

The baseline media call spent 10.637631 s certifying the source, 0.015960 s
opening the audio decoder, 0.000055 s seeking and reading the frame, 0.002435 s
assembling the sample and page, 0.004888 s authorizing and preparing the read,
0.004226 s reacquiring and authorizing the final lock, 0.392863 s publishing the
page, and 0.010456 s refreshing reply metadata. The remaining 0.016738 s includes
polling, role lookup, serialization, and transport. The reported historical 44 s
was not reproduced on this fixture at the pinned base.

The shared host had unrelated CPU load during both runs. Baseline binary SHA-256
was `cccac6c4f035832329ec492dfe849ddba401c4525563c77e15596136e76283b8`.
The measured fixed binary was
`7c195f687eacd6475474b3f65ff040d2290290eb79ddb615d91fe082bd56740f`.
Logs are under `target/te1-safety/`, in `baseline.log` and `after-final.log`.
The generated RF64 file was deleted after the final measurement.

## Fix 3 requires a decision

No Claude first-response guarantee is implemented or claimed. The existing
30-second media wait does not bound role lookup, owner-lock waits, publication,
reply metadata, or other tools. It also cancels decode at the wait limit, so a
repeatably slow warm decode can retry without progress.

A blanket deadline requires a pending-result contract for mutating tools. A
command could still reserve sends or finish an edit after the model receives
"still working". The brief requires a stop before choosing that behavior.

- An all-tool pending-result cache could answer at 40 s and retain one exact
  request and reply per endpoint. This leaves a proposed 5 s delivery margin.
  Commands continue after that answer. At most one pending operation runs per
  endpoint. An operation taking D seconds needs roughly ceil(D / 40) calls.
- A media-only pending-result cache could retain decoded pages for warm retries
  and keep the existing 30 s media wait. It would preserve command behavior but
  would leave the blanket 45 s guarantee unmet.

These are design estimates, not implemented or measured deadline results.
No paid run, provider launch, or credential read was performed.

## Verification

Removing the source recheck under the lock fails the source-change witness at
`TE1_SAFETY_RECHECK: changed input started a run`. Restoring it passes. The Stop
witnesses cover the initial and final owner-lock waits, hold the owner mutex
through Stop and the refusal, and check that no evidence decision is added.

Media passed with 1,267 tests and 32 ignored across unit, integration, and doctests.
Its only later edit moved a Unix import before a statement for Clippy.
Project passed with 190 tests and 2 ignored after building the required sealed
predecessor. Agent passed with 1,680 tests and 92 ignored across the full crate
suite, with `slow-tests` enabled and `RUST_TEST_THREADS=1`.

The earlier four-thread Agent library run reported 1,293 passed, 1 failed,
and 56 ignored. Its existing 45-second counting-child timeout failed under
concurrent fixture load; the same test passed alone in 29.45 seconds and then
passed in the full single-thread run. Its timeout and assertions are unchanged.
The additional `te1-eval` Stop checks passed both tests, including optional
observation while the owner mutex remains held after Stop.

All logs below were read after their commands ended. Cargo used the required
FFmpeg environment, heavy-job lock, four build jobs, and niceness 10. Media and
the final Agent suite used one test thread; Project used four.

| Gate | Result | Log under `target/te1-safety/` |
| --- | --- | --- |
| `cargo fmt -- --check` | PASS | `workspace-fmt.log` |
| Workspace Clippy, all targets, `-D warnings` | PASS | `clippy.log` |
| Supplied Windows Clippy script | PASS: workspace and Agent `slow-tests`; compile/lint only | `windows-clippy.log` |
| Slow-test lint | PASS: 121 tests, 144 allowlist entries, 27 skip sites | `slow-lint.log` |
| Slow-test lint self-tests | PASS: 44 tests; existing unclosed-file ResourceWarning | `slow-selftest.log` |
| Media full crate suite, `slow-tests` | PASS: 1,267 passed, 32 ignored | `media-tests.log` |
| Project full crate suite | PASS: 190 passed, 2 ignored | `project-tests.log` |
| Agent full crate suite, `slow-tests` | PASS: 1,680 passed, 92 ignored | `agent-tests.log` |
| Stop checks with `te1-eval` | PASS: 2 passed | `eval-stop-witness.log` |
| Locked source-recheck mutation | Intended red, restored green | `mutation-red.log`, `mutation-green.log`, `mutation-summary.log` |
| Final same-fixture timing | PASS: 1 passed | `after-final.log` |
| Generated-footage cleanup | PASS: RF64 source deleted | `cleanup.log` |

The maximal fixture is excluded as requested.
The free recorded-page replay path and its earlier 4.22 s measurement are left
unchanged. The existing scale witness now calls the prepared recovery boundary.

## Files touched

- `ci/ignored-tests.txt`.
- `crates/kinewright-agent/src/orchestration/effect_pump.rs`.
- `crates/kinewright-agent/src/orchestration/tasks.rs`.
- `crates/kinewright-agent/src/writer_host/auth.rs`.
- `crates/kinewright-agent/src/writer_host/completion_fixture.rs`.
- `crates/kinewright-agent/src/writer_host/continuation.rs`.
- `crates/kinewright-agent/src/writer_host/hooks_tests.rs`.
- `crates/kinewright-agent/src/writer_host/ja_s3_host.rs`.
- `crates/kinewright-agent/src/writer_host/mod.rs`.
- `crates/kinewright-agent/src/writer_host/orchestration.rs`.
- `crates/kinewright-agent/src/writer_host/provider_readiness.rs`.
- `crates/kinewright-agent/src/writer_host/run_inputs.rs`.
- `crates/kinewright-agent/src/writer_host/task_consent.rs`.
- `crates/kinewright-agent/src/writer_host/task_mcp.rs`.
- `crates/kinewright-agent/src/writer_host/task_mcp/reply.rs`.
- `crates/kinewright-agent/src/writer_host/te14_fix2_tests.rs`.
- `crates/kinewright-agent/src/writer_host/te1_safety_tests.rs`.
- `crates/kinewright-media/src/owned_io.rs`.
- `crates/kinewright-media/src/owned_io_memo.rs`.
- `crates/kinewright-project/src/owned_inputs.rs`.
- `crates/kinewright-project/src/owned_media.rs`.
- `crates/kinewright-project/src/studio_store.rs`.
- `docs/reports/te1-safety.md`.
