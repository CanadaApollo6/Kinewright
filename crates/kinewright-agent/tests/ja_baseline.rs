#![cfg(feature = "eval-harness")]
use kinewright_media::test_support::TempDirectory;
use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn native_cli_retains_literal_assertions_and_unchanged_fixture_identity() {
    let temp = TempDirectory::new("ja-native");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/history-loss.json");
    let mut hashes = Vec::new();
    let modes: &[&str] = if cfg!(target_os = "linux") {
        &["baseline", "revised"]
    } else {
        &["baseline"]
    };
    for &mode in modes {
        let output = temp.path(mode);
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
            .args(["--mode", mode, "--fixture"])
            .arg(&fixture)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        let expected = i32::from(mode == "baseline");
        assert_eq!(
            result.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let line: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(line["exit_code"], expected);
        let run: Value =
            serde_json::from_slice(&std::fs::read(output.join("run.json")).unwrap()).unwrap();
        hashes.push(run["fixture_sha256"].clone());
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert_eq!(
            summary["assertions"].as_array().unwrap().last().unwrap()["disposition"],
            if mode == "baseline" {
                "baseline_red"
            } else {
                "pass_control"
            }
        );
        for artifact in summary["artifacts"].as_array().unwrap() {
            let bytes =
                std::fs::read(output.join(artifact["relative_file"].as_str().unwrap())).unwrap();
            assert_eq!(
                kinewright_media::sha256_bytes(&bytes),
                artifact["sha256"].as_str().unwrap()
            );
        }
    }
    assert_eq!(hashes[0], kinewright_media::sha256_file(&fixture).unwrap());
    if cfg!(target_os = "linux") {
        assert_eq!(hashes[0], hashes[1]);
    }
    // Windows native durable-success assertions are NOT RUN; its dedicated refusal wrapper is independent.
}

fn native_fixture(name: &str, output: &Path, expected: i32) -> Value {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/ja/{name}.json"));
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(fixture)
        .arg("--output")
        .arg(output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(expected),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap()
}

#[test]
#[cfg(target_os = "linux")]
fn participants_hold_real_locks_and_reap_their_exact_children() {
    let temp = TempDirectory::new("ja-processes");
    let output = temp.path("out");
    native_fixture("participants-lock", &output, 0);
    let observation = |id| -> Value {
        serde_json::from_slice(
            &std::fs::read(output.join(format!("artifacts/observation-{id}.json"))).unwrap(),
        )
        .unwrap()
    };
    let child = observation(17)["pid"].as_u64().unwrap();
    let inventory = observation(18);
    assert_eq!(inventory["owned_child"]["pid"], child);
    assert!(
        inventory["os_children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pid| pid == child)
    );
    let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
    let events = events
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let spawned = events
        .iter()
        .filter(|e| e["kind"] == "process_spawn")
        .collect::<Vec<_>>();
    assert_eq!(spawned.len(), 2);
    assert_ne!(spawned[0]["data"]["pid"], spawned[1]["data"]["pid"]);
    for event in spawned {
        assert!(event["data"]["os_start_identity"].is_string());
    }
    let exits = events
        .iter()
        .filter(|e| e["kind"] == "process_exit")
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 2);
    assert!(exits.iter().all(|e| e["data"]["reaped"] == true));
}

#[test]
fn accepted_edit_reply_loss_and_original_retry_observe_two_undo_entries() {
    let temp = TempDirectory::new("ja-retry");
    let output = temp.path("out");
    let summary = native_fixture("reply-retry", &output, 1);
    assert!(
        summary["positive_controls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["passed"] == true)
    );
    let red = summary["assertions"].as_array().unwrap().last().unwrap();
    assert_eq!(red["disposition"], "baseline_red");
    let detail: Value = serde_json::from_slice(
        &std::fs::read(output.join(red["artifact"]["relative_file"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(detail["actual"], 2);
    assert_eq!(detail["expected"], 1);
    let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
    let events = events
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "reply_dropped" && e["action_id"] == 6)
    );
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "retry_original_action" && e["data"]["original_action_id"] == 6)
    );
}

#[test]
fn controller_failure_still_reaps_both_recorded_participants() {
    let temp = TempDirectory::new("ja-cleanup-failure");
    let fixture = temp.path("fixture.json");
    let template =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/participants-lock.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(template).unwrap()).unwrap();
    value["lane"] = "portable".into();
    value["requirements"] = serde_json::json!([]);
    value["oracles"] = serde_json::json!({});
    value["actions"] =
        serde_json::json!([{"id":1,"participant":"missing","action":"core_start","args":{}}]);
    value["barriers"] = serde_json::json!([]);
    std::fs::write(&fixture, serde_json::to_vec(&value).unwrap()).unwrap();
    let out = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let summary: Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["disposition"], "not_run");
    let events = std::fs::read_to_string(out.join("events.jsonl")).unwrap();
    let exits = events
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["kind"] == "process_exit")
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 2);
    assert!(exits.iter().all(|e| e["data"]["reaped"] == true));
}

