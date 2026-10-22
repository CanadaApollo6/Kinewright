#!/usr/bin/env bash
# Only the affected storage witness runs for each mutation, then restored source.
set -euo pipefail
cd "$(dirname "$0")/.."
evidence="${1:?absolute evidence directory}"
mkdir -p "$evidence"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 TMPDIR="$PWD/target/ja-s1-tmp"
for mutation in hash closure copy live history decoded branch tamper decoded-lease dedup; do
    file=crates/kinewright-project/src/store.rs
    case "$mutation" in
        hash|closure|decoded-lease) test=object_admission_checks_exact_bytes_and_dependency_closure ;;
        copy|dedup) test=reservation_counts_unique_blobs_and_simultaneous_copies ;;
        live) test=failed_seal_and_last_reader_retire_charges_once ;;
        history|decoded|branch) test=storage_ingress_refuses_decoded_and_transient_overflow ;;
        tamper) test=owned_input_tamper_invalidates_verified_identity ;;
    esac
    if [[ "$mutation" == copy || "$mutation" == history || "$mutation" == decoded ]]; then file=crates/kinewright-project/src/storage_budget.rs; fi
    cp "$file" "$evidence/$mutation.before.rs"
    if [[ "$mutation" == decoded-lease ]]; then cp crates/kinewright-project/src/storage_budget.rs "$evidence/decoded-lease-budget.before.rs"; fi
    trap 'cp "$evidence/$mutation.before.rs" "$file"; if [[ "$mutation" == decoded-lease && -f "$evidence/decoded-lease-budget.before.rs" ]]; then cp "$evidence/decoded-lease-budget.before.rs" crates/kinewright-project/src/storage_budget.rs; fi' EXIT
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do free -g; sleep 300; done
    nice -n 19 python3 -I - "$file" "$mutation" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]);s=p.read_text();m=sys.argv[2]
if m=='hash': a='if digest != reference.sha256 {';b='if false && digest != reference.sha256 {'
elif m=='closure':a='        self.closure(&dependencies)?;';b=''
elif m=='copy':
 a='        state.reserved_copy_bytes = reserved_copies;';b='        state.reserved_copy_bytes = 0;'
 s=s.replace(a,b,1);p.write_text(s);sys.exit(0)
elif m=='history':a='            || history_total > limits.live_history_bytes';b=''
elif m=='decoded':a='            || decoded > limits.decoded_record_bytes';b=''
elif m=='branch':a='if bytes > self.limits().branch_history_bytes {';b='if false && bytes > self.limits().branch_history_bytes {'
elif m=='dedup':a='        if existing {';b='        if false && existing {'
elif m=='live':
 s+='''
impl Drop for ProjectStore {
    fn drop(&mut self) {
        if self.inner.session_id.is_some() {
            if let Ok(mut ledger) = self.inner.ledger.lock() { ledger.pins.clear(); }
            let _ = collect(&self.inner);
        }
    }
}
'''
 p.write_text(s);sys.exit(0)
elif m=='tamper':
 a='''        if file.metadata()?.len() != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)
        {'''
 b='''        if false && (file.metadata()?.len() != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256))
        {'''
else:
 # The typed value is returned, but its reservation is released first.
 a='        Ok(decoded_reservation.hold(value))'
 b='''        let held = decoded_reservation.hold(value);
        let held = StorageReservation::uncharge_for_mutation(held);
        Ok(held)'''
 assert s.count(a)==1;s=s.replace(a,b)
 # Use the normal reservation ownership path; the mutation helper lives beside it.
 budget=Path('crates/kinewright-project/src/storage_budget.rs')
 original=budget.read_text()
 original+='''
impl<T> StorageReservation<T> {
    pub(crate) fn uncharge_for_mutation(mut held: Self) -> Self {
        if let Ok(mut state) = held.usage.lock() {
            state.decoded_bytes -= held.decoded;
            state.live_history_bytes -= held.history;
        }
        held.decoded = 0; held.history = 0;
        held
    }
}
'''
 budget.write_text(original);p.write_text(s);sys.exit(0)
assert s.count(a)==1,(m,s.count(a));p.write_text(s.replace(a,b,1))
PY
    for result in red restored; do
        if [[ "$result" == restored ]]; then
            cp "$evidence/$mutation.before.rs" "$file"
            if [[ "$mutation" == decoded-lease ]]; then cp "$evidence/decoded-lease-budget.before.rs" crates/kinewright-project/src/storage_budget.rs; fi
        fi
        while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do free -g; sleep 300; done
        log="$evidence/$mutation-$result.log"
        { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$file" crates/kinewright-project/src/storage_budget.rs; } > "$log"
        set +e
        nice -n 10 cargo test -p kinewright-project --features test-util --test ja_s1_storage "bounded_store::$test" -- --exact >> "$log" 2>&1
        status=$?
        set -e
        if [[ "$result" == red ]]; then [[ "$status" -ne 0 ]]; rg '^test result: FAILED\. 0 passed; 1 failed;' "$log";
        else [[ "$status" -eq 0 ]]; rg '^test result: ok\. 1 passed; 0 failed;' "$log"; fi
        nice -n 19 python3 -I - "$log" <<'PY'
import sys,re,hashlib
from pathlib import Path
p=Path(sys.argv[1]);s=p.read_text();m=re.search(r'Running tests/ja_s1_storage.rs \(([^)]+)\)',s);assert m
with Path(m.group(1)).open('rb') as f:digest=hashlib.file_digest(f,'sha256').hexdigest()
with p.open('a') as f:f.write(f'\nbinary_sha256: {digest} {m.group(1)}\n')
PY
        gzip "$log"
    done
    rm "$evidence/$mutation.before.rs"
    if [[ "$mutation" == decoded-lease ]]; then rm "$evidence/decoded-lease-budget.before.rs"; fi
    trap - EXIT
done
