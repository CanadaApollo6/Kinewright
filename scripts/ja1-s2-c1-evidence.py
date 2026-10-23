#!/usr/bin/env python3
"""C1 source binding and libtest identity/count checks; invoke with nice -n 19 python3 -I."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def git(*args):
    return subprocess.check_output(["git", *args])


def identity(destination):
    files = git("ls-files", "-co", "--exclude-standard", "-z").split(b"\0")
    hashes = {}
    for raw in files:
        if not raw:
            continue
        path = Path(raw.decode())
        if path.is_file() and not path.is_symlink():
            hashes[str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
    write(destination, {"head": git("rev-parse", "HEAD").decode().strip(),
                        "diff_sha256": hashlib.sha256(git("diff", "--binary", "HEAD")).hexdigest(),
                        "files": hashes})


def select(log, filters, destination):
    names = re.findall(r"^(.+): test$", Path(log).read_text(), re.M)
    patterns = filters.split(",")
    selected = [name for name in names if any(
        pattern == "*" or (name.startswith(pattern) if pattern.endswith("::") else name == pattern)
        for pattern in patterns)]
    for pattern in patterns:
        if not any(pattern == "*" or (name.startswith(pattern) if pattern.endswith("::") else name == pattern)
                   for name in selected):
            raise SystemExit(f"Empty applicable selector: {pattern}")
    if len(set(names)) != len(names) or not selected:
        raise SystemExit("Duplicate compiled names or empty applicable target")
    Path(destination).write_text("\n".join(sorted(selected)) + "\n")
    write(destination + ".json", {"compiled": len(names), "expected": len(selected), "selected": len(selected), "filters": patterns})


def verify(selected, log, destination):
    expected = set(Path(selected).read_text().splitlines())
    output = Path(log).read_text()
    outcomes = re.findall(r"^test (.+?) \.\.\. (ok|ignored|FAILED)(?:,.*| .*|)$", output, re.M)
    observed = {name: status for name, status in outcomes}
    if set(observed) != expected or len(observed) != len(outcomes):
        raise SystemExit(f"Selected/run identity mismatch: missing={expected-set(observed)}, extra={set(observed)-expected}")
    summaries = re.findall(r"^test result:.*$", output, re.M)
    if len(summaries) != 1 or any(status == "FAILED" for status in observed.values()):
        raise SystemExit("Missing summary or failed selected tests")
    ignored = [name for name, status in observed.items() if status == "ignored"]
    if any("s2_" in name or "w13_" in name for name in ignored):
        raise SystemExit("New C1 witness was ignored")
    executables = re.findall(r"^\s*Running .* \(([^()]+)\)$", output, re.M)
    if len(executables) != 1:
        raise SystemExit("Missing or ambiguous executed test binary identity")
    binary = Path(executables[0]).resolve()
    with binary.open("rb") as stream:
        binary_sha256 = hashlib.file_digest(stream, "sha256").hexdigest()
    write(destination, {"expected": len(expected), "selected": len(expected),
                        "run": sum(status == "ok" for status in observed.values()),
                        "ignored_not_run": ignored, "raw_summary": summaries[0], "binary": str(binary), "binary_sha256": binary_sha256, "profile": "test"})
    print(f"{destination}: {summaries[0]}")


if __name__ == "__main__":
    {"identity": identity, "select": select, "verify": verify}[sys.argv[1]](*sys.argv[2:])
