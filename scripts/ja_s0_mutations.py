#!/usr/bin/env python3
"""Run only the selected observer witness, retain its red, restore exact bytes,
then rerun that witness. Invoke after setup-ffmpeg.sh, with nice 19 / python -I.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
HISTORY = "ja_harness::tests::edit_save_reopen_has_literal_history_loss_after_successful_control"
CASES = {
    "app-abnormal-classifier": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "(Some(0), Some(0)) | (Some(101), Some(1..=3))", "(Some(_), Some(0..=3))",
        "agent", "ja_harness::controller::tests::app_exit_classifier_rejects_abnormal_numeric_status_despite_completed_report"),
    "app-abnormal-native": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "if !app_exit_matches_report(status, timed_out, Some(exit)) {",
        "if false && !app_exit_matches_report(status, timed_out, Some(exit)) {",
        "native", "native_app_bridge_rejects_abnormal_exit_after_completed_report"),
    "single-finish": ("crates/kinewright-agent/src/ja_harness/supervision.rs",
        "if !self.finish_attempted {", "if !self.released {",
        "agent", "ja_harness::supervision::tests::timed_out_finish_is_not_retried_by_drop_after_its_receipt"),
    "worker-teardown-window": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "} else {\n            super::MEDIA_TEARDOWN_MS + super::WORKER_REAP_MS\n        };",
        "} else {\n            super::WORKER_REAP_MS\n        };",
        "agent", "ja_harness::tests::cleanup_deadline_outlives_all_fixed_role_teardown_allowances"),
    "unexpected-controller-exit": ("crates/kinewright-agent/src/ja_harness/supervision.rs",
        "let mut error = self.stop().err();", "let mut error = None;",
        "native", "unexpected_controller_exit_reaps_a_recorded_stopped_worker_before_return"),
    "app-bridge-classification": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "if timed_out || status.code().is_none() {", "if !status.success() || timed_out || status.code().is_none() {",
        "native", "native_app_bridge_retains_exercised_control_and_property_failures"),
    "app-bridge-setup": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "if timed_out || status.code().is_none() {", "if !status.success() || timed_out || status.code().is_none() {",
        "native", "native_app_bridge_keeps_inner_setup_failure_as_not_run_exit_two"),
    "deadline-exit-race": ("crates/kinewright-agent/src/ja_harness/supervision.rs",
        "Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),", "Ok(()) => Ok(()),",
        "agent", "ja_harness::supervision::tests::deadline_exit_race_accepts_esrch_and_preserves_permission_errors"),
    "cleanup-budget": ("crates/kinewright-agent/src/ja_harness/mod.rs",
        "(workers + 1) * MEDIA_TEARDOWN_MS", "0 * (workers + 1) * MEDIA_TEARDOWN_MS",
        "agent", "ja_harness::tests::cleanup_deadline_outlives_all_fixed_role_teardown_allowances"),
    "control-summary": ("crates/kinewright-agent/src/ja_harness/report.rs",
        'else if control_failed {"control_failed"}', 'else if control_failed {"observed"}',
        "agent", "ja_harness::tests::failed_positive_control_has_control_failed_summary_and_exit_one"),
    "source-root": ("crates/kinewright-agent/src/ja_harness/report.rs",
        'let bytes = git_output(None, &["rev-parse", "--show-toplevel"])?;',
        'let bytes = std::env::current_dir().map_err(|e| e.to_string())?.to_string_lossy().as_bytes().to_vec();',
        "native", "native_source_identity_is_repository_wide_from_root_and_crate_cwd"),
    "participant-error-exit": ("crates/kinewright-agent/src/ja_harness/controller.rs",
        "let cleanup = worker.finish(false, report);", "let cleanup: Result<Value, String> = Ok(Value::Null);",
        "native", "refused_participant_kill_records_the_graceful_exit_and_reap"),
    "path-admission": ("crates/kinewright-agent/src/ja_harness/protocol.rs",
        "| Call::LockAcquire(a)", "",
        "agent", "ja_harness::tests::every_path_argument_is_refused_at_admission_before_any_action"),
    "teardown-drop": ("crates/kinewright-media/tests/generated_media.rs",
        "if self.engine.take().is_some() {",
        'if { self.engine.take(); assert_eq!(self.finished.recv_timeout(Duration::from_secs(30)), Err(crossbeam_channel::RecvTimeoutError::Disconnected)); true } {',
        "media", "teardown_observation_failure_panics_only_in_explicit_finish"),
    "main-adapter-dispatch": ("crates/kinewright-agent/src/ja_harness/mod.rs",
        'let routed = controller.dispatch(action, loaded, &observed, report)?;',
        'let routed = if action.call.main_adapter_only() { None } else { controller.dispatch(action, loaded, &observed, report)? };',
        "agent", "ja_harness::tests::main_adapter_uses_shared_reply_drop_and_retry_validation"),
    "role-collision": ("crates/kinewright-agent/src/ja_harness/protocol.rs",
        'if f.inputs.keys().any(|role| role.starts_with("oracle:")) {',
        'if false && f.inputs.keys().any(|role| role.starts_with("oracle:")) {',
        "agent", "ja_harness::tests::input_roles_cannot_alias_the_oracle_namespace"),
    "participant-routing": ("crates/kinewright-agent/src/ja_harness/protocol.rs",
        'if action.call.main_adapter_only() && action.participant != "main" {',
        'if false && action.call.main_adapter_only() && action.participant != "main" {',
        "agent", "ja_harness::tests::media_adapter_refuses_a_non_main_participant_before_execution"),
    "media-artifact-identity": ("crates/kinewright-agent/src/ja_harness/media_observer.rs",
        'let prefix = format!("media-{}-{}", action.participant, action.id);',
        'let prefix = format!("media-{}", action.participant);',
        "agent", "ja_harness::tests::repeated_media_actions_retain_distinct_complete_artifacts"),
    "native-deadline": ("crates/kinewright-agent/src/ja_harness/supervision.rs",
        'if Instant::now() >= deadline {\n            timed_out = true;',
        'if Instant::now() >= deadline + Duration::from_secs(6) {\n            timed_out = true;',
        "native", "native_deadline_interrupts_pending_confirmation_and_reaps_fixture"),
    "history-trace": ("crates/kinewright-core/src/actor.rs",
        "undo: state.undo.iter().map(observe).collect(),", "undo: Vec::new(),", "agent", HISTORY),
    "io-trace": ("crates/kinewright-project/src/project.rs",
        '    crate::io_observer::note("temp_write_and_file_sync", &temp);',
        "    let _ = &temp;", "agent", "ja_harness::tests::current_io_control_precedes_missing_directory_sync_assertion"),
    "assertion": ("crates/kinewright-agent/src/ja_harness/mod.rs",
        "protocol::Comparison::Equal => actual == expected,", "protocol::Comparison::Equal => true,", "agent", HISTORY),
    "optional-cost": ("crates/kinewright-agent/src/ja_harness/recording.rs",
        '"cached_input_tokens":cached_input_tokens,', '"cached_input_tokens":cached_input_tokens.unwrap_or(0),',
        "agent", "ja_harness::tests::recording_forwards_real_loopback_sessions_and_confirmation"),
    "duplicate-key": ("crates/kinewright-agent/src/ja_harness/protocol.rs",
        "if values.insert(key, value.0).is_some() {", "if { values.insert(key, value.0); false } {",
        "agent", "ja_harness::tests::duplicate_oversize_and_guarded_undo_are_not_run"),
    "actual-paint": ("crates/kinewright-app/src/presenter.rs",
        "        self.record();", "        // S0 observer mutation: omit the actual paint callback mark.",
        "app", "ja_harness::tests::coalesced_app_input_reaches_actual_headless_paint"),
    "letterbox": ("crates/kinewright-app/src/matte_overlay_ui.rs",
        "Rect::from_min_size(pos2(20.0, 30.0), vec2(160.0, 90.0)),",
        "Rect::from_min_size(pos2(21.0, 30.0), vec2(160.0, 90.0)),",
        "app", "ja_harness::tests::coalesced_app_input_reaches_actual_headless_paint"),
}


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def run(case, phase, package, test, out):
    while int(Path("/proc/meminfo").read_text().split("MemAvailable:")[1].split()[0]) < 6 * 1024**2:
        print("mutation: waiting for the required 6 GiB available memory", flush=True)
        time.sleep(10)
    folder = out / phase
    folder.mkdir()
    env = dict(os.environ, CARGO_BUILD_JOBS="4", RUST_TEST_THREADS="4",
               JA_HARNESS_RETAIN_OUTPUT=str(folder / "reports"))
    temporary = folder / "tmp"
    temporary.mkdir()
    env["TMPDIR"] = str(temporary)
    target = ["--lib"] if package == "agent" else (["--features", "eval-harness", "--test", "ja_baseline"] if package == "native" else (["--test", "generated_media"] if package == "media" else ["--bin", "kinewright-app"]))
    crate = "agent" if package == "native" else package
    command = ["nice", "-n", "10", "cargo", "test", "-p", "kinewright-" + crate] + target + [test, "--", "--exact", "--include-ignored", "--nocapture", "--test-threads=1"]
    path = folder / "test.log"
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    with path.open("w") as log:
        log.write(f"source_head={head}\ncase={case}\nphase={phase}\n")
        log.write(json.dumps(command) + "\n")
        log.flush()
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=600)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    raw = path.read_text()
    summaries = re.findall(r"^test result:.*$", raw, re.M)
    assert len(summaries) == 1, raw[-4000:]
    if phase == "mutation":
        assert code == 101 and "FAILED. 0 passed; 1 failed; 0 ignored" in summaries[0], raw[-5000:]
    else:
        assert code == 0 and "ok. 1 passed; 0 failed; 0 ignored" in summaries[0], raw[-5000:]
    binaries = re.findall(r"Running .*\(([^)]+)\)", raw)
    assert len(binaries) == 1, raw[-4000:]
    binary = ROOT / binaries[0]
    result = dict(source_head=head, phase=phase, disposition="mutation_red" if phase == "mutation" else "restored_green",
                  command=command, process_exit=code, raw_test_result=summaries[0], binary_sha256=digest(binary), log_sha256=digest(path))
    (folder / "receipt.json").write_text(json.dumps(result, indent=2) + "\n")
    print(case, phase, summaries[0], flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("case", choices=CASES)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    source_name, before, after, package, test = CASES[args.case]
    source = ROOT / source_name
    original = source.read_bytes()
    text = original.decode()
    assert text.count(before) == 1, (source_name, before)
    mutant = text.replace(before, after, 1).encode()
    (args.output / "original.rs").write_bytes(original)
    (args.output / "mutation.json").write_text(json.dumps(dict(source=source_name, before=before, after=after,
        original_sha256=hashlib.sha256(original).hexdigest(), mutated_sha256=hashlib.sha256(mutant).hexdigest(), witness=test), indent=2) + "\n")

    def interrupt(_signal, _frame):
        raise KeyboardInterrupt("mutation interrupted; restoring exact source")

    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    try:
        source.write_bytes(mutant)
        red = run(args.case, "mutation", package, test, args.output)
    finally:
        source.write_bytes(original)
    assert source.read_bytes() == original
    green = run(args.case, "restored", package, test, args.output)
    (args.output / "result.json").write_text(json.dumps(dict(case=args.case, source=source_name,
        mutation=red, restored=green, original_bytes_restored=True), indent=2) + "\n")


if __name__ == "__main__":
    main()
