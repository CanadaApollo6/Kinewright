#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
evidence="${1:?absolute evidence directory}"
mkdir -p "$evidence"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
for mutation in routing hook; do
    if [[ "$mutation" == routing ]]; then
        task_file=crates/kinewright-agent/src/ja_harness/controller.rs
        witness=revised_mode_reaches_main_and_native_workers
    else
        task_file=crates/kinewright-project/src/project.rs
        witness=paired_io_baseline_has_successful_refusal_controls_and_literal_reds
    fi
    backup="$evidence/$mutation.before.rs"
    cp "$task_file" "$backup"
    trap 'cp "$backup" "$task_file"' EXIT
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do sleep 300; done
    nice -n 19 python3 -I - "$task_file" "$mutation" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]);s=p.read_text()
if sys.argv[2]=='routing':
    a='"revised" => protocol::Mode::Revised,'
    assert s.count(a)==1
    s=s.replace(a,'"revised" => protocol::Mode::Baseline,',1)
else:
    a='''            crate::io_observer::note_bytes("rename_refusal_hook", target, Some(&attempted));
            crate::io_observer::note("rename_refusal_injected", target);'''
    assert s.count(a)==1
    s=s.replace(a,'            let _ = (target, attempted);',1)
p.write_text(s)
PY
    log="$evidence/$mutation-red.log"
    { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$task_file"; } > "$log"
    if nice -n 10 cargo test -p kinewright-agent --features eval-harness --test ja_s1 "$witness" -- --exact >> "$log" 2>&1; then echo "mutation survived: $mutation"; exit 1; fi
    rg '^test result: FAILED\. 0 passed; 1 failed;' "$log"
    gzip "$log"
    cp "$backup" "$task_file"
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do sleep 300; done
    log="$evidence/$mutation-restored.log"
    { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$task_file"; } > "$log"
    nice -n 10 cargo test -p kinewright-agent --features eval-harness --test ja_s1 "$witness" -- --exact >> "$log" 2>&1
    rg '^test result: ok\. 1 passed; 0 failed;' "$log"
    gzip "$log"
    rm "$backup"
    trap - EXIT
done
