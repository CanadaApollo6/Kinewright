# JA-1 S2 implementation reports

C4 adds rescue records, strict L1 restore, selected-target session pins and frozen local
selection, with shared ordinary branch/floor lineage identity. The [C4 report](c4.md)
contains all raw summaries, 48 final-source mutation pairs, additive S0 evidence and lane limits.
Its final Linux gate has 539 selected outcomes, 530 executed passes,
zero failures and 9 existing ignores marked NOT RUN. C4's measured save mean is
333.4 ms beside archived C3's 225.0 ms and contemporaneous C3's 317.2 ms;
all alternating lanes saw other load. Both builds retain zero history decodes and the same
decoded-charge peak. Core MSVC check passes; project/agent native prerequisites block their
cross-checks. Native Windows/CI remains NOT RUN. C5 lifecycle/rescue publication is not started.
The selected point's session-pin crash window remains an explicitly disclosed accepted limit.

C3 adds project-local lifecycle ownership, retained read admission, structured probes,
read-only namespace separation, rescue-slot accounting and non-consuming A writer reuse.
The [C3 report](c3.md) records its exact scope, owner table, gates, mutation pairs and lane
limits. C3 keeps first-save absent reconciliation's staged graph under the retained writer
instead of ordinary copy rollback so L3 can save again through the same namespace.
The implementation is at `crates/kinewright-project/src/bundle_copy.rs:97` and
`crates/kinewright-project/src/editing_session.rs:359`. Rescue publication remains C5.

C3's final Linux gate passed all 51 command rows: 525 selected outcomes, 516 executed
passes, zero failures and nine existing ignores marked NOT RUN. All 47 mutation
red/restored-green pairs passed with complete source restoration. Only two additive
S0 fixtures receive pending-baseline to revised-pass credit; all 88 previous fixture/oracle
files and 27 previous index entries are unchanged. The sealed 12-save rerun averaged
224.993 ms beside C2's recorded 250.8 ms, with zero history decodes. This is one C3 run,
not an interleaved speedup result. Installed Windows project/agent checks both stopped
in native dependencies at missing Windows C headers; native C3 Windows/CI is NOT RUN.

C1 only, based on main `cb4f6e2586974fbd7da1ba935e7c4e77390aeca0`. No C2 actor gates,
rescue lifecycle, host, UI recovery or new persistent format/certificate is introduced.

## Implementation

Every production root publication passes `validate_publication_reopen_fit` at the shared
store boundary before root replacement. Ordinary Save, branch Save, fork, first Save,
Save As and evidence/receipt/floor updates reach that boundary. The reader and publisher
use one bounded traversal under the effective downward-only `StorageLimits`, with exact
history-hash sharing, all current BranchIndex histories, archive/recovery validation,
decoded/replay overlap and physical metadata/owned closure accounting. The publisher's
empty-reader model is operation-local; it never adds hypothetical histories to the
writer's physical ledger. Actual write/shadow/copy reservations remain independent.
The normative rule is tracked in JA-1 §6.1 and S0 §4.3.

S1's sparse definite-success Save As cache was confirmed in source and is exercised
without reconciliation or reopening that writer. The witness independently reopens
successful exact-fit publications, rejects one-byte-over candidates, and compares the
cheapest legal target floor plus every other distinct current history against the limit.
It asserts actual writer overlap fits and that fresh-open admission was reached. The
target-only mutation must publish, then fail the independent reader oracle; corruption,
missing support or an earlier capacity refusal cannot earn that red.

Publication phase is distinct from cleanup/Core settlement. A real post-B5 Core
`HistoryCommitInvariant` remains committed-with-error through ordinary Save and copy
wrappers. Definite staging/root refusal with uncertain temporary cleanup is a failed save
with cleanup pending. Pending root temporaries retain their exact independent charge;
non-permission rename errors retain their name for retry. Owner tags appear after
fallible floor-tag preparation. Ordinary copy to fresh B can read the complete
acknowledged graph through A's retained source owner after definite prepublication
failure and cleanup uncertainty, without reconciling an unavailable A. A keeps cleanup
ownership/charges. Missing owned bytes refuse a partial B. Actual unknown-root A still
blocks another ordinary copy. This is no rescue action or rescue-retention slot.

