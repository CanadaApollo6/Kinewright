"""Targeted native security mutations. Run with nice -n 19 python3 -I."""
from pathlib import Path
import hashlib
import json
import os
import shlex
import signal
import sys
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / "docs/reports/ja1-s4"
OUT = REPORT / "c2-fix2" / "mutations"
OUT.mkdir(parents=True, exist_ok=True)
LINUX = "crates/kinewright-job-sys/src/linux.rs"
GUARDIAN = "crates/kinewright-agent/src/orchestration/guardian.rs"
SERVICE = "crates/kinewright-agent/src/job_guardian/service.rs"
CLAIM = "crates/kinewright-agent/src/orchestration/execution_claim.rs"
NAMESPACE = "crates/kinewright-job-sys/src/namespace.rs"

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

def remove_cgroup_boundary(text):
    text = replace(text, "                | thread::UnshareFlags::NEWCGROUP\n", "")
    return body(text, "fn verify_cgroup_namespace(", "Ok(())")

cases += [
    ("outside-service-migration", NAMESPACE, "s4_c2_migration_outside_service_refused", remove_cgroup_boundary),
    ("service-root-migration", NAMESPACE, "s4_c2_migration_to_service_root_refused", remove_cgroup_boundary),
    ("blocked-guardian-exit", SERVICE, "s4_c2_blocked_guardian_exits_and_manager_kills", lambda t: replace(t, 'return Err(io::Error::other(\n                "cleanup_blocked: manager subtree escalation required",\n            ));', 'loop { thread::sleep(Duration::from_millis(100)); }')),
    ("runtime-directory-mask", NAMESPACE, "s4_c2_runtime_dir_masked", lambda t: body(t, "fn mask_runtime_directory(", "Ok(())")),
    ("nsdelegate-required", NAMESPACE, "s4_c2_launch_refuses_without_nsdelegate", lambda t: body(t, "pub fn verify_nsdelegate(", "Ok(())")),
]

SHIM = "crates/kinewright-agent/src/job_guardian/linux.rs"

def service_session(text, scan_only=False):
    old = "\n                && self.service.no_processes()?"
    if not scan_only:
        text = replace(text, "\n                && self.service.empty()?", "")
    return replace(text, old, "")

def service_recovery(text, scan_only=False):
    if not scan_only:
        text = replace(text, "                    && self.service.empty()?\n", "")
    return replace(text, "                    && self.service.no_processes()?\n", "")

cases += [
    ("pid-namespace", NAMESPACE, "s4_c2_proc_shows_only_the_namespace", lambda t: replace(t, "\n                | thread::UnshareFlags::NEWPID", "")),
    ("fresh-proc", NAMESPACE, "s4_c2_proc_shows_only_the_namespace", lambda t: body(t, "pub fn mount_payload_proc(", "Ok(())")),
    ("runtime-alias-mask", NAMESPACE, "s4_c2_runtime_alias_refused", lambda t: body(body(t, "fn mask_runtime_directory(", "Ok(())"), "fn verify_runtime_mask(", "Ok(())")),
    ("init-orphan-reaping", NAMESPACE, "s4_c2_init_reaps_and_exits_with_provider", lambda t: replace(t, "rustix::process::wait(rustix::process::WaitOptions::empty())", "rustix::process::waitpid(rustix::process::Pid::from_raw(provider.cast_signed()), rustix::process::WaitOptions::empty())")),
    ("init-provider-status", NAMESPACE, "s4_c2_init_reaps_and_exits_with_provider", lambda t: t.replace("return Ok(status", "return Ok(0); return Ok(status", 1)),
    ("descriptor-readiness", SHIM, "s4_c2_launch_under_runtime_dir", lambda t: replace(t, "    write_ready(&mut std::io::stdout(), &boundary)?;", "    super::service::atomic_record(&path.with_file_name(\"boundary-ready.json\"), &boundary, super::protocol::WIRE_CAP)?;\n    write_ready(&mut std::io::stdout(), &boundary)?;")),
    ("pid-namespace-refusal", NAMESPACE, "s4_c2_launch_refuses_without_pid_namespace", lambda t: body(t, "pub fn verify_payload_pid_namespace(", "Ok(())")),
    ("session-service-live", GUARDIAN, "s4_c2_service_proof_session_live", service_session),
    ("recovery-service-live", GUARDIAN, "s4_c2_service_proof_recovery_live", service_recovery),
    ("session-service-zombie", GUARDIAN, "s4_c2_service_proof_session_zombie", lambda t: service_session(t, True)),
    ("recovery-service-zombie", GUARDIAN, "s4_c2_service_proof_recovery_zombie", lambda t: service_recovery(t, True)),
    ("session-service-events", GUARDIAN, "s4_c2_service_events_session", lambda t: replace(t, "\n                && self.service.empty()?", "")),
    ("recovery-service-events", GUARDIAN, "s4_c2_service_events_recovery", lambda t: replace(t, "\n                    && self.service.empty()?", "")),
    ("service-device", GUARDIAN, "s4_c2_service_identity_refused", lambda t: replace(t, "if service.identity() != (recovery.service_device, recovery.service_inode) {", "if service.identity().1 != recovery.service_inode {")),
    ("service-inode", GUARDIAN, "s4_c2_service_identity_refused", lambda t: replace(t, "if service.identity() != (recovery.service_device, recovery.service_inode) {", "if service.identity().0 != recovery.service_device {")),
]

