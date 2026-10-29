# JA-1 S3 C6b engine follow-ups

Branch `ja1/s3-c6b` starts at `74b220012ec9a91a3eaff627254352fc8f89f357` from `origin/ja1/s3`. No push. Studio component files, C6a UI work, and earlier report artifacts remain outside this change.

## Items

| Item | Result and witness |
| --- | --- |
| C2-R4-M2 | Fixed. Thumbnail keys include source fps. Transcript v3 keys include fps, duration, and the requested language, including automatic detection. Model identity is still checked on load. `c6b_thumbnail_cache_misses_when_fps_changes` and `c6b_transcript_cache_requires_the_requested_recipe` require misses for equal source bytes with different recipes. The cache-inventory integration fixture now expects v3. |
| C5F3-m1 | Fixed. Slot admission evicts slots without a job, pending write, pending ASR, live handle, or concurrent user. `c6b_refused_voice_requests_do_not_exhaust_slots` makes eight refusals with a one-slot limit, then publishes a valid note. The strengthened `s3_voice_fix2_any_handle_retries_refused_choice` drops both handles and triggers an eviction sweep before collection and retry. `VoicePublication` and the slot remain the sole SaveVoice publication path. |
| C1-M9 | Already fixed in the parent. `ReadPin::same_reference` checks owner identity and roots before retention, including at the 64-slot limit. Existing `s3_followup_repeated_reads_dedupe_reference_pins_at_capacity` checks 70 reads of each reference through direct, HTTP, and MCP ingress. |
| C1-M3 | Fixed. `validate_run_update` checks the current, nonretired issuer sequence before staging. Commit consumes its admitted records and result through the existing metadata publisher while rechecking the envelope, generation, issuer, and receipt. `c6b_run_update_checks_sequence_and_consumes_admission` checks refusal before writes, no second admission, and exact replay. |
| C1-M4 | Already fixed in the parent. Metadata issuers retain their attachment subject and host ingress checks it before execution and replay. Existing `h3_metadata_subject_ordinary_command_refused_before_replay` and `h3_metadata_subject_refused_before_publish_and_replay` cover the ordinary and hook paths. |
| C1F1-M1 | Already fixed in the parent. `VoiceRef::matches_note` compares `duration_micros`. Existing `s3_late_asr_preserves_audio_only_feedback_and_note_duration` rejects a revision with a mismatched duration. |
| Orphaned hosts | Fixed in the App fixtures. `HostAttachment` had no fixture Drop, so dropping a fixture discarded its live `Child`. Test-only Drop now kills and reaps the fixture-owned host on ordinary return and unwind. `c6b_dropping_app_fixture_reaps_its_host` checks a real host PID disappears. This cause is separate from `child_process::run_to_completion`; PO0 process groups and Windows job objects are not changed here. Production continuation ownership stays with the existing close/detach path. |
| C2-R4-M3 | Fixed fixture teardown. `in1b_app` waits for its probe engine to finish. The incident test drops the App and waits for its media worker, which joins the preview thread. The isolated test passes its assertions and exits zero. Exact-parent GDB evidence captured SIGSEGV in the preview thread's NVIDIA graphics-pipeline creation while the main thread was in NVIDIA/GLX teardown and the media worker was joining preview. |
| `rev3_concurrent_readbacks_never_refuse` | Fixed the probe. Successful readback and upload-batch availability are separate completions. The probe now maintains and waits for upload availability before each render, and records the actual error for any refusal. `c6b_completed_readbacks_can_still_wait_for_upload_callbacks` withholds real completion callbacks after two successful readbacks, observes legitimate `upload ring exhausted` backpressure, delivers the callbacks, and renders again. The compositor product code is unchanged. |
| C2-R4-M1 | Fixed shared-worker starvation. Admission reserves result capacity across queued, running, and delivered jobs for each authority. The worker sends without blocking under the same accounting lock. A full authority refuses new requests before acceptance, so the UI can retry without losing accepted results. `c6b_full_inactive_visual_channel_preserves_active_progress` fills A's registry-held channel, observes B's progress, then drains and retries A. Closed-session registry retirement remains an App lifecycle follow-up; this change preserves authority-keyed retention. |
| C2-R3-M2 | Fixed the late-admission inference case. The actual Whisper abort callback captures its original legacy grant and aborts on revocation as well as explicit cancellation. `c6b_inference_admitted_after_cancellation_aborts_on_revocation` registers a token after the cancellation sweep, enters a real work lease on another thread, revokes, and checks abort plus quiescence. No new media authority or locator bypass is introduced. |
| C1-M7 | Partial. Fixed inexpensive shape checks. `RunView::validate` rejects duplicate actions, empty child waits in run and task projections, and blank control revisions. A typed opaque revision is deferred to the S4 public handoff schema; this change retains the existing bounded-string contract. `c6b_projection_rejects_ambiguous_actions_and_empty_children` checks invalid and valid controls. |
| C1F1-M2 | Already fixed in the parent. `verify_projection_payloads` requires every projected root in the projected manifest. Existing `s3_indexes_survive_all_root_producers` invokes the manifest-omission oracle for the root references. |
| C1F2-D1 | Wording already corrected in `c1.md` to "The lead confirmed". The fix2 binding records that correction, but its report hash differs from today's cumulative report. Refreshing that historical binding is deferred to C6a's report work; the evidence file is outside this brief's edit scope. |
| C0-M4 | Already corrected in both C0 lost-reply audit rows. Retry re-executes admission and returns an equivalent asset and manifest under a fresh same-cut lease. |

