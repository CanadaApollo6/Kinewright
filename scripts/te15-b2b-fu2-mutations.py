#!/usr/bin/env python3
"""Exactly two lead-approved B2b follow-up-2 pairs, and no retained registry."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'target/te1-5/b2b-fu2/pairs'
G = 'crates/kinewright-agent/src/te1_eval/grant.rs'
C = 'crates/kinewright-agent/src/te1_eval/checks.rs'
PAIRS = [
    ('publication-replay', C,
     'let anchor = calls.iter().enumerate().find_map(',
     'let anchor = calls.iter().enumerate().rev().find_map(',
     'te1_eval::checks::tests::te15_b2b_fu2_publication_replay',
     'FU2_PUBLICATION_REPLAY_CANNOT_RESET_BOUNDARY'),
    ('guardian-replacement', G,
     '        verify_guardian_hash(&self.grant, guardian)',
     '        let _ = guardian;\n        Ok(())',
     'te1_eval::grant::tests::te15_b2b_fu2_guardian_replaced_after_hash',
     'FU2_REPLACED_GUARDIAN_REFUSED'),
]
active = None

def stop():
    if active is not None and active.poll() is None:
        os.killpg(active.pid, signal.SIGTERM)
        try:
            active.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(active.pid, signal.SIGKILL)
            active.wait()

def interrupted(sig, _):
    raise InterruptedError(sig)

for sig in (signal.SIGTERM, signal.SIGINT):
    signal.signal(sig, interrupted)

def run(label, witness, marker, red):
    global active
    log = OUT / (label+'.log')
    cmd = ['bash', '-c', 'source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo test -j4 -p kinewright-agent --features te1-eval --lib "$1" -- --exact --nocapture', 'pair', witness]
    with log.open('w') as output:
        active = subprocess.Popen(cmd, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        (OUT/'active.json').write_text(json.dumps({'pid':active.pid,'label':label})+'\n')
        began = None
        try:
            while active.poll() is None:
                # The shared-lock queue is not an active build/test deadline.
                if began is None and re.search(r'^\s*(Compiling|Finished|Checking|Running|Blocking)', log.read_text(), re.M):
                    began = time.monotonic()
                if began is not None and time.monotonic() - began > 900:
                    raise TimeoutError(label)
                time.sleep(1)
            code = active.wait()
        finally:
            stop()
            active = None
            (OUT/'active.json').unlink(missing_ok=True)
    output = log.read_text()
    summaries = re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;', output, re.M)
    assert summaries == [('FAILED','0','1') if red else ('ok','1','0')], (label, summaries)
    assert (code != 0) == red and (not red or marker in output), label
    print(label, code, summaries[-1], flush=True)
    return {'exit':code,'summary':summaries[-1]}

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    os.environ.update({'TMPDIR':str(ROOT/'target/t'),'LOCALAPPDATA':str(ROOT/'target/l')})
    rows = []
    for name, source, before, after, witness, marker in PAIRS:
        path = ROOT/source
        original = path.read_bytes()
        sha = hashlib.sha256(original).hexdigest()
        text = original.decode()
        assert text.count(before) == 1, name
        try:
            path.write_text(text.replace(before, after))
            red = run(name+'-red', witness, marker, True)
        finally:
            stop()
            path.write_bytes(original)
        green = run(name+'-green', witness, marker, False)
        assert hashlib.sha256(path.read_bytes()).hexdigest() == sha
        rows.append({'name':name,'source':source,'original_sha256':sha,'restored_sha256':sha,'red':red,'green':green})
        (OUT/'results.json').write_text(json.dumps(rows, indent=2)+'\n')
    assert len(rows) == 2
    print('Exactly two pairs complete; all source bytes restored', flush=True)

if __name__ == '__main__':
    main()
