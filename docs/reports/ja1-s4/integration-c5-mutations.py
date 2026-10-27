"""C5 fix round 1 protected witnesses. Run with nice -n 19 python3 -I."""
from pathlib import Path
import hashlib
import json
import os
import shlex
import shutil
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "docs/reports/ja1-s4/integration/c5-mutations"
OUT.mkdir(parents=True, exist_ok=True)
ACCOUNTING = "crates/kinewright-project/src/orchestration_accounting.rs"
ACP = "crates/kinewright-agent/src/acp.rs"
MUSE = "crates/kinewright-agent/src/muse.rs"


def replace(text, old, new):
    assert text.count(old) == 1, (old, text.count(old))
    return text.replace(old, new)


def remove_coverage(text):
    return replace(text, "if self.token_limit.is_some() && self.outstanding_usage()? {", "if false {")


def remove_fault_reaping(text):
    start = text.index("fn spawn_muse_incoming(")
    end = text.index("fn notification_targets_session(", start)
    body = text[start:end]
    assert body.count("client.kill();") == 4
    return text[:start] + body.replace("client.kill();", "") + text[end:]


CASES = [
    ("first-turn-coverage", ACCOUNTING, "ja_s4_usage", "s4_first_unreported_turn_blocks_next_send", remove_coverage),
    ("later-turn-coverage", ACCOUNTING, "ja_s4_usage", "s4_unreported_turn_after_settlement_blocks_next_send", remove_coverage),
    ("crash-coverage", ACCOUNTING, "ja_s4_usage", "s4_reply_before_usage_publication_crash_blocks_new_attempt", remove_coverage),
    ("matching-turn-coverage", ACCOUNTING, "ja_s4_usage", "s4_usage_settlement_requires_acknowledged_matching_turn", lambda t: replace(t, ".filter(|sample| sample.attempt == send.attempt && sample.turn == send.turn)", ".filter(|_sample| true)")),
    ("acknowledged-settlement", ACCOUNTING, "ja_s4_usage", "s4_usage_settlement_requires_acknowledged_matching_turn", lambda t: replace(t, "for send in &new.sends[admitted.sends.len()..] {", "for send in new.sends.iter().take(0) {")),
    ("cancellable-reply-write", ACP, "@lib", "muse::tests::s4_full_reply_pipe_cannot_block_raw_fault_cleanup", lambda t: replace(t, "self.wait_write(&completion, budget, restore)", "completion.recv().unwrap_or_else(|_| Err(AgentError::Protocol(\"writer closed\".into())))")),
    ("fault-reap-before-terminal", MUSE, "@lib", "muse::tests::s4_full_reply_pipe_cannot_block_raw_fault_cleanup", remove_fault_reaping),
    ("fault-publication-after-cleanup", ACP, "@lib", "acp::tests::s4_raw_fault_is_retained_when_cleanup_races_publication", lambda t: replace(t, "if notify_fault {", "if !interrupted.load(Ordering::Acquire) {")),
    ("prior-write-fault-publication", ACP, "@lib", "acp::tests::s4_prior_write_fault_survives_cleanup_before_eof", lambda t: replace(t, "!interrupted.load(Ordering::Acquire) || ingress_failed.load(Ordering::Acquire)", "!interrupted.load(Ordering::Acquire)")),
]
TRACKED = {name: (ROOT / name).read_bytes() for name in {case[1] for case in CASES}}
DIGEST = hashlib.sha256(b"".join(name.encode() + TRACKED[name] for name in sorted(TRACKED))).hexdigest()

if "--check" in sys.argv:
    for label, name, binary, test, mutate in CASES:
        assert mutate(TRACKED[name].decode()) != TRACKED[name].decode()
        print(label, "anchor verified")
    raise SystemExit(0)


def invoke(command, log):
    with log.open("w") as stream:
        process = subprocess.Popen(["bash", "-e", "-c", command], cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=7200)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    return code


def outcome(log, code):
    data = log.read_text()
    return {"exit": code,
            "summary": [line for line in data.splitlines() if line.startswith("test result:")],
            "runtime_red": code == 101 and "test result: FAILED." in data and "could not compile" not in data,
            "green": code == 0 and "test result: ok. 1 passed" in data,
            "log": str(log.relative_to(ROOT))}


PREFIX = "source scripts/setup-ffmpeg.sh >/dev/null\n" + \
    "awk '/MemAvailable:/ { if ($2 < 6291456) exit 1 }' /proc/meminfo\n"
CARGO = "flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 timeout --kill-after=5s 600s nice -n 10 cargo "


def run(label, binary, test):
    log = OUT / f"{label}-red.log"
    target = "--lib" if binary == "@lib" else "--test " + shlex.quote(binary)
    command = PREFIX + CARGO + "test -p kinewright-agent --features slow-tests " + target + " " + shlex.quote(test) + " -- --exact --test-threads=1 --nocapture"
    return outcome(log, invoke(command, log))


