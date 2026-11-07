//! Read-only dependencies of an identity-checked executable and isolated model metadata.
use crate::orchestration::adapters::ExecutableIdentity;
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The installed CLI named `command`: the person's explicit path, or the one
/// their shell runs from PATH, or, behind a mise shim, mise's only install.
/// It is measured whole while its size and mtime hold still; the launch seals
/// the bytes it measures again into a private image. No exact build is
/// required: the version floor and the fail-closed stream readers hold a newer
/// CLI to the same contract.
pub(crate) fn installed_executable(command: &str, product: &str) -> io::Result<ExecutableIdentity> {
    use std::io::Read;
    let path = resolve_without_running(&Resolution::from_environment(command, product))?;
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

pub(crate) struct LocalReadiness {
    pub executable: ExecutableIdentity,
    pub source: PathBuf,
    pub identity: [u64; 2],
    pub version: crate::orchestration::adapters::CliVersion,
}

pub(crate) fn executable_refusal(
    error: &io::Error,
) -> crate::orchestration::adapters::CertificationRefusal {
    crate::orchestration::adapters::CertificationRefusal::ExecutableResolution(error.to_string())
}

/// What one resolution reads: everything comes from the environment and the
/// filesystem, and nothing is executed.
struct Resolution<'a> {
    command: &'a str,
    product: &'a str,
    /// `KINEWRIGHT_<COMMAND>_EXECUTABLE`, the person's explicit choice.
    explicit: Option<std::ffi::OsString>,
    path: Option<std::ffi::OsString>,
    /// mise's data folder.
    data: Option<PathBuf>,
}

impl<'a> Resolution<'a> {
    fn from_environment(command: &'a str, product: &'a str) -> Self {
        let data = std::env::var_os("MISE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("mise")))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/mise"))
            });
        Self {
            command,
            product,
            explicit: std::env::var_os(explicit_variable(command)),
            path: std::env::var_os("PATH"),
            data,
        }
    }
}

fn explicit_variable(command: &str) -> String {
    format!("KINEWRIGHT_{}_EXECUTABLE", command.to_ascii_uppercase())
}