#[test]
#[cfg(unix)]
fn native_deadline_interrupts_pending_confirmation_and_reaps_fixture() {
    let temp = TempDirectory::new("ja-native-deadline");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja");
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, temp.root().join(path.file_name().unwrap())).unwrap();
        }
    }
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(fixtures.join("recording-loopback.json")).unwrap())
            .unwrap();
    value["limits"]["action_timeout_ms"] = serde_json::json!(5000);
    value["actions"][10]["args"]["confirmation"] = Value::Null;
    let fixture = temp.path("deadline.json");
    std::fs::write(&fixture, serde_json::to_vec(&value).unwrap()).unwrap();
    let out = if let Some(root) = std::env::var_os("JA_HARNESS_RETAIN_OUTPUT") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        root.join("deadline")
    } else {
        temp.path("out")
    };
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let summary: Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["disposition"], "not_run");
    assert!(
        summary["missing_requirements"]
            .as_str()
            .unwrap()
            .contains("action 11")
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(out.join("supervisor-process.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["reaped"], true);
    assert_eq!(receipt["timed_out"], true);
    assert!(
        receipt["phase_elapsed_ms"].as_u64().unwrap() < 10_000,
        "interrupt the real broker wait at the fixture deadline"
    );
    // This exact ordinary approved commit completed before the later omitted
    // confirmation blocked. A missing server is not the deadline witness.
    let approved: Value =
        serde_json::from_slice(&std::fs::read(out.join("artifacts/observation-8.json")).unwrap())
            .unwrap();
    assert_eq!(approved["trace"]["commit_succeeded"], true);
}

fn copied_fixtures(temp: &TempDirectory) -> std::path::PathBuf {
    let root = temp.path("fixtures");
    std::fs::create_dir(&root).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja");
    for entry in std::fs::read_dir(fixtures).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, root.join(path.file_name().unwrap())).unwrap();
        }
    }
    root
}

fn retained_output(temp: &TempDirectory, label: &str) -> std::path::PathBuf {
    std::env::var_os("JA_HARNESS_RETAIN_OUTPUT").map_or_else(
        || temp.path(label),
        |root| {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            root.join(label)
        },
    )
}

