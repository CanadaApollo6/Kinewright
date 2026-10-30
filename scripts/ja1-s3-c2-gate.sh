#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3() { command nice -n 19 python3 -I "$@"; }
export -f python3
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4
export LOCALAPPDATA="$PWD/target/c1fix-localappdata"
c2_runtime="$PWD/target/ja1-s3-c2/runtime"
export TMPDIR="$c2_runtime/tmp" XDG_STATE_HOME="$c2_runtime/state" XDG_DATA_HOME="$c2_runtime/share" XDG_CACHE_HOME="$c2_runtime/cache" XDG_CONFIG_HOME="$c2_runtime/config"
mkdir -p "$TMPDIR" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$LOCALAPPDATA/Kinewright/recovery" target/ja1-s3-c2/logs
c2_label=${1:?label}; shift
c2_result=0
printf '%q ' "$@" > "target/ja1-s3-c2/logs/$c2_label.command"
printf '\n' >> "target/ja1-s3-c2/logs/$c2_label.command"
if [[ "$1" == python ]]; then
    shift
    nice -n 19 python3 -I "$@" > "target/ja1-s3-c2/logs/$c2_label.log" 2>&1 || c2_result=$?
else
    test "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -ge 6291456
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@" > "target/ja1-s3-c2/logs/$c2_label.log" 2>&1 || c2_result=$?
fi
printf '%s\n' "$c2_result" > "target/ja1-s3-c2/logs/$c2_label.exit"
tail -n 24 "target/ja1-s3-c2/logs/$c2_label.log"
exit "$c2_result"
