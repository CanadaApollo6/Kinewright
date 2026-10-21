#!/usr/bin/env python3
"""Serialized S0 gates. Run with nice -n 19 python3 -I after setup-ffmpeg.sh."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/kinewright-agent/tests/fixtures/ja"


def catalog_check():
    pins = json.loads((FIXTURES / "source-pins.json").read_text())
    assert pins["conflict_resolutions"] == [1, 2, 3, 4, 5]
    assert "PROJECT_FORMAT_VERSION: u32 = 2" in (
        ROOT / "crates/kinewright-core/src/model.rs").read_text()
    slots = json.loads((FIXTURES / "hash-slots.json").read_text())["slots"]
    assert len({s["fixture_id"] for s in slots}) == len(slots)
    assert sum(s["fixture_id"].startswith("text-") for s in slots) == 17
    for slot in slots:
        assert slot["input_sha256"] is None and slot["oracle_sha256"] is None
        assert slot["status"] == "pending_baseline_witness" and slot["owner"]
    rows = json.loads((FIXTURES / "regressions.json").read_text())["tests"]
    if (FIXTURES / "gate-observers.json").exists():
        gates = json.loads((FIXTURES / "gate-observers.json").read_text())["gates"]
        assert len({g["gate_id"] for g in gates}) == len(gates)
        for gate in gates:
            assert gate["status"] == "pending_baseline_witness" and gate["owner"] and gate["fixture_slot"]
            assert all(gate["expected"][key] for key in ["E", "P", "B", "M"])
        editorial = json.loads((FIXTURES / "editorial-slots.json").read_text())["slots"]
        assert {s["fixture_id"] for s in editorial if s["fixture_id"].startswith("text-")} == {s["fixture_id"] for s in slots if s["fixture_id"].startswith("text-")}
        assert all(s["input_sha256"] is None and s["oracle_sha256"] is None for s in editorial)
        for item in json.loads((FIXTURES / "writer-corpus-pins.json").read_text())["files"]:
            assert hashlib.sha256((FIXTURES / item["path"]).read_bytes()).hexdigest() == item["sha256"]
    for row in rows:
        source = (ROOT / row["source"]).read_text()
        name = row["test"].split("::")[-1]
        assert name in source, row
    print(f"catalog: {len(rows)} exact regression pins; {len(slots)} pending slots; ceiling 2")
    return rows


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("stage", type=int, choices=range(1, 6))
    parser.add_argument("--catalog-only", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--tests-only", action="store_true", help="final-HEAD harness, native, App and exact-pin rerun")
    args = parser.parse_args()
    rows = catalog_check()
    if args.catalog_only:
        return
    out = args.output.resolve() if args.output else ROOT / f"target/ja-s0/S0-{args.stage}"
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4")
    temporary = out / "tmp"
    temporary.mkdir(exist_ok=True)
    env["TMPDIR"] = str(temporary)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    diff = subprocess.check_output(["git", "diff", "HEAD"], cwd=ROOT)
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT).split(b"\0")
    source_tree = hashlib.sha256()
    for name in sorted(set(paths) - {b""}):
        path = ROOT / os.fsdecode(name)
        if path.is_file():
            source_tree.update(name + b"\0" + hashlib.sha256(path.read_bytes()).digest())
    source_tree_sha256 = source_tree.hexdigest()
    records = []

    def run(label, command):
        while int(Path("/proc/meminfo").read_text().split("MemAvailable:")[1].split()[0]) < 6 * 1024**2:
            print(f"{label}: waiting for the required 6 GiB available memory", flush=True)
            time.sleep(10)
        path = out / f"{len(records):02}-{label}.log"
        with path.open("w") as log:
            log.write(f"source_head={head}\nsource_tree_sha256={source_tree_sha256}\ndirty_diff_sha256={hashlib.sha256(diff).hexdigest()}\n")
            log.write(json.dumps(command) + "\n")
            log.flush()
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        raw = path.read_text()
        summaries = re.findall(r"^test result:.*$", raw, re.M)
        record = dict(label=label, command=command, exit=result.returncode,
                      sha256=hashlib.sha256(path.read_bytes()).hexdigest(), summaries=summaries)
        records.append(record)
        (out / "gates.json").write_text(json.dumps(dict(source_head=head, source_tree_sha256=source_tree_sha256,
            dirty_diff_sha256=hashlib.sha256(diff).hexdigest(), gates=records), indent=2) + "\n")
        print(f"{label}: exit {result.returncode}", flush=True)
        for summary in summaries:
            print(summary, flush=True)
        if result.returncode:
            print(raw[-10000:])
            sys.exit(result.returncode)
        if label.startswith("pin-"):
            assert len(summaries) == 1 and "1 passed; 0 failed" in summaries[0], summaries

    cargo = ["nice", "-n", "10", "cargo"]
    harness = "kinewright-eval" if args.stage == 1 else "kinewright-ja-baseline"
    if not args.tests_only:
        run("workspace-build", cargo + ["build", "--workspace"])
        run("harness-build", cargo + ["build", "-p", "kinewright-agent", "--features", "eval-harness", "--bin", harness])
        run("shipping-app-build", cargo + ["build", "-p", "kinewright-app"])
    if args.stage >= 4:
        run("app-adapter-build", cargo + ["test", "-p", "kinewright-app", "--bin", "kinewright-app", "--no-run", "--message-format=json"])
        artifacts = []
        for line in (out / f"{len(records)-1:02}-app-adapter-build.log").read_text().splitlines():
            if line.startswith('{'):
                message = json.loads(line)
                if message.get("reason") == "compiler-artifact" and message["target"]["name"] == "kinewright-app" and message.get("executable") and message["profile"]["test"]:
                    artifacts.append(message["executable"])
        assert len(artifacts) == 1, artifacts
        env["JA_HARNESS_APP_TEST_BINARY"] = artifacts[0]
        (out / "app-test-binary.txt").write_text(artifacts[0] + "\n")
    if not args.tests_only:
        run("workspace-clippy", cargo + ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
        run("harness-clippy", cargo + ["clippy", "-p", "kinewright-agent", "--features", "eval-harness", "--bin", harness, "--", "-D", "warnings"])
        touched = subprocess.check_output(["git", "diff", "--name-only", "HEAD"], cwd=ROOT, text=True).splitlines()
        touched += subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"], cwd=ROOT, text=True).splitlines()
        rust = sorted({p for p in touched if p.endswith(".rs")})
        if rust:
            run("rustfmt", ["rustfmt", "--edition", "2024", "--config", "skip_children=true", "--check"] + rust)
    if args.stage >= 2:
        run("harness-tests", cargo + ["test", "-p", "kinewright-agent", "--lib", "ja_harness::", "--", "--test-threads=1"])
        run("native-harness-tests", cargo + ["test", "-p", "kinewright-agent", "--features", "eval-harness", "--test", "ja_baseline", "--", "--include-ignored", "--test-threads=1"])
    if args.stage >= 4:
        run("teardown-wrapper-test", cargo + ["test", "-p", "kinewright-media", "--test", "generated_media", "teardown_observation_failure_panics_only_in_explicit_finish", "--", "--exact", "--test-threads=1"])
        run("app-observer-test", cargo + ["test", "-p", "kinewright-app", "--bin", "kinewright-app", "ja_harness::tests::coalesced_app_input_reaches_actual_headless_paint", "--", "--exact", "--test-threads=1"])
    for i, row in enumerate(rows):
        affected_s0_2 = args.stage == 2 and (row["stage"] == 1 or row["package"] == "kinewright-core")
        affected_s0_3 = args.stage == 3 and row["test"].split("::")[-1] in {
            "an_idle_engine_holds_no_preview_reader_or_decoder", "dropping_the_engine_stops_the_preview_thread", "sync_decoders_counts_open_decoders_until_teardown"}
        if row["stage"] != args.stage and args.stage != 5 and not affected_s0_2 and not affected_s0_3:
            continue
        target = row["target"].split(":")
        flag = ["--lib"] if target == ["lib"] else ["--" + target[0], target[1]]
        run(f"pin-{i:02}", cargo + ["test", "-p", row["package"]] + flag + [row["test"], "--", "--exact", "--test-threads=1"])
    run("classification", ["nice", "-n", "19", "python3", "-I", "scripts/slow_tests.py", "lint"])


if __name__ == "__main__":
    main()
