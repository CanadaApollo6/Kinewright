#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
exec 9>/tmp/kinewright-cargo-heavy.lock
flock 9
report=${C2_REPORT_DIR:-docs/reports/ja1-s4}
snapshot() {
    date --utc --iso-8601=ns
    uptime
    nproc
    ps -eo pid,ppid,pcpu,comm --sort=-pcpu | head -n 6 || true
    nvidia-smi --query-gpu=pstate,clocks.current.graphics,clocks.current.memory --format=csv,noheader 2>/dev/null || true
}
timed_fixture="$PWD/target/c2-native-timed"
nice -n 19 rustc --edition 2024 crates/kinewright-agent/tests/fixtures/ja_s4_native.rs -o "$timed_fixture"
snapshot > "$report/c2-timing-context.log"
runner=${C2_TIMING_RUNNER:?set the exact freshly built slow-feature test executable}
[[ -x "$runner" && -f "$runner" ]]
matches=0
for candidate in target/debug/deps/ja_s4_guardian-*; do
    if [[ "$candidate" -ef "$runner" ]]; then matches=$((matches + 1)); fi
done
[[ "$matches" -eq 1 ]]
sha256sum "$runner" target/debug/kinewright-job-guardian "$timed_fixture" crates/kinewright-agent/tests/fixtures/ja_s4_native.rs > "$report/c2-timing-identities.sha256"
setsid nohup timeout 60s env KINEWRIGHT_S4_PREBUILT_NATIVE="$timed_fixture" nice -n 19 "$runner" --test-threads=1 --nocapture 9>&- > "$report/c2-timing.log" 2>&1 &
lane_pid=$!
(
    while kill -0 "$lane_pid" 2>/dev/null; do
        date --utc --iso-8601=ns
        ps -eo pid,ppid,pcpu,comm --sort=-pcpu | awk 'NR==1 || $3>20'
        sleep 5
    done
) > "$report/c2-contamination.log" &
sampler_pid=$!
status=0
wait "$lane_pid" || status=$?
kill "$sampler_pid" 2>/dev/null || true
wait "$sampler_pid" 2>/dev/null || true
snapshot >> "$report/c2-timing-context.log"
[[ "$status" -eq 0 ]]
! rg -q C2_NATIVE_NOT_RUN "$report/c2-timing.log"
printf '%s\n' 'C2 deadline lane complete'
