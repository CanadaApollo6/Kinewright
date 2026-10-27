#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
report=docs/reports/ja1-s4
run_cargo() {
    local label=$1
    shift
    awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo "$@" > "$report/c5-$label.log" 2>&1
    tail -n 2 "$report/c5-$label.log"
}
run_cargo build build --workspace
run_cargo clippy clippy --workspace --all-targets -- -D warnings
run_cargo fmt fmt -- --check --config skip_children=true
rustfmt --edition 2024 --config skip_children=true --check \
    crates/kinewright-project/src/lib.rs \
    crates/kinewright-project/src/orchestration_records.rs \
    crates/kinewright-project/src/orchestration_commit.rs \
    crates/kinewright-project/src/orchestration_accounting.rs \
    crates/kinewright-agent/src/acp.rs \
    crates/kinewright-agent/src/drivers.rs \
    crates/kinewright-agent/src/protocol.rs \
    crates/kinewright-agent/src/muse.rs \
    crates/kinewright-agent/src/orchestration/usage.rs \
    crates/kinewright-agent/src/orchestration/limits.rs \
    crates/kinewright-agent/tests/ja_s4_usage.rs \
    crates/kinewright-agent/tests/ja_s4_admission.rs \
    crates/kinewright-agent/tests/fixtures/ja_s4_native.rs > "$report/c5-rustfmt.log" 2>&1
run_cargo focused test -p kinewright-agent --test ja_s4_usage --test ja_s4_admission --test ja_s4_guardian --test ja_s4_kernel -- --test-threads=1 --nocapture
if rg -q C2_NATIVE_NOT_RUN "$report/c5-focused.log"; then exit 2; fi
run_cargo project-contracts test -p kinewright-project --features test-util --test ja_s4_store --test ja_s1_storage -- --test-threads=1
run_cargo agent-unit-build test -p kinewright-agent --lib --no-run --message-format=json
nice -n 19 python3 -I - "$report" <<'PYSCRIPT'
import json,sys
from pathlib import Path
report=Path(sys.argv[1]);artifacts=[]
for line in (report/"c5-agent-unit-build.log").read_text().splitlines():
    try: row=json.loads(line)
    except ValueError: continue
    if row.get("reason")=="compiler-artifact" and row.get("executable") and row.get("profile",{}).get("test"):
        artifacts.append(row["executable"])
assert len(artifacts)==1,artifacts
(report/"c5-agent-test-binary.txt").write_text(artifacts[0]+"\n")
PYSCRIPT
read -r agent_test < "$report/c5-agent-test-binary.txt"
awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo
flock -o /tmp/kinewright-cargo-heavy.lock env RUST_TEST_THREADS=4 nice -n 19 "$agent_test" --test-threads=1 \
    acp::tests:: acp_session::tests:: cursor::tests:: protocol::tests:: drivers::tests:: muse::tests:: \
    session::tests::in2_the_agent_crate_has_one_agent_event_loop \
    server::tests::edit_plan_applies_atomically_and_undoes_once confirmation_ > "$report/c5-agent-unit.log" 2>&1
run_cargo app-architecture test -p kinewright-app --bin kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact
run_cargo authority test -p kinewright-agent --test ja_s2_host auth_and_project_protocol_guard_every_read_and_command -- --exact --test-threads=1
run_cargo lost-reply test -p kinewright-agent --test ja_s2_host g05l_fast_restart_absent_vs_committed_lost_reply_suffix -- --exact --test-threads=1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/c5-slow-lint.log" 2>&1
git diff --check > "$report/c5-diff-check.log"
printf '%s\n' 'C5 gate complete'
