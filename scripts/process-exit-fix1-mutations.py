#!/usr/bin/env python3
"""Forced native-owner regressions. Owns the lock and restores exact source bytes."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target/process-exit-fix1"
APP = "crates/kinewright-app/src/"
PREFIX = "app::process_exit_fix1_tests::"
PAIRS = [
    ("endpoint-cancels-export", "chat_ui.rs",
     "        if let Some(old) = self.mcp_server.replace(server) {\n            retirement.server(old);",
     "        if let Some(old) = self.mcp_server.replace(server) {\n            if let Some(queue) = old.export_queue() { queue.cancel_all(); }\n            retirement.server(old);",
     "exit_fix1_accepted_export_survives_refresh_apply_and_discard", "EXIT_FIX1_ACCEPTED_EXPORT_SURVIVES"),
    ("endpoint-frame-join", "retirement.rs", "        self.servers.push(server);", "        server.shutdown();",
     "exit_fix1_branch_and_thread_close_retire_without_joining_a_live_server", "EXIT_FIX1_BRANCH_FRAME_RETURNS"),
    ("project-frame-join", "retirement.rs",
     "    pub(crate) fn project(&mut self, mut project: ProjectSession) {",
     "    pub(crate) fn project(&mut self, mut project: ProjectSession) {\n        project.join_closed();",
     "exit_fix1_project_close_retires_its_live_investigator_reaper", "EXIT_FIX1_PROJECT_FRAME_RETURNS"),
    ("quit-joins-before-all-persistence", "app.rs",
     'if !project.prepare_close("Kinewright is closing") && pending_close.is_none() {',
     'if !{ let ready = project.prepare_close("Kinewright is closing"); project.join_closed(); ready } && pending_close.is_none() {',
     "exit_fix1_quit_persists_and_shuts_down_all_projects_before_export_join", "EXIT_FIX1_ALL_PROJECTS_PERSIST_AND_SIGNAL_BEFORE_JOIN"),
    ("export-close-choice", "app.rs", None, None,
     "exit_fix1_close_and_quit_ask_before_explicit_export_cancel", "EXIT_FIX1_EXPORT_CLOSE_CHOICE"),
]


def main():
    selected = sys.argv[1:]
    assert not selected or all(name in {p[0] for p in PAIRS} for name in selected)
    OUT.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="4",
               TMPDIR=str(ROOT / "target/t"), LOCALAPPDATA=str(ROOT / "target/l"))
    rows = []
    for name, file, before, after, witness, marker in PAIRS:
        if selected and name not in selected:
            continue
        print("WAIT", name, flush=True)
        with open("/tmp/kinewright-cargo-heavy.lock", "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            path = ROOT / APP / file
            original = path.read_bytes()
            source = original.decode()
            if before is None:
                start = source.index("    fn project_has_exports(&self, index: usize) -> bool {")
                end = source.index("\n    fn export_close_choice", start)
                before = source[start:end]
                after = "    fn project_has_exports(&self, index: usize) -> bool { self.projects.get(index).is_none() }\n"
            assert source.count(before) == 1, (name, "target drift")
            try:
                for color in ("red", "green"):
                    path.write_text(source.replace(before, after)) if color == "red" else path.write_bytes(original)
                    log = OUT / f"{name}-{color}.log"
                    with log.open("w") as output:
                        result = subprocess.run([
                            "bash", "-c", 'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --features slow-tests,kinewright-agent/te1-eval "$1" -- --exact --nocapture',
                            "pair", PREFIX + witness,
                        ], cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT, check=False)
                    text = log.read_text()
                    summaries = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", text)
                    assert summaries and summaries[-1] == (("FAILED", "0", "1") if color == "red" else ("ok", "1", "0")), (name, color, summaries)
                    assert bool(result.returncode) == (color == "red"), (name, color, result.returncode)
                    assert color != "red" or marker in text, (name, "wrong red")
                    rows.append({"pair": name, "color": color, "exit": result.returncode})
                    print(name, color, result.returncode, flush=True)
            finally:
                path.write_bytes(original)
                assert hashlib.sha256(path.read_bytes()).digest() == hashlib.sha256(original).digest()
            print(name, "byte-identical restore", flush=True)
    (OUT / ("pairs" + ("-" + "-".join(selected) if selected else "") + ".json")).write_text(json.dumps(rows, indent=2) + "\n")


if __name__ == "__main__":
    main()
