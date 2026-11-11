# Run-limit C2: person publication admission

Status: C2 verified. C3 is separate work.

The approval cap stays at 256. C1's cap and instrumentation changes were discarded. C3 cleanup admission and C5 linear storage are separate work.

## Person authority

`PersonAdmission` is a private store-identity token, held only by a scoped publisher. `with_person_publication` uses the existing publication-store restoration guard. Its store uses raw project limits, bypasses the run's sticky write guard and encoded/decoded/graph clamps, and preserves raw `Capacity` errors without latching the run. It shares the ordinary namespace and ledger, so project capacity, disk, identity, pending-command, uncertainty and fresh-open checks remain in force. A nested terminal reconciliation retains both the person origin and its exact terminal proof. Pins and ordinary store factories do not retain person admission.

Mint sites are the manual `ProjectBundle::save` entry, source, destination and destination reconciliation of manual Save As, authenticated manual lease issuance and accepted manual-history publication, feedback/snapshot metadata transactions, feedback evidence, voice ingress and voice evidence. Read-only published-history acknowledgement and voice capture/state reservations use the same project admission. Issuer bookkeeping uses `save_current` without minting person admission (the manual-lease caller explicitly owns its person transaction); task checkpoints, orchestration and run effects do not mint a person token. Authenticated host roles and producer scopes gate the person commands before these project APIs. No token or token constructor crosses the wire.

## Witnesses

Passed in the corrected focused and full runs: the registered small slow-tier test exercises both an object-capacity latch (512 raw objects, 256 ordinary) and the 128-send count latch, each before Stop, after Stop with cleanup unresolved, and after restart. It performs manual Save with Redo, version snapshot, typed/drawn/spoken feedback, dirty Save As, later manual edits and Save, then closes and reopens both source and destination exactly. Send counts are a validated cumulative scripted accounting publication followed by the production send-budget guard; no provider is invoked.

Passed in the corrected focused and full runs: a real staged task checkpoint with a root-publication fault, manual Save, then idempotent task-checkpoint retry; task and run producers refused general snapshot publication, task producer refused Save, and an ordinary run effect still refused after a person Save. A GUI-transport witness acquires the manual lease, publishes accepted Do/Do/Undo through Host `CommitHistory` at both latches, verifies the run remains refused, and reopens exact. Project-store witnesses cover a record above the run's decoded clamp and a history above its 1 MiB run clamp, both below raw capacity, and a raw refusal that remains `Capacity` without latching. The existing public-write probe distinguishes person feedback/Save from ordinary run writes.

By code inspection, import still performs ordinary owned-media admission before the manual Save transaction (`editing_session/owned_media.rs`); there is no end-to-end import witness in this change. Acquiring the manual lease is person-owned, but media bytes still refuse with `RunLimit` before the asset edit is accepted. It should eventually have an explicit person import transaction because import is a person action, but admitting its bytes and corresponding Core edit is a separate boundary; this commit does not broaden that path.

## Completed C1 measurement retained as input

The zero-approval full maximal finished: **1 passed, 0 failed, 1,369 filtered**, 3,665.52 seconds. It exercised eight tasks, 24 attempts, 128 sends, Stop, cleanup and exact reopen. An initial 64-approval build failed on a temporary fixture constant path; the corrected maximal was queued but cancelled before Cargo or the fixture started when C1 was dropped. No 64-approval measurement ran. No approval cap change remains. The temporary test-only instrumentation is saved under `target/run-limit-c1/instrumented-c1.patch`, with source hashes and the original log. It is not committed.

After the last send the ordinary manifest was 13,329,764 decoded bytes and 1,704,679 encoded bytes. Cleanup and reopen retained a 13,438,677-byte decoded manifest (1,718,556 encoded). The narrowest observed ordinary storage margin was the transition record: 152,521 / 196,608 bytes, **22.42%** free, even with zero approvals. The fixture checkpoints explicitly and sends do not edit; its successful cleanup does not prove dirty-task cleanup, as Muse observed. The claimed 64-approval send-phase failure remains unmeasured here. Both reviewers are correct that count caps can latch independently of remaining space; no reviewed code assertion was found wrong in the C2 paths checked.

Peak admission values below come from the temporary per-admission trace, not a retained-usage estimate. Byte quotas use bytes; object, receipt, issuer, version, command, send, attempt and depth rows use counts. `scratch_request` reserves the full 64 MiB preflight scratch allowance by design; its zero margin is the fixed reservation request, not a 64 MiB record. Sends and attempts intentionally reach their logical caps. `budget.terminal` is sampled before each admission; the final persisted count is 3 / 32. No decoder was used. Free-disk reservation limits reflect the filesystem at each request.

