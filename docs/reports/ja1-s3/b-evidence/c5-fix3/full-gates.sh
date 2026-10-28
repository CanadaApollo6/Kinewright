#!/usr/bin/env bash
set -uo pipefail
cd /home/riels/.t3/worktrees/Kinewright/t3-0fe63804
gate=docs/reports/ja1-s3/b-evidence/c5-fix3/gate.sh
out=c5-fix3
# Independent commands continue after a failure, so no earlier binary hides a later crate.
bash "$gate" "$out" app test -p kinewright-app -- --nocapture
bash "$gate" "$out" project test -p kinewright-project -- --nocapture
bash "$gate" "$out" project-test-util test -p kinewright-project --features test-util -- --nocapture
bash "$gate" "$out" media test -p kinewright-media --lib -- --test-threads=1 --nocapture
bash "$gate" "$out" build build --workspace
bash "$gate" "$out" fmt fmt -- --check
bash "$gate" "$out" windows check -p kinewright-project -p kinewright-app -p kinewright-media --target x86_64-pc-windows-msvc
