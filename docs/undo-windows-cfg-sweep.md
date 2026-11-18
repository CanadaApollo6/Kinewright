# Windows history API sweep

Base: `dfe5e4d7`. The follow-up covers the four crates changed by the switch:
core, project, agent and app. It changes test construction and helper gates.
It changes no product behavior, bounds or timeouts.

## Corrected sites

| File and item | Correction |
| --- | --- |
| `tests/ja_s2_switch.rs::native_windows_switch_admission_refuses_before_mutation` | Make a local pool and pass it to `ProjectBundle::activate`; there is no actor in this refusal fixture. |
| `tests/ja_s3_read_only_inputs.rs::s3_read_only_windows_certification_keeps_write_refusal_zero_mutation` | Make and pass a local pool. Require `DurabilityUnsupported`, preserving certified read access and the unchanged directory. |
| `tests/ja_s2_rescue_record.rs::w14_raw_switch_structure_and_ordinary_v3_backward_control` | Pass a local pool in the unsupported-platform activation block. |
| `writer_host/mod.rs` | Gate `document_for_test`, `received_reply_for_test`, `received_snapshot_for_test`, `host_snapshot_for_test` and `locked_host_snapshot_for_test` with `all(test, target_os = "linux")`, matching every caller. |
| `app.rs::ja_s2h_legacy_activation_witness` | Use its caller's Linux test gate and remove the dead-code allowance. |

The project paths above are relative to `crates/kinewright-project`.
The writer path is under `crates/kinewright-agent/src`, and the app path is
under `crates/kinewright-app/src`.

## Sweep boundaries

Discovery included `cfg` and `cfg_attr`, positive and negated `windows` and
`unix`, target OS/family/environment/architecture conditions, and `cfg!`.
It found 1,429 platform attributes in 199 Rust files, including 223 non-Linux
or mixed conditions. Those are discovery counts, not independent tests.

The API review covered pool and actor construction, `ProjectBundle::activate`,
`LoadedBundle::read`, owner open, snapshot access and the leased operation
readers. A constructor scan covered 518 call sites. Old kernel constructors in
the remaining comparison proofs and measurements were distinguished from the
new actor. Their removal remains the final H2b stage.

The unsupported-profile paths in storage, Save, switch, rescue, source ownership,
read-only inputs, headless admission and retained work keep their original
zero-mutation refusals. App Save and the portable host protocol paths retain
their new actor and combined snapshot APIs. Native namespace, process and
capture-backend branches do not call the changed history constructors.

The helper review included 80 declarations marked `cfg(test)` and the callers
of the shared writer helpers. The writer callers are in Linux-only hook, host,
snapshot-lease, voice and switch tests. Portable checkpoint codecs, identities,
in-memory session models and unsupported-platform refusal tests stay portable.

The scan also found old APIs in the Linux-only `te1-eval` feature bridge:
construction, candidate checkpoint reading and reopen verification. That reader
belongs to the resumed reader/door audit, separately from this Windows fix.

## Verification

Fmt and workspace clippy with `--all-targets -- -D warnings` passed. The core,
project, agent and app slow suites passed 4,373 tests across 81 targets, with
zero failures and 116 existing skips. All 64 long session-model seeds passed.
The three separate shipping builds and slow-test lint passed.

A compile-only probe enabled the three hidden refusal bodies in temporary Linux
views of the original test modules. `cargo check` passed all three targets
against the same public APIs. The probe never ran those Windows assertions on
Linux, and its temporary targets were removed before commit.

The installed MSVC target's `cargo check --tests --target x86_64-pc-windows-msvc`
was attempted. It stopped in FFmpeg, Whisper and AWS-LC native dependencies,
including missing `errno.h` and `stdio.h`, before project-test compilation.
This Linux environment lacks the Windows SDK/C toolchain. Neither source
inspection nor the portable-body probe qualifies native Windows compilation.

Every Cargo command used FFmpeg setup, the requested heavy lock, four build
jobs and `nice -n 10`. Logs and discovery lists are under
`target/hx-windows-sweep/`. All started runs have finished.
