"""Targeted native security mutations. Run with nice -n 19 python3 -I."""
from pathlib import Path
import hashlib
import json
import os
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / "docs/reports/ja1-s4"
OUT = REPORT / "c2-mutations"
OUT.mkdir(exist_ok=True)
LINUX = "crates/kinewright-job-sys/src/linux.rs"
GUARDIAN = "crates/kinewright-agent/src/orchestration/guardian.rs"
SERVICE = "crates/kinewright-agent/src/job_guardian/service.rs"
CLAIM = "crates/kinewright-agent/src/orchestration/execution_claim.rs"

def body(text, signature, replacement):
    start = text.index(signature)
    opening = text.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        if text[end] == "{":
            depth += 1
        elif text[end] == "}":
            depth -= 1
        end += 1
    return text[:opening + 1] + "\n" + replacement + "\n" + text[end - 1:]

def replace(text, old, new):
    assert text.count(old) == 1, (old, text.count(old))
    return text.replace(old, new)

cases = [
    ("commit-barrier", SERVICE, "s4_precommit_cannot_execute", lambda t: replace(t, "expected(receiver, identity, true)?;", "")),
    ("failed-startup-closes-lifetime", GUARDIAN, "s4_precommit_cannot_execute", lambda t: replace(t, "if outcome.is_err() {\n            self.close_lifetime();\n        }", "")),
    ("lifetime-writer", LINUX, "s4_real_host_death_skips_grace", lambda t: body(t, "pub fn check_lifetime_writer(", "rustix::io::fcntl_setfd(writer, rustix::io::FdFlags::empty())?; Ok(())")),
    ("native-cgroup-kill", LINUX, "s4_real_host_death_skips_grace", lambda t: body(t, "pub fn kill(&mut self)", "Ok(())")),
    ("manager-whole-tree", GUARDIAN, "s4_guardian_death_and_observation_loss", lambda t: replace(t, '"--property=KillMode=control-group"', '"--property=KillMode=process"')),
    ("retained-nlink", LINUX, "s4_retained_inode_deletion_signal", lambda t: body(t, "pub fn kernel_deleted(", "Ok(true)")),
    ("native-identity", GUARDIAN, "s4_reaped_guardian_and_identity_are_required", lambda t: replace(t, "if cgroup.identity() != (recovery.device, recovery.inode) {", "if false {")),
    ("zombie-scan", LINUX, "s4_deleted_cgroup_zombies_still_block_retirement", lambda t: body(t, "pub fn no_processes(", "Ok(true)")),
    ("guardian-reaped", GUARDIAN, "s4_reaped_guardian_and_identity_are_required", lambda t: replace(t, "Ok(self.guardian.reaped()?", "Ok(true")),
    ("retained-slot", CLAIM, "s4_guardian_death_and_observation_loss", lambda t: replace(t, 'if root.join("run.json").exists() || !claim.attempts.is_empty() {', 'if false {')),
    ("subreaper", LINUX, "s4_guardian_subreaps_real_orphans", lambda t: body(t, "pub fn install_subreaper(", "Ok(())")),
]

tracked = {name: (ROOT / name).read_bytes() for name in {case[1] for case in cases}}
source_hash = hashlib.sha256(b"".join(name.encode() + tracked[name] for name in sorted(tracked))).hexdigest()
results = []

def cleanup_owned_units():
    # These are only this witness binary's private, nonce-bound recovery records.
    for path in (ROOT / "target/tmp").glob("kinewright-c2-*/**/*recovery.json"):
        try:
            data = json.loads(path.read_text())
            nonce = data["attempt"]["nonce"]
            assert len(nonce) == 32 and all(c in "0123456789abcdef" for c in nonce)
        except (ValueError, KeyError, AssertionError):
            continue
        unit = "kinewright-job-" + nonce + ".service"
        subprocess.run(["systemctl", "--user", "kill", "--kill-whom=all", "--signal=KILL", unit], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5, check=False)
        subprocess.run(["systemctl", "--user", "reset-failed", unit], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5, check=False)

def run(label, test, phase):
    log = OUT / (label + "-" + phase + ".log")
    cmd = "test \"$(free -g | awk '/^Mem:/{print $7}')\" -ge 6 && source scripts/setup-ffmpeg.sh && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo test -p kinewright-agent --test ja_s4_guardian " + shlex.quote(test) + " -- --test-threads=1 --nocapture"
    with log.open("w") as stream:
        result = subprocess.run(["bash", "-c", cmd], cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT, timeout=600, check=False)
    data = log.read_text()
    summaries = [line for line in data.splitlines() if line.startswith("test result:")]
    runtime_red = result.returncode == 101 and "test result: FAILED." in data and "could not compile" not in data
    green = result.returncode == 0 and "test result: ok." in data and "C2_NATIVE_NOT_RUN" not in data
    return {"exit": result.returncode, "summary": summaries, "runtime_red": runtime_red, "green": green, "log": str(log.relative_to(ROOT))}

try:
    for label, name, test, mutate in cases:
        path = ROOT / name
        original = tracked[name]
        path.write_text(mutate(original.decode()))
        try:
            red = run(label, test, "red")
        finally:
            path.write_bytes(original)
            cleanup_owned_units()
        green = run(label, test, "green")
        row = {"guard": label, "witness": test, "red": red, "green": green}
        results.append(row)
        (OUT / "results.json").write_text(json.dumps({"source_sha256": source_hash, "pairs": results}, indent=2) + "\n")
        print(label, "red=" + str(red["runtime_red"]), "restored_green=" + str(green["green"]), flush=True)
        if not red["runtime_red"] or not green["green"]:
            raise SystemExit("mutation pair did not earn behavioral credit: " + label)
finally:
    for name, data in tracked.items():
        (ROOT / name).write_bytes(data)
    assert all((ROOT / name).read_bytes() == data for name, data in tracked.items())
