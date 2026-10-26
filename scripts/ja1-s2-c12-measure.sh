#!/usr/bin/env bash
# Prepare both exact-source binaries before a serialized, alternating timing window.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
mode="${1:?usage: ja1-s2-c12-measure.sh prepare|run /absolute/evidence-directory}"
base="$(realpath -m -- "${2:?evidence directory required}")"
cd "$root"
helper=(nice -n 19 python3 -I "$root/scripts/ja1-s2-c12-measure.py")
if [[ "$mode" == prepare ]]; then
    mkdir -p "$base/sealed" "$base/parent-src" "$base/parent-tmp" "$root/target/c12-tmp"
    git archive 2aa6633db61912a416948edb580e2df8a3ddd58f | tar -x -C "$base/parent-src"
    if [[ ! -e "$base/parent-src/third_party" ]]; then ln -s "$root/third_party" "$base/parent-src/third_party"; fi
    for side in parent current; do
        (
        if [[ "$side" == parent ]]; then
            cd "$base/parent-src"
            export CARGO_TARGET_DIR="$base/parent-target" TMPDIR="$base/parent-tmp"
        else
            cd "$root"
            export CARGO_TARGET_DIR="$root/target/c12" TMPDIR="$root/target/c12-tmp"
        fi
        source "$root/scripts/setup-ffmpeg.sh"
        export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
        nice -n 19 python3 -I - "$base/$side-build-environment.json" <<'NATIVE_ENV'
import json,os,sys
from pathlib import Path
keys=['FFMPEG_DIR','PKG_CONFIG_PATH','LD_LIBRARY_PATH','RUSTFLAGS','PATH','CARGO_BUILD_JOBS','RUST_TEST_THREADS']
value=dict(native={key:os.environ.get(key) for key in keys},target=os.environ['CARGO_TARGET_DIR'],tmpdir=os.environ['TMPDIR'],archive_sha256=(Path(os.environ['FFMPEG_DIR'])/'.archive-sha256').read_text().strip())
Path(sys.argv[1]).write_text(json.dumps(value,indent=2)+'\n')
NATIVE_ENV
        test "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 6291456
        nice -n 10 cargo test -p kinewright-project --features test-util --test ja_s2_save_cost --no-run --message-format=json > "$base/$side-cost-build.jsonl" 2> "$base/$side-cost-build.stderr"
        )
    done
    cd "$root"
    nice -n 19 python3 -I - "$root" "$base" <<'PY'
import hashlib,json,shutil,subprocess,sys
from pathlib import Path
root,base=map(Path,sys.argv[1:]); rows=[]
parent_env=json.loads((base/'parent-build-environment.json').read_text());current_env=json.loads((base/'current-build-environment.json').read_text())
assert parent_env['native']==current_env['native'] and parent_env['archive_sha256']==current_env['archive_sha256']
assert parent_env['target']!=current_env['target'] and parent_env['tmpdir']!=current_env['tmpdir']
for side in ['parent','current']:
    values=[]
    for line in (base/(side+'-cost-build.jsonl')).read_text().splitlines():
        try:r=json.loads(line)
        except ValueError:continue
        if r.get('reason')=='compiler-artifact' and r['target']['name']=='ja_s2_save_cost' and r.get('executable'):values.append(r)
    assert len(values)==1
    binary=Path(values[0]['executable']);digest=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
    label='2aa6633' if side=='parent' else 'C12'
    target=base/'sealed'/(label+'-save-'+digest[:16]);shutil.copy2(binary,target)
    rows.append(dict(side=side,source=label,binary=str(binary),sealed=str(target),sha256=digest,profile='test/debug'))
parent={}
for name in subprocess.check_output(['git','ls-tree','-r','--name-only','2aa6633'],cwd=root).decode().splitlines():
    path=base/'parent-src'/name
    if path.is_file() and not path.is_symlink():
        data=path.read_bytes();assert data==subprocess.check_output(['git','show','2aa6633:'+name],cwd=root),name
        parent[name]=hashlib.sha256(data).hexdigest()
(base/'parent-source-manifest.json').write_text(json.dumps(parent,indent=2)+'\n')
names=subprocess.check_output(['git','ls-files','-co','--exclude-standard','-z'],cwd=root).decode().split('\0')
current={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in names if name and (name.endswith('.rs') or Path(name).name in ('Cargo.toml','Cargo.lock'))}
gate=json.loads((base.parent/'gate-final/source-before.json').read_text())['files']
assert all(gate[name]==digest for name,digest in current.items())
benchmark='crates/kinewright-project/tests/ja_s2_save_cost.rs'
assert (root/benchmark).read_bytes()==(base/'parent-src'/benchmark).read_bytes()
(base/'current-source-manifest.json').write_text(json.dumps(current,indent=2)+'\n')
(base/'sealed'/'binaries.json').write_text(json.dumps(rows,indent=2)+'\n')
(base/'sealed'/'paths.txt').write_text('\n'.join(r['sealed'] for r in rows)+'\n')
PY
    printf '%s\n' 'Descriptive paired debug publisher cost on six populated branches; 12 real comment saves per lane, five alternating pairs. Report means, median/range, sync and verification counters, and correct reopen. No speedup or product latency claim.' > "$base/measurement-claim.txt"
    rustc -Vv > "$base/timing-toolchain.txt"
    git rev-parse HEAD > "$base/timing-parent-head.txt"
    git diff --binary HEAD > "$base/timing-source.patch"
    exit 0
fi
if [[ "$mode" != run ]]; then echo 'unknown timing mode' >&2; exit 2; fi
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4 TMPDIR="$root/target/c12-tmp"
readarray -t sealed < "$base/sealed/paths.txt"
mkdir -p "$base/timings"
lane() {
    local label="$1" binary="$2" directory="$base/timings/$1"
    mkdir "$directory"
    test "$(awk '/MemAvailable/ {print $2}' /proc/meminfo)" -ge 6291456
    nice -n 19 python3 -I -c 'import hashlib,json,sys; from pathlib import Path; rows=json.loads(Path(sys.argv[1]).read_text()); assert all(hashlib.file_digest(Path(r["sealed"]).open("rb"), "sha256").hexdigest()==r["sha256"] for r in rows)' "$base/sealed/binaries.json"
    "${helper[@]}" annotation "$directory/start.json"
    printf '%s\n' "$label" > "$base/timings/current.txt"
    local code=0
    JA_SAVE_BENCH_OUTPUT="$directory/save-cost.json" timeout 600 nice -n 10 "$binary" --exact repeated_small_saves_with_six_populated_branches --test-threads=1 > "$directory/stdout.log" 2> "$directory/stderr.log" &
    local owned_pid=$!
    "${helper[@]}" sample "$owned_pid" "$directory/contamination.json" &
    local sampler=$!
    wait "$owned_pid" || code=$?
    wait "$sampler"
    "${helper[@]}" annotation "$directory/end.json"
    "${helper[@]}" verify "$directory" "$code" 1
}
for pair in 1 2 3 4 5; do
    lane "pair-$pair-A" "${sealed[0]}"
    lane "pair-$pair-B" "${sealed[1]}"
done
"${helper[@]}" summarize "$base/timings"
echo complete > "$base/timings/current.txt"
