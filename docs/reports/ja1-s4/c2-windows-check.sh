#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
source scripts/setup-ffmpeg.sh >/dev/null
check_root="$PWD/target/c2-windows-check"
mkdir -p "$check_root"
cat > "$check_root/Cargo.toml" <<MANIFEST
[package]
name = "kinewright-agent"
version = "0.1.0"
edition = "2024"
[workspace]
[features]
default = ["eval-harness"]
eval-harness = []
[dependencies]
kinewright-job-sys = { path = "$PWD/crates/kinewright-job-sys" }
serde = { version = "=1.0.229", features = ["derive", "rc"] }
serde_json = { version = "=1.0.151", features = ["raw_value"] }
getrandom = "=0.3.4"
[lib]
path = "native_lib.rs"
[[bin]]
name = "kinewright-job-guardian"
path = "$PWD/crates/kinewright-agent/src/bin/kinewright-job-guardian.rs"
[[test]]
name = "ja_s4_guardian"
path = "$PWD/crates/kinewright-agent/tests/ja_s4_guardian.rs"
[lints.rust]
unsafe_code = "forbid"
[lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
assert_is_empty = "allow"
MANIFEST
cat > "$check_root/native_lib.rs" <<LIBRARY
#[path = "$PWD/crates/kinewright-agent/src/job_guardian/mod.rs"]
pub mod job_guardian;
#[path = "$PWD/crates/kinewright-agent/src/orchestration/guardian.rs"]
pub mod guardian;
#[path = "$PWD/crates/kinewright-agent/src/orchestration/execution_claim.rs"]
pub mod execution_claim;
LIBRARY
cp Cargo.lock "$check_root/Cargo.lock"
test "$(free -g | awk '/^Mem:/{print $7}')" -ge 6
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo clippy --manifest-path "$check_root/Cargo.toml" --all-targets --target x86_64-pc-windows-msvc -- -D warnings
