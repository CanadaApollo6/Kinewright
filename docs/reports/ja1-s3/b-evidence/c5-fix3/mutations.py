"""Three data-loss invariant witnesses; mutate locally and restore exact source bytes."""
from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent / 'mutations'
VOICE = Path('crates/kinewright-project/src/voice_store.rs')
STUDIO = Path('crates/kinewright-project/src/studio_store.rs')
GATE = 'docs/reports/ja1-s3/b-evidence/c5-fix3/gate.sh'


def digest(data):
    return hashlib.sha256(data).hexdigest()


CASES = [
    ('slot-only-publisher', STUDIO,
     '        if matches!(update.change, MetadataChange::SaveVoice(_)) {\n'
     '            return crate::voice_store::metadata_command(self, envelope, scope, update);\n'
     '        }\n', '',
     's3_voice_fix3_direct_original_cannot_bypass_pending', 'S3_VOICE_SLOT_ONLY_PUBLISHER'),
    ('choice-cas', VOICE,
     'if voice.chosen_transcript_revision != expected_choice {',
     'if false && voice.chosen_transcript_revision != expected_choice {',
     's3_voice_fix3_choice_retry_preserves_newer_choice', 'S3_VOICE_CHOICE_RETRY_CAS'),
    ('completion-contention', VOICE,
     '        self.with_state(|state| state.accept_asr(owner, envelope, key, current, completion))',
     '        let mut consumed_before_lock = completion.take();\n'
     '        self.with_state(|state| state.accept_asr(owner, envelope, key, current, &mut consumed_before_lock))',
     's3_voice_fix3_contention_keeps_completed_asr', 'S3_VOICE_COMPLETION_RETAINED_ON_CONTENTION'),
]


def run(name, phase, test):
    command = ['test', '-p', 'kinewright-app', '--bin', 'kinewright-app',
               f'recording::feedback_capture::tests::{test}', '--', '--exact', '--nocapture']
    with (OUT / f'{name}-{phase}-driver.log').open('w') as log:
        result = subprocess.run(['bash', GATE, 'c5-fix3/mutations', f'{name}-{phase}', *command],
                                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False)
    path = OUT / f'{name}-{phase}.log.gz'
    body = gzip.open(path, 'rt', errors='replace').read() if path.exists() else ''
    return result.returncode, body, digest(path.read_bytes()) if path.exists() else None


def main():
    for label in ['build', 'clippy', 'fmt', 'app', 'project', 'project-test-util', 'media', 'workspace-fast']:
        assert (OUT.parent / f'{label}.exit').read_text().strip() == '0', label
    for line in (OUT.parent / 'source.sha256').read_text().splitlines():
        expected, relative = line.split('  ', 1)
        assert digest((ROOT / relative).read_bytes()) == expected, relative
    OUT.mkdir(parents=True, exist_ok=True)
    summary = []
    for name, relative, before, after, test, marker in CASES:
        path = ROOT / relative
        original = path.read_bytes()
        source = original.decode()
        assert source.count(before) == 1, name
        mutated = source.replace(before, after, 1).encode()
        record = {'case': name, 'source': str(relative), 'test': test, 'marker': marker,
                  'original_sha256': digest(original), 'mutated_sha256': digest(mutated)}
        try:
            path.write_bytes(mutated)
            code, body, log_sha = run(name, 'red', test)
            record.update(red_exit=code, red_log_sha256=log_sha,
                          runtime_red=code == 101 and 'test result: FAILED. 0 passed; 1 failed;' in body
                          and marker in body and 'panicked at' in body)
            assert record['runtime_red'], (name, code, body[-2000:])
        finally:
            path.write_bytes(original)
            record['restored_exactly'] = path.read_bytes() == original
            (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        code, body, log_sha = run(name, 'green', test)
        record.update(green_exit=code, green_log_sha256=log_sha,
                      runtime_green=code == 0 and 'test result: ok. 1 passed; 0 failed;' in body)
        record['restored_exactly'] = path.read_bytes() == original
        (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        summary.append(record)
        (OUT / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        assert record['runtime_green'] and record['restored_exactly'], name
        print(f'{name}: runtime red {record["red_exit"]}, restored green {code}', flush=True)


if __name__ == '__main__':
    main()
