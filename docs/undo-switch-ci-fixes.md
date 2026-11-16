# Checkpoint 1 CI corrections

Base: `1896a9a1`, CI run `43808547180`. The reader and door audit is paused.
All reported Linux failures passed. The Linux gates below are complete; native
Windows compilation remains for CI because the cross-check lacks its SDK.

## Product corrections

Close flushes the journal and incidents, then retires the foreground history
before acknowledging the close. Worker and namespace teardown remains on the
reaper. Stale actor handles cannot retain the full ordinary lease across an
immediate reopen. A sealed view retires using its existing accepted cut.

Feedback mapping keeps a charged proof of the original occurrence's ancestor
identities, drops the original full cold history, then opens the candidate.
The existing decoded store budget reserves 512 bytes per ancestor slot for
the typed identity, its two 32-byte IDs and allocation slack. No pool grows.

Task consent preflight checks the proposed batch charge against the executor's
scalar step and payload totals. It copies and measures no document and walks
no retained history. Execution remains the atomic authority for replay and
owner admission; preflight rejects batches that cannot fit before consent.

## API boundaries

`TaskEndpoint` sends a payload `Charge` through `Core::check_batch_capacity` to
`History::check_batch_capacity`; the result is advisory and cannot change the
accepted cut. `Ja1EditingSession::retire_closed_history` requires the completed
close transition and the owner's exact frozen cut. Candidate admission converts
only fixed history/owner-budget errors to the keep-frozen action; pool and
workspace errors keep their original types.

## Regression list

| CI failure | Protection checked after the correction |
| --- | --- |
| `in2b_opening_an_already_open_path_focuses_it` | Close retires the actor with stale handles present; only the other project's ordinary lease remains; the path immediately reopens. |
| `in2b_recovery_without_a_sidecar_seeds_from_the_disk_file` | The original disk-digest assertion remains. |
| `in2b_refuse_suspension_blocks_sidecar_writes_not_project_save` | The original suspension, sidecar preservation and project-save assertions remain. |
| `s2h_lost_save_floor_stays_frozen_until_exact_publisher_result` | Handover advances the epoch and preserves the whole owner; the resulting cut stays frozen until the exact publisher reply; all 12,000 steps survive. |
| `manual_value_fix2_healthy_previews_wait_without_abandoning` | Every blocked preview still compares the complete owner. Drop the no-longer-used full oracle before draining; assert one lease released and the retained final value 2 lands without a notice. |
| `s3_studio_end_to_end` | Original end-to-end feedback mapping, publication, selection, reopen and copy checks. |
| `c4_followups_uncertain_edits_reach_both_readers` | An unverifiable native source binding produces uncertainty in both readers, with target identities and actions; legacy coverage is not evidence. |
| `c4_followups_undo_and_later_removal_match_kept_targets` | Renamed `c4_followups_undo_and_later_removal_keep_command_fates_separate`: the first discarded approval is missing, only the second command is kept, in both readers. |
| `g05a_switch_reply_drop_and_participant_death` | Proves the setup's automatic recovery offer expired after handover and stream-root publication; runs the original participant and publication cuts on a currently offered checkpoint. The serialized launch capture is released; the actual source owner and callback remain. |
| `g05l_host_death_recovers_absent_and_committed_drag_suffix` | The forged sequence reaches writer validation inside the encoded message; all original recovery refusals remain. |
| `run_limit_person_save_feedback_version_and_dirty_copy_survive_latches` | Save As preserves the complete owner and branch while advancing the install epoch and revision exactly once; original and copy still reopen independently by combined identity, branch and revision. The consumed oracle retains no extra full capture during the next Save. |
| `run_limit_cleanup_history_bound_refuses_before_any_live_mutation` | Pre-consent refusal leaves the whole history and approval/control projection unchanged; only the single cumulative accounting reference is replaced, with exactly the refusal count, independent completed-reply measurements and monotone clock observation. Finish remains available. |
| `settled_release_400_clip_resident_heap_after_six_runs` / `settled_release_heap_child` | Exactly 200 accepted commands remain: 199 Undo steps and one Redo step, under NeverFold. The six-run heap limit is unchanged. |
| `rescue_reopen_recopy_retention_stress` | Recopy changes only the install epoch; the resulting complete owner reopens exactly. The CI values have identical Ordinary roles and sizing descriptors. Consuming the prior oracle releases its full capture before reconcile; a counter asserts only the live actor and copied oracle remain. |
| `run_limit_cleanup_dirty_stop_reopens_and_releases_every_latch` | The selected live project is Ordinary; the original saved task checkpoint still opens exactly with the Task role and NeverFold policy. |

The specialist quota fixture now saves distinct unchanged-document heads before
forking. Repeated persistence of one immutable base shares that base and cannot
exhaust specialist slots. The fixture keeps its existing small quota and releases
only verified, checkpointed branches; all dirty-task and restart checks remain.

