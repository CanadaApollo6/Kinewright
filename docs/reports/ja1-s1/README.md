# JA1 S1 implementation evidence

Worktree: `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`.
Baseline: `076a96c922b2f05984df4bb839ad7cf0900e6183`.
Authority: accepted `target/review/ja/ja1-s1-plan-rev2.md` and implementation brief.
No push or independent review is authorized by this task.

The eight implementation steps are complete. No release or native Windows acceptance
is claimed by this file.
The eight commits retain the accepted order. Each commit requires the build,
clippy, touched-file formatting and exact affected-test gate before publication.
`scripts/ja-s1-gate.sh` is the rerunnable Linux gate; raw logs live outside Git
under `target/review/ja/s1-evidence/` in the main checkout and are compressed.
Each log records its source HEAD and modified/untracked source identities.

Windows native tests and Windows durability are distinct lanes. Installed targets
are `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu`. A compile check is not
native Windows execution. S6 owns native Windows storage-crash certification.
Decoder-open certification remains L4's separate integration dependency.
Storage limits remain provisional until L6's workload measurements.


## Fix round 1

The current corrections and complete A1–A7, O1–O28, L1/L2 disposition are in
[fix-r1.md](fix-r1.md). It supersedes the historical implementation notes below
where behavior or validation changed. Fresh paired measurements are in
[r1-storage-measurements.json](r1-storage-measurements.json); the current numerical
interpretation is in [JA-S1-STORAGE-EVIDENCE.md](../../JA-S1-STORAGE-EVIDENCE.md).
The complete local stricter Linux gate passed, including all five full crate
targets and the workspace fast tier with TMPDIR unset on tmpfs; classification
lint and final workspace clippy passed. MSVC Core lib/tests compile passed; the
project target remains blocked by native dependencies. L6 remains open for the
lead and its 250 ms target is unchanged.

O11 is deferred: the latent legacy writer authority race belongs to the separate
legacy authorization path, outside this durable-bundle correction.
O26 is deferred: recovery of a deleted registered predecessor checkout needs
separate worktree lifecycle handling; this task retains that existing checkout.

Session storage requires a caller-selected durable parent and refuses tmpfs before
mutation. S1's desktop unsaved session remains in memory; S2 owns GUI Studio
attachment. No native Windows runtime, hosted CI, full slow tier, L4 decoder
certification or physical power-fault result is claimed.

### Fix-round implementation adjustments

History capacity is handled by the existing ordered floor, with the baseline
execution shared through `kinewright-core/src/actor.rs:546` and the persistence
owner choosing a bounded suffix in `kinewright-project/src/bundle_commit.rs:820`.
Core exports the accepted stream; the durable command owner installs the saved
floor after acknowledgment. This avoids dropping unacknowledged provenance.
The decode reservation uses encoded-format structure (`store.rs:1420`) and the
separate allocator calibration in the current measurement report.

Collection is explicit after acknowledgment (`bundle.rs:1031`) and retried
before the next save. B5 keeps a conservative old/new inventory union until
maintenance publishes the selected inventory and then unlinks. This supersedes
the old exact-at-B5 inventory note below without adding another cleanup owner.
Recovery validates archived receipt checkpoints one at a time and caches only
active branch histories, so cumulative archives do not exhaust live decode quota.

The stricter gate now includes the actual five legacy app regressions via
the full app target and the later full workspace gate. The two eval tests split
portable v3 refusal from portable admitted-legacy rendering; neither assertion was
removed. Windows remains refusal-only for Studio and needs native CI. The first full
media run exposed AU6's existing ban on whole-source OS gates in eval; the
redundant Linux-only attribute on the legacy test was removed. Both rerender
tests are portable, and the per-commit gate explicitly runs the AU6 inventory
witness. The failed preliminary attempt is retained separately.

## Historical implementation record

## Commit ledger

1. Revised routing and Core history: Linux gate passed (`c1-gate-attempt6`);
   five history and two adapter mutation red/restored-green pairs passed. Exact
   raw summaries are in `commit-1.json`. The three ignored native bridge cases
   were each explicitly executed and passed. No durable S0 flip is claimed.
