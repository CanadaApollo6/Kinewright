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
4. V3 publication and durable history: not committed.
5. Command receipts and issuer retirement: not committed.
6. Durable bases, lineage and floors: not committed.
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
