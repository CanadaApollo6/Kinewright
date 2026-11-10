#!/usr/bin/env python3
"""Exactly the two lead-approved terminal reply follow-up pairs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'target/te1-5/merge-reply/pairs'
C = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'
PAIRS = [('terminal-observation',
  'crates/kinewright-agent/src/writer_host/task_mcp.rs',
  '        let Some(observer) = self.evaluation_observer.get() else {\n'
  '            return Ok(());\n'
  '        };',
  '        let Some(host) = self.host.upgrade() else {\n'
  '            return Ok(());\n'
  '        };\n'
  '        let observer = match self.lock_reply_owner(&host) {\n'
  '            Ok(host) => host.evaluation_observer.get().cloned(),\n'
  '            Err(error) => {\n'
  '                self.measurements.lock().map_err(|e| '
  'std::io::Error::other(e.to_string()))?.incomplete = true;\n'
  '                if self.endpoint.check_live().is_err() { return Ok(()); }\n'
  '                return Err(std::io::Error::other(error.to_string()));\n'
  '            }\n'
  '        };\n'
  '        let Some(observer) = observer else { return Ok(()); };',
  'writer_host::hooks_tests::tests::te15_b2b_kept_observer_after_publication',
  'TE1_MERGE_RETIRED_PUBLICATION_RECORDED'),
 ('retired-reply-size',
  'crates/kinewright-agent/src/writer_host/task_mcp.rs',
  '            // Check host before measurements to preserve the lock order.\n'
  '            // Contention at a retired endpoint is not evidence of detachment:\n'
  '            // its per-run measurement entry remains retained for accounting.\n'
  '            let incomplete = match self.host.upgrade() {\n'
  '                None => true,\n'
  '                Some(owner) => match self.lock_reply_owner(&owner) {\n'
  '                    Ok(host) => !host.hooks.runs.get(&authority.run).is_some_and(|run| {\n'
  '                        run.tasks.contains_key(&authority.task)\n'
  '                            && run\n'
  '                                .measurements\n'
  '                                .get(&authority.task)\n'
  '                                .is_some_and(|observer| Arc::ptr_eq(observer, '
  '&self.measurements))\n'
  '                    }),\n'
  '                    // A busy owner (or retirement while waiting) proves nothing\n'
  '                    // about attachment. Poison remains a measurement failure.\n'
  '                    Err(_) => owner.is_poisoned(),\n'
  '                },\n'
  '            };\n',
  "            // Retain this task's bounded observer even during host/run teardown.\n"
  '            // Check host first to preserve the host -> measurement lock order.\n'
  '            let incomplete = !self.host.upgrade().is_some_and(|owner| {\n'
  '                self.lock_reply_owner(&owner).is_ok_and(|host| {\n'
  '                    host.hooks.runs.get(&authority.run).is_some_and(|run| {\n'
  '                        run.tasks.contains_key(&authority.task)\n'
  '                            && run\n'
  '                                .measurements\n'
  '                                .get(&authority.task)\n'
  '                                .is_some_and(|observer| Arc::ptr_eq(observer, '
  '&self.measurements))\n'
  '                    })\n'
  '                })\n'
  '            });\n',
  'writer_host::hooks_tests::tests::te15_b2b_retired_reply_sizes_survive_contention',
  'TE1_RETIRED_SIZE_CONTENTION_COMPLETE')]
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
    cmd = ['bash', '-c', 'source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 nice -n 10 cargo test -j4 -p kinewright-agent --features te1-eval --lib "$1" -- --exact --nocapture', 'pair', witness]
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
    expected = ('FAILED','0','1') if red else ('ok','1','0')
    # This HOME-isolated witness reports the child failure inside its parent.
    assert summaries == [expected] * (2 if red else 1), (label, summaries)
    assert (code != 0) == red and (not red or marker in output), label
    print(label, code, summaries[-1], flush=True)
    return {'exit':code,'summary':summaries[-1]}

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    os.environ.update({'TMPDIR':str(ROOT/'target/t'),'LOCALAPPDATA':str(ROOT/'target/l')})
    parser = argparse.ArgumentParser()
    parser.add_argument('--only', choices=[p[0] for p in PAIRS])
    args = parser.parse_args()
    saved = OUT/'results.json'
    rows = json.loads(saved.read_text()) if args.only and saved.exists() else []
    for name, source, before, after, witness, marker in PAIRS:
        if args.only and name != args.only:
            continue
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
        rows = [row for row in rows if row['name'] != name]
        rows.append({'name':name,'source':source,'original_sha256':sha,'restored_sha256':sha,'red':red,'green':green})
        (OUT/'results.json').write_text(json.dumps(rows, indent=2)+'\n')
    assert (len(rows) == 2 if not args.only else args.only in [r['name'] for r in rows])
    print(f'{len(rows)} named pairs complete; all source bytes restored', flush=True)

if __name__ == '__main__':
    main()