## Verification

All native commands source the pinned FFmpeg 8 setup in the same shell. The local wrapper checks at least 6 GiB available memory, takes `flock -o /tmp/kinewright-cargo-heavy.lock`, sets four Cargo jobs, invokes `nice -n 10`, and gives each invocation 600 seconds. `LOCALAPPDATA` and `TMPDIR` use this worktree's owned durable `target/` paths. Python runs with `nice -n 19 python3 -I`.

| Gate | Final result |
| --- | --- |
| `cargo build --workspace` | PASS, exit 0. |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS, exit 0. |
| `cargo fmt -- --check` | PASS, exit 0. |
| Project full suite, `test-util` | Complete: 418 passed, zero failed, two ignored across all 18 unit, integration, and documentation targets. See the runner qualification below. |
| Media full suite, `slow-tests` | PASS: 1,261 passed, zero failed, 32 ignored. Library serially: 1,227 passed, 29 ignored; integration: 33 passed, three ignored; documentation: one passed. |
| App full suite, `slow-tests` | PASS: 932 passed, zero failed, three ignored. |
| Workspace fast suite and skip verifier | PASS, exit 0: 4,449 passed, zero failed, 142 expected ignores. All 65 native test targets (4,445 passes) plus six documentation targets (four passes) completed. The verifier observed exactly 82 slow skips, 60 allowlisted ignores, and 34 passing other-OS controls. |
| Slow/ignore manifest lint | PASS, exit 0: 117 manifest tests, 60 well-formed allowlist entries, 27 equipped-session skip sites counted. |
| Windows compile check of media, project, App | BLOCKED, exit 101 before any touched crate compiled. FFmpeg and AWS-LC native build scripts fail on this Linux cross environment; both also fail at exact `74b22001`. Native Windows compilation and runtime remain NOT RUN. |

The timeout reruns use exhaustive bounded runs. Media library partitions come from its final compiled test inventory; all 1,256 entries are accounted for. The project target-completion ledger accounts for all 18 Cargo targets, including a separate clean rerun of the documentation target after the shell-runner error. These are complete suites, with the command-level timeout and exit-127 events retained separately.

Workspace fast testing uses the unmodified `scripts/slow_tests.py fast` verifier with a local Cargo shim. The shim first builds `cargo test --workspace --no-run --message-format=json`, records every test artifact and feature set, then executes every listed test exactly once in bounded native libtest runs with Cargo's package working directory. Media library runs are serial. Other test binaries use four threads. Original libtest output, exact artifact identities, source hashes, complete test counts, and workspace documentation runs are retained; no skip manifest or product test is changed by the shim.

Raw App summary:

```text
test result: ok. 932 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 202.93s
```

The media library runner's raw exhaustive total is:

```text
MEDIA_LIB_TOTAL selected=1256 passed=1227 failed=0 ignored=29
```

Raw workspace completion and skip-accounting lines:

```text
C6B_FAST_WORKSPACE targets=65 tests=4587 media_threads=1
fast tier on linux skipped exactly the 82 manifest tests and 60 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else; 34 other-OS manifest tests ran and passed
slow-test manifest and markers agree: 117 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 60 allowlist entries well-formed; 27 equipped-session skip sites counted
```

Evidence is in this worktree's `.review/c6b/`: `final-gate-summary.json`, `final-gate-source-hashes.json`, `project-suite-summary.json`, `media-lib-summary.json`, `fast-workspace-summary.json`, `mutation-summary.json`, and their raw logs. The corrected readback probe records Vulkan llvmpipe (LLVM 23.1.3, 256 bits), `software_fallback=true`, in `readbacks-final.log`.

