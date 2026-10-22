# JA-1 S1 round 5

Worktree `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`, starting HEAD
`62f526721f3f91609c19806d50d2b9dfe6ee458f`. One commit,
`fix(ja1-s1): close round-5 review findings and harness setup timing`. Not pushed, not self-reviewed.

The implementation follows the brief's rules 1–7 as amended by the **lead ruling after the round-5
stop**. A Codex implementer started this round and failed partway through. A Claude Opus implementer
took over its uncommitted work, kept what was right, fixed what was not, and reran every result
below. None of the earlier turn's runs is credited here: its gate ran while sources were still
changing (`diff_sha256` differs between its rows).

Raw evidence: `/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r5/claude/`,
gzipped. Mutation pairs are in `mutations/`; timings are in `measure/`, with the CPU-quota pair in
`measure/quota/`; the m6 pair is in `provenance-pair/`; the gate is in `gate/`.
All witnesses are local Linux namespace witnesses on this desktop's filesystem under
`CARGO_TARGET_TMPDIR`. They are not power-loss certification.

## Mechanism

- **Barrier only for an uncertain root publication (rule 1).** `publish_pending`
  (`bundle.rs`) and first-copy `result()` (`bundle_copy.rs`) keep Core's barrier only when this
  attempt reached B5 (`root_sha256`/`publication_tag` set just before `publish_root`) and the outcome
  is unknown. Any failure or uncertainty before B5 releases at once. Object-directory and staging
  uncertainty stays with the store: `ProjectStore::settle_object_writes` retries the directory
  barriers and object-directory syncs, accounts residual files, then drains the reservations.
- **Tagged ticket (rule 2).** Core keeps a `PendingHistorySave { id, tag, ready }`. The tag holds the
  destination path, parent device/inode and the attempted root SHA-256. Core does no IO. Ticket IDs
  are never reused, so a stale owner's commit or release is a no-op and cannot settle a newer ticket.
- **Anyone settles, from durable state (rule 3 and the lead ruling).** `reconcile_core_save` runs
  before every save, branch save, first save/copy, Save As, command, reopen (`restore_manual`) and
  `reconcile_command`. It reads the tagged destination with a fresh namespace inspection; it never
  reads any store's `ledger.pending`.
  - Same parent, root bytes equal the tag, full graph read verifies, parent synced: **commit**
    against Core's current history.
  - Readable parent with an absent or different root: **release**, keeping every entry.
  - Readable path whose parent is no longer the tagged directory (moved or recreated): **release**.
    It can never hold this attempt's root, so this is a definite reconciliation.
  - Unreadable (missing parent, I/O error): refused with `durability_unknown: <path>`
    (`BundleError::DurabilityUnknownAt`), before any mutation. The ticket stays; edits and
    Undo/Redo above the floor keep working; the next attempt reconciles again.
- **Owner drop (rule 4)** leaves the ready ticket in Core; the next operation settles it.
- **Floor alignment (rule 5).** `publish_core` aligns the selection to at least the branch's
  durable floor before selecting and before the barrier. `BundleCommit::publish` refuses a selection
  below the aligned floor as an internal invariant.
- **Typed contention (rule 6).** A command landing during selection is `HistoryChanged`
  (retryable). A second barrier while a save is in flight is `SaveInProgress` (retryable). A second
  barrier over an unsettled ready ticket is `HistoryBarrierInvariant`. None is `Corrupt`.
- **Typed blocked Undo (rule 7).** `Event::UndoBlockedBySave { floor_offset }`; the App shows a
  retry message.

## Dispositions

`round_five::` witnesses are in `kinewright-project --features test-util --test ja_s1_storage`.
Each listed mutation is a runtime red with a restored green, one witness per run
(`scripts/ja-s1-fix-r5-mutations.py`).

