source scripts/setup-ffmpeg.sh
# The fixture uses CARGO_TARGET_TMPDIR for its durable namespace.
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo test -p kinewright-project --features test-util --test ja_s2_save_cost -- --test-threads=1
