use kinewright_project::{
    bundle::{BundleError, LoadedBundle},
    bundle_recovery::without_api_probe,
    io_observer,
};
use std::{collections::BTreeMap, path::Path};

pub fn tree(path: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(path, path, &mut out);
    out
}
pub fn fixture(root: &Path, kind: &str, declaration: &str) -> std::path::PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kinewright-agent/tests/fixtures/ja/s1-v3-bundle");
    for (relative, bytes) in tree(&source) {
        let target = root.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    let path = root.join("studio.kinewright");
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let objects = root.join("studio.kinewright-assets/studio/objects");
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            objects.join(
                raw["studio"]["store_manifest_ref"]["sha256"]
                    .as_str()
                    .unwrap(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    if kind == "rescue-copy" {
        let mut record = serde_json::json!({
            "role":"rescue_copy_v1", "schema":1, "project_id":raw["studio"]["project_id"],
            "originating_session":"11111111111111111111111111111111", "full_capture_sha256":"a".repeat(64), "selected_capture_sha256":"b".repeat(64),
            "old_floor":0,"new_floor":0,"removed_undo":0,"redo_preserved":true,"gesture_preserved":true,
            "acknowledged_root":null,"acknowledged_absence":"unsaved source", "recovery_origin":null,"omissions":[],"attempts":[],
            "summary":{"count":0,"count_overflow":false,"digest":"","unresolved":false}
        });
        if declaration == "schema" {
            record["schema"] = serde_json::json!(2);
        }
        let bytes = serde_json::to_vec(&record).unwrap();
        let hash = kinewright_media::sha256_bytes(&bytes);
        std::fs::write(objects.join(&hash), &bytes).unwrap();
        let reference = serde_json::json!({"sha256":hash,"byte_length":bytes.len(),"record_kind":"owned_evidence"});
        manifest["objects"]
            .as_array_mut()
            .unwrap()
            .push(reference.clone());
        manifest["durable_pins"]
            .as_array_mut()
            .unwrap()
            .push(reference);
        manifest["dependencies"][&hash] = serde_json::json!([]);
        manifest["evidence_roles"][&hash] = serde_json::json!("rescue_copy_v1");
        if declaration == "role" {
            manifest["evidence_roles"][&hash] = serde_json::json!("unknown-rescue-role");
        }
        if declaration == "edge" {
            manifest["dependencies"][&hash] = serde_json::json!([{"sha256":"e".repeat(64),"byte_length":1,"record_kind":"document"}]);
        }
        manifest["required_kinds"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("owned_evidence"));
    } else if kind == "manual-switch" {
        raw["studio"]["generation"] = serde_json::json!(2);
        raw["studio"]["manual_switch_generation"] = serde_json::json!(1);
    }
    if kind != "ordinary" {
        manifest["required_apis"][kind] =
            serde_json::json!(if declaration == "mismatch" { 2 } else { 1 });
        if declaration == "missing" {
            manifest["required_apis"]
                .as_object_mut()
                .unwrap()
                .remove(kind);
        }
    }
    manifest["generation"] = raw["studio"]["generation"].clone();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let hash = kinewright_media::sha256_bytes(&bytes);
    std::fs::write(objects.join(&hash), &bytes).unwrap();
    raw["studio"]["store_manifest_ref"]["sha256"] = serde_json::json!(hash);
    raw["studio"]["store_manifest_ref"]["byte_length"] = serde_json::json!(bytes.len());
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    path
}
#[cfg(target_os = "linux")]
fn creation_observer_control() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        super::durable_test_root(),
        "c6-creation-observer",
    );
    let before = tree(temp.root());
    let (created, trace) = io_observer::observe(|| {
        kinewright_project::bundle_copy::SessionStore::create(
            temp.root(),
            kinewright_project::storage_budget::StorageLimits::default(),
        )
    });
    let created = created.unwrap();
    let directory = created
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(directory.is_dir());
    assert_eq!(
        trace
            .events
            .iter()
            .filter(|e| e.operation == "session_store_create")
            .count(),
        1
    );
    drop(created);
    assert!(!directory.exists());
    assert_eq!(tree(temp.root()), before);
}
pub fn matrix(mut caller: impl FnMut(&Path) -> Result<(), BundleError>) {
    use kinewright_media::test_support::TempDirectory;
    #[cfg(target_os = "linux")]
    creation_observer_control();
    for kind in ["ordinary", "rescue-copy", "manual-switch"] {
        for mode in [
            "supported",
            "missing",
            "mismatch",
            "masked",
            "schema",
            "role",
            "edge",
        ] {
            if kind != "rescue-copy" && matches!(mode, "schema" | "role" | "edge") {
                continue;
            }
            if kind == "ordinary" && mode != "supported" {
                continue;
            }
            let temp = TempDirectory::new_in(super::durable_test_root(), "c6-admission");
            let path = fixture(temp.root(), kind, mode);
            let before = tree(temp.root());
            let refused = kind != "ordinary" && mode != "supported";
            if !refused {
                LoadedBundle::read(&path).unwrap();
            }
            let mut run = || io_observer::observe(|| caller(&path));
            let (result, trace) = if mode == "masked" {
                without_api_probe(kind, run)
            } else {
                run()
            };
            assert!(!trace.overflowed);
            if refused {
                assert!(
                    matches!(
                        result,
                        Err(BundleError::Unsupported(_) | BundleError::Corrupt(_))
                    ),
                    "{kind}/{mode}: {result:?}"
                );
                assert!(
                    trace.events.is_empty(),
                    "session/sidecar/journal/store events: {:?}",
                    trace.events
                );
                assert_eq!(tree(temp.root()), before, "{kind}/{mode}");
            } else {
                #[cfg(target_os = "linux")]
                {
                    result.unwrap();
                    assert!(
                        trace
                            .events
                            .iter()
                            .any(|e| e.operation == "ja1_session_attach")
                    );
                    assert!(
                        trace
                            .events
                            .iter()
                            .any(|e| e.operation == "root_rename"
                                || e.operation == "root_parent_sync"),
                        "real save: {:?}",
                        trace.events
                    );
                    LoadedBundle::read(&path).unwrap();
                }
                #[cfg(not(target_os = "linux"))]
                {
                    assert!(
                        matches!(result, Err(BundleError::DurabilityUnsupported)),
                        "{result:?}"
                    );
                    assert!(trace.events.is_empty());
                    assert_eq!(tree(temp.root()), before);
                }
            }
        }
    }
}
