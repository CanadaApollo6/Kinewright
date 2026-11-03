#!/usr/bin/env python3
"""Run the two TE1 accounting mutation pairs, restoring source on every exit."""
from pathlib import Path
import os
import subprocess

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/kinewright-project/src/orchestration_accounting.rs"
OUT = ROOT / "target/te1/mutations"
OUT.mkdir(parents=True, exist_ok=True)
for key in ("TMPDIR", "LOCALAPPDATA"):
    path = ROOT / "target" / ("t" if key == "TMPDIR" else "l")
    path.mkdir(exist_ok=True)
    os.environ[key] = str(path)
os.environ["RUST_TEST_THREADS"] = "4"


def run(name, witness, expect_failure):
    command = (
        "source scripts/setup-ffmpeg.sh && "
        "flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 "
        "nice -n 10 cargo test -p kinewright-agent --test te1_usage " + witness
    )
    with (OUT / (name + ".log")).open("w") as output:
        result = subprocess.run(["bash", "-c", command], cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=False)
    log = (OUT / (name + ".log")).read_text()
    if expect_failure:
        assert result.returncode and "test result: FAILED" in log, name + " must fail in the witness, not the compiler"
    else:
        assert result.returncode == 0, name + " must pass"
    print(name, next(line for line in log.splitlines() if line.startswith("test result:")), flush=True)


original = SOURCE.read_text()
mutations = [
    ("replay", "old.max(value)", "old.checked_add(value).ok_or(AccountingRefusal::Capacity)?", "te1_codex_cumulative_turns_replay"),
    ("reset", "binding.resumed != prior.is_some()", "false", "te1_resume_fresh_reset"),
]
for name, before, after, witness in mutations:
    assert original.count(before) == 1, name + " mutation site drifted"
    try:
        SOURCE.write_text(original.replace(before, after))
        run(name + "-red", witness, True)
    finally:
        SOURCE.write_text(original)
    run(name + "-green", witness, False)
