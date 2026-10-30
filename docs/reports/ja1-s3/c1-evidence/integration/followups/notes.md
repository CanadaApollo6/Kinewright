## Follow-ups

This commit addresses C1F3-m1, C1F1-M1, C1F1-M2, C1F1-M4, and C1-M9 after the main merge. The one-live-Editor invariant remains. No design plan or S4 hook semantics are changed.

| Follow-up | Change and witness |
| --- | --- |
| C1F3-m1 | A detached, lease-free retained save or switch result remains bound to its client. Before cache access or execution, another client's numbered or manual-control request is refused with `handoff_pending`, including numbered and unnumbered `AcquireManual` and `ConfirmSwitch`. Connector reads remain available. The trusted local issuer can explicitly discard the current detached result through `discard_detached_handoff`, supplying the retained client. It is not an HTTP/MCP command. `s3_followup_detached_handoff_requires_explicit_discard` covers both lost Save As and lost Switch over HTTP and direct ingress: A detaches, B is issued, B's acquisition/confirmation/write attempts preserve the slot and root, wrong-client discard refuses, A can be reissued and retry the original result, and an explicit correct discard permits B to acquire or confirm and save. |
| C1F1-M1 | `VoiceRef::matches_note` now compares `duration_micros`. The existing late-ASR witness also refuses feedback whose otherwise valid embedded voice duration differs from the saved note, checking unchanged root bytes and note duration. |
| C1F1-M2 | The projection oracle now requires each projected root reference other than the manifest itself to occur in that manifest's `objects`. The root-producer witness independently seals a manifest missing each supported index declaration while keeping its root ref, and requires the exact new oracle refusal. |
| C1-M9 | Host read pins are deduplicated by exact reference roots and the store ledger that owns the pin. Repeated snapshot, feedback and submitted-batch reads do not spend slots, including at the 64-slot cap. Save As reads retain new pins in the new store even for identical object refs. `s3_followup_repeated_reads_dedupe_reference_pins_at_capacity` exercises all three ingress routes, cross-store reads, 64 distinct pins, refusal of a new reference, repeated reads at capacity, and detach. |
| C1F1-M3 | **NOT RUN**: no ClaimIndex producer; nonempty refused at bundle.rs:156. All four refs remain carried, compared and included in the generic manifest oracle; runtime and mutation coverage exercise the three supported nonempty indexes. |

C1F1-M4 is a documentation correction. The new [root-list inventory](c1-evidence/integration/followups/root-list-audit.txt) searches manifest selectors, root constructors/reference enumerators, inline closures, and pin arrays. The earlier audit's manifest-selection claim remains valid; it was not a literal inventory of every graph check or pin list. The previously omitted sites are classified here:

| Site | Meaning |
| --- | --- |
| `bundle.rs::reconcile_destination`, inline branch/input/manifest closure | Read-only reachability check on the loaded root. The root is adopted whole and committed through complete root references; this list does not construct a root. |
| `bundle_commit.rs`, prior-root checkpoint dependency list | Uses the prior root's complete `references()` when admitting the checkpoint, separately from the new root's selected-manifest roots. |
| `durable_branch.rs`, frozen base/current checkpoint pin arrays | Keep branch checkpoint closures for task/session lifetime. They are not manifest selectors. |
| `editing_session/durable.rs`, base/current/frozen checkpoint pins | S4's registered candidate and specialist lifetimes retain the corresponding branch closures. Candidate publication still reaches the common index-preserving publisher. |

The table plus the inventory distinguishes manifest selection from read-only graph checks and lifetime pins; it does not claim that every array literal is a new root producer. The C0 still/sealing items and H3-owned C1-M2, C1-M4 and C1-M5 remain with their assigned stages.
