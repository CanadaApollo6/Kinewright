#!/usr/bin/env python3
"""Targeted C4 runtime reds and restored greens through the mandatory Cargo wrapper."""
import gzip
import hashlib
import json
from pathlib import Path
import signal
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / 'docs/reports/ja1-s4'
TASKS = 'crates/kinewright-agent/src/orchestration/tasks.rs'
REQUESTS = 'crates/kinewright-agent/src/orchestration/requests.rs'
MAILBOX = 'crates/kinewright-agent/src/orchestration/mailbox.rs'


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise RuntimeError(f'expected one mutation anchor, found {source.count(old)}: {old}')
    return source.replace(old, new, 1)


def wake_boundary(source, kind, target):
    start = source.index('        OrchestrationCommand::AdmitQuestionWake(id)')
    end = source.index('        OrchestrationCommand::AnswerQuestion(answer)', start)
    block = source[start:end]
    header, body = block.split('=> {', 1)
    body = body.rsplit('}', 1)[0]
    return source[:start] + header + '=> {\n' + f'''
            if matches!(envelope.command, OrchestrationCommand::{kind}(_)) {{
                let RuntimeRequest::Question(question) = state.requests.iter_mut().find(|request| matches!(request, RuntimeRequest::Question(q) if &q.id == id)).ok_or(Refusal::InvalidState)? else {{ return Err(Refusal::InvalidState); }};
                question.wake = QuestionWake::{target};
            }} else {{ {body} }}
        }}
''' + source[end:]


MUTATIONS = [
    ('M1-descendant-readiness', TASKS, 's4_nested_followup_readiness',
     lambda s: replace_once(s, '.filter(|child| child.parent.as_ref() == Some(id))', '.filter(|_| false)')),
    ('M2-attention-result-selection', TASKS, 's4_question_attention_and_late_result_preserve_identity',
     lambda s: replace_once(s, '.is_some_and(|turn| turn.purpose == TurnPurpose::Work)', '.is_some_and(|_| true)')),
    ('M3-current-turn-identity', REQUESTS, 's4_question_attention_and_late_result_preserve_identity',
     lambda s: replace_once(s, '        || task.progress.turn.as_ref() != Some(turn)\n', '')),
    ('M4-question-wake-admission', REQUESTS, 's4_stale_question_and_approval',
     lambda s: wake_boundary(s, 'AdmitQuestionWake', 'Admitted')),
    ('M5-question-wake-execution', REQUESTS, 's4_stale_question_and_approval',
     lambda s: wake_boundary(s, 'ExecuteQuestionWake', 'Executed')),
    ('M6-published-result-lifetime', MAILBOX, 's4_question_attention_and_late_result_preserve_identity',
     lambda s: replace_once(s, '    require_publishable(state, task)?;', '    // Mutation removes publication lifetime admission.')),
    ('M7-fresh-runtime-response', REQUESTS, 's4_runtime_request_freshness',
     lambda s: replace_once(s, '            && current == Some(request.binding())', '')),
    ('M8-late-background-completion', TASKS, 's4_background_cancel_reopen',
     lambda s: replace_once(s, '                super::requests::live_identity(state, turn)?;', '                // Mutation accepts late native background work.')),
    ('M10-expired-session-ingress', REQUESTS, 's4_expired_turn_never_manufactures_readiness',
     lambda s: replace_once(s, '                    task.progress.expired = true;', '                    task.progress.expired = false;')),
    ('M9-durable-stop-intent', 'crates/kinewright-agent/src/session.rs', 's4_cancel_intent_precedes_native_stop',
     lambda s: replace_once(s, 'observer.prepare_studio_stop(&stop, session.runtime_request_binding().as_ref())', 'Result::<(), String>::Ok(())')),
]


def run_test(name, test, color):
    result = subprocess.run(['bash', 'docs/reports/ja1-s4/c1-cargo.sh', 'test', '-p', 'kinewright-agent', '--test', 'ja_s4_lifecycle', test, '--', '--exact', '--test-threads=1'], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=600)
    data = result.stdout.encode()
    path = REPORT / f'c4-{name}-{color}.log.gz'
    path.write_bytes(gzip.compress(data, mtime=0))
    lines = result.stdout.splitlines()
    summary = next((line for line in reversed(lines) if line.startswith('test result:')), '')
    if color == 'red':
        passed = result.returncode != 0 and 'test result: FAILED.' in summary and '1 failed' in summary and 'panicked at' in result.stdout
    else:
        passed = result.returncode == 0 and '1 passed; 0 failed' in summary
    if not passed:
        raise RuntimeError(f'{name} {color} was not a runtime {color}: exit={result.returncode}, {summary}; see {path.name}')
    print(f'{name} {color}: {summary}', flush=True)
    return {'exit': result.returncode, 'summary': summary, 'log': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def interrupt(_signum, _frame):
    raise KeyboardInterrupt


signal.signal(signal.SIGTERM, interrupt)
results = []
for name, source, test, mutate in MUTATIONS:
    path = ROOT / source
    original = path.read_bytes()
    try:
        path.write_text(mutate(original.decode()))
        red = run_test(name, test, 'red')
    finally:
        path.write_bytes(original)
    green = run_test(name, test, 'green')
    results.append({'id': name, 'source': source, 'source_sha256': hashlib.sha256(original).hexdigest(), 'test': test, 'red': red, 'green': green})
    (REPORT / 'c4-mutations.json').write_text(json.dumps(results, indent=2) + '\n')
print(f'protected runtime mutation pairs: {len(results)}', flush=True)
