#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
python3() { command nice -n 19 python3 -I "$@"; }
source scripts/setup-ffmpeg.sh >/dev/null
awk '/MemAvailable:/ {if ($2 < 6291456) exit 1}' /proc/meminfo
export RUST_TEST_THREADS=4
export LOCALAPPDATA="$PWD/target/int-localappdata"
mkdir -p "$LOCALAPPDATA/Kinewright/recovery"
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
