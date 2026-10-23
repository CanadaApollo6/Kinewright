# JA-1 S2 C1: publication phase, cleanup and fresh-open admission

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
