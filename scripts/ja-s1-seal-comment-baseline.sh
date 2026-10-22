#!/usr/bin/env bash
# Run only after committing the gated pre-receipt source; never rebuild in this seal step.
set -euo pipefail
cd "$(dirname "$0")/.."
evidence="${1:?absolute evidence directory}"
expected="${2:?clean commit-4 source SHA}"
[[ "$evidence" = /* && "$(git rev-parse HEAD)" == "$expected" ]]
[[ -z "$(git status --porcelain)" ]]
source scripts/setup-ffmpeg.sh
sealed="$PWD/target/ja-s1-comment-baseline/sealed"
mkdir -p "$sealed" "$evidence"
binary="$sealed/kinewright-ja-baseline"
if [[ -e "$binary" ]]; then
    cmp target/debug/kinewright-ja-baseline "$binary"
else
    cp target/debug/kinewright-ja-baseline "$binary"
fi
run="$evidence/run-$(date -u +%Y%m%dT%H%M%S)"
[[ ! -e "$run" ]]
log="$evidence/seal-run-$(date -u +%Y%m%dT%H%M%S).log"
{
    git rev-parse HEAD
    sha256sum "$binary"
} > "$log"
set +e
nice -n 10 "$binary" --mode baseline --fixture "$PWD/crates/kinewright-agent/tests/fixtures/ja/comment-receipt.json" --output "$run" --scenario comment-baseline >> "$log" 2>&1
status=$?
set -e
[[ "$status" -eq 1 ]]
nice -n 19 python3 -I - "$expected" "$binary" "$run" "$sealed" <<'PY'
import hashlib,json,pathlib,subprocess,sys
head,binary,run,sealed=sys.argv[1],*map(pathlib.Path,sys.argv[2:])
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
header=json.loads((run/'run.json').read_text());summary=json.loads((run/'summary.json').read_text())
assert header['source_head']==head and header['binary_sha256']==hashfile(binary)
assert header['dirty_diff']['byte_length']==0
assert header['scenario_selection']=='comment-baseline'
fixture=pathlib.Path('crates/kinewright-agent/tests/fixtures/ja/comment-receipt.json')
assert hashfile(fixture)==header['fixture_sha256']
for refs in [header['inputs'],header['oracles']]:
 for ref in refs.values():
  p=fixture.parent/ref['relative_file'];assert hashfile(p)==ref['sha256'] and p.stat().st_size==ref['byte_length']
controls=[a for a in summary['assertions'] if a['evidence_class']=='control']
assert controls and all(a['passed'] for a in controls)
missing=next(a for a in summary['assertions'] if a['id']=='durable-comment-result-binding')
detail_path=run/missing['artifact']['relative_file']
assert hashfile(detail_path)==missing['artifact']['sha256']
detail=json.loads(detail_path.read_text())
assert not missing['passed'] and missing['disposition']=='baseline_red' and detail['actual'] is False and detail['expected'] is True
observation=json.loads((run/'artifacts/observation-17.json').read_text())
identity={'source_head':head,'source_tree':subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip(),
 'source_clean':True,'binary':str(binary),'binary_sha256':hashfile(binary),'fixture_sha256':header['fixture_sha256'],
 'inputs':header['inputs'],'oracles':header['oracles'],'os':header['os'],'compiler':header['compiler'],
 'backend':observation['backend'],'scenario_selection':header['scenario_selection'],'controls':controls,'literal_missing_result':detail,
 'run_directory':str(run),'run_sha256':hashfile(run/'run.json'),'summary_sha256':hashfile(run/'summary.json'),
 'later_receipt_scenarios':'NOT RUN on this pre-receipt source; preserved in the unchanged fixture'}
for p in [sealed/'identity.json',run.parent/'seal.json']:
 if p.exists():assert json.loads(p.read_text())['source_head']==head
 p.write_text(json.dumps(identity,indent=2)+'\n')
print('Sealed comment baseline:',head,identity['binary_sha256'])
PY
gzip "$log"
