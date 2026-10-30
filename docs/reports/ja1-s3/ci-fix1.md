# JA-1 S3 CI fix 1

Base: `58e470951d1bd4aed7dc80653d6b36063b6e2bfd`, branch `ja1/s3-c2fix`.
Both changes repair fixtures. Production incident policy and media admission stay unchanged.

## Colour witness

The fixture selected `log.all().next()`, which can be the transcript panel's
`panel_worker_error`, classified `Explain`. Its message is
`Transcript: owned_media_operation_unavailable: this operation has no certified source route`.
The owned asset's separate `unknown_source_primaries` incident is still `AutoApply`.

The import poll checks transcript status before draining Core's new document.
If another poll occurs after the imported asset appears, the transcript incident
opens first. One explicit background tick after import reproduces the CI assertion
locally: `left: Explain`, `right: AutoApply`. Unsetting `LOCALAPPDATA` and the XDG
variables, using a durable temporary directory, and selecting software Vulkan did
not reproduce it alone. The full app binary also passed before that extra tick.

The witness now includes the extra production tick and selects its incident by
code and asset subject. Both colour witnesses pass. Removing each production
guard separately still fails at its `c0-colour-auto` or `c0-colour-card` history
assertion; restoring the guards passes both tests.

## Slow-tier classification

Compared the failed logs at [a1450e60, run 40695943213](https://github.com/CanadaApollo6/Kinewright/actions/runs/40695943213),
the current [58e47095 Linux job](https://github.com/CanadaApollo6/Kinewright/actions/runs/40714906032/job/122909826495)
and [Windows job](https://github.com/CanadaApollo6/Kinewright/actions/runs/40714906032/job/122909826088),
and the green [main b2b8630d run 40551797815](https://github.com/CanadaApollo6/Kinewright/actions/runs/40551797815).
The classifications below use those completed logs. A fresh main CI run was also
[dispatched as 40715513418](https://github.com/CanadaApollo6/Kinewright/actions/runs/40715513418)
at `0b353397845c52fba9c34bfd64700d381ce721cb`.
Both fresh-main slow jobs passed, and the whole main CI run is green.
Its Linux slow job passed R28 and reproduced all 17 identical kernel
probe `NOT_RUN` markers. Both the archived comparison and fresh-main comparison
therefore identify the guardian limitation as independent of S3.

| Test / observation | Group | One line of evidence |
| --- | --- | --- |
| `r28_solo_peak_resources_and_elapsed`, Linux | (a), fixed | Both branch runs panic at `mcp_server.rs:11250` with `MediaAdmissionRequired`; b2b8630d passes. |
| `r28_solo_peak_resources_and_elapsed`, Windows | (a), fixed | The current Windows slow job has the same `MediaAdmissionRequired` panic; b2b8630d passes. |
| Linux `s4_c2_*` kernel probes, 17 tests | (b), environment limitation | All 17 emit the identical `kernel_deletion_signal_unavailable: live_nlink=2, deleted_nlink=2` marker on both b2b8630d and 58e47095. |
| `g05l_host_death_recovers_absent_and_committed_drag_suffix`, Linux | No failure to classify | `test g05l_host_death_recovers_absent_and_committed_drag_suffix ... ok` in both branch slow jobs. |
| `g05l_shared_floor_and_recovery_close_reply_drop`, Linux | No failure to classify | `test g05l_shared_floor_and_recovery_close_reply_drop ... ok` in both branch slow jobs. |
| `s4_cancel_tree_and_retire_once`, Windows | No failure to classify | `test s4_cancel_tree_and_retire_once ... ok` in both branch Windows slow jobs. |
| `au4_plan_audio_ducking_converges_through_the_real_engine`, Windows | No failure to classify | `... ok`; `AU4_DUCK ... delta=-1200 budget=50` in both branch Windows slow jobs. |
| `au5_the_repair_and_normalization_planners_agree_in_either_order`, Windows | No failure to classify | `... ok`; errors 1 and 39 against tolerance 100 in both branch Windows slow jobs. |

The guardian markers report native work as `NOT_RUN`; their libtest entries are
`ok`, not failed assertions. They already occur in green main. They do not count
as native containment acceptance. None of these observations is a documented (c)
flake in `.review/PITFALLS.md` or `ci/`.

R28 used the raw engine constructor after S3 made source authority explicit.
The fixture now calls `into_legacy_fixture()` before probing and rendering. Its
local slow run passes both resolution modes on its existing fallback adapter
without changing its assertions.

## Verification

All Cargo commands sourced `scripts/setup-ffmpeg.sh` and ran in the foreground
under `flock -o /tmp/kinewright-cargo-heavy.lock`, `CARGO_BUILD_JOBS=4`,
`nice -n 10`, and a 600-second timeout. Tests used `RUST_TEST_THREADS=4`, with
the additional media run explicitly using one test thread.

| Check | Result |
| --- | --- |
| `cargo build --workspace` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo fmt -- --check` | PASS |
| Full app fast suite, final source, in workspace runs | `902 passed; 0 failed; 6 ignored`, twice |
| Full agent fast suite, final source, in workspace runs | Lib: `761 passed; 0 failed; 2 ignored`; every binary and integration target completed successfully, including MCP: `76 passed; 0 failed; 10 ignored` |
| `cargo test --workspace --doc` | PASS, including the agent doctest |
| Full project fast suite, run separately | Lib: `158 passed; 0 failed; 2 ignored`; all integration targets passed |
| Owned colour witnesses | `2 passed; 0 failed`; both guard removals failed at their named history assertion, restored final source passed both again |
| R28 slow test, agent/media `slow-tests` enabled | `1 passed; 0 failed` |
| `python3 scripts/slow_tests.py lint` | PASS: 117 manifest tests, 60 allowlist entries, 27 equipped-session skip sites |
| Broad `python3 scripts/slow_tests.py fast` | PARTIAL: both attempts reached 600 seconds in the unchanged media crate; no assertion failure was reported. The completed agent, app, core, hash and job-sys sections have 2,813 outcomes and parse successfully with the existing output parser. |
| Additional serial media suite | PARTIAL: reached 600 seconds in unchanged PF1 stress witnesses; its extra retry was stopped when returning to the brief's changed-crate scope. No full media or fast-tier inventory acceptance is claimed. |
| Windows touched-crate cross-check | NOT RUN past native prerequisites: FFmpeg bindings lack `errno.h`; Whisper lacks `stdio.h` and a Visual Studio generator. Native Windows execution of this fix is NOT RUN. |

The first workspace attempt encountered a missing sealed max-2 control. Subsequent
runs reused an existing sealed predecessor read-only after verifying both executable
hashes and its clean pinned `076a96c9` source. Raw local results are retained under
`.review/ci-fix1/`; no selector list or partial media output is counted as a pass.
