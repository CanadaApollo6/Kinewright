# JA1 S1 implementation evidence

Worktree: `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`.
Baseline: `076a96c922b2f05984df4bb839ad7cf0900e6183`.
Authority: accepted `target/review/ja/ja1-s1-plan-rev2.md` and implementation brief.
No push or independent review is authorized by this task.

The implementation is in progress. No S1 acceptance is claimed by this file.
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
7. Save As and first save: not committed.
8. Recovery retention and measured evidence: not committed.

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
