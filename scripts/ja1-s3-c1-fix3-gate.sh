#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3() { command nice -n 19 python3 -I "$@"; }
export -f python3
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4
c1fix_runtime="$PWD/target/c1fix3-runtime"
export LOCALAPPDATA="$PWD/target/c1fix-localappdata"
export TMPDIR="$c1fix_runtime/tmp"
export XDG_STATE_HOME="$c1fix_runtime/state" XDG_DATA_HOME="$c1fix_runtime/share"
export XDG_CACHE_HOME="$c1fix_runtime/cache" XDG_CONFIG_HOME="$c1fix_runtime/config"
mkdir -p "$LOCALAPPDATA/Kinewright/recovery" "$TMPDIR" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"
c1fix_evidence=docs/reports/ja1-s3/c1-evidence/fix3
mkdir -p "$c1fix_evidence"
cargo_locked() {
    if ! test "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -ge 6291456; then
        echo 'C1 fix 3 requires at least 6 GiB available memory.' >&2
        return 75
    fi
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
}
record() {
    local label=$1 result=0
    shift
    printf '%q ' "$@" > "$c1fix_evidence/$label.command"
    printf '\n' >> "$c1fix_evidence/$label.command"
    "$@" > "$c1fix_evidence/$label.log" 2>&1 || result=$?
    printf '%s\n' "$result" > "$c1fix_evidence/$label.exit"
    tail -n 12 "$c1fix_evidence/$label.log"
    gzip -f "$c1fix_evidence/$label.log"
    return "$result"
}
case "${1:-all}" in
    build) record build cargo_locked build --workspace ;;
    clippy) record clippy cargo_locked clippy --workspace --all-targets -- -D warnings ;;
    fmt) record fmt cargo_locked fmt -- --check ;;
    integration) record integration cargo_locked test -p kinewright-agent --test ja_s2_host -- --test-threads=1 ;;
    focused) record focused cargo_locked test -p kinewright-agent --lib writer_host::ja_s3_host::s3_fix3_ -- --test-threads=1 ;;
    host) record host cargo_locked test -p kinewright-agent --lib writer_host:: -- --test-threads=1 ;;
    app) record app cargo_locked test -p kinewright-app --bin kinewright-app ja1_attachment::tests:: -- --test-threads=1 ;;
    scans)
        record agent-architecture cargo_locked test -p kinewright-agent --lib in2_the_agent_crate_has_one_agent_event_loop -- --test-threads=1
        record app-architecture cargo_locked test -p kinewright-app --bin kinewright-app in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --test-threads=1
        record source-guard cargo_locked test -p kinewright-media --lib au6_fixtures::au6_declared_test_names_exist_in_their_source_files -- --exact --test-threads=1
        ;;
    lint) record slow-lint nice -n 19 python3 -I scripts/slow_tests.py lint ;;
    windows) record windows cargo_locked check -p kinewright-agent -p kinewright-app --target x86_64-pc-windows-msvc ;;
    all)
        for c1fix_gate in build clippy fmt focused host integration app scans lint windows; do
            bash "$0" "$c1fix_gate"
        done
        ;;
    *) echo 'Unknown C1 fix 3 gate.' >&2; exit 2 ;;
esac
