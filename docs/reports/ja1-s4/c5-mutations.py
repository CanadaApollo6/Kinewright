"""C5 protected witnesses only. Run with nice -n 19 python3 -I."""
from pathlib import Path
import hashlib
import json
import os
import signal
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "docs/reports/ja1-s4/c5-mutations"
OUT.mkdir(exist_ok=True)
LIMITS = "crates/kinewright-agent/src/orchestration/limits.rs"
USAGE = "crates/kinewright-agent/src/orchestration/usage.rs"
GUARDIAN = "crates/kinewright-agent/src/orchestration/guardian.rs"
SERVICE = "crates/kinewright-agent/src/job_guardian/service.rs"
CLAIM = "crates/kinewright-agent/src/orchestration/execution_claim.rs"
ACCOUNTING = "crates/kinewright-project/src/orchestration_accounting.rs"
DRIVERS = "crates/kinewright-agent/src/drivers.rs"
MUSE = "crates/kinewright-agent/src/muse.rs"
ACP = "crates/kinewright-agent/src/acp.rs"

def replace(text, old, new):
    assert text.count(old) == 1, (old, text.count(old))
    return text.replace(old, new)

cases = [
    ("reader-size", LIMITS, "ja_s4_admission", "s4_ingress_before_allocation", lambda t: replace(t, "if take > FRAME_BYTES.saturating_sub(frame.len()) {", "if false {")),
    ("encoding-size", LIMITS, "ja_s4_admission", "s4_ingress_before_allocation", lambda t: replace(t, "encode(value, FRAME_BYTES)", "encode(value, COMMAND_BYTES * 2)")),
    ("transient-reservation", LIMITS, "ja_s4_admission", "s4_every_cap_and_retirement", lambda t: replace(t, ".reserve_copy(COMMAND_BYTES as u64)", ".reserve_copy(0)")),
    ("live-dependency-pin", LIMITS, "ja_s4_admission", "s4_every_cap_and_retirement", lambda t: replace(t, ".pin(dependencies)", ".pin(&[])")),
    ("acknowledged-cut", USAGE, "ja_s4_usage", "s4_allowance_before_send_contract", lambda t: replace(t, "update\n            .validate_returned_receipt(returned)\n            .map_err(|_| AccountingRefusal::PublicationUnknown)?;", "")),
    ("consumed-send", USAGE, "ja_s4_usage", "s4_allowance_before_send_contract", lambda t: replace(t, "record.task_mut(task)?.consume_send(send.clone())?;", "")),
    ("registered-record-reference", ACCOUNTING, "ja_s4_usage", "s4_registered_accounting_bytes_and_predecessor_contract", lambda t: replace(t, "let bytes = self.record_bytes()?;", "let bytes = encode(self, RECORD_BYTES)?;")),
    ("terminal-reservation", LIMITS, "ja_s4_admission", "s4_ingress_before_allocation", lambda t: replace(t, "!matches!(event, AgentEvent::Done)", "!matches!(event, AgentEvent::Done | AgentEvent::Error(_))")),
    ("codex-turn-retirement", DRIVERS, "@lib", "drivers::tests::s4_codex_old_reader_cannot_retire_a_new_turn", lambda t: replace(t, "if *active_turn != reader_turn {", "if false {")),
    ("muse-reply-capacity", MUSE, "@lib", "muse::tests::s4_muse_tool_map_interrupts_without_losing_accepted_identities", lambda t: replace(t, "if tool_names.len() >= crate::protocol::limits::EVENTS && !tool_names.contains_key(id) {", "if false {")),
    ("raw-ingress-latch", ACP, "@lib", "muse::tests::s4_raw_acp_overflow_reaps_before_terminal_and_rejects_sends", lambda t: replace(t, "ingress_failed.store(true, Ordering::Release);", "")),
    ("raw-fault-reap", MUSE, "@lib", "muse::tests::s4_raw_acp_overflow_reaps_before_terminal_and_rejects_sends", lambda t: replace(t, "AcpIncoming::Fault(message) => {\n                        client.kill();", "AcpIncoming::Fault(message) => {")),
    ("raw-ack-terminal", MUSE, "@lib", "muse::tests::s4_raw_acp_overflow_reaps_before_terminal_and_rejects_sends", lambda t: replace(t, "if client.ingress_failed() {", "if false {")),
    ("commit-barrier", SERVICE, "ja_s4_guardian", "s4_precommit_cannot_execute", lambda t: replace(t, "expected(receiver, identity, true)?;", "")),
    ("guardian-reaped", GUARDIAN, "ja_s4_guardian", "s4_reaped_guardian_and_identity_are_required", lambda t: replace(t, "Ok(self.guardian.reaped()?", "Ok(true")),
    ("retained-slot", CLAIM, "ja_s4_guardian", "s4_guardian_death_and_observation_loss", lambda t: replace(t, 'if root.join("run.json").exists() || !claim.attempts.is_empty() {', "if false {")),
]
tracked = {name: (ROOT / name).read_bytes() for name in {case[1] for case in cases}}
source_hash = hashlib.sha256(b"".join(name.encode() + tracked[name] for name in sorted(tracked))).hexdigest()
results = []
if "--resume" in sys.argv:
    previous=json.loads((OUT/"results.json").read_text())
    assert previous["source_sha256"]==source_hash,"resume requires the identical restored source"
    results=previous["pairs"]
    for index,row in enumerate(results):
        assert row["guard"]==cases[index][0] and row["red"]["runtime_red"] and row["green"]["green"],"only completed pairs may resume"

