"""Run only the C5 retention and late-result witnesses. Restore exact source per pair."""
from __future__ import annotations
import gzip
import hashlib
import json
import pathlib
import signal
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[5]
OUT = ROOT / 'docs/reports/ja1-s3/b-evidence/c5/mutations'
MEDIA = pathlib.Path('crates/kinewright-media/src/voice_note.rs')
APP = pathlib.Path('crates/kinewright-app/src/recording.rs')
MEDIA_TEST = ['test', '-p', 'kinewright-media', '--lib', 'voice_note::tests::s3_voice_job_identity', '--', '--exact', '--nocapture']
def app_test(name):
    return ['test', '-p', 'kinewright-app', '--bin', 'kinewright-app', f'recording::feedback_capture::tests::{name}', '--', '--exact', '--nocapture']
CASES = []
for name, field in [('project', 'project'), ('note', 'note'), ('id', 'job'), ('audio-hash', 'audio_sha256'), ('transcript-revision', 'transcript_revision')]:
    CASES.append((f'job-{name}', MEDIA, f'            || self.key.{field} != key.{field}\n', '', MEDIA_TEST))
CASES += [
    ('job-live-revision', MEDIA, '            || &self.key != current\n', '', MEDIA_TEST),
    ('job-cancel', MEDIA, '            || self.cancellation.is_cancelled()\n', '', MEDIA_TEST),
    ('audio-before-asr', APP, '                save_voice(owner, envelope, &voice, &mut self.pending_write)?;', '                let _ = (&voice, &envelope);', app_test('s3_voice_audio_retention')),
    ('commentary-log-retention', APP, '                    playhead_log_ref: Some(log.reference.clone()),', '                    playhead_log_ref: None,', app_test('s3_commentary_retention')),
    ('project-copy-context', APP, '            if owner.project_id() != &self.project {\n                return Ok(false);\n            }\n', '', app_test('s3_voice_job_identity')),
]
def digest(data):
    return hashlib.sha256(data).hexdigest()
def run(name, phase, command):
    with (OUT / f'{name}-{phase}-driver.log').open('w') as log:
        result = subprocess.run(['bash', 'docs/reports/ja1-s3/b-evidence/c5/gate.sh', 'c5/mutations', f'{name}-{phase}', *command], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    path = OUT / f'{name}-{phase}.log.gz'
    body = gzip.open(path, 'rt', errors='replace').read() if path.exists() else ''
    return result.returncode, body

def main():
    for label in ['build-final', 'clippy-final', 'app', 'fmt', 'media-readback-rerun', 'media-remaining', 'media-doc']:
        path = OUT.parent / f'{label}.exit'
        if not path.exists() or path.read_text().strip() != '0':
            raise SystemExit(f'Baseline gate is not green: {label}')
    media = gzip.open(OUT.parent / 'media-final.log.gz', 'rt', errors='replace').read()
    failed = [line for line in media.splitlines() if line.startswith('test ') and line.endswith(' ... FAILED')]
    if failed != ['test compositor::ledger_probes::rev3_concurrent_readbacks_never_refuse ... FAILED']:
        raise SystemExit(f'Unexpected full media result: {failed}')
    for witness in ['s3_voice_job_identity', 's3_commentary_playhead_log', 's3_commentary_spoken_timecodes', 's3_voice_missing_model_silence_and_sealed_audio', 's3_voice_inference_admission_is_bounded']:
        if f'test voice_note::tests::{witness} ... ok' not in media:
            raise SystemExit(f'Baseline voice witness not green: {witness}')
    for line in (OUT.parent / 'source.sha256').read_text().splitlines():
        expected, relative = line.split('  ', 1)
        if digest((ROOT / relative).read_bytes()) != expected:
            raise SystemExit(f'Baseline source changed: {relative}')
    OUT.mkdir(parents=True, exist_ok=True)
    summary = []
    for name, relative, before, after, command in CASES:
        source = ROOT / relative
        original = source.read_bytes()
        text = original.decode()
        count = text.count(before)
        if count != 1 and not (name == 'audio-before-asr' and count == 2):
            raise AssertionError((name, 'selector count', count))
        mutated = text.replace(before, after, 1).encode()
        record = {'case': name, 'source': str(relative), 'original_sha256': digest(original), 'mutated_sha256': digest(mutated), 'command': command}
        try:
            source.write_bytes(mutated)
            red, red_log = run(name, 'red', command)
            record['red_exit'] = red
            record['runtime_red'] = red == 101 and 'test result: FAILED.' in red_log and '1 failed;' in red_log
            if not record['runtime_red']:
                raise AssertionError((name, 'not a runtime witness red', red))
        finally:
            source.write_bytes(original)
            record['restored_exactly'] = source.read_bytes() == original
            (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        green, green_log = run(name, 'green', command)
        record['green_exit'] = green
        record['runtime_green'] = green == 0 and 'test result: ok. 1 passed; 0 failed;' in green_log
        record['restored_exactly'] = source.read_bytes() == original
        (OUT / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        summary.append(record)
        (OUT / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        if not record['runtime_green'] or not record['restored_exactly']:
            raise AssertionError((name, 'restored green failed', green))
        print(f'{name}: runtime red {red}, restored green {green}, exact source restored', flush=True)
    print(f'{len(summary)} literal red/restored-green pairs completed', flush=True)

if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    main()
