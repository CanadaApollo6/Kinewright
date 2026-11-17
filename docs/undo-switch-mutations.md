# Switch mutation qualification

The seven wrong changes enumerated by the H2b brief ran as compiled runtime
mutations, with the source restored after each. Each exact witness fails at
its required named assertion and its sealed original passes. Three healthy
controls ran first: receipt/restart/install, independent GUI/shadow folding,
and actual Redo-only LUT bytes across collection, cold open and Save As.

Two initial resource breaks reached an unnamed cold-open refusal. Fresh byte
checks confirmed the LUT remained present while the owner lived; the missing
object appeared after owner teardown. Those attempts are preserved as unqualified.
The cold-open reachability assertion is now named too. No save, collection,
fixture value or limit was added or removed. The earlier four qualified cases
are unchanged. Restored source identities match within each run.

| Wrong change | Named assertion | Result |
| --- | --- | --- |
| `ack-before-root` | `ACK_FOLLOWS_DURABLE_ROOT_PUBLICATION` | runtime red / original green |
| `restart-reapplies-receipt` | `RESTART_RECEIPT_RETRY_NEVER_REAPPLIES_COMMAND` | runtime red / original green |
| `install-keeps-chain` | `INSTALL_RETIRES_PREVIOUS_EPOCH_CHAIN` | runtime red / original green |
| `shadow-skips-fold` | `SHADOW_REPLAYS_GUI_FOLD_AT_THE_SAME_COMMAND_BOUNDARY` | runtime red / original green |
| `collect-reachable-redo` | `REACHABLE_REDO_LUT_BYTES_SURVIVE_COLLECTION` | runtime red / original green |
| `uncertain-counted-as-removed` | `UNCERTAIN_APPROVAL_NEVER_COUNTS_AS_REMOVAL` | runtime red / original green |
| `core-only-snapshot-comparison` | `INSTALL_EQUALITY_USES_COMBINED_SNAPSHOT_IDENTITY` | runtime red / original green |

The GUI and shadow use independent pools with the same original owner origin.
The fold case compares combined identities and the complete canonical owners
after every Do, Undo and Redo, and asserts that a real fold occurred. Receipt
retries retain the original producer lease through the restart and
prove the accepted change id did not advance. Acknowledgements name the actual
published root bytes. Each LUT check takes a fresh certificate, reads the real
bytes and drops its pin before the next collection; discarded Redo resources
must still become collectable after the existing root-retention window.

The receipt and store tests stay Linux-only. Independent fold and protocol
identity checks and uncertainty exclusion remain portable. Production behavior,
fixture values, role limits and deadlines are unchanged.

## Restored-source gate

The final source passes 5,464 workspace fast tests (283 declared ignores),
697 Project slow-feature tests (three declared helper/platform ignores), the
explicit 20,000-clip open/edit/fork/merge/Save fixture (one pass, no ignores),
and 41 library plus 13 unpaid opt-in TE1 tests. All 16 session partitions pass:
64 seeds and 65,536 attempts. The Project-only slow command does not activate
integration files gated by `test-util`; the capacity fixture therefore ran
separately with both features. All 17 Project slow-manifest cases are covered.
The full fast gate already includes the integration helpers and all new guards.

Workspace and feature clippy, fmt, workspace build, all three separate shipping
builds, slow-test lint, three mutation-runner fault checks and 52 slow-harness
self-tests pass. The supervised commands report no signals. The observer changes
zero pin leaves. The native MSVC prerequisites remain unavailable as recorded
in the deletion report; no Windows execution qualification is claimed here.

The post-deletion inventory still matches the original inventory hash and has
207 unique, accounted-for rows: 126 moved, 28 retired, 18 preserved and 35
unchanged. None remains pending; the replacement source files and named native
APIs still exist. [Complete runtime and gate evidence](measurements/undo-switch-mutations-proof.json.gz)
retains all seven qualified pairs, both incomplete resource attempts, restoration
identities, original compiler and binary hashes, commands, logs and final diff.
No build or test remains running at this commit. The exact 256-cycle full-tail
qualification and final paired measurements are next.
