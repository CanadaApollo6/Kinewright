# Owner history review fixes

## Historic approvals and cold open

Approval keys retain their original epoch after an install. Only a current-epoch key or prefix end is bounded by the current numeric command counter. Older keys must precede the current epoch; the shared 256-approval cap and role metadata envelope bound them. Prefix addition still checks overflow, and live commands still require exact key/digest/stack matches. The design source under `.review/arch/history-rework.md`, “Kept beside the stacks”, records the same rule.

The named cases are `older_snapshot_install_with_later_do_can_reopen`, `two_older_snapshot_installs_with_later_do_can_reopen`, and `older_snapshot_install_with_approved_undo_can_reopen`. All systems encode and reopen each case in memory, immediately after each install and after another edit. Linux also runs the three `stored_...` counterparts through a real durable session store. Both paths compare documents, both stacks/counters, the ledger and every transition answer, resources, and lineage at every reachable boundary.

`WHAT_CAN_BE_SAVED_CAN_BE_OPENED` is permanent in both generators. The H1 differential checks immediately after every install and at independent seeded random points; its first run made 1,488 checks across 6,480 transitions. The separate owner property generator interleaves approvals, edits, Undo, Redo and older snapshot installs and makes the same check. No additional failure beyond the reported epoch-blind refusals was found in that run.

The owner generator also keeps a three-step bound across installs and checks every fold; positive coverage assertions require both folding and an older-epoch approval beyond the installed numeric counter. `HISTORIC_APPROVAL_POSITION_MUST_NOT_OVERFLOW` separately refuses an impossible maximum position. Restoring the two epoch-blind comparisons made the single-install, two-install and approved-Undo cases fail at `WHAT_CAN_BE_SAVED_CAN_BE_OPENED: InvalidLedger`; the mutation was reverted. The full green Core/Project run then passed 1,141 tests with six ignored (119.22 summed test-seconds, excluding build and lock queue).
