#!/usr/bin/env bash
# Native shipping feature matrix. Invoke without an outer heavy-cargo lock.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/setup-ffmpeg.sh
exec flock -o /tmp/kinewright-cargo-heavy.lock \
    env CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 nice -n 10 bash -euc '
        cargo build -j4 -p kinewright-agent --no-default-features --lib
        cargo build -j4 -p kinewright-app --no-default-features --bin kinewright-app
        cargo build -j4 -p kinewright-agent --no-default-features --bin kinewright
    '
