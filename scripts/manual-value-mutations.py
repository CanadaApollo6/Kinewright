#!/usr/bin/env python3
"""Final manual value controls. Owns the cargo lock; restores exact source bytes."""
import fcntl
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/gesture-final-value/pairs'
APP = 'crates/kinewright-app/src/'
PREFIX = 'ja1_attachment::manual_value_tests::'
CASES = {
    'latest-value': (APP+'manual_edits.rs', PREFIX+'manual_value_final_drag_is_saved_and_one_undo', 'MANUAL_FINAL_VALUE_LANDS'),
    'final-refusal': (APP+'manual_edits.rs', PREFIX+'manual_value_final_capacity_refusal_is_truthful', 'MANUAL_FINAL_REFUSAL_DROPS_VALUE'),
    'pending-close': (APP+'ja1_attachment.rs', PREFIX+'manual_value_close_and_quit_ask_for_unapplied_value', 'MANUAL_CLOSE_KEEPS_PENDING_PROJECT'),
    'release-frame': (APP+'app.rs', PREFIX+'manual_value_release_frame_submits_before_finishing_gesture', 'MANUAL_RELEASE_BEFORE_WIDGETS'),
    'retry-correlation': (APP+'app.rs', 'app::in1_tests::in1b_the_routers_own_rejected_send_is_dropped_not_left_outstanding', 'MANUAL_RETRY_CORRELATION'),
    'retry-reservation': (APP+'app.rs', PREFIX+'manual_value_final_drag_is_saved_and_one_undo', 'MANUAL_RETRY_RESERVATION'),
    'recovery-unaccepted': (APP+'manual_edits.rs', PREFIX+'manual_value_fix1_paused_recovery_keeps_retry_and_accepted_edits', 'MANUAL_RECOVERY_DROPS_UNACCEPTED'),
    'send-time-ack': (APP+'manual_edits.rs', 'manual_edits::tests::refusal_collected_after_curing_ack_retries_without_another_ack', 'ASTRA_MISSED_WAKEUP'),
    'poll-attempt': (APP+'ja1_host.rs', PREFIX+'manual_value_fix1_poll_keeps_publication_behind_attempt', 'MANUAL_POLL_WAIT_FOR_ATTEMPT'),
    'notice-job': (APP+'app.rs', PREFIX+'manual_value_fix1_paused_recovery_keeps_retry_and_accepted_edits', 'MANUAL_NOTICE_DURING_PREVIEW'),
    'accepted-undo-replay': (APP+'manual_edits.rs', 'manual_edits::tests::accepted_command_never_replayed_by_extra_acks_or_polls', 'UNDO_MUST_EXECUTE_EXACTLY_ONCE'),
    'widget-return': (APP+'color_wheel_widget.rs', 'inspector_ui::manual_gesture_tests::manual_widget_return_queued_has_no_edit_or_undo', 'WIDGET_RETURN_QUEUED_FINAL'),
    'widget-partial-return': (APP+'color_wheel_widget.rs', 'inspector_ui::manual_gesture_tests::manual_widget_partial_return_keeps_every_owned_channel', 'WIDGET_PARTIAL_RETURN_ZERO_CHANNEL'),
    'queued-return-no-undo': (APP+'manual_edits.rs', 'inspector_ui::manual_gesture_tests::manual_widget_return_queued_has_no_edit_or_undo', 'WIDGET_RETURN_QUEUED_CLEARS_BEFORE_ACK'),
    'in-flight-return': (APP+'manual_edits.rs', 'inspector_ui::manual_gesture_tests::manual_widget_return_in_flight_follows_the_accepted_excursion', 'WIDGET_RETURN_IN_FLIGHT_FINAL'),
    'slider-return': (APP+'inspector_ui.rs', 'inspector_ui::manual_gesture_tests::native_slider_return_and_relative_release_preserve_intent', 'SLIDER_EQUAL_RAIL_RELEASE_IS_INTENT'),
    'curve-return': (APP+'curve_editor_widget.rs', 'inspector_ui::manual_gesture_tests::curve_widget_emits_its_complete_return_to_the_document', 'CURVE_EQUAL_RETURN_IS_INTENT'),
    'full-notice-room': (APP+'manual_edits.rs', 'manual_edits::tests::full_notice_clears_as_soon_as_the_queue_has_room', 'MANUAL_FULL_NOTICE_CLEARS_WITH_ROOM'),
    'prepared-refusal': (APP+'ja1_attachment.rs', PREFIX+'manual_value_glm_prepared_media_clean_refusal_stays_editable', 'MANUAL_PREPARED_REFUSAL_STAYS_EDITABLE'),
}