2. Namespace authority and strict IO: Linux gate passed (`c2-gate-attempt4`),
   with nine targeted namespace mutation red/restored-green pairs. The same paired
   IO fixture hash has successful controls and revised directory-sync/no-fallback/
   preserved-root properties. The original contradictory IO fixture remains red.
   Full graph crash durability awaits commit 4. Exact raw summaries are in
   `commit-2.json`.
3. Bounded objects and owned inputs: Linux gate passed (`c3-gate-attempt5`),
   including ten storage tests, four revised tests and all selected media/presentation
   regressions (10/5/3 passed, no ignored pins). Ten storage mutation pairs failed
   their intended witness and passed after restoration. Exact summaries are in
   `commit-3.json`. No bundle root is declared durable at this stage.
4. V3 publication and durable history: the Linux gate passed (`c4-resume-gate7`).
   Exact summaries and predecessor identities are in `commit-4.json`. Both architecture
   scans, all harness modules, all three explicit native bridges, the ceiling controls
   and the caller/media selections passed. Known ignored pins remain named NOT RUN.
   Fourteen targeted mutation red/restored-green pairs passed at the final source.
5. Command receipts and issuer retirement: Linux gate passed (`c5-gate1`),
   23 commands and 17 test summaries, 101 selected passes including overlaps.
   All six project receipt witnesses and eleven revised tests passed, plus the
   complete affected harness/format sets and all three explicitly provisioned App
   bridges. Seven targeted mutation pairs failed their intended witness and passed
   after restoration. Exact raw summaries are in `commit-5.json`.
6. Durable bases, lineage and floors: Linux gate passed (`c6-gate2`),
   31 commands, 21 summaries and 112 selected passes including overlaps.
   All four normal/eval architecture scans, no-default builds/checks, six Core history
   tests, four durable branch tests, six receipt tests and all affected harness/format
   selections passed. Eleven targeted mutation pairs failed and passed after restoration.
   Exact raw summaries are in `commit-6.json`.
7. Save As and first save: Linux gate passed (`c7-gate1`), including all six
   new copy witnesses and twelve revised fixtures. Ten targeted mutation pairs
   failed at runtime and passed after restoration. Exact raw summaries are in
   `commit-7.json`.
8. Recovery retention and measured evidence: Linux gate passed (`c8-gate3`);
   44 commands, 31 raw summaries, 267 selected passes including overlaps.
   Exact raw summaries are in `commit-8.json`. All five required new
   storage tests and thirteen revised fixtures passed. Twelve targeted mutation
   red/restored-green pairs and a real combined-cap baseline red are preserved.

### Resume, commit 8

The five required tests remain in the `ja_s1_storage` integration target; their
implementation is factored into `tests/support/ja_s1_retention.rs`. The fresh Linux
production trace now models inventory replacement and actual `object_unlink` calls
at every dropped-write crash cut. At B5 it requires the durable journal to equal
the acknowledged manifest's recovery inventory. The portable frozen first-publication
trace remains unchanged. Process death and the crash model are separate evidence;
neither is native Windows storage certification.

Implementation adjustments and final source evidence:

- `crates/kinewright-project/src/store.rs:2149` scans and charges files absent from
  the selected manifest on writer reopen. Previously a crash could leave physical
  files outside the restored ledger. Unknown immutable bytes remain conservatively
  charged against both owned and metadata quotas, with one physical charge. A typed
  expected admission verifies incoming and stored bytes before reclassification.
  The owner refuses unprovable names/types and retains uncertain reachability.
- `crates/kinewright-project/src/namespace/linux.rs:368` gives new temporary names
  an exact root-specific prefix, so the existing namespace owner can account for
  and retire its own root temporary without scanning/deleting another project's
  stages. Managed files are enumerated through pinned directories at `:954`.
