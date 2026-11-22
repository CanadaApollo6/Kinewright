#!/usr/bin/env python3
"""JA-1 S6 G20: run the scripted host workflow and sample its process tree.

The workflow is the `g20-measure` test in kinewright-agent
(`writer_host/review_s6_c5_measure.rs`). Build the test binary and the MS2
helper and supervisor (`--profile g20`) first, then
run this script with no build or test running alongside it (PITFALLS).

    nice -n 19 python3 -I -B scripts/measure-g20.py --binary <test exe> \
        --lane lavapipe --runs 5 --evidence target/ja1-s6-c5/lavapipe \
        --summary docs/reports/ja1-s6/g20-lavapipe.json

The sampler reads /proc for the whole process tree under the test process
(host, private dbus-daemon, any helper child): per-process peaks, and the sum
of RSS within each sample, whose maximum is the sampled simultaneous peak.
It never adds per-process peaks across samples. VRAM comes from a read-only
`nvidia-smi --query-compute-apps` poll. Nothing changes a GPU, display or
system setting. Load is recorded, never a gate.

Raw sample streams stay in --evidence; --summary is the small committed file.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import threading
import time

TEST = "writer_host::hooks_tests::tests::ja1_s6_c5_g20_measure"
CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE_KIB = os.sysconf("SC_PAGE_SIZE") // 1024


def read_stat(pid):
    """(comm, ppid, cpu ticks) from /proc/<pid>/stat, or None."""
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return None
    head, _, tail = text.rpartition(")")
    comm = head.split("(", 1)[1]
    fields = tail.split()
    # After the comm: state(0) ppid(1) ... utime(11) stime(12).
    return comm, int(fields[1]), int(fields[11]) + int(fields[12])


def read_status(pid):
    try:
        lines = Path(f"/proc/{pid}/status").read_text().splitlines()
    except OSError:
        return None
    status = dict(line.split(":", 1) for line in lines if ":" in line)

    def kib(key):
        return int(status.get(key, "0 kB").split()[0])

    return {"rss_kib": kib("VmRSS"), "hwm_kib": kib("VmHWM"), "threads": int(status.get("Threads", "0"))}


def all_stats():
    stats = {}
    for entry in os.listdir("/proc"):
        if entry.isdigit():
            stat = read_stat(int(entry))
            if stat:
                stats[int(entry)] = stat
    return stats


def exe_name(pid, comm):
    """The executable's file name: `comm` is cut at 15 bytes."""
    try:
        return os.path.basename(os.readlink(f"/proc/{pid}/exe"))
    except OSError:
        return comm


def tree(root, stats):
    children = {}
    for pid, (_, ppid, _) in stats.items():
        children.setdefault(ppid, []).append(pid)
    found, stack = [], [root]
    while stack:
        pid = stack.pop()
        if pid in stats:
            found.append(pid)
            stack.extend(children.get(pid, []))
    return found


def top_cpu(seconds=1.0, count=5):
    """The instantaneous top CPU processes: /proc ticks over `seconds`."""
    before = all_stats()
    time.sleep(seconds)
    after = all_stats()
    rows = []
    for pid, (comm, _, ticks) in after.items():
        if pid in before:
            percent = 100 * (ticks - before[pid][2]) / CLK_TCK / seconds
            rows.append({"pid": pid, "comm": comm, "cpu_percent_one_core": round(percent, 1)})
    rows.sort(key=lambda row: -row["cpu_percent_one_core"])
    return rows[:count]


def nvidia(query, fmt="csv,noheader,nounits"):
    try:
        out = subprocess.run(["nvidia-smi", f"--query-{query}", f"--format={fmt}"],
                             capture_output=True, text=True, timeout=10, check=True).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    return [[cell.strip() for cell in line.split(",")] for line in out.strip().splitlines() if line.strip()]


def gpu_state():
    rows = nvidia("gpu=name,pstate,clocks.sm,clocks.mem,memory.used,utilization.gpu")
    if not rows:
        return None
    name, pstate, sm, mem, used, util = rows[0]
    return {"name": name, "pstate": pstate, "clocks_sm_mhz": sm, "clocks_mem_mhz": mem,
            "memory_used_mib": used, "utilization_percent": util}


def load_snapshot():
    return {"time": time.time(), "loadavg": Path("/proc/loadavg").read_text().split()[:3],
            "top5_cpu": top_cpu(), "gpu": gpu_state()}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def identity(binary, helper_dir):
    def git(*args):
        return subprocess.run(["git", *args], capture_output=True, text=True, check=False).stdout.strip()

    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                if line.startswith("model name")), None)
    return {
        "commit": git("rev-parse", "HEAD"),
        "worktree_clean": git("status", "--porcelain") == "",
        "binary": binary.name,
        "binary_sha256": sha256(binary),
        "helpers_sha256": {name: sha256(helper_dir / name)
                           for name in ("kinewright-media-helper", "kinewright-media-supervisor")},
        "machine": {"platform": platform.platform(), "cpu": cpu, "cpu_count": os.cpu_count(),
                    "mem_total_kib": int(Path("/proc/meminfo").read_text().split()[1]),
                    "gpu": (nvidia("gpu=name,driver_version") or [[None, None]])[0]},
    }


