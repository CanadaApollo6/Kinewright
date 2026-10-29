#!/usr/bin/env bash
set -eu
cd /home/riels/.t3/worktrees/Kinewright/t3-1c0887c5
export PATH="$PWD/target/ja1-s3-c6a/speech-tools/bin:$PATH"
export KINEWRIGHT_S3_VOICE_MODEL_DATA_DIR="$PWD/target/ja1-s3-c6a/model-data"
target/ja1-s3-c6a/cargo.sh build --workspace > target/ja1-s3-c6a/gate2-build-final.log 2>&1
target/ja1-s3-c6a/cargo.sh clippy --workspace --all-targets -- -D warnings > target/ja1-s3-c6a/gate2-clippy-final.log 2>&1
target/ja1-s3-c6a/cargo.sh fmt -- --check > target/ja1-s3-c6a/gate2-fmt-final.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-app --features slow-tests --no-fail-fast > target/ja1-s3-c6a/gate2-app-frozen.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-app --features slow-tests s3_real_ -- --nocapture > target/ja1-s3-c6a/model-equipped-controls.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-app --features slow-tests s3_frozen_selection_survives_background_copy_and_reopen -- --nocapture > target/ja1-s3-c6a/final-selection-fixtures.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-project --features test-util s3_ -- --nocapture > target/ja1-s3-c6a/final-project-s3.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-agent --lib writer_host:: -- --nocapture > target/ja1-s3-c6a/final-agent-host.log 2>&1
nice -n 19 python3 -I scripts/slow_tests.py lint > target/ja1-s3-c6a/gate2-lint.log 2>&1
nice -n 19 python3 -I scripts/slow_tests_selftest.py > target/ja1-s3-c6a/gate2-lint-selftest.log 2>&1
target/ja1-s3-c6a/cargo.sh check --target x86_64-pc-windows-msvc -p kinewright-app -p kinewright-media > target/ja1-s3-c6a/gate2-windows.log 2>&1
