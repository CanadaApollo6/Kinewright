#!/usr/bin/env bash
set -euo pipefail
cd /home/riels/.t3/worktrees/Kinewright/t3-0fe63804
source scripts/setup-ffmpeg.sh
export CARGO_TARGET_DIR="$PWD/target"
export TMPDIR="$PWD/target/s3-b-runtime/tmp"
export LOCALAPPDATA="$PWD/target/s3-b-runtime/localappdata"
export XDG_STATE_HOME="$PWD/target/s3-b-runtime/state"
export XDG_DATA_HOME="$PWD/target/s3-b-runtime/data"
export XDG_CONFIG_HOME="$PWD/target/s3-b-runtime/config"
export XDG_CACHE_HOME="$PWD/target/s3-b-runtime/cache"
export RUST_TEST_THREADS=4
out=docs/reports/ja1-s3/b-evidence/c5-fix3
available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
(( available >= 6 * 1024 * 1024 )) || exit 75
printf '%s\n' 'nice -n 19 python3 -I scripts/slow_tests.py fast -- -j 4' > "$out/workspace-fast.command"
code=0
# Python and its Cargo/test children inherit nice 19, with the same shared lock and job cap.
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 19 python3 -I scripts/slow_tests.py fast -- -j 4 > "$out/workspace-fast.log" 2>&1 || code=$?
printf '%s\n' "$code" > "$out/workspace-fast.exit"
tail -n 20 "$out/workspace-fast.log"
gzip -f "$out/workspace-fast.log"
exit "$code"