if "--check" in sys.argv:
    for label,name,binary,test,mutate in cases:
        assert mutate(tracked[name].decode()) != tracked[name].decode()
        print(label,"anchor verified")
    raise SystemExit(0)

def cleanup_owned_units():
    for path in (ROOT / "target/tmp").glob("kinewright-c2-*/**/*recovery.json"):
        try:
            data = json.loads(path.read_text())
            nonce = data["attempt"]["nonce"]
            assert len(nonce) == 32 and all(c in "0123456789abcdef" for c in nonce)
        except (ValueError, KeyError, AssertionError):
            continue
        unit = "kinewright-job-" + nonce + ".service"
        for arguments in (["kill", "--kill-whom=all", "--signal=KILL", unit], ["reset-failed", unit]):
            subprocess.run(["systemctl", "--user", *arguments], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5, check=False)

def run(label, binary, test, phase):
    log = OUT / (label + "-" + phase + ".log")
    target="--lib" if binary=="@lib" else "--test "+shlex.quote(binary)
    command = "source scripts/setup-ffmpeg.sh && awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 timeout --kill-after=5s 600s nice -n 10 cargo test -p kinewright-agent " + target + " " + shlex.quote(test) + " -- --exact --test-threads=1 --nocapture"
    with log.open("w") as stream:
        process=subprocess.Popen(["bash","-c",command],cwd=ROOT,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
        try:
            returncode=process.wait(timeout=7200)
        except BaseException:
            os.killpg(process.pid,signal.SIGKILL)
            process.wait()
            raise
    data = log.read_text()
    summaries = [line for line in data.splitlines() if line.startswith("test result:")]
    return {"exit": returncode, "summary": summaries,
            "runtime_red": returncode == 101 and "test result: FAILED." in data and "could not compile" not in data,
            "green": returncode == 0 and "test result: ok. 1 passed" in data and "C2_NATIVE_NOT_RUN" not in data,
            "log": str(log.relative_to(ROOT))}

try:
    for label, name, binary, test, mutate in cases[len(results):]:
        path = ROOT / name
        original = tracked[name]
        path.write_text(mutate(original.decode()))
        try:
            red = run(label, binary, test, "red")
        finally:
            path.write_bytes(original)
            if binary == "ja_s4_guardian":
                cleanup_owned_units()
        green = run(label, binary, test, "green")
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