- `crates/kinewright-project/src/store.rs:265` checks the incoming object's graph
  depth before publication. Treating only its dependencies as depth-zero roots
  allowed a one-beyond object to be written before its pin failed. GC at `:816`
  removes the retired inventory-edge row and retires owned temporary charges.
- `crates/kinewright-project/tests/ja_s1_storage.rs:931` extends the existing crash
  model rather than creating a second filesystem oracle. Inventory and unlink
  fault seams exercise temporary namespaces, never physical device/power faults.
- `crates/kinewright-project/src/io_observer.rs:13` times actual Linux sync calls
  only under test instrumentation. The release example at
  `crates/kinewright-project/examples/ja_s1_storage_budget.rs:134` uses a pinned,
  dev-only DHAT allocator feature for separate actual-allocation runs. Default
  production binaries keep the system allocator. No unsafe code or Windows
  namespace dependency was added. Project no-default builds/checks and the heap
  example have explicit gates.

Twelve discriminating mutation pairs cover recursive inventory retention, all old
floors, live-pin eviction, full recovery hashing, copy overlap, double retirement,
reopen physical accounting, exact temporary deletion, prospective depth, configured
control limits and the inventory namespace barrier and combined metadata/decode admission. An initial temporary-retirement
mutation exposed a weak test: total unknown bytes could fall through adoption while
the exact root temporary survived. The test now checks the actual staged path.
Removing only the redundant publication-size guard remained protected by the root
serializer; that green mutation was rejected and replaced by ignoring the configured
control limit at its shared ingress. Neither attempt is credited as a red. The final
twelve pairs bind the strengthened source/tests and preserve red/restored-green logs.

`JA-S1-STORAGE-EVIDENCE.md` and `storage-measurements.json` distinguish actual Rust
allocator peaks, kernel RSS, physical file bytes, verification work and conservative
reservation counters. The initial five pairs were repeated after the final Linux
cfg guard so the published measurements bind the final implementation source. The
provisional comment target is reported honestly and no storage cap was increased.
Lowered-ceiling boundaries exercise actual S1 ingress. Voice-specific capture/storage
and later unsupported producers remain NOT RUN/refused under their owning stages;
the default 20 GiB and 10,000-object allowances were not physically filled.

The actual project Kani codegen boundary passed in 49.810 s at a sampled 1,823,285,248
byte process-tree peak. Zero proof harnesses ran. The two optional proof candidates
remain NOT RUN as proofs; no alternate arithmetic implementation or unproved CI
harness was added. Windows native/full MSVC dependency checks, full fast/slow CI,
L4 decoder certification and full G15/G20 remain separately NOT RUN.

### Resume, commit 7

Save As copies the acknowledged bundle through its existing store owner. First save
uses the same engine with an explicit frozen history and session input context; S2
still owns live GUI producer fencing and writer attachment. The source closure and
physical overlap remain reserved through reconciliation and handoff. Active copied
inputs receive the new project/epoch; original project/session identities remain
read-only provenance for exact archived receipts and checkpoints. Copied issuers
retire rather than granting new command authority.

The native copy fixture exercises production first-save failures, exact embedded
font and LUT bytes, retained undo/redo, the old live reader and last-reader charge
retirement. Its unavailable predecessor copy endpoint is NOT RUN, never a baseline
runtime red. Media bytes in the project copy test are a storage control, without
decoder-open certification. Ten targeted mutation pairs exercise graph omissions,
claim ordering, foreign stores, alias binding, final verification, uncertain handoff,
source pins and inventory rollback. A first mutation driver attempt encountered
nested native-child summaries; the driver now validates the final parent summary.
A mutation that disrupted participant setup was rejected and rerun against the
direct contention witness; neither setup failure nor timeout is credited as a red.

Implementation adjustments and evidence:

- `crates/kinewright-project/src/bundle_copy.rs:109` creates missing destination
  parents under a pinned ancestor claim, then binds the actual target claim before
  copying. Both authorities are retained, without a second store or cleanup owner.