## Verification and evidence

The exact affected-set gate is `scripts/ja1-s2-c1-gate.sh`; its libtest list/selection/run
checks reject empty applicable selectors and compare names, counts and raw summaries.
It prepares the existing sealed max-2 predecessor prerequisite in this worktree when absent,
and supplies the actual built App test executable for all three on-demand App bridge tests.
`scripts/ja1-s2-c1-mutations.sh` runs targeted runtime red/restored-green pairs and restores
original bytes even on failure. Its target-only mutation changes publisher residency
charges alone, retaining structural/hash/edge validation and independent reader charges.

Local evidence is under
`/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/c1/`.
Logs and source/fixture/binary identity manifests remain there and are not committed.
The additive `s2-save-phase-cleanup.json` observes real production outcomes in both modes.
Original S0/S1 fixtures, oracle bytes and old index entries are preserved. There is no
new historical baseline credit for unavailable rescue/host surfaces; the original legacy
IO contradiction remains. Exact-byte fixtures already have `-text` across the fixture tree.

[The C1 verification report](c1.md) records the complete enumerated selections, mutation red/restored-green pairs, paired A/B
measurements, source identities, additive S0 dispositions and the Windows cross-check.
A skipped or NOT RUN lane is never a pass. Native Windows/CI, physical storage
crash/removal and the full hosted slow tier are NOT RUN locally. The lead gates the single
C1 SHA before C2.

## Windows timing

The existing cached Windows job now links the workspace and the two no-default shipping
selections, checking each native exit and recording each command's elapsed seconds.
[Timing schema and timeout decision](windows-timing.md) describe the SHA-bound artifact.
No numeric native time is available locally. The 60-minute timeout is retained for the
lead's measured comparison; C1 does not invent a forecast or silently increase it.

The installed `x86_64-pc-windows-msvc` target was attempted separately for project and
agent; both exited 101 in native dependency build scripts before checking those crates.
FFmpeg bindgen could not find the Windows C runtime's `errno.h`; Whisper also reported
missing `stdio.h`, and aws-lc could not build with the available C compiler. Cross-check
is blocked, not passed. Native Windows builds/clippy/fast/refusal execution and measured
added CI minutes remain NOT RUN. No toolchains or Windows SDK were installed.

## Deviations

C4 also touches Core's actor admission, beyond the C4 project/agent file inventory.
At C3, `crates/kinewright-core/src/actor.rs:1124-1138` rejected Sealed restores but allowed
a bound owner to install a checkpoint/floor while Frozen. C4's local-selection witness
reached that real mutation during unknown selection. The smallest faithful fix rejects
RestoreHistory while Frozen inside the actor; exact-ticket settlement remains separate.
The C4 report records its mutation pair and the required CORE/MEDIA-PINS gate results.
This implements C4's explicit frozen save/floor contract rather than deferring the hole.


No functional scope expansion is intended. The common brief's historical design-only
role is superseded by the explicit C1 implementation task. The task-specific `nice -n 10`
Cargo rule supersedes PITFALLS' standing `nice -n 19`; Python runs at `nice -n 19 -I`.

S1 fork initializes a fresh history at the source's current document
(`crates/kinewright-project/src/bundle_branch.rs:129`), so C1's ordinary fork/branch controls use that real
baseline. Shared-hash controls compare independently created matching branch histories;
they do not assume a fork copies the source's Undo batches. No fork semantics change is
included. C1's existing cleanup regression assertion now expects `CleanupPending`, as
required by deferred Minor #4; its preservation/charge/continuation oracles stay intact.

