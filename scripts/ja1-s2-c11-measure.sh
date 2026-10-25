#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
base="${1:?usage: ja1-s2-c11-measure.sh /absolute/evidence-directory}"
cd "$root"
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4 TMPDIR="$root/target/c11-tmp"
helper=(nice -n 19 python3 -I "$root/scripts/ja1-s2-c11-measure.py")
readarray -t sealed < "$base/sealed/paths.txt"
mkdir "$base/timings"
lane() {
    local label="$1" expected="$2" binary="$3"; shift 3
    local directory="$base/timings/$label"
    mkdir "$directory"
    nice -n 19 python3 -I -c 'import hashlib,json,sys; from pathlib import Path; rows=json.loads(Path(sys.argv[1]).read_text()); assert all(hashlib.file_digest(Path(r["sealed"]).open("rb"), "sha256").hexdigest()==r["sha256"] for r in rows)' "$base/sealed/binaries.json"
    "${helper[@]}" annotation "$directory/start.json"
    printf '%s\n' "$label" > "$base/timings/current.txt"
    local code=0
    JA_SWITCH_HOST_BINARY="${sealed[3]}" JA_SWITCH_TIMINGS="$directory/switch-times.jsonl" JA_SAVE_BENCH_OUTPUT="$directory/save-cost.json" "${sealed[4]}" "$directory/resource.json" timeout 600 nice -n 10 "$binary" "$@" > "$directory/stdout.log" 2> "$directory/stderr.log" &
    local owned_pid=$!
    "${helper[@]}" sample "$owned_pid" "$directory/contamination.json" &
    local sampler=$!
    wait "$owned_pid" || code=$?
    wait "$sampler"
    "${helper[@]}" annotation "$directory/end.json"
    "${helper[@]}" verify "$directory" "$code" "$expected"
}
for pair in 1 2 3 4 5; do
    lane "pair-$pair-A" 1 "${sealed[0]}" --exact repeated_small_saves_with_six_populated_branches --test-threads=1
    lane "pair-$pair-B" 1 "${sealed[1]}" --exact repeated_small_saves_with_six_populated_branches --test-threads=1
    lane "switch-$pair" 1 "${sealed[2]}" --exact g05a_fast_b5_switch_reply_loss_before_and_after_restore --test-threads=1 --nocapture
done
"${helper[@]}" summarize "$base/timings"
echo complete > "$base/timings/current.txt"
