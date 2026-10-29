#!/usr/bin/env bash
set -eu
cd /home/riels/.t3/worktrees/Kinewright/t3-1c0887c5
target/ja1-s3-c6a/cargo.sh build --workspace > target/ja1-s3-c6a/gate1-build-final.log 2>&1
target/ja1-s3-c6a/cargo.sh clippy --workspace --all-targets -- -D warnings > target/ja1-s3-c6a/gate1-clippy-final.log 2>&1
target/ja1-s3-c6a/cargo.sh fmt -- --check > target/ja1-s3-c6a/gate1-fmt-final.log 2>&1
target/ja1-s3-c6a/cargo.sh test -p kinewright-app --no-fail-fast > target/ja1-s3-c6a/gate1-app-frozen.log 2>&1
target/ja1-s3-c6a/cargo.sh check --target x86_64-pc-windows-msvc -p kinewright-app -p kinewright-media > target/ja1-s3-c6a/gate1-windows.log 2>&1
