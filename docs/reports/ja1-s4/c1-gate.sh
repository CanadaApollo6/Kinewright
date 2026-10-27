#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
runner=docs/reports/ja1-s4/c1-cargo.sh
report=docs/reports/ja1-s4
bash "$runner" build --workspace > "$report/c1-build.log" 2>&1
bash "$runner" clippy --workspace --all-targets -- -D warnings > "$report/c1-clippy.log" 2>&1
bash "$runner" fmt -- --check > "$report/c1-fmt.log" 2>&1
rustfmt --edition 2024 --check --config skip_children=true \
    crates/kinewright-agent/src/lib.rs \
    crates/kinewright-agent/src/orchestration/mod.rs \
    crates/kinewright-agent/src/orchestration/machine.rs \
    crates/kinewright-agent/src/orchestration/outbox.rs \
    crates/kinewright-agent/tests/ja_s4_kernel.rs \
    crates/kinewright-project/src/orchestration_records.rs \
    crates/kinewright-project/src/orchestration_commit.rs \
    crates/kinewright-project/tests/ja_s4_store.rs > "$report/c1-touched-fmt.log" 2>&1
bash "$report/c1-affected.sh"
git diff --check > "$report/c1-diff-check.log" 2>&1
