#!/usr/bin/env python3
"""Paired sealed-binary C1 measurements; run with setsid nohup nice -n 19 python3 -I.

The JSON config supplies four ordered lanes, each with name, binary, sha256, cwd,
arguments and optional benchmark_output. Build every binary before invoking this.
"""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def command_output(arguments):
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=15, check=False)
        return {"exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"unavailable": str(error)}


def annotation():
    return {
        "time_ns": time.time_ns(),
        "uptime": command_output(["uptime"]),
        # top's second frame measures the preceding one-second interval.
        "top": command_output(["top", "-b", "-d", "1", "-n", "2", "-w", "512", "-o", "%CPU"]),
        "gpu": command_output(["nvidia-smi", "--query-gpu=pstate,clocks.gr,clocks.mem", "--format=csv,noheader"]),
        "meminfo": Path("/proc/meminfo").read_text(),
    }


def process_ticks():
    result = {}
    for item in Path("/proc").iterdir():
        if not item.name.isdecimal():
            continue
        try:
            raw = (item / "stat").read_text()
            end = raw.rindex(")")
            fields = raw[end + 2:].split()
            result[int(item.name)] = {
                "ticks": int(fields[11]) + int(fields[12]),
                "group": int(fields[2]), "parent": int(fields[1]),
                "name": raw[raw.index("(") + 1:end],
            }
        except (OSError, ValueError, IndexError):
            continue
    return result


def is_workload(pid, observations, root_pid):
    for _ in range(64):
        if pid == root_pid:
            return True
        if pid not in observations or observations[pid]["parent"] == pid:
            return False
        pid = observations[pid]["parent"]
    return False


def available_kib():
    return int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:")))


def run_lane(lane, evidence):
    binary = Path(lane["binary"])
    with binary.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if digest != lane["sha256"]:
        raise SystemExit(f"Sealed binary changed: {binary}")
    directory = evidence / lane["name"]
    directory.mkdir()
    waited = time.monotonic()
    while available_kib() < 6 * 1024 * 1024:
        write(evidence / "status.json", {"lane": lane["name"], "waiting_for_6_gib": True})
        if time.monotonic() - waited > 600:
            raise SystemExit("6 GiB memory prerequisite unavailable")
        time.sleep(5)
    write(evidence / "status.json", {"lane": lane["name"], "running": True})
    write(directory / "start.json", annotation())
    arguments = [sys.executable, "-I", str(Path(__file__).resolve()), "--timed-child", str(directory / "resource.json"), "timeout", "600", "nice", "-n", "10", str(binary), *lane["arguments"]]
    write(directory / "command.json", {**lane, "command": arguments})
    environment = os.environ.copy()
    environment.update({"RUST_TEST_THREADS": "4", "CARGO_BUILD_JOBS": "4"})
    if lane.get("benchmark_output"):
        environment["JA_SAVE_BENCH_OUTPUT"] = str(directory / "save-cost.json")
    previous = process_ticks()
    previous_at = time.monotonic()
    started = time.monotonic()
    samples = []
    with (directory / "stdout.log").open("w") as output, (directory / "stderr.log").open("w") as errors:
        process = subprocess.Popen(arguments, cwd=lane["cwd"], env=environment, stdout=output, stderr=errors)
        while process.poll() is None:
            now = time.monotonic()
            if now - previous_at >= 5:
                current = process_ticks()
                observations = []
                for pid, value in current.items():
                    if pid not in previous:
                        continue
                    percent = 100 * (value["ticks"] - previous[pid]["ticks"]) / os.sysconf("SC_CLK_TCK") / (now - previous_at)
                    if percent > 20:
                        observations.append({"pid": pid, "name": value["name"], "cpu_percent": percent, "other_process": pid != os.getpid() and not is_workload(pid, current, process.pid)})
                samples.append({"seconds": now - started, "above_20_percent": observations})
                previous, previous_at = current, now
                write(directory / "contamination.json", samples)
            time.sleep(1)
    runner_wait = time.monotonic() - started
    resources = json.loads((directory / "resource.json").read_text())
    result = {"exit": process.returncode, **resources, "runner_wait_seconds": runner_wait, "samples": len(samples), "other_load_above_20_percent": any(o["other_process"] for s in samples for o in s["above_20_percent"])}
    write(directory / "end.json", annotation())
    write(directory / "contamination.json", samples)
    if lane.get("benchmark_output") and process.returncode == 0:
        report = json.loads((directory / "save-cost.json").read_text())
        ordered = sorted(report["seconds"])
        result["small_saves"] = {
            "count": len(ordered), "sum_seconds": sum(ordered), "mean_seconds": statistics.mean(ordered),
            "median_seconds": statistics.median(ordered), "p95_seconds": ordered[min(len(ordered) - 1, (95 * len(ordered) + 99) // 100 - 1)],
            "max_writer_peak_decoded_bytes": max(report["writer_peak_decoded_bytes_per_save"]),
            "history_decodes": sum(report["history_decodes_per_save"]),
            "sum_publication_fit_seconds": sum(report["publication_fit_seconds"]),
            "sum_sync_seconds": sum(report["sync_seconds"]),
        }
    write(directory / "result.json", result)
    print(f'{lane["name"]}: {json.dumps(result)}', flush=True)
    if process.returncode != 0:
        raise SystemExit(f'{lane["name"]} failed: exit {process.returncode}; raw output retained')
    return result


def main():
    config = json.loads(Path(sys.argv[1]).read_text())
    evidence = Path(config["evidence"])
    evidence.mkdir()
    write(evidence / "config.json", config)
    results = {}
    for lane in config["lanes"]:
        results[lane["name"]] = run_lane(lane, evidence)
    write(evidence / "results.json", results)
    write(evidence / "status.json", {"complete": True})


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--timed-child":
        started = time.monotonic()
        process = subprocess.Popen(sys.argv[3:])
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
        write(Path(sys.argv[2]), {
            "wall_seconds": time.monotonic() - started,
            "user_seconds": usage.ru_utime,
            "system_seconds": usage.ru_stime,
            "max_rss_kib": usage.ru_maxrss,
            "measurement": "Python monotonic wall and Linux wait4 child resource counters",
        })
        sys.exit(process.returncode if process.returncode >= 0 else 128 - process.returncode)
    else:
        main()
