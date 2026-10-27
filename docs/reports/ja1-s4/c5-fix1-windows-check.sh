#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4/c5-fix1
awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo || exit 1
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 \
    cargo check -p kinewright-agent -p kinewright-project --lib --tests \
    --target x86_64-pc-windows-msvc > "$report/windows-full.log" 2>&1
full_status=$?
bash docs/reports/ja1-s4/c2-windows-check.sh > "$report/windows-native-contract.log" 2>&1
native_status=$?
printf 'full_crates_exit=%s\nnative_contract_exit=%s\n' "$full_status" "$native_status" > "$report/windows-status.txt"
cat "$report/windows-status.txt"
exit "$full_status"
