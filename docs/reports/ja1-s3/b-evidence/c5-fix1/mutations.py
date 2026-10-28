"""Focused C5 fix guards and the original ten C5 pairs; restore exact source."""
from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path
import runpy
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent / 'mutations'
APP = Path('crates/kinewright-app/src/recording.rs')
GATE = 'docs/reports/ja1-s3/b-evidence/c5-fix1/gate.sh'

def digest(data):
    return hashlib.sha256(data).hexdigest()

def app_test(name):
    return ['test', '-p', 'kinewright-app', '--bin', 'kinewright-app',
            f'recording::feedback_capture::tests::{name}', '--', '--exact', '--nocapture']

def cases():
    old = runpy.run_path(str(ROOT / 'docs/reports/ja1-s3/b-evidence/c5/mutations.py'))['CASES']
    source = (ROOT / APP).read_text()
    start = source.index('        pub(crate) fn choose_transcript(')
    end = source.index('        pub(crate) fn accept_asr(', start)
    choice = source[start:end]
    guard = '                || owner.feedback_index()?.notes.get(&self.voice.note_id) != Some(&self.voice)\n'
    assert choice.count(guard) == 1
    publish_start = source.index('        fn publish_asr(')
    start = source.index('                let asr = evidence(', publish_start)
    end = source.index(')?;', start) + 3
    admission = source[start:end]
    refusal = '                    self.pending_write = None;\n'
    assert source[publish_start:].count(refusal) == 1
    return [
        ('stale-transcript-choice', APP, choice, choice.replace(guard, '', 1), app_test('s3_voice_fix1_stale_transcript_choice')),
        ('asr-admission-retention', APP, admission, admission[:-2] + '.inspect_err(|_| self.pending_asr = None)?;', app_test('s3_voice_fix1_asr_admission_retry')),
        ('asr-pending-pin', APP, '_pin: asr.into_pin(),', '_pin: owner.store().pin(&[])? ,', app_test('s3_voice_fix1_asr_refusal_collection_retry')),
        ('asr-definite-refusal-rebase', APP, refusal, '', app_test('s3_voice_fix1_asr_refusal_collection_retry')),
        *old,
    ]

def run(name, phase, command):
    driver = OUT / f'{name}-{phase}-driver.log'
    with driver.open('w') as log:
        result = subprocess.run(['bash', GATE, 'c5-fix1/mutations', f'{name}-{phase}', *command],
                                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    log = OUT / f'{name}-{phase}.log.gz'
    body = gzip.open(log, 'rt', errors='replace').read() if log.exists() else ''
    return result.returncode, body, digest(log.read_bytes()) if log.exists() else None

def main():
    for label in ['build-bound-final', 'clippy-bound-final', 'recording-bound-final', 'lineage-accepted',
                  'voice-bound-final', 'transcript', 'fmt-bound-final']:
        path = OUT.parent / f'{label}.exit'
        if not path.exists() or path.read_text().strip() != '0':
            raise SystemExit(f'Baseline gate is not green: {label}')
    for line in (OUT.parent / 'source.sha256').read_text().splitlines():
        expected, relative = line.split('  ', 1)
        if digest((ROOT / relative).read_bytes()) != expected:
            raise SystemExit(f'Baseline source changed: {relative}')
    OUT.mkdir(parents=True, exist_ok=True)
    summary = []
    for name, relative, before, after, command in cases():
        source = ROOT / relative
        original = source.read_bytes()
        text = original.decode()
        count = text.count(before)
        assert count == 1 or (name == 'audio-before-asr' and count == 2), (name, count)
        mutated = text.replace(before, after, 1).encode()
        record = {'case': name, 'source': str(relative), 'command': command,
                  'original_sha256': digest(original), 'mutated_sha256': digest(mutated)}
        try:
            source.write_bytes(mutated)
            code, body, log_sha = run(name, 'red', command)
            record.update(red_exit=code, red_log_sha256=log_sha,
                          runtime_red=code == 101 and 'test result: FAILED.' in body and '1 failed;' in body)
            assert record['runtime_red'], (name, 'not a runtime red', code)
        finally:
            source.write_bytes(original)
            record['restored_exactly'] = source.read_bytes() == original
            (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        code, body, log_sha = run(name, 'green', command)
        record.update(green_exit=code, green_log_sha256=log_sha,
                      runtime_green=code == 0 and 'test result: ok. 1 passed; 0 failed;' in body)
        record['restored_exactly'] = source.read_bytes() == original
        (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        summary.append(record)
        (OUT / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        assert record['runtime_green'] and record['restored_exactly'], (name, code)
        print(f'{name}: runtime red {record["red_exit"]}, restored green {code}, exact source restored', flush=True)
    assert len(summary) == 14
    print('14 literal red/restored-green pairs completed (4 new + 10 C5 reruns)', flush=True)

if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    main()
