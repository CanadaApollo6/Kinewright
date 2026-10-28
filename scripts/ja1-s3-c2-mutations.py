#!/usr/bin/env python3
"""Run C2 data-loss and security witnesses with literal red/restored-green evidence."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(case, log):
    args = ["test", "-p", case["package"]]
    if case.get("features"):
        args += ["--features", case["features"]]
    if case.get("kind"):
        args += [case["kind"]]
        if case.get("target"):
            args += [case["target"]]
    args += [case["witness"], "--", "--exact", "--test-threads=1", "--nocapture"]
    command = (
        "set -e\nsource scripts/setup-ffmpeg.sh\n"
        'c1_runtime="$PWD/target/ja1-s3-c2/runtime"\n'
        'export LOCALAPPDATA="$PWD/target/c1fix-localappdata" TMPDIR="$c1_runtime/tmp"\n'
        'export XDG_STATE_HOME="$c1_runtime/state" XDG_DATA_HOME="$c1_runtime/share"\n'
        'export XDG_CACHE_HOME="$c1_runtime/cache" XDG_CONFIG_HOME="$c1_runtime/config"\n'
        'mkdir -p "$LOCALAPPDATA/Kinewright/recovery" "$TMPDIR" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"\n'
        "test \"$(awk '/MemAvailable/ {print $2}' /proc/meminfo)\" -ge 6291456\n"
        "flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 "
        "RUST_TEST_THREADS=4 timeout --kill-after=5s 600s nice -n 10 cargo " + shlex.join(args)
    )
    process = subprocess.Popen(
        ["bash", "-c", command], cwd=ROOT, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, start_new_session=True,
    )
    try:
        # The timeout inside flock bounds execution without expiring in the queue.
        raw = process.communicate()[0]
    except BaseException:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        raise
    with gzip.open(log, "wb") as stream:
        stream.write(raw)
    return process.returncode, raw.decode(errors="replace"), command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", default="scripts/ja1-s3-c2-mutation-cases.json")
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--evidence", default="docs/reports/ja1-s3/c2-evidence/mutations")
    args = parser.parse_args()
    cases = json.loads((ROOT / args.cases).read_text())
    selected = [c for c in cases if not args.case or c["name"] in args.case]
    if not selected or (args.case and len(selected) != len(args.case)):
        raise RuntimeError("unknown or empty case selection")
    for case in selected:
        for path, old, _new in case["changes"]:
            if (ROOT / path).read_text().count(old) != 1:
                raise RuntimeError(f"{case['name']}: {path} requires one exact anchor")
        if args.list:
            print(case["name"], case["witness"], sep="\t")
    if args.list:
        return
    evidence = ROOT / args.evidence
    evidence.mkdir(parents=True, exist_ok=True)
    manifest_bytes = (evidence.parent / "source-before-gates.json").read_bytes()
    manifest = json.loads(manifest_bytes)["sha256"]
    for case in selected:
        for path, expected in manifest.items():
            if path.startswith("crates/") and digest((ROOT / path).read_bytes()) != expected:
                raise RuntimeError(f"source no longer matches the gated manifest: {path}")
        originals = {path: (ROOT / path).read_bytes() for path, _, _ in case["changes"]}
        result = {"name": case["name"], "witness": case["witness"], "accepted": False,
                  "changes": case["changes"],
                  "expected_red": {"text": case["red_text"],
                                   "location": case.get("red_location")},
                  "baseline_manifest_sha256": digest(manifest_bytes),
                  "witness_sha256": digest((ROOT / case["witness_source"]).read_bytes()),
                  "source_sha256": {p: digest(b) for p, b in originals.items()}}
        status_path = evidence / f"{case['name']}.json"
        try:
            for path, old, new in case["changes"]:
                current = (ROOT / path).read_text()
                if current.count(old) != 1:
                    raise RuntimeError("source changed during mutation campaign")
                (ROOT / path).write_text(current.replace(old, new, 1))
            result["mutated_source_sha256"] = {
                path: digest((ROOT / path).read_bytes()) for path in originals
            }
            code, red, command = run(case, evidence / f"{case['name']}-red.log.gz")
            result.update(red_exit=code, command=command)
            result["red_log_sha256"] = digest((evidence / f"{case['name']}-red.log.gz").read_bytes())
            result["red_summary"] = [line for line in red.splitlines() if "test result:" in line]
            red_ok = (code == 101 and "test result: FAILED. 0 passed; 1 failed; 0 ignored;" in red
                      and case["red_text"] in red and "could not compile" not in red
                      and case.get("red_location", "") in red
                      and "error[E" not in red)
            for path, original in originals.items():
                (ROOT / path).write_bytes(original)
            code, green, _command = run(case, evidence / f"{case['name']}-green.log.gz")
            result["green_exit"] = code
            result["green_log_sha256"] = digest((evidence / f"{case['name']}-green.log.gz").read_bytes())
            result["green_summary"] = [line for line in green.splitlines() if "test result:" in line]
            result["restored"] = all((ROOT / p).read_bytes() == b for p, b in originals.items())
            result["accepted"] = (red_ok and code == 0 and result["restored"]
                                  and "test result: ok. 1 passed; 0 failed; 0 ignored;" in green)
            if not result["accepted"]:
                raise RuntimeError(f"{case['name']}: rejected pair; see {status_path}")
            print(f"{case['name']}: runtime red 101; restored green 0", flush=True)
        finally:
            for path, original in originals.items():
                (ROOT / path).write_bytes(original)
            status_path.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    main()