The old inventory-retirement control used an 8 MiB metadata profile while leaving
64 MiB decode scratch at its default (`crates/kinewright-project/tests/support/ja_s1_fix_r2.rs:475`).
C1 correctly threads that profile through fresh open, so the control now lowers decode
scratch to 4 MiB as well. Its 16 MiB inventory-only LUT still exceeds the 8 MiB metadata
limit and all byte-retention and inventory-before-unlink oracles remain unchanged. This
is a test-profile correction, not a relaxation of reader/publisher admission.

The shared reader now applies `control_bytes` to root checkpoints, `branch_history_bytes`
to histories and `decoded_record_bytes` to other metadata records, matching store
admission (`crates/kinewright-project/src/bundle_recovery.rs:239`). Threading a lowered
control profile exposed the old blanket control cap on immutable metadata. The original
exact-control-cap storage test now passes with its unchanged exact/one-byte-under oracle.

The two existing object-staging uncertainty assertions
(`crates/kinewright-project/tests/support/ja_s1_fix_r5.rs:139`, `:172`) now require
`CleanupPending`; their post-rename unknown-publication cases still require
`DurabilityUnknown`. The root-staging assertion in `ja_s1_fix_r6.rs:122` receives the same
required wording/status correction. These changes implement Minor #4.

The comment test keeps its original zero-history-decode oracle
(`crates/kinewright-project/tests/ja_s1_receipts.rs`), including unchanged saves after Open
and reconciliation. The publication model now obtains payload charges from a bounded,
in-session verified-identity memo in `bundle_recovery/publication.rs`. Its keys include the
full ObjectRef (hash, kind and length) and the entire effective limit profile. It holds
costs, raw support fields and dependency identities; no histories, persistent ledger or
new reservation. Open/reconciliation fully verify and seed the memo. Publication records
already-decoded/verified objects; a miss takes the reader's exact decode/preflight path.
Namespace continuity failure and explicit byte re-verification invalidate affected facts.
Current membership still comes only from the candidate BranchIndex. Shared checkpoint
ordering, physical closure accounting, edge validation and decode/shadow reservation rules
preserve exact fresh-reader peaks. Typed traversal controls are interpreted each time.
The paired storage and incremental-save measurements are recorded in [c1.md](c1.md).

The post-B5 copy witness also models deletion/restoration of its own committed root.
Reconciliation refuses the missing committed root while preserving committed-with-error,
then succeeds after restoration. This is a modeled fault, not physical-device evidence.

## C1 fix1 verification disposition

The fixed implementation passed the complete Linux C1 gate in `fix1/gate2/`: 43 command
rows, 466 selected outcomes, 460 passed and six existing ignored outcomes. The 22 mutation
pairs and their raw summaries are recorded in [c1.md](c1.md). The final clean-commit rerun
is retained in `fix1/gate-final-head/` and bound in `fix1/commit-binding.json`.

The first fix1 gate failed during integration-test compilation because a shared Cargo
target retained the temporary predecessor's project API. It is excluded from acceptance.
Current-head gates/mutations now use the worktree-private `target/c1-head-build`; no baseline
build uses that root. Source files and evidence remained intact. Native Windows cross-checks
were attempted for project and agent and both stopped in FFmpeg bindgen at missing Windows
`errno.h`. They are blocked, not passed; native CI and Windows timing remain NOT RUN.

The sealed paired A/B observations on this machine were `ja_s1_storage` 97.507 s at
`cb4f6e2` versus 109.149 s at amended C1 (1.119x), both 86/86 passing. Twelve small
comment saves on six populated branches took 2.423636 s versus 2.919871 s (1.205x);
mean save 201.970 versus 243.323 ms. Per-save maximum decoded charges were
135,717,260 versus 135,717,260 bytes, with zero history decodes on both revisions.
C1 publication admission accounted for 41.769 ms/save. Whole benchmark process wall
was 16.689 versus 21.189 s (1.270x), including setup and independent Open.
The workload ratios were within the requested ~1.5x target in this pair. Other CPU load
was sampled and remains an annotation, not a gating condition or a causal speedup claim.

