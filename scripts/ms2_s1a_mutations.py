#!/usr/bin/env python3
"""Run MS2's kill-chain and probe-bound red witnesses, restore source, then rerun controls.

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
# A silent helper must get its typed failure within the design's probe bound,
# in both resource modes. Each removal leaves the caller waiting until the
# witness's named outer deadline fires.
SILENT = "a_silent_helper_fails_typed_within_the_probe_bound"
BOUNDS = [
    ("probe-reap-budget", "&& job.answer_at.is_some_and(|deadline| now >= deadline)",
     "&& false", "held"),
    ("probe-deadline", "} else if now >= job.deadline {", "} else if false {", "unheld"),
]
MODES = ("primary", "degraded")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cargo(label: str, args: list[str], extra: tuple[str, ...] = ()) -> tuple[int, Path]:
    log = OUT / (label + ".log")
    command = ["flock", "-o", "/tmp/kinewright-cargo-heavy.lock", "env", *extra,
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


def mode_env(mode: str) -> tuple[str, ...]:
    # Degraded mode: a session bus address with nothing behind it.
    if mode == "degraded":
        return (f"DBUS_SESSION_BUS_ADDRESS=unix:path={OUT / 'no-bus'}",)
    return ()


def silent_test(label: str, mode: str) -> tuple[int, Path]:
    return cargo(label, ["test", "-p", "kinewright-media-client", "--features",
                         "slow-tests", "--test", "witness_native", SILENT, "--",
                         "--exact", "--nocapture"], mode_env(mode))


def bound_failure(text: str, kind: str, mode: str) -> bool:
    if "could not compile" in text or "MS2_TEST_DEADLINE silent helper probe bound" not in text:
        return False
    if ("MS2_RESOURCE_MODE degraded" in text) != (mode == "degraded"):
        return False
    unheld = re.search(r"MS2_SILENT_PROBE held=false degraded=(true|false) .*"
                       r"outcome=Err\(Sandbox\(TimedOut\)\)", text)
    if kind == "held":
        # The unheld probe still ends; only the held cleanup strands the caller.
        return bool(unheld) and "MS2_SILENT_PROBE held=true" not in text
    return "MS2_SILENT_PROBE" not in text


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
        source = originals[ACTOR].decode()
        for name, before, after, kind in BOUNDS:
            if source.count(before) != 1:
                raise RuntimeError(f"{name}: source anchor is not unique")
            ACTOR.write_text(source.replace(before, after))
            try:
                for mode in MODES:
                    code, log = silent_test(f"{name}-{mode}-red", mode)
                    rejected = code != 0 and bound_failure(log.read_text(), kind, mode)
                    rows.append({"mutation": name, "mode": mode, "test": SILENT, "exit": code,
                                 "expected_assertion_failed": rejected,
                                 "log": str(log.relative_to(ROOT)), "log_sha256": digest(log)})
                    if not rejected:
                        raise RuntimeError(f"{name} {mode}: did not fail at its outer deadline")
            finally:
                ACTOR.write_bytes(originals[ACTOR])
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
        bound_controls = []
        for mode in MODES:
            code, log = silent_test(f"restored-bound-{mode}", mode)
            text = log.read_text()
            bound_controls.append({
                "mode": mode, "exit": code, "log": str(log.relative_to(ROOT)),
                "log_sha256": digest(log),
                "mode_matches": ("MS2_RESOURCE_MODE degraded" in text) == (mode == "degraded")})
        report = {"mutations": rows, "restored_controls": controls,
                  "restored_bound_controls": bound_controls,
                  "source_restored": all(path.read_bytes() == data for path, data in originals.items())}
        (OUT / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    if controls["exit"] != 0:
        raise SystemExit("restored native controls failed")
    if any(row["exit"] != 0 or not row["mode_matches"] for row in bound_controls):
        raise SystemExit("restored probe-bound controls failed")


if __name__ == "__main__":
    main()