- `crates/kinewright-project/src/store.rs:1896` copies the selected closure in
  dependency order and drops each temporary seal after admission. The source pin
  and frozen destination GC provide lifetime ownership without quadratic pin sets.
- `crates/kinewright-project/src/store.rs:466` distinguishes automatic recovery
  inventory traversal from content traversal. Opening/copying an archived manifest
  does not promote its retired inventory to new roots. The stronger native test
  found this defect with comments, specialist bases and several predecessor saves.
- `crates/kinewright-project/src/io_observer.rs:34` increases the test observation
  ceiling from 128 to 4096 events so the complete real copy trace fits. This changes
  no production quota or frozen S0 fixture/oracle.
- Nonempty `StoreManifest.mirrors` is refused as an inactive producer. No S1 API
  produces or verifies those alternate files; declaring them charged would be a
  false physical-storage guarantee. Current manifests and frozen fixtures use none.

Windows full cfg checks still require the unavailable native MSVC prerequisites;
native Windows, full CI and L4 decoder certification remain NOT RUN.

## Plan deviations

1. The plan names three format checks under `app::tests`, but their real module
   is `app::in2b_tests` (`crates/kinewright-app/src/app.rs:10590`, functions at
   `:11300`, `:13874`, `:14898`). The gate uses those actual full names and
   rejects missing or empty named selections. No assertion was removed.
2. Core's bounded encoded-history check promotes the already-locked
`serde_json` dependency from dev-only to production use in `kinewright-core`;
it counts serialization bytes through a capped writer without allocating a second
encoded copy. Core continues to have no project/filesystem dependency.

## Execution notes

The initial workspace build passed. An initial clippy run found a borrowed-value
lint, which was corrected. Gate retries retain their own logs. Shared machine
memory sometimes falls below the required 6 GiB available; cargo waits rather
than changing other workloads or weakening the machine rule. No skipped gate is
reported as passed.


The optional pure Core Windows target check passed. Commit 2's isolated real
stable-std Windows API module compile passed using the installed MSVC Rust target.
The full project/agent Windows cfg check was attempted and stopped in
`aws-lc-sys` because the native MSVC cross-toolchain prerequisites are unavailable.
That full lane is NOT RUN, and all native Windows witnesses/CI remain NOT RUN.
It is neither a runtime red nor a passing Windows gate. No toolchain was installed.

Commit 2 records observations adjacent to successful native file/directory sync
calls, using shared narrow sync functions so omission mutations remove the actual
barrier and its observation together. There is no claim that a trace alone is a
storage-crash model. Paired probe observations identify `linux-local-strict` even
though the unchanged compatible paired fixture's scalar protocol lane is portable.


Commit 3 uses a private exclusive session directory in the same namespace/store
implementation. It creates no named lock/discovery object for an unsaved edit, so
last-reader cleanup does not unlink a named OS-lock object. Object and decode
reservations follow their actual ownership; uncertain staged bytes remain charged.
The `StorageReservation` type also carries decoded values, avoiding a second public
lease/accounting type. The five storage witnesses live under the cfg-Linux
`bounded_store` test module; mutation selectors include that real module prefix.
These are implementation details within the planned ownership contract.

Decoded admission charges include conservative native slots and parser scratch;
they are bounds, not measurements of heap allocation or whole-process RSS. L6
remains open. Decoder-open certification remains unavailable; raw stored-byte/read
controls do not claim stability through decoder use. The new owned-input fixture
has successful revised controls. Its baseline capability refusal is NOT RUN and
is not counted as a baseline runtime red. Windows native and full Windows build/CI
remain NOT RUN with the native toolchain prerequisite recorded above.

The gate runs new storage/revised witnesses before longer unchanged harness checks,
without removing any required selection. This only changes ordering within step 4.

### Resume, commit 4

The inherited commit-4 diff built but had not passed its full gate. The resumed
run finishes its lint, caller and lane checks before committing. The predecessor
remains the existing detached `target/ja-s1-predecessor/source` checkout.

Additional plan adjustments found during verification:

- `project::predecessor_format_controls` must launch the sealed predecessor from
  its own validated source checkout. Its source reporter derives HEAD from its
  working directory; launching from the successor falsely associates the old
  executable with the successor SHA. The helper now validates and selects the
  source directory recorded by the prerequisite script.
- Core's existing `skip_json_value` treated a nested comma as a closing bracket.
  Reordered valid envelopes could therefore hide a later top-level version from
  the scanner. The format test now covers nested objects and multi-member arrays;
  commas leave depth unchanged. Without this repair, unsupported-inventory tests
  could refuse for a mistaken version rather than exercise their intended guard.
- The portable crash model replays a frozen, component-based production IO input
  on platforms without Linux publication. Linux captures fresh production calls.
  This is dropped-write model evidence; neither portable replay nor Linux process
  death certifies native Windows storage durability.
- The plan's new app refusal witness lives in the existing `app::in2b_tests`
  module, alongside the real save/session harness. The gate explicitly selects it
  and the new eval refusal witness in addition to the whole requested modules.

Evidence pointers for these adjustments are
`crates/kinewright-project/src/project.rs:630`,
`crates/kinewright-core/src/model.rs:1394`,
`crates/kinewright-project/tests/ja_s1_storage.rs:925` and `:1041`, and
`crates/kinewright-app/src/app.rs:10693`.
The native CLI integration test at
`crates/kinewright-agent/tests/ja_baseline.rs:7` now retains the baseline red and
expects the actual Linux revised history pass. The unchanged fixture hash and
artifact hashes are checked in both modes. Windows executes the baseline there;
its native durable-success properties remain NOT RUN and have separate refusal
witnesses. This updates an obsolete revised-red expectation without weakening any
baseline assertion.

### Resume, commit 5

The compatible comment baseline was sealed from clean commit 4 before receipt code.
`comment-baseline.json` records that actual run's source, binary, fixture and independent
oracle identities, successful exact-byte controls and literal missing-result red. The
executable and full run remain local evidence. CI compares that recorded baseline with
a fresh revised run; it does not pretend to rerun the old source at current HEAD.

Production receipt lookup precedes epoch/head/revision admission. Native fixture workers
preserve original request envelopes on disk and forward `retry_of`; results are always
read from the production receipt owner. The additive edit-restart fixture is separate
from the unchanged S0 reply-retry fixture. Its unavailable predecessor reopen is not
credited as another baseline runtime red.

Verification exposed three adjacent ownership defects, repaired in this commit:

- The store's owner epoch was constant. Each owner now receives a fresh epoch
  (`crates/kinewright-project/src/store.rs:110`). An archived input manifest records
  capture provenance; a new reader validates its project/entry and receives the new
  owner's epoch (`owned_inputs.rs:183`). The old admission request still refuses.
  The lost-reply test exercises actual admitted owned bytes through reopening.
- Pending publication still needs read-only identity checks to reconcile an outcome.
  `namespace/mod.rs:216` permits that read while mutation verification continues to
  refuse uncertainty. Publication freezes collection before sealing any next-root
  object (`store.rs:1461`), so an unknown root cannot lose its graph on drop.
- A restarted fixture participant has no Core yet. The real reopen path now treats
  its before-document observation as unavailable and restores from the verified bundle
  (`crates/kinewright-agent/src/ja_harness/revised.rs:189`). Existing before/after
  comparisons remain when a Core exists.

These are S1 owner/observer corrections. No GUI fencing, producer drain or transport
host is implemented, and decoder-open certification remains unavailable.

### Resume, commit 6

Durable specialist bases now persist the original `(BranchId, HeadId, document_hash)`
and full currently applied delta. The ordinary agent dependency reads those verified
records, restores the candidate and binds the target to the current Core revision.
The retained numeric-counter control actually conflicts after reopen; the durable
path merges as one entry. Equal hashes at another head refuse. Retry lookup precedes
rebinding because a committed merge has already changed the target head.

