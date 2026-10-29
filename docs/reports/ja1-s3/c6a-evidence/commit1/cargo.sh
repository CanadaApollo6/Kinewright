#!/usr/bin/env bash
set -eu
cd /home/riels/.t3/worktrees/Kinewright/t3-1c0887c5
source scripts/setup-ffmpeg.sh
export LOCALAPPDATA="$PWD/target/c1fix-localappdata" TMPDIR="$PWD/target/tmp" RUST_TEST_THREADS=4
awk '/MemAvailable:/ {if ($2 < 6291456) exit 1}' /proc/meminfo
exec timeout 600s flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
