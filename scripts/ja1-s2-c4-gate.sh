#!/usr/bin/env bash
# Exact C4 Linux gate. Run from this worktree; evidence must be outside target/.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
root="$PWD"
evidence="$(realpath -m -- "${1:?usage: ja1-s2-c4-gate.sh /absolute/evidence-directory}")"
case "$evidence" in "$root/target"*) echo 'evidence must be outside build output' >&2; exit 2;; esac
mkdir -p "$evidence" "$root/target/c4-tmp"
export CARGO_TARGET_DIR="$root/target/c4-build" TMPDIR="$root/target/c4-tmp" CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
source scripts/setup-ffmpeg.sh
printf 'cwd=%s\nCARGO_TARGET_DIR=%s\nTMPDIR=%s\nevidence=%s\n' "$PWD" "$CARGO_TARGET_DIR" "$TMPDIR" "$evidence" > "$evidence/environment.txt"
rustc -Vv > "$evidence/rustc.txt"
rustup target list --installed > "$evidence/installed-targets.txt"
nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py identity "$evidence/source-before.json"
: > "$evidence/gate.tsv"
wait_memory() {
    while (( $(awk '/MemAvailable/ {print $2}' /proc/meminfo) < 6291456 )); do
        echo 'Waiting for the required 6 GiB available memory.'
        sleep 5
    done
}
run() {
    local label="$1"; shift
    test -d "$evidence"
    printf '%s\n' "$*" > "$evidence/$label.command"
    local code=0
    # Native FFmpeg stderr can interrupt libtest's unfinished stdout line.
    # Retain separate streams, then append stderr after complete stdout outcomes.
    "$@" > "$evidence/$label.log" 2> "$evidence/$label.stderr" || code=$?
    cat "$evidence/$label.stderr" >> "$evidence/$label.log"
    printf '%s\t%s\n' "$label" "$code" >> "$evidence/gate.tsv"
    if (( code != 0 )); then tail -80 "$evidence/$label.log"; return "$code"; fi
}
cargo_run() { wait_memory; run "$1" nice -n 10 cargo "${@:2}"; }
mapfile -t touched < <({ git diff --name-only 8a555fa -- '*.rs'; git ls-files --others --exclude-standard -- '*.rs'; } | sort -u)
if (( ${#touched[@]} == 0 )); then echo 'No touched Rust files' >&2; exit 2; fi
run rustfmt rustfmt --edition 2024 --config skip_children=true --check "${touched[@]}"
cargo_run workspace-build build --workspace
cargo_run agent-shipping-build build -p kinewright-agent --no-default-features --lib
cargo_run app-shipping-build build -p kinewright-app --no-default-features --bin kinewright-app
cargo_run eval-build build -p kinewright-agent --features eval-harness --bins
cargo_run clippy clippy --workspace --all-targets -- -D warnings
select_tests() {
    local label="$1" package="$2" kind="$3" target="$4" features="$5" filters="$6"
    local args=(test -p "$package" "$kind")
    if [[ "$kind" != --lib ]]; then args+=("$target"); fi
    if [[ "$features" == production ]]; then args+=(--no-default-features)
    elif [[ "$features" != none ]]; then args+=(--features "$features"); fi
    cargo_run "$label-list" "${args[@]}" -- --list
    nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py select "$evidence/$label-list.log" "$filters" "$evidence/$label-selected.txt"
    local names=(); mapfile -t names < "$evidence/$label-selected.txt"
    if (( ${#names[@]} == 0 )); then echo "Empty applicable selector: $label" >&2; exit 2; fi
    local threads=4
    if [[ "$package" == kinewright-media ]]; then threads=1; fi
    local execution=(--exact "${names[@]}" --test-threads="$threads")
    if [[ "$label" == ja_baseline ]]; then execution+=(--include-ignored); fi
    cargo_run "$label" "${args[@]}" -- "${execution[@]}"
    nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py verify "$evidence/$label-selected.txt" "$evidence/$label.log" "$evidence/$label-counts.json"
}
export JA_S1_PREDECESSOR_BINARY="$root/target/ja-s1-predecessor/sealed/kinewright-ja-baseline"
if [[ ! -f "$root/target/ja-s1-predecessor/sealed/identity.json" ]]; then
    wait_memory
    run predecessor-preparation bash scripts/ja-s1-predecessor-controls.sh "$evidence/predecessor"
fi
select_tests arch-agent-eval kinewright-agent --lib '' eval-harness session::tests::in2_the_agent_crate_has_one_agent_event_loop
select_tests arch-agent-production kinewright-agent --lib '' production session::tests::in2_the_agent_crate_has_one_agent_event_loop
select_tests arch-app-production kinewright-app --bin kinewright-app production investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call
select_tests arch-app-default kinewright-app --bin kinewright-app none investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call
JA_HARNESS_APP_TEST_BINARY="$(nice -n 19 python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["binary"])' "$evidence/arch-app-default-counts.json")"
export JA_HARNESS_APP_TEST_BINARY
select_tests core-actor kinewright-core --lib '' none actor::tests::
select_tests core-attachment kinewright-core --lib '' none attachment::tests::
select_tests core-history kinewright-core --test ja_s1_history none '*'
select_tests project-lib kinewright-project --lib '' test-util '*'
for target in ja_s1_storage ja_s1_receipts ja_s1_copy ja_s2_save ja_s2_rescue ja_s2_rescue_record; do
    select_tests "$target" kinewright-project --test "$target" test-util '*'
done
export JA_SAVE_BENCH_OUTPUT="$evidence/save-cost-gate.json"
select_tests ja_s2_save_cost kinewright-project --test ja_s2_save_cost test-util '*'
select_tests specialist-identity kinewright-agent --lib '' eval-harness durable_branch::tests::w13_shared_prefix_occurrence_identity_survives_branch_saves
select_tests harness-lib kinewright-agent --lib '' eval-harness ja_harness::
for target in ja_baseline ja_s1 ja_s1_branches; do
    select_tests "$target" kinewright-agent --test "$target" eval-harness '*'
done
select_tests app kinewright-app --bin kinewright-app none 'project::tests::,app::in2b_tests::,recovery::tests::,ja_harness::tests::,presenter::tests::'
select_tests media-pins kinewright-media --lib '' test-util 'engine::tests::i8_media_0,engine::tests::i8_media_1,engine::tests::i8_media_2,engine::tests::i8_media_3,engine::tests::i8_media_real_audio,engine::tests::only_a_current_stamped_failure_stops_playback,engine::tests::a_request_issued_before_a_new_document_is_superseded_not_relabelled,engine::tests::an_idle_engine_holds_no_preview_reader_or_decoder,engine::tests::dropping_the_engine_stops_the_preview_thread,engine::tests::sync_decoders_counts_open_decoders_until_teardown'
select_tests media-generated kinewright-media --test generated_media test-util '*'
run slow-lint nice -n 19 python3 -I scripts/slow_tests.py lint
run slow-features nice -n 19 python3 -I scripts/slow_tests.py features
nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py identity "$evidence/source-after.json"
cmp "$evidence/source-before.json" "$evidence/source-after.json"
echo 'C4 Linux gate complete. Native Windows and hosted CI remain NOT RUN.'
