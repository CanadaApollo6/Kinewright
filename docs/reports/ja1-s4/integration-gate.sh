#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
report=docs/reports/ja1-s4/integration
mkdir -p "$report"
status=0
run() {
    local label=$1 code=0
    shift
    "$@" > "$report/$label.log" 2>&1 || code=$?
    printf '%s=%s\n' "$label" "$code" | tee -a "$report/gate-status.txt"
    if [[ "$code" -ne 0 ]]; then status=1; fi
}
: > "$report/gate-status.txt"
cargo_route=docs/reports/ja1-s4/integration-cargo.sh
if [[ ! -f target/ja-s1-predecessor/sealed/identity.json ]]; then
    run predecessor-setup bash docs/reports/ja1-s4/c5-fix1-predecessor.sh
fi
run build bash "$cargo_route" build --workspace
run clippy bash "$cargo_route" clippy --workspace --all-targets -- -D warnings
run fmt bash "$cargo_route" fmt -- --check
run windows-native bash docs/reports/ja1-s4/c2-windows-check.sh
run windows-project bash docs/reports/ja1-s4/fu3-windows-check.sh
run job-sys bash "$cargo_route" test -p kinewright-job-sys --lib --tests -- --test-threads=4 --nocapture
run agent bash "$cargo_route" test -p kinewright-agent --lib --tests --features slow-tests --no-fail-fast -- --test-threads=4 --nocapture
run project bash "$cargo_route" test -p kinewright-project --features test-util --lib --tests --no-fail-fast -- --test-threads=4 --nocapture
run app-attachment bash "$cargo_route" test -p kinewright-app --bin kinewright-app --features slow-tests ja1_attachment -- --test-threads=1 --nocapture
run slow-lint nice -n 19 python3 -I scripts/slow_tests.py lint
run slow-selftest nice -n 19 python3 -I scripts/slow_tests_selftest.py
if rg -q C2_NATIVE_NOT_RUN "$report/agent.log"; then
    printf '%s\n' 'native-accounting=NOT RUN' | tee -a "$report/gate-status.txt"
    status=1
fi
exit "$status"
