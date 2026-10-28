#!/usr/bin/env -S nice -n 19 python3 -I
"""Run the PO0 desktop scenes. Missing measurements never pass a bar."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]


def command(args, env=None):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=15, env=env)
        return {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": str(error)}


def proc_cpu():
    if not Path("/proc/self/stat").exists():
        return None
    rows = {}
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            raw = path.read_text()
            name = raw[raw.index("(") + 1:raw.rindex(")")]
            fields = raw[raw.rindex(")") + 2:].split()
            rows[int(path.parent.name)] = (name, int(fields[11]) + int(fields[12]), fields[19])
        except (OSError, ValueError, IndexError):
            pass
    return (time.monotonic(), rows)


def cpu_delta(before, after):
    if before is None or after is None:
        return None
    seconds = after[0] - before[0]
    ticks = os.sysconf("SC_CLK_TCK")
    rows = []
    for pid, (name, cpu, identity) in after[1].items():
        old = before[1].get(pid)
        if old is not None and old[2] == identity:
            rows.append({"pid": pid, "name": name, "cpu_percent_of_one_core": max(0, cpu-old[1])/ticks/seconds*100})
    return sorted(rows, key=lambda row: row["cpu_percent_of_one_core"], reverse=True)


def instant_cpu():
    before = proc_cpu()
    time.sleep(.2)
    rows = cpu_delta(before, proc_cpu())
    return None if rows is None else rows[:5]


def load(env):
    return {"time": time.time(), "load": command(["uptime"]),
            "instantaneous_top5_cpu": instant_cpu(), "lifetime_average_top_cpu": command(["ps", "-eo", "pid,comm,pcpu", "--sort=-pcpu"]).get("stdout", "NOT RUN").splitlines()[:6],
            "gpu": command(["nvidia-smi", "--query-gpu=name,pstate,clocks.current.graphics,clocks.current.memory,utilization.gpu,memory.used", "--format=csv,noheader"]),
            "display": command(["hyprctl", "monitors", "-j"]),
            "session": {key: env.get(key) for key in ("XDG_SESSION_TYPE", "WAYLAND_DISPLAY", "DISPLAY")}}


def at(obj, path):
    for key in path.split("."):
        if not isinstance(obj, dict):
            return None
        obj = obj.get(key)
    return obj


def bars(m, lane, hz):
    rows = []
    def check(name, value, limit, upper=True):
        status = "NOT RUN" if value is None else "PASS" if (value <= limit if upper else value >= limit) else "FAIL"
        rows.append({"bar": name, "value": value, "limit": limit, "status": status})
    check("requested repetitions complete", m.get("completed_repetitions"), m.get("requested_repetitions", 200), False)
    if m.get("failures"):
        rows.append({"bar": "scene correctness", "status": "FAIL", "value": m["failures"]})
    if lane == "nvidia":
        if abs(hz - 120) < 1:
            check("CPU p50 ms", at(m, "cpu_frame_ms.p50"), 3)
            check("CPU p99 ms", at(m, "cpu_frame_ms.p99"), 6)
            check("interval p99 ms", at(m, "interval_ms.p99"), 1.25 * 1000 / hz)
            check("consecutive drops", m.get("consecutive_drops"), False)
        elif abs(hz - 60) < 1:
            check("CPU p99 ms", at(m, "cpu_frame_ms.p99"), 10)
        check("dropped animated frames percent", m.get("dropped_percent"), .5)
        idle = m.get("static_baseline_cpu_percent_of_one_core")
        animated = m.get("animated_cpu_percent_of_one_core")
        check("added CPU percent of one core", None if idle is None or animated is None else animated - idle, 10)
    else:
        check("interval p99 ms", at(m, "interval_ms.p99"), 33)
        if lane == "warp":
            check("dropped animated frames percent", m.get("dropped_percent"), 5)
    if m.get("moment") == 1:
        window = at(m, "repaint_windows_ms.max")
        if m.get("spring_variant", "").startswith("deadline"):
            check("deadline-snap repaint window ms", window, 520)
        else:
            status = "NOT RUN" if window is None else "PASS" if window <= 520 else "DESIGN-DOC CONFLICT"
            rows.append({"bar": "spring repaint window ms", "value": window, "limit": 520, "status": status})
        check("motion repaints after actual settle", m.get("motion_repaints_after_settle"), 0)
    completions = m.get("transition_completions", [])
    curves = [abs(c["elapsed_ms"] - c["nominal_ms"]) for c in completions if c["token"] not in ("Spring", "Panel", "Instant")]
    if curves:
        check("completed curve duration error ms", max(curves), 1000 / hz)
    springs = [c for c in completions if c["token"] in ("Spring", "Panel")]
    if springs and not m.get("spring_variant", "").startswith("deadline"):
        excess = max(abs(c["integrated_ms"] - c["nominal_ms"]) for c in springs)
        rows.append({"bar": "equations versus spring token window ms", "value": excess, "limit": 1000 / hz,
                     "status": "DESIGN-DOC CONFLICT" if excess > 1000 / hz else "PASS"})
    if m.get("moment") == 3:
        check("dissolve start delay frames", at(m, "dissolve_start_delay_frames.max"), 1)
        if m.get("playback"):
            check("PF1 G1 late or dropped percent", m.get("pf1_window_bad_percent"), 1)
            check("PF1 G1 presented interval p95 ms", at(m,"pf1_window_present_ms.p95"), 50)
    if m.get("moment") == 6 and m.get("effects"):
        check("light samples actually applied", m.get("downsample_updates"), 2 if m.get("playback") else 1, False)
        check("light plus glass plus grain GPU max ms", at(m, "gpu_effects_ms.max"), .3 if lane == "nvidia" else 2)
        if lane == "nvidia":
            check("downsample worker max ms", at(m, "downsample_worker_ms.max"), .5)
        rows.append({"bar": "near-black banding, Riel's eye", "status": "NOT RUN"})
    if m.get("moment") == 7:
        check("cold visible strip max ms", at(m, "strip_fill_ms.max"), 3000 if m.get("playback") else 1000)
        check("atlas bytes", m.get("atlas_bytes"), 16 * 1024 * 1024)
        if m.get("playback"):
            check("PF1 G1 late or dropped percent", m.get("pf1_window_bad_percent"), 1)
            check("PF1 G1 presented interval p95 ms", at(m,"pf1_window_present_ms.p95"), 50)
    if m.get("moment") == 10:
        check("tracked pin samples", m.get("tracked_pin_samples"), 2, False)
    return rows


def write_summary(output, runs, args, provenance):
    document = {"schema_version": 1, "lane": args.lane, "hz": args.hz, "provenance": provenance, "runs": runs}
    (output / "results.json").write_text(json.dumps(document, indent=2) + "\n")
    lines = [f"# PO0 {args.lane} at {args.hz:g} Hz", "", "| Run | Bar | Measured | Limit | Verdict |", "| --- | --- | --- | --- | --- |"]
    for run in runs:
        for bar in run["bars"]:
            value = str(bar.get("value", "" )).replace("|", "/").replace("\n", " ")
            lines.append(f"| {run['name']} | {bar['bar']} | {value} | {bar.get('limit', '')} | {bar['status']} |")
    (output / "results.md").write_text("\n".join(lines) + "\n")


def run_one(args, output, moment, playback, effects, repeats, pair, snap=False):
    name = f"m{moment:02d}-{'play' if playback else 'paused'}-{'on' if effects else 'off'}-p{pair}" + ("-snap520" if snap else "")
    report = output / f"{name}.json"
    env = os.environ.copy()
    env.update(KINEWRIGHT_MOTION_PROBE="1", KINEWRIGHT_PROBE_MOMENT=str(moment),
               KINEWRIGHT_PROBE_MEDIA=str(args.media), KINEWRIGHT_PROBE_PLAYBACK=str(int(playback)),
               KINEWRIGHT_PROBE_EFFECTS=str(int(effects)), KINEWRIGHT_PROBE_REPEATS=str(repeats),
               KINEWRIGHT_PROBE_HZ=str(args.hz), KINEWRIGHT_PERF_REPORT=str(report),
               KINEWRIGHT_PERF_SECONDS="580", KINEWRIGHT_PROBE_DEADLINE_SNAP=str(int(snap)),
               XDG_CACHE_HOME=str(output / "cache"), XDG_CONFIG_HOME=str(output / "config"), XDG_DATA_HOME=str(output / "data"), TMPDIR=str(output / "tmp"))
    (output / "tmp").mkdir(exist_ok=True)
    if args.lane == "lavapipe":
        env.update(VK_DRIVER_FILES="/usr/share/vulkan/icd.d/lvp_icd.json", VK_ICD_FILENAMES="/usr/share/vulkan/icd.d/lvp_icd.json", WGPU_ADAPTER_NAME="llvmpipe")
    elif args.lane == "nvidia":
        env.update(VK_DRIVER_FILES="/usr/share/vulkan/icd.d/nvidia_icd.json", VK_ICD_FILENAMES="/usr/share/vulkan/icd.d/nvidia_icd.json", WGPU_ADAPTER_NAME="NVIDIA")
    elif args.lane == "warp" and os.name == "nt":
        env.update(WGPU_BACKEND="dx12", WGPU_ADAPTER_NAME="Microsoft Basic Render Driver")
    elif os.name != "nt":
        raise SystemExit("WARP is a Windows lane. Never substitute a Linux adapter.")
    before = load(env)
    contamination = []
    stopped = threading.Event()
    def sample():
        previous = proc_cpu()
        while not stopped.wait(5):
            current = proc_cpu()
            records = cpu_delta(previous, current)
            previous = current
            flags = None if records is None else [row for row in records if row["cpu_percent_of_one_core"] > 20 and row["pid"] not in (process.pid, os.getpid())]
            entry = {"time": time.time(), "other_processes_above_20_percent": flags, "instantaneous_top5": None if records is None else records[:5]}
            try:
                clients = json.loads(command(["hyprctl", "clients", "-j"])["stdout"])
                monitors = json.loads(command(["hyprctl", "monitors", "-j"])["stdout"])
                client = next(client for client in clients if client.get("pid") == process.pid)
                monitor = next(monitor for monitor in monitors if monitor["id"] == client["monitor"])
                entry["app_monitor"] = {key: monitor[key] for key in ("name", "refreshRate", "width", "height")}
            except (KeyError, ValueError, StopIteration):
                pass
            contamination.append(entry)
    placement = None
    def place_own_window():
        # Move only this disposable probe window; never change monitor modes or other clients.
        if os.environ.get("XDG_SESSION_TYPE") != "wayland":
            return None
        for attempt in range(20):
            try:
                monitors = json.loads(command(["hyprctl", "monitors", "-j"])["stdout"])
                monitor = next(m for m in monitors if abs(m["refreshRate"] - args.hz) < .1)
                clients = json.loads(command(["hyprctl", "clients", "-j"])["stdout"])
                client = next(c for c in clients if c.get("pid") == process.pid)
                lua = "hl.dsp.window.move({monitor=" + json.dumps(monitor["name"]) + ",follow=false,window=" + json.dumps("address:" + client["address"]) + "})"
                result = command(["hyprctl", "dispatch", lua])
                if result.get("returncode") != 0:
                    result["legacy_fallback"] = command(["hyprctl", "dispatch", "movewindow", f"mon:{monitor['name']},address:{client['address']}"])
                return result
            except (KeyError, ValueError, StopIteration):
                time.sleep(.1)
        return {"error": "probe client or matching current-rate monitor unavailable"}
    sampler = threading.Thread(target=sample, daemon=True)
    print(f"START {name}", flush=True)
    started = time.time()
    with (output / f"{name}.log").open("w") as log:
        try:
            process = subprocess.Popen([str(args.binary)], env=env, stdout=log, stderr=subprocess.STDOUT)
            placement = place_own_window()
            sampler.start()
            code = process.wait(timeout=590)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            code = "TIMEOUT"
    stopped.set()
    sampler.join()
    raw = json.loads(report.read_text()) if report.exists() else {}
    motion = raw.get("motion", {})
    rows = bars(motion, args.lane, args.hz) if motion else [{"bar": "scene ran and wrote measurements", "status": "NOT RUN", "value": code}]
    if motion.get("playback"):
        rows.append({"bar": "real playback clock advanced", "status": "PASS" if motion.get("max_position_frame", 0) > 2 else "FAIL", "value": motion.get("max_position_frame")})
    rates = [entry["app_monitor"]["refreshRate"] for entry in contamination if "app_monitor" in entry]
    rows.append({"bar": "actual display refresh matches lane", "status": "NOT RUN" if not rates else "PASS" if all(abs(rate - args.hz) < .1 for rate in rates) else "FAIL", "value": rates})
    rows.append({"bar": "process exited cleanly", "status": "PASS" if code == 0 else "FAIL", "value": code})
    adapter = motion.get("adapter", "")
    if motion and ((args.lane == "nvidia" and "NVIDIA" not in adapter) or (args.lane == "lavapipe" and "llvmpipe" not in adapter.lower())):
        rows = [{"bar": "requested adapter actually used", "status": "FAIL", "value": adapter}]
    run = {"name": name, "returncode": code, "seconds": time.time() - started, "before": before,
           "after": load(env), "own_window_placement": placement, "contamination_5s": contamination, "raw": raw, "bars": rows}
    print(f"END {name} {code} {motion.get('completed_repetitions')} repetitions", flush=True)
    return run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lane", choices=["nvidia", "lavapipe", "warp"], required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / ("target/release/kinewright-app.exe" if os.name=="nt" else "target/release/kinewright-app"))
    parser.add_argument("--media", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--hz", type=float, required=True, help="Actual current display refresh, without changing the display mode")
    parser.add_argument("--moments", default="1,2,3,4,5,6,7,8,9,10")
    parser.add_argument("--repeats", type=int, default=200)
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--snap-variant", action="store_true")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.media = args.media.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.lane == "warp" and os.name != "nt":
        raise SystemExit("Windows WARP NOT RUN. Use this command on the Windows VM.")
    digest = hashlib.file_digest(args.binary.open("rb"), "sha256").hexdigest()
    provenance = {"commit": command(["git", "rev-parse", "HEAD"]), "dirty": command(["git", "status", "--short"]),
                  "binary": str(args.binary), "binary_sha256": digest, "media": str(args.media),
                  "source_probe": command(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(args.media)]),
                  "command": os.sys.argv, "nproc": command(["nproc"])}
    runs = []
    for moment in [int(value) for value in args.moments.split(",")]:
        for playback in [False, True]:
            pairs = args.pairs
            for pair in range(pairs):
                for effects in ([False, True] if moment == 6 else [True]):
                    repeats = max(1, (args.repeats + pairs - 1) // pairs)
                    runs.append(run_one(args, output, moment, playback, effects, repeats, pair, args.snap_variant))
                    write_summary(output, runs, args, provenance)
    write_summary(output, runs, args, provenance)


if __name__ == "__main__":
    main()
