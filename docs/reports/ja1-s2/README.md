# JA-1 S2 implementation reports

## Review fix round

The original C6 claim that Windows carried only raw/native-refusal applicability was false
for `d01d2ff`; it is withdrawn as evidence for that head. Parent `c39e613` fixed the unsafe
fixture calls. This round audits every caller and retains native Windows runtime as NOT RUN.

Fix 2 on c39e613 addresses every accepted S2-R review finding in one unpushed commit.
The [C6 report](c6.md#review-fix-round) contains the exact source sites, witnesses,
raw results, binary/source identities and limitations. Final Linux: 617 expected/selected,
607 passes and 10 ignored selection outcomes; complete workspace
`--no-fail-fast`: 3935 passes, 0 failures, 107 expected ignores.
Both affected slow selectors execute. All 95 original JA files remain unchanged; no new S0 flip.

| Finding | Fix and owning site | Witness | Red/green evidence |
| --- | --- | --- | --- |
| A1 / O2; ruling 1 | Publisher returns exact committed history SHA, including ordinary floors; first Save, Save and Save As use it. `crates/kinewright-project/src/bundle_commit.rs:1059`; `crates/kinewright-project/src/editing_session.rs:511`; `crates/kinewright-project/src/editing_session/recovery.rs:330` | Four owner suffix/floor witnesses `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:115` and actual worker/Close `crates/kinewright-app/src/ja1_attachment_tests.rs:839` | 5 d01d2ff runtime reds → 5 final-source greens |
| A5; ruling 2 | One digest per actor/history identity; identity includes accepted no-op Undo/Redo batches as well as revision/floors. `crates/kinewright-core/src/actor.rs:48`; `crates/kinewright-project/src/editing_session.rs:436` | Actual Studio is_dirty polls across unchanged, empty Undo/Redo, gesture, Undo and Redo `crates/kinewright-app/src/ja1_attachment_tests.rs:844` | d01d2ff red → green; batch-count-omission mutation red → restored green |
| A2; ruling 3 | Failed Retry still probes/stores eligible offer, preserves error and pending-command processing. `crates/kinewright-app/src/ja1_attachment.rs:319` | Rendered button clicks: ordinary/first/prepared Retry → offer → PreviewRescue, no preinstalled choice or owner prepare `crates/kinewright-app/src/ja1_attachment_tests.rs:777` | d01d2ff red → final green |
| A3; ruling 4 | Select evidence from acknowledged manifest in both source closure and B publication. `crates/kinewright-project/src/editing_session/rescue_selection.rs:33`; `crates/kinewright-project/src/bundle_copy.rs:987` | W10 comments, landed and absent A: B manifest/content exclusion, independent evidence and UnresolvedOrigin `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:269` | 2 d01d2ff reds → 2 final greens |
| A4 / O3; ruling 5 | Stage every replacement before evidence mutation; restore original vector and new pending pins on definite publish failure. `crates/kinewright-project/src/editing_session/recovery.rs:597` | Staging failure and publish refusal → later Save → independent reopen; strict full-record comparison and successful edge-retirement control `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:322` | d01d2ff red → final green |
| O4; ruling 5 | Retain existing branch rollback; add missing discriminating witness. `crates/kinewright-project/src/editing_session/recovery.rs:566` | Definite prepublication refusal → later Save/reopen preserves original branch/head `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:366` | Correct d01d2ff control green; drop(removed) red at d01d2ff and final source; restored greens |
| O1; ruling 6 | Parent c39e613 already gates every unsafe fixture/control; add Windows-safe import and full workspace --no-fail-fast. `crates/kinewright-project/tests/support/ja_s2_c6_admission.rs:2`; `docs/reports/ja1-s2/c6-gate.sh:97` | All S2 read/inspect/builders and four matrix callers audited; portable native-refusal assertions, W14 and both stress paths. See windows-audit.md. | Linux matrix/stress and complete no-fail-fast pass; native Windows runtime pair NOT RUN |
| O5; ruling 7 | Backend refuses manual durable reopen and registered session/base targets at prepare and publication. `crates/kinewright-project/src/editing_session/durable.rs:197`; `crates/kinewright-project/src/editing_session/recovery.rs:43` | Manual duplicate, registered prepare and registration after prepare `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:388` | d01d2ff red → green; all 3 guard removals red → restored green |
| O6; ruling 8 | Honest panic text: attachment/retained data were dropped; reopen storage, unsaved work may be lost. `crates/kinewright-app/src/app.rs:982` | Actual panicking worker join/status `crates/kinewright-app/src/ja1_attachment_tests.rs:849` | d01d2ff red → final green |
| O7; ruling 9 | Seal A before resuming B; failed A seal retains B Frozen. `crates/kinewright-project/src/editing_session/rescue.rs:293` | Authorized cut interference makes A seal fail; B cannot remain Live `crates/kinewright-project/tests/support/ja_s2_review_fix2.rs:419` | d01d2ff red → final green |

Fresh paired means: d01d2ff **248.081 ms/save**, fix2
**248.011 ms/save**, observed ratio **0.9997x**;
zero history decodes on both sides. This is an observation, not a causal speedup claim.

Windows fixture applicability is source-audited, including all four shared-matrix callers;
actual native refusal is positively asserted by their non-Linux branches. Native Windows
runtime red/green remains NOT RUN locally. Installed cross-checks retain their actual
PASS/BLOCKED results in C6's report; missing native headers do not earn a pass.
The corrected complete gate supersedes an intentionally interrupted attempt using a history
identity that missed empty Undo/Redo. O4's correct baseline passes and only its exact rollback
mutation should fail. No historical acceptance is relabeled, no C7 work and no push.

## Earlier implementation evidence

C6 adds the independently usable local S2-R App recovery/rescue controls over the same
C4/C5 lifecycle owner, plus local admission witnesses. The [C6 report](c6.md) records 65
passing Linux command rows, 603 expected/selected outcomes, 593 executed passes and ten
ignored selection outcomes (the new slow App test executes separately). Full workspace
fast passes: 3,922 passes, zero failures and 106 expected ignores. Both named slow additions
execute, and all 126 runtime mutation red/restored-green pairs bind to the gate's exact source.
Only the additive App fixture receives pending-baseline to revised-pass credit; all 92 prior
fixture/oracle files and 31 old index entries remain unchanged.

Five alternating lanes per revision measure C5 at 235.429 ms/save and C6 at 230.418 ms/save
(0.9787x), beside archived C5's 230.9 ms. Both sides retain zero history decodes and the same
135,717,260-byte decoded-budget peak; sampled other-process CPU never exceeds 20 percent.
The ratio is an observed paired result, not a causal comparison to the archived run.
Three installed-target MSVC checks are blocked by missing Windows C headers (`stdio.h` /
`errno.h`); native Windows, exact-SHA hosted full slow CI and the lead's two independent
S2-R data-loss reviews remain NOT RUN locally. C6 stops before C7. A selected point retained
only by this session's pin can still be lost in the disclosed crash window after rotation.

C5 composes local recovery, rescue publication/handoff and durable-session registration
under `Ja1EditingSession`. The [C5 report](c5.md) records 58 passing Linux command rows,
563 expected/selected outcomes, 554 executed passes, nine existing ignores marked NOT RUN,
and all 68 runtime mutation red/restored-green pairs. The complete workspace fast tier
also passed: 3,885 passes, zero failures and 104 expected ignores. Only the additive rescue
publication property receives S0 credit; prior fixture/oracle bytes and index entries remain.

Five alternating lanes per revision measured C4 at 232.287 ms/save and C5 at
230.925 ms/save (0.9941x), beside archived C4's 333.4 ms. Both fresh sides retain zero
history decodes; no other process exceeded 20 percent CPU in the sampled lanes. This is
an observed paired ratio, not a causal comparison with the archived run. Project/agent
MSVC cross-checks both stopped in native dependencies at missing Windows `errno.h`.
Native Windows/hosted CI, full slow and physical-device acceptance remain NOT RUN.
C5 stops before App/C6+ integration. The session-pin crash window remains an explicitly
disclosed accepted limit.

C4 added rescue records, strict L1 restore, selected-target session pins and frozen local
selection, with shared ordinary branch/floor lineage identity. The [C4 report](c4.md)
contains all raw summaries, 48 final-source mutation pairs, additive S0 evidence and lane limits.
Its local final Linux gate had 539 selected outcomes, 530 executed passes,
zero failures and 9 existing ignores marked NOT RUN. C4's measured save mean is
333.4 ms beside archived C3's 225.0 ms and contemporaneous C3's 317.2 ms;
all alternating lanes saw other load. Both builds retain zero history decodes and the same
decoded-charge peak. That local report recorded a passing Core MSVC check and blocked
project/agent native prerequisites; native Windows/CI was NOT RUN in that local report.
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

## C5 integration details and deviations

C5 keeps C4's single selection engine and the shared ordinary publisher/collector. No
App/C6+ implementation, P, editable pause, Continue, special headroom or new ledger is added.
The complete report records these integration refinements with their runtime witnesses:

- A retained copy's later unknown Save must refresh its attempt status instead of returning
  its original committed-copy status (`crates/kinewright-project/src/bundle_copy.rs:123`).
- Content-only rescue traversal explicitly adds recovery roles; target record profiles are
  admitted before destination mutation, so archival inventories cannot reintroduce an
  omitted root (`crates/kinewright-project/src/editing_session/rescue_selection.rs:177`).
- Context Close fences actors before ending source reuse. Actual readers retain ownership;
  teardown proves no destination absent (`crates/kinewright-project/src/editing_session.rs:701`).
- Durable creation/reopen must allocate private actors before exposing registered handles
  (`crates/kinewright-project/src/editing_session/durable.rs:98`). Guards route through their
  original namespace epoch after rescue (`crates/kinewright-project/src/editing_session/durable.rs:36`);
  they cannot write B or block its independent
  branch copy.
- The full specialist proposal and bounded checkpoint publish atomically. Charged canonical
  cuts preserve the original prefix through exact floor settlement, accepted suffixes and
  stale guard clones (`crates/kinewright-project/src/editing_session/durable.rs:292`,
  `crates/kinewright-project/src/bundle_branch.rs:157`). This reuses Core's existing replay.
- Late A may settle only its exact original live registered actor's ticket; sealed manual A
  and never-active captures stay status-only (`crates/kinewright-project/src/editing_session.rs:426`).

The pressure witness calibrates its exact-fit limit on the same writer's actual root
(`crates/kinewright-project/tests/support/ja_s2_c5_pressure.rs:165`), rather than assuming
random BranchId ordering in another fixture yields an identical byte peak. The frozen
failure witness includes an actual missing-parent creation failure
(`crates/kinewright-project/tests/support/ja_s2_c5_recovery.rs:524`),
and the recopy witness independently asserts the published R_ack edge
(`crates/kinewright-project/tests/support/ja_s2_c5_rescue.rs:542`). Earlier weak/compile-only
mutation attempts and development gates
are excluded; `mutations-sealed/` contains the complete final 68-pair unchanged-source run.

## C6 integration details and deviations

- `crates/kinewright-project/src/editing_session/recovery.rs:55`: The existing C5 selection engine accepts a pre-seal App preparation callback. Subscription/snapshot failures occur before publication/sealing; final attachment only moves the prepared view.
- `crates/kinewright-project/src/editing_session/rescue.rs:40`: Rescue uses the same callback seam before any destination mutation. There is no second recovery engine or host/stream implementation.
- `crates/kinewright-project/src/editing_session.rs:354`: Never-active captured destinations are distinguished from retained source A. Only a proven capture owner can be explicitly released; source holds, active editing and slot accounting remain.
- `crates/kinewright-project/src/bundle_copy.rs:788`: A successful temporary-store creation now has an actual event through the existing shipping-safe note wrapper. The positive control observes the physical directory and its cleanup; create-then-drop no longer escapes the W14 zero-mutation oracle.
- `crates/kinewright-project/tests/support/ja_s2_c6_admission.rs:164`: The shared matrix retains three Linux save/reopen controls and nine typed graph/API refusals per caller. Windows namespace inspection precedes graph validation, so all twelve cases positively assert native `DurabilityUnsupported` with zero mutation.
- `crates/kinewright-app/src/ja1_attachment_tests.rs:16`: Unchanged independent C5 scenario assertions run through an App action adapter, including actual retry/reconcile/trim/omission/retirement choices. Test-only consent-bit observation translates existing scenario consent into actual UI controls.
- `crates/kinewright-app/src/app.rs:16182`: W8 uses real App New Project/focus-return and status closure, with actor/charge/slot/source-lock assertions and a drop-owner-on-focus mutation.
- `crates/kinewright-app/src/app.rs:1604`: The legacy read-only inspection flag no longer disables Save after explicit Studio activation; Studio retains its own save/read-only guards.
- `scripts/ja1-s2-c6-mutations.py:27`: The backend duplicate-retirement mutation stays at the project witness because App preview already rejects the missing retired branch. App duplicate/refusal behavior is independently retained, without claiming that unreachable backend mutation as App evidence.
- `scripts/ja1-s2-c6-mutations.py:119`: Each restored original test binary is built once, source/profile/SHA-bound and sealed outside target, then actually executes every exact green. Every red is independently compiled and run; the additive App fixture also rebuilds and hashes its mutant child executable.

S2-R has no real App durable-branch session creation route. C6 claims applicable offers,
refusals and rendered blocker/save/close text only; C5 project/agent registration and later
C11/C12 host repeats retain their own attribution. The original C5 dirty-Save-before-Save-As
contract remains, with explicit successor/agent-effect omission confirmation. The in-session
selected-target crash window is accepted and disclosed. Historical acceptance is unchanged.

## CI fix round 1

[The C6 report](c6.md#ci-fix-round-1) records the follow-up on `d01d2ff`: Windows fixture
loads and graph-error expectations now respect native refusal, including the separate
headless setup and both stress routes. The App fixture is explicitly on-demand, allowlisted
on Linux, and actually executed with the built App binary by the C6 gate and mutation runner.
The gate removes ambient harness overrides and runs workspace fast without its fixture variables.

Fresh Linux evidence under `.review/ja/s2-evidence/c6/fix1/`: 66 passing command rows;
603 selected outcomes, 593 executed passes and 10 explicit ignores; full workspace fast
3,921 passes and 107 ignores; clean C6 environment 94 passes plus one on-demand NOT RUN;
six targeted runtime red/restored-green pairs. Native MSVC checks remain blocked at missing
Windows `errno.h`; native Windows and hosted slow acceptance remain NOT RUN. Paired means
are C5 230.294 and fix1 230.469 ms/save (1.0008x), beside archived C5 230.9 ms/save.
Production code, normative contracts and all 95 JA fixture files are unchanged. The original
C6 record remains archived evidence; superseded/interrupted fix runs earn no acceptance.

## C7 writer and journal identity

[C7 report](c7.md) and [rerunnable C7 gate](c7-gate.sh). C7 only on `4952851`,
branch `ja1/s2`; one local commit, no push. Source/SHA binding and raw evidence are under
`.review/ja/s2-evidence/c7/`.

Final Linux gate: 67 passing command rows, 29 nonempty selections, 630 selected,
620 executed passes and 10 explicit ignores. Complete workspace fast with
`--no-fail-fast`: 3,948 passed, 107 ignored, zero failed; verifier passed. Final runtime
mutations: 18 semantic mutations, 20 red/restored-green pairs, exact source restoration.
Both additive S0 properties were literal parent reds after successful controls and
pass on current source in both modes. `participants-lock` stays positive; 94 old fixture
files and 32 old index rows are unchanged.

Paired means (five alternating runs per source, 60 saves each): pristine `4952851`
343.319 ms/save; C7 355.802 ms/save (+3.64%). Ranges overlap; exact rank-sum p=0.6905,
so the timing comparison is inconclusive at this sample size. Every measured save
committed with zero history decodes. Shared workstation load is retained, not gated.
The installed MSVC cross-check was attempted for all four touched crates/all targets:
blocked in native dependencies, including missing Windows `errno.h`. Native Windows
and exact-SHA hosted slow lanes remain NOT RUN for the lead. No workflow changed.

Implementation details and deviations:

- `docs/JA1-STUDIO-ORCHESTRATION.md:181`: Tracked §5 states real legacy/GUI authority, native IO parents, queued lifetime, exact startup decision identity and suffix/release obligations. Host remains C8.
- `docs/JA-S0-CONTRACTS.md:316`: Tracked §4.1 records the same local G4 ownership/handoff contract and preserves historical S0 fallback evidence. A1 is unchanged.
- `crates/kinewright-project/src/legacy_io.rs:11`: The existing Studio namespace refuses Windows durable activation. A narrow legacy directory capability preserves the separate legacy IO profile: Linux fd paths; Windows ancestor deny-delete pins and reparse refusal. This is the plan’s namespace-owned legacy IO, not a second bundle publisher.
- `crates/kinewright-media/src/lut_store.rs:583`: Existing LUT validation/copy logic accepts pinned source/destination luts directories; needed to preserve asset identity without duplicating the copier.
- `crates/kinewright-project/src/sidecar.rs:484`: Queued jobs retain the actual shared lock; joined acknowledgements drop the job capability before waking the caller.
- `crates/kinewright-app/src/recovery.rs:1186`: Explicit close joins the recorder and retires its journal before relinquishing authority. Closed objects clear their old path so Drop cannot delete a successor journal.
- `crates/kinewright-app/src/app.rs:1834`: A restored copy may be suspended while the original project owns the writer. Retirement routes to that real owner, using the scanned pending decision.
- `crates/kinewright-app/src/recovery.rs:1196`: Eight legacy tests constructed private RestoreRequests directly. Their setup now captures the real startup scan before requesting restore; every existing assertion remains.
- `crates/kinewright-app/src/project.rs:1903`: Legacy reopen setup now performs actual close. The second corrupt-sidecar injection happens after close; the stale-write test shares the real authority and closes its reopened session before reopening the original.
- `crates/kinewright-project/src/aw1_race_tests.rs:1194`: Historical ignored defect assertion preserved; classification alone changes, with the existing allowlist row updated.
- `crates/kinewright-project/tests/ja_s2_writer.rs:113`: The actual save child loads the owner-resolved stem and real project digest, asserts readiness, classifies saved versus typed LockLost, and captures stderr privately. This removes precondition/formatting false results without deleting graph assertions.
- `scripts/ja1-s2-c7-mutations.py:56`: The alias mutant retains the new sidecar capability while restoring only caller-path project IO; its validator requires the foreign-graph assertion and both fixture modes with successful controls.

The historical post-scan defect retains its assertions and ignore. The output-format
repair captures child stderr; the strengthened alias setup uses the resolved sidecar
stem, actual digest and typed outcomes. Earlier weak/development results are excluded.
Eight direct RestoreRequest tests now capture the real startup scan; existing assertions
remain. Opus r2 N2 at `crates/kinewright-project/src/editing_session/rescue.rs:293`, `:318-323` is deferred to C12:
C7 leaves that file unchanged. No C8 host/auth/roots/manual-stream implementation or
independent review round was started. Only report/README delivery notes follow testing.
