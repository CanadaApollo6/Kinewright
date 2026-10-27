#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
bash "$runner" build --workspace > "$report/c3-build.log" 2>&1
bash "$runner" clippy --workspace --all-targets -- -D warnings > "$report/c3-clippy.log" 2>&1
bash "$runner" test -p kinewright-agent --lib s4_ -- --test-threads=1 > "$report/c3-witnesses.log" 2>&1
bash "$runner" test -p kinewright-agent --lib orchestration::policy::tests -- --test-threads=1 > "$report/c3-policy.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s4_kernel -- --test-threads=1 > "$report/c3-kernel.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s4_candidate -- --test-threads=1 > "$report/c3-h2.log" 2>&1
bash "$runner" test -p kinewright-project --test ja_s4_store > "$report/c3-store.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s1_receipts -- --test-threads=1 > "$report/c3-receipts.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s2_writer -- --test-threads=1 > "$report/c3-writer.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s1_branches -- --test-threads=1 > "$report/c3-branches.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s2_host auth_and_project_protocol_guard_every_read_and_command -- --exact --test-threads=1 > "$report/c3-host-authority.log" 2>&1
bash "$runner" test -p kinewright-agent --lib confirmation -- --test-threads=1 > "$report/c3-confirmation.log" 2>&1
bash "$runner" test -p kinewright-agent --lib branch::tests::merge_is_atomic_and_rejects_a_stale_base -- --exact > "$report/c3-m31.log" 2>&1
bash "$runner" test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact > "$report/c3-agent-architecture.log" 2>&1
bash "$runner" test -p kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact > "$report/c3-app-architecture.log" 2>&1
rustfmt --edition 2024 --check --config skip_children=true crates/kinewright-agent/src/server.rs crates/kinewright-agent/src/orchestration/{mod,machine,tasks,policy,adapters,mailbox,server_tests}.rs crates/kinewright-agent/tests/ja_s4_kernel.rs > "$report/c3-fmt.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/c3-manifest.log" 2>&1
git diff --check > "$report/c3-diff-check.log" 2>&1
