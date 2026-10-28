#!/usr/bin/env bash
set -euo pipefail
cd /home/riels/.t3/worktrees/Kinewright/t3-0fe63804
source scripts/setup-ffmpeg.sh
export CARGO_TARGET_DIR="$PWD/target"
export TMPDIR="$PWD/target/s3-b-runtime/tmp"
export LOCALAPPDATA="$PWD/target/s3-b-runtime/localappdata"
export XDG_STATE_HOME="$PWD/target/s3-b-runtime/state"
export RUST_TEST_THREADS=4
mkdir -p "$TMPDIR" "$LOCALAPPDATA/Kinewright/recovery" "$XDG_STATE_HOME"
s3_evidence="docs/reports/ja1-s3/b-evidence/$1"
s3_label="$2"
shift 2
mkdir -p "$s3_evidence"
printf '%q ' "$@" > "$s3_evidence/$s3_label.command"
printf '\n' >> "$s3_evidence/$s3_label.command"
s3_available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
(( s3_available >= 6 * 1024 * 1024 )) || exit 75
s3_code=0
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@" > "$s3_evidence/$s3_label.log" 2>&1 || s3_code=$?
printf '%s\n' "$s3_code" > "$s3_evidence/$s3_label.exit"
tail -n 18 "$s3_evidence/$s3_label.log"
gzip -f "$s3_evidence/$s3_label.log"
exit "$s3_code"
