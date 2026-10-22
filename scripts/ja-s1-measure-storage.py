"""Run five serial timing/allocator pairs from sealed release executables."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import re
import resource
import statistics
import subprocess
import sys
import time

out = Path(sys.argv[1])
assert out.is_absolute()
out.mkdir(parents=True, exist_ok=True)
root = Path.cwd()
binaries = {kind: root / "target/ja-s1-measurements/sealed" / kind for kind in ("ordinary", "profiled")}
identity = {
    "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "source_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "--binary"])).hexdigest(),
    "binary_sha256": {kind: hashlib.sha256(path.read_bytes()).hexdigest() for kind, path in binaries.items()},
    "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "cores": os.cpu_count(),
    "uptime_before": subprocess.check_output(["uptime"], text=True).strip(),
    "profile": "release; jobs=4; test-util seams; only profiled executable uses dhat 0.3.3",
    "source_files": {},
}
for prefix in ("crates/kinewright-core/src", "crates/kinewright-media/src", "crates/kinewright-project/src", "crates/kinewright-project/examples"):
    for path in sorted(Path(prefix).rglob("*.rs")):
        identity["source_files"][str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
for path in map(Path, ("Cargo.toml", "Cargo.lock", "crates/kinewright-project/Cargo.toml")):
    identity["source_files"][str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
(out / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
runs = []
for index in range(1, 6):
    for kind, binary in binaries.items():
        stem = f"pair-{index}-{kind}"
        result_path = out / f"{stem}.json"
        if result_path.exists():
            raise SystemExit(f"refusing to overwrite measured run: {result_path}")
        assert hashlib.sha256(binary.read_bytes()).hexdigest() == identity["binary_sha256"][kind]
        env = dict(os.environ, JA_S1_HEAP_FILE=str(out / f"{stem}-heap.json"))
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        started = time.monotonic()
        with result_path.open("w") as stdout, (out / f"{stem}.stderr").open("w") as stderr:
            status = subprocess.run(["nice", "-n", "10", str(binary), str(out / f"{stem}-workload")], env=env, stdout=stdout, stderr=stderr).returncode
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        assert status == 0, (stem, status)
        result = json.loads(result_path.read_text())
        assert result["errors"] == 0 and result["expected_failed_copies"] == 3
        assert result["profiled"] == (kind == "profiled")
        assert result["source"]["physical_bytes"] == result["source_physical"]["payload_bytes"]
        assert result["duplicate"]["first"]["physical_bytes"] == result["duplicate"]["second"]["physical_bytes"]
        assert result["first_save"]["source_after_last_reader"]["physical_bytes"] == 0
        text = (out / f"{stem}.stderr").read_text()
        sync = json.loads(re.search(r"^sync_profile: (.*)$", text, re.M)[1])
        run = {
            "pair": index, "kind": kind,
            "elapsed_seconds": time.monotonic() - started,
            "user_cpu_seconds": after.ru_utime - before.ru_utime,
            "system_cpu_seconds": after.ru_stime - before.ru_stime,
            "sync": sync, "measurements": result,
        }
        runs.append(run)
        (out / f"{stem}-counters.json").write_text(json.dumps(run, indent=2) + "\n")
        for path in [result_path, out / f"{stem}.stderr", out / f"{stem}-heap.json"]:
            if path.exists():
                with gzip.open(str(path) + ".gz", "wb") as compressed:
                    compressed.write(path.read_bytes())
                path.unlink()
        print(stem, f"{run['elapsed_seconds']:.2f}s", "errors=0", flush=True)

ordinary = [r for r in runs if r["kind"] == "ordinary"]
profiled = [r for r in runs if r["kind"] == "profiled"]
def distribution(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}
summary = {"schema": 1, "identity": {k: v for k, v in identity.items() if k != "source_files"}, "pair_count": 5,
           "unexpected_errors": 0, "timing_scope": "ordinary executable only; median and range of per-run percentiles",
           "ordinary_runs": ordinary,
           "profiled_runs": [{k: v for k, v in r.items() if k != "measurements"} | {"heap": {key: r["measurements"][key] for key in ("baseline_heap", "history_heap", "restore_heap", "metadata_heap", "final_heap")}} for r in profiled],
           "timings_ms": {},
           "rss_hwm_bytes": distribution([r["measurements"]["final_rss"]["hwm_bytes"] for r in ordinary]),
           "rust_allocator_peak_bytes": distribution([r["measurements"]["final_heap"]["peak_bytes"] for r in profiled]),
           "sync_share": distribution([r["sync"]["nanoseconds"] / (r["elapsed_seconds"] * 1e9) for r in ordinary]),
           "uptime_after": subprocess.check_output(["uptime"], text=True).strip()}
for group, key in (("metadata", "saves"), ("metadata", "comments"), ("history", "restore"), ("recovery", "latencies"), ("floor", "latencies")):
    summary["timings_ms"][f"{group}_{key}"] = {p: distribution([r["measurements"][group][key][p] for r in ordinary]) for p in ("p50_ms", "p95_ms")}
Path("docs/reports/ja1-s1/storage-measurements.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps({k: summary[k] for k in ("pair_count", "unexpected_errors", "timings_ms", "rss_hwm_bytes", "rust_allocator_peak_bytes", "sync_share")}, indent=2))
