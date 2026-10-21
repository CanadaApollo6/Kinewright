#![cfg(feature = "eval-harness")]
use kinewright_media::test_support::TempDirectory;
use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn native_cli_retains_literal_assertions_and_unchanged_fixture_identity() {
    let temp = TempDirectory::new("ja-native");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/history-loss.json");
    let mut hashes = Vec::new();
    for mode in ["baseline", "revised"] {
        let output = temp.path(mode);
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
            .args(["--mode", mode, "--fixture"])
            .arg(&fixture)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let line: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(line["exit_code"], 1);
        let run: Value =
            serde_json::from_slice(&std::fs::read(output.join("run.json")).unwrap()).unwrap();
        hashes.push(run["fixture_sha256"].clone());
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
        assert_eq!(
            summary["assertions"].as_array().unwrap().last().unwrap()["disposition"],
            "baseline_red"
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
    assert_eq!(hashes[0], hashes[1]);
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
