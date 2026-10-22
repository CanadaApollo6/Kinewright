#!/usr/bin/env bash
# A separate resolver graph has no project dev-dependencies to mask feature wiring.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
probe="$repo/target/ja-s1-r3-feature-consumer"
mkdir -p "$probe/src"
cat > "$probe/Cargo.toml" <<EOF
[package]
name = "ja-s1-feature-consumer"
version = "0.0.0"
edition = "2024"
[workspace]
resolver = "3"
[dependencies]
kinewright-project = { path = "$repo/crates/kinewright-project", features = ["test-util"] }
EOF
echo 'fn main() {}' > "$probe/src/main.rs"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 CARGO_TARGET_DIR="$repo/target"
start=$SECONDS
while [[ "$(free -g | awk '/Mem:/ {print $7}')" -lt 6 ]]; do
    (( SECONDS - start <= 7200 )) || exit 1
    free -g
    sleep 30
done
free -g
nice -n 10 cargo check --offline --manifest-path "$probe/Cargo.toml"
