#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
bash "$runner" build --workspace > "$report/c4-build.log" 2>&1
bash "$runner" clippy --workspace --all-targets -- -D warnings > "$report/c4-clippy.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s4_lifecycle s4_ -- --test-threads=1 > "$report/c4-lifecycle.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s4_kernel -- --test-threads=1 > "$report/c4-kernel.log" 2>&1
bash "$runner" test -p kinewright-project --test ja_s4_store -- --test-threads=1 > "$report/c4-store.log" 2>&1
bash "$runner" test -p kinewright-agent --lib session::tests -- --test-threads=1 > "$report/c4-session.log" 2>&1
bash "$runner" test -p kinewright-agent --lib s4_ -- --test-threads=1 > "$report/c4-c3-witnesses.log" 2>&1
bash "$runner" test -p kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact > "$report/c4-app-architecture.log" 2>&1
bash "$runner" test -p kinewright-agent --lib branch::tests::merge_is_atomic_and_rejects_a_stale_base -- --exact > "$report/c4-m31.log" 2>&1
bash "$runner" test -p kinewright-agent --lib confirmation -- --test-threads=1 > "$report/c4-confirmation.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s2_host auth_and_project_protocol_guard_every_read_and_command -- --exact --test-threads=1 > "$report/c4-s2-authority.log" 2>&1
rustfmt --edition 2024 --check --config skip_children=true crates/kinewright-core/src/{agent,lib}.rs crates/kinewright-project/src/orchestration_{records,commit}.rs crates/kinewright-project/tests/ja_s4_store.rs crates/kinewright-agent/src/{lib,session,acp_session}.rs crates/kinewright-agent/src/orchestration/{mod,machine,tasks,mailbox,requests,context,outbox}.rs crates/kinewright-agent/tests/ja_s4_{kernel,lifecycle}.rs > "$report/c4-fmt.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/c4-manifest.log" 2>&1
git diff --check > "$report/c4-diff-check.log" 2>&1
