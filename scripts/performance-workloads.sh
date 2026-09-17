#!/usr/bin/env bash
# Repeatable release-mode performance workloads (typical 1080p, heavy 4K,
# agent edit plans). Generates synthetic footage, runs all three lanes through
# the real engine/Core, and writes machine JSON.
#
# Usage:
#   scripts/performance-workloads.sh [--lane all|typical|heavy|agent]...
#     [--workdir DIR] [--out FILE] [--seeks N] [--plans N]
#
# Defaults: --lane all, --workdir /tmp/kinewright-perf/workloads,
# --out <workdir>/report.json, 20/10 measured seeks, 25 plans.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"

# shellcheck source=setup-ffmpeg.sh
source "$script_dir/setup-ffmpeg.sh"

cd "$repo_root"
exec cargo run --release -p kinewright-media --example performance_workloads -- "$@"
