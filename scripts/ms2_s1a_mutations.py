#!/usr/bin/env python3
"""Run MS2's kill-chain red witnesses, restore source, then rerun controls.

Invoke after `source scripts/setup-ffmpeg.sh`, without an outer flock.
Only this checkout's two audited source files and its target directory change.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/ms2-s1a/kill-mutations"
BOUNDARY = ROOT / "crates/kinewright-job-sys/src/media_boundary.rs"
ACTOR = ROOT / "crates/kinewright-media-client/src/linux/actor.rs"
LOSS = "watchdog_disabled_degraded_app_and_supervisor_loss"
RACE = "watchdog_disabled_spawner_abort_and_payload_prearm_race"
SIGNAL = "u64::from(libc::SIGKILL.cast_unsigned()),"
CASES = [
    ("shim-parent-death", BOUNDARY, SIGNAL,
     "if expected == 0 { u64::from(libc::SIGKILL.cast_unsigned()) } else { 0 },",
     LOSS, "shim"),
    ("payload-parent-death", BOUNDARY, SIGNAL,
     "if expected != 0 { u64::from(libc::SIGKILL.cast_unsigned()) } else { 0 },",
     LOSS, "payload"),
    ("payload-parent-pidfd", BOUNDARY, "if let Some(parent) = parent {",
     "if let Some(parent) = parent.filter(|_| expected != 0) {", RACE, "pidfd"),
    ("spawner-abort-guard", ACTOR, "std::process::abort();",
     "std::hint::black_box(());", RACE, "abort"),
]


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cargo(label: str, args: list[str]) -> tuple[int, Path]:
    log = OUT / (label + ".log")
    command = ["flock", "-o", "/tmp/kinewright-cargo-heavy.lock", "env",
               "CARGO_BUILD_JOBS=4", "RUST_TEST_THREADS=1", "nice", "-n", "10",
               "cargo", "+1.99.1", *args]
    with log.open("w") as output:
        result = subprocess.run(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                                check=False)
    print(f"{label}: exit={result.returncode}, log={log.relative_to(ROOT)}", flush=True)
    return result.returncode, log


def build(label: str) -> None:
    code, _ = cargo(label, ["build", "-p", "kinewright-media-helper", "-p",
                            "kinewright-media-supervisor", "--bins", "--features",
                            "kinewright-media-helper/slow-tests"])
    if code:
        raise RuntimeError("native witness build failed")


def expected_failure(text: str, kind: str) -> bool:
    if "test result: FAILED." not in text or "could not compile" in text:
        return False
    if kind == "pidfd":
        return "payload failed its own pre-arm pidfd check while the app was stopped" in text
    if kind == "abort":
        return "fixture did not finish native cleanup" in text
    target = re.search(r"MS2_KILL_TARGET mode=degraded stage=before-ready loss=app "
                       r"app=(\d+) shim=(\d+) payload=(\d+)", text)
    if not target:
        return False
    pid = target.group(2 if kind == "shim" else 3)
    return f"live or unreaped owned process {pid}" in text


def main() -> None:
    if not os.environ.get("FFMPEG_DIR"):
        raise SystemExit("source scripts/setup-ffmpeg.sh first; do not add an outer flock")
    OUT.mkdir(parents=True, exist_ok=True)
    originals = {path: path.read_bytes() for path in (BOUNDARY, ACTOR)}
    rows: list[dict] = []
    controls = None
    try:
        for name, path, before, after, test, kind in CASES:
            source = originals[path].decode()
            if source.count(before) != 1:
                raise RuntimeError(f"{name}: source anchor is not unique")
            path.write_text(source.replace(before, after))
            try:
                build(name + "-build")
                helper_hash = digest(ROOT / "target/debug/kinewright-media-helper-witness")
                code, log = cargo(name + "-red", ["test", "-p", "kinewright-media-client",
                    "--features", "slow-tests", "--test", "witness_native", test,
                    "--", "--exact", "--nocapture"])
                rejected = code != 0 and expected_failure(log.read_text(), kind)
                rows.append({"mutation": name, "test": test, "exit": code,
                             "expected_assertion_failed": rejected,
                             "helper_sha256": helper_hash,
                             "log": str(log.relative_to(ROOT)), "log_sha256": digest(log)})
                if not rejected:
                    raise RuntimeError(f"{name}: mutation did not fail at its intended assertion")
            finally:
                path.write_bytes(originals[path])
    finally:
        for path, data in originals.items():
            path.write_bytes(data)
        # Never leave a mutant executable as the checkout's helper, including
        # after an unexpected red result or interruption.
        build("restored-build")
        code, log = cargo("restored-controls", ["test", "-p", "kinewright-media-client",
            "--features", "slow-tests", "--test", "witness_native", "--", "--show-output"])
        controls = {"exit": code, "log": str(log.relative_to(ROOT)),
                    "log_sha256": digest(log)}
        report = {"mutations": rows, "restored_controls": controls,
                  "source_restored": all(path.read_bytes() == data for path, data in originals.items())}
        (OUT / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    if controls["exit"] != 0:
        raise SystemExit("restored native controls failed")


if __name__ == "__main__":
    main()
