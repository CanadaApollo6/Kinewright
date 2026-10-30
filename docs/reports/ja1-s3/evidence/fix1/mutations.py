"""Run individual C0 fix guards in this worker's exclusive source lane."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run(root, evidence, case, phase):
    args = ["test", "-p", case["crate"]]
    if case["crate"] == "kinewright-project":
        args += ["--features", "test-util"]
    args += ["--lib"] if case["target"] == "lib" else ["--test", case["target"]]
    args += [case["selector"], "--", "--test-threads=1", "--nocapture"]
    command = ["bash", str(evidence / "run-cargo.sh"), *args]
    print(case["id"], phase, flush=True)
    process = subprocess.Popen(command, cwd=root, text=True, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, start_new_session=True)
    try:
        output = process.communicate(timeout=1800)[0]
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        output = process.communicate()[0]
    result = subprocess.CompletedProcess(command, process.returncode, output)
    log_path = evidence / f"{case['id']}-{phase}.log.gz"
    if log_path.exists():
        attempt = 1
        while (evidence / f"{case['id']}-{phase}-prior{attempt}.log.gz").exists():
            attempt += 1
        log_path.rename(evidence / f"{case['id']}-{phase}-prior{attempt}.log.gz")
    with gzip.open(log_path, "wt") as log:
        log.write(result.stdout)
    return result, args


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--case", action="append")
    args = parser.parse_args()
    root = Path.cwd()
    if root != Path("/home/riels/.t3/worktrees/Kinewright/ja1-s3-c0fix"):
        raise SystemExit("wrong worktree")
    branch = subprocess.check_output(["git", "branch", "--show-current"], text=True).strip()
    if branch != "ja1/s3-c0fix":
        raise SystemExit("wrong branch")
    evidence = root / "docs/reports/ja1-s3/evidence/fix1"
    cases = json.loads((evidence / "mutations.json").read_text())
    if args.case and set(args.case) - {case["id"] for case in cases}:
        raise SystemExit("unknown mutation")
    selected = [case for case in cases if not args.case or case["id"] in args.case]
    snapshots = {case["file"]: (root / case["file"]).read_bytes() for case in cases}
    for case in cases:
        if snapshots[case["file"]].count(case["old"].encode()) != 1:
            raise SystemExit("mutation selector drift: " + case["id"])
    records_path = evidence / "mutation-results.json"
    records = json.loads(records_path.read_text()) if records_path.exists() else []
    for case in selected:
        path = root / case["file"]
        original = snapshots[case["file"]]
        if any((root / name).read_bytes() != data for name, data in snapshots.items()):
            raise SystemExit("source changed since preflight")
        mutant = original.replace(case["old"].encode(), case["new"].encode(), 1)
        try:
            path.write_bytes(mutant)
            red, command = run(root, evidence, case, "red")
        finally:
            path.write_bytes(original)
        if sha(path.read_bytes()) != sha(original):
            raise SystemExit("restored source differs")
        green, _ = run(root, evidence, case, "green")
        markers = [case["marker"], *case.get("alternate_markers", [])]
        observed = [marker for marker in markers if marker in red.stdout]
        behavioral_red = (red.returncode == 101 and bool(observed)
                          and "panicked at" in red.stdout and "test result: FAILED." in red.stdout
                          and not re.search(r"could not compile|error\[E\d+\]", red.stdout))
        restored_green = (green.returncode == 0 and
                          "test result: ok. 1 passed; 0 failed; 0 ignored;" in green.stdout)
        record = dict(id=case["id"], file=case["file"], source_sha256=sha(original),
                      witness_file=case["witness_file"],
                      witness_sha256=sha((root / case["witness_file"]).read_bytes()),
                      mutant_sha256=sha(mutant), restored_sha256=sha(path.read_bytes()),
                      command=command, selector=case["selector"],
                      red_exit=red.returncode, green_exit=green.returncode,
                      marker=case["marker"], behavioral_red=behavioral_red,
                      observed_markers=observed,
                      restored_green=restored_green, credited=behavioral_red and restored_green)
        records.append(record)
        records_path.write_text(json.dumps(records, indent=2) + "\n")
        print(case["id"], "PASS" if record["credited"] else "UNQUALIFIED", flush=True)
        if not record["credited"]:
            raise SystemExit("unqualified mutation pair; source restored")


if __name__ == "__main__":
    main()
