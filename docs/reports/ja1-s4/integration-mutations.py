"""Integration security witnesses. Run with nice -n 19 python3 -I."""
from pathlib import Path
import hashlib
import json
import os
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "docs/reports/ja1-s4/integration/mutations"
OUT.mkdir(parents=True, exist_ok=True)
SHIM = "crates/kinewright-agent/src/job_guardian/linux.rs"
NAMESPACE = "crates/kinewright-job-sys/src/namespace.rs"
ACP = "crates/kinewright-agent/src/acp.rs"


def replace(text, old, new):
    assert text.count(old) == 1, (old, text.count(old))
    return text.replace(old, new)


def body(text, signature, replacement):
    start = text.index("{", text.index(signature))
    end, depth = start + 1, 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[:start + 1] + "\n" + replacement + "\n" + text[end - 1:]


def no_pid(text):
    return replace(text, "        thread::UnshareFlags::NEWPID,\n", "")


def payload_pid_view(text):
    text = body(no_pid(text), "pub fn verify_payload_pid_namespace(", "Ok(())")
    # Without NEWPID the host-owned procfs mount also refuses startup. Disable
    # that independent startup defence so this pair reaches the payload oracle.
    return body(text, "pub fn mount_payload_proc(", "verify_payload_pid_namespace(rustix::process::getpid().as_raw_nonzero().get().cast_unsigned())")


def no_runtime_mask(text):
    return body(body(text, "fn mask_runtime_directory(", "Ok(())"), "fn verify_runtime_mask(", "Ok(())")


GUARDIAN = ("kinewright-agent", "ja_s4_guardian")
AGENT = ("kinewright-agent", "@lib")
SYS = ("kinewright-job-sys", "@lib")
CASES = [
    ("init-cgroup-handles", SHIM, GUARDIAN, "s4_c2_proc_shows_only_the_namespace", lambda t: replace(t, "    drop(cgroup);\n", ""), "init retained cgroup descriptor"),
    ("pid-payload-assertion", NAMESPACE, GUARDIAN, "s4_c2_proc_shows_only_the_namespace", payload_pid_view, "procfs must contain only namespace init and provider"),
    ("nested-fresh-proc", NAMESPACE, GUARDIAN, "s4_c2_runtime_alias_refused", lambda t: body(t, "pub fn mount_payload_proc(", "Ok(())"), "nested procfs must remain namespace-only"),
    ("empty-readiness", SHIM, GUARDIAN, "job_guardian::linux::tests::s4_empty_readiness_eof_has_named_reason", lambda t: replace(t, '            Ok(0) if bytes.is_empty() => {\n                return Err(io::Error::other(\n                    "payload_namespace_unavailable: readiness closed without result",\n                ));\n            }\n', ""), "assertion `left == right` failed"),
    ("unshare-reason", NAMESPACE, SYS, "namespace::tests::s4_unshare_failure_names_flag_and_errno", lambda t: body(t, "fn unshare_payload_namespace(", 'unsafe { thread::unshare_unsafe(flag) }.map_err(|error| io::Error::other(format!("payload_pid_namespace_unavailable: {error}")))'), "failing flag missing"),
    ("queued-frame-latch", ACP, AGENT, "acp::tests::s4_queued_executable_frame_stops_at_ingress_latch_but_restore_writes", lambda t: replace(t, "if ingress_failed.load(Ordering::Acquire) && !write.restore {", "if false {"), "queued executable frame must never be written after ingress latch"),
    ("proc-view-pid-refusal", NAMESPACE, GUARDIAN, "s4_c2_proc_shows_only_the_namespace", no_pid, "payload_pid_namespace_unavailable"),
    ("proc-view-fresh-proc", NAMESPACE, GUARDIAN, "s4_c2_proc_shows_only_the_namespace", lambda t: body(t, "pub fn mount_payload_proc(", "Ok(())"), "procfs must contain only namespace init and provider"),
    ("runtime-alias", NAMESPACE, GUARDIAN, "s4_c2_runtime_alias_refused", no_runtime_mask, "every visible proc-root alias must refuse both sockets"),
    ("descriptor-readiness", SHIM, GUARDIAN, "s4_c2_launch_under_runtime_dir", lambda t: replace(t, "    write_ready(&mut std::io::stdout(), &boundary)?;", '    super::service::atomic_record(&path.with_file_name("boundary-ready.json"), &boundary, super::protocol::WIRE_CAP)?;\n    write_ready(&mut std::io::stdout(), &boundary)?;'), "descriptor readiness must survive a masked production claim root"),
]
SOURCE_FILES = {case[1] for case in CASES} | {
    "crates/kinewright-agent/tests/ja_s4_guardian.rs",
    "crates/kinewright-agent/tests/fixtures/ja_s4_native.rs",
}
SAVED = {name: (ROOT / name).read_bytes() for name in SOURCE_FILES}
DIGEST = hashlib.sha256(b"".join(name.encode() + SAVED[name] for name in sorted(SAVED))).hexdigest()

if "--check" in sys.argv:
    for label, name, target, test, mutate, marker in CASES:
        assert mutate(SAVED[name].decode()) != SAVED[name].decode()
        print(label, "anchor verified")
    raise SystemExit(0)


def run(label, target, test, phase):
    crate, binary = target
    args = ["bash", "docs/reports/ja1-s4/integration-cargo.sh", "test", "-p", crate]
    args += ["--lib"] if binary == "@lib" else ["--test", binary]
    if crate == "kinewright-agent":
        args += ["--features", "slow-tests"]
    args += [test, "--", "--exact", "--test-threads=1", "--nocapture"]
    log = OUT / f"{label}-{phase}.log"
    with log.open("w") as stream:
        process = subprocess.Popen(args, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=1800)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
    text = log.read_text()
    valid = "could not compile" not in text and "C2_NATIVE_NOT_RUN" not in text
    return {"exit": code, "summary": [line for line in text.splitlines() if line.startswith("test result:")],
            "runtime_red": code == 101 and "test result: FAILED." in text and valid,
            "green": code == 0 and "test result: ok. 1 passed" in text and valid,
            "log": str(log.relative_to(ROOT))}, text


results = []
result_path = OUT / "results.json"
if result_path.exists():
    previous = json.loads(result_path.read_text())
    assert previous["source_sha256"] == DIGEST, "source changed; archive the earlier run before retrying"
    results = previous["pairs"]
try:
    for label, name, target, test, mutate, marker in CASES:
        if len(sys.argv) > 1 and label not in sys.argv[1:]:
            continue
        original = SAVED[name]
        mutated = mutate(original.decode())
        assert mutated != original.decode()
        path = ROOT / name
        path.write_text(mutated)
        try:
            red, output = run(label, target, test, "red")
            red["expected_assertion"] = not marker or marker in output
        finally:
            path.write_bytes(original)
        green, _ = run(label, target, test, "green")
        results = [row for row in results if row["guard"] != label]
        results.append({"guard": label, "witness": test, "red": red, "green": green})
        result_path.write_text(json.dumps({"source_sha256": DIGEST, "pairs": results}, indent=2) + "\n")
        print(label, "runtime_red=" + str(red["runtime_red"]), "expected_assertion=" + str(red["expected_assertion"]), "restored_green=" + str(green["green"]), flush=True)
        if not red["runtime_red"] or not red["expected_assertion"] or not green["green"]:
            raise SystemExit("mutation pair did not earn runtime credit: " + label)
finally:
    for name, data in SAVED.items():
        (ROOT / name).write_bytes(data)
    assert all((ROOT / name).read_bytes() == data for name, data in SAVED.items())