| Quota / measurement | Peak reached | Limit | Margin |
| --- | ---: | ---: | ---: |
| project.physical | 38,389,120 | 44,023,414,784 | 43,985,025,664 (99.91%) |
| project.metadata_plus_decoded | 126,717,623 | 536,870,912 | 410,153,289 (76.40%) |
| project.history | 891,125 | 134,217,728 | 133,326,603 (99.34%) |
| project.decoder | 0 | 4,294,967,296 | 4,294,967,296 (100.00%) |
| project.noncopy_physical | 38,389,120 | 22,011,707,392 | 21,973,318,272 (99.83%) |
| project.copy_overlap | 0 | 22,011,707,392 | 22,011,707,392 (100.00%) |
| project.reserved | 1,718,556 | 317,904,433,152 | 317,902,714,596 (100.00%) |
| project.scratch_request | 67,108,864 | 67,108,864 | 0 (0.00%) |
| project.control | 8,685 | 67,108,864 | 67,100,179 (99.99%) |
| project.control_metadata | 40,715,828 | 536,870,912 | 496,155,084 (92.42%) |
| json_depth | 14 | 128 | 114 (89.06%) |
| project.record_encoded | 1,718,556 | 67,108,864 | 65,390,308 (97.44%) |
| project.record_decoded | 13,438,677 | 67,108,864 | 53,670,187 (79.97%) |
| project.objects | 811 | 10,000 | 9,189 (91.89%) |
| project.owned | 27,948 | 21,474,836,480 | 21,474,808,532 (100.00%) |
| project.object_metadata_plus_decoded | 57,512,867 | 536,870,912 | 479,358,045 (89.29%) |
| project.history_encoded | 6,649 | 33,554,432 | 33,547,783 (99.98%) |
| project.history_decoded | 105,876 | 33,554,432 | 33,448,556 (99.68%) |
| depth | 14 | 32 | 18 (56.25%) |
| project.manifest_encoded | 1,718,556 | 67,108,864 | 65,390,308 (97.44%) |
| project.manifest_decoded | 13,438,677 | 67,108,864 | 53,670,187 (79.97%) |
| receipts | 241 | 4,064 | 3,823 (94.07%) |
| issuers | 10 | 124 | 114 (91.94%) |
| versions | 1 | 99 | 98 (98.99%) |
| ordinary.transition | 152,521 | 196,608 | 44,087 (22.42%) |
| ordinary.receipt_encoded | 4,653 | 196,608 | 191,955 (97.63%) |
| budget.ordinary | 239 | 1,024 | 785 (76.66%) |
| budget.terminal | 2 | 32 | 30 (93.75%) |
| budget.attempts | 24 | 24 | 0 (0.00%) |
| h1_aggregate_with_headroom | 144,787,442 | 268,435,456 | 123,648,014 (46.06%) |
| accounting_encoded | 117,665 | 1,048,576 | 930,911 (88.78%) |
| sends | 128 | 128 | 0 (0.00%) |
| usage_samples | 256 | 512 | 256 (50.00%) |
| metadata_command | 152,640 | 262,144 | 109,504 (41.77%) |
| pending_commands | 1 | 63 | 62 (98.41%) |
| ordinary.physical | 37,924,428 | 43,469,766,656 | 43,431,842,228 (99.91%) |
| ordinary.metadata_plus_decoded | 112,563,317 | 268,435,456 | 155,872,139 (58.07%) |
| ordinary.history | 891,125 | 117,440,512 | 116,549,387 (99.24%) |
| ordinary.decoder | 0 | 4,294,967,296 | 4,294,967,296 (100.00%) |
| ordinary.noncopy_physical | 37,924,428 | 21,734,883,328 | 21,696,958,900 (99.83%) |
| ordinary.copy_overlap | 0 | 21,734,883,328 | 21,734,883,328 (100.00%) |
| ordinary.reserved | 1,704,679 | 317,904,060,416 | 317,902,355,737 (100.00%) |
| ordinary.scratch_request | 67,108,864 | 67,108,864 | 0 (0.00%) |
| ordinary.record_encoded | 1,704,679 | 8,388,608 | 6,683,929 (79.68%) |
| ordinary.record_decoded | 13,329,764 | 33,554,432 | 20,224,668 (60.27%) |
| ordinary.objects | 804 | 9,744 | 8,940 (91.75%) |
| ordinary.owned | 27,948 | 21,466,447,872 | 21,466,419,924 (100.00%) |
| ordinary.object_metadata_plus_decoded | 56,961,129 | 268,435,456 | 211,474,327 (78.78%) |
| ordinary.control | 8,685 | 8,388,608 | 8,379,923 (99.90%) |
| ordinary.control_metadata | 40,223,424 | 268,435,456 | 228,212,032 (85.02%) |
| ordinary.manifest_encoded | 1,704,679 | 8,388,608 | 6,683,929 (79.68%) |
| ordinary.manifest_decoded | 13,329,764 | 33,554,432 | 20,224,668 (60.27%) |
| ordinary.history_encoded | 6,642 | 1,048,576 | 1,041,934 (99.37%) |
| ordinary.history_decoded | 105,993 | 1,048,576 | 942,583 (89.89%) |
| terminal.transition | 152,433 | 524,288 | 371,855 (70.93%) |
| terminal.receipt_encoded | 835 | 6,144 | 5,309 (86.41%) |

