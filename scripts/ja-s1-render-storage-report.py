"""Render the compact measured-results section from the preserved numeric report."""
from pathlib import Path
import json
import statistics

data = json.loads(Path("docs/reports/ja1-s1/storage-measurements.json").read_text())
assert data["pair_count"] == 5 and data["unexpected_errors"] == 0
runs = [run["measurements"] for run in data["ordinary_runs"]]
first = runs[0]
heap = data["profiled_runs"][0]["heap"]
lines = [
    "| Durable operation | Median run p50 | Median run p95 | Range of run p95 |",
    "| --- | ---: | ---: | ---: |",
]
labels = {
    "metadata_saves": "Metadata save", "metadata_comments": "Comment",
    "history_restore": "Core history restore", "recovery_latencies": "Full bundle open",
    "floor_latencies": "Ordered floor",
}
for key, label in labels.items():
    value = data["timings_ms"][key]
    lines.append(f"| {label} | {value['p50_ms']['median']:.3f} ms | {value['p95_ms']['median']:.3f} ms | {value['p95_ms']['min']:.3f}–{value['p95_ms']['max']:.3f} ms |")
comment = data["timings_ms"]["metadata_comments"]["p95_ms"]
target = "not consistently met" if comment["max"] > 250 else "met in these bounded runs only"
peak = data["rust_allocator_peak_bytes"]
rss = data["rss_hwm_bytes"]
cpu = [run["user_cpu_seconds"] + run["system_cpu_seconds"] for run in data["ordinary_runs"]]
lines.append(f"""
Five pairs completed with zero unexpected errors and 30 intended copy refusals
(three in each of ten processes). The provisional ≤250 ms comment target is **{target}**:
median run p95 is {comment['median']:.3f} ms, with {comment['max']:.3f} ms worst run p95.
No cap was increased and L6 remains open.

The history encodes to {first['history']['encoded_bytes']:,} bytes and reserves
{first['history']['decoded_reservation_bytes']:,} decoded-history bytes. Actual
Rust allocator live bytes after export are {heap['history_heap']['current_bytes']:,};
its restore-phase global peak is {heap['restore_heap']['peak_bytes']:,} bytes.
Whole-workload allocator peak has median {peak['median']:,.0f} bytes
({peak['min']:,}–{peak['max']:,}). Ordinary-process RSS baseline has median
{statistics.median(run['baseline_rss']['rss_bytes'] for run in runs):,.0f} bytes;
median VmHWM is {rss['median']:,.0f} bytes ({rss['min']:,}–{rss['max']:,}).
Allocator peaks, RSS, cumulative allocations and conservative reservations are
different measurements. The sampled workload fits the admitted limits; it does
not physically fill default capacities or establish a whole-process allowance.

In ordinary pair 1 the source has {first['source']['objects']} immutable objects,
{first['source']['edges']} content edges and {first['source_physical']['files']} files including root/inventory.
Charged and on-disk payload bytes both equal {first['source']['physical_bytes']:,};
stat reports {first['source_physical']['allocated_file_bytes']:,} allocated file bytes,
with a separate {first['source_physical']['lock_payload_bytes']}-byte OS-lock file.
Source peak physical bytes are {first['source']['peak_physical_bytes']:,},
peak reserved bytes {first['source']['peak_reserved_bytes']:,}, and peak decoded
reservation bytes {first['source']['peak_decoded_reservation_bytes']:,}. These
reservations include parser scratch and metadata; they are not actual heap.
Eight incremental saves verify {first['metadata']['verified_bytes_delta']:,} bytes total,
without rereading the retained 8 MiB payload. One full recovery verifies
{first['recovery']['usage']['verified_bytes']:,} bytes in {first['recovery']['usage']['verified_calls']} calls.

First-save pair 1 holds {first['first_save']['source_overlap']['physical_bytes']:,} source bytes
and {first['first_save']['destination']['physical_bytes']:,} destination bytes through handoff.
The old reader remains valid; source bytes become exactly zero after the final
reader closes. Complete per-run copy/duplicate/floor/recovery counters remain in
the JSON report and compressed raw output.

Native sync calls consume median {data['sync_share']['median']*100:.1f}% of ordinary elapsed time
(range {data['sync_share']['min']*100:.1f}–{data['sync_share']['max']*100:.1f}%). Process CPU is
{min(cpu):.3f}–{max(cpu):.3f} seconds per ordinary workload. The measured limiter
is file/directory durability barriers in `namespace/linux.rs::sync_file` and
`sync_directory`. Profiling is separate and slower. Shared-desktop variation is
retained; this is not evidence of a performance improvement.

Ordinary binary SHA-256: `{data['identity']['binary_sha256']['ordinary']}`.
Profiled binary SHA-256: `{data['identity']['binary_sha256']['profiled']}`.
Parent HEAD: `{data['identity']['source_head']}` plus the exact modified-source
manifest and diff hash in the raw measurement directory's `identity.json`.
Compiler: `{data['identity']['rustc']}`. The final report verifies measured source
hashes; test/document edits are identified by the commit gate separately.
""")
path = Path("docs/JA-S1-STORAGE-EVIDENCE.md")
text = path.read_text()
start = text.index("| Durable operation")
end = text.index("## Ingress and recovery witnesses", start)
path.write_text(text[:start] + "\n".join(lines).rstrip() + "\n\n" + text[end:])
