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

#[test]
fn cut_score_cli_scores_supported_studio_save_without_writing() {
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        owned_inputs::OwnedInputManifest,
    };
    fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn visit(
            root: &std::path::Path,
            path: &std::path::Path,
            out: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
        ) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        std::fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        visit(root, root, &mut out);
        out
    }
    let temp = TempDirectory::new("dq4-studio");
    let path = temp.path("studio.kinewright");
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    // A real small WAV is admitted only while creating the Studio fixture.
    let samples = [0_u8; 9_600];
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(9_636_u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(96_000_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(9_600_u32.to_le_bytes());
    bytes.extend(samples);
    let (asset, entry, pins) = bundle
        .store_mut()
        .unwrap()
        .import_media(
            &mut bytes.as_slice(),
            bytes.len() as u64,
            AssetId(1),
            &temp.path("absent.wav"),
            None,
        )
        .unwrap();
    let mut doc = document();
    doc.duration = asset.duration;
    doc.tracks[0].clips[0].source_range.end = asset.duration;
    doc.media_pool[0] = asset;
    let manifest = OwnedInputManifest {
        project_id: Some(bundle.project_id().to_owned()),
        session_id: None,
        owner_epoch: bundle.store().owner_epoch(),
        entries: vec![entry],
    };
    let inputs = bundle.admit_inputs(&manifest).unwrap().reference;
    let core = kinewright_core::Core::spawn(doc).unwrap();
    bundle.save(&core, &inputs).unwrap();
    drop(pins);
    drop(bundle);
    drop(core);
    assert!(BundleWriteAdmission::read(&path).unwrap().is_studio());
    let case_path = temp.path("case.json");
    let mut studio_case = case();
    studio_case.recording_sha256 = manifest.entries.into_iter().next().unwrap().object.sha256;
    studio_case.duration_micros = 100_000;
    studio_case.reference_kept = vec![SourceRange {
        start_micros: 0,
        end_micros: 100_000,
    }];
    std::fs::write(&case_path, serde_json::to_vec(&studio_case).unwrap()).unwrap();
    // Snapshot fixture bytes to prove that this real Studio read changes none.
    let before = snapshot(temp.root());
    let output = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--project")
        .arg(&path)
        .args(["--asset-id", "1", "--track-id", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "scored");
    assert_eq!(result["score"]["reference_equivalent"], true);
    assert_eq!(snapshot(temp.root()), before);
}

#[test]
fn cut_score_cli_uses_exit_one_for_usage_read_and_json_errors() {
    let temp = TempDirectory::new("dq4-errors");
    for args in [vec![], vec!["--unknown"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
    }
    let case_path = temp.path("case.json");
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--ranges")
        .arg(temp.path("ranges.json"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    std::fs::write(&case_path, b"{not json}").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_kinewright-cut-score"))
        .arg("--case")
        .arg(&case_path)
        .arg("--ranges")
        .arg(temp.path("ranges.json"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
}
