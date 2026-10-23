#!/usr/bin/env bash
# Targeted runtime red/restored-green pairs. Never runs a full crate/workspace mutation.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
root="$PWD"
evidence="$(realpath -m -- "${1:?usage: ja1-s2-c1-mutations.sh /absolute/new-evidence-directory [case...]}")"
shift
mkdir -p "$evidence" target/c1-tmp
export CARGO_TARGET_DIR="$root/target/c1-head-build" TMPDIR="$root/target/c1-tmp" CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
source scripts/setup-ffmpeg.sh
nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py identity "$evidence/source-before.json"
active=''
restore() {
    if [[ -n "$active" ]]; then nice -n 19 python3 -I scripts/ja1-s2-c1-mutations.py restore "$active" "$evidence/$active"; active=''; fi
}
trap restore EXIT INT TERM
cases=(phase copy-phase committed-proof rename named-copy charge wording tag ordinary-barrier owned-input unknown-phase target-only no-dedup memo-stale-cost memo-wrong-ref memo-missing-branch memo-invalidation encoded-cost no-preflight receipt-validation fixture-phase fixture-cleanup)
if (( $# )); then cases=("$@"); fi
for id in "${cases[@]}"; do
    package=kinewright-project; target=ja_s2_save; features=test-util
    case "$id" in
        phase|copy-phase|committed-proof) test=linux::w12_post_b5_floor_error_keeps_publication_phase ;;
        rename) test=linux::w12_nonpermission_rename_retains_charged_temporary ;;
        named-copy) test=linux::w12_named_owner_cleanup_allows_ordinary_fresh_copy ;;
        charge) test=linux::m03_pending_controls_have_an_exact_independent_charge ;;
        wording) test=linux::m04_cleanup_only_status_does_not_claim_unknown_root ;;
        tag) test=linux::m05_tag_appears_only_after_floor_tagging_succeeds ;;
        ordinary-barrier) test=linux::m08_ordinary_save_staging_and_cleanup_fault_preserves_history ;;
        owned-input) test=linux::w01_ordinary_first_save_phase_and_owned_bytes_control ;;
        unknown-phase) test=linux::w10_pending_publication_phase_control ;;
        target-only|no-dedup) test=linux::w13_every_publication_fits_fresh_open ;;
        memo-stale-cost|memo-missing-branch|encoded-cost|no-preflight) test=linux::w13_memo_matches_reader_after_changed_history_and_cache_misses ;;
        memo-wrong-ref) test=linux::w13_memo_rejects_wrong_object_ref_identity ;;
        memo-invalidation) test=linux::w13_memo_is_discarded_after_namespace_continuity_failure ;;
        receipt-validation) target=ja_s1_receipts; test=unchanged_comment_validates_fresh_open_and_coalesces_directory_syncs ;;
        fixture-*) package=kinewright-agent; target=ja_s1; features=eval-harness; test=s2_additive_phase_cleanup_fixture_observes_real_save_outcomes ;;
        *) echo "unknown case $id" >&2; exit 2 ;;
    esac
    mkdir "$evidence/$id"
    nice -n 19 python3 -I scripts/ja1-s2-c1-mutations.py apply "$id" "$evidence/$id"
    active="$id"
    for phase in red green; do
        while (( $(awk '/MemAvailable/ {print $2}' /proc/meminfo) < 6291456 )); do sleep 5; done
        args=(test -p "$package" --features "$features" --test "$target" "$test" -- --exact --nocapture --test-threads=4)
        printf '%s\n' "cargo ${args[*]}" > "$evidence/$id/$phase.command"
        code=0
        nice -n 10 cargo "${args[@]}" > "$evidence/$id/$phase.log" 2>&1 || code=$?
        binary="$(sed -nE 's/^ +Running .* \((.*)\)$/\1/p' "$evidence/$id/$phase.log")"
        if [[ -n "$binary" && -f "$binary" ]]; then sha256sum "$binary" > "$evidence/$id/$phase.binary.sha256"; fi
        if [[ "$phase" == red ]]; then
            restore
            if (( code != 101 )) || ! rg -q '^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' "$evidence/$id/red.log"; then
                echo "Mutation $id did not produce its one runtime red (exit=$code)" >&2; tail -25 "$evidence/$id/red.log"; exit 2
            fi
            if [[ "$id" == target-only ]] && ! rg -q 'published candidate cannot exceed independent fresh-open capacity' "$evidence/$id/red.log"; then echo 'Target-only mutation was masked before the independent-open oracle' >&2; exit 2; fi
            if [[ "$id" == target-only ]] && ! rg -q 'Capacity\("live history bytes"\)' "$evidence/$id/red.log"; then echo 'Target-only red was not the live-history capacity oracle' >&2; exit 2; fi
            if [[ "$id" == no-dedup ]] && ! rg -q 'shared-history exact-fit publication refused' "$evidence/$id/red.log"; then echo 'Shared-hash red missed the exact-fit publication oracle' >&2; exit 2; fi
        else
            if (( code != 0 )) || ! rg -q '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$evidence/$id/green.log"; then echo "Restored $id failed" >&2; tail -25 "$evidence/$id/green.log"; exit 2; fi
        fi
        printf '%s\t%s\t%s\t%s\n' "$id" "$phase" "$code" "$test" >> "$evidence/results.tsv"
    done
    echo "Mutation $id: runtime red / restored green"
done

nice -n 19 python3 -I scripts/ja1-s2-c1-evidence.py identity "$evidence/source-after.json"
cmp "$evidence/source-before.json" "$evidence/source-after.json"
