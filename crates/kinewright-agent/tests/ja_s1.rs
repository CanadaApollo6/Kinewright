#![cfg(feature = "eval-harness")]
use kinewright_media::test_support::TempDirectory;
use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn revised_mode_reaches_main_and_native_workers() {
    let temp = TempDirectory::new("ja-s1-routing");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-routing.json");
    let bytes = std::fs::read(&fixture).unwrap();
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "revised", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let run: Value =
        serde_json::from_slice(&std::fs::read(output.join("run.json")).unwrap()).unwrap();
    assert_eq!(
        run["fixture_sha256"],
        kinewright_media::sha256_bytes(&bytes)
    );
    assert_eq!(run["mode"], "revised");
    assert_eq!(
        run["binary_sha256"],
        kinewright_media::sha256_file(Path::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline")))
            .unwrap()
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(head.status.success());
    assert_eq!(
        run["source_head"],
        String::from_utf8(head.stdout).unwrap().trim()
    );
    let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
    let spawn = events
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|event| event["kind"] == "process_spawn" && event["participant"] == "A")
        .unwrap();
    assert_eq!(spawn["data"]["executable_sha256"], run["binary_sha256"]);
    let mut pids = std::collections::BTreeSet::new();
    for id in [1, 2] {
        let observed: Value = serde_json::from_slice(
            &std::fs::read(output.join(format!("artifacts/observation-{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(observed["mode"], "revised");
        assert_eq!(observed["backend"], "portable-model");
        assert!(observed["pid"].as_u64().unwrap() > 0);
        pids.insert(observed["pid"].as_u64().unwrap());
    }
    assert_eq!(pids.len(), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn paired_io_baseline_has_successful_refusal_controls_and_literal_reds() {
    let temp = TempDirectory::new("ja-s1-paired-baseline");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-paired-io.json");
    let output = temp.path("out");
    assert_eq!(
        kinewright_agent::ja_harness::run(
            &fixture,
            &output,
            kinewright_agent::ja_harness::protocol::Mode::Baseline
        )
        .unwrap(),
        1
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let assertions = summary["assertions"].as_array().unwrap();
    let controls: Vec<_> = assertions
        .iter()
        .filter(|a| a["evidence_class"] == "control")
        .collect();
    assert_eq!(controls.len(), 4);
    assert!(controls.iter().all(|a| a["passed"] == true));
    for id in [
        "strict-parent-directory-sync",
        "strict-write-no-in-place-fallback",
        "refused-root-exact",
    ] {
        let assertion = assertions.iter().find(|a| a["id"] == id).unwrap();
        assert_eq!(assertion["disposition"], "baseline_red");
        assert_eq!(assertion["passed"], false);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn strict_io_protocol_separates_legacy_control_from_bundle_result() {
    let temp = TempDirectory::new("ja-s1-paired-revised");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-paired-io.json");
    let output = temp.path("out");
    assert_eq!(
        kinewright_agent::ja_harness::run(
            &fixture,
            &output,
            kinewright_agent::ja_harness::protocol::Mode::Revised
        )
        .unwrap(),
        0
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let assertions = summary["assertions"].as_array().unwrap();
    assert_eq!(assertions.len(), 7);
    assert!(
        assertions.iter().all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let observed: Value = serde_json::from_slice(
        &std::fs::read(output.join("artifacts/observation-3.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(observed["backend"], "linux-local-strict");
    assert_eq!(observed["succeeded"], false);
    assert_eq!(observed["root_preserved"], true);
    assert_eq!(observed["refusal_reached"], true);
    assert_eq!(observed["refusal_injected"], true);
    assert_eq!(observed["attempted_bytes_equal"], true);
    assert_eq!(observed["in_place_fallback_observed"], false);
}

#[cfg(target_os = "linux")]
#[test]
fn owned_input_fixture_retains_bytes_and_charges_until_reader_close() {
    let temp = TempDirectory::new("ja-s1-owned-fixture");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-owned-input.json");
    let output = temp.path("out");
    assert_eq!(
        kinewright_agent::ja_harness::run(
            &fixture,
            &output,
            kinewright_agent::ja_harness::protocol::Mode::Revised
        )
        .unwrap(),
        0
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let assertions = summary["assertions"].as_array().unwrap();
    assert_eq!(assertions.len(), 4);
    assert!(
        assertions.iter().all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let observed: Value = serde_json::from_slice(
        &std::fs::read(output.join("artifacts/observation-1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(observed["backend"], "linux-local-owned-store");
    assert_eq!(observed["decoder_certification_unavailable"], true);
}

#[cfg(target_os = "linux")]
#[test]
fn history_loss_fixture_revised_preserves_one_entry() {
    let temp = TempDirectory::new("ja-s1-history-revised");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/history-loss.json");
    let output = temp.path("out");
    assert_eq!(
        kinewright_agent::ja_harness::run(
            &fixture,
            &output,
            kinewright_agent::ja_harness::protocol::Mode::Revised
        )
        .unwrap(),
        0
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let assertions = summary["assertions"].as_array().unwrap();
    assert!(assertions.iter().all(|a| a["passed"] == true));
    assert!(
        assertions
            .iter()
            .any(|a| a["id"] == "history-survives-reopen" && a["passed"] == true)
    );
    let save: Value = serde_json::from_slice(
        &std::fs::read(output.join("artifacts/observation-4.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(save["backend"], "linux-local-bundle");
    assert_eq!(save["writer_bytes_equal"], true);
}

#[cfg(target_os = "linux")]
#[test]
fn comment_observer_persists_exact_bytes_then_missing_receipt() {
    let temp = TempDirectory::new("ja-s1-comment-baseline");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/comment-receipt.json");
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "baseline", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .args(["--scenario", "comment-baseline"])
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    let assertions = summary["assertions"].as_array().unwrap();
    assert!(
        assertions
            .iter()
            .filter(|a| a["evidence_class"] == "control")
            .all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let missing = assertions
        .iter()
        .find(|a| a["id"] == "durable-comment-result-binding")
        .unwrap();
    assert_eq!(missing["passed"], false);
    assert_eq!(missing["disposition"], "baseline_red");
    let events = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
    let events: Vec<Value> = events
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "scenario_selection"
                && e["data"]["selection"] == "comment-baseline")
    );
    let pids: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|e| e["kind"] == "process_spawn" && e["participant"] == "A")
        .map(|e| e["data"]["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(
        pids.len(),
        3,
        "real worker restarts supplied the retained/lost request controls"
    );
    assert!(
        summary["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["selection"] == "comment-baseline")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn strict_io_properties_revised_use_actual_syncs_and_no_fallback() {
    strict_io_protocol_separates_legacy_control_from_bundle_result();
}

#[cfg(target_os = "linux")]
#[test]
fn bundle_migration_fixture_uses_the_real_reader_and_writer() {
    let temp = TempDirectory::new("ja-s1-migration-fixture");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-bundle-migration.json");
    let output = temp.path("out");
    assert_eq!(
        kinewright_agent::ja_harness::run(
            &fixture,
            &output,
            kinewright_agent::ja_harness::protocol::Mode::Revised
        )
        .unwrap(),
        0
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert!(
        summary["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["passed"] == true),
        "{summary:#}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn reply_retry_fixture_revised_has_one_durable_entry() {
    let temp = TempDirectory::new("ja-s1-retry-revised");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/reply-retry.json");
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "revised", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert!(
        summary["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let observation = |id| -> Value {
        serde_json::from_slice(
            &std::fs::read(output.join(format!("artifacts/observation-{id}.json"))).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(observation(6)["receipt"], observation(9)["receipt"]);
    assert_eq!(observation(9)["backend"], "linux-local-durable-receipts");
    assert_eq!(observation(10)["undo_count"], 1);
}
#[cfg(target_os = "linux")]
#[test]
fn comment_receipt_fixture_revised_reconciles_exact_original_result() {
    let temp = TempDirectory::new("ja-s1-comment-revised");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/comment-receipt.json");
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "revised", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert!(
        summary["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let observation = |id| -> Value {
        serde_json::from_slice(
            &std::fs::read(output.join(format!("artifacts/observation-{id}.json"))).unwrap(),
        )
        .unwrap()
    };
    let original = observation(10);
    assert_eq!(original["record_id"], observation(16)["record_id"]);
    assert_eq!(original["receipt"], observation(26)["receipt"]);
    assert_eq!(observation(22)["mutation_calls"], 0);
    assert_eq!(observation(32)["retired_unresolved"], true);
    assert_eq!(observation(32)["mutation_calls"], 0);
    let seal_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reports/ja1-s1/comment-baseline.json");
    // This is the recorded clean commit-4 baseline, not a claimed rerun at this HEAD.
    let seal: Value = serde_json::from_slice(&std::fs::read(seal_path).unwrap()).unwrap();
    assert_eq!(
        seal["source_head"],
        "f9f6089d87037e680e85f7551320739f69926ac0"
    );
    assert_eq!(seal["source_clean"], true);
    assert_eq!(
        seal["fixture_sha256"],
        kinewright_media::sha256_file(&fixture).unwrap()
    );
    assert_eq!(seal["literal_missing_result"]["actual"], false);
    assert_eq!(seal["literal_missing_result"]["expected"], true);
    assert_eq!(
        seal["binary_sha256"],
        "fe073fe00f3c57368d3a294b948b2cf8be8f77123f3d59803ea2c81e1d2a3ffc"
    );
    // CI compares the recorded identities. The sealed executable and raw run remain local evidence.
    let current: Value =
        serde_json::from_slice(&std::fs::read(output.join("run.json")).unwrap()).unwrap();
    assert_eq!(seal["inputs"], current["inputs"]);
    assert_eq!(seal["oracles"], current["oracles"]);
}

#[cfg(target_os = "linux")]
#[test]
fn edit_receipt_restart_reconciles_original_envelope() {
    let temp = TempDirectory::new("ja-s1-edit-restart");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ja/s1-edit-receipt-restart.json");
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "revised", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let observation = |id| -> Value {
        serde_json::from_slice(
            &std::fs::read(output.join(format!("artifacts/observation-{id}.json"))).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(observation(6)["receipt"], observation(14)["receipt"]);
    assert_eq!(observation(14)["original_epoch_is_current"], false);
    assert_eq!(observation(16)["undo_count"], 1);
    let events: Vec<Value> = std::fs::read_to_string(output.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let processes: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|e| e["kind"] == "process_spawn" && e["participant"] == "A")
        .map(|e| e["data"]["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(processes.len(), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn copy_fixture_revised_preserves_owned_graph_and_live_reader() {
    let temp = TempDirectory::new("ja-s1-native-copy");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ja/s1-copy-handoff.json");
    let output = temp.path("out");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-ja-baseline"))
        .args(["--mode", "revised", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["assertions"].as_array().unwrap().len(), 8);
    assert!(
        summary["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["passed"] == true),
        "{summary:#}"
    );
    let observed: Value = serde_json::from_slice(
        &std::fs::read(output.join("artifacts/observation-1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(observed["backend"], "linux-local-complete-bundle-copy");
    assert_eq!(
        observed["history_sha256"],
        observed["reopened_history_sha256"]
    );
    assert!(
        observed["handoff_overlap_reservation_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    let events: Vec<Value> = std::fs::read_to_string(output.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "process_spawn" && event["participant"] == "A")
    );
}
