#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
root="$PWD"
evidence="${1:?usage: ja1-s2-c10-mutations.sh /absolute/evidence-directory [case ...]}"
shift
mkdir -p "$evidence"
export CARGO_TARGET_DIR="$root/target/c10" TMPDIR="$root/target/c10-tmp" CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
mkdir -p "$TMPDIR"
source scripts/setup-ffmpeg.sh
helper=(nice -n 19 python3 -I scripts/ja1-s2-c10-mutations.py)
mapfile -t names < <("${helper[@]}" list)
if (( $# )); then names=("$@"); fi
current=''
restore() { if [[ -n "$current" && -f "$current/originals.json" ]]; then "${helper[@]}" restore "$current"; fi; }
trap restore EXIT INT TERM
for name in "${names[@]}"; do
    current="$evidence/$name"
    "${helper[@]}" prepare "$current" "$name"
    mapfile -t command < <("${helper[@]}" command "$current")
    for phase in red green; do
        if [[ "$phase" == green ]]; then "${helper[@]}" restore "$current"; fi
        (( $(awk '/MemAvailable/ {print $2}' /proc/meminfo) >= 6291456 ))
        code=0
        setsid timeout 600 nice -n 10 "${command[@]}" > "$current/$phase.stdout" 2> "$current/$phase.stderr" &
        owned_pid=$!
        wait "$owned_pid" || code=$?
        # This session contains only this Cargo invocation and its test children.
        kill -TERM -- "-$owned_pid" 2>/dev/null || true
        "${helper[@]}" verify "$current" "$phase" "$code"
    done
    current=''
done
