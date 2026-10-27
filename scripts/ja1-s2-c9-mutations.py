#!/usr/bin/env python3
"""Targeted C9 runtime mutations; run at nice 19 with -I and same-shell FFmpeg setup."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
ROOT = Path(__file__).resolve().parent.parent

def replace(text, before, after):
    if text.count(before) != 1:
        raise RuntimeError(f"mutation anchor is not unique: {before!r}")
    return text.replace(before, after, 1)

def cases():
    return json.loads((ROOT / "scripts/ja1-s2-c9-mutation-cases.json").read_text())

def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def run_case(case, evidence):
    directory = evidence / case["name"]
    directory.mkdir()
    paths = {name: (ROOT / name).read_bytes() for name, _, _ in case["changes"]}
    original = {name: data.decode() for name, data in paths.items()}
    changed = dict(original)
    for name, before, after in case["changes"]:
        changed[name] = replace(changed[name], before, after)
    write_json(directory / "source.json", {
        name: hashlib.sha256(data).hexdigest() for name, data in paths.items()})
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("JA_", "KINEWRIGHT_", "REV2_"))}
    environment.update(CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4")
    environment.setdefault("CARGO_TARGET_DIR", str(ROOT / "target/debug-c9"))
    environment.setdefault("TMPDIR", str(ROOT / "target/c9-tmp"))
    Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    environment.update(case["environment"])
    command = ["nice", "-n", "10", "cargo", "test", "-j", "4", "-p", case["package"]]
    if case["production"]:
        command += ["--no-default-features"]
    if case["features"]:
        command += ["--features", case["features"]]
    command += [case["kind"]]
    if case["kind"] != "--lib":
        command += [case["target"]]
    command += [case["witness"], "--", "--exact", "--test-threads=1"]
    results = []
    try:
        for phase in ("red", "green"):
            for name in paths:
                (ROOT / name).write_text(changed[name] if phase == "red" else original[name])
            phase_environment = dict(case["environment"])
            if case["package"] == "kinewright-app":
                phase_environment["JA_MANUAL_APP_OUTPUT"] = str(directory / (phase + "-app-observations"))
            environment.update(phase_environment)
            write_json(directory / f"{phase}-command.json", {"argv": command, "environment": phase_environment})
            available = int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:")))
            if available < 6 * 1024 * 1024:
                raise RuntimeError("required 6 GiB available memory is unavailable")
            started = time.monotonic()
            with (directory / f"{phase}.stdout").open("w") as stdout, (directory / f"{phase}.stderr").open("w") as stderr:
                process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=stdout,
                                           stderr=stderr, start_new_session=True)
                try:
                    code = process.wait(timeout=600)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    raise RuntimeError("mutation timed out; its owned process group was terminated and reaped") from None
            # Every child belongs to this test session. Retire orphan host children
            # after a failed assertion; no other project/session is touched.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            text = (directory / f"{phase}.stdout").read_text()
            summaries = re.findall(r"^test result:.*$", text, re.M)
            expected = (code == 101 and len(summaries) == 1
                        and "0 passed; 1 failed; 0 ignored" in summaries[0]) if phase == "red" else (
                        code == 0 and len(summaries) == 1
                        and "1 passed; 0 failed; 0 ignored" in summaries[0])
            if phase == "red" and case["red_text"]:
                expected = expected and case["red_text"] in text
            if case["package"] == "kinewright-app" and case["witness"].endswith("g05l_live_coalesced_preview_precedes_disk_ack"):
                summary = json.loads((Path(phase_environment["JA_MANUAL_APP_OUTPUT"]) / "revised/summary.json").read_text())
                expected = expected and summary["exit_code"] == (1 if phase == "red" else 0)
                expected = expected and all(a["passed"] for a in summary["assertions"] if a["evidence_class"] == "control")
            binaries = re.findall(r"^\s*Running .* \(([^()]+)\)$", (directory / f"{phase}.stderr").read_text(), re.M)
            if len(binaries) != 1:
                expected = False
            binary = (ROOT / binaries[0]).resolve() if len(binaries) == 1 else None
            supporting = Path(environment["CARGO_TARGET_DIR"]) / "debug" / (
                "kinewright.exe" if os.name == "nt" else "kinewright")
            row = dict(phase=phase, exit=code, summaries=summaries,
                       binary=str(binary) if binary else None,
                       binary_sha256=hashlib.file_digest(binary.open("rb"), "sha256").hexdigest() if binary and binary.is_file() else None,
                       supporting_host_binary=str(supporting) if case["package"] != "kinewright-project" and supporting.is_file() else None,
                       supporting_host_sha256=hashlib.file_digest(supporting.open("rb"), "sha256").hexdigest() if case["package"] != "kinewright-project" and supporting.is_file() else None,
                       seconds=time.monotonic() - started, accepted=bool(expected))
            write_json(directory / f"{phase}-result.json", row)
            results.append(row)
            print(case["name"], phase, code, summaries, flush=True)
            if not expected:
                raise RuntimeError(f"{case['name']} {phase} is not the required runtime outcome; retained logs explain it")
    finally:
        for name, data in paths.items():
            (ROOT / name).write_bytes(data)
        for name, data in paths.items():
            if (ROOT / name).read_bytes() != data:
                raise RuntimeError(f"source restoration failed: {name}")
    return dict(name=case["name"], witness=case["witness"], results=results)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--group", choices=("host", "project", "app", "core"))
    parser.add_argument("--check", action="store_true", help="validate mutation anchors without changing source")
    arguments = parser.parse_args()
    packages = {"host": "kinewright-agent", "project": "kinewright-project", "app": "kinewright-app", "core": "kinewright-core"}
    chosen = [case for case in cases()
              if not case.get("superseded_by")
              and (not arguments.case or case["name"] in arguments.case)
              and (not arguments.group or case["package"] == packages[arguments.group])]
    if not chosen or set(arguments.case) - {case["name"] for case in chosen}:
        raise SystemExit("empty or unknown mutation selection")
    if arguments.check:
        for case in chosen:
            changed = {name: (ROOT / name).read_text() for name, _, _ in case["changes"]}
            for name, before, after in case["changes"]:
                changed[name] = replace(changed[name], before, after)
            print(case["name"] + ": anchors valid")
        return
    arguments.evidence.mkdir(parents=True, exist_ok=False)
    write_json(arguments.evidence / "cases.json", chosen)
    results = []
    for case in chosen:
        results.append(run_case(case, arguments.evidence))
        write_json(arguments.evidence / "results.json", results)


if __name__ == "__main__":
    main()