def file_hash(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def restored_binaries():
    assert all((ROOT / name).read_bytes() == data for name, data in TRACKED.items())
    log = OUT / 'green-build.log'
    code = invoke(PREFIX + CARGO + 'test -p kinewright-agent --features slow-tests --lib --test ja_s4_usage --no-run --message-format=json', log)
    assert code == 0, 'restored witness build failed'
    artifacts = {}
    for line in log.read_text().splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row.get('reason') == 'compiler-artifact' and row.get('profile', {}).get('test') and row.get('executable'):
            key = {'kinewright_agent': '@lib', 'ja_s4_usage': 'ja_s4_usage'}.get(row['target']['name'])
            if key:
                artifacts[key] = Path(row['executable'])
    assert set(artifacts) == {'@lib', 'ja_s4_usage'}, artifacts
    frozen = ROOT / 'target/int-c5-green'
    frozen.mkdir(exist_ok=True)
    binaries = {}
    for key, original in artifacts.items():
        digest = file_hash(original)
        path = frozen / (key.replace('@', '') + '-' + digest[:16])
        shutil.copy2(original, path)
        assert file_hash(path) == digest
        binaries[key] = {'path': str(path), 'sha256': digest}
    (OUT / 'green-binaries.json').write_text(json.dumps({'source_sha256': DIGEST, 'binaries': binaries}, indent=2) + '\n')
    return binaries


if '--green-batch' in sys.argv:
    plan = json.loads(Path(sys.argv[-1]).read_text())
    assert plan['source_sha256'] == DIGEST
    for binary in plan['binaries'].values():
        assert file_hash(binary['path']) == binary['sha256']
    exits = {}
    for case in plan['cases']:
        with Path(case['log']).open('w') as stream:
            exits[case['guard']] = subprocess.run(
                [plan['binaries'][case['binary']]['path'], '--exact', case['test'], '--test-threads=1', '--nocapture'],
                cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT, check=False).returncode
    (OUT / 'green-exits.json').write_text(json.dumps(exits, indent=2) + '\n')
    raise SystemExit(0)


results = []
if "--resume" in sys.argv:
    previous = json.loads((OUT / "results.json").read_text())
    assert previous["source_sha256"] == DIGEST
    results = previous["pairs"]
    for index, row in enumerate(results):
        assert row["guard"] == CASES[index][0] and row["red"]["runtime_red"] and row["green"]["green"]

try:
    pending = []
    for label, name, binary, test, mutate in CASES[len(results):]:
        path = ROOT / name
        path.write_text(mutate(TRACKED[name].decode()))
        try:
            red = run(label, binary, test)
        finally:
            path.write_bytes(TRACKED[name])
        if not red['runtime_red']:
            raise SystemExit('no runtime mutation failure: ' + label)
        pending.append({'guard': label, 'binary': binary, 'test': test, 'red': red,
                        'log': str(OUT / f'{label}-green.log')})
        (OUT / 'pending-red.json').write_text(json.dumps({'source_sha256': DIGEST, 'cases': pending}, indent=2) + '\n')
        print(label, 'runtime_red=True; restored green pending', flush=True)
    if pending:
        binaries = restored_binaries()
        plan_path = OUT / 'green-plan.json'
        plan_path.write_text(json.dumps({'source_sha256': DIGEST, 'binaries': binaries, 'cases': pending}, indent=2) + '\n')
        command = PREFIX + 'flock -o /tmp/kinewright-cargo-heavy.lock env RUST_TEST_THREADS=4 timeout --kill-after=5s 600s nice -n 19 python3 -I ' + shlex.quote(str(Path(__file__).resolve())) + ' --green-batch ' + shlex.quote(str(plan_path))
        assert invoke(command, OUT / 'green-batch.log') == 0, 'restored witness batch failed'
        exits = json.loads((OUT / 'green-exits.json').read_text())
    for case in pending:
        label, test, red = case['guard'], case['test'], case['red']
        green = outcome(Path(case['log']), exits[label])
        green['binary_sha256'] = binaries[case['binary']]['sha256']
        results.append({"guard": label, "witness": test, "red": red, "green": green})
        (OUT / "results.json").write_text(json.dumps({"source_sha256": DIGEST, "pairs": results}, indent=2) + "\n")
        print(label, "red=" + str(red["runtime_red"]), "restored_green=" + str(green["green"]), flush=True)
        if not red["runtime_red"] or not green["green"]:
            raise SystemExit("no behavioral mutation credit: " + label)
finally:
    for name, data in TRACKED.items():
        (ROOT / name).write_bytes(data)
    assert all((ROOT / name).read_bytes() == data for name, data in TRACKED.items())