| Finding | Disposition | Witness (mutation pair) |
| --- | --- | --- |
| Astra R5-1 (Major): uncertainty before B5 keeps the live barrier forever | Fixed (rule 1). Only an attempted, uncertain root publication keeps the barrier. Object and staging uncertainty stays with the store, which retries it itself. | `round_five::pre_b5_object_uncertainty_releases_and_same_session_retries`: the real objects-directory child-sync fault; every original Undo/Redo target is usable; same-session retry (`pre-b5-release`, `pre-b5-object-retry`). `round_five::dropped_first_copy_can_save_as_in_the_same_session` (`copy-source-recovery`). |
| Opus M1 (Major): a kept barrier has no settlement once its owner cannot reconcile | Fixed (rules 2–4 and the lead ruling). Core keeps a tagged ticket; any later operation settles it from a fresh read of the destination. | Owner drop, reopen and save: `round_five::object_uncertainty_owner_drop_reopen_saves_without_losing_edits` (`object-owner-drop`). Opus's exact scenario, an uncertain object write in an ordinary save to an existing bundle: `round_five::ordinary_save_object_uncertainty_releases_then_new_owner_saves` (`ordinary-pre-b5`, `reconcile-object-retry`). |
| Uncertain B5, then a new owner: an equal root commits, an older root releases | Fixed. | `round_five::orphan_equal_root_commits_with_new_owner` (`orphan-own`), `round_five::orphan_older_root_releases_with_new_owner` (`orphan-older`), `round_five::orphan_absent_root_releases_and_retries` (`orphan-absent`). A published owner refuses a vanished root: `round_five::published_owner_does_not_accept_a_vanished_root_as_unpublished` (`vanished-root`). |
| Orphaned barrier, then Save As to another path: release, and Save As succeeds | Fixed. Readable A: settle, then copy. Unreadable A: `durability_unknown: <A>` with zero mutations, then a retry. A moved or recreated parent: release. | `round_five::readable_orphan_a_reconciles_before_save_as_b` (`readable-save-as`), `round_five::unreadable_orphan_a_refuses_save_as_b_without_mutations_then_retries` (`unreadable-save-as`), `round_five::recreated_parent_a_is_a_definite_release_before_save_as_b` (`recreated-parent`), `round_five::frozen_save_as_settles_its_source_core_before_copying` (`frozen-save-as`). |
| Reopen, or `restore_manual`, over an orphan | Fixed. It reconciles before the replay. | `round_five::reopen_settles_the_orphan_before_restore` (`reopen-entry`). |
| Floor alignment ≥ the durable floor (rule 5) | Fixed. | `round_five::selected_floor_never_precedes_the_durable_floor` (`floor-alignment`). |
| Opus Minor: the stacking refusal has no witness; it must be unreachable from public paths | Fixed. Only `publish_core` (save and branch save) and `first_save_with_limits` begin a barrier, and each reconciles first. A second begin during an in-flight save is `SaveInProgress`; a begin over an unsettled ready ticket is `HistoryBarrierInvariant`. Stale ticket IDs are no-ops. | Each barrier path is driven over an orphan: save (`orphan-own`/`orphan-older`), `round_five::orphan_branch_save_cannot_stack_a_barrier` (`orphan-branch`), first save/copy (`readable-save-as`, `recreated-parent`). Core: `second_barrier_is_an_internal_invariant_and_stale_owners_are_harmless` (`stacking-invariant`, `in-flight-contention`, `ticket-identity`). |
| Opus Minor: drop-release has no witness (S0 `:857-859`) | Fixed. | `round_five::destination_contention_drops_barrier_and_preserves_every_entry` (`contention-drop-release`). |
| Opus Minor: a capture race reports as corruption (rule 6) | Fixed: `HistoryChanged`, retryable. | `round_five::command_during_selection_is_a_retryable_conflict` (`capture-conflict`). |
| Opus Minor: a blocked Undo is silent (rule 7) | Fixed: `Event::UndoBlockedBySave { floor_offset }`, and the App shows a retry message. | Core `pending_floor_blocks_undo_and_commits_current_suffix` (`typed-undo`). |
| Opus Minor: commit replay can stall the actor | Measured; see "Commit replay on the actor thread". The largest replay (26,673 batches) takes at most 9.4 ms. | `R5_ACTOR_COMMIT` lines in `measure/f4-*`. |
| Opus m6: `strip_arguments` is a literal | Fixed. The arguments come from the `strip_arguments` array that the strip command actually runs. | `provenance-pair/`: a red with a literal manifest, and a green that derives the arguments from the changed command. |
| Earlier turn's duplicate barrier hook | Removed. | `round_four::command_lands_mid_save_above_pending_floor` (`single-barrier-hook`). |
| C4: `controller_failure_still_reaps_both_recorded_participants` | Fixed at the root; see "C4". | `hash-vectors`, the setup pair, the quota pair and the five `ja_baseline` runs. |
| C4 follow-up: a participant whose spawn was recorded was reaped off the record when a later participant's setup failed | Fixed. `Controller::new` reaps every recorded worker through `cleanup`, which writes `process_exit`. | `ja_baseline::setup_failure_after_a_recorded_spawn_still_reaps_it` (`recorded-spawn-reap`). |

