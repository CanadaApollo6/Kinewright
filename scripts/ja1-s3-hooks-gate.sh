#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
hook_label=$1
shift
hook_logs="$PWD/target/ja1-s3-hooks/logs"
mkdir -p "$hook_logs" "$PWD/target/ja1-s3-hooks/runtime/tmp"
python3() { command nice -n 19 python3 -I "$@"; }
export -f python3
source scripts/setup-ffmpeg.sh
test "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 6291456
export LOCALAPPDATA="$PWD/target/c1fix-localappdata"
export TMPDIR="$PWD/target/ja1-s3-hooks/runtime/tmp"
export XDG_STATE_HOME="$PWD/target/ja1-s3-hooks/runtime/state"
export XDG_DATA_HOME="$PWD/target/ja1-s3-hooks/runtime/share"
export XDG_CACHE_HOME="$PWD/target/ja1-s3-hooks/runtime/cache"
export XDG_CONFIG_HOME="$PWD/target/ja1-s3-hooks/runtime/config"
mkdir -p "$LOCALAPPDATA/Kinewright/recovery" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"
export RUST_TEST_THREADS=4
printf '%s\n' "$*" > "$hook_logs/$hook_label.command"
set +e
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@" > "$hook_logs/$hook_label.log" 2>&1
hook_exit=$?
set -e
printf '%s\n' "$hook_exit" > "$hook_logs/$hook_label.exit"
tail -n 12 "$hook_logs/$hook_label.log"
exit "$hook_exit"
