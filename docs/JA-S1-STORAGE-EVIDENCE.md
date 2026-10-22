# JA-1 S1 storage evidence

The current measurements are the fix-round-1 results in
[fix-r1.md](reports/ja1-s1/fix-r1.md#l6-paired-measurements) and
[r1-storage-measurements.json](reports/ja1-s1/r1-storage-measurements.json).
The original eight-commit measurements remain in
[storage-measurements.json](reports/ja1-s1/storage-measurements.json) as historical
evidence. Limits remain provisional; L6 stays open for the lead and its 250 ms
comment target is unchanged.

## Reproduce

Build and seal the original `0f4f88e` release example with per-comment SyncProfile
counter snapshots only. Build the current example with `test-util`, then separately
with `test-util,storage-heap-profile`. Seal them under
`target/ja-s1-measurements/r1/{before,after,profiled}`. Source FFmpeg in the shell,
then launch `setsid nohup nice -n 19 python3 -I scripts/ja-s1-fix-r1-measure.py
<new-absolute-evidence-dir>`. The runner checks sealed binary hashes and runs five
alternating ordinary before/after pairs, then five separate profiled workloads.
Each child has a ten-minute timeout. No own build/test overlaps timing.

The ordinary release binaries use the system allocator; the separate profiling
binary uses pinned dev-only DHAT 0.3.3. DHAT measures actual Rust allocation
requests and live/peak heap, without counting foreign allocators. Kernel RSS,
physical payload bytes and conservative reservations are reported separately.
The runner records CPU counters, start/end desktop CPU load, GPU clocks/pstate
and five-second process CPU contamination samples without changing other work.

## Fresh paired results

| Durable operation | Before median run p50 / p95 | After median run p50 / p95 |
| --- | ---: | ---: |
| Metadata save | 175.829 / 188.040 ms | 93.371 / 101.042 ms |
| Comment | 238.948 / 262.516 ms | 131.511 / 164.736 ms |

Comment p95 ranges are 254.199–312.907 ms before and 161.305–165.546 ms after.
Each run has eight samples; these are medians/ranges of per-run percentiles,
not pooled percentiles. All five after runs meet 250 ms in this workload. This
is a bounded shared-desktop measurement and does not close L6.

Actual sync calls per comment are `[28,24,25,26,26,26,26,26]` before and
`[20,16,16,16,16,16,16,16]` after, identical across all five pairs. Counts include
file and directory syncs and next-save maintenance. During B0–B5, object-directory
barriers coalesce to one per directory; collection executes after acknowledgment
or before the next publication. The command witness separately asserts no GC or
full history decode in unchanged-history publication. Required file/root/inventory
barriers remain. Sync time and decode/counter evidence support the observed
reduction without attributing unrelated desktop load to the change.

The unchanged history encodes to 216,346 bytes. Five separate decodes each retain
640,687 actual Rust allocator bytes under the 3,676,919-byte stable encoded-format
reservation. Each full-workload allocator peak is 16,259,646 bytes. Median ordinary
kernel VmHWM is 37,154,816 bytes before and 36,057,088 bytes after; these are whole
process values, not decode allowances. Profiled VmHWM is 40,865,792–41,185,280 bytes.
No timing from a profiled run contributes to the latency table.

All 15 processes complete with zero unexpected errors and 45 intended copy
refusals. The workload restores complete undo/redo, retains an 8 MiB evidence
payload, advances six ordered floors, copies acknowledged content, performs full
recovery and first-saves a 2 MiB session input while an old reader remains valid.
Source bytes become zero after the final reader closes. The raw output compares
physical payload bytes with the owner's counter and verifies exact source roots
on failed copies.

After ordinary pair 1, 39 immutable objects, 119 edges and 41 files hold 8,854,750
charged/on-disk payload bytes; allocated file bytes are 8,986,624 and the lock
payload is zero. Peak physical bytes are 8,859,145. Eight incremental saves verify
149,096 bytes without rereading the retained 8 MiB payload. First-save handoff
holds 2,097,508 source bytes and 2,111,792 destination bytes, then releases all
source bytes after the last reader. Delayed maintenance conservatively retains
old/new recovery inventory at B5 until the next inventory/collection barrier.

## Identity and scope

Before binary SHA-256:
`00faaaeee9005727ed7efaf7123538c7954750fb54c810fdfca1721c0aca5b38`.
After binary SHA-256:
`9d46c7b2c3a5199df322d9d75d619ef14d4139df054fe77a0dca46a8d893611e`.
Profiled binary SHA-256:
`93e4bd1bd13bdd9dc3c2fcff2bc4dd0de50f31f24b3afce74be48d947c4fb0ea`.
The raw identity records the exact measured source manifest/diff and compiler.
The later removal of a Linux-only attribute from an agent eval test does not
change any benchmark dependency; the final binding records this test-only delta. Raw
logs, outputs, load samples and DHAT profiles are compressed under the main
checkout's `target/review/ja/s1-evidence/r1/timing/`, outside Git.

`realistic_history_saves_every_edit_and_advances_durable_floor` separately saves
110 edits of a 2,000-track document under a lowered 8 MiB history cap. Every save
preserves the current edit, the durable floor advances, latest undo survives,
and full recovery validates all retained receipt checkpoints without retaining
all decoded archives simultaneously. The default caps are unchanged. Other
boundary tests use lowered ceilings rather than physically filling 20 GiB or
10,000 objects. A sparse over-quota orphan tests logical file admission and
safe retirement; it is not a physical 20 GiB workload.

Linux syscall observations, dropped-write CrashFs, process-death witnesses and
allocator measurements are distinct evidence classes. Native Windows execution,
Windows durability, hosted fast/slow CI, L4 decoder-open certification, full
G15/G20, S2 GUI attachment and physical power/device faults remain NOT RUN.
Earlier Kani codegen evidence is historical and was not rerun here; zero new
proof harnesses ran. No decoder-open or whole-process memory allowance follows
from the measured opaque-byte/Core workloads.
