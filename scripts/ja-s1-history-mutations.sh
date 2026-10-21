#!/usr/bin/env bash
# Each mutation runs only the witness it is meant to discriminate.
set -euo pipefail
cd "$(dirname "$0")/.."
evidence="${1:?absolute evidence directory}"
mkdir -p "$evidence"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
rustc -Vv > "$evidence/environment.txt"
uname -a >> "$evidence/environment.txt"
task_file=crates/kinewright-core/src/actor.rs
backup="$evidence/actor.before.rs"
cp "$task_file" "$backup"
trap 'cp "$backup" "$task_file"' EXIT
for mutation in redo gesture batch corrupt identity; do
    case "$mutation" in
        redo|batch) witness=history_export_restore_preserves_cursor_redo_and_batches ;;
        gesture) witness=open_drag_restore_replays_suffix_before_close ;;
        corrupt) witness=history_restore_rejects_corruption_atomically ;;
        identity) witness=identity_effects_distinguish_reuse_and_split_undo ;;
    esac
    if [[ ! -f "$evidence/$mutation-red.log.gz" ]]; then
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do sleep 300; done
    nice -n 19 python3 -I - "$task_file" "$mutation" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]);s=p.read_text();mutation=sys.argv[2]
anchor='                    state = candidate;'
if mutation == 'redo': replacement='                    candidate.redo.clear();\n'+anchor
elif mutation == 'gesture': replacement='                    if let Some(entry) = candidate.undo.last_mut() { entry.coalesce_key = None; }\n'+anchor
elif mutation == 'batch': replacement='                    candidate.batches.pop();\n'+anchor
elif mutation == 'identity':
    anchor='identity_effects: std::mem::take(&mut state.identity_effects),'
    replacement='identity_effects: Vec::new(),'
else:
    anchor='if candidate.history() != *snapshot {'
    replacement='if false && candidate.history() != *snapshot {'
assert s.count(anchor)==1,(mutation,s.count(anchor))
p.write_text(s.replace(anchor,replacement,1))
PY
    log="$evidence/$mutation-red.log"
    { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$task_file"; } > "$log"
    if nice -n 10 cargo test -p kinewright-core --test ja_s1_history "$witness" -- --exact >> "$log" 2>&1; then
        echo "Mutation survived: $mutation"
        exit 1
    fi
    rg '^test result: FAILED\. 0 passed; 1 failed;' "$log"
    nice -n 19 python3 -I - "$log" <<'PYHASH' >> "$log"
import sys,re,hashlib
from pathlib import Path
log=Path(sys.argv[1]).read_text()
match=re.search(r'Running tests/ja_s1_history.rs \(([^)]+)\)',log)
assert match,'missing executed test binary identity'
p=Path(match.group(1))
with p.open('rb') as f: print('binary_sha256:',hashlib.file_digest(f,'sha256').hexdigest(),p)
PYHASH
    gzip "$log"
    fi
    cp "$backup" "$task_file"
    if [[ -f "$evidence/$mutation-restored.log.gz" ]]; then continue; fi
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do sleep 300; done
    log="$evidence/$mutation-restored.log"
    { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$task_file"; } > "$log"
    nice -n 10 cargo test -p kinewright-core --test ja_s1_history "$witness" -- --exact >> "$log" 2>&1
    rg '^test result: ok\. 1 passed; 0 failed;' "$log"
    nice -n 19 python3 -I - "$log" <<'PYHASH' >> "$log"
import sys,re,hashlib
from pathlib import Path
log=Path(sys.argv[1]).read_text()
match=re.search(r'Running tests/ja_s1_history.rs \(([^)]+)\)',log)
assert match,'missing executed test binary identity'
p=Path(match.group(1))
with p.open('rb') as f: print('binary_sha256:',hashlib.file_digest(f,'sha256').hexdigest(),p)
PYHASH
    gzip "$log"
done
rm "$backup"
trap - EXIT
