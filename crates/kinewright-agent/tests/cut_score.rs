#![cfg(feature = "eval-harness")]
use kinewright_agent::cut_score::{Candidate, Case, SourceRange};
use kinewright_core::{
    AssetId, Clip, ColorDescription, Document, MediaAsset, MediaKind, MediaSourceFingerprint,
    Rational, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use serde_json::Value;
use std::process::Command;

fn case() -> Case {
    Case {
        recording_sha256: "a".repeat(64),
        duration_micros: 20_000_000,
        request: "Keep the recording.".into(),
        words: vec![],
        reference_kept: vec![SourceRange {
            start_micros: 0,
            end_micros: 20_000_000,
        }],
        required_lines: None,
    }
}

fn document() -> Document {
    let mut d = Document::default();
    d.media_pool.push(MediaAsset {
        id: AssetId(1),
        path: "recording.wav".into(),
        name: "recording".into(),
        duration: TimeCode(600),
        fps: Rational::default(),
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(100),
        },
        color_description: ColorDescription::default(),
        assumed_from: None,
    });
    let clip:Clip=serde_json::from_value(serde_json::json!({"id":1,"asset":1,"source_range":{"start":0,"end":600},"timeline_start":0,"effects":[],"transition_in":null})).unwrap();
    d.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![clip],
    });
    d.duration = TimeCode(600);
    d
}

#[test]
fn cut_score_cli_reads_ranges_and_saved_project_without_media_or_provider() {
    let temp = TempDirectory::new("dq4-offline");
    let case_path = temp.path("case.json");
    std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
    let ranges_path = temp.path("candidate.json");
    std::fs::write(
        &ranges_path,
        serde_json::to_vec(&Candidate {
            recording_sha256: "a".repeat(64),
            kept: vec![SourceRange {
                start_micros: 0,
                end_micros: 20_000_000,
            }],
        })
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--ranges")
        .arg(&ranges_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ranges: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(ranges["status"], "scored");
    assert_eq!(ranges["score"]["reference_equivalent"], true);

    let project_path = temp.path("edit.kinewright");
    let d = document();
    kinewright_project::write_project_document(&d, &project_path, None).unwrap();
    let before = std::fs::read(&project_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--project")
        .arg(&project_path)
        .args(["--asset-id", "1", "--track-id", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(ranges, project);
    assert_eq!(std::fs::read(&project_path).unwrap(), before);
    assert!(!temp.path("recording.wav").exists()); // scoring uses identity and ranges; never opens the source
}

#[test]
fn cut_score_cli_typed_cannot_score_for_speed_and_wrong_recording() {
    let temp = TempDirectory::new("dq4-refusal");
    let case_path = temp.path("case.json");
    std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
    let project_path = temp.path("edit.kinewright");
    let mut d = document();
    d.tracks[0].clips[0].speed_percent = 200;
    d.duration = TimeCode(300);
    kinewright_project::write_project_document(&d, &project_path, None).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--project")
        .arg(&project_path)
        .args(["--asset-id", "1", "--track-id", "1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "cannot_score");
    assert_eq!(result["error"]["refusal"]["reason"], "speed");
    let ranges = temp.path("candidate.json");
    std::fs::write(
        &ranges,
        serde_json::to_vec(&Candidate {
            recording_sha256: "b".repeat(64),
            kept: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(case_path)
        .arg("--ranges")
        .arg(ranges)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["refusal"]["reason"], "recording_mismatch");
}
