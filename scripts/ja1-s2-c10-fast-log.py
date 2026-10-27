#!/usr/bin/env python3
"""Restore Cargo's ordered section headers after capturing child stderr privately."""
import hashlib
import json
from pathlib import Path
import re
import sys

combined, errors, destination = map(Path, sys.argv[1:])
raw = combined.read_bytes()
stderr = errors.read_bytes()
if not stderr or not raw.endswith(stderr):
    raise SystemExit("expected the gate's exact stdout-then-stderr capture")
stdout = raw[:-len(stderr)]
is_header = lambda line: re.match(rb"^\s+(?:Running .*\([^()]+\)|Doc-tests \S+)\s*$", line)
headers = [line for line in stderr.splitlines(keepends=True) if is_header(line)]
starts = re.findall(rb"^running \d+ tests?$", stdout, re.M)
summaries = re.findall(rb"^test result:.*$", stdout, re.M)
if not headers or len(headers) != len(starts) or len(starts) != len(summaries):
    raise SystemExit("Cargo headers, libtest starts and summaries must correspond one-to-one")
parts = []
index = 0
for line in stdout.splitlines(keepends=True):
    if re.fullmatch(rb"running \d+ tests?\r?\n?", line):
        parts.append(headers[index])
        index += 1
    parts.append(line)
parts.extend(line for line in stderr.splitlines(keepends=True) if not is_header(line))
destination.write_bytes(b"".join(parts))
destination.with_suffix(".json").write_text(json.dumps({
    "combined_sha256": hashlib.sha256(raw).hexdigest(),
    "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
    "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
    "structured_sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
    "sections": index,
    "operation": "insert each unchanged ordered Cargo header before its corresponding libtest start; preserve every stdout byte and append non-header stderr",
}, indent=2) + "\n")
print(f"Preserved {index} complete sections; verify with the unchanged stock fast-tier parser.")
