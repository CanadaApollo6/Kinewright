#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
available_kib=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
if (( available_kib < 6 * 1024 * 1024 )); then
    echo "C1 blocked: less than 6 GiB memory available" >&2
    exit 75
fi
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4
flock /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