class Sampler:
    """Samples the process tree every `interval`; VRAM every `vram_interval`."""

    def __init__(self, root, interval=0.05, vram_interval=0.5):
        self.root, self.interval, self.vram_interval = root, interval, vram_interval
        self.samples, self.vram, self.contamination = [], [], []
        self.stop = threading.Event()
        self.started = time.monotonic()

    def run_tree(self):
        while not self.stop.is_set():
            stats = all_stats()
            members = tree(self.root, stats)
            procs = {}
            for pid in members:
                status = read_status(pid)
                if status:
                    procs[pid] = {"comm": exe_name(pid, stats[pid][0]), "cpu_ticks": stats[pid][2], **status}
            if procs:
                self.samples.append({"t": time.monotonic() - self.started, "procs": procs})
            self.stop.wait(self.interval)

    def run_vram(self):
        while not self.stop.is_set():
            rows = nvidia("compute-apps=pid,used_memory")
            members = set(tree(self.root, all_stats()))
            used = {int(pid): int(mib) for pid, mib in (rows or []) if pid.isdigit() and mib.isdigit()}
            state = gpu_state()
            self.vram.append({"t": time.monotonic() - self.started,
                              "tree_mib": {pid: mib for pid, mib in used.items() if pid in members},
                              "compute_apps": len(used),
                              "device_used_mib": state and state["memory_used_mib"]})
            self.stop.wait(self.vram_interval)

    def run_contamination(self):
        """Every 5 s: any process outside the tree above 20% of one core."""
        while not self.stop.wait(4.0):
            members = set(tree(self.root, all_stats()))
            flagged = [row for row in top_cpu(1.0, 10)
                       if row["pid"] not in members and row["cpu_percent_one_core"] > 20]
            if flagged:
                self.contamination.append({"t": time.monotonic() - self.started, "processes": flagged})

    def start(self):
        self.threads = [threading.Thread(target=target, daemon=True)
                        for target in (self.run_tree, self.run_vram, self.run_contamination)]
        for thread in self.threads:
            thread.start()

    def finish(self):
        self.stop.set()
        for thread in self.threads:
            thread.join()


