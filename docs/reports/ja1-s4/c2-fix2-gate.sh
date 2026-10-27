#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4/c2-fix2
mkdir -p "$report"
run_cargo() {
    local label=$1
    shift
    test "$(free -g | awk '/^Mem:/{print $7}')" -ge 6
    printf '%s\n' "$label"
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo "$@" > "$report/$label.log" 2>&1
}
run_cargo build build --workspace
run_cargo clippy clippy --workspace --all-targets -- -D warnings
bash docs/reports/ja1-s4/c2-windows-check.sh > "$report/windows-cross.log" 2>&1
run_cargo fmt fmt -- --check --config skip_children=true
rustfmt --edition 2024 --config skip_children=true --check \
    crates/kinewright-job-sys/src/namespace.rs \
    crates/kinewright-agent/src/orchestration/guardian.rs \
    crates/kinewright-agent/src/job_guardian/linux.rs \
    crates/kinewright-agent/tests/{ja_s4_guardian.rs,fixtures/ja_s4_native.rs} > "$report/rustfmt.log" 2>&1
run_cargo job-sys test -p kinewright-job-sys
run_cargo witnesses test -p kinewright-agent --test ja_s4_guardian --features slow-tests -- --test-threads=1 --show-output
! rg -q C2_NATIVE_NOT_RUN "$report/witnesses.log"
(
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$PWD/target/c2-no-session-bus"
    export XDG_RUNTIME_DIR="$PWD/target/c2-no-runtime-dir"
    run_cargo skip-capture test -p kinewright-agent --test ja_s4_guardian --features slow-tests s4_c2_proc_shows_only_the_namespace -- --exact --test-threads=1 --show-output
)
run_cargo orchestration test -p kinewright-agent --lib orchestration --features slow-tests -- --test-threads=1
run_cargo guardian-unit test -p kinewright-agent --lib guardian:: --features slow-tests -- --test-threads=1
run_cargo orchestration-fixtures test -p kinewright-agent --test ja_s4_kernel --features slow-tests -- --test-threads=1
run_cargo profile-token test -p kinewright-agent --doc guardian::CertifiedProfile
run_cargo agent-architecture test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact
run_cargo app-architecture test -p kinewright-app --bin kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/slow-lint.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests_selftest.py > "$report/selftest.log" 2>&1
nice -n 19 python3 -I docs/reports/ja1-s4/c2-fix1-slow-check.py > "$report/slow-parser.log" 2>&1
nice -n 19 python3 -I docs/reports/ja1-s4/c2-fix2-slow-check.py > "$report/skip-accounting.log" 2>&1
git diff --check > "$report/diff-check.log"
sha256sum target/debug/kinewright-job-guardian > "$report/guardian.sha256"
printf '%s\n' 'C2 fix round 2 gate complete'
