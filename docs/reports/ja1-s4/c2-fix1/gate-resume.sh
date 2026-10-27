#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4/c2-fix1
mkdir -p "$report"
run_cargo() {
    local label=$1
    shift
    test "$(free -g | awk '/^Mem:/{print $7}')" -ge 6
    printf '%s\n' "$label"
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo "$@" > "$report/$label.log" 2>&1
}
run_cargo orchestration-fixtures test -p kinewright-agent --test ja_s4_kernel --features slow-tests -- --test-threads=1
run_cargo profile-token test -p kinewright-agent --doc guardian::CertifiedProfile
run_cargo queued-deadlines test -p kinewright-agent --lib queued_ -- --test-threads=1
run_cargo agent-architecture test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact
run_cargo app-architecture test -p kinewright-app --bin kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact
run_cargo m31 test -p kinewright-agent --lib server::tests::edit_plan_applies_atomically_and_undoes_once -- --exact
run_cargo confirmation test -p kinewright-agent --lib confirmation_ -- --test-threads=1
run_cargo s2-authority test -p kinewright-agent --test ja_s2_host auth_and_project_protocol_guard_every_read_and_command -- --exact --test-threads=1
run_cargo s2-editor test -p kinewright-agent --test ja_s2_host g05l_two_clients_one_editor_ordered_history -- --exact --test-threads=1
run_cargo s2-lost-reply test -p kinewright-agent --test ja_s2_host g05l_fast_restart_absent_vs_committed_lost_reply_suffix -- --exact --test-threads=1
nice -n 19 python3 -I docs/reports/ja1-s4/c2-fix1-slow-check.py > "$report/slow-parser.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/slow-lint.log" 2>&1
actionlint .github/workflows/ci.yml > "$report/actionlint.log" 2>&1
git diff --check > "$report/diff-check.log"
sha256sum target/debug/kinewright-job-guardian > "$report/guardian.sha256"
printf '%s\n' 'C2 fix round 1 gate complete'