The baseline storage executable was built from clean `cb4f6e2`; the focused benchmark
uses byte-identical harness source and only test counter adapters on that predecessor.
The accepted identical runner uses isolated Python for both parent and timer, monotonic
time and Linux `wait4` resource counters. Its start/end CPU/GPU and 5-second samples,
raw measurements and source/binary identities are retained in `fix1/`. The implementation
measured at `ab69312ef41a22565d26a413a5a5d33d689e6d6d` is unchanged by the
final report-only amendment; the final binding verifies that identity. The absent-time
setup failure, earlier timer without `-I`, and deliberately stopped partial gate are
retained and excluded from final acceptance.


## C2: actor attachment and destination settlement

[C2 verification report](c2.md) records the eager proof binding, actor-side mutation/freeze/seal gates,
exact original actor/ticket/tag destination settlement, and the separate authorized ordinary install.
C2 only is implemented on parent `ea4d719`; C3/lifecycle, rescue publication and App handoff remain
later work. The preceding C1 report is retained as historical evidence.

The final Linux gate completed 50 command rows: 505 selected outcomes, 496 executed passes,
zero failures and nine existing ignored outcomes explicitly NOT RUN. All 31 targeted mutation
red/restored-green pairs passed with complete source restoration. All 87 original fixture files
and 26 original index entries remain unchanged; only additive `s2-core-seal.json` receives C2
actor-level baseline-red/revised-pass credit. Native Windows/CI on C2 is NOT RUN. The installed
Windows target checks Core/all-targets successfully; project/agent/App are blocked in native
C dependency build scripts (missing Windows errno.h / incompatible compiler pthread types).

Deviations and gate discoveries, with source evidence: typed Event required the exhaustive App
router/drain (`app.rs:2286`, `:2856`), recovery-test replay (`recovery.rs:1107`) and performance
example (`performance_workloads.rs:569`) updates. Bound setup/install callers use the authorized
capability; standalone unbound setup remains available. A runtime-only previous-root SHA binds
retained older-root release; it adds no persisted format. Reconciliation records known commit
before Core settlement (`bundle.rs:1221`) and carries copy committed-with-error (`bundle_copy.rs:202`).
The unchanged S1 vanished-root oracle caught C2 retaining its ticket too long: `bundle.rs:1242`
now permits only exact lossless release under continuity while preserving the owner's missing-root
refusal; `:1553` distinguishes absence from a different nonempty root. No assertion was weakened
and no global fetch-current-ticket fallback returned. A transient existing lock-race fairness
failure passed its exact rerun and final full library selection, with no lock change. Native FFmpeg
stderr interleaving required separate gate stream capture (`ja1-s2-c2-gate.sh:29`); the complete
gate reran successfully. The sealed max-2 predecessor was SHA-verified and its pinned source
checkout recreated; its controls ran fresh, without claiming a new predecessor build.

Fresh paired small-save means were 251.841 ms at C1 and 250.843 ms at C2 (0.9960x), next to C1's
recorded 243.323 ms. Both fresh lanes had zero history decodes; peak production decoded charges
were 135,717,260 and 135,717,257 bytes. Other desktop CPU load was observed in both lanes.
The report retains all raw summaries, exact CoreMessage/handle audit, NOT RUN lanes and source
binding. Evidence is under `.review/ja/s2-evidence/c2/`; no logs or binaries are committed.

C3 returns a private owned retained-read lease at
`crates/kinewright-project/src/store.rs:115` so an issued reader keeps the real pin
and charge across context Close. Native exact-ticket IO errors remain typed at
`crates/kinewright-project/src/bundle.rs:1603`. These small ownership and diagnostic
adjustments are documented as C3 plan details in the report.

The C3 older-root first-save witness uses a compatible independently admitted baseline
at `crates/kinewright-project/tests/support/ja_s2_c3.rs:1156`. Unrelated Studio histories
correctly fail the existing continuation guard; that production rule remains intact.
