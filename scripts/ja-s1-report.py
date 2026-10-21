"""Summarize preserved gate logs without replacing their literal test summaries."""
import argparse
import gzip
import json
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("ordinal", type=int)
parser.add_argument("gate", type=Path)
parser.add_argument("mutations", type=Path)
args = parser.parse_args()


def summaries(directory):
    return {
        path.name.removesuffix(".log.gz"): re.findall(
            r"^test result:.*$", gzip.open(path, "rt").read(), re.M
        )
        for path in sorted(directory.glob("*.log.gz"))
    }


gates = summaries(args.gate)
lines = [line for group in gates.values() for line in group]
assert lines and all("test result: ok." in line for line in lines)
report = {
    "ordinal": args.ordinal,
    "parent": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "linux_gate": args.gate.name,
    "gates": gates,
    "gate_command_count": len(gates),
    "test_summary_count": len(lines),
    "selected_passes_including_overlapping_selections": sum(
        int(re.search(r"ok\. (\d+) passed", line)[1]) for line in lines
    ),
    "mutations_directory": args.mutations.name,
    "mutations": summaries(args.mutations),
    "windows_native": "NOT RUN; lead CI lane",
    "windows_full_cfg": "NOT RUN; native MSVC cross-build prerequisites unavailable",
    "full_fast_and_slow_ci": "NOT RUN; no push authorized",
    "decoder_open_certification": "NOT RUN; L4 integration dependency",
}
path = Path(f"docs/reports/ja1-s1/commit-{args.ordinal}.json")
path.write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({key: report[key] for key in (
    "gate_command_count", "test_summary_count",
    "selected_passes_including_overlapping_selections",
)}))
