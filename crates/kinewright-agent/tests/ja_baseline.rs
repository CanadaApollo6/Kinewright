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
