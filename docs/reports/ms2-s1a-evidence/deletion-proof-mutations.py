"""Deletion-proof and cleanup-bound mutations (lead ruling 2026-11-18).

Run from the repository root with `nice -n 19 python3 -I` after
`source scripts/setup-ffmpeg.sh`. Each mutation must compile and turn its
witness red; the restored source must turn it green again. Original bytes are
restored in a finally block. Pass mutation names to run a subset; their
results replace the same names in the JSON record.
"""
from pathlib import Path
import json
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).with_suffix(".json")
LINUX = "crates/kinewright-job-sys/src/linux.rs"
WORKER = "crates/kinewright-media-client/src/linux/worker.rs"
HEAVY = ["flock", "-o", "/tmp/kinewright-cargo-heavy.lock", "env", "CARGO_BUILD_JOBS=4", "nice", "-n", "10", "timeout", "600"]
JOB_SYS_UNIT = ["cargo", "test", "--locked", "-p", "kinewright-job-sys", "--lib", "deletion_proof_tests"]
GUARDIAN = ["cargo", "test", "--locked", "-p", "kinewright-agent", "--features", "slow-tests",
            "--test", "ja_s4_guardian", "--", "--exact", "s4_retained_inode_deletion_signal", "--nocapture"]
CLIENT = ["cargo", "test", "--locked", "-p", "kinewright-media-client", "--features", "slow-tests",
          "--test", "runtime_native", "--", "--exact"]


def replace(text, old, new):
    assert text.count(old) == 1, (old, text.count(old))
    return text.replace(old, new)


def body(text, signature, replacement):
    start = text.index(signature)
    opening = text.index("{", start)
    depth, end = 1, opening + 1
    while depth:
        depth += {"{": 1, "}": -1}.get(text[end], 0)
        end += 1
    return text[:opening + 1] + "\n" + replacement + "\n" + text[end - 1:]


CASES = [
    # The ENODEV arm: without it a removed two-link cgroup is never deleted.
    ("deletion-enodev", LINUX, JOB_SYS_UNIT,
     lambda t: replace(t, "CgroupEvents::Removed => Ok(true),", "CgroupEvents::Removed => Ok(false),")),
    # A readable events file must never prove deletion, whatever the links say.
    ("deletion-readable", LINUX, JOB_SYS_UNIT,
     lambda t: replace(t, "CgroupEvents::Populated(_) => Ok(false),", "CgroupEvents::Populated(_) => Ok(true),")),
    # The pre-existing C2 mutation, on the new body: a live leaf proves deletion.
    ("retained-nlink", LINUX, GUARDIAN,
     lambda t: body(t, "pub fn kernel_deleted(", "Ok(true)")),
    # Back to the link-count-only proof: the hosted-kernel shape is never deleted.
    ("nlink-only", LINUX, CLIENT + ["runtime_scope_proof_holds_when_a_removed_scope_keeps_two_links"],
     lambda t: replace(t, "CgroupEvents::Removed => Ok(true),", "CgroupEvents::Removed => Ok(self.links? == 0),")),
]
if (ROOT / WORKER).read_text(encoding="utf-8").count("SCOPE_PROOF_ERRORS"):
    CASES.append(
        # The cleanup loop keeps polling a persistent error for ever.
        ("cleanup-error-bound", WORKER, CLIENT + ["runtime_scope_proof_error_keeps_the_reservation"],
         lambda t: replace(t, "if errors_since.elapsed() >= SCOPE_PROOF_ERRORS {", "if false {")),
    )


def run(command):
    done = subprocess.run(HEAVY + command, cwd=ROOT, capture_output=True, text=True)
    output = done.stdout + done.stderr
    compiled = "error[E" not in output and "could not compile" not in output
    last = next((line for line in reversed(output.splitlines()) if line.strip()), "")
    return done.returncode, compiled, last, output


def main():
    selected = set(sys.argv[1:])
    results = []
    for name, file, command, mutate in CASES:
        if selected and name not in selected:
            continue
        path = ROOT / file
        original = path.read_bytes()
        try:
            path.write_text(mutate(original.decode("utf-8")), encoding="utf-8")
            red, compiled, red_last, red_output = run(command)
        finally:
            path.write_bytes(original)
        green, _, green_last, _ = run(command)
        killed = compiled and red != 0 and green == 0
        (ROOT / "target/ms2-hang" / f"mutation-{name}-red.log").write_text(red_output, encoding="utf-8")
        results.append({"mutation": name, "killed": killed, "compiled": compiled,
                        "red_exit": red, "red_last": red_last, "green_exit": green, "green_last": green_last})
        print(f"{name}: {'killed' if killed else 'SURVIVED'} red_exit={red} green_exit={green} :: {red_last}")
    ran = {result["mutation"] for result in results}
    kept = json.loads(OUT.read_text(encoding="utf-8")) if OUT.exists() else []
    merged = [result for result in kept if result["mutation"] not in ran] + results
    OUT.write_text(json.dumps(merged, indent=2) + "\n", encoding="utf-8")
    return 0 if all(result["killed"] for result in results) else 1


if __name__ == "__main__":
    sys.exit(main())
