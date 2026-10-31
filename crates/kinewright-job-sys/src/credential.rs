//! Single-file credentials. The directory containing the source is never granted.
use std::{
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
};

/// Open without following a final symlink and verify the kernel file identity.
///
/// # Errors
/// Refuses a nonregular, replaced, foreign-owned, or overly accessible file.
pub fn pin_credential_file(path: &Path, expected: Option<[u64; 2]>) -> io::Result<File> {
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !credential_metadata_valid(&metadata, rustix::process::getuid().as_raw())
        || expected.is_some_and(|identity| identity != [metadata.dev(), metadata.ino()])
    {
        return Err(io::Error::other(
            "credential file identity or permissions refused",
        ));
    }
    Ok(file)
}

fn credential_metadata_valid(metadata: &std::fs::Metadata, uid: u32) -> bool {
    metadata.is_file() && metadata.uid() == uid && metadata.mode() & 0o7777 & !0o600 == 0
}

/// Bind one pinned file into the private mount namespace before provider execution.
///
/// # Errors
/// Refuses an invalid source/target or a failed single-file mount.
pub fn mount_credential_file(source: &Path, target: &Path, identity: [u64; 2]) -> io::Result<()> {
    let source = pin_credential_file(source, Some(identity))?;
    let target_file = pin_credential_file(target, None)?;
    let target_path = format!("/proc/self/fd/{}", target_file.as_raw_fd());
    rustix::mount::mount_bind(format!("/proc/self/fd/{}", source.as_raw_fd()), target_path)?;
    // Check by pathname after mounting, not the descriptor to the old placeholder.
    pin_credential_file(target, Some(identity))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5_credential_refuses_symlink_owner_and_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("c5-credential-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("auth.json");
        std::fs::write(&path, b"fixture").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = pin_credential_file(&path, None).unwrap();
        let metadata = file.metadata().unwrap();
        assert!(!credential_metadata_valid(&metadata, metadata.uid() + 1));
        assert!(pin_credential_file(&path, Some([metadata.dev(), metadata.ino() + 1])).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(pin_credential_file(&link, None).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(pin_credential_file(&path, None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
