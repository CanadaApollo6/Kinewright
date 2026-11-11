#!/usr/bin/env python3
"""Filter unreachable leading apt mirrors into a job-local file; never edit sources."""
import argparse
from pathlib import Path
import subprocess


def select(source, destination, suite, seconds):
    lines = source.read_text().splitlines(keepends=True)
    # The runner image lists explicit priorities in ascending order.
    for index, line in enumerate(lines):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        mirror = line.split()[0]
        print(f"Probing apt mirror {mirror} (at most {seconds:g}s)", flush=True)
        result = subprocess.run([
            "curl", "--noproxy", "*", "--fail", "--location", "--silent",
            "--show-error", "--connect-timeout", str(seconds), "--max-time", str(seconds),
            "--retry", "0", "--output", "/dev/null",
            f"{mirror.rstrip('/')}/dists/{suite}/InRelease",
        ], check=False)
        if result.returncode == 0:
            destination.write_text("".join(lines[index:]))
            print(f"Selected apt mirrors: {destination.read_text().strip()}", flush=True)
            return
        print(f"Dropping unreachable apt mirror {mirror} for this job", flush=True)
    raise RuntimeError(f"No apt mirror answered within {seconds:g}s each: {source.read_text().strip()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--suite", default="noble")
    parser.add_argument("--seconds", type=float, default=3)
    args = parser.parse_args()
    if not 0 < args.seconds <= 5:
        parser.error("probe seconds must be greater than zero and at most five")
    select(args.source, args.destination, args.suite, args.seconds)


if __name__ == "__main__":
    main()
