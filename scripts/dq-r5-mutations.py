"""R5 filler-yield witnesses; exact restores, each cargo call owns the lock."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/dq-r5/retained-pairs"
PAIRS = [
    ("early-onset", "selection.rs", b"EARLY_VOICE_MICROS: u64 = 750_000", b"EARLY_VOICE_MICROS: u64 = 300_000", "r5_early_voiced_onset_is_bounded_by_the_neighbour_start", "R5_EARLY_VOICED_ONSET"),
]

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    results = []
    for name, file, before, after, witness, marker in PAIRS:
        path = ROOT / "crates/kinewright-core/src/dialogue" / file
        original = path.read_bytes()
        assert original.count(before) == 1, (name, "mutation drift")
        digest = hashlib.sha256(original).hexdigest()
        row = {"pair": name, "witness": witness, "source_sha256": digest, "runs": []}
        def run(colour):
            env = os.environ.copy()
            env.update(CARGO_INCREMENTAL="0", RUST_TEST_THREADS="4", TMPDIR=str(ROOT / "target/t"))
            command = ["bash", "-c", 'source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 19 cargo test -p kinewright-core --lib "$1" -- --exact --nocapture', name, "dialogue::r5_tests::" + witness]
            start = time.monotonic()
            with (OUT / f"{name}-{colour}.log").open("w") as log:
                code = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT).returncode
            text = (OUT / f"{name}-{colour}.log").read_text()
            row["runs"].append({"colour": colour, "exit": code, "wall_including_lock": time.monotonic() - start})
            return code, text
        try:
            path.write_bytes(original.replace(before, after))
            code, text = run("red")
            assert code != 0 and marker in text and "1 failed" in text, (name, "did not bite")
        finally:
            path.write_bytes(original)
            assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, (name, "restore")
        code, text = run("green")
        assert code == 0 and "1 passed" in text, (name, "green")
        row["byte_identical_restore"] = True
        results.append(row)
        (OUT / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(row), flush=True)

if __name__ == "__main__":
    main()
