//! Inspect only authentication mode and API-key presence. Never retain token fields.
use crate::job_guardian::protocol::CredentialMount;
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) fn source_path() -> io::Result<PathBuf> {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))
        .ok_or_else(|| io::Error::other("Sign in: credential location unavailable"))?;
    Ok(fs::canonicalize(root)?.join("auth.json"))
}

pub(crate) fn inspect(path: &Path, identity: Option<[u64; 2]>) -> io::Result<[u64; 2]> {
    if std::env::var_os("OPENAI_API_KEY").is_some() {
        return Err(io::Error::other("Sign in: API-key override refused"));
    }
    crate::job_guardian::credentials::inspect_subscription(path, identity)
}

pub(crate) fn prepare_home(scratch: &Path) -> io::Result<PathBuf> {
    let home = scratch.join("codex-home");
    match fs::create_dir(&home) {
        Ok(()) => fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(&home)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(io::Error::other("private CODEX_HOME refused"));
    }
    let placeholder = home.join("auth.json");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&placeholder)
    {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            kinewright_job_sys::pin_credential_file(&placeholder, None)?;
        }
        Err(error) => return Err(error),
    }
    let config = b"forced_login_method = \"chatgpt\"\ncli_auth_credentials_store = \"file\"\n";
    let path = home.join("config.toml");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(config)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(&path)?.file_type().is_symlink() || fs::read(&path)? != config {
                return Err(io::Error::other("isolated config changed"));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(home)
}

pub(crate) fn verify_mount(mount: &CredentialMount) -> io::Result<()> {
    inspect(&mount.source, Some(mount.identity)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5_auth_mode_only_and_in_place_refresh() {
        let root = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c5-auth",
        );
        let path = root.path("auth.json");
        for data in [
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"fixture"}"#,
            r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"fixture"}"#,
            r#"{"tokens":{}}"#,
            "{torn",
        ] {
            fs::write(&path, data).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(inspect(&path, None).is_err(), "C5_API_OR_UNKNOWN_REFUSED");
        }
        fs::write(&path, br#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"access_token":"synthetic"}}"#).unwrap();
        let identity = inspect(&path, None).unwrap();
        fs::write(
            &path,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic refreshed"}}"#,
        )
        .unwrap();
        assert_eq!(
            inspect(&path, Some(identity)).unwrap(),
            identity,
            "C5_REFRESH_IDENTITY"
        );
        assert!(inspect(&path, Some([identity[0], identity[1] + 1])).is_err());
        prepare_home(root.root()).unwrap();
    }
}
