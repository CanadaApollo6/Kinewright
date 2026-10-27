#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
repo=$PWD
scratch="$repo/target/ja-s1-predecessor"
mkdir -p "$scratch"
git cat-file -e 076a96c922b2f05984df4bb839ad7cf0900e6183
if [[ ! -d "$scratch/source/.git" ]]; then
    git clone --shared --no-checkout "$repo" "$scratch/source"
    git -C "$scratch/source" checkout --detach 076a96c922b2f05984df4bb839ad7cf0900e6183
fi
nice -n 19 python3 -I - "$repo" "$scratch" <<'PY'
from pathlib import Path
import sys
repo, scratch = map(Path, sys.argv[1:])
text = (repo / 'scripts/ja-s1-predecessor-controls.sh').read_text()
old = '        nice -n 10 cargo '
assert text.count(old) == 2
text = text.replace(old, "        awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo\n        flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo ")
text = text.replace('        nice -n 10 "$sealed/max2-control"', '        flock -o /tmp/kinewright-cargo-heavy.lock env RUST_TEST_THREADS=4 nice -n 19 "$sealed/max2-control"')
text = text.replace('            bash "$repo/scripts/ja-s1-profile-control.sh"', '            flock -o /tmp/kinewright-cargo-heavy.lock nice -n 19 bash "$repo/scripts/ja-s1-profile-control.sh"')
(scratch / 'prepare.sh').write_text(text)
PY
bash "$scratch/prepare.sh"
