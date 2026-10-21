#!/usr/bin/env python3
"""Run the unchanged S0 fixture protocol and verify every retained artifact.
Build first. This is evidence capture, not Studio/native/provider acceptance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/kinewright-agent/tests/fixtures/ja"
LANES = {
    "edit-undo-redo": 0, "format-corpus": 0, "branches-current": 0,
    "clip-id-reuse": 0, "history-loss": 1, "legacy-io": 1,
    "participants-lock": 0, "reply-retry": 1, "recording-loopback": 0,
    "media-shared-control": 0, "media-repeated-control": 0, "app-coalesced-paint": 0,
    "aw2-capability-gap": 0, "pending-only": 3,
    "provider-not-run": 2, "practice-not-run": 2, "windows-not-run": 2,
}
REDS = {
    "history-loss": {"history-survives-reopen": (0, 1)},
    "legacy-io": {"strict-parent-directory-sync": (False, True),
                  "strict-write-no-in-place-fallback": (True, False)},
    "reply-retry": {"retry-must-not-add-second-undo": (2, 1)},
}


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def index():
    records = []
    for name, expected in LANES.items():
        path = FIXTURES / (name + ".json")
        value = json.loads(path.read_text())
        records.append(dict(fixture_id=name, relative_file=path.name, sha256=digest(path),
                            byte_length=path.stat().st_size, expected_baseline_process_exit=expected,
                            inputs=value["inputs"], oracles=value["oracles"]))
    return dict(protocol="kinewright.ja-harness", protocol_version=1, fixtures=records)


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--write-index", action="store_true")
    parser.add_argument("--mode", choices=["baseline", "revised"], default="baseline")
    parser.add_argument("--app-test-binary", type=Path)
    args = parser.parse_args()
    frozen = index()
    path = FIXTURES / "fixtures-index.json"
    if args.write_index:
        path.write_text(json.dumps(frozen, indent=2) + "\n")
        return
    assert json.loads(path.read_text()) == frozen, "fixture/oracle catalog drift"
    assert args.output and args.app_test_binary
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    env = dict(os.environ, JA_HARNESS_APP_TEST_BINARY=str(args.app_test_binary.resolve()))
    temporary = args.output / "tmp"
    temporary.mkdir()
    env["TMPDIR"] = str(temporary)
    results = []
    for name, expected in LANES.items():
        output = args.output / name
        command = ["nice", "-n", "10", str(ROOT / "target/debug/kinewright-ja-baseline"), "--mode", args.mode,
                   "--fixture", str(FIXTURES / (name + ".json")), "--output", str(output)]
        process = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, timeout=600)
        output.mkdir(exist_ok=True)
        (output / "process-stdout.txt").write_bytes(process.stdout)
        (output / "process-stderr.txt").write_bytes(process.stderr)
        receipt = dict(command=command, process_exit=process.returncode,
                       stdout_sha256=digest(output / "process-stdout.txt"), stderr_sha256=digest(output / "process-stderr.txt"))
        (output / "process-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        summary = json.loads((output / "summary.json").read_text())
        run = json.loads((output / "run.json").read_text())
        record = dict(fixture_id=name, mode=args.mode, lane=run["lane"], run_id=run["run_id"], source_head=run["source_head"],
                      fixture_sha256=run["fixture_sha256"], binary_sha256=run["binary_sha256"], process_exit=process.returncode,
                      summary_sha256=digest(output / "summary.json"), assertions=summary["assertions"],
                      pending=summary["pending"], gates=summary["gates"], missing_requirements=summary["missing_requirements"])
        results.append(record)
        (args.output / "lanes.json").write_text(json.dumps(results, indent=2) + "\n")
        acceptable = [expected] if args.mode == "baseline" else ([0, 1] if expected == 1 else [expected])
        assert process.returncode in acceptable and summary["exit_code"] == process.returncode, receipt
        assert all(a["passed"] for a in summary["positive_controls"]), name
        assert all(g["disposition"] == "pending_baseline_witness" for g in summary["gates"]), name
        reds = {a["id"]: a for a in summary["assertions"] if a["disposition"] == "baseline_red"}
        if args.mode == "baseline":
            assert set(reds) == set(REDS.get(name, {})), name
            for assertion, (actual, wanted) in REDS.get(name, {}).items():
                detail = json.loads((output / reds[assertion]["artifact"]["relative_file"]).read_text())
                assert detail["actual"] == actual and detail["expected"] == wanted
                assert detail["positive_control"] in {a["id"] for a in summary["positive_controls"]}
        for reference in summary["artifacts"]:
            artifact = (output / reference["relative_file"]).resolve()
            assert artifact.is_relative_to(output) and artifact.stat().st_size == reference["byte_length"]
            assert digest(artifact) == reference["sha256"], artifact
        assert run["fixture_sha256"] == digest(FIXTURES / (name + ".json"))
        print(name, "process_exit", process.returncode, "controls", len(summary["positive_controls"]),
              "baseline_reds", len(reds), "pending_gates", len(summary["gates"]), flush=True)


if __name__ == "__main__":
    main()