The mutation pairs, all 26 run on one source tree: every red failed at its intended assertion and every restored run passed (`mutations-driver.log`, driver exit 0). Each restored run records the final tree identity `diff_sha256 11febe55…`, which matches before and after the run.

## C4: `controller_failure_still_reaps_both_recorded_participants`

**Cause.** At setup, each harness worker hashes its own 185 MB debug executable. The
hand-written SHA-256 was built at opt-level 0 in dev and test. Workers start one after the other,
and the supervisor's setup deadline is 30 s counted from fixture birth
(`supervision.rs:287-288`). Locally the second worker became ready at about 13.5 s, 45% of the
deadline. On a slower hosted runner it crosses 30 s. The supervisor then kills the whole fixture
process group and records `not_run`, "native fixture deadline exceeded during setup", with zero
`process_spawn` events, so the old assertion saw 0 `process_exit` events. The test asserted reaps
that never had a chance to happen; with zero spawns it was not a reap bug.

**Reproduced.** The unchanged witness binary was run under a user-scope CPU quota of 40%
(`systemd-run --user --scope -p CPUQuota=40%`), once against each CLI:

| CLI | Result | Second spawn |
| --- | --- | --- |
| before (`b66d8c87…`, sealed pre-change build) | FAILED after 30.04 s: setup deadline, 0 spawns | none |
| after | ok, in 3.84 s | 3,748 ms |

**Fix.** The SHA-256 code moved, unchanged, into the `kinewright-hash` workspace crate, built at
`opt-level = 3` under `[profile.dev.package.kinewright-hash]` and
`[profile.test.package.kinewright-hash]`. `kinewright-media` re-exports it. There are no new
third-party dependencies, and the predecessor, fixtures and deadlines are untouched. The
assertion is not loosened. The test now first asserts that both participants spawned, and prints
the summary if they did not, so a setup timeout is reported as one.

