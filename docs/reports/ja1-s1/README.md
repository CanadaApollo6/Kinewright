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
2. Namespace authority and strict IO: not committed.
3. Bounded objects and owned inputs: not committed.
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
