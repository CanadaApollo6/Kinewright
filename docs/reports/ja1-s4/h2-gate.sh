#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
bash "$runner" build --workspace > "$report/h2-build.log" 2>&1
bash "$runner" clippy --workspace --all-targets -- -D warnings > "$report/h2-clippy.log" 2>&1
bash "$runner" test -p kinewright-core --lib attachment::tests -- --test-threads=1 > "$report/h2-core-attachment.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s4_candidate -- --test-threads=1 > "$report/h2-candidate.log" 2>&1
bash "$runner" test -p kinewright-project --test ja_s4_store > "$report/h2-store.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s4_kernel > "$report/h2-kernel.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s1_receipts -- --test-threads=1 > "$report/h2-receipts.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s2_writer -- --test-threads=1 > "$report/h2-writer.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s1_branches -- --test-threads=1 > "$report/h2-branches.log" 2>&1
bash "$runner" test -p kinewright-agent --lib confirmation -- --test-threads=1 > "$report/h2-confirmation.log" 2>&1
bash "$runner" test -p kinewright-agent --lib branch::tests::merge_is_atomic_and_rejects_a_stale_base -- --exact > "$report/h2-m31.log" 2>&1
bash "$runner" test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact > "$report/h2-agent-architecture.log" 2>&1
bash "$runner" test -p kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact > "$report/h2-app-architecture.log" 2>&1
rustfmt --edition 2024 --check --config skip_children=true crates/kinewright-core/src/actor.rs crates/kinewright-core/src/attachment.rs crates/kinewright-project/src/history_save.rs crates/kinewright-project/src/bundle_commit.rs crates/kinewright-project/src/editing_session.rs crates/kinewright-project/src/editing_session/durable.rs crates/kinewright-project/src/orchestration_records.rs crates/kinewright-project/tests/ja_s4_store.rs crates/kinewright-project/tests/ja_s4_candidate.rs crates/kinewright-agent/src/orchestration/mod.rs > "$report/h2-fmt.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/h2-manifest.log" 2>&1
git diff --check > "$report/h2-diff-check.log" 2>&1
