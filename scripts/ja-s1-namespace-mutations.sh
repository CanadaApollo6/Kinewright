#!/usr/bin/env bash
# Targeted namespace mutations, each with its own literal property witness.
set -euo pipefail
cd "$(dirname "$0")/.."
evidence="${1:?absolute evidence directory}"
mkdir -p "$evidence"
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 TMPDIR="$PWD/target/ja-s1-tmp"
for mutation in alias release root-sync ancestors fallback paths root-file object-file object-directory; do
    case "$mutation" in
        alias) file=crates/kinewright-project/src/namespace/linux.rs; test=namespace_remains_bound_after_alias_swap ;;
        release) file=crates/kinewright-project/src/lockfile.rs; test=namespace_release_preserves_replaced_lock ;;
        root-file|object-file|object-directory) file=crates/kinewright-project/src/namespace/linux.rs; test=new_ancestor_syncs_precede_root_publication ;;
        root-sync) file=crates/kinewright-project/src/namespace/linux.rs; test=new_ancestor_syncs_precede_root_publication ;;
        fallback) file=crates/kinewright-project/src/namespace/linux.rs; test=strict_replace_refusal_preserves_exact_root ;;
        ancestors) file=crates/kinewright-project/src/namespace/linux.rs; test=new_ancestor_syncs_precede_root_publication ;;
        paths) file=crates/kinewright-project/src/namespace/mod.rs; test=portable_owned_paths_refuse_nonportable_spellings ;;
    esac
    cp "$file" "$evidence/$mutation.before.rs"
    trap 'cp "$evidence/$mutation.before.rs" "$file"' EXIT
    while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do free -g; sleep 300; done
    nice -n 19 python3 -I - "$file" "$mutation" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1]);s=p.read_text();m=sys.argv[2]
if m=='alias':
 a='let root_name = resolved\n';b='let root_name = path\n'
elif m=='release':
 a='owner.claim_id == self.claim.claim_id && self.verify().is_ok()';b='owner.claim_id == self.claim.claim_id'
elif m=='root-sync':
 a='''        sync_directory(
            &namespace.parent,
            namespace.identity.resolved_root.parent().unwrap(),
        )
        .map_err(|_| BundleError::DurabilityUnknown)?;''';b=''
elif m=='ancestors':
 a='''            sync_directory(&child, &observed)?;
            sync_directory(&dir, observed.parent().unwrap())?;''';b=''
elif m=='root-file':
 a='        sync_file(&file, "root_file_sync", &temp_path)?;';b=''
elif m=='object-file':
 a='        sync_file(&file, "object_file_sync", path)?;';b=''
 assert s.count(a)==2;s=s.replace(a,b);p.write_text(s);sys.exit(0)
elif m=='object-directory':
 a='        sync_directory(&dir, path.parent().unwrap()).map_err(|_| BundleError::DurabilityUnknown)?;';b=''
 assert s.count(a)==2;s=s.replace(a,b);p.write_text(s);sys.exit(0)
elif m=='fallback':
 a='''            return Err(BundleError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "S1 injected rename refusal",
            )));'''
 b='''            let mut target = File::from(fs::openat(&namespace.parent, &namespace.root_name,
                OFlags::WRONLY | OFlags::TRUNC | OFlags::NOFOLLOW, Mode::empty())?);
            target.write_all(bytes)?;
            note("in_place_fallback", &namespace.identity.resolved_root);
'''+a
else:
 a='''        if spelling.is_empty()''';b='''        if false && spelling.is_empty()'''
 # Disable the whole spelling condition, not only its first term.
 start=s.index(a);end=s.index('        for component',start)
 s=s[:start]+s[start:end].replace('if spelling.is_empty()', 'if false && (spelling.is_empty()',1).replace('\n        {','\n        ) {',1)+s[end:]
 p.write_text(s);sys.exit(0)
assert s.count(a)==1,(m,s.count(a));p.write_text(s.replace(a,b,1))
PY
    for result in red restored; do
        if [[ "$result" == restored ]]; then cp "$evidence/$mutation.before.rs" "$file"; fi
        while [[ "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do free -g; sleep 300; done
        log="$evidence/$mutation-$result.log"
        { git rev-parse HEAD; git diff --binary | sha256sum; sha256sum "$file"; } > "$log"
        set +e
        nice -n 10 cargo test -p kinewright-project --features test-util --test ja_s1_storage "$test" -- --exact >> "$log" 2>&1
        status=$?
        set -e
        if [[ "$result" == red ]]; then [[ "$status" -ne 0 ]]; rg '^test result: FAILED\. 0 passed; 1 failed;' "$log";
        else [[ "$status" -eq 0 ]]; rg '^test result: ok\. 1 passed; 0 failed;' "$log"; fi
        nice -n 19 python3 -I - "$log" <<'PY'
import sys,re,hashlib
from pathlib import Path
p=Path(sys.argv[1]);s=p.read_text();m=re.search(r'Running tests/ja_s1_storage.rs \(([^)]+)\)',s);assert m
with Path(m.group(1)).open('rb') as f: digest=hashlib.file_digest(f,'sha256').hexdigest()
with p.open('a') as f:f.write(f'\nbinary_sha256: {digest} {m.group(1)}\n')
PY
        gzip "$log"
    done
    rm "$evidence/$mutation.before.rs"
    trap - EXIT
done
