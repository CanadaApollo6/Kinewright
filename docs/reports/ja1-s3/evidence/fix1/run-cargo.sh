#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/../../../../.."
source scripts/setup-ffmpeg.sh
available_kib=$(awk '/MemAvailable/ {print $2}' /proc/meminfo)
(( available_kib >= 6 * 1024 * 1024 ))
export CARGO_TARGET_DIR="$PWD/target"
export TMPDIR="$PWD/target/tmp"
export RUST_TEST_THREADS=1
mkdir -p "$TMPDIR"
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
