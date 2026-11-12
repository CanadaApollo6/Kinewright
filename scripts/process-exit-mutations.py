#!/usr/bin/env python3
"""Run native-owner pairs without an outer cargo lock or any provider command.

Each pair owns the shared lock across mutation, red run, byte-for-byte restore
and green run. Subprocesses are awaited in the foreground. No signals are sent.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target/process-exit-crash"
SERVER_DROP = """impl Drop for McpServer {
    fn drop(&mut self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}"""
PAIRS = [
    (
        "owner-join", "kinewright-media", "worker.rs",
        "let _ = thread.join();", "drop(thread);",
        "engine::process_exit_tests::process_exit_waits_for_native_preview_teardown",
        "EXIT_NATIVE_TEARDOWN_BEFORE_OWNER_RETURN",
    ),
    (
        "server-owner", "kinewright-agent", "server.rs", SERVER_DROP,
        SERVER_DROP.replace(
            "        if let Some(thread) = self.thread.take() {\n"
            "            let _ = thread.join();\n        }",
            "        self.thread.take();",
        ),
        "server::tests::process_exit_server_drop_joins_its_backend",
        "EXIT_SERVER_BACKEND_BEFORE_OWNER_RETURN",
    ),
]


def run(name, package, witness, marker, color, env):
    log = OUT / f"{name}-{color}-final.log"
    command = [
        "bash", "-c",
        'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 '
        '-p "$1" --lib "$2" -- --exact --nocapture',
        "pair", package, witness,
    ]
    started = time.monotonic()
    with log.open("w") as output:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=output,
                                stderr=subprocess.STDOUT, check=False)
    text = log.read_text()
    summaries = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", text)
    expected = ("FAILED", "0", "1") if color == "red" else ("ok", "1", "0")
    # The engine parent forwards its child's summary as well as its own.
    assert summaries and summaries[-1] == expected, (name, color, summaries)
    assert (result.returncode != 0) == (color == "red"), (name, color, result.returncode)
    assert color != "red" or marker in text, (name, "wrong red failure")
    print(name, color, result.returncode, flush=True)
    return {"pair": name, "color": color, "status": result.returncode,
            "wall": time.monotonic() - started}


def main():
    selected = sys.argv[1:]
    if len(selected) > 1 or any(name not in {pair[0] for pair in PAIRS} for name in selected):
        raise SystemExit("usage: process-exit-mutations.py [owner-join|server-owner]")
    OUT.mkdir(parents=True, exist_ok=True)
    for folder in ("target/t", "target/l"):
        (ROOT / folder).mkdir(exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS="4", CARGO_INCREMENTAL="0",
               TMPDIR=str(ROOT / "target/t"), LOCALAPPDATA=str(ROOT / "target/l"))
    rows = []
    suffix = "-" + selected[0] if selected else ""
    for name, package, source, before, after, witness, marker in PAIRS:
        if selected and name not in selected:
            continue
        print("WAIT", name, flush=True)
        with open("/tmp/kinewright-cargo-heavy.lock", "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            path = ROOT / "crates" / package / "src" / source
            original = path.read_bytes()
            sha = hashlib.sha256(original).hexdigest()
            text = original.decode()
            assert text.count(before) == 1, (name, "target drift")
            try:
                path.write_text(text.replace(before, after))
                rows.append(run(name, package, witness, marker, "red", env))
            finally:
                path.write_bytes(original)
                assert path.read_bytes() == original, (name, "restore failed")
            rows.append(run(name, package, witness, marker, "green", env))
            assert path.read_bytes() == original, (name, "green changed source")
            print("RESTORED", source, sha, flush=True)
            (OUT / f"mutation-results{suffix}.json").write_text(json.dumps(rows, indent=2) + "\n")


if __name__ == "__main__":
    main()
