//! Read-only dependencies of an identity-checked executable and isolated model metadata.
use crate::orchestration::adapters::ExecutableIdentity;
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The installed CLI named `command` on PATH, through a mise shim when there
/// is one. It is measured whole while its size and mtime hold still; the
/// launch seals the bytes it measures again into a private image. No exact
/// build is required: the version floor and the fail-closed stream readers
/// hold a newer CLI to the same contract.
pub(crate) fn installed_executable(command: &str, product: &str) -> io::Result<ExecutableIdentity> {
    use std::io::Read;
    let path = resolve_without_running(command)
        .ok_or_else(|| io::Error::other(format!("Install {product}")))?;
    let mut file = std::fs::File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err(io::Error::other(format!(
            "Check again: {product} executable size refused"
        )));
    }
    kinewright_media::sha256_reader(&mut file.by_ref().take(512 * 1024 * 1024 + 1))?;
    let after = file.metadata()?;
    if metadata.len() != after.len() || metadata.modified()? != after.modified()? {
        return Err(io::Error::other(format!(
            "Check again: {product} executable changed while it was checked"
        )));
    }
    Ok(ExecutableIdentity {
        path,
        file_identity: [metadata.dev(), metadata.ino()],
    })
}

/// The first `command` on PATH that is not a mise shim. A shim is never run
/// to resolve it: when PATH holds only shims, the newest install mise keeps
/// (its `installs/<command>/latest` link) is used, read from the filesystem.
fn resolve_without_running(command: &str) -> Option<PathBuf> {
    let is_shim = |path: &Path| path.file_name().is_some_and(|name| name == "mise");
    let found = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(command))
        .filter(|candidate| candidate.is_file())
        .filter_map(|candidate| std::fs::canonicalize(candidate).ok())
        .find(|path| !is_shim(path));
    found.or_else(|| {
        let data = std::env::var_os("MISE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("mise")))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/mise"))
            })?;
        let latest = data.join("installs").join(command).join("latest");
        [latest.join("bin").join(command), latest.join(command)]
            .into_iter()
            .filter(|candidate| candidate.is_file())
            .filter_map(|candidate| std::fs::canonicalize(candidate).ok())
            .find(|path| !is_shim(path))
    })
}

pub(crate) fn runtime_tree(executable: &ExecutableIdentity) -> io::Result<Option<PathBuf>> {
    let root = crate::codex_credentials::source_path()?;
    runtime_tree_for(
        executable,
        root.parent()
            .ok_or_else(|| io::Error::other("credential root missing"))?,
    )
}
fn runtime_tree_for(
    executable: &ExecutableIdentity,
    credential_root: &Path,
) -> io::Result<Option<PathBuf>> {
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
    kinewright_job_sys::validate_credential_grant(tree, credential_root)?;
    Ok(Some(tree.into()))
}

/// Native-root aliases can leave /etc/ssl through symlinks. Grant only their
/// resolved regular files; never grant the directory containing a target.
pub(crate) fn network_files() -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for alias in [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    ] {
        if let Ok(path) = std::fs::canonicalize(alias)
            && path.is_file()
            && !files.contains(&path)
        {
            files.push(path);
        }
    }
    if files.is_empty() {
        return Err(io::Error::other("native CA bundle unavailable"));
    }
    if std::path::Path::new("/etc/gai.conf").is_file() {
        files.push("/etc/gai.conf".into());
    }
    Ok(files)
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
        let credentials = root.path("credentials");
        std::fs::create_dir(&credentials).unwrap();
        assert_eq!(
            runtime_tree_for(&executable, &credentials).unwrap(),
            Some(package.clone())
        );
        std::fs::create_dir(package.join("private-codex")).unwrap();
        assert!(
            runtime_tree_for(&executable, &package.join("private-codex")).is_err(),
            "C5_FIX1_RUNTIME_CREDENTIAL_OVERLAP"
        );
        assert!(
            runtime_tree_for(&executable, &package).is_err(),
            "C5_FIX1_EXECUTABLE_INSIDE_CREDENTIALS"
        );
        let changed = ExecutableIdentity {
            file_identity: [m.dev(), m.ino() + 1],
            ..executable
        };
        assert!(
            runtime_tree_for(&changed, &credentials).is_err(),
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