The project adds `bundle_branch.rs` for frozen branch and transition admission; the
existing `BundleCommit` still owns staging and publication. Candidate saves preserve
the manual root/document/input manifest and do not switch the manual branch. Shared
physical histories in multiple candidate checkpoints use one `Arc`-held decode charge
in the reader. The lineage replay retains baseline-reachable ancestors, then selects
only the current head's ancestry, including after undo/redo and local-ID reuse.

Core uses one validated `HistoryTransition` and the existing actor restore message
with an optional exact expected-before snapshot. Floor construction replays a closed
prefix and retained suffix through the ordinary executor; it compares the complete
remaining undo/redo cursor and document. Project admission additionally checks the
checkpoint/floor generations and exact command cut, reserves transition overlap and
publishes the resulting receipt and floor descriptor through the same root.

The recovery-close implementation reuses ordinary Undo/Redo in a private Core after
full suffix equivalence. Those two accepted commands close the coalescing key and
remain in the retained stream; they preserve the gesture's single undo entry. This
is a smaller faithful alternative to extending JournalCommand's serialized enum or
adding a second replay engine. The tests continue the same gesture before closing,
then verify subsequent Undo/Redo and a new gesture. Project recovery-close checks its
persisted predecessor/cut/digest and uses the same receipt path. S2's shared GUI/host
freeze, installation and reply handshake remain pending.

The optional transition field is omitted from canonical original edit/comment
requests, preserving their pre-transition digests. Specialized record kinds are
admitted only with their typed validators and raw nested-operation support scan;
the supported API inventory stays compatible with the frozen commit-4 v3 corpus.
`ProjectStore::read_record` exposes the existing verified reader through a sealed
record trait so the ordinary agent dependency can consume these typed values.

The new no-default crate build exposed a pre-existing feature boundary: the
`kinewright-eval` executable imported eval-only modules without a `required-features`
declaration. Its manifest now explicitly requires `eval-harness`, as the JA native
executable already does. Defaults still enable that feature, and every ordinary and
explicit eval gate still builds both executables. A production no-default build can
now build the library without compiling fixture-only executable code. This is build
routing, not a skipped eval acceptance check.

Commit-6 ownership/implementation pointers are
`crates/kinewright-core/src/history_transition.rs:24` and `:63`,
`crates/kinewright-core/src/actor.rs:223`,
`crates/kinewright-project/src/bundle_branch.rs:113` and `:149`,
`crates/kinewright-project/src/bundle_commit.rs:233`, `:323` and `:844`, and
`crates/kinewright-agent/src/durable_branch.rs:98`, `:146`, `:164` and `:201`.

Commit 6's installed MSVC target compiled Core plus its tests successfully (compile
only). Explicit no-default agent and app target checks were attempted separately;
FFmpeg, Whisper and aws-lc native MSVC build prerequisites are unavailable. Those
full cfg lanes and all native Windows execution remain NOT RUN. No toolchains were
installed, no native storage-durability success is inferred, and no full CI ran.

The combined metadata/decode boundary at
`crates/kinewright-project/tests/support/ja_s1_retention.rs:646` reproduced a real
one-byte over-admission: the object index was charged while the caller decode and
streaming scratch remained live. `store.rs:329` now includes live decoded bytes in
the checked metadata sum before object publication. The exact-boundary control and
one-beyond refusal have a preserved baseline red and restored green; the twelfth
mutation removes that guard. This repairs the existing cap rather than raising it.

### Round 2

The round-2 dispositions and source-bound validation are in [fix-r2.md](fix-r2.md).
N4 is deferred: valid floor cuts depend on Undo/Redo and gesture boundaries; no
monotonic binary-search predicate or improved cold-save bound is established here.
C1 remains runner-dependent until the next hosted failing control supplies the
new bounded `disposition`/`missing_requirements` diagnostic. L6 remains open;
round-1 latency figures are historical and were not remeasured at this head.

