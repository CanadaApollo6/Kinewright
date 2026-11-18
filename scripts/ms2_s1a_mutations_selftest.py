#!/usr/bin/env python3
"""Witness that an interrupted mutation run restores its source and stops its step.

For SIGTERM and SIGHUP: start `ms2_s1a_mutations.py` with its self-test step
(a long shell step with a grandchild in its own process group), wait until a
protection-removing edit is on disk and the step is running, send the signal,
then assert that the script exits 128+signal with its named line, every audited
file is byte-identical to before and clean against git, and the step's process
group is gone. Finally, a dirty audited file must make the script refuse to start.

Runs no cargo. Run as `nice -n 19 python3 -I scripts/ms2_s1a_mutations_selftest.py`.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ms2_s1a_mutations.py"
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / "scripts"))
from ms2_s1a_mutations import AUDITED, SELFTEST_STEP, dirty, group_alive  # noqa: E402

sys.path.pop(0)


def hashes() -> dict[Path, str]:
    return {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in AUDITED}


def start() -> subprocess.Popen:
    env = dict(os.environ, **{SELFTEST_STEP: "1"})
    return subprocess.Popen([sys.executable, "-I", str(SCRIPT)], cwd=ROOT, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                            process_group=0)


def interrupted_run(signum: signal.Signals) -> int:
    before = hashes()
    script = start()
    output = []
    group = None
    for line in script.stdout:
        output.append(line)
        found = re.search(r"MS2_MUTATIONS_STEP label=\S+ pid=\d+ pgid=(\d+)", line)
        if found:
            group = int(found.group(1))
            break
    assert group is not None, f"no step started: {''.join(output)}"
    assert hashes() != before, "no protection-removing edit was on disk during the step"
    assert group_alive(group), "the step's process group was not running"
    os.kill(script.pid, signum)
    output.extend(script.stdout)
    code = script.wait(timeout=60)
    text = "".join(output)
    assert code == 128 + signum, f"{signum.name}: exit {code}\n{text}"
    assert f"MS2_MUTATIONS_INTERRUPTED signal={signum.name}" in text, text
    assert "MS2_MUTATIONS_RESTORED files=" in text, text
    assert hashes() == before, f"{signum.name}: audited files differ after the run"
    assert not dirty(AUDITED), f"{signum.name}: audited files differ from git"
    assert not group_alive(group), f"{signum.name}: process group {group} is still alive"
    return group


def refused_when_dirty() -> None:
    target = AUDITED[0]
    original = target.read_bytes()
    try:
        target.write_bytes(original + b"\n")
        script = start()
        text = script.communicate(timeout=60)[0]
        assert script.returncode == 2, f"dirty start: exit {script.returncode}\n{text}"
        assert "MS2_MUTATIONS_REFUSED dirty audited source:" in text, text
        assert "MS2_MUTATIONS_STEP" not in text, text
    finally:
        target.write_bytes(original)
    assert not dirty(AUDITED), "the dirty-start case left a file changed"


def main() -> None:
    if dirty(AUDITED):
        raise SystemExit("audited files already differ from git; check `git status` first")
    groups = [interrupted_run(signum) for signum in (signal.SIGTERM, signal.SIGHUP)]
    refused_when_dirty()
    print(f"MS2_MUTATIONS_SELFTEST ok signals=SIGTERM,SIGHUP files={len(AUDITED)} "
          f"byte_identical=yes git_clean=yes groups_gone={len(groups)} dirty_start_refused=yes")


if __name__ == "__main__":
    main()