def percentile(values, q):
    """Nearest-rank percentile."""
    ordered = sorted(values)
    if not ordered:
        return None
    rank = max(1, min(len(ordered), -(-len(ordered) * q // 100)))
    return ordered[int(rank) - 1]


def p50_p95(values):
    return {"n": len(values), "p50": percentile(values, 50), "p95": percentile(values, 95),
            "max": max(values) if values else None}


def summarize_memory(sampler):
    per_process = {}
    aggregate = []
    for sample in sampler.samples:
        aggregate.append(sum(proc["rss_kib"] for proc in sample["procs"].values()))
        for pid, proc in sample["procs"].items():
            entry = per_process.setdefault(pid, {"comm": proc["comm"], "rss": [], "hwm_kib": 0, "threads": 0})
            entry["rss"].append(proc["rss_kib"])
            entry["hwm_kib"] = max(entry["hwm_kib"], proc["hwm_kib"])
            entry["threads"] = max(entry["threads"], proc["threads"])
    by_comm = {}
    for pid, entry in per_process.items():
        row = by_comm.setdefault(entry["comm"], {"processes": 0, "peak_rss_kib": 0, "peak_vmhwm_kib": 0,
                                                 "steady_rss_median_kib": [], "max_threads": 0})
        row["processes"] += 1
        row["peak_rss_kib"] = max(row["peak_rss_kib"], max(entry["rss"]))
        row["peak_vmhwm_kib"] = max(row["peak_vmhwm_kib"], entry["hwm_kib"])
        row["steady_rss_median_kib"].append(statistics.median(entry["rss"]))
        row["max_threads"] = max(row["max_threads"], entry["threads"])
    for row in by_comm.values():
        # The median of each process's own median: its steady level.
        row["steady_rss_median_kib"] = statistics.median(row["steady_rss_median_kib"])
    vram_tree = [sum(sample["tree_mib"].values()) for sample in sampler.vram]
    device = [int(sample["device_used_mib"]) for sample in sampler.vram
              if sample["device_used_mib"] and str(sample["device_used_mib"]).isdigit()]
    return {
        "samples": len(sampler.samples),
        "interval_ms": sampler.interval * 1000,
        "per_process_by_name": by_comm,
        "simultaneous_aggregate_rss_kib": {
            "sampled_peak": max(aggregate) if aggregate else None,
            "median": statistics.median(aggregate) if aggregate else None,
            "note": "maximum over samples of the sum of RSS of the processes alive in that sample",
        },
        "vram": {
            "samples": len(sampler.vram),
            "tree_compute_apps_peak_mib": max(vram_tree) if vram_tree else None,
            "tree_ever_listed": any(sample["tree_mib"] for sample in sampler.vram),
            "device_used_mib_min": min(device) if device else None,
            "device_used_mib_max": max(device) if device else None,
            "note": "per-process VRAM from nvidia-smi --query-compute-apps; device_used is "
                    "device-wide (every process on the GPU), an annotation only",
        },
        "contamination": sampler.contamination,
    }


def summarize_workflows(report):
    workflows = report["workflows"]
    phases = {}
    for raster in sorted({workflow["raster"] for workflow in workflows}):
        rows = [workflow for workflow in workflows if workflow["raster"] == raster]
        table = {name: p50_p95([row["phases_ms"][name] for row in rows]) for name in rows[0]["phases_ms"]}
        throughput = [row["saved_bytes"] / 1e6 / (row["phases_ms"]["save"] / 1000) for row in rows]
        table["save_throughput_mb_per_s"] = p50_p95(throughput)
        table["saved_bytes"] = p50_p95([row["saved_bytes"] for row in rows])
        table["outcomes"] = sorted({row["outcome"] for row in rows})
        phases[raster] = table
    comments = [value for workflow in workflows for value in workflow["comment_round_trip_ms"]]
    saves = [value for workflow in workflows for value in workflow["comment_save_request_ms"]]
    return {"workflows": len(workflows), "phases_ms": phases,
            "comment_round_trip_ms": p50_p95(comments), "comment_save_request_ms": p50_p95(saves)}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, required=True, help="the built kinewright_agent test executable")
    parser.add_argument("--lane", choices=["lavapipe", "nvidia"], required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--evidence", type=Path, required=True, help="folder for raw samples and logs")
    parser.add_argument("--summary", type=Path, required=True, help="small committed summary JSON")
    parser.add_argument("--helper-dir", type=Path, default=Path("target/g20"),
                        help="folder of the built kinewright-media-helper and -supervisor")
    parser.add_argument("--label", default="baseline")
    parser.add_argument("--deadline", type=int, default=540, help="seconds before the run is stopped")
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("process sampling requires Linux /proc")
    if args.runs < 1:
        parser.error("runs must be positive")
    evidence = args.evidence.resolve()
    env_root = evidence / "env"
    for name in ("local", "tmp", "state", "data", "cache", "config"):
        (env_root / name).mkdir(parents=True, exist_ok=True)
    report_path = evidence / "workflow.json"
    report_path.unlink(missing_ok=True)
    env = os.environ.copy()
    env.update(
        LOCALAPPDATA=str(env_root / "local"), TMPDIR=str(env_root / "tmp"),
        XDG_STATE_HOME=str(env_root / "state"), XDG_DATA_HOME=str(env_root / "data"),
        XDG_CACHE_HOME=str(env_root / "cache"), XDG_CONFIG_HOME=str(env_root / "config"),
        # The workflow notifies on its own private bus; this keeps any other
        # client off the person's session bus.
        DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(env_root / "no-session-bus"),
        KINEWRIGHT_G20_REPORT=str(report_path), KINEWRIGHT_G20_RUNS=str(args.runs),
        KINEWRIGHT_G20_GPU=args.lane, RUST_BACKTRACE="1",
        KINEWRIGHT_MS2_TEST_BIN_DIR=str(args.helper_dir.resolve()),
    )
    command = [str(args.binary.resolve()), "--exact", TEST, "--nocapture", "--test-threads=1"]
    ident = identity(args.binary.resolve(), args.helper_dir.resolve())
    start_load = load_snapshot()
    log_path = evidence / "test.log"
    started = time.monotonic()
    with log_path.open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        sampler = Sampler(process.pid)
        sampler.start()
        try:
            while process.poll() is None:
                if time.monotonic() - started > args.deadline:
                    raise RuntimeError(f"G20 run exceeded {args.deadline} s")
                time.sleep(0.2)
        finally:
            sampler.finish()
            # Only this run's process group; nothing else is touched.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
    wall = time.monotonic() - started
    end_load = load_snapshot()
    (evidence / "samples.json").write_text(json.dumps({"tree": sampler.samples, "vram": sampler.vram}))
    if process.returncode or not report_path.exists():
        raise SystemExit(f"G20 test exited {process.returncode}; see {log_path}")
    report = json.loads(report_path.read_text())
    summary = {
        "schema_version": 1,
        "label": args.label,
        "lane": args.lane,
        "runs_per_raster": args.runs,
        "comment_delay_mutation": report["comment_delay_mutation"],
        "debug_assertions": report["debug_assertions"],
        "identity": ident,
        "niceness": os.nice(0),
        "wall_seconds": round(wall, 1),
        "load_start": start_load,
        "load_end": end_load,
        "timings": summarize_workflows(report),
        "memory": summarize_memory(sampler),
    }
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(json.dumps(summary, indent=1) + "\n")
    print(json.dumps(summary["timings"]["comment_round_trip_ms"]))
    print(json.dumps(summary["memory"]["simultaneous_aggregate_rss_kib"]))


if __name__ == "__main__":
    main()
