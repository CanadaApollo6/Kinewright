thread_local! {
    static S4_INPUT: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
}

fn s4_read_input(
    root: &std::path::Path,
    path: &std::path::Path,
    hash: &str,
    key: &str,
) -> (Document, serde_json::Value) {
    let read = |path: &std::path::Path, hash: &str| {
        let bytes = std::fs::read(path).expect("frozen input exists");
        assert_eq!(
            crate::sha256::sha256_bytes(&bytes),
            hash,
            "frozen input hash"
        );
        bytes
    };
    let manifest: serde_json::Value = serde_json::from_slice(&read(path, hash)).unwrap();
    assert_eq!(manifest["key"], key, "frozen workload identity");
    let artifact = |value: &serde_json::Value| {
        read(
            &root.join(value["path"].as_str().unwrap()),
            value["sha256"].as_str().unwrap(),
        )
    };
    let document: Document = serde_json::from_slice(&artifact(&manifest["document"])).unwrap();
    let media: serde_json::Value = serde_json::from_slice(&artifact(&manifest["media"])).unwrap();
    let files = media["files"].as_array().unwrap();
    assert_eq!(
        document.media_pool.len(),
        files.len(),
        "frozen media completeness"
    );
    for (asset, file) in document.media_pool.iter().zip(files) {
        assert_eq!(
            asset.path,
            root.join(file["path"].as_str().unwrap()),
            "frozen media substitution"
        );
        drop(artifact(file));
    }
    let consumed = serde_json::json!({"workload": hash,
        "document": manifest["document"]["sha256"], "media": manifest["media"]["sha256"]});
    (document, consumed)
}

fn s4_input(key: &str) -> Option<Workload> {
    let path = std::env::var_os("PF1_FROZEN_INPUT")?;
    let (document, consumed) = s4_read_input(
        std::path::Path::new(&std::env::var("PF1_ROOT").expect("input root")),
        std::path::Path::new(&path),
        &std::env::var("PF1_FROZEN_SHA256").expect("frozen manifest hash"),
        key,
    );
    S4_INPUT.with_borrow_mut(|slot| *slot = Some(consumed));
    Some(Workload(document, Vec::new()))
}

#[test]
#[ignore = "PF1 frozen-input preparation; run before pinning and measurement"]
fn s4_prepare_inputs() {
    let root = std::path::PathBuf::from(std::env::var_os("PF1_ROOT").expect("evidence root"));
    let directory = root.join(std::env::var("PF1_PREPARE").expect("new input directory"));
    std::fs::create_dir(&directory).expect("exclusive preparation");
    let key = std::env::var("PF1_ONLY").expect("one workload");
    let Workload(mut document, _owned) = match key.as_str() {
        "seek_gop60" => perf_fixtures::seek_gop60(),
        "title_only" => perf_fixtures::title_only(),
        _ => play_workloads().pop().expect("registered workload").1,
    };
    let reference = |path: &std::path::Path| {
        serde_json::json!({
        "path": path.strip_prefix(&root).unwrap(),
        "sha256": crate::sha256::sha256_file(path).unwrap()})
    };
    let mut files = Vec::new();
    for (index, asset) in document.media_pool.iter_mut().enumerate() {
        let path = directory.join(format!(
            "{index}.{}",
            asset.path.extension().unwrap().to_str().unwrap()
        ));
        std::fs::copy(&asset.path, &path).unwrap();
        asset.path = path;
        files.push(reference(&asset.path));
    }
    let document_path = directory.join("document.json");
    std::fs::write(&document_path, serde_json::to_vec(&document).unwrap()).unwrap();
    let media_path = directory.join("media.json");
    std::fs::write(
        &media_path,
        serde_json::to_vec(&serde_json::json!({"files": files})).unwrap(),
    )
    .unwrap();
    if key == "seek_gop60" {
        let calibration = crate::s4_d13::calibrate(&document.media_pool[0].path);
        let path = directory.join("calibration.json");
        std::fs::write(&path, serde_json::to_vec(&calibration).unwrap()).unwrap();
        println!("S4 calibration {}", reference(&path));
    }
    let manifest = serde_json::json!({"key": key, "document": reference(&document_path), "media": reference(&media_path)});
    let path = directory.join("workload.json");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let (loaded, _) = s4_read_input(
        &root,
        &path,
        &crate::sha256::sha256_file(&path).unwrap(),
        &key,
    );
    assert_eq!(
        serde_json::to_value(loaded).unwrap(),
        serde_json::to_value(document).unwrap()
    );
    println!("S4 prepared {}", reference(&path));
}

#[test]
fn s4_frozen_input_rejects_changed_omitted_and_substituted_bytes() {
    let temp = TempDirectory::new("s4-input");
    let doc = temp.path("document.json");
    let media = temp.path("media.json");
    std::fs::write(
        &doc,
        serde_json::to_vec(&perf_fixtures::title_only().0).unwrap(),
    )
    .unwrap();
    std::fs::write(&media, br#"{"files":[]}"#).unwrap();
    let reference = |name: &str| serde_json::json!({"path": name, "sha256": crate::sha256::sha256_file(&temp.path(name)).unwrap()});
    let path = temp.path("workload.json");
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({"key":"title_only", "document":reference("document.json"), "media":reference("media.json")})).unwrap()).unwrap();
    let hash = crate::sha256::sha256_file(&path).unwrap();
    let load = |key| s4_read_input(temp.root(), &path, &hash, key);
    assert_eq!(load("title_only").1["workload"], hash);
    assert!(std::panic::catch_unwind(|| load("seek_gop60")).is_err());
    let mut changed = std::fs::read(&doc).unwrap();
    changed.push(b'\n');
    std::fs::write(&doc, changed).unwrap();
    assert!(std::panic::catch_unwind(|| load("title_only")).is_err());
    std::fs::remove_file(&doc).unwrap();
    assert!(std::panic::catch_unwind(|| load("title_only")).is_err());
}
