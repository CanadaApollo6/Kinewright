#!/usr/bin/env python3
"""Reuse canonical, hash-checked display inputs through the existing PF1 harness."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
PREPARE_TEST = "pf1_harness::s4_prepare_inputs"
WORKLOAD = re.compile(r"display::tests::i5_production_workload_(typical_1080p|blend_heavy_1080p|explainer_16x9|reel_9x16|feed_4x5|talk_recut)_matches_ten_frames_exactly")


def workload(unit):
    if not unit.get("display", False):
        return ""
    if len(unit["tests"]) != 1 or not (match := WORKLOAD.fullmatch(unit["tests"][0])):
        raise ValueError("display cache requires one canonical workload per unit")
    return match.group(1)


def paths(key):
    directory = ROOT / "ci-fixtures" / ("display-" + key)
    return directory, directory / "workload.json", directory / "ci-ready.json"


def validate(key, digest):
    _, manifest_path, _ = paths(key)
    raw = manifest_path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != digest:
        raise ValueError("display input manifest SHA-256 mismatch")
    manifest = json.loads(raw)
    if manifest["key"] != key:
        raise ValueError("display input workload identity mismatch")

    def artifact(value):
        path = (ROOT / value["path"]).resolve()
        if not path.is_relative_to(ROOT.resolve()):
            raise ValueError("display input escapes its root")
        with path.open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != value["sha256"]:
                raise ValueError("display input artifact SHA-256 mismatch")
        return path

    artifact(manifest["document"])
    media = json.loads(artifact(manifest["media"]).read_text(encoding="utf-8"))
    for value in media["files"]:
        artifact(value)


def environment(unit):
    key = workload(unit)
    if not key:
        return {}
    _, manifest, ready = paths(key)
    digest = json.loads(ready.read_text(encoding="utf-8"))["manifest_sha256"]
    validate(key, digest)
    return {"PF1_ROOT": str(ROOT), "PF1_FROZEN_INPUT": str(manifest), "PF1_FROZEN_SHA256": digest}


def prepare(data, units, output):
    import slow_tests as st
    displays = [u for u in units if workload(u)]
    if len(displays) > 1:
        raise ValueError("display workloads must use separate runners")
    for unit in displays:
        key = workload(unit)
        directory, manifest, ready = paths(key)
        if ready.is_file():
            environment(unit)
            print(f"display input {key}: restored and hash-checked", flush=True)
            continue
        if directory.exists():
            raise ValueError("incomplete display input; refusing to cache partial preparation")
        directory.parent.mkdir(parents=True, exist_ok=True)
        suite = data["suites"][unit["suite"]]
        env = {k: v for k, v in os.environ.items() if k not in ("PF1_FROZEN_INPUT", "PF1_FROZEN_SHA256")}
        env.update(PF1_ROOT=str(ROOT), PF1_ONLY=key, PF1_PREPARE=str(directory.relative_to(ROOT)),
                   CARGO_MANIFEST_DIR=suite["cwd"], RUST_TEST_THREADS="1")
        Path(output).mkdir(parents=True, exist_ok=True)
        log_path = Path(output) / "display-prepare.log"
        print(f"display input {key}: prepare with the original canonical builder", flush=True)
        started = time.monotonic()
        with log_path.open("w", encoding="utf-8") as log:
            log.write(f"Running tests/{suite['label']}.rs ({suite['path']})\n")
            log.flush()
            subprocess.run([suite["path"], "--ignored", "--exact", PREPARE_TEST,
                            "--test-threads", "1", "--show-output"], cwd=suite["cwd"], env=env,
                           stdout=log, stderr=subprocess.STDOUT, timeout=1800, check=True)
        st.verify_exact_rows([(suite["label"], PREPARE_TEST)], st.parse(log_path.read_text(encoding="utf-8")))
        digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
        validate(key, digest)
        ready.write_text(json.dumps({"manifest_sha256": digest}) + "\n", encoding="utf-8")
        print(f"display input {key}: prepared and hash-checked in {time.monotonic() - started:.3f}s", flush=True)