The inherited retirement assertion now distinguishes definite cleanup failure
from namespace uncertainty for N6
(`crates/kinewright-project/tests/support/ja_s1_retention.rs:263`). The Linux
app/session witness is included at `crates/kinewright-app/src/app.rs:10882` to
respect the existing AU6 source inventory. N5's cutoff required staging-only
rollback eligibility in the existing collector
(`crates/kinewright-project/src/store.rs:2425`), preserving exact pre-copy
inventory bytes. These integration adjustments and their witnesses are detailed
in the round-2 report.

### Round 3

See [fix-r3.md](fix-r3.md) for the first-save caps, reused LUT IDs, root-bound Core
reconciliation, persistent cleanup reporting, incremental floor search and standalone
feature wiring. N4's cold-search deferral is replaced by measured default-limit runs.
The frozen predecessor's executable hashes account for the local setup delay; the
Linux script strips the artifact and emits per-phase diagnostics. Hosted C1 confirmation
and new native Windows execution remain with the lead. L4, L6 and the existing O11/O26
deferrals are unchanged. No S2 writer or producer-fencing scope was added.

### Round 4

See [fix-r4.md](fix-r4.md) for the revocable pending floor and production-prefilter witnesses.
The lead's post-stop ruling replaces irrevocable eager flooring; definite failures retain all history.

- m2 deferred: media-ID reuse through an unverified-source relink still fails closed; content-qualified media synthesis remains app-wiring work.
- m3 deferred: first-save manifests with two LUTs under one bare ID still refuse at manifest admission; producers must supply the existing content-qualified spelling.
- m4 deferred: collection reports retain at most 32 per-path attempts and identity refusals halt collection; bounded diagnostics and fail-closed identity handling remain intentional here.
- m5 corrected: archived `r3-first-copy-own` is a double-install red, not a no-install red; the shared `-own` reds cover omitted installation. Archived `r3-persistent-gc` fails at its observable counter, not at the later continuation assertions. Round 4 does not claim that archived mutant proves continuation.
- m6 corrected: predecessor `identity.json` now records the pre-strip SHA-256, strip version and arguments. `run.json.source_head` is the run working directory's HEAD; source-to-executable provenance is the clean-source build and checks within the same script invocation, not a hash-inferred source identity.
- Hosted C1 was reported successful on `9cd2e71` by the lead; fresh round-4 local controls are recorded separately. Native Windows execution and full slow CI remain lead-owned.

### Round 5

See [fix-r5.md](fix-r5.md) for the amended ruling's exact-destination reconciliation,
pre-B5 release, orphaned-ticket settlement, readable/unreadable Save As, floor
alignment, typed selection/Undo refusals, mutation pairs and setup/replay timings.
Core stores runtime reconciliation values and performs no filesystem IO. The
existing source hold remains reachable until its copy destination is reconciled.

Round 5 also adds these, each with its own mutation pair:

- `SaveInProgress`, a retryable refusal of a second barrier while a save is in flight.
- A readable tagged path whose parent was moved or recreated counts as a definite release.
- A previously published owner never accepts a vanished root as unpublished.
- `ProjectBundle::reconcile` retries outstanding object-directory syncs.
- A test-only `object_directory_sync` fault point.
- The harness reaps a recorded participant on the record when a later participant's setup fails.

C4 explicitly authorizes the small `kinewright-hash` workspace crate, an exception
to the original plan's no-new-crate rule. It contains the existing SHA implementation
and adds no third-party dependency. The predecessor source and setup deadlines stay
frozen. Existing m2/m3/m4, L4/L6 and S2/S6 boundaries remain unchanged.

### Round 6 known limitation

A permanently unreadable tagged destination blocks further saves of that session,
including Save As elsewhere. Every entry stays in memory and nothing is lost while
the session lives. An exit requires an S0 amendment, which the lead will raise
separately; round 6 does not add an abandon action.
A1 specifies S2 rescue; unavailable until implemented and gated.
The recorded changed-parent release is the S1 rule, and last-reader retirement is ordinary-handoff evidence;
for rescue-retained A, A1 §6 instead preserves unknown state on unproved remount identity or B/descendant bytes.
Unreadable-graph equality still proves neither commit nor release.