At reopen: 810 store objects; 12,911 usage edges; 40,248,155 metadata bytes; 38,002,983 physical bytes; 27,948 owned bytes; 256,660 live-history bytes; zero decoder, copy and outstanding reservation bytes. The manifest retains 803 objects and 12,231 edges. There are 242 receipts, ten issuers, one version and budgets of 239 ordinary cuts, three terminal cuts and 24 attempts. Cleanup is settled, the run budget inactive, and exact history reopened. Earlier lifecycle peaks were 38,389,120 physical bytes, 86,894,241 decoded bytes and 891,125 live-history bytes. Retained evidence: `target/run-limit-c1/zero-facts.json`, `zero-quota-table.md`, `analyze.py`, `zero-analysis.log`, `zero.log`, source hashes and the instrumentation patch. The 3.3 GB generated event trace was deleted during the requested cleanup; the derived facts and quota table remain.

## Gates

All required checks passed on the final source. Full-suite counts include unit, binary, integration and documentation tests, read from `full.log` after Cargo exited successfully:

| Crate | Passed | Failed | Ignored | Filtered | Suites |
| --- | ---: | ---: | ---: | ---: | ---: |
| kinewright-agent | 1,701 | 0 | 95 | 1 | 30 |
| kinewright-project | 438 | 0 | 3 | 1 | 20 |

Command: `cargo test -p kinewright-project -p kinewright-agent --features kinewright-project/test-util,kinewright-agent/slow-tests`, with only the two predecessor tests named in the brief excluded. Agent's library subset was 1,315 passed, zero failed, 58 ignored and one filtered; project's was 175 passed, zero failed, two ignored and one filtered. The corrected focused agent run passed 4/4 (1,370 filtered), 111.45 seconds. Both project admission witnesses, including the history above the ordinary 1 MiB bound, pass in the full run.

`cargo fmt -- --check` passed; clippy passed for both changed crates, all targets, project test-util and agent slow-tests, `-D warnings` (21.25 seconds); slow-test lint passed (124 slow tests, 150 allowlist entries, 27 equipped-session skip sites). Cargo commands used the shared heavy lock, four build jobs and nice 10. Logs, final source hashes and per-suite counts are under `target/run-limit-c2/`.

Earlier attempts exposed fixture setup errors, float WAV outside the PCM16 capture contract, and a mistaken expectation that manual acknowledgements include full history. These were corrected; the first full attempt finished 1,313 passed, two failed, 58 ignored and one filtered, stopping before the project suite. The final full run above is the gate result. No paid/provider command, backward-compatibility work, push or linear-storage implementation is included.

## Requested disk cleanup

Performed after the full suite and final formatting had exited, with no Cargo/compiler/formatter process in this worktree. Allocated `target/` size fell from **26,951,802,880 bytes (25.10 GiB)** to **14,256,148,480 bytes (13.28 GiB)**: **11.82 GiB freed**.

Removed the generated C1 event trace while retaining the small cited results, logs, source hashes and patch; removed the registered predecessor checkout with `git worktree remove --force target/ja-s1-predecessor/source`, then its parent folder; removed unused `target/win-xwin`; removed 52 superseded test binaries (6,057,553,920 allocated bytes). Binaries used by the completed full gate, the newest unexercised test variant per target, hard-linked binaries, libraries and metadata remain. The predecessor source is no longer registered. Exact size and deletion inventories: `target/run-limit-c2/cleanup-sizes.json` and `stale-test-binaries-removed.json`.
