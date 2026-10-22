#!/usr/bin/env bash
# Run the accepted JA1 S1 per-commit Linux gate, serially, preserving raw evidence.
set -euo pipefail
cd "$(dirname "$0")/.."
stage="${1:?commit ordinal 1-8}"
evidence="${2:?absolute evidence directory}"
[[ "$stage" =~ ^[1-8]$ && "$evidence" = /* ]]
mkdir -p "$evidence"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
run() {
    local label="$1"
    shift
    local log="$evidence/$label.log"
    local memory_wait=$SECONDS
    while [[ "$1" == cargo && "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do
        if (( SECONDS - memory_wait > 7200 )); then echo "Memory blocked for over two hours" >&2; return 1; fi
        echo "Waiting for 6 GiB available before $label"
        sleep 30
    done
    free -g
    {
        git rev-parse HEAD
        if [[ -x "$1" ]]; then sha256sum "$1"; fi
        git diff --binary | sha256sum
        git ls-files -m -o --exclude-standard -z | sort -z | xargs -0 -r sha256sum
        printf 'COMMAND:'
        printf ' %q' "$@"
        printf '\n'
    } > "$log"
    echo "Running $label"
    if nice -n 10 "$@" >> "$log" 2>&1; then
        rg '^test result:' "$log" || true
        gzip "$log"
    else
        tail -n 65 "$log" | cut -c 1-512
        gzip "$log"
        return 1
    fi
}
run build cargo build --workspace
run eval-bins cargo build -p kinewright-agent --features eval-harness --bins
if [[ "$stage" -ge 6 ]]; then
    run agent-no-default cargo build -p kinewright-agent --no-default-features
    run app-no-default cargo build -p kinewright-app --no-default-features
    run agent-no-default-check cargo check -p kinewright-agent --lib --no-default-features
    run app-no-default-check cargo check -p kinewright-app --bin kinewright-app --no-default-features
fi
if [[ "$stage" -eq 8 ]]; then
    run project-no-default cargo build -p kinewright-project --no-default-features
    run project-no-default-check cargo check -p kinewright-project --no-default-features
    run heap-example-check cargo check -p kinewright-project --features test-util,storage-heap-profile --example ja_s1_storage_budget
fi
run clippy cargo clippy --workspace --all-targets -- -D warnings
if [[ "${JA_S1_FINAL_HEAD:-0}" == 1 ]]; then
    mapfile -t touched < <( { git diff --name-only 076a96c HEAD -- '*.rs'; git diff --name-only -- '*.rs'; git ls-files --others --exclude-standard -- '*.rs'; } | sort -u)
else
    mapfile -t touched < <( { git diff --name-only -- '*.rs'; git ls-files --others --exclude-standard -- '*.rs'; } | sort -u)
fi
run rustfmt rustfmt --edition 2024 --config skip_children=true --check "${touched[@]}"
test_build=(cargo test -p kinewright-core -p kinewright-project -p kinewright-agent -p kinewright-app --features kinewright-agent/eval-harness,kinewright-project/test-util --lib --bins --test ja_s1_history --test ja_baseline --test ja_s1 --no-run --message-format=json)
if [[ "$stage" -ge 2 ]]; then test_build+=(--test ja_s1_storage); fi
if [[ "$stage" -ge 5 ]]; then test_build+=(--test ja_s1_receipts); fi
if [[ "$stage" -ge 6 ]]; then test_build+=(--test ja_s1_branches); fi
if [[ "$stage" -ge 7 ]]; then test_build+=(--test ja_s1_copy); fi
if [[ "$stage" -eq 3 || "$stage" -eq 4 || "$stage" -eq 7 || "$stage" -eq 8 ]]; then test_build+=(-p kinewright-media --test generated_media); fi
run test-build "${test_build[@]}"
gzip -cd "$evidence/test-build.log.gz" | nice -n 19 python3 -I -c 'import json,sys; rows=[json.loads(s) for s in sys.stdin if s.startswith("{")]; print(json.dumps({r["target"]["name"]:r["executable"] for r in rows if r.get("reason")=="compiler-artifact" and r.get("executable") and r.get("profile",{}).get("test")}))' > "$evidence/test-binaries.json"
JA_HARNESS_APP_TEST_BINARY="$(nice -n 19 python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["kinewright-app"])' "$evidence/test-binaries.json")"
export JA_HARNESS_APP_TEST_BINARY
run_selected() {
    local label="$1"
    shift
    local -a selected
    mapfile -d '' -t selected < <(nice -n 19 python3 -I scripts/ja-s1-test-command.py "$evidence/test-binaries.json" "$@" 2> "$evidence/$label-selection.json")
    [[ "${#selected[@]}" -gt 0 ]]
    run "$label" "${selected[@]}"
}
run_selected architecture-agent cargo test -p kinewright-agent --lib --features eval-harness -- session::tests::in2_the_agent_crate_has_one_agent_event_loop --exact
run_selected architecture-app cargo test -p kinewright-app --bin kinewright-app -- investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call --exact
if [[ "$stage" -ge 6 ]]; then
    run architecture-agent-normal cargo test -p kinewright-agent --lib --no-default-features -- session::tests::in2_the_agent_crate_has_one_agent_event_loop --exact
    run architecture-app-normal cargo test -p kinewright-app --bin kinewright-app --no-default-features -- investigator::tests::in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call --exact
fi
run_selected core-history cargo test -p kinewright-core --test ja_s1_history
run_selected revised cargo test -p kinewright-agent --features eval-harness --test ja_s1
if [[ "$stage" -ge 2 ]]; then
    run_selected storage cargo test -p kinewright-project --features test-util --test ja_s1_storage
fi
run_selected harness-lib cargo test -p kinewright-agent --lib --features eval-harness ja_harness:: -- --test-threads=1
run_selected harness-integration cargo test -p kinewright-agent --features eval-harness --test ja_baseline
for test in native_app_bridge_retains_exercised_control_and_property_failures native_app_bridge_keeps_inner_setup_failure_as_not_run_exit_two native_app_bridge_rejects_abnormal_exit_after_completed_report; do
    run_selected "$test" cargo test -p kinewright-agent --features eval-harness --test ja_baseline "$test" -- --ignored --exact
done
if [[ "$stage" -eq 1 || "$stage" -ge 4 ]]; then
    run_selected H cargo test -p kinewright-core --lib -- --exact actor::tests::applied_operations_follow_undo_and_redo_instead_of_the_append_only_log actor::tests::batch_broadcasts_once_appends_each_op_and_undoes_as_one_snapshot actor::tests::coalesced_batches_under_one_key_form_a_single_undo_entry actor::tests::coalesced_history_replays_faithfully_through_applied_operations
fi
if [[ "$stage" -eq 1 || "$stage" -ge 6 ]]; then
    run_selected M cargo test -p kinewright-agent --lib -- --exact branch::tests::sibling_branches_diverge_without_touching_live_state branch::tests::merge_is_atomic_and_rejects_a_stale_base branch::tests::cherry_pick_preserves_branch_order_and_validates_indices_first
fi
if [[ "$stage" -ge 2 ]]; then
    run_selected L cargo test -p kinewright-project --lib -- --exact lockfile::tests::r19_second_acquire_refuses_while_held lockfile::tests::r19_release_frees_the_lock_for_a_clean_reacquire lockfile::tests::f4_symlink_alias_shares_one_lock lockfile::tests::g9_fd_check_detects_a_swapped_object lockfile::race_tests::guard_missing_discovery_with_held_lock lockfile::race_tests::guard_alias_spellings_race_one_owner lockfile::race_tests::h7_stale_same_endpoint_release_spares_the_successor lockfile::race_tests::j3_link_retargeted_after_acquire_refuses_the_next_save headless::tests::headless_save_preserves_an_unreplayed_journal headless::tests::g3_headless_occupied_save_as_preserves_and_pairs headless::tests::f6_failed_project_write_restores_the_prior_sidecar
fi
# Ceiling/format-sensitive controls are mandatory even before the ceiling changes.
run_selected format-core cargo test -p kinewright-core --lib model::tests::project_format_version_reads_the_top_level_key -- --exact
run_selected F cargo test -p kinewright-project --lib -- --exact project::tests::r1_save_bytes_are_byte_identical_pre_post_cutover project::tests::v2_stamps_only_when_used project::tests::mo2_inactive_features_still_stamp_v2 project::tests::mo2_old_reader_drops_an_unknown_blend_and_refuses_overwrite project::tests::project_atomic_write_falls_back_when_rename_is_refused project::tests::mo2_old_readers_distinguish_parse_validation_and_advisory
run_selected format-eval cargo test -p kinewright-agent --bin kinewright-eval tests::in2b_eval_refuses_a_newer_file_and_reads_v1_as_before -- --exact
run_selected format-app cargo test -p kinewright-app --bin kinewright-app -- --exact project::tests::can_overwrite_save_gates_on_the_read_version project::tests::in2b_v1_project_bytes_are_identical app::in2b_tests::in2b_a_newer_file_opens_with_one_incident_and_no_overwrite app::in2b_tests::in2b_startup_missing_media_aggregate_is_transient app::in2b_tests::in2b_open_time_aggregates_are_all_transient
if [[ "$stage" -eq 3 || "$stage" -eq 4 || "$stage" -eq 7 || "$stage" -eq 8 ]]; then
    run_selected P-media cargo test -p kinewright-media --lib -- --test-threads=1 --exact engine::tests::i8_media_0 engine::tests::i8_media_1 engine::tests::i8_media_2 engine::tests::i8_media_3 engine::tests::i8_media_real_audio engine::tests::only_a_current_stamped_failure_stops_playback engine::tests::a_request_issued_before_a_new_document_is_superseded_not_relabelled engine::tests::an_idle_engine_holds_no_preview_reader_or_decoder engine::tests::dropping_the_engine_stops_the_preview_thread engine::tests::sync_decoders_counts_open_decoders_until_teardown
    run_selected P-presentation cargo test -p kinewright-app --bin kinewright-app -- --exact presenter::tests::the_marker_marks_what_is_bound_when_it_paints presenter::tests::a_marker_that_never_paints_is_never_acked presenter::tests::the_real_paint_callback_acks_only_painted_images presenter::tests::seeded_interleavings_bind_and_ack_only_current_frames recovery::tests::journal_round_trip_preserves_agent_ops_and_undo_redo_history
    run_selected P-generated cargo test -p kinewright-media --test generated_media -- --test-threads=1 --exact two_track_effect_export_matches_preview_after_h264_redecode title_export_pixels_match_preview_after_h264_redecode cancelled_export_writes_no_output
fi
if [[ "$stage" -eq 4 || "$stage" -eq 8 ]]; then
    run_selected I-headless cargo test -p kinewright-project --lib headless::tests
    run_selected I-lock cargo test -p kinewright-project --lib lockfile::race_tests
    run_selected I-app cargo test -p kinewright-app --bin kinewright-app app::tests
    run_selected I-app-refusal cargo test -p kinewright-app --bin kinewright-app app::in2b_tests::save_as_unsupported_source_or_destination_has_zero_mutations -- --exact
    run_selected I-eval-refusal cargo test -p kinewright-agent --bin kinewright-eval tests::rerender_refuses_v3_before_projection_or_output -- --exact
    run_selected AU6-inventory cargo test -p kinewright-media --lib au6_fixtures::au6_declared_test_names_exist_in_their_source_files -- --exact
    run_selected I-eval-legacy cargo test -p kinewright-agent --bin kinewright-eval tests::rerender_admitted_legacy_reaches_renderer_and_report_writer -- --exact
    run_selected I-project cargo test -p kinewright-app --bin kinewright-app project::tests
fi
if [[ "$stage" -ge 5 ]]; then run_selected receipts cargo test -p kinewright-project --features test-util --test ja_s1_receipts; fi
if [[ "$stage" -ge 6 ]]; then run_selected branches cargo test -p kinewright-agent --features eval-harness --test ja_s1_branches; fi
if [[ "$stage" -ge 7 ]]; then run_selected copy cargo test -p kinewright-project --features test-util --test ja_s1_copy; fi
run classification nice -n 19 python3 -I scripts/slow_tests.py lint
echo "Linux commit $stage gate complete; inspect ignored/NOT RUN lanes separately."