/// Never runs a shim and never picks silently. An explicit path wins. Else
/// the first `command` on PATH is the one the person's shell runs; when it is
/// a mise shim (or PATH has none), mise's selection would decide, and that is
/// read only when it needs no execution: exactly one real install folder. A
/// later PATH entry is never used in a shim's place, a `latest` or other link
/// is never followed, and an install whose executable leaves its own folder is
/// refused. Several installs refuse with how to name the executable.
fn resolve_without_running(resolution: &Resolution<'_>) -> io::Result<PathBuf> {
    let Resolution {
        command, product, ..
    } = *resolution;
    let variable = explicit_variable(command);
    let is_shim = |path: &Path| path.file_name().is_some_and(|name| name == "mise");
    if let Some(explicit) = &resolution.explicit {
        let explicit = Path::new(explicit);
        return std::fs::canonicalize(explicit)
            .ok()
            .filter(|path| explicit.is_absolute() && path.is_file() && !is_shim(path))
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{variable} must name the {product} executable by its full path"
                ))
            });
    }
    let first = resolution.path.as_ref().and_then(|path| {
        std::env::split_paths(path)
            .map(|directory| directory.join(command))
            .find(|candidate| candidate.is_file())
    });
    if let Some(path) = first.and_then(|first| std::fs::canonicalize(first).ok())
        && !is_shim(&path)
    {
        return Ok(path);
    }
    let missing = || io::Error::other(format!("Install {product}"));
    let installs = resolution
        .data
        .as_ref()
        .ok_or_else(missing)?
        .join("installs")
        .join(command);
    let mut found = Vec::new();
    match std::fs::read_dir(&installs) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let real = entry.file_type()?.is_dir();
                if real && !entry.file_name().to_string_lossy().starts_with('.') {
                    found.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let install = match found.as_slice() {
        [] => return Err(missing()),
        [install] => std::fs::canonicalize(install)?,
        _ => {
            return Err(io::Error::other(format!(
                "Several {product} installs are kept by mise, and Kinewright cannot tell which one \
                 mise selects without running it. Set {variable} to the full path of the {product} \
                 executable to use."
            )));
        }
    };
    [install.join("bin").join(command), install.join(command)]
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .find_map(|candidate| std::fs::canonicalize(candidate).ok())
        .filter(|path| path.starts_with(&install) && !is_shim(path))
        .ok_or_else(|| {
            io::Error::other(format!(
                "The {product} install kept by mise has no executable inside its own folder. Set \
                 {variable} to the full path of the {product} executable to use."
            ))
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
    /// Astra re-check: the resolver never runs mise and never picks silently.
    /// A shim decides through mise's only install, never through a later PATH
    /// entry; several installs refuse with the explicit path to set; a
    /// redirected `latest` link is never followed.
    #[test]
    fn s4b_c6_resolver_follows_the_shim_or_refuses() {
        use std::os::unix::fs::symlink;
        let root = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c6-resolver",
        );
        let file = |path: PathBuf| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
            std::fs::canonicalize(path).unwrap()
        };
        // A shim first on PATH (its canonical name is `mise`), then a decoy.
        let shims = root.path("shims");
        file(shims.join("mise"));
        symlink("mise", shims.join("claude")).unwrap();
        let decoy = file(root.path("decoy/claude"));
        let path = std::env::join_paths([&shims, &decoy.parent().unwrap().to_path_buf()]).unwrap();
        let resolve = |data: &Path, explicit: Option<&Path>| {
            resolve_without_running(&Resolution {
                command: "claude",
                product: "Claude Code",
                explicit: explicit.map(|path| path.as_os_str().to_owned()),
                path: Some(path.clone()),
                data: Some(data.to_path_buf()),
            })
        };

        // One real install: it, not the later decoy.
        let one = root.path("one");
        let selected = file(one.join("installs/claude/2.1.317/claude"));
        assert_eq!(
            resolve(&one, None).unwrap(),
            selected,
            "C6_RESOLVER_SHIM_INSTALL"
        );

        // A redirected `latest` link beside it is never followed, and an
        // install that is only a link is not an install.
        let outside = file(root.path("outside/claude"));
        symlink(
            outside.parent().unwrap(),
            one.join("installs/claude/latest"),
        )
        .unwrap();
        symlink(outside.parent().unwrap(), one.join("installs/claude/9.9.9")).unwrap();
        assert_eq!(
            resolve(&one, None).unwrap(),
            selected,
            "C6_RESOLVER_LATEST_REDIRECT"
        );
        let links = root.path("links");
        std::fs::create_dir_all(links.join("installs/claude")).unwrap();
        symlink(
            outside.parent().unwrap(),
            links.join("installs/claude/latest"),
        )
        .unwrap();
        let refused = resolve(&links, None).unwrap_err().to_string();
        assert_eq!(refused, "Install Claude Code", "C6_RESOLVER_LINK_ONLY");

        // The only install's executable may not leave its own folder.
        let escaping = root.path("escaping");
        std::fs::create_dir_all(escaping.join("installs/claude/2.1.317/bin")).unwrap();
        symlink(
            &outside,
            escaping.join("installs/claude/2.1.317/bin/claude"),
        )
        .unwrap();
        let refused = resolve(&escaping, None).unwrap_err().to_string();
        assert!(
            refused.contains("inside its own folder"),
            "C6_RESOLVER_INSTALL_ESCAPE {refused}"
        );

        // Several installs and no readable selection: refused, naming the
        // explicit path, never the decoy or the newest.
        let several = root.path("several");
        file(several.join("installs/claude/2.1.316/claude"));
        let newest = file(several.join("installs/claude/2.1.317/claude"));
        symlink("./2.1.317", several.join("installs/claude/latest")).unwrap();
        let refused = resolve(&several, None).unwrap_err().to_string();
        assert!(
            refused.contains("Several Claude Code installs")
                && refused.contains("KINEWRIGHT_CLAUDE_EXECUTABLE"),
            "C6_RESOLVER_SEVERAL_REFUSED {refused}"
        );

        // The explicit path wins, and must be a full path to a non-shim file.
        assert_eq!(
            resolve(&several, Some(&newest)).unwrap(),
            newest,
            "C6_RESOLVER_EXPLICIT"
        );
        for bad in [
            Path::new("claude"),
            shims.join("claude").as_path(),
            Path::new("/nonexistent/claude"),
        ] {
            let refused = resolve(&several, Some(bad)).unwrap_err().to_string();
            assert!(
                refused.contains("KINEWRIGHT_CLAUDE_EXECUTABLE must name"),
                "C6_RESOLVER_EXPLICIT_REFUSED {bad:?} {refused}"
            );
        }

        // A direct install first on PATH is what the shell runs.
        let direct = std::env::join_paths([decoy.parent().unwrap(), &shims]).unwrap();
        let found = resolve_without_running(&Resolution {
            command: "claude",
            product: "Claude Code",
            explicit: None,
            path: Some(direct),
            data: Some(several.clone()),
        });
        assert_eq!(found.unwrap(), decoy, "C6_RESOLVER_DIRECT_FIRST");
    }
    #[test]
    fn te15_b1_fix_two_installs_keep_the_repair_words() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "te15-two-installs",
        );
        for (command, product) in [("codex", "Codex"), ("claude", "Claude Code")] {
            for version in ["one", "two"] {
                std::fs::create_dir_all(temp.path(&format!("installs/{command}/{version}")))
                    .unwrap();
            }
            let error = resolve_without_running(&Resolution {
                command,
                product,
                explicit: None,
                path: None,
                data: Some(temp.root().into()),
            })
            .unwrap_err();
            let original = error.to_string();
            let refusal = executable_refusal(&error);
            let words = crate::writer_host::provider_readiness::provider_words(command, &refusal);
            assert!(
                words.contains(&original)
                    && words.contains(&format!(
                        "KINEWRIGHT_{}_EXECUTABLE",
                        command.to_ascii_uppercase()
                    ))
                    && words.contains("Settings"),
                "TE15_FIX_EXECUTABLE_REPAIR {words}"
            );
        }
    }
}
