#!/usr/bin/env bash
# AW1 line-count ledger, fix round 2 (G13 / R1 RS4): true production lines
# added from 6c2bdec to HEAD. Replaces the fix-round-1 script: no hand
# deltas are added to S1's unsupported 771 — this prints the whole figure.
#
# Method (scripted end to end):
# - Range: `git diff --no-renames` from BASE to HEAD over '*.rs', so a
#   moved file lands as an explicit delete+add pair on both endpoints.
# - Tests are not counted: col-0 `#[cfg(test)] mod …` regions are
#   stripped from both endpoints, and files mounted under `#[cfg(test)]`
#   via `#[path = "…"]` (detected by scan, e.g. aw1_race_tests.rs) count
#   wholly as test. `#[cfg(any(test, feature = "test-util"))]` seams
#   count as prod — conservative, they compile into the lib under the
#   feature — as do indented single-item `#[cfg(test)]`s.
# - Normalisation: strip leading indent and one visibility prefix
#   (`pub`, `pub(crate)`, `pub(super)`, `pub(self)`, `pub(in …)`), so
#   moved code that was re-indented or re-exported still cancels.
# - Moved-span treatment: multiset difference over the concatenated,
#   normalised prod of every touched file — a normalised line added as
#   often as it was deleted counts 0. Relocated-but-edited lines count
#   as new: the edit is new work.
# - Manifests count as prod: added lines per Cargo.toml, stated below
#   (the S0/S1 allocation). Cargo.lock is generated and excluded.
# - Blank lines and comments count (as in S1's accounting).
#
# Usage: ./scripts/aw1-line-ledger.sh [BASE] [HEAD]
#   BASE defaults to 6c2bdec (pre-AW1 S0/S1); HEAD defaults to HEAD.
set -euo pipefail

BASE="${1:-6c2bdec}"
HEAD="${2:-HEAD}"
ROOT="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Print stdin minus col-0 `#[cfg(test)] mod …` regions (all AW1 test mods
# are col-0; everything inside them is indented, so the first col-0 `}`
# closes the region).
strip_tests() {
    awk '
        /^#\[cfg\(test\)\]$/ {
            attr = $0
            if ((getline line) <= 0) { print attr; exit }
            if (line ~ /^ *(pub\(crate\) *)?mod [A-Za-z0-9_]+ \{$/) { skip = 1 }
            else { print attr; print line }
            next
        }
        skip && /^\}$/ { skip = 0; next }
        skip { next }
        { print }
    '
}

# Print ONLY the col-0 `#[cfg(test)] mod …` regions (test-side measure).
keep_tests() {
    awk '
        /^#\[cfg\(test\)\]$/ {
            attr = $0
            if ((getline line) <= 0) { exit }
            if (line ~ /^ *(pub\(crate\) *)?mod [A-Za-z0-9_]+ \{$/) {
                skip = 1; print attr; print line
            }
            next
        }
        skip && /^\}$/ { skip = 0; print; next }
        skip { print; next }
    '
}

# Mount literals (`#[path = "…"]`) on the line after a col-0
# `#[cfg(test)]` in the given file.
cfg_test_mounts() {
    grep -A1 '^#\[cfg(test)\]$' "$1" \
        | grep '#\[path = ' \
        | sed 's/.*#\[path = "\([^"]*\)".*/\1/' \
        || true
}

# Visibility/indent normalisation for moved-span matching.
normalise() {
    sed -e 's/^[[:space:]]*//' \
        -e 's/^pub(in [^)]*) //' \
        -e 's/^pub(crate) //' \
        -e 's/^pub(super) //' \
        -e 's/^pub(self) //' \
        -e 's/^pub //'
}

mapfile -t files < <(git diff --no-renames --name-only "$BASE" "$HEAD" -- '*.rs' | sort)

# Union of cfg(test)-mounted files across both endpoints of every touched
# file, repo-relative.
: >"$tmp/mounted_test.txt"
for file in "${files[@]}"; do
    dir="$ROOT/$(dirname "$file")"
    for rev in "$BASE" "$HEAD"; do
        git show "$rev:$file" >"$tmp/side.rs" 2>/dev/null || continue
        while read -r mount; do
            [ -n "$mount" ] || continue
            (cd "$dir" && realpath -m --relative-to="$ROOT" "$mount") >>"$tmp/mounted_test.txt"
        done < <(cfg_test_mounts "$tmp/side.rs")
    done
done

: >"$tmp/prod_old.txt"
: >"$tmp/prod_new.txt"
: >"$tmp/test_old.txt"
: >"$tmp/test_new.txt"
printf '%-52s %6s %6s\n' "file" "prod+" "test+"

# Plain loop (no subshell): the accumulators below must survive.
for file in "${files[@]}"; do
    git show "$BASE:$file" >"$tmp/old.rs" 2>/dev/null || : >"$tmp/old.rs"
    git show "$HEAD:$file" >"$tmp/new.rs" 2>/dev/null || : >"$tmp/new.rs"
    if grep -qxF "$file" "$tmp/mounted_test.txt" 2>/dev/null; then
        # A cfg(test)-mounted module: wholly test on both endpoints.
        : >"$tmp/old_prod.rs"
        : >"$tmp/new_prod.rs"
        cp "$tmp/old.rs" "$tmp/old_test.rs"
        cp "$tmp/new.rs" "$tmp/new_test.rs"
    else
        strip_tests <"$tmp/old.rs" >"$tmp/old_stripped.rs"
        strip_tests <"$tmp/new.rs" >"$tmp/new_stripped.rs"
        normalise <"$tmp/old_stripped.rs" >"$tmp/old_prod.rs"
        normalise <"$tmp/new_stripped.rs" >"$tmp/new_prod.rs"
        keep_tests <"$tmp/old.rs" >"$tmp/old_test.rs"
        keep_tests <"$tmp/new.rs" >"$tmp/new_test.rs"
    fi
    cat "$tmp/old_prod.rs" >>"$tmp/prod_old.txt"
    cat "$tmp/new_prod.rs" >>"$tmp/prod_new.txt"
    cat "$tmp/old_test.rs" >>"$tmp/test_old.txt"
    cat "$tmp/new_test.rs" >>"$tmp/test_new.txt"
    prod_adds=$(diff "$tmp/old_prod.rs" "$tmp/new_prod.rs" | grep -c '^>' || true)
    test_adds=$(diff "$tmp/old_test.rs" "$tmp/new_test.rs" | grep -c '^>' || true)
    printf '%-52s %6s %6s\n' "$file" "$prod_adds" "$test_adds"
done

prod_new=$(comm -23 <(sort "$tmp/prod_new.txt") <(sort "$tmp/prod_old.txt") | wc -l)
test_new=$(comm -23 <(sort "$tmp/test_new.txt") <(sort "$tmp/test_old.txt") | wc -l)
echo "---- totals (multiset: moved normalised lines cancel) ----"
echo "new prod lines (rs): $prod_new"
echo "new test lines (rs, not counted): $test_new"
echo "---- manifests, S0/S1 allocation (prod) ----"
manifest_added=0
while read -r added _ path; do
    echo "added $added $path"
    manifest_added=$((manifest_added + added))
done < <(git diff --no-renames --numstat "$BASE" "$HEAD" -- '*Cargo.toml' Cargo.toml)
echo "(Cargo.lock excluded as generated)"
echo "---- figure ----"
echo "production lines $BASE..$HEAD: $((prod_new + manifest_added)) (rs $prod_new + manifests $manifest_added)"