def mutate(name, source):
    if name == 'latest-value':
        before = '            action.latest = Some(command);\n            self.notice = None;'
        after = ('            if action.waiting_at != Some(self.acknowledgement) {\n'
                 '                action.latest = Some(command);\n'
                 '            }\n            self.notice = None;')
    elif name == 'final-refusal':
        before = 'reason: CoreMutationRefusal::ManualBackpressure,'
        after = 'reason: _,'
    elif name == 'pending-close':
        start = source.index('    pub(crate) fn manual_edits_pending(')
        end = source.index('    pub(crate) fn manual_pending_blocks(', start)
        return source[:start] + '    pub(crate) fn manual_edits_pending(&self) -> bool { false }\n' + source[end:]
    elif name == 'retry-correlation':
        before = '*reason != kinewright_core::CoreMutationRefusal::ManualBackpressure'
        after = 'true'
    elif name == 'retry-reservation':
        before = 'if reason != kinewright_core::CoreMutationRefusal::ManualBackpressure {'
        after = 'if true {'
    elif name == 'recovery-unaccepted':
        start = source.index('    pub(crate) fn abandon_unaccepted(')
        end = source.index('    fn note_unapplied(', start)
        return source[:start] + '    pub(crate) fn abandon_unaccepted(&mut self) {}\n' + source[end:]
    elif name == 'send-time-ack':
        before = 'action.waiting_at = Some(flight.acknowledgement);'
        after = 'action.waiting_at = Some(self.acknowledgement);'
    elif name == 'full-notice-room':
        before = '        if self.actions.len() < PENDING_ACTIONS && matches!(self.notice, Some(Notice::Full)) {'
        after = '        if false {'
    elif name == 'prepared-refusal':
        before = '                if let Some(reason) = manual_refusal {'
        after = '                if let Some(reason) = manual_refusal.filter(|_| false) {'
    elif name == 'accepted-undo-replay':
        before = '                    action.accepted = Some(flight.command.command);'
        after = before + '\n                    action.latest = action.accepted.take().map(|command| PendingValue { command, unchanged_at: None });'
    elif name in ('widget-return', 'widget-partial-return'):
        before = '                        response.changes.push((channel, value));'
        after = ('                        if value != state.values.channel(channel) {\n'
                 '                            response.changes.push((channel, value));\n'
                 '                        }')
    elif name == 'queued-return-no-undo':
        before = '.and_then(|v| v.unchanged_at)'
        after = '.and_then(|v| v.unchanged_at.filter(|_| false))'
    elif name == 'in-flight-return':
        before = '.is_none_or(|accepted| accepted <= revision)'
        after = '.is_none_or(|_| true)'
    elif name == 'slider-return':
        before = '            || (response.enabled() && response.drag_stopped() && writes.get() > 1)'
        after = '            || false'
    elif name == 'curve-return':
        before = '            response.points = Some(moved);'
        after = '            if moved != points { response.points = Some(moved); }'
    elif name == 'poll-attempt':
        before = '        if self.edits.attempt_running() {\n            return Ok(None);\n        }'
        after = '        if false && self.edits.attempt_running() {\n            return Ok(None);\n        }'
    elif name == 'notice-job':
        before = 'session.ja1_job_notice = attachment.manual_persistent_status();'
        after = 'session.ja1_job_notice = None;'
    else:
        before = '        self.retirement.prune();'
        after = '        self.finish_manual_frame(ctx);\n        self.retirement.prune();'
        ending = '        self.finish_manual_frame(ui.ctx());'
        assert source.count(ending) == 1
        source = source.replace(ending, '// mutant: finish before release widgets', 1)
    assert source.count(before) == 1, 'mutation target drift: ' + before
    return source.replace(before, after, 1)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pairs', nargs='*')
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args()
    selected = args.pairs or list(CASES)
    assert all(name in CASES for name in selected)
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_INCREMENTAL='0', RUST_TEST_THREADS='4',
               TMPDIR=str(ROOT/'target/t'), LOCALAPPDATA=str(ROOT/'target/l'), PYTHONDONTWRITEBYTECODE='1')
    rows = []
    for name in selected:
        print('WAIT', name, flush=True)
        with open('/tmp/kinewright-cargo-heavy.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            file, witness, marker = CASES[name]
            path = ROOT/file
            original = path.read_bytes()
            digest = hashlib.sha256(original).hexdigest()
            try:
                for side in ('red', 'green'):
                    path.write_text(mutate(name, original.decode())) if side=='red' else path.write_bytes(original)
                    if side=='green': assert path.read_bytes()==original, 'restore differs'
                    log = out/f'{name}-{side}.log'
                    with log.open('w') as stream:
                        run = subprocess.run(['bash', '-c',
                            'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --bin kinewright-app "$1" -- --exact --nocapture',
                            'pair', witness], cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=300)
                    text = log.read_text()
                    summary = 'test result: FAILED. 0 passed; 1 failed;' if side=='red' else 'test result: ok. 1 passed; 0 failed;'
                    assert run.returncode==(101 if side=='red' else 0) and summary in text, (name, side, run.returncode)
                    assert side!='red' or marker in text, (name, 'wrong red')
                    print(name, side, run.returncode, flush=True)
            finally:
                path.write_bytes(original)
                assert path.read_bytes()==original, 'final restore differs'
            rows.append(dict(pair=name, red=101, green=0, restore_sha256=digest, source=file))
            (out/'results.json').write_text(json.dumps(rows, indent=2)+'\n')
            print('PASS', name, 'byte-identical restore', digest, flush=True)

if __name__ == '__main__':
    main()
