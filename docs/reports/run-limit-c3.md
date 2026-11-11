# Run-limit C3: cleanup admission

Base: C2 `059edd11f3efb9be8c9230965e1af520f5621308`, on `te1-run-limit`. Local work only; C1 stays dropped and the approval cap stays 256. C5 and C4/C4b are separate work.

## Implementation

The host retains the exact guardian retirement proof in a synchronous `CleanupOwner`. Only `reconcile_native_effect` (after the not-dispatched fence, when applicable) and `native_completed` can create it. The project capability checks store identity, project, active run budget and the exact non-retired attempt. Its H1 update must retire that same attempt through validated terminal progress. RAII restores ordinary admission on success, error and unwinding; pins, readers and decoder reservers do not inherit it.

Cleanup uses raw project admission for the complete prerequisite transaction, without changing the run's sticky refusal: private and candidate checkpoints, specialist bases, TaskHistory and optional frozen-result evidence, accounting, issuer/receipt support, and terminal publication. The admitted kinds are Document, History, Lineage, BranchCheckpoint, BranchIndex, SpecialistBase, RootCheckpoint, StoreManifest, ReceiptIndex, IssuerState, RunIndex, OwnedEvidence, and the unchanged FeedbackIndex republished by H1. It admits no new media, feedback revision, version snapshot, import or model tool operation.

The existing limits remain eight tasks, 24 attempts and 32 terminal cuts. At most two branches per task need cleanup; a changed branch stages at most six objects (document, baseline, history, lineage, checkpoint and specialist base), independently of edit count. TaskHistory and the optional two frozen-result evidence objects are bounded additions. Already-retired attempts return before minting; retries reuse staged matching history and content-addressed objects. Accounting retains its 1 MiB/128-send/512-sample bounds and existing successor checks.

Ordinary writers now leave 64 MiB (previously 16 MiB) of the existing 128 MiB live-history project limit for cleanup: 16 branches × three 1 MiB retained/staged records, plus 16 MiB of serial replay and old-capture overlap. New ordinary specialist bases have a 1 MiB decoded bound too. Ordinary specialist-base admission also leaves 16 version slots (two per task), checked before StartRun. An existing base can be reused at the raw version limit; a matching retry needs no new slot. Raw project limits are unchanged. Genuine raw project exhaustion, disk failure and ambiguous native liveness retain their existing refusals.

Every writable task endpoint captures its immutable specialist floor prefix and project limits when prepared, reopened or continued. A continued frozen endpoint stays sealed and can outlive its closed durable registration; it receives no new edit admission. Before any live edit, the endpoint validates proposed history and the complete specialist delta. Both encoded and decoded history, and the specialist base, stay within 1 MiB. Destructive preparation checks before an approval request; application rechecks under existing ingress serialization. Undo and Redo also check their prospective journal. The model receives: “Task history limit reached. Finish or freeze this task before making more edits.” The refusal does not latch the run.

Candidate merges already validate and publish their proposed durable command before adopting it into the actor (`commit_candidate`/`BundleCommit`); the specialist-base admission bound also applies there. No history flooring or paging was added.

A passive H1 cut now retains the exact previously published TaskHistory reference without adopting pending task or candidate checkpoints. When a source is staged, its prior checkpoint/head/document must still match the committed binding. New history references retain the original strict pairing checks. This fixes the reproduced staged-Stop `history_changed` refusal without publishing an unpaired checkpoint.

## Witnesses

The targeted agent run passed all three tests (1,374 filtered), including all ten cleanup cases, in 100.86 s. Every cleanup case used three terminal cuts; measured peak live-history admission was at most 442,123 bytes on this small fixture. The real specialist-version quota/retry test passed (1/1, 179 filtered, 3.26 s), and the real-shaped measurement passed (1/1, 179 filtered, 0.03 s). The existing frozen continuation/restart/merge/Stop regression also passed (1/1, 1,376 filtered, 12.65 s). Final full-suite readback is below.

The small slow-tier cleanup matrix uses real object-quota and retained-specialist-version refusals and the real 128-send guard with validated scripted accounting. Attempts and ordinary-cut cases explicitly simulate the common sticky latch using the existing test-only probe; they do not claim 24 launched attempts or 1,024 executed cuts. Every case starts with uncheckpointed Do/Do/Undo task history and a redo tail. The overlap cases retain an unpublished task/candidate checkpoint and perform a manual Save before Stop. Native absence is observed and the unbound transport fenced before Stop; cleanup retains that proof, retires H1, retires the private claims, and releases the machine lock. It checks duplicate retirement, terminal count, exact replay and Redo, two reopens, the person's retained-work switch, and a new run with no replayed send. For the deliberately 512-object and 20-specialist-version projects, normal test limits are restored before the new-run capacity check; cleanup does not invent free project capacity.

