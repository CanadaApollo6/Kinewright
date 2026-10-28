#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
hook_label=$1
shift
hook_logs="$PWD/target/ja1-s3-hooks-fix1/logs"
mkdir -p "$hook_logs" "$PWD/target/ja1-s3-hooks-fix1/runtime/tmp"
python3() { command nice -n 19 python3 -I "$@"; }
export -f python3
source scripts/setup-ffmpeg.sh
test "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 6291456
export LOCALAPPDATA="$PWD/target/c1fix-localappdata"
export TMPDIR="$PWD/target/ja1-s3-hooks-fix1/runtime/tmp"
export XDG_STATE_HOME="$PWD/target/ja1-s3-hooks-fix1/runtime/state"
export XDG_DATA_HOME="$PWD/target/ja1-s3-hooks-fix1/runtime/share"
export XDG_CACHE_HOME="$PWD/target/ja1-s3-hooks-fix1/runtime/cache"
export XDG_CONFIG_HOME="$PWD/target/ja1-s3-hooks-fix1/runtime/config"
mkdir -p "$LOCALAPPDATA/Kinewright/recovery" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"
export RUST_TEST_THREADS=4
printf '%s\n' "$*" > "$hook_logs/$hook_label.command"
hook_attempt=0
while true; do
    hook_attempt=$((hook_attempt + 1))
    test "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 6291456
    set +e
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 timeout --kill-after=5s 600s nice -n 10 cargo "$@" > "$hook_logs/$hook_label.attempt-$hook_attempt.log" 2>&1
    hook_exit=$?
    set -e
    if [ "$hook_exit" -eq 124 ] || [ "$hook_exit" -eq 137 ]; then
        printf '600000 ms timeout for %s (attempt %s); rerunning in foreground.\n' "$hook_label" "$hook_attempt"
        continue
    fi
    cp "$hook_logs/$hook_label.attempt-$hook_attempt.log" "$hook_logs/$hook_label.log"
    printf '%s\n' "$hook_exit" > "$hook_logs/$hook_label.exit"
    tail -n 12 "$hook_logs/$hook_label.log"
    exit "$hook_exit"
done
