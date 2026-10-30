#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4
evidence=docs/reports/ja1-s3/evidence
mkdir -p "$evidence"
cargo_locked() {
    local available
    available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
    if (( available < 6 * 1024 * 1024 )); then
        echo 'C0 gate requires at least 6 GiB available memory.' >&2
        return 75
    fi
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
}
record() {
    local label=$1
    shift
    local result=0
    "$@" > "$evidence/$label.log" 2>&1 || result=$?
    tail -n 8 "$evidence/$label.log"
    gzip -f "$evidence/$label.log"
    return "$result"
}
case "${1:-all}" in
    build) record build cargo_locked build --workspace ;;
    clippy) record clippy cargo_locked clippy --workspace --all-targets -- -D warnings ;;
    media) record media cargo_locked test -p kinewright-media owned_io -- --test-threads=1 ;;
    project) record project cargo_locked test -p kinewright-project --features test-util s3_ -- --test-threads=1 --nocapture ;;
    host) record host cargo_locked test -p kinewright-agent writer_host::s2h_tests::s3_ -- --test-threads=1 ;;
    app) record app cargo_locked test -p kinewright-app s3_owned_ -- --test-threads=1 ;;
    scans)
        record agent-architecture cargo_locked test -p kinewright-agent in2_the_agent_crate_has_one_agent_event_loop -- --test-threads=1
        record app-architecture cargo_locked test -p kinewright-app in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --test-threads=1
        ;;
    lint) record slow-lint nice -n 19 python3 -I scripts/slow_tests.py lint ;;
    fmt)
        mapfile -t files < <({ git diff --name-only; git ls-files --others --exclude-standard; } | rg '\.rs$' | sort -u)
        record fmt cargo_locked fmt -- --check --config skip_children=true "${files[@]}"
        ;;
    all)
        for gate in build clippy media project host app scans lint fmt; do
            bash "$0" "$gate"
        done
        ;;
    *) echo 'Unknown C0 gate.' >&2; exit 2 ;;
esac
