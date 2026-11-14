//! Private application data. The chosen root and its children reject links.
use kinewright_core::MediaError;
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

fn failure(reason: impl std::fmt::Display) -> MediaError {
    MediaError::DataDirectory(reason.to_string())
}
fn metadata(path: &Path) -> io::Result<fs::Metadata> {
    fs::symlink_metadata(path)
}
fn link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
// Check each component rather than create_dir_all's implicit link traversal.
// Application-owned components are created privately and checked for ownership.
pub(crate) fn ensure_directory(root: &Path, relative: &Path) -> Result<PathBuf, MediaError> {
    let mut path = ensure_root(root)?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(failure("data paths require normal components"));
        };
        path.push(name);
        create_private(&path)?;
    }
    Ok(path)
}
fn create_private(path: &Path) -> Result<(), MediaError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(failure(e)),
    }
    let meta = metadata(path).map_err(failure)?;
    if link(&meta) || !meta.is_dir() {
        return Err(failure(format!(
            "data directory must be a real directory: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        require_owner(path, meta.uid(), rustix::process::geteuid().as_raw())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(failure)?;
    }
    Ok(())
}
#[cfg(unix)]
fn require_owner(path: &Path, owner: u32, current: u32) -> Result<(), MediaError> {
    if owner != current {
        return Err(failure(format!(
            "data directory is owned by another user: {}",
            path.display()
        )));
    }
    Ok(())
}
pub(crate) fn ensure_root(root: impl AsRef<Path>) -> Result<PathBuf, MediaError> {
    let root = root.as_ref();
    if !root.is_absolute() || root.file_name().is_none() {
        return Err(failure(
            "data root must be an absolute application directory",
        ));
    }
    let parent = root
        .parent()
        .ok_or_else(|| failure("data root needs a parent"))?;
    // Ancestors belong to the machine/person. Follow their layout, leave existing
    // modes/owners alone, and make only missing parents private at creation.
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(parent).map_err(failure)?;
    let root = fs::canonicalize(parent).map_err(failure)?.join(
        root.file_name()
            .ok_or_else(|| failure("data root needs a name"))?,
    );
    create_private(&root)?;
    Ok(root)
}

fn resolve(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    local: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, MediaError> {
    #[cfg(target_os = "linux")]
    {
        let _ = local;
        if let Some(path) = xdg.map(Path::new).filter(|p| p.is_absolute()) {
            return Ok(path.join("kinewright"));
        }
        home.map(Path::new)
            .filter(|p| p.is_absolute())
            .map(|p| p.join(".local/share/kinewright"))
            .ok_or_else(|| failure("HOME is unavailable; set HOME or an absolute XDG_DATA_HOME"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (xdg, home);
        local
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("Kinewright"))
            .ok_or_else(|| failure("LOCALAPPDATA is unavailable"))
    }
}
/// Resolve and create the private persistent application data root.
/// # Errors
/// Returns a typed data-directory error if no absolute home is available.
pub fn default_data_dir() -> Result<PathBuf, MediaError> {
    let root = resolve(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("LOCALAPPDATA").as_deref(),
    )?;
    ensure_root(&root)
}

/// Create a private child directory without traversing a linked component.
/// # Errors
/// Returns a typed error for an unsafe root, child path or filesystem failure.
pub fn data_subdirectory(root: &Path, relative: &Path) -> Result<PathBuf, MediaError> {
    ensure_directory(root, relative)
}
/// A directory link for layout witnesses. Windows junctions need no elevation.
#[cfg(test)]
pub(crate) fn directory_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cannot create an unelevated junction: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDirectory;
    #[cfg(target_os = "linux")]
    #[test]
    fn dq1_data_location_requires_an_absolute_persistent_home() {
        use std::ffi::OsStr;
        let path = resolve(Some(OsStr::new("/isolated/xdg")), None, None).unwrap();
        assert_eq!(path, Path::new("/isolated/xdg/kinewright"));
        assert_eq!(
            resolve(
                Some(OsStr::new("relative")),
                Some(OsStr::new("/isolated/home")),
                None
            )
            .unwrap(),
            Path::new("/isolated/home/.local/share/kinewright")
        );
        assert!(matches!(
            resolve(None, None, None),
            Err(MediaError::DataDirectory(_))
        ));
        assert!(
            resolve(
                Some(OsStr::new("relative")),
                Some(OsStr::new("relative")),
                None
            )
            .is_err()
        );
    }
    #[cfg(unix)]
    #[test]
    fn dq1_private_root_and_children_are_0700_and_owned() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let temp = TempDirectory::new("dq1-data-mode");
        let root = temp.path("private");
        let child = ensure_directory(&root, Path::new("models/pinned/verified")).unwrap();
        for path in [root, child] {
            let meta = fs::symlink_metadata(path).unwrap();
            assert_eq!(meta.mode() & 0o777, 0o700);
            assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
        }
        fs::set_permissions(temp.root(), fs::Permissions::from_mode(0o755)).unwrap();
        ensure_root(temp.root()).unwrap();
        assert_eq!(fs::metadata(temp.root()).unwrap().mode() & 0o777, 0o700);
    }
    #[cfg(unix)]
    #[test]
    fn dq1_private_root_refuses_a_foreign_owner() {
        use std::os::unix::fs::MetadataExt as _;
        let temp = TempDirectory::new("dq1-foreign-owner");
        let owner = fs::symlink_metadata(temp.root()).unwrap().uid();
        assert!(matches!(
            require_owner(temp.root(), owner, owner.wrapping_add(1)),
            Err(MediaError::DataDirectory(_))
        ));
    }
    #[cfg(unix)]
    #[test]
    fn dq1_private_root_rejects_links_and_outside_components() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let temp = TempDirectory::new("dq1-data-links");
        let outside = TempDirectory::new("dq1-root-outside");
        let before = fs::metadata(outside.root()).unwrap().permissions().mode();
        symlink(outside.root(), temp.path("linked-root")).unwrap();
        assert!(matches!(
            ensure_root(temp.path("linked-root")),
            Err(MediaError::DataDirectory(_))
        ));
        assert_eq!(
            fs::metadata(outside.root()).unwrap().permissions().mode(),
            before
        );
        assert!(ensure_directory(temp.root(), Path::new("../outside")).is_err());
        assert!(ensure_directory(temp.root(), Path::new("/outside")).is_err());
    }

    #[test]
    fn dq1_data_root_allows_machine_links_but_refuses_root_and_child_links() {
        let temp = TempDirectory::new("dq1-layout");
        let real = temp.path("real");
        let parent = real.join("share");
        fs::create_dir_all(&parent).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        }
        crate::data_dir::directory_link(&real, &temp.path("homes"));
        let chosen = temp.path("homes/share/kinewright");
        let resolved = ensure_root(&chosen).unwrap();
        assert_eq!(
            resolved,
            fs::canonicalize(&parent).unwrap().join("kinewright"),
            "DQ1_LINKED_GRANDPARENT_ALLOWED"
        );
        let child = ensure_directory(&chosen, Path::new("captures")).unwrap();
        fs::write(child.join("proof"), b"private").unwrap();
        assert_eq!(
            fs::read(parent.join("kinewright/captures/proof")).unwrap(),
            b"private"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
                0o755,
                "existing parent modes are not ours"
            );
        }
        crate::data_dir::directory_link(&resolved, &temp.path("linked-root"));
        assert!(
            matches!(
                ensure_root(temp.path("linked-root")),
                Err(MediaError::DataDirectory(_))
            ),
            "DQ1_LINKED_ROOT_REFUSED"
        );
        let outside = TempDirectory::new("dq1-layout-outside");
        crate::data_dir::directory_link(outside.root(), &resolved.join("planted"));
        assert!(
            ensure_directory(&resolved, Path::new("planted/child")).is_err(),
            "DQ1_LINKED_CHILD_REFUSED"
        );
        assert_eq!(fs::read_dir(outside.root()).unwrap().count(), 0);
    }
}