The `/proc` inventory after the full and fast App runs found zero host processes whose executable belongs to this worktree. No unrelated host process was signalled. Own build outputs and the isolated parent build are removed at handoff; large evidence logs are gzip-compressed, with archive names and hashes in `cleanup.json`.

## Guard mutations

Each pair reruns only its named witness. The mutation must reach its target assertion with exit 101. The byte-identical restoration must exit zero, and its SHA-256 must match the final source. All four pairs meet those checks.

| Guard | Red assertion | Restored result |
| --- | --- | --- |
| Issuer sequence before admission | `C6B_RUN_SEQUENCE_CHECKED_BEFORE_ADMISSION` | Exit 0 |
| Consume admitted metadata | `C6B_COMMIT_CONSUMES_ADMITTED_SEALS` | Exit 0 |
| Retain an owner-only pending voice choice during eviction | `S3_VOICE_PENDING_CHOICE_PIN` | Exit 0 |
| Revocation-aware inference abort | `C6B_LATE_ADMISSION_ABORTS_ON_REVOCATION` | Exit 0 |

## Failures and parent comparisons

- The first project run reported 149 passed, 10 failed, and two ignored. Five failures reported `EDQUOT`, four lock/process probes timed out, and one lacked the predecessor prerequisite. Replaying the ten names at exact `74b22001` with `/tmp` and the absent predecessor prerequisite gave nine passes and one missing-predecessor failure. The quota and timeout failures did not reproduce there. With the owned durable temporary directory and verified predecessor artifact, the parent library passed 159 tests with two ignores. Its full S3 store target passed all five tests with a clean exit. The final C6b project suite completed all 18 targets with 418 passed and two ignores.
- The initial Clippy error was new. Recipe fields pushed `transcribe_voice_wav` over the existing function-size lint. Factoring its repeated duration conversion removed the error. Parent media-library Clippy and final workspace Clippy both exited zero.
- Both final and parent Windows attempts fail in `ffmpeg-sys-next` and `aws-lc-sys` before touched-crate compilation. AWS-LC reports unknown `pthread_rwlock_t` while invoking Linux `cc` with Windows definitions. The parent also reports a Whisper binding failure because the Windows C headers are absent. `windows-parent-comparison.json` retains the exact observed differences; these failures are not evidence about the Windows Rust changes.
- The visual incident test reproduced SIGSEGV after its passing assertion summary. Exact-parent GDB also captured SIGSEGV in NVIDIA graphics-pipeline creation on the preview thread, simultaneous NVIDIA/GLX teardown on the main thread, and `Worker::shut_down` joining preview. The native library frames remain unsymbolized. The corrected isolated C6b test exits zero after observing worker teardown.
- Whole-suite commands reached the 600-second deadline. The media rerun uses exhaustive serial partitions from the final test binary. Completed project test targets were retained only where the next Cargo target header confirmed their clean exit. The final documentation target was rerun separately. Editing the local shell runner while an older invocation was active also caused a post-Cargo exit 127; the stable runner and exhaustive target verification resolved that infrastructure error. These timeout and runner events are not counted as passing commands.
- A development-only callback witness initially held atlas setup instead of the intended frame callback. It was corrected with a real warm-up. That new witness does not exist at the parent. An initial seal-consumption oracle also failed to detect repeated admission; the final admission-hook witness reaches its red assertion. Neither provisional attempt is counted among the four final mutation pairs.

## Scope and remaining limits

The legacy-grant boundary, authority-partitioned retained state, and slot-only voice publication remain intact. Fix Root Causes shaped the process-fixture and native-teardown fixes. Model the Domain shaped capacity accounting over accepted visual work and retained note state. Prove It Works shaped the real-process, callback, byte-retention, and mutation witnesses.

The cancellation witness drives the production abort predicate with a real grant and lease; it does not run native Whisper inference. Workspace App output explicitly reports `NOT RUN s3_real_spoken_note_control` and `NOT RUN s3_real_commentary_control` because the pinned `ggml-small.bin` is absent. The media synthesized-speech control also reports its opt-in `KINEWRIGHT_TRANSCRIPT_TEST=1` lane was skipped. These real inference controls remain C6a's work. Native Windows execution, hosted CI, independent lead review, and the full workspace slow tier are NOT RUN. The changed media and App crates did run with their slow-test features enabled. Existing ignored and opt-in lanes are not promoted to runtime acceptance by their libtest counts.