The existing candidate-refusal test pressures the current encoded profile rather
than using old small-profile merge counts. It retains the original 40/120/400/1,200
clip documents, short edit streams and ripple batch, and adds two explicitly
granted marker payloads. Each marker has a 190,000-byte label and 79 literal JSON
header bytes; together their operations occupy 380,158 bytes (380,161 for the JSON
array). Two complete source deltas fit, and three payloads alone exceed 1 MiB.
The test checks the marker batch against the actual Editor policy and its exact
serialized byte count. Every accepted merge stays in NeverFold history; refusal
leaves the candidate exact, the result frozen, and Finish and Freeze available.
Cold reopen checks every source command, both marker identities and every byte
of their labels after the refused merge.
The separate cleanup guard fills all 4,096 task steps. No performance fixture or
product limit changes.

The CI dependency-declaration diagnostic belongs to `g05a` (the adjacent
`g05l` failure is the JSON-string mutation). `g05a` selected the root captured
before handover and stream publication. Those two publications expire it from
the unchanged two-generation automatic window. The revised fixture asserts that
it is absent from the current offer, then uses a retained offered checkpoint;
it does not remove dependency validation or accept a damaged bundle.

Candidate merge admission now maps fixed history and owner-budget exhaustion to
the existing keep-frozen / publish-result action. Temporary pool and workspace
refusals keep their typed retryable errors. The frozen source remains protected
before any pending merge envelope or candidate mutation.

Additional guards are `CONSENT_CAPACITY_PREFLIGHT_COPIES_NO_DOCUMENT`,
`CONSENT_CAPACITY_PREFLIGHT_MEASURES_NO_DOCUMENT` and
`CLOSED_SEALED_VIEW_RELEASES_ITS_FULL_LEASE`.

## Windows compilation

Linux-only durable-test imports in `operation_checkpoint_tests.rs` now have
the tests' platform gate. The portable checkpoint codec and receipt tests stay
on every system. The Windows storage refusal test imports the new Core and
checks its native Undo stack; the Windows save refusal test creates its pool
without referring to a nonexistent actor. Both Windows refusal tests remain.
No existing test is newly ignored or gated away by these corrections.

Windows still runs `checkpoint_codec_requires_the_new_owner_and_combined_install_identity`,
`checkpoint_receipts_name_the_current_epoch_position_and_command_cut`,
`windows_portable_fixtures_refuse_durability_without_mutation` and
`native_windows_still_refuses_before_store_mutation`, along with the portable
actor tests, including the new consent-capacity regression. The new sealed-close
test uses a durable session store and is Linux-only.

The installed MSVC target check was attempted and completed with exit 101 before
project tests: this Linux environment lacks the Windows SDK/C cross toolchain
needed by FFmpeg, Whisper and AWS-LC. The affected platform gates and Windows
call sites were read directly. Native Windows compilation remains for CI.

## Verification

The targeted person/cleanup latch, host-switch recovery, rescue recopy,
candidate-refusal and consent-capacity checks passed. The app `in2b_` filter
passed all 60 tests. A full fast pass exposed one additional manual-preview test
retaining a full oracle beyond its last comparison; its focused correction
passed, and the final gate also checks the actual document value after drain.

The final standalone core/project gate passed 1,267 tests across 49 targets,
with 24 existing tier/platform skips. Slow-test lint passed: 160 tests, 162
allowlist entries and 27 equipped-session skip sites. Python slow-test and shard
self-tests passed 52 and 26 tests. The final complete fast workspace gate passed
5,537 tests across 91 targets, with zero failures and 287 existing tier/on-demand
skips. Agent and app unit counts were 1,335 and 1,082.

The slow gate for core, project, agent and app uses the existing Linux slow-tier
profile in `.cargo/ci-test-linux-slow.toml` and enables all five slow-test features.
It passed 4,373 tests across 81 targets, with zero failures and 116 existing
on-demand/platform skips. Agent, app and project unit counts were 1,356, 1,087
and 395. All 64 long session-model seeds passed. Every Linux failure named in
the lead note, plus the additional candidate-limit and manual-preview guards,
passed in this final run.

Workspace clippy (`--all-targets -- -D warnings`), fmt check, workspace build and
slow-test lint passed. The three separate shipping builds passed:

- `cargo build -p kinewright-agent --no-default-features --lib`
- `cargo build -p kinewright-app --no-default-features --bin kinewright-app`
- `cargo build -p kinewright-agent --no-default-features --bin kinewright`

Every Cargo command sourced `scripts/setup-ffmpeg.sh` and used the requested
heavy lock, four build jobs and `nice -n 10`. Tests used four harness threads.
The agent shipping build reports the existing unused `authentication::REFRESH`
constant warning; that source is unchanged. No limits or timeouts increased and
no existing tests were suppressed.

Logs are in `target/hx-ci-fixes/`. Completed test executables were removed only
after their entire gate passed, keeping `target/` below 40 GB without removing
sources or evidence. All started Cargo runs have finished.
