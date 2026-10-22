#!/usr/bin/env bash
# Build a sealed max-2 predecessor in an isolated detached checkout, before tests.
set -euo pipefail
repo="$(git rev-parse --show-toplevel)"
base=076a96c922b2f05984df4bb839ad7cf0900e6183
scratch="$repo/target/ja-s1-predecessor"
source_tree="$scratch/source"
sealed="$scratch/sealed"
evidence="${1:-$scratch/evidence}"
mkdir -p "$sealed" "$evidence"
controls="$(mktemp -d "$evidence/controls-XXXXXXXX")"
finish() {
    local status=$?
    if [[ "$status" -ne 0 ]]; then
        for log in "$evidence"/*.log; do [[ ! -f "$log" ]] || tail -n 80 "$log"; done
        if [[ "${OS:-}" == Windows_NT ]]; then
            python -I "$repo/scripts/ja-s1-control-diagnostics.py" "$controls" || true
        else
            nice -n 19 python3 -I "$repo/scripts/ja-s1-control-diagnostics.py" "$controls" || true
        fi
    fi
    find "$evidence" -maxdepth 1 -type f -name '*.log' -exec gzip -f {} +
    return "$status"
}
trap finish EXIT
if ! git cat-file -e "$base^{commit}" 2>/dev/null; then
    git fetch --no-tags origin "$base"
fi
if [[ ! -d "$source_tree" ]]; then git worktree add --detach "$source_tree" "$base"; fi
[[ "$(git -C "$source_tree" rev-parse HEAD)" == "$base" ]]
[[ -z "$(git -C "$source_tree" status --porcelain --untracked-files=no)" ]]
cat > "$scratch/predecessor-format.rs" <<'RUST'
use kinewright_project::{ProjectFile, can_overwrite_save, load_document};
#[test]
fn pinned_max2_format_controls() {
    assert_eq!(kinewright_core::PROJECT_FORMAT_VERSION,2);
    let fixture=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kinewright-agent/tests/fixtures/ja");
    let temp=std::env::temp_dir().join(format!("kinewright-ja-s1-predecessor-{}",std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let studio=temp.join("studio.kinewright");
    let bytes=std::fs::read(fixture.join("unsupported-v3-studio.kinewright")).unwrap();
    std::fs::write(&studio,&bytes).unwrap();
    let (_,version,_)=load_document(&studio).unwrap();assert_eq!(version,3);assert!(!can_overwrite_save(version));
    assert_eq!(std::fs::read(&studio).unwrap(),bytes);
    let mut raw:serde_json::Value=serde_json::from_slice(&std::fs::read(fixture.join("writer-v2-adjustment.kinewright")).unwrap()).unwrap();
    raw["format_version"]=serde_json::json!(3);
    raw["tracks"][0]["clips"][0]["future_blend"]=serde_json::json!("screen");
    let source=serde_json::to_vec(&raw).unwrap();let path=temp.join("blend.kinewright");std::fs::write(&path,&source).unwrap();
    let file:ProjectFile=serde_json::from_slice(&source).unwrap();
    let (opened,version,_)=load_document(&path).unwrap();assert_eq!(opened,file.document);assert_eq!(version,3);assert!(!can_overwrite_save(version));
    let projected=serde_json::to_value(&opened).unwrap();assert!(projected["tracks"][0]["clips"][0].get("future_blend").is_none());
    assert_eq!(std::fs::read(&path).unwrap(),source);
    std::fs::remove_dir_all(&temp).unwrap();
}
RUST
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4
if [[ "${OS:-}" == Windows_NT ]]; then
    executable=kinewright-ja-baseline.exe
    # The provisioner imports MSVC/FFmpeg into this PowerShell process. Every native exit is checked.
    export JA_S1_PREDECESSOR_TREE="$(cygpath -w "$source_tree")"
    export JA_S1_PREDECESSOR_TARGET="$(cygpath -w "$scratch/build")"
    export JA_S1_PREDECESSOR_SEALED="$(cygpath -w "$sealed")"
    export JA_S1_PREDECESSOR_OBSERVER="$(cygpath -w "$scratch/predecessor-format.rs")"
    export JA_S1_PREDECESSOR_CONTROLS="$(cygpath -w "$controls")"
    cat > "$scratch/build-windows.ps1" <<'PS'
$ErrorActionPreference='Stop'
Set-Location -LiteralPath $env:JA_S1_PREDECESSOR_TREE
& .\scripts\setup-ffmpeg.ps1 -SkipBuildCachePrune
$env:CARGO_TARGET_DIR=$env:JA_S1_PREDECESSOR_TARGET
cargo build -p kinewright-agent --features eval-harness --bin kinewright-ja-baseline
if ($LASTEXITCODE -ne 0) { throw "predecessor build exited $LASTEXITCODE" }
Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'debug\kinewright-ja-baseline.exe') -Destination $env:JA_S1_PREDECESSOR_SEALED
New-Item -ItemType Directory -Path 'crates\kinewright-project\tests' -Force | Out-Null
$observer='crates\kinewright-project\tests\ja_s1_predecessor_format.rs'
Copy-Item -LiteralPath $env:JA_S1_PREDECESSOR_OBSERVER -Destination $observer
try {
    cargo test -p kinewright-project --test ja_s1_predecessor_format --no-run --message-format=json > (Join-Path $env:CARGO_TARGET_DIR 'max2-build.jsonl')
    if ($LASTEXITCODE -ne 0) { throw "max-2 observer build exited $LASTEXITCODE" }
    $testBinary=Get-Content (Join-Path $env:CARGO_TARGET_DIR 'max2-build.jsonl') | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'ja_s1_predecessor_format' -and $_.executable } | Select-Object -Last 1 -ExpandProperty executable
    if (-not $testBinary) { throw 'max-2 observer executable missing' }
    Copy-Item -LiteralPath $testBinary -Destination (Join-Path $env:JA_S1_PREDECESSOR_SEALED 'max2-control.exe')
} finally { Remove-Item -LiteralPath $observer }

PS
    pwsh -NoProfile -File "$scratch/build-windows.ps1" > "$evidence/build.log" 2>&1
    # Run controls in a provisioned process too; DLL/import paths are never guessed from Linux.
    cat > "$scratch/run-windows.ps1" <<'PS'
$ErrorActionPreference='Stop'
Set-Location -LiteralPath $env:JA_S1_PREDECESSOR_TREE
& .\scripts\setup-ffmpeg.ps1 -SkipBuildCachePrune
$control=Join-Path $env:JA_S1_PREDECESSOR_SEALED 'max2-control.exe'
& $control --exact pinned_max2_format_controls
if ($LASTEXITCODE -ne 0) { throw "max-2 observer exited $LASTEXITCODE" }
$binary=Join-Path $env:JA_S1_PREDECESSOR_SEALED 'kinewright-ja-baseline.exe'
foreach ($name in @('format-corpus','history-loss','reply-retry','legacy-io')) {
    $expected=if ($name -eq 'format-corpus') {0} else {1}
    & $binary --mode baseline --fixture "crates\kinewright-agent\tests\fixtures\ja\$name.json" --output (Join-Path $env:JA_S1_PREDECESSOR_CONTROLS $name)
    if ($LASTEXITCODE -ne $expected) { throw "$name exited $LASTEXITCODE, expected $expected" }
}
PS
    pwsh -NoProfile -File "$scratch/run-windows.ps1" > "$evidence/controls.log" 2>&1
else
    executable=kinewright-ja-baseline
    wait_started=$SECONDS
    while [[ -z "${CI:-}" && "$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)" -lt 6291456 ]]; do
        if (( SECONDS - wait_started >= 600 )); then echo "6 GiB memory prerequisite unavailable after 600 seconds" >&2; exit 1; fi
        free -g
        sleep 30
    done
    (
        cd "$source_tree"
        # Reuse only the pinned native distribution; compilation has its own source/target identities.
        if [[ ! -e third_party/ffmpeg && -f "$repo/third_party/ffmpeg/.archive-sha256" ]]; then
            mkdir -p third_party
            ln -s "$repo/third_party/ffmpeg" third_party/ffmpeg
        fi
        source scripts/setup-ffmpeg.sh
        export TMPDIR="$scratch/tmp" CARGO_TARGET_DIR="$scratch/build"
        mkdir -p "$TMPDIR"
        nice -n 10 cargo build -p kinewright-agent --features eval-harness --bin kinewright-ja-baseline
        cp "$CARGO_TARGET_DIR/debug/$executable" "$sealed/$executable"
        mkdir -p crates/kinewright-project/tests
        observer=crates/kinewright-project/tests/ja_s1_predecessor_format.rs
        cp "$scratch/predecessor-format.rs" "$observer"
        trap 'rm -f "$observer"' EXIT
        nice -n 10 cargo test -p kinewright-project --test ja_s1_predecessor_format --no-run --message-format=json > "$scratch/max2-build.jsonl"
        test_binary="$(nice -n 19 python3 -I -c 'import json,sys; rows=[json.loads(x) for x in open(sys.argv[1])]; print([r["executable"] for r in rows if r.get("reason")=="compiler-artifact" and r.get("target",{}).get("name")=="ja_s1_predecessor_format" and r.get("executable")][-1])' "$scratch/max2-build.jsonl")"
        cp "$test_binary" "$sealed/max2-control"
        rm "$observer"
        trap - EXIT
        nice -n 10 "$sealed/max2-control" --exact pinned_max2_format_controls

        for fixture in format-corpus history-loss reply-retry legacy-io; do
            expected=1; if [[ "$fixture" == format-corpus ]]; then expected=0; fi
            set +e
            nice -n 10 "$sealed/$executable" --mode baseline --fixture "crates/kinewright-agent/tests/fixtures/ja/$fixture.json" --output "$controls/$fixture"
            result=$?
            set -e
            if [[ "$result" -ne "$expected" ]]; then
                echo "$fixture exited $result, expected $expected" >&2
                exit 1
            fi
        done
    ) > "$evidence/build-controls.log" 2>&1
fi
# Bind the sealed artifact and the original frozen fixture reports to actual clean predecessor source.
if [[ "${OS:-}" == Windows_NT ]]; then python_runner=(python); else python_runner=(nice -n 19 python3); fi
"${python_runner[@]}" -I - "$source_tree" "$sealed/$executable" "$sealed" "$base" "$controls" <<'PY'
import hashlib,json,pathlib,platform,subprocess,sys
source,binary,sealed=map(pathlib.Path,sys.argv[1:4])
base=sys.argv[4]
controls=pathlib.Path(sys.argv[5])
assert subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip()==base
assert not subprocess.check_output(['git','-C',str(source),'status','--porcelain','--untracked-files=no'],text=True).strip()
hashfile=lambda p:hashlib.file_digest(open(p,'rb'),'sha256').hexdigest()
control=sealed/('max2-control.exe' if platform.system()=='Windows' else 'max2-control')
reports={}
for name in ['format-corpus','history-loss','reply-retry','legacy-io']:
    run=json.loads((controls/name/'run.json').read_text())
    assert run['source_head']==base
    assert run['binary_sha256']==hashfile(binary)
    reports[name]={'run_sha256':hashfile(controls/name/'run.json'),'fixture_sha256':run['fixture_sha256'],'output':str((controls/name).resolve())}
manifest={'source_root':str(source.resolve()),'source_head':base,'binary':str(binary.resolve()),'binary_sha256':hashfile(binary),'source_clean':True,'os':platform.platform(),
          'compiler':subprocess.check_output(['rustc','-Vv'],text=True),'reports':reports,
          'max2_binary':str(control.resolve()),'max2_binary_sha256':hashfile(control),'observer_source_sha256':hashfile(source.parent/'predecessor-format.rs')}
(sealed/'identity.json').write_text(json.dumps(manifest,indent=2)+'\n')
PY
if [[ -n "${GITHUB_ENV:-}" ]]; then
    if [[ "${OS:-}" == Windows_NT ]]; then value="$(cygpath -w "$sealed/$executable")"; else value="$sealed/$executable"; fi
    echo "JA_S1_PREDECESSOR_BINARY=$value" >> "$GITHUB_ENV"
fi
find "$evidence" -maxdepth 1 -type f -name '*.log' -exec gzip -f {} +
echo "Sealed predecessor: $sealed/$executable"
