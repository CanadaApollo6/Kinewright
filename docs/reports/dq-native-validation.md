# Native dialogue wiring validation

The implementation is based on M5, `d4815603` on `origin/dq1`. It was fetched,
fast-forward merged, and built successfully before any native manifest changes.

## Plan as built

- `HostCommandOwner::start_task_server` creates `TaskDialogueMeasurements` from
  the task brief's immutable owned-input manifest and the retained run inputs.
  It rejects identities outside that manifest before engine creation or cache
  lookup. The adapter lazily creates a certified `SessionMedia` and the existing
  `CachedDialogueMeasurements`; it queries the current document without copying
  task undo history.
- Isolated endpoints without admission install a provider returning typed
  `NotMeasured`. A task with no admitted recording returns typed `Identity`.
- Owned sessions select and decode the admitted descriptor, including its
  admitted audio stream. Missing recognition and profiles use the existing
  transcript worker, recognition store, sound-profile store, and resident
  measurement cache. Source paths are labels only on this route. Stored speech
  level and noise floor remain part of the existing bound measurement identity.
- Director and Reviewer receive `get_timeline_transcript` and
  `check_dialogue_cut`. Editor, Motion, Audio, and Colour also receive
  `cut_dialogue`. The native provider allowlist includes all three. Native role
  schemas share the app's actual descriptor constructors.
- Native dialogue calls retain their real product names in task accounting,
  rather than being grouped under `unknown_tool`.
- Existing edit scope, revision, and destructive-edit consent gates still apply.

The main addition to the initial plan was implementing owned-session cache reads:
the previous media adapter required locator authority. Manifest construction also
needed to reuse the app's dialogue descriptors. The requested prerequisite update
installs FFmpeg 9 and updates Linux/MSVC pins and ABI checks together.

## Red and green regressions

| Test | Observed red before production change | Required green outcome |
| --- | --- | --- |
| `dq_native_host_without_admitted_audio_returns_not_measured` | No native provider installed | Exact `Refusal::NotMeasured` for stream and measurement reads |
| `dq_native_task_without_admitted_audio_returns_identity_refusal` | No native provider installed | Exact `Refusal::Identity` for stream and measurement reads |
| `dq_native_task_cannot_measure_a_recording_outside_its_admission` | No native provider installed | A valid foreign recording is inside forwarded path roots and present in the document before endpoint creation; it returns exact `Identity` while the admitted stream remains usable |
| `dq_native_owned_dialogue_measures_without_a_locator_and_reuses_the_profile` | `OwnedMediaOperationUnavailable` | Decode after deleting the locator, real RMS measurements, same stored profile identity, exact authored words, typed stream refusal, and no model download |
| `dq_m5_native_manifests_expose_scoped_dialogue_while_app_catalog_serves_it` | Director lacked `get_timeline_transcript`; subsequent accounting assertion returned `unknown_tool` | Role-specific manifest/schema presence, native-provider allowlist presence, and canonical accounting names |

The first four regressions passed before merging M5. The owned-media regression
also exercises missing-word queueing through a held inference worker, then real
profile-only work after authored words are registered. No real model is loaded
or downloaded by this test.

## Laptop setup

FFmpeg is installed under `third_party/ffmpeg`, with Linux
`n9.0.2-37-gae19868753-20261028` (`9.0-2` mirror) and libavcodec ABI 63.
The Windows pin is MSVC FFmpeg 9.0.2. Rust bindings are `ffmpeg-next` 9.1.0 /
`ffmpeg-sys-next` 9.1.1. Setup archive hashes are checked by the existing script.
The Windows archive was downloaded and checksum-verified without executing it;
its headers, MSVC import library, pkg-config metadata, and ABI 63 DLL are present.

