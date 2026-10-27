#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4/c5-fix1
mkdir -p "$report"
bash docs/reports/ja1-s4/c5-fix1-predecessor.sh > "$report/predecessor-prepare.log" 2>&1
run_cargo() {
    local label=$1
    shift
    awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo "$@" > "$report/$label.log" 2>&1
    tail -n 2 "$report/$label.log"
}
run_cargo build build --workspace
run_cargo clippy clippy --workspace --all-targets -- -D warnings
run_cargo fmt fmt -- --check --config skip_children=true
rustfmt --edition 2024 --config skip_children=true --check \
    crates/kinewright-project/src/orchestration_accounting.rs \
    crates/kinewright-project/src/orchestration_records.rs \
    crates/kinewright-project/src/orchestration_commit.rs \
    crates/kinewright-agent/src/acp.rs \
    crates/kinewright-agent/src/muse.rs \
    crates/kinewright-agent/src/orchestration/usage.rs \
    crates/kinewright-agent/tests/ja_s4_usage.rs \
    crates/kinewright-agent/tests/fixtures/ja_s4_c5_pipe.rs > "$report/rustfmt.log" 2>&1
run_cargo full-crates test -p kinewright-agent -p kinewright-project \
    --features kinewright-agent/slow-tests,kinewright-project/test-util \
    --lib --tests --no-fail-fast -- --test-threads=1
if rg -q C2_NATIVE_NOT_RUN "$report/full-crates.log"; then exit 2; fi
run_cargo app-architecture test -p kinewright-app --bin kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact --test-threads=1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/slow-lint.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests_selftest.py > "$report/slow-selftest.log" 2>&1
git diff --check > "$report/diff-check.log"
printf '%s\n' 'C5 fix round 1 Linux gate complete'
