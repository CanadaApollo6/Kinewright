#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
run() {
    local lane=$1
    shift
    if bash "$runner" "$@" > "$report/fu1-$lane.log" 2>&1; then
        echo "$lane: PASS"
    else
        local status=$?
        tail -60 "$report/fu1-$lane.log"
        echo "$lane: FAIL ($status)"
        return "$status"
    fi
}
case "${1:-all}" in
    all)
        if [[ ! -f target/ja-s1-predecessor/sealed/identity.json ]]; then
            bash "$report/fu1-predecessor.sh" > "$report/fu1-predecessor.log" 2>&1
        fi
        run build build --workspace
        run clippy clippy --workspace --all-targets -- -D warnings
        run project test -p kinewright-project --features test-util --no-fail-fast -- --test-threads=1
        ;;
    remaining)
        run project-lib test -p kinewright-project --features test-util --lib -- --test-threads=1
        ;;
    finalize) ;;
    *) echo 'usage: fu1-gate.sh [all|remaining|finalize]' >&2; exit 2 ;;
esac
if [[ "${1:-all}" != finalize ]]; then
    run agent test -p kinewright-agent --no-fail-fast -- --test-threads=1
fi
run app-architecture test -p kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact --test-threads=1
rustfmt --edition 2024 --check --config skip_children=true \
    crates/kinewright-project/src/editing_session.rs \
    crates/kinewright-project/src/editing_session/durable.rs \
    crates/kinewright-project/src/bundle_commit.rs \
    crates/kinewright-project/src/orchestration_records.rs \
    crates/kinewright-project/src/orchestration_commit.rs \
    crates/kinewright-project/tests/ja_s4_candidate.rs \
    crates/kinewright-agent/src/server.rs \
    crates/kinewright-agent/src/orchestration/tasks.rs \
    crates/kinewright-agent/src/orchestration/machine.rs \
    crates/kinewright-agent/src/orchestration/policy.rs \
    crates/kinewright-agent/src/orchestration/server_tests.rs \
    crates/kinewright-agent/tests/ja_s4_lifecycle.rs > "$report/fu1-fmt.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/fu1-manifest.log" 2>&1
nice -n 19 python3 -I "$report/fu1-audit.py" > "$report/fu1-audit.log" 2>&1
git diff --check > "$report/fu1-diff-check.log" 2>&1
echo 'FU1 gate: PASS'
