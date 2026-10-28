#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3() { command nice -n 19 python3 -I "$@"; }
export -f python3
source scripts/setup-ffmpeg.sh
export LOCALAPPDATA="$PWD/target/c1fix-localappdata"
export XDG_STATE_HOME="$PWD/target/ja1-s3-c2/runtime/state"
export XDG_DATA_HOME="$PWD/target/ja1-s3-c2/runtime/share"
export XDG_CACHE_HOME="$PWD/target/ja1-s3-c2/runtime/cache"
export XDG_CONFIG_HOME="$PWD/target/ja1-s3-c2/runtime/config"
export KINEWRIGHT_SCREENSHOT_STUDIO_BUNDLE="$PWD/target/ja1-s3-c2/fixtures/c0-red.kinewright"
export KINEWRIGHT_SCREENSHOT_AFTER_MS=3000
test -f "$KINEWRIGHT_SCREENSHOT_STUDIO_BUNDLE"
for c2_capture in 1232x862 1600x1000 600x900; do
    test "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -ge 6291456
    export KINEWRIGHT_SCREENSHOT_SIZE="$c2_capture"
    export KINEWRIGHT_SCREENSHOT_TO="$PWD/target/ja1-s3-c2/studio-$c2_capture.png"
    flock -o /tmp/kinewright-cargo-heavy.lock nice -n 10 timeout --kill-after=5s 90s \
        target/debug/kinewright-app > "target/ja1-s3-c2/logs/screenshot-$c2_capture.log" 2>&1
    test -s "$KINEWRIGHT_SCREENSHOT_TO"
    nice -n 19 python3 -I - "$KINEWRIGHT_SCREENSHOT_TO" "$c2_capture" <<'PY'
from pathlib import Path
import struct
import sys

actual = struct.unpack(">II", Path(sys.argv[1]).read_bytes()[16:24])
expected = tuple(int(value) for value in sys.argv[2].split("x"))
assert actual == expected, f"capture dimensions {actual} differ from {expected}"
PY
    printf '%s\n' "$KINEWRIGHT_SCREENSHOT_TO"
done
