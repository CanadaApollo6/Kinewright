#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
mkdir -p target
nice -n 19 python3 -I - <<'PY'
from pathlib import Path
source = Path('scripts/ja-s1-predecessor-controls.sh').read_text()
source = source.replace(
    '# Build a sealed max-2 predecessor in an isolated detached checkout, before tests.',
    '# FU1 uses the existing sealed-predecessor recipe with the required Cargo wrapper.',
)
source = source.replace('export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4', '''export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
fu1_cargo() {
    local available_kib
    available_kib=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
    if (( available_kib < 6 * 1024 * 1024 )); then
        echo 'FU1 predecessor blocked: less than 6 GiB available' >&2
        return 75
    fi
    source scripts/setup-ffmpeg.sh >&2
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
}''')
assert source.count('nice -n 10 cargo build') == 1
assert source.count('nice -n 10 cargo test') == 1
source = source.replace('nice -n 10 cargo build', 'fu1_cargo build')
source = source.replace('nice -n 10 cargo test', 'fu1_cargo test')
source = source.replace('"$sealed/max2-control" --exact pinned_max2_format_controls',
                        '"$sealed/max2-control" --exact pinned_max2_format_controls --test-threads=1')
Path('target/fu1-predecessor-run.sh').write_text(source)
PY
exec bash target/fu1-predecessor-run.sh "$@"