**Related harness gap, now fixed.** The brief asked whether a spawn failure can produce 0
exits. It can, but only in a different case. If `Controller::new` failed after A's
`process_spawn` was recorded (for example, a later participant's setup error), A was reaped by
`Drop` with no `process_exit` on the record. `Controller::new` now reaps every recorded worker
through `cleanup`. The witness uses duplicate participants `["A","A"]` to fail setup
deterministically after one recorded spawn.

**Setup time, both executables in one annotated window, with no build running**
(`measure/setup-pair.json.gz`, `measure/baseline-pair.json.gz`):

| Measurement | Before | After |
| --- | --- | --- |
| Profiled control run, second `process_spawn` | 13,501.6 ms | 1,653.6 ms |
| `ja_baseline` target, the C4 witness's second spawn | 13,434.8 ms | 1,572.9 ms |
| `ja_baseline` target, total | 28.18 s, 8 passed | 6.12 s, 8 passed |

**Deterministic.** Setup time is the test's only dependency on timing. With setup at about 5% of
its deadline it passed in every run here: the measurement pair, the quota pair and the five gate
runs below.

## Commit replay on the actor thread

Under `JA_S1_COMMIT_TIMING`, `R5_ACTOR_COMMIT` times `settle_floor(commit)` on the actor
thread: the suffix replay plus the document comparison. Five sealed runs, debug/test, system
allocator (`measure/summary.json.gz`):

| F4 workload | Batches replayed | Actor commit (5 runs) | Median |
| --- | --- | --- | --- |
| 1, encoded limit | 20 | 23.8–24.3 ms | 23.9 ms |
| 2, batch limit | 26,673 | 4.4–9.4 ms | 5.4 ms |
| 3, large excursion | none: this witness never commits | – | – |

Edits wait at most about 24 ms behind a commit. No change was made. Selection bounds in the same
runs: encoded median 777 ms, batch 214 ms, excursion 32 ms. The first-save retry took 1.78–1.98 s.

## Gate

`scripts/ja-s1-fix-r5-gate.py`, `TMPDIR` unset. Every step records HEAD `62f5267` with the same
`diff_sha256 11febe55…` as the mutation run (`gate/results.json`).

| Step | Result |
| --- | --- |
| Predecessor controls | passed |
| Per-commit gate (`ja-s1-gate.sh 8`): build, eval-harness bins, clippy, rustfmt, affected tests, architecture scans | passed: 336 passed, 0 failed |
| Windows MSVC `cargo check`, core with test-util, lib and tests | passed |
| Windows MSVC `cargo check`, app and project | NOT RUN: the native build script (`ffmpeg-sys-next`) fails on this host |
| Touched crates, full targets: media (`--test-threads=1`) / agent / app / core (test-util) / project (test-util) | 1,164 / 802 / 798 / 793 / 232 passed, 0 failed |
| `kinewright-hash` | builds; its SHA vectors run through the media re-export (`hash-vectors` pair) |
| test-util feature consumer, test-util library, project with no default features | passed |
| Predecessor provenance, setup diagnostic | passed |
| Workspace fast tier (`slow_tests.py fast`) | 3,789 passed, 0 failed. It skipped exactly the 50 manifest tests and the 54 allowlisted ignores |
| `slow_tests.py lint` | "slow-test manifest and markers agree: 50 tests" |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed |
| actionlint, shell syntax, frozen fixtures unchanged since `62f5267` | passed |

Final driver lines:

```
Linux stricter gate complete. Windows execution and full slow CI remain NOT RUN.
gate exit=0
```

Five consecutive `cargo test -p kinewright-agent --test ja_baseline` runs:

| Run | Result | C4 second spawn |
| --- | --- | --- |
| 1 | ok, 9 passed, 3 ignored, 6.06 s | 1,541.7 ms |
| 2 | ok, 9 passed, 3 ignored, 5.97 s | 1,510.0 ms |
| 3 | ok, 9 passed, 3 ignored, 5.99 s | 1,515.3 ms |
| 4 | ok, 9 passed, 3 ignored, 5.95 s | 1,502.6 ms |
| 5 | ok, 9 passed, 3 ignored, 5.94 s | 1,495.2 ms |

Not run: native Windows execution, the full slow tier, hosted CI, and physical power-loss storage
faults.

## Deviations and limits

- C4 adds the small `kinewright-hash` workspace crate (the brief's preferred option). It holds the
  existing SHA-256 code unchanged and adds no third-party dependency.
- A test-only fault point, `object_directory_sync`, was added in `finish_object_batch`, following
  the existing `directory_child_sync` pattern. Without it, no fault reaches Opus's exact M1
  scenario: an uncertain object write in an ordinary save to an existing bundle.
- `ProjectBundle::reconcile` now retries outstanding object-directory syncs before it reports
  settled. Durability did not depend on this, because every publication finishes the object batch
  before B2, but an owner no longer reports settled while a sync is outstanding.
- `ProjectBundle::reconcile` accepts an absent root as "definitely unpublished" only for an owner
  that never had a durable root (Astra's first-publication case). A previously published owner whose
  root vanished still gets `missing reconciliation root`; Core is settled independently.
- A tagged root whose bytes match but whose graph fails to read is still refused as
  `durability_unknown`, not released. That keeps the commit proof strict; recovering such a
  damaged destination is out of this round's scope.
- The earlier turn added an extra executable hash to `run.json` for timing. It was reverted, since it
  added setup work; setup is measured from the existing `process_spawn.elapsed_ns`.
- The constrained reproduction uses a user-scope systemd CPU quota. It changes no system or GPU
  setting.
- m2, m3 and m4 remain deferred as accepted in round 5. Native Windows, the full slow tier and
  hosted CI remain lead-owned.