Ninja was missing and the user CMake wrapper was broken. Worktree-local Ninja
and a link to `/usr/bin/cmake` are in the ignored `third_party/tools/bin`.
The CPU NeMo SDK was provisioned into the ignored `third_party/nemo-speech`.
These verification shells therefore use the following prelude, followed by the
required Cargo lock/jobs/nice wrapper for each Cargo command:

```bash
export PATH="$PWD/third_party/tools/bin:$PATH"
export KINEWRIGHT_NEMO_CACHE="$PWD/third_party/nemo-speech"
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
source scripts/setup-ffmpeg.sh
```

No system packages, sudo, Rust toolchain update, installed provider sessions, or
hardware GPU were used. Python ran with `nice -n 19 python3 -I`.

## Gate results

All final gates below passed on the merged implementation. Cargo used the
required lock, six build jobs, nice level 10, and sourced FFmpeg environment.
Final Cargo tests ran offline; software Vulkan used lavapipe.

| Gate | Result |
| --- | --- |
| Build after the M5 merge, media and agent | Passed |
| `cargo fmt -- --check` | Passed |
| Media/agent all-target clippy, slow features plus `te1-eval`, `-D warnings` | Passed |
| Media crate, `slow-tests` | 1,352 passed; 45 ignored; zero failures, including integration and doc tests |
| Agent crate, `slow-tests` | 1,843 passed; 105 ignored; zero failures, including integration and doc tests |
| Explicit agent `--test mcp_server` | 86 passed; zero failures |
| Measurement pin verification | One matched test passed with clean Cargo exit |
| Pin drift naming every leaf | One matched test passed with clean Cargo exit |
| Focused native admission/provider regressions | Three matched tests passed |
| M5 role presence, schema, allowlist and accounting regression | One matched test passed |
| Slow-test manifest lint | 140 markers agree; allowlist well-formed |
| Setup shell syntax and archive cache keys | Passed |

The full `te1_measurement` test binary also reported all 13 scripted/pin cases
passing. Its enclosing gate shell ended with SIGTERM before starting the crate
suites; the two pin checks were therefore rerun separately with clean exits.
The complete media and agent crate gates above were run afterward and exited
successfully. Zero-test feature-gated targets are not counted as passing tests.

Relative to M5, compact UTF-8 tool JSON grows by 5,936 bytes for each of Editor,
Motion, Audio, and Colour, and by 3,166 bytes for each of Director and Reviewer.
Application-prefix byte lengths are unchanged; the embedded manifest hashes
change. The combined six-role tool JSON increase is 30,076 bytes. The app's
existing dialogue descriptor text is reused unchanged. Both native manifest
hashes and the measurement pin are deliberately regenerated.
Four frozen wake fixtures were also regenerated: a structural comparison found
only `manifest_sha256` changed, with identical payloads and total byte lengths.
The prefix and full-prompt hashes were updated; the wake-size golden is unchanged.

The initial pre-wiring agent slow run had 1,449 passes, 66 ignored tests, and one
existing 500-second `history_capacity_full_lifecycle_crash_reopen` child timeout.
The final runs use `RUST_TEST_THREADS=2` to reduce runtime contention. The timeout
is retained here rather than being counted as a successful gate; that lifecycle
test passed in both later full agent runs. The first merged full run exposed
three expected prompt/prefix golden failures. After the digest-only refresh,
the clean full agent rerun passed every non-ignored case.

The Linux-only test inspection helper is gated with both `test` and Linux,
matching its Linux-only callers. Production measurement helpers and their
callers are platform-generic; no Windows-only dead-code allowance was added.

Final `target/` size: **13 GiB** (`du -sh`), below the 40 GiB ceiling. Available
disk: approximately **774 GiB**, above the 6 GiB floor. Failed, uninstalled NeMo
build trees were pruned; the installed SDK and archive caches remain ignored.

## Handoff

No known implementation issue remains. Full-workspace and Windows CI are still
the lead's push gates; neither was represented as having run on this laptop.
The work is committed locally with `[slow-tier]` and the Codex co-author trailer.
