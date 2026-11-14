# Owner history review fixes

## Historic approvals and cold open

Approval keys retain their original epoch after an install. Only a current-epoch key or prefix end is bounded by the current numeric command counter. Older keys must precede the current epoch; the shared 256-approval cap and role metadata envelope bound them. Prefix addition still checks overflow, and live commands still require exact key/digest/stack matches. The design source under `.review/arch/history-rework.md`, “Kept beside the stacks”, records the same rule.

The named cases are `older_snapshot_install_with_later_do_can_reopen`, `two_older_snapshot_installs_with_later_do_can_reopen`, and `older_snapshot_install_with_approved_undo_can_reopen`. All systems encode and reopen each case in memory, immediately after each install and after another edit. Linux also runs the three `stored_...` counterparts through a real durable session store. Both paths compare documents, both stacks/counters, the ledger and every transition answer, resources, and lineage at every reachable boundary.

`WHAT_CAN_BE_SAVED_CAN_BE_OPENED` is permanent in both generators. The H1 differential checks immediately after every install and at independent seeded random points; its first run made 1,488 checks across 6,480 transitions. The separate owner property generator interleaves approvals, edits, Undo, Redo and older snapshot installs and makes the same check. No additional failure beyond the reported epoch-blind refusals was found in that run.

The owner generator also keeps a three-step bound across installs and checks every fold; positive coverage assertions require both folding and an older-epoch approval beyond the installed numeric counter. `HISTORIC_APPROVAL_POSITION_MUST_NOT_OVERFLOW` separately refuses an impossible maximum position. Restoring the two epoch-blind comparisons made the single-install, two-install and approved-Undo cases fail at `WHAT_CAN_BE_SAVED_CAN_BE_OPENED: InvalidLedger`; the mutation was reverted. The full green Core/Project run then passed 1,141 tests with six ignored (119.22 summed test-seconds, excluding build and lock queue).

## Command-fate and admission fixes in the working tree

The recreation bit and its command-path ledger walk are removed. Do answers are Kept for applied/redone/folded and Missing for undone/discarded. Consent references are unique across Do and transition approvals at admission, typed/byte decode and install merge, in either order. A new probe also found that a malformed Do approval reference was admitted and then refused by capture/open; admission now checks the reference itself. The input generator permanently exercises that refusal.

The 66 owner tests pass (12.64 s). The current differential has 6,512 transitions, 32 approvals, 1,495 round trips, 26 old-uncertain recreation observations, zero other old-uncertain Do observations, the same 48 narrowly checked Do target-shortcut cases, one separately counted sibling Do uncertainty case, and the same four certain control-answer difference classes (54 observations). The owner generator covers 138 round trips, 26 folds, 14 historic-position cases and eight malformed reference refusals. No second final-document approval answer is built.

The requested admission/install tests are `approval_reference_collisions_are_refused_in_both_admission_orders`, `approval_reference_collisions_are_refused_in_both_install_orders`, `one_reference_cannot_name_two_do_approvals`, and `typed_and_encoded_ledgers_refuse_duplicate_approval_references`. Additional regressions cover cross-pool refusal and lease release, duplicate targets, origin keys, shared LUT/font contributors, last-contributor release, and folded-baseline release. The sibling mid-step transition test passes with the existing `origin.step_first` lookup; it already reports ShapeMismatch uncertainty and preserves InvalidOrigin for a missing origin, so that code was not weakened.

All four deliberate wrong changes were reverted:

| Wrong change | Named assertion that failed |
| --- | --- |
| Astra's exact resource-count patch (`if *count == 0` to `if true`) | `SHARED_PAYLOAD_RESOURCE_SURVIVES_ONE_DISCARD` |
| Astra's exact duplicate-target patch | `DUPLICATE_APPROVAL_TARGETS_REFUSED` |
| Astra's exact origin-key patch | `TYPED_ORIGIN_KEY_MUST_MATCH_COMMAND` |
| GLM's missing discard-release change | `LAST_PAYLOAD_RESOURCE_RELEASED_ON_DISCARD` |

## Blocking sibling-folded Do case

The earlier claim that a pre-approval folded Do snapshot cannot arise relied on a single history line. The sibling-line support requested in review item 9 permits this construction through the public owner API:

1. A and B install the same empty snapshot, so both start at epoch 1, position 0, in the same reservation pool and ordinary role.
2. B accepts unapproved C = RemoveTrack81 at `(1, 0)`, folds C by accepting a marker with a one-step cap, and captures S. S has an empty baseline document, fold frontier `(1, 1)`, and no approval or folded-approval fact.
3. Only after S exists, A accepts approved C at `(1, 0)` with exactly the same payload digest and creation origin.
4. A installs S successfully. Its approval for C becomes Discarded/Missing, despite the selected sibling's trusted Core event explicitly reporting that key folded.

The exact failing probe and execution log are [the source](measurements/undo-sibling-folded-do-probe.rs) and [the log](measurements/undo-sibling-folded-do-probe.log). The assertion is `SIBLING_FOLDED_APPROVED_COMMAND_CANNOT_BE_CALLED_DISCARDED`. The probe was temporarily appended to `review_tests.rs`, run by exact name, then removed; production source and the 62-test suite were restored. It uses the same public sibling construction as the passing mid-step test, with matching key/digest identity.

The lead ruled that this case is uncertain, never a refused install, correcting review item 2's single-line impossibility claim. The normative Do install rule is now stated in the design source `history-rework.md`, “Kept beside the stacks”; the reader inventory refers to that rule rather than defining another answer.

