#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/../../../../.."
evidence=docs/reports/ja1-s3/evidence/fix1
run() {
    local label=$1
    shift
    printf '%s\n' "gate $label"
    if [[ -f "$evidence/$label.log.gz" ]]; then
        local attempt=1
        while [[ -f "$evidence/$label-prior$attempt.log.gz" ]]; do
            attempt=$((attempt + 1))
        done
        mv "$evidence/$label.log.gz" "$evidence/$label-prior$attempt.log.gz"
    fi
    local result=0
    bash "$evidence/run-cargo.sh" "$@" > "$evidence/$label.log" 2>&1 || result=$?
    tail -n 8 "$evidence/$label.log"
    printf '%s %s\n' "$label" "$result" >> "$evidence/gate-exits.txt"
    gzip -f "$evidence/$label.log"
    return "$result"
}
if (( $# == 0 )); then
    set -- build project app host host-recovery media clippy windows fmt
fi
for gate in "$@"; do
    case "$gate" in
        project) run project test -p kinewright-project --features test-util --no-fail-fast -- --test-threads=1 ;;
        app) run app test -p kinewright-app --bin kinewright-app -- --test-threads=1 ;;
        host) run host test -p kinewright-agent --lib -- --test-threads=1 s3_ s2h_ c12_tests:: in2_the_agent_crate_has_one_agent_event_loop ;;
        host-recovery) run host-recovery test -p kinewright-agent --test ja_s2_host -- --test-threads=1 g05l_ g05a_fast_ rescue_and_switch_share_one_owner_after_restart ;;
        media) run media test -p kinewright-media owned_io -- --test-threads=1 ;;
        clippy) run clippy clippy --workspace --all-targets -- -D warnings ;;
        build) run build build --workspace ;;
        windows) run windows xwin clippy --target x86_64-pc-windows-msvc -p kinewright-project -p kinewright-media --lib -- -D warnings ;;
        fmt)
            mapfile -t files < <(git diff --name-only -- '*.rs')
            run fmt fmt -- --check --config skip_children=true "${files[@]}"
            ;;
        *) exit 2 ;;
    esac
done
