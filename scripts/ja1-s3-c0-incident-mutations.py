#!/usr/bin/env python3
"""C0 Studio-owned colour guards: baseline, one red run per guard, restored green.

Run from anywhere as `nice -n 19 python3 -I scripts/ja1-s3-c0-incident-mutations.py`.
Every Cargo run re-checks free memory, sources FFmpeg, and takes the shared heavy lock.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
SOURCE = "crates/kinewright-app/src/app.rs"
EVIDENCE = ROOT / "docs/reports/ja1-s3/evidence/c0-incident-mutations"
MIN_AVAILABLE = 6 * 1024**3
TIMEOUT = 600
CARGO = (
    "set -e; source scripts/setup-ffmpeg.sh >/dev/null; "
    "exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 "
    'cargo test -p kinewright-app --bin kinewright-app -- "$@" --exact --test-threads=1'
)
WITNESSES = {
    "auto": (
        "app::in1_tests::c0_studio_owned::c0_studio_owned_auto_colour_fix_is_not_sent_unstaged",
        "c0-colour-auto: the automatic fix must not reach Core unstaged",
    ),
    "card": (
        "app::in1_tests::c0_studio_owned::c0_studio_owned_card_colour_fix_is_not_sent_unstaged",
        "c0-colour-card: the card's fix must not reach Core unstaged",
    ),
}
CONTROLS = [
    "app::in1_tests::in1_the_undo_sticks_and_the_reopen_is_suppressed",
    "app::in1_tests::in1b_a_refused_card_press_is_reported_where_an_auto_apply_is_not",
]
GUARD = (
    "        if self.studio_source_colour_refused(project_index, &operation) {\n"
    "            return;\n"
    "        }\n"
)
MUTATIONS = [
    {
        "name": "auto-apply-guard",
        "red": "auto",
        "before": "        let Some(operation) = operation else {\n"
        "            return;\n"
        "        };\n" + GUARD,
        "after": "        let Some(operation) = operation else {\n"
        "            return;\n"
        "        };\n",
    },
    {
        "name": "card-press-guard",
        "red": "card",
        "before": "        outcome: IncidentOutcome,\n    ) {\n" + GUARD,
        "after": "        outcome: IncidentOutcome,\n    ) {\n",
    },
]
TESTS = [name for name, _ in WITNESSES.values()] + CONTROLS
FALSE_CREDIT = ("called `Result::unwrap()`", "called `Option::unwrap()`")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise RuntimeError("MemAvailable missing from /proc/meminfo")


def cargo(phase):
    available = available_memory()
    if available < MIN_AVAILABLE:
        raise RuntimeError(f"{phase}: only {available / 1024**3:.1f} GiB free, need 6 GiB")
    process = subprocess.Popen(
        ["bash", "-c", CARGO, "bash", *TESTS],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=TIMEOUT)
        code = process.returncode
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.communicate()
        raise
    log = EVIDENCE / f"{phase}.log.gz"
    with gzip.open(log, "wb") as handle:
        handle.write(b"$ " + " ".join(["cargo test -p kinewright-app --bin kinewright-app --", *TESTS]).encode())
        handle.write(b"\n--- stdout ---\n" + stdout + b"\n--- stderr ---\n" + stderr)
    return code, stdout.decode(errors="replace"), log


def outcomes(stdout):
    result = {}
    for name in TESTS:
        ran = re.search(rf"test {re.escape(name)} \.\.\. ", stdout) is not None
        section = re.search(
            rf"---- {re.escape(name)} stdout ----\n(.*?)(?=\n---- |\nfailures:\n|\Z)", stdout, re.S
        )
        result[name] = {
            "ran": ran,
            "failed": section is not None,
            "section": section.group(1) if section else "",
        }
    return result


def assertion_line(text, marker):
    lines = text.splitlines()
    hits = [index for index, line in enumerate(lines) if marker in line]
    if len(hits) != 1:
        raise RuntimeError(f"marker not unique in {SOURCE}: {marker}")
    for index in range(hits[0], max(hits[0] - 8, -1), -1):
        if lines[index].lstrip().startswith(("assert!(", "assert_eq!(")):
            return index + 1
    raise RuntimeError(f"no assertion above marker: {marker}")


def red_at_assertion(test, text, marker):
    panic = re.search(r"panicked at ([^:\n]+):(\d+):\d+:\n(.*)", test["section"], re.S)
    if not test["failed"] or panic is None:
        return False, "no panic recorded"
    location = (panic.group(1), int(panic.group(2)))
    expected = (SOURCE, assertion_line(text, marker))
    if location != expected:
        return False, f"panicked at {location}, expected {expected}"
    if marker not in panic.group(3):
        return False, "panic message lacks the witness marker"
    if any(needle in test["section"] for needle in FALSE_CREDIT):
        return False, "unwrap or expect panic"
    return True, f"{location[0]}:{location[1]}"


def all_green(phase, code, stdout):
    states = outcomes(stdout)
    bad = [name for name, state in states.items() if not state["ran"] or state["failed"]]
    summary = re.findall(rf"^test result: ok\. {len(TESTS)} passed; 0 failed", stdout, re.M)
    accepted = code == 0 and not bad and len(summary) == 1
    return {"phase": phase, "exit": code, "accepted": accepted, "not_green": bad}


def mutation_red(mutation, code, stdout, text):
    states = outcomes(stdout)
    name, marker = WITNESSES[mutation["red"]]
    red, where = red_at_assertion(states[name], text, marker)
    others = [test for test in TESTS if test != name]
    bad = [test for test in others if not states[test]["ran"] or states[test]["failed"]]
    summary = re.findall(rf"^test result: FAILED\. {len(TESTS) - 1} passed; 1 failed", stdout, re.M)
    accepted = code == 101 and red and not bad and len(summary) == 1
    return {
        "phase": mutation["name"],
        "exit": code,
        "accepted": accepted,
        "red_witness": name,
        "red_at": where,
        "unexpected": bad,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    path = ROOT / SOURCE
    original = path.read_bytes()
    original_sha = sha256(original)
    text = original.decode()
    for mutation in MUTATIONS:
        if text.count(mutation["before"]) != 1:
            raise RuntimeError(f"non-unique mutation anchor: {mutation['name']}")
    for name, marker in WITNESSES.values():
        assertion_line(text, marker)
    if args.list:
        for mutation in MUTATIONS:
            name, marker = WITNESSES[mutation["red"]]
            print(mutation["name"], name, marker, sep="\t")
        return
    EVIDENCE.mkdir(parents=True, exist_ok=False)
    rows = []
    try:
        code, stdout, log = cargo("baseline")
        baseline = {**all_green("baseline", code, stdout), "log": log.name, "source_sha256": original_sha}
        rows.append(baseline)
        if not baseline["accepted"]:
            raise RuntimeError("baseline did not pass; no mutation was attempted")
        for mutation in MUTATIONS:
            mutated = text.replace(mutation["before"], mutation["after"], 1)
            path.write_text(mutated)
            try:
                code, stdout, log = cargo(mutation["name"])
            finally:
                path.write_bytes(original)
                if sha256(path.read_bytes()) != original_sha:
                    raise RuntimeError("restoration differs")
            red = {**mutation_red(mutation, code, stdout, mutated), "log": log.name, "source_sha256": sha256(mutated.encode())}
            rows.append(red)
            if not red["accepted"]:
                raise RuntimeError(f"not the intended assertion red: {mutation['name']}")
            label = mutation["name"] + "-restored"
            code, stdout, log = cargo(label)
            green = {**all_green(label, code, stdout), "log": log.name, "source_sha256": sha256(path.read_bytes())}
            rows.append(green)
            if not green["accepted"]:
                raise RuntimeError(f"restored witness failed: {mutation['name']}")
    finally:
        path.write_bytes(original)
        restored = sha256(path.read_bytes()) == original_sha
        (EVIDENCE / "results.json").write_text(
            json.dumps({"source": SOURCE, "source_sha256": original_sha, "restored": restored, "tests": TESTS, "rows": rows}, indent=2) + "\n"
        )
    for row in rows:
        print(row["phase"], "accepted" if row["accepted"] else "REJECTED", row.get("red_at", ""), row["exit"], flush=True)
    if not restored or len(rows) != len(MUTATIONS) * 2 + 1 or not all(row["accepted"] for row in rows):
        sys.exit(1)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    main()
