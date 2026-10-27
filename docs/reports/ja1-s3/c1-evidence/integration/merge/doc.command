set -e
source scripts/setup-ffmpeg.sh
export TMPDIR="$PWD/target/c1-integration-runtime/doc-tmp"
mkdir -p "$TMPDIR"
test "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -ge 6291456
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 timeout --kill-after=5s 600s nice -n 10 cargo test -p kinewright-project -p kinewright-agent --features kinewright-project/test-util --doc
