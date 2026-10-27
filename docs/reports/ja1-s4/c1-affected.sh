#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
bash "$runner" test -p kinewright-project --test ja_s4_store > "$report/c1-store.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s4_kernel > "$report/c1-kernel.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s1_receipts -- --test-threads=1 > "$report/c1-retained-receipts.log" 2>&1
bash "$runner" test -p kinewright-project --features test-util --test ja_s2_writer -- --test-threads=1 > "$report/c1-retained-writer.log" 2>&1
bash "$runner" test -p kinewright-agent --test ja_s1_branches -- --test-threads=1 > "$report/c1-retained-branches.log" 2>&1
bash "$runner" test -p kinewright-agent --lib confirmation -- --test-threads=1 > "$report/c1-confirmation.log" 2>&1
bash "$runner" test -p kinewright-agent --lib branch::tests::merge_is_atomic_and_rejects_a_stale_base -- --exact > "$report/c1-m31.log" 2>&1
bash "$runner" test -p kinewright-agent --lib session::tests::in2_the_agent_crate_has_one_agent_event_loop -- --exact > "$report/c1-agent-architecture.log" 2>&1
bash "$runner" test -p kinewright-app investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --exact > "$report/c1-app-architecture.log" 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > "$report/c1-manifest.log" 2>&1
