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
