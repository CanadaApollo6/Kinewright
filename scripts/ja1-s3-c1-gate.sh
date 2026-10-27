#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/setup-ffmpeg.sh
export RUST_TEST_THREADS=4
c1_runtime="$PWD/target/c1-runtime"
export LOCALAPPDATA="$PWD/target/c1-localappdata"
export TMPDIR="$c1_runtime/tmp"
export XDG_STATE_HOME="$c1_runtime/state"
export XDG_DATA_HOME="$c1_runtime/share"
export XDG_CACHE_HOME="$c1_runtime/cache"
export XDG_CONFIG_HOME="$c1_runtime/config"
mkdir -p "$LOCALAPPDATA/Kinewright/recovery" "$TMPDIR" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"
c1_evidence=docs/reports/ja1-s3/c1-evidence
mkdir -p "$c1_evidence"
cargo_locked() {
    local available
    available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
    if (( available < 6 * 1024 * 1024 )); then
        echo 'C1 requires at least 6 GiB available memory.' >&2
        return 75
    fi
    flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo "$@"
}
record() {
    local label=$1 result=0
    shift
    printf '%q ' "$@" > "$c1_evidence/$label.command"
    printf '\n' >> "$c1_evidence/$label.command"
    "$@" > "$c1_evidence/$label.log" 2>&1 || result=$?
    printf '%s\n' "$result" > "$c1_evidence/$label.exit"
    tail -n 8 "$c1_evidence/$label.log"
    gzip -f "$c1_evidence/$label.log"
    return "$result"
}
case "${1:-all}" in
    build) record build cargo_locked build --workspace ;;
    clippy) record clippy cargo_locked clippy --workspace --all-targets -- -D warnings ;;
    project) record project-c1 cargo_locked test -p kinewright-project --features test-util --test ja_s3_store -- --test-threads=1 ;;
    host)
        record host-c1 cargo_locked test -p kinewright-agent --lib writer_host::ja_s3_host -- --test-threads=1
        record host-portable cargo_locked test -p kinewright-agent --lib writer_host::tests::s3_portable_roles_revoke_and_refuse_before_manual_write_admission -- --exact --test-threads=1
        ;;
    app) record app-c1 cargo_locked test -p kinewright-app --bin kinewright-app ja1_attachment::tests::s3:: -- --test-threads=1 ;;
    app-affected) record app-affected cargo_locked test -p kinewright-app --bin kinewright-app ja1_attachment::tests:: -- --test-threads=1 ;;
    activation) record activation cargo_locked test -p kinewright-app --bin kinewright-app ja1_attachment::tests::c9_fix1_unsaved_studio_activation_requires_first_save_before_manual_edit -- --exact --test-threads=1 ;;
    affected)
        record project-affected cargo_locked test -p kinewright-project --features test-util --test ja_s3_store --test ja_s3_owned_inputs --test ja_s1_receipts --test ja_s1_copy --test ja_s2_save --test ja_s2_switch --no-fail-fast -- --test-threads=1
        record project-store cargo_locked test -p kinewright-project --features test-util --test ja_s1_storage bounded_store:: -- --test-threads=1
        record agent-affected cargo_locked test -p kinewright-agent --lib writer_host:: -- --test-threads=1
        record app-affected cargo_locked test -p kinewright-app --bin kinewright-app ja1_attachment::tests:: -- --test-threads=1
        ;;
    regressions)
        record s2-host cargo_locked test -p kinewright-agent --test ja_s2_host -- --test-threads=1
        record m31-branches cargo_locked test -p kinewright-agent --test ja_s1_branches -- --test-threads=1
        record presenter cargo_locked test -p kinewright-app --bin kinewright-app presenter::tests:: -- --test-threads=1
        ;;
    s2-slow)
        for c1_selector in g05l_host_death_recovers_absent_and_committed_drag_suffix g05l_shared_floor_and_recovery_close_reply_drop g05a_switch_reply_drop_and_participant_death; do
            record "$c1_selector" cargo_locked test -p kinewright-agent --features slow-tests --test ja_s2_host "$c1_selector" -- --exact --test-threads=1
        done
        ;;
    scans)
        record agent-architecture cargo_locked test -p kinewright-agent --lib in2_the_agent_crate_has_one_agent_event_loop -- --test-threads=1
        record app-architecture cargo_locked test -p kinewright-app --bin kinewright-app in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call -- --test-threads=1
        ;;
    source-guard) record source-guard cargo_locked test -p kinewright-media --lib au6_fixtures::au6_declared_test_names_exist_in_their_source_files -- --exact --test-threads=1 ;;
    self-exec) record self-exec cargo_locked test -p kinewright-project --features test-util --lib owned_media::tests::s3_fix1_reused_crash_residue_keeps_bytes_and_one_charge -- --exact --test-threads=1 ;;
    save-cost) record save-cost cargo_locked test -p kinewright-project --features test-util --test ja_s2_save_cost -- --test-threads=1 ;;
    lint) record slow-lint nice -n 19 python3 -I scripts/slow_tests.py lint ;;
    fmt)
        mapfile -t c1_files < <({ git diff --name-only 146072543af79c48efb82273dbf06b5136059cc2; git ls-files --others --exclude-standard; } | rg '\.rs$' | sort -u)
        record fmt cargo_locked fmt -- --check --config skip_children=true "${c1_files[@]}"
        ;;
    all)
        for c1_gate in build clippy affected regressions s2-slow scans source-guard self-exec save-cost lint fmt; do
            bash "$0" "$c1_gate"
        done
        ;;
    *) echo 'Unknown C1 gate.' >&2; exit 2 ;;
esac
