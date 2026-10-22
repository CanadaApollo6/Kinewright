#!/usr/bin/env python3
"""Resolve a declared cargo test selection to its already-built test executable."""
import json
import sys
import subprocess

with open(sys.argv[1], encoding="utf-8") as source:
    binaries = json.load(source)
args = sys.argv[2:]
if args[:2] != ["cargo", "test"]:
    raise SystemExit("expected a cargo test selection")
args = args[2:]
package = target = None
filters = []
while args:
    arg, *args = args
    if arg == "--":
        filters.extend(args)
        break
    if arg == "-p":
        package, *args = args
    elif arg in ("--bin", "--test"):
        target, *args = args
    elif arg == "--lib":
        if package is None:
            raise SystemExit("package must precede --lib")
        target = package.replace("-", "_")
    elif arg == "--features":
        _, *args = args
    elif arg.startswith("-"):
        raise SystemExit(f"unsupported selection option: {arg}")
    else:
        filters.append(arg)
if target not in binaries:
    raise SystemExit(f"missing provisioned test target: {target}")
listing = subprocess.run([binaries[target], "--list"], capture_output=True, text=True, check=True)
names = [line.removesuffix(": test") for line in listing.stdout.splitlines() if line.endswith(": test")]
requested = [arg for arg in filters if not arg.startswith("-")]
exact = "--exact" in filters
for requested_name in requested:
    if not any(name == requested_name if exact else requested_name in name for name in names):
        raise SystemExit(f"selected test does not exist: {target}::{requested_name}")
selected = [name for name in names if not requested or any(name == value if exact else value in name for value in requested)]
if not selected:
    raise SystemExit(f"empty test selection: {target}")
print(json.dumps({"target": target, "binary": binaries[target], "selected": selected, "count": len(selected)}), file=sys.stderr)
for arg in [binaries[target], *filters]:
    sys.stdout.buffer.write(arg.encode() + b"\0")
