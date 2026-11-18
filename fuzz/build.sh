#!/usr/bin/env bash
# Build the libFuzzer targets the way cargo-fuzz does, without installing it:
# pinned nightly, SanitizerCoverage flags, AddressSanitizer, debug assertions.
# Run from the repository root; never wrap in another flock.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/target/fuzz"
export RUSTFLAGS="-Cpasses=sancov-module \
-Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
-Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares \
-Cllvm-args=-sanitizer-coverage-stack-depth \
--cfg fuzzing -Clink-dead-code -Zsanitizer=address \
-Cdebug-assertions -Coverflow-checks -Cforce-frame-pointers"
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 \
    cargo +nightly-2026-08-21 build --offline --release \
    --manifest-path fuzz/Cargo.toml --target x86_64-unknown-linux-gnu --bins