The pre-mutation witness uses the existing 12-track/120-clip save-cost document, checking reversible and destructive refusal before both live mutation and approval, followed by successful Finish/Freeze. Separate exact-boundary Undo and Redo witnesses refuse without changing the live history.

## History-size measurement

Measured with the existing decoded-admission estimator, not heap/RSS measurement. The fixture is factored from `ja_s2_save_cost`; its timeline shape is unchanged. Each edit uses the tool's single-operation `DoBatch` to toggle one track's sync lock.

| State | Journal batches | Encoded bytes | Decoded admission bytes |
| --- | ---: | ---: | ---: |
| Baseline, 120 clips | 0 | 39,541 | 743,805 |
| One edit | 1 | 60,119 | 1,128,927 |
| Two edits | 2 | 80,679 | 1,513,812 |
| Undo | 3 | 80,763 | 1,515,060 |
| Redo | 4 | 80,849 | 1,516,312 |

The first edit adds 385,122 decoded bytes; the second adds 384,885. The baseline leaves 304,771 bytes below the ordinary 1,048,576-byte checkpoint limit. Thus **zero additional edits fit** on this real-shaped document: previously its first edit made ordinary Finish/Freeze/Merge unreachable; C3 refuses that edit beforehand. This is the separately requested task-history capacity problem, not a usability fix. A task that would cross 1 MiB is refused before mutation, rather than relying on oversized cleanup rescue.

Muse's claim that Undo/Redo cannot grow stored state is too broad: they preserve the cursor's document set but append journal entries. Here Undo adds 1,248 decoded bytes and Redo 1,252. Their pre-mutation checks are therefore necessary. The exact-boundary witness starts Undo at 1,048,575 decoded bytes and Redo at 1,048,574.

## Gates and limits

All gates finished successfully on the source recorded in `target/run-limit-c3/gate-source-hashes.json`; all 22 source/manifest hashes matched after completion.

| Full suite (including binaries, integration and doc tests) | Passed | Failed | Ignored | Permitted name skips | Suites |
| --- | ---: | ---: | ---: | ---: | ---: |
| kinewright-agent, slow-tests | 1,704 | 0 | 95 | 1 | 30 |
| kinewright-project, test-util | 440 | 0 | 3 | 1 | 20 |

`cargo fmt -- --check` passed. All-target clippy for both changed crates with `-D warnings` passed (13.15 s). Slow-test lint passed: 125 tests, 150 allowlist entries, 27 counted equipped-session skip sites. The agent library alone passed 1,318 tests, with 58 ignored and one permitted skip, in 1,154.98 s.

Small local evidence: [gate commands](../../target/run-limit-c3/gate-frozen-fix.sh), [full counts](../../target/run-limit-c3/full-counts.json), [full log](../../target/run-limit-c3/full.log), [targeted cleanup](../../target/run-limit-c3/cleanup-targeted-final.log), [quota boundary](../../target/run-limit-c3/quota-final.log), [history measurement](../../target/run-limit-c3/measure-final.log), [frozen continuation](../../target/run-limit-c3/continuation-final.log), [source hashes](../../target/run-limit-c3/gate-source-hashes.json).

All Cargo commands source `scripts/setup-ffmpeg.sh`, use the shared `flock -o /tmp/kinewright-cargo-heavy.lock`, `CARGO_BUILD_JOBS=4` and `nice -n 10`. Full tests exclude only the two predecessor-dependent names allowed by the brief. No provider command, paid run, Windows runtime qualification or history migration is claimed.

The ruled same-boot crash window before an unbound retirement proof remains deferred. Host death before a checkpoint can still lose the last volatile attempt tail, as designed; this work does not create per-edit durability. The cleanup witness obtains valid absence proof before Stop and reopens persisted work after retirement. Raw project/disk capacity remains a real boundary for both person and cleanup publications.

Disk cleanup requested during C2 is recorded in `docs/reports/run-limit-c2.md`: 25.10 GiB before, 13.28 GiB after, 11.82 GiB freed, with small cited evidence retained.
