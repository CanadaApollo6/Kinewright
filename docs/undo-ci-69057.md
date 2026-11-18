# CI repairs after the reader audit

Base: `69057aea`, CI run `43897585078`. This repair preserves the existing
history, pool and authority limits. It adds no retry, ignore or longer deadline.

`Core::for_test` already gives each fixture a private history pool. W13's
temporary actor instead raced its own asynchronous destruction: dropping its
last handle disconnected the queue but returned before the worker released its
history lease. The existing shared handle allocation now owns the worker's join
handle. The final sender drops before the worker joins, so final actor drop
returns only after its lease is released. A retained operation reader continues
to own its separate lease. The portable regression checks both lifetimes over
64 cycles; W13 checks that its finished temporary branch has retired before the
next open.

The Windows Save-refusal test has two live projects. Keeping its expected full
capture while making another required a fourth Ordinary lease. It now compares
the complete encoded owner before and after the refusal, releasing each capture
before requesting the next. Every document, stack, ledger, lineage, input and
resource comparison remains. A portable regression checks that these complete
comparisons use at most three leases and leave none after teardown.

The completed-parent failure exposed two independent disclosure bugs. Startup
recovery status hid retained-work instructions through `Option::or`; the
completed-run reader also chose only one retained task even when both parent
and child had work. ObserveRun now composes recovery status with every retained
task disclosure. The existing durable child-recovery regression checks each
saved branch's exact disclosure both normally and during startup recovery.
Readiness and editing authority remain unchanged.

The cfg sweep includes every crate's source and integration-test trees:
3,070 attributes in 401 files, including 391 attributes in test trees, plus
35 `cfg!` expressions. Linux-only durable-test imports and helpers now match
their callers. The fixture-only refresh contract and platform-specific tool
name consumer also have matching gates. Portable codec, actor and ledger tests
remain available on Windows.

Three compiled wrong changes were restored after their named failures:

| Wrong change | Failing assertion |
|---|---|
| Drop the worker without joining | `ESCAPED_READER_KEEPS_ITS_OWNED_LEASE` |
| Let startup status replace retained work | `RETAINED_WORK_DISCLOSURE_SURVIVES_STARTUP_RECOVERY` |
| Pick only one retained task | `COMPLETED_PARENT_DISCLOSES_EVERY_RETAINED_CHILD` |

The restored source passes 5,547 workspace fast tests (287 expected ignores)
and 4,383 touched-crate slow tests (116 expected ignores), including all 16 long
session partitions: 64 seeds and 65,536 attempts. Opt-in TE1 passes 41 library
and 13 unpaid measurement tests. Workspace and feature clippy, fmt, the workspace
build, all three separate shipping builds and slow-test lint pass.

The installed MSVC target was checked. Whisper/FFmpeg binding generation stops
on missing MSVC C headers (`stdio.h`, `errno.h`); AWS-LC also lacks a working
cross-compiler setup. Project-test compilation is not reached. The source cfg
review does not qualify native Windows compilation or execution.

The CI runner passes fast shard 0 (1,083 passed, 10 expected ignores, 358 seconds)
and shard 8 (449 passed, 160 expected ignores, 389 units). W13 passes in its
original isolated unit. The workflow's matrix label `(8, 35)` means index 8
with a 35-minute job timeout; the actual Linux fast plan has 12 shards.

An earlier local shard 0 run also timed out reading a proxy header in the
paused-recovery test while another build overlapped. The diagnostic now names
the request/response exchange; its 15-second socket deadline and 45-second
condition deadline are unchanged. The focused test, full fast and slow gates,
and final exact shard pass. The timeout's cause remains unproven.

[Gate, cfg and mutation evidence](measurements/undo-ci-69057-proof.json.gz)
records exact commands, source hashes, the three compiled red logs and the
final shard results. No build or test remains running at this commit.