## Missing epoch provenance

The ruling also requires a certain missing answer for an epoch not shared by the selected snapshot, and explicitly requires stopping before adding a field if the existing record cannot distinguish it from missing folded provenance. The [public-API probe](measurements/undo-epoch-provenance-probe.rs) and [complete execution log](measurements/undo-epoch-provenance-probe.log) establish this information loss:

- Both lines accept C at `(1, 0)` without an approval. The first folds C and installs a capture retaining that baseline. The second installs the original empty history, abandoning C, then accepts and folds a replacement removal at `(2, 0)`.
- After the same later markers fold, both captures have epoch 2, counter 4, frontier `(2, 3)`, only epoch-2 retained commands, and no approval facts. Their complete encoded `SnapshotData` records are byte-identical (2,671 bytes), including documents, seeds, origins, resources and ledger.
- In the first line epoch-1 C was folded into the selected baseline; in the second line it was abandoned. The assertion `SERIALIZED_RECORD_HAS_NO_OLD_EPOCH_MEMBERSHIP_INFORMATION` passes, proving that neither the single frontier nor the retained stack epochs can recover this distinction. The test passed by exact name and was then removed from the suite; its source and log are retained above.

The lead explicitly rejected an epoch-provenance field. The corrected rule uses the capture's existing epoch, counter and frontier and conservatively answers uncertain for an absent older-epoch command without selected evidence. The canonical rule is in `history-rework.md`, “Kept beside the stacks”. The implementation retains uncertainty as its own fate, preserves the selected ledger's existing absent-command answer, and does not count uncertainty as kept or missing. The sibling case and all four epoch/frontier rows round-trip; both generators assert that a same-line install never makes a Do answer uncertain. No same-line counterexample was found.

Answering the older-epoch row as Missing failed `OLDER_EPOCH_WITHOUT_SNAPSHOT_EVIDENCE_IS_UNCERTAIN` (exit 101); the mutation was reverted. The same-line property found no exception: the approval cap refuses admission rather than evicting entries, and installing a capture advances the epoch using max(source, destination) + 1 with checked overflow.

Commit gates for this group passed: Core/Project 1,156 tests, six ignored, 120.21 summed test-seconds; workspace clippy (7.00 s after the test-grammar annotation), fmt and slow-test lint. No mutation remains active.

## Owner identity

`SnapshotIdentity` holds the unchanged Core digest and the canonical owner digest; its domain-separated combination is the stored/protocol identity. The owner view binds role, frontier, terminal facts, seeds, origins, resources, ledger and folded Do facts in deterministic container order. Validation keeps typed semantic refusals before checking the shell digest, and moves the shell's owned identity through that boundary without a copy. A validated snapshot retains its fixed-size selected identity for H2b. Owner hashing and bytes are separate phase counters; the warm-edit guard requires zero owner hashes.

The named owner identity cases fix the Core digest while changing a valid origin, seed, transition direction, folded prefix fact, or role. Two actual histories generate the different fold facts with identical final Core records. The Linux shell test expects `STORED_OWNER_SHELL_IDENTITY_MISMATCH` and verifies lease release and unchanged destination. Capture/open identity equality is now part of every generator round trip. Measurements and the shell-check mutation are pending below.

The certain different-digest fold-fact neighbour exposed an additional implementation bug: the ledger's key-only index refused two distinct folded sibling payloads. The public-API witness failed at `DIFFERENT_FOLDED_DIGEST_IS_MISSING_NOT_AN_INSTALL_REFUSAL: CommandConflict`. The index now includes the digest already carried by every entry; no record field is added. Retained-key digest conflicts still refuse, while a different selected fold fact discards only the destination approval and keeps the selected command's own approval. The same 256-approval cap applies; the larger index is covered by a 1,024-byte Do fixed charge (plus target/ref capacity), inside the existing role envelope.

Skipping the combined shell identity check failed `STORED_OWNER_SHELL_IDENTITY_MISMATCH` (exit 101), then was reverted. The Do key/digest collision test also checks exact lookup and round-trip preservation; a compound case keeps the selected transition evidence exact in both digest orderings. Its fixed structural charge has an independent typed-size assertion.

## Owned media

Unhashed references are external/unbound locators, not store-owned edges. The real WAV import asserts hash and length against the sealed owned object, and the production matcher rejects stripping the hash. The owner persistence path refuses binding the unhashed document to that owned context, while an explicitly unbound record has no manifest, chunk-table or media-byte edge. The existing hashed history then survives collection and open with all three edges. A pure blob-binding refusal runs on every system without SessionStore. No binding or existing production reader changed.

The identity/media group passed 76 owner tests (13.14 s), then all 1,166 Core/Project tests with six ignored (120.14 summed test-seconds), workspace clippy (20.70 s after documentation/lint annotations), fmt and the slow-test lint. There are 64 portable owner tests, including four store-free codec/binding tests; the twelve real-store tests are Linux-only. The 256-approval task reports 292,112 owner bytes = 256 × (1,024 + 16 + 64) + 9,024 baseline bytes + 464 live-origin bytes; the record remains 130,576 bytes and its decode forecast 4,467,720 bytes. This is inside the unchanged Task A = 4 MiB and W = 64 MiB, with R = (2 + 4) × 4 + 8 + 4 = 36 MiB below the 41 MiB native retained cap. Heavy-fixture identity timing will be recorded with the final fold measurements.
