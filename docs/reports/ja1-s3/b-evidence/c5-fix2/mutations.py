"""Owner-slot guard and the fourteen prior C5 pairs; restore exact source."""
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
VOICE = Path('crates/kinewright-project/src/voice_store.rs')
GATE = 'docs/reports/ja1-s3/b-evidence/c5-fix2/gate.sh'

def digest(data):
    return hashlib.sha256(data).hexdigest()

def app_test(name):
    return ['test', '-p', 'kinewright-app', '--bin', 'kinewright-app',
            f'recording::feedback_capture::tests::{name}', '--', '--exact', '--nocapture']

def cases():
    old = runpy.run_path(str(ROOT / 'docs/reports/ja1-s3/b-evidence/c5/mutations.py'))['CASES']
    source = (ROOT / VOICE).read_text()
    stale_start = source.index('        if &self.voice != expected {')
    stale_end = source.index('        Ok(())', stale_start)
    stale = source[stale_start:stale_end]
    publish_start = source.index('    fn publish_asr(')
    start = source.index('            let asr = evidence(', publish_start)
    end = source.index(')?;', start) + 3
    admission = source[start:end]
    refusal_start = source.index('        if result', source.index('    fn write_voice('))
    refusal_end = source.index('        result\n', refusal_start)
    refusal = source[refusal_start:refusal_end]
    slot_start = source.index('    pub fn voice_note(')
    slot_end = source.index('    /// Try every slot', slot_start)
    slot = source[slot_start:slot_end]
    independent = slot.replace('slots.get(note)', 'slots.get(&NoteId::fresh()?)', 1).replace('slots.insert(note.clone(),', 'slots.insert(NoteId::fresh()?,', 1)
    copy_start = source.index('        if owner.project_id() != &self.project', source.index('    pub(crate) fn accept_asr('))
    copy_end = source.index('        transcript', copy_start)
    copy_guard = source[copy_start:copy_end]
    old = [(name, VOICE, copy_guard, '', command) if name == 'project-copy-context'
           else (name, path, before, after, command) for name, path, before, after, command in old]
    return [
        ('owner-note-slot', VOICE, slot, independent, app_test('s3_voice_fix2_owner_slot_blocks_competing_handle')),
        ('pending-choice-pin', VOICE, 'voice.chosen_transcript_revision.iter().cloned().collect()', 'std::iter::empty::<ObjectRef>().collect()', app_test('s3_voice_fix2_any_handle_retries_refused_choice')),
        ('stale-transcript-choice', VOICE, stale, '', app_test('s3_voice_fix1_stale_transcript_choice')),
        ('asr-admission-retention', VOICE, admission, admission[:-2] + '.inspect_err(|_| self.pending_asr = None)?;', app_test('s3_voice_fix1_asr_admission_retry')),
        ('asr-pending-pin', VOICE, '_pin: asr.into_pin(),', '_pin: owner.store().pin(&[])? ,', app_test('s3_voice_fix1_asr_refusal_collection_retry')),
        ('asr-definite-refusal-rebase', VOICE, refusal, '', app_test('s3_voice_fix1_asr_refusal_collection_retry')),
        *old,
    ]

def run(name, phase, command):
    driver = OUT / f'{name}-{phase}-driver.log'
    with driver.open('w') as log:
        result = subprocess.run(['bash', GATE, 'c5-fix2/mutations', f'{name}-{phase}', *command],
                                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    log = OUT / f'{name}-{phase}.log.gz'
    body = gzip.open(log, 'rt', errors='replace').read() if log.exists() else ''
    return result.returncode, body, digest(log.read_bytes()) if log.exists() else None

def main():
    for label in ['build', 'clippy', 'fmt', 'app', 'project', 'project-test-util', 'media', 'recording']:
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
    assert len(summary) == 16
    print('16 literal red/restored-green pairs completed (2 new + 14 prior reruns)', flush=True)

if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    main()
