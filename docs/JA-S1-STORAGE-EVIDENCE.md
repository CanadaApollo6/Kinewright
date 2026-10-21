# JA-1 S1 storage evidence

S1 provides Linux bundle storage and bounded synchronous ingress. The numerical
limits remain provisional under L6. This document records bounded workloads; it
does not certify a 20 GiB edit, native Windows durability, decoder-open stability,
GUI fencing or full G15/G20 acceptance.

## Reproduce the measurements

Build `ja_s1_storage_budget` in release mode with `test-util`, seal that executable,
then build and seal it with `test-util,storage-heap-profile`. Run
`nice -n 19 python3 -I scripts/ja-s1-measure-storage.py <new-absolute-evidence-dir>`
after `source scripts/setup-ffmpeg.sh`. The runner expects the two executables at
`target/ja-s1-measurements/sealed/{ordinary,profiled}`. It refuses to overwrite a
run, checks binary hashes, asserts the workload outputs, and preserves separate
JSON, stderr, source identities, CPU counters and compressed allocator profiles.
Its children inherit the Python runner's low scheduling priority. Run cargo with
the task's four-job, memory and serialization limits.

The ordinary executable uses the system allocator. The profiled executable uses
the safe global allocator wrapper from the pinned, dev-only `dhat = 0.3.3`.
The [DHAT profiling API](https://docs.rs/dhat/0.3.3/dhat/) counts actual Rust
allocator requests, current live allocations and the exact high-water allocation
count. It does not count foreign allocators or substitute for process RSS. The
workload uses opaque stored bytes and Core logic; it never opens a media decoder.
Profiler timings are excluded from the latency results.

Five ordinary/profiled pairs run serially and interleave. Each process starts with
a new directory. JSON records the RSS baseline and kernel VmHWM, actual allocator
baseline/history/restore/metadata/whole-run peaks, and conservative store reservation
counters separately. Native sync-call durations identify the measured IO cost.
CPU counters include the whole process and its Core actor threads. The shared
desktop load is recorded before and after; no other project's processes are changed.

## Workloads and output checks

- History: 32 tracks, 96 edits followed by 24 undos; 72 undo and 24 redo entries.
  Five new Core actors restore and compare the complete exported history.
- Metadata: eight saves of the acknowledged history, then eight durable comments.
  The 8 MiB retained evidence payload remains pinned. Verification counters confirm
  the incremental saves do not hash that unchanged large payload.
- Floors: six ordered floors through durable command receipts. Later saves retire
  old automatic floor descriptors while the latest descriptor remains.
- Duplicate bytes: admit the same 8 MiB payload twice and compare unique bytes and
  object counts. Logical pins do not create a second physical charge.
- Failure and copy: three injected copy-verification refusals preserve the exact
  source root and remove the new destination store; a successful complete copy
  retains the source overlap reservation until destination handoff.
- Recovery: five full opens restore the exact history and hash the complete retained
  payload. Incremental-save counters and recovery counters remain separate.
- First save: copy a 2 MiB session input, keep the old reader after handoff, compare
  its exact bytes, then verify zero source bytes after the final reader closes.

The runner checks zero unexpected errors and exactly three intended copy refusals
per process. On-disk file payload bytes equal the source owner's physical counter.
Allocated file blocks and the separate lock-file payload are also reported; these
are not a measurement of filesystem tree or directory overhead.

## Measured results

The completed numerical table and source/binary identities are generated from the
five-pair run in `docs/reports/ja1-s1/storage-measurements.json`. Per-run p50/p95
values have eight save/comment samples, six floor samples, and five restore/open
samples. Headline values are medians and ranges of those five per-run percentiles;
they are not pooled percentiles and make no before/after speedup claim.

| Durable operation | Median run p50 | Median run p95 | Range of run p95 |
| --- | ---: | ---: | ---: |
| Metadata save | 172.790 ms | 187.781 ms | 175.849–222.539 ms |
| Comment | 250.304 ms | 265.138 ms | 253.866–276.095 ms |
| Core history restore | 1.288 ms | 1.565 ms | 1.361–1.592 ms |
| Full bundle open | 43.990 ms | 46.778 ms | 44.543–50.496 ms |
| Ordered floor | 196.475 ms | 214.461 ms | 197.213–228.503 ms |

Five pairs completed with zero unexpected errors and 30 intended copy refusals
(three in each of ten processes). The provisional ≤250 ms comment target is **not consistently met**:
median run p95 is 265.138 ms, with 276.095 ms worst run p95.
No cap was increased and L6 remains open.

The history encodes to 216,346 bytes and reserves
25,000,775 decoded-history bytes. Actual
Rust allocator live bytes after export are 637,345;
its restore-phase global peak is 1,437,927 bytes.
Whole-workload allocator peak has median 16,448,789 bytes
(16,448,789–16,448,789). Ordinary-process RSS baseline has median
5,734,400 bytes;
median VmHWM is 37,019,648 bytes (36,839,424–37,122,048).
Allocator peaks, RSS, cumulative allocations and conservative reservations are
different measurements. The sampled workload fits the admitted limits; it does
not physically fill default capacities or establish a whole-process allowance.

In ordinary pair 1 the source has 33 immutable objects,
95 content edges and 35 files including root/inventory.
Charged and on-disk payload bytes both equal 8,797,259;
stat reports 8,908,800 allocated file bytes,
with a separate 0-byte OS-lock file.
Source peak physical bytes are 8,859,118,
peak reserved bytes 8,796,771, and peak decoded
reservation bytes 142,944,697. These
reservations include parser scratch and metadata; they are not actual heap.
Eight incremental saves verify 5,324,982 bytes total,
without rereading the retained 8 MiB payload. One full recovery verifies
8,799,682 bytes in 35 calls.

First-save pair 1 holds 2,097,509 source bytes
and 2,111,793 destination bytes through handoff.
The old reader remains valid; source bytes become exactly zero after the final
reader closes. Complete per-run copy/duplicate/floor/recovery counters remain in
the JSON report and compressed raw output.

Native sync calls consume median 84.0% of ordinary elapsed time
(range 83.8–84.5%). Process CPU is
1.752–1.876 seconds per ordinary workload. The measured limiter
is file/directory durability barriers in `namespace/linux.rs::sync_file` and
`sync_directory`. Profiling is separate and slower. Shared-desktop variation is
retained; this is not evidence of a performance improvement.

Ordinary binary SHA-256: `c711cd179ec0e0593bf6ada2217166b8def6737d15888f602fa445efacfa1c31`.
Profiled binary SHA-256: `dead4d58a472739fd285b7bdee03b819e039ca2ca13b036b88ada53fe49141f2`.
Parent HEAD: `0b960f2a98e98bcbbde488e715d3fa978d0633ea` plus the exact modified-source
manifest and diff hash in the raw measurement directory's `identity.json`.
Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. The final report verifies measured source
hashes; test/document edits are identified by the commit gate separately.

## Ingress and recovery witnesses

`tests/ja_s1_storage.rs` includes the five required retention/budget tests from
`tests/support/ja_s1_retention.rs`. They use real temporary namespace/store owners:
two verified predecessors and one B3 slot; inventory replacement and unlink faults;
native death after root-file sync; old floor collection with independent branch,
specialist-base and receipt pins; incremental and full byte verification; and
boundary/one-beyond ingress with unchanged acknowledged root and Core state.

Lowered ceilings exercise actual control bytes, manual pending admission, command
bytes, encoded/live branch history, simultaneous history/decode/metadata reservations,
specialist versions, object count, graph depth, comments, text, points, receipts,
issuers, owned bytes and copy overlap. These are deterministic boundary tests, not
physical fills of the default 20 GiB/10,000-object allowances. The synchronous S1
owner admits one pending manual command; queued GUI/transport producers belong to S2.
Voice capture/duration and the voice-specific retained-byte producer remain NOT RUN
under S3; unsupported version/run/task/code producers remain refused. Generic byte
storage does not certify a voice, decoder or provider result.

The additive `s1-storage-budget.json` fixture runs a native participant against the
production session owner. Its exact-byte and boundary controls support quota refusal,
live-reader retention and one-time retirement properties. The old adapter lacks this
capability and returns NOT RUN. No capability gap is counted as a baseline runtime red.

On writer reopen, the same owner scans pinned managed directories and the exact
root-specific temporary prefix. Unclassified immutable files remain charged against
both owned and metadata allowances conservatively, with one physical charge. They
are not blindly deleted. An expected typed admission verifies both incoming and disk
bytes before reclassifying the charge. Owner-specific temporary files can retire
only after safe durable publication; every unlink retains the existing sync barrier.
Unprovable names, file types or ownership refuse rather than granting cleanup authority.

Automatic predecessor traversal excludes historical automatic inventory edges.
Independent content, branch, baseline, evidence, receipt and reader pins still retain
their complete closures. GC also removes retired inventory-edge rows. Prospective
graph-depth validation includes the incoming object before its file is published.

## Remaining lanes

Linux production storage, native participant/process-death tests and the portable
crash model are distinct evidence classes. Windows Core compile checks do not prove
project/native Windows durability. Full MSVC dependency checks, native Windows CI,
full fast/slow CI, decoder certification and full G15/G20 remain separately NOT RUN.
Kani's actual project-crate admission result is recorded with the commit report; no
filesystem or decoder property follows from a bounded arithmetic proof.

The actual project-crate Kani codegen admission probe passed in
49.810 seconds with a sampled process-tree RSS peak of
1,823,285,248 bytes (250 ms sampling). This is a compile
boundary result; **zero proof harnesses ran**. The two §11 candidates remain
optional and NOT RUN as proofs. No alternate arithmetic implementation or unproved
harness/workflow was added. Deterministic ingress, crash and mutation witnesses
supply the executed storage evidence.
