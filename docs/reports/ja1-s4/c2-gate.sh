#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4
run_cargo() {
    local label=$1
    shift
    test "$(free -g | awk '/^Mem:/{print $7}')" -ge 6
    printf '%s\n' "$label"
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo "$@" > "$report/c2-$label.log" 2>&1
}
run_cargo build build --workspace
run_cargo clippy clippy --workspace --all-targets -- -D warnings
run_cargo windows-clippy clippy -p kinewright-job-sys --all-targets --target x86_64-pc-windows-msvc -- -D warnings
run_cargo fmt fmt -- --check --config skip_children=true
rustfmt --edition 2024 --config skip_children=true --check \
    crates/kinewright-job-sys/src/{lib,linux,windows}.rs \
    crates/kinewright-agent/src/lib.rs \
    crates/kinewright-agent/src/bin/kinewright-job-guardian.rs \
    crates/kinewright-agent/src/job_guardian/{mod,protocol,service,linux,windows}.rs \
    crates/kinewright-agent/src/orchestration/{guardian,execution_claim}.rs \
    crates/kinewright-agent/tests/ja_s4_guardian.rs \
    crates/kinewright-agent/tests/fixtures/ja_s4_native.rs > "$report/c2-rustfmt.log" 2>&1
run_cargo witnesses test -p kinewright-agent --test ja_s4_guardian -- --test-threads=1 --nocapture
if rg -q C2_NATIVE_NOT_RUN "$report/c2-witnesses.log"; then exit 2; fi
run_cargo queued-deadlines test -p kinewright-agent --lib queued_ -- --test-threads=1
run_cargo agent-architecture test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact
run_cargo app-architecture test -p kinewright-app --bin kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact
run_cargo m31 test -p kinewright-agent --lib server::tests::edit_plan_applies_atomically_and_undoes_once -- --exact
run_cargo confirmation test -p kinewright-agent --lib confirmation_ -- --test-threads=1
run_cargo s2-authority test -p kinewright-agent --test ja_s2_host auth_and_project_protocol_guard_every_read_and_command -- --exact --test-threads=1
run_cargo s2-editor test -p kinewright-agent --test ja_s2_host g05l_two_clients_one_editor_ordered_history -- --exact --test-threads=1
run_cargo s2-lost-reply test -p kinewright-agent --test ja_s2_host g05l_fast_restart_absent_vs_committed_lost_reply_suffix -- --exact --test-threads=1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/c2-slow-lint.log" 2>&1
ldd target/debug/kinewright-job-guardian > "$report/c2-guardian-linkage.log"
sha256sum target/debug/kinewright-job-guardian > "$report/c2-binary.sha256"
git diff --check > "$report/c2-diff-check.log"
printf '%s\n' 'C2 gate complete'