tracked = {name: (ROOT / name).read_bytes() for name in {case[1] for case in cases}}
source_hash = hashlib.sha256(b"".join(name.encode() + tracked[name] for name in sorted(tracked))).hexdigest()
saved = json.loads((OUT / "results.json").read_text()) if (OUT / "results.json").exists() else None
if len(sys.argv) > 1 and saved is not None:
    assert saved["source_sha256"] == source_hash, "cannot resume pairs from different source bytes"
    results = saved["pairs"]
else:
    results = []
preexisting_records = set((ROOT / "target/tmp").glob("kinewright-c2-*/**/*recovery.json"))

def cleanup_owned_units():
    # These are only this witness binary's private, nonce-bound recovery records.
    for path in (ROOT / "target/tmp").glob("kinewright-c2-*/**/*recovery.json"):
        if path in preexisting_records:
            continue
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
    cmd = "test \"$(free -g | awk '/^Mem:/{print $7}')\" -ge 6 && source scripts/setup-ffmpeg.sh && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo test -p kinewright-agent --test ja_s4_guardian --features slow-tests " + shlex.quote(test) + " -- --exact --test-threads=1 --nocapture"
    with log.open("w") as stream:
        with subprocess.Popen(["bash", "-c", cmd], cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True) as process:
            try:
                exit_code = process.wait(timeout=3600)
            except BaseException:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                cleanup_owned_units()
                raise
    data = log.read_text()
    if "C2_NATIVE_NOT_RUN" in data: print(data, flush=True)
    summaries = [line for line in data.splitlines() if line.startswith("test result:")]
    runtime_red = exit_code == 101 and "test result: FAILED." in data and "could not compile" not in data and "C2_NATIVE_NOT_RUN" not in data
    green = exit_code == 0 and "test result: ok." in data and "C2_NATIVE_NOT_RUN" not in data
    return {"exit": exit_code, "summary": summaries, "runtime_red": runtime_red, "green": green, "log": str(log.relative_to(ROOT))}

try:
    for label, name, test, mutate in cases:
        if len(sys.argv) > 1 and label not in sys.argv[1:]:
            continue
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
        results = [old for old in results if old["guard"] != label]
        results.append(row)
        (OUT / "results.json").write_text(json.dumps({"source_sha256": source_hash, "pairs": results}, indent=2) + "\n")
        print(label, "red=" + str(red["runtime_red"]), "restored_green=" + str(green["green"]), flush=True)
        if not red["runtime_red"] or not green["green"]:
            raise SystemExit("mutation pair did not earn behavioral credit: " + label)
finally:
    for name, data in tracked.items():
        (ROOT / name).write_bytes(data)
    assert all((ROOT / name).read_bytes() == data for name, data in tracked.items())
