#!/usr/bin/env python3
"""One delayed-paint pair; no provider entry. Owns the shared cargo lock."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/kinewright-app/src/app.rs'
OUT = ROOT / 'target/late-paint-r1/pair'
WITNESS = 'app::late_paint_tests::last_project_quit_discards_delayed_scrub_paint_without_changing_saved_bytes'


def once(data, before, after):
    assert data.count(before) == 1, 'target drift: ' + before
    return data.replace(before, after, 1)


def mutate(source):
    source = once(source, '''        if self.projects.len() == 1 {
            self.playback.pause();
            self.playing = false;
            self.resume_after_scrub = false;
            self.clear_preview();
        }
''', '')
    for signature in [
        '    pub(crate) fn acknowledge_paint(&mut self, epoch: u64) {\n',
        '    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {\n',
    ]:
        source = once(source, signature + '''        if self.projects.is_empty() {
            return;
        }
''', signature)
    return source


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_INCREMENTAL='0', RUST_TEST_THREADS='4',
               TMPDIR=str(ROOT/'target/t'), LOCALAPPDATA=str(ROOT/'target/l'))
    with open('/tmp/kinewright-cargo-heavy.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        original = SOURCE.read_bytes()
        digest = hashlib.sha256(original).hexdigest()
        try:
            for side in ['red', 'green']:
                SOURCE.write_text(mutate(original.decode())) if side == 'red' else SOURCE.write_bytes(original)
                if side == 'green':
                    assert SOURCE.read_bytes() == original
                log = OUT / f'{side}.log'
                with log.open('w') as stream:
                    run = subprocess.run(['bash', '-c',
                        'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --features slow-tests,kinewright-agent/te1-eval --bin kinewright-app "$1" -- --exact --nocapture',
                        'pair', WITNESS], cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
                text = log.read_text()
                summary = 'test result: FAILED. 0 passed; 1 failed;' if side == 'red' else 'test result: ok. 1 passed; 0 failed;'
                assert run.returncode == (101 if side == 'red' else 0) and summary in text, (side, run.returncode)
                assert side != 'red' or 'EXIT_LATE_PAINT_EMPTY_PROJECT_IS_SAFE' in text
                print('late-paint', side, run.returncode, flush=True)
        finally:
            SOURCE.write_bytes(original)
            assert SOURCE.read_bytes() == original, 'restore differs'
        (OUT/'result.json').write_text(json.dumps(dict(pair='late-paint', red=101, green=0, restore_sha256=digest), indent=2)+'\n')
        print('PASS late-paint; byte-identical restore', digest, flush=True)


if __name__ == '__main__':
    main()
