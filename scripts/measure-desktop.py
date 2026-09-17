#!/usr/bin/env python3
"""Sample a release desktop build on Linux; write raw, repeatable local evidence.

Run after sourcing scripts/setup-ffmpeg.sh. Uses fresh XDG data/cache directories,
but leaves HOME and installed harness discovery intact. RSS/CPU cover the app
process, excluding driver GPU allocations and harness subprocesses. First UI is
the completion of its first UI callback, not compositor presentation latency.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import tempfile
import time


def sample_process(pid):
    root = Path(f"/proc/{pid}")
    try:
        status = dict(
            line.split(":", 1) for line in (root / "status").read_text().splitlines()
        )
        # comm may contain spaces or parentheses. Fields after its closing
        # parenthesis start at field 3; utime/stime are fields 14 and 15.
        fields = (root / "stat").read_text().rsplit(")", 1)[1].split()
        return {
            "rss_kib": int(status.get("VmRSS", "0 kB").split()[0]),
            "threads": int(status["Threads"]),
            "cpu_seconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK"),
        }
    except (OSError, ValueError, KeyError, IndexError):
        return None


def measure(args, index):
    with tempfile.TemporaryDirectory(prefix="kinewright-desktop-") as temporary:
        root = Path(temporary)
        report = root / "report.json"
        env = os.environ.copy()
        # Existing screenshot settings force additional repaints or early exit.
        for key in list(env):
            if key.startswith("KINEWRIGHT_SCREENSHOT_"):
                del env[key]
        env.update(
            XDG_DATA_HOME=str(root / "data"),
            XDG_CACHE_HOME=str(root / "cache"),
            KINEWRIGHT_PERF_REPORT=str(report),
            KINEWRIGHT_PERF_SECONDS=str(args.seconds),
        )
        command = [str(args.binary.resolve())]
        if args.project:
            command.append(str(args.project.resolve()))
        samples = []
        started = time.monotonic()
        with (root / "stderr.log").open("w+") as log:
            process = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                while process.poll() is None:
                    elapsed = time.monotonic() - started
                    if elapsed > args.seconds + 45:
                        raise RuntimeError("desktop measurement timed out")
                    sample = sample_process(process.pid)
                    if sample:
                        samples.append({"elapsed_seconds": elapsed, **sample})
                    time.sleep(0.05)
                if process.returncode or not report.exists():
                    log.seek(0)
                    raise RuntimeError(f"desktop exited {process.returncode}: {log.read()[-6000:]}")
            finally:
                # Probe CLIs can outlive the GUI. Only this run's process group
                # is terminated; no existing app or user agent is touched.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                # The parent can exit before a probe that ignores SIGTERM.
                # Finish cleanup of this run's group before starting another.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        result = json.loads(report.read_text())
        idle = [sample for sample in samples if 2 <= sample["elapsed_seconds"] <= args.seconds]
        result.update(
            run=index,
            peak_rss_kib=max(sample["rss_kib"] for sample in samples),
            idle_rss_median_kib=statistics.median(sample["rss_kib"] for sample in idle) if idle else None,
            idle_threads_median=statistics.median(sample["threads"] for sample in idle) if idle else None,
            idle_cpu_percent_one_core=(
                100 * (idle[-1]["cpu_seconds"] - idle[0]["cpu_seconds"])
                / (idle[-1]["elapsed_seconds"] - idle[0]["elapsed_seconds"])
            ) if len(idle) > 1 else None,
            process_samples=samples,
        )
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/kinewright-app"))
    parser.add_argument("--project", type=Path)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("process sampling currently requires Linux /proc")
    if args.runs < 1 or not 3 <= args.seconds <= 300:
        parser.error("runs must be positive; seconds must be between 3 and 300")
    results = []
    for index in range(1, args.runs + 1):
        results.append(measure(args, index))
        print(f"completed desktop sample {index}/{args.runs}", flush=True)
    metrics = ["first_ui_ms", "first_preview_ms", "ui_median_ms", "ui_p95_ms", "peak_rss_kib",
               "idle_rss_median_kib", "idle_threads_median", "idle_cpu_percent_one_core"]
    summary = {key: statistics.median(values) if (values := [r[key] for r in results if r[key] is not None]) else None
               for key in metrics}
    artifact = {"schema_version": 1, "platform": platform.platform(), "cpu_count": os.cpu_count(),
                "binary": str(args.binary.resolve()), "project": str(args.project.resolve()) if args.project else None,
                "seconds": args.seconds, "summary_medians": summary, "runs": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(artifact, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
