//! Unpaid, deterministic corpus contracts. Scripted choices do not measure model quality.
use kinewright_agent::orchestration::{
    orchestration_records::{BranchBinding, Role},
    policy::{TaskPolicy, director_scope},
};
use kinewright_core::{Command, Core, Document, Operation, Playback, TimeCode};
use kinewright_project::{load_document, write_project_document};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/te1")
}
fn base() -> BranchBinding {
    serde_json::from_value(json!({"branch":"1".repeat(32),"head":"2".repeat(32),"checkpoint":{"sha256":"3".repeat(64),"byte_length":1,"record_kind":"owned_evidence"},"document_hash":"4".repeat(64)})).unwrap()
}
fn verified_reads(
    case: &Value,
    reads: &[String],
    revision: u64,
    current: u64,
    preview: bool,
    child_complete: bool,
) -> bool {
    case["required_reads"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| reads.iter().any(|s| r.as_str() == Some(s)))
        && revision == current
        && (case["preview_before_commit"] != true || preview)
        && (case["reject_incomplete_child"] != true || child_complete)
}
fn snapshot(doc: &Document) -> Vec<u8> {
    serde_json::to_vec(doc).unwrap()
}
fn replay_history(path: &Path, core: &Core) {
    let expected = core.export_history().unwrap();
    std::fs::write(path, serde_json::to_vec(&expected).unwrap()).unwrap();
    let reopened = Core::spawn((*expected.baseline).clone()).unwrap();
    reopened
        .restore_history(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        expected,
        "exact reopened undo and redo history"
    );
}
fn verify_child_result(case: &Value, reads: &[String], original: &Document) {
    let child: Value = serde_json::from_slice(
        &std::fs::read(root().join(case["child_result"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    let child_core = Core::spawn(original.clone()).unwrap();
    let child_operations: Vec<Operation> =
        serde_json::from_value(child["operations"].clone()).unwrap();
    child_core
        .request(Command::DoBatch(child_operations))
        .unwrap();
    let document = load_document(&root().join(child["document"].as_str().unwrap()))
        .unwrap()
        .0;
    assert_eq!(*child_core.export_history().unwrap().document, document);
    assert_eq!(child["outcome"], "completed");
    assert_eq!(
        child["claimed_satisfied_notes"].as_array().unwrap().len(),
        2
    );
    assert_eq!(child["satisfied_notes"].as_array().unwrap().len(), 1);
    assert_eq!(child["unmet_notes"], json!(["correct-homophone"]));
    let complete =
        child["unmet_notes"].as_array().unwrap().is_empty() && child["proof"]["status"] == "passed";
    assert!(
        !verified_reads(case, reads, 7, 7, true, complete),
        "a plausible completed child with an unmet note and failed proof is rejected"
    );
}

fn verify_scope_and_decision(n: u64, case: &Value, original: &Document, operations: &[Operation]) {
    let mut scope = director_scope(original, base()).unwrap();
    let policy = TaskPolicy {
        role: Role::Director,
        scope: scope.clone(),
    };
    policy.validate_batch(original, operations).unwrap();
    let mut private = original.clone();
    for operation in operations {
        let role = if matches!(operation, Operation::SetTitleParam { .. }) {
            Role::Motion
        } else {
            Role::Editor
        };
        TaskPolicy {
            role,
            scope: scope.clone(),
        }
        .validate_batch(&private, std::slice::from_ref(operation))
        .unwrap();
        kinewright_core::apply_batch(&mut private, std::slice::from_ref(operation)).unwrap();
    }
    if n == 2 || n == 6 {
        scope.objects.remove("clip:6");
        let limited = TaskPolicy {
            role: Role::Editor,
            scope,
        };
        assert!(
            limited.validate_batch(original, operations).is_err(),
            "dependency outside authority requires replan"
        );
    }
    let reads = case["required_reads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(verified_reads(case, &reads, 7, 7, true, true));
    if !reads.is_empty() {
        assert!(
            !verified_reads(case, &[], 7, 7, true, true),
            "missing expansion/evidence refuses commitment"
        );
    }
    if n == 5 {
        verify_child_result(case, &reads, original);
    }
    if n == 6 {
        assert!(!verified_reads(case, &reads, 6, 7, true, true));
        assert!(!verified_reads(case, &reads, 7, 7, false, true));
    }
}

#[test]
fn te1_frozen_six_case_operations_documents_histories_and_evidence() {
    let dir = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te1-corpus",
    );
    let hashes: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(root().join("hashes.json")).unwrap()).unwrap();
    assert_eq!(hashes.len(), 18);
    for (name, hash) in hashes {
        assert_eq!(
            kinewright_media::sha256_file(&root().join(name)).unwrap(),
            hash
        );
    }
    let original = load_document(&root().join("talking-head.kinewright"))
        .unwrap()
        .0;
    for n in 1..=6 {
        let case: Value =
            serde_json::from_slice(&std::fs::read(root().join(format!("case-{n}.json"))).unwrap())
                .unwrap();
        let expected = load_document(&root().join(format!("case-{n}-oracle.kinewright")))
            .unwrap()
            .0;
        let operations: Vec<Operation> =
            serde_json::from_value(case["operations"].clone()).unwrap();
        verify_scope_and_decision(n, &case, &original, &operations);
        let manual = Core::spawn(original.clone()).unwrap();
        manual
            .request(Command::Do(Operation::MoveMarker {
                marker: kinewright_core::MarkerId(2),
                to: TimeCode(94),
            }))
            .unwrap();
        manual.request(Command::Undo).unwrap();
        let manual_before = manual.export_history().unwrap();
        assert!(!manual_before.cursor.redo.is_empty());
        let task = Core::spawn(original.clone()).unwrap();
        task.request(Command::DoBatch(operations.clone())).unwrap();
        let edited = task.export_history().unwrap();
        assert_eq!(*edited.document, expected, "case {n} document oracle");
        assert_eq!(
            edited.cursor.undo.last().unwrap().operations,
            operations,
            "case {n} exact operations"
        );
        for id in case["unchanged_clips"].as_array().unwrap() {
            let id = kinewright_core::ClipId(id.as_u64().unwrap());
            let find = |d: &Document| {
                d.tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .find(|c| c.id == id)
                    .cloned()
            };
            assert_eq!(find(&expected), find(&original));
        }
        task.request(Command::Undo).unwrap();
        assert_eq!(
            snapshot(&task.export_history().unwrap().document),
            snapshot(&original)
        );
        replay_history(&dir.path(&format!("task-{n}.json")), &task);
        task.request(Command::Redo).unwrap();
        assert_eq!(*task.export_history().unwrap().document, expected);
        assert_eq!(manual.export_history().unwrap(), manual_before);
        replay_history(&dir.path(&format!("manual-{n}.json")), &manual);
        manual.request(Command::Redo).unwrap();
        assert_eq!(
            manual.export_history().unwrap().document.markers[1].position,
            TimeCode(94)
        );
        manual.request(Command::Undo).unwrap();
        assert_eq!(*manual.export_history().unwrap().document, original);
        let path = dir.path(&format!("case-{n}.kinewright"));
        write_project_document(&expected, &path, None).unwrap();
        assert_eq!(load_document(&path).unwrap().0, expected);
    }
}
#[test]
fn te1_six_case_rendered_evidence_uses_production_preview() {
    let dir = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te1-frames",
    );
    let engine = kinewright_media::FfmpegMediaEngine::new_with_data_dir(dir.path("cache"))
        .unwrap()
        .into_legacy_fixture();
    for n in 1..=6 {
        let mut document = load_document(&root().join(format!("case-{n}-oracle.kinewright")))
            .unwrap()
            .0;
        for asset in &mut document.media_pool {
            asset.path = root().join(&asset.path);
        }
        engine.set_document(Arc::new(document));
        engine.seek(TimeCode(20));
        let frames = engine.frames();
        while frames.try_recv().is_ok() {}
        engine.request_frame(TimeCode(20));
        let stamp = engine.stamp();
        let deadline = Instant::now() + Duration::from_secs(30);
        let frame = loop {
            let frame = frames
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| {
                    panic!(
                        "case {n}: {e:?}, events {:?}",
                        engine.events().try_iter().collect::<Vec<_>>()
                    )
                });
            if frame.at == TimeCode(20) && frame.stamp == stamp {
                break frame;
            }
        };
        assert_eq!((frame.texture.width, frame.texture.height), (160, 90));
        let forehead = &frame.texture.rgba[(15 * 160 + 80) * 4..(15 * 160 + 80) * 4 + 4];
        assert!(
            forehead[0] > forehead[1] && forehead[1] > forehead[2] && forehead[0] > 80,
            "talking-head source must be rendered: {forehead:?}"
        );
    }
}