#[test]
#[ignore = "on-demand App bridge: requires JA_HARNESS_APP_TEST_BINARY"]
fn native_app_bridge_retains_exercised_control_and_property_failures() {
    assert!(
        std::env::var_os("JA_HARNESS_APP_TEST_BINARY").is_some(),
        "build and supply the actual App test executable"
    );
    for class in ["control", "baseline_property"] {
        let temp = TempDirectory::new("ja-native-app-failure");
        let root = copied_fixtures(&temp);
        let file = root.join("app-coalesced-paint.json");
        let mut fixture: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        fixture["oracles"]["false"] =
            serde_json::from_slice::<Value>(&std::fs::read(root.join("legacy-io.json")).unwrap())
                .unwrap()["oracles"]["false"]
                .clone();
        let failure = fixture["actions"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|a| a["args"]["id"] == "actual-headless-callback-ran")
            .unwrap();
        failure["args"]["oracle"] = "false".into();
        failure["args"]["evidence_class"] = class.into();
        if class == "baseline_property" {
            failure["args"]["positive_control"] = "coalesced-gesture-one-undo".into();
        }
        std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let output = retained_output(&temp, &format!("native-app-{class}"));
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
            .args(["--mode", "baseline", "--fixture"])
            .arg(&file)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert_eq!(
            summary["disposition"],
            if class == "control" {
                "control_failed"
            } else {
                "observed"
            }
        );
        let assertion = summary["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == "actual-headless-callback-ran")
            .unwrap();
        assert_eq!(assertion["passed"], false);
        assert_eq!(
            assertion["disposition"],
            if class == "control" {
                "control_failed"
            } else {
                "baseline_red"
            }
        );
        assert!(
            summary["positive_controls"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["passed"] == true)
        );
        for reference in summary["artifacts"].as_array().unwrap() {
            let bytes =
                std::fs::read(output.join(reference["relative_file"].as_str().unwrap())).unwrap();
            assert_eq!(kinewright_media::sha256_bytes(&bytes), reference["sha256"]);
            assert_eq!(bytes.len() as u64, reference["byte_length"]);
        }
        let detail: Value = serde_json::from_slice(
            &std::fs::read(output.join(assertion["artifact"]["relative_file"].as_str().unwrap()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(detail["actual"], true);
        assert_eq!(detail["expected"], false);
        let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
        assert!(
            events
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .any(|e| e["kind"] == "process_exit"
                    && e["participant"] == "app-test"
                    && e["data"]["status"].as_str().unwrap().contains("101"))
        );
    }
}

#[test]
#[ignore = "on-demand App bridge: requires JA_HARNESS_APP_TEST_BINARY"]
fn native_app_bridge_keeps_inner_setup_failure_as_not_run_exit_two() {
    assert!(
        std::env::var_os("JA_HARNESS_APP_TEST_BINARY").is_some(),
        "build and supply the actual App test executable"
    );
    let temp = TempDirectory::new("ja-native-app-setup");
    let root = copied_fixtures(&temp);
    let file = root.join("app-coalesced-paint.json");
    let mut fixture: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    // The valid protocol reaches App, which lacks its required gesture batch.
    let gesture = b"{\"batches\":[],\"at\":0}";
    std::fs::write(root.join("app-gesture.json"), gesture).unwrap();
    fixture["inputs"]["gesture"]["sha256"] = kinewright_media::sha256_bytes(gesture).into();
    fixture["inputs"]["gesture"]["byte_length"] = (gesture.len() as u64).into();
    std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let output = retained_output(&temp, "native-app-setup");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&file)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["disposition"], "not_run");
    assert!(
        summary["missing_requirements"]
            .as_str()
            .unwrap()
            .contains("App control raster/gesture quota")
    );
    let inner: Value =
        serde_json::from_slice(&std::fs::read(output.join("app-1/summary.json")).unwrap()).unwrap();
    assert_eq!(inner["exit_code"], 2);
    assert!(summary["assertions"].as_array().unwrap().is_empty());
}

#[test]
#[ignore = "on-demand App bridge: requires JA_HARNESS_APP_TEST_BINARY"]
fn native_app_bridge_rejects_abnormal_exit_after_completed_report() {
    assert!(std::env::var_os("JA_HARNESS_APP_TEST_BINARY").is_some());
    for inner_exit in [0, 1] {
        let temp = TempDirectory::new("ja-native-app-abnormal");
        let root = copied_fixtures(&temp);
        let file = root.join("app-coalesced-paint.json");
        if inner_exit == 1 {
            let mut fixture: Value =
                serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            fixture["oracles"]["false"] = serde_json::from_slice::<Value>(
                &std::fs::read(root.join("legacy-io.json")).unwrap(),
            )
            .unwrap()["oracles"]["false"]
                .clone();
            let failure = fixture["actions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|a| a["args"]["id"] == "actual-headless-callback-ran")
                .unwrap();
            failure["args"]["oracle"] = "false".into();
            failure["args"]["evidence_class"] = "baseline_property".into();
            failure["args"]["positive_control"] = "coalesced-gesture-one-undo".into();
            std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
        }
        let output = retained_output(&temp, &format!("native-app-abnormal-{inner_exit}"));
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
            .args(["--mode", "baseline", "--fixture"])
            .arg(&file)
            .arg("--output")
            .arg(&output)
            .env("JA_HARNESS_TEST_ABNORMAL_APP_EXIT", "1")
            .output()
            .unwrap();
        let inner: Value =
            serde_json::from_slice(&std::fs::read(output.join("app-1/summary.json")).unwrap())
                .unwrap();
        assert_eq!(inner["exit_code"], inner_exit);
        assert!(!inner["assertions"].as_array().unwrap().is_empty());
        assert!(
            inner["positive_controls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["passed"] == true)
        );
        assert_eq!(
            result.status.code(),
            Some(2),
            "a completed report cannot override abnormal termination"
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert_eq!(summary["disposition"], "not_run");
        assert!(summary["assertions"].as_array().unwrap().is_empty());
        assert!(
            summary["missing_requirements"]
                .as_str()
                .unwrap()
                .contains("abnormal exit/report combination")
        );
        let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
        let events = events
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>();
        assert!(events.iter().any(|e| e["kind"] == "process_exit"
            && e["participant"] == "app-test"
            && e["data"]["status"].as_str().unwrap().contains("86")));
        assert!(!events.iter().any(|e| e["kind"] == "common_report_imported"));
    }
}

#[test]
fn native_source_identity_is_repository_wide_from_root_and_crate_cwd() {
    let temp = TempDirectory::new("ja-native-source");
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_root.parent().unwrap().parent().unwrap();
    let fixture = crate_root.join("tests/fixtures/ja/history-loss.json");
    let mut identities = Vec::new();
    for (label, cwd) in [("repo", repo), ("crate", crate_root)] {
        let out = temp.path(label);
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
            .current_dir(cwd)
            .args(["--mode", "baseline", "--fixture"])
            .arg(&fixture)
            .arg("--output")
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        let identity: Value = serde_json::from_slice(
            &std::fs::read(out.join("artifacts/source-manifest.json")).unwrap(),
        )
        .unwrap();
        // `git rev-parse --show-toplevel` prints `/` separators on Windows;
        // compare the resolved directories, not the spellings.
        assert_eq!(
            Path::new(identity["repo_root"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            repo.canonicalize().unwrap()
        );
        assert!(identity["head"].as_str().is_some_and(|s| s.len() == 40));
        assert_eq!(
            identity["dirty_diff_sha256"],
            kinewright_media::sha256_bytes(
                &std::fs::read(out.join("artifacts/source.diff")).unwrap()
            )
        );
        identities.push(identity);
    }
    assert_eq!(identities[0], identities[1]);
}

#[test]
#[cfg(target_os = "linux")]
fn refused_participant_kill_records_the_graceful_exit_and_reap() {
    let temp = TempDirectory::new("ja-refused-kill");
    let root = copied_fixtures(&temp);
    let file = root.join("participants-lock.json");
    let mut fixture: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    fixture["initial"]["participants"] = serde_json::json!(["A"]);
    fixture["actions"] = serde_json::json!([
        {"id":1,"participant":"A","action":"spawn_child","args":{}},
        {"id":2,"participant":"A","action":"participant_kill","args":{}}]);
    fixture["barriers"] = serde_json::json!([]);
    std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let out = retained_output(&temp, "refused-kill");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&file)
        .arg("--output")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let events = std::fs::read_to_string(out.join("events.jsonl")).unwrap();
    let exits = events
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|e| e["kind"] == "process_exit" && e["participant"] == "A")
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 1);
    assert_eq!(exits[0]["data"]["reaped"], true);
    assert_eq!(exits[0]["data"]["killed"], false);
}

#[test]
#[cfg(target_os = "linux")]
fn unexpected_controller_exit_reaps_a_recorded_stopped_worker_before_return() {
    use rustix::process::{Pid, Signal, kill_process};
    use std::time::{Duration, Instant};
    let temp = TempDirectory::new("ja-unexpected-exit");
    let root = copied_fixtures(&temp);
    let file = root.join("recording-loopback.json");
    let mut fixture: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    fixture["initial"]["participants"] = serde_json::json!(["A"]);
    fixture["actions"][10]["args"]["confirmation"] = Value::Null;
    fixture["limits"]["action_timeout_ms"] = 30_000.into();
    std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let out = retained_output(&temp, "unexpected-exit");
    let mut supervisor = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&file)
        .arg("--output")
        .arg(&out)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let (leader, worker) = loop {
        let events = std::fs::read_to_string(out.join("events.jsonl")).unwrap_or_default();
        let worker = events
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|e| e["kind"] == "process_spawn" && e["participant"] == "A")
            .and_then(|e| e["data"]["pid"].as_u64());
        let phase: Value = std::fs::read(out.join("supervisor-deadline.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        if let Some(worker) = worker
            && phase["action_id"] == 11
        {
            let receipt: Value = serde_json::from_slice(
                &std::fs::read(out.join("supervisor-process.json")).unwrap(),
            )
            .unwrap();
            break (
                u32::try_from(receipt["pid"].as_u64().unwrap()).unwrap(),
                u32::try_from(worker).unwrap(),
            );
        }
        if Instant::now() >= deadline {
            let _ = supervisor.kill();
            let _ = supervisor.wait();
            panic!("controller did not reach the real pending-confirmation barrier");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let worker_start = kinewright_agent::ja_harness::report::os_start_identity(worker).unwrap();
    let identity = |pid| Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    // SIGSTOP makes EOF insufficient: the worker remains live after its
    // controller dies until the supervisor actively terminates its group.
    kill_process(identity(worker), Signal::STOP).unwrap();
    assert!(Path::new(&format!("/proc/{worker}")).exists());
    kill_process(identity(leader), Signal::KILL).unwrap();
    let result = supervisor.wait_with_output().unwrap();
    let worker_gone = kinewright_agent::ja_harness::report::os_start_identity(worker).as_deref()
        != Some(worker_start.as_str());
    // A mutation must not leave the deliberately stopped witness on the host.
    if !worker_gone {
        let _ = kill_process(identity(worker), Signal::KILL);
    }
    assert!(
        worker_gone,
        "recorded worker must be gone before supervision returns"
    );
    assert_eq!(result.status.code(), Some(2));
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(out.join("supervisor-process.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["cleanup"]["completed"], true);
    assert!(
        receipt["cleanup"]["descendants_reaped"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(worker))
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["exit_code"], 2);
    assert_eq!(summary["disposition"], "not_run");
}
