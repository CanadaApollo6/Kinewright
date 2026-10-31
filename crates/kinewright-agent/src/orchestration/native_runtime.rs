//! Read-only dependencies of an identity-checked executable and isolated model metadata.
use crate::orchestration::adapters::ExecutableIdentity;
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) fn runtime_tree(executable: &ExecutableIdentity) -> io::Result<Option<PathBuf>> {
    let path = std::fs::canonicalize(&executable.path)?;
    let metadata = std::fs::metadata(&path)?;
    if [metadata.dev(), metadata.ino()] != executable.file_identity {
        return Err(io::Error::other("runtime executable changed"));
    }
    if ["/usr", "/bin", "/lib", "/lib64"]
        .iter()
        .any(|root| path.starts_with(root))
    {
        return Ok(None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("runtime tree missing"))?;
    let tree = parent
        .ancestors()
        .take(6)
        .find(|directory| {
            directory
                .components()
                .any(|c| c.as_os_str() == "node_modules")
                && directory.join("package.json").is_file()
        })
        .unwrap_or(parent);
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::fs::canonicalize(home)?;
        if home.starts_with(tree) {
            return Err(io::Error::other("runtime grant includes home"));
        }
    }
    Ok(Some(tree.into()))
}

pub(crate) fn write_model_catalog(scratch: &Path, model: Option<&str>) -> io::Result<()> {
    let source = crate::drivers::codex_model_cache_path()
        .ok_or_else(|| io::Error::other("model metadata unavailable"))?;
    write_model_catalog_from(scratch, &source, model)
}
fn write_model_catalog_from(scratch: &Path, source: &Path, model: Option<&str>) -> io::Result<()> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(source)?
        .take((8 << 20) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 << 20 {
        return Err(io::Error::other("model metadata capacity"));
    }
    let mut catalog: serde_json::Value = serde_json::from_slice(&bytes)?;
    let models = catalog
        .as_array()
        .or_else(|| catalog.get("models").and_then(serde_json::Value::as_array));
    if !models.is_some_and(|models| {
        models
            .iter()
            .any(|entry| entry.get("slug").and_then(serde_json::Value::as_str) == model)
    }) {
        return Err(io::Error::other(
            "selected model absent from local metadata",
        ));
    }
    crate::drivers::force_direct_tool_mode(&mut catalog).map_err(io::Error::other)?;
    let models = if catalog.is_array() {
        catalog.as_array_mut()
    } else {
        catalog
            .get_mut("models")
            .and_then(serde_json::Value::as_array_mut)
    }
    .ok_or_else(|| io::Error::other("model metadata shape"))?;
    for model in models {
        let object = model
            .as_object_mut()
            .ok_or_else(|| io::Error::other("model metadata entry"))?;
        object.insert("shell_type".into(), serde_json::json!("disabled"));
        object.insert("apply_patch_tool_type".into(), serde_json::Value::Null);
        object.insert("experimental_supported_tools".into(), serde_json::json!([]));
    }
    let bytes = serde_json::to_vec(&catalog)?;
    let path = scratch.join("models-direct.json");
    if path.exists() {
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink()
            || std::fs::read(&path)? != bytes
        {
            return Err(io::Error::other("model metadata changed"));
        }
    } else {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5_user_local_tree_and_catalog() {
        let root = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c5-runtime",
        );
        let package = root.path("local/node_modules/@openai/codex");
        let bin = package.join("vendor/target/bin/codex");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(package.join("package.json"), b"{}").unwrap();
        std::fs::write(&bin, b"runtime").unwrap();
        let m = std::fs::metadata(&bin).unwrap();
        let executable = ExecutableIdentity {
            path: bin.clone(),
            file_identity: [m.dev(), m.ino()],
        };
        assert_eq!(runtime_tree(&executable).unwrap(), Some(package));
        let changed = ExecutableIdentity {
            file_identity: [m.dev(), m.ino() + 1],
            ..executable
        };
        assert!(
            runtime_tree(&changed).is_err(),
            "C5_CHANGED_RUNTIME_REFUSED"
        );
        let source = root.path("metadata.json");
        std::fs::write(
            &source,
            br#"{"models":[{"slug":"chosen","tool_mode":"search","supports_search_tool":true,"shell_type":"unified_exec","apply_patch_tool_type":"freeform","experimental_supported_tools":["clock","send_user_message_async"]}]}"#,
        )
        .unwrap();
        let scratch = root.path("scratch");
        std::fs::create_dir(&scratch).unwrap();
        write_model_catalog_from(&scratch, &source, Some("chosen")).unwrap();
        let catalog: serde_json::Value =
            serde_json::from_slice(&std::fs::read(scratch.join("models-direct.json")).unwrap())
                .unwrap();
        assert_eq!(catalog["models"][0]["tool_mode"], "direct");
        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
        assert_eq!(
            catalog["models"][0]["shell_type"], "disabled",
            "C5_NATIVE_MODEL_TOOLS_DISABLED"
        );
        assert!(catalog["models"][0]["apply_patch_tool_type"].is_null());
        assert_eq!(
            catalog["models"][0]["experimental_supported_tools"],
            serde_json::json!([])
        );
        assert!(write_model_catalog_from(&scratch, &source, Some("unknown")).is_err());
        write_model_catalog_from(&scratch, &source, Some("chosen")).unwrap();
    }
}
