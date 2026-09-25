#!/usr/bin/env bash
# AW1 line-count ledger (F8/N1): new prod lines over a revision range.
#
# Rules (mirroring the S1 report's accounting):
# - Prod = crate/manifest sources minus `#[cfg(test)] mod …` regions.
# - Test-only `#[cfg(any(test, feature = "test-util"))]` seams count as
#   prod (they compile into the lib under the feature); indented
#   single-item `#[cfg(test)]`s likewise (conservative).
# - Moved lines count 0: the total is a multiset difference over the
#   concatenated prod of every touched file, so relocated lines cancel.
#   Renamed lines count as new (conservative).
# - Cargo.lock is generated and excluded; manifests count as prod.
# - Blank lines and comments count (as in S1's 771).
#
# Usage: ./scripts/aw1-line-ledger.sh [BASE] [HEAD]
#   BASE defaults to c30bd3d (the fix round); HEAD defaults to HEAD.
set -euo pipefail

BASE="${1:-c30bd3d}"
HEAD="${2:-HEAD}"
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

: >"$tmp/prod_old.txt"
: >"$tmp/prod_new.txt"
: >"$tmp/test_old.txt"
: >"$tmp/test_new.txt"
printf '%-52s %6s %6s\n' "file" "prod+" "test+"

# Process substitution keeps the loop in the main shell, so the
# accumulators survive for the multiset totals below.
while read -r file; do
    git show "$BASE:$file" >"$tmp/old.rs" 2>/dev/null || : >"$tmp/old.rs"
    git show "$HEAD:$file" >"$tmp/new.rs" 2>/dev/null || : >"$tmp/new.rs"
    strip_tests <"$tmp/old.rs" >"$tmp/old_prod.rs"
    strip_tests <"$tmp/new.rs" >"$tmp/new_prod.rs"
    keep_tests <"$tmp/old.rs" >"$tmp/old_test.rs"
    keep_tests <"$tmp/new.rs" >"$tmp/new_test.rs"
    cat "$tmp/old_prod.rs" >>"$tmp/prod_old.txt"
    cat "$tmp/new_prod.rs" >>"$tmp/prod_new.txt"
    cat "$tmp/old_test.rs" >>"$tmp/test_old.txt"
    cat "$tmp/new_test.rs" >>"$tmp/test_new.txt"
    prod_adds=$(diff "$tmp/old_prod.rs" "$tmp/new_prod.rs" | grep -c '^>' || true)
    test_adds=$(diff "$tmp/old_test.rs" "$tmp/new_test.rs" | grep -c '^>' || true)
    printf '%-52s %6s %6s\n' "$file" "$prod_adds" "$test_adds"
done < <(git diff --name-only "$BASE".."$HEAD" -- '*.rs' | sort)

prod_new=$(comm -23 <(sort "$tmp/prod_new.txt") <(sort "$tmp/prod_old.txt") | wc -l)
test_new=$(comm -23 <(sort "$tmp/test_new.txt") <(sort "$tmp/test_old.txt") | wc -l)
echo "---- totals (multiset: moved lines cancel) ----"
echo "new prod lines (rs): $prod_new"
echo "new test lines (rs): $test_new"
echo "---- manifests (prod) ----"
git diff --numstat "$BASE".."$HEAD" -- '*Cargo.toml' Cargo.toml
echo "(Cargo.lock excluded as generated)"
